//! Which recorded game the web page shows, and how fast **Play** plays
//! it, from the URL hash.

/// The game the page shows when the hash names none: the cues
/// `build-page.sh` folds from `fixtures/prompted.jsonl`.
pub const PROMPTED: &str = "prompted";

/// The hash suffix that plays ten times as fast.
const FAST: &str = "-fast";

/// The game a hash names and the pace **Play** plays it at.
#[derive(Debug, Clone, PartialEq)]
pub struct Address {
    /// The name of the game's cues file in `game/`, without `.json`.
    pub game: String,
    /// How many times the usual speed **Play** plays at.
    pub pace: f64,
}

impl Address {
    /// The address `hash` names: `#prompted`, or `#prompted-fast` at ten
    /// times the pace. A name is lowercase letters, digits and dashes;
    /// any other hash names [`PROMPTED`].
    pub fn parse(hash: &str) -> Self {
        let name = hash.strip_prefix('#').unwrap_or(hash);
        let (name, pace) = match name.strip_suffix(FAST) {
            Some(name) => (name, 10.0),
            None => (name, 1.0),
        };
        let named = !name.is_empty()
            && (name.chars()).all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        let game = if named { name } else { PROMPTED };
        Self {
            game: game.to_string(),
            pace,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The address of `game` at `pace`.
    fn address(game: &str, pace: f64) -> Address {
        Address {
            game: game.to_string(),
            pace,
        }
    }

    #[test]
    fn a_hash_names_a_recorded_game_at_the_usual_pace() {
        assert_eq!(Address::parse("#won-in-9"), address("won-in-9", 1.0));
    }

    #[test]
    fn the_fast_suffix_plays_ten_times_as_fast() {
        assert_eq!(Address::parse("#prompted-fast"), address("prompted", 10.0));
        assert_eq!(Address::parse("#-fast"), address(PROMPTED, 10.0));
    }

    #[test]
    fn no_hash_or_one_that_names_no_file_names_the_prompted_game() {
        assert_eq!(Address::parse(""), address(PROMPTED, 1.0));
        assert_eq!(Address::parse("#"), address(PROMPTED, 1.0));
        assert_eq!(Address::parse("#../settings"), address(PROMPTED, 1.0));
        assert_eq!(Address::parse("#Prompted"), address(PROMPTED, 1.0));
    }
}
