//! The preview is a shell: unclaimed strokes still need native field edits.
//!
//! ## Wiring
//! Both wire addressing lanes use these adapters. Runtime handlers get the
//! stroke first; the fallback supplies the field commands a platform shell
//! normally supplies. Committed text uses `Text`, never the key-name lane.

use super::stroke_named;
use bunny_ui_core::action::{Key, KeyPattern, Stroke};
use bunny_ui_core::runtime::Runtime;
use bunny_ui_core::text_input::EditCommand;

pub(super) fn key(runtime: &Runtime, name: &str, modifiers: u32) -> bool {
    let Some(stroke) = stroke_named(name, modifiers) else {
        return false;
    };
    let pattern = stroke.pattern;
    if runtime.key_stroke(stroke).handled {
        return true;
    }
    let edit = match pattern.key {
        Key::Char('a' | 'A')
            if (pattern.command || pattern.control) && !pattern.option && !pattern.shift =>
        {
            Some(EditCommand::SelectAll)
        }
        Key::Escape if pattern.is_plain() => return runtime.blur(),
        key if pattern.is_plain() => match key {
            Key::Backspace => Some(EditCommand::Backspace),
            Key::Delete => Some(EditCommand::Delete),
            Key::Left => Some(EditCommand::Left(pattern.shift)),
            Key::Right => Some(EditCommand::Right(pattern.shift)),
            Key::Home => Some(EditCommand::Home(pattern.shift)),
            Key::End => Some(EditCommand::End(pattern.shift)),
            Key::Up => Some(EditCommand::Up(pattern.shift)),
            Key::Down => Some(EditCommand::Down(pattern.shift)),
            Key::Enter => Some(EditCommand::Newline),
            _ => None,
        },
        _ => None,
    };
    edit.is_some_and(|edit| runtime.key(edit).applied)
}

pub(super) fn text(runtime: &Runtime, text: &str) -> bool {
    let mut changed = false;
    for typed in text.chars() {
        let stroke = Stroke {
            pattern: KeyPattern::key(Key::Char(typed)),
            typed: Some(typed),
        };
        changed |= runtime.key_stroke(stroke).handled
            || runtime.key(EditCommand::Insert(typed.to_string())).applied;
    }
    changed
}
