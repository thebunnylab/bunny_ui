//! Native protocol proof: scrubbed HTML engines cross identities without content,
//! bridge callbacks or history. A second live pane remains independent.
#[cfg(target_os = "macos")]
mod native {
    use bunny_ui::host::{NetworkPolicy, WebviewHandle, webview_html};
    use bunny_ui::prelude::*;
    use bunny_ui_macos::{App, HtmlPreparation, WindowSpec};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    #[derive(Clone)]
    struct Probe {
        app: App,
        reserve: Rc<HtmlPreparation>,
        phase: State<u8>,
        answer: State<String>,
        other: WebviewHandle,
        handle: WebviewHandle,
    }
    impl Component for Probe {
        fn body(self) -> impl View {
            let phase = self.phase.get();
            let answer = self.answer;
            let body = match phase {
            1 => Some(("first", "private-A")),
            3 => Some(("second", "public-B")),
            _ => None,
        }.map(|(id, value)| webview_html(format!("<html><body>{value}</body></html>"), "", NetworkPolicy::Deny)
            .handle(&self.handle)
            .user_script("(() => { const previous = window.previous || 'empty'; document.addEventListener('DOMContentLoaded', () => { window.previous = document.body.innerText; window.bunny.post(previous + ':' + document.body.innerText); }); })();")
            .on_message(move |message| answer.set(message.to_owned()))
            .id(id).frame(320.0, 180.0));
            let probe = self.clone();
            hstack!(body, webview_html("<html><body>independent-C</body></html>", "", NetworkPolicy::Deny).handle(&self.other).id("other").frame(320.0, 180.0))
            .task(move || {
                let probe = probe.clone();
                async move {
                    let deadline = Instant::now() + Duration::from_secs(15);
                    loop {
                        assert!(Instant::now() < deadline, "HTML reuse did not finish: phase={} recycled={} available={} answer={}", probe.phase.get(), probe.reserve.recycled(), probe.reserve.available(), probe.answer.get());
                        match probe.phase.get() {
                            0 if probe.reserve.available() == 2 => probe.phase.set(1),
                            1 if probe.answer.get().ends_with(":private-A") => {
                                probe.answer.set(String::new());
                                probe.phase.set(2);
                            }
                            2 if probe.reserve.recycled() >= 1 => probe.phase.set(3),
                            3 if probe.answer.get()== "empty:public-B" => {
                                let app = probe.app.clone();
                                probe.other.eval("document.body.innerText", move |result| {
                                    assert_eq!(result.unwrap(), "\"independent-C\"");
                                    println!("native reuse: scrubbed engine, new document and independent live pane passed");
                                    app.quit();
                                });
                                break;
                            }
                            _ => {}
                        }
                        task::sleep(Duration::from_millis(10)).await;
                    }
                }
            })
        }
    }
    pub fn run() {
        let app = App::new();
        let runtime = Rc::new(app.runtime());
        app.open(
            WindowSpec::titled("HTML reuse protocol").size(700.0, 260.0),
            runtime,
            Probe {
                app: app.clone(),
                reserve: Rc::new(HtmlPreparation::request()),
                phase: State::new(0),
                answer: State::new(String::new()),
                other: WebviewHandle::new(),
                handle: WebviewHandle::new(),
            },
        );
        app.run();
    }
}
#[cfg(target_os = "macos")]
fn main() {
    native::run();
}
#[cfg(not(target_os = "macos"))]
fn main() {}
