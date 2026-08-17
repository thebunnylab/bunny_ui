//! `bunny_ui` — the typed layer over the `motor` engine.
//!
//! The same instrumented runtime (render tree, `render_stable`,
//! per-site effects), now monomorphic: views are generic values
//! (`VStack<(Text, Button<…>)>`), `body` returns `impl View` and
//! erasure lives only at the borders of real dynamism — [`Erased`] for
//! sheets and `ViewModifier`s, the effect queue and the per-site slots.
//!
//! ```ignore
//! #[derive(Clone, Copy)]
//! struct Counter {
//!     count: State<i32>,
//! }
//!
//! impl Component for Counter {
//!     fn body(self, _ctx: &Context) -> impl View {
//!         vstack!(
//!             text!("count: {}", self.count),
//!             button(text("increment"), move || self.count.add(1)),
//!         )
//!     }
//! }
//! ```
//!
//! Three guarantees from this layer, on top of the engine:
//!
//! - `State<T>` is `Copy` — state-only views derive `Copy` and closures
//!   capture `self` without ceremony, like Swift structs;
//! - `on_change`/`on_receive` sites come from `#[track_caller]` — each
//!   callsite is its own slot, no manual string;
//! - arity in the type ([`Single`]/[`Many`]) — a modifier on a raw tuple
//!   does not compile, instead of silently decorating the wrong node.
//!
//! [`Erased`]: crate::erased::Erased
//! [`Single`]: crate::view::Single
//! [`Many`]: crate::view::Many

#![forbid(unsafe_code)]

pub mod action;
pub mod anim;
pub mod dom;
pub mod effects;
pub mod erased;
pub mod ext;
pub mod layout;
pub mod modifier;
pub mod one_of;
pub mod raster;
mod reconciler;
pub mod runtime;
pub mod state_ext;
pub mod text_engine;
pub mod text_input;
pub mod theme;
pub mod view;
pub(crate) mod viewport;
pub mod views;

/// `text!("Count: {}", self.count)` — the built-in `format!` of text.
/// Displaying a `State` READS the value: the dependency registers itself.
#[macro_export]
macro_rules! text {
    ($($arg:tt)*) => {
        $crate::views::text(::std::format!($($arg)*))
    };
}

/// `vstack!(a, b, c)` — the children without the tuple's doubled parentheses.
#[macro_export]
macro_rules! vstack {
    ($($child:expr),+ $(,)?) => {
        $crate::views::vstack(($($child,)+))
    };
}

/// `hstack!(a, b, c)` — see [`vstack!`].
#[macro_export]
macro_rules! hstack {
    ($($child:expr),+ $(,)?) => {
        $crate::views::hstack(($($child,)+))
    };
}

/// `zstack!(a, b, c)` — see [`vstack!`].
#[macro_export]
macro_rules! zstack {
    ($($child:expr),+ $(,)?) => {
        $crate::views::zstack(($($child,)+))
    };
}

pub mod prelude {
    pub use crate::action::{ActionId, Key, KeyPattern};
    pub use crate::anim::Spring;
    pub use crate::erased::{CustomModifier, Erased, erased};
    pub use crate::{hstack, text, vstack, zstack};
    pub use crate::ext::ViewExt;
    pub use crate::layout::{Color, Rendering, Truncation, VisualProps};
    pub use crate::theme::{self, Theme};
    pub use crate::text_engine::{FontDesign, FontSpec, PixelFont, TextEngine, Weight};
    pub use crate::text_input::{CaretState, EditCommand};
    pub use crate::one_of::{OneOf3, OneOf4, OneOf5, OneOf6, OneOf7, OneOf8};
    pub use crate::runtime::{Edited, ImeSnapshot, Runtime};
    pub use crate::state_ext::{BindingExt, StateExt};
    pub use crate::view::{Component, Either, Many, Single, UnaryView, View};
    pub use crate::views::*;

    // The engine, re-exported: the app only needs this prelude. Nominal
    // names from the mirrored port do not cross the public border.
    pub use std::rc::Rc;
    pub use motor::combine::{AnyPublisher, IntoPublisher, PassthroughSubject, Store};
    pub use motor::loadable::{Loadable, LoadableSubject, LoadError};
    pub use motor::runtime::Site;
    pub use motor::state::{
        Binding, Context, Environment, EnvironmentValues, FromEnvironment, Locale, ProvidesQueries,
        State,
    };
    pub use motor::views::{
        ContentMode, Edge, Font, ListStyle, NavigationPath, ProgressViewStyle, Query,
        TextAlignment,
    };
}

#[cfg(test)]
mod tests {
    use crate::prelude::*;
    use std::cell::RefCell;

    #[test]
    fn renders_the_same_tree_through_the_facade() {
        struct Cell;

        impl Component for Cell {
            fn body(self, _ctx: &Context) -> impl View {
                vstack((
                    text("United States").font(Font::Title),
                    text("Population 125000000").font(Font::Caption),
                ))
                .alignment(HorizontalAlignment::Leading)
                .padding()
            }
        }

        impl Clone for Cell {
            fn clone(&self) -> Self {
                Cell
            }
        }

        let printed = Runtime::new().render_stable(&Cell);
        assert!(printed.contains("Cell"));
        assert!(printed.contains("VStack (alignment: .leading) [.padding()]"));
        assert!(printed.contains("Text(\"United States\") [.font(.title)]"));
    }

    #[test]
    fn state_handles_are_copy_so_views_can_be_too() {
        // The handle is Copy: each closure captures its own implicit copy —
        // no `let this = self.clone()` named per role. (State-only views
        // derive `Copy` whole; see the on_change tests.)
        let count = State::new(0);
        let increment = button(text("increment"), move || count.update(|c| *c += 1));
        increment.tap();
        increment.tap();
        assert_eq!(count.get(), 2);
    }

    #[test]
    fn on_change_fires_only_when_the_value_moves() {
        #[derive(Clone, Copy)]
        struct Probe {
            flag: State<bool>,
            seen: State<Vec<bool>>,
        }

        impl Component for Probe {
            fn body(self, _ctx: &Context) -> impl View {
                text("probe").on_change(
                    move || self.flag.get(),
                    false,
                    move |_, new| self.seen.update(|seen| seen.push(*new)),
                )
            }
        }

        let probe = Probe {
            flag: State::new(false),
            seen: State::new(Vec::new()),
        };
        let runtime = Runtime::new();

        // initial: false → the slot just learns the value, nothing fires
        runtime.render_stable(&probe);
        assert!(probe.seen.get().is_empty());

        // the value moves → fires once, and settles
        probe.flag.set(true);
        runtime.render_stable(&probe);
        assert_eq!(probe.seen.get(), vec![true]);
    }

    #[test]
    fn distinct_callsites_get_distinct_slots() {
        // Two `on_change` of the same type, no manual site: `#[track_caller]`
        // gives each line its own slot — neither leaks into the other's.
        #[derive(Clone, Copy)]
        struct Pair {
            value: State<i32>,
            first: State<i32>,
            second: State<i32>,
        }

        impl Component for Pair {
            fn body(self, _ctx: &Context) -> impl View {
                (
                    text("a").on_change(
                        move || self.value.get(),
                        false,
                        move |_, new| self.first.set(*new),
                    ),
                    text("b").on_change(
                        move || self.value.get(),
                        false,
                        move |_, new| self.second.set(*new),
                    ),
                )
            }
        }

        let pair = Pair {
            value: State::new(0),
            first: State::new(-1),
            second: State::new(-1),
        };
        let runtime = Runtime::new();
        runtime.render_stable(&pair);
        pair.value.set(7);
        runtime.render_stable(&pair);
        assert_eq!(pair.first.get(), 7);
        assert_eq!(pair.second.get(), 7);
    }

    #[test]
    fn row_state_follows_the_item_key_and_dies_with_it() {
        // The proof of the three ownership semantics: row state (a) survives
        // across renders, (b) follows the key through a reorder, (c) dies
        // with the identity and comes back zeroed on a remount. The onAppear
        // log is the detector: one mount = one appear.
        #[derive(Clone)]
        struct LoadRow {
            name: String,
            loaded: State<bool>,
            appeared: Rc<RefCell<Vec<String>>>,
        }

        impl Component for LoadRow {
            fn body(self, _ctx: &Context) -> impl View {
                if self.loaded.get() {
                    Either::First(text(format!("{} ready", self.name)))
                } else {
                    let loaded = self.loaded;
                    let log = self.appeared.clone();
                    let name = self.name.clone();
                    Either::Second(text(format!("{} loading", self.name)).on_appear(move || {
                        log.borrow_mut().push(name.clone());
                        loaded.set(true);
                    }))
                }
            }
        }

        #[derive(Clone)]
        struct Board {
            items: State<Vec<&'static str>>,
            appeared: Rc<RefCell<Vec<String>>>,
        }

        impl Component for Board {
            fn body(self, _ctx: &Context) -> impl View {
                let appeared = self.appeared.clone();
                list(
                    self.items.get(),
                    |item| item.to_string(),
                    move |item| LoadRow {
                        name: item.to_string(),
                        // built INSIDE the row: anchors to the item's key
                        loaded: State::new(false),
                        appeared: appeared.clone(),
                    },
                )
            }
        }

        let board = Board {
            items: State::new(vec!["A", "B"]),
            appeared: Rc::new(RefCell::new(Vec::new())),
        };
        let runtime = Runtime::new();

        let printed = runtime.render_stable(&board);
        assert!(printed.contains("A ready") && printed.contains("B ready"));
        assert_eq!(*board.appeared.borrow(), vec!["A", "B"]);

        // reordering does not reset: the state followed the key, no new appear
        board.items.set(vec!["B", "A"]);
        let printed = runtime.render_stable(&board);
        assert!(printed.contains("A ready") && printed.contains("B ready"));
        assert_eq!(*board.appeared.borrow(), vec!["A", "B"]);

        // removing unmounts; putting back is a new mount — state zeroed, appear again
        board.items.set(vec!["B"]);
        runtime.render_stable(&board);
        board.items.set(vec!["B", "A"]);
        let printed = runtime.render_stable(&board);
        assert!(printed.contains("A ready"));
        assert_eq!(*board.appeared.borrow(), vec!["A", "B", "A"]);
    }

    #[test]
    fn set_dirties_only_the_views_that_read() {
        #[derive(Clone, Copy)]
        struct Digit {
            n: State<i32>,
        }

        impl Component for Digit {
            fn body(self, _ctx: &Context) -> impl View {
                text(format!("{}", self.n.get()))
            }
        }

        #[derive(Clone, Copy)]
        struct Duo {
            a: Digit,
            b: Digit,
        }

        impl Component for Duo {
            fn body(self, _ctx: &Context) -> impl View {
                vstack((self.a, self.b))
            }
        }

        let duo = Duo {
            a: Digit { n: State::new(0) },
            b: Digit { n: State::new(0) },
        };
        let runtime = Runtime::new();
        runtime.render_stable(&duo);

        // writing to `a`'s state dirties ONLY who read it — the Digit at
        // position #0 — never the sibling. This is the fine-grained
        // invalidation the real engine will use to re-run only the hit bodies.
        duo.a.n.set(1);
        let dirty = runtime.take_dirty();
        assert_eq!(dirty.len(), 1, "exactly one dirty view: {dirty:?}");
        assert!(dirty[0].contains("#0"), "the tuple position identifies the sibling: {dirty:?}");
        assert!(dirty[0].ends_with("Digit"));
    }

    #[test]
    fn only_the_dirty_body_reruns_and_the_rest_comes_from_cache() {
        #[derive(Clone, Copy)]
        struct Digit {
            n: State<i32>,
        }

        impl Component for Digit {
            fn body(self, _ctx: &Context) -> impl View {
                text(format!("{}", self.n.get()))
            }
        }

        #[derive(Clone, Copy)]
        struct Duo {
            a: Digit,
            b: Digit,
        }

        impl Component for Duo {
            fn body(self, _ctx: &Context) -> impl View {
                vstack((self.a, self.b))
            }
        }

        let duo = Duo {
            a: Digit { n: State::new(0) },
            b: Digit { n: State::new(0) },
        };
        let runtime = Runtime::new();
        runtime.render_stable(&duo);

        // a set on `a`'s state: the parent (Duo) stays SKIPPED — only Digit #0
        // re-runs, isolated, from the retained value; the sibling comes from the cache
        duo.a.n.set(5);
        let printed = runtime.render(&duo);
        assert_eq!(runtime.body_runs(), vec!["Duo/#0/Digit".to_string()]);
        assert!(printed.contains("Text(\"5\")"));
        assert!(printed.contains("Text(\"0\")"), "the untouched sibling, from the cache");

        // and the pass with no dirt at all runs no body
        let printed = runtime.render(&duo);
        assert!(runtime.body_runs().is_empty());
        assert!(printed.contains("Text(\"5\")"));

        // oracle: the incremental prints byte for byte what the full prints
        let incremental = runtime.render(&duo);
        let full = runtime.render_full(&duo);
        assert_eq!(incremental, full);
    }

    #[test]
    fn animated_colors_move_then_snap_and_the_skip_reconverges() {
        use crate::anim::Spring;
        use crate::layout::Color;

        const OFF: Color = Color { r: 40, g: 40, b: 200, a: 255 };
        const ON: Color = Color { r: 200, g: 40, b: 40, a: 255 };

        #[derive(Clone, Copy)]
        struct Chip {
            on: State<bool>,
        }

        impl Component for Chip {
            fn body(self, _ctx: &Context) -> impl View {
                let color = if self.on.get() { ON } else { OFF };
                text("chip").background_color(color).animated(Spring::smooth())
            }
        }

        // the control: the same scene, never animated — the oracle for
        // both endpoints of the motion
        #[derive(Clone, Copy)]
        struct Plain {
            on: State<bool>,
        }

        impl Component for Plain {
            fn body(self, _ctx: &Context) -> impl View {
                let color = if self.on.get() { ON } else { OFF };
                text("chip").background_color(color)
            }
        }

        let size = crate::layout::Size { width: 120.0, height: 40.0 };
        let fill_of = |display: &crate::layout::DisplayList| {
            display
                .iter()
                .find_map(|command| match command {
                    crate::layout::DrawCommand::FillRect { color, .. } => Some(*color),
                    _ => None,
                })
                .expect("the chip paints a background")
        };

        let chip = Chip { on: State::new(false) };
        let runtime = Runtime::new();
        // mount seeds silently: the animated frame IS the plain frame
        let mounted = runtime.display_frame(&chip, size);
        assert_eq!(fill_of(&mounted), OFF, "first appearance does not animate");
        assert!(!runtime.wants_frame());

        // the state flips: the same frame still paints the OLD color —
        // motion starts on the next tick, not with a jump
        chip.on.set(true);
        let flipped = runtime.display_frame(&chip, size);
        assert_eq!(fill_of(&flipped), OFF, "the flip frame holds the start value");
        assert!(runtime.wants_frame(), "the spring is armed");

        // one tick: the color is in flight, strictly between the ends
        assert!(runtime.tick(1.0 / 120.0));
        let moving = fill_of(&runtime.animation_frame(&chip, size));
        assert_ne!(moving, OFF);
        assert_ne!(moving, ON);

        // run it dry: bounded, and the settle SNAPS bit-exact
        let mut guard = 0;
        while runtime.wants_frame() && guard < 600 {
            runtime.tick(1.0 / 120.0);
            let _ = runtime.animation_frame(&chip, size);
            guard += 1;
        }
        assert!(guard < 600, "the spring settles");
        let settled = runtime.animation_frame(&chip, size);

        let plain = Plain { on: State::new(true) };
        let control = Runtime::new();
        let target = control.display_frame(&plain, size);
        assert_eq!(
            settled.as_slice(),
            target.as_slice(),
            "a finished animation repaints byte-for-byte the plain frame — the skip reconverges"
        );
    }

    #[test]
    fn dead_identities_release_their_input_state() {
        #[derive(Clone, Copy)]
        struct Swap {
            editing: State<bool>,
            value: State<String>,
        }

        impl Component for Swap {
            fn body(self, _ctx: &Context) -> impl View {
                if self.editing.get() {
                    Either::First(
                        list(
                            vec![1],
                            |row| format!("row{row}"),
                            move |_| text_field("type…", self.value.binding()),
                        ),
                    )
                } else {
                    Either::Second(text("plain"))
                }
            }
        }

        let size = crate::layout::Size { width: 200.0, height: 100.0 };
        let swap = Swap { editing: State::new(true), value: State::new("hi".into()) };
        let runtime = Runtime::new();
        let _ = runtime.display_frame(&swap, size);
        let field = runtime
            .layout(&swap, crate::layout::Proposal::exact(size))
            .fields
            .first()
            .expect("field placed")
            .clone();
        runtime.focus(&field.path);
        assert!(runtime.focused().is_some());
        // the list retains a scroll offset at its region site
        runtime.set_scroll_offset("Swap/@First", crate::layout::Point { x: 0.0, y: 3.0 });

        // the arm swaps: the field's identity dies — focus and caret go
        swap.editing.set(false);
        let _ = runtime.display_frame(&swap, size);
        assert_eq!(runtime.focused(), None, "a dead field cannot own the keyboard");

        // the offset SURVIVES the unmount: remounting restores it
        swap.editing.set(true);
        let _ = runtime.display_frame(&swap, size);
        assert_eq!(runtime.scroll_offset("Swap/@First").y, 3.0);
    }

    #[test]
    fn the_input_system_reads_indices_and_rects_from_the_live_field() {
        #[derive(Clone, Copy)]
        struct Form {
            value: State<String>,
        }

        impl Component for Form {
            fn body(self, _ctx: &Context) -> impl View {
                text_field("type…", self.value.binding())
            }
        }

        let size = crate::layout::Size { width: 200.0, height: 60.0 };
        let form = Form { value: State::new("hello".to_string()) };
        let runtime = Runtime::new();
        let _ = runtime.display_frame(&form, size);
        let field = runtime
            .layout(&form, crate::layout::Proposal::exact(size))
            .fields
            .first()
            .expect("the field placed")
            .clone();
        runtime.focus(&field.path);
        let _ = runtime.display_frame(&form, size);

        // pixel font: 8px per glyph — 20px in sits inside glyph 2
        let index = runtime
            .ime_index_at(field.text_origin.x + 20.0, field.frame.origin.y + 4.0)
            .expect("inside the field");
        assert_eq!(index, 2);
        // outside the field there is no index
        assert!(runtime.ime_index_at(field.frame.origin.x - 5.0, 500.0).is_none());

        // the rect for index 2 sits two glyphs in
        let rect = runtime.ime_rect_for(2).expect("focused field answers");
        assert_eq!(rect.origin.x, field.text_origin.x + 16.0);
        assert!(rect.size.height > 0.0);
    }

    #[test]
    fn rows_share_a_first_baseline_and_boxes_sit_on_their_bottom() {
        #[derive(Clone, Copy)]
        struct Rowline;

        impl Component for Rowline {
            fn body(self, _ctx: &Context) -> impl View {
                hstack!(
                    // a baselineless 20px box: its baseline IS its bottom
                    spacer().frame(10.0, 20.0),
                    // pixel font: ascent 13 — the text drops 20−13 = 7
                    text("hi"),
                    // padded text: ascent 13 + 5 of top inset = 18 —
                    // the whole padded box drops 2, the glyph lands at 7
                    text("pad").padding_edge(motor::views::Edge::Top, 5.0),
                )
                .alignment(VerticalAlignment::Baseline)
            }
        }

        let size = crate::layout::Size { width: 200.0, height: 60.0 };
        let runtime = Runtime::new();
        let frame = runtime.display_frame(&Rowline, size);
        let line_y = |needle: &str| {
            frame
                .iter()
                .find_map(|command| match command {
                    crate::layout::DrawCommand::TextLine { origin, content, .. }
                        if content.as_ref() == needle =>
                    {
                        Some(origin.y)
                    }
                    _ => None,
                })
                .expect("the text paints")
        };
        assert_eq!(line_y("hi"), 7.0, "ascent 13 on a shared baseline of 20");
        assert_eq!(line_y("pad"), 7.0, "padding forwards the baseline");
    }

    #[test]
    fn a_virtual_list_materializes_only_the_window() {
        #[derive(Clone, Copy)]
        struct Big;

        impl Component for Big {
            fn body(self, _ctx: &Context) -> impl View {
                virtual_list(10_000, |row| format!("row{row}"), |row| {
                    text(format!("item {row}"))
                })
            }
        }

        let size = crate::layout::Size { width: 200.0, height: 100.0 };
        let lines = |display: &crate::layout::DisplayList| {
            display
                .iter()
                .filter(|command| {
                    matches!(command, crate::layout::DrawCommand::TextLine { .. })
                })
                .count()
        };

        let runtime = Runtime::new();
        // first frame: no geometry yet — the fixed first window rules
        let mounted = runtime.display_frame(&Big, size);
        assert!(lines(&mounted) <= 256, "first window is bounded");
        // second frame: the retained geometry shrinks the window to the
        // viewport plus one viewport of buffer on each side
        let settled = runtime.display_frame(&Big, size);
        let visible = lines(&settled);
        assert!(visible < 40, "window-sized, not count-sized: {visible}");
        // the scroll geometry still sees ALL ten thousand rows
        let result = runtime.layout(&Big, crate::layout::Proposal::exact(size));
        let region = result.scrolls.first().expect("the region exists");
        assert_eq!(region.content.height, 16.0 * 10_000.0);
        assert_eq!(region.row_extent, Some(16.0));
    }

    #[test]
    fn a_small_virtual_list_paints_like_the_dense_one() {
        #[derive(Clone, Copy)]
        struct VirtualTen;

        impl Component for VirtualTen {
            fn body(self, _ctx: &Context) -> impl View {
                virtual_list(10, |row| format!("row{row}"), |row| {
                    text(format!("item {row}"))
                })
            }
        }

        #[derive(Clone, Copy)]
        struct DenseTen;

        impl Component for DenseTen {
            fn body(self, _ctx: &Context) -> impl View {
                list(
                    (0..10).collect::<Vec<_>>(),
                    |row| format!("row{row}"),
                    |row| text(format!("item {row}")),
                )
            }
        }

        let size = crate::layout::Size { width: 200.0, height: 400.0 };
        let virtual_runtime = Runtime::new();
        let dense_runtime = Runtime::new();
        let virtual_frame = virtual_runtime.display_frame(&VirtualTen, size);
        let dense_frame = dense_runtime.display_frame(&DenseTen, size);
        assert_eq!(
            virtual_frame.as_slice(),
            dense_frame.as_slice(),
            "everything fits: the virtual list IS the dense list, byte for byte"
        );
    }

    #[test]
    fn empty_and_tiny_virtual_lists_hold() {
        #[derive(Clone, Copy)]
        struct Empty;

        impl Component for Empty {
            fn body(self, _ctx: &Context) -> impl View {
                virtual_list(0, |row| format!("row{row}"), |row| {
                    text(format!("item {row}"))
                })
            }
        }

        #[derive(Clone, Copy)]
        struct One;

        impl Component for One {
            fn body(self, _ctx: &Context) -> impl View {
                virtual_list(1, |row| format!("row{row}"), |row| {
                    text(format!("item {row}"))
                })
            }
        }

        let size = crate::layout::Size { width: 200.0, height: 100.0 };
        let runtime = Runtime::new();
        let empty = runtime.display_frame(&Empty, size);
        assert_eq!(
            empty
                .iter()
                .filter(|command| {
                    matches!(command, crate::layout::DrawCommand::TextLine { .. })
                })
                .count(),
            0
        );
        let one = Runtime::new().display_frame(&One, size);
        assert!(one.iter().any(|command| matches!(
            command,
            crate::layout::DrawCommand::TextLine { .. }
        )));
        // degenerate viewport: nothing to see, nothing to break
        let flat = Runtime::new()
            .display_frame(&One, crate::layout::Size { width: 200.0, height: 0.0 });
        let _ = flat.len();
    }

    #[test]
    fn web_shaped_fixture_heals_after_a_violent_wheel() {
        use crate::anim::Spring;
        use std::rc::Rc as StdRc;

        #[derive(Clone)]
        struct WebFinder {
            selected: State<usize>,
            visible: State<StdRc<Vec<usize>>>,
        }

        impl Component for WebFinder {
            fn body(self, _ctx: &Context) -> impl View {
                let visible = self.visible.get();
                let count = visible.len();
                let selected = self.selected;
                let selected_index = selected.get().min(count.saturating_sub(1));
                virtual_list(count, move |row| format!("row{row}"), move |row| {
                    let on = row == selected_index;
                    text(format!("item {row}"))
                        .background_color(if on {
                            crate::layout::Color { r: 9, g: 9, b: 9, a: 255 }
                        } else {
                            crate::layout::Color { r: 0, g: 0, b: 0, a: 0 }
                        })
                        .animated(Spring::snappy())
                        .on_click(move || selected.set(row))
                })
                .reveal(selected_index)
            }
        }

        let size = crate::layout::Size { width: 300.0, height: 200.0 };
        let finder = WebFinder {
            selected: State::new(0),
            visible: State::new(StdRc::new((0..10_000).collect())),
        };
        let runtime = Runtime::new();
        let _ = runtime.display_frame(&finder, size);
        let _ = runtime.display_frame(&finder, size);
        // a violent wheel: thousands of px in one event
        assert!(runtime.wheel(50.0, 100.0, 0.0, -50000.0));
        let frame = runtime.display_frame(&finder, size);
        let lines = frame
            .iter()
            .filter_map(|command| match command {
                crate::layout::DrawCommand::TextLine { content, .. } => {
                    Some(content.as_ref().to_string())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!lines.is_empty(), "the band re-materialized: {lines:?}");
        assert!(
            lines.iter().any(|line| line.contains("item 312")),
            "rows near the far offset painted: {lines:?}"
        );
    }

    #[test]
    fn a_jump_past_the_buffer_heals_in_the_same_frame() {
        #[derive(Clone, Copy)]
        struct Big;

        impl Component for Big {
            fn body(self, _ctx: &Context) -> impl View {
                virtual_list(10_000, |row| format!("row{row}"), |row| {
                    text(format!("item {row}"))
                })
            }
        }

        let size = crate::layout::Size { width: 200.0, height: 100.0 };
        let has_line = |display: &crate::layout::DisplayList, needle: &str| {
            display.iter().any(|command| match command {
                crate::layout::DrawCommand::TextLine { content, .. } => {
                    content.as_ref() == needle
                }
                _ => false,
            })
        };

        let runtime = Runtime::new();
        let _ = runtime.display_frame(&Big, size);
        let _ = runtime.display_frame(&Big, size);
        // a jump FAR past the buffer: the retained window cannot cover
        // it — the miss invalidates the boundary and the same frame
        // re-materializes around the new offset
        runtime.set_scroll_offset("Big", crate::layout::Point { x: 0.0, y: 8000.0 });
        let jumped = runtime.display_frame(&Big, size);
        assert!(
            has_line(&jumped, "item 500"),
            "the window re-materialized around row 500"
        );
        assert!(
            !has_line(&jumped, "item 0"),
            "the old window is gone"
        );
    }

    #[test]
    fn reveal_reaches_a_row_far_outside_the_window() {
        #[derive(Clone, Copy)]
        struct Picker {
            selected: State<usize>,
        }

        impl Component for Picker {
            fn body(self, _ctx: &Context) -> impl View {
                virtual_list(10_000, |row| format!("row{row}"), |row| {
                    text(format!("item {row}"))
                })
                .reveal(self.selected.get())
            }
        }

        let size = crate::layout::Size { width: 200.0, height: 100.0 };
        let picker = Picker { selected: State::new(0) };
        let runtime = Runtime::new();
        let _ = runtime.display_frame(&picker, size);
        assert_eq!(runtime.scroll_offset("Picker").y, 0.0);

        // the selection jumps to row 9000: the pin gives the follow-up
        // a real frame, the offset bottom-aligns it, and the window
        // re-materializes around it — all inside one frame
        picker.selected.set(9000);
        let jumped = runtime.display_frame(&picker, size);
        assert_eq!(
            runtime.scroll_offset("Picker").y,
            9000.0 * 16.0 + 16.0 - 100.0,
            "the region bottom-aligns the revealed row"
        );
        assert!(jumped.iter().any(|command| match command {
            crate::layout::DrawCommand::TextLine { content, .. } => {
                content.as_ref() == "item 9000"
            }
            _ => false,
        }));
        // the wheel stays sovereign afterwards
        assert!(runtime.wheel(10.0, 10.0, 0.0, 32.0));
        let _ = runtime.display_frame(&picker, size);
        assert_eq!(runtime.scroll_offset("Picker").y, 9000.0 * 16.0 + 16.0 - 132.0);
    }

    #[test]
    fn off_window_rows_die_and_are_born_again() {
        use std::cell::RefCell;

        thread_local! {
            static APPEARED: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
        }

        #[derive(Clone, Copy)]
        struct Lazy;

        impl Component for Lazy {
            fn body(self, _ctx: &Context) -> impl View {
                virtual_list(10_000, |row| format!("row{row}"), |row| {
                    text(format!("item {row}"))
                        .on_appear(move || APPEARED.with(|log| log.borrow_mut().push(row)))
                })
            }
        }

        let size = crate::layout::Size { width: 200.0, height: 100.0 };
        let runtime = Runtime::new();
        APPEARED.with(|log| log.borrow_mut().clear());
        let _ = runtime.display_frame(&Lazy, size);
        runtime.pump();
        let first_mount =
            APPEARED.with(|log| log.borrow().iter().filter(|row| **row == 0).count());
        assert!(first_mount >= 1, "row 0 appeared on mount");

        // jump far away: the old window unmounts (lazy, by contract)…
        runtime.set_scroll_offset("Lazy", crate::layout::Point { x: 0.0, y: 8000.0 });
        let _ = runtime.display_frame(&Lazy, size);
        runtime.pump();
        // …and coming back REMOUNTS row 0: onAppear fires again
        runtime.set_scroll_offset("Lazy", crate::layout::Point { x: 0.0, y: 0.0 });
        let _ = runtime.display_frame(&Lazy, size);
        runtime.pump();
        let reborn =
            APPEARED.with(|log| log.borrow().iter().filter(|row| **row == 0).count());
        assert!(
            reborn > first_mount,
            "the row was born again: {first_mount} then {reborn}"
        );
    }

    #[test]
    fn a_shrinking_count_clamps_the_window_and_the_offset() {
        #[derive(Clone, Copy)]
        struct Shrinker {
            count: State<usize>,
        }

        impl Component for Shrinker {
            fn body(self, _ctx: &Context) -> impl View {
                let count = self.count.get();
                virtual_list(count, |row| format!("row{row}"), |row| {
                    text(format!("item {row}"))
                })
            }
        }

        let size = crate::layout::Size { width: 200.0, height: 100.0 };
        let shrinker = Shrinker { count: State::new(10_000) };
        let runtime = Runtime::new();
        let _ = runtime.display_frame(&shrinker, size);
        runtime.set_scroll_offset("Shrinker", crate::layout::Point { x: 0.0, y: 8000.0 });
        let _ = runtime.display_frame(&shrinker, size);

        // the filter empties almost everything: the window clamps, the
        // retained offset re-clamps at place, the content shrinks
        shrinker.count.set(5);
        let shrunk = runtime.display_frame(&shrinker, size);
        assert!(shrunk.iter().any(|command| match command {
            crate::layout::DrawCommand::TextLine { content, .. } => {
                content.as_ref() == "item 0"
            }
            _ => false,
        }));
        let result = runtime.layout(&shrinker, crate::layout::Proposal::exact(size));
        assert_eq!(result.scrolls.first().expect("region").content.height, 80.0);
    }

    #[test]
    fn animated_rows_slide_on_reorder_and_settle_on_the_real_frame() {
        use crate::anim::Spring;

        #[derive(Clone, Copy)]
        struct Board {
            flipped: State<bool>,
        }

        impl Component for Board {
            fn body(self, _ctx: &Context) -> impl View {
                let items =
                    if self.flipped.get() { vec!["b", "a"] } else { vec!["a", "b"] };
                for_each(items, |id| id.to_string(), |id| {
                    text(*id).animated(Spring::smooth())
                })
            }
        }

        let size = crate::layout::Size { width: 80.0, height: 64.0 };
        let line_of = |display: &crate::layout::DisplayList, needle: &str| {
            display
                .iter()
                .find_map(|command| match command {
                    crate::layout::DrawCommand::TextLine { origin, content, .. }
                        if content.as_ref() == needle =>
                    {
                        Some(origin.y)
                    }
                    _ => None,
                })
                .expect("the row paints its text")
        };

        let board = Board { flipped: State::new(false) };
        let runtime = Runtime::new();
        let mounted = runtime.display_frame(&board, size);
        assert_eq!(line_of(&mounted, "a"), 0.0);
        assert_eq!(line_of(&mounted, "b"), 16.0);
        assert!(!runtime.wants_frame(), "mount seeds at rest");

        // the flip: same frame still paints the OLD positions
        board.flipped.set(true);
        let held = runtime.display_frame(&board, size);
        assert_eq!(line_of(&held, "a"), 0.0, "the flight starts from rest");
        assert_eq!(line_of(&held, "b"), 16.0);
        assert!(runtime.wants_frame());

        // in flight: strictly between the endpoints
        runtime.tick(1.0 / 120.0);
        let flying = runtime.animation_frame(&board, size);
        let a = line_of(&flying, "a");
        let b = line_of(&flying, "b");
        assert!(a > 0.0 && a < 16.0, "a slides down: {a}");
        assert!(b > 0.0 && b < 16.0, "b slides up: {b}");

        // run dry: the settled frame IS the plain frame
        let mut guard = 0;
        while runtime.wants_frame() && guard < 600 {
            runtime.tick(1.0 / 120.0);
            let _ = runtime.animation_frame(&board, size);
            guard += 1;
        }
        assert!(guard < 600, "the slide settles");
        let settled = runtime.animation_frame(&board, size);
        let control = Runtime::new();
        let target =
            control.display_frame(&Board { flipped: State::new(true) }, size);
        assert_eq!(settled.as_slice(), target.as_slice());
    }

    #[test]
    fn scrolling_never_bends_an_animated_row() {
        use crate::anim::Spring;

        #[derive(Clone, Copy)]
        struct Rows;

        impl Component for Rows {
            fn body(self, _ctx: &Context) -> impl View {
                list(
                    (0..10).collect::<Vec<_>>(),
                    |row| format!("row{row}"),
                    |row| text(format!("item {row}")).animated(Spring::smooth()),
                )
            }
        }

        #[derive(Clone, Copy)]
        struct Plain;

        impl Component for Plain {
            fn body(self, _ctx: &Context) -> impl View {
                list(
                    (0..10).collect::<Vec<_>>(),
                    |row| format!("row{row}"),
                    |row| text(format!("item {row}")),
                )
            }
        }

        let size = crate::layout::Size { width: 100.0, height: 100.0 };
        let runtime = Runtime::new();
        let _ = runtime.display_frame(&Rows, size);
        assert!(!runtime.wants_frame());

        // a wheel scroll moves the anchor, not the springs
        assert!(runtime.wheel(10.0, 10.0, 0.0, -40.0));
        let scrolled = runtime.display_frame(&Rows, size);
        assert!(
            !runtime.wants_frame(),
            "scrolling is 1:1 — no spring armed by the offset"
        );

        let control = Runtime::new();
        let _ = control.display_frame(&Plain, size);
        assert!(control.wheel(10.0, 10.0, 0.0, -40.0));
        let plain = control.display_frame(&Plain, size);
        assert_eq!(scrolled.as_slice(), plain.as_slice());
    }

    #[test]
    fn an_animated_region_reveals_over_ticks_and_the_wheel_cancels() {
        use crate::anim::Spring;

        #[derive(Clone, Copy)]
        struct Revealer {
            selected: State<usize>,
        }

        impl Component for Revealer {
            fn body(self, _ctx: &Context) -> impl View {
                list(
                    (0..10).collect::<Vec<_>>(),
                    |row| format!("row{row}"),
                    |row| text(format!("item {row}")),
                )
                .animated(Spring::smooth())
                .scroll_target(format!("row{}", self.selected.get()))
            }
        }

        let size = crate::layout::Size { width: 100.0, height: 48.0 };
        let revealer = Revealer { selected: State::new(0) };
        let runtime = Runtime::new();
        let _ = runtime.display_frame(&revealer, size);
        assert_eq!(runtime.scroll_offset("Revealer").y, 0.0);

        // the reveal flies instead of snapping
        revealer.selected.set(8);
        let _ = runtime.display_frame(&revealer, size);
        assert_eq!(
            runtime.scroll_offset("Revealer").y,
            0.0,
            "the offset has not jumped"
        );
        assert!(runtime.wants_frame(), "the flight is armed");
        runtime.tick(1.0 / 120.0);
        let mid = runtime.scroll_offset("Revealer").y;
        assert!(mid > 0.0, "the offset is moving: {mid}");

        let mut guard = 0;
        while runtime.wants_frame() && guard < 600 {
            runtime.tick(1.0 / 120.0);
            let _ = runtime.animation_frame(&revealer, size);
            guard += 1;
        }
        assert!(guard < 600, "the reveal settles");
        // the settled offset equals the snap the plain path would take:
        // row 8 bottom (144) minus the viewport (48) = 96
        assert_eq!(runtime.scroll_offset("Revealer").y, 96.0);

        // a new flight, and the wheel kills it mid-air
        revealer.selected.set(2);
        let _ = runtime.display_frame(&revealer, size);
        assert!(runtime.wants_frame());
        assert!(runtime.wheel(10.0, 10.0, 0.0, 8.0));
        assert!(!runtime.wants_frame(), "the wheel is sovereign");
        assert_eq!(runtime.scroll_offset("Revealer").y, 88.0);
    }

    #[test]
    fn borders_animate_and_a_boundary_closes_the_color_scope() {
        use crate::anim::Spring;
        use crate::layout::Color;

        const OFF: Color = Color { r: 40, g: 40, b: 200, a: 255 };
        const ON: Color = Color { r: 200, g: 40, b: 40, a: 255 };

        // the border color moves through the spring…
        #[derive(Clone, Copy)]
        struct Framed {
            on: State<bool>,
        }

        impl Component for Framed {
            fn body(self, _ctx: &Context) -> impl View {
                let color = if self.on.get() { ON } else { OFF };
                text("x").border(color, 1.0).animated(Spring::smooth())
            }
        }

        let size = crate::layout::Size { width: 80.0, height: 32.0 };
        let framed = Framed { on: State::new(false) };
        let runtime = Runtime::new();
        let _ = runtime.display_frame(&framed, size);
        framed.on.set(true);
        let _ = runtime.display_frame(&framed, size);
        assert!(runtime.wants_frame(), "the border is in flight");

        // …but a child component's colors are its own business: the
        // scope closes at the boundary
        #[derive(Clone, Copy)]
        struct Inner {
            on: State<bool>,
        }

        impl Component for Inner {
            fn body(self, _ctx: &Context) -> impl View {
                let color = if self.on.get() { ON } else { OFF };
                text("i").background_color(color)
            }
        }

        #[derive(Clone, Copy)]
        struct Outer {
            inner: Inner,
        }

        impl Component for Outer {
            fn body(self, _ctx: &Context) -> impl View {
                self.inner.animated(Spring::smooth())
            }
        }

        let outer = Outer { inner: Inner { on: State::new(false) } };
        let sealed = Runtime::new();
        let _ = sealed.display_frame(&outer, size);
        outer.inner.on.set(true);
        let flipped = sealed.display_frame(&outer, size);
        let fill = flipped
            .iter()
            .find_map(|command| match command {
                crate::layout::DrawCommand::FillRect { color, .. } => Some(*color),
                _ => None,
            })
            .expect("the inner chip paints");
        assert_eq!(fill, ON, "the child's color jumps — the scope closed");
        assert!(!sealed.wants_frame());
    }

    #[test]
    fn reduce_motion_completes_instantly() {
        use crate::anim::Spring;
        use crate::layout::Color;

        const OFF: Color = Color { r: 40, g: 40, b: 200, a: 255 };
        const ON: Color = Color { r: 200, g: 40, b: 40, a: 255 };

        #[derive(Clone, Copy)]
        struct Chip {
            on: State<bool>,
        }

        impl Component for Chip {
            fn body(self, _ctx: &Context) -> impl View {
                let color = if self.on.get() { ON } else { OFF };
                text("chip").background_color(color).animated(Spring::smooth())
            }
        }

        let size = crate::layout::Size { width: 120.0, height: 40.0 };
        let chip = Chip { on: State::new(false) };
        let runtime = Runtime::new();
        runtime.set_reduce_motion(true);
        let _ = runtime.display_frame(&chip, size);
        chip.on.set(true);
        let flipped = runtime.display_frame(&chip, size);
        let fill = flipped
            .iter()
            .find_map(|command| match command {
                crate::layout::DrawCommand::FillRect { color, .. } => Some(*color),
                _ => None,
            })
            .expect("the chip paints a background");
        assert_eq!(fill, ON, "reduce motion jumps to the target");
        assert!(!runtime.wants_frame());
        assert!(!runtime.tick(1.0 / 120.0));
    }

    #[test]
    fn a_tick_without_animations_asks_for_nothing() {
        // the frame-driver contract: a tick on a runtime with no live
        // animation reports no repaint and no wish for a next frame —
        // the shell parks the display link on this answer
        let runtime = Runtime::new();
        assert!(!runtime.tick(1.0 / 120.0));
        assert!(!runtime.wants_frame());
    }

    #[test]
    fn the_animation_frame_runs_zero_bodies_on_a_stable_tree() {
        #[derive(Clone, Copy)]
        struct Label;

        impl Component for Label {
            fn body(self, _ctx: &Context) -> impl View {
                text("steady")
            }
        }

        let runtime = Runtime::new();
        let size = crate::layout::Size { width: 200.0, height: 100.0 };
        // a real event frame mounts the tree (settle + layout)
        let event_frame = runtime.display_frame(&Label, size);
        // the tick frame: layout only — same pixels, zero bodies
        let tick_frame = runtime.animation_frame(&Label, size);
        assert!(runtime.body_runs().is_empty(), "a tick never runs a body");
        assert_eq!(tick_frame.as_slice(), event_frame.as_slice());
    }

    #[test]
    fn store_reads_in_the_body_are_dependencies_too() {
        // Object granularity: whoever read `store.value()` in the body depends
        // on the whole store — `send` re-runs the view, even with no State in
        // between (the sheet/blur case: dispatched bindings read the store directly).
        #[derive(Clone)]
        struct Badge {
            store: Store<i32>,
        }

        impl Component for Badge {
            fn body(self, _ctx: &Context) -> impl View {
                text(format!("badge {}", self.store.value()))
            }
        }

        let badge = Badge {
            store: Store::new(1),
        };
        let runtime = Runtime::new();
        runtime.render_stable(&badge);

        badge.store.send(2);
        let printed = runtime.render(&badge);
        assert_eq!(runtime.body_runs(), vec!["Badge".to_string()]);
        assert!(printed.contains("badge 2"));
    }

    #[test]
    fn on_receive_delivers_once_per_value_change() {
        #[derive(Clone)]
        struct Watcher {
            store: Store<i32>,
            seen: Rc<RefCell<Vec<i32>>>,
        }

        impl Component for Watcher {
            fn body(self, _ctx: &Context) -> impl View {
                // the publisher is recomputed on every body — as in the real app
                text("w").on_receive(self.store.updates(|value| *value), move |value| {
                    self.seen.borrow_mut().push(value)
                })
            }
        }

        let watcher = Watcher {
            store: Store::new(1),
            seen: Rc::new(RefCell::new(Vec::new())),
        };
        let runtime = Runtime::new();

        // two full render_stable passes: the initial value delivers ONCE,
        // however short the life of the publisher recreated per body
        runtime.render_stable(&watcher);
        runtime.render_stable(&watcher);
        assert_eq!(*watcher.seen.borrow(), vec![1]);

        // the value moves → delivers again
        watcher.store.send(5);
        runtime.render_stable(&watcher);
        assert_eq!(*watcher.seen.borrow(), vec![1, 5]);
    }

    #[test]
    fn a_real_view_tree_lays_out_through_the_runtime() {
        use crate::layout::{LINE_H, Proposal};

        // The screen: title + spacer + button, in a 200×100 viewport — the
        // whole path (body-eval → layout tree → retention →
        // expansion → frames) in a view with real state.
        #[derive(Clone, Copy)]
        struct Title {
            count: State<i32>,
        }

        impl Component for Title {
            fn body(self, _ctx: &Context) -> impl View {
                text(format!("count: {}", self.count.get()))
            }
        }

        #[derive(Clone, Copy)]
        struct Screen {
            title: Title,
        }

        impl Component for Screen {
            fn body(self, _ctx: &Context) -> impl View {
                let count = self.title.count;
                vstack((
                    self.title,
                    spacer(),
                    button(text("increment"), move || count.update(|n| *n += 1)),
                ))
            }
        }

        let screen = Screen { title: Title { count: State::new(0) } };
        let runtime = Runtime::new();
        runtime.render_stable(&screen);

        let viewport = Proposal { width: Some(200.0), height: Some(100.0) };
        let result = runtime.layout(&screen, viewport);

        assert_eq!(result.size.height, 100.0);
        let screen_frame = result.frames.get("Screen").unwrap();
        assert_eq!(screen_frame.size.height, 100.0);
        let title = result.frames.get("Screen/#0/Title").unwrap();
        assert_eq!(title.origin.y, 0.0);
        assert_eq!(title.size.height, LINE_H);

        // change the state → only the Title re-runs (fine invalidation) and
        // the next layout reflects the new text — with the REST from the cache
        screen.title.count.set(42);
        let result = runtime.layout(&screen, viewport);
        assert_eq!(runtime.body_runs(), vec!["Screen/#0/Title".to_string()]);
        let title = result.frames.get("Screen/#0/Title").unwrap();
        // "count: 42" = 9 chars × 8px — the frame follows the content
        assert_eq!(title.size.width, 72.0);
    }

    #[test]
    fn a_click_routes_through_hit_test_to_the_action_and_repaints() {
        use crate::layout::{Proposal, Size, hit_test};

        #[derive(Clone, Copy)]
        struct Tapper {
            count: State<i32>,
        }

        impl Component for Tapper {
            fn body(self, _ctx: &Context) -> impl View {
                vstack((
                    text(format!("count: {}", self.count.get())),
                    button(text("tap!"), move || self.count.update(|n| *n += 1)),
                ))
            }
        }

        let tapper = Tapper { count: State::new(0) };
        let runtime = Runtime::new();
        runtime.render_stable(&tapper);

        let viewport = Proposal::exact(Size { width: 200.0, height: 100.0 });
        let result = runtime.layout(&tapper, viewport);

        // the button is among the hit-test targets; a "click" at its center
        // resolves to the action key
        let (path, rect) = result.hits.last().expect("the button registers a target").clone();
        let key = hit_test(
            &result.hits,
            rect.origin.x + rect.size.width / 2.0,
            rect.origin.y + rect.size.height / 2.0,
        )
        .expect("the click hits the button");
        assert_eq!(key, path);

        // a click outside hits nothing
        assert!(hit_test(&result.hits, 199.0, 99.0).is_none());

        // firing the action changes the state; the next frame re-runs ONLY the
        // Tapper and the layout reflects the new text — the live loop, headless
        assert!(runtime.activate(key));
        let result = runtime.layout(&tapper, viewport);
        assert_eq!(runtime.body_runs(), vec!["Tapper".to_string()]);
        let title = result.frames.get("Tapper").unwrap();
        // with nothing flexible in the body, the root answers its natural
        // size, not the viewport — a proposal is an offer, not an imposition.
        // Natural = title line (16) + button with chrome (16 label + 2×6 built-in padding = 28)
        assert_eq!(title.size.height, 44.0);
        assert!(runtime.render(&tapper).contains("count: 1"));

        // and a click on a SKIPPED view's frame keeps working (the
        // action is retained like the effects)
        let result = runtime.layout(&tapper, viewport);
        assert!(runtime.body_runs().is_empty(), "everything from the cache");
        let (path, _) = result.hits.last().unwrap().clone();
        assert!(runtime.activate(&path));
        assert!(runtime.render(&tapper).contains("count: 2"));
    }

    /// The pair (settled runtime, button center) that the pointer tests
    /// share.
    fn pressable() -> (Runtime, TapperFixture, f64, f64) {
        use crate::layout::{Proposal, Size};

        let tapper = TapperFixture { count: State::new(0) };
        let runtime = Runtime::new();
        runtime.render_stable(&tapper);
        let result =
            runtime.layout(&tapper, Proposal::exact(Size { width: 200.0, height: 100.0 }));
        let (_, rect) = result.hits.last().expect("the button registers a target").clone();
        let cx = rect.origin.x + rect.size.width / 2.0;
        let cy = rect.origin.y + rect.size.height / 2.0;
        (runtime, tapper, cx, cy)
    }

    #[derive(Clone, Copy)]
    struct TapperFixture {
        count: State<i32>,
    }

    impl Component for TapperFixture {
        fn body(self, _ctx: &Context) -> impl View {
            vstack((
                text(format!("count: {}", self.count.get())),
                button(text("tap!"), move || self.count.update(|n| *n += 1)),
            ))
        }
    }

    #[test]
    fn hover_repaints_without_running_a_single_body() {
        use crate::layout::{DrawCommand, Proposal, Size};

        let (runtime, tapper, cx, cy) = pressable();
        let viewport = Proposal::exact(Size { width: 200.0, height: 100.0 });
        let cold = runtime.layout(&tapper, viewport);

        assert!(runtime.pointer_moved(cx, cy), "entering the target changes the state");
        let hot = runtime.layout(&tapper, viewport);
        assert!(runtime.body_runs().is_empty(), "hover repaints with ZERO bodies");

        // the LAW: byte-identical frames under any interaction
        for (path, frame) in cold.frames.iter() {
            assert_eq!(Some(frame), hot.frames.get(path));
        }
        let backgrounds = |result: &crate::layout::LayoutResult| -> Vec<Color> {
            result
                .display
                .iter()
                .filter_map(|command| match command {
                    DrawCommand::FillRect { color, .. } => Some(*color),
                    _ => None,
                })
                .collect()
        };
        assert_ne!(backgrounds(&cold), backgrounds(&hot), "the paint changes");

        // 1px within the same target: nothing to repaint
        assert!(!runtime.pointer_moved(cx + 1.0, cy));
    }

    #[test]
    fn up_inside_fires_and_down_alone_does_not() {
        let (runtime, tapper, cx, cy) = pressable();

        assert!(runtime.pointer_pressed(cx, cy));
        assert!(
            runtime.render_stable(&tapper).contains("count: 0"),
            "down alone does not fire"
        );
        assert!(runtime.pointer_released(cx, cy).is_some(), "up-inside fires");
        assert!(runtime.render_stable(&tapper).contains("count: 1"));
    }

    #[test]
    fn release_outside_never_fires() {
        let (runtime, tapper, cx, cy) = pressable();

        runtime.pointer_pressed(cx, cy);
        assert_eq!(runtime.pointer_released(199.0, 99.0), None, "released outside");
        runtime.pointer_pressed(199.0, 99.0);
        assert_eq!(runtime.pointer_released(cx, cy), None, "press outside, up inside");
        assert!(runtime.render_stable(&tapper).contains("count: 0"));
    }

    #[test]
    fn drag_out_and_back_rearms_the_press() {
        let (runtime, tapper, cx, cy) = pressable();

        runtime.pointer_pressed(cx, cy);
        runtime.pointer_moved(199.0, 99.0);
        assert!(
            runtime.interaction().hovered.is_none(),
            "dragging out releases the visual"
        );
        runtime.pointer_moved(cx, cy);
        assert_eq!(
            runtime.interaction().hovered,
            runtime.interaction().pressed,
            "coming back re-arms"
        );
        assert!(runtime.pointer_released(cx, cy).is_some());
        assert!(runtime.render_stable(&tapper).contains("count: 1"));
    }

    #[test]
    fn pointer_exited_clears_the_hover() {
        let (runtime, _tapper, cx, cy) = pressable();

        runtime.pointer_moved(cx, cy);
        assert!(runtime.pointer_exited());
        assert!(runtime.interaction().hovered.is_none());
        assert!(!runtime.pointer_exited(), "already clear — nothing to repaint");
    }

    #[test]
    fn style_modifiers_merge_into_one_styled() {
        use crate::layout::{DrawCommand, Proposal};

        let runtime = Runtime::new();
        let view = text("ab").corner_radius(5.0).background_color(Color::hex(0x123456));
        let result = runtime.layout(&view, Proposal::unspecified());

        let fills: Vec<(Color, f64)> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::FillRect { color, corner_radius, .. } => {
                    Some((*color, *corner_radius))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            fills,
            vec![(Color::hex(0x123456), 5.0)],
            "a single Styled — the radius rounds THIS background"
        );
    }

    #[test]
    fn the_nearest_background_wins_within_one_view() {
        use crate::layout::{DrawCommand, Proposal};

        let runtime = Runtime::new();
        let view = text("ab")
            .background_color(Color::hex(0x111111))
            .background_color(Color::hex(0x222222));
        let result = runtime.layout(&view, Proposal::unspecified());

        let fills: Vec<Color> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::FillRect { color, .. } => Some(*color),
                _ => None,
            })
            .collect();
        assert_eq!(
            fills,
            vec![Color::hex(0x111111)],
            "the one nearest the view wins; a single command"
        );
    }

    #[test]
    fn padding_and_background_order_mirror_the_api() {
        use crate::layout::{DrawCommand, Proposal, Rect};

        let runtime = Runtime::new();
        let outer = runtime.layout(
            &text("ab").padding_length(10.0).background_color(Color::BLACK),
            Proposal::unspecified(),
        );
        let inner = runtime.layout(
            &text("ab").background_color(Color::BLACK).padding_length(10.0),
            Proposal::unspecified(),
        );

        let fill_rect = |result: &crate::layout::LayoutResult| -> Rect {
            result
                .display
                .iter()
                .find_map(|command| match command {
                    DrawCommand::FillRect { rect, .. } => Some(*rect),
                    _ => None,
                })
                .unwrap()
        };
        // background outside the padding covers the padded area; inside,
        // only the content — total size does not change, who paints what does
        assert_eq!(fill_rect(&outer).size.width, 36.0);
        assert_eq!(fill_rect(&inner).size.width, 16.0);
        assert_eq!(outer.size, inner.size);
    }

    #[test]
    fn visual_modifiers_print_their_suffixes() {
        let runtime = Runtime::new();
        let printed = runtime.render(
            &text("x")
                .background_color(Color::hex(0xAA69FB))
                .foreground_color(Color::hex(0x070510))
                .border(Color::hex(0x2A1B3F), 1.0)
                .corner_radius(6.0),
        );
        assert!(printed.contains("[.background(#AA69FB)]"), "{printed}");
        assert!(printed.contains("[.foregroundColor(#070510)]"), "{printed}");
        assert!(printed.contains("[.border(#2A1B3F, width: 1)]"), "{printed}");
        assert!(printed.contains("[.cornerRadius(6)]"), "{printed}");
    }

    #[test]
    fn button_chrome_shows_up_in_the_display_list() {
        use crate::layout::{DrawCommand, Proposal};

        #[derive(Clone, Copy)]
        struct One;

        impl Component for One {
            fn body(self, _ctx: &Context) -> impl View {
                button(text("go"), || {})
            }
        }

        let runtime = Runtime::new();
        runtime.render_stable(&One);
        let result = runtime.layout(&One, Proposal::unspecified());

        // the chrome background comes before the label text, with the corners
        let mut fill_before_text = false;
        let mut saw_text = false;
        for command in result.display.iter() {
            match command {
                DrawCommand::FillRect { corner_radius, .. } if !saw_text => {
                    assert_eq!(*corner_radius, 6.0);
                    fill_before_text = true;
                }
                DrawCommand::TextLine { .. } => saw_text = true,
                _ => {}
            }
        }
        assert!(fill_before_text && saw_text);

        // the hit-rect is the whole chrome: label + built-in padding
        let (_, rect) = result.hits.last().unwrap().clone();
        assert_eq!(rect.size.height, 28.0, "16 from the label + 2×6");
        assert_eq!(rect.size.width, 44.0, "2 chars × 8 + 2×14");
    }

    #[test]
    fn wheel_scrolls_clamps_and_persists_by_identity() {
        use crate::layout::{DrawCommand, Proposal, Size};

        #[derive(Clone, Copy)]
        struct Rows {
            flip: State<bool>,
        }

        impl Component for Rows {
            fn body(self, _ctx: &Context) -> impl View {
                let _ = self.flip.get(); // read: set() invalidates this body
                list(
                    (0..10).map(|index| index.to_string()).collect(),
                    |item: &String| item.clone(),
                    |item: &String| text(format!("row {item}")),
                )
            }
        }

        let rows = Rows { flip: State::new(false) };
        let runtime = Runtime::new();
        runtime.render_stable(&rows);
        let viewport = Proposal::exact(Size { width: 120.0, height: 100.0 });
        let result = runtime.layout(&rows, viewport);

        assert_eq!(result.scrolls.len(), 1, "the List is a region with identity");
        let path = result.scrolls[0].path.clone();

        // negative delta (content moves up) → offset grows
        assert!(runtime.wheel(10.0, 10.0, 0.0, -30.0));
        assert_eq!(runtime.scroll_offset(&path).y, 30.0);
        // clamp snapped at the end of travel: 10×16 − 100 = 60
        assert!(runtime.wheel(10.0, 10.0, 0.0, -500.0));
        assert_eq!(runtime.scroll_offset(&path).y, 60.0);
        assert!(!runtime.wheel(10.0, 10.0, 0.0, -1.0), "no repaint at the end of travel");
        assert!(!runtime.wheel(500.0, 500.0, 0.0, -10.0), "outside any region");

        // the offset applies in layout: the first text line moves up 60
        let scrolled = runtime.layout(&rows, viewport);
        let first_line_y = scrolled
            .display
            .iter()
            .find_map(|command| match command {
                DrawCommand::TextLine { origin, .. } => Some(origin.y),
                _ => None,
            })
            .unwrap();
        assert_eq!(first_line_y, -60.0);

        // invalidation and re-render do NOT lose the position — restoration
        // by structural identity
        rows.flip.set(true);
        runtime.render_stable(&rows);
        let after = runtime.layout(&rows, viewport);
        assert_eq!(runtime.scroll_offset(&path).y, 60.0);
        let line_y = after
            .display
            .iter()
            .find_map(|command| match command {
                DrawCommand::TextLine { origin, .. } => Some(origin.y),
                _ => None,
            })
            .unwrap();
        assert_eq!(line_y, -60.0);

        // programmatic scrolling counts in the SAME frame
        runtime.set_scroll_offset(&path, crate::layout::Point { x: 0.0, y: 8.0 });
        let programmatic = runtime.layout(&rows, viewport);
        let line_y = programmatic
            .display
            .iter()
            .find_map(|command| match command {
                DrawCommand::TextLine { origin, .. } => Some(origin.y),
                _ => None,
            })
            .unwrap();
        assert_eq!(line_y, -8.0);
    }

    #[test]
    fn a_text_field_edits_through_focus_and_the_binding() {
        use crate::layout::{DrawCommand, Proposal, Size};
        use crate::text_input::EditCommand;

        #[derive(Clone, Copy)]
        struct Form {
            name: State<String>,
        }

        impl Component for Form {
            fn body(self, _ctx: &Context) -> impl View {
                vstack((
                    text(format!("hello {}", self.name.get())),
                    text_field("Your name", self.name.binding()),
                ))
            }
        }

        let form = Form { name: State::new(String::new()) };
        let runtime = Runtime::new();
        runtime.render_stable(&form);
        let viewport = Proposal::exact(Size { width: 240.0, height: 100.0 });
        let result = runtime.layout(&form, viewport);

        // empty: the placeholder paints in its own color, no focus
        let has_placeholder = result.display.iter().any(|command| matches!(
            command,
            DrawCommand::TextLine { color, content, range, .. }
                if *color == Color::PLACEHOLDER && &content[range.0..range.1] == "Your name"
        ));
        assert!(has_placeholder);

        // clicking the field focuses (up-inside → editor → focus)
        let (field_path, rect) = result.hits.last().expect("the field is a target").clone();
        let (cx, cy) = (
            rect.origin.x + rect.size.width / 2.0,
            rect.origin.y + rect.size.height / 2.0,
        );
        runtime.pointer_pressed(cx, cy);
        assert_eq!(runtime.pointer_released(cx, cy), Some(field_path.clone()));
        assert_eq!(runtime.focused(), Some(field_path.clone()));

        // typing flows through the binding: the TITLE (another view) sees the change
        assert!(runtime.key(EditCommand::Insert("Deco".into())).applied);
        let printed = runtime.render_stable(&form);
        assert!(printed.contains("hello Deco"), "{printed}");

        // the focused frame paints caret and focus border
        let focused_frame = runtime.layout(&form, viewport);
        assert!(focused_frame.display.iter().any(|command| matches!(
            command,
            DrawCommand::FillRect { rect, color, .. }
                if *color == Color::BLACK && rect.size.width < 2.0
        )));
        assert!(focused_frame.display.iter().any(|command| matches!(
            command,
            DrawCommand::StrokeRect { color, .. } if *color == Color::FOCUS
        )));

        // editing continues: backspace eats the "o"
        assert!(runtime.key(EditCommand::Backspace).applied);
        assert!(runtime.render_stable(&form).contains("hello Dec"));

        // copy/cut extract through the output (the clipboard bridge)
        assert!(runtime.key(EditCommand::SelectAll).applied);
        assert_eq!(runtime.key(EditCommand::Copy).output.as_deref(), Some("Dec"));
        assert_eq!(runtime.key(EditCommand::Cut).output.as_deref(), Some("Dec"));
        assert!(!runtime.render_stable(&form).contains("hello Dec"), "the cut removed it");
        assert!(runtime.key(EditCommand::Insert("Dec".into())).applied);

        // clicking outside removes focus; keying without focus does nothing
        runtime.pointer_pressed(239.0, 99.0);
        runtime.pointer_released(239.0, 99.0);
        assert_eq!(runtime.focused(), None);
        assert!(!runtime.key(EditCommand::Insert("x".into())).applied);
        assert!(runtime.render_stable(&form).contains("hello Dec"));
    }

    #[test]
    fn actions_register_dispatch_and_the_innermost_wins() {
        const PING: ActionId = ActionId("test.ping");

        #[derive(Clone, Copy)]
        struct Inner {
            hits: State<i32>,
        }

        impl Component for Inner {
            fn body(self, _ctx: &Context) -> impl View {
                text("inner").on_action(PING, move || self.hits.add(10))
            }
        }

        #[derive(Clone, Copy)]
        struct Outer {
            hits: State<i32>,
        }

        impl Component for Outer {
            fn body(self, _ctx: &Context) -> impl View {
                vstack!(Inner { hits: self.hits })
                    .on_action(PING, move || self.hits.add(1))
            }
        }

        let outer = Outer { hits: State::new(0) };
        let runtime = Runtime::new();
        runtime.render_stable(&outer);

        // the DEEPEST handler (Inner) beats Outer's
        assert!(runtime.dispatch_action(PING));
        assert_eq!(outer.hits.get(), 10);
        assert!(!runtime.dispatch_action(ActionId("test.nope")), "id with no handler");

        // bind + match compose; the modifier is exact
        const NEXT: ActionId = ActionId("test.next");
        runtime.bind(KeyPattern::key(Key::Down), NEXT);
        assert_eq!(runtime.match_key(&KeyPattern::key(Key::Down)), Some(NEXT));
        assert_eq!(runtime.match_key(&KeyPattern::command(Key::Down)), None);
        // a binding with no mounted handler does NOT consume — the property
        // that lets the key flow on to the field
        assert!(!runtime.dispatch_action(NEXT));
    }

    #[test]
    fn a_skipped_views_handler_stays_alive_and_dies_with_it() {
        const POKE: ActionId = ActionId("test.poke");

        #[derive(Clone, Copy)]
        struct Holder {
            mounted: State<bool>,
            other: State<i32>,
            hits: State<i32>,
        }

        #[derive(Clone, Copy)]
        struct Palette {
            hits: State<i32>,
        }

        impl Component for Palette {
            fn body(self, _ctx: &Context) -> impl View {
                text("palette").on_action(POKE, move || self.hits.add(1))
            }
        }

        impl Component for Holder {
            fn body(self, _ctx: &Context) -> impl View {
                let _ = self.other.get();
                if self.mounted.get() {
                    Either::First(Palette { hits: self.hits })
                } else {
                    Either::Second(text("closed"))
                }
            }
        }

        let holder = Holder {
            mounted: State::new(true),
            other: State::new(0),
            hits: State::new(0),
        };
        let runtime = Runtime::new();
        runtime.render_stable(&holder);

        // stable pass (Palette SKIPPED): the retained handler stays alive
        runtime.render(&holder);
        assert!(runtime.dispatch_action(POKE));
        assert_eq!(holder.hits.get(), 1);

        // unmounting the Palette sweeps the entry — the handler dies with it
        holder.mounted.set(false);
        runtime.render_stable(&holder);
        eprintln!("POST-UNMOUNT PRINT: {}", runtime.render(&holder));
        assert!(!runtime.dispatch_action(POKE), "an unmounted handler does not respond");
    }

    #[test]
    fn handlers_reregister_with_fresh_captures() {
        use crate::layout::{Proposal, Size};
        const NEXT: ActionId = ActionId("test.select_next");
        const DISMISS: ActionId = ActionId("test.dismiss");

        // the whole finder shape: field + filtered list + selection with
        // wrap capturing the COUNT of the current body
        #[derive(Clone, Copy)]
        struct MiniPalette {
            query: State<String>,
            selected: State<usize>,
        }

        impl Component for MiniPalette {
            fn body(self, _ctx: &Context) -> impl View {
                let query = self.query.get();
                let all = ["alpha", "beta", "gamma"];
                let count = all.iter().filter(|name| name.contains(&query)).count();
                let selected = self.selected;
                vstack!(
                    text_field("filter", self.query.binding()),
                    text!("{count} items"),
                )
                .on_action(NEXT, move || {
                    if count > 0 {
                        selected.set((selected.get() + 1) % count)
                    }
                })
                .on_action(DISMISS, move || self.query.set(String::new()))
            }
        }

        let palette = MiniPalette { query: State::new(String::new()), selected: State::new(0) };
        let runtime = Runtime::new();
        runtime.bind(KeyPattern::key(Key::Down), NEXT);
        runtime.bind(KeyPattern::key(Key::Escape), DISMISS);
        runtime.render_stable(&palette);

        // focus the field and type — the filter shrinks to 1 ("beta")
        let result =
            runtime.layout(&palette, Proposal::exact(Size { width: 200.0, height: 80.0 }));
        let (_, rect) = result.hits.first().unwrap().clone();
        runtime.pointer_pressed(rect.origin.x + 4.0, rect.origin.y + 4.0);
        runtime.pointer_released(rect.origin.x + 4.0, rect.origin.y + 4.0);
        runtime.key(EditCommand::Insert("bet".into()));
        runtime.render_stable(&palette);

        // the re-registered handler captured the NEW count: wrap at 1
        let action = runtime.match_key(&KeyPattern::key(Key::Down)).unwrap();
        assert!(runtime.dispatch_action(action));
        assert_eq!(palette.selected.get(), 0, "wrap with count=1 goes back to 0");

        // Esc dispatches the action and FOCUS stays (blur is the shell's fallback)
        let action = runtime.match_key(&KeyPattern::key(Key::Escape)).unwrap();
        assert!(runtime.dispatch_action(action));
        runtime.render_stable(&palette);
        assert_eq!(palette.query.get(), "");
        assert!(runtime.focused().is_some(), "dismiss does not steal the focus");
    }

    #[test]
    fn installing_a_theme_reskins_retained_views() {
        use crate::layout::{DrawCommand, Proposal, Size};

        #[derive(Clone, Copy)]
        struct Chip;

        impl Component for Chip {
            fn body(self, _ctx: &Context) -> impl View {
                // a token read in the BODY gets baked into the retained scene —
                // install has to rebuild with no dirty state at all
                button(text("go").foreground_color(theme::accent()), || {})
            }
        }

        let runtime = Runtime::new();
        runtime.render_stable(&Chip);
        let viewport = Proposal::exact(Size { width: 200.0, height: 60.0 });

        let control_of = |result: &crate::layout::LayoutResult| {
            result
                .display
                .iter()
                .find_map(|command| match command {
                    DrawCommand::FillRect { color, .. } => Some(*color),
                    _ => None,
                })
                .unwrap()
        };
        let text_of = |result: &crate::layout::LayoutResult| {
            result
                .display
                .iter()
                .find_map(|command| match command {
                    DrawCommand::TextLine { color, .. } => Some(*color),
                    _ => None,
                })
                .unwrap()
        };

        let light = runtime.layout(&Chip, viewport);
        assert_eq!(control_of(&light), Theme::light().control);
        assert_eq!(text_of(&light), Theme::light().accent);

        theme::install(Theme::dark());
        let dark = runtime.layout(&Chip, viewport);
        assert_eq!(control_of(&dark), Theme::dark().control, "chrome re-read");
        assert_eq!(text_of(&dark), Theme::dark().accent, "body token re-read");

        theme::install(Theme::light());
    }

    #[test]
    fn the_dream_snippet_compiles_and_reacts() {
        use crate::layout::{Proposal, Size};

        // The code the ergonomics LAW demands: no `let this`, no
        // `.get()`, no `format!`, no doubled parentheses — and underneath,
        // the SAME incremental pipeline as always.
        #[derive(Clone, Copy)]
        struct Counter {
            count: State<i32>,
        }

        impl Component for Counter {
            fn body(self, _ctx: &Context) -> impl View {
                vstack!(
                    text!("Count: {}", self.count),
                    button(text("Tap"), move || self.count.add(1)),
                )
            }
        }

        let counter = Counter { count: State::new(0) };
        let runtime = Runtime::new();
        assert!(runtime.render_stable(&counter).contains("Count: 0"));

        let result =
            runtime.layout(&counter, Proposal::exact(Size { width: 200.0, height: 100.0 }));
        let (_, rect) = result.hits.last().unwrap().clone();
        let cx = rect.origin.x + rect.size.width / 2.0;
        let cy = rect.origin.y + rect.size.height / 2.0;
        runtime.pointer_pressed(cx, cy);
        runtime.pointer_released(cx, cy);

        // State's Display READS — the click invalidates ONLY the Counter
        runtime.render(&counter);
        assert_eq!(runtime.body_runs(), vec!["Counter".to_string()]);
        assert!(runtime.render_stable(&counter).contains("Count: 1"));
    }

    #[test]
    fn ime_composition_flows_through_the_runtime() {
        use crate::layout::{DrawCommand, Proposal, Size};
        use crate::text_input::EditCommand;

        #[derive(Clone, Copy)]
        struct Form {
            name: State<String>,
        }

        impl Component for Form {
            fn body(self, _ctx: &Context) -> impl View {
                text_field("name", self.name.binding())
            }
        }

        let form = Form { name: State::new(String::new()) };
        let runtime = Runtime::new();
        runtime.render_stable(&form);
        let viewport = Proposal::exact(Size { width: 240.0, height: 40.0 });
        let result = runtime.layout(&form, viewport);
        let (path, rect) = result.hits.last().unwrap().clone();
        let _ = path;
        runtime.pointer_pressed(rect.origin.x + 4.0, rect.origin.y + 4.0);
        runtime.pointer_released(rect.origin.x + 4.0, rect.origin.y + 4.0);

        // live composition: the text enters MARKED (underlined, in the binding)
        let mark = EditCommand::SetMarked { text: "にほん".into(), caret_utf16: (3, 0) };
        assert!(runtime.key(mark).applied);
        assert!(runtime.render_stable(&form).contains("にほん"));
        let composing = runtime.layout(&form, viewport);
        let underline = |result: &crate::layout::LayoutResult| {
            result
                .display
                .iter()
                .filter(|command| matches!(
                    command,
                    DrawCommand::FillRect { rect, color, .. }
                        if *color == Color::BLACK && rect.size.height == 1.0
                ))
                .count()
        };
        assert_eq!(underline(&composing), 1, "the composition paints underlined");

        // the snapshot speaks UTF-16 with the platform
        let snapshot = runtime.ime_snapshot().expect("focused field");
        assert_eq!(snapshot.marked, Some((0, 3)));
        assert_eq!(snapshot.selected, (3, 0), "caret collapsed at the end of the composition");

        // the commit swaps the marked text for the final text and the underline goes away
        assert!(runtime.key(EditCommand::Insert("日本".into())).applied);
        let committed = runtime.render_stable(&form);
        assert!(committed.contains("日本"), "{committed}");
        assert!(!committed.contains("にほん"));
        let after = runtime.layout(&form, viewport);
        assert_eq!(underline(&after), 0);
        assert_eq!(runtime.ime_snapshot().unwrap().marked, None);
    }

    #[test]
    fn click_positions_the_caret_and_blink_toggles_it() {
        use crate::layout::{DrawCommand, Proposal, Size};
        use crate::text_input::EditCommand;

        #[derive(Clone, Copy)]
        struct Form {
            name: State<String>,
        }

        impl Component for Form {
            fn body(self, _ctx: &Context) -> impl View {
                text_field("name", self.name.binding())
            }
        }

        let form = Form { name: State::new("abcdef".to_string()) };
        let runtime = Runtime::new();
        runtime.render_stable(&form);
        let viewport = Proposal::exact(Size { width: 240.0, height: 40.0 });
        let result = runtime.layout(&form, viewport);
        let field = result.fields.first().expect("field placed").clone();

        // click in the middle of "abcdef" (PixelFont: 8px/char): between c and d
        let x = field.text_origin.x + 3.0 * 8.0 + 2.0;
        let y = field.frame.origin.y + field.frame.size.height / 2.0;
        runtime.pointer_pressed(x, y);
        runtime.pointer_released(x, y);
        assert_eq!(runtime.focused(), Some(field.path.clone()));
        // typing at the clicked point proves the position without exposing the index
        runtime.key(EditCommand::Insert("X".into()));
        assert!(runtime.render_stable(&form).contains("abcXdef"));

        // blink: the tick toggles the caret paint without touching anything else
        let caret_count = |result: &crate::layout::LayoutResult| {
            result
                .display
                .iter()
                .filter(|command| matches!(
                    command,
                    DrawCommand::FillRect { rect, color, .. }
                        if *color == Color::BLACK && rect.size.width < 2.0
                ))
                .count()
        };
        let visible = runtime.layout(&form, viewport);
        assert_eq!(caret_count(&visible), 1);
        assert!(runtime.blink(), "focused: the tick requests a repaint");
        let hidden = runtime.layout(&form, viewport);
        assert_eq!(caret_count(&hidden), 0, "off half-period");
        assert!(runtime.blink());
        let back = runtime.layout(&form, viewport);
        assert_eq!(caret_count(&back), 1);
        // typing goes back to solid even in the off half-period
        runtime.blink();
        runtime.key(EditCommand::Insert("!".into()));
        let typing = runtime.layout(&form, viewport);
        assert_eq!(caret_count(&typing), 1, "an active caret does not blink");
        // without focus, the tick requests no repaint
        runtime.blur();
        assert!(!runtime.blink());
    }

    #[test]
    fn text_measures_through_the_engine() {
        use crate::layout::{Proposal, Size};
        use crate::text_engine::{LineMetrics, TextRaster};

        // a 10px/char engine proves the pluggable border: the frame changes
        // with NO component knowing which engine is active
        struct Wide;
        impl TextEngine for Wide {
            fn measure_line(&self, text: &str, _font: &FontSpec) -> LineMetrics {
                LineMetrics {
                    width: text.chars().count() as f64 * 10.0,
                    ascent: 15.0,
                    descent: 5.0,
                }
            }
            fn raster_line(
                &self,
                _text: &str,
                _font: &FontSpec,
                _color: Color,
                _scale: usize,
            ) -> Option<TextRaster> {
                None
            }
        }

        let runtime = Runtime::new().text_engine(Rc::new(Wide));
        let result = runtime.layout(&text("abcd"), Proposal::unspecified());
        assert_eq!(result.size, Size { width: 40.0, height: 20.0 });
    }

    #[test]
    fn font_style_inherits_and_overrides() {
        use crate::layout::{DrawCommand, Proposal};

        let runtime = Runtime::new();
        let view = vstack((text("big"), text("small").font(Font::Caption))).font(Font::Title);
        let result = runtime.layout(&view, Proposal::unspecified());

        let sizes: Vec<f64> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { font, .. } => Some(font.size),
                _ => None,
            })
            .collect();
        assert_eq!(sizes, vec![22.0, 10.0], "Title inherited; Caption wins in the child");
    }

    #[test]
    fn paint_puts_ink_where_the_layout_put_the_text() {
        use crate::layout::Size;

        #[derive(Clone, Copy)]
        struct Badge {
            n: State<i32>,
        }

        impl Component for Badge {
            fn body(self, _ctx: &Context) -> impl View {
                text(format!("n: {}", self.n.get()))
            }
        }

        let badge = Badge { n: State::new(0) };
        let runtime = Runtime::new();
        runtime.render_stable(&badge);

        let size = Size { width: 64.0, height: 16.0 };
        let bitmap = runtime.paint(&badge, size);

        let white = bitmap.pixel(63, 15).unwrap();
        let ink_count = |bitmap: &crate::raster::Bitmap| {
            (0..16)
                .flat_map(|y| (0..64).map(move |x| (x, y)))
                .filter(|&(x, y)| bitmap.pixel(x, y) != Some(white))
                .count()
        };
        let before = ink_count(&bitmap);
        assert!(before > 0, "the text paints ink");

        // the state changes → incremental re-render → the bitmap changes with it
        badge.n.set(42);
        let after = ink_count(&runtime.paint(&badge, size));
        assert!(after > before, "\"n: 42\" has more ink than \"n: 0\"");
    }

    #[test]
    fn state_and_binding_read_through_get() {
        let state = State::new(7);
        assert_eq!(state.get(), 7);

        let binding = state.binding();
        binding.set(9);
        assert_eq!(binding.get(), 9);
        assert_eq!(state.get(), 9);
    }

    #[test]
    fn tuples_option_and_one_of_compose_statically() {
        #[derive(Clone)]
        struct Row;

        impl Component for Row {
            fn body(self, _ctx: &Context) -> impl View {
                // Option None does not print; Either/OneOf pick the arm at
                // compile time — the type is the sum, the discriminant is runtime
                vstack((
                    None::<Text>,
                    text("kept"),
                    Either::<Text, Text>::First(text("first")),
                    OneOf3::<Text, Text, Text>::C(text("third")),
                ))
            }
        }

        let printed = Runtime::new().render_stable(&Row);
        assert!(printed.contains("Row"));
        assert!(printed.contains("Text(\"kept\")"));
        assert!(printed.contains("Text(\"first\")"));
        assert!(printed.contains("Text(\"third\")"));
        assert!(!printed.contains("TupleView"));
    }

    #[test]
    fn scroll_target_reveals_the_selection_and_leaves_the_wheel_alone() {
        use crate::layout::{Proposal, Size};

        #[derive(Clone, Copy)]
        struct Rows {
            selected: State<usize>,
        }

        impl Component for Rows {
            fn body(self, _ctx: &Context) -> impl View {
                let items: Vec<usize> = (0..10).collect();
                let selected = self.selected.get();
                list(items, |index| format!("row{index}"), |index| text(format!("r{index}")))
                    .scroll_target(format!("row{selected}"))
            }
        }

        // 10 rows × 16px in an 48px viewport: three rows visible
        let viewport = Proposal::exact(Size { width: 120.0, height: 48.0 });
        let rows = Rows { selected: State::new(0) };
        let runtime = Runtime::new();
        runtime.settle(&rows);
        let result = runtime.layout(&rows, viewport);
        let region = result.scrolls.first().expect("scroll region").path.clone();
        assert_eq!(runtime.scroll_offset(&region).y, 0.0, "row0 already visible: no scroll");

        // selection jumps below the fold: the region follows, bottom-aligned
        rows.selected.set(8);
        runtime.settle(&rows);
        runtime.layout(&rows, viewport);
        let offset = runtime.scroll_offset(&region).y;
        assert_eq!(offset, 8.0 * 16.0 + 16.0 - 48.0, "row8 bottom-aligned into view");

        // the wheel is sovereign while the target stays put
        runtime.wheel(60.0, 24.0, 0.0, 30.0);
        let wheeled = runtime.scroll_offset(&region).y;
        assert_ne!(wheeled, offset, "the wheel moved the region");
        runtime.layout(&rows, viewport);
        assert_eq!(
            runtime.scroll_offset(&region).y,
            wheeled,
            "an unchanged target never drags the wheel back"
        );

        // target changes again: reveal wins again
        rows.selected.set(0);
        runtime.settle(&rows);
        runtime.layout(&rows, viewport);
        assert_eq!(runtime.scroll_offset(&region).y, 0.0, "row0 top-aligned back into view");
    }

    #[test]
    fn auto_focus_takes_the_keyboard_once_and_respects_blur() {
        use crate::layout::{Proposal, Size};

        #[derive(Clone, Copy)]
        struct Form {
            name: State<String>,
        }

        impl Component for Form {
            fn body(self, _ctx: &Context) -> impl View {
                text_field("Your name", self.name.binding()).monospaced().auto_focus()
            }
        }

        let viewport = Proposal::exact(Size { width: 200.0, height: 60.0 });
        let form = Form { name: State::new(String::new()) };
        let runtime = Runtime::new();
        runtime.settle(&form);
        runtime.layout(&form, viewport);
        let path = runtime.focused().expect("first appearance focuses the field");

        // typing goes straight in — the field already owns the keyboard
        assert!(runtime.key(EditCommand::Insert("d".into())).applied);
        assert_eq!(form.name.get(), "d");

        // a user blur is final: the next frames never steal focus back
        runtime.blur();
        runtime.settle(&form);
        runtime.layout(&form, viewport);
        assert_eq!(runtime.focused(), None, "auto focus fires once per identity");
        let _ = path;
    }

    #[test]
    fn scoped_bindings_answer_only_while_their_context_is_mounted() {
        use crate::layout::{Proposal, Size};

        const CLOSE: ActionId = ActionId("palette.close");
        const GLOBAL: ActionId = ActionId("app.global");

        #[derive(Clone, Copy)]
        struct App {
            palette_open: State<bool>,
        }

        impl Component for App {
            fn body(self, _ctx: &Context) -> impl View {
                let palette = self
                    .palette_open
                    .get()
                    .then(|| text("palette").key_context("palette"));
                vstack!(text("app"), palette)
            }
        }

        let viewport = Proposal::exact(Size { width: 200.0, height: 100.0 });
        let app = App { palette_open: State::new(false) };
        let runtime = Runtime::new();
        let escape = KeyPattern::key(Key::Escape);
        runtime.bind(escape, GLOBAL);
        runtime.bind_in("palette", escape, CLOSE);

        // palette closed: the scoped binding is silent, the global answers
        runtime.settle(&app);
        runtime.layout(&app, viewport);
        assert_eq!(runtime.match_key(&escape), Some(GLOBAL), "context down, global wins");

        // palette mounts: its binding takes the same key over the global
        app.palette_open.set(true);
        runtime.settle(&app);
        runtime.layout(&app, viewport);
        assert_eq!(runtime.match_key(&escape), Some(CLOSE), "mounted context wins the key");

        // palette unmounts: the sweep deactivates the context
        app.palette_open.set(false);
        runtime.settle(&app);
        runtime.layout(&app, viewport);
        assert_eq!(runtime.match_key(&escape), Some(GLOBAL), "unmounted context goes quiet");
    }

    #[test]
    fn one_click_swaps_the_theme_scene_and_one_click_swaps_it_back() {
        use crate::layout::{DrawCommand, Proposal, Size};

        #[derive(Clone, Copy)]
        struct App {
            dark: State<bool>,
        }

        impl Component for App {
            fn body(self, _ctx: &Context) -> impl View {
                let dark_on = self.dark.get();
                vstack!(
                    text("hello").foreground_color(theme::fg()),
                    button(text(if dark_on { "light" } else { "dark" }), move || {
                        let next = !self.dark.get();
                        self.dark.set(next);
                        theme::install(if next { Theme::dark() } else { Theme::light() });
                    }),
                )
                .background_color(theme::panel())
            }
        }

        theme::install(Theme::light());
        let size = Size { width: 200.0, height: 120.0 };
        let app = App { dark: State::new(false) };
        let runtime = Runtime::new();
        runtime.settle(&app);
        let target = runtime.layout(&app, Proposal::exact(size)).hits.first().expect("toggle").1;
        let (x, y) = (target.origin.x + 4.0, target.origin.y + 4.0);
        let panel = |runtime: &Runtime| {
            runtime
                .display_frame(&app, size)
                .iter()
                .find_map(|command| match command {
                    DrawCommand::FillRect { color, .. } => Some(*color),
                    _ => None,
                })
                .expect("panel fill")
        };

        let light = panel(&runtime);
        runtime.pointer_pressed(x, y);
        runtime.pointer_released(x, y);
        let dark = panel(&runtime);
        assert_ne!(dark, light, "ONE click reskins the scene");
        assert_eq!(dark, Theme::dark().panel, "the dark panel token landed");

        runtime.pointer_pressed(x, y);
        runtime.pointer_released(x, y);
        assert_eq!(panel(&runtime), light, "one more click swaps it back");
        theme::install(Theme::light());
    }

    #[test]
    fn a_surface_repaints_a_real_hover_incrementally() {
        use crate::layout::{Proposal, Size};
        use crate::raster::{Surface, rasterize_scaled};
        use crate::text_engine::PixelFont;

        #[derive(Clone, Copy)]
        struct Rows;

        impl Component for Rows {
            fn body(self, _ctx: &Context) -> impl View {
                vstack!(
                    text("alpha")
                        .padding_length(6.0)
                        .background_color(Color::hex(0xEEEEF2))
                        .background_hovered(Color::hex(0xD8DCE6))
                        .on_click(|| {}),
                    text("beta")
                        .padding_length(6.0)
                        .background_color(Color::hex(0xEEEEF2))
                        .background_hovered(Color::hex(0xD8DCE6))
                        .on_click(|| {}),
                )
            }
        }

        let viewport = Proposal::exact(Size { width: 120.0, height: 80.0 });
        let runtime = Runtime::new();
        runtime.settle(&Rows);

        let mut surface = Surface::new(120, 80, 1, Color::CANVAS);
        let first = surface.frame(runtime.layout(&Rows, viewport).display, &PixelFont);
        assert_eq!(first, vec![(0, 0, 120, 80)], "first frame damages everything");

        // hover the second row: the frame through the REAL pipeline
        // (stamp → layout → surface) damages only that row, and the
        // pixels match a full repaint byte for byte
        let target = runtime
            .layout(&Rows, viewport)
            .hits
            .get(1)
            .expect("second row is a pointer target")
            .1;
        runtime.pointer_moved(target.origin.x + 4.0, target.origin.y + 4.0);
        let result = runtime.layout(&Rows, viewport);
        let oracle = rasterize_scaled(&result.display, 120, 80, 1, Color::CANVAS);
        let damage = surface.frame(result.display, &PixelFont);

        assert_eq!(surface.bitmap().pixels(), oracle.pixels(), "golden: incremental == full");
        assert_eq!(damage.len(), 1, "one row hovered, one rect: {damage:?}");
        let (_, y0, _, y1) = damage[0];
        let row_height = (y1 - y0) as f64;
        assert!(
            row_height <= target.size.height + 4.0,
            "damage is row-sized ({row_height}px tall), not the window"
        );
    }
}
