//! What every game shares in telling its story: the order the seats are
//! told in, and how long something took.

use crate::seat::PlayerId;
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
    players.sort_unstable_by(|a, b| seat_order(a, b));
    players
}

/// Seat order: by the number a name ends in, so that `player2` comes
/// before `player10`, and by the name where that does not tell.
pub fn seat_order(a: &PlayerId, b: &PlayerId) -> Ordering {
    (numbered(a), a).cmp(&(numbered(b), b))
}

/// `name` as what comes before its number and the number, when it ends in
/// one.
pub(crate) fn numbered(name: &str) -> (&str, Option<u64>) {
    let prefix = name.trim_end_matches(|c: char| c.is_ascii_digit());
    let number = name[prefix.len()..].parse().ok();
    (prefix, number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn players_come_in_seat_order_with_player2_before_player10() {
        let players = ["player10", "ann", "player2", "player1"].map(String::from);
        assert_eq!(
            in_order(&players),
            ["ann", "player1", "player2", "player10"]
        );
    }

    #[test]
    fn a_name_is_split_from_the_number_it_ends_in() {
        assert_eq!(numbered("player12"), ("player", Some(12)));
        assert_eq!(numbered("ann"), ("ann", None));
    }

    #[test]
    fn a_duration_is_told_to_the_nearest_whole_second() {
        assert_eq!(whole_seconds(Duration::from_millis(40_400)), "40s");
        assert_eq!(whole_seconds(Duration::from_millis(64_600)), "1m 5s");
    }
}
