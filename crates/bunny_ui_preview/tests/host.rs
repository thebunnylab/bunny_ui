//! The host against hand-written entries exported from this very binary:
//! it lists them, renders the variants a selection asks for, keeps the
//! other scenes rendering when one panics, and answers the small talk.

#![cfg(not(target_arch = "wasm32"))]

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use bunny_ui_core as bunny_ui;
use bunny_ui::prelude::*;
use bunny_ui_hot::preview::{Meta, Output, PreviewEntry};
use bunny_ui_preview::host::{Config, Engines, Host};
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

/// A manifest directory whose `src/lib.rs` the scan reads: the two
/// previews above, and one the binary does not export.
fn manifest_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bunny-preview-host-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.rs"),
        "use bunny_ui::prelude::*;\n\n#[preview(name = \"Card\", scope = \"Cards\")]\nfn card() -> impl View { text(\"\") }\n\n#[preview(name = \"Boom\", scope = \"Cards\")]\nfn boom() -> impl View { text(\"\") }\n\n#[preview]\nfn ghost() -> impl View { text(\"\") }\n\n#[preview(name = \"Zoomer\", scope = \"Gestures\")]\nfn zoomer() -> impl View { text(\"\") }\n\n#[preview(name = \"Tapper\", scope = \"Gestures\")]\nfn tapper() -> impl View { text(\"\") }\n",
    )
    .unwrap();
    dir
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
    assert!(matches!(&messages[0], ToIde::Hello { wire: 1, package, .. } if package == "host"));
    match &messages[1] {
        ToIde::Ready { generation: 0, previews, missing } => {
            let idents: Vec<&str> = previews.iter().map(|p| p.ident.as_str()).collect();
            assert_eq!(idents, ["card", "boom", "zoomer", "tapper"]);
            assert_eq!(previews[0].id, "src/lib.rs#card");
            assert_eq!(previews[0].scope, "Cards");
            assert_eq!(previews[0].traits, Traits::default());
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
    assert_eq!(again[0].seq, 2);
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

