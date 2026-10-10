//! A legible account of a game, told from its log as the log is written.

use super::{Entry, Message, Observation, Phase, PlayerId, Role};
use std::collections::{HashMap, HashSet};
use std::num::NonZero;

/// Turns the log's entries, one at a time, into lines of prose: the
/// table, then each night and day with what every player did and how it
/// ended, then who won. It learns who died from who is shown as alive
/// next, so a phase's result is told as the next one begins. A phase
/// announced is told as one requested, and a selection as a reply.
#[derive(Debug, Default)]
pub struct Narrator {
    /// Every player's role, from the deal.
    roles: HashMap<PlayerId, Role>,
    /// Who was alive when the current phase began.
    alive: HashSet<PlayerId>,
    /// The phase being told, once one has begun.
    phase: Option<(NonZero<u8>, Phase)>,
}

impl Narrator {
    /// The lines `entry` adds to the account, often none.
    pub fn narrate(&mut self, entry: &Entry) -> Vec<String> {
        match entry {
            Entry::Start { variation, roles } => {
                self.roles = roles.clone();
                self.alive = roles.keys().cloned().collect();
                vec![self.table(variation)]
            }
            Entry::Sent {
                message: Message::Observation(observation) | Message::Announce { observation, .. },
                ..
            } => self.begin(observation),
            Entry::Sent { .. } => vec![],
            Entry::Replied {
                from,
                message: Message::Action(chosen),
            }
            | Entry::Received {
                from,
                message: Message::Select { target: chosen, .. },
            } => vec![self.deed(from, chosen)],
            Entry::Replied { .. } | Entry::Received { .. } => vec![],
            Entry::End {
                winner,
                days,
                survivors,
            } => {
                let mut lines = self.outcome(survivors);
                let mut names: Vec<&PlayerId> = survivors.iter().collect();
                names.sort();
                let seats: Vec<String> = names.iter().map(|name| self.seat(name)).collect();
                lines.push(format!(
                    "The {winner} win after {days} {}. Survivors: {}.",
                    if *days == 1 { "day" } else { "days" },
                    seats.join(", ")
                ));
                lines
            }
        }
    }

    /// The deal, with every player's role.
    fn table(&self, variation: &str) -> String {
        let mut seats: Vec<(&PlayerId, &Role)> = self.roles.iter().collect();
        seats.sort_by(|a, b| a.0.cmp(b.0));
        let seats: Vec<String> = seats.iter().map(|(player, _)| self.seat(player)).collect();
        format!(
            "{variation} with {} players: {}.",
            self.roles.len(),
            seats.join(", ")
        )
    }

    /// A player with its role, as "player (role)", or the name alone when
    /// the role is unknown.
    fn seat(&self, player: &PlayerId) -> String {
        match self.roles.get(player) {
            Some(role) => format!("{player} ({role})"),
            None => player.clone(),
        }
    }

    /// A phase begins when a player is first shown it. The phase before
    /// it ends then, with whoever is no longer alive.
    fn begin(&mut self, observation: &Observation) -> Vec<String> {
        let phase = (observation.round, observation.phase.clone());
        if self.phase.as_ref() == Some(&phase) {
            return vec![];
        }
        let mut lines = self.outcome(&observation.alive);
        self.alive = observation.alive.clone();
        self.phase = Some(phase);
        lines.push(match observation.phase {
            Phase::Night => format!("Night {}.", observation.round),
            Phase::Day => format!("Day {}.", observation.round),
        });
        lines
    }

    /// How the phase being told ended, given who is alive after it.
    fn outcome(&mut self, alive_after: &HashSet<PlayerId>) -> Vec<String> {
        let Some((_, phase)) = &self.phase else {
            return vec![];
        };
        let mut dead: Vec<&PlayerId> = self.alive.difference(alive_after).collect();
        dead.sort();
        let line = match (phase, dead.as_slice()) {
            (Phase::Night, []) => "Nobody dies.".to_string(),
            (Phase::Night, dead) => format!("{} dies.", names(dead)),
            (Phase::Day, []) => "Nobody is voted out.".to_string(),
            (Phase::Day, dead) => format!("{} is voted out.", names(dead)),
        };
        self.alive = alive_after.clone();
        vec![line]
    }

    /// What `player` did by choosing `chosen`, which depends on the phase
    /// and, by night, on the player's role.
    fn deed(&self, player: &PlayerId, chosen: &PlayerId) -> String {
        let role = self.roles.get(player).copied();
        match (self.phase.as_ref().map(|(_, phase)| phase), role) {
            (Some(Phase::Night), Some(Role::Werewolf)) => {
                format!("{player} (werewolf) votes to kill {chosen}.")
            }
            (Some(Phase::Night), Some(Role::Doctor)) => {
                format!("{player} (doctor) protects {chosen}.")
            }
            (Some(Phase::Night), Some(Role::Seer)) => match self.roles.get(chosen) {
                Some(seen) => format!("{player} (seer) learns {chosen} is a {seen}."),
                None => format!("{player} (seer) asks about {chosen}."),
            },
            _ => format!("{player} votes against {chosen}."),
        }
    }
}

/// Players as a list in prose.
fn names(players: &[&PlayerId]) -> String {
    let names: Vec<&str> = players.iter().map(|name| name.as_str()).collect();
    names.join(" and ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::werewolf::Team;

    fn id(name: &str) -> PlayerId {
        name.to_string()
    }

    fn roles() -> HashMap<PlayerId, Role> {
        HashMap::from([
            (id("wolf"), Role::Werewolf),
            (id("seer"), Role::Seer),
            (id("doctor"), Role::Doctor),
            (id("ann"), Role::Villager),
        ])
    }

    fn seen(round: u8, phase: Phase, alive: &[&str]) -> Observation {
        Observation {
            round: NonZero::new(round).unwrap(),
            phase,
            roles: HashMap::new(),
            alive: alive.iter().map(|name| id(name)).collect(),
        }
    }

    fn observation(round: u8, phase: Phase, alive: &[&str]) -> Message {
        Message::Observation(seen(round, phase, alive))
    }

    fn shown(to: &str, round: u8, phase: Phase, alive: &[&str]) -> Entry {
        Entry::Sent {
            to: id(to),
            message: observation(round, phase, alive),
        }
    }

    fn chose(from: &str, chosen: &str) -> Entry {
        Entry::Replied {
            from: id(from),
            message: Message::Action(id(chosen)),
        }
    }

    /// The environment announced the phase to `to`.
    fn announced(to: &str, seq: u64, round: u8, phase: Phase, alive: &[&str]) -> Entry {
        Entry::Sent {
            to: id(to),
            message: Message::Announce {
                seq,
                observation: seen(round, phase, alive),
            },
        }
    }

    /// `from` selected `target` in the phase numbered `seq`.
    fn selected(from: &str, seq: u64, target: &str) -> Entry {
        Entry::Received {
            from: id(from),
            message: Message::Select {
                seq,
                from: id(from),
                target: id(target),
            },
        }
    }

    fn ended(winner: Team, days: u8, survivors: &[&str]) -> Entry {
        Entry::End {
            winner,
            days,
            survivors: survivors.iter().map(|name| id(name)).collect(),
        }
    }

    fn start() -> Entry {
        Entry::Start {
            variation: "Uniform Random".to_string(),
            roles: roles(),
        }
    }

    #[test]
    fn the_table_is_told_first() {
        let mut narrator = Narrator::default();
        assert_eq!(
            narrator.narrate(&start()),
            [
                "Uniform Random with 4 players: ann (villager), doctor (doctor), seer (seer), wolf (werewolf)."
            ]
        );
    }

    #[test]
    fn a_phase_begins_when_a_player_is_first_shown_it() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Night, &everyone)),
            ["Night 1."]
        );
        assert!(
            narrator
                .narrate(&shown("seer", 1, Phase::Night, &everyone))
                .is_empty()
        );
    }

    #[test]
    fn by_night_each_role_does_its_own_thing() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown(
            "wolf",
            1,
            Phase::Night,
            &["wolf", "seer", "doctor", "ann"],
        ));
        assert_eq!(
            narrator.narrate(&chose("wolf", "ann")),
            ["wolf (werewolf) votes to kill ann."]
        );
        assert_eq!(
            narrator.narrate(&chose("doctor", "seer")),
            ["doctor (doctor) protects seer."]
        );
        assert_eq!(
            narrator.narrate(&chose("seer", "wolf")),
            ["seer (seer) learns wolf is a werewolf."]
        );
    }

    #[test]
    fn a_nights_result_is_told_as_the_day_begins() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown(
            "wolf",
            1,
            Phase::Night,
            &["wolf", "seer", "doctor", "ann"],
        ));
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Day, &["wolf", "seer", "doctor"])),
            ["ann dies.", "Day 1."]
        );
    }

    #[test]
    fn a_night_in_which_nobody_dies_says_so() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        narrator.narrate(&shown("wolf", 1, Phase::Night, &everyone));
        assert_eq!(
            narrator.narrate(&shown("wolf", 1, Phase::Day, &everyone)),
            ["Nobody dies.", "Day 1."]
        );
    }

    #[test]
    fn by_day_everyone_votes() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown("wolf", 1, Phase::Day, &["wolf", "seer", "doctor"]));
        assert_eq!(
            narrator.narrate(&chose("wolf", "seer")),
            ["wolf votes against seer."]
        );
        assert_eq!(
            narrator.narrate(&chose("seer", "wolf")),
            ["seer votes against wolf."]
        );
    }

    #[test]
    fn a_days_result_is_told_as_the_night_begins() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown("wolf", 1, Phase::Day, &["wolf", "seer", "doctor"]));
        assert_eq!(
            narrator.narrate(&shown("wolf", 2, Phase::Night, &["wolf", "seer", "doctor"])),
            ["Nobody is voted out.", "Night 2."]
        );
        narrator.narrate(&shown("wolf", 2, Phase::Day, &["wolf", "seer", "doctor"]));
        assert_eq!(
            narrator.narrate(&shown("wolf", 3, Phase::Night, &["seer", "doctor"])),
            ["wolf is voted out.", "Night 3."]
        );
    }

    #[test]
    fn the_end_tells_the_last_result_then_the_winner() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown("wolf", 1, Phase::Day, &["wolf", "seer", "doctor"]));
        assert_eq!(
            narrator.narrate(&ended(Team::Villagers, 1, &["seer", "doctor"])),
            [
                "wolf is voted out.",
                "The villagers win after 1 day. Survivors: doctor (doctor), seer (seer)."
            ]
        );
    }

    #[test]
    fn an_announced_game_is_told_as_a_requested_one_is() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let everyone = ["wolf", "seer", "doctor", "ann"];
        assert_eq!(
            narrator.narrate(&announced("wolf", 1, 1, Phase::Night, &everyone)),
            ["Night 1."]
        );
        assert!(
            narrator
                .narrate(&announced("seer", 1, 1, Phase::Night, &everyone))
                .is_empty()
        );
        assert_eq!(
            narrator.narrate(&selected("wolf", 1, "ann")),
            ["wolf (werewolf) votes to kill ann."]
        );
        assert_eq!(
            narrator.narrate(&announced(
                "wolf",
                2,
                1,
                Phase::Day,
                &["wolf", "seer", "doctor"]
            )),
            ["ann dies.", "Day 1."]
        );
        assert_eq!(
            narrator.narrate(&selected("seer", 2, "wolf")),
            ["seer votes against wolf."]
        );
    }

    #[test]
    fn a_statement_that_is_not_a_selection_is_passed_over() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let received = Entry::Received {
            from: id("wolf"),
            message: Message::Action(id("ann")),
        };
        assert!(narrator.narrate(&received).is_empty());
    }

    #[test]
    fn a_message_going_the_other_way_is_passed_over() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        let backwards = Entry::Sent {
            to: id("wolf"),
            message: Message::Action(id("ann")),
        };
        assert!(narrator.narrate(&backwards).is_empty());
        let backwards = Entry::Replied {
            from: id("wolf"),
            message: observation(1, Phase::Night, &["wolf"]),
        };
        assert!(narrator.narrate(&backwards).is_empty());
    }

    #[test]
    fn a_seer_asking_about_nobody_at_the_table_is_told_as_asking() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown(
            "seer",
            1,
            Phase::Night,
            &["wolf", "seer", "doctor", "ann"],
        ));
        assert_eq!(
            narrator.narrate(&chose("seer", "stranger")),
            ["seer (seer) asks about stranger."]
        );
    }

    #[test]
    fn a_survivor_the_deal_never_named_is_named_alone() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        assert_eq!(
            narrator.narrate(&ended(Team::Villagers, 1, &["stranger"])),
            ["The villagers win after 1 day. Survivors: stranger."]
        );
    }

    #[test]
    fn days_are_counted_in_the_plural() {
        let mut narrator = Narrator::default();
        narrator.narrate(&start());
        narrator.narrate(&shown(
            "wolf",
            1,
            Phase::Night,
            &["wolf", "seer", "doctor", "ann"],
        ));
        assert_eq!(
            narrator.narrate(&ended(Team::Werewolves, 0, &["wolf", "ann"])),
            [
                "doctor and seer dies.",
                "The werewolves win after 0 days. Survivors: ann (villager), wolf (werewolf)."
            ]
        );
    }
}
