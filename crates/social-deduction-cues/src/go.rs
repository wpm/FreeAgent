//! Where the cursor of a game under review goes, and the **Go** menu that
//! moves it. The desktop app builds the menu and the page moves the
//! cursor, so a [`Go`] crosses between the two.

use serde::{Deserialize, Serialize};

/// Where the cursor goes: the **Go** menu's items, and the keys the main
/// window handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

impl Go {
    /// Whether it moves between games, which a log of several has.
    pub fn between_games(self) -> bool {
        matches!(self, Go::PreviousGame | Go::NextGame)
    }
}

/// One item of the **Go** menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    /// Where it takes the cursor.
    pub go: Go,
    /// What the menu calls it.
    pub label: &'static str,
    /// Its shortcut on macOS.
    pub mac: &'static str,
    /// Its shortcut elsewhere.
    pub elsewhere: &'static str,
}

impl Item {
    /// Its shortcut, on macOS when `mac`.
    pub fn shortcut(&self, mac: bool) -> &'static str {
        if mac { self.mac } else { self.elsewhere }
    }
}

/// The **Go** menu's items, in groups with a separator between: either
/// end, chapter by chapter, and game by game. The bubble before and after
/// are the arrow keys', unmodified.
pub const MENU: [[Item; 2]; 3] = [
    [
        Item {
            go: Go::First,
            label: "First Bubble",
            mac: "Cmd+Up",
            elsewhere: "Ctrl+Home",
        },
        Item {
            go: Go::Last,
            label: "Last Bubble",
            mac: "Cmd+Down",
            elsewhere: "Ctrl+End",
        },
    ],
    [
        Item {
            go: Go::PreviousChapter,
            label: "Previous Chapter",
            mac: "Alt+Up",
            elsewhere: "Alt+Up",
        },
        Item {
            go: Go::NextChapter,
            label: "Next Chapter",
            mac: "Alt+Down",
            elsewhere: "Alt+Down",
        },
    ],
    [
        Item {
            go: Go::PreviousGame,
            label: "Previous Game",
            mac: "Cmd+[",
            elsewhere: "Ctrl+[",
        },
        Item {
            go: Go::NextGame,
            label: "Next Game",
            mac: "Cmd+]",
            elsewhere: "Ctrl+]",
        },
    ],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn where_the_cursor_goes_crosses_in_snake_case() {
        let named = |go: Go| serde_json::to_string(&go).unwrap();
        assert_eq!(named(Go::PreviousChapter), "\"previous_chapter\"");
        let keyed = [Go::Previous, Go::Next];
        for go in MENU.iter().flatten().map(|item| item.go).chain(keyed) {
            assert_eq!(serde_json::from_str::<Go>(&named(go)).unwrap(), go);
        }
    }

    #[test]
    fn only_the_last_group_moves_between_games() {
        let [ends, chapters, games] = MENU;
        for item in ends.iter().chain(&chapters) {
            assert!(!item.go.between_games(), "{item:?}");
        }
        for item in games {
            assert!(item.go.between_games(), "{item:?}");
        }
        assert!(!Go::Previous.between_games());
    }

    #[test]
    fn a_shortcut_takes_command_on_a_mac_and_control_elsewhere() {
        let [[first, _], [chapter, _], [game, _]] = MENU;
        assert_eq!(first.shortcut(true), "Cmd+Up");
        assert_eq!(first.shortcut(false), "Ctrl+Home");
        assert_eq!(chapter.shortcut(true), chapter.shortcut(false));
        assert_eq!(game.shortcut(false), "Ctrl+[");
    }
}
