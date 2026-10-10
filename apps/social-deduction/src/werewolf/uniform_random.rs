//! The uniform-random variant: an environment that runs the game as a loop
//! and players who choose at random.

use super::{Choices, Entry, Message, Observation, PlayerId, Role, Rules, State, Team, candidates};
use async_trait::async_trait;
use free_agent::{ActorInit, Behavior, Builder, Context, Episode, Logger};
use futures_util::future::try_join_all;
use rand::seq::IndexedRandom;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;

/// An episode of a game of `roles` under `rules`: the environment, which
/// may reach and stop every player and holds `logger`, and a player for
/// each role built by `player`, who may send to the environment. The
/// winning team is sent on `winner` when the game ends.
pub fn game(
    roles: HashMap<PlayerId, Role>,
    rules: Rules,
    winner: oneshot::Sender<Team>,
    logger: Logger<Entry>,
    player: impl FnMut(&PlayerId) -> Builder<Actor>,
) -> Episode<Actor> {
    let players: HashSet<PlayerId> = roles.keys().cloned().collect();
    let environment = ActorInit {
        behavior: Actor::environment(roles, rules, winner),
        think: None,
        can_send_to: players.clone(),
        can_shut_down: players.clone(),
        has_logger: true,
    };
    super::episode(environment, players, player, logger)
}

/// What an actor in the game does: run it, or play in it. An episode holds
/// one kind of actor, so the two sides meet here and each method goes to
/// whichever side this is.
pub enum Actor {
    /// The side that holds the game.
    Environment(Box<Environment>),
    /// A side that sees only what it is shown.
    Player(Player),
    /// A player who never answers.
    #[cfg(test)]
    Mute(tests::Mute),
}

impl Actor {
    /// Builds the environment for a game of `roles` under `rules` once the
    /// episode has made its context. The winning team is sent on `winner`
    /// when the game ends.
    pub fn environment(
        roles: HashMap<PlayerId, Role>,
        rules: Rules,
        winner: oneshot::Sender<Team>,
    ) -> Builder<Self> {
        Box::new(move |context| {
            Actor::Environment(Box::new(Environment {
                context,
                state: State::new(roles),
                rules,
                winner: Some(winner),
            }))
        })
    }

    /// Builds a player once the episode has made its context.
    pub fn player() -> Builder<Self> {
        Box::new(|context| Actor::Player(Player::new(context)))
    }
}

#[async_trait]
impl Behavior for Actor {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        match self {
            Actor::Environment(environment) => environment.context(),
            Actor::Player(player) => player.context(),
            #[cfg(test)]
            Actor::Mute(mute) => mute.context(),
        }
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.initialize().await,
            Actor::Player(player) => player.initialize().await,
            #[cfg(test)]
            Actor::Mute(mute) => mute.initialize().await,
        }
    }

    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.receive(message).await,
            Actor::Player(player) => player.receive(message).await,
            #[cfg(test)]
            Actor::Mute(mute) => mute.receive(message).await,
        }
    }

    async fn answer(&mut self, message: &Message) -> anyhow::Result<Vec<Message>> {
        match self {
            Actor::Environment(environment) => environment.answer(message).await,
            Actor::Player(player) => player.answer(message).await,
            #[cfg(test)]
            Actor::Mute(mute) => mute.answer(message).await,
        }
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.start().await,
            Actor::Player(player) => player.start().await,
            #[cfg(test)]
            Actor::Mute(mute) => mute.start().await,
        }
    }

    async fn clean_up(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.clean_up().await,
            Actor::Player(player) => player.clean_up().await,
            #[cfg(test)]
            Actor::Mute(mute) => mute.clean_up().await,
        }
    }
}

/// The actor that holds the game and tells each player what it may see.
pub struct Environment {
    context: Context<Message, Entry>,
    state: State,
    rules: Rules,
    /// Where the winning team goes when the game ends. Taken when sent.
    winner: Option<oneshot::Sender<Team>>,
}

impl Environment {
    /// The werewolves, the doctor, and the seer each choose, and the night
    /// is resolved from what they chose.
    async fn night(&mut self) -> anyhow::Result<()> {
        let choices = self.gather(self.rules.night_limit).await?;
        let dead = self
            .state
            .resolve_night(&choices, self.rules.night_vote.as_ref());
        self.bury(dead)
    }

    /// Everyone alive chooses, and the day is resolved from what they chose.
    async fn day(&mut self) -> anyhow::Result<()> {
        let choices = self.gather(self.rules.day_limit).await?;
        let dead = self
            .state
            .resolve_day(&choices, self.rules.day_vote.as_ref());
        self.bury(dead)
    }

    /// Ask everyone awake what they choose, all at once, and gather their
    /// choices by player, writing each one down. A player who has not
    /// answered within `limit` is left out.
    async fn gather(&self, limit: Duration) -> anyhow::Result<Choices> {
        let asked = self
            .state
            .awake()
            .into_iter()
            .map(|player| self.ask(player, limit));
        let replies = try_join_all(asked).await?;
        let mut choices = Choices::new();
        for (player, said) in replies.into_iter().flatten() {
            for message in said {
                if let Message::Action(chosen) = &message {
                    choices.insert(player.clone(), chosen.clone());
                }
                self.context.log(Entry::Replied {
                    from: player.clone(),
                    message,
                });
            }
        }
        Ok(choices)
    }

    /// Show `player` what it may see and wait up to `limit` for what it
    /// says back. Past the limit it is taken to have said nothing.
    async fn ask(
        &self,
        player: PlayerId,
        limit: Duration,
    ) -> anyhow::Result<HashMap<PlayerId, Vec<Message>>> {
        let message = Message::Observation(self.state.observation(player.clone())?);
        self.context.log(Entry::Sent {
            to: player.clone(),
            message: message.clone(),
        });
        let asked = self.context.request(message, HashSet::from([player]));
        match timeout(limit, asked).await {
            Ok(replied) => replied,
            Err(_) => Ok(HashMap::new()),
        }
    }

    /// Stop the actor of a player the state has just killed, so a death
    /// in the state and the end of the actor go together.
    fn bury(&self, dead: Option<PlayerId>) -> anyhow::Result<()> {
        match dead {
            Some(player) => self.context.stop(&player),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl Behavior for Environment {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        &self.context
    }

    /// Log the deal, play night and day in turn until a team has won, log
    /// the result, then stop the survivors, report the winner, and end the
    /// episode.
    async fn start(&mut self) -> anyhow::Result<()> {
        self.context.log(Entry::Start {
            variation: self.rules.variation.clone(),
            roles: self.state.roles.clone(),
        });
        let winner = loop {
            self.night().await?;
            if let Some(team) = self.state.winner() {
                break team;
            }
            self.state.next();
            self.day().await?;
            if let Some(team) = self.state.winner() {
                break team;
            }
            self.state.next();
        };
        self.context.log(Entry::End {
            winner,
            days: self.state.days(),
            survivors: self.state.alive.clone(),
        });
        for player in &self.state.alive {
            self.context.stop(player)?;
        }
        if let Some(report) = self.winner.take() {
            // The owner of the episode may have stopped listening.
            let _ = report.send(winner);
        }
        self.context.shutdown();
        Ok(())
    }
}

/// A player who, asked to choose, picks one of its [`candidates`] at
/// random.
pub struct Player {
    context: Context<Message, Entry>,
}

impl Player {
    /// A player holding `context`, built once the episode has made it.
    fn new(context: Context<Message, Entry>) -> Self {
        Player { context }
    }
}

#[async_trait]
impl Behavior for Player {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        &self.context
    }

    /// Asked with an observation, choose from it. Asked anything else, say
    /// nothing.
    async fn answer(&mut self, message: &Message) -> anyhow::Result<Vec<Message>> {
        let Message::Observation(observation) = message else {
            return Ok(vec![]);
        };
        Ok(choose(&self.context.id, observation)
            .into_iter()
            .map(Message::Action)
            .collect())
    }
}

/// One of the [`candidates`] `me` sees in `observation`, chosen
/// uniformly at random. None when there are no candidates. The scripted
/// player chooses this way too.
pub(super) fn choose(me: &PlayerId, observation: &Observation) -> Option<PlayerId> {
    candidates(me, observation)
        .choose(&mut rand::rng())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::werewolf::ENVIRONMENT;
    use crate::werewolf::tests::{between_the_deal_and_the_end, one_wolf_against, seen, village};
    use tokio::sync::mpsc::unbounded_channel;

    /// A player who never answers, for a game with a time limit to wait
    /// out.
    pub struct Mute {
        context: Context<Message, Entry>,
    }

    #[async_trait]
    impl Behavior for Mute {
        type Message = Message;
        type Log = Entry;

        fn context(&self) -> &Context<Message, Entry> {
            &self.context
        }

        /// Clears its throat to the environment, the one statement in a
        /// game of players who otherwise only answer, so a statement goes
        /// through [`Actor`] too. The environment ignores it.
        async fn initialize(&mut self) -> anyhow::Result<()> {
            let me = self.context.id.clone();
            self.context.send(
                Message::Action(me),
                HashSet::from([ENVIRONMENT.to_string()]),
            )
        }

        async fn answer(&mut self, _: &Message) -> anyhow::Result<Vec<Message>> {
            std::future::pending().await
        }
    }

    impl Actor {
        fn mute() -> Builder<Self> {
            Box::new(|context| Actor::Mute(Mute { context }))
        }
    }

    /// Run a game of `roles` under `rules`, each player choosing at random
    /// except those in `mute`, who never answer. Returns the winner and
    /// everything the environment logged.
    async fn play(
        roles: HashMap<PlayerId, Role>,
        rules: Rules,
        mute: &[&str],
    ) -> anyhow::Result<(Team, Vec<Entry>)> {
        let (winner, won) = oneshot::channel();
        let (logger, mut log) = unbounded_channel();
        let episode = game(roles, rules, winner, logger, |player| {
            if mute.contains(&player.as_str()) {
                Actor::mute()
            } else {
                Actor::player()
            }
        });

        episode.run(Duration::from_secs(60)).await?;

        let mut logged = Vec::new();
        while let Some(event) = log.recv().await {
            logged.push(event.payload);
        }
        Ok((won.await?, logged))
    }

    #[tokio::test]
    async fn a_game_is_logged_from_the_deal_to_the_end() {
        let roles = village().roles;
        let (winner, logged) = play(roles.clone(), Rules::default(), &[]).await.unwrap();

        // Everything between is a message to or from a player, as it was.
        let mut shown = 0;
        for entry in between_the_deal_and_the_end(&logged, "Uniform Random", &roles, winner) {
            match entry {
                Entry::Sent {
                    to,
                    message: Message::Observation(_),
                } => {
                    assert!(roles.contains_key(to), "{to}");
                    shown += 1;
                }
                Entry::Replied {
                    from,
                    message: Message::Action(chosen),
                } => {
                    assert!(roles.contains_key(from), "{from}");
                    assert!(roles.contains_key(chosen), "{chosen}");
                }
                other => panic!("not a message to or from a player: {other:?}"),
            }
        }
        // By the first night three players are shown the game.
        assert!(shown >= 3, "{logged:?}");
    }

    #[tokio::test]
    async fn the_werewolves_win_on_reaching_parity() {
        // One werewolf against two: whoever it kills the first night, the
        // werewolf then equals the village.
        let roles = one_wolf_against(&["ann", "bob"]);
        let (winner, _) = play(roles, Rules::default(), &[]).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_who_misses_the_time_limit_is_left_out() {
        // The werewolf never answers, so no night kills anyone, and the
        // villagers vote by day until the game ends one way or the other.
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let rules = Rules {
            night_limit: Duration::from_secs(1),
            day_limit: Duration::from_secs(1),
            ..Rules::default()
        };
        let (winner, _) = play(roles, rules, &["wolf"]).await.unwrap();
        assert!(matches!(winner, Team::Werewolves | Team::Villagers));
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_time_limit_a_silent_player_holds_up_the_game() {
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let rules = Rules {
            night_limit: Duration::from_secs(120),
            ..Rules::default()
        };
        let error = play(roles, rules, &["wolf"]).await.unwrap_err();
        assert!(error.to_string().contains("patience"), "{error}");
    }

    #[test]
    fn a_player_chooses_one_of_its_candidates() {
        let me = "wolf1".to_string();
        let observation = seen(
            &["wolf1", "wolf2", "ann", "bob"],
            &[("wolf1", Role::Werewolf), ("wolf2", Role::Werewolf)],
        );
        let allowed = candidates(&me, &observation);
        assert_eq!(allowed.len(), 2);

        for _ in 0..50 {
            let chosen = choose(&me, &observation).unwrap();
            assert!(allowed.contains(&chosen), "{chosen}");
        }
    }

    #[test]
    fn a_player_without_candidates_chooses_no_one() {
        let observation = seen(&["ann"], &[("ann", Role::Villager)]);
        assert_eq!(choose(&"ann".to_string(), &observation), None);
    }
}
