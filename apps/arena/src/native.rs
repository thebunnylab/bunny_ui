//! Windows input is native: an external producer raises every wheel notch
//! and keystroke through the operating system's own queue (`SendInput`),
//! the road a person's hand takes. The app never synthesizes input.
//!
//! Stdin carries observation boundaries only — `BEGIN`, `DOWN`, `UP`,
//! `DONE` — and each one is answered with the scene's actual state, so a
//! run that did less work than its script cannot pass for one that did it.
//! A boundary never mutates the scene and never counts as input.
//!
//! `BEGIN` also starts the streaming scene's producer. Its clock belongs to
//! a worker thread with absolute deadlines, never to the frames it measures.

use std::io::{self, BufRead};
use std::time::{Duration, Instant};

use bunny_ui::prelude::*;

use crate::schedule::{Cadence, wait_until};
use crate::{Args, Checkpoint, Step, first_frame, unix_ms};

/// One observation boundary from the external producer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Boundary {
    /// The scene has settled and the timed stretch is about to start.
    Begin,
    /// The downward wheel leg has settled.
    Down,
    /// The upward wheel leg has settled.
    Up,
    /// The script is over: report the final state and exit.
    Done,
}

impl Boundary {
    fn parse(line: &str) -> Result<Self, String> {
        match line {
            "BEGIN" => Ok(Self::Begin),
            "DOWN" => Ok(Self::Down),
            "UP" => Ok(Self::Up),
            "DONE" => Ok(Self::Done),
            other => Err(format!("unknown observation boundary {other:?}")),
        }
    }

    fn step(self) -> Step {
        match self {
            Self::Begin => Step::Checkpoint(Checkpoint::Ready),
            Self::Down => Step::Checkpoint(Checkpoint::Down),
            Self::Up => Step::Checkpoint(Checkpoint::Up),
            Self::Done => Step::Done,
        }
    }
}

enum Event {
    Boundary(Result<Boundary, String>),
    Append,
}

/// The script, mounted on a view: the first frame is announced, stdin's
/// boundaries are answered on the main thread, and `DONE` ends the process.
/// `on_step` sees every boundary and every streamed append.
pub(super) fn scripted<V: View<Arity = Single>>(
    view: V,
    args: Args,
    on_step: impl Fn(Step) + 'static,
) -> impl View<Arity = Single> {
    let on_step = std::rc::Rc::new(on_step);
    view.task(move || {
        let args = args.clone();
        let on_step = on_step.clone();
        async move {
            first_frame();
            let (sender, receiver) = task::channel::<Event>();
            let boundaries = sender.clone();
            std::thread::spawn(move || read_boundaries(&boundaries));
            let mut begun = false;
            while let Some(event) = receiver.recv().await {
                let boundary = match event {
                    Event::Append => {
                        on_step(Step::Append);
                        continue;
                    }
                    Event::Boundary(Ok(boundary)) => boundary,
                    Event::Boundary(Err(error)) => protocol_error(&error),
                };
                if (boundary == Boundary::Begin) == begun {
                    protocol_error("exactly one BEGIN must precede every other boundary");
                }
                begun = true;
                on_step(boundary.step());
                match boundary {
                    Boundary::Done => {
                        println!("NATIVE_DONE {}", unix_ms());
                        std::process::exit(0);
                    }
                    Boundary::Begin => {
                        if matches!(Cadence::for_script(&args.script), Some(Cadence::Stream)) {
                            let appends = sender.clone();
                            let secs = args.secs;
                            std::thread::spawn(move || stream(secs, &appends));
                        }
                        println!("NATIVE_ACK {} boundary=BEGIN", unix_ms());
                    }
                    Boundary::Down | Boundary::Up => {
                        println!("NATIVE_ACK {} boundary={boundary:?}", unix_ms());
                    }
                }
            }
        }
    })
}

/// Forwards stdin's lines until `DONE`; an ended or unreadable stdin before
/// `DONE` is a protocol failure, never a quiet early exit.
fn read_boundaries(sender: &task::Sender<Event>) {
    for line in io::stdin().lock().lines() {
        let boundary = line
            .map_err(|error| error.to_string())
            .and_then(|line| Boundary::parse(line.trim_end()));
        let terminal = matches!(boundary, Ok(Boundary::Done) | Err(_));
        if sender.send(Event::Boundary(boundary)).is_err() || terminal {
            return;
        }
    }
    let _ = sender.send(Event::Boundary(Err("stdin ended before DONE".to_owned())));
}

/// The streaming producer: one append every 33 ms on absolute deadlines for
/// `secs`, then a receipt of what it queued and how late it ever ran.
fn stream(secs: f64, sender: &task::Sender<Event>) {
    let cadence = Cadence::Stream;
    let count = cadence.count(secs.max(0.1));
    let start = Instant::now();
    let mut max_late = Duration::ZERO;
    for index in 0..count {
        let deadline = cadence.deadline(index);
        wait_until(start, deadline);
        max_late = max_late.max(start.elapsed().saturating_sub(deadline));
        if sender.send(Event::Append).is_err() {
            return;
        }
    }
    println!(
        "STREAM_QUEUED {} count={count} elapsed_ns={} max_late_ns={}",
        unix_ms(),
        start.elapsed().as_nanos(),
        max_late.as_nanos()
    );
}

fn protocol_error(reason: &str) -> ! {
    eprintln!("NATIVE_PROTOCOL_ERROR {reason}");
    std::process::exit(2)
}

#[cfg(test)]
mod tests {
    use super::{Boundary, Checkpoint, Step};

    #[test]
    fn boundaries_name_observations_and_can_never_carry_input() {
        assert_eq!(Boundary::parse("BEGIN"), Ok(Boundary::Begin));
        assert_eq!(Boundary::parse("DOWN"), Ok(Boundary::Down));
        assert_eq!(Boundary::parse("UP"), Ok(Boundary::Up));
        assert_eq!(Boundary::parse("DONE"), Ok(Boundary::Done));
        for not_a_boundary in ["", "begin", "TYPE x", "WHEEL -6", "DONE count=100"] {
            assert!(
                Boundary::parse(not_a_boundary).is_err(),
                "{not_a_boundary:?}"
            );
        }
    }

    #[test]
    fn every_boundary_is_an_observation_or_the_end() {
        assert!(matches!(
            Boundary::Begin.step(),
            Step::Checkpoint(Checkpoint::Ready)
        ));
        assert!(matches!(
            Boundary::Down.step(),
            Step::Checkpoint(Checkpoint::Down)
        ));
        assert!(matches!(
            Boundary::Up.step(),
            Step::Checkpoint(Checkpoint::Up)
        ));
        assert!(matches!(Boundary::Done.step(), Step::Done));
    }
}
