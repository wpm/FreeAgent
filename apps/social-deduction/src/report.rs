//! What every game shares in telling its story: the order the seats are
//! told in, and how long something took.

use crate::seat::{PlayerId, PlayerName};
use humantime::format_duration;
use std::cmp::Ordering;
use std::time::Duration;

/// `elapsed` to the nearest whole second, as `humantime` writes it: `40s`,
/// `1m 5s`.
pub(crate) fn whole_seconds(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs_f64().round() as u64;
    format_duration(Duration::from_secs(seconds)).to_string()
}

/// `players` in seat order.
pub(crate) fn in_order<'a>(players: impl IntoIterator<Item = &'a PlayerId>) -> Vec<&'a PlayerId> {
    let mut players: Vec<&PlayerId> = players.into_iter().collect();
    players.sort_by_cached_key(|player| PlayerName::from(player.as_str()));
    players
}

/// Seat order: the order of the [`PlayerName`]s, so that `Player 2`
/// comes before `Player 10`.
pub fn seat_order(a: &PlayerId, b: &PlayerId) -> Ordering {
    PlayerName::from(a.as_str()).cmp(&PlayerName::from(b.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn players_come_in_seat_order_with_player_2_before_player_10() {
        let players = ["Player 10", "ann", "Player 2", "Player 1"].map(String::from);
        assert_eq!(
            in_order(&players),
            ["ann", "Player 1", "Player 2", "Player 10"]
        );
    }

    #[test]
    fn a_duration_is_told_to_the_nearest_whole_second() {
        assert_eq!(whole_seconds(Duration::from_millis(40_400)), "40s");
        assert_eq!(whole_seconds(Duration::from_millis(64_600)), "1m 5s");
    }
}
