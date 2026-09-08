//! A letter in a bunny box, on the phone — the DOCUMENT leg of the
//! webview under the finger.
//!
//! The reader of a mail client shows html a stranger wrote. This
//! example is that reader: the letter rides from MEMORY under a
//! network policy (`docs/webview.md`), and the witness — a server on
//! the loopback that remembers every path it is asked for — says what
//! the letter managed to fetch. The letter carries a stranger's whole
//! kit: a tracking pixel, a relative one, a stylesheet, a script, a
//! background image, a refresh, a form and two links.
//!
//! What to check by hand:
//! - under "deny" the witness stays EMPTY: nothing the letter carries
//!   reaches the network, and the inline image still shows;
//! - under "remote images" the pixels and the background arrive at
//!   the witness — the stylesheet, the script, the refresh and the
//!   form never do;
//! - tap the first link: the footer shows its url and the letter does
//!   not move (the commit count stays at one); the second link asks
//!   for a new window and lands in the same place; the form's button
//!   sends nothing;
//! - the chips above the letter paint OVER the page — the sandwich —
//!   and a tap on a chip is the chip's, a tap beside it the page's.
//!
//! ```sh
//! crates/bunny_ui_ios/simulator/run-sim.sh letter_window_ios
//! ```

#![cfg_attr(not(target_os = "ios"), allow(dead_code, unused_imports))]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
#[cfg(target_os = "ios")]
use bunny_ui_ios::CoreTextEngine;
#[cfg(target_os = "ios")]
use std::rc::Rc;

/// The letter, with `PORT` standing for the witness's.
const LETTER: &str = r#"<!DOCTYPE html>
<html><head>
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="refresh" content="1;url=http://127.0.0.1:PORT/refresh">
<link rel="stylesheet" href="http://127.0.0.1:PORT/style.css">
<style>
 body { font: 16px -apple-system, sans-serif; color: #223; margin: 0; padding: 16px; }
 a, button { display: block; box-sizing: border-box; width: 100%; height: 44px;
   line-height: 44px; margin: 0 0 8px; padding: 0; text-align: center; background: #dde;
   color: #225; text-decoration: none; border: 0; font: inherit; border-radius: 8px; }
 .bg { background: url(http://127.0.0.1:PORT/bg.png); }
</style>
</head><body>
<a id="link" href="https://example.com/offer?ref=letter">a link in the letter</a>
<a id="blank" href="https://example.com/window" target="_blank">a link to a new window</a>
<form action="http://127.0.0.1:PORT/form" method="post">
  <button id="send" type="submit">a form to send</button></form>
<h1>Dear reader,</h1>
<p>This letter carries a tracking pixel, a relative one, a stylesheet, a script,
a <span class="bg">background</span>, a refresh, a form and two links.</p>
<p><img src="http://127.0.0.1:PORT/pixel.gif" width="1" height="1" alt="">
<img src="pixel-relative.gif" width="1" height="1" alt=""></p>
<p>An inline image, which no policy touches:
<img src="data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAICRAEAOw=="
     width="40" height="40" alt="inline" style="background:#8c8;vertical-align:middle"></p>
<script>fetch('http://127.0.0.1:PORT/script');</script>
</body></html>"#;

/// One pixel of gif, and the answer that carries it.
const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff\x21\xf9\x04\x01\
\x00\x00\x00\x00\x2c\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00\x3b";
const ANSWER: &str =
    "HTTP/1.1 200 OK\r\nContent-Type: image/gif\r\nContent-Length: 43\r\nConnection: close\r\n\r\n";

/// The witness: a server on the loopback that answers every request
/// with a pixel and REMEMBERS the path.
fn witness() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("the port").port();
    let hits = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                continue;
            };
            let mut buffer = [0u8; 4096];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]);
            let path = request.split_whitespace().nth(1).unwrap_or("?").to_string();
            record.lock().expect("the list").push(path);
            let _ = stream.write_all(ANSWER.as_bytes());
            let _ = stream.write_all(GIF);
        }
    });
    (port, hits)
}

#[derive(Clone)]
struct Reader {
    port: u16,
    hits: Arc<Mutex<Vec<String>>>,
    policy: State<NetworkPolicy>,
    fetched: State<String>,
    linked: State<String>,
    commits: State<usize>,
    handle: WebviewHandle,
}

impl Component for Reader {
    fn body(self, _ctx: &Context) -> impl View {
        let (policy, fetched, linked, commits) = (self.policy, self.fetched, self.linked, self.commits);
        let (port, hits, handle) = (self.port, self.hits, self.handle);

        let chip = |label: &str, on: bool| {
            text(label)
                .padding_length(10.0)
                .background_color(if on { theme::control_hovered() } else { theme::control() })
                .corner_radius(8.0)
        };
        let deny = chip("deny", policy.get() == NetworkPolicy::Deny)
            .on_click(move || policy.set(NetworkPolicy::Deny));
        let images = chip("remote images", policy.get() == NetworkPolicy::RemoteImages)
            .on_click(move || policy.set(NetworkPolicy::RemoteImages));
        let controls = hstack!(deny, images, spacer()).spacing(8.0);

        let report = vstack!(
            text("the witness heard").foreground_color(theme::fg_secondary()),
            text(fetched.get()),
            text("the link").foreground_color(theme::fg_secondary()),
            text(linked.get()),
            text(format!("commits: {}", commits.get())).foreground_color(theme::fg_secondary()),
        )
        .spacing(4.0)
        .alignment(HorizontalAlignment::Leading)
        // the witness is read on a beat: what the letter fetched shows
        // up here without a tap
        .task({
            let hits = Arc::clone(&hits);
            move || {
                let hits = Arc::clone(&hits);
                async move {
                    loop {
                        task::sleep(std::time::Duration::from_millis(200)).await;
                        let list = hits.lock().expect("the list").join(", ");
                        let list = if list.is_empty() { String::from("nothing") } else { list };
                        if fetched.get() != list {
                            fetched.set(list);
                        }
                    }
                }
            }
        });

        let pane = webview_html(
            LETTER.replace("PORT", &port.to_string()),
            format!("http://127.0.0.1:{port}/"),
            policy.get(),
        )
        .handle(&handle)
        .on_link(move |url| linked.set(url.to_string()))
        .on_navigate(move |_url| commits.update(|count| *count += 1))
        .on_navigate_failed(|url, why| eprintln!("refused: {url} — {why}"));

        // the chips ride ABOVE the page: the sandwich lifts them onto a
        // segment over the engine's view
        vstack!(
            zstack!(pane, vstack!(controls.padding_length(8.0), spacer())),
            report.padding_length(12.0),
        )
        .spacing(0.0)
    }
}

#[cfg(target_os = "ios")]
fn main() {
    let (port, hits) = witness();
    eprintln!("the witness listens on 127.0.0.1:{port}");
    let runtime = Runtime::new().text_engine(Rc::new(CoreTextEngine::new()));
    bunny_ui_ios::run_window_with(
        "a letter",
        Size { width: 390.0, height: 844.0 },
        runtime,
        Reader {
            port,
            hits,
            policy: State::new(NetworkPolicy::Deny),
            fetched: State::new(String::from("nothing")),
            linked: State::new(String::from("nothing yet")),
            commits: State::new(0),
            handle: WebviewHandle::new(),
        },
    );
}

#[cfg(not(target_os = "ios"))]
fn main() {} // this example is iOS-only
