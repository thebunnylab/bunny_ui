//! The half of a bunny-ui shell that every Apple platform shares.
//!
//! macOS and iOS draw with the same Metal, shape text with the same
//! CoreText, decode pictures with the same ImageIO and keep secrets in
//! the same Security framework. Only the window and the hand differ:
//! AppKit's events and `NSWindow` on the Mac, UIKit's touches and
//! `UIWindow` on the phone. This crate holds the shared half, so a fix
//! lands once and both shells compile it. The shells hold the rest.
//!
//! Like the shells, this crate is an `unsafe` border: the frameworks are
//! called through hand-written FFI ([`ffi`]) with not a single
//! dependency. The core and the facade keep `#![forbid(unsafe_code)]`.

#![cfg(target_vendor = "apple")]

pub mod credentials;
pub mod ffi;
pub mod image;
pub mod metal;
pub mod text;
pub mod trace;

pub use image::CoreGraphicsImageEngine;
pub use metal::{MetalPresenter, OffscreenGpu};
pub use text::CoreTextEngine;
