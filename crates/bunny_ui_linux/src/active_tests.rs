//! The keyboard's resting place reaches the app through the scene registry.
//! No display: the registry and the app's doors are plain Rust.
use super::settle;
use crate::ffi::{forget_geometry, register_geometry};
use bunny_ui::app::{self, AppEvent};
use bunny_ui::window::{Lifecycle, WindowId};
use std::rc::Rc;

fn heard(life: &bunny_ui::task::Receiver<AppEvent>) -> Vec<(WindowId, bool)> {
    std::iter::from_fn(|| life.try_recv())
        .filter_map(|event| match event {
            AppEvent::WindowActive { window, active } => Some((window, active)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_keyboard_at_rest_is_said_once_and_a_closed_window_is_left() {
    const SURFACE: usize = 0x51;
    let lifecycle = Lifecycle::default();
    let id = lifecycle.id();
    register_geometry(SURFACE, Rc::clone(lifecycle.geometry_events()));
    let (sender, life) = bunny_ui::task::channel::<AppEvent>();
    app::subscribe(sender);

    // the same answer turn after turn is one event
    settle(SURFACE);
    settle(SURFACE);
    assert_eq!(heard(&life), [(id, true)]);
    assert!(app::window_active(id));

    // the keyboard left for another client
    settle(0);
    assert_eq!(heard(&life), [(id, false)]);

    // a surface that is none of the app's windows names nobody
    settle(0x99);
    assert_eq!(heard(&life), []);

    // closed with the keyboard on it: the address no longer names the scene
    settle(SURFACE);
    forget_geometry(SURFACE, id);
    settle(SURFACE);
    assert_eq!(heard(&life), [(id, true), (id, false)]);
    assert!(!app::window_active(id));
}
