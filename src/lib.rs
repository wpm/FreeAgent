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
pub type OnIdle<M> = (
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
    timer: Option<OnIdle<M>>,
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
    pub fn new(subject: impl Into<String>, timer: Option<OnIdle<M>>) -> Self {
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
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use std::time::Duration;

    use async_nats::Message as NatsMessage;
    use futures::Stream;
    use serde::{Deserialize, Serialize};
    use tokio::sync::mpsc;

    use super::*;

    // ── MockNats ─────────────────────────────────────────────────────────────

    /// In-memory NATS substitute. Pass `MockNats` into `run()`; use the
    /// [`MockNatsHandle`] to inject envelopes directly from test code.
    ///
    /// `subscribe()` is called exactly once by `run()`, at which point it
    /// hands ownership of the receiver to the entity's subscription stream.
    /// The handle retains the sender and can push messages at any time.
    #[derive(Clone)]
    struct MockNats {
        rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<NatsMessage>>>>,
    }

    struct MockNatsHandle {
        tx: mpsc::UnboundedSender<NatsMessage>,
    }

    struct MockSubscription(mpsc::UnboundedReceiver<NatsMessage>);

    impl Stream for MockSubscription {
        type Item = NatsMessage;
        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.0.poll_recv(cx)
        }
    }

    impl NatsConnection for MockNats {
        type Subscription = MockSubscription;
        fn subscribe(&self, _subject: String) -> impl Future<Output = Self::Subscription> + Send {
            let rx_slot = self.rx.clone();
            async move {
                let rx = rx_slot
                    .lock()
                    .unwrap()
                    .take()
                    .expect("MockNats::subscribe called more than once");
                MockSubscription(rx)
            }
        }
    }

    fn mock_nats() -> (MockNats, MockNatsHandle) {
        let (tx, rx) = mpsc::unbounded_channel();
        let mock = MockNats {
            rx: Arc::new(Mutex::new(Some(rx))),
        };
        (mock, MockNatsHandle { tx })
    }

    impl MockNatsHandle {
        fn inject<M: Serialize>(&self, subject: &str, env: &Envelope<M>) {
            let bytes = serde_json::to_vec(env).unwrap();
            let length = bytes.len();
            let msg = NatsMessage {
                subject: subject.to_string().into(),
                reply: None,
                payload: bytes.into(),
                headers: None,
                status: None,
                description: None,
                length,
            };
            let _ = self.tx.send(msg);
        }

        fn inject_control(&self, ctrl: Control) {
            self.inject("test", &Envelope::<Ping>::Control(ctrl));
        }
    }

    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    struct Ping {
        value: u32,
    }

    // ── construction ────────────────────────────────────────────────────────

    #[test]
    fn new_sets_subject() {
        let entity = Entity::<Ping>::new("test.subject", None);
        assert_eq!(entity.subject, "test.subject");
    }

    // ── unit tests (no NATS server) ──────────────────────────────────────────

    /// Helper: spawn an entity over MockNats that collects processed values.
    fn spawn_fake_collecting(
        timer: Option<OnIdle<Ping>>,
    ) -> (
        tokio::task::JoinHandle<()>,
        Arc<Mutex<Vec<u32>>>,
        MockNatsHandle,
    ) {
        let (fake, handle) = mock_nats();
        let entity = Entity::<Ping>::new("test", timer);
        let collected: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let collected_clone = collected.clone();
        let join = tokio::spawn(async move {
            entity
                .run(fake, move |msg: Ping, _nats| {
                    let c = collected_clone.clone();
                    async move {
                        c.lock().unwrap().push(msg.value);
                    }
                })
                .await;
        });
        (join, collected, handle)
    }

    /// Helper: spawn an entity over MockNats that discards messages.
    fn spawn_fake_entity(
        timer: Option<OnIdle<Ping>>,
    ) -> (tokio::task::JoinHandle<()>, MockNatsHandle) {
        let (fake, handle) = mock_nats();
        let entity = Entity::<Ping>::new("test", timer);
        let join = tokio::spawn(async move {
            entity.run(fake, |_msg: Ping, _nats| async {}).await;
        });
        (join, handle)
    }

    #[tokio::test]
    async fn unit_processes_messages_when_active() {
        let (join, collected, handle) = spawn_fake_collecting(None);

        handle.inject_control(Control::Start);
        handle.inject("test", &Envelope::Message(Ping { value: 42 }));
        tokio::time::sleep(Duration::from_millis(20)).await;
        handle.inject_control(Control::Stop);

        tokio::time::timeout(Duration::from_millis(500), join)
            .await
            .expect("run() did not exit")
            .unwrap();

        assert!(collected.lock().unwrap().contains(&42));
    }

    #[tokio::test]
    async fn unit_idle_ignores_messages_until_start() {
        let (join, collected, handle) = spawn_fake_collecting(None);

        // Message while idle — should be ignored.
        handle.inject("test", &Envelope::Message(Ping { value: 99 }));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            collected.lock().unwrap().is_empty(),
            "should be ignored while idle"
        );

        // Start, then send a real message.
        handle.inject_control(Control::Start);
        handle.inject("test", &Envelope::Message(Ping { value: 42 }));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            collected.lock().unwrap().contains(&42),
            "should process after start"
        );

        handle.inject_control(Control::Stop);
        tokio::time::timeout(Duration::from_millis(500), join)
            .await
            .expect("run() did not exit")
            .unwrap();
    }

    #[tokio::test]
    async fn unit_stop_exits_run() {
        let (join, handle) = spawn_fake_entity(None);

        handle.inject_control(Control::Start);
        tokio::time::sleep(Duration::from_millis(5)).await;
        handle.inject_control(Control::Stop);

        tokio::time::timeout(Duration::from_millis(500), join)
            .await
            .expect("run() did not exit after Stop")
            .unwrap();
    }

    #[tokio::test]
    async fn unit_unexpected_stop_while_idle_does_not_exit() {
        let (join, collected, handle) = spawn_fake_collecting(None);

        // Stop while idle — should warn but stay alive.
        handle.inject_control(Control::Stop);
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Entity should still be running.
        handle.inject_control(Control::Start);
        handle.inject("test", &Envelope::Message(Ping { value: 7 }));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            collected.lock().unwrap().contains(&7),
            "entity should still process after spurious Stop"
        );

        handle.inject_control(Control::Stop);
        tokio::time::timeout(Duration::from_millis(500), join)
            .await
            .expect("run() did not exit")
            .unwrap();
    }

    #[tokio::test]
    async fn unit_idle_timeout_fires_when_idle() {
        let fired: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
        let fired_clone = fired.clone();
        let timer: OnIdle<Ping> = (
            Duration::from_millis(20),
            Box::new(move |tx| {
                *fired_clone.lock().unwrap() += 1;
                let _ = tx.send(Ping { value: 99 });
            }),
        );
        let (join, handle) = spawn_fake_entity(Some(timer));

        handle.inject_control(Control::Start);
        tokio::time::sleep(Duration::from_millis(150)).await;
        join.abort();

        let count = *fired.lock().unwrap();
        assert!(
            count >= 2,
            "idle timeout should have fired at least twice, fired {count} times"
        );
    }

    #[tokio::test]
    async fn unit_idle_timeout_does_not_fire_while_busy() {
        let fired: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
        let fired_clone = fired.clone();
        let timer: OnIdle<Ping> = (
            Duration::from_millis(50),
            Box::new(move |_tx| {
                *fired_clone.lock().unwrap() += 1;
            }),
        );
        let (join, handle) = spawn_fake_entity(Some(timer));

        handle.inject_control(Control::Start);
        for _ in 0..20 {
            handle.inject("test", &Envelope::Message(Ping { value: 1 }));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        join.abort();

        let count = *fired.lock().unwrap();
        assert_eq!(
            count, 0,
            "idle timeout should NOT fire while busy, fired {count} times"
        );
    }

    // ── no timer ─────────────────────────────────────────────────────────────

    #[test]
    fn no_timer_does_not_panic() {
        let entity = Entity::<Ping>::new("entity.test.no_timer", None);
        assert_eq!(entity.subject, "entity.test.no_timer");
    }
}
