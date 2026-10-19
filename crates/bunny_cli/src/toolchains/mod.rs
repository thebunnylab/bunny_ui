//! What this machine has for building each platform: found the way the
//! platform's own tools find it, so `doctor`, `run` and `build` agree on
//! what is there and say the same thing when it is not.

pub mod android;
pub mod android_packages;
pub mod apple;
pub mod linux;
pub mod rust;
pub mod windows;

use std::time::Duration;

/// How long a quick question to a tool may take (`--version`, a list):
/// long enough for a cold `xcrun`, short enough that `doctor` never
/// hangs on a tool that does.
pub const QUICK: Duration = Duration::from_secs(20);
