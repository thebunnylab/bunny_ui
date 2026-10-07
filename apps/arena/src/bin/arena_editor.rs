//! A text editor over 400 or 30 000 lines, the keyboard in it: typing, or
//! the wheel over the text.
use std::cell::Cell;
use std::rc::Rc;

use arena::{Args, FONT_FAMILY, Step, WINDOW, lines, scripted, unix_ms};
use bunny_ui::prelude::*;
use bunny_ui_macos::{App, CoreGraphicsImageEngine, CoreTextEngine, WindowSpec};

const OVER: (f64, f64) = (WINDOW.0 * 0.5, WINDOW.1 * 0.5);

#[derive(Clone)]
struct Editor {
    text: State<String>,
}

impl Component for Editor {
    fn body(self, _ctx: &Context) -> impl View {
        text_editor("", self.text.binding())
            .font_family(FONT_FAMILY)
            .font_size(13.0)
            .auto_focus()
    }
}

fn main() {
    let args = Args::parse();
    let text = State::new(lines(args.lines));
    // Keep the same assembly as run_window, retaining the runtime only to
    // observe the actual selection at the two ends of the script.
    let runtime = Rc::new(
        Runtime::new()
            .text_engine(Rc::new(CoreTextEngine::new()))
            .image_engine(Rc::new(CoreGraphicsImageEngine::new())),
    );
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
    });
    let app = App::new();
    app.open(
        WindowSpec::titled("arena — editor").size(WINDOW.0, WINDOW.1),
        runtime,
        root,
    );
    app.run();
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
