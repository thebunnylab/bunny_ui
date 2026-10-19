//! Provenance declarations follow the structural cursor without owning state.
//!
//! Retained subtrees keep their declarations; an evaluated subtree retires every
//! declaration it did not visit. The ID is never a source-edit capability.

use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::hash::FxHashMap as HashMap;
use crate::runtime::Site;

/// A mounted declaration's identity, never reused during this process lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MountedId(NonZeroU64);

impl MountedId {
    /// The nonzero value transported alongside a provenance snapshot.
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    fn fresh() -> Option<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let mut value = NEXT.load(Ordering::Relaxed);
        loop {
            let next = value.checked_add(1)?;
            match NEXT.compare_exchange_weak(value, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return NonZeroU64::new(value).map(Self),
                Err(current) => value = current,
            }
        }
    }
}

#[derive(Default)]
struct Declaration {
    pass: u64,
    used: usize,
    ids: Vec<MountedId>,
}

/// Lazily populated only by a runtime recording provenance.
#[derive(Default)]
pub(super) struct MountedSites {
    scopes: HashMap<String, HashMap<(Site, &'static str), Declaration>>,
}

impl MountedSites {
    pub(super) fn is_empty(&self) -> bool {
        self.scopes.is_empty()
    }

    pub(super) fn claim(
        &mut self,
        scope: &str,
        site: Site,
        role: &'static str,
        pass: u64,
    ) -> Option<MountedId> {
        if !self.scopes.contains_key(scope) {
            self.scopes.insert(scope.to_owned(), HashMap::default());
        }
        let declaration = self.scopes.get_mut(scope)?.entry((site, role)).or_default();
        if declaration.pass != pass {
            declaration.pass = pass;
            declaration.used = 0;
        }
        let id = match declaration.ids.get(declaration.used) {
            Some(id) => *id,
            None => {
                let id = MountedId::fresh()?;
                declaration.ids.push(id);
                id
            }
        };
        declaration.used += 1;
        Some(id)
    }

    pub(super) fn sweep(&mut self, root: &str, pass: u64, protected: impl Fn(&str) -> bool) {
        self.scopes.retain(|scope, declarations| {
            if !under(scope, root) || protected(scope) {
                return true;
            }
            declarations.retain(|_, declaration| {
                if declaration.pass != pass {
                    return false;
                }
                declaration.ids.truncate(declaration.used);
                true
            });
            !declarations.is_empty()
        });
    }

    pub(super) fn reset(&mut self, root: &str) {
        self.scopes.retain(|scope, _| !under(scope, root));
    }
}

fn under(scope: &str, root: &str) -> bool {
    scope == root
        || scope
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}
