//! The shell's tape: a clock the engine's stage timers share, and the
//! lines a run prints when asked (`BUNNY_FRAME_STATS=1`) — an `F` line
//! per frame, where the time went before the present opened, and a
//! `P` line per present, naming the tier. Both carry the same clock,
//! so a frame and its present line up. The drive sheets and the
//! container's matrix read the `P` lines.

use std::sync::OnceLock;
use std::time::Instant;

/// True when the run asked for the tape — the gate a caller checks
/// before paying for anything a mark would need.
pub fn active() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("BUNNY_FRAME_STATS").is_some() || std::env::var_os("BUNNY_TRACE").is_some()
    })
}

fn ms() -> f64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// The tape's own clock, in milliseconds — the one the shell installs
/// for the engine's stage timers, so an `F` line and a `P` line share
/// a time base.
pub fn clock_ms() -> f64 {
    ms()
}

/// One line on the tape: `<tag> t=<ms> <args>`, on stderr.
pub fn mark(tag: &str, args: std::fmt::Arguments) {
    eprintln!("{tag} t={:.2} {args}", ms());
}

/// How many frames the open door has presented — every tier counts.
pub(crate) fn presents() -> u64 {
    if crate::ffi::is_x11() {
        crate::x11::presents()
    } else {
        crate::ffi::presents()
    }
}
