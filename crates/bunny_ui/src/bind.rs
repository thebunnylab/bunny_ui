//! Bindings — a node that reads for itself.
//!
//! A body reads a value and the body re-runs when the value moves: that
//! is the framework's rule, and it costs a body. A BINDING moves the
//! read from the body to the node. `text!("{} rows", count)` keeps a
//! closure and a cache; the node reads the closure when it is placed,
//! the reads land on the node's own key, and a write to `count` leaves
//! the body alone: the node goes stale, the next frame reads it again,
//! and the element lowering patches that one text. Zero bodies for a
//! label that moves, in every mode — on the pixel path the measure
//! reads the new value, on the element path the patch is the text
//! alone.
//!
//! Nothing new is spelled for it. The eager forms stay what they were:
//! `text(string)` is fixed, and `text(label.get())` is a body read.
//!
//! A binding is named by where it stands — the cursor's path at the
//! node, the same path the state it reads anchors by — and it belongs
//! to the body that made it: a body that re-runs makes its bindings
//! again, one that leaves the tree takes them along.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use motor::hash::FxHashMap as HashMap;

/// A value a node reads for itself: the closure, the last value, and
/// whether a write made that value stale.
pub struct Bound<T> {
    key: Rc<str>,
    eval: Rc<dyn Fn() -> T>,
    cache: RefCell<Option<T>>,
    stale: Cell<bool>,
}

/// The one thing the frame says to a binding it cannot name the type
/// of: a write reached you.
trait Stale {
    fn mark_stale(&self);
}

impl<T> Stale for Bound<T> {
    fn mark_stale(&self) {
        self.stale.set(true);
    }
}

thread_local! {
    /// Every live binding by key — the frame marks the dirty ones stale
    /// here, and a binding that dropped leaves on its way out.
    static LIVE: RefCell<HashMap<Rc<str>, Weak<dyn Stale>>> = RefCell::new(HashMap::default());
}

impl<T: Clone + 'static> Bound<T> {
    /// Makes the binding at `key` and reads it once, under the view
    /// whose body is running (`owner`): the reads land on the key,
    /// never on the body.
    pub(crate) fn new(key: Rc<str>, owner: Option<&str>, eval: Rc<dyn Fn() -> T>) -> Rc<Self> {
        let bound = Rc::new(Bound { key, eval, cache: RefCell::new(None), stale: Cell::new(true) });
        bound.evaluate(owner);
        let weak = Rc::downgrade(&bound);
        let weak: Weak<dyn Stale> = weak;
        LIVE.with(|live| {
            live.borrow_mut().insert(Rc::clone(&bound.key), weak);
        });
        bound
    }

    /// Where the binding stands.
    pub fn key(&self) -> &Rc<str> {
        &self.key
    }

    /// The value now: the cache while nothing it read has moved, a
    /// fresh read after.
    pub fn get(&self) -> T {
        if !self.stale.get()
            && let Some(value) = self.cache.borrow().as_ref()
        {
            return value.clone();
        }
        self.evaluate(None)
    }

    fn evaluate(&self, owner: Option<&str>) -> T {
        let value = {
            let _scope = motor::identity::begin_binding(&self.key, owner);
            (self.eval)()
        };
        *self.cache.borrow_mut() = Some(value.clone());
        self.stale.set(false);
        value
    }

    /// Did the last read depend on anything? A binding that read
    /// nothing is a constant: nothing can ever make it stale.
    pub(crate) fn reads_anything(&self) -> bool {
        motor::identity::binding_read_count(&self.key) > 0
    }
}

impl<T> Drop for Bound<T> {
    fn drop(&mut self) {
        // a binding held by a retained tree may drop while the thread
        // itself is ending, after the register is gone: nothing to leave
        let _ = LIVE.try_with(|live| {
            let mut live = live.borrow_mut();
            // the key may already name a newer binding (a body that
            // re-ran made one before this one dropped): only a dead
            // entry leaves
            if live.get(&self.key).is_some_and(|weak| weak.strong_count() == 0) {
                live.remove(&self.key);
            }
        });
    }
}

impl<T> std::fmt::Debug for Bound<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bound({})", self.key)
    }
}

/// Two bindings are the same when they are the same object: a body
/// that re-ran made a new one at the same key, and that one may read
/// something else.
impl<T> PartialEq for Bound<T> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// The frame opens here: the bindings a write reached go stale, and
/// their keys come back for the lowering that patches by key.
pub(crate) fn settle_dirty() -> Vec<Rc<str>> {
    let dirty = motor::identity::take_dirty_bindings();
    if dirty.is_empty() {
        return dirty;
    }
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        for key in &dirty {
            match live.get(key).and_then(Weak::upgrade) {
                Some(bound) => bound.mark_stale(),
                None => {
                    live.remove(key);
                }
            }
        }
    });
    dirty
}

/// Did a write reach a binding since the last frame?
pub(crate) fn has_dirty() -> bool {
    motor::identity::has_dirty_bindings()
}

/// The key of a node at the cursor: a binding is named by where it
/// stands, with a suffix for what it is, the way a measure probe or a
/// hover key is named. `None` outside a pass — a decorative render has
/// no identity to hang a binding on.
pub(crate) fn key_at_cursor(suffix: &str) -> Option<Rc<str>> {
    motor::identity::cursor_scope().map(|scope| Rc::from(format!("{scope}/{suffix}")))
}

// MARK: - Text

/// What a text shows: a fixed string, or a value the node reads for
/// itself.
#[derive(Clone)]
pub enum TextSource {
    Fixed(Arc<str>),
    /// Not placed yet: the closure waits for the node's key, which a
    /// render gives it.
    Lazy(Rc<dyn Fn() -> Arc<str>>),
    Bound(Rc<Bound<Arc<str>>>),
}

impl TextSource {
    /// The text now.
    pub fn get(&self) -> Arc<str> {
        match self {
            TextSource::Fixed(text) => Arc::clone(text),
            TextSource::Lazy(eval) => eval(),
            TextSource::Bound(bound) => bound.get(),
        }
    }

    /// The binding behind the text, when it reads for itself.
    pub fn bound(&self) -> Option<&Rc<Bound<Arc<str>>>> {
        match self {
            TextSource::Bound(bound) => Some(bound),
            _ => None,
        }
    }

    /// At render: a lazy source becomes a binding keyed by the cursor —
    /// or a fixed string, when its one read depended on nothing.
    pub(crate) fn place(&self) -> TextSource {
        match self {
            TextSource::Lazy(eval) => match key_at_cursor("#text") {
                Some(key) => {
                    let owner = motor::identity::current_view_path();
                    let bound = Bound::new(key, owner.as_deref(), Rc::clone(eval));
                    if bound.reads_anything() {
                        TextSource::Bound(bound)
                    } else {
                        TextSource::Fixed(bound.get())
                    }
                }
                None => TextSource::Fixed(eval()),
            },
            other => other.clone(),
        }
    }
}

impl From<Arc<str>> for TextSource {
    fn from(text: Arc<str>) -> Self {
        TextSource::Fixed(text)
    }
}

impl From<&str> for TextSource {
    fn from(text: &str) -> Self {
        TextSource::Fixed(Arc::from(text))
    }
}

impl From<String> for TextSource {
    fn from(text: String) -> Self {
        TextSource::Fixed(Arc::from(text.as_str()))
    }
}

impl PartialEq for TextSource {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

impl std::fmt::Debug for TextSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TextSource::Bound(bound) => write!(f, "{:?}@{}", bound.get(), bound.key()),
            other => write!(f, "{:?}", other.get()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    #[test]
    fn a_binding_reads_for_itself_and_goes_stale_on_a_write() {
        let count = State::new(1usize);
        motor::identity::begin_pass();
        let key: Rc<str> = Rc::from("Root/#0/#text");
        let bound = Bound::<Arc<str>>::new(key, Some("Root"), Rc::new(move || Arc::from(format!("{} rows", count.get()).as_str())));
        let _ = motor::identity::end_pass();
        assert_eq!(&*bound.get(), "1 rows");
        assert!(bound.reads_anything());
        assert!(!has_dirty(), "a read dirties nothing");

        count.set(2);
        assert!(has_dirty(), "the write reached the binding");
        assert_eq!(&*bound.get(), "1 rows", "nothing read it again yet: the cache stands");
        let dirty = settle_dirty();
        assert_eq!(dirty.len(), 1);
        assert_eq!(&*bound.get(), "2 rows", "stale, read again");
        assert!(!has_dirty());
    }

    #[test]
    fn a_binding_that_reads_nothing_is_a_constant() {
        motor::identity::begin_pass();
        let bound = Bound::<Arc<str>>::new(Rc::from("Root/#1/#text"), Some("Root"), Rc::new(|| Arc::from("fixed")));
        let _ = motor::identity::end_pass();
        assert!(!bound.reads_anything());
    }

    #[test]
    fn a_dropped_binding_leaves_the_register() {
        let count = State::new(0usize);
        motor::identity::begin_pass();
        let bound = Bound::<Arc<str>>::new(Rc::from("Root/#2/#text"), Some("Root"), Rc::new(move || Arc::from(count.get().to_string().as_str())));
        let _ = motor::identity::end_pass();
        drop(bound);
        count.set(1);
        // the write still marks the key: the frame drops it as dead
        let dirty = settle_dirty();
        assert_eq!(dirty.len(), 1);
        assert!(LIVE.with(|live| live.borrow().is_empty()), "a dead key leaves on the frame that meets it");
    }
}
