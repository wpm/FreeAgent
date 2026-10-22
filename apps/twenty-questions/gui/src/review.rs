//! Review: a game log's games, each folded at once into a [`Timeline`],
//! and the bubble under the cursor. A game opens on its first bubble, and a
//! game just played on its last, where it left off. The cursor moves
//! bubble by bubble, by chapter, to either end, or to another game.

use twenty_questions_gui_types::go::Go;
use twenty_questions_gui_types::{Cue, Opened, Outcome, Victory};

use crate::screen::{Screen, Timeline};

/// One game of the log, folded.
#[derive(Debug, Clone, PartialEq)]
struct Game {
    /// Its secret and how it ended, for the list of games.
    title: String,
    /// The screen at each of its bubbles.
    timeline: Timeline,
}

impl Game {
    /// The game that `timeline` folds.
    fn new(timeline: Timeline) -> Self {
        let secret = timeline.secret().unwrap_or("no secret");
        Self {
            title: format!("{secret}, {}", ended(timeline.outcome(), timeline.won())),
            timeline,
        }
    }
}

/// A game log under review.
#[derive(Debug, Clone, PartialEq)]
pub struct Review {
    /// The file's name, for a log.
    name: Option<String>,
    /// Every game with a bubble to show, in the log's order.
    games: Vec<Game>,
    /// Why each episode left out was left out.
    problems: Vec<String>,
    /// The game shown.
    game: usize,
    /// The bubble under the cursor.
    cursor: usize,
}

impl Review {
    /// The log `opened`, on the first bubble of its first game. An episode
    /// with no bubble is left out, as a problem.
    ///
    /// # Errors
    ///
    /// Every problem, when no game is left to show.
    pub fn new(opened: Opened) -> Result<Self, String> {
        let Opened { name, recordings } = opened;
        let mut problems = recordings.problems;
        let mut games = Vec::new();
        for recording in recordings.games {
            let timeline = Timeline::new(&recording.cues);
            if timeline.is_empty() {
                problems.push(format!("episode {} shows nothing", recording.episode));
                continue;
            }
            games.push(Game::new(timeline));
        }
        if games.is_empty() {
            return Err(if problems.is_empty() {
                format!("{name} holds no games.")
            } else {
                problems.join("\n")
            });
        }
        Ok(Self {
            name: Some(name),
            games,
            problems,
            game: 0,
            cursor: 0,
        })
    }

    /// The game just played that `cues` tell, as they were shown, on its
    /// last bubble, where the game left off. A game with no bubble has
    /// nothing to review.
    pub fn played(cues: &[Cue]) -> Option<Self> {
        let timeline = Timeline::new(cues);
        if timeline.is_empty() {
            return None;
        }
        Some(Self {
            name: None,
            cursor: timeline.len() - 1,
            games: vec![Game::new(timeline)],
            problems: Vec::new(),
            game: 0,
        })
    }

    /// The file's name, for a log.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Why each episode left out was left out.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// Each game's secret and how it ended, numbered: `2. a unicycle, the
    /// players won in 9`.
    pub fn titles(&self) -> Vec<String> {
        self.games
            .iter()
            .enumerate()
            .map(|(index, game)| format!("{}. {}", index + 1, game.title))
            .collect()
    }

    /// How many games there are.
    pub fn games(&self) -> usize {
        self.games.len()
    }

    /// The game shown, counted from 0.
    pub fn game(&self) -> usize {
        self.game
    }

    /// The bubble under the cursor, counted from 0.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// How many bubbles the game shown has.
    pub fn len(&self) -> usize {
        self.timeline().len()
    }

    /// Whether the game shown has no bubbles, which a game under review
    /// never is.
    pub fn is_empty(&self) -> bool {
        self.timeline().is_empty()
    }

    /// Where the cursor is, `1 / 212`.
    pub fn position(&self) -> String {
        format!("{} / {}", self.cursor + 1, self.len())
    }

    /// The screen with the cursor on its bubble.
    pub fn screen(&self) -> Screen {
        self.timeline().screen(self.cursor)
    }

    /// Move the cursor as `go` says.
    pub fn go(&mut self, go: Go) {
        let last = self.len() - 1;
        let cursor = self.cursor;
        let chapters = self.timeline().chapters();
        let before = chapters.iter().rev().find(|&&k| k < cursor).copied();
        let after = chapters.iter().find(|&&k| k > cursor).copied();
        match go {
            Go::Previous => self.select(cursor.saturating_sub(1)),
            Go::Next => self.select(cursor + 1),
            Go::First => self.select(0),
            Go::Last => self.select(last),
            Go::PreviousChapter => self.select(before.unwrap_or(0)),
            Go::NextChapter => self.select(after.unwrap_or(last)),
            Go::PreviousGame => self.choose(self.game.saturating_sub(1)),
            Go::NextGame => self.choose(self.game + 1),
        }
    }

    /// Put the cursor on bubble `k`, or the last bubble when there is no
    /// bubble `k`.
    pub fn select(&mut self, k: usize) {
        self.cursor = k.min(self.len() - 1);
    }

    /// Show game `game`, or the last game when there is no game `game`, on
    /// its first bubble.
    pub fn choose(&mut self, game: usize) {
        self.game = game.min(self.games.len() - 1);
        self.cursor = 0;
    }

    /// The timeline of the game shown.
    fn timeline(&self) -> &Timeline {
        &self.games[self.game].timeline
    }
}

/// How a game ended, for the list of games: `the players won in 9`, given
/// the guess that `won`.
fn ended(outcome: Option<&Outcome>, won: Option<u64>) -> String {
    match outcome {
        Some(Outcome::Won {
            by: Victory::Named, ..
        }) => match won {
            Some(guess) => format!("the players won in {guess}"),
            None => "the players won".to_string(),
        },
        Some(Outcome::Won { .. }) => "the host won".to_string(),
        Some(Outcome::Stopped) => "stopped".to_string(),
        Some(Outcome::Failed(_)) => "failed".to_string(),
        Some(Outcome::Unfinished) | None => "unfinished".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use twenty_questions_gui_types::{Recording, Recordings};
    use uuid::Uuid;

    /// The recorded game in `fixtures/`, as the episode `episode`.
    fn recording(episode: u128) -> Recording {
        let log = include_str!("../fixtures/prompted.jsonl");
        let mut read = twenty_questions::stage::recordings(log).unwrap();
        Recording {
            episode: Uuid::from_u128(episode),
            ..read.games.remove(0)
        }
    }

    /// The recorded game cut off after its first `cues` cues, as the
    /// episode `episode`.
    fn cut(episode: u128, cues: usize) -> Recording {
        let mut recording = recording(episode);
        recording.cues.truncate(cues);
        recording
    }

    /// A log of three games: the recorded one, the recorded one cut off
    /// half way, and the recorded one again.
    fn three() -> Review {
        let half = recording(0).cues.len() / 2;
        let games = vec![recording(1), cut(2, half), recording(3)];
        Review::new(Opened {
            name: "three.jsonl".to_string(),
            recordings: Recordings {
                games,
                problems: vec!["the log is bad at line 9".to_string()],
            },
        })
        .unwrap()
    }

    #[test]
    fn a_log_of_three_games_opens_on_the_first_bubble_of_the_first() {
        let review = three();
        assert_eq!(review.name(), Some("three.jsonl"));
        assert_eq!(review.games(), 3);
        assert_eq!((review.game(), review.cursor()), (0, 0));
        assert_eq!(review.position(), format!("1 / {}", review.len()));
        assert!(!review.is_empty());
        assert_eq!(review.screen(), Timeline::new(&recording(1).cues).screen(0));
        assert_eq!(review.problems(), ["the log is bad at line 9"]);
    }

    #[test]
    fn a_game_just_played_opens_on_its_last_bubble_as_its_log_would_read_back() {
        let played = recording(1);
        let review = Review::played(&played.cues).unwrap();
        let last = review.len() - 1;
        assert_eq!(review.name(), None);
        assert_eq!(
            (review.games(), review.game(), review.cursor()),
            (1, 0, last)
        );
        assert_eq!(review.screen(), Timeline::new(&played.cues).screen(last));
        assert!(review.problems().is_empty());
        let opened = Review::new(Opened {
            name: "twenty-questions.jsonl".to_string(),
            recordings: Recordings {
                games: vec![played],
                problems: Vec::new(),
            },
        })
        .unwrap();
        let read_back = Review {
            name: None,
            cursor: last,
            ..opened
        };
        assert_eq!(review, read_back);
    }

    #[test]
    fn a_game_that_showed_no_bubble_has_nothing_to_review() {
        assert_eq!(Review::played(&[]), None);
        assert_eq!(Review::played(&cut(1, 1).cues), None);
    }

    #[test]
    fn each_game_is_listed_by_its_secret_and_how_it_ended() {
        let titles = three().titles();
        let secret = Timeline::new(&recording(1).cues)
            .secret()
            .unwrap()
            .to_string();
        let won = Timeline::new(&recording(1).cues).won().unwrap();
        assert_eq!(
            titles,
            [
                format!("1. {secret}, the players won in {won}"),
                format!("2. {secret}, unfinished"),
                format!("3. {secret}, the players won in {won}"),
            ]
        );
        let host = |by| Outcome::Won {
            by,
            how: String::new(),
        };
        assert_eq!(ended(Some(&host(Victory::Named)), None), "the players won");
        assert_eq!(ended(Some(&host(Victory::OutOfTime)), None), "the host won");
        assert_eq!(
            ended(Some(&host(Victory::OutOfGuesses)), None),
            "the host won"
        );
        assert_eq!(ended(Some(&Outcome::Stopped), None), "stopped");
        assert_eq!(
            ended(Some(&Outcome::Failed("no".to_string())), None),
            "failed"
        );
        assert_eq!(ended(Some(&Outcome::Unfinished), None), "unfinished");
    }

    #[test]
    fn a_game_cut_off_before_its_secret_is_listed_as_having_none() {
        let review = Review::new(Opened {
            name: "early.jsonl".to_string(),
            recordings: Recordings {
                games: vec![cut(1, 3)],
                problems: Vec::new(),
            },
        })
        .unwrap();
        assert_eq!(review.titles(), ["1. no secret, unfinished"]);
    }

    #[test]
    fn another_game_opens_on_its_first_bubble() {
        let mut review = three();
        review.go(Go::Last);
        review.go(Go::NextGame);
        assert_eq!((review.game(), review.cursor()), (1, 0));
        review.go(Go::Next);
        review.go(Go::PreviousGame);
        assert_eq!((review.game(), review.cursor()), (0, 0));
        review.choose(2);
        review.go(Go::Next);
        review.go(Go::NextGame);
        assert_eq!((review.game(), review.cursor()), (2, 0));
        review.go(Go::PreviousGame);
        review.go(Go::PreviousGame);
        review.go(Go::PreviousGame);
        assert_eq!(review.game(), 0);
        review.choose(9);
        assert_eq!(review.game(), 2);
    }

    #[test]
    fn first_and_last_go_to_either_end_and_the_cursor_stops_there() {
        let mut review = three();
        let last = review.len() - 1;
        review.go(Go::Last);
        assert_eq!(review.cursor(), last);
        assert_eq!(review.position(), format!("{} / {}", last + 1, last + 1));
        review.go(Go::Next);
        assert_eq!(review.cursor(), last);
        review.go(Go::First);
        assert_eq!(review.cursor(), 0);
        review.go(Go::Previous);
        assert_eq!(review.cursor(), 0);
        review.select(last + 10);
        assert_eq!(review.cursor(), last);
    }

    #[test]
    fn stepping_shows_the_screen_at_each_bubble() {
        let mut review = three();
        let timeline = Timeline::new(&recording(1).cues);
        review.go(Go::Next);
        review.go(Go::Next);
        assert_eq!(review.screen(), timeline.screen(2));
        review.go(Go::Previous);
        assert_eq!(review.screen(), timeline.screen(1));
    }

    #[test]
    fn chapters_step_from_guess_to_guess_and_stop_at_either_end() {
        let mut review = three();
        let guesses = Timeline::new(&recording(1).cues).chapters();
        assert!(guesses.len() > 2, "{guesses:?}");
        review.go(Go::NextChapter);
        assert_eq!(review.cursor(), guesses[0]);
        review.go(Go::NextChapter);
        assert_eq!(review.cursor(), guesses[1]);
        review.go(Go::Next);
        review.go(Go::PreviousChapter);
        assert_eq!(review.cursor(), guesses[1]);
        review.go(Go::PreviousChapter);
        assert_eq!(review.cursor(), guesses[0]);
        review.go(Go::PreviousChapter);
        assert_eq!(review.cursor(), 0);
        review.select(*guesses.last().unwrap());
        review.go(Go::NextChapter);
        assert_eq!(review.cursor(), review.len() - 1);
    }

    #[test]
    fn an_episode_with_nothing_to_show_is_left_out_and_a_log_with_none_does_not_open() {
        let empty = Recording {
            episode: Uuid::from_u128(7),
            cues: Vec::new(),
        };
        let opened = |games: Vec<Recording>, problems: Vec<String>| Opened {
            name: "empty.jsonl".to_string(),
            recordings: Recordings { games, problems },
        };
        let review = Review::new(opened(vec![empty.clone(), recording(2)], vec![])).unwrap();
        assert_eq!(review.games(), 1);
        assert_eq!(
            review.problems(),
            [format!("episode {} shows nothing", Uuid::from_u128(7))]
        );
        let error = Review::new(opened(vec![empty], vec!["bad".to_string()])).unwrap_err();
        assert_eq!(
            error,
            format!("bad\nepisode {} shows nothing", Uuid::from_u128(7))
        );
        assert_eq!(
            Review::new(opened(vec![], vec![])).unwrap_err(),
            "empty.jsonl holds no games."
        );
    }
}
