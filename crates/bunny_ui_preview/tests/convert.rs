//! The conversions against a living runtime: a frame the framework laid
//! out goes to draws and back unchanged, the traits round-trip with a
//! theme, a patch is the modifier it names.

use std::collections::HashMap;

use bunny_ui_core as bunny_ui;
use bunny_ui::icon::Verb::{Close, Cubic, Line, Move};
use bunny_ui::icon::{Draw, Glyph};
use bunny_ui::image_engine::{ImageSource, raster_source};
use bunny_ui::layout::{DisplayList, DrawCommand};
use bunny_ui::prelude::*;
use bunny_ui::preview::{Device, Preview, Traits as CoreTraits};
use bunny_ui::raster::rasterize_scaled;
use bunny_ui_preview::convert::{core_traits, image_ref, image_source, lower, modifier, raise, resource, site_box, traits};
use bunny_ui_preview::wire::{self, Strings};

#[derive(Clone)]
struct Page;

impl Component for Page {
    fn body(self) -> impl View {
        vstack!(
            text!("Hello, wire").font(Font::Title).foreground_color(theme::accent()),
            button(text("Tap"), || {}).padding_length(6.0),
        )
        .padding_length(12.0)
        .background_color(theme::panel())
        .corner_radius(8.0)
    }
}

#[test]
fn a_frame_goes_to_the_wire_and_back_unchanged() {
    let runtime = Runtime::new();
    let display = runtime.display_frame(&Page, Size { width: 240.0, height: 120.0 });
    assert!(display.len() > 3, "{}", display.len());
    let mut strings = Strings::new();
    let (commands, images) = lower(&display, &mut strings);
    assert_eq!(commands.len(), display.len());
    assert!(images.is_empty());
    let table = strings.into_table();
    assert!(table.iter().any(|s| s == "Hello, wire"), "{table:?}");
    let back = raise(&commands, &table, &HashMap::new()).unwrap();
    assert_eq!(back.as_slice(), display.as_slice());
    // the wire's own bytes carry it whole
    let frame = wire::Frame {
        preview: String::from("page"),
        variant: 0,
        seq: 1,
        generation: 1,
        size: wire::Size { width: 240.0, height: 120.0 },
        scale: 1.0,
        background: wire::Color { r: 0, g: 0, b: 0, a: 0 },
        strings: table.clone(),
        commands: commands.clone(),
        sites: Vec::new(),
        hosts: Vec::new(),
        metrics: wire::Metrics::default(),
    };
    let bytes = wire::encode(&wire::Message::ToIde(wire::ToIde::Frame(frame.clone())));
    let mut reader = wire::Reader::new();
    reader.feed(&bytes);
    match reader.pull().unwrap() {
        Some(wire::Received::Message(wire::Message::ToIde(wire::ToIde::Frame(decoded)))) => {
            assert_eq!(decoded, frame);
            let again = raise(&decoded.commands, &decoded.strings, &HashMap::new()).unwrap();
            assert_eq!(again.as_slice(), display.as_slice());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn images_travel_by_key_with_their_wraps_folded_in() {
    let rgba = ImageSource::Rgba { key: 7, size: (1, 1), rgba: std::rc::Rc::from([1u8, 2, 3, 4].as_slice()) };
    let wrapped = ImageSource::Faded { key: 8, inner: std::rc::Rc::new(ImageSource::Mirrored { key: 9, inner: std::rc::Rc::new(rgba.clone()) }), alpha: 128 };
    let (image, inner) = image_ref(&wrapped);
    assert_eq!(image, wire::ImageRef { key: 7, alpha: 128, mirrored: true });
    assert!(matches!(*inner, ImageSource::Rgba { key: 7, .. }));
    let sent = resource(&wrapped).expect("the innermost source has bytes");
    assert_eq!(sent.key, 7);
    let received = image_source(&sent).expect("rgba comes back");
    assert!(matches!(received, ImageSource::Rgba { key: 7, size: (1, 1), .. }));
    let symbol = resource(&ImageSource::Symbol { key: 11, symbol: bunny_ui::icon::house::CHECK, color: Color::BLACK, forced: false }).unwrap();
    assert!(matches!(image_source(&symbol), Some(ImageSource::Symbol { key: 11, .. })));
    let raised = raise(
        &[wire::Draw::Image { rect: wire::Rect::default(), image }],
        &[],
        &HashMap::from([(7u64, rgba)]),
    )
    .unwrap();
    assert!(matches!(raised.as_slice(), [bunny_ui::layout::DrawCommand::Image { source: ImageSource::Faded { alpha: 128, .. }, .. }]));
    assert!(matches!(
        raise(&[wire::Draw::Image { rect: wire::Rect::default(), image }], &[], &HashMap::new()),
        Err(bunny_ui_preview::convert::Missing::Image { key: 7 })
    ));
}

#[test]
fn traits_round_trip_with_a_theme_by_token_name() {
    let mut theme = Theme::dark();
    theme.accent = Color { r: 1, g: 2, b: 3, a: 255 };
    let core = Preview::new(text("x")).dark().theme(theme).device(Device::Pixel9).locale("ar").rtl().compact().scale(2.0).traits;
    let on_wire = traits(&core);
    assert_eq!(on_wire.scheme, wire::Scheme::Dark);
    assert_eq!(on_wire.size, wire::Sizing::Device(wire::Device::Pixel9));
    assert_eq!(on_wire.locale.as_deref(), Some("ar"));
    let tokens = on_wire.theme.as_ref().unwrap();
    assert_eq!(tokens.len(), Theme::NAMES.len());
    assert!(tokens.iter().any(|(name, color)| name == "accent" && *color == wire::Color { r: 1, g: 2, b: 3, a: 255 }));
    let back: CoreTraits = core_traits(&on_wire);
    assert_eq!(back, core);
    // a token the receiver does not know is left out, the rest kept
    let mut strange = on_wire.clone();
    strange.theme.as_mut().unwrap().push((String::from("neon"), wire::Color::default()));
    assert_eq!(core_traits(&strange), core);
}

#[test]
fn a_patch_is_the_modifier_it_names_and_a_site_keeps_its_place() {
    use bunny_ui::modifier::Modifier;
    assert!(matches!(modifier(wire::Patch::PaddingLength(4.0)), Modifier::PaddingLength(length) if length == 4.0));
    assert!(matches!(
        modifier(wire::Patch::Frame { width: 1.0, height: 2.0, alignment: wire::Alignment::Center }),
        Modifier::FrameWH(1.0, 2.0, Alignment::Center)
    ));
    assert!(matches!(modifier(wire::Patch::Hidden), Modifier::Hidden));
    assert!(matches!(modifier(wire::Patch::FontWeight(wire::Weight::Bold)), Modifier::FontWeight(Weight::Bold)));
    assert!(matches!(modifier(wire::Patch::PaddingEdge(wire::Edge::Leading, 3.0)), Modifier::PaddingEdge(Edge::Leading, 3.0)));
    let runtime = Runtime::new();
    runtime.record_sites(true);
    let _ = runtime.display_frame(&Page, Size { width: 240.0, height: 120.0 });
    let sites = runtime.sites();
    assert!(!sites.is_empty());
    let mut strings = Strings::new();
    let boxes: Vec<wire::SiteBox> = sites.iter().map(|placement| site_box(placement, &mut strings)).collect();
    let table = strings.into_table();
    let here = boxes.iter().find(|b| b.file.is_some_and(|f| table[f as usize].ends_with("convert.rs"))).expect("a site in this file");
    assert!(here.line > 10);
    assert!(!table[here.kind as usize].is_empty());
}

#[test]
fn a_place_patch_is_the_modifier_it_names() {
    use bunny_ui::modifier::Modifier;
    use bunny_ui_preview::convert::place_modifier;
    assert!(matches!(
        place_modifier(wire::PlacePatch::Position(wire::Point { x: 12.0, y: 34.0 })),
        Modifier::Position(Point { x: 12.0, y: 34.0 })
    ));
    assert!(matches!(
        place_modifier(wire::PlacePatch::Span(2, 3)),
        Modifier::Span(2, 3)
    ));
    assert!(matches!(
        place_modifier(wire::PlacePatch::Span(0, 0)),
        Modifier::Span(1, 1)
    ));
}

#[test]
fn ellipse_geometry_and_ink_round_trip_through_existing_vector_resources() {
    let size = Size {
        width: 200.0,
        height: 80.0,
    };
    let runtime = Runtime::new();
    let display = runtime.display_frame(&ellipse().foreground_color(theme::accent()), size);
    let mut strings = Strings::new();
    let (commands, sources) = lower(&display, &mut strings);
    assert_eq!(sources.len(), 1);
    let mut images = HashMap::new();
    for source in &sources {
        let sent = resource(source).expect("shape has a vector resource");
        assert!(matches!(sent.kind, wire::ResourceKind::Vector { .. }));
        images.insert(sent.key, image_source(&sent).unwrap());
    }
    let raised = raise(&commands, &strings.into_table(), &images).unwrap();
    assert_eq!(raised.as_slice(), display.as_slice());
}

// An app's own symbols: the tool knows only the house's by name, so the
// host sends the path an app symbol draws whenever one path draws it.

const PEN: Paint = Paint::Stroke { width: 2.0 };
const INK: Color = Color { r: 0x1d, g: 0x1d, b: 0x1f, a: 255 };
const ACCENT: Color = Color { r: 0x89, g: 0x57, b: 0xe5, a: 255 };
const ORANGE: Color = Color { r: 0xf7, g: 0x8c, b: 0x3c, a: 255 };

/// The arch's two curves, which its edit keeps.
const OUTER: &[Verb] = &[
    Move(4.0, 21.0),
    Line(4.0, 11.0),
    Cubic(4.0, 6.6, 7.6, 3.0, 12.0, 3.0),
    Cubic(16.4, 3.0, 20.0, 6.6, 20.0, 11.0),
    Line(20.0, 21.0),
];
const INNER: &[Verb] = &[
    Move(8.5, 21.0),
    Line(8.5, 12.0),
    Cubic(8.5, 10.1, 10.1, 8.5, 12.0, 8.5),
    Cubic(13.9, 8.5, 15.5, 10.1, 15.5, 12.0),
    Line(15.5, 21.0),
];

/// An arch over a ground line: three pens of one width, the shape most
/// of an app's icon set takes.
const ARCH: Symbol = Symbol::new(
    "app.arch",
    &Glyph {
        draws: &[
            Draw { paint: PEN, path: OUTER, tint: None },
            Draw { paint: PEN, path: INNER, tint: None },
            Draw { paint: PEN, path: &[Move(2.0, 21.0), Line(22.0, 21.0)], tint: None },
        ],
    },
);

/// The same arch with its ground line moved — an edit under hot reload.
const ARCH_EDITED: Symbol = Symbol::new(
    "app.arch",
    &Glyph {
        draws: &[
            Draw { paint: PEN, path: OUTER, tint: None },
            Draw { paint: PEN, path: INNER, tint: None },
            Draw { paint: PEN, path: &[Move(2.0, 20.0), Line(22.0, 20.0)], tint: None },
        ],
    },
);

/// Two pens, the second's table opening with verbs before its `Move` —
/// which draw nothing on their own, and must not carry on the first
/// pen's contour once the two are one path.
const LOOSE: Symbol = Symbol::new(
    "app.loose",
    &Glyph {
        draws: &[
            Draw { paint: PEN, path: &[Move(4.0, 6.0), Line(12.0, 6.0), Line(12.0, 14.0)], tint: None },
            Draw { paint: PEN, path: &[Close, Line(20.0, 20.0), Move(16.0, 18.0), Line(20.0, 10.0)], tint: None },
        ],
    },
);

/// A badge with a hole: one fill under the even-odd rule.
const BADGE: Symbol = Symbol::new(
    "app.badge",
    &Glyph {
        draws: &[Draw {
            paint: Paint::Fill(Rule::EvenOdd),
            path: &[
                Move(12.0, 2.0), Line(22.0, 12.0), Line(12.0, 22.0), Line(2.0, 12.0), Close,
                Move(12.0, 8.0), Line(16.0, 12.0), Line(12.0, 16.0), Line(8.0, 12.0), Close,
            ],
            tint: None,
        }],
    },
);

/// A flame in its own colour: both pens carry the drawing's palette.
const FLAME: Symbol = Symbol::new(
    "app.flame",
    &Glyph {
        draws: &[
            Draw {
                paint: PEN,
                path: &[Move(12.0, 3.0), Cubic(18.0, 9.0, 18.0, 16.0, 12.0, 21.0), Cubic(6.0, 16.0, 6.0, 9.0, 12.0, 3.0), Close],
                tint: Some(ORANGE),
            },
            Draw { paint: PEN, path: &[Move(12.0, 12.0), Line(12.0, 17.0)], tint: Some(ORANGE) },
        ],
    },
);

/// Half in its own colour, half in the ink: two runs, one path each.
const TWO_TONE: Symbol = Symbol::new(
    "app.two-tone",
    &Glyph {
        draws: &[
            Draw { paint: PEN, path: &[Move(4.0, 4.0), Line(4.0, 20.0)], tint: Some(ORANGE) },
            Draw { paint: PEN, path: &[Move(20.0, 4.0), Line(20.0, 20.0)], tint: None },
        ],
    },
);

/// A line ending in a dot: a pen and a fill in one ink, which the house
/// unions by MAX where they overlap.
const DOTTED: Symbol = Symbol::new(
    "app.dotted",
    &Glyph {
        draws: &[
            Draw { paint: PEN, path: &[Move(5.5, 18.5), Line(18.5, 5.5)], tint: None },
            Draw {
                paint: Paint::Fill(Rule::NonZero),
                path: &[
                    Move(21.0, 5.0),
                    Cubic(21.0, 6.1, 20.1, 7.0, 19.0, 7.0),
                    Cubic(17.9, 7.0, 17.0, 6.1, 17.0, 5.0),
                    Cubic(17.0, 3.9, 17.9, 3.0, 19.0, 3.0),
                    Cubic(20.1, 3.0, 21.0, 3.9, 21.0, 5.0),
                    Close,
                ],
                tint: None,
            },
        ],
    },
);

/// An app's own `check`, drawn otherwise than the house's.
const OWN_CHECK: Symbol = Symbol::new(
    "check",
    &Glyph {
        draws: &[Draw {
            paint: Paint::Stroke { width: 3.0 },
            path: &[Move(4.0, 13.0), Line(9.0, 18.0), Line(20.0, 6.0)],
            tint: None,
        }],
    },
);

/// A glyph with nothing in it.
const BLANK: Symbol = Symbol::new("app.blank", &Glyph { draws: &[] });

/// The bytes the house rasterizes for `source` on a `side` square — the
/// one door every pipeline asks.
fn pixels(source: &ImageSource, side: usize) -> Vec<u8> {
    raster_source(&RawImages::default(), source, side, side).expect("a symbol and a path both raster").rgba.clone()
}

/// The source a tool raises from what the host sent for `source`.
fn received(source: &ImageSource) -> (wire::Resource, ImageSource) {
    let sent = resource(source).expect("a symbol has a resource");
    let back = image_source(&sent).expect("the tool draws what it was sent");
    (sent, back)
}

#[test]
fn an_app_symbol_travels_as_the_path_it_draws() {
    let readings = [
        ImageSource::symbol(ARCH, INK),
        ImageSource::symbol(LOOSE, INK),
        ImageSource::symbol(BADGE, INK),
        ImageSource::symbol(FLAME, INK),
        ImageSource::symbol_forced(FLAME, ACCENT),
        ImageSource::symbol_forced(TWO_TONE, ACCENT),
        ImageSource::symbol(OWN_CHECK, INK),
        ImageSource::symbol(BLANK, INK),
        ImageSource::symbol(ARCH, Color { a: 153, ..INK }),
    ];
    for source in &readings {
        let (sent, back) = received(source);
        assert!(matches!(sent.kind, wire::ResourceKind::Vector { .. }), "{source:?} went as {:?}", sent.kind);
        assert!(matches!(back, ImageSource::Path { .. }), "{back:?}");
        // the tool paints the app's own pixels, at every size an icon takes
        for side in [12, 16, 20, 24, 32, 48, 64] {
            assert!(pixels(&back, side) == pixels(source, side), "{source:?} differs at {side}");
        }
        // on the wire's own resource message, which every tool already reads
        let bytes = wire::encode(&wire::Message::ToIde(wire::ToIde::Resource(sent.clone())));
        assert_eq!(bytes[4], wire::kind::RESOURCE);
        let mut reader = wire::Reader::new();
        reader.feed(&bytes);
        assert!(matches!(
            reader.pull().unwrap(),
            Some(wire::Received::Message(wire::Message::ToIde(wire::ToIde::Resource(decoded)))) if decoded == sent
        ));
    }
}

#[test]
fn a_frame_of_app_symbols_paints_as_the_app_paints_it() {
    let square = |x: f64, side: f64| Rect { origin: Point { x, y: 4.0 }, size: Size { width: side, height: side } };
    let display = DisplayList::from(vec![
        DrawCommand::Image { rect: square(4.0, 24.0), source: ImageSource::symbol(ARCH, INK) },
        DrawCommand::Image { rect: square(32.0, 16.0), source: ImageSource::symbol(ARCH, INK).faded(0.4) },
        DrawCommand::Image { rect: square(52.0, 32.0), source: ImageSource::symbol(BADGE, INK).mirrored() },
        DrawCommand::Image {
            rect: square(88.0, 20.0),
            source: ImageSource::symbol_forced(TWO_TONE, ACCENT).mirrored().faded(0.6),
        },
        DrawCommand::Image { rect: square(112.0, 24.0), source: ImageSource::symbol(FLAME, INK) },
    ]);
    let mut strings = Strings::new();
    let (commands, sources) = lower(&display, &mut strings);
    let images: HashMap<u64, ImageSource> = sources
        .iter()
        .map(|source| {
            let (sent, back) = received(source);
            (sent.key, back)
        })
        .collect();
    assert!(images.values().all(|source| matches!(source, ImageSource::Path { .. })), "{images:?}");
    let raised = raise(&commands, &strings.into_table(), &images).unwrap();
    for scale in [1, 2] {
        let paint = |list: &DisplayList| rasterize_scaled(list, 140 * scale, 40 * scale, scale, Color::WHITE).to_rgba_bytes();
        let app = paint(&display);
        assert!(app.chunks(4).any(|pixel| pixel != [255, 255, 255, 255]), "the app's frame has ink at {scale}x");
        assert!(paint(&raised) == app, "the frame differs at {scale}x");
    }
}

#[test]
fn a_house_symbol_still_travels_by_name() {
    let source = ImageSource::symbol(bunny_ui::icon::house::CHECK, INK);
    let (sent, back) = received(&source);
    assert_eq!(sent.key, source.key(), "under its own identity");
    assert_eq!(sent.kind, wire::ResourceKind::Symbol { name: String::from("check"), color: INK.into(), forced: false });
    assert!(matches!(back, ImageSource::Symbol { symbol, .. } if symbol == bunny_ui::icon::house::CHECK));
    // a symbol named like a house one but drawn otherwise is the app's
    let (sent, _) = received(&ImageSource::symbol(OWN_CHECK, INK));
    assert!(matches!(sent.kind, wire::ResourceKind::Vector { .. }), "{:?}", sent.kind);
}

#[test]
fn a_glyph_one_path_cannot_draw_still_travels_by_name() {
    // a pen and a fill in one ink overlap by MAX in the house; two
    // pictures composited over each other would darken the joint
    let sent = resource(&ImageSource::symbol(DOTTED, INK)).unwrap();
    assert_eq!(sent.kind, wire::ResourceKind::Symbol { name: String::from("app.dotted"), color: INK.into(), forced: false });
    // which no tool knows by name: it draws a stand-in there, and names it
    assert!(image_source(&sent).is_none());
    // and a drawing in two inks paints one over the other
    let sent = resource(&ImageSource::symbol(TWO_TONE, INK)).unwrap();
    assert!(matches!(sent.kind, wire::ResourceKind::Symbol { ref name, .. } if name == "app.two-tone"), "{:?}", sent.kind);
}

#[test]
fn an_app_symbol_travels_under_its_drawings_identity() {
    let key = |source: &ImageSource| image_ref(source).0.key;
    let arch = ImageSource::symbol(ARCH, INK);
    assert_eq!(key(&arch), key(&ImageSource::symbol(ARCH, INK)), "one drawing in one ink, one key");
    assert_eq!(key(&arch), resource(&arch).unwrap().key, "the frame names what the resource carries");
    assert_eq!(image_ref(&arch.faded(0.5)).0, wire::ImageRef { key: key(&arch), alpha: 128, mirrored: false });
    // the host sends a key once: an edited drawing must be a new one, or
    // the tool would keep the drawing from before the reload
    assert_ne!(key(&arch), key(&ImageSource::symbol(ARCH_EDITED, INK)), "an edited drawing under the same name");
    assert_ne!(key(&arch), key(&ImageSource::symbol(ARCH, ACCENT)), "a re-tint");
    let mut strings = Strings::new();
    let display = DisplayList::from(vec![DrawCommand::Image { rect: Rect::default(), source: arch.clone() }]);
    let (commands, _) = lower(&display, &mut strings);
    assert!(matches!(commands.as_slice(), [wire::Draw::Image { image, .. }] if image.key == key(&arch)));
}
