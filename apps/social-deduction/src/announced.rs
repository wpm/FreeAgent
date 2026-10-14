//! The announcing environment, which runs every game: it opens each phase
//! by announcing it to each seat the game names, takes the players' moves
//! as they come, and ends the phase at its limit or once the game says
//! the last move it waits for has come. It then has the game resolve the
//! phase, and opens the next one or ends the game. Players who take
//! seconds to decide hold nothing up.
//!
//! An [`Announced`] game supplies only its rules: what each phase shows
//! whom and how long it lasts, which moves count, and what a phase comes
//! to. A move counts only in the phase underway, and the game's own timers
//! reach it as moves too.
//!
//! The environment logs the deal, then the configuration; an
//! [`Entry::Action`] for each announcement and each message of its own it
//! sends, numbered in one series from 1; an [`Entry::Observation`] for each
//! move it receives from a player, counted or not, with the phase it was in
//! when it arrived; how each phase came out, as the game records it; and the
//! game's end. The reminders it sends itself are not logged.

use crate::game::{Actor, Entry, Game, Message, Tagged, act, observe};
use crate::seat::{Dealt, PlayerId};
use async_trait::async_trait;
use free_agent::{ActorInit, Behavior, Context, Think};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::Instant;

/// A game the announcing environment runs: the rules it supplies for each
/// phase.
pub trait Announced: Game<Environment = Environment<Self>> {
    /// What the environment holds of the game.
    type State: Send + 'static;
    /// What the environment tracks of the phase underway.
    type Underway: Send + 'static;

    /// The game of `roles`, before its first phase.
    fn state(roles: HashMap<PlayerId, Self::Role>) -> Self::State;

    /// The phase `core` is at, about to open: each seat it is announced
    /// to, in seat order, with what that seat may see of it; how long it
    /// lasts; and what the environment tracks of it.
    fn open(core: &Core<Self>) -> anyhow::Result<Opening<Self>>;

    /// Count `played`, a move in the phase underway, if the rules let it
    /// count. Returns what the environment does about a move that counted,
    /// and nothing for one that did not.
    fn counts(
        core: &mut Core<Self>,
        underway: &mut Self::Underway,
        played: &Self::Move,
    ) -> Option<Counted<Self>>;

    /// What the phase underway comes to, now that it has ended with
    /// `underway`, moving `core` on to the next phase if the game goes on.
    fn resolve(core: &mut Core<Self>, underway: Self::Underway) -> Resolved<Self>;
}

/// What the environment's two loops share: the game, the phase it is at,
/// and where the winner goes.
pub struct Core<G: Announced> {
    /// The game.
    pub state: G::State,
    /// How it is played.
    pub rules: G::Rules,
    /// The number of the current phase. The first is 1.
    pub seq: u64,
    /// The number of the last message the environment sent to a player.
    /// Each announcement is a message of its own.
    pub n: u64,
    /// The phase underway, while one is: taken when it ends, so that
    /// nothing more counts until the next one opens.
    pub underway: Option<G::Underway>,
    /// Where the winning team goes when the game ends. Taken when sent.
    winner: Option<oneshot::Sender<G::Team>>,
    /// When the game was made, which is as good as when it was dealt.
    started: Instant,
}

/// A phase about to open, as the game opens it.
pub struct Opening<G: Announced> {
    /// Each seat it is announced to, in seat order, with what that seat
    /// may see of it.
    pub views: Vec<(PlayerId, G::View)>,
    /// How long it lasts.
    pub limit: Duration,
    /// What the environment tracks of it.
    pub underway: G::Underway,
}

/// What the environment does about a move that counted.
#[derive(Debug, PartialEq)]
pub struct Counted<G: Game> {
    /// The move was the last the phase was waiting for: end it.
    pub last: bool,
    /// A message of the environment's own to send, and to whom.
    pub tell: Option<(Message<G>, HashSet<PlayerId>)>,
    /// A reminder for the environment to send itself after a while.
    pub remind: Option<(Duration, Message<G>)>,
}

impl<G: Game> Default for Counted<G> {
    /// A move that counted and calls for nothing more.
    fn default() -> Self {
        Self {
            last: false,
            tell: None,
            remind: None,
        }
    }
}

/// What a phase came to.
pub struct Resolved<G: Game> {
    /// What the log records of it, if anything.
    pub outcome: Option<G::Event>,
    /// A player it put out of the game, stopped at once.
    pub out: Option<PlayerId>,
    /// How the game ended, if the phase ended it.
    pub over: Option<Over<G>>,
}

/// How a game ended.
pub struct Over<G: Game> {
    /// The team that won.
    pub winner: G::Team,
    /// The players still playing, stopped before the end is logged.
    pub stopped: Vec<PlayerId>,
    /// The end, as the log records it.
    pub end: G::Event,
}

/// The players a phase awaits, and the first choice each has made in it.
pub struct Awaited<C> {
    /// The players the phase awaits.
    pub players: HashSet<PlayerId>,
    /// Each awaited player's first choice to count.
    pub choices: HashMap<PlayerId, C>,
}

impl<C> Awaited<C> {
    /// A phase awaiting `players`, none of whom has chosen.
    pub fn new(players: HashSet<PlayerId>) -> Self {
        Self {
            players,
            choices: HashMap::new(),
        }
    }

    /// Whether every awaited player has chosen.
    pub fn all(&self) -> bool {
        self.choices.len() == self.players.len()
    }

    /// Count `from`'s `choice` if the phase awaits `from` and it is its
    /// first. Returns what the environment does about a choice that
    /// counted, ending the phase at the last, and nothing for one that did
    /// not.
    pub fn first<G: Game>(&mut self, from: &PlayerId, choice: C) -> Option<Counted<G>> {
        if !self.players.contains(from) || self.choices.contains_key(from) {
            return None;
        }
        self.choices.insert(from.clone(), choice);
        Some(Counted {
            last: self.all(),
            ..Counted::default()
        })
    }
}

impl<G: Announced> Core<G> {
    /// A game of `roles` under `rules`, before its first phase. The
    /// winning team is sent on `winner` when the game ends.
    pub fn new(
        roles: HashMap<PlayerId, G::Role>,
        rules: G::Rules,
        winner: oneshot::Sender<G::Team>,
    ) -> Self {
        Self {
            state: G::state(roles),
            rules,
            seq: 0,
            n: 0,
            underway: None,
            winner: Some(winner),
            started: Instant::now(),
        }
    }

    /// The number of the environment's next message to a player.
    pub fn number(&mut self) -> u64 {
        self.n += 1;
        self.n
    }

    /// How long the game has taken so far.
    pub fn took(&self) -> Duration {
        self.started.elapsed()
    }

    /// Open the next phase: number it, write each seat its announcement,
    /// numbered in turn in seat order, and track it.
    pub fn open(&mut self) -> anyhow::Result<Opened<G>> {
        self.seq += 1;
        let Opening {
            views,
            limit,
            underway,
        } = G::open(self)?;
        let announcements = views
            .into_iter()
            .map(|(to, view)| {
                let message = Message::Announce {
                    seq: self.seq,
                    n: self.number(),
                    view,
                    limit,
                };
                (to, message)
            })
            .collect();
        self.underway = Some(underway);
        Ok(Opened {
            seq: self.seq,
            announcements,
            limit,
        })
    }

    /// Count `played` if it is for the phase underway and the rules let it
    /// count. Returns what the environment does about a move that counted,
    /// and nothing for one that did not.
    pub fn count(&mut self, played: &G::Move) -> Option<Counted<G>> {
        if played.seq() != self.seq {
            return None;
        }
        let mut underway = self.underway.take()?;
        let counted = G::counts(self, &mut underway, played);
        self.underway = Some(underway);
        counted
    }

    /// End the phase numbered `seq`, unless it has already ended. Returns
    /// what it came to, or nothing if it had already.
    pub fn end(&mut self, seq: u64) -> Option<Resolved<G>> {
        if seq != self.seq {
            return None;
        }
        let underway = self.underway.take()?;
        Some(G::resolve(self, underway))
    }
}

/// A phase just opened: its number, the announcement for each seat, and
/// how long it lasts.
pub struct Opened<G: Game> {
    /// The number of the phase.
    pub seq: u64,
    /// Each seat's announcement, in seat order.
    pub announcements: Vec<(PlayerId, Message<G>)>,
    /// How long it lasts.
    pub limit: Duration,
}

/// Everything the episode needs to run the environment for a game of
/// `deal`, every player's role and strategy, under `rules`: it may send to
/// and stop every player, holds the logger, and thinks. It logs the deal,
/// then `configuration`, before it opens the first phase. The winning team
/// is sent on `winner` when the game ends.
pub fn init<G: Announced>(
    deal: HashMap<PlayerId, Dealt<G::Role>>,
    rules: G::Rules,
    configuration: Entry<G>,
    winner: oneshot::Sender<G::Team>,
) -> ActorInit<Actor<G>> {
    let players: HashSet<PlayerId> = deal.keys().cloned().collect();
    let opening = Box::new([Entry::start(&deal, &rules), configuration]);
    let roles = deal
        .into_iter()
        .map(|(id, dealt)| (id, dealt.role))
        .collect();
    let core = Arc::new(Mutex::new(Core::new(roles, rules, winner)));
    let shared = Arc::clone(&core);
    ActorInit {
        behavior: Box::new(move |context| {
            Actor::Environment(Environment {
                context,
                core,
                opening: Some(opening),
            })
        }),
        think: Some(Box::new(move |context| {
            Box::new(Environment {
                context,
                core: shared,
                opening: None,
            })
        })),
        can_send_to: players.clone(),
        can_shut_down: players,
        has_logger: true,
    }
}

/// The actor that holds the game. Its perceive loop and its think loop are
/// each an `Environment` with a context of its own over the one shared
/// game: the perceive loop opens the first phase and takes the players'
/// moves, and the think loop takes the environment's own reminders and
/// ends phases. [`Actor::Environment`] holds the perceive loop's, built by
/// [`init`].
pub struct Environment<G: Announced> {
    context: Context<Message<G>, Entry<G>>,
    core: Arc<Mutex<Core<G>>>,
    /// The entries the perceive loop opens the log with: the deal, then
    /// the configuration. Taken when logged. Boxed to keep the environment
    /// no larger than a player in [`Actor`].
    opening: Option<Box<[Entry<G>; 2]>>,
}

impl<G: Announced> Environment<G> {
    /// The game, locked for as long as the guard lives. Never held across
    /// an await.
    fn core(&self) -> MutexGuard<'_, Core<G>> {
        self.core.lock().unwrap()
    }

    /// Open the next phase: show each seat what it may see of it and how
    /// long it lasts, each announcement an action of its own, and set the
    /// timer that ends it at its limit.
    fn open_phase(&self) -> anyhow::Result<()> {
        let Opened {
            seq,
            announcements,
            limit,
        } = self.core().open()?;
        for (to, message) in announcements {
            act(&self.context, message, HashSet::from([to]), None, None)?;
        }
        self.context.think_after(limit, Message::EndPhase { seq })
    }

    /// Carry out what a move that counted in the phase numbered `seq`
    /// calls for, if it counted: send the environment's message, set its
    /// reminder, and end the phase after the last move it waits for.
    fn carry_out(&self, seq: u64, counted: Option<Counted<G>>) -> anyhow::Result<()> {
        let Some(Counted { last, tell, remind }) = counted else {
            return Ok(());
        };
        if let Some((message, to)) = tell {
            act(&self.context, message, to, None, None)?;
        }
        if let Some((after, reminder)) = remind {
            self.context.think_after(after, reminder)?;
        }
        if last {
            self.context.think(Message::EndPhase { seq })?;
        }
        Ok(())
    }

    /// Carry out the end of a phase, if `ended` says it did: stop whoever
    /// it put out, log what it came to, and then end the game or open the
    /// next phase.
    fn end_phase(&self, ended: Option<Resolved<G>>) -> anyhow::Result<()> {
        let Some(Resolved { outcome, out, over }) = ended else {
            return Ok(());
        };
        if let Some(out) = out {
            self.context.stop(&out)?;
        }
        if let Some(outcome) = outcome {
            self.context.log(Entry::Event(outcome));
        }
        match over {
            Some(over) => self.end_game(over),
            None => self.open_phase(),
        }
    }

    /// Stop the players still playing, log the end, send the winner, and
    /// shut down. The players stop first: a player still waiting on its
    /// model is dropped at that wait, and logs nothing after the end.
    fn end_game(&self, over: Over<G>) -> anyhow::Result<()> {
        let Over {
            winner,
            stopped,
            end,
        } = over;
        for player in &stopped {
            self.context.stop(player)?;
        }
        self.context.log(Entry::Event(end));
        if let Some(report) = self.core().winner.take() {
            // The owner of the episode may have stopped listening.
            let _ = report.send(winner);
        }
        self.context.shutdown();
        Ok(())
    }
}

#[async_trait]
impl<G: Announced> Behavior for Environment<G> {
    type Message = Message<G>;
    type Log = Entry<G>;

    fn context(&self) -> &Context<Message<G>, Entry<G>> {
        &self.context
    }

    /// Log a player's move as observed in the current phase, counted or
    /// not, and carry out what it calls for if it counts. Anything else is
    /// ignored.
    async fn receive(&mut self, message: &Message<G>) -> anyhow::Result<()> {
        let (Message::Move(played), Some(n)) = (message, message.number()) else {
            return Ok(());
        };
        let (current, counted) = {
            let mut core = self.core();
            (core.seq, core.count(played))
        };
        observe(&self.context, played.sender(), n, current);
        self.carry_out(current, counted)
    }

    /// Log the deal and the limits, then the configuration, and open the
    /// first phase.
    async fn start(&mut self) -> anyhow::Result<()> {
        for entry in self.opening.take().into_iter().flat_map(|opening| *opening) {
            self.context.log(entry);
        }
        self.open_phase()
    }
}

#[async_trait]
impl<G: Announced> Think for Environment<G> {
    type Message = Message<G>;

    /// End the phase the message says is over, unless it has ended
    /// already, and carry out a reminder of the game's own that counts.
    /// Nothing else is thought about.
    async fn think(&mut self, message: &Message<G>) -> anyhow::Result<()> {
        if let Message::EndPhase { seq } = message {
            let ended = self.core().end(*seq);
            self.end_phase(ended)?;
        }
        if let Message::Move(reminder) = message {
            let (current, counted) = {
                let mut core = self.core();
                (core.seq, core.count(reminder))
            };
            self.carry_out(current, counted)?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::avalon::{self, Avalon};
    use crate::werewolf::{self, Werewolf};

    /// A move that counted and calls for nothing more.
    pub(crate) fn counted<G: Game>() -> Option<Counted<G>> {
        Some(Counted::default())
    }

    /// A move that counted and was the last the phase waited for.
    pub(crate) fn last<G: Game>() -> Option<Counted<G>> {
        Some(Counted {
            last: true,
            ..Counted::default()
        })
    }

    /// Open the first phase of a game of `roles` under `rules`, end it,
    /// open the next, and end the first again.
    fn ended_twice<G: Announced>(roles: HashMap<PlayerId, G::Role>, rules: G::Rules) {
        let (winner, _) = oneshot::channel();
        let mut core = Core::<G>::new(roles, rules, winner);
        let seq = core.open().unwrap().seq;
        assert!(core.end(seq).is_some());
        core.open().unwrap();
        assert!(core.end(seq).is_none());
    }

    #[test]
    fn a_phase_that_has_ended_does_not_end_again() {
        ended_twice::<Werewolf>(
            werewolf::tests::one_wolf_against(&["ann", "bob", "cat"]),
            werewolf::Rules::default(),
        );
        ended_twice::<Avalon>(avalon::tests::five(), avalon::Rules::default());
    }
}
