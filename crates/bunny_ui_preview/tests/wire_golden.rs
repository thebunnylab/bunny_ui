//! The wire against its golden bytes: every message and every draw,
//! encoded, must match `tests/fixtures/wire_v1.bin` byte for byte — a
//! change to those bytes is a change to the wire, and the wire is
//! frozen. Set `BUNNY_WIRE_WRITE=1` to write the fixture anew, on
//! purpose, when a new kind joins it.

use std::path::Path;

use bunny_ui_preview::wire::*;

fn color(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color { r, g, b, a }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { origin: Point { x, y }, size: Size { width: w, height: h } }
}

fn corners(radius: f64) -> Corners {
    Corners { top_left: radius, top_right: radius, bottom_right: radius, bottom_left: radius }
}

fn traits() -> Traits {
    Traits {
        scheme: Scheme::Dark,
        theme: Some(vec![(String::from("canvas"), color(1, 2, 3, 255)), (String::from("accent"), color(9, 8, 7, 200))]),
        size: Sizing::Device(Device::IPhone16),
        locale: Some(String::from("pt-BR")),
        direction: Some(Direction::RightToLeft),
        size_class: Some(SizeClass::Compact),
        scale: Some(3.0),
        safe_area: Some(Edges { top: 59.0, trailing: 0.0, bottom: 34.0, leading: 0.0 }),
    }
}

/// One of every draw, in a frame with every other field set.
fn frame() -> Frame {
    let font = Font { size: 13.0, weight: Weight::Semibold, design: Design::Mono, slant: Slant::Italic, family: Some(1), tracking: 0.5 };
    Frame {
        preview: String::from("home"),
        variant: 1,
        seq: 42,
        generation: 3,
        size: Size { width: 320.0, height: 200.0 },
        scale: 2.0,
        background: color(255, 255, 255, 255),
        strings: vec![String::from("Hello, wire"), String::from("Geist Mono"), String::from("src/lib.rs"), String::from("text"), String::from("padding"), String::from("w0/Home"), String::from("webview")],
        commands: vec![
            Draw::FillRect { rect: rect(0.0, 0.0, 320.0, 200.0), color: color(250, 250, 252, 255), corners: corners(0.0) },
            Draw::Gradient {
                rect: rect(10.0, 10.0, 100.0, 50.0),
                paint: GradientPaint::Linear { start: Point { x: 10.0, y: 10.0 }, end: Point { x: 110.0, y: 60.0 }, from: color(1, 2, 3, 255), to: color(4, 5, 6, 255) },
                corners: corners(4.0),
            },
            Draw::Gradient {
                rect: rect(10.0, 70.0, 100.0, 50.0),
                paint: GradientPaint::Radial { center: Point { x: 60.0, y: 95.0 }, start: 0.0, end: 50.0, aspect: 1.0, inner: color(7, 7, 7, 255), outer: color(8, 8, 8, 0) },
                corners: corners(0.0),
            },
            Draw::Backdrop {
                rect: rect(120.0, 10.0, 80.0, 80.0),
                glass: Glass {
                    blur: 12.0,
                    tint: color(255, 255, 255, 40),
                    refraction_band: 8.0,
                    refraction_amount: 2.0,
                    chromatic: 0.5,
                    highlight: color(255, 255, 255, 120),
                    highlight_band: 3.0,
                    highlight_intensity: 0.8,
                    saturation: 1.2,
                    brightness: 1.05,
                    sheen: 0.3,
                    spot_center: Point { x: 140.0, y: 30.0 },
                    spot_radius: 40.0,
                    spot_alpha: 0.25,
                },
                corners: corners(16.0),
            },
            Draw::Shadow { rect: rect(20.0, 130.0, 60.0, 40.0), radius: 10.0, color: color(0, 0, 0, 90), corners: corners(6.0) },
            Draw::StrokeRect { rect: rect(20.0, 130.0, 60.0, 40.0), color: color(0, 0, 0, 255), width: 1.0, corners: corners(6.0) },
            Draw::PushClip { rect: rect(0.0, 0.0, 160.0, 100.0), corners: corners(2.0) },
            Draw::TextLine { origin: Point { x: 24.0, y: 140.0 }, text: 0, range: (0, 5), color: color(20, 20, 25, 255), font },
            Draw::Image { rect: rect(200.0, 120.0, 64.0, 64.0), image: ImageRef { key: 0xdead_beef, alpha: 128, mirrored: true } },
            Draw::PopClip,
        ],
        sites: vec![
            SiteBox { index: 0, parent: None, kind: 3, modifier: None, file: Some(2), name: None, line: 12, column: 5, frame: rect(0.0, 0.0, 320.0, 200.0), path: Some(5) },
            SiteBox { index: 1, parent: Some(0), kind: 3, modifier: Some(4), file: None, name: Some(3), line: 0, column: 0, frame: rect(24.0, 140.0, 60.0, 16.0), path: None },
        ],
        hosts: vec![HostBox { index: 2, kind: 6, frame: rect(200.0, 0.0, 120.0, 100.0) }],
        metrics: Metrics { build_ms: 412.5, layout_ms: 0.8, body_passes: 1, commands: 10 },
    }
}

/// Every message of both directions, once.
fn every_message() -> Vec<Message> {
    vec![
        Message::ToIde(ToIde::Hello {
            wire: WIRE_VERSION,
            framework: String::from("0.2.2"),
            pid: 4242,
            generation: 1,
            workspace_root: String::from("/w"),
            package: String::from("app"),
            caps: 0b11,
        }),
        Message::ToIde(ToIde::Ready {
            generation: 1,
            previews: vec![PreviewInfo {
                id: String::from("src/lib.rs#home"),
                ident: String::from("home"),
                name: String::from("Home"),
                scope: String::from("Screens"),
                example: String::from("home"),
                module: String::from("app"),
                file: String::from("src/lib.rs"),
                line: 40,
                traits: traits(),
            }],
            missing: vec![String::from("gone")],
        }),
        Message::ToIde(ToIde::Frame(frame())),
        Message::ToIde(ToIde::Resource(Resource { key: 1, kind: ResourceKind::Rgba { size: (2, 1), rgba: vec![1, 2, 3, 4, 5, 6, 7, 8] } })),
        Message::ToIde(ToIde::Resource(Resource { key: 2, kind: ResourceKind::Encoded { bytes: vec![0x89, b'P', b'N', b'G'] } })),
        Message::ToIde(ToIde::Resource(Resource {
            key: 3,
            kind: ResourceKind::Vector {
                verbs: vec![Verb::Move(0.0, 0.0), Verb::Line(1.0, 0.0), Verb::Quad(1.0, 1.0, 0.0, 1.0), Verb::Cubic(0.0, 0.5, 0.5, 0.0, 0.0, 0.0), Verb::Close],
                paint: Paint::Stroke { width: 1.5 },
                ink: Ink::Ramp(Gradient::Radial { center: Point { x: 0.5, y: 0.5 }, start: 0.0, end: None, aspect: 1.0, inner: color(1, 1, 1, 255), outer: color(2, 2, 2, 255) }),
                box_size: (24.0, 24.0),
            },
        })),
        Message::ToIde(ToIde::Resource(Resource { key: 4, kind: ResourceKind::Symbol { name: String::from("check"), color: color(0, 128, 0, 255), forced: false } })),
        Message::ToIde(ToIde::Resource(Resource { key: 5, kind: ResourceKind::FileIcon { path: String::from("src/main.rs") } })),
        Message::ToIde(ToIde::Resource(Resource { key: 6, kind: ResourceKind::Feed { generation: 9, size: (1, 1), rgba: vec![0, 0, 0, 255] } })),
        Message::ToIde(ToIde::Status { text: String::from("building") }),
        Message::ToIde(ToIde::Panicked { preview: String::from("home"), message: String::from("index out of bounds") }),
        Message::ToIde(ToIde::Panic { message: String::from("the host is ending") }),
        Message::ToIde(ToIde::Snapshot { preview: String::from("home"), variant: 0, size: (1, 1), rgba: vec![9, 9, 9, 255] }),
        Message::ToIde(ToIde::Pong { nonce: 77 }),
        Message::ToHost(ToHost::Select(vec![Selection { id: String::from("src/lib.rs#home"), ident: String::from("home"), traits: traits() }])),
        Message::ToHost(ToHost::Retrait { id: String::from("src/lib.rs#home"), traits: Traits::default() }),
        Message::ToHost(ToHost::Resize { id: String::from("src/lib.rs#home"), size: Size { width: 393.0, height: 852.0 } }),
        Message::ToHost(ToHost::Pointer { id: String::from("src/lib.rs#home"), kind: PointerKind::Down, x: 10.0, y: 20.0, modifiers: 4 }),
        Message::ToHost(ToHost::Wheel { id: String::from("src/lib.rs#home"), x: 10.0, y: 20.0, dx: 0.0, dy: -12.0 }),
        Message::ToHost(ToHost::Touch { id: String::from("src/lib.rs#home"), phase: TouchPhase::Began, touch: 7, x: 10.0, y: 20.0, taps: 2 }),
        Message::ToHost(ToHost::Magnify { id: String::from("src/lib.rs#home"), x: 10.0, y: 20.0, scale: 1.25 }),
        Message::ToHost(ToHost::Key { id: String::from("src/lib.rs#home"), key: String::from("cmd-s"), modifiers: 8 }),
        Message::ToHost(ToHost::Text { id: String::from("src/lib.rs#home"), text: String::from("ol\u{e1}") }),
        Message::ToHost(ToHost::Override {
            id: String::from("src/lib.rs#home"),
            site: SiteKey { file: String::from("src/lib.rs"), line: 44, column: 9 },
            op: OverrideOp::Replace,
            patch: Patch::PaddingEdge(Edge::Leading, 12.0),
        }),
        Message::ToHost(ToHost::Override {
            id: String::from("src/lib.rs#home"),
            site: SiteKey { file: String::from("src/lib.rs"), line: 45, column: 9 },
            op: OverrideOp::Append,
            patch: Patch::Frame { width: 100.0, height: 40.0, alignment: Alignment::Center },
        }),
        Message::ToHost(ToHost::ClearOverrides { id: String::from("src/lib.rs#home") }),
        Message::ToHost(ToHost::RequestSnapshot { id: String::from("src/lib.rs#home") }),
        Message::ToHost(ToHost::Forget { keys: vec![1, 2, 3] }),
        Message::ToHost(ToHost::Ping { nonce: 77 }),
        Message::ToHost(ToHost::Bye),
    ]
}

/// Every patch, once — each through the override message.
fn every_patch() -> Vec<Patch> {
    vec![
        Patch::Padding,
        Patch::PaddingLength(8.0),
        Patch::PaddingEdge(Edge::Top, 1.0),
        Patch::Frame { width: 1.0, height: 2.0, alignment: Alignment::Leading },
        Patch::FrameMax { width: 3.0, height: 4.0, alignment: Alignment::Trailing },
        Patch::FrameWidth(5.0, Alignment::Center),
        Patch::FrameHeight(6.0),
        Patch::BackgroundColor(color(1, 2, 3, 4)),
        Patch::ForegroundColor(color(5, 6, 7, 8)),
        Patch::CornerRadius(corners(3.0)),
        Patch::FontSize(15.0),
        Patch::Opacity(0.5),
        Patch::Border(color(9, 9, 9, 9), 2.0),
        Patch::Shadow(6.0, color(0, 0, 0, 80)),
        Patch::Bold,
        Patch::Italic,
        Patch::Hidden,
        Patch::Blur(4.0),
        Patch::LineHeight(1.4),
        Patch::FontWeight(Weight::Black),
        Patch::Monospaced,
        Patch::Clipped,
        Patch::HugHeight,
    ]
}

fn stream() -> (Vec<Message>, Vec<u8>) {
    let mut messages = every_message();
    for patch in every_patch() {
        messages.push(Message::ToHost(ToHost::Override {
            id: String::from("p"),
            site: SiteKey { file: String::from("f"), line: 1, column: 1 },
            op: OverrideOp::Hide,
            patch,
        }));
    }
    let bytes = messages.iter().flat_map(encode).collect();
    (messages, bytes)
}

#[test]
fn the_bytes_are_the_golden_ones_and_decode_to_what_was_sent() {
    let (messages, bytes) = stream();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wire_v1.bin");
    if std::env::var_os("BUNNY_WIRE_WRITE").is_some() {
        std::fs::write(&fixture, &bytes).unwrap();
    }
    let golden = std::fs::read(&fixture).expect("tests/fixtures/wire_v1.bin is committed");
    assert!(golden == bytes, "the wire's bytes moved: {} golden vs {} encoded — the wire is frozen", golden.len(), bytes.len());
    let mut reader = Reader::new();
    reader.feed(&bytes);
    let mut decoded = Vec::new();
    while let Some(received) = reader.pull().unwrap() {
        match received {
            Received::Message(message) => decoded.push(message),
            Received::Unknown { kind, .. } => panic!("an unknown kind {kind} in our own stream"),
        }
    }
    assert_eq!(decoded, messages);
}

#[test]
fn a_truncated_frame_is_malformed_and_a_stranger_kind_is_skipped() {
    let (_, bytes) = stream();
    // the first frame, cut one byte short of its payload, claims its full length
    let first_len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let mut cut = bytes[..4 + first_len].to_vec();
    cut[0..4].copy_from_slice(&(first_len as u32 + 8).to_le_bytes());
    cut.extend_from_slice(&[0; 8]);
    let mut reader = Reader::new();
    reader.feed(&cut);
    assert!(matches!(reader.pull(), Err(Malformed::Trailing { .. }) | Err(Malformed::Truncated { .. })), "{:?}", reader.pull());
    // a kind from the future, then a real one
    let mut reader = Reader::new();
    reader.feed(&[2, 0, 0, 0, 250, 1]);
    reader.feed(&encode(&Message::ToHost(ToHost::Bye)));
    assert_eq!(reader.pull().unwrap(), Some(Received::Unknown { kind: 250, len: 1 }));
    assert_eq!(reader.pull().unwrap(), Some(Received::Message(Message::ToHost(ToHost::Bye))));
}
