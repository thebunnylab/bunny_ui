//! Opt-in receipts for one image feed in one native window.
//!
//! Desktop shells attach these with `App::observe_feed`. A receipt proves the
//! platform reported a drawable presentation, not that every feed pixel was
//! unobscured or that photons reached the reader. Submission and GPU completion
//! are not presentation. See `docs/feed.md` for platform and timing limits.

#[cfg(feature = "gpu")]
mod gpu;

use crate::image_engine::FeedKey;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

/// Why the current path cannot provide a presentation receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Unavailable {
    /// Observation begins after attachment, without retroactive credit.
    AwaitingFrame,
    /// The shell uses CPU drawing without a presentation timestamp.
    Cpu,
    /// This platform has no implemented receipt source.
    UnsupportedPlatform,
    /// Native bands, software layers, or partial/reused patches lack receipts.
    NativeCompositing,
    /// No live draw of the selected feed intersects the drawable's clip.
    NotDrawn,
    /// Multiple snapshots, a transformed source, or a non-monotonic generation.
    UnsupportedSnapshot,
    /// The GPU texture does not contain the requested generation.
    UploadFailed,
    /// Three earlier observations still await the platform. No drop is inferred.
    Backpressure,
    /// No drawable was available; no native commit was observed.
    NoDrawable,
    /// No render command could be created for this drawable.
    CommandUnavailable,
    /// The platform lacks the required presentation timestamp selectors.
    TimestampUnsupported,
    /// The owning window is hidden or reported occluded by the native shell.
    WindowNotVisible,
}

/// Scalar metadata only. Every event belongs to the observer's feed and session.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum PresentationEvent {
    /// The renderer encountered a new snapshot, not a capture or `ImageFeed::push`.
    Submitted { generation: u64 },
    /// A newer snapshot replaced this generation before any native commit.
    Superseded { generation: u64 },
    /// The native render command completed with an error, so no receipt counts.
    RenderFailed { generation: u64 },
    /// The selected generation was encoded for an actual native drawable.
    Committed { generation: u64 },
    /// Metal's presented handler reported a finite positive `presentedTime`.
    /// Its timestamp is host time in seconds, not Unix time or a Rust `Instant`.
    /// `drawable_id` is local to the layer, so use the session as well.
    Presented {
        generation: u64,
        drawable_id: u64,
        host_time_seconds: f64,
    },
    /// The presented handler reported no valid presentation time. This neither
    /// proves a visible frame nor guesses why the platform skipped it.
    Unconfirmed { generation: u64 },
    /// No trustworthy receipt is available for this path or attempt.
    Unavailable(Unavailable),
    /// The window closed or another observer replaced this subscription.
    Closed,
}

/// Bounded observation state.
///
/// `presented` counts unique generations whose
/// callbacks were accepted, including receipts subsequently evicted from the
/// queue. It is not FPS. Nonzero `lost_events` prevents complete cadence analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationSnapshot {
    pub capacity: usize,
    pub queued: usize,
    pub pending: usize,
    pub lost_events: u64,
    pub presented: u64,
    pub unavailable: Option<Unavailable>,
    pub closed: bool,
}

/// A single consumer for one window/feed subscription.
///
/// Dropping it immediately
/// disables tracking. The shell and callback tickets hold weak references only.
/// Poll from an existing application task; observing does not start a frame clock.
pub struct FeedPresentation {
    session: u64,
    feed: FeedKey,
    state: Arc<Mutex<State>>,
}

impl FeedPresentation {
    /// A process-local unique subscription ID, never a native pointer. A new
    /// subscription has a new ID even if the native window address is reused.
    #[must_use]
    pub const fn session(&self) -> u64 {
        self.session
    }

    /// The selected feed's stable slot.
    #[must_use]
    pub const fn feed(&self) -> FeedKey {
        self.feed
    }

    /// Remove up to `capacity` pending events in delivery order. Native callback
    /// delivery can differ from timestamp order; sort timestamps for cadence.
    #[must_use]
    pub fn drain(&self) -> Vec<PresentationEvent> {
        lock(&self.state).events.drain(..).collect()
    }

    /// Read counters and availability without consuming events.
    #[must_use]
    pub fn snapshot(&self) -> PresentationSnapshot {
        let state = lock(&self.state);
        PresentationSnapshot {
            capacity: state.capacity,
            queued: state.events.len(),
            pending: state.pending.len(),
            lost_events: state.lost_events,
            presented: state.presented,
            unavailable: state.unavailable,
            closed: state.phase == Phase::Closed,
        }
    }
}

struct State {
    feed: FeedKey,
    capacity: usize,
    events: VecDeque<PresentationEvent>,
    pending: Vec<Pending>, // at most three native attempts
    next_ticket: u64,
    current: Option<u64>,
    current_presented: bool,
    current_committed: bool,
    lost_events: u64,
    presented: u64,
    unavailable: Option<Unavailable>,
    phase: Phase,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Active,
    Suspended,
    Closed,
}

struct Pending {
    id: u64,
    generation: u64,
    rendered: bool,
    presentation: Option<(f64, u64)>,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    // No application callback executes under this lock. Recovering poison keeps
    // a native callback from unwinding across its foreign boundary.
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl State {
    fn emit(&mut self, event: PresentationEvent) {
        if self.events.len() == self.capacity {
            self.events.pop_front();
            self.lost_events = self.lost_events.saturating_add(1);
        }
        self.events.push_back(event);
    }

    fn unavailable(&mut self, reason: Unavailable) {
        if self.unavailable != Some(reason) {
            self.unavailable = Some(reason);
            self.emit(PresentationEvent::Unavailable(reason));
        }
    }
}

/// Native-backend implementation seam. Applications consume receipts from their
/// shell; manually issuing tickets is not evidence of native presentation.
#[doc(hidden)]
pub mod backend {
    #[cfg(feature = "gpu")]
    pub use super::gpu::FrameObservation;
    use super::{FeedPresentation, Pending, Phase, PresentationEvent, State, Unavailable, lock};
    use crate::image_engine::FeedKey;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, Weak};
    static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

    /// The window owns this guard. Dropping it closes the observation even if a
    /// callback or a copied sender outlives the native window.
    pub struct PresentationSource {
        sender: PresentationSender,
    }

    /// A weak producer; never owns a frame, window, or observer queue.
    #[derive(Clone)]
    pub struct PresentationSender {
        state: Weak<Mutex<State>>,
    }

    /// A bounded native callback ticket. Clones cannot issue duplicate credit.
    #[derive(Clone)]
    pub struct PresentationTicket {
        sender: PresentationSender,
        id: u64,
    }

    #[must_use]
    pub fn channel(feed: FeedKey, capacity: usize) -> (FeedPresentation, PresentationSource) {
        let capacity = capacity.clamp(1, 256);
        let state = Arc::new(Mutex::new(State {
            feed,
            capacity,
            events: VecDeque::with_capacity(capacity),
            pending: Vec::with_capacity(3),
            next_ticket: 0,
            current: None,
            current_presented: false,
            current_committed: false,
            lost_events: 0,
            presented: 0,
            unavailable: None,
            phase: Phase::Active,
        }));
        lock(&state).unavailable(Unavailable::AwaitingFrame);
        let source = PresentationSource {
            sender: PresentationSender {
                state: Arc::downgrade(&state),
            },
        };
        (
            FeedPresentation {
                session: NEXT_SESSION.fetch_add(1, Ordering::Relaxed),
                feed,
                state,
            },
            source,
        )
    }

    impl PresentationSource {
        #[must_use]
        pub fn sender(&self) -> PresentationSender {
            self.sender.clone()
        }
    }

    impl Drop for PresentationSource {
        fn drop(&mut self) {
            if let Some(state) = self.sender.state.upgrade() {
                let mut state = lock(&state);
                state.phase = Phase::Closed;
                state.pending.clear();
                state.emit(PresentationEvent::Closed);
            }
        }
    }

    impl PresentationSender {
        #[must_use]
        pub fn feed(&self) -> Option<FeedKey> {
            let state = self.state.upgrade()?;
            let state = lock(&state);
            (state.phase != Phase::Closed).then_some(state.feed)
        }

        /// True when this monotonic generation still needs a receipt. Old
        /// pending callbacks remain valid when a newer generation is submitted.
        #[must_use]
        pub fn submitted(&self, generation: u64) -> bool {
            let Some(state) = self.state.upgrade() else {
                return false;
            };
            let mut state = lock(&state);
            if state.phase != Phase::Active {
                return false;
            }
            if generation == 0 || state.current.is_some_and(|current| generation < current) {
                state.unavailable(Unavailable::UnsupportedSnapshot);
                return false;
            }
            if state.current != Some(generation) {
                if !state.current_committed
                    && let Some(previous) = state.current
                {
                    state.emit(PresentationEvent::Superseded {
                        generation: previous,
                    });
                }
                state.current_committed = false;
                state.current = Some(generation);
                state.current_presented = false;
                state.emit(PresentationEvent::Submitted { generation });
            }
            !state.current_presented
                && !state
                    .pending
                    .iter()
                    .any(|pending| pending.generation == generation)
        }

        pub fn unavailable(&self, reason: Unavailable) {
            if let Some(state) = self.state.upgrade() {
                let mut state = lock(&state);
                if state.phase != Phase::Closed {
                    state.unavailable(reason);
                }
            }
        }

        /// Invalidate pending observations when the native window becomes
        /// hidden/occluded. A later callback cannot credit that interval.
        pub fn suspend(&self) {
            if let Some(state) = self.state.upgrade() {
                let mut state = lock(&state);
                if state.phase != Phase::Closed {
                    state.pending.clear();
                    state.phase = Phase::Suspended;
                    state.unavailable(Unavailable::WindowNotVisible);
                }
            }
        }

        /// Resume only after the owning native window reports visibility again.
        pub fn resume(&self) {
            if let Some(state) = self.state.upgrade() {
                let mut state = lock(&state);
                if state.phase != Phase::Closed {
                    state.phase = Phase::Active;
                }
            }
        }

        #[must_use]
        pub fn commit(&self, generation: u64) -> Option<PresentationTicket> {
            let state = self.state.upgrade()?;
            let mut state = lock(&state);
            if state.phase != Phase::Active
                || state.current != Some(generation)
                || state.current_presented
                || state
                    .pending
                    .iter()
                    .any(|pending| pending.generation == generation)
            {
                return None;
            }
            if state.pending.len() == 3 {
                state.unavailable(Unavailable::Backpressure);
                return None;
            }
            state.next_ticket = state.next_ticket.checked_add(1)?;
            let id = state.next_ticket;
            state.pending.push(Pending {
                id,
                generation,
                rendered: false,
                presentation: None,
            });
            state.current_committed = true;
            state.unavailable = None;
            state.emit(PresentationEvent::Committed { generation });
            drop(state);
            Some(PresentationTicket {
                sender: self.clone(),
                id,
            })
        }
    }

    impl PresentationTicket {
        /// An independent command-completion gate. Success alone never credits
        /// a presentation; the drawable timestamp must also arrive.
        pub fn rendered(&self, success: bool) {
            let Some(state) = self.sender.state.upgrade() else {
                return;
            };
            let mut state = lock(&state);
            if state.phase == Phase::Closed {
                return;
            }
            let Some(index) = state
                .pending
                .iter()
                .position(|pending| pending.id == self.id)
            else {
                return;
            };
            if !success {
                let generation = state.pending.remove(index).generation;
                state.emit(PresentationEvent::RenderFailed { generation });
                return;
            }
            state.pending[index].rendered = true;
            finish(&mut state, index);
            drop(state);
        }

        /// Called only by the platform's presented callback. GPU command
        /// completion, paint, and elapsed-time fallbacks must not call this.
        pub fn presented(&self, host_time_seconds: f64, drawable_id: u64) {
            let Some(state) = self.sender.state.upgrade() else {
                return;
            };
            let mut state = lock(&state);
            if state.phase == Phase::Closed {
                return;
            }
            let Some(index) = state
                .pending
                .iter()
                .position(|pending| pending.id == self.id)
            else {
                return;
            };
            if !host_time_seconds.is_finite() || host_time_seconds <= 0.0 {
                let generation = state.pending.remove(index).generation;
                state.emit(PresentationEvent::Unconfirmed { generation });
                return;
            }
            state.pending[index].presentation = Some((host_time_seconds, drawable_id));
            finish(&mut state, index);
            drop(state);
        }
    }

    fn finish(state: &mut State, index: usize) {
        let pending = &state.pending[index];
        if !pending.rendered {
            return;
        }
        let Some((host_time_seconds, drawable_id)) = pending.presentation else {
            return;
        };
        let generation = state.pending.remove(index).generation;
        state.presented = state.presented.saturating_add(1);
        if state.current == Some(generation) {
            state.current_presented = true;
        }
        state.emit(PresentationEvent::Presented {
            generation,
            drawable_id,
            host_time_seconds,
        });
    }
}
