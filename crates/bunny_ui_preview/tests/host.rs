//! The host against hand-written entries exported from this very binary:
//! it lists them, renders the variants a selection asks for, keeps the
//! other scenes rendering when one panics, and answers the small talk.

#![cfg(not(target_arch = "wasm32"))]

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use bunny_ui_core as bunny_ui;
use bunny_ui::prelude::*;
use bunny_ui_hot::preview::{Meta, Output, PreviewEntry};
use bunny_ui::image_engine::ImageSource;
use bunny_ui::layout::HostPlacement;
use bunny_ui_preview::host::{Config, Engines, Host, HostId, Pictures};
use bunny_ui_preview::wire::{self, Selection, ToHost, ToIde, Traits};

#[derive(Clone)]
struct Card;

impl Component for Card {
    fn body(self) -> impl View {
        vstack!(text!("card").foreground_color(theme::fg())).padding_length(8.0).background_color(theme::canvas())
    }
}

#[derive(Clone)]
struct Boom;

impl Component for Boom {
    fn body(self) -> impl View {
        if true {
            panic!("the preview that cannot be built");
        }
        text("never")
    }
}

fn make_card() -> Output {
    Output::of(Card)
}

fn make_boom() -> Output {
    Output::of(Boom)
}

/// A box that zooms under two fingers, and says how far.
#[derive(Clone)]
struct Zoomer {
    scale: State<f64>,
}

struct Pinch {
    scale: State<f64>,
}

impl CustomElement for Pinch {
    fn paint(&self, _: &PaintCtx, _: &mut Painter) {}

    fn event(&self, event: &ElementEvent, _: &EventCtx) -> Response {
        match event {
            ElementEvent::Magnify { scale, .. } => {
                self.scale.set(self.scale.get() * scale);
                Response::handled()
            }
            _ => Response::ignored(),
        }
    }
}

impl Component for Zoomer {
    fn body(self) -> impl View {
        vstack!(text!("zoom {:.1}", self.scale), custom(Pinch { scale: self.scale }).frame(100.0, 100.0).id("pinch"))
    }
}

/// A button that counts, as the template's does.
#[derive(Clone)]
struct Tapper {
    taps: State<i32>,
}

impl Component for Tapper {
    fn body(self) -> impl View {
        vstack!(text!("taps {}", self.taps), button(text("tap"), move || self.taps.add(1)))
    }
}

/// A page: the OS webview in a box.
#[derive(Clone)]
struct Page;

impl Component for Page {
    fn body(self) -> impl View {
        vstack!(text("docs"), webview("https://docs.example.test").frame(160.0, 90.0))
    }
}

fn make_page() -> Output {
    Output::of(Page)
}

fn make_zoomer() -> Output {
    Output::of(Zoomer { scale: State::new(1.0) })
}

fn make_tapper() -> Output {
    Output::of(Tapper { taps: State::new(0) })
}

static CARD: PreviewEntry = PreviewEntry::new(
    Meta { ident: "card", name: "Card", scope: "Cards", example: "card", module: "host", file: "src/lib.rs", line: 3 },
    make_card,
);
static BOOM: PreviewEntry = PreviewEntry::new(
    Meta { ident: "boom", name: "Boom", scope: "Cards", example: "boom", module: "host", file: "src/lib.rs", line: 8 },
    make_boom,
);

static ZOOMER: PreviewEntry = PreviewEntry::new(
    Meta { ident: "zoomer", name: "Zoomer", scope: "Gestures", example: "zoomer", module: "host", file: "src/lib.rs", line: 11 },
    make_zoomer,
);
static TAPPER: PreviewEntry = PreviewEntry::new(
    Meta { ident: "tapper", name: "Tapper", scope: "Gestures", example: "tapper", module: "host", file: "src/lib.rs", line: 14 },
    make_tapper,
);
static PAGE: PreviewEntry = PreviewEntry::new(
    Meta { ident: "page", name: "Page", scope: "Hosts", example: "page", module: "host", file: "src/lib.rs", line: 17 },
    make_page,
);

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub fn bunny_preview__page() -> &'static PreviewEntry {
    &PAGE
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub fn bunny_preview__card() -> &'static PreviewEntry {
    &CARD
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub fn bunny_preview__zoomer() -> &'static PreviewEntry {
    &ZOOMER
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub fn bunny_preview__tapper() -> &'static PreviewEntry {
    &TAPPER
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub fn bunny_preview__boom() -> &'static PreviewEntry {
    &BOOM
}

#[derive(Clone)]
struct PlacedCard;
impl Component for PlacedCard {
    fn body(self) -> impl View {
        vstack!(
            rectangle().frame(200.0, 100.0),
            text!("positioned").position(10.0, 20.0)
        )
    }
}

#[derive(Clone)]
struct GridCard;
impl Component for GridCard {
    fn body(self) -> impl View {
        grid!(text!("a").span(1, 1), text!("b"), text!("c"), text!("d"))
            .columns(2)
            .spacing(4.0)
            .frame_width(100.0)
    }
}

fn make_placed() -> Output {
    Output::of(PlacedCard)
}
fn make_grid() -> Output {
    Output::of(GridCard)
}
static PLACED: PreviewEntry = PreviewEntry::new(
    Meta { ident: "placed", name: "Placed", scope: "Layout", example: "placed", module: "host", file: "src/lib.rs", line: 20 }, make_placed,
);
static GRID: PreviewEntry = PreviewEntry::new(
    Meta { ident: "grid_card", name: "Grid", scope: "Layout", example: "grid_card", module: "host", file: "src/lib.rs", line: 23 }, make_grid,
);
#[unsafe(export_name = "bunny_preview__placed")]
pub fn placed_entry() -> &'static PreviewEntry {
    &PLACED
}
#[unsafe(export_name = "bunny_preview__grid_card")]
pub fn grid_entry() -> &'static PreviewEntry {
    &GRID
}

/// An app's own mark that no single path draws: a pen and a filled dot in
/// one ink, which the house piles in by MAX.
const DOT_LINE: Symbol = Symbol::new(
    "app.dot-line",
    &bunny_ui::icon::Glyph {
        draws: &[
            bunny_ui::icon::Draw { paint: Paint::Stroke { width: 2.0 }, path: &[Verb::Move(5.5, 18.5), Verb::Line(18.5, 5.5)], tint: None },
            bunny_ui::icon::Draw {
                paint: Paint::Fill(Rule::NonZero),
                path: &[Verb::Move(21.0, 5.0), Verb::Cubic(21.0, 7.8, 17.0, 7.8, 17.0, 5.0), Verb::Cubic(17.0, 2.2, 21.0, 2.2, 21.0, 5.0), Verb::Close],
                tint: None,
            },
        ],
    },
);

#[derive(Clone)]
struct Glyphs;
impl Component for Glyphs {
    fn body(self) -> impl View {
        icon(DOT_LINE)
    }
}

fn make_glyphs() -> Output {
    Output::of(Glyphs)
}
static GLYPHS: PreviewEntry = PreviewEntry::new(
    Meta { ident: "glyphs", name: "Glyphs", scope: "Images", example: "glyphs", module: "host", file: "src/lib.rs", line: 26 }, make_glyphs,
);
#[unsafe(export_name = "bunny_preview__glyphs")]
pub fn glyphs_entry() -> &'static PreviewEntry {
    &GLYPHS
}

/// A manifest directory whose `src/lib.rs` the scan reads: the previews
/// above, and one the binary does not export. Written ONCE per process:
/// every test in this binary reads it, and the tests run in parallel, so a
/// write per call — a truncate, then the bytes — could land while another
/// test's `hello` scanned the file and listed nothing.
fn manifest_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
    let dir = std::env::temp_dir().join(format!("bunny-preview-host-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.rs"),
        "use bunny_ui::prelude::*;\n\n#[preview(name = \"Card\", scope = \"Cards\")]\nfn card() -> impl View { text(\"\") }\n\n#[preview(name = \"Boom\", scope = \"Cards\")]\nfn boom() -> impl View { text(\"\") }\n\n#[preview]\nfn ghost() -> impl View { text(\"\") }\n\n#[preview(name = \"Zoomer\", scope = \"Gestures\")]\nfn zoomer() -> impl View { text(\"\") }\n\n#[preview(name = \"Tapper\", scope = \"Gestures\")]\nfn tapper() -> impl View { text(\"\") }\n\n#[preview(name = \"Page\", scope = \"Hosts\")]\nfn page() -> impl View { text(\"\") }\n\n#[preview]\nfn placed() -> impl View { text(\"\") }\n\n#[preview]\nfn grid_card() -> impl View { text(\"\") }\n\n#[preview]\nfn glyphs() -> impl View { text(\"\") }\n\nbunny_ui::app!(root);\n",
    )
    .unwrap();
    dir
    })
    .clone()
}

fn host() -> (Host, Rc<RefCell<Vec<ToIde>>>) {
    let seen: Rc<RefCell<Vec<ToIde>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&seen);
    let host = Host::new(
        Engines::house(),
        Config { package: String::from("host"), manifest_dir: manifest_dir(), framework: "test" },
        Box::new(move |message| sink.borrow_mut().push(message)),
    );
    (host, seen)
}

fn frames(seen: &Rc<RefCell<Vec<ToIde>>>) -> Vec<wire::Frame> {
    seen.borrow_mut()
        .drain(..)
        .filter_map(|message| match message {
            ToIde::Frame(frame) => Some(frame),
            _ => None,
        })
        .collect()
}

fn fills(frame: &wire::Frame) -> Vec<wire::Color> {
    frame
        .commands
        .iter()
        .filter_map(|command| match command {
            wire::Draw::FillRect { color, .. } => Some(*color),
            _ => None,
        })
        .collect()
}

fn select(ident: &str, traits: Traits) -> ToHost {
    ToHost::Select(vec![Selection { id: format!("src/lib.rs#{ident}"), ident: ident.to_string(), traits }])
}

#[test]
fn hello_lists_what_the_binary_exports_and_names_what_it_does_not() {
    let (mut host, seen) = host();
    host.hello();
    let messages = seen.borrow_mut().drain(..).collect::<Vec<_>>();
    assert!(matches!(&messages[0], ToIde::Hello { wire: 1, package, caps, .. } if package == "host" && caps & wire::caps::MOUNT != 0));
    match &messages[1] {
        ToIde::Ready { generation: 0, previews, missing } => {
            let idents: Vec<&str> = previews.iter().map(|p| p.ident.as_str()).collect();
            assert_eq!(
                idents,
                [
                    "app",
                    "card",
                    "boom",
                    "zoomer",
                    "tapper",
                    "page",
                    "placed",
                    "grid_card",
                    "glyphs"
                ]
            );
            assert_eq!(previews[1].id, "src/lib.rs#card");
            assert_eq!(previews[1].scope, "Cards");
            assert_eq!(previews[1].traits, Traits::default());
            assert_eq!(missing, &["ghost"]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_both_selection_renders_two_scenes_in_their_own_tokens_and_only_once() {
    let (mut host, seen) = host();
    host.handle(select("card", Traits { scheme: wire::Scheme::Both, ..Traits::default() }));
    assert_eq!(host.scene_ids(), [(String::from("src/lib.rs#card"), 0), (String::from("src/lib.rs#card"), 1)]);
    assert!(host.tick(), "nothing sent: {:?} poisoned: {:?}", seen.borrow(), host.poisoned());
    let rendered = frames(&seen);
    assert_eq!(rendered.len(), 2);
    assert_eq!((rendered[0].variant, rendered[1].variant), (0, 1));
    let light: wire::Color = Theme::light().canvas.into();
    let dark: wire::Color = Theme::dark().canvas.into();
    assert!(fills(&rendered[0]).contains(&light));
    assert!(fills(&rendered[1]).contains(&dark));
    assert_eq!(rendered[0].background, light);
    assert_eq!(rendered[1].background, dark);
    assert!(rendered[0].size.width > 0.0 && rendered[0].size.height > 0.0, "fits: the view's own size");
    assert!(!rendered[0].sites.is_empty(), "sites ride the frame");
    // settled: nothing more to send
    assert!(!host.tick());
    assert!(frames(&seen).is_empty());
    // a retrait moves the scene and sends again
    host.handle(ToHost::Retrait { id: String::from("src/lib.rs#card"), traits: Traits { scheme: wire::Scheme::Both, size: wire::Sizing::Fixed(wire::Size { width: 200.0, height: 100.0 }), ..Traits::default() } });
    assert!(host.tick(), "nothing sent: {:?} poisoned: {:?}", seen.borrow(), host.poisoned());
    let again = frames(&seen);
    assert_eq!(again.len(), 2);
    assert_eq!(again[0].size, wire::Size { width: 200.0, height: 100.0 });
    assert!(again[0].seq > rendered[0].seq);
    assert!(again[1].seq > rendered[1].seq);
}

#[test]
fn a_panic_in_one_scene_is_reported_and_the_other_keeps_rendering() {
    let (mut host, seen) = host();
    host.handle(ToHost::Select(vec![
        Selection { id: String::from("src/lib.rs#card"), ident: String::from("card"), traits: Traits::default() },
        Selection { id: String::from("src/lib.rs#boom"), ident: String::from("boom"), traits: Traits::default() },
    ]));
    host.tick();
    let messages = seen.borrow_mut().drain(..).collect::<Vec<_>>();
    assert!(messages.iter().any(|m| matches!(m, ToIde::Frame(frame) if frame.preview == "card")), "{messages:?}");
    assert!(messages.iter().any(|m| matches!(m, ToIde::Panicked { preview, message } if preview == "boom" && message.contains("cannot be built"))), "{messages:?}");
    assert_eq!(host.poisoned(), ["src/lib.rs#boom"]);
    // the card renders on, poisoned scene left alone
    host.handle(ToHost::Resize { id: String::from("src/lib.rs#card"), size: wire::Size { width: 120.0, height: 60.0 } });
    assert!(host.tick(), "nothing sent: {:?} poisoned: {:?}", seen.borrow(), host.poisoned());
    let rendered = frames(&seen);
    assert_eq!(rendered.len(), 1);
    assert_eq!(rendered[0].preview, "card");
    assert_eq!(rendered[0].size, wire::Size { width: 120.0, height: 60.0 });
}

#[test]
fn the_small_talk_is_answered() {
    let (mut host, seen) = host();
    host.handle(ToHost::Ping { nonce: 9 });
    host.handle(ToHost::RequestSnapshot { id: String::from("x") });
    host.handle(ToHost::Forget { keys: vec![1] });
    let messages = seen.borrow_mut().drain(..).collect::<Vec<_>>();
    assert!(matches!(messages[0], ToIde::Pong { nonce: 9 }));
    assert!(matches!(&messages[1], ToIde::Status { text } if text.contains("not selected")));
    host.handle(select("card", Traits::default()));
    host.handle(ToHost::Override {
        id: String::from("src/lib.rs#card"),
        site: wire::SiteKey { file: String::from("src/lib.rs"), line: 3, column: 1 },
        op: wire::OverrideOp::Replace,
        patch: wire::Patch::Padding,
    });
    assert_eq!(host.overrides_of("src/lib.rs#card"), 1);
    host.handle(ToHost::ClearOverrides { id: String::from("src/lib.rs#card") });
    assert_eq!(host.overrides_of("src/lib.rs#card"), 0);
}

fn text_x(frame: &wire::Frame) -> f64 {
    frame
        .commands
        .iter()
        .find_map(|command| match command {
            wire::Draw::TextLine { origin, .. } => Some(origin.x),
            _ => None,
        })
        .expect("a text line")
}

/// The site of the card's `.padding_length(8.0)`, read off a frame.
fn padding_site(frame: &wire::Frame) -> wire::SiteKey {
    let site = frame
        .sites
        .iter()
        .find(|site| site.modifier.is_some_and(|name| frame.strings.get(name as usize).is_some_and(|name| name == "padding")))
        .expect("the padding has a site");
    wire::SiteKey { file: frame.strings[site.file.expect("a recorded site") as usize].clone(), line: site.line, column: site.column }
}

/// A scrub: a second `Replace` on a site stands in for the first, so the
/// last gesture shows — and each one moves the picture.
#[test]
fn a_second_replace_on_a_site_stands_in_for_the_first() {
    let (mut host, seen) = host();
    let id = String::from("src/lib.rs#card");
    host.handle(select("card", Traits::default()));
    assert!(host.tick());
    let first = frames(&seen).remove(0);
    let site = padding_site(&first);
    let at_rest = text_x(&first);
    let gesture = |length: f64| ToHost::Override { id: id.clone(), site: site.clone(), op: wire::OverrideOp::Replace, patch: wire::Patch::PaddingLength(length) };
    host.handle(gesture(32.0));
    assert!(host.tick(), "the first gesture renders");
    let wider = text_x(&frames(&seen).remove(0));
    assert!(wider > at_rest, "{wider} > {at_rest}");
    host.handle(gesture(48.0));
    assert!(host.tick(), "the second gesture renders too");
    let widest = text_x(&frames(&seen).remove(0));
    assert!(widest > wider, "{widest} > {wider}");
    assert_eq!(host.overrides_of(&id), 1, "one override stands on the site");
    host.handle(ToHost::ClearOverrides { id: id.clone() });
    assert!(host.tick());
    assert_eq!(text_x(&frames(&seen).remove(0)), at_rest, "the code says the padding again");
}

/// A `Hide` stands in for a `Replace` on its site; an `Append` stands in
/// for an `Append` of the same modifier and stacks beside another's.
#[test]
fn an_override_stands_in_for_one_of_its_kind_and_stacks_beside_another() {
    let (mut host, _seen) = host();
    let id = String::from("src/lib.rs#card");
    let site = wire::SiteKey { file: String::from("src/lib.rs"), line: 3, column: 1 };
    let put = |op, patch| ToHost::Override { id: id.clone(), site: site.clone(), op, patch };
    host.handle(select("card", Traits::default()));
    host.handle(put(wire::OverrideOp::Replace, wire::Patch::PaddingLength(8.0)));
    host.handle(put(wire::OverrideOp::Hide, wire::Patch::Hidden));
    assert_eq!(host.overrides_of(&id), 1, "the hide took the replace's place");
    host.handle(put(wire::OverrideOp::Append, wire::Patch::PaddingLength(8.0)));
    host.handle(put(wire::OverrideOp::Append, wire::Patch::PaddingLength(12.0)));
    assert_eq!(host.overrides_of(&id), 2, "a scrub of the appended padding is one override");
    host.handle(put(wire::OverrideOp::Append, wire::Patch::Opacity(0.5)));
    assert_eq!(host.overrides_of(&id), 3, "another modifier appended stacks");
    for x in [10.0, 20.0] {
        host.handle(ToHost::Place { id: id.clone(), site: site.clone(), op: wire::OverrideOp::Append, patch: wire::PlacePatch::Position(wire::Point { x, y: 0.0 }) });
        assert_eq!(host.overrides_of(&id), 4, "position scrubs retain the style appends");
    }
    host.handle(ToHost::Place { id: id.clone(), site: site.clone(), op: wire::OverrideOp::Append, patch: wire::PlacePatch::Span(2, 1) });
    assert_eq!(host.overrides_of(&id), 5, "span and position have separate names");
}

fn fixed(width: f64, height: f64) -> Traits {
    Traits { size: wire::Sizing::Fixed(wire::Size { width, height }), ..Traits::default() }
}

fn says(frame: &wire::Frame, text: &str) -> bool {
    frame.strings.iter().any(|s| s.contains(text))
}

/// A snapshot is the frame's own pixels: the scene's display list painted
/// through the engines it was laid out with — what `Runtime::frame`
/// paints for the same view at the same size.
#[test]
fn a_snapshot_is_the_frames_pixels() {
    let (mut host, seen) = host();
    let id = String::from("src/lib.rs#card");
    host.handle(select("card", fixed(120.0, 60.0)));
    assert!(host.tick());
    let frame = frames(&seen).remove(0);
    assert_eq!(frame.size, wire::Size { width: 120.0, height: 60.0 });
    host.handle(ToHost::RequestSnapshot { id });
    let messages = seen.borrow_mut().drain(..).collect::<Vec<_>>();
    let (size, rgba) = match &messages[..] {
        [ToIde::Snapshot { preview, variant: 0, size, rgba }] if preview == "card" => (*size, rgba.clone()),
        other => panic!("{other:?}"),
    };
    assert_eq!(size, (120, 60));
    let runtime = Runtime::new();
    let display = runtime.display_frame(&Card, bunny_ui::layout::Size { width: 120.0, height: 60.0 });
    let expected = bunny_ui::raster::rasterize(&display, 120, 60, Theme::light().canvas).to_rgba_bytes();
    assert_eq!(rgba.len(), expected.len());
    assert!(rgba == expected, "the snapshot is the frame painted");
    let (pixels, _) = rgba.as_chunks::<4>();
    assert!(pixels.iter().any(|px| px != &pixels[0]), "something was painted");
    host.handle(ToHost::RequestSnapshot { id: String::from("src/lib.rs#ghost") });
    assert!(matches!(&seen.borrow()[..], [ToIde::Status { text }] if text.contains("not selected")));
}

/// A magnify over a box reaches it: the pinch zooms, the label follows.
#[test]
fn a_magnify_reaches_the_box_under_the_point() {
    let (mut host, seen) = host();
    let id = String::from("src/lib.rs#zoomer");
    host.handle(select("zoomer", fixed(200.0, 200.0)));
    assert!(host.tick());
    let frame = frames(&seen).remove(0);
    assert!(says(&frame, "zoom 1.0"), "{:?}", frame.strings);
    // a custom box records no site of its own: the stack sits at the
    // origin, the label above the 100-point box — the pinch lands in
    // the box's middle
    host.handle(ToHost::Magnify { id: id.clone(), x: 50.0, y: 70.0, scale: 1.5 });
    assert!(host.tick(), "the pinch repaints");
    let frame = frames(&seen).remove(0);
    assert!(says(&frame, "zoom 1.5"), "{:?}", frame.strings);
    // beside the box, nothing zooms
    host.handle(ToHost::Magnify { id, x: 190.0, y: 190.0, scale: 2.0 });
    assert!(!host.tick());
}

/// A finger on the button counts like a click: began and ended at its
/// centre, the label says one.
#[test]
fn a_touch_on_the_button_counts() {
    let (mut host, seen) = host();
    let id = String::from("src/lib.rs#tapper");
    host.handle(select("tapper", fixed(200.0, 120.0)));
    assert!(host.tick());
    let frame = frames(&seen).remove(0);
    assert!(says(&frame, "taps 0"), "{:?}", frame.strings);
    let button = frame.sites.iter().find(|site| frame.strings.get(site.kind as usize).is_some_and(|kind| kind == "button")).expect("the button has a site");
    let (x, y) = (button.frame.origin.x + button.frame.size.width / 2.0, button.frame.origin.y + button.frame.size.height / 2.0);
    host.handle(ToHost::Touch { id: id.clone(), phase: wire::TouchPhase::Began, touch: 1, x, y, taps: 1 });
    host.handle(ToHost::Touch { id, phase: wire::TouchPhase::Ended, touch: 1, x, y, taps: 1 });
    assert!(host.tick(), "the tap repaints");
    let frame = frames(&seen).remove(0);
    assert!(says(&frame, "taps 1"), "{:?}", frame.strings);
}

/// A webview's box rides the frame with its kind and rect, and no
/// picture — the tool's placeholder is the truth — until a shell's
/// door answers, when the picture is spliced where the box stood and
/// sent as the resource it is.
#[test]
fn a_host_box_rides_the_frame_and_a_door_fills_it() {
    let (mut host, seen) = host();
    host.handle(select("page", fixed(200.0, 140.0)));
    assert!(host.tick());
    let frame = frames(&seen).remove(0);
    assert_eq!(frame.hosts.len(), 1, "{:?}", frame.hosts);
    let webview = frame.hosts[0].clone();
    assert_eq!(frame.strings.get(webview.kind as usize).map(String::as_str), Some("webview"));
    assert_eq!((webview.frame.size.width, webview.frame.size.height), (160.0, 90.0));
    assert!(!frame.commands.iter().any(|command| matches!(command, wire::Draw::Image { .. })), "no door, no picture");

    let mut host = host.with_pictures(Box::new(|placement: &HostPlacement| {
        assert!(matches!(placement.spec, bunny_ui::host::HostSpec::Webview { .. }));
        Some(ImageSource::Rgba { key: 77, size: (2, 2), rgba: Rc::from(vec![9u8; 16]) })
    }));
    assert!(host.tick(), "the door's picture is a new frame");
    let messages = seen.borrow_mut().drain(..).collect::<Vec<_>>();
    assert!(
        messages.iter().any(|message| matches!(message, ToIde::Resource(wire::Resource { key: 77, kind: wire::ResourceKind::Rgba { size: (2, 2), .. } }))),
        "the picture travels once as a resource: {messages:?}"
    );
    let frame = messages
        .iter()
        .find_map(|message| match message {
            ToIde::Frame(frame) => Some(frame.clone()),
            _ => None,
        })
        .expect("a frame followed");
    assert!(
        frame.commands.iter().any(|command| matches!(command, wire::Draw::Image { rect, image } if *rect == webview.frame && image.key == 77)),
        "the picture is drawn where the box stood: {:?}",
        frame.commands
    );
    assert_eq!(frame.hosts.len(), 1, "the box still rides the frame");
}

#[test]
fn a_glyph_no_single_path_draws_goes_by_name_then_whole_before_its_frame() {
    let (mut host, seen) = host();
    host.handle(select("glyphs", fixed(40.0, 40.0)));
    assert!(host.tick(), "nothing sent: {:?} poisoned: {:?}", seen.borrow(), host.poisoned());
    let messages = seen.borrow_mut().drain(..).collect::<Vec<_>>();
    let resources: Vec<&wire::Resource> = messages
        .iter()
        .filter_map(|message| match message {
            ToIde::Resource(resource) => Some(resource),
            _ => None,
        })
        .collect();
    // a tool that predates the glyph kind skips it and keeps the name; a
    // newer one keeps what came last under the key, the drawing
    let [named, whole] = resources.as_slice() else {
        panic!("the name, then the glyph: {resources:?}");
    };
    assert_eq!(named.key, whole.key);
    assert!(matches!(&named.kind, wire::ResourceKind::Symbol { name, .. } if name == "app.dot-line"), "{named:?}");
    assert!(matches!(&whole.kind, wire::ResourceKind::Glyph { name, draws, .. } if name == "app.dot-line" && draws.len() == 2), "{whole:?}");
    let frame_at = messages.iter().position(|message| matches!(message, ToIde::Frame(_))).expect("a frame");
    assert!(
        messages[..frame_at].iter().filter(|message| matches!(message, ToIde::Resource(_))).count() == 2,
        "both travel before the frame that draws them: {messages:?}"
    );
    let ToIde::Frame(frame) = &messages[frame_at] else { unreachable!() };
    assert!(frame.commands.iter().any(|command| matches!(command, wire::Draw::Image { image, .. } if image.key == named.key)));
}

/// Boxes by scene and path, as the host names them to a door.
type Boxes = Vec<(String, String)>;

/// A door for the tests: the picture it answers, the move it reports,
/// whether it is busy — the test's to set — and what the host asked and
/// told it.
#[derive(Clone, Default)]
struct Door {
    picture: Rc<RefCell<Option<ImageSource>>>,
    moved: Rc<Cell<bool>>,
    busy: Rc<Cell<bool>>,
    asked: Rc<RefCell<Boxes>>,
    told: Rc<RefCell<Vec<Boxes>>>,
}

impl Pictures for Door {
    fn picture(&mut self, id: HostId<'_>, _placement: &HostPlacement) -> Option<ImageSource> {
        self.asked.borrow_mut().push((id.scene.to_string(), id.path.to_string()));
        self.picture.borrow().clone()
    }

    fn turn(&mut self) -> bool {
        self.moved.replace(false)
    }

    fn busy(&self) -> bool {
        self.busy.get()
    }

    fn sweep(&mut self, alive: &[HostId<'_>]) {
        self.told.borrow_mut().push(alive.iter().map(|id| (id.scene.to_string(), id.path.to_string())).collect());
    }
}

/// A two-by-two red picture under `key`.
fn red(key: u64) -> ImageSource {
    ImageSource::Rgba { key, size: (2, 2), rgba: Rc::from([255u8, 0, 0, 255].repeat(4)) }
}

fn snapshots(seen: &Rc<RefCell<Vec<ToIde>>>) -> Vec<((u32, u32), Vec<u8>)> {
    seen.borrow()
        .iter()
        .filter_map(|message| match message {
            ToIde::Snapshot { size, rgba, .. } => Some((*size, rgba.clone())),
            _ => None,
        })
        .collect()
}

/// The door is asked by scene and path on every render; a picture that
/// moved on its turn renders the scene with the box again, and the
/// same picture does not.
#[test]
fn a_moved_picture_renders_the_scene_with_the_box_again() {
    let (host, seen) = host();
    let door = Door::default();
    let mut host = host.with_pictures(Box::new(door.clone()));
    host.handle(select("page", fixed(200.0, 140.0)));
    assert!(host.tick());
    let frame = frames(&seen).remove(0);
    assert!(!frame.commands.iter().any(|command| matches!(command, wire::Draw::Image { .. })), "no picture yet: the placeholder");
    let asked = door.asked.borrow().clone();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0].0, "p0", "the scene names the page");
    assert!(!asked[0].1.is_empty(), "and so does the path");
    assert!(!host.tick(), "nothing moved: no frame");
    *door.picture.borrow_mut() = Some(red(7));
    door.moved.set(true);
    assert!(host.tick(), "a moved picture is a new frame");
    let frame = frames(&seen).remove(0);
    assert!(frame.commands.iter().any(|command| matches!(command, wire::Draw::Image { image, .. } if image.key == 7)), "{:?}", frame.commands);
    assert!(!host.tick(), "the same picture: no frame");
}

/// A snapshot asked while the door is busy waits: the host keeps its
/// turns short, and answers with the page's pixels where the box stood
/// once the door settled.
#[test]
fn a_snapshot_asked_while_the_door_is_busy_waits_for_the_picture() {
    let (host, seen) = host();
    let door = Door::default();
    door.busy.set(true);
    let mut host = host.with_pictures(Box::new(door.clone()));
    host.handle(select("page", fixed(200.0, 140.0)));
    assert!(host.tick());
    let frame = frames(&seen).remove(0);
    let webview = frame.hosts[0].clone();
    host.handle(ToHost::RequestSnapshot { id: String::from("src/lib.rs#page") });
    assert!(host.needs_turn(), "a busy door keeps the turns short");
    host.tick();
    assert!(snapshots(&seen).is_empty(), "the snapshot waits for the picture");
    *door.picture.borrow_mut() = Some(red(9));
    door.moved.set(true);
    door.busy.set(false);
    host.tick();
    let taken = snapshots(&seen);
    assert_eq!(taken.len(), 1, "the snapshot answers once the door settled");
    let ((width, _), rgba) = &taken[0];
    let x = (webview.frame.origin.x + webview.frame.size.width / 2.0) as usize;
    let y = (webview.frame.origin.y + webview.frame.size.height / 2.0) as usize;
    let at = (y * *width as usize + x) * 4;
    assert_eq!(&rgba[at..at + 4], &[255, 0, 0, 255], "the page's red at the box's centre");
}

/// Patience runs out: the snapshot answers with the frame as it stands.
#[test]
fn a_snapshot_held_past_its_patience_answers_without_the_picture() {
    let (host, seen) = host();
    let door = Door::default();
    door.busy.set(true);
    let mut host = host.with_pictures(Box::new(door.clone())).with_patience(Duration::ZERO);
    host.handle(select("page", fixed(200.0, 140.0)));
    assert!(host.tick());
    let _ = frames(&seen);
    host.handle(ToHost::RequestSnapshot { id: String::from("src/lib.rs#page") });
    host.tick();
    assert_eq!(snapshots(&seen).len(), 1, "patience ran out: the frame as it stands");
}

/// The door hears which boxes live after the first frame, and hears
/// nothing lives when the scene leaves.
#[test]
fn the_door_is_told_what_lives_when_a_scene_leaves() {
    let (host, _seen) = host();
    let door = Door::default();
    let mut host = host.with_pictures(Box::new(door.clone()));
    host.handle(select("page", fixed(200.0, 140.0)));
    host.tick();
    let told = door.told.borrow().clone();
    let last = told.last().expect("told after the first frame");
    assert_eq!(last.len(), 1, "{told:?}");
    assert_eq!(last[0].0, "p0");
    host.handle(ToHost::Select(vec![]));
    host.tick();
    assert_eq!(door.told.borrow().last().map(Vec::len), Some(0), "nothing lives");
}

#[test]
fn hello_announces_live_placement() {
    let (mut host, seen) = host();
    host.hello();
    assert!(
        seen.borrow()
            .iter()
            .any(|m| matches!(m, ToIde::Hello { caps, .. } if caps & wire::caps::PLACE != 0))
    );
}

fn modifier_site(frame: &wire::Frame, name: &str) -> wire::SiteKey {
    let site = frame.sites.iter().find(|site| site.modifier.is_some_and(|index| frame.strings[index as usize] == name)).expect("recorded modifier");
    wire::SiteKey { file: frame.strings[site.file.unwrap() as usize].clone(), line: site.line, column: site.column }
}

#[test]
fn a_place_on_a_position_site_moves_the_child() {
    let (mut host, seen) = host();
    host.handle(select("placed", Traits::default()));
    assert!(host.tick());
    let before = frames(&seen).remove(0);
    let site = modifier_site(&before, "position");
    host.handle(ToHost::Place { id: String::from("src/lib.rs#placed"), site, op: wire::OverrideOp::Replace, patch: wire::PlacePatch::Position(wire::Point { x: 50.0, y: 60.0 }) });
    assert!(host.tick());
    let after = frames(&seen).remove(0);
    assert_eq!(text_x(&after) - text_x(&before), 40.0);
    host.handle(ToHost::ClearOverrides { id: String::from("src/lib.rs#placed") });
    assert!(host.tick());
    assert_eq!(text_x(&frames(&seen).remove(0)), text_x(&before));
}

#[test]
fn a_span_place_regrows_the_grid() {
    let (mut host, seen) = host();
    host.handle(select("grid_card", Traits::default()));
    assert!(host.tick());
    let before = frames(&seen).remove(0);
    let site = modifier_site(&before, "span");
    host.handle(ToHost::Place { id: String::from("src/lib.rs#grid_card"), site, op: wire::OverrideOp::Replace, patch: wire::PlacePatch::Span(2, 1) });
    assert!(host.tick());
    let after = frames(&seen).remove(0);
    assert_eq!(before.size.height, 36.0);
    assert_eq!(after.size.height, 56.0);
}

thread_local! {
    static APP_PANICS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn make_app() -> Output {
    assert!(!APP_PANICS.get(), "app root construction failed");
    let mut output = make_tapper();
    output.traits.size = bunny_ui::preview::Sizing::Fixed(bunny_ui::layout::Size { width: 420.0, height: 640.0 });
    output
}

static APP: PreviewEntry = PreviewEntry::new(
    Meta { ident: "app", name: "App", scope: "App", example: "host", module: "host", file: "src/lib.rs", line: 28 }, make_app,
);
#[unsafe(export_name = "bunny_preview__app")]
pub fn app_entry() -> &'static PreviewEntry {
    &APP
}

#[test]
fn hello_lists_the_app_first_with_its_scope() {
    assert_eq!(wire::APP_IDENT, bunny_ui_hot::preview::APP_IDENT);
    let (mut host, seen) = host();
    host.hello();
    let messages = seen.borrow();
    let ToIde::Ready { previews, .. } = &messages[1] else {
        panic!("no Ready")
    };
    let app = &previews[0];
    assert_eq!(
        (
            app.id.as_str(),
            app.ident.as_str(),
            app.name.as_str(),
            app.scope.as_str(),
            app.example.as_str()
        ),
        ("src/lib.rs#app", "app", "App", "App", "host")
    );
    assert_eq!(
        app.traits.size,
        wire::Sizing::Fixed(wire::Size {
            width: 420.0,
            height: 640.0
        })
    );
}

#[test]
fn the_app_scene_renders_with_real_state() {
    let (mut host, seen) = host();
    host.handle(select("app", fixed(420.0, 640.0)));
    assert!(host.tick());
    let frame = frames(&seen).remove(0);
    assert!(says(&frame, "taps 0"));
    assert_eq!(frame.size, wire::Size { width: 420.0, height: 640.0 });
    let button = frame.sites.iter().find(|site| frame.strings[site.kind as usize] == "button").unwrap();
    let (x, y) = (button.frame.origin.x + button.frame.size.width / 2.0, button.frame.origin.y + button.frame.size.height / 2.0);
    for kind in [wire::PointerKind::Down, wire::PointerKind::Up] {
        host.handle(ToHost::Pointer { id: String::from("src/lib.rs#app"), kind, x, y, modifiers: 0 });
    }
    assert!(host.tick());
    assert!(says(&frames(&seen).remove(0), "taps 1"));
    host.ready();
    host.handle(ToHost::Resize { id: String::from("src/lib.rs#app"), size: wire::Size { width: 500.0, height: 700.0 } });
    assert!(host.tick());
    assert!(
        says(&frames(&seen).remove(0), "taps 1"),
        "discovery and resizing preserve state"
    );
}

#[test]
fn an_app_root_panic_during_discovery_is_reported_without_killing_the_host() {
    let (mut host, seen) = host();
    APP_PANICS.set(true);
    host.hello();
    APP_PANICS.set(false);
    assert!(seen.borrow().iter().any(|message| matches!(message, ToIde::Panicked { preview, message } if preview == "app" && message.contains("app root construction failed"))));
    assert!(seen.borrow().iter().any(
        |message| matches!(message, ToIde::Ready { previews, .. } if previews[0].ident == "app")
    ));
    host.handle(select("app", fixed(420.0, 640.0)));
    assert!(host.tick(), "repaired app can be selected");
}

#[path = "support/instances.rs"]
mod instances;

#[path = "support/execution.rs"]
mod execution;

#[path = "support/input.rs"]
mod input;

#[path = "support/views.rs"]
mod views;

#[path = "support/state.rs"]
mod state;
