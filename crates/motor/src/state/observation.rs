//! Observe execution without evaluating application code or joining its read graph.
//!
//! ## Wiring
//! The cold edge visits typed arenas through one reader per type. Allocation
//! records only identity and caller location; inspection owns no application
//! values, callbacks or subscriptions. Constructors are execution provenance,
//! not proof of a Rust field, expression or permission to change source.
//!
//! Identities belong to the current thread and process. A transport must add its
//! connection, scene and frame lifetime before presenting a value as current.

use crate::hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use crate::identity::DepKey;
use std::any::Any;
use std::cell::RefCell;
use std::panic::Location;
use std::rc::Rc;
use std::sync::Arc;

/// Maximum number of requested dependencies, before deduplication.
pub const DEPENDENCY_LIMIT: usize = 512;
/// Maximum bytes copied from one string. Oversized values are never truncated.
pub const TEXT_LIMIT: usize = 4096;

/// Built-in values read without calling user-provided formatting or conversion.
#[derive(Clone, Debug)]
pub enum Value {
    /// Signed integers retain their full range.
    Signed(i128),
    /// Unsigned integers retain their full range.
    Unsigned(u128),
    /// Floating point, including non-finite values; this is not JSON.
    Float(f64),
    /// Boolean value.
    Bool(bool),
    /// Unicode scalar.
    Char(char),
    /// A complete UTF-8 string within the size bound.
    Text(String),
    /// A type with no built-in inspector. No Debug/Serialize bound is imposed.
    Opaque,
    /// The value exists but exceeds the bound; no prefix is returned.
    TooLarge { bytes: usize },
    /// A compound update temporarily owns the value outside its slot.
    Updating,
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Signed(a), Self::Signed(b)) => a == b,
            (Self::Unsigned(a), Self::Unsigned(b)) => a == b,
            // Observation equality is byte equality, including NaN payloads and
            // signed zero. It must not invent a change on every publication.
            (Self::Float(a), Self::Float(b)) => a.to_bits() == b.to_bits(),
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Char(a), Self::Char(b)) => a == b,
            (Self::Text(a), Self::Text(b)) => a == b,
            (Self::TooLarge { bytes: a }, Self::TooLarge { bytes: b }) => a == b,
            (Self::Opaque, Self::Opaque) | (Self::Updating, Self::Updating) => true,
            _ => false,
        }
    }
}
impl Eq for Value {}

/// One actual state value and the location that first allocated its lifetime.
#[derive(Clone, Debug, PartialEq)]
pub struct ObservedState {
    /// Diagnostic type spelling, never semantic type identity.
    pub type_name: &'static str,
    /// The original constructor caller, retained when a declaration is revived.
    pub constructor: &'static Location<'static>,
    /// The slot's write count. Reading it does not subscribe to changes.
    pub version: u64,
    /// The observed value, or its explicit inspection limitation.
    pub value: Value,
}

/// Evidence for one requested dependency, without inventing absent state.
#[derive(Clone, Debug, PartialEq)]
pub enum Evidence {
    /// The live state belongs to the current thread's arena.
    State(ObservedState),
    /// The state is absent or its lifetime ended.
    Missing,
    /// Store dependencies do not yet have a value inspection contract.
    Unsupported,
}

/// An execution dependency and its read-only evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct Observation {
    /// The framework dependency identity, not a source name.
    pub dependency: DepKey,
    /// State evidence or an explicit limitation.
    pub evidence: Evidence,
}

/// Why a complete snapshot could not be collected. No partial result escapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// The request exceeds the fixed bound.
    TooManyDependencies,
    /// Application code currently borrows storage; retry after it returns.
    Borrowed,
}
impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooManyDependencies => "too many dependencies to inspect",
            Self::Borrowed => "state storage is currently borrowed",
        })
    }
}
impl std::error::Error for Unavailable {}

/// Inspect requested dependencies, retaining first-occurrence order.
///
/// This does not clone arbitrary application values, format them, record reads,
/// write state, dirty views or add observers. No application callback executes.
pub fn snapshot(dependencies: &[DepKey]) -> Result<Vec<Observation>, Unavailable> {
    if dependencies.len() > DEPENDENCY_LIMIT {
        return Err(Unavailable::TooManyDependencies);
    }
    let wanted: HashSet<u64> = dependencies
        .iter()
        .filter_map(|dep| match dep {
            DepKey::State(id) => Some(*id),
            DepKey::Store(_) => None,
        })
        .collect();
    let mut found = HashMap::default();
    if !wanted.is_empty() {
        super::ARENAS.with(|arenas| {
            let arenas = arenas.try_borrow().map_err(|_| Unavailable::Borrowed)?;
            for arena in arenas.values() {
                (arena.observe)(&*arena.cell, &wanted, &mut found)?;
            }
            Ok(())
        })?;
    }
    let mut seen = HashSet::default();
    Ok(dependencies
        .iter()
        .filter(|dep| seen.insert(**dep))
        .map(|dep| Observation {
            dependency: *dep,
            evidence: match dep {
                DepKey::State(id) => found.remove(id).map_or(Evidence::Missing, Evidence::State),
                DepKey::Store(_) => Evidence::Unsupported,
            },
        })
        .collect())
}

pub(super) type Reader =
    fn(&dyn Any, &HashSet<u64>, &mut HashMap<u64, ObservedState>) -> Result<(), Unavailable>;

pub(super) fn read_arena<T: 'static>(
    cell: &dyn Any,
    wanted: &HashSet<u64>,
    found: &mut HashMap<u64, ObservedState>,
) -> Result<(), Unavailable> {
    // Registered together with the cell by TypeId in arena_cell.
    let arena = cell
        .downcast_ref::<RefCell<super::TypedArena<T>>>()
        .expect("the reader and arena are registered for the same type");
    let arena = arena.try_borrow().map_err(|_| Unavailable::Borrowed)?;
    for slot in &arena.slots {
        if let Some(dep) = slot.dep.filter(|dep| wanted.contains(dep)) {
            found.insert(
                dep,
                ObservedState {
                    type_name: std::any::type_name::<T>(),
                    constructor: slot.constructor,
                    version: slot.writes,
                    value: slot
                        .value
                        .as_ref()
                        .map_or(Value::Updating, |value| inspect(value)),
                },
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "observation_tests.rs"]
mod tests;

fn inspect(value: &dyn Any) -> Value {
    macro_rules! scalar {
        ($variant:ident, $($ty:ty),+) => {$ (
            if let Some(value) = value.downcast_ref::<$ty>() { return Value::$variant((*value).into()); }
        )+};
    }
    scalar!(Signed, i8, i16, i32, i64, i128);
    scalar!(Unsigned, u8, u16, u32, u64, u128);
    if let Some(value) = value.downcast_ref::<isize>() {
        return Value::Signed(*value as i128);
    }
    if let Some(value) = value.downcast_ref::<usize>() {
        return Value::Unsigned(*value as u128);
    }
    scalar!(Float, f32, f64);
    scalar!(Bool, bool);
    scalar!(Char, char);
    let text = value
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| value.downcast_ref::<&'static str>().copied())
        .or_else(|| value.downcast_ref::<Rc<str>>().map(AsRef::as_ref))
        .or_else(|| value.downcast_ref::<Arc<str>>().map(AsRef::as_ref))
        .or_else(|| value.downcast_ref::<Box<str>>().map(AsRef::as_ref));
    match text {
        Some(text) if text.len() > TEXT_LIMIT => Value::TooLarge { bytes: text.len() },
        Some(text) => Value::Text(text.to_owned()),
        None => Value::Opaque,
    }
}
