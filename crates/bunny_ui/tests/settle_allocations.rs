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
    fn body(self) -> impl View {
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
    fn body(self) -> impl View {
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

/// The allocations `stage` makes in a frame that mounts `count` rows
/// into a page that was mounted empty.
fn mount_of<B, V>(stage: Stage, count: usize, body: B) -> u64
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
    stats::take().allocs(stage)
}

/// The settle allocations of a frame that mounts `count` rows into a
/// page that was mounted empty.
fn settle_of<B, V>(count: usize, body: B) -> u64
where
    B: Fn(Item) -> V + Copy + 'static,
    V: View,
{
    mount_of(Stage::Settle, count, body)
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
/// its path, the retained copy of itself, and the entry and the slot its
/// tree is filed in. Nothing on top of that: a body that opens mints no
/// placeholder path, a frame names no boundary it never prints, the seed
/// of the row's parents is where they end in its path, held inline, the
/// environment it ran in is the page's own, shared, and the note that
/// its body ran is its own path, counted once more.
#[test]
fn a_row_that_mounts_pays_for_its_entry_and_nothing_else() {
    let cost = per_row(|_| empty());
    assert!(cost <= 6 + KEY_CHECK, "an empty row costs the settle {cost} allocations");
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

/// A `text!` of a value that is no state reads nothing, and is no
/// binding: it pays for its closure, its text and the list its node is
/// held in — no key, no binding object, nothing filed under the body.
#[test]
fn a_text_that_reads_nothing_pays_for_its_words_alone() {
    let cost = beyond_empty(|item| {
        let id = item.id;
        text!("{id}")
    });
    assert!(cost <= 3, "a text of no state costs the settle {cost} allocations beyond the empty row");
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

/// A cell in an ink of its own wears the look the rows before it were
/// given: one record of the props is shared by every row, however many
/// modifiers made it, and the styled node pays for the box its child is
/// held in and nothing more.
#[test]
fn a_styled_cell_shares_the_look_of_the_rows_before_it() {
    let plain = beyond_empty(|_| text("a"));
    let styled = beyond_empty(|_| {
        text("a")
            .foreground_color(Color::hex(0x336699))
            .background_color(Color::hex(0x112233))
            .corner_radius(4.0)
    });
    assert!(styled <= plain + 1, "a styled cell costs the settle {styled} allocations, a plain one {plain}");
}

/// A row hints a cell, a link and a glyph in every body it runs, and
/// each hint rides the node it names: a text, a stack or a style that
/// wears a tag, a class and an id costs what it costs bare. A box around
/// each was an allocation per hint per row.
#[test]
fn a_hint_rides_the_node_it_names() {
    let text_bare = beyond_empty(|_| text("a"));
    let text_hinted = beyond_empty(|_| text("a").element("td").css_class("cell").element_id("first"));
    assert!(text_hinted <= text_bare, "a hinted text costs {text_hinted}, a bare one {text_bare}");
    let stack_bare = beyond_empty(|_| hstack!(empty()));
    let stack_hinted = beyond_empty(|_| hstack!(empty()).element("span").css_class("glyph"));
    assert!(stack_hinted <= stack_bare, "a hinted stack costs {stack_hinted}, a bare one {stack_bare}");
    let ink = Color::hex(0x336699);
    let styled_bare = beyond_empty(move |_| text("a").foreground_color(ink));
    let styled_hinted = beyond_empty(move |_| text("a").foreground_color(ink).element("td"));
    assert!(styled_hinted <= styled_bare, "a hinted style costs {styled_hinted}, a bare one {styled_bare}");
}

/// The row of the keyed benchmark: its own class, an id cell, a cell
/// with a link to its bound label, a cell with a link around a glyph,
/// and an empty cell — seven hints and two actions.
fn benchmark_row(item: Item) -> impl View {
    let id = item.id;
    (
        boundary_class_when(item.on, "danger"),
        text(id.to_string()).element("td").css_class("col-md-1"),
        hstack!(text!(item.label).element("a").on_click(|| {})).element("td").css_class("col-md-4"),
        hstack!(
            hstack!(hstack!(empty()).element("span").css_class("glyphicon glyphicon-remove"))
                .element("a")
                .on_click(|| {})
        )
        .element("td")
        .css_class("col-md-1"),
        hstack!(empty()).element("td").css_class("col-md-6"),
    )
}

/// Beyond the empty row the benchmark's row pays for its two bindings,
/// its two texts, the closures of its two clicks and their paths, and
/// the lists its stacks and its actions are held in; its hints and the
/// targets its clicks arm cost nothing.
#[test]
fn a_row_of_the_benchmark_pays_for_its_bindings_and_its_clicks() {
    let cost = beyond_empty(benchmark_row);
    assert!(cost <= 18, "a row of the benchmark costs the settle {cost} allocations beyond the empty row");
}

// MARK: - The lowering

/// A row that mounts is kept as the scene made it: the diff writes the
/// element ids and the looks on the scene's own nodes, where they stand,
/// and keeps the vectors they came in — the scene's list of rows becomes
/// the retention's. Twice the rows cost the diff nothing more than the
/// rows: no vector per parent (the benchmark's row has four, and each
/// took a vector of its own, the nodes copied into it), and none to
/// give back room the capture made and did not fill.
#[test]
fn a_row_that_mounts_is_kept_as_the_scene_made_it() {
    const ROWS: usize = 100;
    let once = mount_of(Stage::Diff, ROWS, benchmark_row);
    let twice = mount_of(Stage::Diff, 2 * ROWS, benchmark_row);
    let per_row = (twice.saturating_sub(once) as f64 / ROWS as f64).round() as u64;
    assert_eq!(per_row, 0, "a row that mounts costs the diff {per_row} allocations ({once} for {ROWS}, {twice} for twice)");
}

/// A row's links are armed in every body it runs, and the action rides
/// the node it arms: a link that answers a click pays for the click's
/// closure, its path and the row's list of actions, and nothing for a
/// target around it.
#[test]
fn a_click_rides_the_node_it_arms() {
    let bare = beyond_empty(|_| text("a").element("a"));
    let armed = beyond_empty(|_| text("a").element("a").on_click(|| {}));
    assert!(armed <= bare + 3, "a link that clicks costs {armed}, one that does not {bare}");
    let bare = beyond_empty(|_| hstack!(hstack!(empty()).element("span")).element("a"));
    let armed = beyond_empty(|_| hstack!(hstack!(empty()).element("span")).element("a").on_click(|| {}));
    assert!(armed <= bare + 3, "a stack that clicks costs {armed}, one that does not {bare}");
}
