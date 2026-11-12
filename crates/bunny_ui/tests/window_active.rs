//! The active window reaches a subscriber once per change, and the query
//! answers what the last event said.
//!
//! One test, because the active window is one fact for the whole process.
use bunny_ui_core::app::{self, AppEvent};
use bunny_ui_core::task::{self, Receiver};
use bunny_ui_core::window::{Lifecycle, WindowId};

/// What the subscriber heard about its windows since it was last asked.
fn heard(life: &Receiver<AppEvent>) -> Vec<(WindowId, bool)> {
    std::iter::from_fn(|| life.try_recv())
        .filter_map(|event| match event {
            AppEvent::WindowActive { window, active } => Some((window, active)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_active_window_is_said_once_per_change_and_the_query_agrees() {
    let workbench = Lifecycle::default().id();
    let composer = Lifecycle::default().id();
    assert!(
        !app::window_active(workbench),
        "a window nobody named is not active"
    );

    // a subscriber that arrives AFTER the window is already active hears
    // nothing of the past, and the query is how it learns where it stands
    app::observe_active_window(Some(workbench));
    let (sender, life) = task::channel::<AppEvent>();
    app::subscribe(sender);
    assert_eq!(heard(&life), []);
    assert!(app::window_active(workbench));
    assert!(!app::window_active(composer));

    // the platform repeating itself is no event
    app::observe_active_window(Some(workbench));
    assert_eq!(heard(&life), []);

    // the place passes to another window of the app: the one that lost it
    // first, then the one that has it
    app::observe_active_window(Some(composer));
    assert_eq!(heard(&life), [(workbench, false), (composer, true)]);
    assert!(!app::window_active(workbench));
    assert!(app::window_active(composer));

    // the person leaves the app: one event, and no window is active
    app::observe_active_window(None);
    app::observe_active_window(None);
    assert_eq!(heard(&life), [(composer, false)]);
    assert!(!app::window_active(workbench));
    assert!(!app::window_active(composer));

    // and comes back: every answer of the query is the last event's word
    app::observe_active_window(Some(workbench));
    app::observe_active_window(Some(workbench));
    assert_eq!(heard(&life), [(workbench, true)]);
    assert!(app::window_active(workbench));

    // a second subscriber hears the same stream from here on
    let (sender, late) = task::channel::<AppEvent>();
    app::subscribe(sender);
    app::observe_active_window(None);
    assert_eq!(heard(&life), [(workbench, false)]);
    assert_eq!(heard(&late), [(workbench, false)]);
}
