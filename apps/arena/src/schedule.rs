//! Deadlines belong to the input producer, independently of UI work.

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub(super) enum Cadence {
    Wheel,
    Keystroke,
    Stream,
}

impl Cadence {
    pub(super) fn for_script(script: &str) -> Option<Self> {
        match script {
            "wheel" => Some(Self::Wheel),
            "type" | "append" => Some(Self::Keystroke),
            "stream" => Some(Self::Stream),
            _ => None,
        }
    }

    pub(super) fn count(self, seconds: f64) -> u64 {
        match self {
            Self::Wheel => (seconds * crate::STEPS_PER_SECOND as f64) as u64,
            Self::Keystroke => (seconds * 10.0) as u64,
            Self::Stream => (seconds * 1000.0 / 33.0) as u64,
        }
    }

    pub(super) fn deadline(self, index: u64) -> Duration {
        let rate = match self {
            Self::Wheel => crate::STEPS_PER_SECOND,
            Self::Keystroke => 10,
            Self::Stream => {
                return Duration::from_secs(index / 1000 * 33)
                    + Duration::from_millis(index % 1000 * 33);
            }
        };
        Duration::from_secs(index / rate)
            + Duration::from_nanos(index % rate * 1_000_000_000 / rate)
    }
}

pub(super) fn wait_until(start: Instant, deadline: Duration) {
    if let Some(remaining) = deadline.checked_sub(start.elapsed()) {
        std::thread::sleep(remaining);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_durations_keep_the_same_floor_based_event_count() {
        assert_eq!(Cadence::Wheel.count(10.0), 2400);
        assert_eq!(Cadence::Keystroke.count(10.0), 100);
        assert_eq!(Cadence::Stream.count(10.0), 303);
        assert_eq!(Cadence::Wheel.count(0.105), 25);
        assert_eq!(Cadence::Keystroke.count(0.105), 1);
        assert_eq!(Cadence::Stream.count(0.105), 3);
    }

    #[test]
    fn deadlines_never_accumulate_the_rounded_period() {
        assert_eq!(Cadence::Wheel.deadline(1), Duration::from_nanos(4_166_666));
        assert_eq!(Cadence::Wheel.deadline(240), Duration::from_secs(1));
        assert_eq!(Cadence::Wheel.deadline(7200), Duration::from_secs(30));
        assert_eq!(Cadence::Keystroke.deadline(300), Duration::from_secs(30));
        assert_eq!(Cadence::Stream.deadline(1000), Duration::from_secs(33));
        assert_eq!(Cadence::Stream.deadline(303), Duration::from_millis(9999));
    }
}
