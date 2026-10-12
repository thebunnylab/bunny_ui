//! Native text failures are distinct from an empty raster or unsupported geometry.
//!
//! ## Wiring
//! Contextual text operations carry these values to their caller. Native handles
//! and platform error objects remain in their adapters; operation names identify
//! the failed boundary without changing application text or selecting a fallback.

use crate::text_geometry::GeometryError;
use crate::text_source::TextRangeError;

/// A contextual text operation that could not produce a valid result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextError {
    /// Source coordinates cannot be represented by the native API.
    IndexOverflow { engine: &'static str },
    /// Native paragraph preparation refused the input or a required allocation.
    Preparation {
        engine: &'static str,
        operation: &'static str,
    },
    /// A native bitmap or its backing memory could not be created.
    Rasterization {
        engine: &'static str,
        operation: &'static str,
    },
    /// Native advances or font metrics were not finite.
    InvalidMetrics { engine: &'static str },
    /// A source range was rejected before native preparation.
    Range(TextRangeError),
    /// A native cell map did not satisfy the shared geometry contract.
    Geometry(GeometryError),
}
impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IndexOverflow { engine } => {
                write!(f, "{engine}: text exceeds native index limits")
            }
            Self::Preparation { engine, operation } => {
                write!(f, "{engine}: text preparation failed at {operation}")
            }
            Self::Rasterization { engine, operation } => {
                write!(f, "{engine}: text rasterization failed at {operation}")
            }
            Self::InvalidMetrics { engine } => {
                write!(f, "{engine}: native text metrics are not finite")
            }
            Self::Range(error) => error.fmt(f),
            Self::Geometry(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for TextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Range(error) => Some(error),
            Self::Geometry(error) => Some(error),
            _ => None,
        }
    }
}
impl From<TextRangeError> for TextError {
    fn from(error: TextRangeError) -> Self {
        Self::Range(error)
    }
}
impl From<GeometryError> for TextError {
    fn from(error: GeometryError) -> Self {
        Self::Geometry(error)
    }
}
