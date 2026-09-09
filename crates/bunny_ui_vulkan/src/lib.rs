//! The Vulkan tier every Vulkan shell shares.
//!
//! Linux and Android present through the same swapchain, the same
//! pipelines and the same shelf atlas; only the window differs — an
//! xcb window or a `wl_surface` on the desktop, an `ANativeWindow` on
//! the phone. This crate holds the tier, and a shell hands it the
//! window as a [`SurfaceSource`]. What the shell keeps is its own:
//! the ladder (which tier comes first, what catches a lost device),
//! the env switch that skips the tier, and the hooks a present rides
//! in (a Wayland frame callback, an ack).
//!
//! Like the shells, this crate is an `unsafe` border: the loader
//! comes in through `dlopen` and every entry point resolves through
//! `vkGetInstanceProcAddr`, with not a single dependency. The core
//! keeps `#![forbid(unsafe_code)]`.
//!
//! The crate compiles on every unix, not only where a loader lives:
//! a Mac with no `libvulkan` skips the device tests honestly and still
//! runs the two that need no device — the SPIR-V drift gate and the
//! push-range layout.

#![cfg(unix)]

mod vk;

pub use vk::{OffscreenVk, Presented, SurfaceSource, VkPresenter};
