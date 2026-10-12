//! The preview vocabulary against a living runtime: a scene renders under
//! a theme of its own, quietly — two scenes alternate light and dark on one
//! thread with no extra body passes — and the traits reach the scene through
//! the runtime's own doors.
use bunny_ui_core as bunny_ui;

use bunny_ui::layout::{Color, DisplayList, DrawCommand, Size};
use bunny_ui::prelude::*;
use bunny_ui::preview::{Device, Preview};
use bunny_ui::stats;

const SIZE: Size = Size { width: 200.0, height: 80.0 };

/// A page whose root paints the theme's floor from its BODY — a token read
/// inside the pass and baked into the retention — and whose text takes its
/// ink at PLACEMENT, where no colour was set and the chrome reads the
/// thread's theme. A component, so the retention has a boundary to keep:
/// a settled frame runs no body at all.
#[derive(Clone)]
struct Page;

impl Component for Page {
    fn body(self) -> impl View {
        vstack!(text!("hello")).background_color(theme::canvas())
    }
}

fn page() -> Page {
    Page
}

fn fills(display: &DisplayList) -> Vec<Color> {
    display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::FillRect { color, .. } => Some(*color),
            _ => None,
        })
        .collect()
}

fn inks(display: &DisplayList) -> Vec<Color> {
    display
        .iter()
        .filter_map(|command| match command {
            DrawCommand::TextLine { color, .. } => Some(*color),
            _ => None,
        })
        .collect()
}

/// One frame, and the body passes it cost.
fn frame(runtime: &Runtime, view: &impl View) -> (DisplayList, u32) {
    let _ = stats::take();
    let display = runtime.display_frame(view, SIZE);
    (display, stats::take().body_passes)
}

#[test]
fn two_scenes_alternate_light_and_dark_without_extra_body_passes() {
    theme::install(Theme::light());
    let light = Runtime::scene("light").theme(Theme::light());
    let dark = Runtime::scene("dark").theme(Theme::dark());
    let view = page();
    for round in 0..10u32 {
        let (display, passes) = frame(&light, &view);
        assert_eq!(passes == 0, round > 0, "light scene, round {round}: {passes} body passes");
        assert!(fills(&display).contains(&Theme::light().canvas), "the light floor, baked in body");
        assert!(!inks(&display).is_empty());
        assert!(inks(&display).iter().all(|ink| *ink == Theme::light().fg), "the light ink, read at placement");

        let (display, passes) = frame(&dark, &view);
        assert_eq!(passes == 0, round > 0, "dark scene, round {round}: {passes} body passes");
        assert!(fills(&display).contains(&Theme::dark().canvas), "the dark floor, baked in body");
        assert!(inks(&display).iter().all(|ink| *ink == Theme::dark().fg), "the dark ink, read at placement");
    }
    // a runtime without a theme of its own renders under the installed one,
    // whatever the dark scene left on the thread
    let plain = Runtime::scene("plain");
    let (display, _) = frame(&plain, &view);
    assert!(fills(&display).contains(&Theme::light().canvas));
    assert_eq!(theme::current(), Theme::light());
    assert_eq!(plain.scene_theme(), None);
}

#[test]
fn set_theme_reruns_the_bodies_once_and_the_next_frame_is_quiet() {
    theme::install(Theme::light());
    let scene = Runtime::scene("moving").theme(Theme::light());
    let view = page();
    let _ = frame(&scene, &view);
    let (_, passes) = frame(&scene, &view);
    assert_eq!(passes, 0, "settled");
    assert!(scene.set_theme(Some(Theme::dark())));
    assert!(!scene.set_theme(Some(Theme::dark())), "the same theme moves nothing");
    let (display, passes) = frame(&scene, &view);
    assert!(passes >= 1, "the bodies baked the old tokens: they run again, once");
    assert!(fills(&display).contains(&Theme::dark().canvas));
    assert!(inks(&display).iter().all(|ink| *ink == Theme::dark().fg));
    let (_, passes) = frame(&scene, &view);
    assert_eq!(passes, 0, "and the frame after is quiet again");
    assert_eq!(scene.scene_theme(), Some(Theme::dark()));
    assert!(scene.set_theme(None), "back to the installed theme");
    let (display, _) = frame(&scene, &view);
    assert!(fills(&display).contains(&Theme::light().canvas));
}

#[test]
fn traits_reach_the_scene_through_the_runtimes_doors() {
    let scene = Runtime::scene("traits");
    let preview = Preview::new(text("x"))
        .dark()
        .device(Device::IPhone16)
        .locale("ar")
        .rtl()
        .compact();
    let (view, traits) = preview.into_parts();
    traits.apply(&scene);
    assert_eq!(scene.scene_theme(), Some(Theme::dark()));
    assert_eq!(scene.locale(), Locale::new("ar"));
    assert_eq!(scene.layout_direction(), LayoutDirection::RightToLeft);
    assert_eq!(scene.size_class(), SizeClass::Compact);
    assert_eq!(scene.device_scale(), Device::IPhone16.scale());
    assert_eq!(scene.safe_area(), Device::IPhone16.safe_area());
    assert_eq!(traits.proposal(), Proposal::exact(Device::IPhone16.frame()));
    // applying the same traits again moves nothing
    traits.apply(&scene);
    assert!(!scene.set_theme(Some(Theme::dark())));
    // and the view lays out in the device's frame under them
    let display = scene.display_frame(&view, Device::IPhone16.frame());
    assert!(inks(&display).iter().all(|ink| *ink == Theme::dark().fg));
}

#[test]
fn both_schemes_render_as_two_scenes_with_their_own_tokens() {
    theme::install(Theme::light());
    let traits = Preview::new(page()).schemes().traits;
    let scenes: Vec<Runtime> = traits
        .variants()
        .iter()
        .enumerate()
        .map(|(index, variant)| {
            let scene = Runtime::scene(format!("variant{index}"));
            variant.apply(&scene);
            scene
        })
        .collect();
    assert_eq!(scenes.len(), 2);
    let view = page();
    let light = fills(&scenes[0].display_frame(&view, SIZE));
    let dark = fills(&scenes[1].display_frame(&view, SIZE));
    assert!(light.contains(&Theme::light().canvas));
    assert!(dark.contains(&Theme::dark().canvas));
    assert_eq!(theme::installed(), Theme::light(), "the quiet themes never touched the installed one");
}
