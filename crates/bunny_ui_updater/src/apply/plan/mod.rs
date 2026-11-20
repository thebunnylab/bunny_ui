//! The half of each install that can be wrong, compiled and tested on
//! every system.
//!
//! The arguments of each tool, which formats each system takes, the
//! helper's handshake and its wait for the images are pure functions
//! here. Only the process spawns and the exclusive open are behind a
//! `cfg`, and neither can be wrong in an interesting way.

pub mod linux;
pub mod macos;
pub mod windows;

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
