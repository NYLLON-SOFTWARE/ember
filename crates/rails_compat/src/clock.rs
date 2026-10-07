//! `Time.current` behind a trait, and `ActiveSupport::Testing::TimeHelpers` as [`TestClock`].
//!
//! The models, cookies and sessions read the time from a [`SharedClock`] as they go; what needs
//! it once (verifiers, storage) takes a `now` argument instead. Parity runs freeze the whole app
//! with a [`TestClock`] (`MATCHBOX_FROZEN_TIME`), and tests move one.

use std::sync::{Arc, Mutex, MutexGuard};

use jiff::{SignedDuration, Timestamp};

pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

pub type SharedClock = Arc<dyn Clock>;

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// Real time shifted by [`travel`](Self::travel), or frozen with [`travel_to`](Self::travel_to).
#[derive(Debug, Default)]
pub struct TestClock {
    state: Mutex<TestClockState>,
}

#[derive(Debug, Default)]
struct TestClockState {
    offset: SignedDuration,
    frozen: Option<Timestamp>,
}

impl TestClock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn frozen_at(at: Timestamp) -> Self {
        let clock = Self::new();
        clock.travel_to(at);
        clock
    }

    /// `travel_to`: freezes time at `at`.
    pub fn travel_to(&self, at: Timestamp) {
        self.state().frozen = Some(at);
    }

    /// `travel`: moves the clock forward by `by`, keeping it frozen if it was.
    pub fn travel(&self, by: SignedDuration) {
        let mut state = self.state();
        match state.frozen {
            Some(at) => state.frozen = Some(at + by),
            None => state.offset += by,
        }
    }

    pub fn travel_back(&self) {
        *self.state() = TestClockState::default();
    }

    fn state(&self) -> MutexGuard<'_, TestClockState> {
        self.state.lock().unwrap()
    }
}

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        let state = self.state();
        state.frozen.unwrap_or_else(|| Timestamp::now() + state.offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frozen_clock_moves_only_when_told() {
        let t: Timestamp = "2024-06-01T12:00:00Z".parse().unwrap();
        let clock = TestClock::frozen_at(t);
        assert_eq!(clock.now(), t);
        clock.travel(SignedDuration::from_secs(60));
        assert_eq!(clock.now().to_string(), "2024-06-01T12:01:00Z");
        clock.travel_to(t);
        assert_eq!(clock.now(), t);
    }

    #[test]
    fn travel_shifts_real_time_until_travel_back() {
        let clock = TestClock::new();
        clock.travel(SignedDuration::from_hours(1));
        let shifted = clock.now().duration_since(Timestamp::now());
        assert!(shifted > SignedDuration::from_mins(59) && shifted <= SignedDuration::from_hours(1), "{shifted:?}");

        clock.travel_back();
        assert!(clock.now().duration_since(Timestamp::now()) <= SignedDuration::ZERO);
    }
}
