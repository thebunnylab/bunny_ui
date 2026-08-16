//! ViewInspector's `Inspection<V>` — kept so the ported files stay untouched;
//! visits are no-ops (UI tests are out of scope for the fake port).

use crate::combine::PassthroughSubject;
use std::marker::PhantomData;

/// `let inspection = Inspection<Self>()` — a plain stored field, so it clones
/// with the view.
#[derive(Clone)]
pub struct Inspection<V> {
    /// `let notice = PassthroughSubject<UInt, Never>()`
    pub notice: PassthroughSubject<u32>,
    _phantom: PhantomData<fn() -> V>,
}

impl<V> Inspection<V> {
    pub fn new() -> Self {
        Inspection { notice: PassthroughSubject::new(), _phantom: PhantomData }
    }

    /// `func visit(_ view: V, _ line: UInt)`
    pub fn visit(&self, _view: &V, _line: u32) {}
}

impl<V> Default for Inspection<V> {
    fn default() -> Self {
        Self::new()
    }
}
