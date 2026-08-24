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
pub mod custom;
pub mod dom;
mod dom_flow;
pub mod effects;
pub mod erased;
pub mod ext;
pub mod glass;
pub mod icon;
pub mod image_engine;
pub mod layout;
pub mod modifier;
pub mod one_of;
#[cfg(feature = "gpu")]
pub mod gpu;
#[cfg(feature = "canvas")]
pub mod raster;
mod reconciler;
pub mod runtime;
pub mod ssr;
pub mod state_ext;
pub mod stats;
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

/// The engine's task queue: what `.task` runs on, and the channel a
/// worker thread (or a browser callback) hands its results over.
pub mod task {
    pub use motor::task::{
        Receiver, Recv, SendError, Sender, Sleep, Spawned, channel, sleep, spawn,
    };
}

pub mod prelude {
    pub use crate::action::{ActionId, Key, KeyPattern};
    pub use crate::anim::{FramePace, Loop, Spring, Ticked};
    pub use crate::custom::{
        Custom, CustomElement, ElementEvent, EventCtx, ImeContext, Metrics, PaintCtx, Painter,
        Response,
    };
    #[cfg(feature = "canvas")]
    pub use crate::custom::{canvas, custom};
    pub use crate::erased::{CustomModifier, Erased, erased};
    pub use crate::{hstack, text, vstack, zstack};
    pub use crate::ext::ViewExt;
    pub use crate::icon::house as symbol;
    pub use crate::icon::{ICON_GRID, Ink, Paint, Rule, Symbol, Verb};
    pub use crate::image_engine::{ImageEngine, ImageRaster, ImageSource, RawImages, file_icon};
    // geometry is app vocabulary the moment the app paints a box of
    // its own (`custom(…)` / `canvas(…)`)
    pub use crate::layout::{
        Color, CrossAlign, Fraction, Glass, Gradient, Point, Proposal, Px, Rect, Rendering, Side,
        Size, Truncation, UnitPoint, VisualProps,
    };
    pub use crate::theme::{self, Theme};
    pub use crate::text_engine::{FontDesign, FontSpec, PixelFont, TextEngine, Weight};
    pub use crate::text_input::{CaretState, EditCommand};
    pub use crate::one_of::{OneOf3, OneOf4, OneOf5, OneOf6, OneOf7, OneOf8};
    pub use crate::runtime::{Edited, ImeSnapshot, LiveBlit, Runtime};
    pub use crate::state_ext::{BindingExt, StateExt};
    pub use crate::task;
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
        assert!(runtime.tick(1.0 / 120.0).scene);
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

    /// Scrolling a UNIFORM virtual list deep must keep the viewport
    /// COVERED — not merely busy. The window counts rows by one extent
    /// and the placement lays them out by another, so a mismatch of a
    /// single pixel walks the window off the screen one row at a time:
    /// at nine hundred rows it has drifted nine hundred pixels, and
    /// what is left is the handful of rows the two bands still share.
    ///
    /// Counting painted rows does not catch it. The window is still
    /// full of rows; they are simply above the viewport. So this asks
    /// the question the eye asks: is anything drawn near the BOTTOM?
    #[test]
    fn a_deep_window_still_covers_the_bottom_of_the_viewport() {
        #[derive(Clone, Copy)]
        struct Deep;
        impl Component for Deep {
            fn body(self, _ctx: &Context) -> impl View {
                virtual_list(10_000, |row| format!("row{row}"), |row| {
                    text(format!("file_{row:04}.rs")).frame_height(28.0)
                })
                // the shape the finder ships: a row that MEASURES 28
                // and a declaration that says 29
                .row_height(29.0)
            }
        }
        let size = crate::layout::Size { width: 300.0, height: 640.0 };
        let runtime = Runtime::new();
        let _ = runtime.display_frame(&Deep, size);
        let result = runtime.layout(&Deep, crate::layout::Proposal::exact(size));
        let region = result.scrolls.first().expect("the region exists").clone();

        for step in 1..=40 {
            let offset = step as f64 * size.height;
            runtime.set_scroll_offset(&region.path, crate::layout::Point { x: 0.0, y: offset });
            let display = runtime.display_frame(&Deep, size);
            // the deepest text the frame drew, in the region's own space
            let lowest = display
                .iter()
                .filter_map(|command| match command {
                    crate::layout::DrawCommand::TextLine { origin, .. } => Some(origin.y),
                    _ => None,
                })
                .fold(f64::MIN, f64::max);
            assert!(
                lowest >= region.frame.origin.y + region.frame.size.height - 40.0,
                "step {step} at offset {offset}: the lowest row painted sits at {lowest}, \
                 leaving the bottom of the region (ends at {}) empty",
                region.frame.origin.y + region.frame.size.height,
            );
        }
    }

    #[test]
    fn variable_rows_keep_the_geometry_honest() {
        #[derive(Clone, Copy)]
        struct Varied;
        impl Component for Varied {
            fn body(self, _ctx: &Context) -> impl View {
                virtual_list(300, |row| format!("row{row}"), |row| {
                    text(format!("item {row}"))
                })
                .row_height_with(|row| if row % 3 == 0 { 40.0 } else { 20.0 })
            }
        }
        let size = crate::layout::Size { width: 200.0, height: 100.0 };
        let runtime = Runtime::new();
        let _ = runtime.display_frame(&Varied, size);
        let result = runtime.layout(&Varied, crate::layout::Proposal::exact(size));
        let region = result.scrolls.first().expect("the region exists");
        // one hundred tall rows, two hundred short ones — the closure
        // is the authority and the extent stays honest to all of them
        assert_eq!(region.content.height, 100.0 * 40.0 + 200.0 * 20.0);
        let offsets =
            region.row_offsets.as_ref().expect("variable offsets ride the region");
        assert_eq!(offsets.len(), 301);
        assert_eq!(offsets[1], 40.0, "row zero is tall");
        assert_eq!(offsets[2], 60.0, "row one is short");

        // roll deep: the fresh window finds its rows by binary search
        // and places them by prefix sums — bounded, covering, exact
        let region_path = region.path.clone();
        runtime
            .set_scroll_offset(&region_path, crate::layout::Point { x: 0.0, y: 4000.0 });
        let result = runtime.layout(&Varied, crate::layout::Proposal::exact(size));
        // offset 4000 = fifty groups of (40+20+20) — row 150 starts there
        let frame = result
            .frames
            .find("[row150]")
            .expect("the row under the offset is materialized");
        assert_eq!(frame.origin.y, 0.0, "placed at its prefix-sum offset");
        let lines = result
            .display
            .iter()
            .filter(|command| {
                matches!(command, crate::layout::DrawCommand::TextLine { .. })
            })
            .count();
        assert!(lines < 40, "window-sized, not count-sized: {lines}");
    }

    #[test]
    fn a_reveal_lands_on_a_variable_row() {
        #[derive(Clone, Copy)]
        struct Pinned;
        impl Component for Pinned {
            fn body(self, _ctx: &Context) -> impl View {
                virtual_list(300, |row| format!("row{row}"), |row| {
                    text(format!("item {row}"))
                })
                .row_height_with(|row| if row % 3 == 0 { 40.0 } else { 20.0 })
                .reveal(250)
            }
        }
        let size = crate::layout::Size { width: 200.0, height: 100.0 };
        let runtime = Runtime::new();
        let _ = runtime.display_frame(&Pinned, size);
        let result = runtime.layout(&Pinned, crate::layout::Proposal::exact(size));
        let frame = result.frames.find("[row250]").expect("the pinned row exists");
        assert!(
            frame.origin.y >= -1.0 && frame.origin.y < 100.0,
            "the reveal scrolled the row into the viewport: {frame:?}"
        );
    }

    #[test]
    fn a_variable_list_survives_a_count_change() {
        #[derive(Clone)]
        struct Shrinking {
            count: State<usize>,
        }
        impl Component for Shrinking {
            fn body(self, _ctx: &Context) -> impl View {
                virtual_list(self.count.get(), |row| format!("row{row}"), |row| {
                    text(format!("item {row}"))
                })
                .row_height_with(|row| if row % 3 == 0 { 40.0 } else { 20.0 })
            }
        }
        let size = crate::layout::Size { width: 200.0, height: 100.0 };
        let runtime = Runtime::new();
        let view = Shrinking { count: State::new(300) };
        let _ = runtime.display_frame(&view, size);
        let _ = runtime.display_frame(&view, size);

        // stale offsets (len 301) no longer describe count 50 — the
        // window falls back and the geometry re-answers honestly
        view.count.set(50);
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        let region = result.scrolls.first().expect("the region survives");
        assert_eq!(region.content.height, 17.0 * 40.0 + 33.0 * 20.0);
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
    fn a_nested_panel_with_a_scroll_stays_inside_the_window() {
        #[derive(Clone, Copy)]
        struct Launcher;

        impl Component for Launcher {
            fn body(self, _ctx: &Context) -> impl View {
                // the launcher shape: a padded, styled panel WRAPPING a
                // scroll — the stack must stay flexible through the
                // nesting, never freeze at the content's full extent
                crate::vstack!(crate::vstack!(
                    text("header"),
                    virtual_list(10_000, |row| format!("row{row}"), |row| {
                        text(format!("item {row}"))
                    })
                )
                .background_color(crate::layout::Color::WHITE)
                .corner_radius(12.0))
                .padding_length(20.0)
            }
        }

        let size = crate::layout::Size { width: 400.0, height: 300.0 };
        let runtime = Runtime::new();
        let result = runtime.layout(&Launcher, crate::layout::Proposal::exact(size));
        let region = result.scrolls.first().expect("the list scrolls");
        assert!(
            region.frame.size.height <= 300.0,
            "the panel's scroll is bounded by the window: {:?}",
            region.frame
        );
        assert!(
            region.content.height > 100_000.0,
            "the content is still the honest full extent: {:?}",
            region.content
        );
    }

    #[test]
    fn a_window_resize_snaps_animated_origins() {
        use crate::anim::Spring;

        // the spacer pushes the text to the trailing edge — every width
        // change moves the animated view
        #[derive(Clone, Copy)]
        struct Trailing;

        impl Component for Trailing {
            fn body(self, _ctx: &Context) -> impl View {
                crate::hstack!(
                    spacer(),
                    text("steady")
                        .background_color(crate::layout::Color::hex(0x334455))
                        .animated(Spring::smooth())
                )
            }
        }

        let narrow = crate::layout::Size { width: 300.0, height: 100.0 };
        let wide = crate::layout::Size { width: 500.0, height: 100.0 };
        let runtime = Runtime::new();
        let _ = runtime.display_frame(&Trailing, narrow);
        let _ = runtime.display_frame(&Trailing, narrow);

        // a resize is not an animation: the very first frame at the new
        // size paints exactly where a fresh runtime would — no flight,
        // no trail behind a live-resizing window
        let resized = runtime.display_frame(&Trailing, wide);
        let fresh = Runtime::new().display_frame(&Trailing, wide);
        assert_eq!(
            resized.as_slice(),
            fresh.as_slice(),
            "the resized frame snapped to the new geometry"
        );
        assert!(!runtime.wants_frame(), "nothing left mid-flight after the snap");
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
    fn a_dynamic_collection_lays_itself_out_on_either_axis() {
        use crate::layout::{DrawCommand, Proposal};

        let runtime = Runtime::new();
        let items = vec!["one", "two"];
        // a tab strip: the items sit side by side, the taller one sets
        // the height and the shorter one centers against it
        let strip = for_each(items.clone(), |id| id.to_string(), |id| match *id {
            "one" => Either::First(text(*id)),
            _ => Either::Second(text(*id).padding_length(4.0)),
        })
        .horizontal()
        .spacing(6.0);

        let ink: Vec<(f64, f64)> = runtime
            .layout(&strip, Proposal::unspecified())
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { origin, .. } => Some((origin.x, origin.y)),
                _ => None,
            })
            .collect();
        // PixelFont: 8px a glyph, 16 tall. "one" is 24 wide, the gap is
        // 6, the padded "two" starts its ink 4 further in — and both
        // sit at y = 4, the centered offset inside a 24pt strip
        assert_eq!(ink, vec![(0.0, 4.0), (34.0, 4.0)]);

        let printed = runtime.render_stable(&strip);
        assert!(
            printed.contains("ForEach (2, axis: .horizontal, spacing: 6)"),
            "the print says how it lays out: {printed}"
        );
        let column = for_each(items, |id| id.to_string(), |id| text(*id));
        assert!(
            runtime.render_stable(&column).contains("ForEach (2)"),
            "the default column prints as it always did"
        );
        // a cross alignment of its own — the frame-per-row a page used to
        // write to center a column of headings, said once on the run
        let rows = vec!["a", "b"];
        let centered = for_each(rows, |id| id.to_string(), |id| text(*id))
            .cross_alignment(crate::layout::CrossAlign::Center);
        assert!(
            runtime.render_stable(&centered).contains("ForEach (2, alignment: Center)"),
            "a cross alignment of its own says so"
        );
    }

    #[test]
    fn the_weight_scale_travels_as_a_number() {
        use crate::text_engine::Weight;
        // the six the vocabulary can now spell, and the code each one
        // travels as — the glue's table is indexed by exactly this
        let scale = [
            Weight::Regular,
            Weight::Medium,
            Weight::Semibold,
            Weight::Bold,
            Weight::ExtraBold,
            Weight::Black,
        ];
        let code_of = |weight: Weight| -> u8 {
            let text = crate::dom::DomText {
                content: std::sync::Arc::from("x"),
                color: Color::BLACK,
                inherits_ink: false,
                font: crate::text_engine::FontSpec { weight, ..crate::text_engine::FontSpec::DEFAULT },
                line_height: None,
                text_align: None,
                highlights: None,
                truncation: None,
            };
            let bytes = crate::dom::encode(&[crate::dom::DomPatch::SetText { id: 1, text }]);
            // count(4), op(1), id(4), color(4), inherits(1), size(4) —
            // then the weight
            bytes[18]
        };
        let codes: Vec<u8> = scale.iter().map(|weight| code_of(*weight)).collect();
        assert_eq!(codes, vec![0, 1, 2, 3, 4, 5]);
        // the glue mirrors them by hand: one CSS number per code
        let glue = include_str!("../../bunny_ui_web/glue/glue_dom.js");
        assert!(
            glue.contains("const CSS_WEIGHTS = [400, 500, 600, 700, 800, 900];"),
            "the glue's weight table drifted from the engine's scale"
        );
        // and the modifier carries the name through to the printed tree
        let runtime = Runtime::new();
        let printed = runtime.render_stable(&text("heavy").font_weight(Weight::ExtraBold));
        assert!(printed.contains("[.fontWeight(.ExtraBold)]"), "{printed}");
    }

    #[test]
    fn a_vec_of_views_flattens_like_a_tuple() {
        let runtime = Runtime::new();
        // the same three children, spelled two ways — a Vec says what a
        // tuple says, which is what lets a list of unknown length exist
        let tupled = runtime.render_stable(&vstack((text("a"), text("b"), text("c"))));
        let listed =
            runtime.render_stable(&vstack(vec![text("a"), text("b"), text("c")]));
        assert_eq!(tupled, listed);
        // and it is Many, so it flattens INTO the stack rather than
        // wrapping itself in one
        assert_eq!(listed.matches("Text").count(), 3);
    }

    #[test]
    fn line_height_steps_the_lines_and_centres_them() {
        use crate::layout::{DrawCommand, Proposal};
        let runtime = Runtime::new();
        let line_tops = |display: &crate::layout::DisplayList| -> Vec<f64> {
            display
                .iter()
                .filter_map(|command| match command {
                    DrawCommand::TextLine { origin, .. } => Some(origin.y),
                    _ => None,
                })
                .collect()
        };
        // PixelFont is 8px a glyph, 16 tall; "aaa bbb" is 56 wide and
        // wraps in a 30pt column into "aaa" and "bbb". A 24pt line box
        // steps the lines by 24 and splits the 8pt of extra leading, so
        // the first line sits 4 down.
        let para = text("aaa bbb").line_height(24.0).frame_width(30.0);
        let stepped = line_tops(&runtime.layout(&para, Proposal::unspecified()).display);
        assert_eq!(stepped, vec![4.0, 28.0], "each line steps by 24, centred by a 4pt half-leading");
        // and with no line height the lines step by the face's own 16
        let plain = text("aaa bbb").frame_width(30.0);
        let bare = line_tops(&runtime.layout(&plain, Proposal::unspecified()).display);
        assert_eq!(bare, vec![0.0, 16.0], "unset, the old face box stands");
    }

    #[test]
    fn what_a_task_lands_reaches_the_next_frame() {
        #[derive(Clone, Copy)]
        struct Panel {
            lines: State<usize>,
        }

        impl Component for Panel {
            fn body(self, _ctx: &Context) -> impl View {
                text(format!("{} lines", self.lines.get()))
            }
        }

        let runtime = Runtime::new();
        let view = Panel { lines: State::new(0) };
        assert!(runtime.render_stable(&view).contains("0 lines"));

        // the APP owns the work and the thread; the bridge is the
        // channel, and the engine only learns the result
        let (sender, receiver) = motor::task::channel::<usize>();
        let lines = view.lines;
        let task = runtime.spawn(async move {
            while let Some(count) = receiver.recv().await {
                lines.set(count);
            }
        });
        std::thread::spawn(move || {
            sender.send(3).expect("the panel is reading");
            sender.send(7).expect("the panel is reading");
        })
        .join()
        .expect("the worker");

        // one settle drains what landed and renders with it
        assert!(runtime.render_stable(&view).contains("7 lines"));

        // and the view stops hearing when the task is gone
        drop(task);
        assert_eq!(motor::task::pending(), 0);
    }

    #[test]
    fn a_task_starts_once_and_dies_with_its_view() {
        use std::cell::Cell;

        #[derive(Clone)]
        struct Screen {
            open: State<bool>,
            other: State<usize>,
            starts: Rc<Cell<usize>>,
        }

        impl Component for Screen {
            fn body(self, _ctx: &Context) -> impl View {
                let starts = Rc::clone(&self.starts);
                if self.open.get() {
                    Either::First(
                        text(format!("watching {}", self.other.get())).task(move || {
                            starts.set(starts.get() + 1);
                            // a job that never finishes: what matters
                            // here is who ends it
                            std::future::pending::<()>()
                        }),
                    )
                } else {
                    Either::Second(text("closed"))
                }
            }
        }

        let runtime = Runtime::new();
        let view = Screen {
            open: State::new(true),
            other: State::new(0),
            starts: Rc::new(Cell::new(0)),
        };

        let printed = runtime.render_stable(&view);
        assert!(printed.contains("[.task()]"), "the print says it: {printed}");
        assert_eq!(view.starts.get(), 1, "the task started on appearance");
        assert_eq!(motor::task::pending(), 1);

        // a re-render for another reason never restarts it
        view.other.set(1);
        runtime.render_stable(&view);
        assert_eq!(view.starts.get(), 1, "a re-render is not a restart");
        assert_eq!(motor::task::pending(), 1);

        // the view leaves the tree: the identity sweep drops the slot,
        // and dropping the handle cancels
        view.open.set(false);
        runtime.render_stable(&view);
        assert_eq!(motor::task::pending(), 0, "the task died with the view");
    }

    #[test]
    fn a_named_view_is_addressed_by_its_name_not_its_place() {
        use crate::layout::{Proposal, Size};

        #[derive(Clone, Copy)]
        struct Stripe {
            extra: State<bool>,
        }

        impl Component for Stripe {
            fn body(self, _ctx: &Context) -> impl View {
                // a slot arrives above the named one; without the name
                // the hit path of the git slot would shift with it
                let head = if self.extra.get() {
                    Either::First(text("search").on_click(|| {}))
                } else {
                    Either::Second(empty())
                };
                vstack((
                    head,
                    text("files").on_click(|| {}).id("explorer"),
                    text("branch").on_click(|| {}).id("git"),
                ))
            }
        }

        let runtime = Runtime::new();
        let view = Stripe { extra: State::new(false) };
        let viewport = Proposal::exact(Size { width: 40.0, height: 200.0 });
        let path_of = |result: &crate::layout::LayoutResult, name: &str| {
            result
                .hits
                .iter()
                .find(|(path, _)| path.contains(name))
                .map(|(path, _)| path.clone())
                .unwrap_or_default()
        };

        let before = runtime.layout(&view, viewport);
        let git = path_of(&before, "[git]");
        assert!(git.contains("[git]"), "the name is in the address: {git}");

        view.extra.set(true);
        let after = runtime.layout(&view, viewport);
        assert_eq!(path_of(&after, "[git]"), git, "a new sibling above renames nothing");
        assert!(
            runtime.render_stable(&view).contains("[.id(\"git\")]"),
            "the print says the name"
        );
    }

    #[test]
    fn a_sleeping_task_wakes_on_the_engines_clock() {
        #[derive(Clone, Copy)]
        struct Debounced {
            typed: State<usize>,
            searched: State<usize>,
        }

        impl Component for Debounced {
            fn body(self, _ctx: &Context) -> impl View {
                let searched = self.searched;
                let typed = self.typed;
                // the search field's recipe: every keystroke restarts
                // the task, so only the last one reaches the work
                text(format!("searched {}", searched.get())).task_id(typed.get(), move || async move {
                    task::sleep(std::time::Duration::from_millis(250)).await;
                    searched.set(typed.get());
                })
            }
        }

        let runtime = Runtime::new();
        let view = Debounced { typed: State::new(1), searched: State::new(0) };
        runtime.render_stable(&view);
        assert!(runtime.wants_frame(), "a sleeper keeps the clock moving");

        // 200ms of frames: still waiting
        for _ in 0..12 {
            runtime.tick(1.0 / 60.0);
        }
        runtime.render_stable(&view);
        assert_eq!(view.searched.get(), 0, "not yet");

        // a keystroke restarts the wait
        view.typed.set(2);
        runtime.render_stable(&view);
        for _ in 0..12 {
            runtime.tick(1.0 / 60.0);
        }
        runtime.render_stable(&view);
        assert_eq!(view.searched.get(), 0, "the restart threw the wait away");

        for _ in 0..4 {
            runtime.tick(1.0 / 60.0);
        }
        runtime.render_stable(&view);
        assert_eq!(view.searched.get(), 2, "the last keystroke is the one that searched");
        assert!(!runtime.wants_frame(), "and the driver goes back to sleep");
    }

    #[test]
    fn an_empty_virtual_list_asks_for_no_row() {
        use crate::layout::{Proposal, Size};
        use std::cell::Cell;

        let asked = Rc::new(Cell::new(0));
        let list = {
            let asked = Rc::clone(&asked);
            virtual_list(
                0,
                move |index| {
                    asked.set(asked.get() + 1);
                    index.to_string()
                },
                |index| text(format!("row {index}")),
            )
        };

        let runtime = Runtime::new();
        runtime.layout(&list, Proposal::exact(Size { width: 200.0, height: 100.0 }));
        assert_eq!(
            asked.get(),
            0,
            "a list still waiting for its data has no row to ask about"
        );
    }

    #[test]
    fn a_skipped_view_keeps_its_task() {
        use std::cell::Cell;

        #[derive(Clone)]
        struct Watcher {
            starts: Rc<Cell<usize>>,
        }

        impl Component for Watcher {
            fn body(self, _ctx: &Context) -> impl View {
                let starts = Rc::clone(&self.starts);
                text("watching").task(move || {
                    starts.set(starts.get() + 1);
                    std::future::pending::<()>()
                })
            }
        }

        #[derive(Clone)]
        struct Screen {
            ticks: State<usize>,
            starts: Rc<Cell<usize>>,
        }

        impl Component for Screen {
            fn body(self, _ctx: &Context) -> impl View {
                vstack((
                    text(format!("tick {}", self.ticks.get())),
                    Watcher { starts: Rc::clone(&self.starts) },
                ))
            }
        }

        let runtime = Runtime::new();
        let view = Screen { ticks: State::new(0), starts: Rc::new(Cell::new(0)) };
        runtime.render_stable(&view);
        assert_eq!(motor::task::pending(), 1);

        // the parent re-renders and the child is SKIPPED — its effects
        // still come from the retention, so the task is still declared
        view.ticks.set(1);
        runtime.render_stable(&view);
        assert!(
            !runtime.body_runs().iter().any(|path| path.contains("Watcher")),
            "the child was skipped: {:?}",
            runtime.body_runs()
        );
        assert_eq!(view.starts.get(), 1, "and it was not restarted");
        assert_eq!(motor::task::pending(), 1, "nor cancelled by the sweep");
    }

    #[test]
    fn a_task_restarts_when_its_id_moves() {
        use std::cell::Cell;

        #[derive(Clone)]
        struct Detail {
            file: State<usize>,
            starts: Rc<Cell<usize>>,
        }

        impl Component for Detail {
            fn body(self, _ctx: &Context) -> impl View {
                let starts = Rc::clone(&self.starts);
                // the id is the file being read: another file, another
                // read, and the one in flight is cancelled
                text("detail").task_id(self.file.get(), move || {
                    starts.set(starts.get() + 1);
                    std::future::pending::<()>()
                })
            }
        }

        let runtime = Runtime::new();
        let view = Detail { file: State::new(0), starts: Rc::new(Cell::new(0)) };
        let printed = runtime.render_stable(&view);
        assert!(printed.contains("[.task(id:)]"), "{printed}");
        assert_eq!(view.starts.get(), 1);

        runtime.render_stable(&view);
        assert_eq!(view.starts.get(), 1, "the same id keeps the same task");

        view.file.set(1);
        runtime.render_stable(&view);
        assert_eq!(view.starts.get(), 2, "a new id starts the work again");
        assert_eq!(motor::task::pending(), 1, "and the old one is gone");
    }

    #[test]
    fn a_layered_stack_hugs_the_edge_it_was_given() {
        use crate::layout::{DrawCommand, Proposal, Size};

        let accent = Color::hex(0xFF6600);
        let runtime = Runtime::new();
        // the active row: a 2pt accent bar layered over a background
        // that fills the whole row
        let bar_x = |alignment| {
            let row = zstack!(
                spacer().background_color(theme::panel()),
                spacer().frame(2.0, 20.0).background_color(accent),
            )
            .alignment(alignment);
            runtime
                .layout(&row, Proposal::exact(Size { width: 120.0, height: 20.0 }))
                .display
                .iter()
                .find_map(|command| match command {
                    DrawCommand::FillRect { rect, color, .. } if *color == accent => {
                        Some(rect.origin.x)
                    }
                    _ => None,
                })
                .expect("the bar paints")
        };

        assert_eq!(bar_x(Alignment::Leading), 0.0, "hugs the leading edge");
        assert_eq!(bar_x(Alignment::Center), 59.0, "centered, as always");
        assert_eq!(bar_x(Alignment::Trailing), 118.0, "hugs the trailing edge");
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
    fn a_looping_box_reads_the_phase_and_a_step_repaints_it_alone() {
        use crate::anim::Loop;
        use crate::custom::canvas;

        // a box whose picture IS the phase: the red channel counts it
        #[derive(Clone, Copy)]
        struct Mark;
        impl Component for Mark {
            fn body(self, _ctx: &Context) -> impl View {
                canvas(|ctx, p| {
                    let red = (ctx.phase * 255.0).round() as u8;
                    p.fill(ctx.bounds(), crate::layout::Color::rgba(red, 0, 0, 255));
                })
                .looping(Loop::secs(1.0).fps(4.0))
                .frame(20.0, 20.0)
            }
        }

        let red_of = |display: &crate::layout::DisplayList| {
            display
                .iter()
                .find_map(|command| match command {
                    crate::layout::DrawCommand::FillRect { color, .. } => Some(color.r),
                    _ => None,
                })
                .expect("the box paints its fill")
        };

        let size = crate::layout::Size { width: 20.0, height: 20.0 };
        let runtime = Runtime::new();
        let mounted = runtime.display_frame(&Mark, size);
        assert_eq!(red_of(&mounted), 0, "the clock seeds on the still frame");
        assert!(runtime.wants_frame(), "a live loop keeps frames coming");
        assert_eq!(
            runtime.frame_pace(),
            crate::anim::FramePace::Slow(0.25),
            "loops alone ask one frame per step"
        );

        // inside the first step: nothing anywhere
        assert!(!runtime.tick(0.1).any());
        assert!(runtime.live_islands(1).is_empty());

        // crossing a step: the island repaints, the scene never asked
        let moved = runtime.tick(0.16);
        assert!(moved.islands && !moved.scene, "a loop never asks for layout");
        let blits = runtime.live_islands(1);
        assert_eq!(blits.len(), 1);
        let first = &blits[0];
        assert_eq!((first.width, first.height), (20, 20));
        assert_eq!(first.frame.size.width, 20.0);
        assert_eq!(first.rgba[0], 64, "the pixels carry the quarter phase");
        assert!(runtime.live_islands(1).is_empty(), "the step was drained");

        // the next step paints a DIFFERENT picture
        assert!(runtime.tick(0.25).islands);
        let second = runtime.live_islands(1);
        assert_eq!(second.len(), 1);
        assert_ne!(second[0].rgba[0], first.rgba[0]);

        // an ordinary scene frame paints the CURRENT phase — the box
        // never jumps back when the app changes around it
        let scene = runtime.animation_frame(&Mark, size);
        assert_eq!(red_of(&scene), second[0].rgba[0]);
    }

    #[test]
    fn the_scene_without_the_live_slices_keeps_everything_else() {
        use crate::anim::Loop;
        use crate::custom::canvas;

        #[derive(Clone, Copy)]
        struct Bar;
        impl Component for Bar {
            fn body(self, _ctx: &Context) -> impl View {
                hstack!(
                    canvas(|ctx, p| {
                        p.fill(ctx.bounds(), crate::layout::Color::rgba(1, 2, 3, 255));
                    })
                    .looping(Loop::secs(1.0))
                    .frame(10.0, 10.0),
                    text("beside the mark")
                )
            }
        }

        let runtime = Runtime::new();
        let size = crate::layout::Size { width: 120.0, height: 20.0 };
        let full = runtime.display_frame(&Bar, size);
        let slices = runtime.live_slices();
        assert_eq!(slices.len(), 1, "one live box, one slice");
        // the scene DOES hold the box — carving is a choice, and a
        // shell that declines it (a window mid-resize, where a layer of
        // its own would land in a different beat) draws the box like
        // anything else
        assert!(full.iter().any(|command| matches!(
            command,
            crate::layout::DrawCommand::FillRect { color, .. }
                if color.r == 1 && color.g == 2 && color.b == 3
        )));
        let carved = full.without_slices(&slices);
        // the box's commands (its clip pair and its fill) leave; the
        // text beside it stays
        let (start, end) = slices[0];
        assert_eq!(carved.as_slice().len(), full.as_slice().len() - (end - start));
        assert!(carved.iter().all(|command| !matches!(
            command,
            crate::layout::DrawCommand::FillRect { color, .. }
                if color.r == 1 && color.g == 2 && color.b == 3
        )));
        assert!(carved.iter().any(|command| matches!(
            command,
            crate::layout::DrawCommand::TextLine { .. }
        )));
    }

    #[test]
    fn a_step_that_paints_the_same_picture_blits_nothing() {
        use crate::anim::Loop;
        use crate::custom::canvas;

        // alive, but blind to the phase: steps arrive, pixels never move
        #[derive(Clone, Copy)]
        struct Steady;
        impl Component for Steady {
            fn body(self, _ctx: &Context) -> impl View {
                canvas(|ctx, p| {
                    p.fill(ctx.bounds(), crate::layout::Color::rgba(9, 9, 9, 255));
                })
                .looping(Loop::secs(1.0).fps(10.0))
                .frame(8.0, 8.0)
            }
        }

        let runtime = Runtime::new();
        let size = crate::layout::Size { width: 8.0, height: 8.0 };
        let _ = runtime.display_frame(&Steady, size);
        assert!(runtime.tick(0.15).islands);
        assert_eq!(runtime.live_islands(1).len(), 1, "the first step seeds the ledger");
        assert!(runtime.tick(0.1).islands);
        assert!(
            runtime.live_islands(1).is_empty(),
            "the ledger drops a step that paints the same picture"
        );
    }

    #[test]
    fn an_ordinary_frame_never_repaints_an_unchanged_live_box() {
        use crate::anim::Loop;
        use crate::custom::canvas;

        #[derive(Clone, Copy)]
        struct Mark;
        impl Component for Mark {
            fn body(self, _ctx: &Context) -> impl View {
                canvas(|ctx, p| {
                    let red = (ctx.phase * 255.0).round() as u8;
                    p.fill(ctx.bounds(), crate::layout::Color::rgba(red, 0, 0, 255));
                })
                .looping(Loop::secs(1.0).fps(4.0))
                .frame(12.0, 12.0)
            }
        }

        let runtime = Runtime::new();
        let size = crate::layout::Size { width: 12.0, height: 12.0 };
        let _ = runtime.display_frame(&Mark, size);
        // the first ordinary present seeds the surface...
        assert_eq!(runtime.live_islands_all(1).len(), 1);
        // ...and the next one (a wake, a poll — no step between them)
        // rasters NOTHING: the ledger answers for the app's chatter
        assert!(runtime.live_islands_all(1).is_empty());
        // the placement is still re-announced for the layer to follow
        assert_eq!(runtime.live_frames().len(), 1);
        // a real step still repaints exactly once
        assert!(runtime.tick(0.3).islands);
        assert_eq!(runtime.live_islands(1).len(), 1);
    }

    /// A live box owns a surface, and a surface holds the pixels it was
    /// handed: place it bigger without new pixels and it STRETCHES the
    /// old ones. The window resize is where that shows — the editor
    /// deforms while the chrome around it stays sharp.
    #[test]
    fn a_live_box_that_grows_owes_its_surface_new_pixels() {
        use crate::anim::Loop;
        use crate::custom::canvas;

        // the paint does NOT depend on the box's width: a caret is a
        // caret whatever the window does. That is precisely the box the
        // old ledger dropped on a resize.
        #[derive(Clone, Copy)]
        struct Caret;
        impl Component for Caret {
            fn body(self, _ctx: &Context) -> impl View {
                canvas(|ctx, p| {
                    let on = ctx.phase < 0.5;
                    p.fill(
                        crate::layout::Rect {
                            origin: crate::layout::Point { x: 0.0, y: 0.0 },
                            size: crate::layout::Size { width: 2.0, height: 12.0 },
                        },
                        crate::layout::Color::rgba(0, 0, 0, if on { 255 } else { 0 }),
                    );
                })
                .looping(Loop::secs(1.0).fps(2.0))
            }
        }

        let runtime = Runtime::new();
        let narrow = crate::layout::Size { width: 100.0, height: 40.0 };
        let _ = runtime.display_frame(&Caret, narrow);
        let seed = runtime.live_islands_all(1);
        assert_eq!(seed.len(), 1, "the first present seeds the surface");
        assert_eq!(seed[0].width, 100, "at the window's width");
        // an unchanged frame still costs no raster — the whole point of
        // the ledger, and it must survive this fix
        assert!(runtime.live_islands_all(1).is_empty());

        // the window grows. The box paints the SAME two-by-twelve bar,
        // so the picture is byte for byte what it was — and the surface
        // is now 300 wide with 100 points of pixels in it
        let wide = crate::layout::Size { width: 300.0, height: 40.0 };
        let _ = runtime.display_frame(&Caret, wide);
        let grown = runtime.live_islands_all(1);
        assert_eq!(grown.len(), 1, "a box that grew owes new pixels");
        assert_eq!(grown[0].width, 300, "rasterized at the NEW size");
        assert_eq!(grown[0].frame.size.width, 300.0, "and placed at it");
        // and settling at the new size goes quiet again
        assert!(runtime.live_islands_all(1).is_empty());

        // the same law for the SCALE: dragging the window to a retina
        // screen re-rasters, because those are new pixels too
        let retina = runtime.live_islands_all(2);
        assert_eq!(retina.len(), 1, "a new scale is new pixels");
        assert_eq!(retina[0].width, 600);
        assert!(runtime.live_islands_all(2).is_empty());
    }

    /// A mark anchored to the TOP does not move when the window grows
    /// taller — its layout position is the same number before and
    /// after. Whatever a shell does with its surface has to preserve
    /// that: a world where the mark's position depends on the window's
    /// height is a world where a placement that has not happened yet is
    /// already wrong, and the error grows with the drag.
    #[test]
    fn a_top_anchored_live_box_does_not_move_when_the_window_grows() {
        use crate::anim::Loop;
        use crate::custom::canvas;

        #[derive(Clone, Copy)]
        struct Bar;
        impl Component for Bar {
            fn body(self, _ctx: &Context) -> impl View {
                vstack!(
                    canvas(|ctx, p| {
                        p.fill(ctx.bounds(), crate::layout::Color::BLACK);
                    })
                    .looping(Loop::secs(4.8).fps(5.0))
                    .frame(24.0, 24.0),
                    spacer(),
                )
            }
        }

        let runtime = Runtime::new();
        let short = runtime.display_frame(&Bar, crate::layout::Size { width: 400.0, height: 300.0 });
        let _ = short;
        let before = runtime.live_frames();
        assert_eq!(before.len(), 1);

        let _ = runtime.display_frame(&Bar, crate::layout::Size { width: 400.0, height: 900.0 });
        let after = runtime.live_frames();
        assert_eq!(
            after[0].1.origin, before[0].1.origin,
            "three hundred points of window later, the mark is where it was",
        );
        assert_eq!(after[0].1.size, before[0].1.size);
    }

    /// A shell that dissolves a live box's surface (a window mid-resize
    /// draws the box into the scene instead) has to say so, or the
    /// ledger keeps answering for a surface that holds nothing and the
    /// box comes back empty.
    #[test]
    fn a_dissolved_surface_is_seeded_again_on_the_next_present() {
        use crate::anim::Loop;
        use crate::custom::canvas;

        #[derive(Clone, Copy)]
        struct Mark;
        impl Component for Mark {
            fn body(self, _ctx: &Context) -> impl View {
                canvas(|ctx, p| {
                    p.fill(ctx.bounds(), crate::layout::Color::BLACK);
                })
                .looping(Loop::secs(1.0).fps(4.0))
                .frame(24.0, 24.0)
            }
        }

        let runtime = Runtime::new();
        let size = crate::layout::Size { width: 200.0, height: 60.0 };
        let _ = runtime.display_frame(&Mark, size);
        assert_eq!(runtime.live_islands_all(1).len(), 1, "the first present seeds it");
        assert!(runtime.live_islands_all(1).is_empty(), "and the next one costs nothing");

        // the shell took the surfaces away — the box was drawn into the
        // scene while the window changed size
        runtime.forget_live_surfaces();
        assert_eq!(
            runtime.live_islands_all(1).len(),
            1,
            "the surface is gone, so the picture is owed again",
        );
        assert!(runtime.live_islands_all(1).is_empty(), "and then it settles");
    }

    #[test]
    fn the_last_viewport_is_the_world_a_live_layer_flips_into() {
        use crate::anim::Loop;
        use crate::custom::canvas;

        #[derive(Clone, Copy)]
        struct Mark;
        impl Component for Mark {
            fn body(self, _ctx: &Context) -> impl View {
                canvas(|ctx, p| {
                    p.fill(ctx.bounds(), crate::layout::Color::BLACK);
                })
                .looping(Loop::secs(1.0).fps(4.0))
                .frame(10.0, 10.0)
            }
        }

        let runtime = Runtime::new();
        assert_eq!(runtime.last_viewport(), None, "nothing was laid out yet");
        let size = crate::layout::Size { width: 220.0, height: 140.0 };
        let _ = runtime.display_frame(&Mark, size);
        assert_eq!(
            runtime.last_viewport(),
            Some(size),
            "the shell flips a live layer into THIS height, not the view's",
        );
    }

    #[test]
    fn reduce_motion_holds_a_looping_box_on_its_resting_frame() {
        use crate::anim::Loop;
        use crate::custom::canvas;

        #[derive(Clone, Copy)]
        struct Mark;
        impl Component for Mark {
            fn body(self, _ctx: &Context) -> impl View {
                canvas(|ctx, p| {
                    let red = (ctx.phase * 255.0).round() as u8;
                    p.fill(ctx.bounds(), crate::layout::Color::rgba(red, 0, 0, 255));
                })
                .looping(Loop::secs(1.0).fps(4.0).still_at(0.5))
                .frame(20.0, 20.0)
            }
        }

        let runtime = Runtime::new();
        runtime.set_reduce_motion(true);
        let size = crate::layout::Size { width: 20.0, height: 20.0 };
        let mounted = runtime.display_frame(&Mark, size);
        let red = mounted
            .iter()
            .find_map(|command| match command {
                crate::layout::DrawCommand::FillRect { color, .. } => Some(color.r),
                _ => None,
            })
            .expect("the box paints its fill");
        assert_eq!(red, 128, "the resting frame, not the start of the loop");
        assert!(!runtime.wants_frame(), "no clock runs for a reduced-motion user");
        assert!(!runtime.tick(1.0).any());
        assert_eq!(runtime.frame_pace(), crate::anim::FramePace::Idle);
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
        assert!(!runtime.tick(1.0 / 120.0).any());
    }

    #[test]
    fn a_tick_without_animations_asks_for_nothing() {
        // the frame-driver contract: a tick on a runtime with no live
        // animation reports no repaint and no wish for a next frame —
        // the shell parks the display link on this answer
        let runtime = Runtime::new();
        assert!(!runtime.tick(1.0 / 120.0).any());
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
    fn a_divider_drag_writes_the_binding_and_moves_the_seam() {
        use crate::layout::{Proposal, Size};

        #[derive(Clone, Copy)]
        struct Bench {
            seam: State<f64>,
        }

        impl Component for Bench {
            fn body(self, _ctx: &Context) -> impl View {
                hsplit(
                    self.seam.binding(),
                    text("panel"),
                    text("editor"),
                )
                .min_sizes(120.0, 200.0)
            }
        }

        let bench = Bench { seam: State::new(260.0) };
        let runtime = Runtime::new();
        runtime.render_stable(&bench);
        let viewport = Proposal::exact(Size { width: 1200.0, height: 700.0 });
        let result = runtime.layout(&bench, viewport);

        // the grip is where the seam is
        let (grip_path, grip) = result
            .hits
            .iter()
            .find(|(path, _)| path.ends_with("/#split"))
            .expect("the seam registers a grip")
            .clone();
        let grip_center_x = grip.origin.x + grip.size.width / 2.0;

        // press the grip, drag right: the binding follows the pointer
        assert!(runtime.pointer_pressed(grip_center_x, 300.0));
        assert!(runtime.pointer_moved(400.0, 300.0, false));
        assert_eq!(bench.seam.get(), 400.0);
        let dragged = runtime.layout(&bench, viewport);
        let moved = dragged
            .hits
            .iter()
            .find(|(path, _)| path.ends_with("/#split"))
            .expect("the seam persists")
            .clone();
        assert!((moved.1.origin.x + moved.1.size.width / 2.0 - 400.5).abs() < 0.75);

        // past the floor the clamp holds — and a release fires nothing
        runtime.pointer_moved(5.0, 300.0, false);
        assert_eq!(bench.seam.get(), 120.0);
        assert_eq!(runtime.pointer_released(5.0, 300.0), None);

        // after the release the pointer is free again: moving does not drag
        runtime.pointer_moved(700.0, 300.0, false);
        assert_eq!(bench.seam.get(), 120.0);
        let _ = grip_path;
    }

    /// Dragging a TRAILING seam writes back the lane the app is holding.
    ///
    /// The pointer lands where it lands; what changes is which side of it
    /// the binding names. Get this backwards and a dock resizes the wrong
    /// way under the hand — the grip goes left and the dock grows — which
    /// is the kind of wrong that reads as the whole workbench being broken.
    #[test]
    fn dragging_a_trailing_seam_writes_the_far_lane() {
        #[derive(Clone)]
        struct Bench {
            seam: State<f64>,
        }
        impl Component for Bench {
            fn body(self, _ctx: &Context) -> impl View {
                hsplit(self.seam.binding(), text("editor"), text("dock"))
                    .min_sizes(320.0, 180.0)
                    .seam_on_trailing()
            }
        }

        let bench = Bench { seam: State::new(248.0) };
        let runtime = Runtime::new();
        runtime.render_stable(&bench);
        let viewport = Proposal::exact(Size { width: 1200.0, height: 700.0 });
        let result = runtime.layout(&bench, viewport);

        // The seam sits 248 from the TRAILING edge, not from the leading one.
        let grip = result
            .hits
            .iter()
            .find(|(path, _)| path.ends_with("/#split"))
            .expect("the seam registers a grip")
            .1;
        let centre = grip.origin.x + grip.size.width / 2.0;
        assert!((centre - (1200.0 - 248.0)).abs() < 1.0, "the grip rides the dock's edge: {centre}");

        // Drag it LEFT and the dock gets wider — the pointer names the
        // dock's edge, and the dock is what the binding holds.
        assert!(runtime.pointer_pressed(centre, 300.0));
        assert!(runtime.pointer_moved(800.0, 300.0, false));
        assert!(
            (bench.seam.get() - (1199.0 - 800.0)).abs() < 1.0,
            "the far lane is what was written: {}",
            bench.seam.get(),
        );

        // The floors still name the lanes: pushing the grip at the window's
        // edge cannot squeeze the LEADING lane under its own floor.
        runtime.pointer_moved(10.0, 300.0, false);
        assert!(
            (bench.seam.get() - (1199.0 - 320.0)).abs() < 1.0,
            "the editor keeps its 320: {}",
            bench.seam.get(),
        );
        runtime.pointer_released(10.0, 300.0);
    }

    #[test]
    fn a_fractional_seam_keeps_its_share_when_the_window_moves() {
        use crate::layout::{Fraction, Proposal, Size};

        #[derive(Clone, Copy)]
        struct Panes {
            share: State<Fraction>,
        }

        impl Component for Panes {
            fn body(self, _ctx: &Context) -> impl View {
                hsplit(self.share.binding(), text("left"), text("right"))
            }
        }

        let panes = Panes { share: State::new(Fraction(0.25)) };
        let runtime = Runtime::new();
        runtime.render_stable(&panes);
        // the divider is one point, so the lanes share what is left —
        // and the grip sits on its middle, half a point past the lane
        let lane = |width: f64| {
            let result =
                runtime.layout(&panes, Proposal::exact(Size { width, height: 400.0 }));
            let grip = result
                .hits
                .iter()
                .find(|(path, _)| path.ends_with("/#split"))
                .expect("the seam registers a grip")
                .1;
            grip.origin.x + grip.size.width / 2.0 - 0.5
        };
        assert_eq!(lane(801.0), 200.0, "a quarter of eight hundred");
        assert_eq!(lane(401.0), 100.0, "still a quarter when the window halves");

        // the same seam in POINTS does the opposite, on purpose: lane A
        // is a number and lane B eats every point the window gains
        #[derive(Clone, Copy)]
        struct Pinned {
            seam: State<f64>,
        }

        impl Component for Pinned {
            fn body(self, _ctx: &Context) -> impl View {
                hsplit(self.seam.binding(), text("left"), text("right"))
                    .min_sizes(50.0, 50.0)
            }
        }

        let pinned = Pinned { seam: State::new(200.0) };
        let runtime = Runtime::new();
        runtime.render_stable(&pinned);
        let pinned_lane = |width: f64| {
            let result =
                runtime.layout(&pinned, Proposal::exact(Size { width, height: 400.0 }));
            let grip = result
                .hits
                .iter()
                .find(|(path, _)| path.ends_with("/#split"))
                .expect("the seam registers a grip")
                .1;
            grip.origin.x + grip.size.width / 2.0 - 0.5
        };
        assert_eq!(pinned_lane(801.0), 200.0);
        assert_eq!(pinned_lane(401.0), 200.0);
    }

    #[test]
    fn a_seam_names_the_axis_it_resizes() {
        use crate::layout::{Axis, Proposal, Size};

        // a workbench is seams in BOTH directions: a dock beside the
        // editor, and a pane split downwards inside it
        #[derive(Clone, Copy)]
        struct Bench {
            dock: State<f64>,
            pane: State<f64>,
        }

        impl Component for Bench {
            fn body(self, _ctx: &Context) -> impl View {
                hsplit(
                    self.dock.binding(),
                    text("dock"),
                    vsplit(self.pane.binding(), text("editor"), text("terminal")),
                )
            }
        }

        let bench = Bench { dock: State::new(200.0), pane: State::new(200.0) };
        let runtime = Runtime::new();
        runtime.render_stable(&bench);
        let result = runtime.layout(&bench, Proposal::exact(Size { width: 800.0, height: 400.0 }));
        let grips: Vec<_> = result
            .hits
            .iter()
            .filter(|(path, _)| path.ends_with("/#split"))
            .cloned()
            .collect();
        assert_eq!(grips.len(), 2, "two seams: {grips:?}");
        // the tall one divides lanes side by side; the wide one, lanes
        // stacked — the geometry names them, not the order
        let centre = |rect: crate::layout::Rect| {
            (rect.origin.x + rect.size.width / 2.0, rect.origin.y + rect.size.height / 2.0)
        };
        let (side_by_side, _) = grips
            .iter()
            .find(|(_, rect)| rect.size.height > rect.size.width)
            .cloned()
            .expect("a seam between lanes side by side stands tall");
        let (stacked, stacked_rect) = grips
            .iter()
            .find(|(_, rect)| rect.size.width > rect.size.height)
            .cloned()
            .expect("a seam between stacked lanes lies flat");
        let _ = side_by_side;

        // nothing under the pointer, nothing to dress
        assert_eq!(runtime.seam_axis(), None);

        // hovering a grip names the way THAT seam travels
        for (path, rect) in &grips {
            let (x, y) = centre(*rect);
            runtime.pointer_moved(x, y, false);
            let want = match rect.size.height > rect.size.width {
                true => Axis::Horizontal,
                false => Axis::Vertical,
            };
            assert_eq!(runtime.seam_axis(), Some(want), "over {path}");
        }

        // and a drag under way keeps it, even while the hand runs
        // ahead of the seam and off every hit in the scene
        let (x, y) = centre(stacked_rect);
        runtime.pointer_pressed(x, y);
        // the drag holds the SPLIT's path; the grip's suffix was the hit
        assert_eq!(
            runtime.interaction().split_drag.as_deref(),
            stacked.strip_suffix("/#split"),
        );
        runtime.pointer_moved(x, y - 90.0, false);
        assert_eq!(runtime.seam_axis(), Some(Axis::Vertical), "the drag keeps the resizer");
        runtime.pointer_released(x, y - 90.0);
        runtime.pointer_moved(5.0, 5.0, false);
        assert_eq!(runtime.seam_axis(), None, "and the release gives it back");
    }

    #[test]
    fn a_fractional_drag_writes_a_share_and_stops_at_a_tenth() {
        use crate::layout::{Fraction, Proposal, Size};

        #[derive(Clone, Copy)]
        struct Panes {
            share: State<Fraction>,
        }

        impl Component for Panes {
            fn body(self, _ctx: &Context) -> impl View {
                hsplit(self.share.binding(), text("left"), text("right"))
            }
        }

        let panes = Panes { share: State::new(Fraction(0.5)) };
        let runtime = Runtime::new();
        runtime.render_stable(&panes);
        let viewport = Proposal::exact(Size { width: 1001.0, height: 700.0 });
        let result = runtime.layout(&panes, viewport);
        let grip = result
            .hits
            .iter()
            .find(|(path, _)| path.ends_with("/#split"))
            .expect("the seam registers a grip")
            .1;
        let grip_center_x = grip.origin.x + grip.size.width / 2.0;

        assert!(runtime.pointer_pressed(grip_center_x, 300.0));
        // the pointer names points; what lands in the binding is a share
        assert!(runtime.pointer_moved(250.0, 300.0, false));
        assert_eq!(panes.share.get(), Fraction(0.25));

        // the floor is RELATIVE — a tenth of the pair, not a number of
        // points, so it holds at every window size
        runtime.pointer_moved(1.0, 300.0, false);
        assert_eq!(panes.share.get(), Fraction(0.1));
        runtime.pointer_moved(2000.0, 300.0, false);
        assert_eq!(panes.share.get(), Fraction(0.9));
        assert_eq!(runtime.pointer_released(2000.0, 300.0), None);
    }

    #[test]
    fn a_fade_covers_the_paint_and_never_the_layout() {
        use crate::layout::{DrawCommand, Proposal, Size};

        #[derive(Clone, Copy)]
        struct Card {
            fade: State<f64>,
        }

        impl Component for Card {
            fn body(self, _ctx: &Context) -> impl View {
                text("half")
                    .foreground_color(Color { r: 10, g: 20, b: 30, a: 200 })
                    .background_color(Color { r: 0, g: 0, b: 0, a: 100 })
                    .opacity(self.fade.get())
            }
        }

        let viewport = Proposal::exact(Size { width: 200.0, height: 60.0 });
        let card = Card { fade: State::new(1.0) };
        let runtime = Runtime::new();
        let read = || {
            runtime.render_stable(&card);
            let result = runtime.layout(&card, viewport);
            let alphas: Vec<u8> = result
                .display
                .iter()
                .filter_map(|command| match command {
                    DrawCommand::FillRect { color, .. }
                    | DrawCommand::TextLine { color, .. } => Some(color.a),
                    _ => None,
                })
                .collect();
            (alphas, result.hits.clone())
        };

        let (solid, solid_hits) = read();
        card.fade.set(0.5);
        let (half, half_hits) = read();
        assert_eq!(solid, vec![100, 200], "untouched, the paint keeps its own alpha");
        assert_eq!(half, vec![50, 100], "the veil multiplies the background AND the ink");
        // the LAW: paint only. a box at half fade lays out — and hits —
        // exactly like a box at none
        assert_eq!(solid_hits, half_hits);
    }

    #[test]
    fn a_mark_lights_by_the_hover_of_its_group() {
        use crate::layout::{DrawCommand, Proposal, Size};

        // the product's tab chip: a slot holds a mark that is a target
        // of its OWN (it closes the tab), and what reveals it is the
        // pointer over the CHIP — never over the mark
        #[derive(Clone, Copy)]
        struct Chip {
            follows: State<bool>,
        }

        impl Component for Chip {
            fn body(self, _ctx: &Context) -> impl View {
                let mark = text("x")
                    .foreground_color(Color { r: 200, g: 200, b: 200, a: 255 })
                    .opacity(0.0)
                    .opacity_hovered(1.0);
                let mark = if self.follows.get() {
                    Either::First(mark.group_hovered())
                } else {
                    Either::Second(mark)
                };
                hstack((text("file.rs"), mark.on_click(|| {})))
                    .on_click(|| {})
                    .hover_group()
            }
        }

        let viewport = Proposal::exact(Size { width: 300.0, height: 40.0 });
        let chip = Chip { follows: State::new(false) };
        let runtime = Runtime::new();
        let mark_alpha = || {
            runtime.render_stable(&chip);
            // the pointer sits over the label, well clear of the mark
            runtime.pointer_moved(6.0, 8.0, false);
            let result = runtime.layout(&chip, viewport);
            result
                .display
                .iter()
                .filter_map(|command| match command {
                    DrawCommand::TextLine { content, color, .. } if &**content == "x" => {
                        Some(color.a)
                    }
                    _ => None,
                })
                .next()
                .expect("the mark paints, faded or not")
        };

        assert_eq!(mark_alpha(), 0, "on its own the mark waits for its OWN hover");
        chip.follows.set(true);
        assert_eq!(mark_alpha(), 255, "following the group, the chip's hover reveals it");

        // and the pointer can REACH it: a group is hovered while the
        // hovered target is the group or anything under it, so the mark
        // does not blink out from under the pointer on its way there
        runtime.render_stable(&chip);
        let result = runtime.layout(&chip, viewport);
        let mark_rect = result
            .hits
            .iter()
            .filter(|(_, rect)| rect.size.width < 100.0)
            .map(|(_, rect)| *rect)
            .next_back()
            .expect("the mark is a target of its own");
        runtime.pointer_moved(mark_rect.origin.x + 2.0, mark_rect.origin.y + 2.0, false);
        let over = runtime.layout(&chip, viewport);
        let alpha = over
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { content, color, .. } if &**content == "x" => Some(color.a),
                _ => None,
            })
            .next()
            .expect("the mark paints");
        assert_eq!(alpha, 255, "the mark stays lit under the pointer that reached it");
    }

    #[test]
    fn a_box_that_declares_its_content_rides_the_region() {
        use crate::layout::{Proposal, Size};

        // the contract of dor 21: on an OPEN axis the box answers the
        // extent of its CONTENT, and the framework's region owns the
        // rest — thumb, wheel, travel and reveal
        #[derive(Clone, Copy)]
        struct Ledger {
            caret: State<f64>,
        }

        struct Surface {
            caret: f64,
        }

        impl CustomElement for Surface {
            fn measure(&self, proposal: Proposal, _metrics: &Metrics) -> Size {
                Size {
                    width: proposal.width.unwrap_or(0.0),
                    // 200 rows of 20pt: four thousand points of content
                    height: proposal.height.unwrap_or(4000.0),
                }
            }

            fn paint(&self, _ctx: &PaintCtx, _painter: &mut Painter) {}

            fn reveal(&self) -> Option<Rect> {
                Some(Rect {
                    origin: Point { x: 0.0, y: self.caret * 20.0 },
                    size: Size { width: 2.0, height: 20.0 },
                })
            }
        }

        impl Component for Ledger {
            fn body(self, _ctx: &Context) -> impl View {
                scroll(custom(Surface { caret: self.caret.get() })).id("region")
            }
        }

        let ledger = Ledger { caret: State::new(0.0) };
        let runtime = Runtime::new();
        runtime.render_stable(&ledger);
        let viewport = Proposal::exact(Size { width: 300.0, height: 200.0 });
        let result = runtime.layout(&ledger, viewport);

        let region = result.scrolls.first().expect("the box sits in a region").clone();
        assert_eq!(region.content.height, 4000.0, "the region sees the CONTENT extent");
        let thumb = result
            .hits
            .iter()
            .find(|(path, _)| path.ends_with("/#thumb-v"))
            .expect("the thumb is a target")
            .1;

        // the wheel travels, because the geometry is honest
        assert!(runtime.wheel(150.0, 100.0, 0.0, -120.0));
        assert!(runtime.scroll_offset(&region.path).y > 0.0);
        runtime.set_scroll_offset(&region.path, Point::ZERO);

        // the thumb travels WITH the hand: the press remembers where
        // inside the band it landed, so nothing jumps on the first move
        let grab_y = thumb.origin.y + 4.0;
        assert!(runtime.pointer_pressed(thumb.origin.x + 2.0, grab_y));
        assert!(runtime.pointer_moved(thumb.origin.x + 2.0, grab_y + 1.0, false));
        let after_one = runtime.scroll_offset(&region.path).y;
        assert!(after_one > 0.0 && after_one < 100.0, "one point of thumb is a small step: {after_one}");
        assert!(runtime.pointer_moved(thumb.origin.x + 2.0, 10_000.0, false));
        assert_eq!(
            runtime.scroll_offset(&region.path).y,
            3800.0,
            "and the far end is the far end"
        );
        assert_eq!(runtime.pointer_released(thumb.origin.x + 2.0, 10_000.0), None);
        // after the release the pointer is free again
        runtime.pointer_moved(150.0, 100.0, false);
        assert_eq!(runtime.scroll_offset(&region.path).y, 3800.0);
    }

    #[test]
    fn a_box_asks_the_region_to_reveal_what_it_holds() {
        use crate::layout::{Proposal, Size};

        #[derive(Clone, Copy)]
        struct Ledger {
            caret: State<f64>,
        }

        struct Surface {
            caret: f64,
        }

        impl CustomElement for Surface {
            fn measure(&self, proposal: Proposal, _metrics: &Metrics) -> Size {
                Size { width: proposal.width.unwrap_or(0.0), height: 4000.0 }
            }

            fn paint(&self, _ctx: &PaintCtx, _painter: &mut Painter) {}

            fn reveal(&self) -> Option<Rect> {
                Some(Rect {
                    origin: Point { x: 0.0, y: self.caret * 20.0 },
                    size: Size { width: 2.0, height: 20.0 },
                })
            }
        }

        impl Component for Ledger {
            fn body(self, _ctx: &Context) -> impl View {
                scroll(custom(Surface { caret: self.caret.get() })).id("region")
            }
        }

        let ledger = Ledger { caret: State::new(0.0) };
        let runtime = Runtime::new();
        runtime.render_stable(&ledger);
        let viewport = Proposal::exact(Size { width: 300.0, height: 200.0 });
        let result = runtime.layout(&ledger, viewport);
        let path = result.scrolls.first().expect("a region").path.clone();
        assert_eq!(runtime.scroll_offset(&path).y, 0.0, "row zero already shows");

        // the caret walks off the bottom: the region travels the
        // SHORTEST way that shows it
        ledger.caret.set(80.0);
        runtime.render_stable(&ledger);
        runtime.layout(&ledger, viewport);
        assert_eq!(runtime.scroll_offset(&path).y, 1420.0, "bottom-aligned, not centred");

        // the hand turns the wheel with the caret where it was: a
        // reveal that did not CHANGE never fights the wheel
        runtime.wheel(150.0, 100.0, 0.0, 120.0);
        let after_wheel = runtime.scroll_offset(&path).y;
        assert!(after_wheel < 1420.0, "the wheel moved: {after_wheel}");
        runtime.render_stable(&ledger);
        runtime.layout(&ledger, viewport);
        assert_eq!(runtime.scroll_offset(&path).y, after_wheel, "and it stayed moved");
    }

    #[test]
    fn the_island_of_a_tall_box_is_only_a_screen() {
        // element mode: the box becomes a canvas, and a box that
        // declared four thousand points of content must not mint a
        // canvas four thousand points tall
        struct Surface;
        impl CustomElement for Surface {
            fn measure(
                &self,
                proposal: crate::layout::Proposal,
                _metrics: &Metrics,
            ) -> Size {
                Size { width: proposal.width.unwrap_or(0.0), height: 4000.0 }
            }
            fn paint(&self, _ctx: &PaintCtx, _painter: &mut Painter) {}
        }

        #[derive(Clone, Copy)]
        struct Ledger;
        impl Component for Ledger {
            fn body(self, _ctx: &Context) -> impl View {
                scroll(custom(Surface))
            }
        }

        let runtime = Runtime::new();
        let patches = runtime.dom_frame(&Ledger, Size { width: 300.0, height: 200.0 });
        let canvas = patches
            .iter()
            .find_map(|patch| match patch {
                crate::dom::DomPatch::Create { id, kind, .. }
                    if *kind == crate::dom::CreateKind::Canvas =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .expect("the box lowers to an island");
        // the flow lowering carries a box in the layout record: the
        // height is PINNED to one screen, and the width belongs to the
        // browser — the island stretches and reports its real box back
        let layout = patches
            .iter()
            .find_map(|patch| match patch {
                crate::dom::DomPatch::SetLayout { id, layout } if *id == canvas => {
                    Some(layout.clone())
                }
                _ => None,
            })
            .expect("the island is sized");
        assert_eq!(
            layout.height,
            Some(200.0),
            "the island is the window, not the content"
        );
    }

    #[test]
    fn a_click_hands_the_platforms_own_count() {
        use crate::layout::{Proposal, Size};
        use std::cell::RefCell;

        #[derive(Clone)]
        struct Row {
            seen: Rc<RefCell<Vec<u8>>>,
        }

        impl Component for Row {
            fn body(self, _ctx: &Context) -> impl View {
                let seen = Rc::clone(&self.seen);
                text("file.rs").on_click_count(move |clicks| seen.borrow_mut().push(clicks))
            }
        }

        let row = Row { seen: Rc::new(RefCell::new(Vec::new())) };
        let runtime = Runtime::new();
        runtime.render_stable(&row);
        let result = runtime.layout(&row, Proposal::exact(Size { width: 200.0, height: 40.0 }));
        let (_, rect) = result.hits.first().expect("the row is a target").clone();
        let (x, y) = (rect.origin.x + 2.0, rect.origin.y + 2.0);

        // a real double click is TWO press/release pairs, and the
        // platform's count climbs across them
        runtime.pointer_clicked(x, y, 1, false);
        runtime.pointer_released(x, y);
        runtime.pointer_clicked(x, y, 2, false);
        runtime.pointer_released(x, y);
        assert_eq!(*row.seen.borrow(), vec![1, 2]);
    }

    #[test]
    fn the_old_click_door_fires_twice_on_a_double() {
        use crate::layout::{Proposal, Size};
        use std::cell::Cell;

        // the semantics an app must be told about: `.on_click` knows
        // nothing of counts, so a double click fires it TWICE
        #[derive(Clone)]
        struct Row {
            plain: Rc<Cell<u32>>,
            doubled: Rc<Cell<u32>>,
        }

        impl Component for Row {
            fn body(self, _ctx: &Context) -> impl View {
                let plain = Rc::clone(&self.plain);
                let doubled = Rc::clone(&self.doubled);
                hstack((
                    text("plain").on_click(move || plain.set(plain.get() + 1)),
                    text("doubled").on_double_click(move || doubled.set(doubled.get() + 1)),
                ))
                .spacing(20.0)
            }
        }

        let row = Row { plain: Rc::new(Cell::new(0)), doubled: Rc::new(Cell::new(0)) };
        let runtime = Runtime::new();
        runtime.render_stable(&row);
        let result = runtime.layout(&row, Proposal::exact(Size { width: 300.0, height: 40.0 }));
        let targets: Vec<_> = result.hits.iter().map(|(_, rect)| *rect).collect();
        let double_click = |rect: crate::layout::Rect| {
            let (x, y) = (rect.origin.x + 2.0, rect.origin.y + 2.0);
            for clicks in [1, 2] {
                runtime.pointer_clicked(x, y, clicks, false);
                runtime.pointer_released(x, y);
            }
        };

        double_click(targets[0]);
        assert_eq!(row.plain.get(), 2, "the plain door hears both presses");
        double_click(targets[1]);
        assert_eq!(row.doubled.get(), 1, "the double door hears only the second");

        // and a TRIPLE does not fire the double door again: 1, 2, 3 and
        // only the 2 lands
        let rect = targets[1];
        let (x, y) = (rect.origin.x + 2.0, rect.origin.y + 2.0);
        for clicks in [1, 2, 3] {
            runtime.pointer_clicked(x, y, clicks, false);
            runtime.pointer_released(x, y);
        }
        assert_eq!(row.doubled.get(), 2, "one more double inside the triple, not two");
    }

    #[test]
    fn a_count_never_leaks_to_the_next_target() {
        use crate::layout::{Proposal, Size};
        use std::cell::RefCell;

        #[derive(Clone)]
        struct Board {
            seen: Rc<RefCell<Vec<(char, u8)>>>,
        }

        impl Component for Board {
            fn body(self, _ctx: &Context) -> impl View {
                let left = Rc::clone(&self.seen);
                let right = Rc::clone(&self.seen);
                hstack((
                    text("left").on_click_count(move |clicks| left.borrow_mut().push(('l', clicks))),
                    text("right")
                        .on_click_count(move |clicks| right.borrow_mut().push(('r', clicks))),
                ))
                .spacing(20.0)
            }
        }

        let board = Board { seen: Rc::new(RefCell::new(Vec::new())) };
        let runtime = Runtime::new();
        runtime.render_stable(&board);
        let result = runtime.layout(&board, Proposal::exact(Size { width: 300.0, height: 40.0 }));
        let targets: Vec<_> = result.hits.iter().map(|(_, rect)| *rect).collect();
        let click = |rect: crate::layout::Rect, clicks: u8| {
            let (x, y) = (rect.origin.x + 2.0, rect.origin.y + 2.0);
            runtime.pointer_clicked(x, y, clicks, false);
            runtime.pointer_released(x, y);
        };

        click(targets[0], 1);
        click(targets[0], 2);
        // the hand moves to the other target and starts over
        click(targets[1], 1);
        assert_eq!(*board.seen.borrow(), vec![('l', 1), ('l', 2), ('r', 1)]);
    }

    #[test]
    fn both_click_doors_print_the_same() {
        // the two doors are ONE registration, and the printed tree must
        // not tell them apart — this is what buys the goldens
        #[derive(Clone, Copy)]
        struct Plain;
        impl Component for Plain {
            fn body(self, _ctx: &Context) -> impl View {
                text("a").on_click(|| {})
            }
        }
        #[derive(Clone, Copy)]
        struct Counted;
        impl Component for Counted {
            fn body(self, _ctx: &Context) -> impl View {
                text("a").on_click_count(|_| {})
            }
        }
        let plain = Runtime::new().render_stable(&Plain).replace("Plain", "X");
        let counted = Runtime::new().render_stable(&Counted).replace("Counted", "X");
        assert_eq!(plain, counted);
        assert!(plain.contains("[.onClick()]"), "and it is the click suffix: {plain}");
    }

    /// The split that makes a TWIN: after it, two live boxes wear the
    /// same name and the projection cannot choose. The hold must not
    /// stay pointing at the box that died — an app that is told `None`
    /// can focus again, and one that is told a ghost cannot even find
    /// out. This is the second of the two answers the port asked for,
    /// and the case the first one declines.
    #[test]
    fn an_ambiguous_name_after_a_split_kills_the_hold_honestly() {
        use crate::layout::{Proposal, Size};
        use std::cell::Cell;

        struct Pane {
            typed: Rc<Cell<usize>>,
        }

        impl CustomElement for Pane {
            fn accepts_keys(&self) -> bool {
                true
            }
            fn paint(&self, _ctx: &PaintCtx, _painter: &mut Painter) {}
            fn event(&self, event: &ElementEvent, _ctx: &EventCtx) -> crate::custom::Response {
                if let ElementEvent::Text(text) = event {
                    self.typed.set(self.typed.get() + text.len());
                    return crate::custom::Response::handled();
                }
                crate::custom::Response::ignored()
            }
        }

        #[derive(Clone)]
        struct Bench {
            split: State<bool>,
            seam: State<f64>,
            typed: Rc<Cell<usize>>,
        }

        impl Component for Bench {
            fn body(self, _ctx: &Context) -> impl View {
                let one = custom(Pane { typed: Rc::clone(&self.typed) }).id("code");
                if self.split.get() {
                    // the split duplicates the pane — and its name with
                    // it, which is what makes the projection ambiguous
                    let two = custom(Pane { typed: Rc::clone(&self.typed) }).id("code");
                    Either::First(hsplit(self.seam.binding(), one, two))
                } else {
                    Either::Second(one)
                }
            }
        }

        let bench = Bench {
            split: State::new(false),
            seam: State::new(300.0),
            typed: Rc::new(Cell::new(0)),
        };
        let runtime = Runtime::new();
        let viewport = Proposal::exact(Size { width: 600.0, height: 400.0 });
        runtime.render_stable(&bench);
        let result = runtime.layout(&bench, viewport);
        let pane = result.customs.first().expect("the box is placed").frame;
        runtime.pointer_pressed(pane.origin.x + 10.0, pane.origin.y + 10.0);
        runtime.pointer_released(pane.origin.x + 10.0, pane.origin.y + 10.0);
        let held = runtime.focused().expect("the click focused the pane");

        bench.split.set(true);
        runtime.render_stable(&bench);
        runtime.layout(&bench, viewport);

        // two twins wear the name, so nobody inherits the keyboard —
        // and the hold does NOT stay on the box that is gone
        assert_eq!(
            runtime.focused(),
            None,
            "an ambiguous name is answered with silence, never with a ghost",
        );
        assert_ne!(runtime.focused().as_deref(), Some(held.as_str()));

        // and the app can act on that: focusing again lands for real
        let split = runtime.layout(&bench, viewport);
        let left = split.customs.first().expect("the panes are placed").frame;
        runtime.pointer_pressed(left.origin.x + 10.0, left.origin.y + 10.0);
        runtime.pointer_released(left.origin.x + 10.0, left.origin.y + 10.0);
        assert!(runtime.focused().is_some(), "a fresh click still focuses");
    }

    /// A modal box declines text while its command mode is on, and the
    /// bare stroke reaches the keymap — the whole of a vim layer is
    /// bare keys, and none of them would arrive otherwise.
    #[test]
    fn a_modal_box_declines_text_and_the_bare_stroke_reaches_the_map() {
        use crate::action::{ActionId, Key, KeyPattern};
        use crate::layout::{Proposal, Size};
        use std::cell::Cell;

        #[derive(Clone, Copy, PartialEq)]
        enum Mode {
            Insert,
            Command,
        }

        struct Modal {
            mode: Rc<Cell<Mode>>,
            typed: Rc<Cell<usize>>,
        }

        impl CustomElement for Modal {
            fn accepts_keys(&self) -> bool {
                true
            }

            // the whole pain: only the box knows that `d` is a command
            // right now and not a letter
            fn takes_text(&self) -> bool {
                self.mode.get() == Mode::Insert
            }

            fn paint(&self, _ctx: &PaintCtx, _painter: &mut Painter) {}

            fn event(&self, event: &ElementEvent, _ctx: &EventCtx) -> crate::custom::Response {
                if let ElementEvent::Text(text) = event {
                    self.typed.set(self.typed.get() + text.len());
                    return crate::custom::Response::handled();
                }
                // a stroke the box does not want goes back to the scene
                crate::custom::Response::ignored()
            }
        }

        const DELETE_LINE: ActionId = ActionId("vim.delete_line");

        #[derive(Clone)]
        struct Bench {
            mode: Rc<Cell<Mode>>,
            typed: Rc<Cell<usize>>,
            deleted: Rc<Cell<usize>>,
        }

        impl Component for Bench {
            fn body(self, _ctx: &Context) -> impl View {
                let deleted = Rc::clone(&self.deleted);
                custom(Modal { mode: Rc::clone(&self.mode), typed: Rc::clone(&self.typed) })
                    .id("editor")
                    .on_action(DELETE_LINE, move || deleted.set(deleted.get() + 1))
            }
        }

        let bench = Bench {
            mode: Rc::new(Cell::new(Mode::Insert)),
            typed: Rc::new(Cell::new(0)),
            deleted: Rc::new(Cell::new(0)),
        };
        let deleted = Rc::clone(&bench.deleted);
        let runtime = Runtime::new();
        runtime.bind(KeyPattern::key(Key::Char('d')), DELETE_LINE);

        let viewport = Proposal::exact(Size { width: 400.0, height: 200.0 });
        runtime.render_stable(&bench);
        let result = runtime.layout(&bench, viewport);
        let box_rect = result.customs.first().expect("the box is placed").frame;
        runtime.pointer_pressed(box_rect.origin.x + 5.0, box_rect.origin.y + 5.0);
        runtime.pointer_released(box_rect.origin.x + 5.0, box_rect.origin.y + 5.0);
        assert!(runtime.focused().is_some(), "the click focused the box");

        // INSERT: the box is typing, so the gate hands `d` to it and
        // the binding never hears the stroke
        assert!(runtime.focus_takes_text(), "an inserting box types");

        // COMMAND: the same box, the same focus, the same key — and now
        // the gate steps aside
        bench.mode.set(Mode::Command);
        assert!(!runtime.focus_takes_text(), "a commanding box declines text");
        // the shells' gate is `focus_takes_text() && is_text_input()`;
        // with the box declining, the stroke walks the ordinary road
        let pattern = KeyPattern::key(Key::Char('d'));
        assert!(pattern.is_text_input(), "`d` is a bare character");
        assert!(
            !runtime.key_stroke(&pattern).handled,
            "the box ignores it, so the keymap gets its turn",
        );
        let action = match runtime.chord(&pattern) {
            crate::action::KeyMatch::Action(action) => action,
            other => panic!("the map answers the bare key, got {other:?}"),
        };
        assert!(runtime.dispatch_action(action), "and the binding runs");
        assert_eq!(deleted.get(), 1);
        assert_eq!(bench.typed.get(), 0, "nothing was typed in command mode");

        // nothing focused is nobody typing — the gate never fires on an
        // empty keyboard
        runtime.blur();
        assert!(!runtime.focus_takes_text());
    }

    /// The keyboard follows the NAME, not the position: wrapping a
    /// pane in a split shifts every positional segment below it, and
    /// the focused box must keep typing across the edit.
    #[test]
    fn the_keyboard_survives_a_split_above_it() {
        use crate::layout::{Proposal, Size};
        use std::cell::Cell;

        struct Editor {
            typed: Rc<Cell<usize>>,
        }

        impl CustomElement for Editor {
            fn accepts_keys(&self) -> bool {
                true
            }
            fn paint(&self, _ctx: &PaintCtx, _painter: &mut Painter) {}
            fn event(&self, event: &ElementEvent, _ctx: &EventCtx) -> crate::custom::Response {
                if let ElementEvent::Text(text) = event {
                    self.typed.set(self.typed.get() + text.len());
                    return crate::custom::Response::handled();
                }
                crate::custom::Response::ignored()
            }
        }

        #[derive(Clone)]
        struct Bench {
            split: State<bool>,
            seam: State<f64>,
            typed: Rc<Cell<usize>>,
        }

        impl Component for Bench {
            fn body(self, _ctx: &Context) -> impl View {
                let typed = Rc::clone(&self.typed);
                let pane = custom(Editor { typed }).id("code");
                if self.split.get() {
                    Either::First(hsplit(self.seam.binding(), pane, text("other")))
                } else {
                    Either::Second(pane)
                }
            }
        }

        let bench = Bench {
            split: State::new(false),
            seam: State::new(200.0),
            typed: Rc::new(Cell::new(0)),
        };
        let runtime = Runtime::new();
        let viewport = Proposal::exact(Size { width: 600.0, height: 400.0 });
        runtime.render_stable(&bench);
        let result = runtime.layout(&bench, viewport);
        let box_rect = result.customs.first().expect("the box is placed").frame;
        runtime.pointer_pressed(box_rect.origin.x + 10.0, box_rect.origin.y + 10.0);
        runtime.pointer_released(box_rect.origin.x + 10.0, box_rect.origin.y + 10.0);
        assert!(runtime.focused().is_some(), "the click focused the box");
        assert!(runtime.key(EditCommand::Insert("a".into())).applied, "and it types");

        // the tree edit ABOVE: the pane gets wrapped in a split
        bench.split.set(true);
        runtime.render_stable(&bench);
        runtime.layout(&bench, viewport);
        assert!(
            runtime.key(EditCommand::Insert("b".into())).applied,
            "PAIN 23: the keyboard survives a tree edit above"
        );
    }

    #[test]
    fn a_field_keeps_its_caret_and_its_name_through_a_split_above() {
        use crate::layout::{Proposal, Size};

        // the other half of PAIN 23: the port's editor is a box, but a
        // FIELD is the same promise — and the reported symptom was not
        // only that typing stopped, it was that `focused()` went on
        // answering an address that no longer existed
        #[derive(Clone, Copy)]
        struct Bench {
            split: State<bool>,
            seam: State<f64>,
            name: State<String>,
        }

        impl Component for Bench {
            fn body(self, _ctx: &Context) -> impl View {
                let field = text_field("name", self.name.binding()).id("subject");
                if self.split.get() {
                    Either::First(hsplit(self.seam.binding(), field, text("other")))
                } else {
                    Either::Second(field)
                }
            }
        }

        let bench = Bench {
            split: State::new(false),
            seam: State::new(200.0),
            name: State::new(String::new()),
        };
        let runtime = Runtime::new();
        let viewport = Proposal::exact(Size { width: 600.0, height: 400.0 });
        runtime.render_stable(&bench);
        let result = runtime.layout(&bench, viewport);
        let (path, rect) = result.hits.last().expect("the field is a target").clone();
        runtime.pointer_clicked(rect.origin.x + 8.0, rect.origin.y + 8.0, 1, false);
        runtime.pointer_released(rect.origin.x + 8.0, rect.origin.y + 8.0);
        assert_eq!(runtime.focused(), Some(path.clone()));
        assert!(runtime.key(EditCommand::Insert("deco".into())).applied);
        assert!(runtime.key(EditCommand::Left(false)).applied);
        assert!(runtime.key(EditCommand::Left(false)).applied);

        // the tree edit ABOVE: every positional segment below it shifts
        bench.split.set(true);
        runtime.render_stable(&bench);
        runtime.layout(&bench, viewport);

        let moved = runtime.focused().expect("the keyboard stayed somewhere");
        assert_ne!(moved, path, "the address moved with the tree");
        assert!(
            runtime.key(EditCommand::Insert("!".into())).applied,
            "PAIN 23: the field types through a split above it",
        );
        // and the CARET went with it — the keyboard surviving in the
        // wrong place would be its own bug
        assert_eq!(bench.name.get(), "de!co", "two lefts, then the edit, all held");

        // the honest half: an input that truly dies leaves NO stale
        // address behind — the app can see it went
        #[derive(Clone, Copy)]
        struct Gone {
            here: State<bool>,
            name: State<String>,
        }

        impl Component for Gone {
            fn body(self, _ctx: &Context) -> impl View {
                self.here.get().then(|| text_field("name", self.name.binding()).id("only"))
            }
        }

        let gone = Gone { here: State::new(true), name: State::new(String::new()) };
        runtime.render_stable(&gone);
        let result = runtime.layout(&gone, viewport);
        let rect = result.hits.last().expect("the field is a target").1;
        runtime.pointer_clicked(rect.origin.x + 8.0, rect.origin.y + 8.0, 1, false);
        runtime.pointer_released(rect.origin.x + 8.0, rect.origin.y + 8.0);
        assert!(runtime.focused().is_some());
        gone.here.set(false);
        runtime.render_stable(&gone);
        runtime.layout(&gone, viewport);
        assert_eq!(runtime.focused(), None, "a dead field answers None, never its old path");
    }

    #[test]
    fn a_moved_box_never_hears_that_it_lost_the_keyboard() {
        use crate::layout::{Proposal, Size};
        use std::cell::RefCell;

        // the re-point must be SILENT: as far as the box knows it never
        // lost the keyboard, which is the truth
        struct Editor {
            log: Rc<RefCell<Vec<bool>>>,
        }

        impl CustomElement for Editor {
            fn accepts_keys(&self) -> bool {
                true
            }
            fn paint(&self, _ctx: &PaintCtx, _painter: &mut Painter) {}
            fn event(&self, event: &ElementEvent, _ctx: &EventCtx) -> crate::custom::Response {
                if let ElementEvent::Focused(on) = event {
                    self.log.borrow_mut().push(*on);
                }
                crate::custom::Response::handled()
            }
        }

        #[derive(Clone)]
        struct Bench {
            split: State<bool>,
            seam: State<f64>,
            log: Rc<RefCell<Vec<bool>>>,
        }

        impl Component for Bench {
            fn body(self, _ctx: &Context) -> impl View {
                let log = Rc::clone(&self.log);
                let pane = custom(Editor { log }).id("code");
                if self.split.get() {
                    Either::First(hsplit(self.seam.binding(), pane, text("other")))
                } else {
                    Either::Second(pane)
                }
            }
        }

        let bench = Bench {
            split: State::new(false),
            seam: State::new(200.0),
            log: Rc::new(RefCell::new(Vec::new())),
        };
        let runtime = Runtime::new();
        let viewport = Proposal::exact(Size { width: 600.0, height: 400.0 });
        runtime.render_stable(&bench);
        let result = runtime.layout(&bench, viewport);
        let rect = result.customs.first().expect("the box is placed").frame;
        runtime.pointer_pressed(rect.origin.x + 10.0, rect.origin.y + 10.0);
        runtime.pointer_released(rect.origin.x + 10.0, rect.origin.y + 10.0);
        assert_eq!(*bench.log.borrow(), vec![true], "one focus, on the click");

        bench.split.set(true);
        runtime.render_stable(&bench);
        runtime.layout(&bench, viewport);
        assert_eq!(*bench.log.borrow(), vec![true], "and nothing at all on the split");
    }

    #[test]
    fn a_name_that_two_boxes_share_hands_the_keyboard_to_neither() {
        use crate::layout::{Proposal, Size};

        // an ambiguous name must never hand the keyboard over on a
        // guess: the honest answer is that the focus died
        struct Editor;
        impl CustomElement for Editor {
            fn accepts_keys(&self) -> bool {
                true
            }
            fn paint(&self, _ctx: &PaintCtx, _painter: &mut Painter) {}
        }

        #[derive(Clone, Copy)]
        struct Bench {
            twin: State<bool>,
            seam: State<f64>,
        }

        impl Component for Bench {
            fn body(self, _ctx: &Context) -> impl View {
                let one = custom(Editor).id("code");
                if self.twin.get() {
                    // the edit above AND a second box wearing the name
                    Either::First(hsplit(self.seam.binding(), one, custom(Editor).id("code")))
                } else {
                    Either::Second(one)
                }
            }
        }

        let bench = Bench { twin: State::new(false), seam: State::new(200.0) };
        let runtime = Runtime::new();
        let viewport = Proposal::exact(Size { width: 600.0, height: 400.0 });
        runtime.render_stable(&bench);
        let result = runtime.layout(&bench, viewport);
        let rect = result.customs.first().expect("a box").frame;
        runtime.pointer_pressed(rect.origin.x + 10.0, rect.origin.y + 10.0);
        runtime.pointer_released(rect.origin.x + 10.0, rect.origin.y + 10.0);
        assert!(runtime.focused().is_some());

        bench.twin.set(true);
        runtime.render_stable(&bench);
        runtime.layout(&bench, viewport);
        assert_eq!(runtime.focused(), None, "two names, no winner");
    }

    #[test]
    fn a_box_with_no_name_of_its_own_still_dies_honestly() {
        use crate::layout::{Proposal, Size};

        struct Editor;
        impl CustomElement for Editor {
            fn accepts_keys(&self) -> bool {
                true
            }
            fn paint(&self, _ctx: &PaintCtx, _painter: &mut Painter) {}
        }

        #[derive(Clone, Copy)]
        struct Bench {
            split: State<bool>,
            seam: State<f64>,
        }

        impl Component for Bench {
            fn body(self, _ctx: &Context) -> impl View {
                if self.split.get() {
                    Either::First(hsplit(self.seam.binding(), custom(Editor), text("other")))
                } else {
                    Either::Second(custom(Editor))
                }
            }
        }

        let bench = Bench { split: State::new(false), seam: State::new(200.0) };
        let runtime = Runtime::new();
        let viewport = Proposal::exact(Size { width: 600.0, height: 400.0 });
        runtime.render_stable(&bench);
        let result = runtime.layout(&bench, viewport);
        let rect = result.customs.first().expect("a box").frame;
        runtime.pointer_pressed(rect.origin.x + 10.0, rect.origin.y + 10.0);
        runtime.pointer_released(rect.origin.x + 10.0, rect.origin.y + 10.0);
        assert!(runtime.focused().is_some());

        // no name, nothing to follow — the keyboard goes, honestly
        bench.split.set(true);
        runtime.render_stable(&bench);
        runtime.layout(&bench, viewport);
        assert_eq!(runtime.focused(), None);
    }

    #[test]
    fn a_named_field_takes_its_caret_with_it() {
        use crate::layout::{Proposal, Size};

        // the caret column is the FIELD's memory, and it is keyed by
        // path — so it has to move house with the field
        #[derive(Clone, Copy)]
        struct Bench {
            split: State<bool>,
            seam: State<f64>,
            query: State<String>,
        }

        impl Component for Bench {
            fn body(self, _ctx: &Context) -> impl View {
                let field = text_field("search", self.query.binding()).id("query");
                if self.split.get() {
                    Either::First(hsplit(self.seam.binding(), field, text("other")))
                } else {
                    Either::Second(field)
                }
            }
        }

        let bench = Bench {
            split: State::new(false),
            seam: State::new(200.0),
            query: State::new("hello".to_string()),
        };
        let runtime = Runtime::new();
        let viewport = Proposal::exact(Size { width: 600.0, height: 400.0 });
        runtime.render_stable(&bench);
        let result = runtime.layout(&bench, viewport);
        let field = result.fields.first().expect("the field is placed").clone();
        runtime.focus(&field.path);
        runtime.key(EditCommand::Insert("!".into()));
        assert_eq!(bench.query.get(), "hello!");

        // the tree above changes shape: the field keeps the keyboard
        // AND the column, so the next keystroke lands where the last
        // one did instead of at the start
        bench.split.set(true);
        runtime.render_stable(&bench);
        runtime.layout(&bench, viewport);
        assert!(runtime.focused().is_some(), "the keyboard followed the name");
        runtime.key(EditCommand::Insert("?".into()));
        assert_eq!(bench.query.get(), "hello!?", "and so did the caret");
    }

    #[test]
    fn a_layer_takes_the_box_and_gives_it_nothing() {
        use crate::layout::{DrawCommand, Proposal, Size, UnitPoint};

        // the pain itself: a rule wide enough to CROSS a chip must not
        // make the chip flexible, or every tab eats half the strip
        #[derive(Clone, Copy)]
        struct Strip {
            ruled: State<bool>,
        }

        impl Component for Strip {
            fn body(self, _ctx: &Context) -> impl View {
                let chip = text("file.rs").padding_length(8.0);
                let one = if self.ruled.get() {
                    Either::First(chip.overlay(
                        UnitPoint::BOTTOM,
                        spacer().frame_height(2.0).background_color(Color::hex(0x3B82F6)),
                    ))
                } else {
                    Either::Second(chip)
                };
                hstack((one, text("other"), spacer())).spacing(4.0)
            }
        }

        let strip = Strip { ruled: State::new(false) };
        let runtime = Runtime::new();
        let viewport = Proposal::exact(Size { width: 400.0, height: 40.0 });
        // where the SIBLING starts is where the chip ends
        let sibling_x = |ruled: bool| {
            strip.ruled.set(ruled);
            runtime.render_stable(&strip);
            runtime
                .layout(&strip, viewport)
                .display
                .iter()
                .find_map(|command| match command {
                    DrawCommand::TextLine { content, origin, .. } if &**content == "other" => {
                        Some(origin.x)
                    }
                    _ => None,
                })
                .expect("the sibling is drawn")
        };

        let plain = sibling_x(false);
        let ruled = sibling_x(true);
        assert_eq!(ruled, plain, "the rule never moved the sibling: the chip still hugs");
        assert!(plain < 100.0, "and the chip really is hugging: {plain}");
    }

    #[test]
    fn a_layer_hangs_where_the_unit_point_says() {
        use crate::layout::{DrawCommand, Proposal, Size, UnitPoint};

        // ONE runtime: two of them on a thread share the retention, so
        // the second body would never run and the badge would never move
        #[derive(Clone, Copy)]
        struct Card {
            at: State<UnitPoint>,
        }

        impl Component for Card {
            fn body(self, _ctx: &Context) -> impl View {
                rectangle()
                    .frame(200.0, 100.0)
                    .background_color(Color::hex(0x111111))
                    .overlay(
                        self.at.get(),
                        spacer().frame(20.0, 10.0).background_color(Color::hex(0xFF0000)),
                    )
            }
        }

        let card = Card { at: State::new(UnitPoint::TOP_LEADING) };
        let runtime = Runtime::new();
        let viewport = Proposal::exact(Size { width: 200.0, height: 100.0 });
        let badge = |at: UnitPoint| {
            card.at.set(at);
            runtime.render_stable(&card);
            runtime
                .layout(&card, viewport)
                .display
                .iter()
                .find_map(|command| match command {
                    DrawCommand::FillRect { rect, color, .. }
                        if color.r == 255 && color.g == 0 =>
                    {
                        Some(*rect)
                    }
                    _ => None,
                })
                .expect("the badge paints")
        };

        assert_eq!(badge(UnitPoint::TOP_LEADING).origin, Point { x: 0.0, y: 0.0 });
        assert_eq!(
            badge(UnitPoint::BOTTOM_TRAILING).origin,
            Point { x: 180.0, y: 90.0 },
            "the badge's corner meets the box's corner"
        );
        assert_eq!(badge(UnitPoint::CENTER).origin, Point { x: 90.0, y: 45.0 });
        assert_eq!(badge(UnitPoint::BOTTOM).origin, Point { x: 90.0, y: 90.0 });
    }

    #[test]
    fn a_layer_that_fills_an_axis_crosses_the_whole_box() {
        use crate::layout::{DrawCommand, Proposal, Size, UnitPoint};

        // the 2pt bar of the active tab: it must cross the box the
        // PARENT finally handed it, not the one measure guessed
        #[derive(Clone, Copy)]
        struct Bar;
        impl Component for Bar {
            fn body(self, _ctx: &Context) -> impl View {
                text("tab")
                    .frame(300.0, 40.0)
                    .overlay(
                        UnitPoint::BOTTOM,
                        spacer().frame_height(2.0).background_color(Color::hex(0x00FF00)),
                    )
            }
        }

        let runtime = Runtime::new();
        runtime.render_stable(&Bar);
        let result = runtime.layout(&Bar, Proposal::exact(Size { width: 400.0, height: 60.0 }));
        let rule = result
            .display
            .iter()
            .find_map(|command| match command {
                DrawCommand::FillRect { rect, color, .. } if color.g == 255 => Some(*rect),
                _ => None,
            })
            .expect("the rule paints");
        assert_eq!(rule.size, Size { width: 300.0, height: 2.0 });
        assert_eq!(rule.origin.y, 38.0, "and it hangs on the bottom edge");
    }

    #[test]
    fn a_layer_paints_over_and_a_background_paints_under() {
        use crate::layout::{DrawCommand, Proposal, Size, UnitPoint};

        #[derive(Clone, Copy)]
        struct Both;
        impl Component for Both {
            fn body(self, _ctx: &Context) -> impl View {
                text("middle")
                    .background(
                        UnitPoint::CENTER,
                        spacer().background_color(Color::hex(0x0000FF)),
                    )
                    .overlay(
                        UnitPoint::CENTER,
                        spacer().background_color(Color::hex(0xFF0000)),
                    )
            }
        }

        let runtime = Runtime::new();
        runtime.render_stable(&Both);
        let result =
            runtime.layout(&Both, Proposal::exact(Size { width: 200.0, height: 40.0 }));
        let index = |red: bool| {
            result
                .display
                .iter()
                .position(|command| match command {
                    DrawCommand::FillRect { color, .. } => {
                        if red { color.r == 255 } else { color.b == 255 }
                    }
                    _ => false,
                })
                .expect("both layers paint")
        };
        let label = result
            .display
            .iter()
            .position(|command| matches!(command, DrawCommand::TextLine { .. }))
            .expect("the label paints");
        assert!(index(false) < label, "the background is under the box");
        assert!(index(true) > label, "and the overlay is over it");
    }

    #[test]
    fn in_element_mode_a_decorative_layer_lets_the_click_through() {
        use crate::layout::UnitPoint;

        // the browser routes by ELEMENT there, so a rule covering a row
        // would eat the row's click unless it says it must not
        #[derive(Clone, Copy)]
        struct Row;
        impl Component for Row {
            fn body(self, _ctx: &Context) -> impl View {
                text("row")
                    .background_color(Color::hex(0x202020))
                    .on_click(|| {})
                    .overlay(
                        UnitPoint::BOTTOM,
                        spacer().frame_height(2.0).background_color(Color::hex(0x3B82F6)),
                    )
            }
        }

        let runtime = Runtime::new();
        let patches = runtime.dom_frame(&Row, Size { width: 200.0, height: 40.0 });
        let styles: Vec<crate::dom::DomStyle> = patches
            .iter()
            .filter_map(|patch| match patch {
                crate::dom::DomPatch::SetStyle { style, .. } => Some(style.clone()),
                _ => None,
            })
            .collect();
        assert!(
            styles.iter().any(|style| style.pass_through),
            "the rule lets the pointer through: {styles:#?}"
        );
        assert!(
            styles.iter().any(|style| style.interactive.is_some() && !style.pass_through),
            "and the row keeps its own click"
        );
    }

    #[test]
    fn a_layer_never_hides_a_rewrite_from_the_base() {
        use crate::layout::UnitPoint;

        // modifier order stays irrelevant: `.scroll_target` descends to
        // the Scroll node through the wrappers, and the overlay is one
        #[derive(Clone, Copy)]
        struct Panel;
        impl Component for Panel {
            fn body(self, _ctx: &Context) -> impl View {
                let rows: Vec<usize> = (0..30).collect();
                scroll(for_each(
                    rows,
                    |row| format!("row{row}"),
                    |row| text(format!("row {row}")).frame_height(20.0),
                ))
                    .overlay(
                        UnitPoint::TOP,
                        spacer().frame_height(2.0).background_color(Color::hex(0x3B82F6)),
                    )
                    .scroll_target("row20")
                    .id("panel")
            }
        }

        let runtime = Runtime::new();
        runtime.render_stable(&Panel);
        let result = runtime.layout(
            &Panel,
            crate::layout::Proposal::exact(Size { width: 200.0, height: 100.0 }),
        );
        let region = result.scrolls.first().expect("the region survived the layer");
        assert_eq!(
            region.target.as_deref(),
            Some("row20"),
            "the rewrite reached the Scroll through the overlay"
        );
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

        assert!(runtime.pointer_moved(cx, cy, false), "entering the target changes the state");
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
        assert!(!runtime.pointer_moved(cx + 1.0, cy, false));
    }

    #[test]
    fn the_ink_answers_to_hover_and_press() {
        use crate::layout::{DrawCommand, LayoutResult, Proposal, Size};

        const FAINT: Color = Color::hex(0x8A8A8A);
        const BRIGHT: Color = Color::hex(0xF5F5F5);
        const SUNK: Color = Color::hex(0x3B82F6);

        #[derive(Clone, Copy)]
        struct CloseGlyph;

        impl Component for CloseGlyph {
            fn body(self, _ctx: &Context) -> impl View {
                // the tab's ✕: faint until the pointer arrives
                text("x")
                    .foreground_color(FAINT)
                    .foreground_hovered(BRIGHT)
                    .foreground_pressed(SUNK)
                    .padding_length(4.0)
                    .on_click(|| {})
            }
        }

        let runtime = Runtime::new();
        let viewport = Proposal::exact(Size { width: 60.0, height: 30.0 });
        let ink = |result: &LayoutResult| {
            result
                .display
                .iter()
                .find_map(|command| match command {
                    DrawCommand::TextLine { color, .. } => Some(*color),
                    _ => None,
                })
                .expect("the glyph paints")
        };

        let cold = runtime.layout(&CloseGlyph, viewport);
        assert_eq!(ink(&cold), FAINT, "at rest the glyph stays faint");
        let (_, target) = cold.hits.last().expect("the glyph is a target").clone();
        let cx = target.origin.x + target.size.width / 2.0;
        let cy = target.origin.y + target.size.height / 2.0;

        runtime.pointer_moved(cx, cy, false);
        let hot = runtime.layout(&CloseGlyph, viewport);
        assert_eq!(ink(&hot), BRIGHT, "the pointer brightens it");
        // the LAW: ink is paint — no frame moves under the pointer
        for (path, frame) in cold.frames.iter() {
            assert_eq!(Some(frame), hot.frames.get(path));
        }

        runtime.pointer_pressed(cx, cy);
        assert_eq!(ink(&runtime.layout(&CloseGlyph, viewport)), SUNK, "press sinks it");
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
        runtime.pointer_moved(199.0, 99.0, false);
        assert!(
            runtime.interaction().hovered.is_none(),
            "dragging out releases the visual"
        );
        runtime.pointer_moved(cx, cy, false);
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

        runtime.pointer_moved(cx, cy, false);
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

        let fills: Vec<(Color, crate::layout::Corners)> = result
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
            vec![(Color::hex(0x123456), crate::layout::Corners::all(5.0))],
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
                .corner_radius(6.0)
                .clipped(),
        );
        assert!(printed.contains("[.background(#AA69FB)]"), "{printed}");
        assert!(printed.contains("[.foregroundColor(#070510)]"), "{printed}");
        assert!(printed.contains("[.border(#2A1B3F, width: 1)]"), "{printed}");
        assert!(printed.contains("[.cornerRadius(6)]"), "{printed}");
        assert!(printed.contains("[.clipped()]"), "{printed}");
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
                    assert_eq!(*corner_radius, crate::layout::Corners::all(6.0));
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
    fn the_field_cuts_its_text_and_the_run_follows_the_caret() {
        use crate::layout::{DrawCommand, Point, Proposal, Size};
        use crate::text_input::EditCommand;

        #[derive(Clone, Copy)]
        struct Form {
            name: State<String>,
        }

        impl Component for Form {
            fn body(self, _ctx: &Context) -> impl View {
                text_field("name", self.name.binding()).frame_width(120.0)
            }
        }

        // 120 wide, 8 of padding each side: thirteen cells of run
        let form = Form { name: State::new(String::new()) };
        let runtime = Runtime::new();
        runtime.render_stable(&form);
        let viewport = Proposal::exact(Size { width: 240.0, height: 60.0 });
        let result = runtime.layout(&form, viewport);

        // the box cuts what it holds: a clip with the field's own
        // corner, around the field's own frame
        let (field_path, frame) = result.hits.last().expect("the field is a target").clone();
        let clip = result
            .display
            .iter()
            .find_map(|command| match command {
                DrawCommand::PushClip { rect, corner_radius } => Some((*rect, *corner_radius)),
                _ => None,
            })
            .expect("the field pushes a clip");
        assert_eq!(clip.0, frame, "the cut IS the box");
        assert_eq!(clip.1, crate::layout::Corners::all(5.0), "and it follows the corner");

        // the border is drawn OUTSIDE the cut — a clipped stroke would
        // eat its own outer half
        let order: Vec<&str> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::PushClip { .. } => Some("push"),
                DrawCommand::PopClip => Some("pop"),
                DrawCommand::StrokeRect { .. } => Some("border"),
                _ => None,
            })
            .collect();
        assert_eq!(order, ["push", "pop", "border"]);

        let text_x = |result: &crate::layout::LayoutResult| {
            result
                .display
                .iter()
                .find_map(|command| match command {
                    DrawCommand::TextLine { origin, .. } => Some(origin.x),
                    _ => None,
                })
                .expect("the field paints its run")
        };
        let home = text_x(&result);

        // a string that fits leaves the run at home
        runtime.pointer_pressed(frame.origin.x + 4.0, frame.origin.y + 4.0);
        runtime.pointer_released(frame.origin.x + 4.0, frame.origin.y + 4.0);
        assert_eq!(runtime.focused(), Some(field_path.clone()));
        assert!(runtime.key(EditCommand::Insert("short".into())).applied);
        assert_eq!(text_x(&runtime.layout(&form, viewport)), home, "five cells fit");

        // past the right edge the run walks left, and the caret stays
        // inside the box that holds it
        assert!(runtime.key(EditCommand::Insert(" and then some more".into())).applied);
        let scrolled = runtime.layout(&form, viewport);
        let offset = runtime.scroll_offset(&field_path).x;
        assert!(offset > 0.0, "the run moved: {offset}");
        assert_eq!(text_x(&scrolled), home - offset);
        let caret_x = scrolled
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::FillRect { rect, corner_radius, .. }
                    if rect.size.width < 2.0 && !corner_radius.is_zero() =>
                {
                    Some(rect.origin.x)
                }
                _ => None,
            })
            .last()
            .expect("a focused field paints its caret");
        assert!(
            caret_x >= frame.origin.x && caret_x <= frame.origin.x + frame.size.width,
            "the caret stayed in sight at {caret_x}",
        );

        // Home walks the caret back and the run comes home with it
        assert!(runtime.key(EditCommand::Home(false)).applied);
        assert_eq!(runtime.scroll_offset(&field_path).x, 0.0);
        assert_eq!(text_x(&runtime.layout(&form, viewport)), home);

        // and a run scrolled to the end never leaves a gap when the
        // text shrinks under it: the clamp is the placement's, so it
        // holds even for an offset written against an older string
        assert!(runtime.key(EditCommand::End(false)).applied);
        let end = runtime.scroll_offset(&field_path).x;
        assert!(end > 0.0);
        runtime.set_scroll_offset(&field_path, Point { x: end + 500.0, y: 0.0 });
        let clamped = runtime.layout(&form, viewport);
        assert_eq!(text_x(&clamped), home - end, "the clamp is the box's, not the caret's");
    }

    #[test]
    fn the_mouse_sweeps_a_selection_and_the_count_picks_the_unit() {
        use crate::layout::{DrawCommand, Proposal, Size};
        use crate::text_input::EditCommand;

        #[derive(Clone, Copy)]
        struct Form {
            note: State<String>,
        }

        impl Component for Form {
            fn body(self, _ctx: &Context) -> impl View {
                text_field("note", self.note.binding()).frame_width(200.0)
            }
        }

        // eight points per cell in the test font: byte i sits at 8i
        let form = Form { note: State::new("hello world".to_string()) };
        let runtime = Runtime::new();
        runtime.render_stable(&form);
        let viewport = Proposal::exact(Size { width: 240.0, height: 60.0 });
        let result = runtime.layout(&form, viewport);
        let (path, frame) = result.hits.last().expect("the field is a target").clone();
        let run_x = frame.origin.x + 8.0;
        let y = frame.origin.y + frame.size.height / 2.0;
        let at = |byte: usize| run_x + 8.0 * byte as f64;
        let selected = || {
            let snapshot = runtime.ime_snapshot().expect("a focused field answers the platform");
            snapshot.selected
        };

        // the press focuses and puts the caret under the pointer — a
        // field acts on the DOWN, and there is nothing selected yet
        assert!(runtime.pointer_clicked(at(2), y, 1, false));
        assert_eq!(runtime.focused(), Some(path.clone()));
        assert_eq!(selected(), (2, 0), "the anchor is armed, the selection empty");

        // the hand sweeps: the anchor holds and the caret walks
        assert!(runtime.pointer_moved(at(9), y, false));
        assert_eq!(selected(), (2, 7));
        // backwards past the anchor still reads as one range
        assert!(runtime.pointer_moved(at(0), y, false));
        assert_eq!(selected(), (0, 2));
        assert!(runtime.pointer_moved(at(9), y, false));

        // the release changes nothing — and it still names the field
        assert_eq!(runtime.pointer_released(at(9), y), Some(path.clone()));
        assert_eq!(selected(), (2, 7), "what the sweep took, it keeps");
        assert_eq!(runtime.focused(), Some(path.clone()), "and the keyboard stayed");

        // the selection is PAINTED, not just held
        let painted = runtime.layout(&form, viewport);
        assert!(painted.display.iter().any(|command| matches!(
            command,
            DrawCommand::FillRect { rect, color, .. }
                if *color == Color::SELECTION && rect.size.width == 56.0
        )));

        // a move with the button up sweeps nothing
        assert_eq!(selected(), (2, 7));
        runtime.pointer_moved(at(1), y, false);
        assert_eq!(selected(), (2, 7), "the sweep ended with the button");

        // two clicks take the word under the pointer, three take the line
        runtime.pointer_clicked(at(8), y, 2, false);
        runtime.pointer_released(at(8), y);
        assert_eq!(selected(), (6, 5), "the word `world`");
        runtime.pointer_clicked(at(8), y, 3, false);
        runtime.pointer_released(at(8), y);
        assert_eq!(selected(), (0, 11), "and a one-line field IS the line");

        // shift+click extends from where the selection stood instead of
        // dropping a fresh caret
        runtime.pointer_clicked(at(2), y, 1, false);
        runtime.pointer_released(at(2), y);
        runtime.pointer_clicked(at(5), y, 1, true);
        runtime.pointer_released(at(5), y);
        assert_eq!(selected(), (2, 3), "shift kept the anchor at 2");

        // typing replaces what the mouse took — the two halves are one
        // caret, not two
        assert!(runtime.key(EditCommand::Insert("HEY".into())).applied);
        assert!(runtime.render_stable(&form).contains("heHEY world"));

        // and a sweep that leaves the box rolls the run under it
        assert!(runtime.key(EditCommand::SelectAll).applied);
        const LONG: &str = "a string far longer than the box can hold";
        assert!(runtime.key(EditCommand::Insert(LONG.into())).applied);
        // typing at the tail left the run scrolled; Home brings it back
        assert!(runtime.key(EditCommand::Home(false)).applied);
        runtime.layout(&form, viewport);
        assert_eq!(runtime.scroll_offset(&path).x, 0.0);
        runtime.pointer_clicked(frame.origin.x + 12.0, y, 1, false);
        assert_eq!(runtime.scroll_offset(&path).x, 0.0, "the press is inside the box");
        let out = frame.origin.x + frame.size.width + 400.0;
        assert!(runtime.pointer_moved(out, y, false));
        assert!(
            runtime.scroll_offset(&path).x > 0.0,
            "the run followed the hand out of the box",
        );
        assert_eq!(selected(), (0, LONG.len()), "and the sweep took everything it passed");
        runtime.pointer_released(out, y);
    }

    #[test]
    fn a_many_line_field_takes_the_box_and_wraps_inside_it() {
        use crate::layout::{DrawCommand, Proposal, Size};
        use crate::text_input::EditCommand;

        #[derive(Clone, Copy)]
        struct Panel {
            note: State<String>,
            name: State<String>,
        }

        impl Component for Panel {
            fn body(self, _ctx: &Context) -> impl View {
                vstack((
                    text_editor("write a note", self.note.binding()).frame(120.0, 72.0),
                    text_field("name", self.name.binding()).frame_width(120.0),
                ))
            }
        }

        // 120 wide is thirteen cells of run; the line is sixteen tall
        const NOTE: &str = "one two three four five six";
        let panel = Panel {
            note: State::new(NOTE.to_string()),
            name: State::new(String::new()),
        };
        let runtime = Runtime::new();
        runtime.render_stable(&panel);
        let viewport = Proposal::exact(Size { width: 240.0, height: 200.0 });
        let result = runtime.layout(&panel, viewport);

        let box_of = |result: &crate::layout::LayoutResult, tall: bool| {
            result
                .hits
                .iter()
                .find(|(_, rect)| (rect.size.height > 40.0) == tall)
                .cloned()
                .expect("both fields are targets")
        };
        let (note_path, note_frame) = box_of(&result, true);
        let (name_path, _) = box_of(&result, false);

        // the height the parent gave IS the box — not a hole with one
        // line centred in it
        assert_eq!(note_frame.size.height, 72.0);
        assert_eq!(note_frame.size.width, 120.0, "and a long line never widened it");

        // the note wraps by word: two runs, one line apart
        let runs = |result: &crate::layout::LayoutResult| {
            result
                .display
                .iter()
                .filter_map(|command| match command {
                    DrawCommand::TextLine { origin, content, range, .. }
                        if content.starts_with("one two") =>
                    {
                        Some((origin.y, content[range.0..range.1].to_string()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let wrapped = runs(&result);
        assert_eq!(wrapped.len(), 2, "{wrapped:?}");
        assert_eq!(wrapped[0].1, "one two three ");
        assert_eq!(wrapped[1].1, "four five six");
        assert_eq!(wrapped[1].0 - wrapped[0].0, 16.0, "one line apart");

        // a press picks the line by its Y — the second run, not the first
        let second = note_frame.origin.y + 5.0 + 20.0;
        assert!(runtime.pointer_clicked(note_frame.origin.x + 8.0, second, 1, false));
        assert_eq!(runtime.focused(), Some(note_path.clone()));
        assert_eq!(
            runtime.ime_snapshot().expect("focused").selected,
            (14, 0),
            "the caret landed at the start of the second line",
        );

        // and a third click takes THAT line, not the whole note
        runtime.pointer_clicked(note_frame.origin.x + 8.0, second, 3, false);
        runtime.pointer_released(note_frame.origin.x + 8.0, second);
        assert_eq!(runtime.ime_snapshot().expect("focused").selected, (14, 13));

        // more text does not grow the box: it rolls inside it
        assert!(runtime.key(EditCommand::SelectAll).applied);
        assert!(runtime.key(EditCommand::Insert(
            "alpha bravo charlie delta echo foxtrot golf hotel india".into()
        )).applied);
        let grown = runtime.layout(&panel, viewport);
        assert_eq!(box_of(&grown, true).1.size.height, 72.0, "the box held");
        assert!(
            runtime.scroll_offset(&note_path).y > 0.0,
            "the caret at the tail rolled the note under the box",
        );
        // and the rolled offset is a whole number of lines' worth, never
        // past the end of the text
        let offset = runtime.scroll_offset(&note_path).y;
        assert!(runtime.key(EditCommand::Home(false)).applied);
        assert_eq!(runtime.scroll_offset(&note_path).y, 0.0, "Home brought it back");
        assert!(offset > 0.0);

        // the one-line field beside it kept its own natural height
        let (_, name_frame) = box_of(&runtime.layout(&panel, viewport), false);
        assert_eq!(name_frame.size.height, 26.0);
        assert_eq!(name_path, name_path);
    }

    #[test]
    fn a_many_line_field_fills_the_height_the_stack_offers() {
        use crate::layout::{Proposal, Size};

        // two components, so the two trees never share an identity —
        // and neither does anything the reconciler retains under it
        #[derive(Clone, Copy)]
        struct Note {
            text: State<String>,
        }
        #[derive(Clone, Copy)]
        struct Name {
            text: State<String>,
        }

        impl Component for Note {
            fn body(self, _ctx: &Context) -> impl View {
                vstack((text("head"), text_editor("note", self.text.binding())))
            }
        }
        impl Component for Name {
            fn body(self, _ctx: &Context) -> impl View {
                vstack((text("head"), text_field("note", self.text.binding())))
            }
        }

        let viewport = Proposal::exact(Size { width: 240.0, height: 200.0 });
        fn box_of(root: &impl View, viewport: Proposal) -> crate::layout::Rect {
            let runtime = Runtime::new();
            runtime.render_stable(root);
            runtime.layout(root, viewport).hits.last().expect("the field is a target").1
        }

        // the many-line field is the only flexible one in the stack, so
        // the leftover is all its own — it reaches the bottom
        let many = box_of(&Note { text: State::new(String::new()) }, viewport);
        assert!(many.size.height > 100.0, "took the leftover: {}", many.size.height);
        assert_eq!(many.origin.y + many.size.height, 200.0, "down to the last point");

        // the one-line field in the same place keeps its natural height:
        // flexible is a DECLARATION, never a side effect of the room
        assert_eq!(box_of(&Name { text: State::new(String::new()) }, viewport).size.height, 26.0);
    }

    #[test]
    fn the_many_line_field_owns_the_break_and_the_vertical_arrows() {
        use crate::layout::{Proposal, Size};
        use crate::text_input::EditCommand;

        #[derive(Clone, Copy)]
        struct Panel {
            note: State<String>,
            name: State<String>,
        }

        impl Component for Panel {
            fn body(self, _ctx: &Context) -> impl View {
                vstack((
                    text_editor("write a note", self.note.binding()).frame(120.0, 72.0),
                    text_field("name", self.name.binding()).frame_width(120.0),
                ))
            }
        }

        let panel = Panel {
            note: State::new(String::new()),
            name: State::new("deco".to_string()),
        };
        let runtime = Runtime::new();
        runtime.render_stable(&panel);
        let viewport = Proposal::exact(Size { width: 240.0, height: 200.0 });
        let result = runtime.layout(&panel, viewport);
        let box_of = |result: &crate::layout::LayoutResult, tall: bool| {
            result
                .hits
                .iter()
                .find(|(_, rect)| (rect.size.height > 40.0) == tall)
                .cloned()
                .expect("both fields are targets")
        };
        let (_, note_frame) = box_of(&result, true);
        let (_, name_frame) = box_of(&result, false);
        let into = |frame: crate::layout::Rect| {
            (frame.origin.x + 8.0, frame.origin.y + frame.size.height / 2.0)
        };

        // the break belongs to the many-line field
        let (x, y) = into(note_frame);
        runtime.pointer_clicked(x, y, 1, false);
        runtime.pointer_released(x, y);
        assert!(runtime.key(EditCommand::Insert("ab".into())).applied);
        assert!(runtime.key(EditCommand::Newline).applied, "the note takes a break");
        assert!(runtime.key(EditCommand::Insert("cd".into())).applied);
        assert_eq!(panel.note.get(), "ab\ncd", "the break landed in the app's own string");

        // and NOT to the one-line one: it declines, so the app's binding
        // hears the stroke and `\u{2318}\u{21a9}` keeps meaning commit
        let (x, y) = into(name_frame);
        runtime.pointer_clicked(x, y, 1, false);
        runtime.pointer_released(x, y);
        assert!(!runtime.key(EditCommand::Newline).applied, "a one-line field declines");
        assert!(!runtime.key(EditCommand::Up(false)).applied, "and the arrows too");
        assert_eq!(panel.name.get(), "deco", "untouched");

        // a paste of many lines into the one-line field arrives flat
        assert!(runtime.key(EditCommand::SelectAll).applied);
        assert!(runtime.key(EditCommand::Insert("two\nlines".into())).applied);
        assert_eq!(panel.name.get(), "two lines");

        // the vertical arrows walk the note's visual lines and KEEP the
        // column across a short one
        let (x, y) = into(note_frame);
        runtime.pointer_clicked(x, y, 1, false);
        runtime.pointer_released(x, y);
        // three lines of twelve, one and twelve cells — none of them
        // wide enough to wrap inside the box
        assert!(runtime.key(EditCommand::SelectAll).applied);
        assert!(runtime.key(EditCommand::Insert("first line x\ny\nthird line z".into())).applied);
        let selected = || runtime.ime_snapshot().expect("focused").selected;
        assert_eq!(selected(), (27, 0), "at the tail of the third line, column twelve");
        assert!(runtime.key(EditCommand::Up(false)).applied);
        assert_eq!(selected(), (14, 0), "the short line has no column twelve — its end");
        assert!(runtime.key(EditCommand::Up(false)).applied);
        assert_eq!(selected(), (12, 0), "and the column came back on the long one");
        assert!(runtime.key(EditCommand::Down(true)).applied);
        assert_eq!(selected(), (12, 2), "shift extends the walk");

        // off the top is the start of the note; off the bottom, the end
        assert!(runtime.key(EditCommand::Up(false)).applied);
        assert!(runtime.key(EditCommand::Up(false)).applied);
        assert_eq!(selected(), (0, 0));
        for _ in 0..4 {
            assert!(runtime.key(EditCommand::Down(false)).applied);
        }
        assert_eq!(selected(), (27, 0));
    }

    #[test]
    fn a_view_hears_the_pointer_arrive_and_leave() {
        use crate::layout::{Proposal, Size};
        use std::cell::RefCell;
        use std::rc::Rc;

        // ONE log, written by both rows, because the ORDER is the law
        // under test: a flyout handing over to the next one must hear
        // it closed before the next hears it opened
        #[derive(Clone)]
        struct Menu {
            log: Rc<RefCell<Vec<&'static str>>>,
        }

        impl Component for Menu {
            fn body(self, _ctx: &Context) -> impl View {
                let first = Rc::clone(&self.log);
                let second = Rc::clone(&self.log);
                vstack((
                    text("Language Servers").frame(200.0, 40.0).on_hover(move |inside| {
                        first.borrow_mut().push(if inside { "servers in" } else { "servers out" });
                    }),
                    text("Extensions").frame(200.0, 40.0).on_hover(move |inside| {
                        second
                            .borrow_mut()
                            .push(if inside { "extensions in" } else { "extensions out" });
                    }),
                ))
            }
        }

        let menu = Menu { log: Rc::new(RefCell::new(Vec::new())) };
        let runtime = Runtime::new();
        runtime.render_stable(&menu);
        runtime.layout(&menu, Proposal::exact(Size { width: 200.0, height: 100.0 }));

        // the pointer arrives on the first row: the flyout opens, which
        // is the whole gesture a submenu is made of
        runtime.pointer_moved(100.0, 20.0, false);
        assert_eq!(menu.log.borrow().as_slice(), &["servers in"]);

        // moving INSIDE the same row says nothing new: it fires on the
        // CHANGE, not on the pointer
        runtime.pointer_moved(120.0, 30.0, false);
        assert_eq!(menu.log.borrow().len(), 1, "still once");

        // onto the second row, and the ORDER is the point: the row the
        // pointer left hears it FIRST, so two flyouts are never open at
        // the same moment
        runtime.pointer_moved(100.0, 60.0, false);
        assert_eq!(
            menu.log.borrow().as_slice(),
            &["servers in", "servers out", "extensions in"],
            "left before arrived"
        );

        // and off the menu entirely
        runtime.pointer_exited();
        assert_eq!(
            menu.log.borrow().as_slice(),
            &["servers in", "servers out", "extensions in", "extensions out"],
        );
    }

    #[test]
    fn content_sliding_under_a_still_pointer_is_an_arrival() {
        use crate::layout::{Point, Proposal, Size};
        use std::cell::RefCell;
        use std::rc::Rc;

        // the pointer never moves. The LIST does — and a row that
        // slides under a hand is a row the hand is on, which is what
        // every list in every editor already looks like
        #[derive(Clone)]
        struct Rows {
            log: Rc<RefCell<Vec<String>>>,
        }

        impl Component for Rows {
            fn body(self, _ctx: &Context) -> impl View {
                let log = Rc::clone(&self.log);
                scroll(for_each(
                    (0..20).collect::<Vec<usize>>(),
                    |row| format!("row{row}"),
                    move |row| {
                        let (log, row) = (Rc::clone(&log), *row);
                        text(format!("row {row}")).frame(200.0, 20.0).on_hover(move |inside| {
                            if inside {
                                log.borrow_mut().push(format!("row {row}"));
                            }
                        })
                    },
                ))
                .id("rows")
            }
        }

        let rows = Rows { log: Rc::new(RefCell::new(Vec::new())) };
        let runtime = Runtime::new();
        runtime.render_stable(&rows);
        let viewport = Proposal::exact(Size { width: 200.0, height: 100.0 });
        runtime.layout(&rows, viewport);

        // the hand lands on the third row and STAYS there
        runtime.pointer_moved(100.0, 50.0, false);
        assert_eq!(rows.log.borrow().as_slice(), &["row 2".to_string()]);

        // the wheel slides the list by two rows under it. Nothing about
        // the pointer changed; what it is over did
        assert!(runtime.wheel(100.0, 50.0, 0.0, -40.0));
        assert_eq!(runtime.scroll_offset("Rows/[rows]"), Point { x: 0.0, y: 40.0 });
        let _ = runtime.frame(
            &rows,
            Size { width: 200.0, height: 100.0 },
            1,
            crate::layout::Color::rgb(0, 0, 0),
        );
        assert_eq!(
            rows.log.borrow().as_slice(),
            &["row 2".to_string(), "row 4".to_string()],
            "the row that slid under the hand is the row the hand is on"
        );
    }

    #[test]
    fn a_binding_spelled_as_the_character_finds_the_key_that_types_it() {
        use crate::action::{ActionId, Key, KeyMatch, KeyPattern, Stroke};

        const PUSH_INDENT: ActionId = ActionId("test.push_indent");
        const REPEAT: ActionId = ActionId("test.repeat");
        const SPELLED_BY_KEY: ActionId = ActionId("test.spelled_by_key");

        let runtime = Runtime::new();
        // how a hand writes a keymap: the character, not the key and
        // the modifier that make it. Nobody spells `shift-.` when they
        // mean `>` — and until now those bindings were dead, because no
        // keyboard produces a bare `>`
        runtime.bind(KeyPattern::key(Key::Char('>')), PUSH_INDENT);
        runtime.bind(KeyPattern::key(Key::Char('.')), REPEAT);

        // the physical stroke: shift and the period key, which types `>`
        let shifted = KeyPattern { shift: true, ..KeyPattern::key(Key::Char('.')) };
        assert_eq!(
            runtime.chord(Stroke::new(shifted, Some('>'))),
            KeyMatch::Action(PUSH_INDENT),
            "the character spelling found it"
        );

        // and the bare key still answers its own binding: one stroke,
        // one reading, and the two never trade places
        assert_eq!(
            runtime.chord(Stroke::new(KeyPattern::key(Key::Char('.')), Some('.'))),
            KeyMatch::Action(REPEAT),
        );

        // an app that spelled BOTH gets the one it was more precise
        // about: the key's own spelling wins, always
        runtime.bind(shifted, SPELLED_BY_KEY);
        assert_eq!(
            runtime.chord(Stroke::new(shifted, Some('>'))),
            KeyMatch::Action(SPELLED_BY_KEY),
            "the key beats the character it typed"
        );

        // a modifier the layout IGNORED leaves no second spelling: the
        // space bar with shift still makes a space, and a binding on
        // the space bar must not fire for shift-space. This is the same
        // shape as a plain Alt on a platform where Alt types nothing —
        // the character comes back unchanged, and an unchanged
        // character is the key's own name said twice
        const LEADER: ActionId = ActionId("test.leader");
        let runtime = Runtime::new();
        runtime.bind(KeyPattern::key(Key::Char(' ')), LEADER);
        let shift_space = KeyPattern { shift: true, ..KeyPattern::key(Key::Char(' ')) };
        assert_eq!(
            runtime.chord(Stroke::new(shift_space, Some(' '))),
            KeyMatch::None,
            "shift-space is not the space bar"
        );
        assert_eq!(
            runtime.chord(Stroke::new(KeyPattern::key(Key::Char(' ')), Some(' '))),
            KeyMatch::Action(LEADER),
            "and the space bar still is"
        );
        // shift and a letter only change its case, and a keymap spells
        // a letter in ONE case: no second reading there either
        runtime.bind(KeyPattern::key(Key::Char('a')), REPEAT);
        let shift_a = KeyPattern { shift: true, ..KeyPattern::key(Key::Char('a')) };
        assert_eq!(runtime.chord(Stroke::new(shift_a, Some('A'))), KeyMatch::None);

        // a chord types nothing, so nothing answers for it by character
        let runtime = Runtime::new();
        runtime.bind(KeyPattern::key(Key::Char('$')), PUSH_INDENT);
        let chord = KeyPattern { shift: true, ..KeyPattern::command(Key::Char('4')) };
        assert_eq!(
            runtime.chord(Stroke::new(chord, Some('$'))),
            KeyMatch::None,
            "cmd-shift-4 is a chord, and a chord types nothing"
        );

        // the same key with no chord modifier DOES answer
        let plain = KeyPattern { shift: true, ..KeyPattern::key(Key::Char('4')) };
        assert_eq!(
            runtime.chord(Stroke::new(plain, Some('$'))),
            KeyMatch::Action(PUSH_INDENT),
        );
    }

    #[test]
    fn a_field_answers_its_own_key_and_only_where_the_app_asked() {
        use crate::action::{Key, KeyPattern};
        use crate::layout::{Proposal, Size};
        use crate::text_input::EditCommand;

        #[derive(Clone, Copy)]
        struct Panel {
            note: State<String>,
            name: State<String>,
            bare: State<String>,
            sent: State<i32>,
            committed: State<i32>,
        }

        impl Component for Panel {
            fn body(self, _ctx: &Context) -> impl View {
                vstack((
                    text_editor("write a note", self.note.binding())
                        .on_submit(move || self.committed.add(1))
                        .frame(120.0, 72.0),
                    text_field("name", self.name.binding())
                        .on_submit(move || self.sent.add(1))
                        .frame(120.0, 26.0),
                    text_field("nothing", self.bare.binding()).frame_width(120.0),
                ))
            }
        }

        let panel = Panel {
            note: State::new(String::new()),
            name: State::new(String::new()),
            bare: State::new(String::new()),
            sent: State::new(0),
            committed: State::new(0),
        };
        let runtime = Runtime::new();
        runtime.render_stable(&panel);
        let result =
            runtime.layout(&panel, Proposal::exact(Size { width: 240.0, height: 240.0 }));
        // by the slot each field holds in the body — a one-line field
        // keeps its natural height whatever box it is given, so the
        // three cannot be told apart by size
        let field_of = |slot: &str| {
            result
                .hits
                .iter()
                .find(|(path, _)| path.ends_with(slot))
                .map(|(_, rect)| (rect.origin.x + 8.0, rect.origin.y + rect.size.height / 2.0))
                .expect("the field is a target")
        };
        let click = |(x, y): (f64, f64)| {
            runtime.pointer_clicked(x, y, 1, false);
            runtime.pointer_released(x, y);
        };
        let (note, name, bare) = (field_of("#0"), field_of("#1"), field_of("#2"));

        // the one-line field: the bare Enter IS its submit
        click(name);
        assert!(runtime.key(EditCommand::Insert("deco".into())).applied);
        assert!(runtime.key(EditCommand::Newline).applied, "the field took its own key");
        assert_eq!(panel.sent.get(), 1, "and the app heard it");
        assert_eq!(panel.name.get(), "deco", "a submit is not an edit");

        // and the chord stays the app's there: one door per field
        assert!(
            !runtime.key_stroke(&KeyPattern::command(Key::Enter)).handled,
            "the chord over a one-line field belongs to the app"
        );
        assert_eq!(panel.sent.get(), 1);

        // a field that named no handler declines exactly as it always did
        click(bare);
        assert!(!runtime.key(EditCommand::Newline).applied, "no handler, no door");

        // the many-line field keeps Enter for its break...
        click(note);
        assert!(runtime.key(EditCommand::Insert("ab".into())).applied);
        assert!(runtime.key(EditCommand::Newline).applied);
        assert_eq!(panel.note.get(), "ab\n", "Enter is still the break");
        assert_eq!(panel.committed.get(), 0, "and a break is not a submit");

        // ...and puts its own key on the chord, before the keymap
        assert!(runtime.key_stroke(&KeyPattern::command(Key::Enter)).handled);
        assert_eq!(panel.committed.get(), 1);
        assert_eq!(panel.note.get(), "ab\n", "the chord left the note alone");
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

    /// A host mounts a TABLE outside the tree: the shape ninety actions
    /// from a shared `const` arrive in. It answers when nothing in the
    /// tree claims the id, and it stands across passes.
    #[test]
    fn a_host_mounts_a_table_outside_the_tree() {
        const LEFT: ActionId = ActionId("test.vim.left");
        const RIGHT: ActionId = ActionId("test.vim.right");

        #[derive(Clone, Copy)]
        struct Page;
        impl Component for Page {
            fn body(self, _ctx: &Context) -> impl View {
                text("a page that claims nothing")
            }
        }

        let runtime = Runtime::new();
        let log = Rc::new(std::cell::RefCell::new(Vec::new()));
        for (id, verb) in [(LEFT, "left"), (RIGHT, "right")] {
            let log = Rc::clone(&log);
            runtime.on_action(id, move || log.borrow_mut().push(verb));
        }
        runtime.render_stable(&Page);

        assert!(runtime.dispatch_action(LEFT));
        assert!(runtime.dispatch_action(RIGHT));
        assert_eq!(*log.borrow(), vec!["left", "right"]);

        // a pass does not sweep it: the host mounted it, not the tree
        runtime.render(&Page);
        assert!(runtime.dispatch_action(LEFT));
        assert_eq!(log.borrow().len(), 3);

        // an id nobody took still answers nobody — the key walks on
        assert!(!runtime.dispatch_action(ActionId("test.vim.nowhere")));

        // and the host takes its own table down
        runtime.clear_action_handlers();
        assert!(!runtime.dispatch_action(LEFT), "the table came down whole");
    }

    /// The host's table is the FLOOR: a mounted view claiming the same
    /// id shadows it, and the floor comes back when that view leaves.
    /// It is the same law the tree keeps among its own.
    #[test]
    fn a_mounted_view_shadows_the_hosts_table() {
        const JUMP: ActionId = ActionId("test.vim.jump");

        #[derive(Clone, Copy)]
        struct Pane {
            focused: State<bool>,
            local: State<i32>,
        }
        impl Component for Pane {
            fn body(self, _ctx: &Context) -> impl View {
                let local = self.local;
                if self.focused.get() {
                    Either::First(text("pane").on_action(JUMP, move || local.add(1)))
                } else {
                    Either::Second(text("pane"))
                }
            }
        }

        let runtime = Runtime::new();
        let pane = Pane { focused: State::new(false), local: State::new(0) };
        let host = Rc::new(std::cell::Cell::new(0));
        {
            let host = Rc::clone(&host);
            runtime.on_action(JUMP, move || host.set(host.get() + 1));
        }
        runtime.render_stable(&pane);

        assert!(runtime.dispatch_action(JUMP));
        assert_eq!((host.get(), pane.local.get()), (1, 0), "the floor answers");

        pane.focused.set(true);
        runtime.render_stable(&pane);
        assert!(runtime.dispatch_action(JUMP));
        assert_eq!((host.get(), pane.local.get()), (1, 1), "the view shadows it");

        pane.focused.set(false);
        runtime.render_stable(&pane);
        assert!(runtime.dispatch_action(JUMP));
        assert_eq!((host.get(), pane.local.get()), (2, 1), "and the floor comes back");
    }

    /// Mounting an id twice REPLACES it, the way a rebind does — a
    /// cascade re-installed must not fire the layer it replaced.
    #[test]
    fn mounting_a_host_handler_twice_replaces_it() {
        const VERB: ActionId = ActionId("test.vim.verb");

        #[derive(Clone, Copy)]
        struct Blank;
        impl Component for Blank {
            fn body(self, _ctx: &Context) -> impl View {
                text("blank")
            }
        }

        let runtime = Runtime::new();
        runtime.render_stable(&Blank);
        let log = Rc::new(std::cell::RefCell::new(Vec::new()));
        for layer in ["first", "second"] {
            let log = Rc::clone(&log);
            runtime.on_action(VERB, move || log.borrow_mut().push(layer));
        }
        assert!(runtime.dispatch_action(VERB));
        assert_eq!(*log.borrow(), vec!["second"]);
    }

    /// A field's content can carry runs of another ink — the same
    /// record and the same splitter a `text(…)` uses, so a template
    /// variable stays yellow while the line is being edited.
    #[test]
    fn a_field_tints_runs_of_its_own_content() {
        use crate::layout::{DrawCommand, Proposal, Size};
        const TEMPLATE: Color = Color::hex(0xE5C07B);

        #[derive(Clone, Copy)]
        struct Bar {
            url: State<String>,
        }
        impl Component for Bar {
            fn body(self, _ctx: &Context) -> impl View {
                // "https://{{host}}/v1" — the braces are the run
                let text = self.url.get();
                let start = text.find("{{").unwrap_or(0);
                let end = text.find("}}").map(|at| at + 2).unwrap_or(start);
                text_field("url", self.url.binding())
                    .highlight(vec![(start, end)], TEMPLATE)
            }
        }

        let runtime = Runtime::new();
        let bar = Bar { url: State::new("https://{{host}}/v1".to_string()) };
        let result = runtime
            .settled_layout(&bar, Proposal::exact(Size { width: 400.0, height: 40.0 }));
        let inks: Vec<(Color, String)> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { content, range, color, .. } => {
                    Some((*color, content[range.0..range.1].to_string()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(inks.len(), 3, "three runs, not one line: {inks:?}");
        assert_eq!(inks[0].1, "https://");
        assert_eq!(inks[1], (TEMPLATE, "{{host}}".to_string()), "the variable is tinted");
        assert_eq!(inks[2].1, "/v1");
        assert_ne!(inks[0].0, TEMPLATE, "the rest keeps the inherited ink");
    }

    /// The ranges index the CONTENT, so an empty field showing its
    /// placeholder ignores them — otherwise a stale range would tint a
    /// slice of a string it never named.
    #[test]
    fn an_empty_field_ignores_the_runs_of_a_content_it_has_not_got() {
        use crate::layout::{DrawCommand, Proposal, Size};

        #[derive(Clone, Copy)]
        struct Bar {
            url: State<String>,
        }
        impl Component for Bar {
            fn body(self, _ctx: &Context) -> impl View {
                text_field("type a url here", self.url.binding())
                    .highlight(vec![(0, 4)], Color::hex(0xE5C07B))
            }
        }

        let runtime = Runtime::new();
        let bar = Bar { url: State::new(String::new()) };
        let result = runtime
            .settled_layout(&bar, Proposal::exact(Size { width: 400.0, height: 40.0 }));
        let runs = result
            .display
            .iter()
            .filter(|command| matches!(command, DrawCommand::TextLine { .. }))
            .count();
        assert_eq!(runs, 1, "the placeholder is one run in the placeholder ink");
    }

    /// A bare field paints no ground, no edge and no rounding, so what
    /// is behind it is the ground — a grid cell that is editable
    /// without wearing a box the design never had.
    #[test]
    fn a_bare_field_wears_no_chrome_of_its_own() {
        use crate::layout::{Corners, DrawCommand, Proposal, Size};

        #[derive(Clone, Copy)]
        struct Cell {
            value: State<String>,
            bare: bool,
        }
        impl Component for Cell {
            fn body(self, _ctx: &Context) -> impl View {
                let field = text_field("", self.value.binding());
                if self.bare {
                    Either::First(field.bare())
                } else {
                    Either::Second(field)
                }
            }
        }

        let room = Proposal::exact(Size { width: 200.0, height: 30.0 });
        let boxes = |bare: bool| {
            let runtime = Runtime::new();
            let cell = Cell { value: State::new("value".to_string()), bare };
            let result = runtime.settled_layout(&cell, room);
            let fills = result
                .display
                .iter()
                .filter(|command| matches!(command, DrawCommand::FillRect { .. }))
                .count();
            let strokes = result
                .display
                .iter()
                .filter(|command| matches!(command, DrawCommand::StrokeRect { .. }))
                .count();
            let round = result.display.iter().any(|command| match command {
                DrawCommand::FillRect { corner_radius, .. }
                | DrawCommand::StrokeRect { corner_radius, .. } => {
                    *corner_radius != Corners::ZERO
                }
                _ => false,
            });
            (fills, strokes, round)
        };

        let (themed_fills, themed_strokes, themed_round) = boxes(false);
        assert!(themed_fills >= 1, "the themed field paints its ground");
        assert_eq!(themed_strokes, 1, "and its edge");
        assert!(themed_round, "with the field radius");

        let (bare_fills, bare_strokes, bare_round) = boxes(true);
        assert_eq!(bare_strokes, 0, "a bare field draws no edge");
        assert!(bare_fills < themed_fills, "and no ground of its own");
        // the field is not focused here, so there is no caret — which
        // IS rounded by design, and is not chrome
        assert!(!bare_round, "and no corner of its own is rounded");
    }

    /// A lane that HUGS beside a spacer takes what it needs, not half
    /// the row.
    ///
    /// The waterfall only ever hands surplus DOWNWARD: a child that
    /// takes less than its quota releases the rest. Nothing lets a
    /// child that wants MORE take from a filler that wants nothing in
    /// particular — so a hug and a spacer both answered "I took the
    /// whole quota" on the first round and the loop stopped there, with
    /// the hug pinned at half the row and the other half blank.
    #[test]
    fn a_hugging_lane_beside_a_spacer_takes_what_it_needs() {
        use crate::layout::{Proposal, Size};

        const ROW: f64 = 1000.0;
        const CONTENT: f64 = 720.0;

        #[derive(Clone, Copy)]
        struct Strip {
            lane: State<f64>,
        }
        impl Component for Strip {
            fn body(self, _ctx: &Context) -> impl View {
                let lane = self.lane;
                hstack((
                    scroll(empty().frame(CONTENT, 24.0))
                        .horizontal()
                        .hugging()
                        .on_measure(move |size| lane.set(size.width)),
                    spacer(),
                ))
            }
        }

        let runtime = Runtime::new();
        let strip = Strip { lane: State::new(0.0) };
        let _ = runtime
            .settled_layout(&strip, Proposal::exact(Size { width: ROW, height: 40.0 }));
        assert_eq!(
            strip.lane.get(),
            CONTENT,
            "the hug asked for {CONTENT} of a {ROW} row and the spacer wanted nothing",
        );
    }

    /// Two lanes that both want more than half still split it evenly.
    ///
    /// The tier only moves surplus that EXISTS. When every ideal cannot
    /// fit, the children are in genuine contention and the quota is the
    /// fair answer — the same one this stack has always given.
    #[test]
    fn two_hugging_lanes_that_do_not_fit_still_share_the_row() {
        use crate::layout::{Proposal, Size};

        #[derive(Clone, Copy)]
        struct Strip {
            left: State<f64>,
            right: State<f64>,
        }
        impl Component for Strip {
            fn body(self, _ctx: &Context) -> impl View {
                let (left, right) = (self.left, self.right);
                let lane = |width: f64| scroll(empty().frame(width, 24.0)).horizontal().hugging();
                hstack((
                    lane(700.0).on_measure(move |size| left.set(size.width)),
                    lane(900.0).on_measure(move |size| right.set(size.width)),
                ))
            }
        }

        let runtime = Runtime::new();
        let strip = Strip { left: State::new(0.0), right: State::new(0.0) };
        let _ = runtime
            .settled_layout(&strip, Proposal::exact(Size { width: 1000.0, height: 40.0 }));
        assert_eq!(
            (strip.left.get(), strip.right.get()),
            (500.0, 500.0),
            "1600 of ideal into a 1000 row is contention, and the quota is fair",
        );
    }

    /// The size a view resolved to reaches the app, and reaches it in
    /// the SAME frame — a body that turns the measurement into a frame
    /// runs before anything is painted.
    #[test]
    fn a_view_reports_the_size_it_resolved_to() {
        use crate::layout::{Proposal, Size};

        #[derive(Clone, Copy)]
        struct Page {
            lines: State<usize>,
            measured: State<Size>,
            reports: State<usize>,
        }
        impl Component for Page {
            fn body(self, _ctx: &Context) -> impl View {
                let measured = self.measured;
                let reports = self.reports;
                vstack(for_each(
                    (0..self.lines.get()).collect::<Vec<usize>>(),
                    |line| line.to_string(),
                    |line| text(format!("line {line}")).frame(200.0, 20.0),
                ))
                .on_measure(move |size| {
                    measured.set(size);
                    reports.set(reports.get() + 1);
                })
            }
        }

        let runtime = Runtime::new();
        let page = Page {
            lines: State::new(3),
            measured: State::new(Size::default()),
            reports: State::new(0),
        };
        let room = Proposal::exact(Size { width: 400.0, height: 400.0 });

        let _ = runtime.settled_layout(&page, room);
        assert_eq!(page.measured.get().height, 60.0, "three lines of twenty");
        assert_eq!(page.reports.get(), 1);

        // a view at REST says nothing: a probe that fired every frame
        // would dirty the world forever
        let _ = runtime.settled_layout(&page, room);
        assert_eq!(page.reports.get(), 1, "a size that did not change is not news");

        // and a size that changes reports the new one, once
        page.lines.set(5);
        let _ = runtime.settled_layout(&page, room);
        assert_eq!(page.measured.get().height, 100.0);
        assert_eq!(page.reports.get(), 2);
    }

    /// The reaction lands in the same frame as the report: a body that
    /// turns a measured height into a FRAME shows the right size on the
    /// first paint, never a wrong one corrected on the next.
    #[test]
    fn a_body_that_reacts_to_a_measure_runs_before_the_paint() {
        use crate::layout::{Proposal, Size};

        #[derive(Clone, Copy)]
        struct Card {
            natural: State<f64>,
        }
        impl Component for Card {
            fn body(self, _ctx: &Context) -> impl View {
                let natural = self.natural;
                // the document measures freely; the card caps it
                let document = vstack(for_each(
                    (0..9).collect::<Vec<usize>>(),
                    |line| line.to_string(),
                    |line| text(format!("line {line}")).frame(200.0, 20.0),
                ))
                .on_measure(move |size| natural.set(size.height));
                let capped = self.natural.get().min(120.0);
                vstack(document).frame(200.0, if capped > 0.0 { capped } else { 400.0 })
            }
        }

        let runtime = Runtime::new();
        let card = Card { natural: State::new(0.0) };
        let result = runtime
            .settled_layout(&card, Proposal::exact(Size { width: 400.0, height: 400.0 }));
        assert_eq!(card.natural.get(), 180.0, "nine lines of twenty, unrestricted");
        // the FRAME this pass produced already carries the cap — the
        // 400 the first pass used never reaches a pixel
        let root = result.frames.get("Card").expect("the card is placed");
        assert_eq!(root.size.height, 120.0, "capped, in the same frame");
    }

    /// Two probes in one body are two addresses. They sit at different
    /// positions, and a position is what the identity is made of — but
    /// the segment is fixed, so this is the collision worth pinning.
    #[test]
    fn two_probes_in_one_body_do_not_share_an_address() {
        use crate::layout::{Proposal, Size};

        #[derive(Clone, Copy)]
        struct Page {
            top: State<f64>,
            bottom: State<f64>,
        }
        impl Component for Page {
            fn body(self, _ctx: &Context) -> impl View {
                let (top, bottom) = (self.top, self.bottom);
                vstack((
                    text("a").frame(100.0, 30.0).on_measure(move |s| top.set(s.height)),
                    text("b").frame(100.0, 70.0).on_measure(move |s| bottom.set(s.height)),
                ))
            }
        }

        let runtime = Runtime::new();
        let page = Page { top: State::new(0.0), bottom: State::new(0.0) };
        let _ = runtime
            .settled_layout(&page, Proposal::exact(Size { width: 400.0, height: 400.0 }));
        assert_eq!((page.top.get(), page.bottom.get()), (30.0, 70.0));
    }

    /// A probe under a SKIPPED body still reports. The retained tree is
    /// what the frame is laid out from, so the node is still placed —
    /// and the writer is retained beside it, like every other one.
    #[test]
    fn a_probe_under_a_skipped_body_still_reports() {
        use crate::layout::{Proposal, Size};

        #[derive(Clone, Copy)]
        struct Inner {
            seen: State<f64>,
        }
        impl Component for Inner {
            fn body(self, _ctx: &Context) -> impl View {
                let seen = self.seen;
                // the WIDTH follows the room the outer hands down, so
                // the measured size can move while this body does not
                empty()
                    .frame_max(f64::INFINITY, 40.0, Alignment::Center)
                    .on_measure(move |size| seen.set(size.width))
            }
        }

        #[derive(Clone, Copy)]
        struct Outer {
            room: State<f64>,
            seen: State<f64>,
        }
        impl Component for Outer {
            fn body(self, _ctx: &Context) -> impl View {
                // the outer re-runs on `room`; the inner's payload never
                // changes, so its body is skipped
                vstack(Inner { seen: self.seen }).frame(self.room.get(), 400.0)
            }
        }

        let runtime = Runtime::new();
        let page = Outer { room: State::new(300.0), seen: State::new(0.0) };
        let room = Proposal::exact(Size { width: 400.0, height: 400.0 });
        let _ = runtime.settled_layout(&page, room);
        assert_eq!(page.seen.get(), 300.0);

        // the outer re-runs and hands down a different width; the inner
        // body is SKIPPED, and the probe still reports the new size
        page.room.set(180.0);
        let _ = runtime.settled_layout(&page, room);
        let runs = runtime.body_runs();
        assert!(
            !runs.iter().any(|path| path.ends_with("Inner")),
            "the inner body was skipped, which is the case under test: {runs:?}"
        );
        assert_eq!(page.seen.get(), 180.0, "the probe reported through the retention");
    }

    /// A region can hold its offset in the app's own state, both ways:
    /// the app WRITES where to go, and the wheel tells it where the
    /// region landed. It is what an anchor that names a POSITION needs
    /// — "put this line in the middle" is not a visibility.
    #[test]
    fn an_app_can_command_and_read_a_regions_offset() {
        use crate::layout::{Point, Proposal, Size};

        #[derive(Clone, Copy)]
        struct Page {
            at: State<Point>,
        }
        impl Component for Page {
            fn body(self, _ctx: &Context) -> impl View {
                scroll(for_each(
                    (0..40).collect::<Vec<i32>>(),
                    |line| line.to_string(),
                    |line| text(format!("line {line}")).frame(200.0, 20.0),
                ))
                .offset(self.at.binding())
            }
        }

        let runtime = Runtime::new();
        let page = Page { at: State::new(Point::default()) };
        let size = Size { width: 200.0, height: 100.0 };
        let path = "Page";
        let _ = runtime.settled_layout(&page, Proposal::exact(size));

        // the wheel is sovereign, and the binding hears where it landed
        assert!(runtime.wheel(100.0, 50.0, 0.0, -60.0));
        let _ = runtime.settled_layout(&page, Proposal::exact(size));
        assert_eq!(runtime.scroll_offset(path).y, 60.0);
        assert_eq!(page.at.get().y, 60.0, "the app reads a true offset");

        // and the app can say WHERE: the middle of the window, which
        // no reveal could express
        page.at.set(Point { x: 0.0, y: 320.0 });
        let _ = runtime.settled_layout(&page, Proposal::exact(size));
        assert_eq!(runtime.scroll_offset(path).y, 320.0, "the region went there");

        // a value past the end comes home already clamped: the app's
        // state and the region never disagree about where it is
        page.at.set(Point { x: 0.0, y: 9000.0 });
        let _ = runtime.settled_layout(&page, Proposal::exact(size));
        let travel = 40.0 * 20.0 - 100.0;
        assert_eq!(runtime.scroll_offset(path).y, travel);
        assert_eq!(page.at.get().y, travel, "clamped, and the app was told");

        // a frame that changes nothing writes nothing — a region at
        // rest must not dirty the world every pass
        let before = page.at.get();
        let _ = runtime.settled_layout(&page, Proposal::exact(size));
        assert_eq!(page.at.get(), before);
    }

    /// A region with no binding is untouched: the offset stays the
    /// engine's, and nothing is written anywhere.
    #[test]
    fn a_region_without_a_binding_keeps_the_offset_to_itself() {
        use crate::layout::{Proposal, Size};

        #[derive(Clone, Copy)]
        struct Page;
        impl Component for Page {
            fn body(self, _ctx: &Context) -> impl View {
                scroll(for_each(
                    (0..40).collect::<Vec<i32>>(),
                    |line| line.to_string(),
                    |line| text(format!("line {line}")).frame(200.0, 20.0),
                ))
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 200.0, height: 100.0 };
        let _ = runtime.settled_layout(&Page, Proposal::exact(size));
        assert!(runtime.wheel(100.0, 50.0, 0.0, -40.0));
        let _ = runtime.settled_layout(&Page, Proposal::exact(size));
        assert_eq!(runtime.scroll_offset("Page").y, 40.0);
    }

    /// The other end of the bridge, walked whole: a bare stroke reaches
    /// the keymap, the keymap names an action, and the host's table
    /// ANSWERS it — which is what makes the stroke consumed.
    #[test]
    fn a_stroke_crosses_the_keymap_into_the_hosts_table() {
        const NEXT_WORD: ActionId = ActionId("test.vim.next_word");

        #[derive(Clone, Copy)]
        struct Page;
        impl Component for Page {
            fn body(self, _ctx: &Context) -> impl View {
                text("no view claims a thing")
            }
        }

        let runtime = Runtime::new();
        runtime.render_stable(&Page);
        let moved = Rc::new(std::cell::Cell::new(0));
        {
            let moved = Rc::clone(&moved);
            runtime.on_action(NEXT_WORD, move || moved.set(moved.get() + 1));
        }
        runtime.bind(KeyPattern::key(Key::Char('w')), NEXT_WORD);

        let stroke = KeyPattern::key(Key::Char('w'));
        let action = match runtime.chord(&stroke) {
            crate::action::KeyMatch::Action(action) => action,
            other => panic!("the map answers the bare key, got {other:?}"),
        };
        assert!(runtime.dispatch_action(action), "the stroke is consumed");
        assert_eq!(moved.get(), 1);

        // an action the host never took leaves the stroke alone, so a
        // field downstream still types it
        runtime.bind(KeyPattern::key(Key::Char('q')), ActionId("test.vim.unmounted"));
        let orphan = match runtime.chord(&KeyPattern::key(Key::Char('q'))) {
            crate::action::KeyMatch::Action(action) => action,
            other => panic!("bound, got {other:?}"),
        };
        assert!(!runtime.dispatch_action(orphan), "a binding with no handler walks on");
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
    fn the_wheel_goes_to_the_region_that_paints_last_under_the_point() {
        use crate::layout::{Point, Proposal, Side, Size};

        // a page that fills the window, and a panel over it that
        // travels too — the shape of a settings modal over a document
        #[derive(Clone, Copy)]
        struct Stacked;

        impl Component for Stacked {
            fn body(self, _ctx: &Context) -> impl View {
                zstack!(
                    scroll(text("page").frame(400.0, 4000.0)).id("page"),
                    scroll(text("panel").frame(180.0, 900.0)).id("panel").frame(200.0, 100.0),
                )
            }
        }

        let runtime = Runtime::new();
        runtime.render_stable(&Stacked);
        let result = runtime.layout(&Stacked, Proposal::exact(Size { width: 400.0, height: 300.0 }));
        let region = |suffix: &str| {
            result
                .scrolls
                .iter()
                .find(|region| region.path.ends_with(suffix))
                .unwrap_or_else(|| panic!("{suffix} is a region"))
                .clone()
        };
        let (page, panel) = (region("[page]"), region("[panel]"));
        // the page's frame contains the panel WHOLE — which is what
        // made a first-match-wins pick answer with the wrong one
        assert!(page.frame.contains(200.0, 150.0) && panel.frame.contains(200.0, 150.0));

        assert!(runtime.wheel(200.0, 150.0, 0.0, -60.0));
        assert!(runtime.scroll_offset(&panel.path).y > 0.0, "the panel under the pointer moves");
        assert_eq!(runtime.scroll_offset(&page.path), Point::ZERO, "the page behind holds still");

        // ...and beside the panel the page is what the pointer is over,
        // so the page is what moves. A layer answers for what it covers
        // and for nothing else.
        assert!(runtime.wheel(20.0, 20.0, 0.0, -60.0));
        assert!(runtime.scroll_offset(&page.path).y > 0.0, "beside it, the page is the one under the point");

        // a popover's own list is the same law: it places AFTER the
        // root, so it paints last and it answers
        #[derive(Clone, Copy)]
        struct Anchored {
            open: State<bool>,
        }

        impl Component for Anchored {
            fn body(self, _ctx: &Context) -> impl View {
                scroll(text("page").frame(400.0, 4000.0)).id("page").popover(
                    self.open.binding(),
                    Side::Trailing,
                    move |_| {
                        erased(scroll(text("list").frame(160.0, 900.0)).id("pop").frame(180.0, 120.0))
                    },
                )
            }
        }

        let anchored = Anchored { open: State::new(true) };
        let runtime = Runtime::new();
        runtime.render_stable(&anchored);
        let result = runtime.layout(&anchored, Proposal::exact(Size { width: 400.0, height: 300.0 }));
        let find = |suffix: &str| {
            result.scrolls.iter().find(|r| r.path.ends_with(suffix)).expect("region").clone()
        };
        let (page, pop) = (find("[page]"), find("[pop]"));
        let at = (
            pop.frame.origin.x + pop.frame.size.width / 2.0,
            pop.frame.origin.y + pop.frame.size.height / 2.0,
        );
        assert!(page.frame.contains(at.0, at.1), "the page runs under the card");
        assert!(runtime.wheel(at.0, at.1, 0.0, -60.0));
        assert!(runtime.scroll_offset(&pop.path).y > 0.0, "the card's own list moves");
        assert_eq!(runtime.scroll_offset(&page.path), Point::ZERO, "the page under it holds still");
    }

    #[test]
    fn a_sheet_owns_what_it_covers() {
        use crate::layout::{Point, Proposal, Size};

        #[derive(Clone, Copy)]
        struct Stage {
            behind: State<i32>,
            open: State<bool>,
        }

        impl Component for Stage {
            fn body(self, _ctx: &Context) -> impl View {
                let behind = self.behind;
                scroll(text("the page behind").frame(400.0, 4000.0))
                    .id("page")
                    .on_click(move || behind.add(1))
                    .sheet(self.open.binding(), move |_| {
                        erased(text("panel").frame(120.0, 80.0))
                    })
            }
        }

        let stage = Stage { behind: State::new(0), open: State::new(true) };
        let runtime = Runtime::new();
        runtime.render_stable(&stage);
        let result = runtime.layout(&stage, Proposal::exact(Size { width: 400.0, height: 300.0 }));
        let page = result
            .scrolls
            .iter()
            .find(|region| region.path.ends_with("[page]"))
            .expect("the page is a region")
            .path
            .clone();

        // the wheel over the sheet's own card, where the card has
        // nothing scrollable: it stops there, it does not fall through
        assert!(!runtime.wheel(200.0, 150.0, 0.0, -60.0), "nothing moved");
        assert_eq!(runtime.scroll_offset(&page), Point::ZERO);

        // and BESIDE the card too — a modal covers the window, not just
        // the rectangle it paints. This is the half a frame test cannot
        // do, because there is nothing drawn out here at all
        assert!(!runtime.wheel(20.0, 20.0, 0.0, -60.0), "still nothing");
        assert_eq!(runtime.scroll_offset(&page), Point::ZERO, "the page behind holds still");

        // the pointer stops at the same line: a press straight through
        // the card used to reach the page's own action
        runtime.pointer_pressed(200.0, 150.0);
        runtime.pointer_released(200.0, 150.0);
        assert_eq!(stage.behind.get(), 0, "the card is not a window onto what it covers");
        runtime.pointer_pressed(20.0, 20.0);
        runtime.pointer_released(20.0, 20.0);
        assert_eq!(stage.behind.get(), 0, "and neither is the room beside it");

        // a right press does not reach through it either: the line is
        // ONE line, across every list the pointer consults, because a
        // layer that eats the wheel and not the menu is a modal with
        // holes and the holes are where the bugs live
        assert!(!runtime.context_click(200.0, 150.0), "no menu from what it covers");

        // closed, everything answers again — the capture lives exactly
        // as long as the sheet does
        stage.open.set(false);
        runtime.render_stable(&stage);
        let _ = runtime.layout(&stage, Proposal::exact(Size { width: 400.0, height: 300.0 }));
        assert!(runtime.wheel(200.0, 150.0, 0.0, -60.0), "the page takes the wheel back");
        assert!(runtime.scroll_offset(&page).y > 0.0);
        runtime.pointer_pressed(200.0, 150.0);
        runtime.pointer_released(200.0, 150.0);
        assert_eq!(stage.behind.get(), 1, "and its action fires again");
    }

    #[test]
    fn a_modal_covers_what_it_paints_over_and_not_the_window() {
        use crate::layout::{Proposal, Size};

        // the sheet hangs on the HEADER of a column, so the list below
        // is placed after it — drawn ON TOP of the card, not behind it
        #[derive(Clone, Copy)]
        struct Misplaced {
            open: State<bool>,
        }

        impl Component for Misplaced {
            fn body(self, _ctx: &Context) -> impl View {
                vstack((
                    text("header").frame(400.0, 40.0).sheet(self.open.binding(), move |_| {
                        erased(text("panel").frame(120.0, 80.0))
                    }),
                    scroll(text("list").frame(400.0, 4000.0)).id("list"),
                ))
            }
        }

        let stage = Misplaced { open: State::new(true) };
        let runtime = Runtime::new();
        runtime.render_stable(&stage);
        let result = runtime.layout(&stage, Proposal::exact(Size { width: 400.0, height: 300.0 }));
        let list = result
            .scrolls
            .iter()
            .find(|region| region.path.ends_with("[list]"))
            .expect("the list is a region")
            .path
            .clone();

        // and so it still answers. "Covers" is paint: a thing drawn on
        // top of a modal was never behind it, and pretending otherwise
        // would make capture depend on nothing the reader can see.
        // A sheet that means to be modal to a scene hangs over that
        // scene — this shape is the honest edge of the law, not a hole
        // in it.
        assert!(runtime.wheel(200.0, 200.0, 0.0, -60.0), "drawn after the sheet, so not covered");
        assert!(runtime.scroll_offset(&list).y > 0.0);
    }

    #[test]
    fn a_nested_region_still_beats_the_one_around_it() {
        use crate::layout::{Point, Proposal, Size};

        #[derive(Clone, Copy)]
        struct Nest;

        impl Component for Nest {
            fn body(self, _ctx: &Context) -> impl View {
                scroll(vstack((
                    text("head").frame(300.0, 50.0),
                    scroll(text("inner").frame(280.0, 900.0)).id("inner").frame(300.0, 100.0),
                    text("tail").frame(300.0, 2000.0),
                )))
                .id("outer")
            }
        }

        let runtime = Runtime::new();
        runtime.render_stable(&Nest);
        let result = runtime.layout(&Nest, Proposal::exact(Size { width: 300.0, height: 300.0 }));
        let find = |suffix: &str| {
            result.scrolls.iter().find(|r| r.path.ends_with(suffix)).expect("region").clone()
        };
        let (outer, inner) = (find("[outer]"), find("[inner]"));
        let (x, y) = (150.0, inner.frame.origin.y + 50.0);
        assert!(outer.frame.contains(x, y), "the outer runs under the inner");

        // a child paints over its parent, so the same rule that puts a
        // layer above what it covers puts the inner list above the page
        // it sits in — nothing here needs a second law
        assert!(runtime.wheel(x, y, 0.0, -60.0));
        assert!(runtime.scroll_offset(&inner.path).y > 0.0, "the inner list takes it");
        assert_eq!(runtime.scroll_offset(&outer.path), Point::ZERO, "the page around it holds still");

        // and where only the outer travels, the outer answers
        assert!(runtime.wheel(150.0, 20.0, 0.0, -60.0));
        assert!(runtime.scroll_offset(&outer.path).y > 0.0);
    }

    #[test]
    fn a_named_family_travels_to_the_engine_and_names_only_the_face() {
        use crate::layout::{Proposal, Size};
        use crate::text_engine::{Family, FontPatch, LineMetrics, TextRaster, Weight};

        // the table: one number per name, for the life of the process
        let menlo = Family::named("Menlo");
        assert_eq!(menlo, Family::named("Menlo"), "the same name is the same face");
        assert_ne!(menlo, Family::named("Georgia"));
        assert_eq!(menlo.name().as_deref(), Some("Menlo"));
        assert_eq!(Family::SYSTEM.name(), None, "the system's face has no name of its own");
        assert_eq!(Family::named(""), Family::SYSTEM, "and no name IS the system's face");

        // two families are two cache keys — one raster must never
        // answer for the other
        let base = FontSpec::DEFAULT;
        assert_ne!(base.family("Menlo").key(), base.family("Georgia").key());

        // the patch names ONLY the face: a size and a weight on either
        // side of it survive, like every other font modifier
        let over = FontSpec { size: 11.0, weight: Weight::Bold, ..base };
        let patched = FontPatch { family: Some(menlo), ..FontPatch::default() }.apply_over(over);
        assert_eq!(patched.family, menlo);
        assert_eq!(patched.size, 11.0, "the size around it survives");
        assert_eq!(patched.weight, Weight::Bold, "and the weight");

        // and it reaches the engine through the scene: this one gives
        // the named family twice the room, so the frame says whether
        // the face crossed the border
        struct ByFamily;
        impl TextEngine for ByFamily {
            fn measure_line(&self, text: &str, font: &FontSpec) -> LineMetrics {
                let wide = font.family.name().is_some_and(|name| &*name == "Menlo");
                LineMetrics {
                    width: text.chars().count() as f64 * if wide { 20.0 } else { 10.0 },
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

        let runtime = Runtime::new().text_engine(Rc::new(ByFamily));
        let plain = runtime.layout(&text("abcd"), Proposal::unspecified());
        assert_eq!(plain.size, Size { width: 40.0, height: 20.0 });
        let named = runtime.layout(&text("abcd").font_family("Menlo"), Proposal::unspecified());
        assert_eq!(named.size, Size { width: 80.0, height: 20.0 }, "the face crossed");

        // the roster is the ENGINE's word, and an engine with one face
        // answers nothing rather than making a name up
        assert!(runtime.font_families().is_empty());
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
    fn a_size_off_the_scale_keeps_the_rest_of_the_font() {
        use crate::layout::{DrawCommand, Proposal};
        use crate::text_engine::Weight;

        let runtime = Runtime::new();
        // 9pt and 26pt are not on the preset scale; the weight still
        // comes from the font around each one
        let view = vstack((
            text("badge").font_size(9.0).bold(),
            text("mark").font_size(26.0),
        ));
        let result = runtime.layout(&view, Proposal::unspecified());

        let fonts: Vec<(f64, Weight)> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { font, .. } => Some((font.size, font.weight)),
                _ => None,
            })
            .collect();
        assert_eq!(fonts, vec![(9.0, Weight::Bold), (26.0, Weight::Regular)]);
    }

    #[cfg(feature = "canvas")]
    #[test]
    fn a_box_rounds_only_the_corners_it_names() {
        use crate::layout::{Color, Corners, DrawCommand, Proposal, Size};

        // one number still spreads to four — every call that had a
        // radius means exactly what it meant
        assert_eq!(Corners::from(8.0), Corners::all(8.0));
        assert_eq!(Corners::all(8.0).uniform(), Some(8.0));
        assert_eq!(Corners::top(6.0).uniform(), None);

        let runtime = Runtime::new();
        let view = vstack((
            empty().frame(40.0, 20.0).background_color(Color::hex(0x112233))
                .corner_radius(Corners::top(6.0)),
            empty().frame(40.0, 20.0).background_color(Color::hex(0x112233)),
            empty().frame(40.0, 20.0).background_color(Color::hex(0x112233))
                .corner_radius(Corners::bottom(6.0)),
        ));
        let result = runtime.layout(&view, Proposal::unspecified());
        let radii: Vec<Corners> = result
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::FillRect { corner_radius, .. } => Some(*corner_radius),
                _ => None,
            })
            .collect();
        assert_eq!(
            radii,
            vec![Corners::top(6.0), Corners::ZERO, Corners::bottom(6.0)],
            "the band carries the corners that END it, and no others",
        );

        // and the pixels agree: the first band's TOP corner is bitten
        // out while its BOTTOM one is square, so the two bands meet on
        // a full edge
        let sheet = Corners { top_left: 6.0, top_right: 6.0, bottom_right: 0.0, bottom_left: 0.0 };
        let band = empty()
            .frame(40.0, 20.0)
            .background_color(Color::BLACK)
            .corner_radius(sheet)
            .padding_length(4.0);
        let bitmap = runtime.paint(&band, Size { width: 48.0, height: 28.0 });
        let ink = |x: usize, y: usize| bitmap.pixel(x, y) != bitmap.pixel(47, 27);
        assert!(!ink(4, 4), "the top left corner is cut away");
        assert!(!ink(43, 4), "so is the top right");
        assert!(ink(4, 23), "the bottom left is SQUARE — the band continues below");
        assert!(ink(43, 23), "and so is the bottom right");
    }

    #[test]
    fn the_paint_knows_the_screen_scale_and_can_land_on_a_whole_pixel() {
        use crate::custom::{CustomElement, Metrics, PaintCtx, Painter};
        use crate::layout::{Point, Proposal, Rect, Size};
        use crate::custom::custom;
        use std::cell::Cell;
        use std::rc::Rc;

        // a box made of parts that TOUCH: the app puts the shared edge
        // on a whole physical pixel, which needs the screen's scale
        struct Bands {
            seen: Rc<Cell<f64>>,
        }

        impl CustomElement for Bands {
            fn measure(&self, proposal: Proposal, _metrics: &Metrics) -> Size {
                Size {
                    width: proposal.width.unwrap_or(0.0),
                    height: proposal.height.unwrap_or(0.0),
                }
            }

            fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
                self.seen.set(ctx.scale);
                // the hairline the product wants: ONE physical pixel,
                // whatever the screen is worth
                let top = ctx.snap(4.0);
                painter.fill(
                    Rect {
                        origin: Point { x: 0.0, y: top },
                        size: Size { width: ctx.size().width, height: 1.0 / ctx.scale },
                    },
                    crate::layout::Color::BLACK,
                );
            }
        }

        #[derive(Clone, Copy)]
        struct Sheet {
            seen: &'static std::thread::LocalKey<Rc<Cell<f64>>>,
        }

        thread_local! {
            static SEEN: Rc<Cell<f64>> = Rc::new(Cell::new(0.0));
        }

        impl Component for Sheet {
            fn body(self, _ctx: &Context) -> impl View {
                custom(Bands { seen: self.seen.with(Rc::clone) })
            }
        }

        let sheet = Sheet { seen: &SEEN };
        let runtime = Runtime::new();
        runtime.render_stable(&sheet);
        let size = Size { width: 16.0, height: 16.0 };

        // no shell said anything: one point is one pixel
        let plain = runtime.paint(&sheet, size);
        assert_eq!(SEEN.with(|seen| seen.get()), 1.0, "the default scale is 1");
        let rows = |bitmap: &crate::raster::Bitmap, height: usize| {
            (0..height).filter(|&y| bitmap.pixel(0, y) != bitmap.pixel(0, 0)).count()
        };
        assert_eq!(rows(&plain, 16), 1, "a hairline is one row at 1x");

        // a retina screen: the SAME code paints a thinner line, still
        // exactly one physical row — the half point of sharpness the
        // app cannot ask for without the number
        let retina = runtime.paint_at_scale(&sheet, size, 2);
        assert_eq!(SEEN.with(|seen| seen.get()), 2.0, "the shell's scale reaches the paint");
        assert_eq!(retina.height(), 32, "the bitmap is physical");
        assert_eq!(rows(&retina, 32), 1, "still one row, now half a point tall");
        // and it starts where the snap put it: 4pt is physical row 8
        assert_ne!(retina.pixel(0, 8), retina.pixel(0, 0), "the band opens on the snapped row");
        assert_eq!(retina.pixel(0, 9), retina.pixel(0, 0), "and closes after ONE row");
    }

    #[test]
    fn a_snapped_box_keeps_the_edge_two_neighbours_share() {
        use crate::custom::{Metrics, PaintCtx};
        use crate::layout::{Point, Rect, Size};
        use crate::text_engine::{FontSpec, MeasureCache, PixelFont};

        let engine = PixelFont;
        let cache = MeasureCache::default();
        let ctx = PaintCtx {
            frame: Rect { origin: Point::ZERO, size: Size { width: 100.0, height: 100.0 } },
            visible: Rect { origin: Point::ZERO, size: Size { width: 100.0, height: 100.0 } },
            metrics: Metrics::new(&engine, &cache, FontSpec::DEFAULT),
            focused: false,
            caret_visible: false,
            phase: 0.0,
            scale: 2.0,
        };

        // the product's own line, `(v * scale).round() / scale`
        assert_eq!(ctx.snap(10.4), 10.5, "a retina screen keeps the half point");
        assert_eq!(ctx.snap(10.1), 10.0);

        // the LAW of a figure made of parts: snapping moves EDGES, so
        // two boxes that shared one still do. Snapping the size instead
        // would open a gap here — a translucent tint would show it.
        let top = Rect {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size { width: 20.0, height: 10.4 },
        };
        let bottom = Rect {
            origin: Point { x: 0.0, y: 10.4 },
            size: Size { width: 20.0, height: 9.6 },
        };
        let top = ctx.snap_rect(top);
        let bottom = ctx.snap_rect(bottom);
        assert_eq!(
            top.origin.y + top.size.height,
            bottom.origin.y,
            "the seam stays ONE edge",
        );
        assert_eq!(bottom.origin.y, 10.5, "and it sits on a whole pixel");
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
    fn a_sequence_holds_the_keyboard_until_the_second_stroke() {
        use crate::action::KeyMatch;
        use crate::layout::{Proposal, Size};

        const KEYMAP: ActionId = ActionId("workbench.open_keymap");
        const QUICK_DOC: ActionId = ActionId("editor.quick_doc");
        const LONE_K: ActionId = ActionId("app.lone_k");
        const SAVE: ActionId = ActionId("app.save");
        const ZEN: ActionId = ActionId("editor.zen");

        #[derive(Clone, Copy)]
        struct App {
            editing: State<bool>,
        }

        impl Component for App {
            fn body(self, _ctx: &Context) -> impl View {
                let editor = self.editing.get().then(|| text("code").key_context("editor"));
                vstack!(text("bench"), editor)
            }
        }

        let viewport = Proposal::exact(Size { width: 200.0, height: 100.0 });
        let app = App { editing: State::new(false) };
        let runtime = Runtime::new();
        runtime.settle(&app);
        runtime.layout(&app, viewport);

        let k = KeyPattern::command(Key::Char('k'));
        let s_key = KeyPattern::command(Key::Char('s'));
        let i = KeyPattern::command(Key::Char('i'));
        let x = KeyPattern::command(Key::Char('x'));
        let escape = KeyPattern::key(Key::Escape);

        runtime.bind_sequence(&[k, s_key], KEYMAP);
        runtime.bind_sequence_in("editor", &[k, i], QUICK_DOC);
        runtime.bind(s_key, SAVE);
        // a single stroke that is also a PREFIX can never fire: it is
        // the start of something, and the sequence has to win
        runtime.bind(k, LONE_K);

        // the first stroke fires nothing and is spent — the keyboard is
        // held, and a which-key panel can read what is on offer
        assert_eq!(runtime.chord(&k), KeyMatch::Pending);
        assert_eq!(runtime.pending_chord(), vec![k]);
        // the second completes it
        assert_eq!(runtime.chord(&s_key), KeyMatch::Action(KEYMAP));
        assert!(runtime.pending_chord().is_empty(), "and the sequence is over");

        // that same second stroke on its OWN is the single binding
        assert_eq!(runtime.chord(&s_key), KeyMatch::Action(SAVE));

        // a sequence that leads nowhere ends, and SPENDS its last
        // stroke: re-reading it fresh is how an editor fires something
        // nobody asked for
        assert_eq!(runtime.chord(&k), KeyMatch::Pending);
        assert_eq!(runtime.chord(&x), KeyMatch::None);
        assert!(runtime.pending_chord().is_empty());

        // Escape is the way out, and it CONSUMES — a chord abandoned
        // must not also close the palette standing behind it
        assert_eq!(runtime.chord(&k), KeyMatch::Pending);
        assert_eq!(runtime.chord(&escape), KeyMatch::Pending);
        assert!(runtime.pending_chord().is_empty());
        // with nothing in the air, Escape is the app's again
        runtime.bind(escape, ZEN);
        assert_eq!(runtime.chord(&escape), KeyMatch::Action(ZEN));

        // the hand leaving for the pointer drops it too
        assert_eq!(runtime.chord(&k), KeyMatch::Pending);
        runtime.pointer_pressed(10.0, 10.0);
        assert!(runtime.pending_chord().is_empty(), "a press ends the chord");

        // and the slow clock ages it out: the SECOND tick drops it, the
        // tooltip's own idiom
        assert_eq!(runtime.chord(&k), KeyMatch::Pending);
        assert!(!runtime.chord_tick(), "one tick only ages it");
        assert_eq!(runtime.pending_chord(), vec![k]);
        assert!(runtime.chord_tick(), "the second lets the keyboard go");
        assert!(runtime.pending_chord().is_empty());
        assert!(!runtime.chord_tick(), "and an empty hand ticks for free");

        // a scoped sequence answers only while its context is mounted
        assert_eq!(runtime.chord(&k), KeyMatch::Pending);
        assert_eq!(runtime.chord(&i), KeyMatch::None, "the editor is not up");
        app.editing.set(true);
        runtime.settle(&app);
        runtime.layout(&app, viewport);
        assert_eq!(runtime.chord(&k), KeyMatch::Pending);
        assert_eq!(runtime.chord(&i), KeyMatch::Action(QUICK_DOC));

        // and emptying the table takes the sequences with it
        runtime.clear_bindings();
        assert_eq!(runtime.chord(&k), KeyMatch::None, "no prefix left to hold");
        assert!(runtime.pending_chord().is_empty());
    }

    #[test]
    fn the_key_table_can_be_emptied_so_a_cascade_re_installs() {
        use crate::layout::{Proposal, Size};

        const SAVE: ActionId = ActionId("app.save");
        const OLD: ActionId = ActionId("app.removed_by_the_user");

        #[derive(Clone, Copy)]
        struct App {
            card: State<bool>,
        }
        impl Component for App {
            fn body(self, _ctx: &Context) -> impl View {
                text("editor").key_context("editor").popover(
                    self.card.binding(),
                    crate::layout::Side::Trailing,
                    |_| erased(text("card")),
                )
            }
        }

        let viewport = Proposal::exact(Size { width: 200.0, height: 100.0 });
        let app = App { card: State::new(true) };
        let runtime = Runtime::new();
        runtime.settle(&app);
        runtime.layout(&app, viewport);

        // a cascade goes in: one global layer and one scoped
        let save = KeyPattern::command(Key::Char('s'));
        let old = KeyPattern::command(Key::Char('j'));
        runtime.bind(save, SAVE);
        runtime.bind(old, OLD);
        runtime.bind_in("editor", old, OLD);
        assert_eq!(runtime.match_key(&save), Some(SAVE));
        assert_eq!(runtime.match_key(&old), Some(OLD));

        // the user edits the keymap and the host re-installs it. The
        // binding the edit REMOVED has nothing to overwrite it, so
        // only an empty table can make it go
        runtime.clear_bindings();
        runtime.bind(save, SAVE);
        assert_eq!(runtime.match_key(&save), Some(SAVE), "the layer went back in");
        assert_eq!(runtime.match_key(&old), None, "and what the edit dropped is gone");

        // the house's own context stood through it: a popover is still
        // dismissible, which is not the app's to take away
        assert_eq!(
            runtime.match_key(&KeyPattern::key(Key::Escape)),
            Some(crate::action::OVERLAY_DISMISS),
            "the reserved context survives an app's clear",
        );

        // and one layer can be re-stacked on its own
        runtime.bind_in("editor", old, OLD);
        assert_eq!(runtime.match_key(&old), Some(OLD));
        runtime.clear_bindings_in("editor");
        assert_eq!(runtime.match_key(&old), None);
        assert_eq!(runtime.match_key(&save), Some(SAVE), "the other layers stayed");
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

    #[cfg(feature = "canvas")]
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
        let first = surface.frame(runtime.layout(&Rows, viewport).display, &PixelFont, &RawImages::default());
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
        runtime.pointer_moved(target.origin.x + 4.0, target.origin.y + 4.0, false);
        let result = runtime.layout(&Rows, viewport);
        let oracle = rasterize_scaled(&result.display, 120, 80, 1, Color::CANVAS);
        let damage = surface.frame(result.display, &PixelFont, &RawImages::default());

        assert_eq!(surface.bitmap().pixels(), oracle.pixels(), "golden: incremental == full");
        assert_eq!(damage.len(), 1, "one row hovered, one rect: {damage:?}");
        let (_, y0, _, y1) = damage[0];
        let row_height = (y1 - y0) as f64;
        assert!(
            row_height <= target.size.height + 4.0,
            "damage is row-sized ({row_height}px tall), not the window"
        );
    }

    // MARK: - Window controls

    #[test]
    fn a_window_control_marks_the_button_and_wins_by_design() {
        use crate::layout::WindowControl;
        #[derive(Clone)]
        struct Crowned;
        impl Component for Crowned {
            fn body(self, _ctx: &Context) -> impl View {
                vstack!(
                    hstack!(
                        text("title"),
                        spacer(),
                        text("x")
                            .frame(46.0, 40.0)
                            .window_control(WindowControl::Close),
                    )
                    .frame(200.0, 40.0)
                    .window_drag_region(),
                    spacer().frame(200.0, 160.0),
                )
            }
        }
        let runtime = Runtime::new();
        let result = runtime.layout(
            &Crowned,
            crate::layout::Proposal::exact(crate::layout::Size {
                width: 200.0,
                height: 200.0,
            }),
        );
        assert_eq!(result.control_regions.len(), 1, "one button, one region");
        let (control, region) = result.control_regions[0];
        assert_eq!(control, WindowControl::Close);
        assert_eq!((region.size.width, region.size.height), (46.0, 40.0));

        // inside the button the control answers; outside it is silent;
        // the bar around it still drags
        let center =
            (region.origin.x + region.size.width / 2.0, region.origin.y + region.size.height / 2.0);
        assert_eq!(runtime.window_control_at(center.0, center.1), Some(WindowControl::Close));
        assert_eq!(runtime.window_control_at(10.0, 20.0), None, "the bare bar is no button");
        assert!(runtime.window_drag_at(10.0, 20.0), "the bar still drags around it");
        assert_eq!(runtime.window_control_at(100.0, 120.0), None, "the body is no button");
    }

    // MARK: - Window drag regions

    #[test]
    fn a_drag_region_reports_its_frame_and_yields_to_buttons() {
        #[derive(Clone)]
        struct Barred;
        impl Component for Barred {
            fn body(self, _ctx: &Context) -> impl View {
                vstack!(
                    hstack!(text("title"), spacer(), button(text("act"), || {}))
                        .frame(200.0, 40.0)
                        .window_drag_region(),
                    spacer().frame(200.0, 160.0),
                )
            }
        }
        let runtime = Runtime::new();
        let result = runtime.layout(
            &Barred,
            crate::layout::Proposal::exact(crate::layout::Size {
                width: 200.0,
                height: 200.0,
            }),
        );
        assert_eq!(result.drag_regions.len(), 1, "one bar, one region");
        let bar = result.drag_regions[0];
        assert_eq!((bar.size.width, bar.size.height), (200.0, 40.0));

        // empty bar space drags; the button on it still clicks; the
        // body below never drags
        assert!(runtime.window_drag_at(10.0, 20.0), "bare bar drags");
        let (_, button_rect) = result.hits.last().expect("the bar's button").clone();
        assert!(
            !runtime.window_drag_at(
                button_rect.origin.x + button_rect.size.width / 2.0,
                button_rect.origin.y + button_rect.size.height / 2.0,
            ),
            "an interactive target wins over the drag"
        );
        assert!(!runtime.window_drag_at(100.0, 120.0), "the body is not a handle");
    }

    // MARK: - Popovers

    use crate::action::{Key, KeyPattern, OVERLAY_DISMISS};
    use crate::layout::Rect;

    #[derive(Clone)]
    struct Anchored {
        open: State<bool>,
        side: Side,
        /// Height of the filler ABOVE the anchor — moves it around.
        above: f64,
    }

    impl Component for Anchored {
        fn body(self, _ctx: &Context) -> impl View {
            vstack!(
                spacer().frame(180.0, self.above),
                spacer().frame(20.0, 20.0).popover(self.open.binding(), self.side, |_| {
                    erased(spacer().frame(40.0, 30.0))
                }),
            )
        }
    }

    fn opened(side: Side, above: f64) -> (Runtime, Anchored) {
        let runtime = Runtime::new();
        let view = Anchored { open: State::new(true), side, above };
        (runtime, view)
    }

    fn window() -> crate::layout::Proposal {
        crate::layout::Proposal::exact(crate::layout::Size { width: 200.0, height: 200.0 })
    }

    #[test]
    fn a_popover_hangs_off_every_side_with_the_gap() {
        // ONE runtime, the side in state: retention is thread-local by
        // identity, so a fresh runtime with the same component type
        // would keep the FIRST side's tree — the set marks dirty and
        // the body re-runs for real
        #[derive(Clone)]
        struct Turning {
            side: State<usize>,
        }
        impl Component for Turning {
            fn body(self, _ctx: &Context) -> impl View {
                let side =
                    [Side::Bottom, Side::Top, Side::Trailing, Side::Leading][self.side.get()];
                vstack!(
                    spacer().frame(180.0, 80.0),
                    spacer().frame(20.0, 20.0).popover(
                        State::new(true).binding(),
                        side,
                        |_| erased(spacer().frame(40.0, 30.0)),
                    ),
                )
            }
        }
        let runtime = Runtime::new();
        let view = Turning { side: State::new(0) };
        // the anchor sits at (80, 80)–(100, 100); the overlay is 40×30
        let frame = |index: usize| {
            view.side.set(index);
            let result = runtime.layout(&view, window());
            assert_eq!(result.overlays.len(), 1, "one open popover");
            result.overlays[0].frame
        };
        let bottom = frame(0);
        assert_eq!((bottom.origin.x, bottom.origin.y), (70.0, 106.0), "below, centered");
        let top = frame(1);
        assert_eq!((top.origin.x, top.origin.y), (70.0, 44.0), "above, centered");
        let trailing = frame(2);
        assert_eq!((trailing.origin.x, trailing.origin.y), (106.0, 75.0), "after, centered");
        let leading = frame(3);
        assert_eq!((leading.origin.x, leading.origin.y), (34.0, 75.0), "before, centered");
    }

    #[test]
    fn a_popover_flips_when_its_side_has_no_room() {
        // anchor at y 160–180: below wants 186..216, past the window —
        // the frame flips above instead
        let (runtime, view) = opened(Side::Bottom, 160.0);
        let result = runtime.layout(&view, window());
        assert_eq!(result.overlays[0].frame.origin.y, 124.0, "flipped above the anchor");
    }

    #[test]
    fn a_popover_clamps_inside_its_container() {
        let runtime = Runtime::new();
        let view = {
            #[derive(Clone)]
            struct Wide {
                open: State<bool>,
            }
            impl Component for Wide {
                fn body(self, _ctx: &Context) -> impl View {
                    vstack!(
                        spacer().frame(180.0, 80.0),
                        spacer().frame(20.0, 20.0).popover(
                            self.open.binding(),
                            Side::Bottom,
                            |_| erased(spacer().frame(190.0, 30.0)),
                        ),
                    )
                }
            }
            Wide { open: State::new(true) }
        };
        let result = runtime.layout(&view, window());
        // centered would start at -5; the clamp holds the left edge
        assert_eq!(result.overlays[0].frame.origin.x, 0.0, "clamped to the container");
    }

    #[test]
    fn overlay_bounds_let_a_popover_leave_the_window() {
        // the mac shape, headless: the container is the SCREEN, larger
        // than the window — the same anchor no longer flips, it spills
        let (runtime, view) = opened(Side::Bottom, 160.0);
        runtime.set_overlay_bounds(Some(Rect {
            origin: crate::layout::Point { x: -100.0, y: -100.0 },
            size: crate::layout::Size { width: 400.0, height: 400.0 },
        }));
        let result = runtime.layout(&view, window());
        let frame = result.overlays[0].frame;
        assert_eq!(frame.origin.y, 186.0, "no flip — the screen has room");
        assert!(
            frame.origin.y + frame.size.height > 200.0,
            "the popover overflows the window: {frame:?}"
        );
    }

    #[test]
    fn a_popover_paints_on_top_and_wins_the_hit() {
        #[derive(Clone)]
        struct Covered {
            open: State<bool>,
        }
        impl Component for Covered {
            fn body(self, _ctx: &Context) -> impl View {
                zstack!(
                    button(text("under"), || {}).frame(120.0, 120.0),
                    spacer().frame(20.0, 20.0).popover(self.open.binding(), Side::Bottom, |_| {
                        erased(button(text("over"), || {}).frame(60.0, 30.0))
                    }),
                )
            }
        }
        let runtime = Runtime::new();
        let view = Covered { open: State::new(true) };
        let result = runtime.layout(&view, window());

        let overlay = &result.overlays[0];
        // the slice is a SUFFIX of the display list
        assert_eq!(overlay.display.1, result.display.len(), "the popover paints last");
        assert!(overlay.display.0 < overlay.display.1, "and it painted something");
        // the hit inside the overlay resolves to the popover's button
        let inside = (
            overlay.frame.origin.x + overlay.frame.size.width / 2.0,
            overlay.frame.origin.y + overlay.frame.size.height / 2.0,
        );
        let path = crate::layout::hit_test(&result.hits, inside.0, inside.1)
            .expect("the popover is clickable");
        assert!(path.contains("popover"), "the popover's own button wins: {path}");
    }

    /// A list whose FIRST row anchors a popover — the scroll shape of
    /// the scroll tests.
    #[derive(Clone)]
    struct RowAnchored {
        open: State<bool>,
    }

    impl Component for RowAnchored {
        fn body(self, _ctx: &Context) -> impl View {
            let open = self.open;
            vstack!(
                list(
                    (0..20).collect::<Vec<_>>(),
                    |row| row.to_string(),
                    move |row| {
                        if *row == 0 {
                            erased(spacer().frame(20.0, 20.0).popover(
                                open.binding(),
                                Side::Trailing,
                                |_| {
                                    erased(
                                        spacer()
                                            .frame(20.0, 20.0)
                                            .background_color(Color::hex(0x123456)),
                                    )
                                },
                            ))
                        } else {
                            erased(spacer().frame(120.0, 20.0))
                        }
                    },
                )
                .frame(160.0, 60.0)
            )
        }
    }

    #[test]
    fn a_popover_escapes_a_scroll_clip() {
        let runtime = Runtime::new();
        let view = RowAnchored { open: State::new(true) };
        let result = runtime.layout(&view, window());
        let overlay = &result.overlays[0];
        // the popover paints, and its slice sits AFTER every clip of
        // the walk — the scroll viewport cannot cut it
        assert!(overlay.display.0 < overlay.display.1, "the popover painted");
        let clips_before_overlay = result.display.as_slice()[..overlay.display.0]
            .iter()
            .fold(0i32, |depth, command| match command {
                crate::layout::DrawCommand::PushClip { .. } => depth + 1,
                crate::layout::DrawCommand::PopClip => depth - 1,
                _ => depth,
            });
        assert_eq!(clips_before_overlay, 0, "no clip is open where the popover paints");
    }

    #[test]
    fn a_press_outside_closes_and_consumes() {
        let (runtime, view) = opened(Side::Bottom, 80.0);
        let _ = runtime.layout(&view, window());

        // outside the popover (and over nothing clickable): closes
        assert!(runtime.pointer_pressed(5.0, 5.0), "the press repaints");
        assert!(!view.open.get(), "the popover closed");
        // the press was CONSUMED: nothing armed, so the release fires
        // nothing either
        assert_eq!(runtime.pointer_released(5.0, 5.0), None);

        // reopen; a press INSIDE stays open
        view.open.set(true);
        let result = runtime.layout(&view, window());
        let frame = result.overlays[0].frame;
        runtime.pointer_pressed(
            frame.origin.x + frame.size.width / 2.0,
            frame.origin.y + frame.size.height / 2.0,
        );
        assert!(view.open.get(), "a press inside never dismisses");
    }

    #[test]
    fn escape_closes_from_the_inside_out() {
        #[derive(Clone)]
        struct Nested {
            outer: State<bool>,
            inner: State<bool>,
        }
        impl Component for Nested {
            fn body(self, _ctx: &Context) -> impl View {
                let inner = self.inner;
                vstack!(
                    spacer().frame(180.0, 80.0),
                    spacer().frame(20.0, 20.0).popover(
                        self.outer.binding(),
                        Side::Bottom,
                        move |_| {
                            erased(
                                spacer()
                                    .frame(30.0, 20.0)
                                    .popover(inner.binding(), Side::Trailing, |_| {
                                        erased(spacer().frame(20.0, 20.0))
                                    }),
                            )
                        },
                    ),
                )
            }
        }
        let runtime = Runtime::new();
        let view =
            Nested { outer: State::new(true), inner: State::new(true) };
        let result = runtime.layout(&view, window());
        assert_eq!(result.overlays.len(), 2, "both popovers open");

        let escape = KeyPattern::key(Key::Escape);
        let action = runtime.match_key(&escape).expect("escape binds while a popover is open");
        assert_eq!(action, OVERLAY_DISMISS);
        assert!(runtime.dispatch_action(action));
        assert!(!view.inner.get(), "the INNERMOST closes first");
        assert!(view.outer.get(), "the outer stays");

        let _ = runtime.layout(&view, window());
        assert!(runtime.dispatch_action(runtime.match_key(&escape).expect("still bound")));
        assert!(!view.outer.get(), "the second escape closes the outer");

        // with nothing open, escape stops matching (the context died)
        let _ = runtime.layout(&view, window());
        assert_eq!(runtime.match_key(&escape), None, "no popover, no binding");
    }

    #[test]
    fn a_scrolled_away_anchor_dismisses_its_popover() {
        let runtime = Runtime::new();
        let view = RowAnchored { open: State::new(true) };
        let result = runtime.layout(&view, window());
        assert!(view.open.get());

        // roll the anchor out of the viewport; the follow-up closes it
        let region = result.scrolls.first().expect("a scroll region").path.clone();
        runtime.set_scroll_offset(&region, crate::layout::Point { x: 0.0, y: 300.0 });
        let result = runtime.layout(&view, window());
        assert!(!view.open.get(), "the orphaned popover closed");
        assert!(result.overlays.is_empty(), "and the relayout dropped it");
    }

    #[test]
    fn dismissal_tells_the_app_through_every_door() {
        #[derive(Clone)]
        struct Told {
            open: State<bool>,
            told: State<usize>,
        }
        impl Component for Told {
            fn body(self, _ctx: &Context) -> impl View {
                let told = self.told;
                vstack!(
                    spacer().frame(180.0, 80.0),
                    spacer().frame(20.0, 20.0).popover_on_dismiss(
                        self.open.binding(),
                        Side::Bottom,
                        move || told.set(told.get() + 1),
                        |_| erased(spacer().frame(40.0, 30.0)),
                    ),
                )
            }
        }
        let runtime = Runtime::new();
        let view = Told { open: State::new(true), told: State::new(0) };
        let _ = runtime.layout(&view, window());

        // door one: the outside press
        runtime.pointer_pressed(5.0, 5.0);
        assert_eq!(view.told.get(), 1);
        assert!(!view.open.get());

        // door two: escape
        view.open.set(true);
        let _ = runtime.layout(&view, window());
        let escape = KeyPattern::key(Key::Escape);
        assert!(runtime.dispatch_action(runtime.match_key(&escape).expect("bound")));
        assert_eq!(view.told.get(), 2);

        // door three: the app switch — every open popover closes
        view.open.set(true);
        let _ = runtime.layout(&view, window());
        assert!(runtime.dismiss_all_overlays());
        assert_eq!(view.told.get(), 3);
        assert!(!view.open.get());

        // the app clearing the binding itself does not count — it
        // already knows
        view.open.set(true);
        let _ = runtime.layout(&view, window());
        view.open.set(false);
        let _ = runtime.layout(&view, window());
        assert_eq!(view.told.get(), 3, "self-service closing is not a dismissal");
    }

    #[test]
    fn a_closed_popover_costs_nothing() {
        let closed = {
            let (runtime, view) = opened(Side::Bottom, 80.0);
            view.open.set(false);
            runtime.layout(&view, window()).display
        };
        let bare = {
            #[derive(Clone)]
            struct Bare;
            impl Component for Bare {
                fn body(self, _ctx: &Context) -> impl View {
                    vstack!(spacer().frame(180.0, 80.0), spacer().frame(20.0, 20.0))
                }
            }
            Runtime::new().layout(&Bare, window()).display
        };
        assert_eq!(closed.as_slice(), bare.as_slice(), "closed = not there, byte for byte");
    }

    // MARK: - Images

    #[test]
    fn the_image_measure_table_holds() {
        use crate::layout::{LayoutNode, Proposal, Size, layout};
        let wide = ImageSource::from_bytes(RawImages::encode(4, 2, &[255u8; 32]));
        let node = |resizable: bool, fit: Option<ContentMode>| LayoutNode::Image {
            source: Some(wide.clone()),
            resizable,
            fit,
        };
        let size = |node: &LayoutNode, width: Option<f64>, height: Option<f64>| {
            layout(node, Proposal { width, height }).size
        };

        // rigid: the intrinsic size, 1 pixel = 1 point, proposal ignored
        assert_eq!(
            size(&node(false, None), Some(100.0), Some(100.0)),
            Size { width: 4.0, height: 2.0 }
        );
        // resizable stretches to the box; an open axis stays intrinsic
        assert_eq!(
            size(&node(true, None), Some(100.0), Some(50.0)),
            Size { width: 100.0, height: 50.0 }
        );
        assert_eq!(
            size(&node(true, None), Some(100.0), None),
            Size { width: 100.0, height: 2.0 }
        );
        // Fit contains, keeping the ratio; an open axis derives
        assert_eq!(
            size(&node(true, Some(ContentMode::Fit)), Some(100.0), Some(100.0)),
            Size { width: 100.0, height: 50.0 }
        );
        assert_eq!(
            size(&node(true, Some(ContentMode::Fit)), Some(100.0), None),
            Size { width: 100.0, height: 50.0 }
        );
        assert_eq!(
            size(&node(true, Some(ContentMode::Fit)), None, None),
            Size { width: 4.0, height: 2.0 }
        );
        // Fill answers the box exactly (the paint covers and clips)
        assert_eq!(
            size(&node(true, Some(ContentMode::Fill)), Some(60.0), Some(60.0)),
            Size { width: 60.0, height: 60.0 }
        );
        // undecoded measures zero on every path — reflow on ready
        let broken = ImageSource::from_bytes(&b"junk"[..]);
        let pending =
            LayoutNode::Image { source: Some(broken), resizable: true, fit: Some(ContentMode::Fit) };
        assert_eq!(size(&pending, Some(100.0), Some(100.0)), Size::default());
        // the stub keeps the classic rigid box
        let stub = LayoutNode::Image { source: None, resizable: false, fit: None };
        assert_eq!(
            size(&stub, Some(100.0), Some(100.0)),
            Size { width: 40.0, height: 40.0 }
        );
    }

    #[test]
    fn an_async_engine_reflows_when_it_reports_in() {
        use crate::image_engine::{ImageEngine, ImageRaster, ImageSource};
        use crate::layout::{Proposal, Size};

        // the web shape: intrinsic answers None until the platform
        // reports in — the same scene then measures for real
        struct Late {
            ready: std::cell::Cell<bool>,
        }
        impl ImageEngine for Late {
            fn intrinsic(&self, _: &ImageSource) -> Option<(u32, u32)> {
                self.ready.get().then_some((30, 10))
            }
            fn raster(&self, _: &ImageSource, _: usize, _: usize) -> Option<Rc<ImageRaster>> {
                None
            }
        }

        let engine = Rc::new(Late { ready: std::cell::Cell::new(false) });
        let runtime = Runtime::new().image_engine(engine.clone());
        let root = image(ImageSource::bytes_keyed(1, &b"pending"[..]))
            .resizable()
            .aspect_ratio(ContentMode::Fit);

        let proposal = Proposal { width: Some(60.0), height: None };
        let before = runtime.layout(&root, proposal);
        assert_eq!(before.size, Size::default(), "undecoded measures zero");

        engine.ready.set(true);
        let after = runtime.layout(&root, proposal);
        assert_eq!(
            after.size,
            Size { width: 60.0, height: 20.0 },
            "the ready callback reflows to the real ratio"
        );
    }

    #[cfg(feature = "canvas")]
    #[test]
    fn a_fill_image_covers_and_clips_inside_its_frame() {
        use crate::layout::{DrawCommand, LayoutNode, Proposal, layout};
        // a wide red 4×2 inside a TALL 20×40 frame: cover scales by
        // height (×20) and spills horizontally — the built-in clip eats
        // the spill, no `.clipped()` anywhere
        let red: Vec<u8> = [[255u8, 0, 0, 255]; 8].concat();
        let source = ImageSource::from_bytes(RawImages::encode(4, 2, &red));
        let node = LayoutNode::Frame {
            width: Some(20.0),
            height: Some(40.0),
            child: Box::new(LayoutNode::Image {
                source: Some(source),
                resizable: true,
                fit: Some(ContentMode::Fill),
            }),
        };
        let result = layout(&node, Proposal { width: Some(60.0), height: Some(40.0) });
        let rect = result
            .display
            .iter()
            .find_map(|command| match command {
                DrawCommand::Image { rect, .. } => Some(*rect),
                _ => None,
            })
            .expect("the image painted");
        assert_eq!((rect.size.width, rect.size.height), (80.0, 40.0), "cover spills: {rect:?}");
        assert!(rect.origin.x < 0.0, "spills left of the frame: {rect:?}");

        let bitmap =
            crate::raster::rasterize_scaled(&result.display, 60, 40, 1, Color::WHITE);
        assert_eq!(bitmap.pixel(10, 20), Some(0xFF00_00FF), "inside the frame: red");
        assert_eq!(bitmap.pixel(30, 20), Some(0xFFFF_FFFF), "outside the frame: canvas");
    }

    #[test]
    fn a_tooltip_waits_two_beats_and_shows() {
        use crate::layout::TOOLTIP_PATH;

        #[derive(Clone, Copy)]
        struct Rail;
        impl Component for Rail {
            fn body(self, _ctx: &Context) -> impl View {
                vstack!(
                    text("gear").tooltip("Settings"),
                    text("below"),
                )
                .spacing(20.0)
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 200.0, height: 120.0 };
        let overlay_count = |runtime: &Runtime| {
            runtime
                .layout(&Rail, crate::layout::Proposal::exact(size))
                .overlays
                .iter()
                .filter(|overlay| overlay.path == TOOLTIP_PATH)
                .count()
        };
        // prime the retained geometry, then hover the labelled view
        let _ = runtime.layout(&Rail, crate::layout::Proposal::exact(size));
        runtime.pointer_moved(10.0, 8.0, false);
        assert_eq!(overlay_count(&runtime), 0, "no bubble before the wait");
        // the first beat only ages the wait
        assert!(!runtime.tooltip_tick(), "one beat is not the delay");
        assert_eq!(overlay_count(&runtime), 0);
        // the second beat shows — and asks for a repaint
        assert!(runtime.tooltip_tick(), "the second beat shows the bubble");
        let result = runtime.layout(&Rail, crate::layout::Proposal::exact(size));
        let bubble = result
            .overlays
            .iter()
            .find(|overlay| overlay.path == TOOLTIP_PATH)
            .expect("the bubble is an overlay");
        // below the anchor, past the gap, painted at the very end
        assert!(bubble.frame.origin.y > 8.0);
        assert_eq!(bubble.display.1, result.display.len());
        let says: Vec<String> = result
            .display
            .iter()
            .skip(bubble.display.0)
            .take(bubble.display.1 - bubble.display.0)
            .filter_map(|command| match command {
                crate::layout::DrawCommand::TextLine { content, .. } => {
                    Some(content.to_string())
                }
                _ => None,
            })
            .collect();
        assert_eq!(says, vec!["Settings".to_string()]);

        // moving OFF the anchor takes the bubble with it
        assert!(runtime.pointer_moved(10.0, 60.0, false), "leaving repaints");
        assert_eq!(overlay_count(&runtime), 0, "the bubble left with the pointer");
    }

    #[test]
    fn a_tooltip_never_eats_the_click() {
        #[derive(Clone)]
        struct Two {
            count: State<usize>,
        }
        impl Component for Two {
            fn body(self, _ctx: &Context) -> impl View {
                let count = self.count;
                vstack!(
                    text("hover me").tooltip("An explanation"),
                    text("press me").on_click(move || count.set(count.get() + 1)),
                )
                .spacing(30.0)
            }
        }

        let runtime = Runtime::new();
        let view = Two { count: State::new(0) };
        let size = Size { width: 200.0, height: 120.0 };
        let _ = runtime.layout(&view, crate::layout::Proposal::exact(size));
        // show the bubble over the first line
        runtime.pointer_moved(10.0, 8.0, false);
        runtime.tooltip_tick();
        assert!(runtime.tooltip_tick(), "the bubble is up");
        // pressing the OTHER line must still arm and fire — a popover
        // would have eaten this press; the tooltip must not
        let button = runtime
            .layout(&view, crate::layout::Proposal::exact(size))
            .hits
            .last()
            .map(|(_, rect)| {
                (rect.origin.x + rect.size.width / 2.0, rect.origin.y + rect.size.height / 2.0)
            })
            .expect("the button is a target");
        runtime.pointer_pressed(button.0, button.1);
        runtime.pointer_released(button.0, button.1);
        assert_eq!(view.count.get(), 1, "the press reached the button");
    }

    #[test]
    fn a_press_or_a_wheel_ends_the_wait() {
        #[derive(Clone, Copy)]
        struct One;
        impl Component for One {
            fn body(self, _ctx: &Context) -> impl View {
                text("label").tooltip("Explains")
            }
        }
        let runtime = Runtime::new();
        let size = Size { width: 100.0, height: 40.0 };
        let _ = runtime.layout(&One, crate::layout::Proposal::exact(size));
        runtime.pointer_moved(10.0, 8.0, false);
        runtime.pointer_pressed(10.0, 8.0);
        assert!(!runtime.tooltip_tick(), "a press ends the wait");
        assert!(!runtime.tooltip_tick());
        runtime.pointer_released(10.0, 8.0);
        runtime.pointer_moved(10.0, 8.0, false);
        runtime.wheel(10.0, 8.0, 0.0, 3.0);
        assert!(!runtime.tooltip_tick(), "a wheel ends the wait");
        assert!(!runtime.tooltip_tick());
    }

    #[test]
    fn a_tooltip_survives_the_modifier_stack() {
        // the exact shape the demos write: paint modifiers BETWEEN the
        // view and the tooltip — the attribute must still land
        #[derive(Clone, Copy)]
        struct Chevron;
        impl Component for Chevron {
            fn body(self, _ctx: &Context) -> impl View {
                icon(symbol::CHEVRON_RIGHT)
                    .foreground_color(Color::hex(0x3B82F6))
                    .tooltip("The selected file opens here")
            }
        }
        let runtime = Runtime::new();
        let patches = runtime.dom_frame(&Chevron, Size { width: 100.0, height: 40.0 });
        let landed = patches.iter().any(|patch| {
            matches!(patch, crate::dom::DomPatch::SetStyle { style, .. } if style.tooltip.is_some())
        });
        assert!(landed, "the attribute lands under paint modifiers: {patches:#?}");
    }

    #[test]
    fn in_element_mode_the_browser_owns_the_fade_and_the_group() {
        #[derive(Clone, Copy)]
        struct Chip;
        impl Component for Chip {
            fn body(self, _ctx: &Context) -> impl View {
                hstack((
                    text("file.rs"),
                    text("x").opacity(0.0).opacity_hovered(1.0).group_hovered().on_click(|| {}),
                ))
                .on_click(|| {})
                .hover_group()
            }
        }
        let runtime = Runtime::new();
        let size = Size { width: 300.0, height: 40.0 };
        let patches = runtime.dom_frame(&Chip, size);
        let faded = patches
            .iter()
            .find_map(|patch| match patch {
                crate::dom::DomPatch::SetStyle { style, .. } if style.opacity.is_some() => {
                    Some(style.clone())
                }
                _ => None,
            })
            .expect("the fade reaches the element");
        assert_eq!(faded.opacity, Some(0.0));
        assert_eq!(faded.hover_opacity, Some(1.0));
        // the mark names the ancestor whose :hover drives it — and that
        // ancestor is a real box in the scene, which is the one thing a
        // selector can name
        let group = faded.group.expect("the mark carries its group");
        let owner = patches.iter().any(|patch| {
            matches!(patch, crate::dom::DomPatch::SetStyle { style, .. }
                if style.group_owner == Some(group))
        });
        assert!(owner, "the group owns a box of its own: {patches:#?}");

        // and the LAW holds: hovering the chip moves nothing — the
        // browser flips the rules on its own
        let hit = runtime
            .layout(&Chip, crate::layout::Proposal::exact(size))
            .hits
            .first()
            .expect("the chip is a target")
            .1;
        assert!(runtime.pointer_moved(hit.origin.x + 2.0, hit.origin.y + 2.0, false));
        assert_eq!(runtime.dom_frame(&Chip, size), vec![], "a group hover is zero patches");
    }

    #[test]
    fn in_element_mode_the_browser_owns_the_tooltip() {
        #[derive(Clone, Copy)]
        struct Labelled;
        impl Component for Labelled {
            fn body(self, _ctx: &Context) -> impl View {
                text("gear").tooltip("Settings")
            }
        }
        let runtime = Runtime::new();
        let size = Size { width: 100.0, height: 40.0 };
        let patches = runtime.dom_frame(&Labelled, size);
        let style = patches
            .iter()
            .find_map(|patch| match patch {
                crate::dom::DomPatch::SetStyle { style, .. } if style.tooltip.is_some() => {
                    Some(style.clone())
                }
                _ => None,
            })
            .expect("the text lands as a data attribute");
        assert_eq!(style.tooltip.as_deref(), Some("Settings"));
        // and the wire says so: bit 15, u16 len + utf8 at the tail
        let bytes = crate::dom::encode(&[crate::dom::DomPatch::SetStyle {
            id: 3,
            style: crate::dom::DomStyle {
                tooltip: Some(std::sync::Arc::from("Hi")),
                ..crate::dom::DomStyle::default()
            },
        }]);
        let expected: Vec<u8> = [
            &1u32.to_le_bytes()[..],
            &[5],
            &3u32.to_le_bytes()[..],
            &0x8000u32.to_le_bytes()[..],
            &2u16.to_le_bytes()[..],
            b"Hi",
        ]
        .concat();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn a_right_press_offers_the_menu_and_a_row_fires_on_the_down() {
        use crate::layout::MENU_PATH;

        #[derive(Clone)]
        struct Row {
            opened: State<usize>,
        }
        impl Component for Row {
            fn body(self, _ctx: &Context) -> impl View {
                let opened = self.opened;
                vstack!(
                    text("file_0001.rs").context_menu(vec![
                        menu_item("Open", move || opened.set(opened.get() + 1)),
                        menu_divider(),
                        menu_item("Delete", || {}),
                    ]),
                    text("below"),
                )
                .spacing(30.0)
            }
        }

        let runtime = Runtime::new();
        let view = Row { opened: State::new(0) };
        let size = Size { width: 300.0, height: 200.0 };
        let _ = runtime.layout(&view, crate::layout::Proposal::exact(size));

        // a right press outside every region changes nothing
        assert!(!runtime.context_click(200.0, 150.0));

        // inside: the menu opens at the pointer
        assert!(runtime.context_click(30.0, 8.0));
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        let menu = result
            .overlays
            .iter()
            .find(|overlay| overlay.path == MENU_PATH)
            .expect("the menu is an overlay");
        assert_eq!(menu.frame.origin.x, 30.0);
        assert_eq!(menu.frame.origin.y, 8.0);
        let labels: Vec<String> = result
            .display
            .iter()
            .skip(menu.display.0)
            .filter_map(|command| match command {
                crate::layout::DrawCommand::TextLine { content, .. } => {
                    Some(content.to_string())
                }
                _ => None,
            })
            .collect();
        assert_eq!(labels, vec!["Open".to_string(), "Delete".to_string()]);

        // hovering the first row highlights it and quiets the scene
        let row_y = menu.frame.origin.y + 5.0 + 12.0;
        runtime.pointer_moved(menu.frame.origin.x + 20.0, row_y, false);
        let stamped = runtime.interaction();
        assert_eq!(stamped.menu.as_ref().and_then(|open| open.hovered), Some(0));
        assert_eq!(stamped.hovered, None, "the scene under the menu goes quiet");

        // the pick fires ON THE DOWN and the menu closes
        assert!(runtime.pointer_pressed(menu.frame.origin.x + 20.0, row_y));
        assert_eq!(view.opened.get(), 1, "the action ran");
        assert!(runtime.interaction().menu.is_none(), "the menu closed");

        // reopen, then a press OUTSIDE closes, consumes, and fires
        // nothing — AppKit manners
        runtime.context_click(30.0, 8.0);
        let _ = runtime.layout(&view, crate::layout::Proposal::exact(size));
        assert!(runtime.pointer_pressed(250.0, 180.0));
        assert_eq!(view.opened.get(), 1, "nothing underneath activated");
        assert!(runtime.interaction().menu.is_none());

        // reopen, Escape closes through the stroke gate
        runtime.context_click(30.0, 8.0);
        let handled = runtime
            .key_stroke(&crate::action::KeyPattern::key(crate::action::Key::Escape));
        assert!(handled.handled, "Escape closed the menu");
        assert!(runtime.interaction().menu.is_none());

        // reopen, a wheel closes (content would slide under it)
        runtime.context_click(30.0, 8.0);
        let _ = runtime.layout(&view, crate::layout::Proposal::exact(size));
        runtime.wheel(150.0, 100.0, 0.0, 5.0);
        assert!(runtime.interaction().menu.is_none());
    }


    /// `.on_context_click` hands the press to the APP, at the point, and the
    /// runtime opens nothing of its own.
    ///
    /// It exists because a menu row in a real app is rarely a label and an
    /// action — Trinity's carry a keybind hint, a status dot, a checked mark,
    /// a submenu — and a host that draws its own panel needs the one thing
    /// only the runtime has: the press, and where it landed.
    #[test]
    fn a_right_press_can_be_the_apps_instead() {
        #[derive(Clone)]
        struct Row {
            at: State<Option<(f64, f64)>>,
        }
        impl Component for Row {
            fn body(self, _ctx: &Context) -> impl View {
                let at = self.at;
                vstack!(
                    text("file_0001.rs")
                        .on_context_click(move |point| at.set(Some((point.x, point.y)))),
                    text("below"),
                )
                .spacing(30.0)
            }
        }

        let runtime = Runtime::new();
        let view = Row { at: State::new(None) };
        let size = Size { width: 300.0, height: 200.0 };
        let _ = runtime.layout(&view, crate::layout::Proposal::exact(size));

        // Outside every region: nothing, as before.
        assert!(!runtime.context_click(200.0, 150.0));
        assert_eq!(view.at.get(), None);

        // Inside: the app hears the point, in WINDOW coordinates…
        assert!(runtime.context_click(30.0, 8.0));
        assert_eq!(view.at.get(), Some((30.0, 8.0)));
        // …and no menu of the runtime's own opened over it.
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        assert!(
            !result
                .overlays
                .iter()
                .any(|overlay| overlay.path == crate::layout::MENU_PATH),
            "the runtime opened nothing",
        );
        assert!(runtime.interaction().menu.is_none());
    }

    /// The two doors are ONE gesture with two answers: whichever region is
    /// inner wins the press, and an app-drawn menu inside a card that offers
    /// its own items is the app's.
    #[test]
    fn the_innermost_region_answers_whichever_door_it_is() {
        #[derive(Clone)]
        struct Nest {
            heard: State<usize>,
            picked: State<usize>,
        }
        impl Component for Nest {
            fn body(self, _ctx: &Context) -> impl View {
                let (heard, picked) = (self.heard, self.picked);
                vstack!(
                    text("inner").on_context_click(move |_| heard.set(heard.get() + 1)),
                    text("outer"),
                )
                .spacing(30.0)
                .context_menu(vec![menu_item("Card", move || {
                    picked.set(picked.get() + 1);
                })])
            }
        }

        let runtime = Runtime::new();
        let view = Nest { heard: State::new(0), picked: State::new(0) };
        let size = Size { width: 300.0, height: 200.0 };
        let _ = runtime.layout(&view, crate::layout::Proposal::exact(size));

        // Over the inner row: the app's door.
        assert!(runtime.context_click(30.0, 8.0));
        assert_eq!(view.heard.get(), 1);
        assert!(runtime.interaction().menu.is_none(), "and no panel of the runtime's");

        // Over the card but not the row: the items the card offered. (The
        // card hugs its children, so a point far to the right is outside it —
        // the second row is what "inside the card, outside the row" means.)
        assert!(runtime.context_click(10.0, 45.0));
        assert_eq!(view.heard.get(), 1, "the inner region did not hear this one");
        assert!(runtime.interaction().menu.is_some(), "the card's own menu opened");
    }

    #[test]
    fn a_divider_is_never_a_pick() {
        #[derive(Clone)]
        struct One {
            fired: State<usize>,
        }
        impl Component for One {
            fn body(self, _ctx: &Context) -> impl View {
                let fired = self.fired;
                text("target").context_menu(vec![
                    menu_item("First", move || fired.set(fired.get() + 1)),
                    menu_divider(),
                    menu_item("Second", || {}),
                ])
            }
        }
        let runtime = Runtime::new();
        let view = One { fired: State::new(0) };
        let size = Size { width: 300.0, height: 200.0 };
        let _ = runtime.layout(&view, crate::layout::Proposal::exact(size));
        runtime.context_click(20.0, 8.0);
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        let menu = result
            .overlays
            .iter()
            .find(|overlay| overlay.path == crate::layout::MENU_PATH)
            .expect("open");
        // the divider band: past the first row, inside the gap
        let divider_y = menu.frame.origin.y + 5.0 + 24.0 + 4.0;
        assert!(runtime.pointer_pressed(menu.frame.origin.x + 20.0, divider_y));
        assert_eq!(view.fired.get(), 0, "a divider fires nothing");
        assert!(runtime.interaction().menu.is_none(), "but the press still closes");
    }

    #[derive(Clone, Copy, PartialEq, Debug)]
    struct TabDrag {
        index: usize,
    }

    #[derive(Clone)]
    struct DragBoard {
        clicked: State<usize>,
        landed: State<usize>,
        wrong: State<usize>,
    }

    impl Component for DragBoard {
        fn body(self, _ctx: &Context) -> impl View {
            let clicked = self.clicked;
            let landed = self.landed;
            let wrong = self.wrong;
            vstack!(
                text("tab 2")
                    .on_drag(|| drag(TabDrag { index: 2 }, "tab 2"))
                    .on_click(move || clicked.set(clicked.get() + 1)),
                text("pane").on_drop(move |tab: &TabDrag| landed.set(tab.index)),
                text("trash").on_drop(move |_: &String| wrong.set(wrong.get() + 1)),
            )
            .spacing(30.0)
        }
    }

    #[test]
    fn the_innermost_explanation_wins_the_hover() {
        // the same disease the drop had: a tooltip on a chip inside a
        // card must be the CHIP's, not the card's
        #[derive(Clone, Copy)]
        struct Card;
        impl Component for Card {
            fn body(self, _ctx: &Context) -> impl View {
                vstack!(text("chip").frame(60.0, 20.0).tooltip("the chip"), spacer())
                    .frame(200.0, 80.0)
                    .tooltip("the card")
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 300.0, height: 200.0 };
        let result = runtime.layout(&Card, crate::layout::Proposal::exact(size));
        let chip = result
            .tooltips
            .iter()
            .find(|region| region.rect.size.width == 60.0)
            .expect("the chip explains itself")
            .rect;
        runtime.pointer_moved(chip.origin.x + 4.0, chip.origin.y + 4.0, false);
        for _ in 0..40 {
            runtime.tooltip_tick();
        }
        let shown = runtime.interaction().tooltip.map(|(text, _, _)| text.to_string());
        assert_eq!(shown.as_deref(), Some("the chip"));
    }

    #[test]
    fn the_innermost_source_is_what_the_hand_lifts() {
        // and again for the lift: pressing a chip inside a draggable
        // card carries the CHIP
        #[derive(Clone, Copy)]
        struct Card;
        impl Component for Card {
            fn body(self, _ctx: &Context) -> impl View {
                vstack!(
                    text("chip")
                        .frame(60.0, 20.0)
                        .on_drag(|| drag(TabDrag { index: 1 }, "chip")),
                    spacer(),
                )
                .frame(200.0, 80.0)
                .on_drag(|| drag(TabDrag { index: 9 }, "card"))
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 300.0, height: 200.0 };
        let result = runtime.layout(&Card, crate::layout::Proposal::exact(size));
        let chip = result
            .drag_sources
            .iter()
            .find(|region| region.rect.size.width == 60.0)
            .expect("the chip lifts")
            .rect;
        runtime.pointer_pressed(chip.origin.x + 4.0, chip.origin.y + 4.0);
        runtime.pointer_moved(chip.origin.x + 4.0, chip.origin.y + 40.0, false);
        let label = runtime.interaction().drag.map(|live| live.label.to_string());
        assert_eq!(label.as_deref(), Some("chip"));
    }

    /// The nested fixture of PAIN 36: a pane whose strip holds a chip,
    /// both taking the same drag. The chip is strictly inside.
    #[derive(Clone)]
    struct NestedBoard {
        on_chip: State<usize>,
        on_pane: State<usize>,
    }

    impl Component for NestedBoard {
        fn body(self, _ctx: &Context) -> impl View {
            let on_chip = self.on_chip;
            let on_pane = self.on_pane;
            vstack!(
                text("tab 7").on_drag(|| drag(TabDrag { index: 7 }, "tab 7")),
                vstack!(
                    text("chip")
                        .frame(60.0, 20.0)
                        .on_drop(move |tab: &TabDrag| on_chip.set(tab.index)),
                    spacer(),
                )
                .frame(200.0, 80.0)
                .on_drop(move |tab: &TabDrag| on_pane.set(tab.index)),
            )
            .spacing(10.0)
        }
    }

    /// PAIN 36's OTHER shape, and the one the dock actually has: a
    /// transparent catcher laid OVER the pane's body, both accepting
    /// the same drag, and the two the SAME size. Nothing tells them
    /// apart but which is inside which.
    #[derive(Clone)]
    struct CaughtBoard {
        on_catcher: State<usize>,
        on_pane: State<usize>,
    }

    impl Component for CaughtBoard {
        fn body(self, _ctx: &Context) -> impl View {
            let on_catcher = self.on_catcher;
            let on_pane = self.on_pane;
            vstack!(
                text("tab 7").on_drag(|| drag(TabDrag { index: 7 }, "tab 7")),
                zstack!(
                    text("body"),
                    spacer().on_drop(move |tab: &TabDrag| on_catcher.set(tab.index)),
                )
                .frame(200.0, 80.0)
                .on_drop(move |tab: &TabDrag| on_pane.set(tab.index)),
            )
            .spacing(10.0)
        }
    }

    #[test]
    fn a_transparent_catcher_over_a_body_takes_the_drop() {
        use crate::layout::{Proposal, Size};

        let board = CaughtBoard { on_catcher: State::new(0), on_pane: State::new(0) };
        let runtime = Runtime::new();
        runtime.render_stable(&board);
        let size = Size { width: 300.0, height: 200.0 };
        let result = runtime.layout(&board, Proposal::exact(size));

        // both regions cover the same 200x80 box: only the nesting can
        // decide, which is the whole of pain 36
        let same: Vec<_> = result
            .drops
            .iter()
            .filter(|region| region.frame.size == (Size { width: 200.0, height: 80.0 }))
            .collect();
        assert_eq!(same.len(), 2, "the catcher and the pane share a box: {same:?}");
        let target = same[0].frame;

        let chip = result.drag_sources.first().expect("the tab lifts").rect;
        runtime.pointer_pressed(chip.origin.x + 2.0, chip.origin.y + 2.0);
        runtime.pointer_moved(chip.origin.x + 2.0, chip.origin.y + 30.0, false);
        let (x, y) = (
            target.origin.x + target.size.width / 2.0,
            target.origin.y + target.size.height / 2.0,
        );
        runtime.pointer_moved(x, y, false);
        runtime.pointer_released(x, y);

        assert_eq!(board.on_catcher.get(), 7, "the catcher is inside, so it catches");
        assert_eq!(board.on_pane.get(), 0, "and the pane under it hears nothing");
    }

    /// The two targets by their GEOMETRY — never by their index in the
    /// vector, which is the very thing under test.
    fn chip_and_pane(result: &crate::layout::LayoutResult) -> (crate::layout::Rect, crate::layout::Rect) {
        let by_width = |width: f64| {
            result
                .drops
                .iter()
                .find(|region| region.frame.size.width == width)
                .unwrap_or_else(|| panic!("no drop target {width} wide"))
                .frame
        };
        (by_width(60.0), by_width(200.0))
    }

    fn nested_board() -> (Runtime, NestedBoard, Size) {
        let runtime = Runtime::new();
        let view =
            NestedBoard { on_chip: State::new(usize::MAX), on_pane: State::new(usize::MAX) };
        let size = Size { width: 300.0, height: 200.0 };
        let _ = runtime.layout(&view, crate::layout::Proposal::exact(size));
        (runtime, view, size)
    }

    #[test]
    fn the_drops_run_outer_before_inner() {
        // the invariant itself, with no pointer in sight: an ancestor is
        // recorded BEFORE the subtree it holds
        let (runtime, view, size) = nested_board();
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        assert_eq!(result.drops.len(), 2);
        let (pane, chip) = (result.drops[0].frame, result.drops[1].frame);
        assert_eq!(pane.size, Size { width: 200.0, height: 80.0 }, "the outer comes first");
        assert_eq!(chip.size, Size { width: 60.0, height: 20.0 }, "the inner comes second");
        assert!(
            chip.origin.x >= pane.origin.x
                && chip.origin.y >= pane.origin.y
                && chip.origin.x + chip.size.width <= pane.origin.x + pane.size.width
                && chip.origin.y + chip.size.height <= pane.origin.y + pane.size.height,
            "and the second really sits inside the first"
        );
    }

    #[test]
    fn the_innermost_drop_target_takes_the_drop() {
        let (runtime, view, size) = nested_board();
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        let source = result.drag_sources.first().expect("the chip lifts").rect;
        let (chip, _) = chip_and_pane(&result);

        runtime.pointer_pressed(source.origin.x + 4.0, source.origin.y + 4.0);
        runtime.pointer_moved(source.origin.x + 4.0, source.origin.y + 40.0, false);
        let (x, y) = (chip.origin.x + 4.0, chip.origin.y + 4.0);
        runtime.pointer_moved(x, y, false);
        runtime.pointer_released(x, y);

        assert_eq!(view.on_chip.get(), 7, "the chip took its own drop");
        assert_eq!(view.on_pane.get(), usize::MAX, "and the pane never saw it");
    }

    #[test]
    fn a_drop_outside_the_inner_target_still_reaches_the_ancestor() {
        // the ancestor keeps the ground its children do not cover
        let (runtime, view, size) = nested_board();
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        let source = result.drag_sources.first().expect("the chip lifts").rect;
        let (_, pane) = chip_and_pane(&result);

        runtime.pointer_pressed(source.origin.x + 4.0, source.origin.y + 4.0);
        runtime.pointer_moved(source.origin.x + 4.0, source.origin.y + 40.0, false);
        let (x, y) = (pane.origin.x + 4.0, pane.origin.y + pane.size.height - 4.0);
        runtime.pointer_moved(x, y, false);
        runtime.pointer_released(x, y);

        assert_eq!(view.on_pane.get(), 7);
        assert_eq!(view.on_chip.get(), usize::MAX);
    }

    #[test]
    fn an_inner_target_of_the_wrong_type_lets_the_ancestor_catch() {
        // the depth rule never eats the TYPE filter: an inner target
        // that cannot take this drag is not in the way
        #[derive(Clone)]
        struct Mixed {
            landed: State<usize>,
        }

        impl Component for Mixed {
            fn body(self, _ctx: &Context) -> impl View {
                let landed = self.landed;
                vstack!(
                    text("tab 3").on_drag(|| drag(TabDrag { index: 3 }, "tab 3")),
                    vstack!(
                        text("chip").frame(60.0, 20.0).on_drop(move |_: &String| {}),
                        spacer(),
                    )
                    .frame(200.0, 80.0)
                    .on_drop(move |tab: &TabDrag| landed.set(tab.index)),
                )
                .spacing(10.0)
            }
        }

        let runtime = Runtime::new();
        let view = Mixed { landed: State::new(usize::MAX) };
        let size = Size { width: 300.0, height: 200.0 };
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        let source = result.drag_sources.first().expect("it lifts").rect;
        let chip = result
            .drops
            .iter()
            .find(|region| region.frame.size.width == 60.0)
            .expect("the wrong-typed chip is still a region")
            .frame;

        runtime.pointer_pressed(source.origin.x + 4.0, source.origin.y + 4.0);
        runtime.pointer_moved(source.origin.x + 4.0, source.origin.y + 40.0, false);
        let (x, y) = (chip.origin.x + 4.0, chip.origin.y + 4.0);
        runtime.pointer_moved(x, y, false);
        runtime.pointer_released(x, y);

        assert_eq!(view.landed.get(), 3, "the pane caught what the chip cannot take");
    }

    #[test]
    fn the_ring_follows_the_innermost_target_and_paints_over_the_child() {
        let (runtime, view, size) = nested_board();
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        let source = result.drag_sources.first().expect("the chip lifts").rect;
        let (chip, _) = chip_and_pane(&result);

        runtime.pointer_pressed(source.origin.x + 4.0, source.origin.y + 4.0);
        runtime.pointer_moved(source.origin.x + 4.0, source.origin.y + 40.0, false);
        runtime.pointer_moved(chip.origin.x + 4.0, chip.origin.y + 4.0, false);
        let over = runtime.layout(&view, crate::layout::Proposal::exact(size));

        let accent = crate::theme::current().accent;
        let rings: Vec<(usize, crate::layout::Rect)> = over
            .display
            .iter()
            .enumerate()
            .filter_map(|(index, command)| match command {
                crate::layout::DrawCommand::StrokeRect { rect, color, width, .. }
                    if *color == accent && *width == 2.0 =>
                {
                    Some((index, *rect))
                }
                _ => None,
            })
            .collect();
        assert_eq!(rings.len(), 1, "one target, one ring");
        assert_eq!(rings[0].1, chip, "and it is the INNERMOST target's box");

        // the ring is paint: it covers the child it rings
        let label = over
            .display
            .iter()
            .position(|command| {
                matches!(command, crate::layout::DrawCommand::TextLine { content, .. }
                    if &**content == "chip")
            })
            .expect("the chip's own text is drawn");
        assert!(rings[0].0 > label, "the ring paints AFTER what it rings");
    }

    fn drag_board() -> (Runtime, DragBoard, Size) {
        let runtime = Runtime::new();
        let view = DragBoard {
            clicked: State::new(0),
            landed: State::new(usize::MAX),
            wrong: State::new(0),
        };
        let size = Size { width: 300.0, height: 200.0 };
        let _ = runtime.layout(&view, crate::layout::Proposal::exact(size));
        (runtime, view, size)
    }

    #[test]
    fn a_click_that_never_moves_stays_a_click() {
        let (runtime, view, _) = drag_board();
        runtime.pointer_pressed(20.0, 8.0);
        runtime.pointer_released(20.0, 8.0);
        assert_eq!(view.clicked.get(), 1, "the press never lifted");
        assert!(runtime.interaction().drag.is_none());
    }

    #[test]
    fn a_lift_carries_the_typed_value_to_its_target() {
        use crate::layout::DRAG_LABEL_PATH;
        let (runtime, view, size) = drag_board();
        // press on the tab, move past the threshold: the drag lifts
        runtime.pointer_pressed(20.0, 8.0);
        assert!(runtime.pointer_moved(30.0, 20.0, false), "the lift repaints");
        let live = runtime.interaction().drag.expect("a drag is live");
        assert_eq!(&*live.label, "tab 2");
        // the chip follows the cursor as an overlay
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        let chip = result
            .overlays
            .iter()
            .find(|overlay| overlay.path == DRAG_LABEL_PATH)
            .expect("the label rides the pointer");
        let says: Vec<String> = result
            .display
            .iter()
            .skip(chip.display.0)
            .filter_map(|command| match command {
                crate::layout::DrawCommand::TextLine { content, .. } => {
                    Some(content.to_string())
                }
                _ => None,
            })
            .collect();
        assert_eq!(says, vec!["tab 2".to_string()]);
        // over the compatible pane: the target rings
        let target = result
            .drops
            .iter()
            .find(|region| region.accepts == std::any::TypeId::of::<TabDrag>())
            .expect("the pane is a target")
            .rect;
        let (cx, cy) =
            (target.origin.x + target.size.width / 2.0, target.origin.y + target.size.height / 2.0);
        assert!(runtime.pointer_moved(cx, cy, false));
        assert_eq!(runtime.interaction().drag.unwrap().over, Some(target));
        let ringed = runtime.layout(&view, crate::layout::Proposal::exact(size));
        let accent = crate::theme::current().accent;
        assert!(
            ringed.display.iter().any(|command| matches!(
                command,
                crate::layout::DrawCommand::StrokeRect { color, width, .. }
                    if *color == accent && *width == 2.0
            )),
            "the framework rings the target"
        );
        // the release lands the value, typed — and the click never fired
        runtime.pointer_released(cx, cy);
        assert_eq!(view.landed.get(), 2, "the pane took TabDrag {{ index: 2 }}");
        assert_eq!(view.clicked.get(), 0, "the click died at the lift");
        assert!(runtime.interaction().drag.is_none(), "the drag went home");
        assert_eq!(view.wrong.get(), 0);
    }

    #[test]
    fn a_wrong_type_never_lights_nor_lands() {
        let (runtime, view, size) = drag_board();
        runtime.pointer_pressed(20.0, 8.0);
        runtime.pointer_moved(30.0, 20.0, false);
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        let trash = result
            .drops
            .iter()
            .find(|region| region.accepts == std::any::TypeId::of::<String>())
            .expect("the decoy is a target")
            .rect;
        let (cx, cy) =
            (trash.origin.x + trash.size.width / 2.0, trash.origin.y + trash.size.height / 2.0);
        runtime.pointer_moved(cx, cy, false);
        assert_eq!(runtime.interaction().drag.unwrap().over, None, "the types do not meet");
        runtime.pointer_released(cx, cy);
        assert_eq!(view.wrong.get(), 0, "nothing landed");
        assert_eq!(view.landed.get(), usize::MAX);
    }

    #[test]
    fn escape_sends_the_drag_home() {
        let (runtime, view, _) = drag_board();
        runtime.pointer_pressed(20.0, 8.0);
        runtime.pointer_moved(60.0, 40.0, false);
        assert!(runtime.interaction().drag.is_some());
        let handled = runtime
            .key_stroke(&crate::action::KeyPattern::key(crate::action::Key::Escape));
        assert!(handled.handled);
        assert!(runtime.interaction().drag.is_none());
        runtime.pointer_released(60.0, 40.0);
        assert_eq!(view.landed.get(), usize::MAX, "a cancelled drag lands nowhere");
        assert_eq!(view.clicked.get(), 0, "and the click stayed dead");
    }

    #[test]
    fn a_rising_press_reaches_the_pane_around_the_box() {
        use std::cell::Cell;

        use crate::custom::{CustomElement, ElementEvent, EventCtx, PaintCtx, Painter, Response};

        struct Surface {
            rises: bool,
            downs: Rc<Cell<usize>>,
        }
        impl CustomElement for Surface {
            fn paint(&self, _ctx: &PaintCtx, _painter: &mut Painter) {}
            fn event(&self, event: &ElementEvent, _ctx: &EventCtx) -> Response {
                match event {
                    ElementEvent::PointerDown { .. } => {
                        self.downs.set(self.downs.get() + 1);
                        if self.rises {
                            Response::handled_rising()
                        } else {
                            Response::handled()
                        }
                    }
                    _ => Response::ignored(),
                }
            }
            fn name(&self) -> &str {
                "surface"
            }
        }

        #[derive(Clone)]
        struct Pane {
            rises: State<bool>,
            focused: State<usize>,
            downs: Rc<Cell<usize>>,
        }
        impl Component for Pane {
            fn body(self, _ctx: &Context) -> impl View {
                let focused = self.focused;
                custom(Surface { rises: self.rises.get(), downs: Rc::clone(&self.downs) })
                    .frame(120.0, 80.0)
                    .on_click(move || focused.set(focused.get() + 1))
            }
        }

        let size = Size { width: 160.0, height: 100.0 };
        let runtime = Runtime::new();
        let pane = Pane {
            rises: State::new(false),
            focused: State::new(0),
            downs: Rc::new(Cell::new(0)),
        };
        let _ = runtime.layout(&pane, crate::layout::Proposal::exact(size));
        // the old manners: the box swallows, the pane never hears
        runtime.pointer_pressed(60.0, 40.0);
        runtime.pointer_released(60.0, 40.0);
        assert_eq!(pane.downs.get(), 1, "the box heard the press");
        assert_eq!(pane.focused.get(), 0, "a handled press stops, as ever");

        // the new answer: handled AND rising — one click does both
        pane.rises.set(true);
        let _ = runtime.layout(&pane, crate::layout::Proposal::exact(size));
        runtime.pointer_pressed(60.0, 40.0);
        let fired = runtime.pointer_released(60.0, 40.0);
        assert_eq!(pane.downs.get(), 2, "the box still heard the press");
        assert_eq!(pane.focused.get(), 1, "and the pane's click fired");
        assert!(fired.is_some());

        // released OUTSIDE the pane: the risen press dies quietly
        runtime.pointer_pressed(60.0, 40.0);
        runtime.pointer_released(150.0, 95.0);
        assert_eq!(pane.focused.get(), 1, "up-outside is button manners");
    }

    #[test]
    fn a_region_can_travel_sideways_and_both_ways() {
        #[derive(Clone, Copy)]
        struct Sheet;
        impl Component for Sheet {
            fn body(self, _ctx: &Context) -> impl View {
                scroll(
                    vstack!(
                        text("a wide row that runs far past the viewport edge"),
                        text("and a second one below it"),
                        text("and a third"),
                    )
                    .spacing(30.0),
                )
                .both_axes()
                .frame(120.0, 60.0)
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 140.0, height: 80.0 };
        let result = runtime.layout(&Sheet, crate::layout::Proposal::exact(size));
        let region = result.scrolls.first().expect("the region registered");
        assert!(region.content.width > 120.0, "the content keeps its width");
        assert!(region.content.height > 60.0, "and its height");

        // the wheel travels BOTH ways and clamps per axis
        assert!(runtime.wheel(60.0, 30.0, -4.0, -6.0));
        let offsets = runtime
            .layout(&Sheet, crate::layout::Proposal::exact(size))
            .scrolls
            .first()
            .map(|_| ())
            .expect("still there");
        let _ = offsets;
        // a huge wheel pins to the far corner instead of flying off
        runtime.wheel(60.0, 30.0, -100_000.0, -100_000.0);
        let pinned = runtime.layout(&Sheet, crate::layout::Proposal::exact(size));
        let region = pinned.scrolls.first().unwrap();
        // both thumbs paint: the vertical lane hugs the right edge,
        // the horizontal one lies along the bottom
        let scrollbar = crate::theme::current().scrollbar;
        let thumbs: Vec<crate::layout::Rect> = pinned
            .display
            .iter()
            .filter_map(|command| match command {
                crate::layout::DrawCommand::FillRect { rect, color, .. }
                    if *color == scrollbar =>
                {
                    Some(*rect)
                }
                _ => None,
            })
            .collect();
        assert_eq!(thumbs.len(), 2, "one thumb per travelling axis");
        assert!(thumbs.iter().any(|rect| rect.size.height > rect.size.width), "the tall one");
        assert!(thumbs.iter().any(|rect| rect.size.width > rect.size.height), "the flat one");
        let _ = region;
    }

    #[test]
    fn a_sideways_only_region_ignores_the_vertical_wheel() {
        #[derive(Clone, Copy)]
        struct Line;
        impl Component for Line {
            fn body(self, _ctx: &Context) -> impl View {
                scroll(text("one very long unwrapped line of code that overflows"))
                    .horizontal()
                    .frame(100.0, 24.0)
            }
        }
        let runtime = Runtime::new();
        let size = Size { width: 120.0, height: 40.0 };
        let _ = runtime.layout(&Line, crate::layout::Proposal::exact(size));
        // dy alone finds no travel; dx moves it
        assert!(!runtime.wheel(50.0, 12.0, 0.0, -5.0), "no vertical travel to take");
        assert!(runtime.wheel(50.0, 12.0, -5.0, 0.0), "sideways is the whole point");
    }

    #[test]
    fn a_table_stands_on_columns_and_windows_its_rows() {
        #[derive(Clone, Copy)]
        struct Sheet;
        impl Component for Sheet {
            fn body(self, _ctx: &Context) -> impl View {
                table(
                    vec![
                        column("Name", 200.0),
                        column("Kind", 90.0),
                        column("Size", 80.0),
                        column("Modified", 140.0),
                    ],
                    10_000,
                    |row| row.to_string(),
                    |row, col| text(format!("r{row}c{col}")),
                )
                .frame(260.0, 120.0)
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 280.0, height: 140.0 };
        let result = runtime.layout(&Sheet, crate::layout::Proposal::exact(size));

        // two regions: the sideways sheet and the vertical rows
        assert_eq!(result.scrolls.len(), 2, "{:?}", result.scrolls);
        let across = result
            .scrolls
            .iter()
            .find(|region| region.content.width > 400.0)
            .expect("the sideways region spans the columns");
        assert_eq!(across.content.width, 510.0, "the columns add up");
        let down = result
            .scrolls
            .iter()
            .find(|region| region.content.height > 1000.0)
            .expect("the rows make the tall region");
        assert!(down.content.height >= 10_000.0 * 26.0, "every row exists in geometry");

        // ten thousand rows, a SCREENFUL of text painted
        let cells = result
            .display
            .iter()
            .filter(|command| {
                matches!(command, crate::layout::DrawCommand::TextLine { content, .. }
                    if content.starts_with('r'))
            })
            .count();
        assert!(cells < 200, "the window stays a screenful: {cells}");
        // the header paints its four titles
        let headers = result
            .display
            .iter()
            .filter(|command| {
                matches!(command, crate::layout::DrawCommand::TextLine { content, .. }
                    if ["Name", "Kind", "Size", "Modified"].contains(&&**content))
            })
            .count();
        assert_eq!(headers, 4);

        // a vertical wheel moves the rows and leaves the header put
        assert!(runtime.wheel(100.0, 80.0, 0.0, -52.0));
        let scrolled = runtime.layout(&Sheet, crate::layout::Proposal::exact(size));
        let first_cell_y = scrolled
            .display
            .iter()
            .find_map(|command| match command {
                crate::layout::DrawCommand::TextLine { origin, content, .. }
                    if content.starts_with("r2c") =>
                {
                    Some(origin.y)
                }
                _ => None,
            });
        assert!(first_cell_y.is_some(), "row 2 rode into view");
        let header_y = scrolled
            .display
            .iter()
            .find_map(|command| match command {
                crate::layout::DrawCommand::TextLine { origin, content, .. }
                    if &**content == "Name" =>
                {
                    Some(origin.y)
                }
                _ => None,
            })
            .expect("the header stays");
        assert!(header_y < 26.0, "the header never scrolls away: {header_y}");

        // a sideways wheel slides header AND rows together, in step
        let name_x = |result: &crate::layout::LayoutResult| {
            result
                .display
                .iter()
                .find_map(|command| match command {
                    crate::layout::DrawCommand::TextLine { origin, content, .. }
                        if &**content == "Name" =>
                    {
                        Some(origin.x)
                    }
                    _ => None,
                })
                .expect("the header is painted")
        };
        let cell_x = |result: &crate::layout::LayoutResult| {
            result
                .display
                .iter()
                .find_map(|command| match command {
                    crate::layout::DrawCommand::TextLine { origin, content, .. }
                        if content.starts_with("r2c0") =>
                    {
                        Some(origin.x)
                    }
                    _ => None,
                })
                .expect("the cell is painted")
        };
        let before = (name_x(&scrolled), cell_x(&scrolled));
        assert!(runtime.wheel(100.0, 80.0, -60.0, 0.0));
        let slid = runtime.layout(&Sheet, crate::layout::Proposal::exact(size));
        assert_eq!(name_x(&slid), before.0 - 60.0, "the header slid with its columns");
        assert_eq!(cell_x(&slid), before.1 - 60.0, "and the rows slid in step");
    }

    #[derive(Clone, Copy, PartialEq, Debug)]
    struct Tab {
        index: usize,
    }

    /// The three questions pain 31 asks of one drop: WHERE it landed,
    /// where the hand is WHILE it moves, and whether a target that is
    /// half scrolled away still tells the truth.
    #[test]
    fn a_drop_says_where_it_landed() {
        #[derive(Clone)]
        struct Board {
            landed: State<(usize, usize)>,
            zone: State<Option<(usize, usize)>>,
        }
        impl Component for Board {
            fn body(self, _ctx: &Context) -> impl View {
                let landed = self.landed;
                let zone = self.zone;
                // a pane body: 200x100, dropped on by quadrant
                vstack!(
                    text("tab").on_drag(|| drag(Tab { index: 7 }, "tab 7")),
                    empty()
                        .frame(200.0, 100.0)
                        .on_drop_at(move |_: &Tab, at| {
                            let (x, y) = at.fraction();
                            landed.set(((x * 100.0) as usize, (y * 100.0) as usize));
                        })
                        .preview(move |at| {
                            zone.set(at.map(|at| {
                                let (x, y) = at.fraction();
                                ((x * 100.0) as usize, (y * 100.0) as usize)
                            }))
                        }),
                )
                .spacing(0.0)
            }
        }

        let runtime = Runtime::new();
        let view = Board { landed: State::new((0, 0)), zone: State::new(None) };
        let size = Size { width: 220.0, height: 160.0 };
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        let target = result.drops.first().expect("the body accepts").frame;
        let source = result.drag_sources.first().expect("the tab lifts").rect;

        // lift the tab, then travel to the target's own quarter point
        runtime.pointer_pressed(
            source.origin.x + source.size.width / 2.0,
            source.origin.y + source.size.height / 2.0,
        );
        runtime.pointer_moved(source.origin.x + 30.0, source.origin.y + 30.0, false);
        assert!(runtime.interaction().drag.is_some(), "the drag lifted");
        let quarter = (
            target.origin.x + target.size.width * 0.25,
            target.origin.y + target.size.height * 0.75,
        );
        runtime.pointer_moved(quarter.0, quarter.1, false);
        // the preview heard the place WHILE the drag moves — this is
        // the whole of pain 31: the veil can be painted now
        assert_eq!(view.zone.get(), Some((25, 75)), "the hand reports its quarter");

        // the framework's ring stands down for a box that previews
        let ringed = runtime
            .layout(&view, crate::layout::Proposal::exact(size))
            .display
            .iter()
            .any(|command| matches!(command,
                crate::layout::DrawCommand::StrokeRect { color, width, .. }
                    if *color == crate::theme::current().accent && *width == 2.0));
        assert!(!ringed, "the app paints its own preview, so the ring keeps quiet");

        // the release lands the value AND the place, and closes the
        // preview before the action runs
        runtime.pointer_released(quarter.0, quarter.1);
        assert_eq!(view.landed.get(), (25, 75), "the drop knows its quadrant");
        assert_eq!(view.zone.get(), None, "the preview closed with the gesture");
    }

    #[test]
    fn a_preview_hears_the_leave_and_the_escape() {
        #[derive(Clone)]
        struct Board {
            seen: State<Vec<String>>,
        }
        impl Component for Board {
            fn body(self, _ctx: &Context) -> impl View {
                let seen = self.seen;
                vstack!(
                    text("tab").on_drag(|| drag(Tab { index: 1 }, "tab")),
                    empty().frame(80.0, 40.0).on_drop_at(|_: &Tab, _| {}).preview({
                        let seen = seen;
                        move |at| {
                            let mut log = seen.get();
                            log.push(match at {
                                Some(_) => "over".to_string(),
                                None => "left".to_string(),
                            });
                            seen.set(log);
                        }
                    }),
                    empty().frame(80.0, 40.0),
                )
                .spacing(0.0)
            }
        }

        let runtime = Runtime::new();
        let view = Board { seen: State::new(Vec::new()) };
        let size = Size { width: 120.0, height: 200.0 };
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        let target = result.drops.first().expect("a target").frame;
        let source = result.drag_sources.first().expect("the tab lifts").rect;
        let inside = (
            target.origin.x + target.size.width / 2.0,
            target.origin.y + target.size.height / 2.0,
        );

        runtime.pointer_pressed(
            source.origin.x + source.size.width / 2.0,
            source.origin.y + source.size.height / 2.0,
        );
        // lift sideways, away from the target below
        runtime.pointer_moved(source.origin.x + 40.0, source.origin.y, false);
        runtime.pointer_moved(inside.0, inside.1, false);
        assert_eq!(view.seen.get(), vec!["over".to_string()], "entering speaks once");

        // moving WITHIN the same box does not re-enter, it just moves
        runtime.pointer_moved(inside.0 + 4.0, inside.1 + 4.0, false);
        assert_eq!(view.seen.get(), vec!["over".to_string(), "over".to_string()]);

        // leaving the box: exactly ONE None, no matter how far it goes
        runtime.pointer_moved(inside.0, target.origin.y + target.size.height + 30.0, false);
        runtime.pointer_moved(inside.0, target.origin.y + target.size.height + 40.0, false);
        let log = view.seen.get();
        assert_eq!(log.last().map(String::as_str), Some("left"));
        assert_eq!(
            log.iter().filter(|entry| *entry == "left").count(),
            1,
            "leaving speaks once, not once per move: {log:?}"
        );

        // back in, then Escape — the preview must not be left hanging
        runtime.pointer_moved(inside.0, inside.1, false);
        assert_eq!(view.seen.get().last().map(String::as_str), Some("over"));
        let handled =
            runtime.key_stroke(&crate::action::KeyPattern::key(crate::action::Key::Escape));
        assert!(handled.handled, "escape cancelled the drag");
        assert_eq!(view.seen.get().last().map(String::as_str), Some("left"),
            "a cancelled drag closes the preview");
    }

    /// The trap: a target half scrolled out of view. The rect a drop
    /// HITS is the visible slice; the box it reports against must be
    /// the whole one, or every quadrant lies.
    #[test]
    fn a_scrolled_target_still_tells_the_truth() {
        #[derive(Clone)]
        struct Board {
            at: State<Option<(i64, i64)>>,
        }
        impl Component for Board {
            fn body(self, _ctx: &Context) -> impl View {
                let at = self.at;
                vstack!(
                    text("tab").on_drag(|| drag(Tab { index: 0 }, "t")),
                    scroll(vstack!(
                        empty().frame(100.0, 60.0),
                        empty()
                            .frame(100.0, 100.0)
                            .on_drop_at(move |_: &Tab, place| {
                                at.set(Some((place.local.y as i64, place.size.height as i64)))
                            }),
                    )
                    .spacing(0.0))
                    .frame(100.0, 80.0),
                )
                .spacing(0.0)
            }
        }

        let runtime = Runtime::new();
        let view = Board { at: State::new(None) };
        let size = Size { width: 120.0, height: 200.0 };
        let _ = runtime.layout(&view, crate::layout::Proposal::exact(size));
        // scroll the target halfway up under its clip
        let region = runtime
            .layout(&view, crate::layout::Proposal::exact(size))
            .scrolls
            .first()
            .map(|region| region.path.clone())
            .expect("a region");
        // all the way down: the target's top now sits ABOVE the clip,
        // so its visible slice starts twenty points into its own box
        runtime.set_scroll_offset(&region, crate::layout::Point { x: 0.0, y: 80.0 });
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        let target = result.drops.first().expect("the target is placed");
        assert!(
            target.rect.size.height < target.frame.size.height,
            "the visible slice IS smaller: {:?} vs {:?}",
            target.rect.size,
            target.frame.size
        );

        // drop at the top edge of what is VISIBLE — which is 20pt down
        // the target's own box, not zero
        let source = result.drag_sources.first().expect("the tab lifts").rect;
        runtime.pointer_pressed(
            source.origin.x + source.size.width / 2.0,
            source.origin.y + source.size.height / 2.0,
        );
        runtime.pointer_moved(source.origin.x + 30.0, source.origin.y + 20.0, false);
        let visible_top = (target.rect.origin.x + 10.0, target.rect.origin.y + 1.0);
        runtime.pointer_released(visible_top.0, visible_top.1);
        let (local_y, height) = view.at.get().expect("it landed");
        assert_eq!(height, 100, "the box it reports against is the WHOLE one");
        assert!(
            local_y >= 20 && local_y <= 22,
            "the place is measured down the target's own box, not the slice: {local_y}"
        );
    }

    /// Element mode never reads the draw list, so the drop ring had to
    /// become an ELEMENT there — and a drag has to be possible in that
    /// mode at all, which needs the pointer-move door the glue opens
    /// only for an armed press.
    #[test]
    fn in_element_mode_the_ring_is_a_box_and_the_drag_still_lifts() {
        #[derive(Clone, Copy)]
        struct Board;
        impl Component for Board {
            fn body(self, _ctx: &Context) -> impl View {
                vstack!(
                    text("tab").on_drag(|| drag(Tab { index: 3 }, "tab 3")),
                    // NO preview declared: this one wears the
                    // framework's ring, and must wear it in the browser
                    text("pane").frame(80.0, 40.0).on_drop(|_: &Tab| {}),
                )
                .spacing(0.0)
            }
        }

        let runtime = Runtime::new();
        let size = Size { width: 120.0, height: 120.0 };
        let mount = runtime.dom_frame(&Board, size);
        let accent = crate::theme::current().accent;
        let ringed = |patches: &[crate::dom::DomPatch]| {
            patches.iter().any(|patch| matches!(patch,
                crate::dom::DomPatch::SetStyle { style, .. }
                    if style.border == Some((accent, 2.0))))
        };
        assert!(!ringed(&mount), "no drag, no ring");

        // the drag lifts through the very door the element mode opens
        let geometry = runtime.layout(&Board, crate::layout::Proposal::exact(size));
        let source = geometry.drag_sources.first().expect("the tab lifts").rect;
        let target = geometry.drops.first().expect("the pane accepts").rect;
        runtime.pointer_pressed(
            source.origin.x + source.size.width / 2.0,
            source.origin.y + source.size.height / 2.0,
        );
        assert!(runtime.drag_armed(), "the press armed it — this is what the glue asks");
        runtime.pointer_moved(source.origin.x + 40.0, source.origin.y, false);
        runtime.pointer_moved(
            target.origin.x + target.size.width / 2.0,
            target.origin.y + target.size.height / 2.0,
            false,
        );
        assert!(runtime.interaction().drag.is_some(), "the drag is live in this mode too");

        // the ring reached the browser as a bordered box
        let over = runtime.dom_frame(&Board, size);
        assert!(ringed(&over), "the ring is an element: {over:#?}");

        // and it leaves with the gesture
        runtime.pointer_released(
            target.origin.x + target.size.width / 2.0,
            target.origin.y + target.size.height / 2.0,
        );
        let after = runtime.dom_frame(&Board, size);
        assert!(
            after.iter().any(|patch| matches!(patch, crate::dom::DomPatch::Remove { .. })),
            "the ring element dies with the drag: {after:#?}"
        );
    }

    #[test]
    fn the_text_can_lean_and_the_lean_is_its_own_font() {
        use crate::text_engine::{FontSpec, Slant, Weight};

        #[derive(Clone, Copy)]
        struct Tabs;
        impl Component for Tabs {
            fn body(self, _ctx: &Context) -> impl View {
                vstack!(
                    // the preview tab of an editor: leaning says "you
                    // are only looking"
                    text("preview.rs").italic(),
                    text("pinned.rs"),
                    text("both").bold().italic(),
                    // the same sentence, written the two ways a hand
                    // writes it. The second is the one that comes out
                    // first: the face belongs to the view, the lean to
                    // the state — and neither order may lose one
                    text("lean first").italic().font(Font::Callout),
                    text("role first").font(Font::Callout).italic(),
                    text("mono").font(Font::Callout).monospaced(),
                )
                .spacing(0.0)
            }
        }

        let runtime = Runtime::new();
        let result =
            runtime.layout(&Tabs, crate::layout::Proposal::exact(Size { width: 200.0, height: 180.0 }));
        let fonts: Vec<(String, FontSpec)> = result
            .display
            .iter()
            .filter_map(|command| match command {
                crate::layout::DrawCommand::TextLine { content, font, .. } => {
                    Some((content.to_string(), *font))
                }
                _ => None,
            })
            .collect();
        assert_eq!(fonts[0].1.slant, Slant::Italic, "the preview leans");
        assert_eq!(fonts[1].1.slant, Slant::Upright, "the pinned one does not");
        // the two modifiers compose: each carries only its own field
        assert_eq!(fonts[2].1.slant, Slant::Italic);
        assert_eq!(fonts[2].1.weight, Weight::Bold);
        // and the ORDER cannot matter, because a modifier can only undo
        // what it names: a role names a size and a weight, never a lean
        // and never a face design
        assert_eq!(fonts[3].1.slant, Slant::Italic, "lean, then role");
        assert_eq!(fonts[3].1.size, 12.0, "and the role still sized it");
        assert_eq!(fonts[4].1.slant, Slant::Italic, "role, then lean — the silent one");
        assert_eq!(fonts[4].1.size, 12.0);
        assert_eq!(fonts[3].1, fonts[4].1, "the same sentence, either way round");
        assert_eq!(
            fonts[5].1.design,
            crate::text_engine::FontDesign::Mono,
            "and a role does not undo a face either",
        );

        // the LEAN is part of the font's identity: an upright and a
        // leaning line are two cache entries, never one answering for
        // the other
        let upright = FontSpec::DEFAULT;
        let leaning = FontSpec { slant: Slant::Italic, ..FontSpec::DEFAULT };
        assert_ne!(upright.key(), leaning.key());

        // and the print says it, beside .bold()
        let printed = runtime.render(&text("x").italic());
        assert!(printed.contains("[.italic()]"), "{printed}");
    }

    #[test]
    fn a_wrapped_line_sits_where_the_alignment_puts_it() {
        use crate::layout::{DrawCommand, Proposal};
        use motor::views::TextAlignment;
        let runtime = Runtime::new();
        let line_lefts = |display: &crate::layout::DisplayList| -> Vec<f64> {
            display
                .iter()
                .filter_map(|command| match command {
                    DrawCommand::TextLine { origin, .. } => Some(origin.x),
                    _ => None,
                })
                .collect()
        };
        // PixelFont: 8px a glyph. "aa bbbb" wraps in a 40pt column into
        // "aa" (16 wide) and "bbbb" (32) — centred, they start at 12 and
        // 4; trailing, at 24 and 8.
        let centred = text("aa bbbb")
            .multiline_text_alignment(TextAlignment::Center)
            .frame_width(40.0);
        assert_eq!(
            line_lefts(&runtime.layout(&centred, Proposal::unspecified()).display),
            vec![12.0, 4.0],
            "each line centres on its own width"
        );
        let trailing = text("aa bbbb")
            .multiline_text_alignment(TextAlignment::Trailing)
            .frame_width(40.0);
        assert_eq!(
            line_lefts(&runtime.layout(&trailing, Proposal::unspecified()).display),
            vec![24.0, 8.0]
        );
        // unset, every line starts at the leading edge, as they always did
        let plain = text("aa bbbb").frame_width(40.0);
        assert_eq!(
            line_lefts(&runtime.layout(&plain, Proposal::unspecified()).display),
            vec![0.0, 0.0]
        );
    }

    #[test]
    fn the_line_box_reaches_the_browser_on_the_wire() {
        use crate::text_engine::FontSpec;

        let stepped = crate::dom::DomText {
            content: std::sync::Arc::from("a paragraph"),
            color: Color::hex(0x202531),
            inherits_ink: false,
            font: FontSpec::DEFAULT,
            line_height: Some(24.0),
            text_align: None,
            highlights: None,
            truncation: None,
        };
        let with = crate::dom::encode(&[crate::dom::DomPatch::SetText { id: 4, text: stepped }]);
        let plain = crate::dom::DomText {
            content: std::sync::Arc::from("a paragraph"),
            color: Color::hex(0x202531),
            inherits_ink: false,
            font: FontSpec::DEFAULT,
            line_height: None,
            text_align: None,
            highlights: None,
            truncation: None,
        };
        let without = crate::dom::encode(&[crate::dom::DomPatch::SetText { id: 4, text: plain }]);

        // the same stream, four bytes apart — the line box is an f32
        // beside the family, and NONE travels as a plain zero
        assert_eq!(with.len(), without.len());
        assert_ne!(with, without);
        assert!(without.windows(4).any(|window| window == 0f32.to_le_bytes()));
        assert!(with.windows(4).any(|window| window == 24f32.to_le_bytes()));
    }

    #[test]
    fn the_lean_reaches_the_browser_on_the_wire() {
        use crate::text_engine::{FontSpec, Slant};

        let leaning = crate::dom::DomText {
            content: std::sync::Arc::from("preview.rs"),
            color: Color::hex(0x202531),
            inherits_ink: false,
            font: FontSpec { slant: Slant::Italic, ..FontSpec::DEFAULT },
            line_height: None,
            text_align: None,
            highlights: None,
            truncation: None,
        };
        let bytes = crate::dom::encode(&[crate::dom::DomPatch::SetText { id: 4, text: leaning }]);
        let upright = crate::dom::DomText {
            content: std::sync::Arc::from("preview.rs"),
            color: Color::hex(0x202531),
            inherits_ink: false,
            font: FontSpec::DEFAULT,
            line_height: None,
            text_align: None,
            highlights: None,
            truncation: None,
        };
        let plain = crate::dom::encode(&[crate::dom::DomPatch::SetText { id: 4, text: upright }]);

        // the same stream, ONE byte apart — the slant is a flag beside
        // mono, not a payload that grows the wire
        assert_eq!(plain.len(), bytes.len(), "one byte either way");
        let differing: Vec<usize> = plain
            .iter()
            .zip(&bytes)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(differing.len(), 1, "exactly one byte says it leans: {differing:?}");
        let at = differing[0];
        assert_eq!((plain[at], bytes[at]), (0, 1));
        // and it rides right after the mono flag
        assert_eq!(bytes[at - 1], 0, "mono sits before it");
    }

    /// The contract pain 34 was missing: `Key::Char` names the key by
    /// what it types with NO modifier, so a chord on shifted
    /// punctuation is spellable at all. The engine cannot press a real
    /// key — what it CAN pin is that the vocabulary is unambiguous and
    /// that a shell reporting the shifted twin produces a pattern that
    /// does not match, which is exactly the bug the port hit.
    #[test]
    fn a_chord_names_the_key_not_the_character_it_types() {
        use crate::action::{Key, KeyPattern};

        let spec = KeyPattern::command_shift(Key::Char('\\'));
        // what a shell that reports the SHIFTED character would build
        let shifted_twin = KeyPattern::command_shift(Key::Char('|'));
        assert_ne!(spec, shifted_twin, "the twin is a DIFFERENT pattern — hence the dead chord");

        // the keymap matches on the pattern whole, so the spec answers
        // only for the spec
        let runtime = Runtime::new();
        let split = crate::action::ActionId("test.split");
        runtime.bind(spec, split);
        assert_eq!(runtime.match_key(&spec), Some(split));
        assert_eq!(
            runtime.match_key(&shifted_twin),
            None,
            "a shell that hands over the shifted char finds nothing bound"
        );

        // and the letters that survived by accident: lowercasing is why
        // command-shift-G lives, and it says nothing about punctuation
        assert_eq!(
            KeyPattern::command_shift(Key::Char('g')),
            KeyPattern::command_shift(Key::Char('G'.to_ascii_lowercase()))
        );
    }
}
