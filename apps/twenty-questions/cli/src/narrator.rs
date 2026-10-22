//! A legible account of a game, told from the log as it is written.
//!
//! The log is written in actions and observations, by every actor. Talk is
//! told from its speaker's action, and the host's answers from the host's.
//! A guess is told when the host observes it, numbered as the host counts
//! it, with the text its asker's action holds.

use crate::log::{Entry, Victory};
use crate::message::{HOST, Message, folded};
use free_agent::{ActorId, Event};
use free_agent_game::report::{Line, configured, in_prose, whole_seconds};
use free_agent_game::seat::PlayerId;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Turns the events of a game, one at a time, into lines of prose. It
/// tells who plays which strategy and what they have, each model in play,
/// the secret object, then everything said and every guess and answer as
/// it comes, and how the game ended. A blank line sets the talk off from
/// what came before it, and another the ending. A failure to act is told
/// as it happens, with what went wrong. Nothing is told after the end.
#[derive(Debug, Default)]
pub struct Narrator {
    /// How many guesses the players have.
    guesses: u64,
    /// The text of every guess, by its asker and the asker's number for it.
    asked: HashMap<(ActorId, u64), String>,
    /// How many guesses the host has counted.
    counted: u64,
    /// Every guess the host has counted, [`folded`].
    put: HashSet<String>,
    /// Whether the game is over.
    over: bool,
}

impl Narrator {
    /// The lines the log's `event` adds to the account, often none.
    pub fn narrate(&mut self, event: &Event<Entry>) -> Vec<Line> {
        if self.over {
            return vec![];
        }
        match &event.payload {
            Entry::Start {
                strategies,
                guesses,
                time_limit,
            } => {
                self.guesses = *guesses;
                let mut lines = playing(strategies);
                lines.push(Line::Told(format!(
                    "They have {guesses} guesses and {}.",
                    whole_seconds(*time_limit)
                )));
                lines
            }
            Entry::Configuration(configuration) => {
                let mut lines = configured(&configuration.models);
                if let Some(object) = &configuration.host.object {
                    lines.push(thinking_of(object));
                }
                lines
            }
            Entry::Chosen { object, category } => vec![match category {
                Some(category) => Line::Told(format!(
                    "Asked for {category}, the host is thinking of {object}."
                )),
                None => thinking_of(object),
            }],
            Entry::Action { message, .. } => self.acted(message),
            Entry::Observation { by, from, n } if by == HOST => self.counted(from, *n),
            Entry::Uncounted { from, n } => vec![Line::Told(format!(
                "{from} guesses after the last guess: {} It is not counted.",
                self.question(from, *n)
            ))],
            Entry::Repeated { from, n } => vec![Line::Told(format!(
                "{from} repeats a guess put already: {} It is not counted.",
                self.question(from, *n)
            ))],
            Entry::Failed { by, error } => vec![Line::Told(format!("{by} fails to play: {error}"))],
            Entry::End {
                by,
                used,
                object,
                took,
                ..
            } => {
                self.over = true;
                vec![
                    Line::Blank,
                    Line::Told(self.ended(*by, *used, object, *took)),
                ]
            }
            _ => vec![],
        }
    }

    /// How many guesses the host has counted.
    pub(crate) fn count(&self) -> u64 {
        self.counted
    }

    /// The text of the guess `from` numbered `n`, once it is put.
    pub(crate) fn asked(&self, from: &ActorId, n: u64) -> Option<&str> {
        self.asked.get(&(from.clone(), n)).map(String::as_str)
    }

    /// Whether the game is over.
    pub(crate) fn is_over(&self) -> bool {
        self.over
    }

    /// What sending `message` adds: the beginning sets the talk off, speech
    /// and answers are said, and a guess is kept to be told when the host
    /// counts it.
    fn acted(&mut self, message: &Message) -> Vec<Line> {
        match message {
            Message::Begin { .. } => vec![Line::Blank],
            Message::Say { from, text, .. } => vec![said(from, text)],
            Message::Guess { from, n, text } => {
                self.asked.insert((from.clone(), *n), text.clone());
                vec![]
            }
            Message::Answer { text, .. } => vec![said(&HOST.to_string(), text)],
            _ => vec![],
        }
    }

    /// The question of the guess `from` numbered `n`.
    fn question(&self, from: &ActorId, n: u64) -> String {
        self.asked
            .get(&(from.clone(), n))
            .cloned()
            .unwrap_or_default()
    }

    /// What the host observing the message `from` numbered `n` adds: the
    /// guess, numbered, if it is one and is counted, as the host counts:
    /// while guesses are left, and not repeating one counted already.
    fn counted(&mut self, from: &ActorId, n: u64) -> Vec<Line> {
        let Some(text) = self.asked.get(&(from.clone(), n)) else {
            return vec![];
        };
        if self.counted == self.guesses || !self.put.insert(folded(text)) {
            return vec![];
        }
        self.counted += 1;
        vec![Line::Told(format!(
            "{from} guesses ({} of {}): {text}",
            self.counted, self.guesses
        ))]
    }

    /// How the game ended, won `by` this with `used` guesses answered, the
    /// object `object`, after `took`.
    fn ended(&self, by: Victory, used: u64, object: &str, took: std::time::Duration) -> String {
        let (guesses, took) = (self.guesses, whole_seconds(took));
        match by {
            Victory::Named => {
                format!(
                    "The players named {object} with {used} of their {guesses} guesses, in {took}."
                )
            }
            Victory::OutOfGuesses => format!(
                "The host wins: the players used all {guesses} guesses without naming {object}, in {took}."
            ),
            Victory::OutOfTime => format!(
                "The host wins: time ran out after {took}, with {used} of {guesses} guesses \
                 used. It was {object}."
            ),
        }
    }
}

/// That the host is thinking of `object`.
fn thinking_of(object: &str) -> Line {
    Line::Told(format!("The host is thinking of {object}."))
}

/// `who` says `text`.
fn said(who: &ActorId, text: &str) -> Line {
    Line::Said {
        who: who.clone(),
        speaker: who.clone(),
        text: text.to_string(),
    }
}

/// Who plays which strategy: a line for each strategy, its players in
/// seat order.
fn playing(strategies: &BTreeMap<PlayerId, String>) -> Vec<Line> {
    let mut by_strategy: BTreeMap<&str, Vec<&PlayerId>> = BTreeMap::new();
    for (player, strategy) in strategies {
        by_strategy.entry(strategy).or_default().push(player);
    }
    by_strategy
        .into_iter()
        .map(|(strategy, players)| {
            let s = if players.len() == 1 { "s" } else { "" };
            Line::Told(format!("{} play{s} {strategy}.", in_prose(players)))
        })
        .collect()
}

/// How the console shows `line`: speech as `speaker: text`, and any other
/// line as it displays.
pub fn shown(line: &Line) -> String {
    match line {
        Line::Said { speaker, text, .. } => format!("{speaker}: {text}"),
        line => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::tests::configuration;
    use crate::log::Side;
    use crate::log::tests::{logged, logged_at};
    use free_agent_game::report::testing::in_play;
    use free_agent_model::ToolChoice;
    use std::collections::HashSet;
    use std::time::Duration;

    /// `from`'s action of `message`, numbered `n`.
    fn action(from: &str, n: u64, message: Message) -> Entry {
        Entry::Action {
            from: from.to_string(),
            n,
            to: HashSet::new(),
            message,
            seen: None,
            forgot: None,
        }
    }

    fn say(from: &str, n: u64, text: &str) -> Entry {
        let message = Message::Say {
            from: from.to_string(),
            n,
            text: text.to_string(),
        };
        action(from, n, message)
    }

    fn guess(from: &str, n: u64, text: &str) -> Entry {
        let message = Message::Guess {
            from: from.to_string(),
            n,
            text: text.to_string(),
        };
        action(from, n, message)
    }

    fn observed(by: &str, from: &str, n: u64) -> Entry {
        Entry::Observation {
            by: by.to_string(),
            from: from.to_string(),
            n,
        }
    }

    fn answer(n: u64, guess: u64, text: &str, named: bool) -> Entry {
        let message = Message::Answer {
            n,
            guess,
            asked_by: "Bob".to_string(),
            text: text.to_string(),
            named,
            left: 0,
        };
        action(HOST, n, message)
    }

    /// What the narrator tells of `entries`, each logged a second after the
    /// last, as the console shows it.
    fn told(entries: Vec<Entry>) -> Vec<String> {
        let mut narrator = Narrator::default();
        (0..)
            .zip(entries)
            .flat_map(|(seconds, entry)| narrator.narrate(&logged_at(seconds, entry)))
            .map(|line| shown(&line))
            .collect()
    }

    /// The start of a game of `guesses` played by Alice, Bob and Carol.
    fn start(guesses: u64) -> Entry {
        Entry::Start {
            strategies: [
                ("Alice", "chatty"),
                ("Bob", "everyone"),
                ("Carol", "everyone"),
            ]
            .into_iter()
            .map(|(player, strategy)| (player.to_string(), strategy.to_string()))
            .collect(),
            guesses,
            time_limit: Duration::from_secs(600),
        }
    }

    fn begin() -> Entry {
        action(
            HOST,
            1,
            Message::Begin {
                n: 1,
                view: crate::message::View {
                    players: vec![],
                    guesses: 2,
                    time_limit: Duration::from_secs(600),
                },
            },
        )
    }

    #[test]
    fn a_game_is_told_from_the_table_and_the_object_through_the_talk_to_how_it_ended() {
        let mut configured = configuration();
        configured.models = in_play(&[("qwen", ToolChoice::Auto)]);
        let lines = told(vec![
            start(2),
            Entry::Configuration(Box::new(configured)),
            begin(),
            say("Bob", 1, "Should we ask whether it's alive?"),
            guess("Bob", 2, "Is it alive?"),
            observed("Alice", "Bob", 2),
            observed(HOST, "Bob", 1),
            observed(HOST, "Bob", 2),
            say(HOST, 2, "Is that your guess, Bob?"),
            guess("Carol", 2, "IS IT ALIVE?"),
            observed(HOST, "Carol", 2),
            Entry::Repeated {
                from: "Carol".to_string(),
                n: 2,
            },
            answer(3, 1, "No, it isn't alive.", false),
            Entry::Failed {
                by: "Carol".to_string(),
                error: "no model".to_string(),
            },
            guess("Bob", 3, "Is it a teapot?"),
            observed(HOST, "Bob", 3),
            guess("Carol", 1, "Is it a kettle?"),
            observed(HOST, "Carol", 1),
            Entry::Uncounted {
                from: "Carol".to_string(),
                n: 1,
            },
            answer(4, 2, "Yes, it is!", true),
            Entry::End {
                winner: Side::Players,
                by: Victory::Named,
                used: 2,
                object: "a teapot".to_string(),
                took: Duration::from_secs(401),
            },
            say("Alice", 1, "Too late."),
        ]);
        assert_eq!(
            lines,
            [
                "Alice plays chatty.",
                "Bob and Carol play everyone.",
                "They have 2 guesses and 10m.",
                "qwen's context window is 4096 tokens, the default.",
                "qwen does not accept a required tool call, so its players are asked to call one.",
                "The host is thinking of a teapot.",
                "",
                "Bob: Should we ask whether it's alive?",
                "Bob guesses (1 of 2): Is it alive?",
                "Host: Is that your guess, Bob?",
                "Carol repeats a guess put already: IS IT ALIVE? It is not counted.",
                "Host: No, it isn't alive.",
                "Carol fails to play: no model",
                "Bob guesses (2 of 2): Is it a teapot?",
                "Carol guesses after the last guess: Is it a kettle? It is not counted.",
                "Host: Yes, it is!",
                "",
                "The players named a teapot with 2 of their 2 guesses, in 6m 41s.",
            ]
        );
    }

    #[test]
    fn a_picked_object_is_told_with_its_category_and_the_host_s_wins_are_told_with_the_object() {
        let chosen = |category: Option<&str>| Entry::Chosen {
            object: "a banjo".to_string(),
            category: category.map(String::from),
        };
        let mut configured = configuration();
        configured.host.object = None;
        assert_eq!(
            told(vec![
                Entry::Configuration(Box::new(configured)),
                chosen(Some("a musical instrument")),
                chosen(None),
            ]),
            [
                "Asked for a musical instrument, the host is thinking of a banjo.",
                "The host is thinking of a banjo.",
            ]
        );
        let ended = |by| {
            let mut narrator = Narrator::default();
            narrator.narrate(&logged(start(20)));
            let end = Entry::End {
                winner: Victory::winner(by),
                by,
                used: 7,
                object: "a banjo".to_string(),
                took: Duration::from_secs(600),
            };
            shown(narrator.narrate(&logged(end)).last().unwrap())
        };
        assert_eq!(
            ended(Victory::OutOfGuesses),
            "The host wins: the players used all 20 guesses without naming a banjo, in 10m."
        );
        assert_eq!(
            ended(Victory::OutOfTime),
            "The host wins: time ran out after 10m, with 7 of 20 guesses used. It was a banjo."
        );
    }

    #[test]
    fn what_the_narrator_does_not_tell_adds_nothing() {
        assert!(
            told(vec![
                Entry::Thinking {
                    by: "Bob".to_string()
                },
                observed(HOST, "Bob", 9),
                action(HOST, 1, Message::Wake),
            ])
            .is_empty()
        );
    }
}
