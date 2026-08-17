//! The ONE-line text editing model — headless by decision.
//!
//! The app owns the STRING (via `Binding<String>`); the framework owns
//! the caret and the selection. The internal indices are BYTE offsets
//! always on a `char` boundary; the IME boundary speaks UTF-16 — the
//! conversion lives here, ONCE, instead of copied into every handmade
//! field.

/// A field's caret + selection anchor, per identity. `caret` is the
/// active point; `anchor` marks the other side of the selection (None =
/// no selection); `marked` is the live IME composition (underlined, not
/// yet committed). Byte offsets; values outside the text clamp on
/// apply.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct CaretState {
    pub caret: usize,
    pub anchor: Option<usize>,
    pub marked: Option<(usize, usize)>,
}

impl CaretState {
    /// The normalized selection `[start, end)` — `None` = collapsed.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        if anchor == self.caret {
            return None;
        }
        Some((anchor.min(self.caret), anchor.max(self.caret)))
    }
}

/// What keyboard and IME ask of a field. `bool` = extend the selection
/// (shift held). `Read`/`Copy`/`Cut` return text through the output of
/// [`apply`] — the clipboard and platform-synchronization bridge.
#[derive(Clone, PartialEq, Debug)]
pub enum EditCommand {
    Insert(String),
    Backspace,
    Delete,
    Left(bool),
    Right(bool),
    Home(bool),
    End(bool),
    SelectAll,
    /// Reads the whole text without mutating (IME synchronization uses it).
    Read,
    /// Returns the selection without mutating (`None` = collapsed).
    Copy,
    /// Returns the selection and removes it.
    Cut,
    /// IME composition: replaces the marked range (or the selection) with
    /// the composing text and keeps it MARKED (underlined, not
    /// committed). `caret_utf16` = (location, length) INSIDE the marked
    /// text — the platform's vocabulary.
    SetMarked { text: String, caret_utf16: (usize, usize) },
    /// Ends the composition, committing the marked text as it stands.
    Unmark,
}

/// The previous char boundary (or 0).
fn previous_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    loop {
        if index == 0 {
            return 0;
        }
        index -= 1;
        if text.is_char_boundary(index) {
            return index;
        }
    }
}

/// The next char boundary (or the end).
fn next_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    loop {
        if index >= text.len() {
            return text.len();
        }
        index += 1;
        if text.is_char_boundary(index) {
            return index;
        }
    }
}

fn clamp_to_boundary(text: &str, index: usize) -> usize {
    let index = index.min(text.len());
    if text.is_char_boundary(index) {
        index
    } else {
        previous_boundary(text, index)
    }
}

/// Clamps a retained index against the current text — the stamp uses it.
pub(crate) fn clamp_index(text: &str, index: usize) -> usize {
    clamp_to_boundary(text, index)
}

/// Boundaries exposed to the crate (the layout's ellipsis walks them).
pub(crate) fn boundary_after(text: &str, index: usize) -> usize {
    next_boundary(text, index)
}

pub(crate) fn boundary_before(text: &str, index: usize) -> usize {
    previous_boundary(text, index)
}

/// Applies a command to the (text, caret) pair — the ONLY mutation door.
/// State outside the text (the app swapped the string from outside)
/// clamps here. The output is the text `Read`/`Copy`/`Cut` extract.
pub fn apply(text: &mut String, state: &mut CaretState, command: EditCommand) -> Option<String> {
    state.caret = clamp_to_boundary(text, state.caret);
    if let Some(anchor) = state.anchor {
        state.anchor = Some(clamp_to_boundary(text, anchor));
    }
    if let Some((start, end)) = state.marked {
        let start = clamp_to_boundary(text, start);
        let end = clamp_to_boundary(text, end);
        state.marked = (start < end).then_some((start, end));
    }

    // any command that is not composition or reading ends the live
    // composition (commits it as it stands) before acting — except
    // Insert, which is the composition's COMMIT
    if state.marked.is_some()
        && !matches!(
            command,
            EditCommand::Read
                | EditCommand::Copy
                | EditCommand::Insert(_)
                | EditCommand::SetMarked { .. }
        )
    {
        state.marked = None;
    }

    // editing over a selection removes the selection first
    let remove_selection = |text: &mut String, state: &mut CaretState| {
        if let Some((start, end)) = state.selection() {
            text.replace_range(start..end, "");
            state.caret = start;
            state.anchor = None;
            true
        } else {
            state.anchor = None;
            false
        }
    };

    // movement: with shift, the anchor arms at the current point and
    // stays; without shift, a live selection collapses to the move's edge
    let moved = |state: &mut CaretState, select: bool, target: usize, collapse: usize| {
        if select {
            if state.anchor.is_none() {
                state.anchor = Some(state.caret);
            }
            state.caret = target;
        } else if state.selection().is_some() {
            state.caret = collapse;
            state.anchor = None;
        } else {
            state.caret = target;
            state.anchor = None;
        }
    };

    match command {
        EditCommand::Insert(insertion) => {
            // the composition's commit: the final text replaces the marked one
            if let Some((start, end)) = state.marked.take() {
                text.replace_range(start..end, &insertion);
                state.caret = start + insertion.len();
                state.anchor = None;
            } else {
                remove_selection(text, state);
                text.insert_str(state.caret, &insertion);
                state.caret += insertion.len();
            }
        }
        EditCommand::SetMarked { text: composition, caret_utf16 } => {
            let (start, end) = state
                .marked
                .or(state.selection())
                .unwrap_or((state.caret, state.caret));
            text.replace_range(start..end, &composition);
            state.anchor = None;
            if composition.is_empty() {
                // an emptied composition = canceled
                state.marked = None;
                state.caret = start;
            } else {
                state.marked = Some((start, start + composition.len()));
                let (location, length) = caret_utf16;
                state.caret = start + utf16_to_byte(&composition, location + length);
            }
        }
        EditCommand::Unmark => {
            // the clamp/commit up top already cleaned it; nothing more to do
        }
        EditCommand::Read => return Some(text.clone()),
        EditCommand::Copy => {
            return state.selection().map(|(start, end)| text[start..end].to_string());
        }
        EditCommand::Cut => {
            let cut = state.selection().map(|(start, end)| text[start..end].to_string());
            if cut.is_some() {
                remove_selection(text, state);
            }
            return cut;
        }
        EditCommand::Backspace => {
            if !remove_selection(text, state) && state.caret > 0 {
                let start = previous_boundary(text, state.caret);
                text.replace_range(start..state.caret, "");
                state.caret = start;
            }
        }
        EditCommand::Delete => {
            if !remove_selection(text, state) && state.caret < text.len() {
                let end = next_boundary(text, state.caret);
                text.replace_range(state.caret..end, "");
            }
        }
        EditCommand::Left(select) => {
            let target = previous_boundary(text, state.caret);
            let collapse = state.selection().map(|(start, _)| start).unwrap_or(target);
            moved(state, select, target, collapse);
        }
        EditCommand::Right(select) => {
            let target = next_boundary(text, state.caret);
            let collapse = state.selection().map(|(_, end)| end).unwrap_or(target);
            moved(state, select, target, collapse);
        }
        EditCommand::Home(select) => moved(state, select, 0, 0),
        EditCommand::End(select) => {
            let end = text.len();
            moved(state, select, end, end);
        }
        EditCommand::SelectAll => {
            state.anchor = Some(0);
            state.caret = text.len();
        }
    }
    None
}

// MARK: - The UTF-16 boundary (IME and platforms speak UTF-16; we speak bytes)

pub fn utf16_to_byte(text: &str, utf16: usize) -> usize {
    let mut count = 0;
    for (byte_index, ch) in text.char_indices() {
        if count >= utf16 {
            return byte_index;
        }
        count += ch.len_utf16();
    }
    text.len()
}

pub fn byte_to_utf16(text: &str, byte: usize) -> usize {
    text[..clamp_to_boundary(text, byte)].chars().map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(caret: usize) -> CaretState {
        CaretState { caret, anchor: None, marked: None }
    }

    #[test]
    fn insert_backspace_and_delete_respect_char_boundaries() {
        let mut text = String::from("caé");
        let mut caret = state(text.len());

        apply(&mut text, &mut caret, EditCommand::Insert("!".into()));
        assert_eq!(text, "caé!");

        apply(&mut text, &mut caret, EditCommand::Backspace);
        apply(&mut text, &mut caret, EditCommand::Backspace);
        assert_eq!(text, "ca", "the é (2 bytes) comes out whole");
        assert_eq!(caret.caret, 2);

        let mut caret = state(0);
        apply(&mut text, &mut caret, EditCommand::Delete);
        assert_eq!(text, "a");
        assert_eq!(caret.caret, 0);
    }

    #[test]
    fn selection_replaces_collapses_and_extends() {
        let mut text = String::from("hello world");
        let mut caret = state(0);

        apply(&mut text, &mut caret, EditCommand::SelectAll);
        assert_eq!(caret.selection(), Some((0, 11)));

        apply(&mut text, &mut caret, EditCommand::Insert("hi".into()));
        assert_eq!(text, "hi", "typing over the selection replaces it");
        assert_eq!(caret.caret, 2);

        // shift+left arms the anchor and extends
        apply(&mut text, &mut caret, EditCommand::Left(true));
        assert_eq!(caret.selection(), Some((1, 2)));
        // left without shift collapses to the left edge
        apply(&mut text, &mut caret, EditCommand::Left(false));
        assert_eq!(caret.caret, 1);
        assert_eq!(caret.selection(), None);

        apply(&mut text, &mut caret, EditCommand::End(true));
        assert_eq!(caret.selection(), Some((1, 2)));
        apply(&mut text, &mut caret, EditCommand::Backspace);
        assert_eq!(text, "h", "backspace eats the selection");
    }

    #[test]
    fn stale_caret_from_an_outside_write_clamps() {
        // the app swapped the string from outside — the retained state
        // clamps instead of blowing up
        let mut text = String::from("ab");
        let mut caret = state(usize::MAX);
        apply(&mut text, &mut caret, EditCommand::Insert("c".into()));
        assert_eq!(text, "abc");
        assert_eq!(caret.caret, 3);
    }

    #[test]
    fn ime_composition_marks_replaces_and_commits() {
        let mut text = String::from("ab");
        let mut state = state(1); // between a and b

        // incremental composition: each SetMarked replaces the marked text
        apply(
            &mut text,
            &mut state,
            EditCommand::SetMarked { text: "ｎ".into(), caret_utf16: (1, 0) },
        );
        assert_eq!(text, "aｎb");
        assert_eq!(state.marked, Some((1, 1 + "ｎ".len())));
        apply(
            &mut text,
            &mut state,
            EditCommand::SetMarked { text: "に".into(), caret_utf16: (1, 0) },
        );
        assert_eq!(text, "aにb");
        assert_eq!(state.marked, Some((1, 1 + "に".len())));

        // the commit: Insert swaps the marked text for the final one
        apply(&mut text, &mut state, EditCommand::Insert("日本".into()));
        assert_eq!(text, "a日本b");
        assert_eq!(state.marked, None);
        assert_eq!(state.caret, 1 + "日本".len());

        // an emptied composition = canceled
        apply(
            &mut text,
            &mut state,
            EditCommand::SetMarked { text: "x".into(), caret_utf16: (1, 0) },
        );
        apply(
            &mut text,
            &mut state,
            EditCommand::SetMarked { text: String::new(), caret_utf16: (0, 0) },
        );
        assert_eq!(text, "a日本b");
        assert_eq!(state.marked, None);

        // movement mid-composition commits it as it stands
        apply(
            &mut text,
            &mut state,
            EditCommand::SetMarked { text: "y".into(), caret_utf16: (1, 0) },
        );
        apply(&mut text, &mut state, EditCommand::Left(false));
        assert_eq!(state.marked, None);
        assert!(text.contains('y'), "committed as it was: {text}");
    }

    #[test]
    fn the_utf16_border_translates_both_ways() {
        let text = "a🐰b"; // 🐰 = 4 bytes, 2 UTF-16 units
        assert_eq!(byte_to_utf16(text, 0), 0);
        assert_eq!(byte_to_utf16(text, 1), 1);
        assert_eq!(byte_to_utf16(text, 5), 3, "after the rabbit: 1 + 2");
        assert_eq!(utf16_to_byte(text, 1), 1);
        assert_eq!(utf16_to_byte(text, 3), 5);
        assert_eq!(utf16_to_byte(text, 99), text.len());
        // an index in the MIDDLE of the surrogate pair rounds to the
        // NEXT boundary — never splits a char
        assert_eq!(utf16_to_byte(text, 2), 5, "middle of the surrogate lands on the next boundary");
    }
}
