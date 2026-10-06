//! A text editor over 400 or 30 000 lines, the keyboard in it: typing, or
//! the wheel over the text.
use arena::{Args, Step, WINDOW, lines, scripted};
use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

const OVER: (f64, f64) = (WINDOW.0 * 0.5, WINDOW.1 * 0.5);

#[derive(Clone)]
struct Editor {
    text: State<String>,
}

impl Component for Editor {
    fn body(self, _ctx: &Context) -> impl View {
        text_editor("", self.text.binding()).font_size(13.0).auto_focus()
    }
}

fn main() {
    let args = Args::parse();
    let text = State::new(lines(args.lines));
    bunny_ui_macos::run_window(
        "arena — editor",
        Size { width: WINDOW.0, height: WINDOW.1 },
        scripted(Editor { text }, args, OVER, |_: Step| {}),
    );
}
