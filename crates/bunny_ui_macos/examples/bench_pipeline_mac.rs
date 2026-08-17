//! The frame harness with the PLATFORM text engine (CoreText) — the twin
//! of bunny-ui's `bench_pipeline` with real shaping, for a fair
//! comparison against any baseline that also shapes real text.
//!
//! ```sh
//! cargo run --release -p bunny-ui-macos --example bench_pipeline_mac
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use bunny_ui::layout::{Proposal, Size};
use bunny_ui::prelude::*;
use bunny_ui::raster::rasterize_with;
use bunny_ui_macos::CoreTextEngine;

struct CountingAllocator;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn allocation_snapshot() -> (u64, u64) {
    (ALLOCATIONS.load(Ordering::Relaxed), BYTES.load(Ordering::Relaxed))
}

const FILES: &[(&str, &str)] = &[
    ("main.rs", "src/"),
    ("layout.rs", "crates/ui/src/"),
    ("raster.rs", "crates/ui/src/"),
    ("runtime.rs", "crates/ui/src/"),
    ("reconciler.rs", "crates/ui/src/"),
    ("text_engine.rs", "crates/ui/src/"),
    ("text_input.rs", "crates/ui/src/"),
    ("views.rs", "crates/ui/src/"),
    ("modifier.rs", "crates/ui/src/"),
    ("identity.rs", "crates/engine/src/"),
    ("state.rs", "crates/engine/src/"),
    ("combine.rs", "crates/engine/src/"),
    ("loadable.rs", "crates/engine/src/"),
    ("ffi.rs", "crates/shell/src/"),
    ("window.rs", "crates/shell/src/"),
    ("text.rs", "crates/shell/src/"),
    ("Cargo.toml", ""),
    ("README.md", ""),
    ("countries_list.rs", "apps/countries/src/ui/"),
    ("country_cell.rs", "apps/countries/src/ui/"),
    ("country_details.rs", "apps/countries/src/ui/"),
    ("image_view.rs", "apps/countries/src/ui/"),
    ("error_view.rs", "apps/countries/src/ui/"),
    ("detail_row.rs", "apps/countries/src/ui/"),
    ("root.rs", "apps/countries/src/"),
    ("finder_window.rs", "examples/"),
    ("counter_window.rs", "examples/"),
    ("benchmark.rs", "tools/src/"),
    ("glyphs.rs", "tools/src/"),
    ("palette.rs", "tools/src/"),
];

const SELECT_NEXT: ActionId = ActionId("bench.select_next");
const CLEAR: Color = Color::rgba(0, 0, 0, 0);

/// Subsequence match over `dir` then `name`, no allocation — the row
/// model shares `Rc<str>`s, so a keystroke filters and rebuilds rows
/// without copying a byte of content.
fn matches(dir: &str, name: &str, needle: &str) -> bool {
    let mut haystack = dir.chars().chain(name.chars()).map(|c| c.to_ascii_lowercase());
    needle.chars().map(|c| c.to_ascii_lowercase()).all(|wanted| haystack.any(|c| c == wanted))
}

#[derive(Clone)]
struct Finder {
    query: State<String>,
    selected: State<usize>,
    files: Rc<Vec<(Rc<str>, Rc<str>)>>,
}

impl Component for Finder {
    fn body(self, _ctx: &Context) -> impl View {
        let query = self.query.get();
        let items: Vec<(usize, Rc<str>, Rc<str>)> = self
            .files
            .iter()
            .filter(|(name, dir)| query.is_empty() || matches(dir, name, &query))
            .enumerate()
            .map(|(index, (name, dir))| (index, name.clone(), dir.clone()))
            .collect();
        let count = items.len();
        let selected = self.selected;
        let selected_index = selected.get().min(count.saturating_sub(1));

        let rows = list(
            items,
            |(_, name, dir)| format!("{dir}{name}"),
            move |(index, name, dir)| {
                hstack!(
                    text(name.clone()).foreground_color(theme::fg()),
                    text(dir.clone())
                        .font(Font::Subheadline)
                        .monospaced()
                        .foreground_color(theme::fg_secondary()),
                    spacer(),
                )
                .spacing(8.0)
                .alignment(VerticalAlignment::Center)
                .padding_edge(Edge::Leading, 12.0)
                .padding_edge(Edge::Trailing, 12.0)
                .padding_edge(Edge::Top, 7.0)
                .padding_edge(Edge::Bottom, 7.0)
                .background_color(if *index == selected_index {
                    theme::row_pressed()
                } else {
                    CLEAR
                })
                .background_hovered(theme::row_hover())
                .on_click(|| {})
            },
        );

        vstack!(
            hstack!(
                text("›").foreground_color(theme::accent()),
                text_field("Search files by name…", self.query.binding()).monospaced(),
            )
            .spacing(10.0)
            .alignment(VerticalAlignment::Center)
            .padding_length(10.0),
            rows,
        )
        .alignment(HorizontalAlignment::Leading)
        .frame(640.0, 480.0)
        .background_color(theme::panel())
        .corner_radius(9.0)
        .on_action(SELECT_NEXT, move || {
            if count > 0 {
                selected.set((selected.get().min(count - 1) + 1) % count)
            }
        })
    }
}

/// The certified fixture with springs armed: rows fade and slide, the
/// region reveals through a spring. A SEPARATE component so the plain
/// rows above stay byte-identical to the certified table.
#[derive(Clone)]
struct AnimatedFinder {
    query: State<String>,
    selected: State<usize>,
    files: Rc<Vec<(Rc<str>, Rc<str>)>>,
}

impl Component for AnimatedFinder {
    fn body(self, _ctx: &Context) -> impl View {
        let query = self.query.get();
        let items: Vec<(usize, Rc<str>, Rc<str>)> = self
            .files
            .iter()
            .filter(|(name, dir)| query.is_empty() || matches(dir, name, &query))
            .enumerate()
            .map(|(index, (name, dir))| (index, name.clone(), dir.clone()))
            .collect();
        let count = items.len();
        let selected = self.selected;
        let selected_index = selected.get().min(count.saturating_sub(1));

        let rows = list(
            items,
            |(_, name, dir)| format!("{dir}{name}"),
            move |(index, name, dir)| {
                hstack!(
                    text(name.clone()).foreground_color(theme::fg()),
                    text(dir.clone())
                        .font(Font::Subheadline)
                        .monospaced()
                        .foreground_color(theme::fg_secondary()),
                    spacer(),
                )
                .spacing(8.0)
                .alignment(VerticalAlignment::Center)
                .padding_edge(Edge::Leading, 12.0)
                .padding_edge(Edge::Trailing, 12.0)
                .padding_edge(Edge::Top, 7.0)
                .padding_edge(Edge::Bottom, 7.0)
                .background_color(if *index == selected_index {
                    theme::row_pressed()
                } else {
                    CLEAR
                })
                .background_hovered(theme::row_hover())
                .animated(Spring::snappy())
                .on_click(|| {})
            },
        );

        vstack!(
            hstack!(
                text("›").foreground_color(theme::accent()),
                text_field("Search files by name…", self.query.binding()).monospaced(),
            )
            .spacing(10.0)
            .alignment(VerticalAlignment::Center)
            .padding_length(10.0),
            rows.animated(Spring::smooth()),
        )
        .alignment(HorizontalAlignment::Leading)
        .frame(640.0, 480.0)
        .background_color(theme::panel())
        .corner_radius(9.0)
    }
}

/// Ten thousand rows behind a virtual window — the scale story. The
/// filter lives in retained state and recomputes ONLY when the query
/// changes (the on_change effect) — moving the selection re-runs the
/// body without walking ten thousand rows. The other runner mirrors
/// the same hoisting for a fair ruler.
#[derive(Clone)]
struct VirtualFinder {
    query: State<String>,
    selected: State<usize>,
    visible: State<Rc<Vec<usize>>>,
    files: Rc<Vec<(Rc<str>, Rc<str>)>>,
}

impl Component for VirtualFinder {
    fn body(self, _ctx: &Context) -> impl View {
        let files = Rc::clone(&self.files);
        let visible = self.visible.get();
        let count = visible.len();
        let selected_index = self.selected.get().min(count.saturating_sub(1));
        let id_files = Rc::clone(&files);
        let id_visible = Rc::clone(&visible);
        vstack!(
            hstack!(
                text("›").foreground_color(theme::accent()),
                text_field("Search files by name…", self.query.binding()).monospaced(),
            )
            .spacing(10.0)
            .alignment(VerticalAlignment::Center)
            .padding_length(10.0),
            virtual_list(
                count,
                move |row| {
                    let (name, dir) = &id_files[id_visible[row]];
                    format!("{dir}{name}")
                },
                move |row| {
                    let (name, dir) = &files[visible[row]];
                    hstack!(
                        text(name.clone()).foreground_color(theme::fg()),
                        text(dir.clone())
                            .font(Font::Subheadline)
                            .monospaced()
                            .foreground_color(theme::fg_secondary()),
                        spacer(),
                    )
                    .spacing(8.0)
                    .alignment(VerticalAlignment::Center)
                    .padding_edge(Edge::Leading, 12.0)
                    .padding_edge(Edge::Trailing, 12.0)
                    .padding_edge(Edge::Top, 7.0)
                    .padding_edge(Edge::Bottom, 7.0)
                    .background_color(if row == selected_index {
                        theme::row_pressed()
                    } else {
                        CLEAR
                    })
                    .on_click(|| {})
                },
            )
            .reveal(selected_index),
        )
        .alignment(HorizontalAlignment::Leading)
        .frame(640.0, 480.0)
        .background_color(theme::panel())
        .corner_radius(9.0)
    }
}

/// The eager derivation a real app performs ON the keystroke — the
/// exact mirror of the other runner's update-then-refilter.
fn refilter(finder: &VirtualFinder) {
    let query = finder.query.get();
    finder.visible.set(Rc::new(
        (0..finder.files.len())
            .filter(|index| {
                let (name, dir) = &finder.files[*index];
                query.is_empty() || matches(dir, name, &query)
            })
            .collect(),
    ));
}

struct Report {
    label: &'static str,
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
    allocations: u64,
    kibibytes: u64,
}

fn measure(label: &'static str, warmup: usize, frames: usize, mut step: impl FnMut()) -> Report {
    for _ in 0..warmup {
        step();
    }
    let mut samples = Vec::with_capacity(frames);
    let (allocations_before, bytes_before) = allocation_snapshot();
    for _ in 0..frames {
        let start = Instant::now();
        step();
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let (allocations_after, bytes_after) = allocation_snapshot();
    samples.sort_by(f64::total_cmp);
    let at = |q: f64| samples[((samples.len() as f64 * q) as usize).min(samples.len() - 1)];
    Report {
        label,
        p50: at(0.50),
        p95: at(0.95),
        p99: at(0.99),
        max: *samples.last().unwrap(),
        allocations: (allocations_after - allocations_before) / frames as u64,
        kibibytes: (bytes_after - bytes_before) / frames as u64 / 1024,
    }
}

fn main() {
    let viewport = Proposal::exact(Size { width: 760.0, height: 640.0 });
    let files: Rc<Vec<(Rc<str>, Rc<str>)>> = Rc::new(
        FILES.iter().map(|(name, dir)| (Rc::from(*name), Rc::from(*dir))).collect(),
    );
    let finder =
        Finder { query: State::new(String::new()), selected: State::new(0), files: Rc::clone(&files) };
    let engine = std::rc::Rc::new(CoreTextEngine::new());
    let runtime = Runtime::new().text_engine(engine.clone());
    runtime.bind(KeyPattern::key(Key::Down), SELECT_NEXT);
    runtime.settle(&finder);
    runtime.layout(&finder, viewport);

    let result = runtime.layout(&finder, viewport);
    let field = result.fields.first().expect("field").clone();
    runtime.pointer_pressed(field.frame.origin.x + 8.0, field.frame.origin.y + 8.0);
    runtime.pointer_released(field.frame.origin.x + 8.0, field.frame.origin.y + 8.0);

    let mut reports = Vec::new();

    let mut forward = true;
    reports.push(measure("keystroke (filter+layout)", 5, 200, || {
        if forward {
            runtime.key(EditCommand::Insert("e".into()));
        } else {
            runtime.key(EditCommand::Backspace);
        }
        forward = !forward;
        runtime.settle(&finder);
        runtime.layout(&finder, viewport);
    }));

    let select = runtime.match_key(&KeyPattern::key(Key::Down)).unwrap();
    reports.push(measure("select_next (dispatch+layout)", 5, 200, || {
        runtime.dispatch_action(select);
        runtime.settle(&finder);
        runtime.layout(&finder, viewport);
    }));

    let result = runtime.layout(&finder, viewport);
    let row = result.hits.get(1).expect("row").1;
    let mut inside = true;
    reports.push(measure("hover (stamp+layout)", 5, 200, || {
        let y = row.origin.y + row.size.height / 2.0;
        runtime.pointer_moved(row.origin.x + 30.0, if inside { y } else { 4.0 });
        inside = !inside;
        runtime.layout(&finder, viewport);
    }));

    let mut down = true;
    reports.push(measure("wheel (offset+layout)", 5, 200, || {
        runtime.wheel(320.0, 300.0, 0.0, if down { -8.0 } else { 8.0 });
        down = !down;
        runtime.layout(&finder, viewport);
    }));

    // the tick path: springs mid-flight, ZERO bodies — the engine-side
    // cost of one animated frame. the selection retargets every 24
    // ticks so the flight never lands (steady state is the story).
    let animated = AnimatedFinder {
        query: State::new(String::new()),
        selected: State::new(0),
        files: Rc::clone(&files),
    };
    let anim_runtime = Runtime::new().text_engine(engine.clone());
    let window = Size { width: 760.0, height: 640.0 };
    anim_runtime.settle(&animated);
    anim_runtime.layout(&animated, viewport);
    let mut ticks = 0usize;
    reports.push(measure("animated frame (tick+layout)", 5, 200, || {
        if ticks % 24 == 0 {
            animated.selected.set(ticks / 24 % 12 + 1);
            let _ = anim_runtime.display_frame(&animated, window);
        }
        ticks += 1;
        anim_runtime.tick(1.0 / 120.0);
        std::hint::black_box(anim_runtime.animation_frame(&animated, window).len());
    }));

    let laid_out = runtime.layout(&finder, viewport);
    reports.push(measure("raster 1520×1280 @2x (paint)", 3, 60, || {
        let bitmap =
            rasterize_with(&laid_out.display, 1520, 1280, 2, Color::CANVAS, &*engine, &RawImages::default());
        std::hint::black_box(bitmap.width());
    }));

    // incremental repaint: the surface retains the frame and repaints
    // only the damage — the full frame above is the ceiling it beats
    use bunny_ui::raster::Surface;
    let mut surface = Surface::new(1520, 1280, 2, Color::CANVAS);
    surface.frame(runtime.layout(&finder, viewport).display, &*engine, &RawImages::default());
    let row = runtime.layout(&finder, viewport).hits.get(1).expect("row").1;
    let mut inside = true;
    reports.push(measure("hover repaint (damage)", 3, 200, || {
        let y = row.origin.y + row.size.height / 2.0;
        runtime.pointer_moved(row.origin.x + 30.0, if inside { y } else { 4.0 });
        inside = !inside;
        let damage = surface.frame(runtime.layout(&finder, viewport).display, &*engine, &RawImages::default());
        std::hint::black_box(damage.len());
    }));
    let mut forward = true;
    reports.push(measure("keystroke repaint (damage)", 3, 200, || {
        if forward {
            runtime.key(EditCommand::Insert("e".into()));
        } else {
            runtime.key(EditCommand::Backspace);
        }
        forward = !forward;
        runtime.settle(&finder);
        let damage = surface.frame(runtime.layout(&finder, viewport).display, &*engine, &RawImages::default());
        std::hint::black_box(damage.len());
    }));

    // the WHOLE presentation cost of a hover frame: incremental repaint
    // + damage-only sync of the RGBA mirror (what the shell blits from)
    let mut inside = true;
    reports.push(measure("hover present (repaint+rgba)", 3, 200, || {
        let y = row.origin.y + row.size.height / 2.0;
        runtime.pointer_moved(row.origin.x + 30.0, if inside { y } else { 4.0 });
        inside = !inside;
        surface.frame(runtime.layout(&finder, viewport).display, &*engine, &RawImages::default());
        std::hint::black_box(surface.rgba().len());
    }));
    // the OLD presentation cost, for the record: full byte conversion
    let bitmap = rasterize_with(
        &runtime.layout(&finder, viewport).display,
        1520,
        1280,
        2,
        Color::CANVAS,
        &*engine,
        &RawImages::default(),
    );
    reports.push(measure("full rgba conversion (old blit)", 3, 60, || {
        std::hint::black_box(bitmap.to_rgba_bytes().len());
    }));

    // the second backend: the SAME display list presented by metal —
    // walk + upload + encode + commit + the GPU itself, WAITED, so the
    // number hides nothing. the twin of the cpu paint row above.
    use bunny_ui_macos::OffscreenGpu;
    if let Some(mut gpu) = OffscreenGpu::new(1520, 1280) {
        // warm the atlas first — steady state is the story
        gpu.present_wait(&laid_out.display, 2, Color::CANVAS, &*engine, &RawImages::default());
        reports.push(measure("present GPU 1520×1280 (full)", 3, 200, || {
            gpu.present_wait(&laid_out.display, 2, Color::CANVAS, &*engine, &RawImages::default());
        }));
        // the cpu-side cost alone: commit and move on, like a window
        // does — the twin of a present that never waits for the gpu
        reports.push(measure("present GPU (encode, no wait)", 3, 200, || {
            gpu.present_nowait(&laid_out.display, 2, Color::CANVAS, &*engine, &RawImages::default());
        }));

        // an animated frame END TO END: tick + layout + encode — the
        // whole per-frame cost while springs fly (zero bodies inside)
        let mut gpu_ticks = 0usize;
        reports.push(measure("animated frame (tick+layout+encode)", 3, 200, || {
            if gpu_ticks % 24 == 0 {
                animated.selected.set(gpu_ticks / 24 % 12 + 1);
                let _ = anim_runtime.display_frame(&animated, window);
            }
            gpu_ticks += 1;
            anim_runtime.tick(1.0 / 120.0);
            let display = anim_runtime.animation_frame(&animated, window);
            gpu.present_nowait(&display, 2, Color::CANVAS, &*engine, &RawImages::default());
        }));
    }

    // ten thousand rows behind the virtual window — the scale story.
    // the fixture is generated ONCE, outside every step.
    let big: Rc<Vec<(Rc<str>, Rc<str>)>> = Rc::new(
        (0..10_000)
            .map(|index| {
                (
                    Rc::from(format!("file_{index:04}.rs")),
                    Rc::from(format!("src/mod_{:02}/", index % 100)),
                )
            })
            .collect(),
    );
    let big_finder = VirtualFinder {
        query: State::new(String::new()),
        selected: State::new(0),
        visible: State::new(Rc::new((0..10_000).collect())),
        files: big,
    };
    let big_runtime = Runtime::new().text_engine(engine.clone());
    big_runtime.settle(&big_finder);
    big_runtime.layout(&big_finder, viewport);
    big_runtime.layout(&big_finder, viewport);
    let result = big_runtime.layout(&big_finder, viewport);
    let field = result.fields.first().expect("field").clone();
    big_runtime.pointer_pressed(field.frame.origin.x + 8.0, field.frame.origin.y + 8.0);
    big_runtime.pointer_released(field.frame.origin.x + 8.0, field.frame.origin.y + 8.0);

    let mut forward = true;
    reports.push(measure("keystroke 10k (filter+layout)", 5, 200, || {
        if forward {
            big_runtime.key(EditCommand::Insert("e".into()));
        } else {
            big_runtime.key(EditCommand::Backspace);
        }
        forward = !forward;
        refilter(&big_finder);
        big_runtime.settle(&big_finder);
        big_runtime.layout(&big_finder, viewport);
    }));

    let mut down = true;
    reports.push(measure("wheel 10k (offset+layout)", 5, 200, || {
        big_runtime.wheel(320.0, 300.0, 0.0, if down { -8.0 } else { 8.0 });
        down = !down;
        big_runtime.layout(&big_finder, viewport);
    }));

    let mut far = true;
    reports.push(measure("reveal 10k (jump+layout)", 5, 200, || {
        big_finder.selected.set(if far { 9_999 } else { 0 });
        far = !far;
        big_runtime.settle(&big_finder);
        big_runtime.layout(&big_finder, viewport);
    }));

    reports.push(measure("layout 10k (no change)", 5, 200, || {
        std::hint::black_box(big_runtime.layout(&big_finder, viewport).display.len());
    }));

    // editor-class window: where a cpu full frame stops scaling and the
    // gpu must not care — the same scene laid out at 3024×1964 logical
    let editor = Proposal::exact(Size { width: 3024.0, height: 1964.0 });
    let editor_layout = runtime.layout(&finder, editor);
    reports.push(measure("raster 6048×3928 @2x (paint)", 3, 20, || {
        let bitmap =
            rasterize_with(&editor_layout.display, 6048, 3928, 2, Color::CANVAS, &*engine, &RawImages::default());
        std::hint::black_box(bitmap.width());
    }));
    if let Some(mut gpu) = OffscreenGpu::new(6048, 3928) {
        gpu.present_wait(&editor_layout.display, 2, Color::CANVAS, &*engine, &RawImages::default());
        reports.push(measure("present GPU 6048×3928 (full)", 3, 100, || {
            gpu.present_wait(&editor_layout.display, 2, Color::CANVAS, &*engine, &RawImages::default());
        }));
    }

    println!(
        "\n{:<32} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "scenario (frame =)", "p50 ms", "p95 ms", "p99 ms", "max ms", "allocs", "KiB"
    );
    println!("{}", "─".repeat(86));
    for report in &reports {
        println!(
            "{:<32} {:>8.3} {:>8.3} {:>8.3} {:>8.3} {:>8} {:>8}",
            report.label,
            report.p50,
            report.p95,
            report.p99,
            report.max,
            report.allocations,
            report.kibibytes
        );
    }
    println!("\nfixture: finder 30 rows; viewport 760×640; CoreText text (real shaping)");
    println!(
        "gpu rows: walk + upload + encode + commit + WAIT on an offscreen target — \
         nothing deferred; alloc columns count rust allocations only"
    );
}

