//! Which game the page shows, and how fast **Play** replays it, from the
//! URL hash.

/// The games the page links to, each by the name of its cues file in
/// `game/` and its title. The cues are in a directory of their own so that
/// the page builds without them, as the desktop app's does.
pub const GAMES: [(&str, &str); 2] = [("avalon", "Avalon"), ("werewolf", "Werewolf")];

/// The recorded prompted games, which the screen check opens by hash.
pub const RECORDED: [&str; 2] = ["avalon-prompted", "werewolf-prompted"];

/// The hash suffix that replays ten times as fast.
const FAST: &str = "-fast";

/// The game a hash names and the pace of its replay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Address {
    /// The name of the game's cues file, without `.json`.
    pub game: &'static str,
    /// How many times the usual speed **Play** replays at.
    pub pace: f64,
}

impl Address {
    /// The address `hash` names: `#werewolf`, or `#werewolf-prompted-fast`
    /// at ten times the pace. Any other hash names the first of the
    /// [`GAMES`].
    pub fn parse(hash: &str) -> Self {
        let name = hash.strip_prefix('#').unwrap_or(hash);
        let (name, pace) = match name.strip_suffix(FAST) {
            Some(name) => (name, 10.0),
            None => (name, 1.0),
        };
        let game = GAMES
            .iter()
            .map(|(game, _)| *game)
            .chain(RECORDED)
            .find(|game| *game == name)
            .unwrap_or(GAMES[0].0);
        Self { game, pace }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hash_names_a_linked_game_at_the_usual_pace() {
        assert_eq!(
            Address::parse("#werewolf"),
            Address {
                game: "werewolf",
                pace: 1.0
            }
        );
    }

    #[test]
    fn a_hash_names_a_recorded_game() {
        assert_eq!(Address::parse("#avalon-prompted").game, "avalon-prompted");
    }

    #[test]
    fn the_fast_suffix_replays_ten_times_as_fast() {
        assert_eq!(
            Address::parse("#werewolf-prompted-fast"),
            Address {
                game: "werewolf-prompted",
                pace: 10.0
            }
        );
    }

    #[test]
    fn no_hash_or_an_unknown_one_names_avalon() {
        let avalon = Address {
            game: "avalon",
            pace: 1.0,
        };
        assert_eq!(Address::parse(""), avalon);
        assert_eq!(Address::parse("#chess"), avalon);
    }
}
