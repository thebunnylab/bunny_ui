//! Mounted identities travel beside frozen frames and require an exact join.

use std::collections::HashSet;
use std::fmt;
use std::num::NonZeroU64;

use super::{Cursor, Frame, Malformed, Put, SceneInstanceId};

/// A host-lifetime mounted identity, never a frame index or source location.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MountedViewId(NonZeroU64);

/// Zero cannot identify a mounted view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidMountedViewId;
impl fmt::Display for InvalidMountedViewId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a mounted view ID must be nonzero")
    }
}
impl std::error::Error for InvalidMountedViewId {}
impl MountedViewId {
    /// Validate a value read from a host or transport.
    pub fn parse(value: u64) -> Result<Self, InvalidMountedViewId> {
        NonZeroU64::new(value).map(Self).ok_or(InvalidMountedViewId)
    }
    /// The nonzero wire value.
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// One frame-local site and its optional mounted identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewIdentity {
    /// Index in the accompanying frame's site table.
    pub index: u32,
    /// None means identity is unavailable, not an index-based fallback.
    pub mounted: Option<MountedViewId>,
}

/// Additional metadata for exactly one unchanged version-1 frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewSnapshot {
    /// None addresses the legacy preview lane.
    pub instance: Option<SceneInstanceId>,
    /// Source preview identifier.
    pub preview: String,
    /// Preview variant.
    pub variant: u32,
    /// Compiled generation; connection lifetime is fenced separately by the client.
    pub generation: u32,
    /// Exact frame publication sequence.
    pub seq: u64,
    /// One record for every site of the matching frame.
    pub nodes: Vec<ViewIdentity>,
}

/// Identity metadata cannot establish a unique correspondence to this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMismatch {
    /// Another scene, preview or variant.
    Scene,
    /// Another build or frame publication.
    Frame,
    /// A site index occurs more than once.
    DuplicateSite(u32),
    /// Two sites claim one mounted occurrence.
    DuplicateView(MountedViewId),
    /// Metadata does not describe this frame site.
    MissingSite(u32),
    /// Metadata names a site absent from this frame.
    UnknownSite(u32),
    /// A site explicitly has no mounted identity.
    UnavailableSite(u32),
}
impl fmt::Display for ViewMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scene => f.write_str("view metadata belongs to another scene"),
            Self::Frame => f.write_str("view metadata belongs to another frame or build"),
            Self::DuplicateSite(index) => write!(f, "duplicate view site {index}"),
            Self::DuplicateView(id) => write!(f, "duplicate mounted view {}", id.get()),
            Self::MissingSite(index) => write!(f, "missing view site {index}"),
            Self::UnknownSite(index) => write!(f, "unknown view site {index}"),
            Self::UnavailableSite(index) => write!(f, "unavailable identity for view site {index}"),
        }
    }
}
impl std::error::Error for ViewMismatch {}

/// A snapshot whose identities uniquely cover the addressed frame's sites.
/// This proof is connection-local and grants no source-edit ownership.
#[derive(Debug)]
pub struct MatchedViews<'a> {
    snapshot: &'a ViewSnapshot,
}
impl MatchedViews<'_> {
    /// Validated frame metadata; callers still fence their host connection.
    pub const fn snapshot(&self) -> &ViewSnapshot {
        self.snapshot
    }
}

impl ViewSnapshot {
    /// Establish an exact, complete and unique frame/scene correspondence.
    pub fn match_frame(
        &self,
        frame: &Frame,
        instance: Option<&SceneInstanceId>,
    ) -> Result<MatchedViews<'_>, ViewMismatch> {
        if self.instance.as_ref() != instance
            || self.preview != frame.preview
            || self.variant != frame.variant
        {
            return Err(ViewMismatch::Scene);
        }
        if self.generation != frame.generation || self.seq != frame.seq {
            return Err(ViewMismatch::Frame);
        }
        let mut sites = HashSet::with_capacity(frame.sites.len());
        for site in &frame.sites {
            if !sites.insert(site.index) {
                return Err(ViewMismatch::DuplicateSite(site.index));
            }
        }
        let mut seen = HashSet::with_capacity(self.nodes.len());
        let mut mounted = HashSet::with_capacity(self.nodes.len());
        for node in &self.nodes {
            if !seen.insert(node.index) {
                return Err(ViewMismatch::DuplicateSite(node.index));
            }
            if !sites.contains(&node.index) {
                return Err(ViewMismatch::UnknownSite(node.index));
            }
            let id = node
                .mounted
                .ok_or(ViewMismatch::UnavailableSite(node.index))?;
            if !mounted.insert(id) {
                return Err(ViewMismatch::DuplicateView(id));
            }
        }
        for site in &frame.sites {
            if !seen.contains(&site.index) {
                return Err(ViewMismatch::MissingSite(site.index));
            }
        }
        Ok(MatchedViews { snapshot: self })
    }

    pub(super) fn put(&self, out: &mut Vec<u8>) {
        out.option(self.instance.as_ref(), |out, instance| instance.put(out));
        out.string(&self.preview);
        out.u32(self.variant);
        out.u32(self.generation);
        out.u64(self.seq);
        out.list(&self.nodes, |out, node| {
            out.u32(node.index);
            out.option(node.mounted.as_ref(), |out, id| out.u64(id.get()));
        });
    }
    pub(super) fn get(c: &mut Cursor<'_>) -> Result<Self, Malformed> {
        Ok(Self {
            instance: c.option(SceneInstanceId::get)?,
            preview: c.string()?,
            variant: c.u32()?,
            generation: c.u32()?,
            seq: c.u64()?,
            nodes: c.list(|c| {
                Ok(ViewIdentity {
                    index: c.u32()?,
                    mounted: c.option(|c| {
                        MountedViewId::parse(c.u64()?).map_err(|_| Malformed::InvalidMountedView)
                    })?,
                })
            })?,
        })
    }
}
