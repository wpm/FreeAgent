//! Log time as the viewer reads it, and the countdowns that run on it.
//!
//! A [`Clock`] runs log time on from the latest cue. A [`Timer`] counts a
//! limit down in log time, and [`Countdown`] and [`CountdownClock`] draw
//! it.

use leptos::prelude::*;

/// Log time as the viewer reads it: the time of the latest cue, run on
/// at the clock's rate for as long as the viewer has waited since.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clock {
    /// Log time when the latest cue arrived.
    pub at: f64,
    /// Log milliseconds per viewer millisecond.
    pub rate: f64,
    /// Whether the clock runs. A saved game shown at rest stops it at its
    /// last cue.
    pub running: bool,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            at: 0.0,
            rate: 1.0,
            running: false,
        }
    }
}

impl Clock {
    /// Log time `elapsed` viewer milliseconds after the latest cue arrived.
    pub fn now(&self, elapsed: f64) -> f64 {
        if self.running {
            self.at + elapsed.max(0.0) * self.rate
        } else {
            self.at
        }
    }
}

/// A countdown in log time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timer {
    /// When it started, in log milliseconds.
    pub began: u64,
    /// How long it runs.
    pub limit_ms: u64,
    /// When it stopped, in log milliseconds, if it has: it keeps what it
    /// had left then.
    pub stopped: Option<u64>,
}

impl Timer {
    /// Milliseconds left at log time `now`, never below zero.
    pub fn left(&self, now: f64) -> f64 {
        let now = self.stopped.map_or(now, |stopped| now.min(stopped as f64));
        (self.began as f64 + self.limit_ms as f64 - now).clamp(0.0, self.limit_ms as f64)
    }

    /// Whole seconds left at log time `now`, rounded up.
    pub fn seconds(&self, now: f64) -> u64 {
        (self.left(now) / 1_000.0).ceil() as u64
    }

    /// The share of the countdown left at log time `now`, from 1 to 0.
    pub fn share(&self, now: f64) -> f64 {
        if self.limit_ms == 0 {
            0.0
        } else {
            self.left(now) / self.limit_ms as f64
        }
    }
}

/// Seconds as a clock's face shows them: `0:53`.
pub(crate) fn face(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// A bar draining over `timer` as log time runs on to `now`.
#[component]
pub fn Countdown(
    /// What it counts down.
    timer: Timer,
    /// Log time, now.
    now: Signal<f64>,
) -> impl IntoView {
    let width = move || format!("width: {:.2}%", timer.share(now.get()) * 100.0);
    view! {
        <span class="fa-countdown">
            <span class="fa-countdown-left" style=width></span>
        </span>
    }
}

/// A clock for `timer`: a draining bar and the seconds left, which change
/// once a second as log time runs on to `now`.
#[component]
pub fn CountdownClock(
    /// What it counts down.
    timer: Timer,
    /// Log time, now.
    now: Signal<f64>,
) -> impl IntoView {
    let seconds = Memo::new(move |_| timer.seconds(now.get()));
    view! {
        <span class="fa-clock">
            <Countdown timer=timer now=now />
            <span>{move || face(seconds.get())}</span>
        </span>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_runs_at_its_rate_and_never_backward() {
        let clock = Clock {
            at: 1_000.0,
            rate: 2.0,
            running: true,
        };
        assert_eq!(clock.now(0.0), 1_000.0);
        assert_eq!(clock.now(250.0), 1_500.0);
        assert_eq!(clock.now(-5.0), 1_000.0);
    }

    #[test]
    fn a_stopped_clock_stays_at_its_latest_cue() {
        let clock = Clock {
            at: 9_000.0,
            ..Clock::default()
        };
        assert_eq!(clock.now(60_000.0), 9_000.0);
    }

    #[test]
    fn a_timer_counts_down_its_limit_from_its_start() {
        let timer = Timer {
            began: 1_000,
            limit_ms: 20_000,
            stopped: None,
        };
        assert_eq!(timer.left(0.0), 20_000.0);
        assert_eq!(timer.left(6_000.0), 15_000.0);
        assert_eq!(timer.seconds(6_500.0), 15);
        assert_eq!(timer.share(11_000.0), 0.5);
        assert_eq!(timer.left(30_000.0), 0.0);
        assert_eq!(timer.seconds(30_000.0), 0);
        let none = Timer {
            began: 0,
            limit_ms: 0,
            stopped: None,
        };
        assert_eq!(none.share(0.0), 0.0);
    }

    #[test]
    fn a_stopped_timer_keeps_what_it_had_left_when_it_stopped() {
        let timer = Timer {
            began: 1_000,
            limit_ms: 20_000,
            stopped: Some(6_000),
        };
        assert_eq!(timer.left(3_000.0), 18_000.0);
        assert_eq!(timer.left(30_000.0), 15_000.0);
        assert_eq!(timer.seconds(30_000.0), 15);
    }

    #[test]
    fn a_face_shows_minutes_and_seconds() {
        assert_eq!(face(53), "0:53");
        assert_eq!(face(72), "1:12");
        assert_eq!(face(600), "10:00");
    }
}
