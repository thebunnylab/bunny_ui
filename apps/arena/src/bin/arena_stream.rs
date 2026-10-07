//! A chat that grows by one message every 33 ms: what a streaming answer
//! costs the window per append.
use arena::{Args, FONT_FAMILY, Step, WINDOW, scripted};
use bunny_ui::layout::{Color, Size};
use bunny_ui::prelude::*;

const OVER: (f64, f64) = (WINDOW.0 * 0.5, WINDOW.1 * 0.5);

#[derive(Clone)]
struct Chat {
    messages: State<std::rc::Rc<Vec<String>>>,
}

impl Component for Chat {
    fn body(self, _ctx: &Context) -> impl View {
        let messages = self.messages.get();
        let count = messages.len();
        virtual_list(
            count,
            |i| format!("m-{i}"),
            move |i| {
                let line = messages[i].clone();
                text!(line)
                    .font_size(13.0)
                    .padding_length(8.0)
                    .frame_height(28.0)
                    .background_color(if i % 2 == 0 {
                        Color::hex(0x1C1C21)
                    } else {
                        Color::hex(0x17171C)
                    })
            },
        )
        .row_height(28.0)
        .font_family(FONT_FAMILY)
    }
}

fn main() {
    let args = Args::parse();
    let messages: State<std::rc::Rc<Vec<String>>> = State::new(std::rc::Rc::new(Vec::new()));
    let chat = Chat { messages };
    let append = move |step: Step| {
        if matches!(step, Step::Append) {
            messages.update(|all| {
                let n = all.len();
                std::rc::Rc::make_mut(all).push(format!(
                    "message {n}: a token lands, and the list grows by one line"
                ));
            });
        }
    };
    bunny_ui_macos::run_window(
        "arena — stream",
        Size {
            width: WINDOW.0,
            height: WINDOW.1,
        },
        scripted(chat, args, OVER, append),
    );
}
