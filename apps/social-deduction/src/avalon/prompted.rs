//! The prompted policy: a player whose choices a language model makes.
//! The [`config`](super::config) sets each one up: its system prompt, its
//! model, the tool choice and the context window.
//!
//! A player remembers what it is shown, what it hears and what it does, as
//! a [`Memory`] of each message stamped when it was received or sent, and
//! keeps with the history the story that tells it from its own point of
//! view: each memory adds its lines to the story as it is remembered, so
//! the story is never told over. Every player is announced every phase, so
//! a player knows which phase it is in, and it ignores a message tagged
//! with any other: it logs the receipt, as it does every message's, and
//! neither remembers the message nor wakes for it. Each time it is woken,
//! it tells the model the story so far and where the game stands, with how
//! long the phase lasts, and carries out what the model answers. The
//! request fits the context window by estimate: when the story would not,
//! the oldest talk is forgotten for that request. Since every announcement
//! carries the record of every team, vote and quest, forgetting talk never
//! forgets a fact of the game. The call is made in the player's think
//! loop, so the player keeps perceiving while the model works.
//!
//! A player may talk in every phase, so it is woken by every message it
//! remembers: the announcement, and another player's utterance. Wake-ups
//! are debounced: the perceive loop remembers each message and wakes the
//! think loop only when no wake-up is already waiting, and the think loop
//! takes the history as it stands when it is woken, so a player has at
//! most one call in progress and one waiting, however fast the talk comes.
//! Every request offers `speak`, which sends what it says to every other
//! player and not to the environment, and `wait`, which asks to be woken
//! after some seconds unless something wakes the player sooner. A player
//! the phase waits on is offered one tool more, whose call goes to the
//! environment alone: the leader `propose`, every player `vote`, each
//! member of the team `perform`, and the Assassin `select`. That tool is
//! offered again after the player has used it; a second use is sent, and
//! the environment counts only the first. A reminder is numbered so that
//! only the latest set counts, and one for a phase that has ended is
//! ignored. The calls are carried out in the order the model made them.
//! Before carrying out anything a response asks for, the player checks
//! that the phase the call began in is still the one it was last
//! announced; if not, it sends nothing and logs each message it would have
//! sent as [`Entry::Withheld`].
//!
//! A model that is not reached, or an answer that is not well formed, is a
//! failure: the player logs it as [`Entry::Failed`], carries nothing out,
//! and the game goes on. The player logs an [`Entry::Observation`] for each
//! message it receives and an [`Entry::Action`] for each utterance and
//! choice it sends, numbered together from 1, with how many memories the
//! model was shown when it made the call that produced the message and how
//! many lines of talk that call forgot. Before a game begins,
//! [`check_plays`] asks the model four times, once as the player each phase
//! waits on, so a model that does not answer each request well fails at
//! launch, in the provider's words. Every request makes the model call a
//! tool, unless the probe finds the model refuses that, and then every
//! request asks it to call one, and an answer in prose is a failure like
//! any other.

use super::report::{Story, in_prose, sabotages_fail};
use super::{
    Actor, Ballot, Choice, Deed, Entry, Message, PROPOSALS, Phase, Role, Rules, Seat, Team, View,
    act, awaited, candidates, observe, receivers, team_size,
};
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
use std::collections::{HashMap, HashSet};
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What a player remembers: a message it received from another actor in
/// the phase it was in, or one it sent. Reminders to itself, messages from
/// another phase, and what it withheld, are not remembered.
#[derive(Debug, Clone, PartialEq)]
pub enum Memory {
    /// The player received `message`, whose [`Message::sender`] sent it.
    Observed {
        /// What was sent.
        message: Message,
    },
    /// The player sent `message`.
    Acted {
        /// What it sent.
        message: Message,
    },
}

/// What a player's two loops share: what it has been shown and what it has
/// done, with the story that tells it, the phase it is in, whether a
/// wake-up is waiting, how many reminders it has set, what it is told, and
/// whom it asks.
struct Mind {
    /// The history and the story told from it. Never locked across an
    /// await.
    remembered: Mutex<Remembered>,
    /// The number of the last message the player sent to others.
    n: AtomicU64,
    /// The latest phase the player was announced: the phase it is in, so far
    /// as it knows. Never locked across an await.
    announced: Mutex<Option<Announced>>,
    /// Whether a [`Message::Wake`] is waiting for the think loop. The
    /// perceive loop hands the think loop one only when none is waiting,
    /// and the think loop clears this before it takes the history, so
    /// however fast messages come the player has at most one call in
    /// progress and one waiting, and nothing it heard is left out of both.
    waking: AtomicBool,
    /// How many reminders the player has set, which numbers them so that
    /// only the latest counts.
    reminders: AtomicU64,
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
}

/// A player's history and the story that tells it, kept together so that
/// the story is always the narration of exactly the history, and a request
/// joins the story's lines instead of narrating every memory again.
struct Remembered {
    /// The history, in order: each message received and each choice or
    /// utterance sent, stamped when the player remembered it.
    history: Vec<Event<Memory>>,
    /// The story told from the history.
    story: Story,
}

/// What the model was shown for a call: how many memories the story was
/// told from, and how many lines of talk it forgot to fit the context
/// window, if any.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shown {
    seen: usize,
    forgot: Option<usize>,
}

/// A phase as a player was announced it: its number, what the player may
/// see of it, and how long it lasts.
#[derive(Clone)]
struct Announced {
    seq: u64,
    view: View,
    limit: Duration,
}

impl Mind {
    /// The mind of `me` with nothing in its history yet, told `prompt`,
    /// asking `model` as the model `model_id` under `tool_choice`, within
    /// `context_window` tokens.
    fn new(
        me: PlayerId,
        prompt: String,
        model_id: String,
        tool_choice: ToolChoice,
        context_window: u64,
        model: Arc<dyn model::Model>,
    ) -> Self {
        Self {
            remembered: Mutex::new(Remembered {
                history: Vec::new(),
                story: Story::of(me),
            }),
            n: AtomicU64::new(0),
            announced: Mutex::new(None),
            waking: AtomicBool::new(false),
            reminders: AtomicU64::new(0),
            prompt,
            model_id,
            tool_choice,
            context_window,
            model,
        }
    }

    /// Add `event` to the end of the history, and what it adds to the
    /// account to the end of the story, together. A message is remembered
    /// when it is received or sent, so a choice sent in the moment between
    /// the check that its phase is still on and the send comes after the
    /// next phase's announcement, and is told in that phase.
    fn remember(&self, event: Event<Memory>) {
        let mut remembered = self.remembered.lock().unwrap();
        remembered.story.recall(&event);
        remembered.history.push(event);
    }

    /// The story so far within `budget` tokens, its lines joined, and
    /// what it was told from: how many memories, and how many lines of talk
    /// it forgot to fit.
    fn story(&self, budget: u64) -> (String, Shown) {
        let remembered = self.remembered.lock().unwrap();
        let (lines, forgot) = remembered.story.told_within(budget);
        let shown = Shown {
            seen: remembered.history.len(),
            forgot: (forgot > 0).then_some(forgot),
        };
        (lines.join("\n"), shown)
    }

    /// The next number for a message to others: one up from the last.
    fn number(&self) -> u64 {
        self.n.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Take in the announcement of the phase numbered `seq`, showing `view`
    /// and lasting `limit`: the phase the player is in from now on.
    fn announce(&self, seq: u64, view: &View, limit: Duration) {
        *self.announced.lock().unwrap() = Some(Announced {
            seq,
            view: view.clone(),
            limit,
        });
    }

    /// The latest phase the player was announced, if any.
    fn announced(&self) -> Option<Announced> {
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
}

/// A player whose choices are made by a language model. Its perceive loop
/// and its think loop are each a `Player` with a context of its own over
/// one shared mind: the perceive loop remembers each message it is sent
/// and wakes the think loop, which tells the model the history as a story
/// and carries out what the model answers. [`Actor::Prompted`] holds the
/// perceive loop's, built by [`Player::init`].
pub struct Player {
    context: Context<Message, Entry>,
    mind: Arc<Mind>,
}

impl Player {
    /// Builds a player's two loops once the episode has made their
    /// contexts: the perceive loop as the actor, and the think loop. The
    /// player `me` is told `prompt`, and asks `model` as the model
    /// `model_id` under `tool_choice`, within `context_window` tokens.
    pub fn init(
        me: PlayerId,
        prompt: String,
        model_id: String,
        tool_choice: ToolChoice,
        context_window: u64,
        model: Arc<dyn model::Model>,
    ) -> Seat {
        let mind = Arc::new(Mind::new(
            me,
            prompt,
            model_id,
            tool_choice,
            context_window,
            model,
        ));
        let shared = Arc::clone(&mind);
        let behavior: Builder<Actor> =
            Box::new(move |context| Actor::Prompted(Player { context, mind }));
        let think: ThinkBuilder<Message, Entry> = Box::new(move |context| {
            Box::new(Player {
                context,
                mind: shared,
            })
        });
        (behavior, Some(think))
    }

    /// Add `memory` to the history, stamped now, and what it tells to the
    /// story.
    fn remember(&self, memory: Memory) {
        self.mind.remember(Event::now(self.context.episode, memory));
    }

    /// Wake the think loop, unless a wake-up is already waiting for it.
    fn wake(&self) -> anyhow::Result<()> {
        if self.mind.waking() {
            self.context.think(Message::Wake)?;
        }
        Ok(())
    }

    /// The request for what to do in the phase `view` shows, which lasts
    /// `limit`, with `duty` asked of the player, if anything: the system
    /// prompt, then the story so far, a blank line, and where the game
    /// stands, with the tools to answer with. The story fits what is left
    /// of the context window once the reply, the system prompt, the tools
    /// and the situation are counted, by estimate, forgetting talk if it
    /// has to. With the request, what the story was told from.
    fn request(&self, view: &View, limit: Duration, duty: Option<&Duty>) -> (Request, Shown) {
        let situation = format!("\n\n{}", situation(view, limit, duty));
        let mind = &self.mind;
        let tools = tools(duty, limit);
        let asking = |asked: String| {
            request(
                &mind.model_id,
                mind.tool_choice,
                &mind.prompt,
                asked,
                tools.clone(),
            )
        };
        let taken = model::estimated(&asking(situation.clone()));
        let (story, shown) = mind.story(mind.context_window.saturating_sub(taken));
        (asking(story + &situation), shown)
    }

    /// Woken, decide: take the wake-up, and ask the model what to do in
    /// the phase the player is in. If that fails, log the failure with the
    /// phase and what went wrong, causes included, which is all there is to
    /// do about it: the player has not acted, and the game goes on. The
    /// failure is not remembered, so the model is never told of it. Being
    /// woken before any phase was announced is an error.
    async fn decide(&self) -> anyhow::Result<()> {
        self.mind.woken();
        let Announced { seq, view, limit } = self
            .mind
            .announced()
            .context("woken before any phase was announced")?;
        if let Err(error) = self.play(seq, &view, limit).await {
            self.context.log(Entry::Failed {
                player: self.context.id.clone(),
                seq,
                error: format!("{error:#}"),
            });
        }
        Ok(())
    }

    /// Ask the model what to do in the phase numbered `seq`, which `view`
    /// shows and which lasts `limit`: speak, make the choice the phase
    /// waits on the player for, if it does, or wait, in any number and
    /// order, with one response. A response that is not well formed, as
    /// [`asked`] judges it, is an error, and nothing in it is carried out.
    /// Otherwise the messages are sent in the order the model asked for
    /// them, each utterance to every other player and each choice to the
    /// environment, and a wait sets a reminder; unless the phase has ended
    /// by the time the model answers, and then the messages are withheld
    /// and no reminder is set.
    async fn play(&self, seq: u64, view: &View, limit: Duration) -> anyhow::Result<()> {
        let duty = Duty::of(&self.context.id, view);
        let (request, shown) = self.request(view, limit, duty.as_ref());
        let response = self.mind.model.complete(&request).await?;
        let Asked { sends, wait } = asked(&response, duty.as_ref(), limit)?;
        let still_on = self.carry_out(seq, view, sends, shown)?;
        if still_on && let Some(after) = wait {
            self.remind(seq, after)?;
        }
        Ok(())
    }

    /// Carry out `sends`, what a response made from what it was `shown`
    /// asked for in the phase numbered `seq`, which `view` shows, if that is
    /// still the phase the player was last announced: send each in order,
    /// numbered, to whom the rules of the game send it. A call may begin in
    /// one phase and end in the next, and then nothing is sent, and each
    /// message is logged as withheld, unnumbered. Says whether the phase
    /// was still on.
    fn carry_out(
        &self,
        seq: u64,
        view: &View,
        sends: Vec<Sending>,
        shown: Shown,
    ) -> anyhow::Result<bool> {
        let me = &self.context.id;
        let still_on = self.mind.in_phase(seq);
        for sending in sends {
            if still_on {
                let message = sending.message(seq, me, self.mind.number());
                let to = receivers(&message, me, view);
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
        Ok(still_on)
    }

    /// Remember sending `message` and send it to everyone in `to`, logged
    /// as the player's action with what the model was `shown` when it made
    /// the call that produced it.
    fn send(&self, message: Message, to: HashSet<ActorId>, shown: Shown) -> anyhow::Result<()> {
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
impl Behavior for Player {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        &self.context
    }

    /// Log a message from another actor, which is any numbered one, as
    /// observed in the latest phase announced. An announcement is of a new
    /// phase. It, and anything else tagged with the phase it announced,
    /// another player's utterance above all, is remembered and wakes the
    /// think loop. One tagged with another phase is observed and nothing
    /// more, neither remembered nor woken for: the player keeps to the
    /// phase it is in, as the environment does when it drops a late
    /// choice, and the observation's `seq` beside the message's shows it
    /// came late. A reminder to itself is ignored.
    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        let Some(n) = message.number() else {
            return Ok(());
        };
        if let Message::Announce {
            seq, view, limit, ..
        } = message
        {
            self.mind.announce(*seq, view, *limit);
        }
        let counts = message.seq().is_some_and(|seq| self.mind.in_phase(seq));
        observe(&self.context, message.sender(), n, self.mind.seq());
        if counts {
            self.remember(Memory::Observed {
                message: message.clone(),
            });
            self.wake()?;
        }
        Ok(())
    }
}

#[async_trait]
impl Think for Player {
    type Message = Message;

    /// Woken, decide what to do in the phase the player is in. Reminded,
    /// wake if the reminder still counts: it is the latest set and its
    /// phase has not ended. Nothing else is thought about. The think loop
    /// drops an error a thought returns, so the log is the one place a
    /// failure shows, whether the model's or the player's own.
    async fn think(&mut self, message: &Message) -> anyhow::Result<()> {
        match message {
            Message::Wake => self.decide().await,
            Message::Remind { seq, count } if self.mind.reminder_counts(*seq, *count) => {
                self.wake()
            }
            _ => Ok(()),
        }
    }
}

/// What a phase waits on a player to choose, with what it may choose from.
#[derive(Debug, Clone, PartialEq)]
enum Duty {
    /// The leader proposes a team of `size` from `seats`, every player at
    /// the table in seat order.
    Propose {
        /// How many go on the quest.
        size: usize,
        /// Everyone at the table.
        seats: Vec<PlayerId>,
    },
    /// Every player votes on the team.
    Vote,
    /// Each member of the team does one of `deeds`.
    Perform {
        /// What the member may do: support, and for an evil player
        /// sabotage.
        deeds: Vec<Deed>,
    },
    /// The Assassin names one of `targets` as Merlin.
    Select {
        /// The players it does not see as evil.
        targets: Vec<PlayerId>,
    },
}

impl Duty {
    /// What the phase `view` shows waits on `me` to choose, if it
    /// [awaits](awaited) it.
    fn of(me: &PlayerId, view: &View) -> Option<Self> {
        if !awaited(me, view) {
            return None;
        }
        Some(match view.phase {
            Phase::Propose => Duty::Propose {
                size: view.size,
                seats: view.players.clone(),
            },
            Phase::Vote => Duty::Vote,
            Phase::Quest => Duty::Perform {
                deeds: deeds(view.role),
            },
            Phase::Assassinate => Duty::Select {
                targets: candidates(me, view),
            },
        })
    }

    /// The phase that waits on a player for this.
    fn phase(&self) -> Phase {
        match self {
            Duty::Propose { .. } => Phase::Propose,
            Duty::Vote => Phase::Vote,
            Duty::Perform { .. } => Phase::Quest,
            Duty::Select { .. } => Phase::Assassinate,
        }
    }

    /// What the player is asked to do.
    fn asked(&self) -> String {
        match self {
            Duty::Propose { size, .. } => format!("You lead: propose a team of {size}."),
            Duty::Vote => "Vote to approve or reject the team.".to_string(),
            Duty::Perform { deeds } if deeds.len() == 1 => {
                "You are on the quest: support it.".to_string()
            }
            Duty::Perform { .. } => "You are on the quest: support or sabotage it.".to_string(),
            Duty::Select { .. } => "Name the player you believe is Merlin.".to_string(),
        }
    }

    /// The tool the player chooses with.
    fn tool(&self) -> Tool {
        match self {
            Duty::Propose { size, seats } => propose(*size, seats),
            Duty::Vote => vote(),
            Duty::Perform { deeds } => perform(deeds),
            Duty::Select { targets } => select(targets),
        }
    }
}

/// The deeds a player of `role` may do on a quest: support, and for an
/// evil player sabotage.
fn deeds(role: Role) -> Vec<Deed> {
    match role.team() {
        Team::Good => vec![Deed::Support],
        Team::Evil => vec![Deed::Support, Deed::Sabotage],
    }
}

/// A deed as the `perform` tool names it.
fn deed_name(deed: Deed) -> &'static str {
    match deed {
        Deed::Support => "support",
        Deed::Sabotage => "sabotage",
    }
}

/// Where the game stands as `view` shows it, how long the phase lasts, and
/// what is asked of the player: its `duty`, if it has one, and otherwise
/// that it may speak or wait. The quest and the proposal, the leader, the
/// size of the team and how many sabotages fail it, and the team proposed
/// when there is one.
fn situation(view: &View, limit: Duration, duty: Option<&Duty>) -> String {
    let mut said = format!(
        "It is quest {}, proposal {} of {PROPOSALS}, led by {}. The quest needs a team of {}, and {} it.",
        view.quest,
        view.proposal,
        view.leader,
        view.size,
        sabotages_fail(view.sabotages_needed)
    );
    if let Some(team) = &view.team {
        said.push_str(&format!(
            " The team proposed is {}.",
            in_prose(team.iter().cloned())
        ));
    }
    let asked = duty.map_or("You may speak or wait.".to_string(), Duty::asked);
    said.push_str(&format!(
        " The {} lasts {}. {asked}",
        view.phase.told(),
        format_duration(limit)
    ));
    said
}

/// A request of a player's shape to `model_id` under `tool_choice`:
/// `prompt` as the system message, `asked` and then [`ANSWER`] as the user
/// message, and `tools` to answer with. This is the one way a player
/// builds a request and the one way the probe does, so they agree on what
/// is asked.
fn request(
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

/// The most a request to the model `model_id` from `me`, dealt `role` and
/// told `prompt`, takes of the context window before its story, by
/// estimate, at a table of `seats` under `rules`: the system prompt, and
/// the largest of the four phases' situations and tools on the fifth
/// proposal of the fifth quest, whose team is the largest, and the reply.
pub(crate) fn unstoried(
    model_id: &str,
    prompt: &str,
    me: &PlayerId,
    role: Role,
    seats: &[PlayerId],
    rules: &Rules,
) -> u64 {
    let quest = NonZero::new(5).unwrap();
    let size = team_size(seats.len(), quest);
    let team = seats[seats.len().saturating_sub(size)..].to_vec();
    let others: Vec<PlayerId> = seats.iter().filter(|seat| *seat != me).cloned().collect();
    let duties = [
        Duty::Propose {
            size,
            seats: seats.to_vec(),
        },
        Duty::Vote,
        Duty::Perform { deeds: deeds(role) },
        Duty::Select { targets: others },
    ];
    let mut most = 0;
    for duty in &duties {
        let (phase, limit) = (duty.phase(), rules.limit(duty.phase()));
        let view = View {
            quest,
            proposal: NonZero::new(PROPOSALS).unwrap(),
            phase,
            leader: me.clone(),
            size,
            sabotages_needed: 2,
            team: Some(team.clone()),
            tally: None,
            record: vec![],
            role,
            sees: HashMap::new(),
            players: seats.to_vec(),
        };
        let asked = format!("\n\n{}", situation(&view, limit, Some(duty)));
        let tools = tools(Some(duty), limit);
        let request = request(model_id, ToolChoice::Required, prompt, asked, tools);
        most = most.max(model::estimated(&request));
    }
    most
}

/// What every request ends with, under either tool choice: that a tool
/// call is the one answer that counts.
const ANSWER: &str = "Answer by calling one of the tools.";

/// The tools a player is offered in a phase that lasts `limit`: [`speak`],
/// the tool its `duty` chooses with if it has one, and [`wait`].
fn tools(duty: Option<&Duty>, limit: Duration) -> Vec<Tool> {
    std::iter::once(speak())
        .chain(duty.map(Duty::tool))
        .chain([wait(limit)])
        .collect()
}

/// The placeholder seats the probe asks about.
fn placeholders() -> Vec<PlayerId> {
    (1..=5).map(|seat| format!("player{seat}")).collect()
}

/// How long each phase the probe asks about lasts.
const PROBE_LIMIT: Duration = Duration::from_secs(60);

/// The four requests the probe makes, each as whom it asks the model to
/// play and what that player's phase waits on it for, over five
/// placeholder seats: the leader proposing a team of 2, a voter, an evil
/// member of a team, offered both deeds, and the Assassin.
fn probes() -> [(&'static str, Duty); 4] {
    let seats = placeholders();
    [
        (
            "the leader",
            Duty::Propose {
                size: 2,
                seats: seats.clone(),
            },
        ),
        ("a voter", Duty::Vote),
        (
            "an evil member of a team",
            Duty::Perform {
                deeds: deeds(Role::Minion),
            },
        ),
        (
            "the Assassin",
            Duty::Select {
                targets: seats[1..].to_vec(),
            },
        ),
    ]
}

/// What a probe's request says: how long its phase lasts, and what the
/// player is asked to do.
fn probe_asked(duty: &Duty) -> String {
    format!(
        "The {} lasts {}. {}",
        duty.phase().told(),
        format_duration(PROBE_LIMIT),
        duty.asked()
    )
}

/// Check that `model`, asked as a player is, plays, and find out whether
/// it may be made to call a tool: four requests of a player's shape, one
/// after the other, each with a short system prompt, as the player each
/// phase waits on, offered that phase's tools, and judged as a player's
/// answer is by `asked`: the leader proposing a team of 2 from five
/// placeholder seats, a voter, an evil member of a team, and the Assassin.
///
/// The first request requires a tool call. If the provider refuses that as
/// bad, it is asked again allowing one, and both refusals are given if
/// that is refused too. The first tool choice it is answered under is the
/// game's, and the rest are asked under it. No request is made after one
/// fails. A model that could not be asked at all, because the provider
/// was not reached or answered with anything but the model's answer, is
/// an error naming the model, with the cause beneath it.
pub async fn check_plays(model: &dyn model::Model, model_id: &str) -> anyhow::Result<Probed> {
    let ask = async |duty: &Duty, tool_choice| {
        let probe = request(
            model_id,
            tool_choice,
            PROBE_PROMPT,
            probe_asked(duty),
            tools(Some(duty), PROBE_LIMIT),
        );
        let response = model.complete(&probe).await?;
        asked(&response, Some(duty), PROBE_LIMIT).map(drop)
    };
    let [(first, leading), rest @ ..] = probes();
    let (tool_choice, answered) = match ask(&leading, ToolChoice::Required).await {
        Err(required) if model::refused(&required) => {
            let answered = ask(&leading, ToolChoice::Auto).await.map_err(|auto| {
                if model::concerns_the_model(&auto) {
                    anyhow!("made to call a tool: {required:#}\nasked to call one: {auto:#}")
                } else {
                    auto
                }
            });
            (ToolChoice::Auto, answered)
        }
        answered => (ToolChoice::Required, answered),
    };
    let mut failed = answered.err().map(|error| (first, error));
    for (who, duty) in &rest {
        if failed.is_some() {
            break;
        }
        failed = ask(duty, tool_choice)
            .await
            .err()
            .map(|error| (*who, error));
    }
    match failed {
        None => Ok(Probed::Played(tool_choice)),
        Some((who, error)) if model::concerns_the_model(&error) => Ok(Probed::DidNotPlay(
            error.context(format!("{model_id} did not answer as {who}")),
        )),
        Some((_, error)) => Err(error.context(format!("{model_id} could not be asked to play"))),
    }
}

/// The system prompt the probe tells the model.
const PROBE_PROMPT: &str = "You are a player in a game, about to be asked to play.";

/// The name of the tool a player speaks with.
const SPEAK: &str = "speak";

/// The name of the tool the leader proposes a team with.
const PROPOSE: &str = "propose";

/// The name of the tool a player votes with.
const VOTE: &str = "vote";

/// The name of the tool a member of the team does its deed with.
const PERFORM: &str = "perform";

/// The name of the tool the Assassin names Merlin with.
const SELECT: &str = "select";

/// The name of the tool a player waits with.
const WAIT: &str = "wait";

/// The tool the leader proposes with: `propose`, taking a `team` of `size`
/// distinct players, each one of `seats`.
fn propose(size: usize, seats: &[PlayerId]) -> Tool {
    Tool::new(
        PROPOSE,
        "Propose the team to go on the quest. Your first proposal is final.",
        json!({
            "type": "object",
            "properties": {"team": {
                "type": "array",
                "items": {"type": "string", "enum": seats},
                "minItems": size,
                "maxItems": size,
                "uniqueItems": true,
            }},
            "required": ["team"],
            "additionalProperties": false,
        }),
    )
}

/// The tool a player votes with: `vote`, taking whether it would
/// `approve` the team.
fn vote() -> Tool {
    Tool::new(
        VOTE,
        "Vote on the team: true to approve it, false to reject it. Your first vote is final.",
        json!({
            "type": "object",
            "properties": {"approve": {"type": "boolean"}},
            "required": ["approve"],
            "additionalProperties": false,
        }),
    )
}

/// The tool a member of the team does its deed with: `perform`, taking a
/// `deed` that is one of `deeds`.
fn perform(deeds: &[Deed]) -> Tool {
    let deeds: Vec<&str> = deeds.iter().copied().map(deed_name).collect();
    Tool::new(
        PERFORM,
        "Do your deed on the quest. Your first deed is final.",
        json!({
            "type": "object",
            "properties": {"deed": {"type": "string", "enum": deeds}},
            "required": ["deed"],
            "additionalProperties": false,
        }),
    )
}

/// The tool the Assassin names Merlin with: `select`, taking a `target`
/// that is one of `targets`.
fn select(targets: &[PlayerId]) -> Tool {
    Tool::new(
        SELECT,
        "Name the player you believe is Merlin. Your first choice is final.",
        json!({
            "type": "object",
            "properties": {"target": {"type": "string", "enum": targets}},
            "required": ["target"],
            "additionalProperties": false,
        }),
    )
}

/// The tool a player speaks with: `speak`, taking `text`, which is not
/// empty, nor blank once trimmed.
fn speak() -> Tool {
    Tool::new(
        SPEAK,
        "Say something to every other player.",
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
fn wait(limit: Duration) -> Tool {
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

/// The arguments of a [`propose`] call.
#[derive(Deserialize)]
struct Proposal {
    team: Vec<PlayerId>,
}

/// The arguments of a [`vote`] call.
#[derive(Deserialize)]
struct Voting {
    approve: bool,
}

/// The arguments of a [`perform`] call.
#[derive(Deserialize)]
struct Performing {
    deed: String,
}

/// The arguments of a [`select`] call.
#[derive(Deserialize)]
struct Selection {
    target: PlayerId,
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

/// What a response asks for: the messages to send, in the order the model
/// made the calls, and how long to wait, if it asked to. Of several waits,
/// the last counts.
#[derive(Debug, Default, PartialEq)]
struct Asked {
    /// What to send, in order.
    sends: Vec<Sending>,
    /// How long to wait, if asked.
    wait: Option<Duration>,
}

impl Asked {
    /// Take in `call`, one of a response's to a player with `duty` in a
    /// phase that lasts `limit`, if it is well formed, as [`asked`] judges
    /// it.
    fn take(
        &mut self,
        call: &model::Call,
        duty: Option<&Duty>,
        limit: Duration,
    ) -> anyhow::Result<()> {
        match (call.name.as_str(), duty) {
            (SPEAK, _) => {
                let Speech { text } = arguments(call)?;
                if text.trim().is_empty() {
                    bail!("the model's {SPEAK} call says nothing");
                }
                self.sends.push(Sending::Say(text));
            }
            (PROPOSE, Some(Duty::Propose { size, seats })) => {
                let Proposal { team } = arguments(call)?;
                proposed(&team, *size, seats)?;
                self.sends.push(Sending::Choose(Choice::Team(team)));
            }
            (VOTE, Some(Duty::Vote)) => {
                let Voting { approve } = arguments(call)?;
                let ballot = if approve {
                    Ballot::Approve
                } else {
                    Ballot::Reject
                };
                self.sends.push(Sending::Choose(Choice::Ballot(ballot)));
            }
            (PERFORM, Some(Duty::Perform { deeds })) => {
                let Performing { deed } = arguments(call)?;
                let Some(deed) = deeds.iter().copied().find(|d| deed_name(*d) == deed) else {
                    bail!("the model chose to {deed}, which is not a deed it was offered");
                };
                self.sends.push(Sending::Choose(Choice::Deed(deed)));
            }
            (SELECT, Some(Duty::Select { targets })) => {
                let Selection { target } = arguments(call)?;
                if !targets.contains(&target) {
                    bail!("the model named {target}, who is not a candidate");
                }
                self.sends.push(Sending::Choose(Choice::Target(target)));
            }
            (WAIT, _) => {
                let Waiting { seconds } = arguments(call)?;
                if !(1..=limit.as_secs()).contains(&seconds) {
                    bail!(
                        "the model asked to wait {seconds}s, which is not between 1s and {}",
                        format_duration(limit)
                    );
                }
                self.wait = Some(Duration::from_secs(seconds));
            }
            (other, _) => bail!("the model called {other}, which is not a tool it was offered"),
        }
        Ok(())
    }
}

/// Check that `team` is a team of `size` distinct players, each one of
/// `seats`.
fn proposed(team: &[PlayerId], size: usize, seats: &[PlayerId]) -> anyhow::Result<()> {
    if team.len() != size {
        bail!(
            "the model proposed a team of {}, and the quest needs {size}",
            team.len()
        );
    }
    for (i, player) in team.iter().enumerate() {
        if !seats.contains(player) {
            bail!("the model proposed {player}, who is not at the table");
        }
        if team[..i].contains(player) {
            bail!("the model proposed {player} twice");
        }
    }
    Ok(())
}

/// What a call has the player send: what it says, or the choice it makes.
#[derive(Debug, PartialEq)]
enum Sending {
    /// Say `text` to every other player.
    Say(String),
    /// Make the choice, for the environment alone.
    Choose(Choice),
}

impl Sending {
    /// The message `from` sends for this in the phase numbered `seq`,
    /// numbered `n`.
    fn message(self, seq: u64, from: &PlayerId, n: u64) -> Message {
        let from = from.clone();
        match self {
            Sending::Say(text) => Message::Say { seq, from, n, text },
            Sending::Choose(choice) => choice.message(seq, from, n),
        }
    }
}

/// What `response` asks for, its calls taken in the order the model made
/// them, from a player with `duty`, if any, in a phase that lasts `limit`.
/// The response is judged as a whole: no tool call, a call to a tool
/// [`tools`] does not offer the player, arguments that do not parse, a
/// team not of the size the quest needs, of distinct players at the
/// table, a deed or a target not offered, text that is empty once
/// trimmed, or seconds not between 1 and `limit` in whole seconds is an
/// error, and then nothing in it is carried out.
fn asked(response: &Response, duty: Option<&Duty>, limit: Duration) -> anyhow::Result<Asked> {
    let made = response.tool_calls();
    if made.is_empty() {
        bail!("the model made no tool call");
    }
    let mut asked = Asked::default();
    for call in made {
        asked.take(&call.function, duty, limit)?;
    }
    Ok(asked)
}

/// The arguments of `call`, parsed. A provider may send no arguments as no
/// text at all, which parses as `{}`. Arguments that do not parse are an
/// error naming the tool and showing them.
fn arguments<T: DeserializeOwned>(call: &model::Call) -> anyhow::Result<T> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avalon::report::{FORGOTTEN, Narrator};
    use crate::avalon::tests::{
        Played, Script, configured, dealt, five, logged, mute, observed_by, quick, run, utterance,
        view_of,
    };
    use crate::avalon::{Entry, Rules, Tally, Victory, game};
    use crate::model::canned;
    use crate::model::fake::{FakeModel, SAID, canned_calls, choosing_first};
    use crate::model::{CONTEXT_WINDOW, Provider, REQUEST_TIMEOUT};
    use crate::seat::{ENVIRONMENT, to_the_environment, to_the_players_but};
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::sync::oneshot;

    /// The id of the model every test's players ask for.
    const MODEL_ID: &str = "qwen2.5-7b-instruct";

    /// The name of the strategy every test's model players play.
    const PROMPTED: &str = "prompted";

    fn id(name: &str) -> PlayerId {
        name.to_string()
    }

    /// The prompt the player `name` is told, which names it, so that a
    /// request says whose it is.
    fn told(name: &str) -> String {
        format!("You are {name}.")
    }

    /// What the player `name` asked `model`, in order: the user message of
    /// each request carrying its prompt.
    fn asked_of(model: &FakeModel, name: &str) -> Vec<String> {
        let prompt = model::Message::system(told(name));
        model
            .requests()
            .iter()
            .filter(|request| request.messages[0] == prompt)
            .map(|request| request.messages[1].content.clone())
            .collect()
    }

    /// The story a request's user message tells, before the situation.
    fn story(asked: &str) -> &str {
        asked.rsplit_once("\n\n").unwrap().0
    }

    /// The situation a request's user message ends with, before what
    /// every request ends with.
    fn situated(asked: &str) -> &str {
        let ending = asked.rsplit_once("\n\n").unwrap().1;
        ending.strip_suffix(&format!(" {ANSWER}")).unwrap()
    }

    /// Run a game of `roles` under `rules`, every player a model player
    /// told its prompt and asking `model` under `tool_choice` within
    /// `window` tokens, except those in `puppets`, who follow their own
    /// scripts.
    async fn play_as(
        roles: HashMap<PlayerId, Role>,
        rules: Rules,
        model: Arc<FakeModel>,
        mut puppets: HashMap<&str, Seat>,
        tool_choice: ToolChoice,
        window: u64,
    ) -> anyhow::Result<Played> {
        let configuration = configured(&roles, &rules);
        let (winner, won) = oneshot::channel();
        let (logger, log) = unbounded_channel();
        let model: Arc<dyn model::Model> = model;
        let player = |id: &PlayerId| {
            puppets.remove(id.as_str()).unwrap_or_else(|| {
                Player::init(
                    id.clone(),
                    told(id),
                    MODEL_ID.to_string(),
                    tool_choice,
                    window,
                    Arc::clone(&model),
                )
            })
        };
        let episode = game(
            dealt(&roles, PROMPTED),
            rules,
            configuration,
            winner,
            player,
            logger,
        );
        run(episode, won, log).await
    }

    /// Run a game of the default table under `rules`, every player a model
    /// player asking `model`, made to call a tool, within the default
    /// window, except those in `puppets`.
    async fn play_with(
        rules: Rules,
        model: Arc<FakeModel>,
        puppets: HashMap<&str, Seat>,
    ) -> anyhow::Result<Played> {
        let required = ToolChoice::Required;
        play_as(five(), rules, model, puppets, required, CONTEXT_WINDOW).await
    }

    /// Every seat of the default table but `prompted` a player who never
    /// chooses.
    fn mute_but(prompted: &[&str]) -> HashMap<&'static str, Seat> {
        ["player1", "player2", "player3", "player4", "player5"]
            .into_iter()
            .filter(|seat| !prompted.contains(seat))
            .map(|seat| (seat, mute()))
            .collect()
    }

    /// A model answering every request with `answer` of it.
    fn answering(
        answer: impl Fn(&Request) -> anyhow::Result<Response> + Send + Sync + 'static,
    ) -> Arc<FakeModel> {
        Arc::new(FakeModel::answering(answer))
    }

    /// A tool call as a model makes it: the tool's name and its arguments.
    type Made = (String, serde_json::Value);

    /// A call to `speak` saying `text`.
    fn speaking(text: &str) -> Made {
        (SPEAK.to_string(), json!({"text": text}))
    }

    /// A call to `propose` naming `team`.
    fn proposing(team: &[&str]) -> Made {
        (PROPOSE.to_string(), json!({"team": team}))
    }

    /// A call to `vote`, approving or not.
    fn voting(approve: bool) -> Made {
        (VOTE.to_string(), json!({"approve": approve}))
    }

    /// A call to `perform` doing `deed`.
    fn performing(deed: &str) -> Made {
        (PERFORM.to_string(), json!({"deed": deed}))
    }

    /// A call to `select` naming `target`.
    fn selecting(target: &str) -> Made {
        (SELECT.to_string(), json!({"target": target}))
    }

    /// A call to `wait` for `seconds`.
    fn waiting(seconds: u64) -> Made {
        (WAIT.to_string(), json!({"seconds": seconds}))
    }

    /// Whether `request` offers the tool `name`.
    fn offers(request: &Request, name: &str) -> bool {
        request.tools.iter().any(|tool| tool.name == name)
    }

    /// The names of the tools `request` offers, in order.
    fn offered(request: &Request) -> Vec<&str> {
        request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect()
    }

    /// The messages `player` logged sending, each with everyone it was sent
    /// to, in the log's order.
    fn sent_by<'a>(logged: &'a [Entry], player: &str) -> Vec<(&'a Message, &'a HashSet<ActorId>)> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Action {
                    from, to, message, ..
                } if from == player => Some((message, to)),
                _ => None,
            })
            .collect()
    }

    /// What `player` logged withholding, in the log's order.
    fn withheld_by<'a>(logged: &'a [Entry], player: &str) -> Vec<&'a Message> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Withheld { from, message, .. } if from == player => Some(message),
                _ => None,
            })
            .collect()
    }

    /// The failures the players logged, each as who failed, in which
    /// phase, and what went wrong, in the log's order.
    fn failed(logged: &[Entry]) -> Vec<(&str, u64, &str)> {
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

    /// The omniscient account of `entries`, line by line.
    fn narrated(entries: Vec<Entry>) -> Vec<String> {
        let mut narrator = Narrator::default();
        entries
            .into_iter()
            .flat_map(|entry| narrator.narrate(&logged(entry)))
            .collect()
    }

    /// How the log says the game ended: who won and how.
    fn ending(logged: &[Entry]) -> (Team, Victory) {
        crate::avalon::tests::ending(logged)
    }

    #[tokio::test(start_paused = true)]
    async fn the_leader_s_request_tells_the_prompt_the_story_and_the_situation_and_offers_propose()
    {
        let model = answering(choosing_first);
        play_with(quick(), Arc::clone(&model), mute_but(&["player1"]))
            .await
            .unwrap();
        let seats = ["player1", "player2", "player3", "player4", "player5"];
        assert_eq!(
            model.requests()[0],
            Request::offering(
                MODEL_ID,
                vec![
                    model::Message::system("You are player1."),
                    model::Message::user(
                        "You are player1, Merlin. You see that player2 and player5 are evil.\n\
                         \n\
                         Quest 1, proposal 1 of 5. You lead and need a team of 2.\n\
                         \n\
                         It is quest 1, proposal 1 of 5, led by player1. The quest needs a team \
                         of 2, and 1 sabotage fails it. The proposal lasts 10s. You lead: propose \
                         a team of 2. Answer by calling one of the tools."
                    ),
                ],
                vec![
                    Tool::new(
                        "speak",
                        "Say something to every other player.",
                        json!({
                            "type": "object",
                            "properties": {"text": {"type": "string", "minLength": 1}},
                            "required": ["text"],
                            "additionalProperties": false,
                        }),
                    ),
                    Tool::new(
                        "propose",
                        "Propose the team to go on the quest. Your first proposal is final.",
                        json!({
                            "type": "object",
                            "properties": {"team": {
                                "type": "array",
                                "items": {"type": "string", "enum": seats},
                                "minItems": 2,
                                "maxItems": 2,
                                "uniqueItems": true,
                            }},
                            "required": ["team"],
                            "additionalProperties": false,
                        }),
                    ),
                    Tool::new(
                        "wait",
                        "Say nothing now, and be woken after this many seconds unless something wakes you sooner.",
                        json!({
                            "type": "object",
                            "properties": {"seconds": {"type": "integer", "minimum": 1, "maximum": 10}},
                            "required": ["seconds"],
                            "additionalProperties": false,
                        }),
                    ),
                ],
                ToolChoice::Required,
            )
        );
    }

    #[test]
    fn each_phase_waits_on_the_leader_everyone_the_team_or_the_assassin_for_its_one_tool() {
        let duty =
            |seat: &str, phase, team: &[&str]| Duty::of(&id(seat), &view_of(seat, phase, team));
        let seats: Vec<PlayerId> = ["player1", "player2", "player3", "player4", "player5"]
            .map(id)
            .into();
        // The leader proposes a team of the quest's size from every seat.
        assert_eq!(
            duty("player1", Phase::Propose, &[]),
            Some(Duty::Propose {
                size: 2,
                seats: seats.clone()
            })
        );
        assert_eq!(duty("player3", Phase::Propose, &[]), None);
        // Everyone votes.
        for seat in &seats {
            assert_eq!(duty(seat, Phase::Vote, &["player1"]), Some(Duty::Vote));
        }
        // A good member is offered only support, an evil one sabotage too,
        // and anyone else nothing.
        let team = ["player1", "player2"];
        assert_eq!(
            duty("player1", Phase::Quest, &team),
            Some(Duty::Perform {
                deeds: vec![Deed::Support]
            })
        );
        assert_eq!(
            duty("player2", Phase::Quest, &team),
            Some(Duty::Perform {
                deeds: vec![Deed::Support, Deed::Sabotage]
            })
        );
        assert_eq!(duty("player3", Phase::Quest, &team), None);
        // The Assassin names anyone but itself and the Minion it sees as
        // evil.
        assert_eq!(
            duty("player2", Phase::Assassinate, &[]),
            Some(Duty::Select {
                targets: ["player1", "player3", "player4"].map(id).into()
            })
        );
        assert_eq!(duty("player1", Phase::Assassinate, &[]), None);
        // Each duty's tool comes between speak and wait, and a player with
        // none is offered those two alone.
        let names = |duty: Option<&Duty>| -> Vec<String> {
            tools(duty, Duration::from_secs(60))
                .into_iter()
                .map(|tool| tool.name)
                .collect()
        };
        assert_eq!(names(None), [SPEAK, WAIT]);
        for (duty, tool) in [
            (Duty::Vote, VOTE),
            (
                Duty::Perform {
                    deeds: deeds(Role::Merlin),
                },
                PERFORM,
            ),
            (
                Duty::Select {
                    targets: vec![id("player1")],
                },
                SELECT,
            ),
        ] {
            assert_eq!(names(Some(&duty)), [SPEAK, tool, WAIT]);
        }
        assert_eq!(
            perform(&[Deed::Support]).parameters["properties"]["deed"]["enum"],
            json!(["support"])
        );
        assert_eq!(
            select(&[id("player3")]).parameters["properties"]["target"]["enum"],
            json!(["player3"])
        );
        assert_eq!(
            vote().parameters["properties"]["approve"],
            json!({"type": "boolean"})
        );
    }

    #[test]
    fn the_situation_tells_the_quest_the_team_how_long_the_phase_lasts_and_what_is_asked() {
        let limit = Duration::from_secs(90);
        let propose = view_of("player1", Phase::Propose, &[]);
        let opening = "It is quest 1, proposal 1 of 5, led by player1. \
                       The quest needs a team of 2, and 1 sabotage fails it.";
        assert_eq!(
            situation(&propose, limit, Duty::of(&id("player1"), &propose).as_ref()),
            format!("{opening} The proposal lasts 1m 30s. You lead: propose a team of 2.")
        );
        assert_eq!(
            situation(&propose, limit, None),
            format!("{opening} The proposal lasts 1m 30s. You may speak or wait.")
        );
        let vote = view_of("player4", Phase::Vote, &["player1", "player3"]);
        assert_eq!(
            situation(&vote, limit, Some(&Duty::Vote)),
            format!(
                "{opening} The team proposed is player1 and player3. The vote lasts 1m 30s. \
                 Vote to approve or reject the team."
            )
        );
        let mut quest = view_of("player2", Phase::Quest, &["player1", "player2", "player4"]);
        quest.sabotages_needed = 2;
        let member = |role| Duty::Perform { deeds: deeds(role) };
        assert_eq!(
            situation(&quest, limit, Some(&member(Role::Assassin))),
            "It is quest 1, proposal 1 of 5, led by player1. The quest needs a team of 2, \
             and 2 sabotages fail it. The team proposed is player1, player2 and player4. \
             The quest lasts 1m 30s. You are on the quest: support or sabotage it."
        );
        assert!(
            situation(&quest, limit, Some(&member(Role::Percival)))
                .ends_with("You are on the quest: support it.")
        );
        let assassinate = view_of("player2", Phase::Assassinate, &[]);
        let targets = Duty::Select { targets: vec![] };
        assert!(
            situation(&assassinate, limit, Some(&targets)).ends_with(
                "The assassination lasts 1m 30s. Name the player you believe is Merlin."
            )
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_of_model_players_runs_to_the_end_each_message_going_where_the_rules_send_it() {
        // Every player plays as the canned player does: the leader proposes
        // the first two seats, everyone approves, and every member supports,
        // so three quests succeed, and the Assassin names the first player
        // it does not see as evil, who is Merlin.
        let model = answering(choosing_first);
        let (winner, logged, _) = play_with(Rules::default(), Arc::clone(&model), HashMap::new())
            .await
            .unwrap();
        assert_eq!(winner, Team::Evil);
        assert_eq!(ending(&logged), (Team::Evil, Victory::Assassination));
        assert!(failed(&logged).is_empty(), "{logged:?}");
        let seats: Vec<PlayerId> = ["player1", "player2", "player3", "player4", "player5"]
            .map(id)
            .into();
        // Talk goes to every other player and each choice to the
        // environment alone, and each was made by a model shown its
        // memories.
        let mut said = 0;
        let mut chosen = HashMap::new();
        for entry in &logged {
            if let Entry::Action {
                from,
                to,
                message,
                seen,
                ..
            } = entry
                && from != ENVIRONMENT
            {
                assert!(seen.is_some(), "{entry:?}");
                match message.choice() {
                    Some((_, _, choice)) => {
                        assert_eq!(to, &to_the_environment());
                        *chosen.entry(choice).or_insert(0) += 1;
                    }
                    None => {
                        assert_eq!(to, &to_the_players_but(from, &seats));
                        said += 1;
                    }
                }
            }
        }
        assert!(said >= 5 * 7, "{said}");
        let team = Choice::Team(["player1", "player2"].map(id).into());
        assert!(chosen[&team] >= 1, "{chosen:?}");
        assert_eq!(chosen[&Choice::Ballot(Ballot::Approve)], 15);
        assert_eq!(chosen[&Choice::Deed(Deed::Support)], 7);
        assert_eq!(chosen[&Choice::Target(id("player1"))], 1);
        // Each player was asked what each phase is, as it sees it.
        let situations: HashSet<String> = model
            .requests()
            .iter()
            .map(|request| situated(&request.messages[1].content).to_string())
            .collect();
        for line in [
            "You lead: propose a team of 2.",
            "Vote to approve or reject the team.",
            "You are on the quest: support it.",
            "You are on the quest: support or sabotage it.",
            "Name the player you believe is Merlin.",
            "You may speak or wait.",
        ] {
            assert!(
                situations.iter().any(|situation| situation.ends_with(line)),
                "{line}: {situations:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_is_told_the_game_from_its_own_point_of_view() {
        let model = answering(choosing_first);
        play_with(Rules::default(), Arc::clone(&model), HashMap::new())
            .await
            .unwrap();
        // The Assassin's last request, made as it names Merlin.
        let asked = asked_of(&model, "player2");
        let last = story(asked.last().unwrap());
        let lines: Vec<&str> = last.lines().collect();
        assert_eq!(
            lines[0],
            "You are player2, the Assassin. You see that player5 is evil."
        );
        for told in [
            "Quest 1, proposal 1 of 5. player1 leads and needs a team of 2.",
            "player1 says, \"I have nothing to add.\"",
            "Everyone votes on the team of player1 and player2.",
            "You approve the team.",
            "player1, player2, player3, player4 and player5 approve.",
            "The team goes on the quest.",
            "player1 and player2 go on quest 1, which 1 sabotage fails.",
            "You support the quest.",
            "The quest succeeds, with no sabotage.",
            "Quest 2, proposal 1 of 5. You lead and need a team of 3.",
            "You propose player1, player2 and player3.",
            "The Assassin tries to name Merlin.",
        ] {
            assert!(lines.contains(&told), "{told}: {last}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_the_phase_does_not_wait_on_is_offered_only_speak_and_wait() {
        let model = answering(choosing_first);
        play_with(quick(), Arc::clone(&model), mute_but(&["player3"]))
            .await
            .unwrap();
        let requests = model.requests();
        assert_eq!(offered(&requests[0]), [SPEAK, WAIT]);
        assert!(situated(&requests[0].messages[1].content).ends_with("You may speak or wait."));
    }

    /// A leader who proposes `team` at once, for every proposal it leads,
    /// and does nothing else.
    fn proposing_at_once(team: &'static [&'static str]) -> Seat {
        let script: Script = Box::new(move |me, seq, view| match view.phase {
            Phase::Propose if view.leader == *me => {
                let team = team.iter().map(|player| id(player)).collect();
                vec![Choice::Team(team).message(seq, me.clone(), seq)]
            }
            _ => vec![],
        });
        crate::avalon::Actor::puppet(script)
    }

    #[tokio::test(start_paused = true)]
    async fn a_second_vote_is_sent_and_only_the_first_counts() {
        // player1 proposes itself and player3, and player3 votes to approve
        // and then to reject, in one response, and waits after that.
        let model = answering(|request| {
            let calls = match offers(request, VOTE) {
                true if !request.messages[1].content.contains("You approve") => {
                    vec![voting(true), voting(false)]
                }
                _ => vec![waiting(10)],
            };
            Ok(Response::calling(&calls))
        });
        let mut puppets = mute_but(&["player3"]);
        puppets.insert("player1", proposing_at_once(&["player1", "player3"]));
        let (_, logged, _) = play_with(quick(), model, puppets).await.unwrap();
        let votes: Vec<&Message> = sent_by(&logged, "player3")
            .into_iter()
            .filter(|(message, to)| {
                assert_eq!(*to, &to_the_environment());
                matches!(message, Message::Vote { seq: 2, .. })
            })
            .map(|(message, _)| message)
            .collect();
        let vote = |n, ballot| Choice::Ballot(ballot).message(2, id("player3"), n);
        assert_eq!(votes, [&vote(1, Ballot::Approve), &vote(2, Ballot::Reject)]);
        let tally = logged
            .iter()
            .find_map(|entry| match entry {
                Entry::Tallied { seq: 2, tally } => Some(tally),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            tally,
            &Tally::of(HashMap::from([(id("player3"), Ballot::Approve)]), 5)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn each_choice_goes_to_the_environment_alone_and_talk_to_every_other_player() {
        // player2, the Assassin, proposes as leader of the second proposal,
        // after player1 proposes nobody, puts itself on its team, approves
        // it with player3 and player4, and sabotages the quest.
        let model = answering(|request| {
            let calls = if offers(request, PROPOSE) {
                vec![
                    speaking("Me and player4."),
                    proposing(&["player2", "player4"]),
                ]
            } else if offers(request, PERFORM) {
                vec![speaking("Done."), performing("sabotage")]
            } else if offers(request, VOTE) {
                vec![speaking("Yes."), voting(true)]
            } else {
                vec![speaking("Hm.")]
            };
            Ok(Response::calling(&calls))
        });
        let approver = || {
            crate::avalon::Actor::puppet(Box::new(|me, seq, view| match view.phase {
                Phase::Vote => vec![Choice::Ballot(Ballot::Approve).message(seq, me.clone(), seq)],
                _ => vec![],
            }))
        };
        let puppets = HashMap::from([
            ("player1", mute()),
            ("player3", approver()),
            ("player4", approver()),
            ("player5", mute()),
        ]);
        let (_, logged, _) = play_with(quick(), model, puppets).await.unwrap();
        let seats: Vec<PlayerId> = ["player1", "player2", "player3", "player4", "player5"]
            .map(id)
            .into();
        let others = to_the_players_but(&id("player2"), &seats);
        let sent = sent_by(&logged, "player2");
        let proposal = sent
            .iter()
            .position(|(message, _)| matches!(message, Message::Propose { .. }))
            .unwrap();
        assert_eq!(
            sent[proposal - 1],
            (
                &utterance(
                    2,
                    "player2",
                    sent[proposal - 1].0.number().unwrap(),
                    "Me and player4."
                ),
                &others
            )
        );
        assert_eq!(sent[proposal].1, &to_the_environment());
        let sabotaged = sent
            .iter()
            .find(|(message, _)| matches!(message, Message::Perform { .. }))
            .unwrap();
        let deed = sabotaged.0.choice().map(|(_, _, choice)| choice);
        assert_eq!(deed, Some(Choice::Deed(Deed::Sabotage)));
        assert_eq!(sabotaged.1, &to_the_environment());
        // Its utterances reach the other players, who observe them, and
        // its choices the environment alone.
        let n = sent[proposal - 1].0.number().unwrap();
        assert!(observed_by(&logged, "player3").contains(&("player2", n, 2)));
        let environment = observed_by(&logged, ENVIRONMENT);
        assert!(
            environment.contains(&("player2", n + 1, 2)),
            "{environment:?}"
        );
        assert!(!environment.contains(&("player2", n, 2)), "{environment:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_response_with_one_bad_call_is_a_failure_and_nothing_in_it_is_carried_out() {
        // The leader speaks and then proposes a team of the wrong size.
        let model = answering(|request| {
            let calls = match offers(request, PROPOSE) {
                true => vec![speaking("Trust me."), proposing(&["player1"])],
                false => vec![waiting(10)],
            };
            Ok(Response::calling(&calls))
        });
        let (winner, logged, took) = play_with(quick(), Arc::clone(&model), mute_but(&["player1"]))
            .await
            .unwrap();
        assert_eq!(winner, Team::Evil);
        // Five proposals lapse.
        assert_eq!(took, quick().propose_limit * 5);
        assert_eq!(
            failed(&logged)[0],
            (
                "player1",
                1,
                "the model proposed a team of 1, and the quest needs 2"
            )
        );
        assert!(sent_by(&logged, "player1").is_empty(), "{logged:?}");
        let told = narrated(logged);
        assert!(
            told.contains(
                &"player1 (Merlin) fails to play: the model proposed a team of 1, and the quest needs 2"
                    .to_string()
            ),
            "{told:?}"
        );
    }

    #[test]
    fn a_response_is_judged_as_a_whole_and_its_calls_kept_in_order() {
        let seats: Vec<PlayerId> = ["player1", "player2", "player3"].map(id).into();
        let limit = Duration::from_secs(60);
        let judged = |calls: &[Made], duty: Option<&Duty>| {
            super::asked(&Response::calling(calls), duty, limit)
        };
        let propose = Duty::Propose {
            size: 2,
            seats: seats.clone(),
        };
        let say = |text: &str| Sending::Say(text.to_string());
        let choose = Sending::Choose;
        assert_eq!(
            judged(
                &[
                    speaking("Hi."),
                    proposing(&["player3", "player1"]),
                    waiting(60)
                ],
                Some(&propose)
            )
            .unwrap(),
            Asked {
                sends: vec![
                    say("Hi."),
                    choose(Choice::Team(["player3", "player1"].map(id).into()))
                ],
                wait: Some(Duration::from_secs(60)),
            }
        );
        // Of several waits the last counts, and a choice may be made twice.
        assert_eq!(
            judged(
                &[voting(false), waiting(5), voting(true), waiting(1)],
                Some(&Duty::Vote)
            )
            .unwrap(),
            Asked {
                sends: vec![
                    choose(Choice::Ballot(Ballot::Reject)),
                    choose(Choice::Ballot(Ballot::Approve))
                ],
                wait: Some(Duration::from_secs(1)),
            }
        );
        let evil = Duty::Perform {
            deeds: deeds(Role::Morgana),
        };
        assert_eq!(
            judged(&[performing("sabotage")], Some(&evil))
                .unwrap()
                .sends,
            [choose(Choice::Deed(Deed::Sabotage))]
        );
        let assassin = Duty::Select {
            targets: seats[..2].to_vec(),
        };
        assert_eq!(
            judged(&[selecting("player2")], Some(&assassin))
                .unwrap()
                .sends,
            [choose(Choice::Target(id("player2")))]
        );
        let good = Duty::Perform {
            deeds: deeds(Role::Servant),
        };
        let wrong: Vec<(&str, Vec<Made>, Option<&Duty>, &str)> = vec![
            ("no tool call", vec![], None, "the model made no tool call"),
            (
                "a tool not offered",
                vec![voting(true)],
                None,
                "the model called vote, which is not a tool it was offered",
            ),
            (
                "another phase's tool",
                vec![proposing(&["player1", "player2"])],
                Some(&Duty::Vote),
                "the model called propose, which is not a tool it was offered",
            ),
            (
                "too small a team",
                vec![proposing(&["player1"])],
                Some(&propose),
                "the model proposed a team of 1, and the quest needs 2",
            ),
            (
                "the same player twice",
                vec![proposing(&["player2", "player2"])],
                Some(&propose),
                "the model proposed player2 twice",
            ),
            (
                "a player not at the table",
                vec![proposing(&["player1", "player9"])],
                Some(&propose),
                "the model proposed player9, who is not at the table",
            ),
            (
                "a team that is not a list",
                vec![(PROPOSE.to_string(), json!({"team": "player1"}))],
                Some(&propose),
                "the arguments of the model's propose call",
            ),
            (
                "a vote that is not a boolean",
                vec![(VOTE.to_string(), json!({"approve": "yes"}))],
                Some(&Duty::Vote),
                "the arguments of the model's vote call",
            ),
            (
                "a deed not offered",
                vec![performing("sabotage")],
                Some(&good),
                "the model chose to sabotage, which is not a deed it was offered",
            ),
            (
                "a deed that is no deed",
                vec![performing("flee")],
                Some(&evil),
                "the model chose to flee, which is not a deed it was offered",
            ),
            (
                "a target not a candidate",
                vec![selecting("player3")],
                Some(&assassin),
                "the model named player3, who is not a candidate",
            ),
            (
                "a blank utterance",
                vec![speaking(" \n")],
                None,
                "the model's speak call says nothing",
            ),
            (
                "no text",
                vec![(SPEAK.to_string(), json!({}))],
                None,
                "the arguments of the model's speak call",
            ),
            (
                "too long a wait",
                vec![speaking("Hi."), waiting(61)],
                None,
                "the model asked to wait 61s, which is not between 1s and 1m",
            ),
            (
                "too short a wait",
                vec![waiting(0)],
                None,
                "the model asked to wait 0s, which is not between 1s and 1m",
            ),
            (
                "a wait that is not a number",
                vec![(WAIT.to_string(), json!({"seconds": "soon"}))],
                None,
                "the arguments of the model's wait call",
            ),
            (
                "a selection with no target",
                vec![(SELECT.to_string(), json!({}))],
                Some(&assassin),
                "the arguments of the model's select call",
            ),
            (
                "a deed with no deed",
                vec![(PERFORM.to_string(), json!({}))],
                Some(&evil),
                "the arguments of the model's perform call",
            ),
        ];
        for (what, calls, duty, said) in wrong {
            let error = judged(&calls, duty).unwrap_err();
            assert!(format!("{error:#}").contains(said), "{what}: {error:#}");
        }
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

    #[tokio::test(start_paused = true)]
    async fn a_response_that_comes_after_the_phase_ended_is_withheld_and_logged_as_such() {
        // The leader's model takes fifteen seconds, and each proposal lapses
        // after ten, so nothing it answers is sent.
        let model = Arc::new(FakeModel::answering(choosing_first).taking(Duration::from_secs(15)));
        let (winner, logged, took) = play_with(quick(), model, mute_but(&["player1"]))
            .await
            .unwrap();
        assert_eq!(winner, Team::Evil);
        assert_eq!(took, quick().propose_limit * 5);
        assert!(sent_by(&logged, "player1").is_empty(), "{logged:?}");
        let withheld = withheld_by(&logged, "player1");
        let team = Choice::Team(["player1", "player2"].map(id).into());
        assert_eq!(
            withheld[..2],
            [
                &utterance(1, "player1", 0, SAID),
                &team.clone().message(1, id("player1"), 0)
            ]
        );
        let told = narrated(logged);
        for line in [
            format!("player1 (Merlin) is too late to say, for proposal 1 of quest 1: \"{SAID}\""),
            "player1 (Merlin) is too late to propose player1 (Merlin) and player2 (Assassin) for proposal 1 of quest 1."
                .to_string(),
        ] {
            assert!(told.contains(&line), "{line}: {told:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn messages_heard_during_a_call_lead_to_one_more_call_and_another_phase_s_to_none() {
        // player3 takes two seconds to answer and waits the phase out.
        // player4 says five things a second into each phase, while
        // player3's first call is in progress: they wake player3 once
        // more, and that call is told them all. Into the second phase it
        // says something tagged with the first, which player3 observes and
        // neither remembers nor wakes for.
        let model = Arc::new(
            FakeModel::answering(|_| Ok(Response::calling(&[waiting(10)])))
                .taking(Duration::from_secs(2)),
        );
        let talker: Script = Box::new(|me, seq, _| match seq {
            1 => (1..=5)
                .map(|i| utterance(seq, me, i, &format!("Point {i}.")))
                .collect(),
            _ => vec![utterance(seq - 1, me, 10 + seq, "Old talk.")],
        });
        let mut puppets = mute_but(&["player3"]);
        puppets.insert(
            "player4",
            crate::avalon::Actor::puppet_after(Duration::from_secs(1), talker),
        );
        let (_, logged, _) = play_with(quick(), Arc::clone(&model), puppets)
            .await
            .unwrap();
        let asked = asked_of(&model, "player3");
        let in_phase = |proposal: u8| -> Vec<&String> {
            asked
                .iter()
                .filter(|asked| situated(asked).contains(&format!("proposal {proposal} of 5")))
                .collect()
        };
        let first = in_phase(1);
        assert_eq!(first.len(), 2, "{asked:?}");
        assert!(!first[0].contains("player4 says"), "{}", first[0]);
        for i in 1..=5 {
            let heard = format!("player4 says, \"Point {i}.\"");
            assert!(story(first[1]).contains(&heard), "{}", first[1]);
        }
        assert_eq!(in_phase(2).len(), 1, "{asked:?}");
        assert!(asked.iter().all(|asked| !asked.contains("Old talk.")));
        let observed = observed_by(&logged, "player3");
        assert!(observed.contains(&("player4", 12, 2)), "{observed:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_wait_wakes_the_player_when_it_is_up_unless_a_later_wait_or_the_end_of_the_phase_came_first()
     {
        // player3 asks to wait five seconds and then two, of which only the
        // second counts; woken by it, it asks to wait nine, which is after
        // the proposal lapses, so it is not woken again until the next one.
        // After that it speaks whenever it is woken.
        let answers = [vec![waiting(5), waiting(2)], vec![waiting(9)]];
        let calls = AtomicUsize::new(0);
        let model = answering(move |_| {
            let call = calls.fetch_add(1, Ordering::Relaxed);
            let answer = answers.get(call).cloned();
            Ok(Response::calling(
                &answer.unwrap_or_else(|| vec![speaking("Hm.")]),
            ))
        });
        let (_, logged, _) = play_with(quick(), Arc::clone(&model), mute_but(&["player3"]))
            .await
            .unwrap();
        assert!(failed(&logged).is_empty(), "{logged:?}");
        let asked = asked_of(&model, "player3");
        let in_first = asked
            .iter()
            .filter(|asked| situated(asked).contains("proposal 1 of 5"))
            .count();
        assert_eq!(in_first, 2, "{asked:?}");
        // One request a proposal after that, and five proposals lapse.
        assert_eq!(asked.len(), 6, "{asked:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_whose_model_always_fails_is_told_failure_by_failure_and_still_ends() {
        let model = Arc::new(FakeModel::failing("the model is down"));
        let (winner, logged, took) = play_with(quick(), model, HashMap::new()).await.unwrap();
        assert_eq!(winner, Team::Evil);
        assert_eq!(ending(&logged), (Team::Evil, Victory::Rejections));
        assert_eq!(took, quick().propose_limit * 5);
        assert_eq!(failed(&logged).len(), 25);
        let told = narrated(logged);
        assert!(
            told.contains(&"player3 (Servant) fails to play: the model is down".to_string()),
            "{told:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn allowed_a_tool_call_a_player_that_answers_in_prose_fails_and_the_game_goes_on() {
        let prose = json!({"choices": [{"message": {"content": "Hello."}}]});
        let model = Arc::new(FakeModel::responding(
            serde_json::from_value(prose).unwrap(),
        ));
        let (_, logged, _) = play_as(
            five(),
            quick(),
            Arc::clone(&model),
            mute_but(&["player1"]),
            ToolChoice::Auto,
            CONTEXT_WINDOW,
        )
        .await
        .unwrap();
        assert_eq!(
            failed(&logged)[0],
            ("player1", 1, "the model made no tool call")
        );
        let requests = model.requests();
        assert!(!requests.is_empty());
        assert!(
            requests
                .iter()
                .all(|request| request.tool_choice == ToolChoice::Auto)
        );
    }

    /// How many lines of talk each of the players' actions says the model
    /// forgot, in the log's order.
    fn forgotten(logged: &[Entry]) -> Vec<Option<usize>> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Action { from, forgot, .. } if from != ENVIRONMENT => Some(*forgot),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_in_a_small_window_plays_to_the_end_its_later_requests_forgetting_talk() {
        // Everyone plays as the canned player does, at length.
        let model = answering(|request| {
            let calls: Vec<Made> = canned_calls(&serde_json::to_value(request)?)?
                .into_iter()
                .map(|(name, arguments)| match name.as_str() {
                    SPEAK => speaking(&SAID.repeat(10)),
                    _ => (name, arguments),
                })
                .collect();
            Ok(Response::calling(&calls))
        });
        let window = 2000;
        let (winner, logged, _) = play_as(
            five(),
            quick(),
            Arc::clone(&model),
            HashMap::new(),
            ToolChoice::Required,
            window,
        )
        .await
        .unwrap();
        assert_eq!(winner, Team::Evil);
        for request in model.requests() {
            assert!(model::estimated(&request) <= window, "{request:?}");
        }
        let asked = asked_of(&model, "player2");
        assert!(!asked[0].contains(FORGOTTEN), "{asked:?}");
        assert!(asked.last().unwrap().contains(FORGOTTEN), "{asked:?}");
        let forgot = forgotten(&logged);
        assert_eq!(forgot[0], None);
        assert!(
            forgot.iter().any(|forgot| forgot.is_some_and(|n| n > 0)),
            "{forgot:?}"
        );
        // Within the default window, nothing is forgotten.
        let model = answering(choosing_first);
        let (_, logged, _) = play_with(quick(), Arc::clone(&model), HashMap::new())
            .await
            .unwrap();
        assert!(forgotten(&logged).iter().all(Option::is_none));
    }

    /// A player's memory of sending `message`, in no episode in particular.
    fn sent(message: Message) -> Event<Memory> {
        logged(Memory::Acted { message })
    }

    /// A player's memory of receiving `message`, in no episode in
    /// particular.
    fn received(message: Message) -> Event<Memory> {
        logged(Memory::Observed { message })
    }

    #[test]
    fn the_story_kept_with_the_history_is_at_every_step_a_fresh_narration_of_it() {
        let mind = Mind::new(
            id("player3"),
            told("player3"),
            MODEL_ID.to_string(),
            ToolChoice::Required,
            CONTEXT_WINDOW,
            Arc::new(FakeModel::failing("down")),
        );
        let announced = |seq, phase, team: &[&str]| {
            crate::avalon::tests::announcement(seq, view_of("player3", phase, team))
        };
        let script = [
            received(announced(1, Phase::Propose, &[])),
            received(utterance(1, "player1", 1, "Me and you.")),
            sent(utterance(1, "player3", 1, "Fine.")),
            received(announced(2, Phase::Vote, &["player1", "player3"])),
            sent(Choice::Ballot(Ballot::Approve).message(2, id("player3"), 2)),
        ];
        for (n, event) in script.iter().enumerate() {
            mind.remember(event.clone());
            let remembered = mind.remembered.lock().unwrap();
            assert_eq!(remembered.history, script[..=n]);
            let mut fresh = Story::of(id("player3"));
            for event in &remembered.history {
                fresh.recall(event);
            }
            assert_eq!(remembered.story.lines(), fresh.lines());
        }
        assert_eq!(
            mind.story(u64::MAX),
            (
                [
                    "You are player3, a loyal servant of Arthur.",
                    "",
                    "Quest 1, proposal 1 of 5. player1 leads and needs a team of 2.",
                    "player1 says, \"Me and you.\"",
                    "You say, \"Fine.\"",
                    "",
                    "Everyone votes on the team of player1 and player3.",
                    "You approve the team.",
                ]
                .join("\n"),
                Shown {
                    seen: script.len(),
                    forgot: None,
                }
            )
        );
    }

    #[tokio::test]
    async fn the_probe_asks_as_the_player_each_phase_waits_on_and_passes_when_the_model_plays() {
        let model = FakeModel::answering(choosing_first);
        let probed = check_plays(&model, MODEL_ID).await;
        assert!(
            matches!(probed, Ok(Probed::Played(ToolChoice::Required))),
            "{probed:?}"
        );
        assert!(not_playing(probed).is_none());
        let requests = model.requests();
        let offered: Vec<Vec<&str>> = requests.iter().map(offered).collect();
        assert_eq!(
            offered,
            [
                [SPEAK, PROPOSE, WAIT],
                [SPEAK, VOTE, WAIT],
                [SPEAK, PERFORM, WAIT],
                [SPEAK, SELECT, WAIT]
            ]
        );
        for (request, (_, duty)) in requests.iter().zip(probes()) {
            assert_eq!(request.model, MODEL_ID);
            assert_eq!(request.tool_choice, ToolChoice::Required);
            assert_eq!(request.messages[0], model::Message::system(PROBE_PROMPT));
            assert_eq!(
                request.messages[1].content,
                format!("{} {ANSWER}", probe_asked(&duty))
            );
            assert_eq!(request.tools, tools(Some(&duty), PROBE_LIMIT));
        }
        assert_eq!(
            requests[0].messages[1].content,
            format!("The proposal lasts 1m. You lead: propose a team of 2. {ANSWER}")
        );
        assert_eq!(
            requests[2].tools[1],
            perform(&[Deed::Support, Deed::Sabotage])
        );
    }

    /// The error of a probe that found the model did not play, if it found
    /// so.
    fn not_playing(probed: anyhow::Result<Probed>) -> Option<anyhow::Error> {
        match probed.unwrap() {
            Probed::Played(_) => None,
            Probed::DidNotPlay(error) => Some(error),
        }
    }

    #[tokio::test]
    async fn the_probe_names_whom_the_model_did_not_answer_as_and_asks_nothing_after() {
        for (fails, who, asked) in [
            (PROPOSE, "the leader", 1),
            (VOTE, "a voter", 2),
            (PERFORM, "an evil member of a team", 3),
            (SELECT, "the Assassin", 4),
        ] {
            let model = FakeModel::answering(move |request| match offers(request, fails) {
                true => Ok(Response::calling(&[speaking(" ")])),
                false => choosing_first(request),
            });
            let error = not_playing(check_plays(&model, MODEL_ID).await).unwrap();
            assert_eq!(
                format!("{error:#}"),
                format!("{MODEL_ID} did not answer as {who}: the model's speak call says nothing")
            );
            assert_eq!(model.requests().len(), asked, "{who}");
        }
    }

    #[tokio::test]
    async fn the_probe_allows_a_tool_call_when_the_model_refuses_to_be_made_to_call_one() {
        let base_url = canned::refusing_required(&[MODEL_ID]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        assert!(matches!(
            check_plays(&provider, MODEL_ID).await.unwrap(),
            Probed::Played(ToolChoice::Auto)
        ));
    }

    #[tokio::test]
    async fn the_probe_gives_both_refusals_when_the_first_request_is_refused_either_way() {
        let base_url = canned::refusing(&[MODEL_ID]);
        let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
        let error = not_playing(check_plays(&provider, MODEL_ID).await).unwrap();
        let refused = format!(
            "POST {base_url}/chat/completions was answered 400 Bad Request. \
             The provider said: {}",
            canned::refusal(MODEL_ID)
        );
        assert_eq!(
            format!("{error:#}"),
            format!(
                "{MODEL_ID} did not answer as the leader: \
                 made to call a tool: {refused}\nasked to call one: {refused}"
            )
        );
    }

    #[tokio::test]
    async fn the_probe_tells_a_model_that_could_not_be_asked_from_one_that_did_not_play() {
        for base_url in [
            canned::refusing_required_then_busy(&[MODEL_ID]),
            canned::answering("429 Too Many Requests", "{}").0,
        ] {
            let provider = Provider::new(&base_url, None, REQUEST_TIMEOUT).unwrap();
            let error = check_plays(&provider, MODEL_ID).await.unwrap_err();
            let shown = format!("{error:#}");
            assert!(
                shown.starts_with(&format!(
                    "{MODEL_ID} could not be asked to play: POST {base_url}/chat/completions \
                     was answered 429 Too Many Requests"
                )),
                "{shown}"
            );
        }
    }

    #[test]
    fn the_request_before_its_story_is_the_largest_of_the_four_phases() {
        let seats: Vec<PlayerId> = (1..=10).map(|seat| format!("player{seat}")).collect();
        let rules = Rules::default();
        let limit = Duration::from_secs(60);
        let me = id("player1");
        let needed = unstoried(MODEL_ID, "Hi.", &me, Role::Assassin, &seats, &rules);
        let view = View {
            team: None,
            ..view_of("player1", Phase::Propose, &[])
        };
        let proposing = request(
            MODEL_ID,
            ToolChoice::Required,
            "Hi.",
            situation(&view, limit, None),
            tools(None, limit),
        );
        assert!(needed > model::estimated(&proposing));
        // A longer prompt takes as much more.
        let longer = unstoried(
            MODEL_ID,
            &"x".repeat(300),
            &me,
            Role::Assassin,
            &seats,
            &rules,
        );
        assert_eq!(longer, needed + 99);
    }
}
