//! The prompted policy, of any game: a player whose decisions a language
//! model makes. Every game's prompted player shares the shape of a
//! request, the `speak` and `wait` tools every request offers and how
//! their calls are read, how the probe settles the tool choice a model is
//! asked under, and the [`Player`] itself, all but what it does when it is
//! woken, which is its game's [`play`](Game::play).
//!
//! A player remembers what it is shown, what it hears and what it does, as
//! a [`Memory`] of each message stamped when it was received or sent, and
//! keeps with the history the story that tells it from its own point of
//! view: each memory adds its lines to the story as it is remembered, so
//! the story is never told over. Every player is announced every phase, so
//! a player knows which phase it is in, and it ignores a message tagged
//! with any other: it logs the receipt, as it does every message's, and
//! neither remembers the message nor wakes for it. A message of its own
//! phase wakes it as its game [says](Game::wakes). Each time it is woken,
//! it tells the model the story so far and where the game stands, with how
//! long the phase lasts, and carries out what the model answers. The
//! request fits the context window by estimate: when the story would not,
//! the oldest talk is forgotten for that request. The call is made in the
//! player's think loop, so the player keeps perceiving while the model
//! works.
//!
//! Wake-ups are debounced: the perceive loop remembers each message and
//! wakes the think loop only when no wake-up is already waiting, and the
//! think loop takes the history as it stands when it is woken, so a player
//! has at most one call in progress and one waiting, however fast the talk
//! comes. A reminder is numbered so that only the latest set counts, and
//! one for a phase that has ended is ignored. The calls are carried out in
//! the order the model made them. Speech is always sent, tagged with the
//! latest phase the player was announced. A choice is sent only if the
//! phase the call began in is still the latest and not closing; otherwise
//! the player logs it as [`Entry::Withheld`], and sets no reminder either.
//!
//! Told that the phase it is in is closing, a player makes no choice for
//! it: it starts no model call for it, sends the speech of the call it has
//! in flight and withholds the choices, and closes the phase once it has
//! no call in flight, at once if it had none. Neither the closing nor its
//! close is remembered, since the model is never asked while a phase
//! closes.
//!
//! A model that is not reached, or an answer that is not well formed, is a
//! failure: the player logs it as [`Entry::Failed`], carries nothing out,
//! and the game goes on. The player logs an [`Entry::Observation`] for each
//! message it receives and an [`Entry::Action`] for each message it sends,
//! numbered together from 1, with how many memories the model was shown
//! when it made the call that produced the message and how many lines of
//! talk that call forgot. Every request makes the model call a tool, unless
//! the probe finds the model refuses that, and then every request asks it
//! to call one, and an answer in prose is a failure like any other.

use crate::game::{Actor, Ending, Entry, Game, Message, Seat, act, close, observe};
use crate::model::spec::Probed;
use crate::model::{self, Request, Response, Tool, ToolChoice};
use crate::seat::PlayerId;
use anyhow::{Context as _, anyhow, bail};
use async_trait::async_trait;
use free_agent::{ActorId, Behavior, Builder, Context, Event, Think, ThinkBuilder};
use humantime::format_duration;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What every request ends with, under either tool choice: that a tool
/// call is the one answer that counts.
pub(crate) const ANSWER: &str = "Answer by calling one of the tools.";

/// The system prompt the probe tells the model.
pub(crate) const PROBE_PROMPT: &str = "You are a player in a game, about to be asked to play.";

/// The name of the tool a player speaks with.
pub(crate) const SPEAK: &str = "speak";

/// The name of the tool a player waits with.
pub(crate) const WAIT: &str = "wait";

/// A request of a player's shape to `model_id` under `tool_choice`:
/// `prompt` as the system message, `asked` and then [`ANSWER`] as the user
/// message, and `tools` to answer with. This is the one way a player
/// builds a request and the one way the probe does, so they agree on what
/// is asked.
pub(crate) fn request(
    model_id: &str,
    tool_choice: ToolChoice,
    prompt: &str,
    mut asked: String,
    tools: Vec<Tool>,
) -> Request {
    asked.push(' ');
    asked.push_str(ANSWER);
    Request::offering(
        model_id,
        vec![model::Message::system(prompt), model::Message::user(asked)],
        tools,
        tool_choice,
    )
}

/// The tool a player speaks with: `speak`, described to the model by
/// `description`, taking `text`, which is not empty, nor blank once
/// trimmed.
pub(crate) fn speak(description: &str) -> Tool {
    Tool::new(
        SPEAK,
        description,
        json!({
            "type": "object",
            "properties": {"text": {"type": "string", "minLength": 1}},
            "required": ["text"],
            "additionalProperties": false,
        }),
    )
}

/// The tool a player waits with: `wait`, taking `seconds`, a whole number
/// from 1 to the phase's `limit` in whole seconds.
pub(crate) fn wait(limit: Duration) -> Tool {
    Tool::new(
        WAIT,
        "Say nothing now, and be woken after this many seconds unless something wakes you sooner.",
        json!({
            "type": "object",
            "properties": {"seconds": {"type": "integer", "minimum": 1, "maximum": limit.as_secs()}},
            "required": ["seconds"],
            "additionalProperties": false,
        }),
    )
}

/// The arguments of a [`speak`] call.
#[derive(Deserialize)]
struct Speech {
    text: String,
}

/// The arguments of a [`wait`] call.
#[derive(Deserialize)]
struct Waiting {
    seconds: u64,
}

/// What `call`, a call to [`speak`], says. Arguments that do not parse,
/// or text that is empty once trimmed, is an error.
pub(crate) fn said(call: &model::Call) -> anyhow::Result<String> {
    let Speech { text } = arguments(call)?;
    if text.trim().is_empty() {
        bail!("the model's {SPEAK} call says nothing");
    }
    Ok(text)
}

/// How long `call`, a call to [`wait`] in a phase that lasts `limit`,
/// waits. Arguments that do not parse, or seconds not between 1 and
/// `limit` in whole seconds, is an error.
pub(crate) fn waited(call: &model::Call, limit: Duration) -> anyhow::Result<Duration> {
    let Waiting { seconds } = arguments(call)?;
    if !(1..=limit.as_secs()).contains(&seconds) {
        bail!(
            "the model asked to wait {seconds}s, which is not between 1s and {}",
            format_duration(limit)
        );
    }
    Ok(Duration::from_secs(seconds))
}

/// The arguments of `call`, parsed. A provider may send no arguments as no
/// text at all, which parses as `{}`. Arguments that do not parse are an
/// error naming the tool and showing them.
pub(crate) fn arguments<T: DeserializeOwned>(call: &model::Call) -> anyhow::Result<T> {
    let text = match call.arguments.trim() {
        "" => "{}",
        _ => &call.arguments,
    };
    serde_json::from_str(text).with_context(|| {
        format!(
            "the arguments of the model's {} call: {}",
            call.name, call.arguments
        )
    })
}

/// The tool choice the probe's first request, which `ask` makes under the
/// tool choice it is given, is answered under, and how `ask` came out.
/// The request is first asked under `from`. If that requires a tool call
/// and the provider refuses it as bad, it is asked again allowing one, and
/// both refusals are given if that is refused too.
pub(crate) async fn first_answered(
    from: ToolChoice,
    ask: impl AsyncFn(ToolChoice) -> anyhow::Result<()>,
) -> (ToolChoice, anyhow::Result<()>) {
    match ask(from).await {
        Err(required) if from == ToolChoice::Required && model::refused(&required) => {
            let answered = ask(ToolChoice::Auto).await.map_err(|auto| {
                if model::concerns_the_model(&auto) {
                    anyhow!("made to call a tool: {required:#}\nasked to call one: {auto:#}")
                } else {
                    auto
                }
            });
            (ToolChoice::Auto, answered)
        }
        answered => (from, answered),
    }
}

/// What the probe of `model_id` for `G` found, its requests asked under
/// `tool_choice`: that the model played, if none `failed`; that it did not
/// play `G`, naming the request it did not answer, as `what` the game words
/// it, if the failure concerns the model; and otherwise an error saying the
/// model could not be asked, with the cause beneath it.
pub(crate) fn probed<G: Game>(
    model_id: &str,
    tool_choice: ToolChoice,
    failed: Option<(String, anyhow::Error)>,
) -> anyhow::Result<Probed> {
    match failed {
        None => Ok(Probed::Played(tool_choice)),
        Some((what, error)) if model::concerns_the_model(&error) => Ok(Probed::DidNotPlay(
            error
                .context(format!("did not answer {what}"))
                .context(format!("{model_id} did not play {}", G::NAME)),
        )),
        Some((_, error)) => Err(error.context(format!("{model_id} could not be asked to play"))),
    }
}

/// What a player remembers: a message it received from another actor in
/// the phase it was in, or one it sent. Reminders to itself, messages from
/// another phase, what it withheld, and a phase's closing and its close,
/// are not remembered.
#[derive(Debug, Clone, PartialEq)]
pub enum Memory<G: Game> {
    /// The player received `message`, whose [`Message::sender`] sent it.
    Observed {
        /// What was sent.
        message: Message<G>,
    },
    /// The player sent `message`.
    Acted {
        /// What it sent.
        message: Message<G>,
    },
}

/// A player's account of its own game, told a memory at a time.
pub trait Account<G: Game> {
    /// Add what `event` adds to the account.
    fn recall(&mut self, event: &Event<Memory<G>>);

    /// The lines told within `budget` tokens by estimate, the oldest talk
    /// forgotten first to fit, and how many lines of talk were left out.
    /// The account itself keeps every line.
    fn told_within(&self, budget: u64) -> (Vec<&str>, usize);
}

/// What a call has a player send, as the message it makes.
pub(crate) trait Sending<G: Game> {
    /// Whether this is speech, which is always sent. All else is a choice,
    /// which is sent only in the phase it was made for while it is open.
    fn speaks(&self) -> bool;

    /// The message `from` sends for this in the phase numbered `seq`,
    /// numbered `n`.
    fn message(self, seq: u64, from: &PlayerId, n: u64) -> Message<G>;
}

/// What a player's two loops share: what it has been shown and what it has
/// done, with the story that tells it, the phase it is in, whether a
/// wake-up is waiting, how many reminders it has set, what it is told, and
/// whom it asks.
pub(crate) struct Mind<G: Game> {
    /// The history and the story told from it. Never locked across an
    /// await.
    pub(crate) remembered: Mutex<Remembered<G>>,
    /// The number of the last message the player sent to others.
    n: AtomicU64,
    /// The latest phase the player was announced: the phase it is in, so far
    /// as it knows. Never locked across an await.
    announced: Mutex<Option<Announced<G>>>,
    /// Whether a [`Message::Wake`] is waiting for the think loop. The
    /// perceive loop hands the think loop one only when none is waiting,
    /// and the think loop clears this before it takes the history, so
    /// however fast messages come the player has at most one call in
    /// progress and one waiting, and nothing it heard is left out of both.
    waking: AtomicBool,
    /// How many reminders the player has set, which numbers them so that
    /// only the latest counts.
    reminders: AtomicU64,
    /// The model call in flight and the phase closing, if any. Never locked
    /// across an await.
    flight: Mutex<Flight>,
    /// Its system prompt, rendered for its seat and role.
    prompt: String,
    /// The model's id, as each request names it.
    model_id: String,
    /// Whether each request makes the model call a tool.
    tool_choice: ToolChoice,
    /// The model's context window, in tokens, which each request fits.
    context_window: u64,
    /// The model asked each time the player is woken.
    model: Arc<dyn model::Model>,
    /// How long each phase of its game waits.
    pub(crate) limits: G::Limits,
}

/// A player's history and the story that tells it, kept together so that
/// the story is always the narration of exactly the history, and a request
/// joins the story's lines instead of narrating every memory again.
pub(crate) struct Remembered<G: Game> {
    /// The history, in order: each message received and each message sent,
    /// stamped when the player remembered it.
    pub(crate) history: Vec<Event<Memory<G>>>,
    /// The story told from the history.
    pub(crate) story: G::Story,
}

/// What the model was shown for a call: how many memories the story was
/// told from, and how many lines of talk it forgot to fit the context
/// window, if any.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Shown {
    pub(crate) seen: usize,
    pub(crate) forgot: Option<usize>,
}

/// What the think loop is doing about the phases, as the perceive loop
/// needs to know it to close one.
#[derive(Default)]
struct Flight {
    /// The phase the think loop has a model call in flight for, set just
    /// before the call and cleared once its answer or failure is carried
    /// out.
    calling: Option<u64>,
    /// The latest phase the player was told is closing. A call in flight
    /// for it owes the environment the player's close of it, which the
    /// think loop sends once the call is done.
    closing: Option<u64>,
}

/// A phase as a player was announced it: its number, what the player may
/// see of it, and how long it lasts.
#[derive(Clone)]
struct Announced<G: Game> {
    seq: u64,
    view: G::View,
    limit: Duration,
}

impl<G: Game> Mind<G> {
    /// The mind of `me` with nothing in its history yet, told `prompt`, in a
    /// game under `limits`, asking `model` as the model `model_id` under
    /// `tool_choice`, within `context_window` tokens.
    pub(crate) fn new(
        me: PlayerId,
        prompt: String,
        model_id: String,
        tool_choice: ToolChoice,
        context_window: u64,
        model: Arc<dyn model::Model>,
        limits: G::Limits,
    ) -> Self {
        Self {
            remembered: Mutex::new(Remembered {
                history: Vec::new(),
                story: G::story(me, &limits),
            }),
            n: AtomicU64::new(0),
            announced: Mutex::new(None),
            waking: AtomicBool::new(false),
            reminders: AtomicU64::new(0),
            flight: Mutex::new(Flight::default()),
            prompt,
            model_id,
            tool_choice,
            context_window,
            model,
            limits,
        }
    }

    /// Add `event` to the end of the history, and what it adds to the
    /// account to the end of the story, together. A message is remembered
    /// when it is received or sent, so a message sent in the moment
    /// between the check that its phase is still on and the send comes
    /// after the next phase's announcement, and is told in that phase.
    pub(crate) fn remember(&self, event: Event<Memory<G>>) {
        let mut remembered = self.remembered.lock().unwrap();
        remembered.story.recall(&event);
        remembered.history.push(event);
    }

    /// The story so far within `budget` tokens, its lines joined, and
    /// what it was told from: how many memories, and how many lines of talk
    /// it forgot to fit.
    pub(crate) fn story(&self, budget: u64) -> (String, Shown) {
        let remembered = self.remembered.lock().unwrap();
        let (lines, forgot) = remembered.story.told_within(budget);
        let shown = Shown {
            seen: remembered.history.len(),
            forgot: (forgot > 0).then_some(forgot),
        };
        (lines.join("\n"), shown)
    }

    /// Whether the player has sent a message that `sent` holds of.
    pub(crate) fn acted(&self, sent: impl Fn(&Message<G>) -> bool) -> bool {
        let remembered = self.remembered.lock().unwrap();
        remembered.history.iter().any(|event| match &event.payload {
            Memory::Acted { message } => sent(message),
            Memory::Observed { .. } => false,
        })
    }

    /// The next number for a message to others: one up from the last.
    fn number(&self) -> u64 {
        self.n.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Take in the announcement of the phase numbered `seq`, showing `view`
    /// and lasting `limit`: the phase the player is in from now on.
    fn announce(&self, seq: u64, view: &G::View, limit: Duration) {
        *self.announced.lock().unwrap() = Some(Announced {
            seq,
            view: view.clone(),
            limit,
        });
    }

    /// The latest phase the player was announced, if any.
    fn announced(&self) -> Option<Announced<G>> {
        self.announced.lock().unwrap().clone()
    }

    /// The number of the latest phase the player was announced, and 0
    /// before any.
    fn seq(&self) -> u64 {
        self.announced
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |announced| announced.seq)
    }

    /// Whether `message` counts for `me`, tagged with the phase it is in,
    /// and if it does, whether it [wakes](Game::wakes) it.
    fn counts(&self, me: &PlayerId, message: &Message<G>) -> Option<bool> {
        let announced = self.announced.lock().unwrap();
        let announced = announced
            .as_ref()
            .filter(|announced| message.seq() == Some(announced.seq))?;
        Some(G::wakes(me, message, &announced.view))
    }

    /// Mark a wake-up waiting, and say whether none was before: whether
    /// the think loop has to be handed one.
    fn waking(&self) -> bool {
        !self.waking.swap(true, Ordering::Relaxed)
    }

    /// Take the wake-up waiting, so that the next message heard sets
    /// another.
    fn woken(&self) {
        self.waking.store(false, Ordering::Relaxed);
    }

    /// The number of a new reminder: one up from the last, which no longer
    /// counts.
    fn remind(&self) -> u64 {
        self.reminders.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Whether the phase numbered `seq` is the one the player is in: the
    /// latest it was announced.
    fn in_phase(&self, seq: u64) -> bool {
        seq == self.seq()
    }

    /// Whether the reminder numbered `count` for the phase numbered `seq`
    /// counts: it is the latest set, and the phase is the one the player is
    /// in.
    fn reminder_counts(&self, seq: u64, count: u64) -> bool {
        count == self.reminders.load(Ordering::Relaxed) && self.in_phase(seq)
    }

    /// Take in that the phase numbered `seq` is closing, and say whether
    /// the player closes it now: whether it has no call in flight for it.
    /// If it has one, the close is owed once the call is done.
    fn close(&self, seq: u64) -> bool {
        let mut flight = self.flight.lock().unwrap();
        flight.closing = Some(seq);
        flight.calling != Some(seq)
    }

    /// Whether the phase numbered `seq` is closing.
    fn closing(&self, seq: u64) -> bool {
        self.flight.lock().unwrap().closing == Some(seq)
    }

    /// Mark a call in flight for the phase numbered `seq`, unless the phase
    /// is closing. Returns whether the call may be made.
    fn take_off(&self, seq: u64) -> bool {
        let mut flight = self.flight.lock().unwrap();
        if flight.closing == Some(seq) {
            return false;
        }
        flight.calling = Some(seq);
        true
    }

    /// Mark the call in flight done. Returns the phase whose close it owed,
    /// if it owed one.
    fn land(&self) -> Option<u64> {
        let mut flight = self.flight.lock().unwrap();
        let landed = flight.calling.take();
        landed.filter(|seq| flight.closing == Some(*seq))
    }
}

/// A player whose decisions are made by a language model. Its perceive
/// loop and its think loop are each a `Player` with a context of its own
/// over one shared mind: the perceive loop remembers each message it is
/// sent and wakes the think loop, which tells the model the history as a
/// story and carries out what the model answers.
/// [`Actor::Prompted`] holds the perceive loop's, built by
/// [`Player::init`].
pub struct Player<G: Game> {
    pub(crate) context: Context<Message<G>, Entry<G>>,
    pub(crate) mind: Arc<Mind<G>>,
}

impl<G: Game> Player<G> {
    /// Builds a player's two loops once the episode has made their
    /// contexts: the perceive loop as the actor, and the think loop. The
    /// player `me` is told `prompt`, in a game under `limits`, and asks
    /// `model` as the model `model_id` under `tool_choice`, within
    /// `context_window` tokens.
    pub fn init(
        me: PlayerId,
        prompt: String,
        model_id: String,
        tool_choice: ToolChoice,
        context_window: u64,
        model: Arc<dyn model::Model>,
        limits: G::Limits,
    ) -> Seat<G> {
        let mind = Arc::new(Mind::new(
            me,
            prompt,
            model_id,
            tool_choice,
            context_window,
            model,
            limits,
        ));
        let shared = Arc::clone(&mind);
        let behavior: Builder<Actor<G>> =
            Box::new(move |context| Actor::Prompted(Player { context, mind }));
        let think: ThinkBuilder<Message<G>, Entry<G>> = Box::new(move |context| {
            Box::new(Player {
                context,
                mind: shared,
            })
        });
        (behavior, Some(think))
    }

    /// Add `memory` to the history, stamped now, and what it tells to the
    /// story.
    fn remember(&self, memory: Memory<G>) {
        self.mind.remember(Event::now(self.context.episode, memory));
    }

    /// Wake the think loop, unless a wake-up is already waiting for it.
    fn wake(&self) -> anyhow::Result<()> {
        if self.mind.waking() {
            self.context.think(Message::Wake)?;
        }
        Ok(())
    }

    /// The request for a decision where the game stands as `situation`
    /// says, with `tools` to answer with: the system prompt, then the story
    /// so far, a blank line, and the situation. The story fits what is left
    /// of the context window once the reply, the system prompt, the tools
    /// and the situation are counted, by estimate, forgetting talk if it
    /// has to. With the request, what the story was told from.
    pub(crate) fn request(&self, situation: &str, tools: Vec<Tool>) -> (Request, Shown) {
        let situation = format!("\n\n{situation}");
        let mind = &self.mind;
        let asking = |asked: String, tools| {
            request(&mind.model_id, mind.tool_choice, &mind.prompt, asked, tools)
        };
        let taken = model::estimated(&asking(situation.clone(), tools.clone()));
        let (story, shown) = mind.story(mind.context_window.saturating_sub(taken));
        (asking(story + &situation, tools), shown)
    }

    /// Woken, decide: take the wake-up, and play the phase the player is
    /// in as its game plays it, unless the phase is closing. If that fails,
    /// log the failure with the phase and what went wrong, causes included,
    /// which is all there is to do about it: the player has not acted, and
    /// the game goes on. The failure is not remembered, so the model is
    /// never told of it. Then close the phase if it began closing in the
    /// meantime. Being woken before any phase was announced is an error.
    async fn decide(&self) -> anyhow::Result<()> {
        self.mind.woken();
        let Announced { seq, view, limit } = self
            .mind
            .announced()
            .context("woken before any phase was announced")?;
        if !self.mind.take_off(seq) {
            return Ok(());
        }
        if let Err(error) = G::play(self, seq, &view, limit).await {
            self.context.log(Entry::Failed {
                player: self.context.id.clone(),
                seq,
                error: format!("{error:#}"),
            });
        }
        match self.mind.land() {
            Some(seq) => self.closed(seq),
            None => Ok(()),
        }
    }

    /// Tell the environment the player has closed the phase numbered
    /// `seq`.
    fn closed(&self, seq: u64) -> anyhow::Result<()> {
        close(&self.context, seq, self.mind.number())
    }

    /// Ask the model `request` in the phase numbered `seq`, logging the
    /// call as it goes out and as it comes back, and then the usage its
    /// provider reports for the answer, if it reports one, before the
    /// answer is judged.
    pub(crate) async fn complete(&self, seq: u64, request: &Request) -> anyhow::Result<Response> {
        let player = &self.context.id;
        self.context.log(Entry::Thinking {
            player: player.clone(),
            seq,
        });
        let answer = self.mind.model.complete(request).await;
        let ending = match &answer {
            Ok(_) => Ending::Answered,
            Err(error) => Ending::Failed(format!("{error:#}")),
        };
        self.context.log(Entry::Thought {
            player: player.clone(),
            seq,
            ending,
        });
        let mut response = answer?;
        if let Some(usage) = response.usage.take() {
            self.context.log(Entry::Usage {
                player: player.clone(),
                seq,
                usage,
            });
        }
        Ok(response)
    }

    /// Carry out `sends` and `wait`, what a response made from what it was
    /// `shown` asked for in the phase numbered `seq`: send each in order,
    /// numbered, to whom the rules of the game send it in the latest phase
    /// the player was announced, and then set a reminder to be woken after
    /// `wait`. Speech is always sent, tagged with the latest phase. A
    /// choice is sent only while the call's phase is the latest and not
    /// closing, and so is a reminder set; otherwise the choice is logged as
    /// withheld, unnumbered, and no reminder is set.
    pub(crate) fn carry_out(
        &self,
        seq: u64,
        sends: Vec<impl Sending<G>>,
        wait: Option<Duration>,
        shown: Shown,
    ) -> anyhow::Result<()> {
        let me = &self.context.id;
        let latest = self
            .mind
            .announced()
            .context("answered before any phase was announced")?;
        let open = latest.seq == seq && !self.mind.closing(seq);
        for sending in sends {
            if sending.speaks() || open {
                let message = sending.message(latest.seq, me, self.mind.number());
                let to = G::receivers(&message, me, &latest.view);
                self.send(message, to, shown)?;
            } else {
                self.context.log(Entry::Withheld {
                    from: me.clone(),
                    message: sending.message(seq, me, 0),
                    seen: Some(shown.seen),
                    forgot: shown.forgot,
                });
            }
        }
        match wait {
            Some(after) if open => self.remind(seq, after),
            _ => Ok(()),
        }
    }

    /// Remember sending `message` and send it to everyone in `to`, logged
    /// as the player's action with what the model was `shown` when it made
    /// the call that produced it.
    fn send(&self, message: Message<G>, to: HashSet<ActorId>, shown: Shown) -> anyhow::Result<()> {
        self.remember(Memory::Acted {
            message: message.clone(),
        });
        act(&self.context, message, to, Some(shown.seen), shown.forgot)
    }

    /// Set a reminder to be woken `after` from now in the phase numbered
    /// `seq`, numbered so that any reminder set before it no longer counts.
    fn remind(&self, seq: u64, after: Duration) -> anyhow::Result<()> {
        let count = self.mind.remind();
        self.context
            .think_after(after, Message::Remind { seq, count })
    }
}

#[async_trait]
impl<G: Game> Behavior for Player<G> {
    type Message = Message<G>;
    type Log = Entry<G>;

    fn context(&self) -> &Context<Message<G>, Entry<G>> {
        &self.context
    }

    /// Log a message from another actor, which is any numbered one, as
    /// observed in the latest phase announced. An announcement is of a new
    /// phase. It, and anything else tagged with the phase it announced, is
    /// remembered, and wakes the think loop if the game says it
    /// [wakes](Game::wakes) the player. One tagged with another phase is
    /// observed and nothing more, neither remembered nor woken for: the
    /// player keeps to the phase it is in, as the environment does when it
    /// drops a late move, and the observation's `seq` beside the message's
    /// shows it came late. Told the phase it is in is closing, it closes
    /// it, at once if it has no call in flight for it. A reminder to itself
    /// is ignored.
    async fn receive(&mut self, message: &Message<G>) -> anyhow::Result<()> {
        let Some(n) = message.number() else {
            return Ok(());
        };
        if let Message::Closing { seq, .. } = message {
            observe(&self.context, message.sender(), n, self.mind.seq());
            if self.mind.in_phase(*seq) && self.mind.close(*seq) {
                self.closed(*seq)?;
            }
            return Ok(());
        }
        if let Message::Announce {
            seq, view, limit, ..
        } = message
        {
            self.mind.announce(*seq, view, *limit);
        }
        let counts = self.mind.counts(&self.context.id, message);
        observe(&self.context, message.sender(), n, self.mind.seq());
        if let Some(wakes) = counts {
            self.remember(Memory::Observed {
                message: message.clone(),
            });
            if wakes {
                self.wake()?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl<G: Game> Think for Player<G> {
    type Message = Message<G>;

    /// Woken, decide what to do in the phase the player is in. Reminded,
    /// wake if the reminder still counts: it is the latest set and its
    /// phase has not ended. Nothing else is thought about. The think loop
    /// drops an error a thought returns, so the log is the one place a
    /// failure shows, whether the model's or the player's own.
    async fn think(&mut self, message: &Message<G>) -> anyhow::Result<()> {
        match message {
            Message::Wake => self.decide().await,
            Message::Remind { seq, count } if self.mind.reminder_counts(*seq, *count) => {
                self.wake()
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::game::Narrate;
    use crate::game::tests::logged;
    use crate::model::Response;
    use crate::model::fake::FakeModel;
    use crate::seat::ENVIRONMENT;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// The id of the model every test's players ask for.
    pub(crate) const MODEL_ID: &str = "qwen2.5-7b-instruct";

    /// The name of the strategy every test's model players play.
    pub(crate) const PROMPTED: &str = "prompted";

    /// The prompt the player `name` is told, which names it, so that a
    /// request says whose it is.
    pub(crate) fn told(name: &str) -> String {
        format!("You are {name}.")
    }

    /// What the player `name` asked `model`, in order: the user message of
    /// each request carrying its prompt.
    pub(crate) fn asked_of(model: &FakeModel, name: &str) -> Vec<String> {
        let prompt = model::Message::system(told(name));
        model
            .requests()
            .iter()
            .filter(|request| request.messages[0] == prompt)
            .map(|request| request.messages[1].content.clone())
            .collect()
    }

    /// The story a request's user message tells, before the situation.
    pub(crate) fn story(asked: &str) -> &str {
        asked.rsplit_once("\n\n").unwrap().0
    }

    /// The situation a request's user message ends with, before what
    /// every request ends with.
    pub(crate) fn situated(asked: &str) -> &str {
        let ending = asked.rsplit_once("\n\n").unwrap().1;
        ending.strip_suffix(&format!(" {ANSWER}")).unwrap()
    }

    /// A model answering every request with `answer` of it, held so that
    /// the test can read what it was asked while the players ask it.
    pub(crate) fn answering(
        answer: impl Fn(&Request) -> anyhow::Result<Response> + Send + Sync + 'static,
    ) -> Arc<FakeModel> {
        Arc::new(FakeModel::answering(answer))
    }

    /// `answer`, its provider reporting a usage naming whose request it
    /// answered.
    pub(crate) fn reporting(
        answer: impl Fn(&Request) -> anyhow::Result<Response> + Send + Sync + 'static,
    ) -> Arc<FakeModel> {
        answering(move |request| {
            let usage = json!({"asked": request.messages[0].content});
            answer(request).map(|response| response.reporting(usage))
        })
    }

    /// A tool call as a model makes it: the tool's name and its arguments.
    pub(crate) type Made = (String, serde_json::Value);

    /// A call to `speak` saying `text`.
    pub(crate) fn speaking(text: &str) -> Made {
        (SPEAK.to_string(), json!({"text": text}))
    }

    /// A call to `wait` for `seconds`.
    pub(crate) fn waiting(seconds: u64) -> Made {
        (WAIT.to_string(), json!({"seconds": seconds}))
    }

    /// A player's memory of sending `message`, in no episode in particular.
    pub(crate) fn sent<G: Game>(message: Message<G>) -> Event<Memory<G>> {
        logged(Memory::Acted { message })
    }

    /// A player's memory of receiving `message`, in no episode in
    /// particular.
    pub(crate) fn received<G: Game>(message: Message<G>) -> Event<Memory<G>> {
        logged(Memory::Observed { message })
    }

    /// The omniscient account of `entries`, line by line.
    pub(crate) fn narrated<G: Game>(entries: Vec<Entry<G>>) -> Vec<String> {
        let mut narrator = G::Narrator::default();
        entries
            .into_iter()
            .flat_map(|entry| narrator.narrate(&logged(entry)))
            .map(|line| line.to_string())
            .collect()
    }

    /// The failures the players logged, each as who failed, in which
    /// phase, and what went wrong, in the log's order.
    pub(crate) fn failed<G: Game>(logged: &[Entry<G>]) -> Vec<(&str, u64, &str)> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Failed { player, seq, error } => {
                    Some((player.as_str(), *seq, error.as_str()))
                }
                _ => None,
            })
            .collect()
    }

    /// The usages the players logged, each with where it is in the log,
    /// who logged it, and in which phase, in the log's order.
    pub(crate) fn usages<G: Game>(
        logged: &[Entry<G>],
    ) -> Vec<(usize, &str, u64, &serde_json::Value)> {
        logged
            .iter()
            .enumerate()
            .filter_map(|(i, entry)| match entry {
                Entry::Usage { player, seq, usage } => Some((i, player.as_str(), *seq, usage)),
                _ => None,
            })
            .collect()
    }

    /// The phases of what `player` logged sending, withholding or failing
    /// after the entry at `i` in the log and before its next usage: what
    /// came of the call whose usage is at `i`. A close of a phase is no
    /// part of what came of a call.
    pub(crate) fn came_of<G: Game>(
        logged: &[Entry<G>],
        i: usize,
        player: &str,
    ) -> Vec<Option<u64>> {
        logged[i + 1..]
            .iter()
            .take_while(|entry| !matches!(entry, Entry::Usage { player: by, .. } if by == player))
            .filter_map(|entry| match entry {
                Entry::Action { from, message, .. } | Entry::Withheld { from, message, .. }
                    if from == player && !closes(message) =>
                {
                    Some(message.seq())
                }
                Entry::Failed {
                    player: by, seq, ..
                } if by == player => Some(Some(*seq)),
                _ => None,
            })
            .collect()
    }

    /// Whether `message` is a player's close of a phase, which no model
    /// made.
    pub(crate) fn closes<G: Game>(message: &Message<G>) -> bool {
        matches!(message, Message::Closed { .. })
    }

    /// How many lines of talk each of the players' actions but their closes
    /// says the model forgot, in the log's order.
    pub(crate) fn forgotten<G: Game>(logged: &[Entry<G>]) -> Vec<Option<usize>> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Action {
                    from,
                    forgot,
                    message,
                    ..
                } if from != ENVIRONMENT && !closes(message) => Some(*forgot),
                _ => None,
            })
            .collect()
    }

    /// Check what `logged`, a game whose players asked `model`, which
    /// reported a usage naming whose request it answered, says of the
    /// usages: one for each call, each naming the player and the phase of
    /// what came of the call, and none in the narration.
    pub(crate) fn every_call_logs_its_usage<G: Game>(model: &FakeModel, logged: Vec<Entry<G>>) {
        let usages = usages(&logged);
        assert_eq!(usages.len(), model.requests().len());
        let mut came = 0;
        for (i, player, seq, usage) in usages {
            assert_eq!(*usage, json!({"asked": told(player)}));
            let phases = came_of(&logged, i, player);
            assert!(
                phases.iter().all(|phase| *phase == Some(seq)),
                "{player} in {seq}: {phases:?}"
            );
            came += phases.len();
        }
        assert!(came > 0);
        narration_leaves_out(logged, |entry| matches!(entry, Entry::Usage { .. }));
    }

    /// Check that `logged`, a game in which some response was not well
    /// formed, logs each failure after the usage of the call that failed.
    pub(crate) fn a_failure_logs_its_usage_first<G: Game>(logged: &[Entry<G>]) {
        let usages = usages(logged);
        let failures: Vec<(usize, &str, u64)> = logged
            .iter()
            .enumerate()
            .filter_map(|(i, entry)| match entry {
                Entry::Failed { player, seq, .. } => Some((i, player.as_str(), *seq)),
                _ => None,
            })
            .collect();
        assert!(!failures.is_empty());
        for (failed, player, seq) in failures {
            let (i, _, called, _) = usages
                .iter()
                .rev()
                .find(|(i, by, _, _)| *i < failed && *by == player)
                .unwrap();
            assert_eq!(*called, seq, "{player}");
            assert_eq!(came_of(logged, *i, player), [Some(seq)], "{player}");
        }
    }

    /// What a player logged of a call or of what came of it: what kind of
    /// entry, the phase it names, and the error it tells of, if any.
    pub(crate) type Called = (&'static str, Option<u64>, String);

    /// Every call each player logged, in the log's order: the entries the
    /// player logged from its `Thinking` up to its next but its closes,
    /// each told as [`Called`].
    pub(crate) fn calls<G: Game>(logged: &[Entry<G>]) -> BTreeMap<&str, Vec<Vec<Called>>> {
        let mut calls: BTreeMap<&str, Vec<Vec<Called>>> = BTreeMap::new();
        for entry in logged {
            let (player, called): (&str, Called) = match entry {
                Entry::Thinking { player, seq } => {
                    let thinking = ("thinking", Some(*seq), String::new());
                    calls.entry(player).or_default().push(vec![thinking]);
                    continue;
                }
                Entry::Thought {
                    player,
                    seq,
                    ending: Ending::Answered,
                } => (player, ("answered", Some(*seq), String::new())),
                Entry::Thought {
                    player,
                    seq,
                    ending: Ending::Failed(error),
                } => (player, ("failed", Some(*seq), error.clone())),
                Entry::Usage { player, seq, .. } => (player, ("usage", Some(*seq), String::new())),
                Entry::Action { from, message, .. } | Entry::Withheld { from, message, .. }
                    if from != ENVIRONMENT && !closes(message) =>
                {
                    (from, ("acted", message.seq(), String::new()))
                }
                Entry::Failed { player, seq, error } => {
                    (player, ("could not act", Some(*seq), error.clone()))
                }
                _ => continue,
            };
            if let Some(call) = calls.get_mut(player).and_then(|calls| calls.last_mut()) {
                call.push(called);
            }
        }
        calls
    }

    /// Every call the players logged in `logged`, a game whose players
    /// asked `model`, checking that there is one for each request.
    fn every_call<G: Game>(model: &FakeModel, logged: &[Entry<G>]) -> Vec<Vec<Called>> {
        let all: Vec<Vec<Called>> = calls(logged).into_values().flatten().collect();
        assert!(!all.is_empty());
        assert_eq!(all.len(), model.requests().len());
        all
    }

    /// Check that narrating `logged` tells the same as narrating it without
    /// the entries `left_out` picks.
    fn narration_leaves_out<G: Game>(logged: Vec<Entry<G>>, left_out: fn(&Entry<G>) -> bool) {
        let without: Vec<Entry<G>> = logged
            .iter()
            .filter(|entry| !left_out(entry))
            .cloned()
            .collect();
        assert_eq!(narrated(logged), narrated(without));
    }

    /// Check what `logged`, a game whose players asked `model`, which
    /// answered every call and reported a usage for it, says of the calls:
    /// each one logged as `Thinking`, then `Thought` answered, then its
    /// usage, then what came of it, all in its phase, and none of it in the
    /// narration.
    pub(crate) fn every_call_logs_thinking_then_thought<G: Game>(
        model: &FakeModel,
        logged: Vec<Entry<G>>,
    ) {
        let mut acted = 0;
        for call in every_call(model, &logged) {
            let seq = call[0].1;
            let kinds: Vec<&str> = call.iter().map(|(kind, _, _)| *kind).collect();
            assert_eq!(kinds[..3], ["thinking", "answered", "usage"], "{call:?}");
            assert!(call.iter().all(|(_, phase, _)| *phase == seq), "{call:?}");
            acted += kinds.iter().filter(|kind| **kind == "acted").count();
        }
        assert!(acted > 0);
        narration_leaves_out(logged, |entry| {
            matches!(entry, Entry::Thinking { .. } | Entry::Thought { .. })
        });
    }

    /// Check what `logged`, a game whose players asked `model`, which
    /// failed every call, says of the calls: each one logged as `Thinking`,
    /// then `Thought` failed with the error, causes included, then the
    /// player's failure to act with the same error, all in its phase.
    pub(crate) fn a_failed_call_logs_its_error<G: Game>(
        model: &FakeModel,
        logged: &[Entry<G>],
        error: &str,
    ) {
        for call in every_call(model, logged) {
            let seq = call[0].1;
            assert_eq!(
                call,
                [
                    ("thinking", seq, String::new()),
                    ("failed", seq, error.to_string()),
                    ("could not act", seq, error.to_string()),
                ]
            );
        }
    }

    /// Check what `logged`, a game whose players asked `model`, which never
    /// answers, says of the calls: each one logged as `Thinking` alone, the
    /// player stopped before its call came back.
    pub(crate) fn a_call_cut_off_logs_no_thought<G: Game>(model: &FakeModel, logged: &[Entry<G>]) {
        let all = every_call(model, logged);
        assert!(all.iter().all(|call| call.len() == 1), "{all:?}");
    }

    #[test]
    fn arguments_sent_as_no_text_are_no_arguments() {
        let call = |arguments: &str| model::Call {
            name: SPEAK.to_string(),
            arguments: arguments.to_string(),
        };
        for bare in ["", " \n"] {
            let error = arguments::<Speech>(&call(bare)).err().unwrap();
            assert!(
                format!("{error:#}").contains("missing field `text`"),
                "{error:#}"
            );
        }
    }
}
