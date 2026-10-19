//! A text editor over 400 or 30 000 lines, the keyboard in it: typing, or
//! the wheel over the text.
use std::cell::Cell;

use arena::{Args, FONT_FAMILY, Step, WINDOW, lines, platform, scripted, unix_ms};
use bunny_ui::prelude::*;

/// The editor's face size. On Windows the shared contract makes every glyph
/// eight units wide: the installed face's advance is 1126 units of a
/// 2048-unit em, so `8 × 2048 / 1126` is the size that lands on it.
#[cfg(target_os = "windows")]
const EDITOR_FONT_SIZE: f64 = 8.0 * 2048.0 / 1126.0;
#[cfg(not(target_os = "windows"))]
const EDITOR_FONT_SIZE: f64 = 13.0;

const OVER: (f64, f64) = (WINDOW.0 * 0.5, WINDOW.1 * 0.5);

#[derive(Clone)]
struct Editor {
    text: State<String>,
}

impl Component for Editor {
    fn body(self) -> impl View {
        line_box(
            text_editor("", self.text.binding())
                .font_family(FONT_FAMILY)
                .font_size(EDITOR_FONT_SIZE)
                .auto_focus(),
        )
    }
}

/// The Windows contract's line box: sixteen units, the rhythm every
/// framework measured beside it is given.
#[cfg(target_os = "windows")]
fn line_box(editor: impl View<Arity = Single>) -> impl View {
    editor.line_height(16.0)
}

/// macOS keeps the face's own line box.
#[cfg(not(target_os = "windows"))]
fn line_box(editor: impl View<Arity = Single>) -> impl View {
    editor
}

fn main() {
    let args = Args::parse();
    let text = State::new(lines(args.lines));
    // The window's own runtime, retained only to observe the actual
    // selection at the two ends of the script.
    let runtime = platform::runtime();
    let observer = runtime.clone();
    let ready = Cell::new(false);
    let script = args.clone();
    let root = scripted(Editor { text }, args, OVER, move |step| {
        if !matches!(script.script.as_str(), "type" | "append") {
            return;
        }
        let done = matches!(step, Step::Done);
        if !done && ready.replace(true) {
            return;
        }
        let Some(snapshot) = observer.ime_snapshot() else {
            eprintln!("EDITOR_STATE_ERROR no focused editor");
            return;
        };
        let kind = if done { "EDITOR_DONE" } else { "EDITOR_READY" };
        let result = if done {
            // Reload after the measurement: a second full document must not
            // inflate the editor's steady-state footprint.
            let original = lines(script.lines);
            let count = (script.secs.max(0.1) * 10.0) as usize;
            let added = if script.script == "append" {
                count
            } else {
                count % 2
            };
            format!(
                " text_ok={}",
                u8::from(matches_text(&snapshot.text, &original, added))
            )
        } else {
            String::new()
        };
        println!(
            "{kind} {} length_utf16={} caret_utf16={} selection_utf16={}{result}",
            unix_ms(),
            snapshot.text.encode_utf16().count(),
            snapshot.selected.0,
            snapshot.selected.1,
        );
        #[cfg(target_os = "windows")]
        caret_witness(&snapshot, observer.last_viewport());
    });
    platform::open("arena — editor", runtime, root);
}

/// Where the caret actually is. The Windows producer types through the
/// system's queue, so the app reports what landed: a script that typed out
/// of view, or into a viewport that never followed the caret, did different
/// work than the one it claims.
#[cfg(target_os = "windows")]
fn caret_witness(
    snapshot: &bunny_ui::runtime::ImeSnapshot,
    viewport: Option<bunny_ui::layout::Size>,
) {
    let caret = snapshot.caret_rect;
    let state = serde_json::json!({
        "caret_rect": [caret.origin.x, caret.origin.y, caret.size.width, caret.size.height],
        "viewport": viewport.map(|size| [size.width, size.height]),
        "caret_visible": viewport.is_some_and(|size| caret.origin.x >= 0.0
            && caret.origin.y >= 0.0
            && caret.origin.x + caret.size.width <= size.width
            && caret.origin.y + caret.size.height <= size.height),
    });
    println!("EDITOR_CARET {} {state}", unix_ms());
}

fn matches_text(actual: &str, original: &str, added: usize) -> bool {
    actual
        .strip_prefix(original)
        .is_some_and(|suffix| suffix.len() == added && suffix.bytes().all(|byte| byte == b'x'))
}

#[cfg(test)]
mod tests {
    use super::matches_text;

    #[test]
    fn completion_checks_the_whole_document_and_exact_suffix() {
        let original = "a😀é\n";
        assert!(matches_text(original, original, 0));
        assert!(matches_text("a😀é\nx", original, 1));
        assert!(matches_text("a😀é\nxxx", original, 3));
        assert!(!matches_text("xa😀é\n", original, 1));
        assert!(!matches_text("a😀é\ny", original, 1));
        assert!(!matches_text("a😀é\nxx", original, 1));
    }
}
