//! The random policy, of any game: a player who answers each announcement
//! of a phase it acts in at once with a move drawn at random, each word
//! that a phase is closing at once by closing it, and does nothing about
//! the rest. A player logs an [`Entry::Observation`] for
//! each message it receives, in the latest phase it was announced, and an
//! [`Entry::Action`] for each message it sends, numbering them from 1; no
//! model made them, so `seen` is `None`.

use crate::game::{Actor, Entry, Game, Message, Seat, act, close, observe};
use async_trait::async_trait;
use free_agent::{Behavior, Builder, Context};

/// A player who answers each announcement of a phase it
/// [acts](Game::acts) in at once, with the move its game
/// [picks](Game::pick), and does nothing about one it does not act in.
/// Its move goes to the game's [`receivers`](Game::receivers) of it. Told
/// a phase is closing, it closes it at once: its one move was made when the
/// phase was announced. It has no think loop.
pub struct Player<G: Game> {
    context: Context<Message<G>, Entry<G>>,
    /// The number of the last message it sent.
    n: u64,
    /// The number of the latest phase it was announced, and 0 before any.
    seq: u64,
}

impl<G: Game> Player<G> {
    /// Builds a player once the episode has made its context. It has no
    /// think loop.
    pub fn init() -> Seat<G> {
        let behavior: Builder<Actor<G>> = Box::new(|context| {
            Actor::Random(Player {
                context,
                n: 0,
                seq: 0,
            })
        });
        (behavior, None)
    }
}

#[async_trait]
impl<G: Game> Behavior for Player<G> {
    type Message = Message<G>;
    type Log = Entry<G>;

    fn context(&self) -> &Context<Message<G>, Entry<G>> {
        &self.context
    }

    /// Log a message from another actor, which is any numbered one, as
    /// observed in the latest phase announced. Announced a phase it acts
    /// in, move at once; otherwise, or with no move to make, say nothing.
    /// Told a phase is closing, close it. Anything else, another player's
    /// utterance or move above all, is
    /// observed and nothing more, whatever phase it is tagged with: the
    /// player keeps nothing it could put it in.
    async fn receive(&mut self, message: &Message<G>) -> anyhow::Result<()> {
        let Some(n) = message.number() else {
            return Ok(());
        };
        if let Message::Announce { seq, .. } = message {
            self.seq = *seq;
        }
        observe(&self.context, message.sender(), n, self.seq);
        let me = &self.context.id;
        if let Message::Announce { seq, view, .. } = message
            && G::acts(me, view)
            && let Some(picked) = G::pick(me, view, *seq, self.n + 1)
        {
            self.n += 1;
            let message = Message::Move(picked);
            let to = G::receivers(&message, me, view);
            act(&self.context, message, to, None, None)?;
        }
        if let Message::Closing { seq, .. } = message {
            self.n += 1;
            close(&self.context, *seq, self.n)?;
        }
        Ok(())
    }
}
