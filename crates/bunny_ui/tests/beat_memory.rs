//! A ring of a `Beat` is answered once, also by a field that leaves the
//! scene and comes back.
//!
//! A find bar lives in a tab. The reader summons it, goes back to the
//! document, changes tab and returns. The bar is made new on the way
//! back, and must not take the keyboard for a shortcut pressed before.

extern crate bunny_ui_core as bunny_ui; // the core, by the name an application uses

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use bunny_ui::prelude::*;

// MARK: - Counting both ways, per thread

/// The harness runs each test on a thread of its own, and the engine is
/// single-threaded: a count kept per thread is the test's own.
struct Counting;

thread_local! {
    static MADE: Cell<u64> = const { Cell::new(0) };
    static FREED: Cell<u64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = MADE.try_with(|made| made.set(made.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let _ = FREED.try_with(|freed| freed.set(freed.get() + 1));
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Blocks this thread made and has not freed — what a leak grows.
fn live() -> i64 {
    MADE.with(Cell::get) as i64 - FREED.with(Cell::get) as i64
}

// MARK: - The scene

const SIZE: Size = Size {
    width: 360.0,
    height: 240.0,
};

fn holds(runtime: &Runtime, name: &str) -> bool {
    runtime
        .focused()
        .is_some_and(|path| path.split('/').any(|part| part == format!("[{name}]")))
}

/// Two tabs. The first holds a find bar over an editor; the second holds
/// another document. The beat and the texts are kept above the tabs.
#[derive(Clone, Copy)]
struct Tabs {
    tab: State<usize>,
    find: Beat,
    query: State<String>,
    body: State<String>,
    other: State<String>,
}

impl Tabs {
    fn new() -> Self {
        Self {
            tab: State::new(0),
            find: Beat::new(),
            query: State::new(String::from("needle")),
            body: State::new(String::new()),
            other: State::new(String::new()),
        }
    }
}

impl Component for Tabs {
    fn body(self) -> impl View {
        if self.tab.get() == 0 {
            Either::First(
                vstack!(
                    text_field("Find", self.query.binding())
                        .select_on(self.find)
                        .hug_height()
                        .id("find-query"),
                    text_field("Body", self.body.binding()).id("editor"),
                )
                .id("first-tab"),
            )
        } else {
            Either::Second(text_field("Other", self.other.binding()).id("other").id("second-tab"))
        }
    }
}

fn show(runtime: &Runtime, tabs: &Tabs, tab: usize) {
    tabs.tab.set(tab);
    runtime.display_frame(tabs, SIZE);
}

#[test]
fn a_beat_that_never_rang_summons_nobody() {
    let runtime = Runtime::new();
    let tabs = Tabs::new();
    runtime.display_frame(&tabs, SIZE);
    assert_eq!(runtime.focused(), None);
    assert_eq!(tabs.find.rung(), 0);
}

#[test]
fn a_summoned_query_does_not_take_the_keys_again_when_its_tab_returns() {
    let runtime = Runtime::new();
    let tabs = Tabs::new();
    runtime.display_frame(&tabs, SIZE);
    assert!(runtime.focus_named("editor"));

    // the shortcut: the query takes the keyboard, its text selected
    assert_eq!(tabs.find.ring(), 1);
    runtime.display_frame(&tabs, SIZE);
    assert!(holds(&runtime, "find-query"), "the ring summoned the query: {:?}", runtime.focused());
    assert!(runtime.key(EditCommand::Insert("x".into())).applied);
    assert_eq!(tabs.query.get(), "x", "selected whole, so the key replaced it");

    // the reader goes back to the document, then away and back
    assert!(runtime.focus_named("editor"));
    show(&runtime, &tabs, 1);
    assert!(!holds(&runtime, "find-query"));
    show(&runtime, &tabs, 0);
    assert!(
        !holds(&runtime, "find-query"),
        "the bar came back with its tab; the ring it answered before it left is not answered again"
    );
    // further passes do not change that
    runtime.display_frame(&tabs, SIZE);
    assert!(!holds(&runtime, "find-query"));

    // the shortcut again is a new intent
    assert_eq!(tabs.find.ring(), 2);
    runtime.display_frame(&tabs, SIZE);
    assert!(holds(&runtime, "find-query"), "a new ring summons the query again");
}

#[test]
fn a_ring_nobody_answered_is_still_pending_on_return() {
    let runtime = Runtime::new();
    let tabs = Tabs::new();
    runtime.display_frame(&tabs, SIZE);
    show(&runtime, &tabs, 1);

    // the bar is not on screen when the beat rings
    tabs.find.ring();
    runtime.display_frame(&tabs, SIZE);
    assert!(!holds(&runtime, "find-query"));

    show(&runtime, &tabs, 0);
    assert!(holds(&runtime, "find-query"), "the field answers the pending ring when it appears");
}

/// Two fields wear one beat.
#[derive(Clone, Copy)]
struct Pair {
    summon: Beat,
    first: State<String>,
    second: State<String>,
}

impl Component for Pair {
    fn body(self) -> impl View {
        vstack!(
            text_field("First", self.first.binding()).focus_on(self.summon).id("first"),
            text_field("Second", self.second.binding()).focus_on(self.summon).id("second"),
        )
    }
}

#[test]
fn two_fields_wearing_one_beat_answer_a_ring_once() {
    let runtime = Runtime::new();
    let pair = Pair {
        summon: Beat::new(),
        first: State::new(String::new()),
        second: State::new(String::new()),
    };
    runtime.display_frame(&pair, SIZE);
    pair.summon.ring();
    runtime.display_frame(&pair, SIZE);
    assert!(holds(&runtime, "first"), "the first field in reading order answers");

    assert!(runtime.blur());
    for _ in 0..3 {
        runtime.display_frame(&pair, SIZE);
        assert_eq!(runtime.focused(), None, "the second field does not answer a ring already answered");
    }
}

#[test]
fn a_thousand_returns_keep_no_memory() {
    let runtime = Runtime::new();
    let tabs = Tabs::new();
    runtime.display_frame(&tabs, SIZE);
    tabs.find.ring();
    runtime.display_frame(&tabs, SIZE);

    let round_trip = || {
        show(&runtime, &tabs, 1);
        show(&runtime, &tabs, 0);
    };
    // the tables reach their size, and what left is freed
    for _ in 0..50 {
        round_trip();
    }
    runtime.collect_garbage();
    let before = live();
    for _ in 0..1000 {
        round_trip();
    }
    runtime.collect_garbage();
    let grown = live() - before;
    assert!(
        grown <= 16,
        "a field that leaves and returns keeps nothing per return: {grown} blocks more after 1000 returns"
    );
}
