//! A field's bounded replacement history, independent of application commands.
//!
//! The current text shares the existing lending cache. Older entries retain only
//! changed bytes and selection, not a copy of the document per keystroke.

use crate::{
    menu::Edit,
    reconciler::LentText,
    text_input::{CaretState, EditCommand},
};
use motor::state::Binding;
use std::{cell::RefCell, collections::VecDeque, rc::Rc, sync::Arc};

const MAX_GROUPS: usize = 128;
const MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
struct Snapshot {
    text: Arc<str>,
    version: Option<(u64, u64)>,
}
impl Snapshot {
    fn same(&self, other: &Self) -> bool {
        match (self.version, other.version) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.text == other.text,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Typing,
    Backspace,
    Delete,
    Composition,
    Separate,
}
impl Kind {
    fn of(command: &EditCommand, caret: CaretState) -> Self {
        match command {
            EditCommand::SetMarked { text, .. } if text.is_empty() && caret.marked.is_none() => {
                Self::Separate
            }
            EditCommand::SetMarked { .. } => Self::Composition,
            EditCommand::Insert(_) | EditCommand::Unmark if caret.marked.is_some() => {
                Self::Composition
            }
            EditCommand::Insert(text)
                if caret.selection().is_none()
                    && text.chars().count() == 1
                    && !text.chars().any(char::is_whitespace) =>
            {
                Self::Typing
            }
            EditCommand::Backspace if caret.selection().is_none() => Self::Backspace,
            EditCommand::Delete if caret.selection().is_none() => Self::Delete,
            _ => Self::Separate,
        }
    }
}

struct Patch {
    start: usize,
    removed: String,
    inserted: String,
}
impl Patch {
    fn between(before: &str, after: &str) -> Option<Self> {
        if before == after {
            return None;
        }
        let mut start = before
            .bytes()
            .zip(after.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        while !before.is_char_boundary(start) || !after.is_char_boundary(start) {
            start -= 1;
        }
        let mut suffix = before[start..]
            .bytes()
            .rev()
            .zip(after[start..].bytes().rev())
            .take_while(|(a, b)| a == b)
            .count();
        while !before.is_char_boundary(before.len() - suffix)
            || !after.is_char_boundary(after.len() - suffix)
        {
            suffix -= 1;
        }
        Some(Self {
            start,
            removed: before[start..before.len() - suffix].into(),
            inserted: after[start..after.len() - suffix].into(),
        })
    }
    fn bytes(&self) -> usize {
        size_of::<Self>() + self.removed.len() + self.inserted.len()
    }
    fn apply(&self, text: &mut String, forward: bool) -> bool {
        let (old, new) = if forward {
            (&self.removed, &self.inserted)
        } else {
            (&self.inserted, &self.removed)
        };
        let Some(end) = self.start.checked_add(old.len()) else {
            return false;
        };
        if text.get(self.start..end) != Some(old) {
            return false;
        }
        text.replace_range(self.start..end, new);
        true
    }
}
struct Group {
    edits: Vec<Patch>,
    before: CaretState,
    after: CaretState,
    kind: Kind,
    bytes: usize,
}
impl Group {
    fn restore(&self, text: &str, forward: bool) -> Option<String> {
        let mut result = text.to_owned();
        let valid = if forward {
            self.edits.iter().all(|edit| edit.apply(&mut result, true))
        } else {
            self.edits
                .iter()
                .rev()
                .all(|edit| edit.apply(&mut result, false))
        };
        valid.then_some(result)
    }
}

#[derive(Default, PartialEq, Eq)]
enum CompositionHistory {
    #[default]
    Retained,
    Evicted,
}

#[derive(Default)]
struct History {
    baseline: Option<Snapshot>,
    undo: VecDeque<Group>,
    redo: Vec<Group>,
    bytes: usize,
    join: bool,
    composition: CompositionHistory,
}
impl History {
    fn clear(&mut self, snapshot: Snapshot) {
        *self = Self {
            baseline: Some(snapshot),
            ..Self::default()
        };
    }
    fn sync(&mut self, snapshot: &Snapshot) {
        if self
            .baseline
            .as_ref()
            .is_none_or(|known| !known.same(snapshot))
        {
            self.clear(snapshot.clone());
        }
    }
    fn clear_redo(&mut self) {
        self.bytes -= self.redo.iter().map(|group| group.bytes).sum::<usize>();
        self.redo.clear();
    }
    fn finish_composition(&mut self, snapshot: &Snapshot, mut caret: CaretState) {
        let Some(group) = self
            .undo
            .back_mut()
            .filter(|group| group.kind == Kind::Composition)
        else {
            return;
        };
        caret.marked = None;
        group.after = caret;
        let cancelled = group
            .restore(&snapshot.text, false)
            .is_some_and(|before| before == *snapshot.text);
        if cancelled {
            if let Some(group) = self.undo.pop_back() {
                self.bytes -= group.bytes;
            }
        } else {
            self.clear_redo();
        }
        self.join = false;
    }
    fn record(&mut self, before: Before, after: Snapshot, caret: CaretState) {
        if self
            .baseline
            .as_ref()
            .is_none_or(|known| !known.same(&before.snapshot))
        {
            self.clear(after);
            return;
        }
        if self.composition == CompositionHistory::Evicted {
            self.clear_redo();
            if caret.marked.is_none() {
                self.composition = CompositionHistory::Retained;
            }
            // Once its start is gone, no suffix of a preedit is undoable.
            if before.kind == Kind::Composition {
                self.baseline = Some(after);
                return;
            }
        }
        if before.caret.marked.is_some() && before.kind != Kind::Composition {
            self.finish_composition(&before.snapshot, before.caret);
        }
        if before.kind == Kind::Composition
            && before.caret.marked.is_none()
            && caret.marked.is_some()
        {
            // A first preedit can replace the selection with identical bytes.
            // Keep that original selection even before there is a text patch.
            self.undo.push_back(Group {
                edits: Vec::new(),
                before: before.caret,
                after: before.caret,
                kind: Kind::Composition,
                bytes: 0,
            });
        }
        if let Some(patch) = Patch::between(&before.snapshot.text, &after.text) {
            if before.kind != Kind::Composition {
                self.clear_redo();
            }
            let bytes = patch.bytes();
            let join = self.undo.back().is_some_and(|group| {
                group.kind == before.kind
                    && if before.kind == Kind::Composition {
                        group.after.marked.is_some() || group.edits.is_empty()
                    } else {
                        self.join && before.kind != Kind::Separate && group.after == before.caret
                    }
            });
            if let Some(group) = self.undo.back_mut().filter(|_| join) {
                group.edits.push(patch);
                group.after = caret;
                group.bytes += bytes;
            } else {
                self.undo.push_back(Group {
                    edits: vec![patch],
                    before: before.caret,
                    after: caret,
                    kind: before.kind,
                    bytes,
                });
            }
            self.bytes += bytes;
            self.join = before.kind != Kind::Separate;
        } else if before.kind != Kind::Composition {
            self.join = false;
        } else if let Some(group) = self
            .undo
            .back_mut()
            .filter(|group| group.kind == Kind::Composition)
        {
            group.after = caret;
        }
        if before.kind == Kind::Composition && caret.marked.is_none() {
            self.finish_composition(&after, caret);
        }
        self.baseline = Some(after);
        while self.undo.len() > MAX_GROUPS || self.bytes > MAX_BYTES {
            if let Some(group) = self.undo.pop_front() {
                self.bytes -= group.bytes;
            } else {
                self.clear_redo();
                break;
            }
            if self.undo.is_empty() {
                self.join = false;
                if caret.marked.is_some() {
                    self.composition = CompositionHistory::Evicted;
                    self.clear_redo();
                }
            }
        }
    }
    fn group(&self, edit: Edit) -> Option<&Group> {
        match edit {
            Edit::Undo => self.undo.back(),
            Edit::Redo => self.redo.last(),
            _ => None,
        }
    }
}

pub(crate) struct Before {
    snapshot: Snapshot,
    caret: CaretState,
    kind: Kind,
}

/// The binding adapter is refreshed at render; only history memory is retained.
#[derive(Clone)]
pub(crate) struct FieldHistory {
    memory: Rc<RefCell<History>>,
    binding: Binding<String>,
    lent: LentText,
}
impl FieldHistory {
    pub(crate) fn new(binding: Binding<String>, lent: LentText, previous: Option<Self>) -> Self {
        Self {
            memory: previous.map_or_else(Default::default, |old| old.memory),
            binding,
            lent,
        }
    }
    fn snapshot(&self) -> Snapshot {
        let version = self.binding.version();
        let text = self
            .binding
            .with(|value| crate::text_input::lend_text(&self.lent, value, version));
        Snapshot { text, version }
    }
    pub(crate) fn begin(&self, command: &EditCommand, caret: &mut CaretState) -> Option<Before> {
        if matches!(command, EditCommand::Read | EditCommand::Copy) {
            return None;
        }
        let snapshot = self.snapshot();
        crate::text_input::clamp_state(&snapshot.text, caret);
        self.memory.borrow_mut().sync(&snapshot);
        Some(Before {
            snapshot,
            caret: *caret,
            kind: Kind::of(command, *caret),
        })
    }
    pub(crate) fn finish(&self, before: Option<Before>, caret: CaretState) {
        if let Some(before) = before {
            let after = self.snapshot();
            self.memory.borrow_mut().record(before, after, caret);
        }
    }
    pub(crate) fn separate(&self) {
        self.memory.borrow_mut().join = false;
    }
    pub(crate) fn available(&self, edit: Edit, caret: CaretState) -> bool {
        let snapshot = self.snapshot();
        let mut history = self.memory.borrow_mut();
        history.sync(&snapshot);
        caret.marked.is_none() && history.group(edit).is_some()
    }
    pub(crate) fn restore(&self, edit: Edit, caret: &mut CaretState) -> bool {
        if caret.marked.is_some() {
            return false;
        }
        let snapshot = self.snapshot();
        let prepared = {
            let mut history = self.memory.borrow_mut();
            history.sync(&snapshot);
            let Some(group) = history.group(edit) else {
                return false;
            };
            group
                .restore(&snapshot.text, edit == Edit::Redo)
                .map(|text| {
                    (
                        text,
                        if edit == Edit::Undo {
                            group.before
                        } else {
                            group.after
                        },
                    )
                })
        };
        let Some((text, selection)) = prepared else {
            self.memory.borrow_mut().clear(snapshot);
            return false;
        };
        // No history borrow survives a setter, which may re-enter the runtime.
        self.binding.set(text.clone());
        let after = self.snapshot();
        let mut history = self.memory.borrow_mut();
        if after.text.as_ref() != text
            || history
                .baseline
                .as_ref()
                .is_none_or(|known| !known.same(&snapshot))
        {
            history.clear(after);
        } else {
            match edit {
                Edit::Undo => {
                    if let Some(group) = history.undo.pop_back() {
                        history.redo.push(group);
                    }
                }
                Edit::Redo => {
                    if let Some(group) = history.redo.pop() {
                        history.undo.push_back(group);
                    }
                }
                _ => return false,
            }
            history.baseline = Some(after);
            history.join = false;
            *caret = selection;
            caret.marked = None;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_patches_round_trip_shared_utf8_prefixes_and_suffixes() {
        let samples = [
            "",
            "é",
            "ê",
            "日",
            "本",
            "aéz",
            "aêz",
            "👩🏽‍🚀",
            "e\u{301}",
            "🐰🐰",
        ];
        for before in samples {
            for after in samples {
                let patch = Patch::between(before, after);
                if before == after {
                    assert!(patch.is_none());
                } else {
                    let patch = patch.unwrap();
                    let mut value = before.to_owned();
                    assert!(patch.apply(&mut value, true));
                    assert_eq!(value, after);
                    assert!(patch.apply(&mut value, false));
                    assert_eq!(value, before);
                }
            }
        }
    }

    #[test]
    fn a_patch_refuses_an_incompatible_range_without_mutation() {
        let patch = Patch::between("a日z", "a本z").unwrap();
        for original in ["", "a", "a🐰z", "a本z"] {
            let mut value = original.to_owned();
            assert!(!patch.apply(&mut value, true));
            assert_eq!(value, original);
        }
    }
}
