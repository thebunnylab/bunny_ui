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
                    let bound = Bound::new(key, Rc::clone(eval));
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
            ClassSource::Lazy(eval) => match key_at_cursor("#class") {
                Some(key) => {
                    let bound = Bound::new(key, Rc::clone(eval));
                    if bound.reads_anything() {
                        ClassSource::Bound(bound)
                    } else {
                        ClassSource::Fixed(bound.get())
                    }
                }
                None => ClassSource::Fixed(eval()),
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
        let families: Vec<Option<std::sync::Arc<str>>> = mount
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::DefineRule { text: Some(text), .. } => Some(text.font.family.name()),
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
                    DomPatch::DefineRule { text: Some(text), .. } => Some((text.font.family.name(), text.font.size)),
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
        // the new face is one look, defined once; the head and every
        // row's text wear it
        let defined = patches.iter().filter(|patch| matches!(patch, DomPatch::DefineRule { .. })).count();
        let worn = patches.iter().filter(|patch| matches!(patch, DomPatch::UseRule { .. })).count();
        assert_eq!((defined, worn), (1, 4), "the head and every row's text wear the new font: {patches:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    /// A keyed list under a page that did not run: the rows that leave
    /// the list leave the retention too — their boundaries, their
    /// bindings, their reads. The page is skipped, and a skipped page
    /// must not shelter what the list under it let go.
    #[test]
    fn rows_that_leave_a_list_under_a_clean_page_are_swept() {
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
        assert_eq!(live_count(), empty_bindings, "their bindings left with them");
        let counts = motor::identity::registry_counts();
        assert_eq!(counts[3], 0, "no view keeps bindings: {counts:?}");
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
}
