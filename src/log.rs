//! Logging is out of band. The episode is given one logger channel, each
//! actor that wants to log holds a copy of it, and a behavior logs through
//! [`Context::log`](crate::Context::log). The other end of the
//! channel is drained by [`console_log`] or [`drain`], which write each
//! event as one line of JSON.

use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::time::SystemTime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

/// Something an actor wanted written down, stamped with when it said so
/// and which episode it was in. The payload is whatever the episode's
/// actors log, most often their message type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event<L> {
    /// When the event was logged.
    pub time: SystemTime,
    /// The episode it was logged in.
    pub episode: Uuid,
    /// What the actor logged.
    pub payload: L,
}

impl<L> Event<L> {
    /// An event with `payload` in `episode`, stamped now.
    pub fn now(episode: Uuid, payload: L) -> Self {
        Self {
            time: SystemTime::now(),
            episode,
            payload,
        }
    }
}

impl<L: Serialize> Event<L> {
    /// Write this event to `sink` as one line of JSON. Nothing is
    /// summarized: the line carries the whole event, with its time as
    /// seconds and nanoseconds since the Unix epoch.
    ///
    /// # Errors
    ///
    /// Fails when writing to `sink` fails, or when the event cannot be
    /// serialized, as one stamped before the epoch cannot.
    pub fn write<W: Write>(&self, mut sink: W) -> io::Result<()> {
        serde_json::to_writer(&mut sink, self)?;
        sink.write_all(b"\n")
    }
}

/// The sending end of an episode's log.
pub type Logger<L> = UnboundedSender<Event<L>>;

/// Write every event from `events` to `sink`, each as [`Event::write`]
/// does, until every logger is gone.
///
/// # Errors
///
/// Fails when writing to `sink` fails, or when an event cannot be
/// serialized.
pub async fn drain<L: Serialize, W: Write>(
    mut events: UnboundedReceiver<Event<L>>,
    mut sink: W,
) -> io::Result<()> {
    while let Some(event) = events.recv().await {
        event.write(&mut sink)?;
    }
    sink.flush()
}

/// [`drain`] to standard error.
///
/// Spawn this beside the episode, then await it once the episode is over.
/// It finishes when the last actor lets go of its logger, so every event
/// logged is written.
pub async fn console_log<L: Serialize>(events: UnboundedReceiver<Event<L>>) -> io::Result<()> {
    drain(events, io::stderr()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};
    use tokio::sync::mpsc::unbounded_channel;

    fn episode() -> Uuid {
        Uuid::from_u128(1)
    }

    #[test]
    fn an_event_is_stamped_when_it_is_made() {
        let before = SystemTime::now();
        let event = Event::now(episode(), "something happened");
        let after = SystemTime::now();

        assert!(before <= event.time && event.time <= after, "{event:?}");
        assert_eq!(event.episode, episode());
        assert_eq!(event.payload, "something happened");
    }

    #[tokio::test]
    async fn drain_writes_each_event_as_a_line_of_json_until_every_logger_is_gone() {
        let (logger, events) = unbounded_channel();
        let at = UNIX_EPOCH + Duration::from_millis(1_700_000_000_123);
        let one = Event {
            time: at,
            episode: episode(),
            payload: "one".to_string(),
        };
        let two = Event {
            time: at + Duration::from_millis(1_000),
            episode: episode(),
            payload: "two".to_string(),
        };
        logger.send(one.clone()).unwrap();
        logger.send(two.clone()).unwrap();
        drop(logger);

        let mut sink = Vec::new();
        drain(events, &mut sink).await.unwrap();

        let written = String::from_utf8(sink).unwrap();
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(
            lines[0],
            concat!(
                r#"{"time":{"secs_since_epoch":1700000000,"nanos_since_epoch":123000000},"#,
                r#""episode":"00000000-0000-0000-0000-000000000001","payload":"one"}"#
            )
        );
        let read: Vec<Event<String>> = lines
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(read, [one, two]);
    }

    #[tokio::test]
    async fn a_time_before_the_epoch_cannot_be_written() {
        let (logger, events) = unbounded_channel();
        logger
            .send(Event {
                time: UNIX_EPOCH - Duration::from_secs(1),
                episode: episode(),
                payload: "long ago",
            })
            .unwrap();
        drop(logger);

        let mut sink = Vec::new();
        assert!(drain(events, &mut sink).await.is_err());
    }

    #[tokio::test]
    async fn console_log_writes_to_standard_error_until_every_logger_is_gone() {
        let (logger, events) = unbounded_channel();
        logger
            .send(Event::now(episode(), "to standard error"))
            .unwrap();
        drop(logger);

        console_log(events).await.unwrap();
    }
}
