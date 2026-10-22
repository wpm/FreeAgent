//! What a player remembers, a [`Memory`] at a time, and the [`Story`] that
//! tells it to the player's model from its own point of view.
//!
//! A story is told in the second person, with no times. It opens with the
//! game's beginning, and then tells everything said and every guess as it
//! was said or heard, and every answer with the guess it answers and how
//! many guesses are left. Told within a budget it forgets talk, oldest
//! first, until it fits, but never a guess or an answer.

use crate::message::{HOST, Message, View};
use free_agent::ActorId;
use free_agent_game::report::{in_prose, told_within, whole_seconds};
use free_agent_game::seat::PlayerId;

/// What a player remembers: a message it received from another, or one it
/// sent. Reminders to itself are not remembered.
#[derive(Debug, Clone, PartialEq)]
pub enum Memory {
    /// The actor received `message`.
    Observed {
        /// What was sent.
        message: Message,
    },
    /// The actor sent `message`.
    Acted {
        /// What it sent.
        message: Message,
    },
}

/// What a line of a story is, which decides whether a story told within a
/// budget may forget it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    /// Something said, which a story may forget.
    Talk,
    /// Anything else, which stays.
    Kept,
}

/// A player's account of its own game, told a memory at a time.
pub struct Story {
    /// Whose story this is.
    me: ActorId,
    /// The guesses heard and not yet answered, oldest first: who asked
    /// each, the question, and where in the account it was told. The host
    /// answers each player's guesses in the order it received them.
    waiting: Vec<(PlayerId, String, usize)>,
    /// The lines told so far, in order, each with what it is.
    account: Vec<(String, Kind)>,
}

impl Story {
    /// The story the player `me` tells of its own game, with nothing in it
    /// yet.
    pub fn of(me: impl Into<ActorId>) -> Self {
        Self {
            me: me.into(),
            waiting: Vec::new(),
            account: Vec::new(),
        }
    }

    /// The lines told, in order.
    pub fn lines(&self) -> Vec<&str> {
        self.account.iter().map(|(text, _)| text.as_str()).collect()
    }

    /// Add what `memory` adds to the story: the beginning, a line of talk,
    /// a guess or an answer.
    pub fn recall(&mut self, memory: &Memory) {
        let (Memory::Observed { message } | Memory::Acted { message }) = memory;
        let (line, kind) = match message {
            Message::Begin { view, .. } => (self.begun(view), Kind::Kept),
            Message::Say { from, text, .. } => {
                let (subject, s) = self.subject(from);
                (format!("{subject} say{s}, \"{text}\""), Kind::Talk)
            }
            Message::Guess { from, text, .. } => {
                self.waiting
                    .push((from.clone(), text.clone(), self.account.len()));
                let (subject, es) = match self.subject(from) {
                    (subject, "") => (subject, ""),
                    (subject, _) => (subject, "es"),
                };
                (format!("{subject} guess{es}: \"{text}\""), Kind::Kept)
            }
            Message::Answer {
                guess,
                asked_by,
                text,
                left,
                ..
            } => (self.answered(*guess, asked_by, text, *left), Kind::Kept),
            Message::Over | Message::Wake | Message::Remind { .. } => return,
        };
        self.account.push((line, kind));
    }

    /// The guesses heard and not yet answered, oldest first, each as who
    /// asked it and the question.
    pub fn waiting(&self) -> impl Iterator<Item = (&PlayerId, &str)> {
        self.waiting
            .iter()
            .map(|(asker, question, _)| (asker, question.as_str()))
    }

    /// The lines told within `budget` tokens by estimate, the oldest talk
    /// forgotten first to fit, and how many lines of talk were left out.
    pub fn told_within(&self, budget: u64) -> (Vec<&str>, usize) {
        told_within(&self.account, |kind| *kind == Kind::Talk, budget)
    }

    /// How the game began, as `view` shows it: who the player is and whom
    /// it plays with.
    fn begun(&self, view: &View) -> String {
        let others: Vec<&PlayerId> = view
            .players
            .iter()
            .filter(|player| **player != self.me)
            .collect();
        let with = match others.is_empty() {
            true => "alone".to_string(),
            false => format!("with {}", in_prose(others)),
        };
        format!(
            "You are {}, playing Twenty Questions {with}. The host holds a secret object, and \
             you have {} guesses among you and {} to name it.",
            self.me,
            view.guesses,
            whole_seconds(view.time_limit)
        )
    }

    /// The host's answer `text` to the `guess`th guess, `asked_by`'s, with
    /// `left` guesses left, told with the question it answers when it was
    /// heard. The answer holds the question from then on, so the line that
    /// told the guess may be forgotten like talk.
    fn answered(&mut self, guess: u64, asked_by: &PlayerId, text: &str, left: u64) -> String {
        let whose = match *asked_by == self.me {
            true => "your".to_string(),
            false => format!("{asked_by}'s"),
        };
        let question = match self
            .waiting
            .iter()
            .position(|(asker, _, _)| asker == asked_by)
        {
            Some(at) => {
                let (_, question, line) = self.waiting.remove(at);
                self.account[line].1 = Kind::Talk;
                format!(" \"{question}\"")
            }
            None => String::new(),
        };
        format!(
            "The host answers {whose} guess {guess}{question}: \"{text}\" {}",
            guesses_left(left)
        )
    }

    /// `actor` as the subject of a sentence, with the ending its verb takes:
    /// "You" and none for the actor itself, "The host" and an s for the
    /// host, and a player's name and an s for anyone else.
    fn subject(&self, actor: &ActorId) -> (String, &'static str) {
        if *actor == self.me {
            ("You".to_string(), "")
        } else if actor == HOST {
            ("The host".to_string(), "s")
        } else {
            (actor.clone(), "s")
        }
    }
}

/// How many guesses are left, in a sentence: "No guesses are left.", "1
/// guess is left.", "12 guesses are left."
pub(crate) fn guesses_left(left: u64) -> String {
    match left {
        0 => "No guesses are left.".to_string(),
        1 => "1 guess is left.".to_string(),
        left => format!("{left} guesses are left."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn view(players: &[&str]) -> View {
        View {
            players: players.iter().map(|player| player.to_string()).collect(),
            guesses: 20,
            time_limit: Duration::from_secs(600),
        }
    }

    fn begin(players: &[&str]) -> Message {
        Message::Begin {
            n: 1,
            view: view(players),
        }
    }

    fn say(from: &str, text: &str) -> Message {
        Message::Say {
            from: from.to_string(),
            n: 1,
            text: text.to_string(),
        }
    }

    fn guess(from: &str, text: &str) -> Message {
        Message::Guess {
            from: from.to_string(),
            n: 2,
            text: text.to_string(),
        }
    }

    fn answer(guess: u64, asked_by: &str, text: &str, left: u64) -> Message {
        Message::Answer {
            n: guess + 1,
            guess,
            asked_by: asked_by.to_string(),
            text: text.to_string(),
            named: false,
            left,
        }
    }

    /// The story of `me`, told from `memories`.
    fn told(me: &str, memories: Vec<Memory>) -> Vec<String> {
        let mut story = Story::of(me);
        for memory in &memories {
            story.recall(memory);
        }
        story.lines().into_iter().map(String::from).collect()
    }

    fn heard(message: Message) -> Memory {
        Memory::Observed { message }
    }

    fn did(message: Message) -> Memory {
        Memory::Acted { message }
    }

    #[test]
    fn a_player_s_story_tells_who_it_is_what_was_said_every_guess_and_every_answer() {
        let lines = told(
            "Alice",
            vec![
                heard(begin(&["Alice", "Bob", "Carol"])),
                heard(say("Bob", "Shall we ask if it's alive?")),
                did(say("Alice", "Yes.")),
                heard(say(HOST, "Is that your guess, Bob?")),
                heard(guess("Bob", "Is it alive?")),
                did(guess("Alice", "Is it big?")),
                heard(answer(1, "Bob", "No, it isn't alive.", 18)),
                heard(answer(2, "Alice", "No.", 18)),
                heard(answer(3, "Carol", "Yes.", 1)),
                heard(Message::Answer {
                    n: 5,
                    guess: 4,
                    asked_by: "Carol".to_string(),
                    text: "It is!".to_string(),
                    named: true,
                    left: 0,
                }),
            ],
        );
        assert_eq!(
            lines,
            [
                "You are Alice, playing Twenty Questions with Bob and Carol. The host holds a \
                 secret object, and you have 20 guesses among you and 10m to name it.",
                "Bob says, \"Shall we ask if it's alive?\"",
                "You say, \"Yes.\"",
                "The host says, \"Is that your guess, Bob?\"",
                "Bob guesses: \"Is it alive?\"",
                "You guess: \"Is it big?\"",
                "The host answers Bob's guess 1 \"Is it alive?\": \"No, it isn't alive.\" 18 guesses are left.",
                "The host answers your guess 2 \"Is it big?\": \"No.\" 18 guesses are left.",
                "The host answers Carol's guess 3: \"Yes.\" 1 guess is left.",
                "The host answers Carol's guess 4: \"It is!\" No guesses are left.",
            ]
        );
    }

    #[test]
    fn a_lone_player_plays_alone_and_the_waiting_guesses_are_those_unanswered() {
        assert_eq!(
            told("Alice", vec![heard(begin(&["Alice"]))]),
            [
                "You are Alice, playing Twenty Questions alone. The host holds a secret object, \
              and you have 20 guesses among you and 10m to name it."
            ]
        );
        let mut story = Story::of("Alice");
        story.recall(&did(guess("Alice", "Is it alive?")));
        story.recall(&heard(guess("Bob", "Is it big?")));
        story.recall(&heard(answer(1, "Alice", "No.", 18)));
        let waiting: Vec<(&PlayerId, &str)> = story.waiting().collect();
        assert_eq!(waiting, [(&"Bob".to_string(), "Is it big?")]);
        story.recall(&heard(Message::Wake));
        assert_eq!(story.lines().len(), 3);
    }

    #[test]
    fn told_within_a_budget_the_story_forgets_talk_but_never_a_guess_or_an_answer() {
        let mut story = Story::of("Alice");
        story.recall(&heard(begin(&["Alice", "Bob"])));
        story.recall(&heard(say("Bob", &"chatter ".repeat(50))));
        story.recall(&heard(guess("Bob", "Is it alive?")));
        story.recall(&heard(answer(1, "Bob", "No.", 19)));
        let (all, forgot) = story.told_within(10_000);
        assert_eq!((all.len(), forgot), (4, 0));
        let (lines, forgot) = story.told_within(100);
        assert_eq!(forgot, 1);
        assert_eq!(lines[1], free_agent_game::report::FORGOTTEN);
        assert_eq!(lines[2], "Bob guesses: \"Is it alive?\"");
    }

    #[test]
    fn guesses_left_are_told_in_a_sentence() {
        assert_eq!(guesses_left(0), "No guesses are left.");
        assert_eq!(guesses_left(1), "1 guess is left.");
        assert_eq!(guesses_left(7), "7 guesses are left.");
    }
}
