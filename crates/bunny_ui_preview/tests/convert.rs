//! The conversions against a living runtime: a frame the framework laid
//! out goes to draws and back unchanged, the traits round-trip with a
//! theme, a patch is the modifier it names.

use std::collections::HashMap;

use bunny_ui_core as bunny_ui;
use bunny_ui::image_engine::ImageSource;
use bunny_ui::prelude::*;
use bunny_ui::preview::{Preview, Traits as CoreTraits};
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
    assert!(matches!(inner, ImageSource::Rgba { key: 7, .. }));
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
