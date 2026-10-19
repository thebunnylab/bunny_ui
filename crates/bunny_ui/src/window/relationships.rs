//! A live ownership forest and a borrow-free native transition guard.

use super::{WindowError, WindowId, WindowParent, WindowParentKind};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// Backend ownership state. Applications use their shell's `App` methods.
#[derive(Default)]
pub struct Relationships {
    live: RefCell<HashMap<WindowId, Option<WindowParent>>>,
    changing: Cell<bool>,
}

impl Relationships {
    /// Registers a fresh window without disturbing any existing relationship.
    pub fn register(&self, id: WindowId) {
        self.live.borrow_mut().entry(id).or_insert(None);
    }

    /// Retires a lifetime and removes every reference to it.
    pub fn retire(&self, id: WindowId) {
        let mut live = self.live.borrow_mut();
        live.remove(&id);
        for parent in live.values_mut() {
            if parent.is_some_and(|parent| parent.window == id) {
                *parent = None;
            }
        }
    }

    /// Reads the committed relationship of a live window.
    ///
    /// # Errors
    /// Returns `StaleWindow` for a closed or foreign lifetime.
    pub fn parent(&self, id: WindowId) -> Result<Option<WindowParent>, WindowError> {
        self.live
            .borrow()
            .get(&id)
            .copied()
            .ok_or(WindowError::StaleWindow)
    }

    /// The direct children, oldest lifetime first. No borrow survives the call.
    #[must_use]
    pub fn children(&self, id: WindowId) -> Vec<WindowId> {
        let mut children: Vec<_> = self
            .live
            .borrow()
            .iter()
            .filter_map(|(&child, parent)| {
                parent
                    .is_some_and(|parent| parent.window == id)
                    .then_some(child)
            })
            .collect();
        children.sort_by_key(|child| child.0);
        children
    }

    /// Reserves a cycle-free change without holding a registry borrow across FFI.
    /// Dropping the guard after native failure leaves the old relation intact.
    ///
    /// # Errors
    /// Rejects stale endpoints, cycles and recursive relationship changes.
    pub fn change(
        &self,
        child: WindowId,
        parent: Option<WindowId>,
    ) -> Result<RelationshipChange<'_>, WindowError> {
        if self.changing.replace(true) {
            return Err(WindowError::RelationshipInProgress);
        }
        let change = RelationshipChange {
            graph: self,
            child,
            parent,
        };
        self.parent(child)?;
        let mut ancestor = parent;
        while let Some(id) = ancestor {
            if id == child {
                return Err(WindowError::RelationshipCycle);
            }
            ancestor = self.parent(id)?.map(WindowParent::window);
        }
        Ok(change)
    }
}

/// A validated native transition. Commit only after the adapter accepts it.
pub struct RelationshipChange<'a> {
    graph: &'a Relationships,
    child: WindowId,
    parent: Option<WindowId>,
}

impl RelationshipChange<'_> {
    /// Publishes a successful native request after revalidating both lifetimes.
    ///
    /// # Errors
    /// Returns `StaleWindow` if a native callback retired either endpoint.
    pub fn commit(self, kind: WindowParentKind) -> Result<(), WindowError> {
        let mut live = self.graph.live.borrow_mut();
        if self
            .parent
            .is_some_and(|parent| !live.contains_key(&parent))
        {
            return Err(WindowError::StaleWindow);
        }
        let entry = live.get_mut(&self.child).ok_or(WindowError::StaleWindow)?;
        *entry = self.parent.map(|window| WindowParent { window, kind });
        Ok(())
    }
}

impl Drop for RelationshipChange<'_> {
    fn drop(&mut self) {
        self.graph.changing.set(false);
    }
}
