//! How the three activation messages are read, with no window behind them.
use super::{Turn, turn};
use crate::ffi::{WM_ACTIVATE, WM_ACTIVATEAPP, WM_NCACTIVATE};

const MINIMIZED: usize = 1 << 16;

#[test]
fn activation_reads_the_low_word_and_ignores_the_minimized_flag() {
    assert_eq!(turn(WM_ACTIVATE, 0), Some(Turn::Left), "WA_INACTIVE");
    assert_eq!(
        turn(WM_ACTIVATE, MINIMIZED),
        Some(Turn::Left),
        "minimized away"
    );
    assert_eq!(turn(WM_ACTIVATE, 1), Some(Turn::Arrived), "WA_ACTIVE");
    assert_eq!(turn(WM_ACTIVATE, 2), Some(Turn::Arrived), "WA_CLICKACTIVE");
    assert_eq!(
        turn(WM_ACTIVATE, 1 | MINIMIZED),
        Some(Turn::Arrived),
        "restored from the taskbar"
    );
}

#[test]
fn the_app_leaving_and_coming_back_is_heard_without_a_window_of_ours_active() {
    // a message box holds the activation: only WM_ACTIVATEAPP reaches this procedure
    assert_eq!(turn(WM_ACTIVATEAPP, 0), Some(Turn::Left));
    assert_eq!(turn(WM_ACTIVATEAPP, 1), Some(Turn::Arrived));
}

#[test]
fn the_caption_speaks_only_when_it_goes_active() {
    assert_eq!(turn(WM_NCACTIVATE, 1), Some(Turn::Arrived));
    // inactive under the scene's own dialog is not leaving the scene
    assert_eq!(turn(WM_NCACTIVATE, 0), None);
}

#[test]
fn no_other_message_speaks_of_activation() {
    const WM_SETFOCUS: u32 = 0x0007;
    assert_eq!(turn(WM_SETFOCUS, 1), None);
}
