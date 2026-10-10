//! A legible account of a game, told from its log as the log is written,
//! either of everything or of what one player saw.

use super::{Entry, Message, Observation, Phase, PlayerId, Role};
use std::collections::{HashMap, HashSet};
use std::num::NonZero;

/// Turns the log's entries, one at a time, into lines of prose. The
/// omniscient narrator tells the table, then each night and day with what
/// every player did and how it ended, then who won. A player's narrator
/// tells the same nights and days from what that player was shown, in the
/// second person, with no table. Both learn who died from who is shown as
/// alive next, so a phase's result is told as the next one begins. A phase
/// announced is told as one requested, and a selection as a reply.
#[derive(Debug, Default)]
pub struct Narrator {
    /// The player whose account this is, or nobody's for the omniscient
    /// account.
    me: Option<PlayerId>,
    /// Every role known so far: from the deal, and from what players are
    /// shown.
    roles: HashMap<PlayerId, Role>,
    /// Who was alive when the current phase began.
    alive: HashSet<PlayerId>,
    /// The phase being told, once one has begun.
    phase: Option<(NonZero<u8>, Phase)>,
}

impl Narrator {
    /// The account `me` would give of its own game. Feed it the entries
    /// about `me`: each observation `me` was sent, and each selection it
    /// made. Entries about anyone else add nothing. It opens with who `me`
    /// is, names the roles `me` knows and nobody else's, and tells `me`'s
    /// deeds as "You ...".
    pub fn for_player(me: PlayerId) -> Self {
        Self {
            me: Some(me),
            ..Self::default()
        }
    }

    /// The lines `entry` adds to the account, often none.
    pub fn narrate(&mut self, entry: &Entry) -> Vec<String> {
        match entry {
            Entry::Start { variation, roles } if self.me.is_none() => {
                self.roles = roles.clone();
                self.alive = roles.keys().cloned().collect();
                vec![self.table(variation)]
            }
            Entry::Sent {
                to,
                message: Message::Observation(observation) | Message::Announce { observation, .. },
            } if self.concerns(to) => self.shown(observation),
            Entry::Replied {
                from,
                message: Message::Action(chosen),
            }
            | Entry::Received {
                from,
                message: Message::Select { target: chosen, .. },
            } if self.concerns(from) => vec![self.deed(from, chosen)],
            Entry::End {
                winner,
                days,
                survivors,
            } => {
                let mut lines = self.outcome(survivors, None);
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
            _ => vec![],
        }
    }

    /// Whether an entry about `player` belongs in this account: every
    /// player's does in the omniscient one, only `me`'s in a player's.
    fn concerns(&self, player: &PlayerId) -> bool {
        self.me.as_ref().is_none_or(|me| me == player)
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

    /// What a player being shown `observation` adds: the roles it shows,
    /// told as who the player is at its first observation and as what it
    /// learns after that, and then the phase it begins.
    fn shown(&mut self, observation: &Observation) -> Vec<String> {
        let mut lines = self.learn(&observation.roles);
        lines.extend(self.begin(observation));
        lines
    }

    /// Takes in the roles an observation shows. In a player's account, its
    /// first observation says who it is, and a role it is shown later is
    /// one it learned.
    fn learn(&mut self, roles: &HashMap<PlayerId, Role>) -> Vec<String> {
        let mut lines = vec![];
        if let Some(me) = &self.me {
            if self.phase.is_none() {
                lines.push(identity(me, roles));
            } else {
                let mut learned: Vec<(&PlayerId, &Role)> = roles
                    .iter()
                    .filter(|(player, _)| !self.roles.contains_key(*player))
                    .collect();
                learned.sort_by(|a, b| a.0.cmp(b.0));
                lines.extend(
                    learned
                        .iter()
                        .map(|(player, role)| format!("You learn {player} is a {role}.")),
                );
            }
        }
        self.roles
            .extend(roles.iter().map(|(player, role)| (player.clone(), *role)));
        lines
    }

    /// A phase begins when a player is first shown it. The phase before
    /// it ends then, with whoever is no longer alive.
    fn begin(&mut self, observation: &Observation) -> Vec<String> {
        let phase = (observation.round, observation.phase.clone());
        if self.phase.as_ref() == Some(&phase) {
            return vec![];
        }
        let mut lines = self.outcome(&observation.alive, Some(&phase));
        self.alive = observation.alive.clone();
        self.phase = Some(phase);
        lines.push(match observation.phase {
            Phase::Night => format!("Night {}.", observation.round),
            Phase::Day => format!("Day {}.", observation.round),
        });
        lines
    }

    /// How the phase being told ended, given who is alive after it. When
    /// `next` is not the phase right after it, the player slept through
    /// phases between, and the deaths are told without saying how.
    fn outcome(
        &mut self,
        alive_after: &HashSet<PlayerId>,
        next: Option<&(NonZero<u8>, Phase)>,
    ) -> Vec<String> {
        let Some(told) = &self.phase else {
            return vec![];
        };
        let mut dead: Vec<&PlayerId> = self.alive.difference(alive_after).collect();
        dead.sort();
        let slept = next.is_some_and(|next| ordinal(next) != ordinal(told) + 1);
        let line = match (&told.1, slept, dead.as_slice()) {
            (_, true, []) => "Nobody died.".to_string(),
            (_, true, dead) => format!("{} died.", names(dead)),
            (Phase::Night, false, []) => "Nobody dies.".to_string(),
            (Phase::Night, false, dead) => format!("{} dies.", names(dead)),
            (Phase::Day, false, []) => "Nobody is voted out.".to_string(),
            (Phase::Day, false, dead) => format!("{} is voted out.", names(dead)),
        };
        self.alive = alive_after.clone();
        vec![line]
    }

    /// What `player` did by choosing `chosen`, which depends on the phase
    /// and, by night, on the player's role. In a player's account it is
    /// always the player's own deed, told to it.
    fn deed(&self, player: &PlayerId, chosen: &PlayerId) -> String {
        let role = self.roles.get(player).copied();
        let phase = self.phase.as_ref().map(|(_, phase)| phase);
        if self.me.is_some() {
            return match (phase, role) {
                (Some(Phase::Night), Some(Role::Werewolf)) => format!("You vote to kill {chosen}."),
                (Some(Phase::Night), Some(Role::Doctor)) => format!("You protect {chosen}."),
                (Some(Phase::Night), Some(Role::Seer)) => format!("You ask about {chosen}."),
                _ => format!("You vote against {chosen}."),
            };
        }
        match (phase, role) {
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

/// Who `me` is, from the roles its first observation shows: its own, and
/// for a werewolf the whole pack.
fn identity(me: &PlayerId, roles: &HashMap<PlayerId, Role>) -> String {
    match roles.get(me) {
        Some(Role::Werewolf) => {
            let mut pack: Vec<&PlayerId> = roles
                .iter()
                .filter(|(_, role)| **role == Role::Werewolf)
                .map(|(player, _)| player)
                .collect();
            pack.sort();
            if pack.len() == 1 {
                format!("You are {me}, the only werewolf.")
            } else {
                format!(
                    "You are {me}, a werewolf. The werewolves are {}.",
                    names(&pack)
                )
            }
        }
        Some(Role::Villager) => format!("You are {me}, a villager."),
        Some(role) => format!("You are {me}, the {role}."),
        None => format!("You are {me}."),
    }
}

/// Where a phase falls in the game, so that consecutive phases are one
/// apart.
fn ordinal((round, phase): &(NonZero<u8>, Phase)) -> u16 {
    let half = match phase {
        Phase::Night => 0,
        Phase::Day => 1,
    };
    u16::from(round.get()) * 2 + half
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

    /// `to` was shown round `round`'s `phase` with `alive` alive and `known`
    /// roles, as a player of a requesting environment is.
    fn told(to: &str, round: u8, phase: Phase, alive: &[&str], known: &[(&str, Role)]) -> Entry {
        let mut observation = seen(round, phase, alive);
        observation.roles = known.iter().map(|(name, role)| (id(name), *role)).collect();
        Entry::Sent {
            to: id(to),
            message: Message::Observation(observation),
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

    #[test]
    fn a_player_is_shown_no_table() {
        let mut narrator = Narrator::for_player(id("wolf"));
        assert!(narrator.narrate(&start()).is_empty());
    }

    #[test]
    fn a_werewolf_opens_with_its_pack_and_kills_in_the_second_person() {
        let mut narrator = Narrator::for_player(id("wolf"));
        let everyone = ["wolf", "bob", "seer", "doctor", "ann"];
        let pack = [("wolf", Role::Werewolf), ("bob", Role::Werewolf)];
        assert_eq!(
            narrator.narrate(&told("wolf", 1, Phase::Night, &everyone, &pack)),
            [
                "You are wolf, a werewolf. The werewolves are bob and wolf.",
                "Night 1."
            ]
        );
        assert_eq!(
            narrator.narrate(&chose("wolf", "ann")),
            ["You vote to kill ann."]
        );
        assert_eq!(
            narrator.narrate(&told(
                "wolf",
                1,
                Phase::Day,
                &["wolf", "bob", "seer", "doctor"],
                &pack
            )),
            ["ann dies.", "Day 1."]
        );
        assert_eq!(
            narrator.narrate(&chose("wolf", "seer")),
            ["You vote against seer."]
        );
    }

    #[test]
    fn a_werewolf_without_a_pack_is_told_it_is_alone() {
        let mut narrator = Narrator::for_player(id("wolf"));
        assert_eq!(
            narrator.narrate(&told(
                "wolf",
                1,
                Phase::Night,
                &["wolf", "ann"],
                &[("wolf", Role::Werewolf)]
            )),
            ["You are wolf, the only werewolf.", "Night 1."]
        );
    }

    #[test]
    fn the_seer_asks_by_night_and_learns_by_morning() {
        let mut narrator = Narrator::for_player(id("seer"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        assert_eq!(
            narrator.narrate(&told(
                "seer",
                1,
                Phase::Night,
                &everyone,
                &[("seer", Role::Seer)]
            )),
            ["You are seer, the seer.", "Night 1."]
        );
        assert_eq!(
            narrator.narrate(&chose("seer", "wolf")),
            ["You ask about wolf."]
        );
        assert_eq!(
            narrator.narrate(&told(
                "seer",
                1,
                Phase::Day,
                &["wolf", "seer", "doctor"],
                &[("seer", Role::Seer), ("wolf", Role::Werewolf)]
            )),
            ["You learn wolf is a werewolf.", "ann dies.", "Day 1."]
        );
        assert!(
            narrator
                .narrate(&told(
                    "seer",
                    2,
                    Phase::Night,
                    &["wolf", "seer", "doctor"],
                    &[("seer", Role::Seer), ("wolf", Role::Werewolf)]
                ))
                .starts_with(&["Nobody is voted out.".to_string()])
        );
    }

    #[test]
    fn the_doctor_protects_by_night_and_votes_by_day() {
        let mut narrator = Narrator::for_player(id("doctor"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let known = [("doctor", Role::Doctor)];
        assert_eq!(
            narrator.narrate(&told("doctor", 1, Phase::Night, &everyone, &known)),
            ["You are doctor, the doctor.", "Night 1."]
        );
        assert_eq!(
            narrator.narrate(&chose("doctor", "seer")),
            ["You protect seer."]
        );
        assert_eq!(
            narrator.narrate(&told("doctor", 1, Phase::Day, &everyone, &known)),
            ["Nobody dies.", "Day 1."]
        );
        assert_eq!(
            narrator.narrate(&chose("doctor", "wolf")),
            ["You vote against wolf."]
        );
    }

    #[test]
    fn a_villager_sleeps_through_the_nights() {
        let mut narrator = Narrator::for_player(id("ann"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        assert_eq!(
            narrator.narrate(&told(
                "ann",
                1,
                Phase::Day,
                &everyone,
                &[("ann", Role::Villager)]
            )),
            ["You are ann, a villager.", "Day 1."]
        );
        assert_eq!(
            narrator.narrate(&chose("ann", "wolf")),
            ["You vote against wolf."]
        );
        assert_eq!(
            narrator.narrate(&told(
                "ann",
                2,
                Phase::Day,
                &["seer", "doctor", "ann"],
                &[("ann", Role::Villager)]
            )),
            ["wolf died.", "Day 2."]
        );
        assert_eq!(
            narrator.narrate(&told(
                "ann",
                3,
                Phase::Day,
                &["seer", "doctor", "ann"],
                &[("ann", Role::Villager)]
            )),
            ["Nobody died.", "Day 3."]
        );
    }

    #[test]
    fn entries_about_other_players_are_not_in_a_players_account() {
        let mut narrator = Narrator::for_player(id("wolf"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        assert!(
            narrator
                .narrate(&told(
                    "seer",
                    1,
                    Phase::Night,
                    &everyone,
                    &[("seer", Role::Seer)]
                ))
                .is_empty()
        );
        assert!(narrator.narrate(&chose("seer", "wolf")).is_empty());
        assert!(
            narrator
                .narrate(&announced("seer", 1, 1, Phase::Night, &everyone))
                .is_empty()
        );
        assert!(narrator.narrate(&selected("seer", 1, "wolf")).is_empty());
    }

    #[test]
    fn an_announced_game_is_told_to_its_player_as_a_requested_one_is() {
        let mut narrator = Narrator::for_player(id("wolf"));
        let everyone = ["wolf", "seer", "doctor", "ann"];
        let mut announcement = announced("wolf", 1, 1, Phase::Night, &everyone);
        if let Entry::Sent {
            message: Message::Announce { observation, .. },
            ..
        } = &mut announcement
        {
            observation.roles = HashMap::from([(id("wolf"), Role::Werewolf)]);
        }
        assert_eq!(
            narrator.narrate(&announcement),
            ["You are wolf, the only werewolf.", "Night 1."]
        );
        assert_eq!(
            narrator.narrate(&selected("wolf", 1, "ann")),
            ["You vote to kill ann."]
        );
    }

    #[test]
    fn a_player_names_the_survivors_it_knows_the_roles_of() {
        let mut narrator = Narrator::for_player(id("seer"));
        narrator.narrate(&told(
            "seer",
            1,
            Phase::Night,
            &["wolf", "seer", "doctor", "ann"],
            &[("seer", Role::Seer), ("wolf", Role::Werewolf)],
        ));
        assert_eq!(
            narrator.narrate(&ended(Team::Werewolves, 0, &["wolf", "seer", "doctor"])),
            [
                "ann dies.",
                "The werewolves win after 0 days. Survivors: doctor, seer (seer), wolf (werewolf)."
            ]
        );
    }
}
