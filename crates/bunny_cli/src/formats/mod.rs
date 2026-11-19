//! File formats and digests `bunny` reads and writes itself, on the
//! standard library alone.
//!
//! The digests and the archives live in `bunny-ui-formats`, which the
//! updater reads too. The executable and image formats stay here.

pub use bunny_ui_formats::{deflate, inflate, sha, tar, zip};

pub mod elf;
pub mod pe;
pub mod png;
pub mod res;
