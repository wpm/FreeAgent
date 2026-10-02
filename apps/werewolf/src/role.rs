//! What the players are, and what each kind may do.
//!
//! Every role acts the same way — it names one living player — so a role is
//! not distinguished by the shape of its action but by when it acts, what it
//! may name, and what the naming means. [`Role`] carries that: [`acts_in`]
//! says which phases wake it, and [`legal_targets`] says who it may name.
//!
//! [`acts_in`]: Role::acts_in
//! [`legal_targets`]: Role::legal_targets

use crate::game::{Phase, Player};
use free_agent::actor::ActorId;
use serde::{Deserialize, Serialize};

/// What a player is.
///
/// Roles are closed rather than a trait, because the environment must reason
/// about all of them at once — who wakes tonight, who wins — and a fixed set
/// lets the compiler check that every rule covers every role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    /// Kills by night, and knows the other werewolves.
    Werewolf,
    /// Saves one player from the night's kill. Knows nobody.
    Doctor,
    /// Learns one player's team each night. Knows nobody at the start.
    Seer,
    /// Votes by day and nothing more.
    Villager,
}

/// Which side a player is on, which is what the [`Seer`](Role::Seer) learns
/// and what decides the game.
///
/// Named for the sides rather than for virtue: a seer learns that someone
/// hunts with the wolves, not that they are wicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Team {
    /// The werewolves, who hunt at night.
    Werewolves,
    /// Everyone else, whatever else they can do.
    Villagers,
}

/// Everything that distinguishes one role from another.
///
/// Each role is one row of this, rather than a case in each of several
/// methods, so what a werewolf *is* can be read in one place. Adding a role
/// is adding a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Behavior {
    /// Which side this role wins with.
    pub team: Team,
    /// What it does when woken at night, or `None` if it sleeps through.
    pub night_action: Option<NightAction>,
    /// Whether it may name itself.
    pub may_name_self: bool,
    /// Whether it may name someone on its own side.
    pub may_name_own_team: bool,
}

impl Role {
    /// What this role is, in one piece.
    ///
    /// The one place a role is described. Every other method here reads a
    /// field of it, so the rules cannot drift apart between them.
    pub fn behavior(self) -> Behavior {
        match self {
            // Wolves hunt the village, never each other.
            Self::Werewolf => Behavior {
                team: Team::Werewolves,
                night_action: Some(NightAction::Kill),
                may_name_self: false,
                may_name_own_team: false,
            },
            // A doctor may save anyone, itself included.
            Self::Doctor => Behavior {
                team: Team::Villagers,
                night_action: Some(NightAction::Save),
                may_name_self: true,
                may_name_own_team: true,
            },
            // Reading itself would tell a seer nothing it does not know.
            Self::Seer => Behavior {
                team: Team::Villagers,
                night_action: Some(NightAction::Inspect),
                may_name_self: false,
                may_name_own_team: true,
            },
            // A villager sleeps, and by day may vote for anyone.
            Self::Villager => Behavior {
                team: Team::Villagers,
                night_action: None,
                may_name_self: true,
                may_name_own_team: true,
            },
        }
    }

    /// Which side this role wins with.
    pub fn team(self) -> Team {
        self.behavior().team
    }

    /// Whether this role is woken to act during `phase`.
    ///
    /// Everyone votes by day. By night only the roles with something to do
    /// are woken; a plain villager sleeps through it.
    pub fn acts_in(self, phase: Phase) -> bool {
        match phase {
            Phase::Day => true,
            Phase::Night => self.behavior().night_action.is_some(),
        }
    }

    /// What naming someone means at night, for the rules and the log.
    pub fn night_action(self) -> Option<NightAction> {
        self.behavior().night_action
    }

    /// Who this player may name, given who is alive.
    ///
    /// The rules of sense as well as legality, read off the role's
    /// [`Behavior`]: nobody may name a corpse, a werewolf does not eat its
    /// own, a seer does not read itself. What a role may not repeat across
    /// nights — the seer's past readings — is the player's own memory, not
    /// something the rules can see, so the player applies it.
    pub fn legal_targets<'a>(
        self,
        actor: &'a ActorId,
        living: &'a [Player],
    ) -> impl Iterator<Item = &'a ActorId> + 'a {
        let behavior = self.behavior();
        living
            .iter()
            .filter(move |player| {
                (behavior.may_name_self || &player.id != actor)
                    && (behavior.may_name_own_team || player.role.team() != behavior.team)
            })
            .map(|player| &player.id)
    }
}

/// What a role does when it names someone at night.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NightAction {
    /// The werewolves' victim.
    Kill,
    /// The doctor's patient, who survives a kill this night.
    Save,
    /// The seer's reading, which returns a [`Team`].
    Inspect,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn living() -> Vec<Player> {
        vec![
            Player::new("Wolf", Role::Werewolf),
            Player::new("Doc", Role::Doctor),
            Player::new("Seer", Role::Seer),
            Player::new("Villager", Role::Villager),
        ]
    }

    fn targets(role: Role, actor: &str) -> Vec<String> {
        let living = living();
        let actor = actor.to_string();
        let mut names: Vec<String> = role
            .legal_targets(&actor, &living)
            .map(|id| id.to_string())
            .collect();
        names.sort();
        names
    }

    /// Every role is described in one row, and the methods only read it.
    #[test]
    fn the_methods_agree_with_the_table() {
        for role in [Role::Werewolf, Role::Doctor, Role::Seer, Role::Villager] {
            let behavior = role.behavior();
            assert_eq!(role.team(), behavior.team, "{role:?}");
            assert_eq!(role.night_action(), behavior.night_action, "{role:?}");
            assert_eq!(
                role.acts_in(Phase::Night),
                behavior.night_action.is_some(),
                "{role:?}"
            );
        }
    }

    /// A role that acts at night has something to do there, and one that
    /// does not, sleeps. The two cannot disagree.
    #[test]
    fn waking_at_night_means_having_a_night_action() {
        for role in [Role::Werewolf, Role::Doctor, Role::Seer, Role::Villager] {
            assert_eq!(
                role.acts_in(Phase::Night),
                role.night_action().is_some(),
                "{role:?}"
            );
        }
    }

    #[test]
    fn only_werewolves_are_evil() {
        assert_eq!(Role::Werewolf.team(), Team::Werewolves);
        for role in [Role::Doctor, Role::Seer, Role::Villager] {
            assert_eq!(role.team(), Team::Villagers);
        }
    }

    #[test]
    fn everyone_acts_by_day() {
        for role in [Role::Werewolf, Role::Doctor, Role::Seer, Role::Villager] {
            assert!(role.acts_in(Phase::Day));
        }
    }

    #[test]
    fn a_plain_villager_sleeps_through_the_night() {
        assert!(!Role::Villager.acts_in(Phase::Night));
        for role in [Role::Werewolf, Role::Doctor, Role::Seer] {
            assert!(role.acts_in(Phase::Night));
        }
    }

    #[test]
    fn a_werewolf_does_not_eat_its_own() {
        assert_eq!(targets(Role::Werewolf, "Wolf"), ["Doc", "Seer", "Villager"]);
    }

    #[test]
    fn a_doctor_may_save_itself() {
        assert!(targets(Role::Doctor, "Doc").contains(&"Doc".to_string()));
    }

    /// Reading itself would tell the seer nothing it does not know.
    #[test]
    fn a_seer_may_not_read_itself() {
        let read = targets(Role::Seer, "Seer");
        assert!(!read.contains(&"Seer".to_string()));
        assert_eq!(read, ["Doc", "Villager", "Wolf"]);
    }

    #[test]
    fn nobody_may_name_the_dead() {
        let living = vec![Player::new("Wolf", Role::Werewolf)];
        let actor = "Doc".to_string();
        let names: Vec<_> = Role::Doctor.legal_targets(&actor, &living).collect();
        assert_eq!(names, [&"Wolf".to_string()]);
    }

    #[test]
    fn each_night_role_has_an_action() {
        assert_eq!(Role::Werewolf.night_action(), Some(NightAction::Kill));
        assert_eq!(Role::Doctor.night_action(), Some(NightAction::Save));
        assert_eq!(Role::Seer.night_action(), Some(NightAction::Inspect));
        assert_eq!(Role::Villager.night_action(), None);
    }
}
