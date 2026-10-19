//! Appearance is observable even when the application owns its palette.
use bunny_ui::layout::{Color, Size};
use bunny_ui::prelude::*;
use bunny_ui::theme::{self, Appearance, Theme};
use bunny_ui_core as bunny_ui;
use std::{cell::Cell, rc::Rc};

#[test]
fn system_observations_and_explicit_palette_ownership_are_independent() {
    theme::observe_system_appearance(None);
    theme::follow_system();
    assert_eq!(theme::system_appearance(), None);
    assert_eq!(theme::current(), Theme::light());
    let before = theme::version();
    theme::observe_system_appearance(Some(Appearance::Dark));
    assert_eq!(theme::current(), Theme::dark());
    assert_eq!(theme::version(), before + 1);
    theme::observe_system_appearance(Some(Appearance::Dark));
    assert_eq!(
        theme::version(),
        before + 1,
        "duplicate native reports are inert"
    );
    let custom = Theme {
        accent: Color::hex(0xAA3311),
        ..Theme::light()
    };
    theme::install(custom);
    let before = theme::version();
    theme::observe_system_appearance(Some(Appearance::Light));
    assert_eq!(theme::system_appearance(), Some(Appearance::Light));
    assert_eq!(theme::current(), custom);
    assert_eq!(theme::version(), before + 1);
    theme::observe_system_appearance(Some(Appearance::Dark));
    assert_eq!(theme::current(), custom);
    theme::follow_system();
    assert_eq!(theme::current(), Theme::dark());
    theme::observe_system_appearance(None);
    assert_eq!(theme::system_appearance(), None);
    assert_eq!(
        theme::current(),
        Theme::light(),
        "unknown uses the documented light fallback"
    );
}

#[derive(Clone)]
struct Reader(Rc<Cell<usize>>, Rc<Cell<Option<Appearance>>>);
impl Component for Reader {
    fn body(self) -> impl View {
        self.0.set(self.0.get() + 1);
        let appearance = theme::system_appearance();
        self.1.set(appearance);
        text(format!("{appearance:?}"))
    }
}

#[test]
fn custom_palette_observers_rebuild_every_retained_scene_and_survive_first_scene_drop() {
    theme::observe_system_appearance(Some(Appearance::Light));
    theme::install(Theme::light());
    let mut readers: Vec<_> = (0..3)
        .map(|i| {
            (
                Runtime::scene(format!("appearance-{i}")),
                Reader(Rc::new(Cell::new(0)), Rc::new(Cell::new(None))),
            )
        })
        .collect();
    let size = Size {
        width: 240.0,
        height: 180.0,
    };
    for (runtime, root) in &readers {
        runtime.display_frame(root, size);
        assert_eq!(root.1.get(), Some(Appearance::Light));
        let before = root.0.get();
        runtime.display_frame(root, size);
        assert_eq!(root.0.get(), before, "unchanged scene retains its body");
    }
    theme::observe_system_appearance(Some(Appearance::Dark));
    for (runtime, root) in &readers {
        assert!(runtime.needs_frame());
        let before = root.0.get();
        runtime.display_frame(root, size);
        assert!(root.0.get() > before);
        assert_eq!(root.1.get(), Some(Appearance::Dark));
    }
    assert_eq!(
        theme::current(),
        Theme::light(),
        "custom palette is not a system observation"
    );
    drop(readers.remove(0));
    theme::observe_system_appearance(Some(Appearance::Light));
    for (runtime, root) in &readers {
        runtime.display_frame(root, size);
        assert_eq!(root.1.get(), Some(Appearance::Light));
    }
    let before = theme::version();
    theme::observe_system_appearance(Some(Appearance::Light));
    assert_eq!(theme::version(), before);
}

#[derive(Clone)]
struct PaletteReader(Rc<Cell<Theme>>);
impl Component for PaletteReader {
    fn body(self) -> impl View {
        self.0.set(theme::current());
        text("palette").foreground_color(theme::fg())
    }
}

#[test]
fn system_palette_survives_scene_theme_restoration() {
    theme::observe_system_appearance(Some(Appearance::Light));
    theme::follow_system();
    let ordinary = Runtime::scene("system-palette");
    let quiet = Runtime::scene("quiet-palette").theme(Theme::light());
    let seen = Rc::new(Cell::new(Theme::light()));
    let root = PaletteReader(Rc::clone(&seen));
    let size = Size {
        width: 100.0,
        height: 80.0,
    };
    for appearance in [Appearance::Dark, Appearance::Light, Appearance::Dark] {
        theme::observe_system_appearance(Some(appearance));
        quiet.display_frame(&root, size);
        ordinary.display_frame(&root, size);
        let expected = if appearance == Appearance::Dark {
            Theme::dark()
        } else {
            Theme::light()
        };
        assert_eq!(
            seen.get(),
            expected,
            "an ordinary scene follows the observed palette"
        );
        assert_eq!(theme::installed(), expected);
    }
}
