//! The scripted variant: the announcing environment, played by players who
//! answer each announcement at once with a selection made at random. It
//! plays the model-played variant's game without a model.

use super::announced::{self, Environment};
use super::uniform_random::choose;
use super::{ENVIRONMENT, Entry, Message, PlayerId, Role, Rules, Team};
use async_trait::async_trait;
use free_agent::{Behavior, Builder, Context, Episode, Logger};
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
    player: impl FnMut(&PlayerId) -> Builder<Actor>,
) -> Episode<Actor> {
    let players: Vec<PlayerId> = roles.keys().cloned().collect();
    let environment = announced::init(roles, rules, winner, Actor::Environment);
    super::episode(environment, players, player, logger)
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

/// A player who answers each announcement at once, selecting one of its
/// [candidates](super::candidates) at random, as the uniform-random player
/// chooses. It has no think loop.
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
    use crate::werewolf::tests::{
        Played, between_the_deal_and_the_end, one_wolf_against, received, run, selection, village,
    };
    use crate::werewolf::{Observation, Phase};
    use std::time::Duration;
    use tokio::sync::mpsc::unbounded_channel;

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

    /// A player who never selects.
    fn mute() -> Builder<Actor> {
        Actor::puppet(Box::new(|_, _, _| vec![]))
    }

    /// Run a game of `roles` under `rules`, each player scripted except
    /// those in `puppets`, who follow their own scripts.
    async fn play(
        roles: HashMap<PlayerId, Role>,
        rules: Rules,
        mut puppets: HashMap<&str, Builder<Actor>>,
    ) -> anyhow::Result<Played> {
        let (winner, won) = oneshot::channel();
        let (logger, log) = unbounded_channel();
        let episode = episode(roles, rules, winner, logger, |player| {
            puppets
                .remove(player.as_str())
                .unwrap_or_else(Actor::player)
        });
        run(episode, won, log).await
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

    #[tokio::test(start_paused = true)]
    async fn a_game_is_logged_from_the_deal_to_the_end() {
        let roles = village().roles;
        let (winner, won) = oneshot::channel();
        let (logger, log) = unbounded_channel();
        let episode = game(roles.clone(), rules(), winner, logger);
        let (winner, logged, _) = run(episode, won, log).await.unwrap();

        // Everything between is a message to or from a player, as it was.
        let mut announced = 0;
        for entry in between_the_deal_and_the_end(&logged, "Scripted", &roles, winner) {
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
        let stale: Script = Box::new(|me, seq, _| vec![selection(seq - 1, me, "ann")]);
        let puppets = HashMap::from([("wolf", Actor::puppet(stale))]);
        let (_, logged, took) = play(roles, quick(), puppets).await.unwrap();
        assert_eq!(received(&logged), [&selection(0, "wolf", "ann")]);
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
        let twice: Script =
            Box::new(|me, seq, _| vec![selection(seq, me, "ann"), selection(seq, me, "doctor")]);
        let puppets = HashMap::from([("wolf", Actor::puppet(twice)), ("doctor", mute())]);
        let (winner, logged, took) = play(roles, quick(), puppets).await.unwrap();
        let wolf = "wolf".to_string();
        assert_eq!(
            received(&logged),
            [&selection(1, &wolf, "ann"), &selection(1, &wolf, "doctor")]
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
            vec![selection(seq, me, target)]
        });
        let cat: Script = Box::new(|me, seq, _| vec![selection(seq, me, "ann")]);
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
