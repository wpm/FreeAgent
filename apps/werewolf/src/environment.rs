//! The environment: the actor that runs the game and enforces the rules.
//!
//! It owns the [state machine](crate::phase) and the only complete view of
//! the village. Each round it wakes whoever the phase calls for, asks them
//! all at once, waits out one deadline for the lot, and applies whatever
//! came back. A player that misses the deadline or names someone it was not
//! offered simply does not act — a round cannot be held up by one player.
//!
//! Everything it decides goes to the episode's log, which is the record a
//! model is trained on.

use crate::game::{Outcome, Phase, Village};
use crate::phase::{Choices, Dawn, Day, Dusk, Night, Over, Step};
use crate::protocol::{ActionSpace, Assignment, Knowledge, Message, Observation};
use crate::role::Role;
use anyhow::{Result, anyhow};
use free_agent::actor::{Actor, ActorId, async_trait};
use free_agent::episode::Channels;
use futures::future::join_all;
use rand::prelude::*;
use rand::rngs::StdRng;
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::{Instant, timeout_at};

/// One line of the training log.
///
/// Every entry says what happened and, where it matters, what the player
/// could have done instead — an action without its action space teaches
/// nothing.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The roster, before anything happens.
    Started {
        /// Every player and what they are. The log is for training, so it
        /// records the hidden truth a player never sees.
        roles: Vec<(ActorId, Role)>,
    },
    /// A phase opened.
    PhaseBegan {
        /// Which round.
        round: u32,
        /// Night or day.
        phase: Phase,
        /// Who is still alive.
        living: Vec<ActorId>,
    },
    /// A player acted.
    Acted {
        /// Which round.
        round: u32,
        /// Night or day.
        phase: Phase,
        /// Who acted.
        actor: ActorId,
        /// What they are, which a player does not see but a model needs.
        role: Role,
        /// Who they named.
        target: ActorId,
        /// Everyone they could have named.
        choices: Vec<ActorId>,
    },
    /// A player did not act, and why.
    Silent {
        /// Which round.
        round: u32,
        /// Night or day.
        phase: Phase,
        /// Who stayed silent.
        actor: ActorId,
        /// Whether they ran out of time or answered nonsense.
        reason: String,
    },
    /// What the night came to.
    NightEnded {
        /// Which round.
        round: u32,
        /// Who died, if anyone.
        died: Option<ActorId>,
    },
    /// What the day came to.
    DayEnded {
        /// Which round.
        round: u32,
        /// Who was lynched, if anyone.
        lynched: Option<ActorId>,
    },
    /// Something a player was told privately.
    Told {
        /// Which round.
        round: u32,
        /// Who was told.
        actor: ActorId,
        /// What they learned.
        knowledge: Knowledge,
    },
    /// The game finished.
    Finished {
        /// Which round it ended in.
        round: u32,
        /// Who won.
        outcome: Outcome,
        /// Who was left standing.
        survivors: Vec<ActorId>,
    },
}

/// Runs one game.
#[derive(Debug)]
pub struct Environment {
    village: Option<Village>,
    /// How long a whole round of asking may take.
    patience: Duration,
    /// Breaks the ties the rules leave to chance, seeded so a game replays.
    rng: StdRng,
}

impl Environment {
    /// An environment that will run `village`, giving every round
    /// `patience` to answer in.
    pub fn new(village: Village, patience: Duration) -> Self {
        Self::seeded(village, patience, 0)
    }

    /// The same, but breaking ties from `seed`.
    pub fn seeded(village: Village, patience: Duration, seed: u64) -> Self {
        Self {
            village: Some(village),
            patience,
            rng: StdRng::seed_from_u64(seed),
        }
    }

    /// Tells every player what they are, and a werewolf who its allies are.
    fn assign(&self, village: &Village, channels: &Channels<Message>) -> Result<()> {
        let everyone: Vec<ActorId> = village.everyone().iter().map(|p| p.id.clone()).collect();
        let wolves: Vec<ActorId> = village
            .everyone()
            .iter()
            .filter(|p| p.role == Role::Werewolf)
            .map(|p| p.id.clone())
            .collect();

        channels.log(&Event::Started {
            roles: village
                .everyone()
                .iter()
                .map(|p| (p.id.clone(), p.role))
                .collect(),
        })?;

        for player in village.everyone() {
            let allies = match player.role {
                Role::Werewolf => wolves
                    .iter()
                    .filter(|w| **w != player.id)
                    .cloned()
                    .collect(),
                _ => Vec::new(),
            };
            channels.send(
                Message::Assigned(Assignment {
                    role: player.role,
                    players: everyone.clone(),
                    allies,
                }),
                &player.id,
            )?;
        }
        Ok(())
    }

    /// Asks everyone woken by this phase what they want to do, and collects
    /// whatever arrives before the deadline.
    ///
    /// The requests are joined, so every question is in flight before any
    /// answer is waited on and a round costs the slowest player rather than
    /// the sum of them all.
    async fn poll(
        &self,
        village: &Village,
        round: u32,
        phase: Phase,
        choices_for: impl Fn(&ActorId) -> Vec<ActorId>,
        channels: &Channels<Message>,
    ) -> Result<Choices> {
        let living: Vec<ActorId> = village.living().iter().map(|p| p.id.clone()).collect();

        channels.log(&Event::PhaseBegan {
            round,
            phase,
            living: living.clone(),
        })?;

        let asked: Vec<_> = village
            .woken_by(phase)
            .into_iter()
            .map(|player| {
                let space = ActionSpace::naming(choices_for(&player.id));
                (player, space)
            })
            .filter(|(_, space)| !space.is_empty())
            .collect();

        // One deadline for the whole round, not one apiece.
        let deadline = Instant::now() + self.patience;
        let answers = join_all(asked.iter().map(|(player, space)| {
            let question = Message::Decide(
                Observation {
                    round,
                    phase,
                    living: living.clone(),
                },
                space.clone(),
            );
            async move {
                timeout_at(deadline, channels.request(question, &player.id))
                    .await
                    .unwrap_or_else(|_| Err(anyhow!("{} did not answer in time", player.id)))
            }
        }))
        .await;

        let mut chosen = Choices::new();
        for ((player, space), answer) in asked.into_iter().zip(answers) {
            let action = match answer {
                Ok(Message::Act(action)) => action,
                Ok(other) => {
                    channels.log(&Event::Silent {
                        round,
                        phase,
                        actor: player.id.clone(),
                        reason: format!("answered with {other:?} instead of an action"),
                    })?;
                    continue;
                }
                Err(e) => {
                    channels.log(&Event::Silent {
                        round,
                        phase,
                        actor: player.id.clone(),
                        reason: format!("{e:#}"),
                    })?;
                    continue;
                }
            };

            // An action must come from the space it was offered.
            if !space.allows(&action) {
                channels.log(&Event::Silent {
                    round,
                    phase,
                    actor: player.id.clone(),
                    reason: format!("named {}, which was not offered", action.target),
                })?;
                continue;
            }

            channels.log(&Event::Acted {
                round,
                phase,
                actor: player.id.clone(),
                role: player.role,
                target: action.target.clone(),
                choices: space.targets,
            })?;
            chosen.insert(player.id.clone(), action.target);
        }
        Ok(chosen)
    }

    /// Tells each seer what it read, and logs it.
    fn report_readings(
        &self,
        village: &Village,
        round: u32,
        choices: &Choices,
        channels: &Channels<Message>,
    ) -> Result<()> {
        for seer in village.living_with(Role::Seer) {
            let Some(read) = choices.get(&seer.id) else {
                continue;
            };
            let Some(role) = village.role_of(read) else {
                continue;
            };
            let knowledge = Knowledge::Reading {
                player: read.clone(),
                team: role.team(),
            };
            channels.log(&Event::Told {
                round,
                actor: seer.id.clone(),
                knowledge: knowledge.clone(),
            })?;
            channels.send(Message::Learned(knowledge), &seer.id)?;
        }
        Ok(())
    }

    /// Tells everyone still playing what happened.
    fn broadcast(
        &self,
        village: &Village,
        knowledge: Knowledge,
        channels: &Channels<Message>,
    ) -> Result<()> {
        for player in village.living() {
            channels.send(Message::Learned(knowledge.clone()), &player.id)?;
        }
        Ok(())
    }

    /// Plays one night, from the wolves' choice to the morning's news.
    async fn night(&mut self, night: Night, channels: &Channels<Message>) -> Result<Step<Day>> {
        let round = night.round();
        let choices = self
            .poll(
                night.village(),
                round,
                Phase::Night,
                |actor| night.choices_for(actor),
                channels,
            )
            .await?;
        self.report_readings(night.village(), round, &choices, channels)?;

        match night.resolve(&choices, &mut self.rng) {
            Step::Going(dawn) => {
                self.dawn_news(&dawn, channels)?;
                Ok(dawn.announce())
            }
            Step::Over(over) => {
                channels.log(&Event::NightEnded { round, died: None })?;
                Ok(Step::Over(over))
            }
        }
    }

    /// Announces the night's death.
    fn dawn_news(&self, dawn: &Dawn, channels: &Channels<Message>) -> Result<()> {
        channels.log(&Event::NightEnded {
            round: dawn.round(),
            died: dawn.died().cloned(),
        })?;
        self.broadcast(
            dawn.village(),
            Knowledge::Died(dawn.died().cloned()),
            channels,
        )
    }

    /// Plays one day, from the vote to the lynching.
    async fn day(&mut self, day: Day, channels: &Channels<Message>) -> Result<Step<Night>> {
        let round = day.round();
        let votes = self
            .poll(
                day.village(),
                round,
                Phase::Day,
                |actor| day.choices_for(actor),
                channels,
            )
            .await?;

        match day.resolve(&votes) {
            Step::Going(dusk) => {
                self.dusk_news(&dusk, channels)?;
                Ok(dusk.nightfall())
            }
            Step::Over(over) => {
                channels.log(&Event::DayEnded {
                    round,
                    lynched: None,
                })?;
                Ok(Step::Over(over))
            }
        }
    }

    /// Announces the day's lynching.
    fn dusk_news(&self, dusk: &Dusk, channels: &Channels<Message>) -> Result<()> {
        channels.log(&Event::DayEnded {
            round: dusk.round(),
            lynched: dusk.lynched().cloned(),
        })?;
        self.broadcast(
            dusk.village(),
            Knowledge::Lynched(dusk.lynched().cloned()),
            channels,
        )
    }

    /// Tells everyone the game is over and stops them.
    fn finish(&self, over: &Over, channels: &Channels<Message>) -> Result<()> {
        let survivors: Vec<ActorId> = over
            .village()
            .living()
            .iter()
            .map(|p| p.id.clone())
            .collect();
        channels.log(&Event::Finished {
            round: over.round(),
            outcome: over.outcome(),
            survivors,
        })?;
        for player in over.village().everyone() {
            let _ = channels.send(Message::Ended(over.outcome()), &player.id);
            channels.stop(&player.id)?;
        }
        Ok(())
    }
}

#[async_trait]
impl Actor<Message> for Environment {
    /// Runs the whole game, from assignment to outcome.
    ///
    /// This is a `perceive` override rather than an `act`, because the
    /// environment drives rather than reacts: it decides when to ask and
    /// what to ask for, and the players answer.
    async fn perceive(&mut self, channels: &mut Channels<Message>) -> Result<()> {
        let village = self
            .village
            .take()
            .expect("an environment runs one game, once");
        self.assign(&village, channels)?;

        let mut step = Step::Going(Night::opens(village));
        loop {
            let night = match step {
                Step::Going(night) => night,
                Step::Over(over) => return self.finish(&over, channels),
            };
            step = match self.night(night, channels).await? {
                Step::Going(day) => self.day(day, channels).await?,
                Step::Over(over) => Step::Over(over),
            };
        }
    }
}

/// The roster of a standard game, sized to `players`.
///
/// Roughly a quarter werewolves, one doctor and one seer, the rest plain
/// villagers — the usual shape, and enough for the wolves to have a chance
/// without it being hopeless.
pub fn standard_roles(players: usize) -> Vec<Role> {
    let wolves = (players / 4).max(1);
    let mut roles = vec![Role::Werewolf; wolves];
    if players > wolves {
        roles.push(Role::Doctor);
    }
    if players > roles.len() {
        roles.push(Role::Seer);
    }
    while roles.len() < players {
        roles.push(Role::Villager);
    }
    roles.truncate(players);
    roles
}

/// Counts the players of each role, for a quick look at a roster.
pub fn census(roles: &[Role]) -> HashMap<Role, usize> {
    let mut counts = HashMap::new();
    for role in roles {
        *counts.entry(*role).or_default() += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::game::{Player, Village};
    use crate::player::random;
    use crate::protocol::Message;
    use free_agent::episode::{self, Memory};
    use std::sync::Arc;

    /// A player that never answers anything.
    #[derive(Debug)]
    struct Mute;

    #[async_trait]
    impl Actor<Message> for Mute {
        async fn perceive(&mut self, channels: &mut Channels<Message>) -> Result<()> {
            channels.stop.cancelled().await;
            Ok(())
        }
    }

    /// A round cannot be held up by a player who will not answer: the
    /// deadline passes, the silence is logged, and the game goes on.
    #[tokio::test(start_paused = true)]
    async fn a_silent_player_does_not_stall_the_game() {
        let village = Village::new([
            Player::new("Wolf", Role::Werewolf),
            Player::new("Doc", Role::Doctor),
            Player::new("Seer", Role::Seer),
            Player::new("Villager", Role::Villager),
        ]);
        let log = Arc::new(Memory::new());
        let roster: Vec<(ActorId, Box<dyn Actor<Message>>)> = vec![
            (
                "Environment".to_string(),
                Box::new(Environment::new(village, Duration::from_millis(20))),
            ),
            ("Wolf".to_string(), Box::new(random(0))),
            ("Doc".to_string(), Box::new(Mute)),
            ("Seer".to_string(), Box::new(Mute)),
            ("Villager".to_string(), Box::new(Mute)),
        ];

        let began = Instant::now();
        let result = episode::episode(roster, log.clone(), Some(Duration::from_secs(20))).await;
        assert!(result.is_ok(), "{:?}", result);

        // Only the wolf ever answers, so every round waits out its deadline
        // for the others and then goes on with what it has. The wolf's
        // choices alone settle the game, so it ends on a rule of the game
        // rather than on the episode's backstop.
        let events: Vec<String> = log
            .entries()
            .iter()
            .map(|e| e.payload.to_string())
            .collect();
        assert!(
            events.iter().any(|e| e.contains("silent")),
            "expected silences, got {events:?}"
        );
        let finished = log
            .entries()
            .iter()
            .filter_map(|e| {
                e.payload
                    .get("event")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .any(|event| event == "finished");
        assert!(finished, "the game must reach an end of its own");
        // Well inside the episode's 20s backstop.
        assert!(began.elapsed() < Duration::from_secs(5));
    }

    /// Plays a whole game between random players, everything about it
    /// following from `seed`, and returns what the environment logged.
    async fn game(players: usize, seed: u64) -> Vec<String> {
        let mut roles = standard_roles(players);
        roles.shuffle(&mut StdRng::seed_from_u64(seed));
        let village = Village::numbered(roles);
        let names: Vec<ActorId> = village.everyone().iter().map(|p| p.id.clone()).collect();

        let mut roster: Vec<(ActorId, Box<dyn Actor<Message>>)> = vec![(
            "Environment".to_string(),
            Box::new(Environment::seeded(village, Duration::from_secs(5), seed)),
        )];
        for (n, name) in names.iter().enumerate() {
            roster.push((name.clone(), Box::new(random(seed ^ n as u64))));
        }

        let log = Arc::new(Memory::new());
        let result = episode::episode(roster, log.clone(), Some(Duration::from_secs(20))).await;
        assert!(result.is_ok(), "{:?}", result);
        log.entries()
            .iter()
            .map(|e| e.payload.to_string())
            .collect()
    }

    /// A seed is the whole of a game: played again from the same one, every
    /// choice and every death comes out the same. On one thread a player
    /// finds what it was told and what it was asked waiting together, so
    /// this fails if which of them it happens to notice first can change
    /// what it decides.
    #[tokio::test]
    async fn the_same_seed_replays_the_same_game() {
        for seed in 0..20 {
            let first = game(8, seed).await;
            assert!(
                first.iter().any(|event| event.contains("finished")),
                "seed {seed} did not play to an end: {first:?}"
            );
            assert_eq!(game(8, seed).await, first, "seed {seed}");
        }
    }

    #[test]
    fn a_standard_roster_has_wolves_a_doctor_and_a_seer() {
        let roles = standard_roles(8);
        let counts = census(&roles);
        assert_eq!(roles.len(), 8);
        assert_eq!(counts[&Role::Werewolf], 2);
        assert_eq!(counts[&Role::Doctor], 1);
        assert_eq!(counts[&Role::Seer], 1);
        assert_eq!(counts[&Role::Villager], 4);
    }

    /// Even the smallest game has somebody to fear.
    #[test]
    fn a_tiny_roster_still_has_a_werewolf() {
        assert_eq!(standard_roles(1), [Role::Werewolf]);
        assert_eq!(census(&standard_roles(3))[&Role::Werewolf], 1);
    }

    #[test]
    fn a_roster_is_exactly_as_big_as_asked_for() {
        for size in 1..20 {
            assert_eq!(standard_roles(size).len(), size, "roster of {size}");
        }
    }
}
