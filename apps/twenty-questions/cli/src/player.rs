//! The prompted player: a player whose decisions a language model makes.
//!
//! A player remembers what it hears and what it does, as a [`Memory`] of
//! each message stamped when it was received or sent, and keeps with the
//! history the [`Story`] that tells it from its own point of view. Every
//! message it hears wakes it. Each time it is woken, it tells the model the
//! story so far and where the game stands, with how many guesses are left
//! and how much time, and carries out what the model answers: it may
//! `speak`, which goes to everyone else, `guess`, which puts a question
//! formally to the host and goes to everyone else too, and `wait`, which
//! asks to be woken after some seconds unless something wakes it sooner,
//! in any number and order. The request fits the context window by
//! estimate: when the story would not, the oldest talk is forgotten for
//! that request. The call is made in the player's think loop, so the
//! player keeps perceiving while the model works.
//!
//! Wake-ups are debounced: the perceive loop remembers each message and
//! wakes the think loop only when no wake-up is already waiting, and the
//! think loop takes the history as it stands when it is woken, so a player
//! has at most one call in progress and one waiting, however fast the talk
//! comes. A reminder is numbered so that only the latest set counts.
//!
//! A model that is not reached, or an answer that is not well formed, is a
//! failure: the player logs it as [`Entry::Failed`], carries nothing out,
//! and the game goes on.

use crate::game::{Actor, Seated, act, everyone_but, observe};
use crate::log::Entry;
use crate::message::{HOST, Message, View};
use crate::model::{
    Asker, SPEAK, Shown, WAIT, arguments, calls, request, said, speak, text_parameter, wait, waited,
};
use crate::story::{Memory, Story};
use anyhow::{Context as _, bail};
use async_trait::async_trait;
use free_agent::{Behavior, Context, Event, Think};
use free_agent_game::report::{in_prose, whole_seconds};
use free_agent_game::seat::PlayerId;
use free_agent_model::{Request, Response, Tool, ToolChoice};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

/// The name of the tool a player guesses with.
pub(crate) const GUESS: &str = "guess";

/// The system prompt the probe tells the model.
const PROBE_PROMPT: &str =
    "You are a player in a game of Twenty Questions, about to be asked to play.";

/// The tool a player guesses with: `guess`, taking a `question`.
fn guess() -> Tool {
    Tool::new(
        GUESS,
        "Put a yes-or-no question formally to the host, which uses one of the guesses you share.",
        text_parameter("question"),
    )
}

/// The tools a player is offered in a game that lasts `limit`: [`speak`],
/// [`guess`] when it `may_guess`, and [`wait`].
fn tools(may_guess: bool, limit: Duration) -> Vec<Tool> {
    std::iter::once(speak("Say something to everyone at the table."))
        .chain(may_guess.then(guess))
        .chain([wait(limit)])
        .collect()
}

/// Where the guesses stand, as a player knows it.
#[derive(Debug, Clone, PartialEq)]
struct Guesses {
    /// How many the players have.
    of: u64,
    /// How many are left: all but those the player has heard of, its own
    /// among them.
    left: u64,
    /// Those waiting for the host's answer, each as who asked it and the
    /// question.
    waiting: Vec<(PlayerId, String)>,
    /// Whether a player has spoken since the last answer, or since the game
    /// began.
    talked: bool,
}

impl Guesses {
    /// All `of` the guesses, none put yet, after talk.
    fn fresh(of: u64) -> Self {
        Guesses {
            of,
            left: of,
            waiting: Vec::new(),
            talked: true,
        }
    }

    /// Whether the player may guess: while guesses are left and none is
    /// waiting, once someone has spoken since the last answer, so that a
    /// guess is proposed before it is made.
    fn may_guess(&self) -> bool {
        self.left > 0 && self.waiting.is_empty() && self.talked
    }

    /// Where the game stands, with `time_left`: the guesses waiting, how
    /// many are left, how much time is left, and what the player may do.
    fn situation(&self, time_left: Duration) -> String {
        let mut said = String::new();
        if !self.waiting.is_empty() {
            let asked = self
                .waiting
                .iter()
                .map(|(asker, question)| format!("{asker}'s \"{question}\""));
            said.push_str(&format!(
                "Waiting for the host's answer: {}. ",
                in_prose(asked)
            ));
        }
        said.push_str(&match self.left {
            0 => "No guesses are left.".to_string(),
            left => format!("Guesses left: {left} of {}.", self.of),
        });
        said.push_str(&format!(" Time left: {}. ", whole_seconds(time_left)));
        if self.left > 0 && self.waiting.is_empty() && !self.talked {
            said.push_str("Nobody has proposed the next guess yet. ");
        }
        said.push_str(match self.may_guess() {
            true => "You may speak, put a guess to the host, or wait.",
            false => "You may speak or wait.",
        });
        said
    }
}

/// The most a request to the model `model_id` from a player told `prompt`
/// takes of the context window before its story, by estimate, in a game of
/// `guesses` that lasts `limit`.
pub(crate) fn unstoried(model_id: &str, prompt: &str, guesses: u64, limit: Duration) -> u64 {
    let asked = format!("\n\n{}", Guesses::fresh(guesses).situation(limit));
    let request = request(
        model_id,
        ToolChoice::Required,
        prompt,
        asked,
        tools(true, limit),
    );
    free_agent_model::estimated(&request)
}

/// How long a game the probe asks about lasts.
const PROBE_LIMIT: Duration = Duration::from_secs(600);

/// The request the probe asks the model `model_id` as a player, under
/// `tool_choice`: a player's, with a short system prompt, at the start of
/// a game of twenty guesses.
pub(crate) fn probe(model_id: &str, tool_choice: ToolChoice) -> Request {
    request(
        model_id,
        tool_choice,
        PROBE_PROMPT,
        Guesses::fresh(20).situation(PROBE_LIMIT),
        tools(true, PROBE_LIMIT),
    )
}

/// Judge `response` to the probe's request as a player's answer is
/// judged.
pub(crate) fn probed(response: &Response) -> anyhow::Result<()> {
    asked(response, true, PROBE_LIMIT).map(drop)
}

/// The arguments of a [`guess`] call.
#[derive(Deserialize)]
struct Question {
    question: String,
}

/// `question` as guesses are told apart: trimmed and in lower case.
fn folded(question: &str) -> String {
    question.trim().to_lowercase()
}

/// What a call has the player send.
#[derive(Debug, PartialEq)]
enum Sending {
    /// Say `text` to everyone.
    Say(String),
    /// Put the question to the host, before everyone.
    Guess(String),
}

impl Sending {
    /// The message `from` sends for this, numbered `n`.
    fn message(self, from: &PlayerId, n: u64) -> Message {
        let from = from.clone();
        match self {
            Sending::Say(text) => Message::Say { from, n, text },
            Sending::Guess(text) => Message::Guess { from, n, text },
        }
    }
}

/// What a response asks for: the messages to send, in the order the model
/// made the calls, and how long to wait, if it asked to. Of several waits,
/// the last counts.
#[derive(Debug, Default, PartialEq)]
struct Asked {
    sends: Vec<Sending>,
    wait: Option<Duration>,
}

/// What `response` asks for, its calls taken in the order the model made
/// them, from a player that `may_guess` in a game that lasts `limit`. The
/// response is judged as a whole: no tool call, a call to a tool not
/// offered, arguments that do not parse, text or a question that is empty
/// once trimmed, or seconds not between 1 and `limit` is an error, and then
/// nothing in it is carried out.
fn asked(response: &Response, may_guess: bool, limit: Duration) -> anyhow::Result<Asked> {
    let mut asked = Asked::default();
    for call in calls(response)? {
        match call.name.as_str() {
            SPEAK => asked.sends.push(Sending::Say(said(call)?)),
            GUESS if may_guess => {
                let Question { question } = arguments(call)?;
                if question.trim().is_empty() {
                    bail!("the model's {GUESS} call asks nothing");
                }
                asked.sends.push(Sending::Guess(question));
            }
            WAIT => asked.wait = Some(waited(call, limit)?),
            other => bail!("the model called {other}, which is not a tool it was offered"),
        }
    }
    Ok(asked)
}

/// What a player's two loops share: what it has heard and done, with the
/// story that tells it, when the game began and what it was shown, whether
/// a wake-up is waiting, how many reminders it has set, what it is told,
/// and how it asks its model.
pub(crate) struct Mind {
    /// The history and the story told from it. Never locked across an
    /// await.
    remembered: Mutex<Remembered>,
    /// The number of the last message the player sent to others.
    n: AtomicU64,
    /// What the player was shown as the game began, and when. Never
    /// locked across an await.
    begun: Mutex<Option<(View, Instant)>>,
    /// Whether a [`Message::Wake`] is waiting for the think loop.
    waking: AtomicBool,
    /// How many reminders the player has set, which numbers them so that
    /// only the latest counts.
    reminders: AtomicU64,
    /// Its system prompt, rendered for its seat.
    prompt: String,
    /// How it asks its model.
    asker: Asker,
}

/// A player's history and the story that tells it, kept together so that
/// the story is always the narration of exactly the history, how many
/// guesses it has heard of, its own among them, and whether a player has
/// spoken since the last answer.
struct Remembered {
    history: Vec<Event<Memory>>,
    story: Story,
    guessed: u64,
    talked: bool,
}

impl Mind {
    /// The mind of `me`, told `prompt`, asking its model as `asker` does,
    /// with nothing in its history yet.
    fn new(me: &PlayerId, prompt: String, asker: Asker) -> Self {
        Self {
            remembered: Mutex::new(Remembered {
                history: Vec::new(),
                story: Story::of(me.clone()),
                guessed: 0,
                talked: false,
            }),
            n: AtomicU64::new(0),
            begun: Mutex::new(None),
            waking: AtomicBool::new(false),
            reminders: AtomicU64::new(0),
            prompt,
            asker,
        }
    }

    /// Add `event` to the end of the history, and what it adds to the
    /// story to the end of the story, together.
    fn remember(&self, event: Event<Memory>) {
        let mut remembered = self.remembered.lock().unwrap();
        remembered.story.recall(&event.payload);
        let (Memory::Observed { message } | Memory::Acted { message }) = &event.payload;
        match message {
            Message::Guess { .. } => remembered.guessed += 1,
            Message::Say { from, .. } if from != HOST => remembered.talked = true,
            Message::Answer { .. } => remembered.talked = false,
            _ => {}
        }
        remembered.history.push(event);
    }

    /// The story so far within `budget` tokens, its lines joined, and
    /// what it was told from.
    fn story(&self, budget: u64) -> (String, Shown) {
        let remembered = self.remembered.lock().unwrap();
        let (lines, forgot) = remembered.story.told_within(budget);
        let shown = Shown {
            seen: remembered.history.len(),
            forgot: (forgot > 0).then_some(forgot),
        };
        (lines.join("\n"), shown)
    }

    /// Where the guesses stand, of the `of` the players have.
    fn guesses(&self, of: u64) -> Guesses {
        let remembered = self.remembered.lock().unwrap();
        Guesses {
            of,
            left: of.saturating_sub(remembered.guessed),
            waiting: remembered
                .story
                .waiting()
                .map(|(asker, question)| (asker.clone(), question.to_string()))
                .collect(),
            talked: remembered.talked,
        }
    }

    /// Every guess put so far, by anyone, each trimmed and in lower case.
    fn questions(&self) -> Vec<String> {
        let remembered = self.remembered.lock().unwrap();
        remembered
            .history
            .iter()
            .filter_map(|event| match &event.payload {
                Memory::Observed {
                    message: Message::Guess { text, .. },
                }
                | Memory::Acted {
                    message: Message::Guess { text, .. },
                } => Some(folded(text)),
                _ => None,
            })
            .collect()
    }

    /// The next number for a message to others: one up from the last.
    fn number(&self) -> u64 {
        self.n.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Mark a wake-up waiting, and say whether none was before.
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

    /// Whether the reminder numbered `count` is the latest set.
    fn reminder_counts(&self, count: u64) -> bool {
        count == self.reminders.load(Ordering::Relaxed)
    }
}

/// A player whose decisions are made by a language model. Its perceive
/// loop and its think loop are each a `Player` with a context of its own
/// over one shared mind.
pub struct Player {
    context: Context<Message, Entry>,
    mind: Arc<Mind>,
}

impl Player {
    /// Builds a player's two loops once the episode has made their
    /// contexts. The player `me` is told `prompt` and asks its model as
    /// `asker` does.
    pub fn init(me: &PlayerId, prompt: String, asker: Asker) -> Seated {
        let mind = Arc::new(Mind::new(me, prompt, asker));
        let shared = Arc::clone(&mind);
        (
            Box::new(move |context| Actor::Player(Player { context, mind })),
            Some(Box::new(move |context| {
                Box::new(Player {
                    context,
                    mind: shared,
                })
            })),
        )
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

    /// Woken, decide: take the wake-up, ask the model where the game
    /// stands, and carry out what it answers. If that fails, log the
    /// failure, which is all there is to do about it: the player has not
    /// acted, and the game goes on. Being woken before the game began is
    /// an error.
    async fn decide(&self) -> anyhow::Result<()> {
        self.mind.woken();
        let (view, began) = self
            .mind
            .begun
            .lock()
            .unwrap()
            .clone()
            .context("woken before the game began")?;
        if let Err(error) = self.play(&view, began).await {
            self.context.log(Entry::Failed {
                by: self.context.id.clone(),
                error: format!("{error:#}"),
            });
        }
        Ok(())
    }

    /// Ask the model what to do in the game `view` shows, begun at
    /// `began`, and carry out what it answers: each message in order, to
    /// everyone else, and then a reminder if it asked to wait. A guess put
    /// before the model was asked, whatever its case, or twice in the
    /// answer, fails the answer, which is carried out not at all. While
    /// the model was asked, the same guess may have been put, and then it is
    /// not put again, or another, and then it is said as a proposal instead.
    async fn play(&self, view: &View, began: Instant) -> anyhow::Result<()> {
        let guesses = self.mind.guesses(view.guesses);
        let may_guess = guesses.may_guess();
        let put = self.mind.questions();
        let time_left = view.time_limit.saturating_sub(began.elapsed());
        let (request, shown) = self.mind.asker.request(
            &self.mind.prompt,
            |budget| self.mind.story(budget),
            &guesses.situation(time_left),
            tools(may_guess, view.time_limit),
        );
        let response = self.mind.asker.complete(&self.context, &request).await?;
        let Asked { sends, wait } = asked(&response, may_guess, view.time_limit)?;
        let mut guessed = put;
        for sending in &sends {
            if let Sending::Guess(question) = sending {
                if guessed.contains(&folded(question)) {
                    bail!("the model guessed \"{question}\", which was put already");
                }
                guessed.push(folded(question));
            }
        }
        let me = &self.context.id;
        for sending in sends {
            let sending = match sending {
                Sending::Guess(question) if self.mind.questions().contains(&folded(&question)) => {
                    continue;
                }
                Sending::Guess(question) if !self.mind.guesses(view.guesses).may_guess() => {
                    Sending::Say(format!("I'd guess next: \"{question}\""))
                }
                sending => sending,
            };
            let message = sending.message(me, self.mind.number());
            self.remember(Memory::Acted {
                message: message.clone(),
            });
            let to = everyone_but(me, &view.players);
            act(&self.context, message, to, Some(shown))?;
        }
        match wait {
            Some(after) => {
                let count = self.mind.remind();
                self.context.think_after(after, Message::Remind { count })
            }
            None => Ok(()),
        }
    }
}

#[async_trait]
impl Behavior for Player {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        &self.context
    }

    /// Log a message from another actor as observed, remember it, and wake
    /// the think loop. The beginning is remembered with when it came. A
    /// reminder to itself is ignored.
    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        let Some(n) = message.number() else {
            return Ok(());
        };
        if let Message::Begin { view, .. } = message {
            *self.mind.begun.lock().unwrap() = Some((view.clone(), Instant::now()));
        }
        observe(&self.context, message.sender(), n);
        self.remember(Memory::Observed {
            message: message.clone(),
        });
        self.wake()
    }
}

#[async_trait]
impl Think for Player {
    type Message = Message;

    /// Woken, decide what to do. Reminded, wake if the reminder is the
    /// latest set. Nothing else is thought about.
    async fn think(&mut self, message: &Message) -> anyhow::Result<()> {
        match message {
            Message::Wake => self.decide().await,
            Message::Remind { count } if self.mind.reminder_counts(*count) => self.wake(),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::tests::{Line, Played, Puppet, Rules, asking, hosting};
    use crate::model::canned::canned;
    use free_agent_model::fake::FakeModel;
    use serde_json::json;

    /// A tool call as a model makes it: the tool's name and its arguments.
    type Made = (String, serde_json::Value);

    fn speaking(text: &str) -> Made {
        (SPEAK.to_string(), json!({"text": text}))
    }

    fn guessing(question: &str) -> Made {
        (GUESS.to_string(), json!({"question": question}))
    }

    fn waiting(seconds: u64) -> Made {
        (WAIT.to_string(), json!({"seconds": seconds}))
    }

    /// What `calls` ask for, from a player that `may_guess` in a game of a
    /// minute.
    fn asked_of(calls: &[Made], may_guess: bool) -> anyhow::Result<Asked> {
        asked(
            &Response::calling(calls),
            may_guess,
            Duration::from_secs(60),
        )
    }

    #[test]
    fn an_answer_is_its_calls_in_order_and_the_last_wait() {
        let calls = [
            speaking("Shall we?"),
            waiting(5),
            guessing("Is it alive?"),
            waiting(10),
        ];
        assert_eq!(
            asked_of(&calls, true).unwrap(),
            Asked {
                sends: vec![
                    Sending::Say("Shall we?".to_string()),
                    Sending::Guess("Is it alive?".to_string())
                ],
                wait: Some(Duration::from_secs(10)),
            }
        );
    }

    #[test]
    fn an_answer_that_is_not_well_formed_is_an_error() {
        for (calls, may_guess, error) in [
            (vec![], true, "the model made no tool call"),
            (
                vec![guessing("Is it alive?")],
                false,
                "the model called guess, which is not a tool it was offered",
            ),
            (
                vec![guessing("  ")],
                true,
                "the model's guess call asks nothing",
            ),
            (
                vec![speaking("")],
                true,
                "the model's speak call says nothing",
            ),
            (
                vec![waiting(61)],
                true,
                "the model asked to wait 61s, which is not between 1s and 1m",
            ),
            (
                vec![("shout".to_string(), json!({}))],
                true,
                "the model called shout, which is not a tool it was offered",
            ),
        ] {
            assert_eq!(asked_of(&calls, may_guess).unwrap_err().to_string(), error);
        }
        let bad = vec![("wait".to_string(), json!({"seconds": "soon"}))];
        let error = format!("{:#}", asked_of(&bad, true).unwrap_err());
        assert!(
            error.starts_with("the arguments of the model's wait call"),
            "{error}"
        );
    }

    #[test]
    fn the_situation_says_the_guesses_waiting_how_many_are_left_and_what_the_player_may_do() {
        let time = Duration::from_secs(370);
        let fresh = Guesses::fresh(20);
        assert_eq!(
            fresh.situation(time),
            "Guesses left: 20 of 20. Time left: 6m 10s. You may speak, put a guess to the host, \
             or wait."
        );
        let quiet = Guesses {
            talked: false,
            ..fresh.clone()
        };
        assert!(!quiet.may_guess());
        assert_eq!(
            quiet.situation(time),
            "Guesses left: 20 of 20. Time left: 6m 10s. Nobody has proposed the next guess yet. \
             You may speak or wait."
        );
        let waiting = Guesses {
            left: 13,
            waiting: vec![
                ("Bob".to_string(), "Is it alive?".to_string()),
                ("Carol".to_string(), "Is it big?".to_string()),
            ],
            ..fresh.clone()
        };
        assert!(!waiting.may_guess());
        assert_eq!(
            waiting.situation(time),
            "Waiting for the host's answer: Bob's \"Is it alive?\" and Carol's \"Is it big?\". \
             Guesses left: 13 of 20. Time left: 6m 10s. You may speak or wait."
        );
        let none = Guesses { left: 0, ..fresh };
        assert!(!none.may_guess());
        assert_eq!(
            none.situation(time),
            "No guesses are left. Time left: 6m 10s. You may speak or wait."
        );
        let names = |may_guess| -> Vec<String> {
            tools(may_guess, time)
                .into_iter()
                .map(|tool| tool.name)
                .collect()
        };
        assert_eq!(names(true), [SPEAK, GUESS, WAIT]);
        assert_eq!(names(false), [SPEAK, WAIT]);
    }

    /// A player model answering each request with `answer` of its user
    /// message and the names of the tools it offers, taking `delay`.
    fn player_model(
        answer: impl Fn(&str, &[String]) -> Vec<Made> + Send + Sync + 'static,
        delay: Duration,
    ) -> Arc<FakeModel> {
        Arc::new(
            FakeModel::answering(move |request| {
                let offered: Vec<String> =
                    request.tools.iter().map(|tool| tool.name.clone()).collect();
                Ok(Response::calling(&answer(
                    &request.messages[1].content,
                    &offered,
                )))
            })
            .taking(delay),
        )
    }

    /// A host model answering as the canned host does, taking `delay` to
    /// answer a guess and none to pick or wait.
    fn host_model(delay: u64) -> Arc<FakeModel> {
        Arc::new(FakeModel::answering(canned).taking_by(move |request| {
            match request.tools.iter().any(|tool| tool.name == "named") {
                true => Duration::from_secs(delay),
                false => Duration::ZERO,
            }
        }))
    }

    /// Play a game under `rules` whose host asks `host`, Alice asking
    /// `model` with the prompt "You are Alice." and Bob saying `script`.
    async fn with_alice(
        host: &Arc<FakeModel>,
        model: &Arc<FakeModel>,
        rules: Rules,
        script: &[(u64, Line)],
    ) -> Played {
        hosting(host, rules, |players, seat| match seat.as_str() {
            "Alice" => Player::init(seat, "You are Alice.".to_string(), asking(model)),
            _ => Puppet::init(
                players,
                script
                    .iter()
                    .map(|(seconds, line)| (Duration::from_secs(*seconds), *line))
                    .collect(),
            ),
        })
        .await
    }

    /// The user message of each request `model` was sent, in order.
    fn questions(model: &FakeModel) -> Vec<String> {
        model
            .requests()
            .into_iter()
            .map(|request| request.messages[1].content.clone())
            .collect()
    }

    /// The rules of a game of a minute.
    fn a_minute() -> Rules {
        Rules {
            time_limit: Duration::from_secs(60),
            ..Rules::default()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_proposes_before_anyone_guesses_and_guesses_once_someone_has_spoken() {
        let model = player_model(
            |_, offered| match offered.iter().any(|tool| tool == GUESS) {
                true => vec![guessing("Is it alive?")],
                false => vec![speaking("Shall we ask whether it's alive?"), waiting(60)],
            },
            Duration::from_secs(1),
        );
        let script = [(10, Line::Say("Yes.")), (13, Line::Say("Go on."))];
        let played = with_alice(&host_model(5), &model, a_minute(), &script).await;
        let asked = questions(&model);
        let first = &asked[0];
        let ending = "Nobody has proposed the next guess yet. You may speak or wait. Answer by \
                      calling one of the tools.";
        assert!(first.ends_with(ending), "{first}");
        assert!(asked[0].starts_with(
            "You are Alice, playing Twenty Questions with Bob. The host holds a secret object, \
             and you have 20 guesses among you and 1m to name it.\n\nGuesses left: 20 of 20."
        ));
        let guessed: Vec<&Message> = played
            .sent_by("Alice")
            .into_iter()
            .filter(|message| matches!(message, Message::Guess { .. }))
            .collect();
        assert_eq!(
            guessed,
            [&Message::Guess {
                from: "Alice".to_string(),
                n: 2,
                text: "Is it alive?".to_string(),
            }]
        );
        // Asked while its guess waits for its answer, Alice may not guess.
        let while_waiting = asked
            .iter()
            .find(|asked| asked.contains("Waiting for the host's answer"))
            .unwrap();
        assert!(
            while_waiting.contains(
                "\n\nWaiting for the host's answer: Alice's \"Is it alive?\". Guesses left: 19 of 20."
            ),
            "{while_waiting}"
        );
        let answered = asked.last().unwrap();
        assert!(
            answered.contains(
                "The host answers your guess 1 \"Is it alive?\": \"No.\" 19 guesses are left."
            ),
            "{answered}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_same_guess_twice_in_one_answer_fails_it() {
        let model = player_model(
            |_, offered| match offered.iter().any(|tool| tool == GUESS) {
                true => vec![guessing("Is it big?"), guessing(" is it BIG?")],
                false => vec![waiting(60)],
            },
            Duration::from_secs(1),
        );
        let played = with_alice(&host_model(1), &model, a_minute(), &[(1, Line::Say("Go."))]).await;
        assert!(
            played
                .failures("Alice")
                .contains(&"the model guessed \" is it BIG?\", which was put already")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_guess_put_already_fails_the_whole_answer() {
        let model = player_model(
            |_, offered| match offered.iter().any(|tool| tool == GUESS) {
                true => vec![speaking("Me too!"), guessing("IS IT ALIVE?")],
                false => vec![waiting(60)],
            },
            Duration::from_secs(1),
        );
        let script = [
            (1, Line::Guess("Is it alive?")),
            (3, Line::Say("What next?")),
        ];
        let played = with_alice(&host_model(1), &model, a_minute(), &script).await;
        assert!(
            played
                .failures("Alice")
                .contains(&"the model guessed \"IS IT ALIVE?\", which was put already")
        );
        assert!(played.sent_by("Alice").is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_guess_made_while_another_came_in_is_said_as_a_proposal_unless_it_was_the_same() {
        for (question, said) in [
            ("Is it big?", Some("I'd guess next: \"Is it big?\"")),
            ("is it ALIVE?", None),
        ] {
            let model = player_model(
                move |_, offered| match offered.iter().any(|tool| tool == GUESS) {
                    true => vec![guessing(question)],
                    false => vec![waiting(60)],
                },
                Duration::from_secs(10),
            );
            let script = [
                (1, Line::Say("Let's ask.")),
                (15, Line::Guess("Is it alive?")),
            ];
            let played = with_alice(&host_model(20), &model, a_minute(), &script).await;
            let expected: Vec<Message> = said
                .into_iter()
                .map(|text| Message::Say {
                    from: "Alice".to_string(),
                    n: 1,
                    text: text.to_string(),
                })
                .collect();
            assert_eq!(played.sent_by("Alice"), expected.iter().collect::<Vec<_>>());
            assert!(played.failures("Alice").is_empty());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_waits_as_long_as_it_asked_and_only_its_latest_reminder_counts() {
        let model = player_model(|_, _| vec![waiting(20), waiting(25)], Duration::ZERO);
        with_alice(&host_model(1), &model, a_minute(), &[(5, Line::Say("Hi."))]).await;
        // Woken at the beginning, and by Bob at 5s, Alice asks to be
        // reminded at 25s and at 30s; only the second counts, and it
        // reminds her again at 55s.
        assert_eq!(model.requests().len(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_whose_model_fails_logs_the_failure_and_the_game_goes_on() {
        let model = Arc::new(FakeModel::failing("no model"));
        let played = with_alice(&host_model(1), &model, a_minute(), &[]).await;
        assert_eq!(played.failures("Alice"), ["no model"]);
        played.ran.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_with_no_guesses_left_is_not_offered_guess() {
        let model = player_model(|_, _| vec![waiting(60)], Duration::ZERO);
        let rules = Rules {
            guesses: 1,
            ..a_minute()
        };
        let script = [
            (1, Line::Say("Let's ask.")),
            (2, Line::Guess("Is it alive?")),
        ];
        let host = Arc::new(FakeModel::answering(canned).taking(Duration::from_secs(30)));
        with_alice(&host, &model, rules, &script).await;
        let last = questions(&model).pop().unwrap();
        assert!(last.contains("No guesses are left."), "{last}");
    }

    #[tokio::test(start_paused = true)]
    async fn the_object_never_appears_in_a_player_s_request() {
        let players = Arc::new(FakeModel::answering(canned));
        let host = Arc::new(FakeModel::answering(canned));
        let rules = Rules {
            object: Some("a banjo"),
            ..a_minute()
        };
        let played = hosting(&host, rules, |_, seat| {
            Player::init(seat, format!("You are {seat}."), asking(&players))
        })
        .await;
        let (_, by, _, object, _) = played.end();
        assert_eq!((by, object), (crate::log::Victory::OutOfTime, "a banjo"));
        let asked = players.requests();
        assert!(asked.len() > 4);
        for request in asked {
            let shown = serde_json::to_string(&request).unwrap().to_lowercase();
            assert!(!shown.contains("banjo"), "{shown}");
        }
        assert!(
            host.requests()
                .iter()
                .all(|request| request.messages[0].content.contains("a banjo"))
        );
    }

    #[test]
    fn the_probe_asks_as_a_player_at_the_start_of_a_game_and_judges_as_one() {
        let probe = probe("qwen", ToolChoice::Auto);
        assert_eq!(probe.tool_choice, ToolChoice::Auto);
        assert!(
            probe.messages[1]
                .content
                .starts_with("Guesses left: 20 of 20.")
        );
        assert!(probed(&Response::calling(&[speaking("Hello.")])).is_ok());
        assert!(probed(&Response::calling(&[])).is_err());
        let small = unstoried("qwen", "", 20, Duration::from_secs(600));
        let large = unstoried("qwen", &"x".repeat(3000), 20, Duration::from_secs(600));
        assert_eq!(large - small, 1000);
    }
}
