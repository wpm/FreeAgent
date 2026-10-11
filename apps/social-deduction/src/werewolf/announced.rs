//! The announcing environment, shared by every variant: it opens each
//! phase with a message to every awake player, takes their selections as
//! they come, and ends the phase
//! once everyone awake has selected or at its limit, whichever comes
//! first. Players who take seconds to decide hold nothing up.
//!
//! The environment logs the deal, then the configuration if there is one;
//! an [`Entry::Action`] for each announcement it sends, numbered in one
//! series from 1; an [`Entry::Observation`] for each selection it
//! receives, stale or not, with the phase it was in when the selection
//! arrived; each night's hunt as [`Entry::Hunted`]; and the result. The
//! reminder it sends itself to end a phase is not logged.

use super::{
    Choices, Entry, Hunt, Message, Phase, PlayerId, Role, Rules, State, Team, act, observe,
};
use async_trait::async_trait;
use free_agent::{ActorInit, Behavior, Context, Think};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::Instant;

/// Everything the episode needs to run the environment for a game of
/// `roles` under `rules`, as the actor `wrap` makes of it: it may send to
/// and stop every player, holds the logger, and thinks. It logs the deal,
/// then `configuration` if there is one, before it opens the first night.
/// The winning team is sent on `winner` when the game ends.
pub fn init<A>(
    roles: HashMap<PlayerId, Role>,
    rules: Rules,
    configuration: Option<Entry>,
    winner: oneshot::Sender<Team>,
    wrap: impl FnOnce(Environment) -> A + Send + 'static,
) -> ActorInit<A>
where
    A: Behavior<Message = Message, Log = Entry>,
{
    let players: HashSet<PlayerId> = roles.keys().cloned().collect();
    let game = Arc::new(Mutex::new(Game::new(roles, rules, winner)));
    let shared = Arc::clone(&game);
    ActorInit {
        behavior: Box::new(move |context| {
            wrap(Environment {
                context,
                game,
                configuration: configuration.map(Box::new),
            })
        }),
        think: Some(Box::new(move |context| {
            Box::new(Environment {
                context,
                game: shared,
                configuration: None,
            })
        })),
        can_send_to: players.clone(),
        can_shut_down: players,
        has_logger: true,
    }
}

/// What the environment's two loops share: the game, the phase it is at,
/// and where the winner goes.
struct Game {
    state: State,
    rules: Rules,
    /// The number of the current phase. The first night is 1.
    seq: u64,
    /// The number of the last message the environment sent to a player.
    /// Each announcement is a message of its own.
    n: u64,
    /// The players awake in the current phase.
    awake: HashSet<PlayerId>,
    /// The first selection each awake player made in the current phase.
    selections: Choices,
    /// Where the winning team goes when the game ends. Taken when sent.
    winner: Option<oneshot::Sender<Team>>,
    /// When the game was made, which is as good as when it was dealt.
    started: Instant,
}

impl Game {
    /// A game of `roles` under `rules`, before its first night. The
    /// winning team is sent on `winner` when the game ends.
    fn new(roles: HashMap<PlayerId, Role>, rules: Rules, winner: oneshot::Sender<Team>) -> Self {
        Self {
            state: State::new(roles),
            rules,
            seq: 0,
            n: 0,
            awake: HashSet::new(),
            selections: Choices::new(),
            winner: Some(winner),
            started: Instant::now(),
        }
    }

    /// Open the next phase: number it, wake its players, write each one
    /// its announcement, numbered in turn, and clear the selections.
    fn open(&mut self) -> anyhow::Result<Opened> {
        self.seq += 1;
        self.awake = self.state.awake();
        self.selections.clear();
        let limit = match self.state.phase {
            Phase::Night => self.rules.night_limit,
            Phase::Day => self.rules.day_limit,
        };
        let mut announcements = Vec::new();
        for player in &self.awake {
            self.n += 1;
            let message = Message::Announce {
                seq: self.seq,
                n: self.n,
                view: self.state.view(player.clone())?,
                limit,
            };
            announcements.push((player.clone(), message));
        }
        Ok(Opened {
            seq: self.seq,
            announcements,
            limit,
        })
    }

    /// Record `from`'s selection of `target` in the phase numbered `seq`
    /// if it counts: the phase is the current one, `from` is awake in it,
    /// and this is its first selection. Returns whether this selection was
    /// the last one the phase was waiting for.
    fn select(&mut self, seq: u64, from: &PlayerId, target: &PlayerId) -> bool {
        let counts =
            seq == self.seq && self.awake.contains(from) && !self.selections.contains_key(from);
        if counts {
            self.selections.insert(from.clone(), target.clone());
        }
        counts && self.selections.len() == self.awake.len()
    }

    /// End the phase numbered `seq`, unless it has already ended: resolve
    /// it from its selections under its vote, and move on to the next
    /// phase if the game goes on. Returns who died and who won, if anyone.
    fn end(&mut self, seq: u64) -> Option<Ended> {
        if seq != self.seq {
            return None;
        }
        let (hunt, dead) = match self.state.phase {
            Phase::Night => {
                let hunt = self
                    .state
                    .resolve_night(&self.selections, self.rules.night_vote.as_ref());
                let dead = hunt.as_ref().and_then(|hunt| hunt.dead().cloned());
                (hunt, dead)
            }
            Phase::Day => (
                None,
                self.state
                    .resolve_day(&self.selections, self.rules.day_vote.as_ref()),
            ),
        };
        let winner = self.state.winner();
        if winner.is_none() {
            self.state.next();
        }
        Some(Ended { hunt, dead, winner })
    }
}

/// How a phase ended: the hunt, if it was a night in which the werewolves
/// elected anyone, who died in it, and who won the game if anyone.
struct Ended {
    hunt: Option<Hunt>,
    dead: Option<PlayerId>,
    winner: Option<Team>,
}

/// A phase just opened: its number, the announcement for each awake
/// player, and how long it waits for them.
struct Opened {
    seq: u64,
    announcements: Vec<(PlayerId, Message)>,
    limit: Duration,
}

/// The actor that holds the game. Its perceive loop and its think loop are
/// each an `Environment` with a context of its own over the one shared
/// game: the perceive loop opens the first night and takes selections, and
/// the think loop ends phases. A variant's actor enum holds the perceive
/// loop's, built by [`init`].
pub struct Environment {
    context: Context<Message, Entry>,
    game: Arc<Mutex<Game>>,
    /// The entry the perceive loop logs right after the deal, if any.
    /// Taken when logged. Boxed to keep the environment no larger than a
    /// player in a variant's actor enum.
    configuration: Option<Box<Entry>>,
}

impl Environment {
    /// The game, locked for as long as the guard lives. Never held across
    /// an await.
    fn game(&self) -> MutexGuard<'_, Game> {
        self.game.lock().unwrap()
    }

    /// Open the next phase: show each awake player what it may see of it
    /// and how long it lasts, each announcement an action of its own, and
    /// set the timer that ends it at its limit.
    fn open_phase(&self) -> anyhow::Result<()> {
        let Opened {
            seq,
            announcements,
            limit,
        } = self.game().open()?;
        for (to, message) in announcements {
            act(&self.context, message, HashSet::from([to]), None)?;
        }
        self.context.think_after(limit, Message::EndPhase { seq })
    }

    /// End the phase numbered `seq`, unless it has already ended: log the
    /// hunt if there was one, stop whoever died, and then end the game or
    /// open the next phase.
    fn end_phase(&self, seq: u64) -> anyhow::Result<()> {
        let Some(Ended { hunt, dead, winner }) = self.game().end(seq) else {
            return Ok(());
        };
        if let Some(hunt) = hunt {
            self.context.log(Entry::Hunted { seq, hunt });
        }
        if let Some(player) = &dead {
            self.context.stop(player)?;
        }
        match winner {
            Some(team) => self.end_game(team),
            None => self.open_phase(),
        }
    }

    /// Stop the survivors, log the result, send the winner, and shut down.
    /// The survivors stop first: a player still waiting on its model is
    /// dropped at that wait, and logs nothing after the end.
    fn end_game(&self, winner: Team) -> anyhow::Result<()> {
        let (days, survivors, took, report) = {
            let mut game = self.game();
            (
                game.state.days(),
                game.state.alive.clone(),
                game.started.elapsed(),
                game.winner.take(),
            )
        };
        for player in &survivors {
            self.context.stop(player)?;
        }
        self.context.log(Entry::End {
            winner,
            days,
            survivors,
            took,
        });
        if let Some(report) = report {
            // The owner of the episode may have stopped listening.
            let _ = report.send(winner);
        }
        self.context.shutdown();
        Ok(())
    }
}

#[async_trait]
impl Behavior for Environment {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        &self.context
    }

    /// Log a selection as observed in the current phase, stale or not, and
    /// record it if it counts. The last selection a phase is waiting for
    /// ends it. Anything else is ignored.
    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        if let Message::Select {
            seq,
            from,
            n,
            target,
        } = message
        {
            let (current, last) = {
                let mut game = self.game();
                (game.seq, game.select(*seq, from, target))
            };
            observe(&self.context, from.clone(), *n, current);
            if last {
                self.context.think(Message::EndPhase { seq: *seq })?;
            }
        }
        Ok(())
    }

    /// Log the deal and the limits, then the configuration if there is
    /// one, and open the first night.
    async fn start(&mut self) -> anyhow::Result<()> {
        let start = {
            let game = self.game();
            game.rules.start(game.state.roles.clone())
        };
        self.context.log(start);
        if let Some(configuration) = self.configuration.take() {
            self.context.log(*configuration);
        }
        self.open_phase()
    }
}

#[async_trait]
impl Think for Environment {
    type Message = Message;

    /// End the phase the message says is over. Nothing else is thought
    /// about.
    async fn think(&mut self, message: &Message) -> anyhow::Result<()> {
        if let Message::EndPhase { seq } = message {
            self.end_phase(*seq)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::werewolf::tests::{one_wolf_against, village};

    fn id(name: &str) -> PlayerId {
        name.to_string()
    }

    /// The village, with its first night open.
    fn opened() -> (Game, Opened) {
        let (winner, _) = oneshot::channel();
        let mut game = Game::new(village().roles, Rules::default(), winner);
        let night = game.open().unwrap();
        (game, night)
    }

    /// Whom `opened` announces to, in name order, and the numbers of its
    /// announcements, in order.
    fn announced(opened: &Opened) -> (Vec<&PlayerId>, Vec<u64>) {
        let mut to: Vec<&PlayerId> = opened.announcements.iter().map(|(to, _)| to).collect();
        to.sort();
        let mut numbers: Vec<u64> = opened
            .announcements
            .iter()
            .map(|(_, message)| message.number().unwrap())
            .collect();
        numbers.sort_unstable();
        (to, numbers)
    }

    #[test]
    fn opening_a_phase_numbers_it_from_one_and_announces_it_to_the_awake() {
        let (mut game, night) = opened();
        assert_eq!(night.seq, 1);
        let (to, _) = announced(&night);
        assert_eq!(to, ["seer", "wolf1", "wolf2"]);
        assert_eq!(night.limit, Rules::default().night_limit);
        game.state.next();
        let day = game.open().unwrap();
        assert_eq!(day.seq, 2);
        assert_eq!(day.announcements.len(), 4);
        assert_eq!(day.limit, Rules::default().day_limit);
    }

    #[test]
    fn each_announcement_is_numbered_in_one_series_from_one_and_carries_its_phase_and_limit() {
        let (mut game, night) = opened();
        assert_eq!(announced(&night).1, [1, 2, 3]);
        game.state.next();
        let day = game.open().unwrap();
        assert_eq!(announced(&day).1, [4, 5, 6, 7]);
        for (_, message) in &day.announcements {
            assert!(
                matches!(message, Message::Announce { seq: 2, limit, .. }
                    if *limit == Rules::default().day_limit),
                "{message:?}"
            );
        }
    }

    #[test]
    fn the_last_selection_of_a_phase_ends_it() {
        let (mut game, Opened { seq, .. }) = opened();
        assert!(!game.select(seq, &id("wolf1"), &id("villager")));
        assert!(!game.select(seq, &id("wolf2"), &id("villager")));
        assert!(game.select(seq, &id("seer"), &id("wolf1")));
    }

    #[test]
    fn a_selection_for_another_phase_does_not_count() {
        let (mut game, Opened { seq, .. }) = opened();
        assert!(!game.select(seq - 1, &id("wolf1"), &id("villager")));
        assert!(!game.select(seq + 1, &id("wolf1"), &id("villager")));
        assert!(game.selections.is_empty());
    }

    #[test]
    fn a_selection_by_a_player_who_is_not_awake_does_not_count() {
        let (mut game, Opened { seq, .. }) = opened();
        assert!(!game.select(seq, &id("villager"), &id("wolf1")));
        assert!(game.selections.is_empty());
    }

    #[test]
    fn only_a_players_first_selection_in_a_phase_counts() {
        let (mut game, Opened { seq, .. }) = opened();
        game.select(seq, &id("wolf1"), &id("villager"));
        game.select(seq, &id("wolf2"), &id("villager"));
        assert!(!game.select(seq, &id("wolf1"), &id("seer")));
        assert_eq!(game.selections[&id("wolf1")], id("villager"));
    }

    #[test]
    fn a_phase_is_resolved_from_its_selections_under_its_vote() {
        let (winner, _) = oneshot::channel();
        let roles = one_wolf_against(&["ann", "bob", "cat"]);
        let mut game = Game::new(roles, Rules::default(), winner);
        let seq = game.open().unwrap().seq;
        game.select(seq, &id("wolf"), &id("ann"));
        let night = game.end(seq).unwrap();
        assert_eq!(night.dead, Some(id("ann")));
        let hunt = night.hunt.unwrap();
        assert_eq!(hunt.victim, id("ann"));
        assert!(hunt.protectors.is_empty());
        assert_eq!(night.winner, None);
        // The game goes on, so the next phase is the day.
        let day = game.open().unwrap();
        assert_eq!(day.seq, 2);
        assert_eq!(day.limit, Rules::default().day_limit);
        game.select(day.seq, &id("wolf"), &id("bob"));
        game.select(day.seq, &id("bob"), &id("wolf"));
        let tied = game.end(day.seq).unwrap();
        assert_eq!(tied.hunt, None);
        assert_eq!(tied.dead, None);
        assert_eq!(tied.winner, None);
    }

    #[test]
    fn a_phase_that_has_ended_does_not_end_again() {
        let (mut game, Opened { seq, .. }) = opened();
        assert!(game.end(seq).is_some());
        game.open().unwrap();
        assert!(game.end(seq).is_none());
    }

    #[test]
    fn a_phase_that_decides_the_game_is_the_last() {
        let (mut game, Opened { seq, .. }) = opened();
        game.state.alive.remove("wolf2");
        game.select(seq, &id("wolf1"), &id("villager"));
        let night = game.end(seq).unwrap();
        assert_eq!(night.dead, Some(id("villager")));
        assert_eq!(night.winner, Some(Team::Werewolves));
        assert_eq!(game.state.days(), 0);
    }
}
