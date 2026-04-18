use std::future::Future;

use async_nats::Client;
use futures::{future, Stream, StreamExt};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::warn;

/// Control commands for the entity lifecycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Control {
    Start,
    Stop,
}

/// Wire format for all NATS messages addressed to an entity.
#[derive(Debug, Serialize, Deserialize)]
pub enum Envelope<M> {
    Control(Control),
    Message(M),
}

/// Entity lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Active,
    Stopped,
}

/// Idle timeout configuration: a duration and a callback that fires after silence.
///
/// The callback receives the entity's queue sender so it can enqueue messages.
/// The timeout resets each time the entity processes a message. It only fires
/// when the entity has been idle (no messages) for the configured duration.
pub type Timer<M> = (
    std::time::Duration,
    Box<dyn Fn(mpsc::UnboundedSender<M>) + Send + Sync + 'static>,
);

/// Abstraction over a NATS connection.
///
/// Implemented for `async_nats::Client` out of the box. In tests, inject a
/// client connected to an embedded or local server.
pub trait NatsConnection: Clone + Send + 'static {
    type Subscription: Stream<Item = async_nats::Message> + Unpin + Send + 'static;

    fn subscribe(&self, subject: String) -> impl Future<Output = Self::Subscription> + Send;
}

impl NatsConnection for Client {
    type Subscription = async_nats::Subscriber;

    fn subscribe(&self, subject: String) -> impl Future<Output = Self::Subscription> + Send {
        let inner = self.clone();
        async move { inner.subscribe(subject).await.expect("subscribe failed") }
    }
}

/// A generic autonomous entity that communicates over NATS.
///
/// `M` is the message type carried in `Envelope::Message` payloads. It must be
/// serializable so it can travel over the wire.
///
/// Lifecycle: IDLE → ACTIVE → STOPPED.
/// The entity starts IDLE and only begins processing messages after receiving
/// `Envelope::Control(Control::Start)` over NATS. It shuts down cleanly on
/// `Envelope::Control(Control::Stop)`.
pub struct Entity<M> {
    /// NATS subject this entity subscribes to and is addressed by.
    pub subject: String,
    queue_tx: mpsc::UnboundedSender<M>,
    queue_rx: mpsc::UnboundedReceiver<M>,
    timer: Option<Timer<M>>,
}

impl<M> Entity<M>
where
    M: Serialize + DeserializeOwned + Send + 'static,
{
    /// Create a new entity subscribed to `subject`.
    ///
    /// `timer` — if `Some((duration, f))`, `f` is called after the entity has
    /// been idle for `duration` while ACTIVE. The timer resets each time a
    /// message is processed. The callback receives the internal queue sender.
    pub fn new(subject: impl Into<String>, timer: Option<Timer<M>>) -> Self {
        let (queue_tx, queue_rx) = mpsc::unbounded_channel();
        Self {
            subject: subject.into(),
            queue_tx,
            queue_rx,
            timer,
        }
    }

    /// Run the entity until it receives `Control::Stop` or the task is aborted.
    ///
    /// Starts in the IDLE state — messages are ignored until `Control::Start` arrives.
    /// In the ACTIVE state, `process` is called for every `Envelope::Message` dequeued.
    /// On `Control::Stop`, the NATS subscription is drained and `run()` returns.
    pub async fn run<NC, F, Fut>(mut self, nats: NC, process: F)
    where
        NC: NatsConnection,
        F: Fn(M, NC) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let mut subscription = NatsConnection::subscribe(&nats, self.subject.clone()).await;
        let mut state = State::Idle;
        let timer = self.timer.take();

        loop {
            let idle_sleep = match &timer {
                Some((dur, _)) if state == State::Active => {
                    future::Either::Left(tokio::time::sleep(*dur))
                }
                _ => future::Either::Right(future::pending()),
            };

            tokio::select! {
                nats_msg = subscription.next() => {
                    match nats_msg {
                        None => break,
                        Some(raw) => {
                            state = dispatch(&raw, state, &nats, &process).await;
                            if state == State::Stopped { break; }
                        }
                    }
                }
                queue_msg = self.queue_rx.recv() => {
                    if let Some(m) = queue_msg {
                        process(m, nats.clone()).await;
                    }
                }
                _ = idle_sleep => {
                    if let Some((_, callback)) = &timer {
                        callback(self.queue_tx.clone());
                    }
                }
            }
        }
        // subscription drops here → Subscriber::drop sends Unsubscribe
    }
}

/// Deserialize a raw NATS message as `Envelope<M>` and apply state machine logic.
/// Returns the next state.
async fn dispatch<M, NC, F, Fut>(
    raw: &async_nats::Message,
    state: State,
    nats: &NC,
    process: &F,
) -> State
where
    M: DeserializeOwned,
    NC: NatsConnection,
    F: Fn(M, NC) -> Fut,
    Fut: Future<Output = ()>,
{
    let envelope = match serde_json::from_slice::<Envelope<M>>(&raw.payload) {
        Ok(e) => e,
        Err(e) => {
            warn!(subject = %raw.subject, error = %e, "failed to deserialize message");
            return state;
        }
    };

    match (envelope, state) {
        (Envelope::Control(Control::Start), State::Idle) => State::Active,
        (Envelope::Control(Control::Start), State::Active) => {
            warn!("Control::Start received while already active");
            State::Active
        }
        (Envelope::Control(Control::Stop), State::Active) => State::Stopped,
        (Envelope::Control(Control::Stop), State::Idle) => {
            warn!("Control::Stop received while idle");
            State::Idle
        }
        (Envelope::Control(ctrl), State::Stopped) => {
            warn!(?ctrl, "control message received while stopped");
            State::Stopped
        }
        (Envelope::Message(m), State::Active) => {
            process(m, nats.clone()).await;
            State::Active
        }
        (Envelope::Message(_), s) => s, // silently ignore in Idle/Stopped
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    struct Ping {
        value: u32,
    }

    async fn connect() -> Client {
        async_nats::connect("nats://localhost:4222")
            .await
            .expect("NATS not available — start nats-server before running tests")
    }

    async fn publish<M: Serialize>(nats: &Client, subject: &str, env: &Envelope<M>) {
        let bytes = serde_json::to_vec(env).unwrap();
        nats.publish(subject.to_string(), bytes.into())
            .await
            .unwrap();
        nats.flush().await.unwrap();
    }

    async fn publish_control(nats: &Client, subject: &str, ctrl: Control) {
        publish::<Ping>(nats, subject, &Envelope::Control(ctrl)).await;
    }

    /// Spawn an entity that collects processed `Ping` values into the returned vec.
    fn spawn_collecting_entity(
        subject: &'static str,
        nats: Client,
        timer: Option<Timer<Ping>>,
    ) -> (tokio::task::JoinHandle<()>, Arc<Mutex<Vec<u32>>>) {
        let entity = Entity::<Ping>::new(subject, timer);
        let collected: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let collected_clone = collected.clone();
        let handle = tokio::spawn(async move {
            entity
                .run(nats, move |msg: Ping, _nats| {
                    let c = collected_clone.clone();
                    async move {
                        c.lock().unwrap().push(msg.value);
                    }
                })
                .await;
        });
        (handle, collected)
    }

    /// Spawn an entity that discards all processed messages.
    fn spawn_entity(
        subject: &'static str,
        nats: Client,
        timer: Option<Timer<Ping>>,
    ) -> tokio::task::JoinHandle<()> {
        let entity = Entity::<Ping>::new(subject, timer);
        tokio::spawn(async move {
            entity.run(nats, |_msg: Ping, _nats| async {}).await;
        })
    }

    // ── construction ────────────────────────────────────────────────────────

    #[test]
    fn new_sets_subject() {
        let entity = Entity::<Ping>::new("test.subject", None);
        assert_eq!(entity.subject, "test.subject");
    }

    // ── state machine: message processing ───────────────────────────────────

    #[tokio::test]
    async fn processes_messages_when_active() {
        let nats = connect().await;
        let subject = "entity.test.processes_messages";
        let (handle, collected) = spawn_collecting_entity(subject, nats.clone(), None);

        tokio::time::sleep(Duration::from_millis(20)).await;
        publish_control(&nats, subject, Control::Start).await;
        publish(&nats, subject, &Envelope::Message(Ping { value: 42 })).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        publish_control(&nats, subject, Control::Stop).await;

        tokio::time::timeout(Duration::from_millis(500), handle)
            .await
            .expect("run() did not exit after Control::Stop")
            .unwrap();

        assert!(collected.lock().unwrap().contains(&42));
    }

    // ── state machine: idle ignores messages ─────────────────────────────────

    #[tokio::test]
    async fn idle_entity_ignores_messages_until_start() {
        let nats = connect().await;
        let subject = "entity.test.idle_ignores";
        let (handle, collected) = spawn_collecting_entity(subject, nats.clone(), None);

        // Send a message while idle — should be ignored.
        tokio::time::sleep(Duration::from_millis(20)).await;
        publish(&nats, subject, &Envelope::Message(Ping { value: 99 })).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            collected.lock().unwrap().is_empty(),
            "message should be ignored while idle"
        );

        // Now start and send a message — should be processed.
        publish_control(&nats, subject, Control::Start).await;
        publish(&nats, subject, &Envelope::Message(Ping { value: 42 })).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            collected.lock().unwrap().contains(&42),
            "message should be processed after start"
        );

        publish_control(&nats, subject, Control::Stop).await;
        tokio::time::timeout(Duration::from_millis(500), handle)
            .await
            .expect("run() did not exit")
            .unwrap();
    }

    // ── state machine: stop exits run() ─────────────────────────────────────

    #[tokio::test]
    async fn control_stop_exits_run() {
        let nats = connect().await;
        let subject = "entity.test.stop_exits";
        let handle = spawn_entity(subject, nats.clone(), None);

        tokio::time::sleep(Duration::from_millis(20)).await;
        publish_control(&nats, subject, Control::Start).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        publish_control(&nats, subject, Control::Stop).await;

        tokio::time::timeout(Duration::from_millis(500), handle)
            .await
            .expect("run() did not exit after Control::Stop")
            .unwrap();
    }

    // ── state machine: unexpected stop while idle doesn't exit ───────────────

    #[tokio::test]
    async fn unexpected_stop_while_idle_does_not_exit() {
        let nats = connect().await;
        let subject = "entity.test.stop_while_idle";
        let (handle, collected) = spawn_collecting_entity(subject, nats.clone(), None);

        // Stop while idle — entity should remain alive and stay idle.
        tokio::time::sleep(Duration::from_millis(20)).await;
        publish_control(&nats, subject, Control::Stop).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Entity should still be running — now start it and verify that it processes.
        publish_control(&nats, subject, Control::Start).await;
        publish(&nats, subject, &Envelope::Message(Ping { value: 7 })).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            collected.lock().unwrap().contains(&7),
            "entity should still be alive and processing after spurious Stop"
        );

        publish_control(&nats, subject, Control::Stop).await;
        tokio::time::timeout(Duration::from_millis(500), handle)
            .await
            .expect("run() did not exit")
            .unwrap();
    }

    // ── idle timeout ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn idle_timeout_fires_when_idle() {
        let nats = connect().await;
        let subject = "entity.test.idle_timeout";

        let fired: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
        let fired_clone = fired.clone();
        let timer: Timer<Ping> = (
            Duration::from_millis(20),
            Box::new(move |tx| {
                *fired_clone.lock().unwrap() += 1;
                let _ = tx.send(Ping { value: 99 });
            }),
        );
        let handle = spawn_entity(subject, nats.clone(), Some(timer));

        // Start the entity, then let it idle so the timer fires.
        tokio::time::sleep(Duration::from_millis(20)).await;
        publish_control(&nats, subject, Control::Start).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        handle.abort();

        let count = *fired.lock().unwrap();
        assert!(
            count >= 2,
            "idle timeout should have fired at least twice, fired {count} times"
        );
    }

    #[tokio::test]
    async fn idle_timeout_does_not_fire_while_busy() {
        let nats = connect().await;
        let subject = "entity.test.busy_no_timeout";

        let fired: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
        let fired_clone = fired.clone();
        let timer: Timer<Ping> = (
            Duration::from_millis(50),
            Box::new(move |_tx| {
                *fired_clone.lock().unwrap() += 1;
            }),
        );
        let handle = spawn_entity(subject, nats.clone(), Some(timer));

        tokio::time::sleep(Duration::from_millis(20)).await;
        publish_control(&nats, subject, Control::Start).await;

        // Rapidly publish messages every 10ms for 200ms — timeout is 50ms.
        for _ in 0..20 {
            publish(&nats, subject, &Envelope::Message(Ping { value: 1 })).await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        handle.abort();

        let count = *fired.lock().unwrap();
        assert_eq!(
            count, 0,
            "idle timeout should NOT fire while entity is busy, but fired {count} times"
        );
    }

    // ── no timer ─────────────────────────────────────────────────────────────

    #[test]
    fn no_timer_does_not_panic() {
        let entity = Entity::<Ping>::new("entity.test.no_timer", None);
        assert_eq!(entity.subject, "entity.test.no_timer");
    }
}
