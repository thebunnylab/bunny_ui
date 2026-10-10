//! File formats and digests `bunny` reads and writes itself, on the
//! standard library alone.

pub mod sha;
pub mod zip;
pub mod deflate;
pub mod tar;
pub mod elf;
pub mod pe;
pub mod res;
