//! Logging is out of band. The episode is given one logger channel, each
//! actor that wants to log holds a copy of it, and a behavior logs through
//! [`Context::log`](crate::Context::log). The other end of the
//! channel is drained by [`console_log`] or [`drain`].

use std::fmt::Display;
use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// Something an actor wanted written down, stamped with when it said so.
/// The payload is whatever the episode's actors log, most often their
/// message type.
#[derive(Debug, Clone, PartialEq)]
pub struct Event<P> {
    /// When the event was logged.
    pub at: SystemTime,
    /// What the actor logged.
    pub payload: P,
}

impl<P> Event<P> {
    /// An event with `payload`, stamped now.
    pub fn now(payload: P) -> Self {
        Self {
            at: SystemTime::now(),
            payload,
        }
    }
}

/// The sending end of an episode's log.
pub type Logger<P> = UnboundedSender<Event<P>>;

/// Write every event from `events` to `sink`, one per line, until every
/// logger is gone. A line is the time the event was logged, in seconds since
/// the Unix epoch to the millisecond, a space, and the payload.
///
/// # Errors
///
/// Fails when writing to `sink` fails.
pub async fn drain<P: Display, W: Write>(
    mut events: UnboundedReceiver<Event<P>>,
    mut sink: W,
) -> io::Result<()> {
    while let Some(event) = events.recv().await {
        writeln!(sink, "{} {}", unix_seconds(event.at), event.payload)?;
    }
    sink.flush()
}

/// [`drain`] to standard error.
///
/// Spawn this beside the episode, then await it once the episode is over.
/// It finishes when the last actor lets go of its logger, so every event
/// logged is written.
pub async fn console_log<P: Display>(events: UnboundedReceiver<Event<P>>) -> io::Result<()> {
    drain(events, io::stderr()).await
}

/// `at` as seconds since the Unix epoch, to the millisecond, or `?` for a
/// time before the epoch.
fn unix_seconds(at: SystemTime) -> String {
    match at.duration_since(UNIX_EPOCH) {
        Ok(since) => format!("{}.{:03}", since.as_secs(), since.subsec_millis()),
        Err(_) => "?".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn an_event_is_stamped_when_it_is_made() {
        let before = SystemTime::now();
        let event = Event::now("something happened");
        let after = SystemTime::now();

        assert!(before <= event.at && event.at <= after, "{event:?}");
        assert_eq!(event.payload, "something happened");
    }

    #[tokio::test]
    async fn drain_writes_each_event_as_a_stamped_line_until_every_logger_is_gone() {
        let (logger, events) = unbounded_channel();
        let at = UNIX_EPOCH + Duration::from_millis(1_700_000_000_123);
        logger.send(Event { at, payload: "one" }).unwrap();
        logger
            .send(Event {
                at: at + Duration::from_millis(1_000),
                payload: "two",
            })
            .unwrap();
        drop(logger);

        let mut sink = Vec::new();
        drain(events, &mut sink).await.unwrap();

        let written = String::from_utf8(sink).unwrap();
        assert_eq!(written, "1700000000.123 one\n1700000001.123 two\n");
    }

    #[tokio::test]
    async fn a_time_before_the_epoch_is_stamped_with_a_question_mark() {
        let (logger, events) = unbounded_channel();
        let at = UNIX_EPOCH - Duration::from_secs(1);
        logger
            .send(Event {
                at,
                payload: "long ago",
            })
            .unwrap();
        drop(logger);

        let mut sink = Vec::new();
        drain(events, &mut sink).await.unwrap();

        assert_eq!(String::from_utf8(sink).unwrap(), "? long ago\n");
    }

    #[tokio::test]
    async fn console_log_writes_to_standard_error_until_every_logger_is_gone() {
        let (logger, events) = unbounded_channel();
        logger.send(Event::now("to standard error")).unwrap();
        drop(logger);

        console_log(events).await.unwrap();
    }
}
