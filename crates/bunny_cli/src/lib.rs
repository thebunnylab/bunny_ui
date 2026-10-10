//! The `bunny` command: create a bunny-ui app, check the machine it is
//! built on, and run it on macOS, iOS, Windows, Linux, Android and the
//! web.
//!
//! The binary is `bunny`; this library is its body, public so the
//! integration tests reach the same functions the commands call. Like
//! the framework, it stands on the standard library alone.

#![forbid(unsafe_code)]

pub mod args;
pub mod cli;
pub mod commands;
pub mod error;
pub mod ids;
pub mod template;
pub mod templates;
pub mod term;

pub use cli::main;
