//! The file formats and digests of bunny-ui, on the standard library
//! alone.
//!
//! The `bunny` command writes the archives an app ships in, checks the
//! digests of what it downloads and signs a release. The updater reads
//! the same archives back, checks the same digests and verifies the
//! signature. Both use this one copy.

#![forbid(unsafe_code)]

pub mod base64;
pub mod deflate;
pub mod ed25519;
pub mod inflate;
pub mod paths;
pub mod random;
pub mod sha;
pub mod sha512;
pub mod tar;
mod tar_read;
pub mod zip;
mod zip_read;
