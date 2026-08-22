//! The finder, in a browser tab — the same scene the desktop runs:
//! ten thousand rows behind the virtual window, a live filter, spring
//! animations, all rasterized by the engine and blitted to a canvas.
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use bunny_ui::prelude::*;

#[link(wasm_import_module = "app")]
unsafe extern "C" {
    /// The APP's own door to the network. The engine opens no socket:
    /// this import belongs to the demo, and the answer comes back
    /// through [`finder_fetched`].
    fn js_fetch(pointer: *const u8, len: usize);
}

thread_local! {
    /// Where the answer of the fetch in flight goes. The task takes it
    /// with it when it is cancelled, so a late answer finds nobody.
    static ANSWER: RefCell<Option<task::Sender<String>>> = const { RefCell::new(None) };
}

/// The glue calls this when the fetch resolves — an empty body means
/// it failed. The bytes come from `bunny_alloc`, so ownership crosses
/// back here.
#[unsafe(no_mangle)]
pub extern "C" fn finder_fetched(pointer: *mut u8, len: usize) {
    let body = unsafe { String::from_raw_parts(pointer, len, len.max(1)) };
    if let Some(answer) = ANSWER.with(|slot| slot.borrow_mut().take()) {
        let _ = answer.send(body);
    }
}

// MARK: - The escape hatch, in a browser tab

/// A box the APP paints: it takes the keyboard on a click, types, and
/// follows the pointer with a hairline. On the canvas page it is more
/// commands in the same frame; on the element page it becomes a canvas
/// island - the app's pixels inside a tree of real elements.
#[derive(Clone, Copy)]
struct Scratch {
    note: State<Arc<str>>,
    /// Where the pointer last was, in the box's own coordinates.
    mark: State<f64>,
}

impl Scratch {
    const HEIGHT: f64 = 46.0;
}

impl CustomElement for Scratch {
    fn name(&self) -> &str {
        "scratch"
    }

    fn accepts_keys(&self) -> bool {
        true
    }

    // the box wants the row's width, never the column's leftover —
    // the measure below pins the height and this keeps the stacks
    // from offering more
    fn flexible(&self, _axis: bunny_ui::layout::Axis) -> bool {
        false
    }

    fn measure(&self, proposal: bunny_ui::layout::Proposal, _metrics: &Metrics) -> Size {
        Size { width: proposal.width.unwrap_or(0.0), height: Self::HEIGHT }
    }

    fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
        let note = self.note.get();
        painter.fill_rounded(ctx.bounds(), theme::row_hover(), 8.0);
        let ink = if ctx.focused { theme::fg() } else { theme::fg_faint() };
        let origin = Point { x: 12.0, y: 14.0 };
        painter.text(origin, note.clone(), ink);
        // the caret is the app's: the runtime only hands over the phase
        if ctx.caret_visible {
            let width = ctx.metrics.width(&note);
            painter.fill(
                Rect {
                    origin: Point { x: origin.x + width + 1.0, y: origin.y },
                    size: Size { width: 1.5, height: ctx.metrics.line_height() },
                },
                theme::accent(),
            );
        }
        // a hairline under the pointer — proof the moves arrive
        painter.fill(
            Rect {
                origin: Point { x: self.mark.get(), y: Self::HEIGHT - 6.0 },
                size: Size { width: 2.0, height: 3.0 },
            },
            theme::accent(),
        );
    }

    fn event(&self, event: &ElementEvent, _ctx: &EventCtx) -> Response {
        match event {
            ElementEvent::Text(text) => {
                self.note.set(Arc::from(format!("{}{text}", self.note.get())));
                Response::handled()
            }
            ElementEvent::Key(stroke) if stroke.pattern.key == Key::Backspace => {
                let mut note = self.note.get().to_string();
                note.pop();
                self.note.set(Arc::from(note));
                Response::handled()
            }
            ElementEvent::PointerMoved { at, .. } | ElementEvent::PointerDown { at, .. } => {
                self.mark.set(at.x);
                Response::handled()
            }
            _ => Response::ignored(),
        }
    }
}

fn matches(dir: &str, name: &str, needle: &str) -> bool {
    let mut haystack = dir.chars().chain(name.chars()).map(|c| c.to_ascii_lowercase());
    needle.chars().map(|c| c.to_ascii_lowercase()).all(|wanted| haystack.any(|c| c == wanted))
}

/// The typed cargo a row lifts — the search field takes it.
#[derive(Clone)]
struct FileDrag {
    name: std::sync::Arc<str>,
    dir: std::sync::Arc<str>,
}

#[derive(Clone)]
struct Finder {
    query: State<String>,
    selected: State<usize>,
    /// A second click on the selected row opens its details popover.
    details: State<bool>,
    visible: State<Rc<Vec<usize>>>,
    /// Which half of the search field a dragged row is over — the
    /// preview pain 31 asks for: the app paints the landing while the
    /// hand is still moving.
    drop_hint: State<Option<&'static str>>,
    /// What the page's own manifest says — fetched by a task, so the
    /// header shows the crossing landing.
    manifest: State<Arc<str>>,
    files: Rc<Vec<(Arc<str>, Arc<str>)>>,
    /// Built ONCE (hash + registration happen per identity, never per
    /// body) — the browser decodes it and reports back.
    logo: ImageSource,
    /// The app-painted box under the header.
    scratch: Scratch,
}

impl Component for Finder {
    fn body(self, _ctx: &Context) -> impl View {
        let files = Rc::clone(&self.files);
        let visible = self.visible.get();
        let count = visible.len();
        let selected = self.selected;
        let details = self.details;
        let selected_index = selected.get().min(count.saturating_sub(1));
        let id_files = Rc::clone(&files);
        let id_visible = Rc::clone(&visible);
        // a floating panel over the theme's canvas — rounded, bordered,
        // with a soft shadow: the SAME chrome on every rendering
        vstack!(vstack!(
            hstack!(
                image(self.logo.clone()).resizable().frame(18.0, 18.0),
                icon(symbol::CHEVRON_RIGHT)
                    .foreground_color(theme::accent())
                    .tooltip("The selected file opens here"),
                {
                    let query = self.query;
                    let hint = self.drop_hint;
                    let picked = self.selected;
                    text_field("Search ten thousand files…", self.query.binding())
                        // Enter takes the first match — the field's OWN
                        // key, heard before the keymap and without a Go
                        // button the scene has not got (pain 58)
                        .on_submit(move || picked.set(0))
                        .monospaced()
                        // drop a row here: the search becomes the file,
                        // and the LEFT half searches the name while the
                        // right half searches its folder — the drop
                        // says where it landed, so one target has two
                        // meanings (pain 31)
                        .on_drop_at(move |file: &FileDrag, at| {
                            let (x, _) = at.fraction();
                            query.set(if x < 0.5 {
                                file.name.to_string()
                            } else {
                                file.dir.to_string()
                            })
                        })
                        .preview(move |at| {
                            hint.set(at.map(|at| {
                                if at.fraction().0 < 0.5 { "name" } else { "folder" }
                            }))
                        })
                },
                count_meter(count),
                text(match self.drop_hint.get() {
                    Some(half) => Arc::from(format!("drop to search by {half}")),
                    None => self.manifest.get(),
                })
                    .font_size(11.0)
                    .foreground_color(theme::fg_faint()),
            )
            .spacing(10.0)
            .alignment(VerticalAlignment::Center)
            .padding_length(10.0),
            custom(self.scratch)
                .padding_edge(Edge::Leading, 10.0)
                .padding_edge(Edge::Trailing, 10.0)
                .padding_edge(Edge::Bottom, 8.0),
            virtual_list(
                count,
                move |row| {
                    let (name, dir) = &id_files[id_visible[row]];
                    format!("{dir}{name}")
                },
                move |row| {
                    let (name, dir) = &files[visible[row]];
                    let on = row == selected_index;
                    let details = details;
                    // the path takes NO ink of its own: the row hands it
                    // down, faint at rest and bright under the pointer
                    // every fourth row reads as a PREVIEW tab would in
                    // an editor — leaning says "you are only looking"
                    let preview = row % 4 == 3;
                    let base = hstack!(
                        if preview {
                            Either::First(
                                text(name.clone()).foreground_color(theme::fg()).italic(),
                            )
                        } else {
                            Either::Second(text(name.clone()).foreground_color(theme::fg()))
                        },
                        text(dir.clone()).monospaced(),
                        spacer(),
                        // the mark the row REVEALS: it waits at zero
                        // opacity and the pointer over the ROW lights
                        // it, so the scene never swaps what it holds
                        icon(symbol::CLOSE)
                            .opacity(0.0)
                            .opacity_hovered(1.0)
                            .group_hovered()
                            .on_click(move || selected.set(row)),
                    )
                    .spacing(8.0)
                    .alignment(VerticalAlignment::Center)
                    .padding_edge(Edge::Leading, 12.0)
                    .padding_edge(Edge::Trailing, 12.0)
                    .padding_edge(Edge::Top, 6.0)
                    .padding_edge(Edge::Bottom, 6.0)
                    // the selected row wears a 2pt accent rule on its
                    // leading edge — a LAYER, so the row keeps hugging
                    // its content and a border would tint every side
                    .overlay(
                        UnitPoint::LEADING,
                        if on {
                            Either::First(
                                spacer().frame_width(2.0).background_color(theme::accent()),
                            )
                        } else {
                            Either::Second(empty())
                        },
                    )
                    .background_color(if on { theme::row_pressed() } else { CLEAR })
                    .background_hovered(theme::row_hover())
                    .foreground_color(theme::fg_secondary())
                    .foreground_hovered(theme::fg())
                    .animated(Spring::snappy())
                    // the row is the GROUP: what the pointer does to it
                    // reaches the mark inside, and the mark stays lit
                    // when the pointer finally arrives on it
                    .hover_group()
                    // one click selects; TWO open the details — the
                    // count is the platform's, and the row never has to
                    // hold a clock of its own
                    .on_click_count(move |clicks| {
                        selected.set(row);
                        if clicks >= 2 {
                            details.set(true);
                        }
                    })
                    // the right press offers the row's own menu — the
                    // runtime opens it at the pointer and closes it
                    // through the popover's doors
                    .on_drag({
                        let name = name.clone();
                        let dir = dir.clone();
                        move || {
                            drag(
                                FileDrag { name: name.clone(), dir: dir.clone() },
                                name.clone(),
                            )
                        }
                    })
                    .context_menu(vec![
                        menu_item("Open", move || {
                            selected.set(row);
                            details.set(true);
                        }),
                        menu_item("Select", move || selected.set(row)),
                        menu_divider(),
                        menu_item("Deselect", move || selected.set(0)),
                    ]);
                    if on {
                        let name = name.clone();
                        let dir = dir.clone();
                        erased(base.popover(details.binding(), Side::Trailing, move |_| {
                            details_card(name.clone(), dir.clone())
                        }))
                    } else {
                        erased(base)
                    }
                },
            )
            .reveal(selected_index)
            // the flow page owns layout in the browser, so the row
            // extent is DECLARED — the one number the window math needs
            .row_height(29.0),
        )
        .alignment(HorizontalAlignment::Leading)
        .padding_edge(Edge::Bottom, 10.0)
        .background_color(theme::panel())
        // a ramp behind the panel: the engine owns the geometry, each
        // rendering paints it its own way (a CSS radial-gradient here,
        // our own rasterizer on the canvas page)
        .background_gradient(
            Gradient::radial(theme::accent(), theme::accent().fade())
                .center(UnitPoint::TOP_LEADING)
                .radius(0.0, 260.0),
        )
        .corner_radius(12.0)
        // the cut follows the radius: a selected row's own background
        // dies at the curve instead of squaring the corner off
        .clipped()
        .border(theme::border(), 1.0)
        .shadow(28.0))
        .padding_length(28.0)
        .on_change(
            {
                let query = self.query;
                move || query.get()
            },
            false,
            {
                let files = Rc::clone(&self.files);
                let cache = self.visible;
                move |_, query: &String| {
                    cache.set(Rc::new(
                        (0..files.len())
                            .filter(|index| {
                                let (name, dir) = &files[*index];
                                query.is_empty() || matches(dir, name, query)
                            })
                            .collect(),
                    ));
                }
            },
        )
        // the page's own manifest, fetched once: the app opens the
        // request, the glue answers through `finder_fetched`, and the
        // channel wakes this task with the body
        .task({
            let manifest = self.manifest;
            move || async move {
                let (answer, reply) = task::channel::<String>();
                ANSWER.with(|slot| *slot.borrow_mut() = Some(answer));
                let url = "manifest.txt";
                unsafe { js_fetch(url.as_ptr(), url.len()) };
                if let Some(body) = reply.recv().await {
                    let line = body.lines().next().unwrap_or_default().trim();
                    if !line.is_empty() {
                        manifest.set(Arc::from(line));
                    }
                }
            }
        })
    }
}

const CLEAR: Color = Color { r: 0, g: 0, b: 0, a: 0 };

/// The details popover — the same card chrome on every rendering (and
/// on the mac, its own little window past the edge).
fn details_card(name: Arc<str>, dir: Arc<str>) -> Erased {
    erased(
        vstack!(
            text(name.clone()).bold(),
            text(format!("{dir}{name}"))
                .monospaced()
                .foreground_color(theme::fg_secondary()),
            text("press escape or click outside to close")
                .foreground_color(theme::placeholder()),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(6.0)
        .padding_length(12.0)
        .background_color(theme::panel())
        .corner_radius(10.0)
        .border(theme::border(), 1.0)
        .shadow(24.0),
    )
}

// MARK: - The logo, a png written by hand

/// Bitwise CRC-32 (the PNG polynomial) — tiny, demo-only.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in bytes {
        a = (a + *byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut body = kind.to_vec();
    body.extend_from_slice(payload);
    let mut out = (payload.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
    out
}

/// The 12×12 bunny mark, one bit per pixel — ears, head, eyes.
const BUNNY: [u16; 12] = [
    0b0110_0000_0110,
    0b0110_0000_0110,
    0b0110_0000_0110,
    0b0111_1111_1110,
    0b1111_1111_1111,
    0b1111_1111_1111,
    0b1101_1111_1011,
    0b1111_1111_1111,
    0b1111_1111_1111,
    0b0111_1111_1110,
    0b0011_1111_1100,
    0b0000_0000_0000,
];

/// A REAL png (zlib with one stored deflate block needs no
/// compressor) — the browser decodes it like any other asset.
fn logo() -> ImageSource {
    let accent = theme::accent();
    let mut raw = Vec::new();
    for row in BUNNY {
        raw.push(0); // filter: none
        for column in 0..12u16 {
            let on = row >> (11 - column) & 1 == 1;
            let alpha = if on { 255 } else { 0 };
            raw.extend_from_slice(&[accent.r, accent.g, accent.b, alpha]);
        }
    }
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&12u32.to_be_bytes());
    ihdr.extend_from_slice(&12u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    let mut idat = vec![0x78, 0x01, 0x01];
    idat.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    idat.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    idat.extend_from_slice(&raw);
    idat.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    png.extend_from_slice(&chunk(b"IHDR", &ihdr));
    png.extend_from_slice(&chunk(b"IDAT", &idat));
    png.extend_from_slice(&chunk(b"IEND", &[]));
    ImageSource::from_bytes(png)
}

/// The visible count, drawn as five digit bars — custom pixels. On the
/// Dom page this subtree claims a CANVAS ISLAND (`.rendering(Gpu)`):
/// our layout positions the element, our rasterizer fills it, and the
/// bars redraw live as the filter types. On the canvas page the
/// modifier dissolves — everything is pixels there already.
fn count_meter(count: usize) -> impl View {
    let digit = |place: u32| ((count / 10usize.pow(place)) % 10) as f64;
    let bar = |place: u32| {
        spacer()
            .frame(6.0, 6.0 + digit(place) * 2.0)
            .background_color(theme::accent())
            .corner_radius(2.0)
    };
    hstack!(bar(4), bar(3), bar(2), bar(1), bar(0))
        .spacing(2.0)
        .alignment(VerticalAlignment::Bottom)
        .rendering(Rendering::Gpu)
}

fn finder() -> Finder {
    let files: Rc<Vec<(Arc<str>, Arc<str>)>> = Rc::new(
        (0..10_000)
            .map(|index| {
                (
                    Arc::from(format!("file_{index:04}.rs")),
                    Arc::from(format!("src/mod_{:02}/", index % 100)),
                )
            })
            .collect(),
    );
    Finder {
        query: State::new(String::new()),
        selected: State::new(0),
        details: State::new(false),
        visible: State::new(Rc::new((0..10_000).collect())),
        drop_hint: State::new(None),
        manifest: State::new(Arc::from("reading the manifest…")),
        files,
        logo: logo(),
        scratch: Scratch {
            note: State::new(Arc::from("click here and type — the app paints this box")),
            mark: State::new(12.0),
        },
    }
}

/// The glue calls this once, with the canvas geometry.
#[unsafe(no_mangle)]
pub extern "C" fn start(width: f64, height: f64, scale: f64) {
    bunny_ui_web::start(width, height, scale, finder());
}

/// The Dom page calls this one — the SAME scene, rendered at home.
/// `scale` rasters the canvas islands at the device's density;
/// `hydrate` says the page shipped painted and the boot adopts it.
#[unsafe(no_mangle)]
pub extern "C" fn start_dom(width: f64, height: f64, scale: f64, hydrate: u32) {
    if hydrate != 0 {
        bunny_ui_web::start_dom_hydrated(width, height, scale, finder());
    } else {
        bunny_ui_web::start_dom(width, height, scale, finder());
    }
}

/// Reports what the scroll region and the window math actually see, so
/// a browser can say it instead of a guess. Writes six numbers at
/// `out`: the region's frame height, its content height, the declared
/// row extent, the retained offset, how many text runs the frame
/// painted, and how many rows the viewport has room for.
#[unsafe(no_mangle)]
pub extern "C" fn finder_debug_window(width: f64, height: f64, out: *mut f64) {
    use bunny_ui::layout::{DrawCommand, Proposal, Size};
    let size = Size { width, height };
    let runtime = bunny_ui::runtime::Runtime::new();
    let root = finder();
    let _ = runtime.display_frame(&root, size);
    let result = runtime.layout(&root, Proposal::exact(size));
    let region = result.scrolls.first().cloned();
    let display = runtime.display_frame(&root, size);
    let painted = display
        .iter()
        .filter(|command| matches!(command, DrawCommand::TextLine { .. }))
        .count();
    let numbers = [
        region.as_ref().map_or(0.0, |r| r.frame.size.height),
        region.as_ref().map_or(0.0, |r| r.content.height),
        region.as_ref().and_then(|r| r.row_extent).unwrap_or(0.0),
        region.as_ref().map_or(0.0, |r| r.frame.origin.y),
        painted as f64,
        result.scrolls.len() as f64,
    ];
    unsafe { std::ptr::copy_nonoverlapping(numbers.as_ptr(), out, numbers.len()) };
}
