//! The table: one host and the players, as one kind of actor, [`Actor`],
//! wired by [`game`] into an episode in which every actor may send to
//! every other and the host may stop every player. What every actor does
//! when it sends or receives a message to or from another, `act` and
//! `observe`, is logged the same way by all of them.

use crate::host::Host;
use crate::log::Entry;
use crate::message::{HOST, Message};
use crate::model::Shown;
use crate::player::Player;
use async_trait::async_trait;
use free_agent::{ActorId, ActorInit, Behavior, Context, Episode, Logger};
use free_agent_game::seat::{PlayerId, Seat};
use std::collections::{HashMap, HashSet};

/// How a seat at the table is built: its actor's behavior, and its think
/// loop.
pub type Seated = Seat<Actor>;

/// What an actor at the table does: hold the game, or play in it. An
/// episode holds one kind of actor, so both sides meet here and each
/// method goes to whichever side this is.
pub enum Actor {
    /// The host, who holds the secret object and runs the game.
    Host(Host),
    /// A player whose decisions a language model makes.
    Player(Player),
    /// A player who says whatever its script says.
    #[cfg(test)]
    #[allow(private_interfaces)]
    Puppet(tests::Puppet),
}

#[async_trait]
impl Behavior for Actor {
    type Message = Message;
    type Log = Entry;

    fn context(&self) -> &Context<Message, Entry> {
        match self {
            Actor::Host(host) => host.context(),
            Actor::Player(player) => player.context(),
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.context(),
        }
    }

    async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
        match self {
            Actor::Host(host) => host.receive(message).await,
            Actor::Player(player) => player.receive(message).await,
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.receive(message).await,
        }
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Host(host) => host.start().await,
            Actor::Player(player) => player.start().await,
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.start().await,
        }
    }

    async fn clean_up(&mut self) -> anyhow::Result<()> {
        match self {
            Actor::Host(host) => host.clean_up().await,
            Actor::Player(player) => player.clean_up().await,
            #[cfg(test)]
            Actor::Puppet(puppet) => puppet.clean_up().await,
        }
    }
}

/// An episode of a game at a table of `players`: the host, built by
/// `host`, which may reach and stop every player, and in each seat the
/// player `player` builds for it, which may reach the host and every other
/// player and stops nobody. Everyone holds `logger`.
pub fn game(
    players: &[PlayerId],
    host: Seated,
    mut player: impl FnMut(&PlayerId) -> Seated,
    logger: Logger<Entry>,
) -> Episode<Actor> {
    let everyone: Vec<ActorId> = players.iter().cloned().chain([HOST.to_string()]).collect();
    let others = |me: &str| -> HashSet<ActorId> {
        everyone
            .iter()
            .filter(|actor| *actor != me)
            .cloned()
            .collect()
    };
    let (behavior, think) = host;
    let host = ActorInit {
        behavior,
        think,
        can_send_to: others(HOST),
        can_shut_down: players.iter().cloned().collect(),
        has_logger: true,
    };
    let mut init = HashMap::from([(HOST.to_string(), host)]);
    for id in players {
        let (behavior, think) = player(id);
        init.insert(
            id.clone(),
            ActorInit {
                behavior,
                think,
                can_send_to: others(id),
                can_shut_down: HashSet::new(),
                has_logger: true,
            },
        );
    }
    Episode::new(init, logger)
}

/// Everyone at a table of `players` but `me`, the host included: whom an
/// actor sends to, since everything said goes to everyone.
pub fn everyone_but(me: &str, players: &[PlayerId]) -> HashSet<ActorId> {
    players
        .iter()
        .cloned()
        .chain([HOST.to_string()])
        .filter(|actor| actor != me)
        .collect()
}

/// Send `message`, which its sender numbered, from the actor of `context`
/// to everyone in `to`, logging it first as the actor's action, with what
/// the model was `shown` when it made the call that produced the message,
/// if a model made it.
pub(crate) fn act(
    context: &Context<Message, Entry>,
    message: Message,
    to: HashSet<ActorId>,
    shown: Option<Shown>,
) -> anyhow::Result<()> {
    context.log(Entry::Action {
        from: context.id.clone(),
        n: message.number().unwrap_or_default(),
        to: to.clone(),
        message: message.clone(),
        seen: shown.map(|shown| shown.seen),
        forgot: shown.and_then(|shown| shown.forgot),
    });
    context.send(message, to)
}

/// Log that the actor of `context` received the message `from` numbered
/// `n`: its observation.
pub(crate) fn observe(context: &Context<Message, Entry>, from: ActorId, n: u64) {
    context.log(Entry::Observation {
        by: context.id.clone(),
        from,
        n,
    });
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::{Configuration, HostConfig, HostPrompt};
    use crate::host::Brief;
    use crate::log::{Side, Victory};
    use crate::model::Asker;
    use free_agent::{Event, Think};
    use free_agent_model::fake::FakeModel;
    use free_agent_model::{Model, ToolChoice};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
    use tokio::sync::oneshot;

    /// What a puppet says, and when.
    #[derive(Debug, Clone, Copy)]
    pub(crate) enum Line {
        /// Say this.
        Say(&'static str),
        /// Guess this.
        Guess(&'static str),
    }

    /// A player who says its script once the game begins, each line its
    /// delay after the beginning, and logs what it hears.
    pub(crate) struct Puppet {
        context: Context<Message, Entry>,
        /// When each line is said, after the beginning.
        delays: Vec<Duration>,
    }

    /// The puppet's think loop, which says each line when its time comes.
    struct Speaking {
        context: Context<Message, Entry>,
        lines: Vec<Line>,
        players: Vec<PlayerId>,
        n: u64,
    }

    impl Puppet {
        /// A puppet at a table of `players`, saying `script`.
        pub(crate) fn init(players: &[PlayerId], script: Vec<(Duration, Line)>) -> Seated {
            let (delays, lines) = script.into_iter().unzip();
            let players = players.to_vec();
            (
                Box::new(move |context| Actor::Puppet(Puppet { context, delays })),
                Some(Box::new(move |context| {
                    Box::new(Speaking {
                        context,
                        lines,
                        players,
                        n: 0,
                    })
                })),
            )
        }
    }

    #[async_trait]
    impl Behavior for Puppet {
        type Message = Message;
        type Log = Entry;

        fn context(&self) -> &Context<Message, Entry> {
            &self.context
        }

        /// Observe every message, and once the game begins set a reminder
        /// for each line of the script.
        async fn receive(&mut self, message: &Message) -> anyhow::Result<()> {
            if let Some(n) = message.number() {
                observe(&self.context, message.sender(), n);
            }
            if let Message::Begin { .. } = message {
                for (count, after) in (0..).zip(&self.delays) {
                    self.context
                        .think_after(*after, Message::Remind { count })?;
                }
            }
            Ok(())
        }
    }

    #[async_trait]
    impl Think for Speaking {
        type Message = Message;

        async fn think(&mut self, message: &Message) -> anyhow::Result<()> {
            let Message::Remind { count } = message else {
                return Ok(());
            };
            let from = self.context.id.clone();
            self.n += 1;
            let n = self.n;
            let message = match self.lines[*count as usize] {
                Line::Say(text) => Message::Say {
                    from: from.clone(),
                    n,
                    text: text.to_string(),
                },
                Line::Guess(text) => Message::Guess {
                    from: from.clone(),
                    n,
                    text: text.to_string(),
                },
            };
            act(
                &self.context,
                message,
                everyone_but(&from, &self.players),
                None,
            )
        }
    }

    /// Every event `log` holds once the episode that writes it is over.
    pub(crate) async fn drained<L>(mut log: UnboundedReceiver<Event<L>>) -> Vec<Event<L>> {
        let mut events = Vec::new();
        while let Some(event) = log.recv().await {
            events.push(event);
        }
        events
    }

    /// What a game came back with: the side that won, if one did, every
    /// event logged, and how its run came back.
    pub(crate) struct Played {
        pub(crate) won: Option<Side>,
        pub(crate) events: Vec<Event<Entry>>,
        pub(crate) ran: anyhow::Result<()>,
    }

    impl Played {
        /// Every entry logged, in order.
        pub(crate) fn entries(&self) -> Vec<&Entry> {
            self.events.iter().map(|event| &event.payload).collect()
        }

        /// Every message `from` sent, in order.
        pub(crate) fn sent_by(&self, from: &str) -> Vec<&Message> {
            self.entries()
                .into_iter()
                .filter_map(|entry| match entry {
                    Entry::Action {
                        from: sender,
                        message,
                        ..
                    } if sender == from => Some(message),
                    _ => None,
                })
                .collect()
        }

        /// How the game ended, as the host logged it: who won, how, how
        /// many guesses were used, the object, and how long it took.
        pub(crate) fn end(&self) -> (Side, Victory, u64, &str, Duration) {
            self.entries()
                .into_iter()
                .find_map(|entry| match entry {
                    Entry::End {
                        winner,
                        by,
                        used,
                        object,
                        took,
                    } => Some((*winner, *by, *used, object.as_str(), *took)),
                    _ => None,
                })
                .expect("the game ended")
        }

        /// Every failure `by` logged, in order.
        pub(crate) fn failures(&self, by: &str) -> Vec<&str> {
            self.entries()
                .into_iter()
                .filter_map(|entry| match entry {
                    Entry::Failed { by: who, error } if who == by => Some(error.as_str()),
                    _ => None,
                })
                .collect()
        }
    }

    /// Play a game at a table of `players`, the host built by `host`, which
    /// sends the winner on `won`, and each player by `player`.
    pub(crate) async fn played(
        players: &[PlayerId],
        host: Seated,
        won: oneshot::Receiver<Side>,
        player: impl FnMut(&PlayerId) -> Seated,
    ) -> Played {
        let (logger, log) = unbounded_channel();
        let episode = game(players, host, player, logger);
        let ran = episode.run(Duration::from_secs(60 * 60 * 24)).await;
        let events = drained(log).await;
        Played {
            won: won.await.ok(),
            events,
            ran,
        }
    }

    /// The players at every test's table.
    pub(crate) fn table() -> Vec<PlayerId> {
        ["Alice", "Bob"].map(String::from).to_vec()
    }

    /// The rules a test's game is played under.
    pub(crate) struct Rules {
        pub(crate) object: Option<&'static str>,
        pub(crate) guesses: u64,
        pub(crate) time_limit: Duration,
    }

    impl Default for Rules {
        fn default() -> Self {
            Rules {
                object: Some("a teapot"),
                guesses: 20,
                time_limit: Duration::from_secs(600),
            }
        }
    }

    /// The host of a game under `rules` at the test's [`table`], asking
    /// `model`.
    pub(crate) fn brief(model: &Arc<FakeModel>, rules: &Rules) -> Brief {
        let players = table();
        Brief {
            strategies: players
                .iter()
                .map(|player| (player.clone(), "puppet".to_string()))
                .collect(),
            prompt: HostPrompt::new(
                "You are the host. The secret object is {{ object }}.",
                &players,
                &BTreeMap::new(),
            ),
            players,
            guesses: rules.guesses,
            time_limit: rules.time_limit,
            object: rules.object.map(String::from),
            picks: vec![(Some("an animal".to_string()), "Pick an animal.".to_string())],
            temperature: 1.2,
            asker: asking(model),
        }
    }

    /// How an actor asks `model`, as the model `qwen`.
    pub(crate) fn asking(model: &Arc<FakeModel>) -> Asker {
        Asker {
            model_id: "qwen".to_string(),
            tool_choice: ToolChoice::Required,
            context_window: 4096,
            model: Arc::clone(model) as Arc<dyn Model>,
        }
    }

    /// The configuration a test's game logs.
    pub(crate) fn configuration() -> Configuration {
        Configuration {
            version: "0.0.0".to_string(),
            guesses: 20,
            time_limit: Duration::from_secs(600),
            host: HostConfig {
                model: Some("qwen".to_string()),
                object: Some("a teapot".to_string()),
                temperature: 1.2,
                categories: vec![],
                pick: None,
                system: "You are the host.".to_string(),
            },
            strategies: BTreeMap::new(),
            allotments: BTreeMap::new(),
            models: BTreeMap::new(),
            prompts: BTreeMap::new(),
            prompt_parts: BTreeMap::new(),
        }
    }

    /// Play a game under `rules` at the test's [`table`] whose host asks
    /// `model`, each seat's player built by `player` for the table.
    pub(crate) async fn hosting(
        model: &Arc<FakeModel>,
        rules: Rules,
        mut player: impl FnMut(&[PlayerId], &PlayerId) -> Seated,
    ) -> Played {
        let (winner, won) = oneshot::channel();
        let host = Host::init(brief(model, &rules), configuration(), winner);
        let players = table();
        played(&players, host, won, |seat| player(&players, seat)).await
    }

    #[test]
    fn everyone_but_an_actor_is_every_other_player_and_the_host() {
        let players: Vec<PlayerId> = ["Alice", "Bob"].map(String::from).to_vec();
        assert_eq!(
            everyone_but("Alice", &players),
            HashSet::from(["Bob".to_string(), HOST.to_string()])
        );
        assert_eq!(
            everyone_but(HOST, &players),
            HashSet::from(["Alice".to_string(), "Bob".to_string()])
        );
    }
}
