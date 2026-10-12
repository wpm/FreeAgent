//! The uniform-random variant: the announcing environment, played by
//! players who answer each announcement of a phase they are awake in at
//! once with a selection made at random, and sleep through the rest. A
//! player logs an [`Entry::Observation`] for each announcement it receives
//! and an [`Entry::Action`] for each selection it sends, numbering its
//! selections from 1; no model made them, so `seen` is `None`.

use super::announced::{self, Environment};
use super::{
    ENVIRONMENT, Entry, Message, PlayerId, Role, Rules, Team, View, act, awake, candidates,
    observe, to_the_environment,
};
use async_trait::async_trait;
use free_agent::{Behavior, Builder, Context, Episode, Logger};
use rand::seq::IteratorRandom;
use std::collections::HashMap;
use tokio::sync::oneshot;

/// An episode of a game of `roles` under `rules`: the announcing
/// environment, which may reach and stop every player and holds `logger`,
/// and a random player for each role. The winning team is sent on `winner`
/// when the game ends.
pub fn game(
    roles: HashMap<PlayerId, Role>,
    rules: Rules,
    winner: oneshot::Sender<Team>,
    logger: Logger<Entry>,
) -> Episode<Actor> {
    let players: Vec<PlayerId> = roles.keys().cloned().collect();
    let environment = announced::init(roles, rules, None, winner, Actor::Environment);
    super::episode(environment, players, |_| Actor::player(), logger)
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
        Box::new(|context| Actor::Player(Player { context, n: 0 }))
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

/// A player who answers each announcement of a phase it is [`awake`] in at
/// once, selecting one of its [`candidates`] uniformly at random, and does
/// nothing about one it sleeps through. It has no think loop.
pub struct Player {
    context: Context<Message, Entry>,
    /// The number of the last selection it sent.
    n: u64,
}

#[async_trait]
impl Behavior for Player {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        &self.context
    }

    /// Announced a phase, log the announcement as observed in it, and, if
    /// awake in it, select someone at once. Asleep, or with nobody to
    /// select, say nothing.
    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        if let Message::Announce { seq, n, view, .. } = message {
            observe(&self.context, ENVIRONMENT.to_string(), *n, *seq);
            let me = &self.context.id;
            if awake(me, view)
                && let Some(target) = choose(me, view)
            {
                self.n += 1;
                let selection = Message::Select {
                    seq: *seq,
                    from: self.context.id.clone(),
                    n: self.n,
                    target,
                };
                act(&self.context, selection, to_the_environment(), None)?;
            }
        }
        Ok(())
    }
}

/// One of the [`candidates`] `me` sees in `view`, chosen
/// uniformly at random. None when there are no candidates.
fn choose(me: &PlayerId, view: &View) -> Option<PlayerId> {
    candidates(me, view).into_iter().choose(&mut rand::rng())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::werewolf::report::Narrator;
    use crate::werewolf::tests::{
        Played, between_the_deal_and_the_end, logged, observed_by, observed_by_the_environment,
        one_wolf_against, run, seen, selection, selections, village,
    };
    use crate::werewolf::{Phase, View, thinking_episode, to_the_players_but};
    use free_agent::{Think, ThinkBuilder};
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::mpsc::unbounded_channel;

    /// What a puppet says to the environment on being announced a phase,
    /// given its own name, the phase's number, and what it may see: each
    /// message, and how long after the announcement it says it. A
    /// selection carries the number the script gives it.
    pub(crate) type Script = Box<dyn Fn(&PlayerId, u64, &View) -> Vec<(Duration, Message)> + Send>;

    /// `messages`, each said as soon as the phase is announced.
    pub(crate) fn at_once(messages: Vec<Message>) -> Vec<(Duration, Message)> {
        messages
            .into_iter()
            .map(|message| (Duration::ZERO, message))
            .collect()
    }

    /// A player who says whatever its script says, when the script says
    /// to, for a game with a player who is late, stale, repeats itself,
    /// changes its mind, or talks. Its script is consulted for each phase
    /// it is awake in; announced a phase it sleeps through, it notes who is
    /// alive and says nothing. An utterance goes to every other player
    /// alive at the latest announcement; anything else goes to the
    /// environment. It logs each message it sends as its action, and
    /// nothing it receives. What it says later is said by its think loop,
    /// a [`Cue`].
    pub struct Puppet {
        context: Context<Message, Entry>,
        script: Script,
        /// Who was alive at the latest announcement, shared with the cue.
        alive: Arc<Mutex<HashSet<PlayerId>>>,
    }

    #[async_trait]
    impl Behavior for Puppet {
        type Message = Message;
        type Log = Entry;

        fn context(&self) -> &Context<Message, Entry> {
            &self.context
        }

        async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
            if let Message::Announce { seq, view, .. } = message {
                *self.alive.lock().unwrap() = view.alive.clone();
                if !awake(&self.context.id, view) {
                    return Ok(());
                }
                for (after, said) in (self.script)(&self.context.id, *seq, view) {
                    if after.is_zero() {
                        say(&self.context, said, &self.alive)?;
                    } else {
                        self.context.think_after(after, said)?;
                    }
                }
            }
            Ok(())
        }
    }

    /// Send `message` from the puppet of `context`: an utterance to every
    /// other player in `alive`, anything else to the environment.
    fn say(
        context: &Context<Message, Entry>,
        message: Message,
        alive: &Mutex<HashSet<PlayerId>>,
    ) -> anyhow::Result<()> {
        let to = match &message {
            Message::Say { .. } => to_the_players_but(&context.id, &*alive.lock().unwrap()),
            _ => to_the_environment(),
        };
        act(context, message, to, None)
    }

    /// A puppet's think loop: says each message it is reminded of when the
    /// time comes.
    struct Cue {
        context: Context<Message, Entry>,
        alive: Arc<Mutex<HashSet<PlayerId>>>,
    }

    #[async_trait]
    impl Think for Cue {
        type Message = Message;

        async fn think(&mut self, message: &Message) -> anyhow::Result<()> {
            say(&self.context, message.clone(), &self.alive)
        }
    }

    /// How a seat is built: the player's behavior, and its think loop if
    /// it has one.
    pub(crate) type Seat<A = Actor> = (Builder<A>, Option<ThinkBuilder<Message, Entry>>);

    impl Puppet {
        /// A puppet following `script`, as the actor `wrap` makes of it.
        pub(crate) fn init<A>(
            script: Script,
            wrap: impl FnOnce(Puppet) -> A + Send + 'static,
        ) -> Seat<A>
        where
            A: Behavior<Message = Message, Log = Entry>,
        {
            let alive = Arc::new(Mutex::new(HashSet::new()));
            let shared = Arc::clone(&alive);
            let behavior: Builder<A> = Box::new(move |context| {
                wrap(Puppet {
                    context,
                    script,
                    alive,
                })
            });
            let think: ThinkBuilder<Message, Entry> = Box::new(move |context| {
                Box::new(Cue {
                    context,
                    alive: shared,
                })
            });
            (behavior, Some(think))
        }
    }

    impl Actor {
        /// A puppet following `script`.
        pub(crate) fn puppet(script: Script) -> Seat {
            Puppet::init(script, Actor::Puppet)
        }
    }

    /// A player who never selects.
    pub(crate) fn mute() -> Seat {
        Actor::puppet(Box::new(|_, _, _| vec![]))
    }

    /// Run a game of `roles` under `rules`, each player choosing at random
    /// except those in `puppets`, who follow their own scripts.
    pub(crate) async fn play(
        roles: HashMap<PlayerId, Role>,
        rules: Rules,
        mut puppets: HashMap<&str, Seat>,
    ) -> anyhow::Result<Played> {
        let (winner, won) = oneshot::channel();
        let (logger, log) = unbounded_channel();
        let players: Vec<PlayerId> = roles.keys().cloned().collect();
        let environment = announced::init(roles, rules, None, winner, Actor::Environment);
        let episode = thinking_episode(
            environment,
            players,
            |player| {
                puppets
                    .remove(player.as_str())
                    .unwrap_or_else(|| (Actor::player(), None))
            },
            logger,
        );
        run(episode, won, log).await
    }

    /// Short limits, so a phase that waits them out is told apart from
    /// one that does not.
    pub(crate) fn quick() -> Rules {
        Rules {
            night_limit: Duration::from_secs(10),
            day_limit: Duration::from_secs(10),
            ..Rules::default()
        }
    }

    /// How long the log says the game took.
    fn game_took(logged: &[Entry]) -> Duration {
        logged
            .iter()
            .find_map(|entry| match entry {
                Entry::End { took, .. } => Some(*took),
                _ => None,
            })
            .unwrap()
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
        let episode = game(roles.clone(), Rules::default(), winner, logger);
        let (winner, logged, _) = run(episode, won, log).await.unwrap();

        // Everything between is an announcement or a selection, logged as
        // the action of its sender and the observation of its receiver, or
        // a night's hunt.
        let doctors: HashSet<&PlayerId> = roles
            .iter()
            .filter(|(_, role)| **role == Role::Doctor)
            .map(|(player, _)| player)
            .collect();
        let between = between_the_deal_and_the_end(&logged, &Rules::default(), &roles, winner);
        // How often each message, named by its sender and number, was
        // logged as an action and as an observation.
        let mut actions: HashMap<(&str, u64), usize> = HashMap::new();
        let mut observations: HashMap<(&str, u64), usize> = HashMap::new();
        let mut announced = 0;
        let mut selected = 0;
        let mut observed = 0;
        let mut hunts = 0;
        for entry in between {
            if let Entry::Action {
                from,
                n,
                to,
                message: Message::Announce { seq, .. },
                seen,
            } = entry
            {
                assert_eq!(from, ENVIRONMENT);
                assert_eq!(to.len(), 1, "{to:?}");
                assert!(to.iter().all(|player| roles.contains_key(player)), "{to:?}");
                assert!(*seq >= 1, "{seq}");
                assert_eq!(*seen, None);
                *actions.entry((from, *n)).or_default() += 1;
                announced += 1;
            }
            if let Entry::Action {
                from,
                n,
                to,
                message:
                    Message::Select {
                        seq,
                        from: said,
                        target,
                        ..
                    },
                seen,
            } = entry
            {
                assert_eq!(from, said);
                assert!(roles.contains_key(from), "{from}");
                assert!(roles.contains_key(target), "{target}");
                assert_eq!(*to, to_the_environment());
                assert!(*seq >= 1, "{seq}");
                assert_eq!(*seen, None);
                *actions.entry((from, *n)).or_default() += 1;
                selected += 1;
            }
            if let Entry::Observation { by, from, n, seq } = entry {
                // The environment observes the players, and they it.
                let between_them = (by == ENVIRONMENT && roles.contains_key(from))
                    || (from == ENVIRONMENT && roles.contains_key(by));
                assert!(between_them, "{entry:?}");
                assert!(*seq >= 1, "{seq}");
                *observations.entry((from, *n)).or_default() += 1;
                observed += 1;
            }
            if let Entry::Hunted { seq, hunt } = entry {
                assert!(*seq >= 1, "{seq}");
                assert!(roles.contains_key(&hunt.victim), "{hunt:?}");
                let protectors: HashSet<&PlayerId> = hunt.protectors.iter().collect();
                assert!(protectors.is_subset(&doctors), "{hunt:?}");
                hunts += 1;
            }
        }
        // Nothing else is logged between: no reminder to end a phase.
        assert_eq!(
            announced + selected + observed + hunts,
            between.len(),
            "{logged:?}"
        );
        // Every message is logged once as an action and once as an
        // observation, with the same number.
        assert_eq!(actions, observations, "{logged:?}");
        assert!(actions.values().all(|count| *count == 1), "{logged:?}");
        // Each sender numbers its messages from 1, one up each.
        let mut numbers: HashMap<&str, Vec<u64>> = HashMap::new();
        for (from, n) in actions.keys() {
            numbers.entry(from).or_default().push(*n);
        }
        for (from, mut sent) in numbers {
            sent.sort_unstable();
            let counted: Vec<u64> = (1..=sent.len() as u64).collect();
            assert_eq!(sent, counted, "{from}");
        }
        // By the first night every player is announced the game, awake or
        // not, and the werewolves, who always select, hunt.
        assert!(announced >= roles.len(), "{logged:?}");
        assert!(hunts >= 1, "{logged:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_announced_a_phase_it_sleeps_through_observes_it_and_selects_nothing() {
        // Ann, a villager, is announced the first night, which the werewolf
        // sleeps through too, so that the night waits out its limit with no
        // selection from anyone.
        let roles = one_wolf_against(&["ann"]);
        let puppets = HashMap::from([("wolf", mute())]);
        let (winner, logged, took) = play(roles, quick(), puppets).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(took, quick().night_limit);
        assert!(selections(&logged).is_empty(), "{logged:?}");
        // Ann observed the announcement, the environment's first message,
        // and nothing else.
        assert_eq!(observed_by(&logged, "ann"), [(ENVIRONMENT, 1, 1)]);
    }

    #[tokio::test(start_paused = true)]
    async fn the_werewolves_win_on_reaching_parity() {
        // One werewolf against two: whoever it kills the first night, the
        // werewolf then equals the village.
        let roles = one_wolf_against(&["ann", "bob"]);
        let (winner, _, _) = play(roles, Rules::default(), HashMap::new()).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
    }

    #[tokio::test(start_paused = true)]
    async fn a_phase_in_which_everyone_selects_ends_before_its_limit() {
        // The werewolf alone is awake the first night, and whoever it kills
        // brings the werewolves to parity.
        let roles = one_wolf_against(&["ann", "bob"]);
        let (winner, logged, took) = play(roles, quick(), HashMap::new()).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(selections(&logged).len(), 1);
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
        assert!(selections(&logged).is_empty());
        assert_eq!(took, quick().night_limit);
        // The log says so too.
        assert_eq!(game_took(&logged), quick().night_limit);
    }

    #[tokio::test(start_paused = true)]
    async fn a_selection_with_an_old_number_is_logged_but_not_counted() {
        let roles = one_wolf_against(&["ann"]);
        let stale: Script = Box::new(|me, seq, _| at_once(vec![selection(seq - 1, me, 1, "ann")]));
        let puppets = HashMap::from([("wolf", Actor::puppet(stale))]);
        let (_, logged, took) = play(roles, quick(), puppets).await.unwrap();
        assert_eq!(selections(&logged), [&selection(0, "wolf", 1, "ann")]);
        // The environment observes it in the phase it is in.
        assert_eq!(observed_by_the_environment(&logged), [("wolf", 1, 1)]);
        // Not counted: the night waits out its limit and kills nobody.
        assert_eq!(took, quick().night_limit);
        assert_eq!(survivors(&logged).len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_selection_that_arrives_in_a_later_phase_is_observed_in_it_and_told_as_too_late() {
        // The werewolf kills bob the first night, and by day sends a
        // selection for the night instead of a vote, which the environment
        // observes by day. Nobody votes, so the day waits out its limit,
        // and the second night the werewolf kills ann and wins.
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let wolf: Script = Box::new(|me, seq, _| {
            at_once(match seq {
                1 => vec![selection(1, me, 1, "bob")],
                2 => vec![selection(1, me, 2, "ann")],
                seq => vec![selection(seq, me, seq, "ann")],
            })
        });
        let puppets = HashMap::from([
            ("wolf", Actor::puppet(wolf)),
            ("ann", mute()),
            ("cat", mute()),
        ]);
        let (winner, entries, _) = play(roles, quick(), puppets).await.unwrap();
        assert_eq!(winner, Team::Werewolves);
        assert_eq!(
            observed_by_the_environment(&entries),
            [("wolf", 1, 1), ("wolf", 2, 2), ("wolf", 3, 3)]
        );
        let mut narrator = Narrator::default();
        let told: Vec<String> = entries
            .into_iter()
            .flat_map(|entry| narrator.narrate(&logged(entry)))
            .collect();
        assert!(
            told.contains(
                &"wolf (werewolf) chooses ann (villager) for night 1 too late to count."
                    .to_string()
            ),
            "{told:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_second_selection_from_the_same_player_is_logged_but_not_counted() {
        // The werewolf selects two villagers in turn, and the doctor never
        // selects, so the night waits out its limit with both selections
        // in. Only the first kills.
        let mut roles = one_wolf_against(&["ann"]);
        roles.insert("doctor".to_string(), Role::Doctor);
        let twice: Script = Box::new(|me, seq, _| {
            at_once(vec![
                selection(seq, me, 1, "ann"),
                selection(seq, me, 2, "doctor"),
            ])
        });
        let puppets = HashMap::from([("wolf", Actor::puppet(twice)), ("doctor", mute())]);
        let (winner, logged, took) = play(roles, quick(), puppets).await.unwrap();
        let wolf = "wolf".to_string();
        assert_eq!(
            selections(&logged),
            [
                &selection(1, &wolf, 1, "ann"),
                &selection(1, &wolf, 2, "doctor")
            ]
        );
        assert_eq!(
            observed_by_the_environment(&logged),
            [("wolf", 1, 1), ("wolf", 2, 1)]
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
            ..Rules::default()
        };
        let wolf: Script = Box::new(|me, seq, view| {
            let target = match view.phase {
                Phase::Night => "bob",
                Phase::Day => "ann",
            };
            at_once(vec![selection(seq, me, seq, target)])
        });
        let cat: Script = Box::new(|me, seq, _| at_once(vec![selection(seq, me, 1, "ann")]));
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
                Entry::Action {
                    message: Message::Announce { seq, .. },
                    ..
                } => Some(*seq),
                _ => None,
            })
            .collect();
        assert_eq!(phases, HashSet::from([1, 2]));
    }

    #[tokio::test(start_paused = true)]
    async fn a_message_that_is_not_a_selection_is_neither_counted_nor_observed() {
        let roles = one_wolf_against(&["ann"]);
        let meddler: Script = Box::new(|_, seq, _| at_once(vec![Message::EndPhase { seq }]));
        let puppets = HashMap::from([("wolf", Actor::puppet(meddler))]);
        let (_, logged, took) = play(roles, quick(), puppets).await.unwrap();
        assert!(selections(&logged).is_empty());
        assert!(observed_by_the_environment(&logged).is_empty());
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
        assert!(selections(&logged).is_empty());
        assert_eq!(took, quick().night_limit);
    }

    #[test]
    fn a_player_chooses_one_of_its_candidates() {
        let me = "wolf1".to_string();
        let view = seen(
            &["wolf1", "wolf2", "ann", "bob"],
            &[("wolf1", Role::Werewolf), ("wolf2", Role::Werewolf)],
        );
        let allowed = candidates(&me, &view);
        assert_eq!(allowed.len(), 2);

        for _ in 0..50 {
            let chosen = choose(&me, &view).unwrap();
            assert!(allowed.contains(&chosen), "{chosen}");
        }
    }

    #[test]
    fn a_player_without_candidates_chooses_no_one() {
        let view = seen(&["ann"], &[("ann", Role::Villager)]);
        assert_eq!(choose(&"ann".to_string(), &view), None);
    }
}
