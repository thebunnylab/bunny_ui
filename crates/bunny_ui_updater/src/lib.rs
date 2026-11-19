//! The self-updater of bunny-ui apps.
//!
//! An app asks a feed whether a newer build is there. When the user wants
//! it, the updater downloads the artifact, checks its SHA-256 and its
//! Ed25519 signature against the keys the app was built with, and puts the
//! new build in place. The next start runs it.
//!
//! ## The parts
//!
//! - [`UpdaterConfig`]: the [`Feed`], the product, the channel, the version
//!   that runs, and the keys. The build fixes all of them. None comes from a
//!   setting that a user or an attacker can change.
//! - [`Updater::check`] asks the feed and gives an [`UpdateStatus`]. Nothing
//!   is downloaded.
//! - [`Updater::download`] gives a [`Verified`] artifact, or an error. Only
//!   bytes that verified ever become a `Verified`.
//! - [`UpdateLog`] keeps the reason of each failure on this machine.
//! - [`Fetch`] is the network. The `curl` feature (on by default) gives
//!   [`CurlFetch`]. An app with its own HTTP client implements the trait.
//!
//! ## Production gotchas
//!
//! **The calls block.** They run on a thread of their own, never on the UI
//! thread.
//!
//! **Desktop only.** An app from a store updates through the store, and a
//! web app updates when it is deployed again.
//!
//! **The feed JSON is not signed, only the artifact is.** The `url`, the
//! `format` and the floor come through TLS and the server.

#![forbid(unsafe_code)]

mod client;
mod config;
#[cfg(feature = "curl")]
mod curl;
mod fetch;
mod log;

#[cfg(test)]
mod client_tests;
#[cfg(all(test, feature = "curl"))]
mod curl_tests;
#[cfg(test)]
mod log_tests;

pub use bunny_ui_formats::ed25519::VerifyingKey;
pub use bunny_ui_release as release;
pub use bunny_ui_release::{InstallProgress, Installed, Offer, ReleaseAsset, UpdateError, UpdateStatus, Version};

pub use client::{Updater, Verified, verify_file};
pub use config::{Feed, UpdaterConfig, default_cache, parse_key};
#[cfg(feature = "curl")]
pub use curl::CurlFetch;
pub use fetch::{Fetch, HttpReply, check_url};
pub use log::{Failure, LIMIT as LOG_LIMIT, UpdateLog};
