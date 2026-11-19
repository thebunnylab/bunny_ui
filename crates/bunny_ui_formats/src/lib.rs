//! The file formats and digests of bunny-ui, on the standard library
//! alone.
//!
//! The `bunny` command writes the archives an app ships in and checks
//! the digests of what it downloads. The updater reads the same
//! archives back and checks the same digests. Both use this one copy.

#![forbid(unsafe_code)]

pub mod base64;
pub mod deflate;
pub mod ed25519;
pub mod inflate;
pub mod random;
pub mod sha;
pub mod sha512;
pub mod tar;
pub mod zip;
