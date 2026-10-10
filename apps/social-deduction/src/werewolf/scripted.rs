//! The scripted variant: the announcing environment, played by players who
//! answer each announcement at once with a selection made at random. It
//! plays the model-played variant's game without a model.

use super::announced::{self, ENVIRONMENT, Environment};
use super::uniform_random::choose;
use super::{Entry, Message, PlayerId, Role, Rules, Team};
use async_trait::async_trait;
use free_agent::{ActorInit, Behavior, Builder, Context, Episode, Logger};
use std::collections::{HashMap, HashSet};
use tokio::sync::oneshot;

/// The rules of the scripted variant: the uniform-random variant's, played
/// as "Scripted".
pub fn rules() -> Rules {
    Rules {
        variation: "Scripted".to_string(),
        ..Rules::default()
    }
}

/// An episode of a game of `roles` under `rules`: the announcing
/// environment, which may reach and stop every player and holds `logger`,
/// and a scripted player for each role, who may send to the environment.
/// The winning team is sent on `winner` when the game ends.
pub fn game(
    roles: HashMap<PlayerId, Role>,
    rules: Rules,
    winner: oneshot::Sender<Team>,
    logger: Logger<Entry>,
) -> Episode<Actor> {
    episode(roles, rules, winner, logger, |_| Actor::player())
}

/// The episode of [`game`], with a player for each role built by `player`.
fn episode(
    roles: HashMap<PlayerId, Role>,
    rules: Rules,
    winner: oneshot::Sender<Team>,
    logger: Logger<Entry>,
    mut player: impl FnMut(&PlayerId) -> Builder<Actor>,
) -> Episode<Actor> {
    let players: Vec<PlayerId> = roles.keys().cloned().collect();
    let mut init = HashMap::from([(
        ENVIRONMENT.to_string(),
        announced::init(roles, rules, winner, Actor::Environment),
    )]);
    for id in players {
        let behavior = player(&id);
        init.insert(
            id,
            ActorInit {
                behavior,
                think: None,
                can_send_to: HashSet::from([ENVIRONMENT.to_string()]),
                can_shut_down: HashSet::new(),
                has_logger: false,
            },
        );
    }
    Episode::new(init, logger)
}

/// What an actor in the game does: run it, or play in it. An episode holds
/// one kind of actor, so the two sides meet here and each method goes to
/// whichever side this is.
pub enum Actor {
    /// The side that holds the game.
    Environment(Environment),
    /// A side that sees only what it is shown.
    Player(Player),
    /// A player who says whatever its script says.
    #[cfg(test)]
    Puppet(tests::Puppet),
}

impl Actor {
    /// Builds a player once the episode has made its context.
    pub fn player() -> Builder<Self> {
        Box::new(|context| Actor::Player(Player { context }))
    }
}

// Nobody in this game requests, so the sides only receive and start.
#[async_trait]
impl Behavior for Actor {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        match self {
            Actor::Environment(environment) => environment.context(),
            Actor::Player(player) => player.context(),
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.context(),
        }
    }

    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.receive(message).await,
            Actor::Player(player) => player.receive(message).await,
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.receive(message).await,
        }
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.start().await,
            Actor::Player(player) => player.start().await,
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.start().await,
        }
    }
}

/// A player who answers each announcement at once, selecting at random
/// among the living whose role it does not know, as the uniform-random
/// player chooses. It has no think loop.
pub struct Player {
    context: Context<Message, Entry>,
}

#[async_trait]
impl Behavior for Player {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        &self.context
    }

    /// Announced a phase, select someone in it at once. With nobody to
    /// select, say nothing.
    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        if let Message::Announce { seq, observation } = message
            && let Some(target) = choose(&self.context.id, observation)
        {
            let selection = Message::Select {
                seq: *seq,
                from: self.context.id.clone(),
                target,
            };
            self.context
                .send(selection, HashSet::from([ENVIRONMENT.to_string()]))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::werewolf::tests::village;
    use crate::werewolf::{Observation, Phase};
    use std::time::Duration;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::time::Instant;

    /// What a puppet says to the environment on being announced a phase,
    /// given its own name, the phase's number, and what it may see.
    type Script = Box<dyn Fn(&PlayerId, u64, &Observation) -> Vec<Message> + Send>;

    /// A player who says to the environment whatever its script says, for
    /// a game with a player who is late, stale, or repeats itself.
    pub struct Puppet {
        context: Context<Message, Entry>,
        script: Script,
    }

    #[async_trait]
    impl Behavior for Puppet {
        type Message = Message;
        type Log = Entry;

        fn context(&self) -> &Context<Message, Entry> {
            &self.context
        }

        async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
            if let Message::Announce { seq, observation } = message {
                for said in (self.script)(&self.context.id, *seq, observation) {
                    self.context
                        .send(said, HashSet::from([ENVIRONMENT.to_string()]))?;
                }
            }
            Ok(())
        }
    }

    impl Actor {
        fn puppet(script: Script) -> Builder<Self> {
            Box::new(|context| Actor::Puppet(Puppet { context, script }))
        }
    }

    /// A selection of `target` by `from` in the phase numbered `seq`.
    fn select(seq: u64, from: &PlayerId, target: &str) -> Message {
        Message::Select {
            seq,
            from: from.clone(),
            target: target.to_string(),
        }
    }

    /// The players `me` may select in `observation`, in name order.
    fn candidates(me: &PlayerId, observation: &Observation) -> Vec<PlayerId> {
        let mut candidates: Vec<PlayerId> = observation
            .alive
            .iter()
            .filter(|player| *player != me && !observation.roles.contains_key(*player))
            .cloned()
            .collect();
        candidates.sort();
        candidates
    }

    /// A player who never selects.
    fn mute() -> Builder<Actor> {
        Actor::puppet(Box::new(|_, _, _| vec![]))
    }

    /// Run a game of `roles` under `rules`, each player scripted except
    /// those in `puppets`, who follow their own scripts. Without puppets
    /// it is the game as [`game`] builds it. Returns the winner, everything
    /// the environment logged, and how long the game took.
    async fn play(
        roles: HashMap<PlayerId, Role>,
        rules: Rules,
        mut puppets: HashMap<&str, Builder<Actor>>,
    ) -> anyhow::Result<(Team, Vec<Entry>, Duration)> {
        let (winner, won) = oneshot::channel();
        let (logger, mut log) = unbounded_channel();
        let episode = if puppets.is_empty() {
            game(roles, rules, winner, logger)
        } else {
            episode(roles, rules, winner, logger, |player| {
                puppets
                    .remove(player.as_str())
                    .unwrap_or_else(Actor::player)
            })
        };

        let started = Instant::now();
        episode.run(Duration::from_secs(60 * 60)).await?;
        let took = started.elapsed();

        let mut logged = Vec::new();
        while let Some(event) = log.recv().await {
            logged.push(event.payload);
        }
        Ok((won.await?, logged, took))
    }

    /// Short limits, so a phase that waits them out is told apart from
    /// one that does not.
    fn quick() -> Rules {
        Rules {
            night_limit: Duration::from_secs(10),
            day_limit: Duration::from_secs(10),
            ..rules()
        }
    }

    /// One werewolf against `villagers`.
    fn one_wolf_against(villagers: &[&str]) -> HashMap<PlayerId, Role> {
        let mut roles = HashMap::from([("wolf".to_string(), Role::Werewolf)]);
        for villager in villagers {
            roles.insert(villager.to_string(), Role::Villager);
        }
        roles
    }

    /// Who the log says survived.
    fn survivors(logged: &[Entry]) -> HashSet<PlayerId> {
        logged
            .iter()
            .find_map(|entry| match entry {
                Entry::End { survivors, .. } => Some(survivors.clone()),
                _ => None,
            })
            .unwrap()
    }

    /// The selections the environment logged as received.
    fn received(logged: &[Entry]) -> Vec<&Message> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Received { message, .. } => Some(message),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn a_game_is_logged_from_the_deal_to_the_end() {
        let roles = village().roles;
        let (winner, logged, _) = play(roles.clone(), rules(), HashMap::new()).await.unwrap();

        assert_eq!(
            logged.first(),
            Some(&Entry::Start {
                variation: "Scripted".to_string(),
                roles: roles.clone(),
            })
        );

        let last = logged.last();
        assert!(
            matches!(
                last,
                Some(Entry::End { winner: won, survivors, .. })
                    if *won == winner
                        && !survivors.is_empty()
                        && survivors.iter().all(|player| roles.contains_key(player))
            ),
            "{last:?}"
        );

        // Everything between is a message to or from a player, as it was.
        let mut announced = 0;
        for entry in &logged[1..logged.len() - 1] {
            match entry {
                Entry::Sent {
                    to,
                    message: Message::Announce { seq, .. },
                } => {
                    assert!(roles.contains_key(to), "{to}");
                    assert!(*seq >= 1, "{seq}");
                    announced += 1;
                }
                Entry::Received {
                    from,
                    message:
                        Message::Select {
                            seq,
                            from: said,
                            target,
                        },
                } => {
                    assert_eq!(from, said);
                    assert!(roles.contains_key(from), "{from}");
                    assert!(roles.contains_key(target), "{target}");
                    assert!(*seq >= 1, "{seq}");
                }
                other => panic!("not a message to or from a player: {other:?}"),
            }
        }
        // By the first night three players are announced the game.
        assert!(announced >= 3, "{logged:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_phase_in_which_everyone_selects_ends_before_its_limit() {
        // The werewolf alone is awake the first night, and whoever it kills
        // brings the werewolves to parity.
        let roles = one_wolf_against(&["ann", "bob"]);
        let (winner, logged, took) = play(roles, quick(), HashMap::new()).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(received(&logged).len(), 1);
        assert!(took < quick().night_limit, "{took:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_phase_with_a_player_who_never_selects_ends_at_its_limit() {
        // The werewolf never selects, so nobody dies, and the werewolf is
        // at parity anyway.
        let roles = one_wolf_against(&["ann"]);
        let puppets = HashMap::from([("wolf", mute())]);
        let (winner, logged, took) = play(roles, quick(), puppets).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert!(received(&logged).is_empty());
        assert_eq!(took, quick().night_limit);
    }

    #[tokio::test(start_paused = true)]
    async fn a_selection_with_an_old_number_is_logged_but_not_counted() {
        let roles = one_wolf_against(&["ann"]);
        let stale: Script = Box::new(|me, seq, _| vec![select(seq - 1, me, "ann")]);
        let puppets = HashMap::from([("wolf", Actor::puppet(stale))]);
        let (_, logged, took) = play(roles, quick(), puppets).await.unwrap();
        assert_eq!(received(&logged), [&select(0, &"wolf".to_string(), "ann")]);
        // Not counted: the night waits out its limit and kills nobody.
        assert_eq!(took, quick().night_limit);
        assert_eq!(survivors(&logged).len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_second_selection_from_the_same_player_is_logged_but_not_counted() {
        // The werewolf selects two villagers in turn, and the doctor never
        // selects, so the night waits out its limit with both selections
        // in. Only the first kills.
        let mut roles = one_wolf_against(&["ann"]);
        roles.insert("doctor".to_string(), Role::Doctor);
        let twice: Script = Box::new(|me, seq, observation| {
            candidates(me, observation)
                .iter()
                .map(|target| select(seq, me, target))
                .collect()
        });
        let puppets = HashMap::from([("wolf", Actor::puppet(twice)), ("doctor", mute())]);
        let (winner, logged, took) = play(roles, quick(), puppets).await.unwrap();
        let wolf = "wolf".to_string();
        assert_eq!(
            received(&logged),
            [&select(1, &wolf, "ann"), &select(1, &wolf, "doctor")]
        );
        assert_eq!(took, quick().night_limit);
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(
            survivors(&logged),
            HashSet::from([wolf, "doctor".to_string()])
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_end_of_a_phase_that_already_ended_is_ignored() {
        // Night 1 ends at once when the werewolf kills bob, and its timer
        // fires during day 1, which waits out its longer limit for ann,
        // who never votes. Were the night's timer to end the day, the day
        // would be over at the night's limit instead.
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let rules = Rules {
            night_limit: Duration::from_secs(10),
            day_limit: Duration::from_secs(100),
            ..rules()
        };
        let wolf: Script = Box::new(|me, seq, observation| {
            let target = match observation.phase {
                Phase::Night => "bob",
                Phase::Day => "ann",
            };
            vec![select(seq, me, target)]
        });
        let cat: Script = Box::new(|me, seq, _| vec![select(seq, me, "ann")]);
        let puppets = HashMap::from([
            ("wolf", Actor::puppet(wolf)),
            ("ann", mute()),
            ("cat", Actor::puppet(cat)),
        ]);
        let (winner, logged, took) = play(roles, rules, puppets).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, Duration::from_secs(100));
        let phases: HashSet<u64> = logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Sent {
                    message: Message::Announce { seq, .. },
                    ..
                } => Some(*seq),
                _ => None,
            })
            .collect();
        assert_eq!(phases, HashSet::from([1, 2]));
    }

    #[tokio::test(start_paused = true)]
    async fn a_statement_that_is_not_a_selection_is_ignored() {
        let roles = one_wolf_against(&["ann"]);
        let meddler: Script = Box::new(|_, seq, _| vec![Message::EndPhase { seq }]);
        let puppets = HashMap::from([("wolf", Actor::puppet(meddler))]);
        let (_, logged, took) = play(roles, quick(), puppets).await.unwrap();
        assert!(received(&logged).is_empty());
        assert_eq!(took, quick().night_limit);
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_with_nobody_to_select_says_nothing() {
        // Two werewolves know each other, so neither has anyone to select.
        let roles = HashMap::from([
            ("wolf1".to_string(), Role::Werewolf),
            ("wolf2".to_string(), Role::Werewolf),
        ]);
        let (winner, logged, took) = play(roles, quick(), HashMap::new()).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert!(received(&logged).is_empty());
        assert_eq!(took, quick().night_limit);
    }
}
