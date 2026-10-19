//! Selected state observations supplement frames without interpreting application text.
//!
//! ## Wiring
//! A connection opts into view identities before requesting one mounted occurrence.
//! Requests increase monotonically, including cancellation. A snapshot becomes usable
//! only after joining the current request, target and validated frame identities.
//! These values describe storage at publication time and the binding's last evaluated
//! dependencies, not an inverse evaluation of the painted text or source ownership.
//! Clients must additionally fence connection lifetime and compiled-source freshness.

use super::{MatchedViews, MountedViewId, SceneInstanceId};
use std::{collections::HashSet, fmt, num::NonZeroU64};
mod codec;

/// Maximum dependencies in a selected observation; never silently truncated.
pub const STATE_DEPENDENCY_LIMIT: usize = 512;
/// Maximum UTF-8 bytes in each value or diagnostic metadata string.
pub const STATE_TEXT_LIMIT: usize = 4096;

/// A monotonically increasing nonzero selection/cancellation request per connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct StateRequestId(NonZeroU64);
/// Zero cannot fence an observation request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidStateRequestId;
impl fmt::Display for InvalidStateRequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("state observation request must be nonzero")
    }
}
impl std::error::Error for InvalidStateRequestId {}
impl StateRequestId {
    /// Validate a request counter; callers must not wrap it on exhaustion.
    pub fn parse(value: u64) -> Result<Self, InvalidStateRequestId> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(InvalidStateRequestId)
    }
    /// The nonzero transport counter.
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// An occurrence selected from one compiled scene's published view metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateTarget {
    /// None addresses a legacy scene; ambiguous legacy selections are refused.
    pub instance: Option<SceneInstanceId>,
    /// Exported preview identifier, not its selection alias.
    pub preview: String,
    /// The selected trait variant.
    pub variant: u32,
    /// Compiled generation that produced the selection.
    pub generation: u32,
    /// The mounted occurrence; neither a source site nor a frame-local index.
    pub mounted: MountedViewId,
}

/// Exact dependency identity in the host thread's current lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DependencyKey {
    /// A State arena slot's non-recycled dependency identity.
    State(u64),
    /// A Store read, whose value inspection is not supported yet.
    Store(u64),
}
/// Raw execution provenance. It does not prove a field or initializer expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateConstructor {
    /// Compiler caller file spelling; only a compiled-source proof can resolve it.
    pub file: String,
    /// One-based caller line.
    pub line: u32,
    /// One-based caller column.
    pub column: u32,
}
/// Built-in storage values; no application formatting or serialization runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateValue {
    /// Full signed integer range.
    Signed(i128),
    /// Full unsigned integer range.
    Unsigned(u128),
    /// IEEE-754 f64 bits preserve NaN payloads and signed zero in equality.
    FloatBits(u64),
    /// Boolean.
    Bool(bool),
    /// Unicode scalar.
    Char(char),
    /// Complete bounded UTF-8 value.
    Text(String),
    /// No built-in inspector for this type.
    Opaque,
    /// The value exceeds the bound; no prefix is sent.
    TooLarge { bytes: u64 },
    /// A compound update temporarily owns the value outside storage.
    Updating,
}
/// Actual state evidence or an explicit missing/unsupported dependency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateEvidence {
    /// A live state observed without mutating or subscribing to it.
    State {
        /// Diagnostic spelling only, never semantic Rust type identity.
        type_name: String,
        /// Original constructor caller, not a guessed field declaration.
        constructor: StateConstructor,
        /// Write version at observation time.
        version: u64,
        /// Built-in value or explicit limitation.
        value: StateValue,
    },
    /// The dependency lifetime ended or no live slot exists.
    Missing,
    /// This dependency kind has no value inspection contract.
    Unsupported,
}
/// One exact retained binding dependency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateDependency {
    /// Host-lifetime dependency identity.
    pub key: DependencyKey,
    /// Observation, without interpreting the source expression.
    pub evidence: StateEvidence,
}
/// Complete outcome of inspecting one mounted occurrence's read binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateRead {
    /// Exact dependencies, possibly empty.
    Values(Vec<StateDependency>),
    /// This node has no supported read binding (for example a stack or literal).
    NoBinding,
    /// The binding was retired or has no recorded dependencies.
    UnavailableBinding,
    /// Application code temporarily borrows storage; no partial values escape.
    Borrowed,
    /// Dependency/value/metadata bounds prevent a complete observation.
    Limit,
}
/// One observation published immediately before its view metadata and frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateSnapshot {
    /// Selection request that still owns the observation.
    pub request: StateRequestId,
    /// Exact selected mounted occurrence.
    pub target: StateTarget,
    /// Exact frame publication sequence.
    pub seq: u64,
    /// Read-only storage outcome at publication time.
    pub read: StateRead,
}
/// A request could not remain active. The client must discard its old value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateRefusal {
    /// View observation must be enabled first, and remain enabled.
    Disabled,
    /// The requested scene is no longer selected.
    SceneMissing,
    /// Multiple legacy scenes share the requested preview/variant.
    AmbiguousScene,
    /// The selected occurrence belongs to another compiled generation.
    Generation,
    /// No current frame has published this scene's mounted identities.
    NotPublished,
    /// The selected mounted view has left or was replaced.
    ViewMissing,
    /// The selected scene panicked.
    Poisoned,
    /// This request cannot supersede a newer selection or cancellation.
    StaleRequest,
    /// Target metadata exceeds the observation bounds.
    Limit,
}
/// Frame-bound data and request outcomes are distinct states on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateReply {
    /// Requires an exact frame/request join before use.
    Snapshot(StateSnapshot),
    /// A cancellation has retired the previous watch.
    Stopped { request: StateRequestId },
    /// The named request has no active watch; older refusals cannot clear a newer one.
    Refused {
        request: StateRequestId,
        reason: StateRefusal,
    },
}
/// Why an observation cannot be treated as current.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateMismatch {
    /// The user selected another occurrence or cancelled.
    Request,
    /// Target scene, generation or occurrence differs from the current selection.
    Target,
    /// The observation belongs to another frame or generation.
    Frame,
    /// The occurrence is absent from the validated frame.
    ViewMissing,
    /// Dependency identities must be unique.
    DuplicateDependency(DependencyKey),
    /// An explicit observation bound was exceeded.
    Limit,
}
impl fmt::Display for StateMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "state observation mismatch: {self:?}")
    }
}
impl std::error::Error for StateMismatch {}
/// Validated connection-local observation. It grants no source-edit authority.
#[derive(Debug)]
pub struct MatchedState<'a> {
    snapshot: &'a StateSnapshot,
}
impl MatchedState<'_> {
    /// The bounded observation joined to the current request and frame.
    pub const fn snapshot(&self) -> &StateSnapshot {
        self.snapshot
    }
}
impl StateRead {
    /// Validate bounds and dependency uniqueness, including locally constructed data.
    pub fn validate(&self) -> Result<(), StateMismatch> {
        let Self::Values(dependencies) = self else {
            return Ok(());
        };
        if dependencies.len() > STATE_DEPENDENCY_LIMIT {
            return Err(StateMismatch::Limit);
        }
        let mut keys = HashSet::with_capacity(dependencies.len());
        for dependency in dependencies {
            if !keys.insert(dependency.key) {
                return Err(StateMismatch::DuplicateDependency(dependency.key));
            }
            if let StateEvidence::State {
                type_name,
                constructor,
                value,
                ..
            } = &dependency.evidence
                && (type_name.len() > STATE_TEXT_LIMIT
                    || constructor.file.len() > STATE_TEXT_LIMIT
                    || matches!(value, StateValue::Text(text) if text.len() > STATE_TEXT_LIMIT))
            {
                return Err(StateMismatch::Limit);
            }
        }
        Ok(())
    }
}
impl StateSnapshot {
    /// Join the caller's current request/target and already validated frame identities.
    /// The caller must additionally discard proofs across connection/source changes.
    pub fn match_views(
        &self,
        views: &MatchedViews<'_>,
        request: StateRequestId,
        target: &StateTarget,
    ) -> Result<MatchedState<'_>, StateMismatch> {
        if self.request != request {
            return Err(StateMismatch::Request);
        }
        if &self.target != target {
            return Err(StateMismatch::Target);
        }
        if target.preview.len() > STATE_TEXT_LIMIT {
            return Err(StateMismatch::Limit);
        }
        let views = views.snapshot();
        if views.instance != target.instance
            || views.preview != target.preview
            || views.variant != target.variant
            || views.generation != target.generation
            || views.seq != self.seq
        {
            return Err(StateMismatch::Frame);
        }
        if !views
            .nodes
            .iter()
            .any(|node| node.mounted == Some(target.mounted))
        {
            return Err(StateMismatch::ViewMissing);
        }
        self.read.validate()?;
        Ok(MatchedState { snapshot: self })
    }
}
