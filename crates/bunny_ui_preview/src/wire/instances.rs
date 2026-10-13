//! Instance addressing extends the frozen wire without overloading preview names.
//!
//! ## Wiring
//! Select instances only after negotiating `caps::INSTANCES`. Every command
//! addresses one variant, and each reply carries the stable instance identity.

use super::{
    Cursor, Malformed, OverrideOp, Patch, PlacePatch, PointerKind, Put, SiteKey, Size, TouchPhase,
    Traits, Wire,
};
use std::fmt;

/// A stable execution identity, independent of a Rust preview name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SceneInstanceId(String);

/// An instance name is empty, padded, contains control characters or exceeds 256 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidSceneInstanceId;
impl fmt::Display for InvalidSceneInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a scene instance must be a visible unpadded name of at most 256 bytes")
    }
}
impl std::error::Error for InvalidSceneInstanceId {}

impl SceneInstanceId {
    /// Validate an opaque stable identity; tools own its namespace.
    pub fn parse(value: &str) -> Result<Self, InvalidSceneInstanceId> {
        if value.is_empty()
            || value.len() > 256
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(InvalidSceneInstanceId);
        }
        Ok(Self(value.to_owned()))
    }
    /// The identity carried on the wire.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(super) fn put(&self, out: &mut Vec<u8>) {
        out.string(&self.0);
    }
    pub(super) fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Self::parse(&c.string()?).map_err(|_| Malformed::InvalidSceneInstance)
    }
}
impl fmt::Display for SceneInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// One source preview mounted under its own execution identity and traits.
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceSelection {
    /// Stable identity retained across selection order and source reloads.
    pub instance: SceneInstanceId,
    /// The exported source preview to mount.
    pub ident: String,
    /// The instance's rendering traits; variants remain separately addressable.
    pub traits: Traits,
}
impl InstanceSelection {
    pub(super) fn put(&self, out: &mut Vec<u8>) {
        self.instance.put(out);
        out.string(&self.ident);
        self.traits.put(out);
    }
    pub(super) fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Self {
            instance: SceneInstanceId::get(c)?,
            ident: c.string()?,
            traits: Traits::get(c)?,
        })
    }
}

/// An operation on exactly one instance variant; global host operations stay separate.
#[derive(Clone, Debug, PartialEq)]
pub enum InstanceCommand {
    /// Change rendering traits of the addressed variant.
    Retrait(Traits),
    /// Change its logical dimensions.
    Resize(Size),
    /// Deliver a pointer event.
    Pointer {
        kind: PointerKind,
        x: f64,
        y: f64,
        modifiers: u32,
    },
    /// Deliver a wheel event.
    Wheel { x: f64, y: f64, dx: f64, dy: f64 },
    /// Deliver a key event.
    Key { key: String, modifiers: u32 },
    /// Deliver committed text.
    Text { text: String },
    /// Deliver a touch event.
    Touch {
        phase: TouchPhase,
        touch: u64,
        x: f64,
        y: f64,
        taps: u8,
    },
    /// Deliver a pinch event.
    Magnify { x: f64, y: f64, scale: f64 },
    /// Apply a source modifier only to this scene.
    Override {
        site: SiteKey,
        op: OverrideOp,
        patch: Patch,
    },
    /// Apply placement only to this scene.
    Place {
        site: SiteKey,
        op: OverrideOp,
        patch: PlacePatch,
    },
    /// Remove this scene's provisional overrides.
    ClearOverrides,
    /// Request this scene's pixels.
    RequestSnapshot,
}

impl InstanceCommand {
    pub(super) fn put(&self, out: &mut Vec<u8>) {
        match self {
            Self::Retrait(traits) => {
                out.u8(0);
                traits.put(out);
            }
            Self::Resize(size) => {
                out.u8(1);
                size.put(out);
            }
            Self::Pointer {
                kind,
                x,
                y,
                modifiers,
            } => {
                out.u8(2);
                kind.put(out);
                out.f64(*x);
                out.f64(*y);
                out.u32(*modifiers);
            }
            Self::Wheel { x, y, dx, dy } => {
                out.u8(3);
                out.f64(*x);
                out.f64(*y);
                out.f64(*dx);
                out.f64(*dy);
            }
            Self::Key { key, modifiers } => {
                out.u8(4);
                out.string(key);
                out.u32(*modifiers);
            }
            Self::Text { text } => {
                out.u8(5);
                out.string(text);
            }
            Self::Touch {
                phase,
                touch,
                x,
                y,
                taps,
            } => {
                out.u8(6);
                phase.put(out);
                out.u64(*touch);
                out.f64(*x);
                out.f64(*y);
                out.u8(*taps);
            }
            Self::Magnify { x, y, scale } => {
                out.u8(7);
                out.f64(*x);
                out.f64(*y);
                out.f64(*scale);
            }
            Self::Override { site, op, patch } => {
                out.u8(8);
                site.put(out);
                op.put(out);
                patch.put(out);
            }
            Self::Place { site, op, patch } => {
                out.u8(9);
                site.put(out);
                op.put(out);
                patch.put(out);
            }
            Self::ClearOverrides => {
                out.u8(10);
            }
            Self::RequestSnapshot => {
                out.u8(11);
            }
        }
    }
    pub(super) fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(match c.u8()? {
            0 => Self::Retrait(Traits::get(c)?),
            1 => Self::Resize(Size::get(c)?),
            2 => Self::Pointer {
                kind: PointerKind::get(c)?,
                x: c.f64()?,
                y: c.f64()?,
                modifiers: c.u32()?,
            },
            3 => Self::Wheel {
                x: c.f64()?,
                y: c.f64()?,
                dx: c.f64()?,
                dy: c.f64()?,
            },
            4 => Self::Key {
                key: c.string()?,
                modifiers: c.u32()?,
            },
            5 => Self::Text { text: c.string()? },
            6 => Self::Touch {
                phase: TouchPhase::get(c)?,
                touch: c.u64()?,
                x: c.f64()?,
                y: c.f64()?,
                taps: c.u8()?,
            },
            7 => Self::Magnify {
                x: c.f64()?,
                y: c.f64()?,
                scale: c.f64()?,
            },
            8 => Self::Override {
                site: SiteKey::get(c)?,
                op: OverrideOp::get(c)?,
                patch: Patch::get(c)?,
            },
            9 => Self::Place {
                site: SiteKey::get(c)?,
                op: OverrideOp::get(c)?,
                patch: PlacePatch::get(c)?,
            },
            10 => Self::ClearOverrides,
            11 => Self::RequestSnapshot,
            tag => {
                return Err(Malformed::BadTag {
                    what: "InstanceCommand",
                    tag,
                });
            }
        })
    }
}
