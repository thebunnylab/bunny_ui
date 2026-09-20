//! Paranoid mode: a fast path runs the full path too, and compares.
//!
//! The engine skips work that would change nothing: an assembly whose
//! inputs did not move, a pass over a clean tree. Each of those shortcuts
//! is a claim — "the full path would have produced this". With
//! `BUNNY_PARANOID` set, the shortcut keeps its answer, runs the full
//! path, and panics when the two answers differ.
//!
//! ```sh
//! BUNNY_PARANOID=all cargo test -p bunny-ui
//! BUNNY_PARANOID=assemble,settle cargo test -p bunny-ui
//! ```
//!
//! Off, the cost is one branch for each shortcut taken. It is a test
//! instrument: an app never sets it.

use std::cell::Cell;
use std::sync::OnceLock;

/// The assembly that is skipped when the retention did not move.
pub(crate) const ASSEMBLE: u32 = 1 << 0;
/// The settle round that runs no pass over a clean tree.
pub(crate) const SETTLE: u32 = 1 << 1;

const NAMES: [(&str, u32); 2] = [("assemble", ASSEMBLE), ("settle", SETTLE)];

thread_local! {
    /// A test's own switch, over the environment's.
    static FORCED: Cell<u32> = const { Cell::new(0) };
}

fn from_environment() -> u32 {
    static FLAGS: OnceLock<u32> = OnceLock::new();
    *FLAGS.get_or_init(|| {
        let Ok(value) = std::env::var("BUNNY_PARANOID") else {
            return 0;
        };
        value
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| match name {
                "all" => u32::MAX,
                name => NAMES
                    .iter()
                    .find(|(known, _)| *known == name)
                    .map(|(_, flag)| *flag)
                    .unwrap_or_else(|| {
                        // a name nobody knows would check nothing, in silence
                        panic!("BUNNY_PARANOID: `{name}` is not a check (known: all, assemble, settle)")
                    }),
            })
            .fold(0, |flags, flag| flags | flag)
    })
}

/// Is this cross-check on?
#[inline]
pub(crate) fn on(flag: u32) -> bool {
    (from_environment() | FORCED.with(Cell::get)) & flag != 0
}

/// Turns checks on for the calling thread, until [`release`]. Tests use
/// it; the environment variable serves a whole run.
#[cfg(test)]
pub(crate) fn force(flags: u32) {
    FORCED.with(|forced| forced.set(forced.get() | flags));
}

#[cfg(test)]
pub(crate) fn release() {
    FORCED.with(|forced| forced.set(0));
}
