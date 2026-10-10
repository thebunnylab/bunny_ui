//! Visible-window regression for a native-to-Metal handoff inside an outer
//! Core Animation transaction. Run on an unlocked macOS desktop:
//!
//! `cargo run -p bunny-ui-apple --example window_presentation`
//!
//! Two startup frames arrive before the outer transaction commits. The last
//! must be visible without input, including after a repeated frame, a small
//! patch and an idle resource offer. The probe reads only its own window;
//! capture unavailability is an error, never a successful pixel check.

#[cfg(target_os = "macos")]
mod macos {
    use bunny_ui::image_engine::RawImages;
    use bunny_ui::layout::{Color, Corners, DisplayList, DrawCommand, Point, Rect, Size};
    use bunny_ui::text_engine::PixelFont;
    use bunny_ui_apple::ffi::{
        CGPoint, CGRect, CGSize, Id, Sel, class, objc_autoreleasePoolPop, objc_autoreleasePoolPush,
        sel,
    };
    use bunny_ui_apple::metal::WindowPresenter;
    use std::ptr::null_mut;
    use std::time::{Duration, Instant};

    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {}
    #[allow(clashing_extern_declarations)]
    #[link(name = "objc")]
    unsafe extern "C" {
        #[link_name = "objc_msgSend"]
        fn msg_id(o: Id, s: Sel) -> Id;
        #[link_name = "objc_msgSend"]
        fn msg_void(o: Id, s: Sel);
        #[link_name = "objc_msgSend"]
        fn msg_arg(o: Id, s: Sel, a: Id);
        #[link_name = "objc_msgSend"]
        fn msg_bool(o: Id, s: Sel, a: i8);
        #[link_name = "objc_msgSend"]
        fn msg_size(o: Id, s: Sel, a: CGSize);
        #[link_name = "objc_msgSend"]
        fn msg_rect(o: Id, s: Sel, a: CGRect) -> Id;
        #[link_name = "objc_msgSend"]
        fn msg_window(o: Id, s: Sel, a: CGRect, style: u64, backing: u64, defer: i8) -> Id;
        #[link_name = "objc_msgSend"]
        fn msg_date(o: Id, s: Sel, a: f64) -> Id;
        #[link_name = "objc_msgSend"]
        fn msg_integer(o: Id, s: Sel) -> i64;
        #[link_name = "objc_msgSend"]
        fn msg_id_arg(o: Id, s: Sel, a: Id) -> Id;
        #[link_name = "objc_msgSend"]
        fn msg_id_index(o: Id, s: Sel, a: u64) -> Id;
        #[link_name = "objc_msgSend"]
        fn msg_kind(o: Id, s: Sel, class: Id) -> i8;
        #[link_name = "objc_msgSend"]
        fn msg_color(o: Id, s: Sel, x: i64, y: i64) -> Id;
        #[link_name = "objc_msgSend"]
        fn msg_rgba(o: Id, s: Sel, r: *mut f64, g: *mut f64, b: *mut f64, a: *mut f64);
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCreateImage(r: CGRect, options: u32, window: u32, image_options: u32) -> Id;
        fn CGImageRelease(image: Id);
    }

    unsafe fn has_metal_child(layer: Id) -> bool {
        unsafe {
            let children = msg_id(layer, sel("sublayers"));
            (0..msg_integer(children, sel("count")) as u64).any(|index| {
                let child = msg_id_index(children, sel("objectAtIndex:"), index);
                msg_kind(child, sel("isKindOfClass:"), class("CAMetalLayer")) != 0
            })
        }
    }

    const SIZE: Size = Size {
        width: 320.0,
        height: 240.0,
    };
    const RED: Color = Color::hex(0xff0000);
    const GREEN: Color = Color::hex(0x00ff00);

    fn scene(base: Color, patch: Color) -> DisplayList {
        DisplayList::from(vec![
            DrawCommand::FillRect {
                rect: Rect {
                    origin: Point { x: 0.0, y: 0.0 },
                    size: SIZE,
                },
                color: base,
                corner_radius: Corners::ZERO,
            },
            DrawCommand::FillRect {
                rect: Rect {
                    origin: Point { x: 32.0, y: 32.0 },
                    size: Size {
                        width: 32.0,
                        height: 32.0,
                    },
                },
                color: patch,
                corner_radius: Corners::ZERO,
            },
        ])
    }

    // This executable owns the AppKit main thread. No event handlers, timers,
    // clicks or redraw requests can repair the submitted frame while it waits.
    unsafe fn pump() {
        unsafe {
            let until = msg_date(class("NSDate"), sel("dateWithTimeIntervalSinceNow:"), 0.05);
            msg_arg(
                msg_id(class("NSRunLoop"), sel("currentRunLoop")),
                sel("runUntilDate:"),
                until,
            );
        }
    }

    unsafe fn sample(window: Id, x: i64, y: i64) -> [u8; 3] {
        unsafe {
            // IncludingWindow | BoundsIgnoreFraming | NominalResolution:
            // only this process's window, with coordinates in window points.
            let image = CGWindowListCreateImage(
                CGRect {
                    origin: CGPoint {
                        x: f64::INFINITY,
                        y: f64::INFINITY,
                    },
                    size: CGSize {
                        width: 0.0,
                        height: 0.0,
                    },
                },
                8,
                msg_integer(window, sel("windowNumber")) as u32,
                1 | 16,
            );
            assert!(!image.is_null(), "own-window capture unavailable");
            let bitmap = msg_id_arg(
                msg_id(class("NSBitmapImageRep"), sel("alloc")),
                sel("initWithCGImage:"),
                image,
            );
            let color = msg_color(bitmap, sel("colorAtX:y:"), x, y);
            let rgb = msg_id_arg(
                color,
                sel("colorUsingColorSpace:"),
                msg_id(class("NSColorSpace"), sel("sRGBColorSpace")),
            );
            assert!(!rgb.is_null(), "capture must convert to sRGB");
            let (mut r, mut g, mut b, mut a) = (0.0, 0.0, 0.0, 0.0);
            msg_rgba(
                rgb,
                sel("getRed:green:blue:alpha:"),
                &mut r,
                &mut g,
                &mut b,
                &mut a,
            );
            msg_void(bitmap, sel("release"));
            CGImageRelease(image);
            [r, g, b].map(|channel| (channel * 255.0).round() as u8)
        }
    }

    unsafe fn expect(window: Id, base: Color, patch: Color, label: &str) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            unsafe { pump() };
            let actual = unsafe { [sample(window, 200, 150), sample(window, 48, 48)] };
            let expected = [[base.r, base.g, base.b], [patch.r, patch.g, patch.b]];
            // This is a presence test, not colorimetric parity. Saturated
            // red/green and dark/white markers have distinct channel masks
            // after the display profile's conversion too.
            if actual
                .iter()
                .flatten()
                .zip(expected.iter().flatten())
                .all(|(a, b)| (*a >= 128) == (*b >= 128))
            {
                println!("{label}: {actual:?}");
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{label}: visible {actual:?}, expected {expected:?}"
            );
        }
    }

    pub fn run() {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let app = msg_id(class("NSApplication"), sel("sharedApplication"));
            let frame = CGRect {
                origin: CGPoint { x: 100.0, y: 100.0 },
                size: CGSize {
                    width: SIZE.width,
                    height: SIZE.height,
                },
            };
            let window = msg_window(
                msg_id(class("NSWindow"), sel("alloc")),
                sel("initWithContentRect:styleMask:backing:defer:"),
                frame,
                0,
                2,
                0,
            );
            let view = msg_rect(
                msg_id(class("NSView"), sel("alloc")),
                sel("initWithFrame:"),
                frame,
            );
            let layer = msg_id(msg_id(class("CALayer"), sel("alloc")), sel("init"));
            let mut presenter = WindowPresenter::attach(layer, 1.0).expect("Metal-capable desktop");
            msg_arg(view, sel("setLayer:"), layer);
            msg_bool(view, sel("setWantsLayer:"), 1);
            msg_arg(window, sel("setContentView:"), view);
            presenter.prime(SIZE.width, SIZE.height, 1);
            msg_void(window, sel("orderFrontRegardless"));
            msg_void(app, sel("finishLaunching"));
            let images = RawImages::default();
            let transaction = class("CATransaction");
            msg_void(transaction, sel("begin"));
            for base in [RED, GREEN] {
                presenter.present(
                    &scene(base, Color::WHITE),
                    SIZE,
                    1,
                    Color::BLACK,
                    &PixelFont,
                    &images,
                    false,
                );
            }
            msg_void(transaction, sel("commit"));
            expect(window, GREEN, Color::WHITE, "coalesced startup");
            for (label, patch) in [
                ("repeat", Color::WHITE),
                ("patch", Color::BLACK),
                ("restored patch", Color::WHITE),
            ] {
                presenter.present(
                    &scene(GREEN, patch),
                    SIZE,
                    1,
                    Color::BLACK,
                    &PixelFont,
                    &images,
                    false,
                );
                expect(window, GREEN, patch, label);
            }
            // Several whole frames reuse the drawable pool after coordination.
            for base in [RED, GREEN, RED, GREEN] {
                presenter.present(
                    &scene(base, Color::WHITE),
                    SIZE,
                    1,
                    Color::BLACK,
                    &PixelFont,
                    &images,
                    true,
                );
                presenter.set_transactional(false);
                expect(window, base, Color::WHITE, "whole frame after coordination");
            }
            for size in [
                Size {
                    width: 384.0,
                    height: 288.0,
                },
                SIZE,
            ] {
                msg_size(
                    window,
                    sel("setContentSize:"),
                    CGSize {
                        width: size.width,
                        height: size.height,
                    },
                );
                presenter.present(
                    &scene(GREEN, Color::WHITE),
                    size,
                    1,
                    Color::BLACK,
                    &PixelFont,
                    &images,
                    true,
                );
                presenter.set_transactional(false);
                expect(window, GREEN, Color::WHITE, "resize");
            }
            presenter.rest();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !presenter.offer_drawables() {
                assert!(Instant::now() < deadline, "drawable release did not settle");
                pump();
            }
            expect(window, GREEN, Color::WHITE, "retained while idle");
            drop(presenter);
            // A fresh native presenter must retain its original bitmap after
            // the caller returns, and still promote transactionally later.
            let native_layer = msg_id(msg_id(class("CALayer"), sel("alloc")), sel("init"));
            let mut native = WindowPresenter::attach(native_layer, 1.0).unwrap();
            msg_arg(view, sel("setLayer:"), native_layer);
            native.prime(SIZE.width, SIZE.height, 1);
            let band_scene = |offset: f64| {
                let mut commands = vec![
                    DrawCommand::FillRect {
                        rect: Rect {
                            origin: Point { x: 0.0, y: 0.0 },
                            size: SIZE,
                        },
                        color: Color::WHITE,
                        corner_radius: Corners::ZERO,
                    },
                    DrawCommand::FillRect {
                        rect: Rect {
                            origin: Point { x: 0.0, y: 0.0 },
                            size: Size {
                                width: 8.0,
                                height: 8.0,
                            },
                        },
                        color: Color::BLACK,
                        corner_radius: Corners::ZERO,
                    },
                    DrawCommand::PushClip {
                        rect: Rect {
                            origin: Point { x: 16.0, y: 0.0 },
                            size: Size {
                                width: 288.0,
                                height: SIZE.height,
                            },
                        },
                        corner_radius: Corners::ZERO,
                    },
                ];
                for row in -1..3 {
                    commands.push(DrawCommand::FillRect {
                        rect: Rect {
                            origin: Point {
                                x: 16.0,
                                y: row as f64 * 120.0 - offset,
                            },
                            size: Size {
                                width: 288.0,
                                height: 120.0,
                            },
                        },
                        color: if row % 2 == 0 { RED } else { GREEN },
                        corner_radius: Corners::ZERO,
                    });
                }
                commands.push(DrawCommand::PopClip);
                DisplayList::from(commands)
            };
            for (offset, base, patch, label) in [
                (0.0, GREEN, RED, "native owned pixels"),
                (0.0, GREEN, RED, "native repeated pixels"),
                (120.0, RED, GREEN, "native bands after retiring full base"),
                (0.0, GREEN, RED, "native bands reverse after retirement"),
            ] {
                native.present(
                    &band_scene(offset),
                    SIZE,
                    1,
                    Color::BLACK,
                    &PixelFont,
                    &images,
                    false,
                );
                assert!(
                    !has_metal_child(native_layer),
                    "the image stays native"
                );
                expect(window, base, patch, label);
                assert!(
                    sample(window, 310, 80)
                        .iter()
                        .all(|channel| *channel >= 128),
                    "the outside background is still white"
                );
                assert!(
                    sample(window, 4, 4).iter().all(|channel| *channel < 128),
                    "the outside foreground survives base retirement"
                );
            }
            for height in [60.0, 60.01, 60.0, 20.0, 20.01, 20.0, 100.0, 100.0] {
                let mut commands = band_scene(0.0).as_slice().to_vec();
                commands.push(DrawCommand::FillRect {
                    rect: Rect {
                        origin: Point { x: 290.0, y: 10.0 },
                        size: Size { width: 4.0, height },
                    },
                    color: Color::BLACK,
                    corner_radius: Corners::ZERO,
                });
                native.present(
                    &DisplayList::from(commands),
                    SIZE,
                    1,
                    Color::BLACK,
                    &PixelFont,
                    &images,
                    false,
                );
                expect(window, GREEN, RED, "native decoration over unchanged rows");
                let pixel = sample(window, 292, 40);
                if height > 30.0 {
                    assert!(
                        pixel.iter().all(|channel| *channel < 64),
                        "the new thumb is visible: {pixel:?}"
                    );
                } else {
                    assert!(
                        pixel[0] > 128 && pixel[1] < 128,
                        "the shortened thumb reveals the exact row: {pixel:?}"
                    );
                }
                assert!(!has_metal_child(native_layer));
            }
            assert!(native.rest());
            expect(window, GREEN, RED, "native idle pixels");
            native.present(
                &scene(GREEN, Color::WHITE),
                SIZE,
                1,
                Color::BLACK,
                &PixelFont,
                &images,
                false,
            );
            expect(window, GREEN, Color::WHITE, "promotion after native image");
            drop(native);
            msg_void(native_layer, sel("release"));
            let sparse_layer = msg_id(msg_id(class("CALayer"), sel("alloc")), sel("init"));
            let mut sparse = WindowPresenter::attach(sparse_layer, 1.0).unwrap();
            msg_arg(view, sel("setLayer:"), sparse_layer);
            sparse.prime(SIZE.width, SIZE.height, 1);
            for label in ["sparse native image", "sparse repeated image"] {
                sparse.present(
                    &scene(GREEN, Color::WHITE),
                    SIZE,
                    1,
                    Color::BLACK,
                    &PixelFont,
                    &images,
                    false,
                );
                assert!(
                    !has_metal_child(sparse_layer),
                    "the sparse base stays native"
                );
                expect(window, GREEN, Color::WHITE, label);
            }
            assert!(sparse.rest());
            expect(window, GREEN, Color::WHITE, "sparse idle image");
            sparse.present(
                &scene(RED, Color::WHITE),
                SIZE,
                1,
                Color::BLACK,
                &PixelFont,
                &images,
                false,
            );
            expect(window, RED, Color::WHITE, "promotion after sparse image");
            drop(sparse);
            msg_void(sparse_layer, sel("release"));
            let growing_layer = msg_id(msg_id(class("CALayer"), sel("alloc")), sel("init"));
            let mut growing = WindowPresenter::attach(growing_layer, 1.0).unwrap();
            msg_arg(view, sel("setLayer:"), growing_layer);
            growing.prime(SIZE.width, SIZE.height, 1);
            for rows in [0, 1, 2, 4, 5, 2, 0] {
                let mut commands = vec![
                    DrawCommand::FillRect {
                        rect: Rect {
                            origin: Point { x: 0.0, y: 0.0 },
                            size: SIZE,
                        },
                        color: Color::WHITE,
                        corner_radius: Corners::ZERO,
                    },
                    DrawCommand::PushClip {
                        rect: Rect {
                            origin: Point { x: 0.0, y: 0.0 },
                            size: SIZE,
                        },
                        corner_radius: Corners::ZERO,
                    },
                ];
                for row in 0..rows {
                    commands.push(DrawCommand::FillRect {
                        rect: Rect {
                            origin: Point {
                                x: 0.0,
                                y: row as f64 * 48.0,
                            },
                            size: Size {
                                width: SIZE.width,
                                height: 48.0,
                            },
                        },
                        color: if row % 2 == 0 { RED } else { GREEN },
                        corner_radius: Corners::ZERO,
                    });
                    for (x, width, color) in [(80.0, 16.0, GREEN), (104.0, 4.0, Color::WHITE)] {
                        commands.push(DrawCommand::FillRect {
                            rect: Rect {
                                origin: Point {
                                    x,
                                    y: row as f64 * 48.0 + 8.0,
                                },
                                size: Size {
                                    width,
                                    height: 16.0,
                                },
                            },
                            color,
                            corner_radius: Corners::ZERO,
                        });
                    }
                }
                commands.push(DrawCommand::PopClip);
                growing.present(
                    &DisplayList::from(commands),
                    SIZE,
                    1,
                    Color::BLACK,
                    &PixelFont,
                    &images,
                    false,
                );
                assert!(
                    !has_metal_child(growing_layer),
                    "bounded growth remains native"
                );
                expect(
                    window,
                    if rows > 3 { GREEN } else { Color::WHITE },
                    if rows > 1 { GREEN } else { Color::WHITE },
                    &format!("native growing rows={rows}"),
                );
                if rows > 0 {
                    let first = sample(window, 200, 20);
                    assert!(
                        first[0] >= 128 && first[1] < 128 && first[2] < 128,
                        "the first row is present"
                    );
                    let foreground = sample(window, 86, 14);
                    assert!(
                        foreground[0] < 128 && foreground[1] >= 128 && foreground[2] < 128,
                        "cropped foreground is visible"
                    );
                    let gap = sample(window, 100, 12);
                    assert!(
                        gap.iter().zip(&first).all(|(a, b)| a.abs_diff(*b) <= 2),
                        "the native background and the opaque raster must have the same color: {gap:?}, {first:?}"
                    );
                }
            }
            drop(growing);
            msg_void(growing_layer, sel("release"));
            let patch_layer = msg_id(msg_id(class("CALayer"), sel("alloc")), sel("init"));
            let mut patched = WindowPresenter::attach(patch_layer, 1.0).unwrap();
            msg_arg(view, sel("setLayer:"), patch_layer);
            patched.prime(SIZE.width, SIZE.height, 1);
            for (x, width, expected, label) in [
                (32.0, 64.0, RED, "native rounded base"),
                (32.0, 16.0, GREEN, "native patch shrinks ink"),
                (96.0, 32.0, GREEN, "native patch moves ink"),
                (32.0, 64.0, RED, "native patch returns to base"),
            ] {
                let display = DisplayList::from(vec![
                    DrawCommand::FillRect {
                        rect: Rect {
                            origin: Point::ZERO,
                            size: SIZE,
                        },
                        color: GREEN,
                        corner_radius: Corners::all(12.0),
                    },
                    DrawCommand::FillRect {
                        rect: Rect {
                            origin: Point { x, y: 32.0 },
                            size: Size {
                                width,
                                height: 32.0,
                            },
                        },
                        color: RED,
                        corner_radius: Corners::ZERO,
                    },
                ]);
                patched.present(&display, SIZE, 1, Color::BLACK, &PixelFont, &images, false);
                assert!(
                    !has_metal_child(patch_layer),
                    "small patches keep the native base"
                );
                expect(window, GREEN, expected, label);
                let moved = sample(window, 110, 48);
                assert_eq!(
                    moved[0] >= 128,
                    x == 96.0,
                    "the moved patch is visible without a trail"
                );
            }
            assert!(patched.rest());
            expect(window, GREEN, RED, "native patched idle");
            patched.present(
                &scene(RED, GREEN),
                SIZE,
                1,
                Color::BLACK,
                &PixelFont,
                &images,
                false,
            );
            assert!(
                has_metal_child(patch_layer),
                "a broad rewrite promotes"
            );
            expect(window, RED, GREEN, "native patched promotion");
            drop(patched);
            msg_void(patch_layer, sel("release"));
            msg_arg(window, sel("orderOut:"), null_mut());
            msg_bool(window, sel("setReleasedWhenClosed:"), 0);
            msg_void(window, sel("close"));
            msg_void(view, sel("release"));
            msg_void(layer, sel("release"));
            msg_void(window, sel("release"));
            objc_autoreleasePoolPop(pool);
        }
    }
}

#[cfg(target_os = "macos")]
fn main() {
    macos::run();
}
#[cfg(not(target_os = "macos"))]
fn main() {
    panic!("window_presentation requires a macOS desktop");
}
