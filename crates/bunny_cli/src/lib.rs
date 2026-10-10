//! The `bunny` command: create a bunny-ui app, check the machine it is
//! built on, and run it on macOS, iOS, Windows, Linux, Android and the
//! web.
//!
//! The binary is `bunny`; this library is its body, public so the
//! integration tests reach the same functions the commands call. Like
//! the framework, it stands on the standard library alone.

#![forbid(unsafe_code)]

pub mod args;
pub mod cargo;
pub mod cli;
pub mod commands;
pub mod devices;
pub mod error;
pub mod formats;
pub mod hot;
pub mod ids;
pub mod json;
pub mod keys;
pub mod net;
pub mod platform;
pub mod process;
pub mod project;
pub mod serve;
pub mod template;
pub mod templates;
pub mod term;
pub mod toolchains;
pub mod wasm;

pub use cli::main;
