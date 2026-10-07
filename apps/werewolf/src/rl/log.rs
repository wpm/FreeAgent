//! Logging is out of band. The episode is given one logger channel, each
//! actor that wants to log holds a copy of it, and a behavior logs through
//! [`Context::log`](crate::rl::actor::Context::log).

use std::time::SystemTime;
use tokio::sync::mpsc::UnboundedSender;

/// Something an actor wanted written down, stamped with when it said so.
/// The payload is whatever the episode's actors log, most often their
/// message type.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Event<P> {
    /// When the event was logged.
    pub(super) at: SystemTime,
    pub(super) payload: P,
}

impl<P> Event<P> {
    /// An event with `payload`, stamped now.
    pub(super) fn now(payload: P) -> Self {
        Self {
            at: SystemTime::now(),
            payload,
        }
    }
}

/// The sending end of an episode's log.
pub(super) type Logger<P> = UnboundedSender<Event<P>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_is_stamped_when_it_is_made() {
        let before = SystemTime::now();
        let event = Event::now("something happened");
        let after = SystemTime::now();

        assert!(before <= event.at && event.at <= after, "{event:?}");
        assert_eq!(event.payload, "something happened");
    }
}
