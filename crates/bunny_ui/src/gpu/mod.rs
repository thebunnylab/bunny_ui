//! The half every GPU tier shares: the wire structs a shader reads,
//! the shelf atlas behind text and images, and the walk that turns a
//! display list into batches.
//!
//! The LAW carries over from the rasterizer next door. Every policy
//! decision — snapping, radius clamps, stroke thickness, shadow reach,
//! the clip stack — resolves here, on the CPU, in f64. A tier below is
//! a pure evaluator, and the one seam it must fill is [`walk::AtlasGround`].
//!
//! This module lives beside `raster` and `glass` on purpose: those are
//! the same law written the other way, and the parity tests hold the
//! two against each other. A tier that walks its own scene drifts, and
//! the drift is silent.

pub mod walk;
