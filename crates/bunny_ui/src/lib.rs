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
pub mod effects;
pub mod erased;
pub mod ext;
pub mod icon;
pub mod image_engine;
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

/// The engine's task queue: what `.task` runs on, and the channel a
/// worker thread (or a browser callback) hands its results over.
pub mod task {
    pub use motor::task::{
        Receiver, Recv, SendError, Sender, Sleep, Spawned, channel, sleep, spawn,
    };
}

pub mod prelude {
    pub use crate::action::{ActionId, Key, KeyPattern};
    pub use crate::anim::Spring;
    pub use crate::custom::{
        Custom, CustomElement, ElementEvent, EventCtx, ImeContext, Metrics, PaintCtx, Painter,
        Response, canvas, custom,
    };
    pub use crate::erased::{CustomModifier, Erased, erased};
    pub use crate::{hstack, text, vstack, zstack};
    pub use crate::ext::ViewExt;
    pub use crate::icon::house as symbol;
    pub use crate::icon::{ICON_GRID, Symbol};
    pub use crate::image_engine::{ImageEngine, ImageRaster, ImageSource, RawImages, file_icon};
    // geometry is app vocabulary the moment the app paints a box of
    // its own (`custom(…)` / `canvas(…)`)
    pub use crate::layout::{
        Color, Gradient, Point, Proposal, Px, Rect, Rendering, Side, Size, Truncation, UnitPoint,
        VisualProps,
    };
    pub use crate::theme::{self, Theme};
    pub use crate::text_engine::{FontDesign, FontSpec, PixelFont, TextEngine, Weight};
    pub use crate::text_input::{CaretState, EditCommand};
    pub use crate::one_of::{OneOf3, OneOf4, OneOf5, OneOf6, OneOf7, OneOf8};
    pub use crate::runtime::{Edited, ImeSnapshot, Runtime};
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
        assert!(runtime.pointer_moved(400.0, 300.0));
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
        runtime.pointer_moved(5.0, 300.0);
        assert_eq!(bench.seam.get(), 120.0);
        assert_eq!(runtime.pointer_released(5.0, 300.0), None);

        // after the release the pointer is free again: moving does not drag
        runtime.pointer_moved(700.0, 300.0);
        assert_eq!(bench.seam.get(), 120.0);
        let _ = grip_path;
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

        runtime.pointer_moved(cx, cy);
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
        runtime.pointer_moved(target.origin.x + 4.0, target.origin.y + 4.0);
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
        runtime.pointer_moved(10.0, 8.0);
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
        assert!(runtime.pointer_moved(10.0, 60.0), "leaving repaints");
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
        runtime.pointer_moved(10.0, 8.0);
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
        runtime.pointer_moved(10.0, 8.0);
        runtime.pointer_pressed(10.0, 8.0);
        assert!(!runtime.tooltip_tick(), "a press ends the wait");
        assert!(!runtime.tooltip_tick());
        runtime.pointer_released(10.0, 8.0);
        runtime.pointer_moved(10.0, 8.0);
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
            &0x8000u16.to_le_bytes()[..],
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
        runtime.pointer_moved(menu.frame.origin.x + 20.0, row_y);
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
        assert!(runtime.pointer_moved(30.0, 20.0), "the lift repaints");
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
        assert!(runtime.pointer_moved(cx, cy));
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
        runtime.pointer_moved(30.0, 20.0);
        let result = runtime.layout(&view, crate::layout::Proposal::exact(size));
        let trash = result
            .drops
            .iter()
            .find(|region| region.accepts == std::any::TypeId::of::<String>())
            .expect("the decoy is a target")
            .rect;
        let (cx, cy) =
            (trash.origin.x + trash.size.width / 2.0, trash.origin.y + trash.size.height / 2.0);
        runtime.pointer_moved(cx, cy);
        assert_eq!(runtime.interaction().drag.unwrap().over, None, "the types do not meet");
        runtime.pointer_released(cx, cy);
        assert_eq!(view.wrong.get(), 0, "nothing landed");
        assert_eq!(view.landed.get(), usize::MAX);
    }

    #[test]
    fn escape_sends_the_drag_home() {
        let (runtime, view, _) = drag_board();
        runtime.pointer_pressed(20.0, 8.0);
        runtime.pointer_moved(60.0, 40.0);
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
        runtime.pointer_moved(source.origin.x + 30.0, source.origin.y + 30.0);
        assert!(runtime.interaction().drag.is_some(), "the drag lifted");
        let quarter = (
            target.origin.x + target.size.width * 0.25,
            target.origin.y + target.size.height * 0.75,
        );
        runtime.pointer_moved(quarter.0, quarter.1);
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
        runtime.pointer_moved(source.origin.x + 40.0, source.origin.y);
        runtime.pointer_moved(inside.0, inside.1);
        assert_eq!(view.seen.get(), vec!["over".to_string()], "entering speaks once");

        // moving WITHIN the same box does not re-enter, it just moves
        runtime.pointer_moved(inside.0 + 4.0, inside.1 + 4.0);
        assert_eq!(view.seen.get(), vec!["over".to_string(), "over".to_string()]);

        // leaving the box: exactly ONE None, no matter how far it goes
        runtime.pointer_moved(inside.0, target.origin.y + target.size.height + 30.0);
        runtime.pointer_moved(inside.0, target.origin.y + target.size.height + 40.0);
        let log = view.seen.get();
        assert_eq!(log.last().map(String::as_str), Some("left"));
        assert_eq!(
            log.iter().filter(|entry| *entry == "left").count(),
            1,
            "leaving speaks once, not once per move: {log:?}"
        );

        // back in, then Escape — the preview must not be left hanging
        runtime.pointer_moved(inside.0, inside.1);
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
        runtime.pointer_moved(source.origin.x + 30.0, source.origin.y + 20.0);
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
        runtime.pointer_moved(source.origin.x + 40.0, source.origin.y);
        runtime.pointer_moved(
            target.origin.x + target.size.width / 2.0,
            target.origin.y + target.size.height / 2.0,
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
                )
                .spacing(0.0)
            }
        }

        let runtime = Runtime::new();
        let result =
            runtime.layout(&Tabs, crate::layout::Proposal::exact(Size { width: 200.0, height: 90.0 }));
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
    fn the_lean_reaches_the_browser_on_the_wire() {
        use crate::text_engine::{FontSpec, Slant};

        let leaning = crate::dom::DomText {
            content: std::sync::Arc::from("preview.rs"),
            color: Color::hex(0x202531),
            inherits_ink: false,
            font: FontSpec { slant: Slant::Italic, ..FontSpec::DEFAULT },
            highlights: None,
            truncation: None,
        };
        let bytes = crate::dom::encode(&[crate::dom::DomPatch::SetText { id: 4, text: leaning }]);
        let upright = crate::dom::DomText {
            content: std::sync::Arc::from("preview.rs"),
            color: Color::hex(0x202531),
            inherits_ink: false,
            font: FontSpec::DEFAULT,
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
