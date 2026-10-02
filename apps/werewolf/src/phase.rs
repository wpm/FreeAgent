//! The rules as a state machine, one type per state.
//!
//! A round is a walk through four states, and each transition consumes the
//! state it came from:
//!
//! ```text
//!   Night --(resolve)--> Dawn --(announce)--> Day --(resolve)--> Dusk
//!     ^                                                            |
//!     +-------------------------- next round ----------------------+
//! ```
//!
//! Every transition returns [`Over`] instead when the village has a winner,
//! so a finished game cannot be stepped. Making each state its own type
//! means the compiler enforces the order: there is no method on [`Night`]
//! that counts votes, and none on [`Day`] that resolves a kill.
//!
//! The states hold only the rules. Asking players what they want and waiting
//! for their answers is the environment's job, which keeps this module
//! testable without running an episode.

use crate::game::{Outcome, Village, leaders, plurality};
use crate::role::Role;
use free_agent::actor::ActorId;
use rand::prelude::*;
use std::collections::HashMap;

/// What each player chose, by player.
pub type Choices = HashMap<ActorId, ActorId>;

/// The village is asleep and the night roles are choosing.
#[derive(Debug, Clone)]
pub struct Night {
    village: Village,
    round: u32,
}

/// The night's choices have been resolved, and someone may have died.
#[derive(Debug, Clone)]
pub struct Dawn {
    village: Village,
    round: u32,
    /// Who died, or nobody if the doctor got there first or the wolves
    /// could not agree.
    died: Option<ActorId>,
}

/// The village is awake and voting.
#[derive(Debug, Clone)]
pub struct Day {
    village: Village,
    round: u32,
}

/// The vote has been counted, and someone may have been lynched.
#[derive(Debug, Clone)]
pub struct Dusk {
    village: Village,
    round: u32,
    /// Who was lynched, or nobody if the village could not agree.
    lynched: Option<ActorId>,
}

/// The game is over.
#[derive(Debug, Clone)]
pub struct Over {
    village: Village,
    round: u32,
    outcome: Outcome,
}

/// Either the game goes on in state `S`, or it is [`Over`].
///
/// Every transition returns one of these, so no caller can step a finished
/// game: the next state is only reachable by matching on `Going`.
#[derive(Debug, Clone)]
pub enum Step<S> {
    /// Play continues.
    Going(S),
    /// Somebody won.
    Over(Over),
}

impl<S> Step<S> {
    /// The state play continues in, if it does.
    pub fn going(self) -> Option<S> {
        match self {
            Self::Going(state) => Some(state),
            Self::Over(_) => None,
        }
    }

    /// The finish, if the game reached one.
    pub fn over(self) -> Option<Over> {
        match self {
            Self::Going(_) => None,
            Self::Over(over) => Some(over),
        }
    }
}

/// Wraps `village` in the right [`Step`]: over if it has a winner.
fn step<S>(village: Village, round: u32, going: impl FnOnce(Village) -> S) -> Step<S> {
    match village.outcome() {
        Some(outcome) => Step::Over(Over {
            village,
            round,
            outcome,
        }),
        None => Step::Going(going(village)),
    }
}

impl Night {
    /// The first night of a new game.
    pub fn opens(village: Village) -> Self {
        Self { village, round: 1 }
    }

    /// Who is playing.
    pub fn village(&self) -> &Village {
        &self.village
    }

    /// Which round this is.
    pub fn round(&self) -> u32 {
        self.round
    }

    /// Applies the night's choices.
    ///
    /// The wolves' plurality names the victim, and a tie among them is
    /// broken at random rather than sparing everyone — otherwise wolves who
    /// never agree could keep a game alive indefinitely. The doctor's choice
    /// cancels the kill if the two agree. A seer's reading changes nothing
    /// here: it is knowledge, delivered to the seer alone.
    pub fn resolve(self, choices: &Choices, rng: &mut impl Rng) -> Step<Dawn> {
        let chosen = |role: Role| -> Vec<&ActorId> {
            self.village
                .living_with(role)
                .iter()
                .filter_map(|player| choices.get(&player.id))
                .collect::<Vec<_>>()
                .into_iter()
                .collect()
        };

        // The wolves settle a tie by biting one of the tied at random. A
        // night that killed nobody whenever they disagreed would let them
        // stall the game forever, so their disagreement costs them the
        // choice rather than the kill.
        let tied = leaders(chosen(Role::Werewolf).into_iter().map(|id| id as &ActorId));
        let victim = tied.choose(rng).cloned();
        let saved: Vec<&ActorId> = chosen(Role::Doctor);
        let died = victim.filter(|victim| !saved.contains(&victim));

        let mut village = self.village;
        if let Some(dead) = &died {
            village.kill(dead);
        }
        let round = self.round;
        step(village, round, |village| Dawn {
            village,
            round,
            died,
        })
    }

    /// What `actor` would be told it may do tonight.
    pub fn choices_for(&self, actor: &ActorId) -> Vec<ActorId> {
        let living = self.village.living();
        match self.village.role_of(actor) {
            Some(role) => role.legal_targets(actor, &living).cloned().collect(),
            None => Vec::new(),
        }
    }
}

impl Dawn {
    /// Who is playing.
    pub fn village(&self) -> &Village {
        &self.village
    }

    /// Which round this is.
    pub fn round(&self) -> u32 {
        self.round
    }

    /// Who died in the night, if anyone did.
    pub fn died(&self) -> Option<&ActorId> {
        self.died.as_ref()
    }

    /// Opens the day. The night's death is already applied.
    pub fn announce(self) -> Step<Day> {
        let round = self.round;
        step(self.village, round, |village| Day { village, round })
    }
}

impl Day {
    /// Who is playing.
    pub fn village(&self) -> &Village {
        &self.village
    }

    /// Which round this is.
    pub fn round(&self) -> u32 {
        self.round
    }

    /// Everyone may vote for anyone living, themselves included.
    pub fn choices_for(&self, _actor: &ActorId) -> Vec<ActorId> {
        self.village.living().iter().map(|p| p.id.clone()).collect()
    }

    /// Counts the vote. A tie lynches nobody.
    pub fn resolve(self, votes: &Choices) -> Step<Dusk> {
        let lynched = plurality(votes.values());
        let mut village = self.village;
        if let Some(dead) = &lynched {
            village.kill(dead);
        }
        let round = self.round;
        step(village, round, |village| Dusk {
            village,
            round,
            lynched,
        })
    }
}

impl Dusk {
    /// Who is playing.
    pub fn village(&self) -> &Village {
        &self.village
    }

    /// Which round this is.
    pub fn round(&self) -> u32 {
        self.round
    }

    /// Who the village lynched, if it could agree.
    pub fn lynched(&self) -> Option<&ActorId> {
        self.lynched.as_ref()
    }

    /// Begins the next round.
    ///
    /// However many there have been. The rules have no way to call a game
    /// off: one in which nobody dies plays on until whoever is running it
    /// stops it.
    pub fn nightfall(self) -> Step<Night> {
        let round = self.round + 1;
        step(self.village, round, |village| Night { village, round })
    }
}

impl Over {
    /// Who is playing, and who survived.
    pub fn village(&self) -> &Village {
        &self.village
    }

    /// Which round the game ended in.
    pub fn round(&self) -> u32 {
        self.round
    }

    /// Who won.
    pub fn outcome(&self) -> Outcome {
        self.outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::village;
    use crate::game::Player;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    fn rng() -> StdRng {
        StdRng::seed_from_u64(0)
    }

    /// `village` at its first day, having passed a night in which nobody
    /// acted. For tests about the vote, which do not care how the night
    /// went.
    fn day_of(village: Village) -> Day {
        Night::opens(village)
            .resolve(&Choices::new(), &mut rng())
            .going()
            .expect("a quiet night decides nothing")
            .announce()
            .going()
            .expect("a quiet night decides nothing")
    }

    /// The standard village at its first day.
    fn first_day() -> Day {
        day_of(village())
    }

    fn choices(pairs: &[(&str, &str)]) -> Choices {
        pairs
            .iter()
            .map(|(who, whom)| (who.to_string(), whom.to_string()))
            .collect()
    }

    #[test]
    fn a_game_opens_at_night_in_round_one() {
        let night = Night::opens(village());
        assert_eq!(night.round(), 1);
        assert_eq!(night.village().living().len(), 6);
    }

    #[test]
    fn the_wolves_agreeing_kills_their_victim() {
        let night = Night::opens(village());
        let dawn = night
            .resolve(
                &choices(&[("Wolf1", "Villager1"), ("Wolf2", "Villager1")]),
                &mut rng(),
            )
            .going()
            .expect("game continues");
        assert_eq!(dawn.died(), Some(&"Villager1".to_string()));
        assert!(!dawn.village().is_alive(&"Villager1".to_string()));
    }

    /// Disagreeing wolves still kill: one of the two they named, chosen at
    /// random. Sparing everyone would let wolves who never agree keep a
    /// game alive forever.
    #[test]
    fn the_wolves_disagreeing_still_kills_one_of_them() {
        let night = Night::opens(village());
        let dawn = night
            .resolve(
                &choices(&[("Wolf1", "Villager1"), ("Wolf2", "Villager2")]),
                &mut rng(),
            )
            .going()
            .expect("game continues");
        let died = dawn.died().expect("somebody dies");
        assert!(died == "Villager1" || died == "Villager2", "{died}");
        assert_eq!(dawn.village().living().len(), 5);
    }

    /// Which of the tied is bitten follows the seed, so a game replays.
    #[test]
    fn a_broken_tie_follows_the_seed() {
        let bitten = |seed| {
            Night::opens(village())
                .resolve(
                    &choices(&[("Wolf1", "Villager1"), ("Wolf2", "Villager2")]),
                    &mut StdRng::seed_from_u64(seed),
                )
                .going()
                .unwrap()
                .died()
                .cloned()
        };
        assert_eq!(bitten(7), bitten(7));
    }

    /// A night with no wolf choices at all still kills nobody.
    #[test]
    fn wolves_that_do_not_act_kill_nobody() {
        let dawn = Night::opens(village())
            .resolve(&Choices::new(), &mut rng())
            .going()
            .expect("game continues");
        assert_eq!(dawn.died(), None);
        assert_eq!(dawn.village().living().len(), 6);
    }

    #[test]
    fn the_doctor_saving_the_victim_cancels_the_kill() {
        let night = Night::opens(village());
        let dawn = night
            .resolve(
                &choices(&[
                    ("Wolf1", "Villager1"),
                    ("Wolf2", "Villager1"),
                    ("Doc", "Villager1"),
                ]),
                &mut rng(),
            )
            .going()
            .expect("game continues");
        assert_eq!(dawn.died(), None);
        assert!(dawn.village().is_alive(&"Villager1".to_string()));
    }

    #[test]
    fn the_doctor_saving_someone_else_does_not_help() {
        let night = Night::opens(village());
        let dawn = night
            .resolve(
                &choices(&[
                    ("Wolf1", "Villager1"),
                    ("Wolf2", "Villager1"),
                    ("Doc", "Seer"),
                ]),
                &mut rng(),
            )
            .going()
            .expect("game continues");
        assert_eq!(dawn.died(), Some(&"Villager1".to_string()));
    }

    /// A seer's reading is knowledge, not an effect on the village.
    #[test]
    fn a_seer_reading_changes_nothing() {
        let night = Night::opens(village());
        let dawn = night
            .resolve(&choices(&[("Seer", "Wolf1")]), &mut rng())
            .going()
            .expect("game continues");
        assert_eq!(dawn.died(), None);
        assert_eq!(dawn.village().living().len(), 6);
    }

    #[test]
    fn the_day_lynches_whoever_got_the_most_votes() {
        let dusk = first_day()
            .resolve(&choices(&[
                ("Wolf1", "Villager1"),
                ("Wolf2", "Villager1"),
                ("Doc", "Wolf1"),
            ]))
            .going()
            .expect("game continues");
        assert_eq!(dusk.lynched(), Some(&"Villager1".to_string()));
    }

    #[test]
    fn a_tied_vote_lynches_nobody() {
        let dusk = first_day()
            .resolve(&choices(&[("Wolf1", "Villager1"), ("Doc", "Wolf1")]))
            .going()
            .expect("game continues");
        assert_eq!(dusk.lynched(), None);
    }

    #[test]
    fn nightfall_begins_the_next_round() {
        let dusk = first_day()
            .resolve(&Choices::new())
            .going()
            .expect("a quiet day decides nothing");
        let night = dusk.nightfall().going().expect("game continues");
        assert_eq!(night.round(), 2);
    }

    /// Lynching the last werewolf ends the game rather than opening a dusk.
    #[test]
    fn a_game_ends_the_moment_it_is_decided() {
        let village = Village::new([
            Player::new("Wolf1", Role::Werewolf),
            Player::new("Doc", Role::Doctor),
            Player::new("Seer", Role::Seer),
        ]);
        let over = day_of(village)
            .resolve(&choices(&[("Doc", "Wolf1"), ("Seer", "Wolf1")]))
            .over()
            .expect("game is decided");
        assert_eq!(over.outcome(), Outcome::Villagers);
    }

    #[test]
    fn the_wolves_win_by_reaching_parity() {
        let village = Village::new([
            Player::new("Wolf1", Role::Werewolf),
            Player::new("Villager1", Role::Villager),
            Player::new("Villager2", Role::Villager),
        ]);
        let over = Night::opens(village)
            .resolve(&choices(&[("Wolf1", "Villager1")]), &mut rng())
            .over()
            .expect("game is decided");
        assert_eq!(over.outcome(), Outcome::Werewolves);
        assert_eq!(over.round(), 1);
    }

    /// The rules never call a game off. A village where nobody dies plays
    /// on for as long as it is stepped, and stopping it is left to whoever
    /// is running the episode.
    #[test]
    fn a_game_nobody_ends_is_never_called_off() {
        let mut night = Night::opens(village());
        for _ in 0..500 {
            // Nobody acts, so nobody dies, round after round.
            night = night
                .resolve(&Choices::new(), &mut rng())
                .going()
                .and_then(|dawn| dawn.announce().going())
                .and_then(|day| day.resolve(&Choices::new()).going())
                .and_then(|dusk| dusk.nightfall().going())
                .expect("nothing happened that could decide the game");
        }
        assert_eq!(night.round(), 501);
        assert_eq!(night.village().living().len(), 6);
    }

    #[test]
    fn a_night_offers_each_role_its_own_choices() {
        let night = Night::opens(village());
        let wolf = night.choices_for(&"Wolf1".to_string());
        assert!(!wolf.contains(&"Wolf2".to_string()), "{wolf:?}");

        let seer = night.choices_for(&"Seer".to_string());
        assert!(!seer.contains(&"Seer".to_string()), "{seer:?}");

        let doc = night.choices_for(&"Doc".to_string());
        assert!(doc.contains(&"Doc".to_string()), "{doc:?}");
    }
}
