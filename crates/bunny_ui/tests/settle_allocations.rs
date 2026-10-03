//! What a row costs the settle when it mounts, counted.
//!
//! A list of a thousand rows runs a thousand bodies, and every
//! allocation a body makes is made a thousand times over: the count per
//! row is the number a frame of rows is built from. These tests pin it
//! for the shapes a row is made of, so an allocation that creeps back
//! into the path every row takes fails here, named, instead of showing
//! up as a slower list in the browser.
//!
//! The count is the settle stage's alone (the bodies, the retention,
//! the bindings); the lowering to elements is another stage. A row's
//! cost is read as a DIFFERENCE — the settle of twice the rows minus
//! the settle of the rows — so what a mount pays once (the page, the
//! list, the tables' first room) cancels out. Each budget is a ceiling
//! at what the path costs today: going under it is progress, going over
//! it is the regression these tests are for.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::rc::Rc;

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
use bunny_ui::runtime::Runtime;
use bunny_ui::stats::{self, Stage};

// MARK: - Counting, per thread

/// The test harness runs each test on a thread of its own: a count kept
/// per thread is the test's own, whatever its neighbours allocate.
struct Counting;

thread_local! {
    static MADE: Cell<u64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // a thread that is ending may no longer have its count: the
        // allocation is still served, only not counted
        let _ = MADE.try_with(|made| made.set(made.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn made() -> u64 {
    MADE.with(Cell::get)
}

/// The stages count only when a clock times them; any clock will do.
fn clock() -> f64 {
    0.0
}

// MARK: - The scene

const SIZE: Size = Size { width: 1200.0, height: 800.0 };

/// What a row is made from: an id, and the two values a row of a table
/// reads for itself — its label and whether it is selected.
#[derive(Clone, Copy)]
struct Item {
    id: usize,
    label: State<Rc<str>>,
    on: State<bool>,
}

/// A row: a boundary of its own, its body whatever the test hands it.
#[derive(Clone, Copy)]
struct Row<B> {
    item: Item,
    body: B,
}

impl<B, V> Component for Row<B>
where
    B: Fn(Item) -> V + Copy + 'static,
    V: View,
{
    fn body(self, _ctx: &Context) -> impl View {
        (self.body)(self.item)
    }
}

/// The page: the list of rows and nothing else.
#[derive(Clone, Copy)]
struct Page<B> {
    items: State<Rc<Vec<Item>>>,
    body: B,
}

impl<B, V> Component for Page<B>
where
    B: Fn(Item) -> V + Copy + 'static,
    V: View,
{
    fn body(self, _ctx: &Context) -> impl View {
        let body = self.body;
        for_each(self.items, |item| item.id.to_string(), move |item| Row { item: *item, body })
    }
}

fn items(count: usize) -> Rc<Vec<Item>> {
    Rc::new(
        (1..=count)
            .map(|id| Item {
                id,
                label: State::new(Rc::from(format!("row {id}").as_str())),
                on: State::new(false),
            })
            .collect(),
    )
}

/// The settle allocations of a frame that mounts `count` rows into a
/// page that was mounted empty.
fn settle_of<B, V>(count: usize, body: B) -> u64
where
    B: Fn(Item) -> V + Copy + 'static,
    V: View,
{
    stats::set_clock(Some(clock));
    stats::set_alloc_probe(Some(made));
    let runtime = Runtime::new();
    let page = Page { items: State::new(Rc::new(Vec::new())), body };
    let _ = runtime.dom_frame(&page, SIZE);
    page.items.set(items(count));
    let _ = stats::take();
    let _ = runtime.dom_frame(&page, SIZE);
    stats::take().allocs(Stage::Settle)
}

/// What one more row costs the settle, rounded: the shared costs of a
/// mount cancel out, and so does the odd growth of a table between
/// the two sizes.
fn per_row<B, V>(body: B) -> u64
where
    B: Fn(Item) -> V + Copy + 'static,
    V: View,
{
    const ROWS: usize = 100;
    let once = settle_of(ROWS, body);
    let twice = settle_of(2 * ROWS, body);
    (twice.saturating_sub(once) as f64 / ROWS as f64).round() as u64
}

/// A debug build checks that a list's keys are unique, a copy of each
/// key kept to compare: two allocations a row that a shipped build
/// never makes.
const KEY_CHECK: u64 = if cfg!(debug_assertions) { 2 } else { 0 };

// MARK: - The budgets

/// A row whose body shows nothing still pays for being a row: its key,
/// its path, the retained copy of itself, the entry and the slot its
/// tree is filed in, and the note that its body ran. Nothing on top of
/// that: a body that opens mints no placeholder path, a frame names no
/// boundary it never prints, the seed of the row's parents is where they
/// end in its path, held inline, and the environment it ran in is the
/// page's own, shared.
#[test]
fn a_row_that_mounts_pays_for_its_entry_and_nothing_else() {
    let cost = per_row(|_| empty());
    assert!(cost <= 7 + KEY_CHECK, "an empty row costs the settle {cost} allocations");
}

/// What a row costs beyond the empty row: the measure of one feature.
fn beyond_empty<B, V>(body: B) -> u64
where
    B: Fn(Item) -> V + Copy + 'static,
    V: View,
{
    per_row(body).saturating_sub(per_row(|_| empty()))
}

/// A label the row reads for itself pays for its key, its binding, the
/// closure it reads through, the text it reads and the list its node
/// is held in. The register of what it reads costs nothing more: one
/// value read by one binding, and the bindings of one body, are held
/// inline — four sets and a list a row, before. And the text is one
/// allocation: formatted in the thread's buffer and shared from there.
#[test]
fn a_bound_label_files_its_reads_without_a_set() {
    let cost = beyond_empty(|item| text!(item.label));
    assert!(cost <= 5, "a bound label costs the settle {cost} allocations beyond the empty row");
}

/// A body of five cells holds its five nodes in one list, made once for
/// the five: it grew from four to eight before, two allocations. And a
/// stack that holds nothing makes no room for the child that adds none
/// — the stack's own node is all the row pays for.
#[test]
fn a_body_takes_the_room_its_tuple_says_and_no_more() {
    let five = beyond_empty(|_| (text("a"), text("b"), text("c"), text("d"), text("e")));
    assert!(five <= 6, "five fixed cells cost the settle {five} allocations beyond the empty row");
    let hollow = beyond_empty(|_| hstack!(empty()));
    assert!(hollow <= 1, "a stack of nothing costs the settle {hollow} allocations beyond the empty row");
}

/// The class a row's own element wears while it is selected is a
/// binding too, filed the same way: no set for the flag it reads, none
/// for the one binding reading that flag. With the label beside it the
/// row's body made two bindings, and two are held without a list. A
/// class named by a literal is not copied while the flag reads false:
/// the binding's closure, its key, the binding and the node's list are
/// all a row pays for it.
#[test]
fn a_bound_class_beside_a_bound_label_files_without_a_list() {
    let class = beyond_empty(|item| boundary_class_when(item.on, "selected"));
    assert!(class <= 4, "a bound class costs the settle {class} allocations beyond the empty row");
    let label = beyond_empty(|item| text!(item.label));
    let both = beyond_empty(|item| (boundary_class_when(item.on, "selected"), text!(item.label)));
    assert!(both <= class + label, "the two bindings of one body cost {both}, apart {class} + {label}");
}
