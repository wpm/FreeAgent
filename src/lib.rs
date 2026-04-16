use std::future::Future;

use async_nats::Client;
use futures::StreamExt;
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::mpsc;
use tracing::warn;

/// Idle timeout configuration: a duration and a callback that fires after silence.
///
/// The callback receives the entity's queue sender so it can enqueue messages.
/// The timeout resets each time the entity processes a message. It only fires
/// when the entity has been idle (no messages) for the configured duration.
pub type Timer<M> = (
    std::time::Duration,
    Box<dyn Fn(mpsc::UnboundedSender<M>) + Send + 'static>,
);

/// A generic autonomous entity that communicates over NATS.
///
/// `M` is the message type exchanged on the NATS subject. It must be
/// serializable, so it can be sent over the wire and deserializable so
/// inbound NATS payloads can be decoded.
///
/// The entity has three wake-up sources:
/// 1. A message arrives on its internal queue (from NATS or direct enqueue).
/// 2. An optional idle timeout fires after a period of inactivity.
/// 3. The queue channel closes (signals shutdown).
///
/// What happens when a message is processed is determined by the `process`
/// function passed to [`Entity::run`].
pub struct Entity<M> {
    /// NATS subject this entity subscribes to and is addressed by.
    pub subject: String,
    pub queue_tx: mpsc::UnboundedSender<M>,
    queue_rx: mpsc::UnboundedReceiver<M>,
    /// Optional idle timeout.
    timer: Option<Timer<M>>,
}

impl<M> Entity<M>
where
    M: Serialize + DeserializeOwned + Send + 'static,
{
    /// Create a new entity subscribed to `subject`.
    ///
    /// `timer` — if `Some((duration, f))`, `f` is called after the entity has
    /// been idle (no messages processed) for `duration`. The timer resets each
    /// time a message is processed. The callback receives a clone of the
    /// internal queue sender, so it can enqueue messages.
    pub fn new(subject: impl Into<String>, timer: Option<Timer<M>>) -> Self {
        let (queue_tx, queue_rx) = mpsc::unbounded_channel();
        Self {
            subject: subject.into(),
            queue_tx,
            queue_rx,
            timer,
        }
    }

    /// Run the entity until the queue channel closes or the task is aborted.
    ///
    /// `process` is called for every message dequeued. It receives the message
    /// and a clone of the NATS client so it can publish replies.
    ///
    /// If an idle timeout is configured, it fires when no message has been
    /// processed for the configured duration. The timer resets after each
    /// message and after each timeout callback.
    pub async fn run<F, Fut>(mut self, nats: Client, process: F)
    where
        F: Fn(M, Client) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        // Subscribe to NATS and shuttle inbound messages onto the internal queue.
        let mut subscription = nats
            .subscribe(self.subject.clone())
            .await
            .expect("subscribe failed");

        let shuttle_tx = self.queue_tx.clone();
        let subject_label = self.subject.clone();
        tokio::spawn(async move {
            while let Some(msg) = subscription.next().await {
                match serde_json::from_slice::<M>(&msg.payload) {
                    Ok(m) => {
                        if shuttle_tx.send(m).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(subject = %subject_label, error = %e, "failed to deserialize message");
                    }
                }
            }
        });

        // Main loop with integrated idle timeout.
        let timer = self.timer.take();
        match timer {
            Some((idle_duration, callback)) => {
                loop {
                    tokio::select! {
                        msg = self.queue_rx.recv() => {
                            match msg {
                                Some(m) => process(m, nats.clone()).await,
                                None => break, // channel closed
                            }
                            // Timer resets implicitly: next iteration starts a fresh sleep.
                        }
                        _ = tokio::time::sleep(idle_duration) => {
                            callback(self.queue_tx.clone());
                            // Loop continues — if callback enqueued something, recv() picks it up.
                        }
                    }
                }
            }
            None => {
                while let Some(msg) = self.queue_rx.recv().await {
                    process(msg, nats.clone()).await;
                }
            }
        }
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

    // ── construction ────────────────────────────────────────────────────────

    #[test]
    fn new_sets_subject() {
        let entity = Entity::<Ping>::new("test.subject", None);
        assert_eq!(entity.subject, "test.subject");
    }

    #[test]
    fn queue_tx_is_live_after_construction() {
        let entity = Entity::<Ping>::new("test.subject", None);
        // The receiver is still inside the entity, so sending must succeed.
        assert!(entity.queue_tx.send(Ping { value: 1 }).is_ok());
    }

    // ── direct queue enqueue ─────────────────────────────────────────────────

    /// Enqueue messages via queue_tx before run(), then drive the entity to
    /// completion by dropping the sender so the main loop exits cleanly.
    #[tokio::test]
    async fn processes_pre_enqueued_messages() {
        let nats = async_nats::connect("nats://localhost:4222")
            .await
            .expect("NATS not available — start nats-server before running tests");

        let entity = Entity::<Ping>::new("entity.test.pre_enqueue", None);

        let collected: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let collected_clone = collected.clone();

        // Enqueue two messages before starting run().
        entity.queue_tx.send(Ping { value: 10 }).unwrap();
        entity.queue_tx.send(Ping { value: 20 }).unwrap();

        // run() consumes the entity (and its internal sender), so the channel
        // stays open as long as the task lives. Abort after a short deadline.
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

        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.abort();

        let seen = collected.lock().unwrap().clone();
        assert!(
            seen.contains(&10) && seen.contains(&20),
            "expected both messages processed, got: {seen:?}"
        );
    }

    // ── idle timeout ────────────────────────────────────────────────────────

    /// An idle entity should have its timeout fire repeatedly.
    #[tokio::test]
    async fn idle_timeout_fires_when_idle() {
        let nats = async_nats::connect("nats://localhost:4222")
            .await
            .expect("NATS not available — start nats-server before running tests");

        let fired: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
        let fired_clone = fired.clone();

        let idle_duration = Duration::from_millis(20);
        let timer: Timer<Ping> = (
            idle_duration,
            Box::new(move |tx| {
                *fired_clone.lock().unwrap() += 1;
                let _ = tx.send(Ping { value: 99 });
            }),
        );

        let entity = Entity::<Ping>::new("entity.test.idle_timeout", Some(timer));

        let handle = tokio::spawn(async move {
            entity.run(nats, |_msg: Ping, _nats| async {}).await;
        });

        // Wait long enough for multiple idle timeouts.
        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.abort();

        let count = *fired.lock().unwrap();
        assert!(
            count >= 2,
            "idle timeout should have fired at least twice, fired {count} times"
        );
    }

    /// While the entity is busy processing messages, the idle timeout should
    /// NOT fire — it only fires after a period of inactivity.
    #[tokio::test]
    async fn idle_timeout_does_not_fire_while_busy() {
        let nats = async_nats::connect("nats://localhost:4222")
            .await
            .expect("NATS not available — start nats-server before running tests");

        let fired: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
        let fired_clone = fired.clone();

        // Idle timeout of 50ms — we'll keep the entity busy for 200ms.
        let idle_duration = Duration::from_millis(50);
        let timer: Timer<Ping> = (
            idle_duration,
            Box::new(move |_tx| {
                *fired_clone.lock().unwrap() += 1;
                // Deliberately do NOT enqueue a message — we want to observe
                // whether the timeout fires, not feed the loop.
            }),
        );

        let entity = Entity::<Ping>::new("entity.test.busy_no_timeout", Some(timer));
        let tx = entity.queue_tx.clone();

        let handle = tokio::spawn(async move {
            entity.run(nats, |_msg: Ping, _nats| async {}).await;
        });

        // Rapidly enqueue messages every 10ms for 200ms — well within the 50ms
        // idle timeout. The timeout should never fire.
        for _ in 0..20 {
            let _ = tx.send(Ping { value: 1 });
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
        // Just verify construction and that queue_tx is usable — no async needed.
        let entity = Entity::<Ping>::new("entity.test.no_timer", None);
        assert!(entity.queue_tx.send(Ping { value: 0 }).is_ok());
    }
}
