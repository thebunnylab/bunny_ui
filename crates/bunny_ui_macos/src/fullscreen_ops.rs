//! Native full screen: the trip AppKit animates into a Space of its own,
//! and the mode each window asked for.
//!
//! `toggleFullScreen:` is the only door, and it is a toggle: a second ask
//! while the window is still moving would be lost, so an ask that arrives
//! mid-trip waits and is answered when the trip lands. Presentation is the
//! same trip with the toolbar hidden alongside the menu bar, and Escape as
//! the way back.

use std::cell::RefCell;
use std::collections::HashMap;

use super::{
    AppEvent, Id, Sel, WINDOWS, defer_events, dispatch_to, msg_id, msg_u64, msg_void_u64, sel,
};
use bunny_ui::window::{FullscreenEvent, FullscreenMode, WindowError};

#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn msg_void_id(obj: Id, sel: Sel, a: Id);
    #[link_name = "objc_msgSend"]
    fn msg_after(obj: Id, sel: Sel, selector: Sel, object: Id, delay: f64);
}

const STYLE_FULLSCREEN: u64 = 1 << 14;
const BEHAVIOR_PRIMARY: u64 = 1 << 7;
const BEHAVIOR_AUXILIARY: u64 = 1 << 8;
const AUTO_HIDE_TOOLBAR: u64 = 1 << 11;
const ESCAPE: u16 = 53;

#[derive(Clone, Copy, Default)]
struct Trip {
    presenting: bool,
    moving: bool,
    next: Option<FullscreenMode>,
}

thread_local! {
    static TRIPS: RefCell<HashMap<usize, Trip>> = RefCell::new(HashMap::new());
}

fn trip(window: usize) -> Trip {
    TRIPS.with(|trips| trips.borrow().get(&window).copied().unwrap_or_default())
}

fn update(window: usize, change: impl FnOnce(&mut Trip)) {
    TRIPS.with(|trips| change(trips.borrow_mut().entry(window).or_default()));
}

fn live(window: usize) -> Result<Id, WindowError> {
    WINDOWS.with(|windows| {
        windows
            .borrow()
            .iter()
            .any(|(id, _, _)| *id == window)
            .then_some(window as Id)
            .ok_or(WindowError::StaleWindow)
    })
}

fn is_full(window: Id) -> bool {
    unsafe { msg_u64(window, sel("styleMask")) & STYLE_FULLSCREEN != 0 }
}

const fn mode_of(full: bool, presenting: bool) -> FullscreenMode {
    match (full, presenting) {
        (false, _) => FullscreenMode::Windowed,
        (true, false) => FullscreenMode::Fullscreen,
        (true, true) => FullscreenMode::Presentation,
    }
}

/// The toolbar's manners for a window already on its own screen.
const fn presentation_options(current: u64, presenting: bool) -> u64 {
    if presenting {
        current | AUTO_HIDE_TOOLBAR
    } else {
        current & !AUTO_HIDE_TOOLBAR
    }
}

/// How a live window stands on its screen now.
pub fn window_fullscreen(window: usize) -> Result<FullscreenMode, WindowError> {
    let id = live(window)?;
    Ok(mode_of(is_full(id), trip(window).presenting))
}

/// Asks for a mode. The answer arrives as the trip's events; a window
/// already in the asked mode hears nothing.
pub fn set_window_fullscreen(window: usize, mode: FullscreenMode) -> Result<(), WindowError> {
    let id = live(window)?;
    if trip(window).moving {
        update(window, |trip| trip.next = Some(mode));
        return Ok(());
    }
    let full = is_full(id);
    update(window, |trip| {
        trip.presenting = mode == FullscreenMode::Presentation
    });
    // Between full screen and presentation there is no trip: the system
    // fixes a Space's options when the window enters it, and an app may
    // not rewrite the full-screen bit. What changes is Escape, now.
    if full == mode.is_fullscreen() {
        return Ok(());
    }
    defer_events(|| unsafe {
        if mode.is_fullscreen() {
            // a fixed window gets no full-screen button of its own, and a
            // dialog's auxiliary manners would only join another's Space
            let behavior = msg_u64(id, sel("collectionBehavior"));
            msg_void_u64(
                id,
                sel("setCollectionBehavior:"),
                (behavior | BEHAVIOR_PRIMARY) & !BEHAVIOR_AUXILIARY,
            );
        }
        msg_void_id(id, sel("toggleFullScreen:"), std::ptr::null_mut());
    });
    Ok(())
}

/// Escape in a presentation takes the window back, before the keymap
/// hears it — the way out a player promises.
pub fn escape_presentation(view: Id, code: u16) -> bool {
    if code != ESCAPE {
        return false;
    }
    let window = unsafe { msg_id(view, sel("window")) };
    let state = trip(window as usize);
    if window.is_null() || !state.presenting || state.moving || !is_full(window) {
        return false;
    }
    set_window_fullscreen(window as usize, FullscreenMode::Windowed).is_ok()
}

/// Lets a closed window's state go before its address can be reused.
pub fn forget_fullscreen(window: usize) {
    TRIPS.with(|trips| trips.borrow_mut().remove(&window));
}

fn moved(note: Id, moving: bool, event: FullscreenEvent) {
    let window = unsafe { msg_id(note, sel("object")) };
    let landed = !moving;
    update(window as usize, |trip| {
        trip.moving = moving;
        if event == FullscreenEvent::Exited && trip.next.is_none() {
            trip.presenting = false;
        }
    });
    dispatch_to(window, AppEvent::Fullscreen(event));
    if landed
        && let Some(next) = TRIPS.with(|trips| {
            trips
                .borrow_mut()
                .get_mut(&(window as usize))
                .and_then(|trip| trip.next.take())
        })
    {
        let want_full = next.is_fullscreen();
        update(window as usize, |trip| {
            trip.presenting = next == FullscreenMode::Presentation
        });
        if want_full != is_full(window) {
            // the trip that just landed still holds AppKit's hand: the
            // next one leaves on the run loop's following turn
            unsafe {
                msg_after(
                    window,
                    sel("performSelector:withObject:afterDelay:"),
                    sel("toggleFullScreen:"),
                    std::ptr::null_mut(),
                    0.0,
                );
            }
        }
    }
}

pub(super) extern "C" fn will_enter(_this: Id, _sel: Sel, note: Id) {
    moved(note, true, FullscreenEvent::WillEnter);
}

pub(super) extern "C" fn did_enter(_this: Id, _sel: Sel, note: Id) {
    moved(note, false, FullscreenEvent::Entered);
}

pub(super) extern "C" fn will_exit(_this: Id, _sel: Sel, note: Id) {
    moved(note, true, FullscreenEvent::WillExit);
}

pub(super) extern "C" fn did_exit(_this: Id, _sel: Sel, note: Id) {
    moved(note, false, FullscreenEvent::Exited);
}

pub(super) extern "C" fn did_fail_enter(_this: Id, _sel: Sel, window: Id) {
    landed_without_trip(window, FullscreenEvent::EnterFailed);
}

pub(super) extern "C" fn did_fail_exit(_this: Id, _sel: Sel, window: Id) {
    landed_without_trip(window, FullscreenEvent::ExitFailed);
}

/// The failure callbacks name the window itself, not a notification.
fn landed_without_trip(window: Id, event: FullscreenEvent) {
    update(window as usize, |trip| {
        trip.moving = false;
        trip.next = None;
        if event == FullscreenEvent::EnterFailed {
            trip.presenting = false;
        }
    });
    dispatch_to(window, AppEvent::Fullscreen(event));
}

pub(super) extern "C" fn presentation_for(_this: Id, _sel: Sel, window: Id, proposed: u64) -> u64 {
    presentation_options(proposed, trip(window as usize).presenting)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mode_is_the_style_mask_and_the_ask() {
        assert_eq!(mode_of(false, false), FullscreenMode::Windowed);
        assert_eq!(
            mode_of(false, true),
            FullscreenMode::Windowed,
            "an ask still travelling is no mode yet"
        );
        assert_eq!(mode_of(true, false), FullscreenMode::Fullscreen);
        assert_eq!(mode_of(true, true), FullscreenMode::Presentation);
    }

    #[test]
    fn a_presentation_hides_the_toolbar_and_keeps_the_rest() {
        let proposed = (1 << 10) | 1 | 4;
        assert_eq!(
            presentation_options(proposed, true),
            proposed | AUTO_HIDE_TOOLBAR
        );
        assert_eq!(
            presentation_options(proposed | AUTO_HIDE_TOOLBAR, false),
            proposed
        );
    }

    #[test]
    fn a_window_the_app_never_opened_is_stale() {
        assert_eq!(window_fullscreen(0xdead), Err(WindowError::StaleWindow));
        assert_eq!(
            set_window_fullscreen(0xdead, FullscreenMode::Fullscreen),
            Err(WindowError::StaleWindow)
        );
    }
}
