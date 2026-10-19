//! The desk pictures a page: a `webview_html` box laid out by a runtime,
//! mounted on the offscreen window, red at its centre once the load
//! finished; a second scene with the same path gets a page of its own;
//! a sweep releases both. On the main thread, as AppKit wants.
#[cfg(target_os = "macos")]
fn main() {
    use std::time::{Duration, Instant};

    use bunny_ui::image_engine::ImageSource;
    use bunny_ui::layout::Size;
    use bunny_ui::prelude::*;
    use bunny_ui_macos::pictures::Desk;

    let runtime = Runtime::scene("p0");
    let root = vstack!(
        webview_html("<body style=\"margin:0;background:#ff0000\"></body>", "", NetworkPolicy::Deny)
            .frame(200.0, 100.0)
    );
    let _ = runtime.display_frame(&root, Size { width: 200.0, height: 100.0 });
    let hosts = runtime.hosts();
    assert_eq!(hosts.len(), 1, "the page is a hosted box");
    let placement = &hosts[0];

    let mut desk = Desk::open();
    assert!(desk.picture("p0", placement).is_none(), "no picture before the load");
    assert!(desk.busy(), "the page is loading");
    let began = Instant::now();
    let picture = loop {
        desk.turn();
        if let Some(picture) = desk.picture("p0", placement) {
            break picture;
        }
        assert!(began.elapsed() < Duration::from_secs(20), "no picture within 20 s");
        std::thread::sleep(Duration::from_millis(16));
    };
    println!("pictures: the page pictured in {:.0} ms", began.elapsed().as_secs_f64() * 1000.0);
    let ImageSource::Rgba { size, rgba, .. } = picture else {
        panic!("a page pictures as raw pixels");
    };
    let (width, height) = (size.0 as usize, size.1 as usize);
    assert_eq!(rgba.len(), width * height * 4, "tightly packed");
    assert!(
        (width as f64 / 200.0 - height as f64 / 100.0).abs() < 0.05,
        "the picture keeps the box's shape: {width}x{height}"
    );
    let at = ((height / 2) * width + width / 2) * 4;
    assert_eq!(&rgba[at..at + 4], &[255, 0, 0, 255], "red at the centre of {width}x{height}");
    let corner = &rgba[..4];
    assert_eq!(corner, &[255, 0, 0, 255], "red to the edge: the page fills its box");

    // the same path in another scene is another page
    assert!(desk.picture("p1", placement).is_none(), "the second scene's page loads on its own");
    assert_eq!(desk.pages(), 2);
    desk.sweep(|scene, _| scene == "p0");
    assert_eq!(desk.pages(), 1, "the swept scene's page left");
    assert!(desk.picture("p0", placement).is_some(), "the kept page keeps its picture");
    desk.sweep(|_, _| false);
    assert_eq!(desk.pages(), 0);
    assert!(!desk.busy(), "nothing left to wait for");
    println!("pictures: ok");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
