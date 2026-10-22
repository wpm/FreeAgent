//! The host: the actor that holds the secret object and runs the game, a
//! model player and the episode's environment at once.
//!
//! Like a player, the host has two loops sharing one mind, never locked
//! across a model call. **Perceiving**, it numbers each guess as it
//! arrives, counts it while guesses remain, and queues it to be answered;
//! a guess that comes after the last is logged as [`Entry::Uncounted`],
//! and one that repeats a guess counted already, as two players putting
//! the same guess at once do, as [`Entry::Repeated`].
//! It hears all speech. **Thinking**, it is woken like a player, by each
//! guess and by everyone's speech, with at most one call in flight and one
//! waiting, and a guess waiting to be answered comes first: while one
//! waits, the host is asked to answer the oldest, offered only `answer`
//! and `named`. Otherwise it is offered `speak` and `wait`.
//!
//! A guess that says the object's name wins the moment the host counts
//! it, unanswered. A line of the host's that says the name before the game
//! is won is not sent: it is a failure, logged as [`Entry::Failed`], and
//! the host is asked again. A failed answer is asked again up to
//! [`ATTEMPTS`] times, [`RETRY`] apart, and so is the pick; after that the
//! episode fails with the last error. A failed word in conversation is
//! asked again up to as many times, and then dropped.
//!
//! When the configuration names no object, the host picks one as the game
//! begins, asked with the pick prompt for a category drawn at random, at
//! the configured temperature, before it tells the players the game has
//! begun. The game ends when a guess says the object's name or is
//! answered as naming it, which wins it for the players, or when the last
//! guess is answered without, or at the time limit, which win it for the
//! host. Either way the host stops every player and logs the
//! [`Entry::End`] with the object.

use crate::config::{Configuration, HostPrompt};
use crate::game::{Actor, RETRY, Seated, act, everyone_but, observe};
use crate::log::{Entry, Side, Victory};
use crate::message::{HOST, Message, View, folded};
use crate::model::{
    Asker, SPEAK, Shown, WAIT, arguments, calls, not_offered, request, said, speak, text_parameter,
};
use crate::object::{about, gives_away, says, shares_a_word};
use anyhow::{anyhow, bail};
use async_trait::async_trait;
use free_agent::{Behavior, Context, Think};
use free_agent_game::report::{told_within, whole_seconds};
use free_agent_game::seat::PlayerId;
use free_agent_model::{Request, Response, Tool, ToolChoice, estimate};
use rand::seq::IndexedRandom;
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::Instant;

/// How many times the host is asked for an answer, a pick, or a word in
/// conversation before it gives up.
pub const ATTEMPTS: u32 = 3;

/// The name of the tool the host answers a guess with.
pub(crate) const ANSWER_TOOL: &str = "answer";

/// The name of the tool the host answers a guess that names the object
/// with.
pub(crate) const NAMED: &str = "named";

/// The name of the tool the host picks the object with.
pub(crate) const CHOOSE: &str = "choose";

/// The tools the host answers a guess with: `answer` and `named`, each
/// taking the `text` the players hear.
fn answer_tools() -> Vec<Tool> {
    vec![
        Tool::new(
            ANSWER_TOOL,
            "Answer a guess that does not name the secret object.",
            json!({
                "type": "object",
                "properties": {"text": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Your answer as the players hear it: Yes or No, then at most a few words that call the object 'it', never name it, and tell nothing the guess did not ask.",
                }},
                "required": ["text"],
                "additionalProperties": false,
            }),
        ),
        Tool::new(
            NAMED,
            "Call this instead of answer when the guess names the secret object itself, which wins the game for the players.",
            text_parameter("text"),
        ),
    ]
}

/// What the host is asked when `asker`'s guess `guess` waits for its
/// answer, about `object`: the guess, and the guess put [`about`] the
/// object itself.
fn answering(asker: &str, guess: &str, object: &str) -> String {
    format!(
        "{asker} guesses: \"{guess}\"\n\nAbout the secret object, that asks: \"{}\"\n\nIf \
         the guess names {object} itself, call named. Otherwise call answer, and answer \
         truthfully, calling it 'it' and never by its name.",
        about(guess, object)
    )
}

/// The tools the host talks with: [`speak`], and `wait`, which says
/// nothing.
fn talk_tools() -> Vec<Tool> {
    vec![
        speak("Say something to everyone at the table."),
        Tool::new(
            WAIT,
            "Say nothing now.",
            json!({"type": "object", "properties": {}, "additionalProperties": false}),
        ),
    ]
}

/// What opens the speech the host is told, when no guess waits.
const HEARD: &str = "Since you last spoke or waited, the players said:";

/// How long the players may go without a guess before the host may urge
/// them to put one.
pub const NUDGE: Duration = Duration::from_secs(120);

/// `Yes.` or `No.`, when `text` begins with that word: what an answer that
/// gives the object away after it is cut to.
fn leading_yes_or_no(text: &str) -> Option<&'static str> {
    let first = text
        .split(|c: char| !c.is_alphabetic())
        .find(|word| !word.is_empty())?
        .to_lowercase();
    match first.as_str() {
        "yes" => Some("Yes."),
        "no" => Some("No."),
        _ => None,
    }
}

/// What the host is asked when no guess waits, after the speech it heard:
/// how many of the `guesses` the players have `used`, how much time is
/// left, how long it has been `quiet` since the last guess, and that only
/// guesses are answered.
fn talking(used: u64, guesses: u64, time_left: Duration, quiet: Duration) -> String {
    format!(
        "Guesses used: {used} of {guesses}. Time left: {}. The last guess came {} ago, and \
         no guess is waiting for your answer. Only guesses are answered. The players are \
         talking among themselves, and proposing guesses to one another is not asking you \
         anything. If a player spoke to you, the host, and asked you a yes-or-no question, \
         do not answer it, but ask whether that is their guess, as in \"Is that your guess, \
         Bob?\". If nobody has put a guess for {} or more, you may urge the players to put \
         one. Otherwise, wait.",
        whole_seconds(time_left),
        whole_seconds(quiet),
        whole_seconds(NUDGE)
    )
}

/// Whether `line`, a line of speech the host heard, speaks to the host by
/// name: calls it *host*, and not *the host* or *the host's*, which talk
/// about it. Players mention *the host's answers* all the time.
fn addressed(line: &str) -> bool {
    const TALKING_ABOUT: [&str; 6] = ["the", "a", "an", "our", "this", "that"];
    let lowered = line.to_lowercase();
    let words = lowered
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’'));
    std::iter::once("")
        .chain(words.clone())
        .zip(words)
        .any(|(before, word)| word == "host" && !TALKING_ABOUT.contains(&before))
}

/// The tool the host picks the object with: `choose`, taking the `object`.
fn choose() -> Tool {
    Tool::new(
        CHOOSE,
        "Choose the secret object.",
        json!({
            "type": "object",
            "properties": {"object": {
                "type": "string",
                "minLength": 1,
                "description": "The secret object, as you would say it after \"I'm thinking of\": a unicorn, an octopus, the Eiffel Tower, durian.",
            }},
            "required": ["object"],
            "additionalProperties": false,
        }),
    )
}

/// The system prompt the host picks the object under. Ollama reads no
/// tool call from Qwen asked without a system prompt of its own.
pub(crate) const PICKER: &str = "You are the host of a game of Twenty Questions.";

/// The request that picks the object, of the model `model_id` under
/// `tool_choice`: [`PICKER`], and `pick` and then
/// [`ANSWER`](crate::model::ANSWER), offering
/// [`choose`], at `temperature`.
fn picking(model_id: &str, tool_choice: ToolChoice, pick: &str, temperature: f64) -> Request {
    request(
        model_id,
        tool_choice,
        PICKER,
        pick.to_string(),
        vec![choose()],
    )
    .at_temperature(temperature)
}

/// The most a request to the model `model_id` from the host told `prompt`
/// takes of the context window before its story, by estimate, in a game of
/// `guesses` that lasts `limit`, asking the longest of `picks` when it
/// picks: the largest of an answer's, a word's and a pick's.
pub(crate) fn unstoried(
    model_id: &str,
    prompt: &str,
    picks: &[(Option<String>, String)],
    guesses: u64,
    limit: Duration,
) -> u64 {
    let tool_choice = ToolChoice::Required;
    let estimated = |asked: String, tools| {
        free_agent_model::estimated(&request(
            model_id,
            tool_choice,
            prompt,
            format!("\n\n{asked}"),
            tools,
        ))
    };
    let guess = "Is it something you would find in a kitchen, or is it bigger than a breadbox?";
    let answer = estimated(answering("Alice", guess, "a teapot"), answer_tools());
    let talk = estimated(
        format!("{HEARD}\n\n{}", talking(guesses, guesses, limit, limit)),
        talk_tools(),
    );
    let pick = picks
        .iter()
        .map(|(_, pick)| free_agent_model::estimated(&picking(model_id, tool_choice, pick, 1.0)))
        .max()
        .unwrap_or_default();
    answer.max(talk).max(pick)
}

/// The request the probe asks the model `model_id` as the host answering
/// a guess, under `tool_choice`.
pub(crate) fn probe_answer(model_id: &str, tool_choice: ToolChoice) -> Request {
    request(
        model_id,
        tool_choice,
        "You are the host of a game of Twenty Questions. The secret object is a teapot.",
        answering("Alice", "Is it alive?", "a teapot"),
        answer_tools(),
    )
}

/// The request the probe asks the model `model_id` as the host picking an
/// object, under `tool_choice`.
pub(crate) fn probe_pick(model_id: &str, tool_choice: ToolChoice) -> Request {
    picking(
        model_id,
        tool_choice,
        "Pick a secret object for a game of Twenty Questions: a musical instrument.",
        1.2,
    )
}

/// The arguments of a [`choose`] call.
#[derive(Deserialize)]
struct Chosen {
    object: String,
}

/// The answer `response` gives a guess, and whether it says the guess
/// names the object: its first call, which must be a well-formed `answer`
/// or `named`.
pub(crate) fn answered(response: &Response) -> anyhow::Result<(String, bool)> {
    let call = calls(response)?[0];
    match call.name.as_str() {
        ANSWER_TOOL => Ok((said(call)?, false)),
        NAMED => Ok((said(call)?, true)),
        other => Err(not_offered(other)),
    }
}

/// The object `response` picks: its first call, which must be a
/// well-formed `choose` naming something, with any underscores in the name
/// written as spaces, as a model writing an identifier writes them.
pub(crate) fn chosen(response: &Response) -> anyhow::Result<String> {
    let call = calls(response)?[0];
    if call.name != CHOOSE {
        return Err(not_offered(&call.name));
    }
    let Chosen { object } = arguments(call)?;
    match object.replace('_', " ").trim() {
        "" => bail!("the model chose nothing"),
        object => Ok(object.to_string()),
    }
}

/// What `response`, to the host with no guess waiting, has it say, in
/// order: the text of each `speak`. A `wait` says nothing, and any other
/// call is an error.
fn spoken(response: &Response) -> anyhow::Result<Vec<String>> {
    let mut spoken = Vec::new();
    for call in calls(response)? {
        match call.name.as_str() {
            SPEAK => spoken.push(said(call)?),
            WAIT => {}
            other => return Err(not_offered(other)),
        }
    }
    Ok(spoken)
}

/// Everything the host is set up with.
pub struct Brief {
    /// Everyone at the table, in seat order.
    pub players: Vec<PlayerId>,
    /// Every player's strategy.
    pub strategies: BTreeMap<PlayerId, String>,
    /// How many guesses the players have.
    pub guesses: u64,
    /// How long the game lasts.
    pub time_limit: Duration,
    /// The secret object, when the configuration names it.
    pub object: Option<String>,
    /// Each category the pick may be asked for, if any, with the pick
    /// prompt rendered for it.
    pub picks: Vec<(Option<String>, String)>,
    /// The temperature the pick is asked at.
    pub temperature: f64,
    /// The host's system prompt, to be rendered for the object.
    pub prompt: HostPrompt,
    /// How the host asks its model.
    pub asker: Asker,
    /// The configuration the game runs under, which the host logs as it
    /// starts.
    pub configuration: Configuration,
}

/// A guess waiting for its answer.
#[derive(Debug, Clone)]
struct Waiting {
    /// Its number, in the order the host received the guesses.
    number: u64,
    /// Who asked it.
    from: PlayerId,
    /// The question.
    text: String,
}

/// The object the host's model picked, with the category it was asked for
/// and the host's system prompt rendered for it.
struct Picked {
    object: String,
    category: Option<String>,
    prompt: String,
}

/// What the host's model made of a guess, once judged.
enum Judged {
    /// The answer to send, and whether it names the object.
    Answer { text: String, named: bool },
    /// The time ran out while the model was asked.
    Late,
}

/// Where the game stands, as the host keeps it.
#[derive(Default)]
struct State {
    /// The secret object, once known.
    object: Option<String>,
    /// The host's system prompt, rendered for the object once it is known.
    prompt: String,
    /// How many guesses were counted.
    counted: u64,
    /// How many were answered.
    answered: u64,
    /// The guesses counted and not yet answered, oldest first.
    waiting: VecDeque<Waiting>,
    /// Every guess counted, [`folded`].
    put: HashSet<String>,
    /// When the players were told the game began.
    began: Option<Instant>,
    /// Whether the game is over.
    over: bool,
    /// How many words in conversation in a row failed.
    stumbles: u32,
    /// When the last guess came, or the game began.
    quiet_since: Option<Instant>,
    /// Whether the host was asked to urge a guess since the last one came.
    nudged: bool,
}

/// What the host's two loops share. Nothing in it is locked across an
/// await.
struct Mind {
    brief: Brief,
    state: Mutex<State>,
    /// Each line of speech heard since the host last decided whether to
    /// speak, as `Bob: "…"`.
    heard: Mutex<Vec<(String, ())>>,
    /// The number of the last message the host sent to others.
    n: AtomicU64,
    /// Whether a [`Message::Wake`] is waiting for the think loop.
    waking: AtomicBool,
    /// Where the winning side is sent once the game is won.
    winner: Mutex<Option<oneshot::Sender<Side>>>,
    /// Why the episode failed, once it has: the host's clean-up returns
    /// it, which fails the episode.
    failure: Mutex<Option<anyhow::Error>>,
}

/// The host. Its perceive loop and its think loop are each a `Host` with
/// a context of its own over one shared mind.
pub struct Host {
    context: Context<Message, Entry>,
    mind: Arc<Mind>,
}

impl Host {
    /// Builds the host's two loops once the episode has made their
    /// contexts: the host `brief` describes, which sends the winning side
    /// on `winner` once the game is won.
    pub fn init(brief: Brief, winner: oneshot::Sender<Side>) -> Seated {
        let mind = Arc::new(Mind {
            state: Mutex::new(State {
                object: brief.object.clone(),
                ..State::default()
            }),
            brief,
            heard: Mutex::new(Vec::new()),
            n: AtomicU64::new(0),
            waking: AtomicBool::new(false),
            winner: Mutex::new(Some(winner)),
            failure: Mutex::new(None),
        });
        let shared = Arc::clone(&mind);
        (
            Box::new(move |context| Actor::Host(Host { context, mind })),
            Some(Box::new(move |context| {
                Box::new(Host {
                    context,
                    mind: shared,
                })
            })),
        )
    }

    /// Every player, as the receivers of a message.
    fn players(&self) -> HashSet<PlayerId> {
        everyone_but(HOST, &self.mind.brief.players)
    }

    /// The next number for a message to others: one up from the last.
    fn number(&self) -> u64 {
        self.mind.n.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// The speech heard since the host last decided whether to speak,
    /// within `budget` tokens, the oldest left out first to fit, and what it
    /// was told from: how many lines were heard, and how many left out.
    fn heard(&self, budget: u64) -> (String, Shown) {
        let heard = self.mind.heard.lock().unwrap();
        let (lines, forgot) = told_within(&heard, |_| true, budget);
        let shown = Shown {
            seen: heard.len(),
            forgot: (forgot > 0).then_some(forgot),
        };
        (lines.join("\n"), shown)
    }

    /// Wake the think loop, unless a wake-up is already waiting for it.
    fn wake(&self) -> anyhow::Result<()> {
        if !self.mind.waking.swap(true, Ordering::Relaxed) {
            self.context.think(Message::Wake)?;
        }
        Ok(())
    }

    /// Send `message` to every player, as the host's action with what the
    /// model was `shown`, if a model made it.
    fn tell(&self, message: Message, shown: Option<Shown>) -> anyhow::Result<()> {
        act(&self.context, message, self.players(), shown)
    }

    /// How long the game has gone on, since the players were told it
    /// began.
    fn elapsed(&self) -> Duration {
        let began = self.mind.state.lock().unwrap().began;
        began.map(|began| began.elapsed()).unwrap_or_default()
    }

    /// Whether the game's time is up.
    fn out_of_time(&self) -> bool {
        let began = self.mind.state.lock().unwrap().began;
        began.is_some_and(|began| began.elapsed() >= self.mind.brief.time_limit)
    }

    /// Tell every player the game has begun, and set the time limit.
    fn begin(&self) -> anyhow::Result<()> {
        let brief = &self.mind.brief;
        let view = View {
            players: brief.players.clone(),
            guesses: brief.guesses,
            time_limit: brief.time_limit,
        };
        {
            let mut state = self.mind.state.lock().unwrap();
            state.began = Some(Instant::now());
            state.quiet_since = state.began;
        }
        let begin = Message::Begin {
            n: self.number(),
            view,
        };
        self.tell(begin, None)?;
        self.context.think_after(brief.time_limit, Message::Over)
    }

    /// Log a failure to act, as what went wrong says it.
    fn failed(&self, error: &anyhow::Error) {
        self.context.log(Entry::Failed {
            by: HOST.to_string(),
            error: format!("{error:#}"),
        });
    }

    /// Stop every player and the host itself.
    fn stop(&self) -> anyhow::Result<()> {
        for player in &self.mind.brief.players {
            self.context.stop(player)?;
        }
        self.context.shutdown();
        Ok(())
    }

    /// End the game, won `by` this with the guesses answered.
    fn end(&self, by: Victory) -> anyhow::Result<()> {
        let used = self.mind.state.lock().unwrap().answered;
        self.finish(by, used)
    }

    /// End the game, won `by` this with `used` guesses: log the end with
    /// the object, send the winner, and stop everyone. A game is ended
    /// once.
    fn finish(&self, by: Victory, used: u64) -> anyhow::Result<()> {
        let took = self.elapsed();
        let end = {
            let mut state = self.mind.state.lock().unwrap();
            let over = std::mem::replace(&mut state.over, true);
            (!over).then(|| Entry::End {
                winner: by.winner(),
                by,
                used,
                object: state.object.clone().unwrap_or_default(),
                took,
            })
        };
        end.map_or(Ok(()), |end| {
            self.context.log(end);
            if let Some(winner) = self.mind.winner.lock().unwrap().take() {
                let _ = winner.send(by.winner());
            }
            self.stop()
        })
    }

    /// Fail the episode with `error`: stop everyone, and keep the error for
    /// the host's clean-up to return.
    fn fail(&self, error: anyhow::Error) -> anyhow::Result<()> {
        self.mind.state.lock().unwrap().over = true;
        *self.mind.failure.lock().unwrap() = Some(error);
        self.stop()
    }

    /// Woken, decide: take the wake-up, and pick the object if there is none
    /// yet, or answer the oldest guess waiting if there is one, each asked
    /// as persistently as [`Host::persist`] asks, and otherwise talk.
    /// Nothing is decided once the game is over, since the host stops as it
    /// ends the game, and the time limit, which comes before any wake-up
    /// waiting, ends a game whose host is not asking its model.
    async fn decide(&self) -> anyhow::Result<()> {
        self.mind.waking.store(false, Ordering::Relaxed);
        let (object, waiting) = {
            let state = self.mind.state.lock().unwrap();
            (state.object.clone(), state.waiting.front().cloned())
        };
        match (object, waiting) {
            (None, _) => match self.persist(|| self.try_pick()).await? {
                Some(picked) => self.picked(picked),
                None => Ok(()),
            },
            (Some(object), Some(guess)) => {
                match self.persist(|| self.try_answer(&object, &guess)).await? {
                    Some(Judged::Answer { text, named }) => self.reply(&guess, text, named),
                    Some(Judged::Late) => self.end(Victory::OutOfTime),
                    None => Ok(()),
                }
            }
            (Some(object), None) => self.talk(&object).await,
        }
    }

    /// Make `attempt` up to [`ATTEMPTS`] times, [`RETRY`] apart, logging
    /// each failure, until one succeeds, and give what it made. After the
    /// last failure the episode fails, and there is nothing to give.
    async fn persist<T, F: Future<Output = anyhow::Result<T>>>(
        &self,
        attempt: impl Fn() -> F,
    ) -> anyhow::Result<Option<T>> {
        let mut tries = 0;
        loop {
            tries += 1;
            let error = match attempt().await {
                Ok(made) => return Ok(Some(made)),
                Err(error) => error,
            };
            self.failed(&error);
            if tries == ATTEMPTS {
                return self.fail(error).map(|()| None);
            }
            tokio::time::sleep(RETRY).await;
        }
    }

    /// Ask the model to pick the object for a category drawn at random.
    async fn try_pick(&self) -> anyhow::Result<Picked> {
        let brief = &self.mind.brief;
        let (category, pick) = brief
            .picks
            .choose(&mut rand::rng())
            .cloned()
            .ok_or_else(|| anyhow!("there is no pick prompt"))?;
        let asker = &brief.asker;
        let request = picking(&asker.model_id, asker.tool_choice, &pick, brief.temperature);
        let response = asker.complete(&self.context, &request).await?;
        let object = chosen(&response)?;
        let prompt = brief.prompt.render(&object)?;
        Ok(Picked {
            object,
            category,
            prompt,
        })
    }

    /// Log the object `picked`, and begin the game with it.
    fn picked(&self, picked: Picked) -> anyhow::Result<()> {
        self.context.log(Entry::Chosen {
            object: picked.object.clone(),
            category: picked.category,
        });
        {
            let mut state = self.mind.state.lock().unwrap();
            state.object = Some(picked.object);
            state.prompt = picked.prompt;
        }
        self.begin()
    }

    /// Ask the model to answer `guess`, about `object`, and judge the
    /// answer, unless the time ran out while it was asked. The model is
    /// shown the guess alone, with no story: an answer is about the object
    /// and the question, and a story of earlier answers only tempts a small
    /// model to repeat them. A guess the model calls named that shares no
    /// word with the name is answered Yes and is not named. An answer that
    /// says the name is cut to its first word when that is Yes or No, and
    /// is otherwise an error.
    async fn try_answer(&self, object: &str, guess: &Waiting) -> anyhow::Result<Judged> {
        let prompt = self.mind.state.lock().unwrap().prompt.clone();
        let asker = &self.mind.brief.asker;
        let request = request(
            &asker.model_id,
            asker.tool_choice,
            &prompt,
            answering(&guess.from, &guess.text, object),
            answer_tools(),
        );
        let response = asker.complete(&self.context, &request).await;
        if self.out_of_time() {
            return Ok(Judged::Late);
        }
        let (text, named) = match answered(&response?)? {
            (_, true) if !shares_a_word(&guess.text, object) => ("Yes.".to_string(), false),
            answer => answer,
        };
        let text = match !named && gives_away(&text, object, &guess.text) {
            true => match leading_yes_or_no(&text) {
                Some(word) => word.to_string(),
                None => bail!("the host's answer gives the secret object away: {text}"),
            },
            false => text,
        };
        Ok(Judged::Answer { text, named })
    }

    /// Send `text`, the answer to `guess`, which names the object when
    /// `named`, and only once it is sent count it answered.
    /// End the game if it is won or the last guess is answered. Otherwise,
    /// wake to answer the next guess if one is waiting, or to the speech
    /// heard meanwhile.
    fn reply(&self, guess: &Waiting, text: String, named: bool) -> anyhow::Result<()> {
        let guesses = self.mind.brief.guesses;
        let (answered, more) = {
            let mut state = self.mind.state.lock().unwrap();
            let answer = Message::Answer {
                n: self.number(),
                guess: guess.number,
                asked_by: guess.from.clone(),
                text,
                named,
                left: guesses - state.counted,
            };
            self.tell(answer, Some(Shown::default()))?;
            state.waiting.pop_front();
            state.answered += 1;
            (state.answered, !state.waiting.is_empty())
        };
        if named {
            self.finish(Victory::Named, guess.number)
        } else if answered == guesses {
            self.end(Victory::OutOfGuesses)
        } else if more || !self.mind.heard.lock().unwrap().is_empty() {
            self.wake()
        } else {
            Ok(())
        }
    }

    /// Ask the model whether to say anything, about `object`, and say it,
    /// when a player spoke to the host by name or nobody has put a guess
    /// for [`NUDGE`], once until the next guess; otherwise forget the
    /// speech heard without asking. A failure is asked again, by waking,
    /// until [`ATTEMPTS`] fail in a row, and then the speech it was about is
    /// forgotten.
    async fn talk(&self, object: &str) -> anyhow::Result<()> {
        let quiet = {
            let state = self.mind.state.lock().unwrap();
            let quiet = state
                .quiet_since
                .map(|since| since.elapsed())
                .unwrap_or_default();
            (quiet >= NUDGE && !state.nudged).then_some(quiet)
        };
        let spoken_to = self
            .mind
            .heard
            .lock()
            .unwrap()
            .iter()
            .any(|(line, _)| addressed(line));
        if !spoken_to && quiet.is_none() {
            self.mind.heard.lock().unwrap().clear();
            return Ok(());
        }
        let error = match self.try_talk(object).await {
            Ok(()) => {
                let mut state = self.mind.state.lock().unwrap();
                state.stumbles = 0;
                state.nudged |= quiet.is_some();
                return Ok(());
            }
            Err(error) => error,
        };
        self.failed(&error);
        let mut state = self.mind.state.lock().unwrap();
        state.stumbles += 1;
        if state.stumbles < ATTEMPTS {
            drop(state);
            return self.wake();
        }
        state.stumbles = 0;
        self.mind.heard.lock().unwrap().clear();
        Ok(())
    }

    /// Ask the model whether to say anything about the speech heard since
    /// it last decided, keeping the secret `object`, and say what it says,
    /// unless the time ran out while it was asked. Nothing is said if any of
    /// it says the object's name, which is an error. Once the model has
    /// answered, the speech it was told is forgotten.
    async fn try_talk(&self, object: &str) -> anyhow::Result<()> {
        let (prompt, used, quiet) = {
            let state = self.mind.state.lock().unwrap();
            let quiet = state
                .quiet_since
                .map(|since| since.elapsed())
                .unwrap_or_default();
            (state.prompt.clone(), state.counted, quiet)
        };
        let brief = &self.mind.brief;
        let time_left = brief.time_limit.saturating_sub(self.elapsed());
        let (request, shown) = brief.asker.request(
            &prompt,
            |budget| {
                let (lines, shown) = self.heard(budget.saturating_sub(estimate(HEARD) + 1));
                (format!("{HEARD}\n{lines}"), shown)
            },
            &talking(used, brief.guesses, time_left, quiet),
            talk_tools(),
        );
        let response = brief.asker.complete(&self.context, &request).await;
        if self.out_of_time() {
            return self.end(Victory::OutOfTime);
        }
        let spoken = spoken(&response?)?;
        if let Some(text) = spoken.iter().find(|text| gives_away(text, object, "")) {
            bail!("the host's words give the secret object away: {text}");
        }
        self.mind.heard.lock().unwrap().drain(..shown.seen);
        for text in spoken {
            let say = Message::Say {
                from: HOST.to_string(),
                n: self.number(),
                text,
            };
            self.tell(say, Some(shown))?;
        }
        Ok(())
    }
}

#[async_trait]
impl Behavior for Host {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        &self.context
    }

    /// Log the table and the configuration, and begin the game, once the
    /// object is picked if the configuration names none.
    async fn start(&mut self) -> anyhow::Result<()> {
        let brief = &self.mind.brief;
        self.context.log(Entry::Start {
            strategies: brief.strategies.clone(),
            guesses: brief.guesses,
            time_limit: brief.time_limit,
        });
        let configuration = Box::new(brief.configuration.clone());
        self.context.log(Entry::Configuration(configuration));
        let object = self.mind.state.lock().unwrap().object.clone();
        match object {
            Some(object) => {
                let prompt = brief.prompt.render(&object)?;
                self.mind.state.lock().unwrap().prompt = prompt;
                self.begin()
            }
            None => self.wake(),
        }
    }

    /// Log a message from a player as observed and remember it. A guess is
    /// counted and queued while guesses remain, and logged as uncounted
    /// after, or as repeated when it repeats one counted already. A guess
    /// counted that says the object's name wins the game at once, without
    /// waiting for the model; any other, or speech, wakes the think loop.
    /// Once the game is over, nothing wakes it.
    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        let Some(n) = message.number() else {
            return Ok(());
        };
        let from = message.sender();
        observe(&self.context, from.clone(), n);
        if let Message::Guess { text, .. } = message {
            let mut state = self.mind.state.lock().unwrap();
            let refused = if state.counted == self.mind.brief.guesses {
                Some(Entry::Uncounted {
                    from: from.clone(),
                    n,
                })
            } else if !state.put.insert(folded(text)) {
                Some(Entry::Repeated {
                    from: from.clone(),
                    n,
                })
            } else {
                None
            };
            if let Some(refused) = refused {
                drop(state);
                self.context.log(refused);
                return Ok(());
            }
            state.counted += 1;
            if state
                .object
                .as_deref()
                .is_some_and(|object| says(text, object))
            {
                let used = state.counted;
                drop(state);
                return self.finish(Victory::Named, used);
            }
            state.quiet_since = Some(Instant::now());
            state.nudged = false;
            let waiting = Waiting {
                number: state.counted,
                from,
                text: text.clone(),
            };
            state.waiting.push_back(waiting);
        } else if let Message::Say { text, .. } = message {
            let line = format!("{from}: \"{text}\"");
            self.mind.heard.lock().unwrap().push((line, ()));
        }
        match self.mind.state.lock().unwrap().over {
            true => Ok(()),
            false => self.wake(),
        }
    }

    /// Return why the episode failed, if it did.
    async fn clean_up(&mut self) -> anyhow::Result<()> {
        match self.mind.failure.lock().unwrap().take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl Think for Host {
    type Message = Message;

    /// Woken, decide what to do. At the time limit, end the game unless it
    /// is over already. Nothing else is thought about.
    async fn think(&mut self, message: &Message) -> anyhow::Result<()> {
        match message {
            Message::Wake => self.decide().await,
            Message::Over if !self.mind.state.lock().unwrap().over => self.end(Victory::OutOfTime),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::tests::{Line, Played, Puppet, Rules, configuration, hosting, table};
    use crate::model::ANSWER;
    use crate::model::canned::canned;
    use free_agent_model::fake::FakeModel;
    use std::sync::atomic::AtomicUsize;

    /// Play a game under `rules` whose host asks `model`, each player
    /// saying its script, each line its seconds after the beginning.
    async fn hosted(
        model: &Arc<FakeModel>,
        rules: Rules,
        scripts: &[(&str, &[(u64, Line)])],
    ) -> Played {
        hosting(model, rules, |players, player| {
            let script =
                scripts
                    .iter()
                    .find(|(name, _)| name == player)
                    .map_or(vec![], |(_, script)| {
                        script
                            .iter()
                            .map(|(seconds, line)| (Duration::from_secs(*seconds), *line))
                            .collect()
                    });
            Puppet::init(players, script)
        })
        .await
    }

    /// The guess a request asks the host to answer.
    fn guess_in(request: &Request) -> String {
        let asked = &request.messages.last().unwrap().content;
        let (_, guess) = asked.rsplit_once(" guesses: \"").unwrap();
        guess.split_once('"').unwrap().0.to_string()
    }

    /// Whether `request` asks the host to answer a guess.
    fn answering_a_guess(request: &Request) -> bool {
        request.tools.iter().any(|tool| tool.name == NAMED)
    }

    /// A call to `answer` saying `text`.
    fn answer(text: &str) -> Response {
        Response::tool_call(ANSWER_TOOL, &json!({"text": text}))
    }

    /// A call to `speak` saying `text`.
    fn speaking(text: &str) -> Response {
        Response::tool_call(SPEAK, &json!({"text": text}))
    }

    /// A host model that answers each guess with `answer` of it, taking
    /// `delay` over each.
    fn answering(
        answer: impl Fn(&str) -> anyhow::Result<Response> + Send + Sync + 'static,
        delay: u64,
    ) -> Arc<FakeModel> {
        Arc::new(
            FakeModel::answering(move |request| answer(&guess_in(request)))
                .taking(Duration::from_secs(delay)),
        )
    }

    /// Each answer the host sent, as the guess it answers, who asked it,
    /// its text, whether it names the object, and how many guesses are left.
    fn answers(played: &Played) -> Vec<(u64, &str, &str, bool, u64)> {
        played
            .sent_by(HOST)
            .into_iter()
            .filter_map(|message| match message {
                Message::Answer {
                    guess,
                    asked_by,
                    text,
                    named,
                    left,
                    ..
                } => Some((*guess, asked_by.as_str(), text.as_str(), *named, *left)),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn a_guess_saying_the_object_s_name_wins_the_moment_it_is_counted() {
        // The host was still answering the fourteenth guess when the
        // fifteenth named the orangutan, and the time ran out first.
        let model = answering(|_| Ok(answer("No.")), 50);
        let played = hosted(
            &model,
            Rules {
                time_limit: Duration::from_secs(60),
                ..Rules::default()
            },
            &[
                ("Alice", &[(1, Line::Guess("Is it alive?"))]),
                ("Bob", &[(2, Line::Guess("Is it a TEAPOT?"))]),
            ],
        )
        .await;
        played.ran.as_ref().unwrap();
        assert!(answers(&played).is_empty());
        assert_eq!(
            played.end(),
            (
                Side::Players,
                Victory::Named,
                2,
                "a teapot",
                Duration::from_secs(2)
            )
        );
        assert_eq!(played.won, Some(Side::Players));
        let asked: Vec<String> = model.requests().iter().map(guess_in).collect();
        assert_eq!(asked, ["Is it alive?"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_guess_the_host_calls_named_wins_in_words_of_the_name() {
        let model = answering(
            |_| Ok(Response::tool_call(NAMED, &json!({"text": "Yes!"}))),
            1,
        );
        let script = [
            (1, Line::Guess("Is it a kettle?")),
            (5, Line::Guess("Is it a pot for tea?")),
        ];
        let played = hosted(
            &model,
            Rules {
                object: Some("a china tea pot"),
                ..Rules::default()
            },
            &[("Bob", &script)],
        )
        .await;
        // A guess that names no word of the object's name is only answered
        // Yes, however the host judged it: the host called "Is it
        // furniture?" a loveseat.
        assert_eq!(
            answers(&played),
            [(1, "Bob", "Yes.", false, 19), (2, "Bob", "Yes!", true, 18)]
        );
        assert_eq!(played.end().1, Victory::Named);
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_saying_the_name_before_the_game_is_won_is_not_sent_and_is_asked_again() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let model = answering(
            move |_| match counted.fetch_add(1, Ordering::Relaxed) {
                0 => Ok(answer("A teapot is found in a kitchen.")),
                _ => Ok(answer("Yes.")),
            },
            1,
        );
        let script = [(1, Line::Guess("Is it found in a kitchen?"))];
        let played = hosted(
            &model,
            Rules {
                time_limit: Duration::from_secs(60),
                ..Rules::default()
            },
            &[("Alice", &script)],
        )
        .await;
        assert_eq!(
            played.failures(HOST),
            ["the host's answer gives the secret object away: A teapot is found in a kitchen."]
        );
        assert_eq!(answers(&played), [(1, "Alice", "Yes.", false, 19)]);
        assert_eq!(played.end().1, Victory::OutOfTime);
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_saying_the_name_after_yes_or_no_is_cut_to_yes_or_no() {
        let model = answering(
            |guess| {
                Ok(answer(match guess {
                    "Is it found in a kitchen?" => "Yes, a teapot is found in a kitchen.",
                    _ => "NO! Teapots are not alive.",
                }))
            },
            1,
        );
        let script = [
            (1, Line::Guess("Is it found in a kitchen?")),
            (5, Line::Guess("Is it alive?")),
        ];
        let played = hosted(
            &model,
            Rules {
                time_limit: Duration::from_secs(60),
                ..Rules::default()
            },
            &[("Alice", &script)],
        )
        .await;
        assert!(played.failures(HOST).is_empty());
        assert_eq!(
            answers(&played),
            [
                (1, "Alice", "Yes.", false, 19),
                (2, "Alice", "No.", false, 18)
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn guesses_are_counted_and_answered_one_at_a_time_in_the_order_received() {
        let model = answering(|guess| Ok(answer(&format!("No to {guess}"))), 10);
        let played = hosted(
            &model,
            Rules {
                guesses: 3,
                ..Rules::default()
            },
            &[
                ("Alice", &[(1, Line::Guess("A1")), (3, Line::Guess("A2"))]),
                ("Bob", &[(2, Line::Guess("B1"))]),
            ],
        )
        .await;
        assert_eq!(
            answers(&played),
            [
                (1, "Alice", "No to A1", false, 0),
                (2, "Bob", "No to B1", false, 0),
                (3, "Alice", "No to A2", false, 0),
            ]
        );
        let asked: Vec<String> = model
            .requests()
            .iter()
            .filter(|request| answering_a_guess(request))
            .map(guess_in)
            .collect();
        assert_eq!(asked, ["A1", "B1", "A2"]);
        assert_eq!(played.end().1, Victory::OutOfGuesses);
        assert_eq!(played.won, Some(Side::Host));
    }

    #[tokio::test(start_paused = true)]
    async fn a_guess_after_the_last_is_neither_counted_nor_answered() {
        let model = answering(|_| Ok(answer("No.")), 10);
        let script = [
            (1, Line::Guess("Is it alive?")),
            (2, Line::Guess("Is it big?")),
            (3, Line::Guess("Is it red?")),
        ];
        let played = hosted(
            &model,
            Rules {
                guesses: 2,
                ..Rules::default()
            },
            &[("Alice", &script)],
        )
        .await;
        assert_eq!(answers(&played).len(), 2);
        let uncounted: Vec<&Entry> = played
            .entries()
            .into_iter()
            .filter(|entry| matches!(entry, Entry::Uncounted { .. }))
            .collect();
        assert_eq!(
            uncounted,
            [&Entry::Uncounted {
                from: "Alice".to_string(),
                n: 3
            }]
        );
        assert_eq!(played.end().2, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_guess_repeating_one_put_already_is_neither_counted_nor_answered() {
        let model = answering(|_| Ok(answer("No.")), 10);
        let played = hosted(
            &model,
            Rules::default(),
            &[
                ("Alice", &[(1, Line::Guess("Is it big?"))]),
                (
                    "Bob",
                    &[
                        (2, Line::Guess(" is it BIG? ")),
                        (30, Line::Guess("Is it red?")),
                    ],
                ),
            ],
        )
        .await;
        assert_eq!(
            answers(&played),
            [(1, "Alice", "No.", false, 19), (2, "Bob", "No.", false, 18)]
        );
        let repeated: Vec<&Entry> = played
            .entries()
            .into_iter()
            .filter(|entry| matches!(entry, Entry::Repeated { .. }))
            .collect();
        assert_eq!(
            repeated,
            [&Entry::Repeated {
                from: "Bob".to_string(),
                n: 1
            }]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn at_the_time_limit_the_host_wins_and_a_guess_still_waiting_is_not_answered() {
        let model = answering(|_| Ok(answer("No.")), 50);
        let played = hosted(
            &model,
            Rules {
                time_limit: Duration::from_secs(60),
                ..Rules::default()
            },
            &[
                ("Alice", &[(1, Line::Guess("Is it alive?"))]),
                ("Bob", &[(2, Line::Guess("Is it big?"))]),
            ],
        )
        .await;
        played.ran.as_ref().unwrap();
        assert_eq!(answers(&played), [(1, "Alice", "No.", false, 18)]);
        assert_eq!(
            played.end(),
            (
                Side::Host,
                Victory::OutOfTime,
                1,
                "a teapot",
                Duration::from_secs(101)
            )
        );
        assert_eq!(played.won, Some(Side::Host));
    }

    #[tokio::test(start_paused = true)]
    async fn a_quiet_game_ends_at_its_time_limit() {
        let model = answering(|_| Ok(answer("No.")), 1);
        let played = hosted(
            &model,
            Rules {
                time_limit: Duration::from_secs(30),
                ..Rules::default()
            },
            &[],
        )
        .await;
        assert_eq!(
            played.end(),
            (
                Side::Host,
                Victory::OutOfTime,
                0,
                "a teapot",
                Duration::from_secs(30)
            )
        );
        assert!(model.requests().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn the_host_answers_a_waiting_guess_before_it_speaks() {
        let model = Arc::new(
            FakeModel::answering(|request| match answering_a_guess(request) {
                true => Ok(answer(&format!("No to {}", guess_in(request)))),
                false => Ok(speaking("Is that your guess, Alice?")),
            })
            .taking(Duration::from_secs(5)),
        );
        let script = [
            (1, Line::Guess("G1")),
            (2, Line::Say("Host, is it big?")),
            (3, Line::Guess("G2")),
        ];
        let played = hosted(
            &model,
            Rules {
                time_limit: Duration::from_secs(60),
                ..Rules::default()
            },
            &[("Alice", &script)],
        )
        .await;
        let said: Vec<String> = played
            .sent_by(HOST)
            .into_iter()
            .filter_map(|message| match message {
                Message::Answer { text, .. } | Message::Say { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(said, ["No to G1", "No to G2", "Is that your guess, Alice?"]);
        let talk = model
            .requests()
            .into_iter()
            .find(|request| !answering_a_guess(request))
            .unwrap();
        assert!(
            talk.messages[1].content.starts_with(&format!(
                "{HEARD}\nAlice: \"Host, is it big?\"\n\nGuesses used: 2 of 20."
            )),
            "{}",
            talk.messages[1].content
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_host_is_asked_to_speak_only_when_spoken_to_or_once_to_urge_a_quiet_table() {
        let model = Arc::new(FakeModel::responding(speaking("Put a guess, please.")));
        let script = [
            (1, Line::Say("Let's ask whether it's alive.")),
            (130, Line::Say("What now?")),
            (140, Line::Say("Hmm.")),
        ];
        let played = hosted(
            &model,
            Rules {
                time_limit: Duration::from_secs(200),
                ..Rules::default()
            },
            &[("Alice", &script)],
        )
        .await;
        let asked: Vec<String> = model
            .requests()
            .into_iter()
            .map(|request| request.messages[1].content.clone())
            .collect();
        assert_eq!(asked.len(), 1, "{asked:?}");
        let urged = &asked[0];
        let heard = format!(
            "{HEARD}\nAlice: \"What now?\"\n\nGuesses used: 0 of 20. Time left: 1m 10s. The \
             last guess came 2m 10s ago"
        );
        assert!(urged.starts_with(&heard), "{urged}");
        assert_eq!(played.sent_by(HOST).len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn speech_reaches_everyone_and_the_host_s_reaches_every_player() {
        let model = Arc::new(FakeModel::responding(speaking(
            "Is that your guess, Alice?",
        )));
        let played = hosted(
            &model,
            Rules {
                time_limit: Duration::from_secs(30),
                ..Rules::default()
            },
            &[("Alice", &[(1, Line::Say("Host, is it big?"))])],
        )
        .await;
        let observed = |by: &str| -> Vec<(String, u64)> {
            played
                .entries()
                .into_iter()
                .filter_map(|entry| match entry {
                    Entry::Observation { by: who, from, n } if who == by => {
                        Some((from.clone(), *n))
                    }
                    _ => None,
                })
                .collect()
        };
        assert_eq!(observed(HOST), [("Alice".to_string(), 1)]);
        assert_eq!(
            observed("Bob"),
            [
                (HOST.to_string(), 1),
                ("Alice".to_string(), 1),
                (HOST.to_string(), 2)
            ]
        );
        assert_eq!(
            observed("Alice"),
            [(HOST.to_string(), 1), (HOST.to_string(), 2)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_host_retries_a_failed_answer_and_the_episode_fails_after_the_third() {
        let model = Arc::new(FakeModel::failing("no model"));
        let played = hosted(
            &model,
            Rules::default(),
            &[("Alice", &[(1, Line::Guess("Is it alive?"))])],
        )
        .await;
        assert_eq!(played.failures(HOST), ["no model"; ATTEMPTS as usize]);
        let error = played.ran.unwrap_err();
        assert_eq!(format!("{error:#}"), "actor Host failed: no model");
        assert_eq!(model.requests().len(), ATTEMPTS as usize);
        assert_eq!(played.won, None);
    }

    #[tokio::test(start_paused = true)]
    async fn the_host_waits_before_it_asks_again() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let at = Arc::clone(&asked);
        let model = answering(
            move |_| {
                at.lock().unwrap().push(Instant::now());
                Err(anyhow!("busy"))
            },
            0,
        );
        hosted(
            &model,
            Rules::default(),
            &[("Alice", &[(1, Line::Guess("Is it alive?"))])],
        )
        .await;
        let asked = asked.lock().unwrap();
        let waits: Vec<Duration> = asked.windows(2).map(|at| at[1] - at[0]).collect();
        assert_eq!(waits, [RETRY; ATTEMPTS as usize - 1]);
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_that_is_not_well_formed_fails() {
        let model = answering(|_| Ok(speaking("Hmm.")), 1);
        let played = hosted(
            &model,
            Rules::default(),
            &[("Alice", &[(1, Line::Guess("Is it alive?"))])],
        )
        .await;
        assert_eq!(
            played.failures(HOST),
            ["the model called speak, which is not a tool it was offered"; ATTEMPTS as usize]
        );
        assert!(played.ran.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn the_pick_is_asked_at_its_temperature_for_the_category_drawn_before_the_game_begins() {
        let model = Arc::new(FakeModel::answering(canned).taking(Duration::from_secs(1)));
        let played = hosted(
            &model,
            Rules {
                object: None,
                ..Rules::default()
            },
            &[(
                "Alice",
                &[
                    (1, Line::Guess("Is it alive?")),
                    (5, Line::Guess("Is it a teapot?")),
                ],
            )],
        )
        .await;
        let pick = &model.requests()[0];
        assert_eq!(pick.temperature, Some(1.2));
        assert_eq!(
            pick.messages,
            [
                free_agent_model::Message::system(PICKER),
                free_agent_model::Message::user(format!("Pick an animal. {ANSWER}"))
            ]
        );
        assert_eq!(pick.tools, [choose()]);
        let entries = played.entries();
        let chosen = entries
            .iter()
            .position(|entry| matches!(entry, Entry::Chosen { .. }))
            .unwrap();
        assert_eq!(
            entries[chosen],
            &Entry::Chosen {
                object: "a teapot".to_string(),
                category: Some("an animal".to_string())
            }
        );
        let begun = entries
            .iter()
            .position(|entry| {
                matches!(
                    entry,
                    Entry::Action {
                        message: Message::Begin { .. },
                        ..
                    }
                )
            })
            .unwrap();
        assert!(chosen < begun);
        assert_eq!(
            played.end(),
            (
                Side::Players,
                Victory::Named,
                2,
                "a teapot",
                Duration::from_secs(5)
            )
        );
        // The host's prompt is rendered for the object it picked.
        let answering = model
            .requests()
            .into_iter()
            .find(answering_a_guess)
            .unwrap();
        assert_eq!(
            answering.messages[0].content,
            "You are the host. The secret object is a teapot."
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_picked_object_is_put_as_the_host_said_it() {
        let model = Arc::new(FakeModel::answering(|request| {
            match request.tools.iter().any(|tool| tool.name == CHOOSE) {
                true => Ok(Response::tool_call(CHOOSE, &json!({"object": "durian"}))),
                false => Ok(answer("Yes.")),
            }
        }));
        hosted(
            &model,
            Rules {
                object: None,
                ..Rules::default()
            },
            &[("Alice", &[(1, Line::Guess("Is it a fruit?"))])],
        )
        .await;
        let answering = model
            .requests()
            .into_iter()
            .find(answering_a_guess)
            .unwrap();
        assert_eq!(
            answering.messages[0].content,
            "You are the host. The secret object is durian."
        );
        assert!(
            answering.messages[1]
                .content
                .contains("\"Is durian a fruit?\""),
            "{}",
            answering.messages[1].content
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_pick_is_asked_again_and_the_episode_fails_after_the_third() {
        let model = Arc::new(FakeModel::responding(Response::tool_call(
            CHOOSE,
            &json!({"object": "  "}),
        )));
        let played = hosted(
            &model,
            Rules {
                object: None,
                ..Rules::default()
            },
            &[],
        )
        .await;
        assert_eq!(
            played.failures(HOST),
            ["the model chose nothing"; ATTEMPTS as usize]
        );
        assert!(played.ran.is_err());
        assert!(played.sent_by(HOST).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn the_host_s_words_that_say_the_name_are_not_sent_and_it_is_asked_again_up_to_three_times()
     {
        let leaking = Arc::new(FakeModel::responding(Response::calling(&[
            (SPEAK.to_string(), json!({"text": "Fine."})),
            (SPEAK.to_string(), json!({"text": "It's a teapot!"})),
        ])));
        let script = [(1, Line::Say("Host, what is it?"))];
        let rules = || Rules {
            time_limit: Duration::from_secs(60),
            ..Rules::default()
        };
        let played = hosted(&leaking, rules(), &[("Alice", &script)]).await;
        assert_eq!(
            played.failures(HOST),
            ["the host's words give the secret object away: It's a teapot!"; ATTEMPTS as usize]
        );
        assert_eq!(played.sent_by(HOST).len(), 1, "only the beginning");
        // A second word after giving up is asked of the new speech alone.
        let script = [(1, Line::Say("Host?")), (30, Line::Say("Host, anyone?"))];
        let played = hosted(&leaking, rules(), &[("Alice", &script)]).await;
        let last = leaking.requests().pop().unwrap().messages[1]
            .content
            .clone();
        let heard = format!("{HEARD}\nAlice: \"Host, anyone?\"\n\n");
        assert!(last.starts_with(&heard), "{last}");
        assert_eq!(played.failures(HOST).len(), 2 * ATTEMPTS as usize);
    }

    #[tokio::test(start_paused = true)]
    async fn a_word_or_an_answer_the_model_gives_after_the_time_limit_ends_the_game_unsaid() {
        for (script, said) in [
            ([(1, Line::Say("Host, is it big?"))], "a word"),
            ([(1, Line::Guess("Is it big?"))], "an answer"),
        ] {
            let model = Arc::new(
                FakeModel::answering(|request| match answering_a_guess(request) {
                    true => Ok(answer("No.")),
                    false => Ok(speaking("Is that your guess?")),
                })
                .taking(Duration::from_secs(100)),
            );
            let played = hosted(
                &model,
                Rules {
                    time_limit: Duration::from_secs(60),
                    ..Rules::default()
                },
                &[("Alice", &script)],
            )
            .await;
            assert_eq!(played.sent_by(HOST).len(), 1, "{said}");
            assert_eq!(played.end().1, Victory::OutOfTime, "{said}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_host_logs_the_table_and_the_configuration_as_it_starts() {
        let model = answering(|_| Ok(answer("No.")), 1);
        let played = hosted(
            &model,
            Rules {
                time_limit: Duration::from_secs(30),
                ..Rules::default()
            },
            &[],
        )
        .await;
        let entries = played.entries();
        assert_eq!(
            entries[..2],
            [
                &Entry::Start {
                    strategies: table()
                        .into_iter()
                        .map(|player| (player, "puppet".to_string()))
                        .collect(),
                    guesses: 20,
                    time_limit: Duration::from_secs(30),
                },
                &Entry::Configuration(Box::new(configuration())),
            ]
        );
        assert_eq!(
            played.sent_by(HOST),
            [&Message::Begin {
                n: 1,
                view: View {
                    players: table(),
                    guesses: 20,
                    time_limit: Duration::from_secs(30),
                },
            }]
        );
    }

    #[test]
    fn an_answer_is_its_first_call_to_answer_or_named() {
        assert_eq!(
            answered(&answer("No.")).unwrap(),
            ("No.".to_string(), false)
        );
        let named = Response::tool_call(NAMED, &json!({"text": "Yes!"}));
        assert_eq!(answered(&named).unwrap(), ("Yes!".to_string(), true));
        let nothing = Response::calling(&[]);
        assert_eq!(
            answered(&nothing).unwrap_err().to_string(),
            "the model made no tool call"
        );
        let blank = answer(" ");
        assert_eq!(
            answered(&blank).unwrap_err().to_string(),
            "the model's answer call says nothing"
        );
    }

    #[test]
    fn a_pick_is_a_call_to_choose_naming_something() {
        let chose = |object: &str| Response::tool_call(CHOOSE, &json!({"object": object}));
        assert_eq!(chosen(&chose(" a banjo ")).unwrap(), "a banjo");
        assert_eq!(
            chosen(&chose("pair_of_scissors")).unwrap(),
            "pair of scissors"
        );
        assert_eq!(
            chosen(&answer("No.")).unwrap_err().to_string(),
            "the model called answer, which is not a tool it was offered"
        );
        let wrong = Response::tool_call(CHOOSE, &json!({"thing": "a banjo"}));
        assert!(chosen(&wrong).is_err());
    }

    #[test]
    fn the_host_speaks_with_speak_and_says_nothing_with_wait() {
        let words = Response::calling(&[
            (SPEAK.to_string(), json!({"text": "Is that your guess?"})),
            (WAIT.to_string(), json!({})),
        ]);
        assert_eq!(spoken(&words).unwrap(), ["Is that your guess?"]);
        assert_eq!(
            spoken(&answer("No.")).unwrap_err().to_string(),
            "the model called answer, which is not a tool it was offered"
        );
    }

    #[test]
    fn a_player_speaks_to_the_host_by_calling_it_host_not_by_talking_about_it() {
        for line in [
            "Host, is it alive?",
            "Is it alive, host?",
            "Hey HOST! Is it red?",
            "Bob: \"host: is it big?\"",
        ] {
            assert!(addressed(line), "{line}");
        }
        for line in [
            "Considering the host's answers, it is not alive.",
            "Let's ask the host whether it is alive.",
            "The host said no.",
            "Our host’s answer was yes.",
            "A ghost story?",
            "Bob: \"It is not alive.\"",
        ] {
            assert!(!addressed(line), "{line}");
        }
    }

    #[test]
    fn a_guess_is_asked_of_the_host_about_the_object_itself() {
        let asked = super::answering("Alice", "Is it alive?", "a unicycle");
        assert!(
            asked.starts_with("Alice guesses: \"Is it alive?\""),
            "{asked}"
        );
        assert!(asked.contains("\"Is a unicycle alive?\""), "{asked}");
    }

    #[test]
    fn the_host_s_requests_take_the_largest_of_an_answer_a_word_and_a_pick() {
        let small = unstoried(
            "qwen",
            "You are the host.",
            &[],
            20,
            Duration::from_secs(600),
        );
        let picks = [(None, "Pick something. ".repeat(500))];
        let large = unstoried(
            "qwen",
            "You are the host.",
            &picks,
            20,
            Duration::from_secs(600),
        );
        assert!(small < large, "{small} {large}");
        let probe = probe_pick("qwen", ToolChoice::Auto);
        assert_eq!(probe.tool_choice, ToolChoice::Auto);
        assert_eq!(probe.temperature, Some(1.2));
    }
}
