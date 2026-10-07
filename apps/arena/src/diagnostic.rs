//! Untimed native-input diagnostics establish actual settled wheel travel.
use crate::{Step, schedule::wait_until};
use std::time::{Duration, Instant};

/// A settled observation point in the isolated wheel diagnostic.
#[derive(Clone, Copy, Debug)]
pub enum Checkpoint {
    /// The 120 downward inputs have settled.
    Down,
    /// The 120 upward inputs have settled.
    Up,
}
impl Checkpoint {
    /// The structured state marker recorded by the UI thread.
    pub fn marker(self) -> &'static str {
        match self {
            Self::Down => "SCENE_DOWN",
            Self::Up => "SCENE_UP",
        }
    }
}

pub fn wheel(over: (f64, f64), mut send: impl FnMut(Step) -> bool) {
    std::thread::sleep(Duration::from_secs(1));
    for (dy, checkpoint) in [(-6.0, Checkpoint::Down), (6.0, Checkpoint::Up)] {
        let start = Instant::now();
        for index in 0..120 {
            wait_until(start, Duration::from_secs_f64(index as f64 / 240.0));
            if !send(Step::Wheel {
                x: over.0,
                y: over.1,
                dy,
            }) {
                return;
            }
        }
        wait_until(start, Duration::from_millis(1500));
        if !send(Step::Checkpoint(checkpoint)) {
            return;
        }
        // Let the UI observe the settled state before the next leg arrives.
        wait_until(start, Duration::from_millis(1600));
    }
    let _ = send(Step::Done);
}
