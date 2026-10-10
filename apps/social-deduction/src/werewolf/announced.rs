//! The announcing environment, shared by the scripted and model-played
//! variants: it opens each phase with a statement to every awake player,
//! takes their selections as statements as they come, and ends the phase
//! once everyone awake has selected or at its limit, whichever comes
//! first. Players who take seconds to decide hold nothing up.

use super::{Choices, Entry, Message, Observation, Phase, PlayerId, Role, Rules, State, Team};
use async_trait::async_trait;
use free_agent::{ActorInit, Behavior, Context, Think};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::oneshot;

/// The environment's name in the episode.
pub const ENVIRONMENT: &str = "environment";

/// Everything the episode needs to run the environment for a game of
/// `roles` under `rules`, as the actor `wrap` makes of it: it may send to
/// and stop every player, holds the logger, and thinks. The winning team
/// is sent on `winner` when the game ends.
pub fn init<A>(
    roles: HashMap<PlayerId, Role>,
    rules: Rules,
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
        behavior: Box::new(move |context| wrap(Environment { context, game })),
        think: Some(Box::new(move |context| {
            Box::new(Environment {
                context,
                game: shared,
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
    /// The players awake in the current phase.
    awake: HashSet<PlayerId>,
    /// The first selection each awake player made in the current phase.
    selections: Choices,
    /// Where the winning team goes when the game ends. Taken when sent.
    winner: Option<oneshot::Sender<Team>>,
}

impl Game {
    /// A game of `roles` under `rules`, before its first night. The
    /// winning team is sent on `winner` when the game ends.
    fn new(roles: HashMap<PlayerId, Role>, rules: Rules, winner: oneshot::Sender<Team>) -> Self {
        Self {
            state: State::new(roles),
            rules,
            seq: 0,
            awake: HashSet::new(),
            selections: Choices::new(),
            winner: Some(winner),
        }
    }

    /// Open the next phase: number it, wake its players, and clear the
    /// selections.
    fn open(&mut self) -> anyhow::Result<Opened> {
        self.seq += 1;
        self.awake = self.state.awake();
        self.selections.clear();
        let shown = self
            .awake
            .iter()
            .map(|player| Ok((player.clone(), self.state.observation(player.clone())?)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let limit = match self.state.phase {
            Phase::Night => self.rules.night_limit,
            Phase::Day => self.rules.day_limit,
        };
        Ok(Opened {
            seq: self.seq,
            shown,
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

    /// Resolve the current phase from its selections under its vote.
    /// Returns who died.
    fn resolve(&mut self) -> Option<PlayerId> {
        match self.state.phase {
            Phase::Night => self
                .state
                .resolve_night(&self.selections, self.rules.night_vote.as_ref()),
            Phase::Day => self
                .state
                .resolve_day(&self.selections, self.rules.day_vote.as_ref()),
        }
    }
}

/// A phase just opened: its number, what each awake player may see of it,
/// and how long it waits for them.
struct Opened {
    seq: u64,
    shown: Vec<(PlayerId, Observation)>,
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
}

impl Environment {
    /// The game, locked for as long as the guard lives. Never held across
    /// an await.
    fn game(&self) -> MutexGuard<'_, Game> {
        self.game.lock().unwrap()
    }

    /// Open the next phase: show each awake player what it may see of it,
    /// and set the timer that ends it at its limit.
    fn open_phase(&self) -> anyhow::Result<()> {
        let Opened { seq, shown, limit } = self.game().open()?;
        for (to, observation) in shown {
            let message = Message::Announce { seq, observation };
            self.context.log(Entry::Sent {
                to: to.clone(),
                message: message.clone(),
            });
            self.context.send(message, HashSet::from([to]))?;
        }
        self.context.think_after(limit, Message::EndPhase { seq })
    }

    /// End the phase numbered `seq`, unless it has already ended: resolve
    /// it, stop whoever died, and then end the game or open the next
    /// phase.
    fn end_phase(&self, seq: u64) -> anyhow::Result<()> {
        let (dead, winner) = {
            let mut game = self.game();
            if game.seq != seq {
                return Ok(());
            }
            let dead = game.resolve();
            (dead, game.state.winner())
        };
        if let Some(player) = &dead {
            self.context.stop(player)?;
        }
        match winner {
            Some(team) => self.end_game(team),
            None => {
                self.game().state.next();
                self.open_phase()
            }
        }
    }

    /// Log the result, stop the survivors, send the winner, and shut down.
    fn end_game(&self, winner: Team) -> anyhow::Result<()> {
        let (days, survivors, report) = {
            let mut game = self.game();
            (
                game.state.days(),
                game.state.alive.clone(),
                game.winner.take(),
            )
        };
        self.context.log(Entry::End {
            winner,
            days,
            survivors: survivors.clone(),
        });
        for player in &survivors {
            self.context.stop(player)?;
        }
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

    /// Log the deal and open the first night.
    async fn start(&mut self) -> anyhow::Result<()> {
        let (variation, roles) = {
            let game = self.game();
            (game.rules.variation.clone(), game.state.roles.clone())
        };
        self.context.log(Entry::Start { variation, roles });
        self.open_phase()
    }

    /// Log a selection, stale or not, and record it if it counts. The last
    /// selection a phase is waiting for ends it. Anything else is ignored.
    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        if let Message::Select { seq, from, target } = message {
            self.context.log(Entry::Received {
                from: from.clone(),
                message: message.clone(),
            });
            let last = self.game().select(*seq, from, target);
            if last {
                self.context.think(Message::EndPhase { seq: *seq })?;
            }
        }
        Ok(())
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
    use crate::werewolf::tests::village;

    fn id(name: &str) -> PlayerId {
        name.to_string()
    }

    /// The village, with its first night open.
    fn opened() -> (Game, u64) {
        let (winner, _) = oneshot::channel();
        let mut game = Game::new(village().roles, Rules::default(), winner);
        let seq = game.open().unwrap().seq;
        (game, seq)
    }

    #[test]
    fn opening_a_phase_numbers_it_from_one_and_shows_it_to_the_awake() {
        let (winner, _) = oneshot::channel();
        let mut game = Game::new(village().roles, Rules::default(), winner);
        let night = game.open().unwrap();
        assert_eq!(night.seq, 1);
        let mut shown: Vec<PlayerId> = night.shown.into_iter().map(|(player, _)| player).collect();
        shown.sort();
        assert_eq!(shown, ["seer", "wolf1", "wolf2"]);
        assert_eq!(night.limit, Rules::default().night_limit);
        game.state.next();
        let day = game.open().unwrap();
        assert_eq!(day.seq, 2);
        assert_eq!(day.shown.len(), 4);
        assert_eq!(day.limit, Rules::default().day_limit);
    }

    #[test]
    fn the_last_selection_of_a_phase_ends_it() {
        let (mut game, seq) = opened();
        assert!(!game.select(seq, &id("wolf1"), &id("villager")));
        assert!(!game.select(seq, &id("wolf2"), &id("villager")));
        assert!(game.select(seq, &id("seer"), &id("wolf1")));
    }

    #[test]
    fn a_selection_for_another_phase_does_not_count() {
        let (mut game, seq) = opened();
        assert!(!game.select(seq - 1, &id("wolf1"), &id("villager")));
        assert!(!game.select(seq + 1, &id("wolf1"), &id("villager")));
        assert!(game.selections.is_empty());
    }

    #[test]
    fn a_selection_by_a_player_who_is_not_awake_does_not_count() {
        let (mut game, seq) = opened();
        assert!(!game.select(seq, &id("villager"), &id("wolf1")));
        assert!(game.selections.is_empty());
    }

    #[test]
    fn only_a_players_first_selection_in_a_phase_counts() {
        let (mut game, seq) = opened();
        game.select(seq, &id("wolf1"), &id("villager"));
        game.select(seq, &id("wolf2"), &id("villager"));
        assert!(!game.select(seq, &id("wolf1"), &id("seer")));
        assert_eq!(game.selections[&id("wolf1")], id("villager"));
    }

    #[test]
    fn a_phase_is_resolved_from_its_selections_under_its_vote() {
        let (mut game, seq) = opened();
        game.select(seq, &id("wolf1"), &id("villager"));
        game.select(seq, &id("wolf2"), &id("villager"));
        assert_eq!(game.resolve(), Some(id("villager")));
        game.state.next();
        let seq = game.open().unwrap().seq;
        game.select(seq, &id("wolf1"), &id("seer"));
        game.select(seq, &id("seer"), &id("wolf1"));
        assert_eq!(game.resolve(), None);
    }
}
