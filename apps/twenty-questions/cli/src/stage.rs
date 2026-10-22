//! What the screen is drawn from: a game's log, folded an event at a time
//! into [`Cue`]s.
//!
//! A [`Stage`] reads the same events the console's narrator does and
//! keeps the narrator's words. The start becomes the cast, the secret
//! object is shown as soon as it is known, each guess the host counts is
//! a guess numbered as the host counts it, each answer an answer to its
//! guess, other speech a bubble from its speaker, the rest of the
//! narration a block of told lines, and the end of the game its outcome.

use crate::log::{Entry, Header};
use crate::message::{HOST, Message};
use crate::narrator::Narrator;
use anyhow::Context;
use free_agent::{ActorId, Event};
use free_agent_game::log::Ended;
use free_agent_game::report::{Line, in_order};
use free_agent_game::seat::PlayerId;
use serde::de::IgnoredAny;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use twenty_questions_gui_types::{Beat, Cue, Millis, Outcome, Part, Seat};
pub use twenty_questions_gui_types::{Opened, Recording, Recordings};
use uuid::Uuid;

/// Folds one game's log into cues, an event at a time.
#[derive(Debug, Default)]
pub struct Stage {
    /// The console's narrator, whose lines the cues carry and whose count
    /// of the guesses numbers them.
    narrator: Narrator,
    /// The players and the host whose model call is out.
    thinking: HashSet<ActorId>,
    /// The outcome, once a side has won, held for [`Stage::end`].
    won: Option<Cue>,
}

impl Stage {
    /// The cues `event` adds, each stamped with the event's time. The start
    /// is the cast, then the clock at its live rate. The object is the
    /// secret, from the configuration that names it or the host's pick,
    /// before what the narrator tells of it. The players told the game
    /// has begun is the countdown's start. A guess the host counts is
    /// that guess, numbered, and one after the last is uncounted; the
    /// host's answer is the answer to its guess. Everything else is what
    /// its narrator lines tell: each run of told lines one block, each
    /// utterance a bubble from its speaker, blank lines left out. A model
    /// call going out is its caller thinking, and coming back their
    /// thought. The end is what the narrator tells of it, then a thought
    /// for every call still out; its outcome waits for [`Stage::end`].
    /// Nothing follows the end.
    pub fn fold(&mut self, event: &Event<Entry>) -> Vec<Cue> {
        let at = millis(event.time);
        let (over, counted) = (self.narrator.is_over(), self.narrator.count());
        let lines = self.narrator.narrate(event);
        if over {
            return vec![];
        }
        let beats = match &event.payload {
            Entry::Start {
                strategies,
                guesses,
                time_limit,
            } => vec![
                cast(strategies, *guesses, *time_limit),
                Beat::Clock { rate: 1.0 },
            ],
            Entry::Configuration(configuration) => {
                let object = configuration.host.object.as_deref();
                object
                    .map(secret)
                    .into_iter()
                    .chain(beats(&lines))
                    .collect()
            }
            Entry::Chosen { object, .. } => {
                [secret(object)].into_iter().chain(beats(&lines)).collect()
            }
            Entry::Action { message, .. } => acted(message, &lines),
            Entry::Observation { from, n, .. } if self.narrator.count() > counted => {
                vec![Beat::Guessed {
                    who: from.clone(),
                    text: self.asked(from, *n),
                    guess: self.narrator.count(),
                }]
            }
            Entry::Uncounted { from, n } => vec![Beat::Uncounted {
                who: from.clone(),
                text: self.asked(from, *n),
            }],
            Entry::Thinking { by } => {
                self.thinking.insert(by.clone());
                vec![Beat::Thinking { who: by.clone() }]
            }
            Entry::Thought { by, .. } => self.thought(by).into_iter().collect(),
            Entry::End { by, .. } => {
                let outcome = Outcome::Won {
                    by: *by,
                    how: last(&lines),
                };
                self.won = Some(Cue {
                    at,
                    beat: Beat::Over { outcome },
                });
                [beats(&lines), self.hushed()].concat()
            }
            _ => beats(&lines),
        };
        beats.into_iter().map(|beat| Cue { at, beat }).collect()
    }

    /// Whether a side has won.
    pub fn is_won(&self) -> bool {
        self.won.is_some()
    }

    /// The last cues of a game that `ended` as it did at `at`: a thought
    /// for every call still out, the host's first and then the players' in
    /// seat order, then the outcome. A failure tells its error first and is
    /// a failure even after a win. Otherwise a game won is won, and one with
    /// no end is [`Outcome::Unfinished`].
    pub fn end(&mut self, at: Millis, ended: Option<Ended>) -> Vec<Cue> {
        let mut beats = self.hushed();
        let won = self.won.take();
        let over = |outcome| Cue {
            at,
            beat: Beat::Over { outcome },
        };
        let last = match ended {
            Some(Ended::Failed(error)) => {
                beats.push(Beat::Told(vec![error.clone()]));
                over(Outcome::Failed(error))
            }
            Some(_) => won.unwrap_or_else(|| over(Outcome::Stopped)),
            None => won.unwrap_or_else(|| over(Outcome::Unfinished)),
        };
        let mut cues: Vec<Cue> = beats.into_iter().map(|beat| Cue { at, beat }).collect();
        cues.push(last);
        cues
    }

    /// The text of the guess `from` numbered `n`.
    fn asked(&self, from: &ActorId, n: u64) -> String {
        self.narrator.asked(from, n).unwrap_or_default().to_string()
    }

    /// `who`'s thought, if their call is out.
    fn thought(&mut self, who: &ActorId) -> Option<Beat> {
        self.thinking
            .remove(who)
            .then(|| Beat::Thought { who: who.clone() })
    }

    /// The thoughts of everyone still thinking: the host's first, then the
    /// players' in seat order.
    fn hushed(&mut self) -> Vec<Beat> {
        let thinking = std::mem::take(&mut self.thinking);
        let mut hushed = in_order(&thinking);
        hushed.sort_by_key(|who| *who != HOST);
        hushed
            .into_iter()
            .map(|who| Beat::Thought { who: who.clone() })
            .collect()
    }
}

/// What sending `message`, told as `lines`, adds: the beginning starts the
/// countdown, an answer is shown with its guess, and speech is said. A
/// guess waits for the host to count it.
fn acted(message: &Message, lines: &[Line]) -> Vec<Beat> {
    match message {
        Message::Begin { .. } => vec![Beat::Began],
        Message::Answer {
            guess,
            text,
            named,
            left,
            ..
        } => vec![Beat::Answered {
            guess: *guess,
            text: text.clone(),
            named: *named,
            left: *left,
        }],
        _ => beats(lines),
    }
}

/// The episodes of a log written as JSON Lines, each told apart by its ID,
/// wherever its lines fall.
///
/// Each recording ends with one [`Beat::Over`], after a thought for every
/// call still out: as its episode ended, with the error told first when it
/// failed. An episode won and no more is won, and one cut off before that
/// is [`Outcome::Unfinished`].
///
/// A line that is no event at all belongs to the episode of the line
/// before it. An episode with a bad line is left out, and the problem
/// names the line by its number.
///
/// # Errors
///
/// Every problem, when no episode reads.
pub fn recordings(log: &str) -> anyhow::Result<Recordings> {
    let mut episodes: Vec<(Uuid, Vec<(usize, &str)>)> = Vec::new();
    let mut found = HashMap::new();
    let mut problems = Vec::new();
    let mut current = None;
    for (index, line) in log.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let number = index + 1;
        let episode = match (serde_json::from_str::<Event<IgnoredAny>>(line), current) {
            (Ok(event), _) => event.episode,
            (Err(_), Some(episode)) => episode,
            (Err(error), None) => {
                problems.push(format!("the log is bad at line {number}: {error}"));
                continue;
            }
        };
        current = Some(episode);
        let at = *found.entry(episode).or_insert_with(|| {
            episodes.push((episode, Vec::new()));
            episodes.len() - 1
        });
        episodes[at].1.push((number, line));
    }
    let mut games = Vec::new();
    for (episode, lines) in episodes {
        match recording(episode, &lines) {
            Ok(recording) => games.push(recording),
            Err(error) => problems.push(format!("{error:#}")),
        }
    }
    anyhow::ensure!(
        !games.is_empty() || problems.is_empty(),
        "{}",
        problems.join("\n")
    );
    Ok(Recordings { games, problems })
}

/// The log at `path`, opened for review under its file's name, as
/// [`recordings`] reads it.
///
/// # Errors
///
/// When the file cannot be read, when no episode in it reads, or when it
/// holds none.
pub fn opened(path: &Path) -> anyhow::Result<Opened> {
    let log = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    let name = path.file_name().unwrap_or(path.as_os_str());
    let name = name.to_string_lossy().into_owned();
    let recordings = recordings(&log)?;
    anyhow::ensure!(!recordings.games.is_empty(), "{name} holds no games.");
    Ok(Opened { name, recordings })
}

/// The episode `episode`, its `lines` numbered, its header checked if its
/// first line is one, folded into cues and ended as [`recordings`] ends it.
fn recording(episode: Uuid, lines: &[(usize, &str)]) -> anyhow::Result<Recording> {
    let (number, first) = lines[0];
    Header::check(first).with_context(|| format!("episode {episode} is bad at line {number}"))?;
    let mut stage = Stage::default();
    let mut cues = Vec::new();
    let mut ended = None;
    let mut at = 0;
    for &(number, line) in lines {
        let event: Event<Entry> = serde_json::from_str(line)
            .with_context(|| format!("episode {episode} is bad at line {number}"))?;
        at = millis(event.time);
        if let Entry::Ended(how) = &event.payload {
            ended = Some(how.clone());
        }
        cues.extend(stage.fold(&event));
    }
    cues.extend(stage.end(at, ended));
    Ok(Recording { episode, cues })
}

/// `time` in milliseconds since the Unix epoch.
pub(crate) fn millis(time: SystemTime) -> Millis {
    millis_of(time.duration_since(UNIX_EPOCH).unwrap_or_default())
}

/// `duration` in whole milliseconds.
fn millis_of(duration: Duration) -> Millis {
    Millis::try_from(duration.as_millis()).unwrap_or(Millis::MAX)
}

/// The table of a game dealt `strategies`, with `guesses` and
/// `time_limit`: the host, then every player in seat order, each with the
/// color of their place.
fn cast(strategies: &BTreeMap<PlayerId, String>, guesses: u64, time_limit: Duration) -> Beat {
    let host = Seat {
        name: HOST.to_string(),
        part: Part::Host,
    };
    let players = (0..).zip(strategies).map(|(color, (name, strategy))| Seat {
        name: name.clone(),
        part: Part::Player {
            color,
            strategy: strategy.clone(),
        },
    });
    Beat::Cast {
        seats: std::iter::once(host).chain(players).collect(),
        guesses,
        time_limit_ms: millis_of(time_limit),
    }
}

/// The secret `object`.
fn secret(object: &str) -> Beat {
    Beat::Secret {
        object: object.to_string(),
    }
}

/// The words of the last of narrator `lines`.
fn last(lines: &[Line]) -> String {
    lines.last().map(Line::to_string).unwrap_or_default()
}

/// What narrator `lines` show: each run of told lines as one block, and
/// each utterance, in order.
fn beats(lines: &[Line]) -> Vec<Beat> {
    let mut beats = Vec::new();
    let mut told = Vec::new();
    for line in lines {
        if let Line::Told(text) = line {
            told.push(text.clone());
            continue;
        }
        if !told.is_empty() {
            beats.push(Beat::Told(std::mem::take(&mut told)));
        }
        if let Line::Said { who, text, .. } = line {
            beats.push(Beat::Said {
                who: who.clone(),
                text: text.clone(),
            });
        }
    }
    if !told.is_empty() {
        beats.push(Beat::Told(told));
    }
    beats
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::game::tests::{Line as Script, Played, Puppet, Rules, configuration, hosting};
    use crate::log::tests::logged;
    use crate::log::{Victory, Writer};
    use crate::model::canned::canned;
    use free_agent_game::log::Secrets;
    use free_agent_model::fake::FakeModel;
    use std::sync::Arc;

    /// A game under `rules` whose host plays as the canned host does,
    /// taking a second over each call, each player saying its script, each
    /// line at its time after the beginning.
    pub(crate) async fn scripted(
        rules: Rules,
        scripts: &[(&str, &[(Duration, Script)])],
    ) -> Played {
        let model = Arc::new(FakeModel::answering(canned).taking(Duration::from_secs(1)));
        hosting(&model, rules, |players, player| {
            let script = scripts
                .iter()
                .find(|(name, _)| name == player)
                .map_or(vec![], |(_, script)| script.to_vec());
            Puppet::init(players, script)
        })
        .await
    }

    /// `seconds` after the beginning.
    fn after(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    /// A game the players win with their second guess, after some talk.
    pub(crate) async fn won() -> Played {
        let alice = [
            (after(1), Script::Say("Let's think.")),
            (after(2), Script::Guess("Is it alive?")),
        ];
        let bob = [(after(30), Script::Guess("Is it a teapot?"))];
        scripted(Rules::default(), &[("Alice", &alice), ("Bob", &bob)]).await
    }

    /// The cues a fresh stage folds `events` into, ended where they end.
    fn folded(events: &[Event<Entry>]) -> Vec<Cue> {
        let mut stage = Stage::default();
        let mut cues: Vec<Cue> = events.iter().flat_map(|event| stage.fold(event)).collect();
        let at = events.last().map_or(0, |event| millis(event.time));
        cues.extend(stage.end(at, None));
        cues
    }

    /// The beats of `cues` but the calls going out and coming back.
    fn shown(cues: &[Cue]) -> Vec<Beat> {
        cues.iter()
            .map(|cue| cue.beat.clone())
            .filter(|beat| !matches!(beat, Beat::Thinking { .. } | Beat::Thought { .. }))
            .collect()
    }

    fn said(who: &str, text: &str) -> Beat {
        Beat::Said {
            who: who.to_string(),
            text: text.to_string(),
        }
    }

    fn guessed(who: &str, text: &str, guess: u64) -> Beat {
        Beat::Guessed {
            who: who.to_string(),
            text: text.to_string(),
            guess,
        }
    }

    fn answered(guess: u64, text: &str, named: bool, left: u64) -> Beat {
        Beat::Answered {
            guess,
            text: text.to_string(),
            named,
            left,
        }
    }

    fn told(lines: &[&str]) -> Beat {
        Beat::Told(lines.iter().map(|line| line.to_string()).collect())
    }

    /// The game ended, won `by` as the narrator tells it, `how`.
    fn over(by: Victory, how: &str) -> Vec<Beat> {
        vec![
            told(&[how]),
            Beat::Over {
                outcome: Outcome::Won {
                    by,
                    how: how.to_string(),
                },
            },
        ]
    }

    /// A seat of the test's table, Alice's or Bob's, playing `puppet`.
    fn player(name: &str, color: u8) -> Seat {
        Seat {
            name: name.to_string(),
            part: Part::Player {
                color,
                strategy: "puppet".to_string(),
            },
        }
    }

    /// How every game at the test's table opens, under the configuration
    /// that names a teapot, given `guesses`, with `time_limit`.
    fn opening(guesses: u64, time_limit: Duration) -> Vec<Beat> {
        let host = Seat {
            name: HOST.to_string(),
            part: Part::Host,
        };
        vec![
            Beat::Cast {
                seats: vec![host, player("Alice", 0), player("Bob", 1)],
                guesses,
                time_limit_ms: millis_of(time_limit),
            },
            Beat::Clock { rate: 1.0 },
            secret("a teapot"),
            told(&["The host is thinking of a teapot."]),
            Beat::Began,
        ]
    }

    /// Check the calls `beats` show: every `Thinking` is closed by one
    /// `Thought` of its caller before `Over`.
    fn check_calls(beats: &[Beat]) {
        let mut thinking = HashSet::new();
        let mut calls = 0;
        for beat in beats {
            match beat {
                Beat::Thinking { who } => {
                    assert!(thinking.insert(who), "{who} thinks twice");
                    calls += 1;
                }
                Beat::Thought { who } => assert!(thinking.remove(who), "{who} thought unasked"),
                Beat::Over { .. } => assert!(thinking.is_empty(), "{thinking:?}"),
                _ => {}
            }
        }
        assert!(calls > 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_won_is_its_table_its_secret_its_talk_its_numbered_guesses_and_answers_and_its_end()
     {
        let played = won().await;
        let cues = folded(&played.events);
        let beats = [
            opening(20, after(600)),
            vec![
                said("Alice", "Let's think."),
                guessed("Alice", "Is it alive?", 1),
                answered(1, "No.", false, 19),
                guessed("Bob", "Is it a teapot?", 2),
            ],
            over(
                Victory::Named,
                "The players named a teapot with 2 of their 20 guesses, in 30s.",
            ),
        ]
        .concat();
        assert_eq!(shown(&cues), beats);
        let every: Vec<Beat> = cues.into_iter().map(|cue| cue.beat).collect();
        check_calls(&every);
    }

    #[tokio::test(start_paused = true)]
    async fn a_guess_after_the_last_is_uncounted_and_the_last_answered_wins_for_the_host() {
        let alice = [(after(1), Script::Guess("Is it alive?"))];
        let bob = [(
            after(1) + Duration::from_millis(500),
            Script::Guess("Is it big?"),
        )];
        let rules = Rules {
            guesses: 1,
            ..Rules::default()
        };
        let played = scripted(rules, &[("Alice", &alice), ("Bob", &bob)]).await;
        let beats = [
            opening(1, after(600)),
            vec![
                guessed("Alice", "Is it alive?", 1),
                Beat::Uncounted {
                    who: "Bob".to_string(),
                    text: "Is it big?".to_string(),
                },
                answered(1, "No.", false, 0),
            ],
            over(
                Victory::OutOfGuesses,
                "The host wins: the players used all 1 guesses without naming a teapot, in 2s.",
            ),
        ]
        .concat();
        assert_eq!(shown(&folded(&played.events)), beats);
    }

    #[tokio::test(start_paused = true)]
    async fn a_guess_repeating_one_counted_is_told_and_not_numbered() {
        let alice = [(after(1), Script::Guess("Is it alive?"))];
        let bob = [(after(1), Script::Guess("is it alive?"))];
        let rules = Rules {
            time_limit: after(30),
            ..Rules::default()
        };
        let played = scripted(rules, &[("Alice", &alice), ("Bob", &bob)]).await;
        let beats = shown(&folded(&played.events));
        let guesses: Vec<&Beat> = beats
            .iter()
            .filter(|beat| matches!(beat, Beat::Guessed { .. }))
            .collect();
        assert_eq!(guesses.len(), 1, "{beats:?}");
        // Either may reach the host first.
        let put = [("Alice", "Is it alive?"), ("Bob", "is it alive?")];
        let (other, repeated) = put
            .into_iter()
            .find(
                |(who, _)| !matches!(guesses[0], Beat::Guessed { who: first, .. } if first == who),
            )
            .unwrap();
        assert!(
            beats.contains(&Beat::Told(vec![format!(
                "{other} repeats a guess put already: \"{repeated}\" It is not counted."
            )])),
            "{beats:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_quiet_game_ends_out_of_time() {
        let rules = Rules {
            time_limit: after(30),
            ..Rules::default()
        };
        let played = scripted(rules, &[]).await;
        let beats = [
            opening(20, after(30)),
            over(
                Victory::OutOfTime,
                "The host wins: time ran out after 30s, with 0 of 20 guesses used. It was a teapot.",
            ),
        ]
        .concat();
        assert_eq!(shown(&folded(&played.events)), beats);
    }

    /// `entries`, logged in order, each event's beats apart.
    fn staged(entries: &[Entry]) -> Vec<Vec<Beat>> {
        let mut stage = Stage::default();
        entries
            .iter()
            .map(|entry| {
                let event = logged(entry.clone());
                stage.fold(&event).into_iter().map(|cue| cue.beat).collect()
            })
            .collect()
    }

    #[test]
    fn an_object_the_host_picks_is_the_secret_once_picked() {
        let mut configured = configuration();
        configured.host.object = None;
        let chosen = Entry::Chosen {
            object: "a cat".to_string(),
            category: Some("an animal".to_string()),
        };
        assert_eq!(
            staged(&[Entry::Configuration(Box::new(configured)), chosen]),
            [
                vec![],
                vec![
                    secret("a cat"),
                    told(&["Asked for an animal, the host is thinking of a cat."])
                ]
            ]
        );
    }

    #[test]
    fn a_failure_to_act_is_told_and_nothing_follows_the_end() {
        let failed = Entry::Failed {
            by: HOST.to_string(),
            error: "no model".to_string(),
        };
        let end = Entry::End {
            winner: crate::log::Side::Host,
            by: Victory::OutOfTime,
            used: 0,
            object: "a cat".to_string(),
            took: after(600),
        };
        let thinking = Entry::Thinking {
            by: "Alice".to_string(),
        };
        let staged = staged(&[failed, end, thinking]);
        assert_eq!(staged[0], [told(&["Host fails to play: no model"])]);
        assert!(matches!(staged[1][..], [Beat::Told(_)]), "{:?}", staged[1]);
        assert_eq!(staged[2], []);
    }

    #[test]
    fn a_thought_with_no_call_out_shows_nothing() {
        let thinking = Entry::Thinking {
            by: "Alice".to_string(),
        };
        let thought = Entry::Thought {
            by: "Alice".to_string(),
            ending: crate::log::Ending::Answered,
        };
        let who = "Alice".to_string();
        assert_eq!(
            staged(&[thinking, thought.clone(), thought]),
            [
                vec![Beat::Thinking { who: who.clone() }],
                vec![Beat::Thought { who }],
                vec![]
            ]
        );
    }

    /// `events` written as a log is, in JSON Lines, with no header.
    fn written(events: &[Event<Entry>]) -> String {
        let mut written = Vec::new();
        for event in events {
            event.write(&mut written).unwrap();
        }
        String::from_utf8(written).unwrap()
    }

    /// `events` written as the log's writer writes them, as the episode
    /// `episode`, between its header and how it `ended`.
    fn recorded_as(episode: u128, events: &[Event<Entry>], ended: Ended) -> String {
        let episode = Uuid::from_u128(episode);
        let mut writer = Writer::start(Vec::new(), episode, Secrets::default()).unwrap();
        for event in events {
            writer
                .write(&Event {
                    episode,
                    ..event.clone()
                })
                .unwrap();
        }
        String::from_utf8(writer.end(ended).unwrap()).unwrap()
    }

    /// The cues of the one episode `log` holds.
    pub(crate) fn only(log: &str) -> Vec<Cue> {
        let Recordings {
            mut games,
            problems,
        } = recordings(log).unwrap();
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(games.len(), 1);
        games.remove(0).cues
    }

    /// The episodes `read` holds, in order.
    fn episodes(read: &Recordings) -> Vec<u128> {
        read.games
            .iter()
            .map(|game| game.episode.as_u128())
            .collect()
    }

    /// Check that `cues` start with the cast and end with their one
    /// outcome, `outcome`.
    fn check_whole(cues: &[Cue], outcome: impl Fn(&Outcome) -> bool) {
        assert!(matches!(cues[0].beat, Beat::Cast { .. }), "{:?}", cues[0]);
        let overs: Vec<&Outcome> = cues
            .iter()
            .filter_map(|cue| match &cue.beat {
                Beat::Over { outcome } => Some(outcome),
                _ => None,
            })
            .collect();
        assert_eq!(overs.len(), 1);
        assert!(matches!(cues.last().unwrap().beat, Beat::Over { .. }));
        assert!(outcome(overs[0]), "{:?}", overs[0]);
    }

    /// Whether `outcome` is a win.
    fn is_won(outcome: &Outcome) -> bool {
        matches!(outcome, Outcome::Won { .. })
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_folded_as_it_happens_and_one_read_back_from_its_log_agree() {
        let events = won().await.events;
        assert_eq!(only(&recorded_as(1, &events, Ended::Won)), folded(&events));
        assert_eq!(only(&written(&events)), folded(&events));
    }

    /// A log of the won game as `episodes`, and the cues each folds into.
    async fn won_log(episodes: impl IntoIterator<Item = u128>) -> (String, Vec<Vec<Cue>>) {
        let mut log = String::new();
        let mut each = Vec::new();
        for episode in episodes {
            let events = won().await.events;
            log += &recorded_as(episode, &events, Ended::Won);
            each.push(folded(&events));
        }
        (log, each)
    }

    #[tokio::test(start_paused = true)]
    async fn a_log_of_three_episodes_is_three_games_in_order() {
        let (log, each) = won_log([3, 1, 2]).await;
        let read = recordings(&log).unwrap();
        assert_eq!(episodes(&read), [3, 1, 2]);
        assert!(read.problems.is_empty());
        for (game, cues) in read.games.iter().zip(each) {
            check_whole(&game.cues, is_won);
            assert_eq!(game.cues, cues);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn episodes_whose_lines_are_interleaved_read_as_they_do_one_after_the_other() {
        let (one, _) = won_log([1]).await;
        let (other, _) = won_log([2]).await;
        let mut first = one.lines();
        let mut second = other.lines();
        let mut interleaved = String::new();
        loop {
            let (a, b) = (first.next(), second.next());
            if a.is_none() && b.is_none() {
                break;
            }
            for line in a.into_iter().chain(b) {
                interleaved += line;
                interleaved += "\n";
            }
        }
        let read = recordings(&interleaved).unwrap();
        assert_eq!(episodes(&read), [1, 2]);
        assert_eq!(read, recordings(&(one + &other)).unwrap());
    }

    /// The beats of the one episode `log` holds.
    fn beats_of(log: &str) -> Vec<Beat> {
        only(log).into_iter().map(|cue| cue.beat).collect()
    }

    #[test]
    fn a_game_stopped_ends_stopped_and_one_failed_tells_its_error_and_ends_failed() {
        let thinking = logged(Entry::Thinking {
            by: "Bob".to_string(),
        });
        let who = "Bob".to_string();
        let calls = vec![Beat::Thinking { who: who.clone() }, Beat::Thought { who }];
        let stopped = recorded_as(1, std::slice::from_ref(&thinking), Ended::Stopped);
        let over = |outcome| Beat::Over { outcome };
        assert_eq!(
            beats_of(&stopped),
            [calls.clone(), vec![over(Outcome::Stopped)]].concat()
        );
        let error = "actor Host failed: no model".to_string();
        let failed = recorded_as(1, &[thinking], Ended::Failed(error.clone()));
        assert_eq!(
            beats_of(&failed),
            [
                calls,
                vec![told(&[&error]), over(Outcome::Failed(error.clone()))]
            ]
            .concat()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_that_fails_after_a_win_ends_failed() {
        let events = won().await.events;
        let failed = recorded_as(1, &events, Ended::Failed("no model".to_string()));
        check_whole(&only(&failed), |outcome| {
            *outcome == Outcome::Failed("no model".to_string())
        });
    }

    #[test]
    fn calls_still_out_at_the_end_of_a_log_are_closed_there_host_first_and_it_is_unfinished() {
        // Ian sorts after the host by name.
        let thinking = |by: &str| logged(Entry::Thinking { by: by.to_string() });
        let log = written(&[thinking("Ian"), thinking(HOST), thinking("Alice")]);
        let who = |name: &str| name.to_string();
        let beats = beats_of(&log);
        assert_eq!(
            beats[3..],
            [
                Beat::Thought { who: who(HOST) },
                Beat::Thought { who: who("Alice") },
                Beat::Thought { who: who("Ian") },
                Beat::Over {
                    outcome: Outcome::Unfinished
                },
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_bad_line_leaves_out_its_episode_and_names_the_line() {
        let (log, _) = won_log(1..=3).await;
        let mut lines: Vec<&str> = log.lines().collect();
        let second = Uuid::from_u128(2).to_string();
        // The third line of the second episode, cut short.
        let bad = lines
            .iter()
            .position(|line| line.contains(&second))
            .unwrap()
            + 2;
        lines[bad] = &lines[bad][..20];
        let read = recordings(&lines.join("\n")).unwrap();
        assert_eq!(episodes(&read), [1, 3]);
        assert_eq!(read.problems.len(), 1);
        let number = bad + 1;
        let expected = format!("episode {second} is bad at line {number}: ");
        assert!(
            read.problems[0].starts_with(&expected),
            "{:?}",
            read.problems
        );
    }

    #[test]
    fn a_log_of_another_game_is_refused_at_its_header() {
        let header =
            serde_json::json!({"Header": {"format": 1, "version": "0.2.0", "game": "avalon"}});
        let error = format!("{:#}", recordings(&written_json(header)).unwrap_err());
        assert_eq!(
            error,
            format!(
                "episode {} is bad at line 1: This log is of avalon, not twenty-questions.",
                Uuid::default()
            )
        );
    }

    /// `payload`, logged, as a line of a log.
    fn written_json(payload: serde_json::Value) -> String {
        serde_json::to_string(&logged(payload)).unwrap() + "\n"
    }

    #[test]
    fn a_log_that_is_no_events_at_all_is_an_error_naming_each_line() {
        let error = recordings("{}\n\n[]").unwrap_err().to_string();
        let lines: Vec<&str> = error.lines().collect();
        assert_eq!(lines.len(), 2, "{error}");
        assert!(
            lines[0].starts_with("the log is bad at line 1: "),
            "{error}"
        );
        assert!(
            lines[1].starts_with("the log is bad at line 3: "),
            "{error}"
        );
    }

    #[test]
    fn an_empty_log_holds_no_recordings() {
        assert_eq!(recordings("\n").unwrap(), Recordings::default());
    }

    /// A log holding `text`, written for the test `test` at a path of its
    /// own so the tests do not collide.
    fn logged_at(test: &str, text: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "twenty-questions-{}-{test}.jsonl",
            std::process::id()
        ));
        std::fs::write(&path, text).unwrap();
        path
    }

    #[tokio::test(start_paused = true)]
    async fn a_log_opens_under_its_file_s_name_with_every_game() {
        let (log, _) = won_log([1, 2]).await;
        let path = logged_at("opens", &log);
        let opened = opened(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy();
        assert_eq!(opened.name, name);
        assert_eq!(episodes(&opened.recordings), [1, 2]);
    }

    #[test]
    fn a_log_with_no_games_or_no_file_does_not_open() {
        let path = logged_at("empty", "\n");
        let error = opened(&path).unwrap_err().to_string();
        std::fs::remove_file(&path).unwrap();
        assert!(error.ends_with("holds no games."), "{error}");
        let error = opened(&path).unwrap_err().to_string();
        assert!(error.starts_with(&path.display().to_string()), "{error}");
        let path = logged_at("bad", "not a log\n");
        let error = format!("{:#}", opened(&path).unwrap_err());
        std::fs::remove_file(&path).unwrap();
        assert!(error.contains("line 1"), "{error}");
    }

    #[test]
    fn a_cue_is_stamped_with_its_events_time_in_milliseconds() {
        assert_eq!(millis(UNIX_EPOCH + Duration::from_millis(1_234)), 1_234);
        assert_eq!(millis(UNIX_EPOCH - Duration::from_secs(1)), 0);
    }

    #[test]
    fn told_lines_run_together_until_something_else_comes_between() {
        let said_line = Line::Said {
            who: "Alice".to_string(),
            speaker: "Alice".to_string(),
            text: "Hello.".to_string(),
        };
        let lines = [
            Line::Told("a".to_string()),
            Line::Told("b".to_string()),
            Line::Blank,
            Line::Told("c".to_string()),
            said_line,
            Line::Told("d".to_string()),
        ];
        assert_eq!(
            beats(&lines),
            [
                told(&["a", "b"]),
                told(&["c"]),
                said("Alice", "Hello."),
                told(&["d"]),
            ]
        );
    }
}
