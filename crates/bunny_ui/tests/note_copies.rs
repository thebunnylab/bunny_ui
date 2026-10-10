//! What a keystroke in a long note costs in copies of the note.
//!
//! A note of a megabyte lives in the app's state. A field shows it,
//! the caret follows it and the input method reads it after every frame;
//! each of them used to take a copy of its own, eight a stroke with the
//! edit's. A stroke now copies the note once: the edit changes the text
//! where the state keeps it, and the field turns the new text into the one
//! shared allocation that the layout, the reads and the input method
//! borrow alike. The count is of allocations at least the note's size:
//! a copy of it, or a copy grown into a larger one — never the frame's
//! bookkeeping, nor the table of the note's lines.

extern crate bunny_ui_core as bunny_ui; // the core, by the name an application uses

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use bunny_ui::layout::{Proposal, Size};
use bunny_ui::prelude::*;
use bunny_ui::runtime::Runtime;

struct Counting;

thread_local! {
    /// The size from which an allocation counts as a copy of the note.
    static NOTE: Cell<usize> = const { Cell::new(usize::MAX) };
    static COPIED: Cell<u64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // a thread that is ending may no longer have its counts
        if NOTE.try_with(Cell::get).is_ok_and(|note| layout.size() >= note) {
            let _ = COPIED.try_with(|bytes| bytes.set(bytes.get() + layout.size() as u64));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn copied() -> u64 {
    COPIED.with(Cell::get)
}

#[derive(Clone)]
struct Panel {
    note: State<String>,
}

impl Component for Panel {
    fn body(self) -> impl View {
        text_editor("note", self.note.binding()).frame(400.0, 300.0)
    }
}

#[test]
fn a_keystroke_in_a_long_note_copies_it_once() {
    let note: String = (0..40_000).map(|line| format!("line {line} of a long note\n")).collect();
    let size = note.len() as u64;
    NOTE.with(|note| note.set(size as usize));
    let panel = Panel { note: State::new(note) };
    let runtime = Runtime::new();
    let window = Size { width: 400.0, height: 300.0 };
    runtime.display_frame(&panel, window);
    let layout = runtime.layout(&panel, Proposal::exact(window));
    let path = layout.hits.first().expect("the field is a target").0.clone();
    runtime.focus(&path);
    // the frame a shell draws after a stroke — the edit, the bodies, the
    // layout and the picture — and the input method's read of the field
    let stroke = |text: &str| {
        let before = copied();
        assert!(runtime.key(EditCommand::Insert(text.into())).applied);
        runtime.display_frame(&panel, window);
        let snapshot = runtime.ime_snapshot().expect("the focused note answers the input method");
        assert_eq!(snapshot.text.len(), panel.note.with(String::len), "the snapshot is the note");
        copied() - before
    };
    // the first stroke grows the state's string past its exact size; the
    // strokes after it append within that room
    stroke("x");
    for at in 0..10 {
        let copied = stroke(if at % 2 == 0 { "y" } else { "z" });
        assert!(
            copied <= size + 64,
            "stroke {at} copied the note {:.1} times",
            copied as f64 / size as f64
        );
    }
}

#[test]
fn an_input_method_read_and_collection_release_the_previous_text_version() {
    use std::sync::Arc;

    let panel = Panel { note: State::new("line — é 日本 🦀\n".repeat(300)) };
    let runtime = Runtime::new();
    let window = Size { width: 400.0, height: 300.0 };
    runtime.display_frame(&panel, window);
    let path = runtime.layout(&panel, Proposal::exact(window)).hits[0].0.clone();
    runtime.focus(&path);
    runtime.key(EditCommand::End(false));
    for inserted in ["🦀", "é", "\n", "日本"] {
        let old = runtime.ime_snapshot().expect("focused editor");
        let lifetime = Arc::downgrade(&old.text);
        let old_units = old.selected.0;
        assert!(runtime.key(EditCommand::Insert(inserted.into())).applied);
        runtime.display_frame(&panel, window);
        let current = runtime.ime_snapshot().expect("edited field");
        assert_eq!(current.selected, (old_units + inserted.encode_utf16().count(), 0));
        assert!(current.text.starts_with(&*old.text), "an external snapshot remains immutable");
        // The native host collects the replaced layout after presenting.
        // An outstanding snapshot must survive that same collection.
        runtime.collect_garbage();
        assert!(lifetime.upgrade().is_some());
        drop(old);
        assert!(lifetime.upgrade().is_none(), "the IME bridge must not retain the consumed predecessor after the old layout is collected");
        assert_eq!(runtime.ime_snapshot().unwrap().selected, current.selected);
    }
}

#[test]
fn the_first_input_method_read_also_consumes_the_predecessor() {
    use std::sync::Arc;
    use bunny_ui::layout::DrawCommand;

    let panel = Panel { note: State::new("é 日本 🦀\n".repeat(300)) };
    let runtime = Runtime::new();
    let window = Size { width: 400.0, height: 300.0 };
    let display = runtime.display_frame(&panel, window);
    let old = display.iter().find_map(|command| match command {
        DrawCommand::TextLine { content, .. } => Some(Arc::downgrade(content)),
        _ => None,
    }).expect("the field painted its shared text");
    drop(display);
    let path = runtime.layout(&panel, Proposal::exact(window)).hits[0].0.clone();
    runtime.focus(&path);
    runtime.key(EditCommand::End(false));
    assert!(runtime.key(EditCommand::Insert("x".into())).applied);
    runtime.display_frame(&panel, window);
    runtime.collect_garbage();
    let snapshot = runtime.ime_snapshot().expect("first input-method read");
    assert_eq!(snapshot.selected, (snapshot.text.encode_utf16().count(), 0));
    assert!(old.upgrade().is_none(), "even a full count no longer needs its predecessor");
}
