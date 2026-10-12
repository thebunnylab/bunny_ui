//! Native paragraph levels must survive the runtime's soft-wrapped editor rows.
use bunny_ui::layout::{DrawCommand, Proposal, Size};
use bunny_ui::prelude::*;
use bunny_ui::text_engine::TextEngine;
use bunny_ui::text_input::byte_to_utf16;
use std::rc::Rc;

/// Compare interior letter edges with independently resolved paragraph levels.
/// This deliberately checks direction only; it does not certify raster, widths,
/// selection spans, neutral resolution or actual operating-system IME behavior.
pub fn directions_match_paragraph(
    engine: Rc<dyn TextEngine>,
    text: &str,
    width: f64,
    native_levels: &[(usize, bool)],
) {
    let runtime = Runtime::new().text_engine(engine);
    let draft = State::new(text.to_owned());
    let view = text_editor("", draft.binding())
        .font_size(15.0)
        .frame(width, 400.0)
        .id("editor");
    let size = Size {
        width,
        height: 400.0,
    };
    runtime.display_frame(&view, size).unwrap();
    let layout = runtime.layout(&view, Proposal::exact(size)).unwrap();
    assert!(runtime.focus_named("editor"));
    let mut rows = 0;
    let mut checked = 0;
    let mut mismatches = Vec::new();
    for command in layout.display.iter() {
        let DrawCommand::TextLine { content, range, .. } = command else {
            continue;
        };
        if content.as_ref() != text || range.0 == range.1 {
            continue;
        }
        rows += 1;
        let letters: Vec<_> = text[range.0..range.1].char_indices().collect();
        // Both positions are inside the same word and visual row. Avoid split
        // caret affinity at run/line boundaries and L1's whitespace resetting.
        for pair in letters.windows(2).skip(1) {
            let [(local, character), (next, following)] = pair else {
                unreachable!()
            };
            if !character.is_alphabetic() || !following.is_alphabetic() {
                continue;
            }
            let byte = range.0 + local;
            let expected_rtl = native_levels.iter().find(|(at, _)| *at == byte).unwrap().1;
            let index = byte_to_utf16(text, byte);
            let next_index = byte_to_utf16(text, range.0 + next);
            let before = runtime.ime_rect_for(index).unwrap().unwrap().origin.x;
            let after = runtime.ime_rect_for(next_index).unwrap().unwrap().origin.x;
            checked += 1;
            if (after - before).abs() < 0.01 || (after < before) != expected_rtl {
                mismatches.push(format!(
                    "row {range:?}, UTF16 {index}..{next_index}, native RTL={expected_rtl}, runtime x={before}..{after}"
                ));
            }
        }
    }
    assert!(
        rows >= 2 && checked >= 3,
        "fixture must exercise wrapped letter interiors"
    );
    assert!(
        mismatches.is_empty(),
        "paragraph direction lost for {text:?}:\n{}",
        mismatches.join("\n")
    );
}
