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
    /// never on the body. A render reads first and makes the binding
    /// after ([`Bound::from_probe`]); this eager door is the tests'.
    #[cfg(test)]
    pub(crate) fn new(key: Rc<str>, eval: Rc<dyn Fn() -> T>) -> Rc<Self> {
        let bound = Rc::new(Bound { key, eval, cache: RefCell::new(None), stale: Cell::new(true) });
        bound.evaluate(true);
        let weak = Rc::downgrade(&bound);
        let weak: Weak<dyn Stale> = weak;
        LIVE.with(|live| {
            live.borrow_mut().insert(Rc::clone(&bound.key), weak);
        });
        bound
    }

    /// Makes the binding at `key` from a reading a probe already made
    /// ([`place_lazy`]): the value is its first, and the reads the probe
    /// filed become the key's, under the view whose body is running.
    fn from_probe(key: Rc<str>, eval: Rc<dyn Fn() -> T>, value: T) -> Rc<Self> {
        motor::identity::file_probe_under_view(&key);
        let bound = Rc::new(Bound { key, eval, cache: RefCell::new(Some(value)), stale: Cell::new(false) });
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
        self.evaluate(false)
    }

    /// Reads under the key. `under_view`: the read is the first, made by
    /// the body that is running — the binding is filed under that view,
    /// so it dies and resets with it.
    fn evaluate(&self, under_view: bool) -> T {
        let value = {
            let _scope = if under_view {
                motor::identity::begin_binding_under_view(&self.key)
            } else {
                motor::identity::begin_binding(&self.key, None)
            };
            (self.eval)()
        };
        *self.cache.borrow_mut() = Some(value.clone());
        self.stale.set(false);
        value
    }

    /// Did the last read depend on anything? A binding that read
    /// nothing is a constant: nothing can ever make it stale.
    #[cfg(test)]
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
/// Diagnostics: how many bindings are registered as live.
pub(crate) fn live_count() -> usize {
    LIVE.with(|live| live.borrow().len())
}

/// The bindings of a view that left: dead to the frame now, whatever
/// retained tree still holds their objects until an idle moment frees
/// it. A key that a newer body made again stays — it is the newer
/// binding's.
pub(crate) fn forget_live(keys: &[Rc<str>]) {
    if keys.is_empty() {
        return;
    }
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        for key in keys {
            live.remove(key);
        }
    });
}

pub(crate) fn key_at_cursor(suffix: &str) -> Option<Rc<str>> {
    motor::identity::cursor_key(suffix)
}

/// A lazy source placed at render: its closure is read once, under a
/// probe, and what the reading touched decides what the node is. A read
/// of anything makes it a binding at the cursor's key (`suffix` says what
/// it is), the probe's reads its own; a reading that touched nothing is
/// its value, fixed — no key minted, no binding made, nothing filed under
/// the body, since nothing can ever move it. Outside a pass there is no
/// key to hang a binding on, and the value is fixed too.
fn place_lazy<T: Clone + 'static>(eval: &Rc<dyn Fn() -> T>, suffix: &str) -> Result<Rc<Bound<T>>, T> {
    let (value, read) = {
        let _probe = motor::identity::begin_probe();
        let value = eval();
        (value, motor::identity::probe_read_anything())
    };
    if !read {
        return Err(value);
    }
    match key_at_cursor(suffix) {
        Some(key) => Ok(Bound::from_probe(key, Rc::clone(eval), value)),
        None => {
            motor::identity::forget_probe();
            Err(value)
        }
    }
}

// MARK: - Text

thread_local! {
    /// The buffer `text!` formats into: a reading writes its words here
    /// and shares them from here, and the room stays for the next one.
    static TEXT_BUFFER: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The room the buffer keeps between readings. A text longer than this
/// is formatted all the same; the buffer just lets the room go after.
const TEXT_BUFFER_KEPT: usize = 4096;

/// A formatted text, shared — what `text!` reads through. The words are
/// written into a buffer the thread keeps and shared from there: one
/// allocation for the text, where a `format!` into a string of its own
/// and the copy into the shared one were two, every time a label is
/// read. A format with no arguments is its literal, shared as it is.
///
/// A value whose `Display` formats a text of its own while this one is
/// being written finds the buffer taken and formats its words apart —
/// the same words, one allocation more.
#[doc(hidden)]
pub fn shared_text(words: std::fmt::Arguments<'_>) -> Arc<str> {
    use std::fmt::Write as _;

    if let Some(literal) = words.as_str() {
        return Arc::from(literal);
    }
    TEXT_BUFFER.with(|buffer| match buffer.try_borrow_mut() {
        Ok(mut buffer) => {
            buffer.clear();
            // the same promise `format!` keeps: only a `Display` that
            // lies about its error can fail a write into a string
            buffer
                .write_fmt(words)
                .expect("a formatting trait implementation returned an error when the underlying stream did not");
            let shared = Arc::from(buffer.as_str());
            if buffer.capacity() > TEXT_BUFFER_KEPT {
                *buffer = String::new();
            }
            shared
        }
        Err(_) => Arc::from(std::fmt::format(words)),
    })
}

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
            TextSource::Lazy(eval) => match place_lazy(eval, "#text") {
                Ok(bound) => TextSource::Bound(bound),
                Err(fixed) => TextSource::Fixed(fixed),
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

// MARK: - Class

/// What a boundary's own element wears as a class: a fixed name, or a
/// value the node reads for itself.
#[derive(Clone)]
pub enum ClassSource {
    Fixed(String),
    /// Not placed yet: the closure waits for the node's key.
    Lazy(Rc<dyn Fn() -> String>),
    Bound(Rc<Bound<String>>),
}

impl ClassSource {
    /// The class now.
    pub fn get(&self) -> String {
        match self {
            ClassSource::Fixed(class) => class.clone(),
            ClassSource::Lazy(eval) => eval(),
            ClassSource::Bound(bound) => bound.get(),
        }
    }

    /// The binding behind the class, when it reads for itself.
    pub fn bound(&self) -> Option<&Rc<Bound<String>>> {
        match self {
            ClassSource::Bound(bound) => Some(bound),
            _ => None,
        }
    }

    /// At render: a lazy source becomes a binding keyed by the cursor —
    /// or a fixed class, when its one read depended on nothing.
    pub(crate) fn place(&self) -> ClassSource {
        match self {
            ClassSource::Lazy(eval) => match place_lazy(eval, "#class") {
                Ok(bound) => ClassSource::Bound(bound),
                Err(fixed) => ClassSource::Fixed(fixed),
            },
            other => other.clone(),
        }
    }
}

impl std::fmt::Debug for ClassSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.get())
    }
}

#[cfg(test)]
mod frame_tests {
    //! The bindings in a frame: what a write to a bound value costs the
    //! engine, on the element road and on the pixel road.

    use std::rc::Rc;

    use crate::dom::DomPatch;
    use crate::layout::Size;
    use crate::prelude::*;
    use crate::runtime::Runtime;
    use crate::stats;
    use crate::views::{boundary_class_when, for_each};

    const SIZE: Size = Size { width: 400.0, height: 300.0 };

    #[derive(Clone, Copy)]
    struct Label {
        count: State<usize>,
    }

    impl Component for Label {
        fn body(self, _ctx: &Context) -> impl View {
            crate::text!("{} rows", self.count)
        }
    }

    /// A `text!` that reads no state is its words, fixed: the frame mints
    /// no key for it, makes no binding and files nothing under the body —
    /// and the words are the ones the format writes. A class that reads
    /// nothing is fixed the same way. Beside them, a text that reads is a
    /// binding with its read filed under its key.
    #[test]
    fn a_node_that_reads_nothing_is_fixed_and_files_nothing() {
        #[derive(Clone, Copy)]
        struct Plain {
            id: usize,
        }

        impl Component for Plain {
            fn body(self, _ctx: &Context) -> impl View {
                let id = self.id;
                (crate::views::boundary_class_with(move || format!("row-{id}")), crate::text!("row {}", id))
            }
        }

        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&Plain { id: 7 }, SIZE);
        assert_eq!(super::live_count(), 0, "no binding was made");
        let [_, _, _, view_bindings, binding_reads, _, _] = motor::identity::registry_counts();
        assert_eq!((view_bindings, binding_reads), (0, 0), "and nothing was filed");
        let printed = runtime.render(&Plain { id: 7 });
        assert!(printed.contains("Text(\"row 7\")") && printed.contains("BoundaryClass(\"row-7\")"), "{printed}");

        let label = Label { count: State::new(3) };
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&label, SIZE);
        assert_eq!(super::live_count(), 1, "a text that reads is a binding");
        let [_, _, _, view_bindings, binding_reads, _, _] = motor::identity::registry_counts();
        assert_eq!((view_bindings, binding_reads), (1, 1), "its read filed under its key, the key under its body");
    }

    /// A text that reads for itself wears the face above it like any
    /// other text: the family named by `.font_family` reaches it.
    #[test]
    fn a_bound_text_wears_the_family_above_it() {
        #[derive(Clone)]
        struct Pair {
            count: State<usize>,
        }

        impl Component for Pair {
            fn body(self, _ctx: &Context) -> impl View {
                crate::vstack!(text("fixed"), crate::text!("{} rows", self.count)).font_family("Menlo")
            }
        }

        let runtime = Runtime::new();
        let mount = runtime.dom_frame(&Pair { count: State::new(1) }, SIZE);
        // the texts' own looks (the root and a box declare a face too)
        let families: Vec<Option<std::sync::Arc<str>>> = mount
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::DefineRule { kind: crate::dom::CreateKind::Text, text: Some(text), .. } => {
                    Some(text.font.family.name())
                }
                _ => None,
            })
            .collect();
        assert!(!families.is_empty(), "{mount:?}");
        assert!(
            families.iter().all(|family| family.as_deref() == Some("Menlo")),
            "every text face names the family: {families:?} in {mount:?}"
        );
    }

    /// The same, in the shape of a keyed row: the label a bound text in
    /// a link inside a cell, the row a component the list keys.
    #[test]
    fn a_bound_label_in_a_keyed_row_wears_the_family_above_it() {
        #[derive(Clone)]
        struct Row {
            item: Item,
            on: State<bool>,
        }

        impl Component for Row {
            fn body(self, _ctx: &Context) -> impl View {
                let id = self.item.id;
                (
                    boundary_class_when(self.on, "danger"),
                    text(id.to_string()).foreground_color(Color::BLACK).element("td").css_class("a"),
                    crate::hstack!(
                        crate::text!("label {}", id)
                            .foreground_color(Color::BLACK)
                            .element("a")
                            .on_click(|| {})
                    )
                    .element("td")
                    .css_class("b"),
                )
            }
        }

        #[derive(Clone)]
        struct Page {
            rows: State<Rc<Vec<Item>>>,
        }

        impl Component for Page {
            fn body(self, _ctx: &Context) -> impl View {
                crate::vstack!(for_each(self.rows, |item| item.id.to_string(), |item| {
                    Row { item: *item, on: State::new(false) }.element("tr")
                }))
                .font_family("Menlo")
                .font_size(14.0)
            }
        }

        let faces_of = |patches: &[DomPatch]| -> Vec<(Option<std::sync::Arc<str>>, f64)> {
            patches
                .iter()
                .filter_map(|patch| match patch {
                    DomPatch::DefineRule { kind: crate::dom::CreateKind::Text, text: Some(text), .. } => {
                        Some((text.font.family.name(), text.font.size))
                    }
                    _ => None,
                })
                .collect()
        };
        let runtime = Runtime::new();
        let page = Page { rows: State::new(items(&[1, 2])) };
        let mount = runtime.dom_frame(&page, SIZE);
        let faces = faces_of(&mount);
        assert!(faces.len() >= 2, "{mount:?}");
        assert!(
            faces.iter().all(|(family, size)| family.as_deref() == Some("Menlo") && *size == 14.0),
            "every text face names the family and the size: {faces:?}"
        );

        // and a table that was EMPTY at the mount: its first rows arrive
        // through the list's own update, and their looks are defined
        // then — under the same family
        let runtime = Runtime::new();
        let page = Page { rows: State::new(items(&[])) };
        let _ = runtime.dom_frame(&page, SIZE);
        page.rows.set(items(&[1, 2]));
        let update = runtime.dom_frame(&page, SIZE);
        let later = faces_of(&update);
        assert!(later.len() >= 2, "the first rows define their looks: {update:?}");
        assert!(
            later.iter().all(|(family, size)| family.as_deref() == Some("Menlo") && *size == 14.0),
            "a row the list adds wears the face above it: {later:?}"
        );

        // and the served page, adopted: the build painted the empty
        // table, the first rows arrive by the click — the same face
        let runtime = Runtime::new();
        let page = Page { rows: State::new(items(&[])) };
        runtime.dom_adopt(&page, SIZE);
        assert!(runtime.dom_frame(&page, SIZE).is_empty(), "the adopted page is already true");
        page.rows.set(items(&[1, 2]));
        let update = runtime.dom_frame(&page, SIZE);
        let adopted = faces_of(&update);
        assert!(adopted.len() >= 2, "the first rows define their looks: {update:?}");
        assert!(
            adopted.iter().all(|(family, size)| family.as_deref() == Some("Menlo") && *size == 14.0),
            "a row added to an adopted page wears the face above it: {adopted:?}"
        );
    }

    /// A row whose shape differs from its neighbour's is built, not
    /// cloned — and the row after it, of the first shape again, still
    /// finds the first row's template.
    #[test]
    fn a_row_of_another_shape_is_built_and_the_shape_after_it_still_clones() {
        #[derive(Clone)]
        struct Row {
            item: Item,
        }

        impl Component for Row {
            fn body(self, _ctx: &Context) -> impl View {
                let id = self.item.id;
                // every third row carries a second cell
                crate::hstack!(text(id.to_string()), (id % 3 == 0).then(|| text("marked"))).element("tr")
            }
        }

        #[derive(Clone)]
        struct Page {
            rows: State<Rc<Vec<Item>>>,
        }

        impl Component for Page {
            fn body(self, _ctx: &Context) -> impl View {
                crate::vstack!(for_each(self.rows, |item| item.id.to_string(), |item| Row { item: *item }))
            }
        }

        let runtime = Runtime::new();
        let page = Page { rows: State::new(items(&[1, 2, 3, 4, 5, 6, 7])) };
        let mount = runtime.dom_frame(&page, SIZE);
        let frame = stats::take();
        let groups = mount
            .iter()
            .filter(|patch| matches!(patch, DomPatch::Create { kind: crate::dom::CreateKind::Group, .. }))
            .count();
        let clones = mount.iter().filter(|patch| matches!(patch, DomPatch::Clone { .. })).count();
        // rows 1 and 3 are the two templates; 2, 4, 5, 7 clone the first,
        // 6 clones the second — across neighbours of the other shape
        assert_eq!(groups, 4, "the page, the list and two template rows: {mount:?}");
        assert_eq!(clones, 5, "{mount:?}");
        assert_eq!(frame.clones, 5);
        // the words of every row reached the page
        let words: Vec<&str> = mount
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::SetContent { text, .. } => Some(&**text),
                _ => None,
            })
            .collect();
        assert_eq!(words, ["1", "2", "3", "marked", "4", "5", "6", "marked", "7"]);
    }

    #[test]
    fn a_bound_label_is_one_text_patch_and_no_body() {
        let label = Label { count: State::new(1) };
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&label, SIZE);
        let _ = stats::take();

        label.count.set(2);
        assert!(runtime.needs_frame(), "the write asks for a frame");
        let patches = runtime.dom_frame(&label, SIZE);
        let frame = stats::take();
        assert!(runtime.body_runs().is_empty(), "no body ran: {:?}", runtime.body_runs());
        assert_eq!(frame.capture_nodes, 0, "the walk built nothing");
        assert_eq!(frame.binding_updates, 1);
        match patches.as_slice() {
            [DomPatch::SetContent { text, .. }] => assert_eq!(&**text, "2 rows"),
            other => panic!("one text patch, got {other:?}"),
        }
        // the print reads it live
        assert!(runtime.render(&label).contains("2 rows"));
    }

    #[test]
    fn a_bound_label_moves_the_pixels_with_no_body() {
        let label = Label { count: State::new(1) };
        let runtime = Runtime::new();
        let before = runtime.display_frame(&label, SIZE);
        label.count.set(2);
        let after = runtime.display_frame(&label, SIZE);
        assert!(runtime.body_runs().is_empty(), "the measure read the new value itself");
        assert_ne!(before.as_slice(), after.as_slice(), "and the picture moved");
    }

    #[derive(Clone, Copy)]
    struct Flag {
        on: State<bool>,
    }

    impl Component for Flag {
        fn body(self, _ctx: &Context) -> impl View {
            (boundary_class_when(self.on, "danger"), text("flag"))
        }
    }

    #[test]
    fn a_bound_class_flips_the_element_alone() {
        let flag = Flag { on: State::new(false) };
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&flag, SIZE);
        let _ = stats::take();

        flag.on.set(true);
        let patches = runtime.dom_frame(&flag, SIZE);
        assert!(runtime.body_runs().is_empty());
        assert_eq!(stats::take().binding_updates, 1);
        match patches.as_slice() {
            [DomPatch::SetHints { class: Some(class), .. }] => assert_eq!(&**class, "danger"),
            other => panic!("one class patch, got {other:?}"),
        }
        flag.on.set(false);
        let patches = runtime.dom_frame(&flag, SIZE);
        assert!(matches!(patches.as_slice(), [DomPatch::SetHints { class: None, .. }]), "{patches:?}");
    }

    /// A class spelled at render — a string of the body's own, not a
    /// literal — is kept as it is and worn the same way.
    #[test]
    fn a_class_spelled_at_render_flips_like_a_literal() {
        #[derive(Clone, Copy)]
        struct Tagged {
            on: State<bool>,
            id: usize,
        }

        impl Component for Tagged {
            fn body(self, _ctx: &Context) -> impl View {
                (boundary_class_when(self.on, format!("tag-{}", self.id)), text("tagged"))
            }
        }

        let tagged = Tagged { on: State::new(false), id: 7 };
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&tagged, SIZE);
        tagged.on.set(true);
        let patches = runtime.dom_frame(&tagged, SIZE);
        match patches.as_slice() {
            [DomPatch::SetHints { class: Some(class), .. }] => assert_eq!(&**class, "tag-7"),
            other => panic!("one class patch, got {other:?}"),
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    struct Item {
        id: usize,
    }

    #[derive(Clone, Copy)]
    struct Row {
        id: usize,
    }

    impl Component for Row {
        fn body(self, _ctx: &Context) -> impl View {
            text(format!("row {}", self.id))
        }
    }

    #[derive(Clone, Copy)]
    struct Table {
        rows: State<Rc<Vec<Item>>>,
    }

    impl Component for Table {
        fn body(self, _ctx: &Context) -> impl View {
            for_each(self.rows, |item| item.id.to_string(), |item| Row { id: item.id })
        }
    }

    fn items(ids: &[usize]) -> Rc<Vec<Item>> {
        Rc::new(ids.iter().map(|id| Item { id: *id }).collect())
    }

    #[test]
    fn rows_of_one_shape_mount_as_clones_of_the_first() {
        let table = Table { rows: State::new(items(&[1, 2, 3, 4, 5])) };
        let runtime = Runtime::new();
        let patches = runtime.dom_frame(&table, SIZE);
        let frame = stats::take();
        // the first row mounts whole and becomes the template; the four
        // after it are one clone each, with their own words
        let groups = patches
            .iter()
            .filter(|patch| matches!(patch, DomPatch::Create { kind: crate::dom::CreateKind::Group, .. }))
            .count();
        let clones = patches.iter().filter(|patch| matches!(patch, DomPatch::Clone { .. })).count();
        let words: Vec<&str> = patches
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::SetContent { text, .. } => Some(&**text),
                _ => None,
            })
            .collect();
        assert_eq!(groups, 3, "the table, the list and the first row: {patches:?}");
        assert_eq!(clones, 4, "{patches:?}");
        assert_eq!(frame.clones, 4);
        // each row after the first is compared with the copy made just
        // before it: only the three groups that mounted whole hashed
        assert_eq!(frame.shapes_hashed, 3, "the table, the list and the first row");
        assert_eq!(words, ["row 1", "row 2", "row 3", "row 4", "row 5"]);

        // the served page agrees: a clone is the template's subtree with
        // the copy's words, numbered as a fresh mount numbers them
        let page = crate::ssr::render(&table, SIZE);
        assert_eq!(page.html.matches("row ").count(), 5, "{}", page.html);
        for id in 1..=5 {
            assert!(page.html.contains(&format!("row {id}")), "{}", page.html);
        }
        // and a mount over the served page adopts it in silence
        let fresh = Runtime::new();
        fresh.dom_adopt(&table, SIZE);
        assert!(fresh.dom_frame(&table, SIZE).is_empty(), "adoption says nothing");

        // a sixth row of the shape is one clone and one word
        let _ = stats::take();
        table.rows.set(items(&[1, 2, 3, 4, 5, 6]));
        let patches = runtime.dom_frame(&table, SIZE);
        assert!(
            matches!(patches.as_slice(), [DomPatch::Clone { .. }, DomPatch::SetContent { text, .. }] if &**text == "row 6"),
            "{patches:?}"
        );

        // emptied and filled again: the rows are made back to front, the
        // first one made mounts whole, and every other one is compared
        // with the row made just before it — one shape hashed, not five
        table.rows.set(items(&[]));
        let _ = runtime.dom_frame(&table, SIZE);
        let _ = stats::take();
        table.rows.set(items(&[7, 8, 9, 10, 11]));
        let patches = runtime.dom_frame(&table, SIZE);
        let frame = stats::take();
        let clones = patches.iter().filter(|patch| matches!(patch, DomPatch::Clone { .. })).count();
        assert_eq!(clones, 4, "{patches:?}");
        assert_eq!(frame.shapes_hashed, 1, "the first row made, alone: {patches:?}");
        let mut words: Vec<&str> = patches
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::SetContent { text, .. } => Some(&**text),
                _ => None,
            })
            .collect();
        words.sort_unstable();
        assert_eq!(words, ["row 10", "row 11", "row 7", "row 8", "row 9"], "every row's words: {patches:?}");
    }

    /// The list, and the table around it, have shapes too — but they hold
    /// the rows' template, and a member answers to one template. Were
    /// they templates as well, the row's template would lose its members
    /// to theirs: retired by nobody when its row left, it would hand the
    /// next row a copy of an element no longer on the page.
    #[test]
    fn a_template_never_holds_another_templates_root() {
        let table = Table { rows: State::new(items(&[1, 2, 3, 4, 5])) };
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&table, SIZE);
        // a row joins (the list's live instance changes), then every row
        // leaves, then rows of the same shape come back
        table.rows.set(items(&[1, 2, 3, 4, 5, 6]));
        let _ = runtime.dom_frame(&table, SIZE);
        table.rows.set(items(&[]));
        let cleared = runtime.dom_frame(&table, SIZE);
        let gone: Vec<(u32, u32)> = cleared
            .iter()
            .flat_map(|patch| match patch {
                DomPatch::RemoveChildren { forget, .. } => forget.clone(),
                _ => Vec::new(),
            })
            .collect();
        assert!(!gone.is_empty(), "the rows left in one op: {cleared:?}");
        table.rows.set(items(&[7, 8, 9]));
        let refilled = runtime.dom_frame(&table, SIZE);
        for patch in &refilled {
            if let DomPatch::Clone { template, .. } = patch {
                assert!(
                    !gone.iter().any(|(start, end)| (*start..*end).contains(template)),
                    "a clone of element {template}, which left the page: {refilled:?}"
                );
            }
        }
        // the first row back mounts whole, and the others copy it
        assert!(
            refilled.iter().any(|patch| matches!(patch, DomPatch::Create { kind: crate::dom::CreateKind::Group, .. })),
            "{refilled:?}"
        );
    }

    #[test]
    fn a_keyed_list_reads_its_rows_and_renders_each_once() {
        let table = Table { rows: State::new(items(&[1, 2, 3, 4, 5])) };
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&table, SIZE);
        // the table, the list and every row once
        assert_eq!(stats::take().entries_indexed, 7, "every row once at the mount");

        // a swap: the list re-runs, no row does, and the wire carries two moves
        table.rows.set(items(&[1, 4, 3, 2, 5]));
        let patches = runtime.dom_frame(&table, SIZE);
        let frame = stats::take();
        assert_eq!(frame.entries_indexed, 1, "the list's own body, and no row's");
        assert_eq!(frame.diff_reused, 5, "every row kept wholesale");
        assert_eq!(patches.len(), 2, "{patches:?}");
        assert!(patches.iter().all(|patch| matches!(patch, DomPatch::Move { .. })), "{patches:?}");

        // a new key runs its row, and only it — mounted as a clone of
        // the rows already there
        table.rows.set(items(&[1, 4, 3, 2, 5, 6]));
        let patches = runtime.dom_frame(&table, SIZE);
        assert_eq!(stats::take().entries_indexed, 2, "the list and the new row");
        let groups = patches
            .iter()
            .filter(|patch| {
                matches!(
                    patch,
                    DomPatch::Create { kind: crate::dom::CreateKind::Group, .. } | DomPatch::Clone { .. }
                )
            })
            .count();
        assert_eq!(groups, 1, "one row mounted: {patches:?}");
        assert!(!patches.iter().any(|patch| matches!(patch, DomPatch::Remove { .. })));

        // a key that left takes its row along
        table.rows.set(items(&[1, 3, 2, 5, 6]));
        let patches = runtime.dom_frame(&table, SIZE);
        assert_eq!(stats::take().entries_indexed, 1, "the list alone");
        assert!(matches!(patches.as_slice(), [DomPatch::Remove { .. }]), "{patches:?}");

        // every row leaves: the list is emptied in one op, and the ids
        // it forgets are the rows' own, in a few ranges
        table.rows.set(items(&[]));
        let patches = runtime.dom_frame(&table, SIZE);
        match patches.as_slice() {
            [DomPatch::RemoveChildren { forget, .. }] => {
                let forgotten: u32 = forget.iter().map(|(start, end)| end - start).sum();
                assert_eq!(forgotten, 10, "five rows of two elements: {forget:?}");
                assert!(forget.len() <= 2, "rows mounted together are one range: {forget:?}");
            }
            other => panic!("one emptying op, got {other:?}"),
        }

        // and rows that replace every old one: one op empties, then the
        // new ones mount — nothing is removed one by one
        table.rows.set(items(&[7, 8]));
        let _ = runtime.dom_frame(&table, SIZE);
        table.rows.set(items(&[9, 10, 11]));
        let patches = runtime.dom_frame(&table, SIZE);
        assert!(matches!(patches.first(), Some(DomPatch::RemoveChildren { .. })), "{patches:?}");
        assert!(!patches.iter().any(|patch| matches!(patch, DomPatch::Remove { .. })), "{patches:?}");
        let mounted = patches
            .iter()
            .filter(|patch| {
                matches!(
                    patch,
                    DomPatch::Create { kind: crate::dom::CreateKind::Group, .. } | DomPatch::Clone { .. }
                )
            })
            .count();
        assert_eq!(mounted, 3, "{patches:?}");
    }

    #[derive(Clone, Copy)]
    struct Shelf {
        rows: State<Rc<Vec<Item>>>,
        head: State<usize>,
        big: State<bool>,
    }

    impl Component for Shelf {
        fn body(self, _ctx: &Context) -> impl View {
            // the head and the font are read by the body on purpose: a
            // change to either re-runs the shelf, and the list under it
            let font = if self.big.get() { Font::Title } else { Font::Body };
            crate::vstack!(
                text(self.head.get().to_string()),
                for_each(self.rows, |item| item.id.to_string(), |item| Row { id: item.id })
            )
            .font(font)
        }
    }

    #[test]
    fn a_kept_row_is_lowered_again_when_its_environment_moves() {
        let shelf = Shelf { rows: State::new(items(&[1, 2, 3])), head: State::new(1), big: State::new(false) };
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&shelf, SIZE);
        let _ = stats::take();

        // the shelf re-runs for its head: the rows are promises, not walks
        shelf.head.set(2);
        let patches = runtime.dom_frame(&shelf, SIZE);
        let frame = stats::take();
        assert_eq!(frame.entries_indexed, 2, "the shelf and the list, no row");
        assert_eq!(frame.diff_reused, 3, "the rows were kept: {patches:?}");
        assert!(matches!(patches.as_slice(), [DomPatch::SetContent { text, .. }] if &**text == "2"), "{patches:?}");

        // the shelf re-runs with another font above the rows: the rows
        // are lowered again, in their new environment — no body of
        // theirs runs for it
        shelf.big.set(true);
        let patches = runtime.dom_frame(&shelf, SIZE);
        let frame = stats::take();
        assert_eq!(frame.entries_indexed, 2, "still no row body");
        assert_eq!(frame.diff_reused, 0, "a moved environment keeps nothing: {patches:?}");
        // the new face is declared once, by the shelf's box, and the
        // texts under it inherit it: two looks defined (the box's with
        // the face, the texts' without a font), worn by the box, the
        // head and every row's text
        let defined = patches.iter().filter(|patch| matches!(patch, DomPatch::DefineRule { .. })).count();
        let worn = patches.iter().filter(|patch| matches!(patch, DomPatch::UseRule { .. })).count();
        assert_eq!((defined, worn), (2, 5), "the head and every row's text wear the new font: {patches:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use crate::stats;

    #[derive(Clone, Copy)]
    struct Row {
        id: usize,
        label: State<Rc<str>>,
    }

    // each row is a boundary of its own, the way a list's rows are
    #[derive(Clone, Copy)]
    struct RowView(Row);

    impl Component for RowView {
        fn body(self, _ctx: &Context) -> impl View {
            let row = self.0;
            crate::hstack!(text(row.id.to_string()), crate::text!(row.label))
        }
    }

    #[derive(Clone)]
    struct Page {
        rows: State<Rc<Vec<Row>>>,
    }

    impl Component for Page {
        fn body(self, _ctx: &Context) -> impl View {
            crate::vstack!(
                text("a page that never runs again"),
                for_each(self.rows, |row| row.id.to_string(), |row| RowView(*row)),
            )
        }
    }

    fn labelled(ids: std::ops::RangeInclusive<usize>) -> Vec<Row> {
        ids.map(|id| Row { id, label: State::new(Rc::from(format!("row {id}").as_str())) }).collect()
    }

    /// Bindings the register holds the reads of.
    fn binding_reads() -> usize {
        motor::identity::registry_counts()[4]
    }

    /// A keyed list under a page that did not run: the rows that leave
    /// the list leave the retention too — their boundaries, their
    /// bindings, their reads. The page is skipped, and a skipped page
    /// must not shelter what the list under it let go. The bindings'
    /// reads are unpicked when the page is idle, with the rows' memory:
    /// until then they are retired, and after it nothing of them stays.
    #[test]
    fn rows_that_leave_a_list_under_a_clean_page_are_swept() {
        let rows = State::new(Rc::new(Vec::new()));
        let runtime = Runtime::new();
        let size = Size { width: 400.0, height: 300.0 };
        let _ = runtime.dom_frame(&Page { rows }, size);
        let empty_boundaries = crate::reconciler::retained_len();
        let empty_bindings = live_count();

        let seeds: Vec<Row> = (1..=5).map(|id| Row { id, label: State::new(Rc::from("one")) }).collect();
        rows.set(Rc::new(seeds));
        let _ = runtime.dom_frame(&Page { rows }, size);
        assert_eq!(crate::reconciler::retained_len(), empty_boundaries + 5, "five rows retained");
        assert_eq!(live_count(), empty_bindings + 5, "five bound labels");

        rows.set(Rc::new(Vec::new()));
        let _ = runtime.dom_frame(&Page { rows }, size);
        assert_eq!(crate::reconciler::retained_len(), empty_boundaries, "the rows left the retention");
        let counts = motor::identity::registry_counts();
        assert_eq!(counts[3], 0, "no view keeps bindings: {counts:?}");
        assert_eq!(motor::identity::retired_count(), 5, "their bindings wait retired for the idle");

        runtime.collect_garbage();
        assert_eq!(live_count(), empty_bindings, "their bindings left with them");
        assert_eq!(binding_reads(), 0, "and their reads");
        assert_eq!(motor::identity::retired_count(), 0);
    }

    /// Between the frame that lets rows go and the idle that unpicks
    /// their bindings, the bindings still stand in the register — and a
    /// write to what they read reaches none of them. Nothing goes dirty,
    /// no frame is asked for them, and the next frame patches nothing:
    /// a row that left never hears a write again, idle or not.
    #[test]
    fn a_row_that_left_hears_no_write_before_the_idle() {
        let size = Size { width: 400.0, height: 300.0 };
        let seeds = labelled(1..=3);
        let rows = State::new(Rc::new(seeds.clone()));
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&Page { rows }, size);

        rows.set(Rc::new(Vec::new()));
        let _ = runtime.dom_frame(&Page { rows }, size);
        assert_eq!(motor::identity::retired_count(), 3, "retired, not yet unpicked");

        seeds[1].label.set(Rc::from("written after it left"));
        assert!(!has_dirty(), "the write reached no binding");
        let need = runtime.frame_need();
        assert!(!need.bindings && !need.dirty, "no frame is asked for a row that left: {need:?}");
        let _ = stats::take();
        let patches = runtime.dom_frame(&Page { rows }, size);
        assert!(patches.is_empty(), "nothing to patch: {patches:?}");
        assert_eq!(stats::take().binding_updates, 0);

        runtime.collect_garbage();
        seeds[2].label.set(Rc::from("and after the idle"));
        assert!(!has_dirty(), "the idle took the reads apart");
        assert_eq!(binding_reads(), 0);
    }

    /// The idle unpicks what a row that left had made — never what the
    /// same key made again before it came. A row that leaves and comes
    /// back in ONE frame (an effect puts it back before the frame rests)
    /// makes its bindings again at the same keys; the idle that follows
    /// leaves them alone, and the next write to its label still patches
    /// its text.
    #[test]
    fn a_key_that_comes_back_before_the_idle_keeps_its_bindings() {
        #[derive(Clone)]
        struct Restoring {
            rows: State<Rc<Vec<Row>>>,
            all: Rc<Vec<Row>>,
        }

        impl Component for Restoring {
            fn body(self, _ctx: &Context) -> impl View {
                let rows = self.rows;
                let all = Rc::clone(&self.all);
                let short = rows.get().len() < all.len();
                crate::vstack!(
                    for_each(rows, |row| row.id.to_string(), |row| RowView(*row)),
                    short.then(move || text("restoring").on_appear(move || rows.set(Rc::clone(&all)))),
                )
            }
        }

        let size = Size { width: 400.0, height: 300.0 };
        let all = Rc::new(labelled(1..=3));
        let rows = State::new(Rc::clone(&all));
        let page = Restoring { rows, all: Rc::clone(&all) };
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&page, size);
        let mounted = live_count();

        // row 2 leaves, and the same frame's effect puts it back
        rows.set(Rc::new(vec![all[0], all[2]]));
        let _ = stats::take();
        let _ = runtime.dom_frame(&page, size);
        assert!(stats::take().body_passes >= 2, "it left and came back in one frame");
        assert_eq!(rows.get().len(), 3, "the effect put it back");
        assert_eq!(motor::identity::retired_count(), 0, "the key is a live binding's again");

        runtime.collect_garbage();
        assert_eq!(live_count(), mounted, "the idle kept the new bindings");
        all[1].label.set(Rc::from("row 2, written"));
        assert!(has_dirty(), "the write reaches the binding made again");
        let patches = runtime.dom_frame(&page, size);
        let texts: Vec<&str> = patches
            .iter()
            .filter_map(|patch| match patch {
                crate::dom::DomPatch::SetContent { text, .. } => Some(&**text),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["row 2, written"], "its text is patched: {patches:?}");

        // and the same, a frame later: the key comes back before an idle
        rows.set(Rc::new(vec![all[0], all[2]]));
        let _ = runtime.dom_frame(&page, size);
        runtime.collect_garbage();
        all[1].label.set(Rc::from("row 2, again"));
        let patches = runtime.dom_frame(&page, size);
        assert!(
            patches.iter().any(|patch| matches!(patch, crate::dom::DomPatch::SetContent { text, .. } if &**text == "row 2, again")),
            "{patches:?}"
        );
    }

    /// A list that clears says one word to the page, and what the element
    /// lowering kept of its rows — their group records, their bindings —
    /// stays in the tables until the idle frees the rows: a thousand rows
    /// were three thousand keys hashed out of them in the click. The idle
    /// takes them out; the tables then hold what stands.
    #[test]
    fn a_clear_leaves_its_groups_and_bindings_for_the_idle() {
        let size = Size { width: 400.0, height: 300.0 };
        let rows = State::new(Rc::new(Vec::new()));
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&Page { rows }, size);
        let empty = runtime.dom_tables();
        rows.set(Rc::new(labelled(1..=3)));
        let _ = runtime.dom_frame(&Page { rows }, size);
        let full = runtime.dom_tables();
        assert!(full.0 > empty.0 && full.1 > empty.1, "rows bind and group: {empty:?} -> {full:?}");

        rows.set(Rc::new(Vec::new()));
        let patches = runtime.dom_frame(&Page { rows }, size);
        let removals: Vec<&crate::dom::DomPatch> = patches
            .iter()
            .filter(|patch| matches!(patch, crate::dom::DomPatch::RemoveChildren { .. } | crate::dom::DomPatch::Remove { .. }))
            .collect();
        assert!(matches!(removals.as_slice(), [crate::dom::DomPatch::RemoveChildren { .. }]), "one word: {patches:?}");
        assert_eq!(runtime.dom_tables(), full, "the rows' groups and bindings wait for the idle");

        runtime.collect_garbage();
        assert_eq!(runtime.dom_tables(), empty, "the idle took them out");
    }

    /// When no idle comes between two frames, the second one takes out
    /// what the first let go before it reads either table: the walk that
    /// promises groups and the frame that patches bindings by key never
    /// meet a row that left.
    #[test]
    fn the_next_frame_takes_out_what_no_idle_did() {
        let size = Size { width: 400.0, height: 300.0 };
        let rows = State::new(Rc::new(Vec::new()));
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&Page { rows }, size);
        let empty = runtime.dom_tables();
        rows.set(Rc::new(labelled(1..=3)));
        let _ = runtime.dom_frame(&Page { rows }, size);
        rows.set(Rc::new(Vec::new()));
        let _ = runtime.dom_frame(&Page { rows }, size);
        assert_ne!(runtime.dom_tables(), empty, "waiting for the idle");

        let patches = runtime.dom_frame(&Page { rows }, size);
        assert!(patches.is_empty(), "nothing moved: {patches:?}");
        assert_eq!(runtime.dom_tables(), empty, "the next frame took them out");
        assert!(runtime.garbage_pending(), "and left the freeing to the idle");
    }

    /// Whatever a frame leaves for the idle, the runtime says there is
    /// something to collect — a list that only re-ran, every row kept,
    /// leaves the tree it replaced — and the diagnostics line names each
    /// kind beside what stands, so a probe that watches for a leak sees
    /// the wait for what it is. After the idle nothing waits.
    #[test]
    fn every_kind_the_idle_waits_for_is_pending_and_counted() {
        let size = Size { width: 400.0, height: 300.0 };
        let rows = State::new(Rc::new(labelled(1..=3)));
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&Page { rows }, size);
        runtime.collect_garbage();
        assert!(!runtime.garbage_pending(), "nothing waits: {}", runtime.retained_counts());

        let mut swapped = (*rows.get()).clone();
        swapped.swap(0, 2);
        rows.set(Rc::new(swapped));
        let _ = runtime.dom_frame(&Page { rows }, size);
        assert_eq!(crate::reconciler::graveyard_len(), 0, "no row left");
        assert!(runtime.garbage_pending(), "the tree the list replaced waits for the idle");
        let counts = runtime.retained_counts();
        assert_eq!(counts.replaced, 1, "the tree the list replaced: {counts}");
        runtime.collect_garbage();
        assert!(!runtime.garbage_pending(), "{}", runtime.retained_counts());

        rows.set(Rc::new(Vec::new()));
        let _ = runtime.dom_frame(&Page { rows }, size);
        let counts = runtime.retained_counts();
        assert_eq!(counts.graveyard, 3, "the rows' entries: {counts}");
        assert_eq!(counts.bindings_retired, 3, "their bindings: {counts}");
        assert_eq!(
            (counts.dom_bindings, counts.dom_bindings_waiting),
            (0, 3),
            "their elements' bindings: {counts}"
        );
        assert_eq!(counts.dom_groups_waiting, 3, "and their groups: {counts}");
        assert!(runtime.garbage_pending());

        runtime.collect_garbage();
        assert!(!runtime.garbage_pending());
        let counts = runtime.retained_counts();
        assert_eq!(
            (counts.graveyard, counts.replaced, counts.buried_actions, counts.bindings_retired),
            (0, 0, 0, 0),
            "{counts}"
        );
        assert_eq!((counts.dom_bindings_waiting, counts.dom_groups_waiting), (0, 0), "{counts}");
    }

    /// A row written to and let go in one click is not patched: its text
    /// was going stale when the frame began, but the frame that removes its
    /// element takes its binding out before it patches the stale ones.
    #[test]
    fn a_row_written_as_it_leaves_is_not_patched() {
        let size = Size { width: 400.0, height: 300.0 };
        let seeds = labelled(1..=3);
        let rows = State::new(Rc::new(seeds.clone()));
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&Page { rows }, size);

        seeds[1].label.set(Rc::from("written as it leaves"));
        rows.set(Rc::new(vec![seeds[0], seeds[2]]));
        let patches = runtime.dom_frame(&Page { rows }, size);
        assert!(patches.iter().any(|patch| matches!(patch, crate::dom::DomPatch::Remove { .. })), "row 2 left: {patches:?}");
        assert!(
            !patches.iter().any(|patch| matches!(patch, crate::dom::DomPatch::SetContent { .. })),
            "the element that left is not patched: {patches:?}"
        );
    }

    /// A text the body makes again at the same key, as a new element — a
    /// sibling mounted before it moved it off its place — files its binding
    /// over the one the old element held, in the frame that buries the old
    /// one. The idle that takes the old element's binding out leaves the new
    /// one's: the next write still patches the text.
    #[test]
    fn a_text_made_again_at_its_key_keeps_its_binding_through_the_idle() {
        #[derive(Clone, Copy)]
        struct Badge;

        impl Component for Badge {
            fn body(self, _ctx: &Context) -> impl View {
                text("new")
            }
        }

        #[derive(Clone, Copy)]
        struct Flagged {
            flag: State<bool>,
            label: State<Rc<str>>,
        }

        impl Component for Flagged {
            fn body(self, _ctx: &Context) -> impl View {
                crate::vstack!(self.flag.get().then_some(Badge), crate::text!(self.label))
            }
        }

        let size = Size { width: 400.0, height: 300.0 };
        let page = Flagged { flag: State::new(false), label: State::new(Rc::from("first")) };
        let runtime = Runtime::new();
        let _ = runtime.dom_frame(&page, size);

        page.flag.set(true);
        let patches = runtime.dom_frame(&page, size);
        assert!(
            patches.iter().any(|patch| matches!(
                patch,
                crate::dom::DomPatch::Remove { .. } | crate::dom::DomPatch::RemoveChildren { .. }
            )),
            "the old text left, and a new one took its key: {patches:?}"
        );
        runtime.collect_garbage();

        page.label.set(Rc::from("second"));
        let patches = runtime.dom_frame(&page, size);
        assert!(
            patches.iter().any(|patch| matches!(patch, crate::dom::DomPatch::SetContent { text, .. } if &**text == "second")),
            "the new text is still patched by its key: {patches:?}"
        );
    }

    #[test]
    fn a_binding_reads_for_itself_and_goes_stale_on_a_write() {
        let count = State::new(1usize);
        motor::identity::begin_pass();
        let key: Rc<str> = Rc::from("Root/#0/#text");
        let bound = Bound::<Arc<str>>::new(key, Rc::new(move || Arc::from(format!("{} rows", count.get()).as_str())));
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
        let bound = Bound::<Arc<str>>::new(Rc::from("Root/#1/#text"), Rc::new(|| Arc::from("fixed")));
        let _ = motor::identity::end_pass();
        assert!(!bound.reads_anything());
    }

    #[test]
    fn a_dropped_binding_leaves_the_register() {
        let count = State::new(0usize);
        motor::identity::begin_pass();
        let bound = Bound::<Arc<str>>::new(Rc::from("Root/#2/#text"), Rc::new(move || Arc::from(count.get().to_string().as_str())));
        let _ = motor::identity::end_pass();
        drop(bound);
        count.set(1);
        // the write still marks the key: the frame drops it as dead
        let dirty = settle_dirty();
        assert_eq!(dirty.len(), 1);
        assert!(LIVE.with(|live| live.borrow().is_empty()), "a dead key leaves on the frame that meets it");
    }

    /// A text formatted in the thread's buffer is the text `format!`
    /// writes, byte for byte, whatever the format asks — a literal, the
    /// flags and widths, a name the format captures, a text past the room
    /// the buffer keeps — and the buffer gives that room back after a long
    /// one.
    #[test]
    fn a_text_formatted_in_the_buffer_is_the_text_format_writes() {
        let count = 7usize;
        let label: Rc<str> = Rc::from("row 7");
        let long = "x".repeat(TEXT_BUFFER_KEPT * 2);
        assert_eq!(&*shared_text(format_args!("fixed")), "fixed");
        assert_eq!(&*shared_text(format_args!("")), "");
        assert_eq!(&*shared_text(format_args!("{count} rows")), format!("{count} rows"));
        assert_eq!(&*shared_text(format_args!("{:>5}|{:<4}|{:#x}|{:?}", count, "ab", 255, label)), format!("{:>5}|{:<4}|{:#x}|{:?}", count, "ab", 255, label));
        assert_eq!(&*shared_text(format_args!("{}", label)), "row 7");
        assert_eq!(&*shared_text(format_args!("{long}{count}")), format!("{long}{count}"));
        assert!(TEXT_BUFFER.with(|buffer| buffer.borrow().capacity()) <= TEXT_BUFFER_KEPT, "the long text's room was let go");
        assert_eq!(&*shared_text(format_args!("{count}")), "7", "and the next text formats as ever");
    }

    /// A value whose `Display` formats a text of its own while the outer
    /// text is being written finds the buffer taken: it formats apart,
    /// and both texts come out whole.
    #[test]
    fn a_text_formatted_inside_another_comes_out_whole() {
        struct Nested(usize);

        impl std::fmt::Display for Nested {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let inner = shared_text(format_args!("<{}>", self.0));
                f.write_str(&inner)
            }
        }

        assert_eq!(&*shared_text(format_args!("a {} b {}", Nested(1), Nested(2))), "a <1> b <2>");
    }
}
