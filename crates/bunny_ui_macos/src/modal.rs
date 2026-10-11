//! Scene-owned modal eligibility, independent of `AppKit` object operations.
//!
//! The native adapter updates a scene's ordered dialog stack before making
//! key-window requests. Repeated observation cannot accumulate modal holds.

use std::collections::HashMap;

#[derive(Default)]
pub(super) struct ModalScopes {
    stacks: HashMap<usize, Vec<usize>>,
}

impl ModalScopes {
    pub(super) fn set(&mut self, parent: usize, stack: Vec<usize>) {
        if stack.is_empty() {
            self.forget(parent);
        } else {
            self.stacks.insert(parent, stack);
        }
    }

    pub(super) fn end(&mut self, dialog: usize) {
        self.stacks.retain(|_, stack| {
            stack.retain(|open| *open != dialog);
            !stack.is_empty()
        });
    }

    pub(super) fn forget(&mut self, parent: usize) {
        self.stacks.remove(&parent);
    }

    pub(super) fn blocked(&self) -> impl Iterator<Item = usize> + '_ {
        self.stacks.iter().flat_map(|(&parent, stack)| {
            // Empty stacks are removed by end and set.
            std::iter::once(parent).chain(stack[..stack.len() - 1].iter().copied())
        })
    }

    pub(super) fn is_blocked(&self, window: usize) -> bool {
        self.blocked().any(|blocked| blocked == window)
    }
}

#[cfg(test)]
#[path = "modal_tests.rs"]
mod tests;
