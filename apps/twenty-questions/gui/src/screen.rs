//! What the screen shows, as a pure function of the cues so far.
//!
//! [`Screen::apply`] folds one cue at a time into the chat, the names
//! table, the secret, the guesses used and the clock. The view draws a
//! `Screen` and holds no state of its own, so the whole page is tested
//! here, natively. A [`Timeline`] holds the screen at each bubble of a game
//! already folded, for review.

use std::collections::BTreeSet;

use free_agent_gui::clock::{Clock, Timer};
use leptos_rich_chat::{Look, Names, Position};
use twenty_questions_gui_types::{Beat, Cue, Millis, Outcome, Part, PlayerId, Seat, Victory};

/// The name narration is posted under, centered between the sides.
pub const NARRATOR: &str = "Narrator";

/// How long, in log time, a model call is out before its dots show, so
/// that a call that ends at once does not flicker.
pub const THINKING_DELAY: Millis = 300;

/// How many player colors there are, one per seat.
pub const COLORS: u8 = 12;

/// One message in the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Post {
    /// `m1`, `m2`, … in arrival order.
    pub id: String,
    /// Who it is from: a seat's name, or [`NARRATOR`].
    pub name: String,
    /// What it shows.
    pub body: Body,
}

/// What a message shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Narration: Markdown, centered.
    Told(String),
    /// Speech: Markdown, in its speaker's bubble.
    Said(String),
    /// A counted guess, tagged with its number.
    Guess {
        /// Its number, from 1.
        guess: u64,
        /// The question.
        text: String,
    },
    /// The host's answer to a guess, tagged with its number, under the
    /// guess it answers.
    Answer {
        /// The number of the guess it answers.
        guess: u64,
        /// Who put the guess it answers, if anyone did.
        asker: Option<PlayerId>,
        /// The guess it answers, as asked.
        asked: String,
        /// The answer.
        text: String,
    },
    /// A guess after the last, which nobody answers.
    Uncounted(String),
}

/// A speaker's thinking dots, pinned below the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thinker {
    /// Who is thinking: a seat's name.
    pub who: PlayerId,
}

/// One of the guesses the players have, as the header's pips show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pip {
    /// Not used yet.
    Open,
    /// Put, and waiting for its answer.
    Waiting,
    /// Put and answered, or put once the game was over.
    Used,
    /// The guess that named the object, in its asker's color.
    Won(u8),
}

/// Everything the screen shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    /// Every seat, the host first, in seat order.
    pub seats: Vec<Seat>,
    /// Where each name's bubbles sit and their colors.
    pub names: Names,
    /// The secret object, once it is known.
    pub secret: Option<String>,
    /// The chat, oldest first.
    pub posts: Vec<Post>,
    /// How many guesses the players have among them.
    pub guesses: u64,
    /// Each guess put, in order: who put it and what it asked.
    asked: Vec<(PlayerId, String)>,
    /// The numbers of the guesses answered.
    answered: BTreeSet<u64>,
    /// The number of the guess that named the object, once one has.
    pub won: Option<u64>,
    /// How the game ended, once it has.
    pub outcome: Option<Outcome>,
    /// How long the game lasts, from [`Beat::Began`].
    time_limit_ms: u64,
    /// When the game began, in log time, once it has.
    began: Option<Millis>,
    /// When the game ended, in log time, once it has.
    ended: Option<Millis>,
    /// The dots of every call out longer than [`THINKING_DELAY`], in the
    /// order they showed.
    pub thinkers: Vec<Thinker>,
    /// Every call out whose dots have not shown, with when it went out.
    thinking: Vec<(PlayerId, Millis)>,
    /// The log-time clock.
    pub clock: Clock,
    /// The bubble under review, selected, with every later one dimmed.
    pub cursor: Option<usize>,
    /// How many times the screen has been reset.
    era: u64,
}

impl Default for Screen {
    fn default() -> Self {
        Self {
            seats: Vec::new(),
            names: Names::none(),
            secret: None,
            posts: Vec::new(),
            guesses: 0,
            asked: Vec::new(),
            answered: BTreeSet::new(),
            won: None,
            outcome: None,
            time_limit_ms: 0,
            began: None,
            ended: None,
            thinkers: Vec::new(),
            thinking: Vec::new(),
            clock: Clock::default(),
            cursor: None,
            era: 0,
        }
    }
}

impl Screen {
    /// Show `cue`, the clock running on from it while the game is live.
    /// A cue earlier than the clock leaves it where it is. Every call out
    /// longer than [`THINKING_DELAY`] by the cue's time shows its dots.
    pub fn apply(&mut self, cue: &Cue) {
        self.reveal(cue.at);
        self.clock.at = self.clock.at.max(cue.at as f64);
        self.clock.running = true;
        match &cue.beat {
            Beat::Cast {
                seats,
                guesses,
                time_limit_ms,
            } => {
                self.seats = seats.clone();
                self.names = names(seats);
                self.guesses = *guesses;
                self.time_limit_ms = *time_limit_ms;
            }
            Beat::Secret { object } => self.secret = Some(object.clone()),
            Beat::Clock { rate } => self.clock.rate = *rate,
            Beat::Began => self.began = Some(cue.at),
            Beat::Told(lines) => self.post(NARRATOR, Body::Told(lines.join("  \n"))),
            Beat::Said { who, text } => self.post(who, Body::Said(text.clone())),
            Beat::Guessed { who, text, guess } => {
                self.asked.push((who.clone(), text.clone()));
                let body = Body::Guess {
                    guess: *guess,
                    text: text.clone(),
                };
                self.post(who, body);
            }
            Beat::Answered { guess, text, named } => {
                self.answered.insert(*guess);
                if *named {
                    self.won = Some(*guess);
                }
                let (asker, asked) = self.asker(*guess).cloned().unzip();
                let body = Body::Answer {
                    guess: *guess,
                    asker,
                    asked: asked.unwrap_or_default(),
                    text: text.clone(),
                };
                let host = self.host().unwrap_or("Host").to_string();
                self.post(&host, body);
            }
            Beat::Uncounted { who, text } => self.post(who, Body::Uncounted(text.clone())),
            Beat::Thinking { who } => self.thinking.push((who.clone(), cue.at)),
            Beat::Thought { who } => {
                self.thinking.retain(|(thinker, _)| thinker != who);
                self.thinkers.retain(|thinker| thinker.who != *who);
            }
            Beat::Over { outcome } => {
                self.outcome = Some(outcome.clone());
                self.ended = Some(cue.at);
                self.clock.running = false;
                self.thinking.clear();
                self.thinkers.clear();
            }
        }
    }

    /// Show the dots of every call out longer than [`THINKING_DELAY`] at
    /// log time `now`.
    pub fn reveal(&mut self, now: Millis) {
        if self.thinking.is_empty() {
            return;
        }
        let (due, waiting) = std::mem::take(&mut self.thinking)
            .into_iter()
            .partition(|(_, at)| at + THINKING_DELAY <= now);
        self.thinking = waiting;
        self.thinkers
            .extend(due.into_iter().map(|(who, _)| Thinker { who }));
    }

    /// Clear the screen for a new game, in a new era.
    pub fn reset(&mut self) {
        *self = Screen {
            era: self.era + 1,
            ..Screen::default()
        };
    }

    /// Which era the screen is in. Each [`Screen::reset`] starts another.
    pub fn era(&self) -> u64 {
        self.era
    }

    /// Do `act` to the screen if it is still in `era`, and nothing once it
    /// has been reset: what a timer set for one game does, kept from the
    /// next.
    pub fn within(&mut self, era: u64, act: impl FnOnce(&mut Screen)) {
        if self.era == era {
            act(self);
        }
    }

    /// Show `shown`, a screen under review, in place of this one and in its
    /// era.
    pub fn review(&mut self, shown: Screen) {
        *self = Screen {
            era: self.era,
            ..shown
        };
    }

    /// Run the clock on by `elapsed` viewer milliseconds, as the page does
    /// just before it applies the next cue.
    pub fn advance(&mut self, elapsed: f64) {
        self.clock.at = self.clock.now(elapsed);
    }

    /// How many guesses have been put.
    pub fn used(&self) -> u64 {
        self.asked.len() as u64
    }

    /// Each of the guesses the players have, in order, as its pip shows
    /// it.
    pub fn pips(&self) -> Vec<Pip> {
        (1..=self.guesses)
            .map(|guess| {
                if self.won == Some(guess) {
                    Pip::Won(self.asker_color(guess).unwrap_or_default())
                } else if guess > self.used() {
                    Pip::Open
                } else if self.answered.contains(&guess) || self.outcome.is_some() {
                    Pip::Used
                } else {
                    Pip::Waiting
                }
            })
            .collect()
    }

    /// The game's countdown, once it has begun, stopped when it ends.
    pub fn timer(&self) -> Option<Timer> {
        Some(Timer {
            began: self.began?,
            limit_ms: self.time_limit_ms,
            stopped: self.ended,
        })
    }

    /// The color of the player `who`, an index into the palette, or
    /// `None` for the host or a name that is not a player's.
    pub fn color(&self, who: &str) -> Option<u8> {
        self.seats
            .iter()
            .find(|seat| seat.name == who)
            .and_then(|seat| match seat.part {
                Part::Player { color, .. } => Some(color % COLORS),
                Part::Host => None,
            })
    }

    /// The color of the player who put the guess numbered `guess`, if one
    /// did.
    pub fn asker_color(&self, guess: u64) -> Option<u8> {
        self.asker(guess).and_then(|(who, _)| self.color(who))
    }

    /// Who put the guess numbered `guess`, and what it asked.
    fn asker(&self, guess: u64) -> Option<&(PlayerId, String)> {
        let index = usize::try_from(guess.checked_sub(1)?).ok()?;
        self.asked.get(index)
    }

    /// The host's name, once cast.
    fn host(&self) -> Option<&str> {
        self.seats
            .iter()
            .find(|seat| seat.part == Part::Host)
            .map(|seat| seat.name.as_str())
    }

    /// Post `body` from `name`.
    fn post(&mut self, name: &str, body: Body) {
        let id = format!("m{}", self.posts.len() + 1);
        let name = name.to_string();
        self.posts.push(Post { id, name, body });
    }
}

/// The key the chat knows the post `id` by in `era`, `3-m12`, so that a
/// post of one game is never taken for the post of another with its ID.
pub fn key(era: u64, id: &str) -> String {
    format!("{era}-{id}")
}

/// The bubble whose chat key is `key`: bubble `k` is posted as `m{k+1}`.
pub fn bubble(key: &str) -> Option<usize> {
    let (_, id) = key.split_once('-')?;
    id.strip_prefix('m')?.parse::<usize>().ok()?.checked_sub(1)
}

/// The screen at every bubble of a game: the game's last screen, and the
/// screen while each bubble is the latest, its posts left out.
#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    /// The screen after every cue.
    last: Screen,
    /// For each bubble, the screen after every cue from its own up to the
    /// one that posts the next bubble, or to the end for the last one,
    /// with no posts.
    moments: Vec<Screen>,
}

impl Timeline {
    /// The timeline of the game `cues` tell, folded at once.
    pub fn new(cues: &[Cue]) -> Self {
        let mut screen = Screen::default();
        let mut moments = Vec::new();
        for cue in cues {
            let before = screen.posts.len();
            let now = (moments.len() < before).then(|| moment(&mut screen));
            screen.apply(cue);
            if let Some(now) = now.filter(|_| screen.posts.len() > before) {
                moments.resize(before, now);
            }
        }
        let now = moment(&mut screen);
        moments.resize(screen.posts.len(), now);
        Self {
            last: screen,
            moments,
        }
    }

    /// How many bubbles there are.
    pub fn len(&self) -> usize {
        self.moments.len()
    }

    /// Whether there are no bubbles.
    pub fn is_empty(&self) -> bool {
        self.moments.is_empty()
    }

    /// The bubbles that begin a chapter, each a guess, in order.
    pub fn chapters(&self) -> Vec<usize> {
        let posts = self.last.posts.iter().enumerate();
        posts
            .filter(|(_, post)| matches!(post.body, Body::Guess { .. }))
            .map(|(k, _)| k)
            .collect()
    }

    /// How the game ended, once it has.
    pub fn outcome(&self) -> Option<&Outcome> {
        self.last.outcome.as_ref()
    }

    /// The number of the guess that named the object, once one has.
    pub fn won(&self) -> Option<u64> {
        self.last.won
    }

    /// The secret object, once it is known.
    pub fn secret(&self) -> Option<&str> {
        self.last.secret.as_deref()
    }

    /// The screen with the cursor on bubble `k`: every post, and the rest
    /// as it stood while bubble `k` was the latest, the clock stopped at
    /// its time and nobody thinking.
    ///
    /// # Panics
    ///
    /// When there is no bubble `k`.
    pub fn screen(&self, k: usize) -> Screen {
        let moment = &self.moments[k];
        Screen {
            posts: self.last.posts.clone(),
            cursor: Some(k),
            thinkers: Vec::new(),
            thinking: Vec::new(),
            clock: Clock {
                running: false,
                ..moment.clock
            },
            ..moment.clone()
        }
    }
}

/// `screen` as it stands, without its posts.
fn moment(screen: &mut Screen) -> Screen {
    let posts = std::mem::take(&mut screen.posts);
    let moment = screen.clone();
    screen.posts = posts;
    moment
}

/// When the dots of the call `cue` sends out are due, in log time, if it
/// sends one out.
pub fn dots_due(cue: &Cue) -> Option<Millis> {
    matches!(cue.beat, Beat::Thinking { .. }).then_some(cue.at + THINKING_DELAY)
}

/// How a game ended, as the header says it: `The players won in 9`, `The
/// host won: time ran out`.
pub fn ending(outcome: &Outcome, won: Option<u64>) -> String {
    match outcome {
        Outcome::Won {
            by: Victory::Named, ..
        } => match won {
            Some(guess) => format!("The players won in {guess}"),
            None => "The players won".to_string(),
        },
        Outcome::Won {
            by: Victory::OutOfGuesses,
            ..
        } => "The host won: the guesses ran out".to_string(),
        Outcome::Won {
            by: Victory::OutOfTime,
            ..
        } => "The host won: time ran out".to_string(),
        Outcome::Stopped => "Stopped".to_string(),
        Outcome::Failed(error) => format!("Failed: {error}"),
        Outcome::Unfinished => "The log ends before the game does".to_string(),
    }
}

/// The names table for `seats`: each player on the left in a bubble
/// filled with its color, with the color's mark as `--tq-mark` for its
/// square and borders, the host on the right in ink, and the narrator
/// centered and muted.
fn names(seats: &[Seat]) -> Names {
    let narrator = Look::at(Position::Center)
        .background("transparent")
        .foreground("var(--rc-muted)");
    seats
        .iter()
        .fold(Names::none().name(NARRATOR, narrator), |names, seat| {
            let look = match &seat.part {
                Part::Host => Look::at(Position::Right)
                    .background("var(--fa-ink)")
                    .foreground("var(--fa-paper)")
                    .property("--tq-mark", "var(--fa-ink)"),
                Part::Player { color, .. } => {
                    let n = color % COLORS + 1;
                    Look::at(Position::Left)
                        .background(format!("var(--tq-player-{n})"))
                        .foreground("var(--fa-ink)")
                        .property("--tq-mark", format!("var(--tq-player-{n}-mark)"))
                }
            };
            names.name(seat.name.clone(), look)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `beat` at log time `at`.
    fn at(at: Millis, beat: Beat) -> Cue {
        Cue { at, beat }
    }

    /// The host and the players `players`, in seat order.
    fn cast(players: &[&str]) -> Cue {
        let host = Seat {
            name: "Host".to_string(),
            part: Part::Host,
        };
        let players = players.iter().enumerate().map(|(color, name)| Seat {
            name: name.to_string(),
            part: Part::Player {
                color: color as u8,
                strategy: "everyone".to_string(),
            },
        });
        let seats = std::iter::once(host).chain(players).collect();
        at(
            0,
            Beat::Cast {
                seats,
                guesses: 3,
                time_limit_ms: 60_000,
            },
        )
    }

    /// A guess by `who`, numbered `guess`.
    fn guessed(time: Millis, who: &str, guess: u64, text: &str) -> Cue {
        let beat = Beat::Guessed {
            who: who.to_string(),
            text: text.to_string(),
            guess,
        };
        at(time, beat)
    }

    /// The host's answer to guess `guess`.
    fn answered(time: Millis, guess: u64, text: &str, named: bool) -> Cue {
        let beat = Beat::Answered {
            guess,
            text: text.to_string(),
            named,
        };
        at(time, beat)
    }

    /// The screen after `cues`.
    fn shown(cues: &[Cue]) -> Screen {
        let mut screen = Screen::default();
        for cue in cues {
            screen.apply(cue);
        }
        screen
    }

    /// Each post's name and body.
    fn posted(screen: &Screen) -> Vec<(&str, &Body)> {
        let posts = screen.posts.iter();
        posts.map(|post| (post.name.as_str(), &post.body)).collect()
    }

    #[test]
    fn the_cast_fills_each_player_s_bubble_with_its_color_and_the_host_s_with_ink() {
        let screen = shown(&[cast(&["Alice", "Bob"])]);
        let look = |name: &str| screen.names.get(name).cloned().unwrap();
        assert_eq!(
            look("Alice"),
            Look::at(Position::Left)
                .background("var(--tq-player-1)")
                .foreground("var(--fa-ink)")
                .property("--tq-mark", "var(--tq-player-1-mark)")
        );
        assert_eq!(
            look("Bob").background.as_deref(),
            Some("var(--tq-player-2)")
        );
        assert_eq!(
            look("Host"),
            Look::at(Position::Right)
                .background("var(--fa-ink)")
                .foreground("var(--fa-paper)")
                .property("--tq-mark", "var(--fa-ink)")
        );
        assert_eq!(look(NARRATOR).position, Position::Center);
        assert_eq!(screen.guesses, 3);
        assert_eq!(screen.pips(), [Pip::Open; 3]);
        assert_eq!(screen.color("Bob"), Some(1));
        assert_eq!(screen.color("Host"), None);
        assert_eq!(screen.color("Zed"), None);
    }

    #[test]
    fn the_secret_and_the_countdown_come_as_the_game_starts() {
        let mut screen = shown(&[cast(&["Alice"])]);
        assert_eq!(screen.secret, None);
        assert_eq!(screen.timer(), None);
        let secret = Beat::Secret {
            object: "a teapot".to_string(),
        };
        screen.apply(&at(1_000, secret));
        screen.apply(&at(2_000, Beat::Began));
        assert_eq!(screen.secret.as_deref(), Some("a teapot"));
        assert_eq!(
            screen.timer(),
            Some(Timer {
                began: 2_000,
                limit_ms: 60_000,
                stopped: None
            })
        );
        let over = Beat::Over {
            outcome: Outcome::Stopped,
        };
        screen.apply(&at(5_000, over));
        assert_eq!(screen.timer().unwrap().stopped, Some(5_000));
        assert!(!screen.clock.running);
    }

    #[test]
    fn speech_guesses_and_answers_are_posted_from_their_speakers_in_order() {
        let said = Beat::Said {
            who: "Alice".to_string(),
            text: "Is it alive?".to_string(),
        };
        let told = Beat::Told(vec!["One.".to_string(), "Two.".to_string()]);
        let late = Beat::Uncounted {
            who: "Bob".to_string(),
            text: "Is it red?".to_string(),
        };
        let screen = shown(&[
            cast(&["Alice", "Bob"]),
            at(1, told),
            at(2, said),
            guessed(3, "Bob", 1, "Is it alive?"),
            answered(4, 1, "No.", false),
            at(5, late),
        ]);
        assert_eq!(
            posted(&screen),
            [
                (NARRATOR, &Body::Told("One.  \nTwo.".to_string())),
                ("Alice", &Body::Said("Is it alive?".to_string())),
                (
                    "Bob",
                    &Body::Guess {
                        guess: 1,
                        text: "Is it alive?".to_string()
                    }
                ),
                (
                    "Host",
                    &Body::Answer {
                        guess: 1,
                        asker: Some("Bob".to_string()),
                        asked: "Is it alive?".to_string(),
                        text: "No.".to_string()
                    }
                ),
                ("Bob", &Body::Uncounted("Is it red?".to_string())),
            ]
        );
        let ids: Vec<&str> = screen.posts.iter().map(|post| post.id.as_str()).collect();
        assert_eq!(ids, ["m1", "m2", "m3", "m4", "m5"]);
    }

    #[test]
    fn a_pip_waits_for_its_answer_and_the_one_that_won_takes_its_guesser_s_color() {
        let mut screen = shown(&[
            cast(&["Alice", "Bob"]),
            guessed(1, "Alice", 1, "Is it alive?"),
            guessed(2, "Bob", 2, "Is it a teapot?"),
        ]);
        assert_eq!(screen.used(), 2);
        assert_eq!(screen.pips(), [Pip::Waiting, Pip::Waiting, Pip::Open]);
        screen.apply(&answered(3, 2, "Yes!", true));
        assert_eq!(screen.won, Some(2));
        assert_eq!(screen.pips(), [Pip::Waiting, Pip::Won(1), Pip::Open]);
        screen.apply(&answered(4, 1, "No.", false));
        assert_eq!(screen.pips(), [Pip::Used, Pip::Won(1), Pip::Open]);
    }

    #[test]
    fn a_guess_left_unanswered_at_the_end_is_used() {
        let over = Beat::Over {
            outcome: Outcome::Stopped,
        };
        let screen = shown(&[
            cast(&["Alice"]),
            guessed(1, "Alice", 1, "Is it alive?"),
            at(2, over),
        ]);
        assert_eq!(screen.pips(), [Pip::Used, Pip::Open, Pip::Open]);
    }

    #[test]
    fn an_answer_to_a_guess_never_put_quotes_nothing_and_comes_from_the_host() {
        let screen = shown(&[answered(1, 4, "No.", false)]);
        assert_eq!(
            posted(&screen),
            [(
                "Host",
                &Body::Answer {
                    guess: 4,
                    asker: None,
                    asked: String::new(),
                    text: "No.".to_string()
                }
            )]
        );
        assert_eq!(screen.pips(), []);
        let mut screen = shown(&[cast(&["Alice"])]);
        screen.won = Some(2);
        screen.guesses = 2;
        assert_eq!(screen.pips()[1], Pip::Won(0));
    }

    #[test]
    fn dots_show_once_a_call_has_been_out_a_while_and_go_when_it_is_over() {
        let thinking = |who: &str| Beat::Thinking {
            who: who.to_string(),
        };
        let thought = |who: &str| Beat::Thought {
            who: who.to_string(),
        };
        let mut screen = shown(&[cast(&["Alice"]), at(1_000, thinking("Host"))]);
        assert_eq!(dots_due(&at(1_000, thinking("Host"))), Some(1_300));
        assert_eq!(dots_due(&at(1_000, Beat::Began)), None);
        assert!(screen.thinkers.is_empty());
        screen.apply(&at(1_100, thinking("Alice")));
        screen.reveal(1_300);
        let host = Thinker {
            who: "Host".to_string(),
        };
        assert_eq!(screen.thinkers, [host]);
        screen.apply(&at(1_500, thought("Host")));
        assert_eq!(
            screen.thinkers,
            [Thinker {
                who: "Alice".to_string()
            }]
        );
        screen.apply(&at(1_600, thinking("Host")));
        let over = Beat::Over {
            outcome: Outcome::Unfinished,
        };
        screen.apply(&at(1_700, over));
        assert!(screen.thinkers.is_empty());
        screen.reveal(9_000);
        assert!(screen.thinkers.is_empty());
    }

    #[test]
    fn a_reset_clears_the_screen_and_leaves_the_last_game_s_timers_undone() {
        let mut screen = shown(&[cast(&["Alice"]), at(5_000, Beat::Clock { rate: 2.0 })]);
        assert_eq!(screen.clock.rate, 2.0);
        assert_eq!(screen.clock.at, 5_000.0);
        screen.advance(100.0);
        assert_eq!(screen.clock.at, 5_200.0);
        let era = screen.era();
        screen.reset();
        assert_eq!(screen.era(), era + 1);
        assert!(screen.seats.is_empty());
        screen.within(era, |screen| screen.guesses = 9);
        assert_eq!(screen.guesses, 0);
        screen.within(era + 1, |screen| screen.guesses = 9);
        assert_eq!(screen.guesses, 9);
    }

    #[test]
    fn the_header_says_who_won_and_how() {
        let won = |by| Outcome::Won {
            by,
            how: String::new(),
        };
        assert_eq!(
            ending(&won(Victory::Named), Some(9)),
            "The players won in 9"
        );
        assert_eq!(ending(&won(Victory::Named), None), "The players won");
        assert_eq!(
            ending(&won(Victory::OutOfGuesses), None),
            "The host won: the guesses ran out"
        );
        assert_eq!(
            ending(&won(Victory::OutOfTime), None),
            "The host won: time ran out"
        );
        assert_eq!(ending(&Outcome::Stopped, None), "Stopped");
        assert_eq!(
            ending(&Outcome::Failed("no model".to_string()), None),
            "Failed: no model"
        );
        assert_eq!(
            ending(&Outcome::Unfinished, None),
            "The log ends before the game does"
        );
    }

    /// The recorded game in `fixtures/`, as cues.
    fn recorded() -> Vec<Cue> {
        let log = include_str!("../fixtures/prompted.jsonl");
        let mut recordings = twenty_questions::stage::recordings(log).unwrap();
        recordings.games.remove(0).cues
    }

    /// The screen a fresh `Screen` shows after every cue up to the first
    /// to post once bubble `k` is posted: the screen while bubble `k` is
    /// the latest.
    fn latest_at(cues: &[Cue], k: usize) -> Screen {
        let mut screen = Screen::default();
        for cue in cues {
            let mut next = screen.clone();
            next.apply(cue);
            if screen.posts.len() > k && next.posts.len() > screen.posts.len() {
                break;
            }
            screen = next;
        }
        screen
    }

    #[test]
    fn the_screen_at_bubble_k_is_the_screen_folded_until_the_next_bubble() {
        let cues = recorded();
        let timeline = Timeline::new(&cues);
        let last = shown(&cues);
        assert!(!timeline.is_empty());
        assert_eq!(timeline.len(), last.posts.len());
        for k in 0..timeline.len() {
            let folded = latest_at(&cues, k);
            let expected = Screen {
                posts: last.posts.clone(),
                cursor: Some(k),
                thinkers: Vec::new(),
                thinking: Vec::new(),
                clock: Clock {
                    running: false,
                    ..folded.clock
                },
                ..folded
            };
            assert_eq!(timeline.screen(k), expected, "bubble {k}");
        }
    }

    #[test]
    fn the_count_the_secret_and_the_outcome_follow_the_cursor() {
        let cues = recorded();
        let timeline = Timeline::new(&cues);
        let first = timeline.screen(0);
        assert_eq!(first.used(), 0);
        assert_eq!(first.outcome, None);
        assert_eq!(first.won, None);
        let last = timeline.screen(timeline.len() - 1);
        assert!(last.outcome.is_some());
        assert_eq!(last.timer().unwrap().stopped, shown(&cues).ended);
        assert_eq!(timeline.outcome(), last.outcome.as_ref());
        assert_eq!(timeline.won(), last.won);
        assert_eq!(timeline.secret(), last.secret.as_deref());
        let guess = timeline.chapters()[0];
        let asked = timeline.screen(guess);
        assert_eq!(asked.used(), 1);
        assert_eq!(asked.pips()[0], Pip::Waiting);
    }

    #[test]
    fn each_guess_begins_a_chapter() {
        let cues = recorded();
        let timeline = Timeline::new(&cues);
        let last = shown(&cues);
        let chapters = timeline.chapters();
        assert_eq!(chapters.len() as u64, last.used());
        for (number, &k) in chapters.iter().enumerate() {
            let Body::Guess { guess, .. } = last.posts[k].body else {
                panic!("bubble {k} is no guess");
            };
            assert_eq!(guess, number as u64 + 1);
        }
    }

    #[test]
    fn a_game_with_no_bubble_has_an_empty_timeline() {
        let timeline = Timeline::new(&[cast(&["Alice"])]);
        assert!(timeline.is_empty());
        assert_eq!(timeline.len(), 0);
        assert!(timeline.chapters().is_empty());
        assert_eq!(timeline.outcome(), None);
    }

    #[test]
    fn a_bubble_is_found_by_its_key_in_the_chat() {
        assert_eq!(key(3, "m12"), "3-m12");
        assert_eq!(bubble(&key(0, "m1")), Some(0));
        assert_eq!(bubble(&key(3, "m12")), Some(11));
        assert_eq!(bubble("2-m0"), None);
        assert_eq!(bubble("m1"), None);
        assert_eq!(bubble("think-Alice"), None);
    }

    #[test]
    fn a_screen_under_review_takes_the_place_of_the_screen_in_its_era() {
        let cues = recorded();
        let timeline = Timeline::new(&cues);
        let mut screen = Screen::default();
        screen.reset();
        screen.review(timeline.screen(3));
        assert_eq!(screen.era(), 1);
        assert_eq!(screen.cursor, Some(3));
        assert_eq!(Screen { era: 0, ..screen }, timeline.screen(3));
    }

    #[test]
    fn the_recorded_game_is_won_on_the_guess_its_answer_names() {
        let screen = shown(&recorded());
        let won = screen.won.expect("the recorded game is won");
        assert!(matches!(
            screen.outcome,
            Some(Outcome::Won {
                by: Victory::Named,
                ..
            })
        ));
        assert!(screen.secret.is_some());
        let color = screen
            .asker_color(won)
            .expect("a player put the winning guess");
        assert_eq!(screen.pips()[won as usize - 1], Pip::Won(color));
        let answers = screen.posts.iter().filter(|post| post.name == "Host");
        assert!(answers.count() as u64 >= won);
        assert!(screen.thinkers.is_empty());
    }
}
