//! Review: a game log's games, each folded at once into a [`Timeline`],
//! and the bubble under the cursor. A game opens on its first bubble, and a
//! game just played on its last, where it left off. The cursor moves
//! bubble by bubble, by chapter, to either end, or to another game.

use serde::Deserialize;
use social_deduction_cues::settings::game_title;
use social_deduction_cues::{Cue, GameName, Opened, Outcome};

use crate::screen::{Landed, Screen, Timeline, side_key};

/// Where the cursor goes: the **Go** menu's items, and the keys the main
/// window handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Go {
    /// The bubble before.
    Previous,
    /// The bubble after.
    Next,
    /// The first bubble.
    First,
    /// The last bubble.
    Last,
    /// The chapter heading before.
    PreviousChapter,
    /// The chapter heading after.
    NextChapter,
    /// The first bubble of the game before.
    PreviousGame,
    /// The first bubble of the game after.
    NextGame,
}

/// Where the key `key` takes the cursor, pressed with a modifier or not:
/// an unmodified arrow to the bubble before or after, and Home or End to
/// either end. A key pressed with a modifier is the menu's.
pub fn keyed(key: &str, modified: bool) -> Option<Go> {
    if modified {
        return None;
    }
    match key {
        "ArrowUp" | "ArrowLeft" => Some(Go::Previous),
        "ArrowDown" | "ArrowRight" => Some(Go::Next),
        "Home" => Some(Go::First),
        "End" => Some(Go::Last),
        _ => None,
    }
}

/// One game of the log, folded.
#[derive(Debug, Clone, PartialEq)]
struct Game {
    /// Its name and how it ended, for the list of games.
    title: String,
    /// The screen at each of its bubbles.
    timeline: Timeline,
    /// Its chapter headings' bubbles, in order.
    chapters: Vec<usize>,
}

impl Game {
    /// The game of `game` that `timeline` folds.
    fn new(game: GameName, timeline: Timeline) -> Self {
        Self {
            title: format!("{}, {}", game_title(game), ended(timeline.outcome())),
            chapters: timeline.chapters(),
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
            games.push(Game::new(recording.game, timeline));
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
        let game = timeline.game()?;
        Some(Self {
            name: None,
            cursor: timeline.len() - 1,
            games: vec![Game::new(game, timeline)],
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

    /// Each game's name and how it ended, numbered: `2. Werewolf, good
    /// won`.
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

    /// Move the cursor as `go` says. Moving forward lands what the new
    /// bubble's board makes; moving back lands nothing.
    pub fn go(&mut self, go: Go) -> Vec<Landed> {
        let last = self.len() - 1;
        let cursor = self.cursor;
        let chapters = &self.games[self.game].chapters;
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
    /// bubble `k`, landing what it lands moving forward.
    pub fn select(&mut self, k: usize) -> Vec<Landed> {
        let k = k.min(self.len() - 1);
        let forward = k > self.cursor;
        self.cursor = k;
        if forward {
            self.timeline().landed(k)
        } else {
            Vec::new()
        }
    }

    /// Show game `game`, or the last game when there is no game `game`, on
    /// its first bubble, which lands nothing.
    pub fn choose(&mut self, game: usize) -> Vec<Landed> {
        self.game = game.min(self.games.len() - 1);
        self.cursor = 0;
        Vec::new()
    }

    /// The timeline of the game shown.
    fn timeline(&self) -> &Timeline {
        &self.games[self.game].timeline
    }
}

/// How a game ended, for the list of games: `good won`.
fn ended(outcome: Option<&Outcome>) -> String {
    match outcome {
        Some(Outcome::Won { side, .. }) => format!("{} won", side_key(*side)),
        Some(Outcome::Stopped) => "stopped".to_string(),
        Some(Outcome::Failed(_)) => "failed".to_string(),
        Some(Outcome::Unfinished) | None => "unfinished".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use social_deduction_cues::{Recording, Recordings};
    use uuid::Uuid;

    /// The recorded game `name` in `fixtures/`, as the episode `episode`.
    fn recording(name: &str, episode: u128) -> Recording {
        let path = format!("{}/fixtures/{name}.jsonl", env!("CARGO_MANIFEST_DIR"));
        let log = std::fs::read_to_string(&path).unwrap();
        let mut read = social_deduction::stage::recordings(&log).unwrap();
        Recording {
            episode: Uuid::from_u128(episode),
            ..read.games.remove(0)
        }
    }

    /// A log of three games: Werewolf, Avalon and Werewolf again.
    fn three() -> Review {
        let games = vec![
            recording("werewolf-prompted", 1),
            recording("avalon-prompted", 2),
            recording("werewolf-prompted", 3),
        ];
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
        assert_eq!(
            review.screen(),
            Timeline::new(&recording("werewolf-prompted", 1).cues).screen(0)
        );
        assert_eq!(review.problems(), ["the log is bad at line 9"]);
    }

    #[test]
    fn a_game_just_played_opens_on_its_last_bubble_as_its_log_would_read_back() {
        let played = recording("werewolf-prompted", 1);
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
            name: "werewolf.jsonl".to_string(),
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
        let cast = recording("avalon-prompted", 1).cues[..1].to_vec();
        assert_eq!(Review::played(&cast), None);
    }

    #[test]
    fn each_game_is_listed_by_its_name_and_how_it_ended() {
        let titles = three().titles();
        assert_eq!(titles.len(), 3);
        assert!(titles[0].starts_with("1. Werewolf, "), "{titles:?}");
        assert!(titles[0].ends_with(" won"), "{titles:?}");
        assert!(titles[1].starts_with("2. Avalon, "), "{titles:?}");
        assert_eq!(ended(Some(&Outcome::Stopped)), "stopped");
        assert_eq!(ended(Some(&Outcome::Failed("no".to_string()))), "failed");
        assert_eq!(ended(Some(&Outcome::Unfinished)), "unfinished");
    }

    #[test]
    fn another_game_opens_on_its_first_bubble() {
        let mut review = three();
        review.go(Go::Last);
        assert!(review.go(Go::NextGame).is_empty());
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
    }

    #[test]
    fn first_and_last_go_to_either_end_and_the_cursor_stops_there() {
        let mut review = three();
        let last = review.len() - 1;
        review.go(Go::Last);
        assert_eq!(review.cursor(), last);
        assert_eq!(review.position(), format!("{} / {}", last + 1, last + 1));
        assert!(review.go(Go::Next).is_empty());
        assert_eq!(review.cursor(), last);
        review.go(Go::First);
        assert_eq!(review.cursor(), 0);
        review.go(Go::Previous);
        assert_eq!(review.cursor(), 0);
        review.select(last + 10);
        assert_eq!(review.cursor(), last);
    }

    #[test]
    fn stepping_forward_lands_what_the_bubble_lands_and_stepping_back_lands_nothing() {
        let mut review = three();
        let timeline = Timeline::new(&recording("werewolf-prompted", 1).cues);
        let k = (1..timeline.len())
            .find(|&k| !timeline.landed(k).is_empty())
            .unwrap();
        review.select(k - 1);
        assert_eq!(review.go(Go::Next), timeline.landed(k));
        assert_eq!(review.screen(), timeline.screen(k));
        assert!(review.go(Go::Previous).is_empty());
        assert_eq!(review.select(k), timeline.landed(k));
        assert!(review.select(k).is_empty());
    }

    #[test]
    fn chapters_step_from_heading_to_heading_and_stop_at_either_end() {
        let mut review = three();
        let headings = Timeline::new(&recording("werewolf-prompted", 1).cues).chapters();
        assert!(headings.len() > 2, "{headings:?}");
        review.select(headings[0]);
        review.go(Go::NextChapter);
        assert_eq!(review.cursor(), headings[1]);
        review.go(Go::Next);
        review.go(Go::PreviousChapter);
        assert_eq!(review.cursor(), headings[1]);
        review.go(Go::PreviousChapter);
        assert_eq!(review.cursor(), headings[0]);
        review.select(*headings.last().unwrap());
        review.go(Go::NextChapter);
        assert_eq!(review.cursor(), review.len() - 1);
        review.select(headings[0]);
        review.go(Go::PreviousChapter);
        review.go(Go::PreviousChapter);
        assert_eq!(review.cursor(), 0);
    }

    #[test]
    fn unmodified_arrows_and_home_and_end_move_the_cursor_and_modified_keys_are_the_menu_s() {
        assert_eq!(keyed("ArrowUp", false), Some(Go::Previous));
        assert_eq!(keyed("ArrowLeft", false), Some(Go::Previous));
        assert_eq!(keyed("ArrowDown", false), Some(Go::Next));
        assert_eq!(keyed("ArrowRight", false), Some(Go::Next));
        assert_eq!(keyed("Home", false), Some(Go::First));
        assert_eq!(keyed("End", false), Some(Go::Last));
        assert_eq!(keyed("a", false), None);
        for key in [
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
            "Home",
            "End",
        ] {
            assert_eq!(keyed(key, true), None);
        }
    }

    #[test]
    fn the_menu_names_where_the_cursor_goes() {
        let go: Go = serde_json::from_str("\"previous_chapter\"").unwrap();
        assert_eq!(go, Go::PreviousChapter);
    }

    #[test]
    fn an_episode_with_nothing_to_show_is_left_out_and_a_log_with_none_does_not_open() {
        let empty = Recording {
            episode: Uuid::from_u128(7),
            cues: Vec::new(),
            ..recording("avalon-prompted", 7)
        };
        let opened = |games: Vec<Recording>, problems: Vec<String>| Opened {
            name: "empty.jsonl".to_string(),
            recordings: Recordings { games, problems },
        };
        let review = Review::new(opened(
            vec![empty.clone(), recording("avalon-prompted", 2)],
            vec![],
        ))
        .unwrap();
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
