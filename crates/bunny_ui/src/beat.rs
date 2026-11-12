//! A beat: an intent the application rings and a field answers once.
//!
//! "Find" is an intent. The reader presses the shortcut, and the query
//! field must take the keyboard, with its text selected. The reader then
//! clicks back into the document, and the field must stay away until the
//! shortcut is pressed again.
//!
//! ```ignore
//! #[derive(Clone, Copy)]
//! struct Finder {
//!     find: Beat,
//!     query: State<String>,
//! }
//!
//! // the command
//! self.find.ring();
//!
//! // the view
//! text_field("Find", self.query.binding()).select_on(self.find)
//! ```
//!
//! ## The rule
//!
//! A ring of a [`Beat`] is answered once. The newest ring answered is kept
//! in the `Beat` itself, for as long as the state that owns it lives. So a
//! field that leaves the scene and comes back does not answer the same
//! ring again.
//!
//! That is the difference from [`ViewExt::auto_focus_beat`] with a number.
//! A number is remembered by the field while the field is on screen, and a
//! field that returns answers it again. That is right for a popup whose
//! field is made new on each opening, and it can wear one constant number.
//! It is wrong for a find bar in a tab: the bar leaves when the reader
//! changes tab, and on the way back it would take the keyboard for a
//! shortcut pressed long ago. A `Beat` is kept by the application, where
//! the tab change does not reach it.
//!
//! A ring that no field answered stays pending: the field answers it when
//! it appears. Two fields that wear one beat answer a ring once between
//! them. A beat that never rang summons nobody.
//!
//! [`ViewExt::auto_focus_beat`]: crate::ext::ViewExt::auto_focus_beat

use std::cell::Cell;
use std::rc::Rc;

use motor::state::State;

/// An intent the application rings and a field answers once
/// ([the module](self) has the rule).
///
/// It is a handle like [`State`]: `Copy`, made with [`Beat::new`] where
/// the application keeps its state, and it lives as long as that place.
#[derive(Clone, Copy)]
pub struct Beat(State<Rings>);

/// What a beat holds: how many times it rang, and how far it was answered.
#[derive(Clone)]
struct Rings {
    rung: u64,
    /// The newest ring a field answered. The fields that wear the beat
    /// hold the same cell, so the runtime writes the answer without a
    /// write to the state: answering a ring runs no body.
    answered: Rc<Cell<u64>>,
}

impl Beat {
    /// A beat that has not rung.
    #[track_caller]
    #[must_use]
    pub fn new() -> Self {
        Beat(State::new(Rings { rung: 0, answered: Rc::new(Cell::new(0)) }))
    }

    /// Rings the beat: the field that wears it takes the keyboard on the
    /// next frame. Answers the number of this ring.
    pub fn ring(&self) -> u64 {
        self.0.update(|rings| {
            rings.rung += 1;
            rings.rung
        })
    }

    /// How many times the beat rang. A body that reads it runs again at
    /// the next ring, as with any state.
    #[must_use]
    pub fn rung(&self) -> u64 {
        self.0.with(|rings| rings.rung)
    }

    /// The cell that holds the newest ring answered.
    pub(crate) fn answered(&self) -> Rc<Cell<u64>> {
        self.0.with(|rings| Rc::clone(&rings.answered))
    }
}

impl Default for Beat {
    #[track_caller]
    fn default() -> Self {
        Self::new()
    }
}

/// How a field wears a beat ([`crate::views::TextField::focus_on`],
/// [`crate::views::TextField::select_on`]).
#[derive(Clone, Copy)]
pub(crate) struct Summon {
    pub beat: Beat,
    /// Each ring also selects the field's whole text.
    pub select: bool,
}
