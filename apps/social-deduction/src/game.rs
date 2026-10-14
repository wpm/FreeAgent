//! What every game is made of. A [`Game`] names its own types and supplies
//! the hooks the shared actors call. What the environment and the players
//! say to one another is a [`Message`], and what the log holds an
//! [`Entry`]: each holds what every game has alike, with a variant for
//! what is the game's own. An episode holds one kind of actor, [`Actor`],
//! which is the environment or a player of either policy, and [`game`]
//! builds every episode, with each seat's player built for it.
//!
//! Every message an actor sends to others is numbered by its sender, from
//! 1, so that the pair of sender and number names a message in an
//! episode. A reminder an actor sends itself is not numbered. The log is
//! written by every actor, an [`Entry`] at a time, in the language of
//! reinforcement learning: each message an actor sends to others is an
//! action of its sender, which the sender logs whole, once, and an
//! observation of each receiver, which the receiver logs as a receipt
//! naming the message by its sender and number.

use crate::config::Configuration;
use crate::model::spec::Probed;
use crate::model::{Model, ToolChoice};
use crate::prompted::{self, Account};
use crate::random;
use crate::report::Line;
use crate::seat::{Dealt, ENVIRONMENT, PlayerId, to_everyone_but};
use async_trait::async_trait;
use free_agent::{ActorId, ActorInit, Behavior, Context, Episode, Logger};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::time::Duration;
use tokio::sync::oneshot;

/// What a log or a message carries: it is cloned, compared, shown,
/// written and read back, and travels between actors.
pub trait Carried:
    Clone + Debug + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static
{
}

impl<T: Clone + Debug + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static> Carried
    for T
{
}

/// A social deduction game: the types it is written in, and what the
/// shared actors ask of it. A game is a type with no values, which names
/// the rest.
pub trait Game: Debug + Clone + PartialEq + Send + Sync + 'static {
    /// The game's name, as a player is told it.
    const NAME: &'static str;
    /// What a player is. The roles are ordered as they are dealt.
    type Role: Copy + Eq + Ord + Hash + Display + Carried;
    /// The sides, one of which wins.
    type Team: Send + 'static;
    /// How a game is played.
    type Rules: Send + 'static;
    /// How long each phase waits for the players, as the log records it.
    type Limits: Carried;
    /// What a player is shown of the game.
    type View: Carried;
    /// The messages only this game has.
    type Move: Tagged + Carried;
    /// The log entries only this game has, such as its end.
    type Event: Carried;
    /// A prompted player's account of its own game.
    type Story: Account<Self> + Send + 'static;
    /// What tells the game as it happens, from the log.
    type Narrator: Narrate<Self>;
    /// The actor that holds the game.
    type Environment: Behavior<Message = Message<Self>, Log = Entry<Self>> + 'static;
    /// A player who says whatever its script says.
    #[cfg(test)]
    type Puppet: Behavior<Message = Message<Self>, Log = Entry<Self>> + 'static;

    /// How long each phase of a game under `rules` waits.
    fn limits(rules: &Self::Rules) -> Self::Limits;

    /// The rules a game is played under: the default rules, with `limits`.
    fn rules(limits: &Self::Limits) -> Self::Rules;

    /// The variables a system prompt is rendered with for `role`, besides
    /// the seat's `name` and the text blocks.
    fn variables(role: Self::Role) -> minijinja::Value;

    /// Everything the episode needs to run the environment for a game of
    /// `deal` under `rules`: it may send to and stop every player, holds
    /// the logger, and thinks. It logs the deal, then `configuration`,
    /// before it opens the first phase, and sends the winning team on
    /// `winner` when the game ends.
    fn environment(
        deal: HashMap<PlayerId, Dealt<Self::Role>>,
        rules: Self::Rules,
        configuration: Entry<Self>,
        winner: oneshot::Sender<Self::Team>,
    ) -> ActorInit<Actor<Self>>;

    /// Whether a random player `me` acts in the phase `view` shows.
    fn acts(me: &PlayerId, view: &Self::View) -> bool;

    /// The move a random player `me` makes in the phase numbered `seq`,
    /// which `view` shows, as its `n`th message: drawn at random from
    /// those it may make, and none when it may make none.
    fn pick(me: &PlayerId, view: &Self::View, seq: u64, n: u64) -> Option<Self::Move>;

    /// Whom `me` sends `message` to in the phase `view` shows, by the rules
    /// of the game.
    fn receivers(message: &Message<Self>, me: &PlayerId, view: &Self::View) -> HashSet<ActorId>;

    /// The story `me` tells of its own game under `limits`, with nothing
    /// in it yet.
    fn story(me: PlayerId, limits: &Self::Limits) -> Self::Story;

    /// Whether `message`, which a prompted player `me` remembers in the
    /// phase `view` shows, wakes it.
    fn wakes(me: &PlayerId, message: &Message<Self>, view: &Self::View) -> bool;

    /// What a prompted `player` does in the phase numbered `seq`, which
    /// `view` shows and which lasts `limit`, when it is woken: ask the
    /// model, and carry out what it answers. An error is the player's
    /// failure to act.
    fn play(
        player: &prompted::Player<Self>,
        seq: u64,
        view: &Self::View,
        limit: Duration,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;

    /// The most a request to the model `model_id` from `seat`, dealt `role`
    /// and told `prompt`, takes of the context window before its story, by
    /// estimate, at a table of `seats` under `limits`.
    fn unstoried(
        model_id: &str,
        prompt: &str,
        seat: &PlayerId,
        role: Self::Role,
        seats: &[PlayerId],
        limits: &Self::Limits,
    ) -> anyhow::Result<u64>;

    /// Check that `model`, asked as a player is, plays, and find out
    /// whether it may be made to call a tool, asking it first under `from`.
    fn check_plays(
        model: &dyn Model,
        model_id: &str,
        from: ToolChoice,
    ) -> impl Future<Output = anyhow::Result<Probed>>;
}

/// Turns the events of a game, one at a time, into lines of prose.
pub trait Narrate<G: Game>: Default + Send + 'static {
    /// The lines the log's `event` adds to the account, often none.
    fn narrate(&mut self, event: &free_agent::Event<Entry<G>>) -> Vec<Line>;
}

/// What the shared [`Message`] asks of a game's own moves: each one's
/// number, its phase, and who sent it.
pub trait Tagged {
    /// The sender's number for this move, which a reminder to itself does
    /// not have.
    fn number(&self) -> Option<u64>;
    /// The phase this move is tagged with.
    fn seq(&self) -> u64;
    /// Who sent this move.
    fn sender(&self) -> ActorId;
}

/// What the environment and the players say to one another: what every
/// game has alike, and the game's own moves. The environment numbers its
/// announcements in one series with its own moves, each player its
/// utterances and moves together. Each announcement is a message of its
/// own, since each player is shown a different view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "")]
pub enum Message<G: Game> {
    /// The environment opens a phase for one player, whether the phase
    /// waits on it or not. Phases are numbered from 1, one up for each
    /// phase.
    Announce {
        /// The number of the phase.
        seq: u64,
        /// The environment's number for this message.
        n: u64,
        /// What the player may see of it.
        view: G::View,
        /// How long the phase lasts, written as `humantime` writes it. A
        /// player holds no state but its history, and can see when its
        /// messages arrived, so the limit is all it needs to know the time.
        #[serde(with = "humantime_serde")]
        limit: Duration,
    },
    /// A player's utterance in the phase numbered `seq`, as the speaker
    /// knew it: the latest phase the speaker was announced. It goes to
    /// other players, and never to the environment, which does nothing
    /// with speech.
    Say {
        /// The number of the phase.
        seq: u64,
        /// Who spoke, since a message does not carry its sender.
        from: PlayerId,
        /// The player's number for this message.
        n: u64,
        /// What they said.
        text: String,
    },
    /// The environment's own message to itself that the phase numbered
    /// `seq` is over.
    EndPhase {
        /// The number of the phase.
        seq: u64,
    },
    /// A model player's own message to its think loop to call the model.
    Wake,
    /// A model player's own reminder, the `count`th it set, to be woken in
    /// the phase numbered `seq`. A timer cannot be canceled, so one whose
    /// `count` is not the latest set, or whose phase has ended, is ignored.
    Remind {
        /// The number of the phase.
        seq: u64,
        /// Which reminder of the player's this is.
        count: u64,
    },
    /// A message only this game has, written as the game writes it.
    #[serde(untagged)]
    Move(G::Move),
}
impl<G: Game> free_agent::Message for Message<G> {}

impl<G: Game> Message<G> {
    /// The sender's number for this message, which a reminder to itself
    /// does not have.
    pub fn number(&self) -> Option<u64> {
        match self {
            Message::Announce { n, .. } | Message::Say { n, .. } => Some(*n),
            Message::Move(own) => own.number(),
            Message::EndPhase { .. } | Message::Wake | Message::Remind { .. } => None,
        }
    }

    /// The phase this message is tagged with: for a message to others, the
    /// phase its sender was in; for a reminder, the phase it is for. A
    /// wake-up names none.
    pub fn seq(&self) -> Option<u64> {
        match self {
            Message::Announce { seq, .. }
            | Message::Say { seq, .. }
            | Message::EndPhase { seq }
            | Message::Remind { seq, .. } => Some(*seq),
            Message::Move(own) => Some(own.seq()),
            Message::Wake => None,
        }
    }

    /// Who sent this message: the speaker of an utterance, the sender of a
    /// move, and otherwise the environment.
    pub fn sender(&self) -> ActorId {
        match self {
            Message::Say { from, .. } => from.clone(),
            Message::Move(own) => own.sender(),
            _ => ENVIRONMENT.to_string(),
        }
    }
}

/// What the log holds: the deal, the configuration, every message sent as
/// the action of its sender and the observation of its receiver, the usage
/// the provider reported for each model call, each message a model player
/// withheld because its phase had ended, each failure of a model player to
/// act, and the game's own events, such as how each phase came out and the
/// result. Every actor logs its own actions and observations: the
/// environment logs the deal, the configuration and the game's events
/// besides, and a player its usages, what it withholds and its failures.
/// Every entry made during a phase names the phase in a field, so nothing
/// about phases depends on the order entries reach the log or on their
/// times.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "")]
pub enum Entry<G: Game> {
    /// The game begins: who plays what, with which strategy, and how long
    /// each phase waits for the players. The limits are written as
    /// `humantime` writes them, such as `"60s"`.
    Start {
        /// Every player's role.
        roles: HashMap<PlayerId, G::Role>,
        /// The name of every player's strategy.
        strategies: HashMap<PlayerId, String>,
        /// How long each phase waits.
        #[serde(flatten)]
        limits: G::Limits,
    },
    /// The configuration the game ran under, logged right after the deal.
    Configuration(Box<Configuration<G>>),
    /// `from` sent `message`, which it numbered `n`, to everyone in `to`.
    /// Logged by the sender.
    Action {
        /// Who sent it.
        from: ActorId,
        /// The sender's number for it.
        n: u64,
        /// Everyone it was sent to.
        to: HashSet<ActorId>,
        /// What was sent.
        message: Message<G>,
        /// How many history entries the model was shown when it made the
        /// call that produced the message, and `None` for a message no
        /// model made: the environment's, or a random player's.
        seen: Option<usize>,
        /// How many lines of talk the call that produced the message left
        /// out to fit the context window, `None` when it left out none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forgot: Option<usize>,
    },
    /// `by` received the message `from` numbered `n` while in the phase
    /// numbered `seq`. Logged by the receiver, and naming the message
    /// without repeating it: the sender's `Action` holds it.
    Observation {
        /// Who received it.
        by: ActorId,
        /// Who sent it.
        from: ActorId,
        /// The sender's number for it.
        n: u64,
        /// The phase the receiver was in when it arrived: for the
        /// environment, the phase it is in; for a player, the latest phase
        /// it was announced. Set beside the `seq` of the message named, it
        /// shows a message that arrived in a later phase than it was sent
        /// in.
        seq: u64,
    },
    /// `from` did not send `message`, which a model made from `seen`
    /// history entries, having forgotten `forgot` lines of talk, because
    /// the phase it was made in had ended before the model answered. It was
    /// never sent, so it has no number: its `n` is 0, and the player does
    /// not remember it.
    Withheld {
        /// Who would have sent it.
        from: PlayerId,
        /// What it would have sent.
        message: Message<G>,
        /// How many history entries the model was shown when it made the
        /// call that produced the message.
        seen: Option<usize>,
        /// How many lines of talk the call that produced the message left
        /// out to fit the context window, `None` when it left out none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forgot: Option<usize>,
    },
    /// `player` failed to act in the phase numbered `seq`: the model was
    /// not reached, or answered with something that is not a well-formed
    /// response.
    Failed {
        /// Who failed.
        player: PlayerId,
        /// The phase it was in.
        seq: u64,
        /// What went wrong, as the error says it, causes included.
        error: String,
    },
    /// The model answered a call `player` made in the phase numbered `seq`,
    /// and its provider reported `usage` for it, as sent. Logged by the
    /// player.
    Usage {
        /// Who made the call.
        player: PlayerId,
        /// The phase the call was made in.
        seq: u64,
        /// What the provider reported, as sent.
        usage: serde_json::Value,
    },
    /// An entry only this game has, written as the game writes it.
    #[serde(untagged)]
    Event(G::Event),
}

impl<G: Game> Entry<G> {
    /// The entry that opens a game of `deal` under `rules`: every player's
    /// role and strategy, and the limits.
    pub fn start(deal: &HashMap<PlayerId, Dealt<G::Role>>, rules: &G::Rules) -> Self {
        let (roles, strategies) = deal
            .iter()
            .map(|(id, dealt)| {
                (
                    (id.clone(), dealt.role),
                    (id.clone(), dealt.strategy.clone()),
                )
            })
            .unzip();
        Entry::Start {
            roles,
            strategies,
            limits: G::limits(rules),
        }
    }
}

/// How a seat of game `G` is built: its player's behavior, and its think
/// loop if it has one.
pub type Seat<G> = crate::seat::Seat<Actor<G>>;

/// An episode of a game of `deal` under `rules`: the environment, which
/// may reach and stop every player, logs `configuration` right after the
/// deal, and sends the winning team on `winner` when the game ends; and in
/// each seat the player `player` builds for it. Everyone holds `logger`. A
/// player may send to the environment and to every other player, and
/// stops nobody.
pub fn game<G: Game>(
    deal: HashMap<PlayerId, Dealt<G::Role>>,
    rules: G::Rules,
    configuration: Configuration<G>,
    winner: oneshot::Sender<G::Team>,
    mut player: impl FnMut(&PlayerId) -> Seat<G>,
    logger: Logger<Entry<G>>,
) -> Episode<Actor<G>> {
    let players: Vec<PlayerId> = deal.keys().cloned().collect();
    let configuration = Entry::Configuration(Box::new(configuration));
    let environment = G::environment(deal, rules, configuration, winner);
    let mut init = HashMap::from([(ENVIRONMENT.to_string(), environment)]);
    for id in &players {
        let (behavior, think) = player(id);
        init.insert(
            id.clone(),
            ActorInit {
                behavior,
                think,
                can_send_to: to_everyone_but(id, &players),
                can_shut_down: HashSet::new(),
                has_logger: true,
            },
        );
    }
    Episode::new(init, logger)
}

/// What an actor in a game does: run it, or play in it by one of the
/// policies. An episode holds one kind of actor, so every side meets here
/// and each method goes to whichever side this is.
pub enum Actor<G: Game> {
    /// The side that holds the game.
    Environment(G::Environment),
    /// A player who chooses at random.
    Random(random::Player<G>),
    /// A player whose decisions a language model makes.
    Prompted(prompted::Player<G>),
    /// A player who says whatever its script says.
    #[cfg(test)]
    Puppet(G::Puppet),
}

#[async_trait]
impl<G: Game> Behavior for Actor<G> {
    type Message = Message<G>;
    type Log = Entry<G>;

    fn context(&self) -> &Context<Message<G>, Entry<G>> {
        match self {
            Actor::Environment(environment) => environment.context(),
            Actor::Random(player) => player.context(),
            Actor::Prompted(player) => player.context(),
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.context(),
        }
    }

    async fn receive(&mut self, message: &Message<G>) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.receive(message).await,
            Actor::Random(player) => player.receive(message).await,
            Actor::Prompted(player) => player.receive(message).await,
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.receive(message).await,
        }
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Environment(environment) => environment.start().await,
            Actor::Random(player) => player.start().await,
            Actor::Prompted(player) => player.start().await,
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.start().await,
        }
    }
}

/// Send `message` from the actor of `context` to everyone in `to`, logging
/// it first as the actor's action if it is numbered. `seen` is how many
/// history entries a model was shown when it made the call that produced
/// the message, and `forgot` how many lines of talk the call left out, each
/// `None` for a message no model made.
pub fn act<G: Game>(
    context: &Context<Message<G>, Entry<G>>,
    message: Message<G>,
    to: HashSet<ActorId>,
    seen: Option<usize>,
    forgot: Option<usize>,
) -> anyhow::Result<()> {
    if let Some(n) = message.number() {
        context.log(Entry::Action {
            from: context.id.clone(),
            n,
            to: to.clone(),
            message: message.clone(),
            seen,
            forgot,
        });
    }
    context.send(message, to)
}

/// Log that the actor of `context`, in the phase numbered `seq`, received
/// the message `from` numbered `n`: its observation.
pub fn observe<G: Game>(context: &Context<Message<G>, Entry<G>>, from: ActorId, n: u64, seq: u64) {
    context.log(Entry::Observation {
        by: context.id.clone(),
        from,
        n,
        seq,
    });
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::strategy::RANDOM;
    use free_agent::Event;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
    use tokio::time::Instant;

    /// What a game's tests supply for the shared helpers to play it.
    pub(crate) trait Fixture: Game {
        /// The configuration of a game of `roles` under `rules` with every
        /// player random.
        fn configured(
            roles: &HashMap<PlayerId, Self::Role>,
            rules: &Self::Rules,
        ) -> Configuration<Self>;
    }

    /// `players` in name order.
    pub(crate) fn sorted(players: HashSet<PlayerId>) -> Vec<PlayerId> {
        let mut players: Vec<_> = players.into_iter().collect();
        players.sort();
        players
    }

    /// `roles`, each dealt with the strategy `strategy`.
    pub(crate) fn dealt<R: Copy>(
        roles: &HashMap<PlayerId, R>,
        strategy: &str,
    ) -> HashMap<PlayerId, Dealt<R>> {
        roles
            .iter()
            .map(|(id, role)| {
                let dealt = Dealt {
                    role: *role,
                    strategy: strategy.to_string(),
                };
                (id.clone(), dealt)
            })
            .collect()
    }

    /// What a game reports: the winner, everything logged, and how long
    /// the game took.
    pub(crate) type Played<G> = (<G as Game>::Team, Vec<Entry<G>>, Duration);

    /// Run `episode`, whose winner comes on `won` and whose log on `log`.
    pub(crate) async fn run<
        G: Game,
        A: Behavior<Message = Message<G>, Log = Entry<G>> + 'static,
    >(
        episode: Episode<A>,
        won: oneshot::Receiver<G::Team>,
        mut log: UnboundedReceiver<Event<Entry<G>>>,
    ) -> anyhow::Result<Played<G>> {
        let started = Instant::now();
        episode.run(Duration::from_secs(60 * 60)).await?;
        let took = started.elapsed();

        let mut logged = Vec::new();
        while let Some(event) = log.recv().await {
            logged.push(event.payload);
        }
        Ok((won.await?, logged, took))
    }

    /// Run a game of `roles` under `rules`, each player choosing at random
    /// except those in `puppets`, who follow their own scripts.
    pub(crate) async fn play<G: Fixture>(
        roles: HashMap<PlayerId, G::Role>,
        rules: G::Rules,
        mut puppets: HashMap<&str, Seat<G>>,
    ) -> anyhow::Result<Played<G>> {
        let (winner, won) = oneshot::channel();
        let (logger, log) = unbounded_channel();
        let player = |id: &PlayerId| {
            puppets
                .remove(id.as_str())
                .unwrap_or_else(random::Player::init)
        };
        let configuration = G::configured(&roles, &rules);
        let episode = game(
            dealt(&roles, RANDOM),
            rules,
            configuration,
            winner,
            player,
            logger,
        );
        run(episode, won, log).await
    }

    /// `payload`, an entry or a memory, as an event logged or remembered
    /// when the game began, in no episode in particular.
    pub(crate) fn logged<L>(payload: L) -> Event<L> {
        Event {
            time: std::time::UNIX_EPOCH,
            episode: Default::default(),
            payload,
        }
    }

    /// The environment's announcement of the phase numbered `seq`, showing
    /// `view`, lasting a minute. The number is the phase's.
    pub(crate) fn announcement<G: Game>(seq: u64, view: G::View) -> Message<G> {
        Message::Announce {
            seq,
            n: seq,
            view,
            limit: Duration::from_secs(60),
        }
    }

    /// `from`'s utterance of `text` in the phase numbered `seq`, the `n`th
    /// message `from` sent.
    pub(crate) fn utterance<G: Game>(seq: u64, from: &str, n: u64, text: &str) -> Message<G> {
        Message::Say {
            seq,
            from: from.to_string(),
            n,
            text: text.to_string(),
        }
    }

    /// The messages `by` logged as its observations, each as who sent it,
    /// the sender's number for it, and the phase `by` was in when it
    /// arrived, in the log's order.
    pub(crate) fn observed_by<'a, G: Game>(
        logged: &'a [Entry<G>],
        by: &str,
    ) -> Vec<(&'a str, u64, u64)> {
        logged
            .iter()
            .filter_map(|entry| match entry {
                Entry::Observation {
                    by: observer,
                    from,
                    n,
                    seq,
                } if observer == by => Some((from.as_str(), *n, *seq)),
                _ => None,
            })
            .collect()
    }
}
