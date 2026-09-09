//! A letter in a bunny box — the DOCUMENT leg of the webview.
//!
//! The reader of a mail client shows html a stranger wrote. This
//! example is that reader: the letter rides from MEMORY under a
//! network policy (`docs/webview.md`), and the witness — a server on
//! the loopback that remembers every path it is asked for — says
//! what the letter managed to fetch. The letter carries a stranger's
//! whole kit: a tracking pixel, a relative one, a stylesheet, a
//! script, a background image, a refresh, a form and two links.
//!
//! What to check by hand:
//! - under "deny" the witness stays EMPTY: nothing the letter carries
//!   reaches the network, and the inline image still shows;
//! - under "remote images" the pixels and the background arrive at
//!   the witness — the stylesheet, the script, the refresh and the
//!   form never do;
//! - click the first link: the footer shows its url and the letter
//!   does not move (the commit count stays at one); the second link
//!   asks for a new window and lands in the same place; the form's
//!   button sends nothing;
//! - switch the policy back and forth: the SAME letter reloads only
//!   when the policy changes.
//!
//! `--drive`: the hand runs the whole sheet itself once the letter
//! commits, prints each answer on stdout, and exits 0 when every line
//! holds — the proof, in the real engine.

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use bunny_ui::prelude::*;
#[cfg(target_os = "macos")]
use bunny_ui_macos::CoreTextEngine;
#[cfg(target_os = "macos")]
use std::rc::Rc;

/// The letter, with `PORT` standing for the witness's. The probe sits
/// at a KNOWN place — the links and the button at 20,20 in 32pt rows
/// with 8pt between — so the hand can aim by number.
const LETTER: &str = r#"<!DOCTYPE html>
<html><head>
<meta http-equiv="refresh" content="1;url=http://127.0.0.1:PORT/refresh">
<link rel="stylesheet" href="http://127.0.0.1:PORT/style.css">
<style>
 body { font: 14px sans-serif; color: #223; margin: 0; padding: 150px 24px 24px; }
 .probe { position: fixed; left: 20px; top: 20px; width: 260px; }
 .probe a, .probe button { display: block; box-sizing: border-box; width: 240px; height: 32px;
   line-height: 32px; margin: 0 0 8px; padding: 0; text-align: center; background: #dde;
   color: #225; text-decoration: none; border: 0; font: inherit; }
 .bg { background: url(http://127.0.0.1:PORT/bg.png); }
</style>
</head><body>
<div class="probe">
  <a id="link" href="https://example.com/offer?ref=letter">a link in the letter</a>
  <a id="blank" href="https://example.com/window" target="_blank">a link to a new window</a>
  <form action="http://127.0.0.1:PORT/form" method="post">
    <button id="send" type="submit">a form to send</button></form>
</div>
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

/// Where the probe's three rows are, in the CSS pixels the hand takes.
const LINK: (f64, f64) = (140.0, 36.0);
const BLANK: (f64, f64) = (140.0, 76.0);
const SEND: (f64, f64) = (140.0, 116.0);

/// One pixel of gif, and the answer that carries it.
const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff\x21\xf9\x04\x01\
\x00\x00\x00\x00\x2c\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00\x3b";
const ANSWER: &str =
    "HTTP/1.1 200 OK\r\nContent-Type: image/gif\r\nContent-Length: 43\r\nConnection: close\r\n\r\n";

/// The witness: a server on the loopback that answers every request
/// with a pixel and REMEMBERS the path. What the letter managed to
/// fetch is what this list holds.
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
    /// The witness's list, as the footer shows it.
    fetched: State<String>,
    linked: State<String>,
    committed: State<String>,
    commits: State<usize>,
    handle: WebviewHandle,
    drive: bool,
    fired: State<bool>,
}

impl Component for Reader {
    fn body(self, _ctx: &Context) -> impl View {
        let (policy, fetched, linked) = (self.policy, self.fetched, self.linked);
        let (committed, commits) = (self.committed, self.commits);
        let (port, hits, handle) = (self.port, self.hits, self.handle);

        let chip = |label: &str, on: bool| {
            text(label)
                .padding_length(8.0)
                .background_color(if on { theme::control_hovered() } else { theme::control() })
                .corner_radius(6.0)
        };
        let deny = chip("deny", policy.get() == NetworkPolicy::Deny)
            .on_click(move || policy.set(NetworkPolicy::Deny));
        let images = chip("remote images", policy.get() == NetworkPolicy::RemoteImages)
            .on_click(move || policy.set(NetworkPolicy::RemoteImages));
        let drive = {
            let reader = Reader {
                port,
                hits: Arc::clone(&hits),
                policy,
                fetched,
                linked,
                committed,
                commits,
                handle: handle.clone(),
                drive: false,
                fired: self.fired,
            };
            chip("drive", false).on_click(move || drive_the_hand(reader.clone()))
        };

        let controls = vstack!(
            text("the policy").foreground_color(theme::fg_secondary()),
            deny,
            images,
            spacer().frame_height(12.0),
            drive
        )
        .spacing(6.0)
        .alignment(HorizontalAlignment::Leading);
        let report = vstack!(
            text("the witness heard").foreground_color(theme::fg_secondary()),
            text(fetched.get()),
            spacer().frame_height(12.0),
            text("the link").foreground_color(theme::fg_secondary()),
            text(linked.get()),
            spacer().frame_height(12.0),
            text(format!("commits: {} at {}", commits.get(), committed.get()))
                .foreground_color(theme::fg_secondary())
        )
        .spacing(6.0)
        .alignment(HorizontalAlignment::Leading);
        let sidebar = vstack!(controls, spacer(), report)
            .spacing(6.0)
            .alignment(HorizontalAlignment::Leading)
            .padding_length(12.0)
        .frame_width(260.0)
        .background_color(theme::panel())
        // the witness is read on a beat: what the letter fetched shows
        // up here without a click
        .task({
            let hits = Arc::clone(&hits);
            move || {
                let hits = Arc::clone(&hits);
                async move {
                    loop {
                        task::sleep(std::time::Duration::from_millis(200)).await;
                        let list = hits.lock().expect("the list").join("\n");
                        let list = if list.is_empty() { String::from("nothing") } else { list };
                        if fetched.get() != list {
                            fetched.set(list);
                        }
                    }
                }
            }
        });

        let (driving, fired) = (self.drive, self.fired);
        let armed = Reader {
            port,
            hits: Arc::clone(&hits),
            policy,
            fetched,
            linked,
            committed,
            commits,
            handle: handle.clone(),
            drive: false,
            fired,
        };
        let pane = webview_html(
            LETTER.replace("PORT", &port.to_string()),
            format!("http://127.0.0.1:{port}/"),
            policy.get(),
        )
        .handle(&handle)
        .on_link(move |url| {
            println!("[{}] link: {url}", stamp());
            linked.set(url.to_string());
        })
        .on_navigate(move |url| {
            println!("[{}] commit: {url}", stamp());
            committed.set(url.to_string());
            commits.update(|count| *count += 1);
            // `--drive`: the hand needs a letter under it, and a
            // commit is when there is one
            if driving && !fired.get() {
                fired.set(true);
                drive_the_hand(armed.clone());
            }
        })
        .on_navigate_failed(|url, why| println!("[{}] refused: {url} — {why}", stamp()));

        hstack!(sidebar, pane).spacing(0.0)
    }
}

/// The hand runs the sheet: under deny the witness must stay empty
/// while the letter tries everything; the links must land in
/// `on_link` and move nothing; the form must send nothing; under
/// remote images exactly the images must arrive. Each line prints as
/// it is measured, and the process answers 0 or 1.
fn drive_the_hand(reader: Reader) {
    task::spawn(async move {
        let mut passed = true;
        let mut check = |name: &str, held: bool| {
            println!("[{}] {} — {name}", stamp(), if held { "ok" } else { "FAILED" });
            passed &= held;
        };
        let heard = |since: usize| -> Vec<String> {
            reader.hits.lock().expect("the list")[since..].to_vec()
        };

        // the letter has had its chance: the refresh fires at one
        // second, the pixels at parse
        settle().await;
        settle().await;
        let under_deny = heard(0);
        println!("[{}] deny: the witness heard {under_deny:?}", stamp());
        check("under deny nothing reaches the network", under_deny.is_empty());
        check("the letter committed once, at its base", reader.commits.get() == 1);

        reader.handle.click(LINK.0, LINK.1);
        beat().await;
        check(
            "a link lands in on_link with its url",
            reader.linked.get() == "https://example.com/offer?ref=letter",
        );
        check("and the letter did not move", reader.commits.get() == 1);

        reader.handle.click(BLANK.0, BLANK.1);
        beat().await;
        check(
            "a target=_blank link lands in on_link too",
            reader.linked.get() == "https://example.com/window",
        );
        check("and opens no window, moves nothing", reader.commits.get() == 1);

        reader.handle.click(SEND.0, SEND.1);
        beat().await;
        check("a form sends nothing", heard(0).is_empty() && reader.commits.get() == 1);

        // the same letter under the other policy: ONE reload
        let before = reader.hits.lock().expect("the list").len();
        reader.policy.set(NetworkPolicy::RemoteImages);
        settle().await;
        settle().await;
        let mut under_images = heard(before);
        under_images.sort();
        println!("[{}] remote images: the witness heard {under_images:?}", stamp());
        check(
            "under remote images exactly the images arrive — pixel, relative pixel, background",
            under_images == ["/bg.png", "/pixel-relative.gif", "/pixel.gif"],
        );
        check("the policy change reloaded the letter once", reader.commits.get() == 2);

        println!(
            "[{}] {}",
            stamp(),
            if passed { "the sheet holds" } else { "the sheet has a hole" }
        );
        std::process::exit(if passed { 0 } else { 1 });
    })
    .detach();
}

/// Long enough for a load to try everything it carries.
async fn settle() {
    task::sleep(std::time::Duration::from_millis(1500)).await;
}

/// Long enough for a click to be heard.
async fn beat() {
    task::sleep(std::time::Duration::from_millis(600)).await;
}

/// Milliseconds since the reader opened.
fn stamp() -> String {
    static START: OnceLock<Instant> = OnceLock::new();
    format!("{:>6}ms", START.get_or_init(Instant::now).elapsed().as_millis())
}

#[cfg(target_os = "macos")]
fn main() {
    let (port, hits) = witness();
    println!("[{}] the witness listens on 127.0.0.1:{port}", stamp());
    let runtime = Runtime::new().text_engine(Rc::new(CoreTextEngine::new()));
    bunny_ui_macos::run_window_with(
        "a letter",
        Size { width: 960.0, height: 640.0 },
        runtime,
        Reader {
            port,
            hits,
            policy: State::new(NetworkPolicy::Deny),
            fetched: State::new(String::from("nothing")),
            linked: State::new(String::from("nothing yet")),
            committed: State::new(String::from("nowhere")),
            commits: State::new(0),
            handle: WebviewHandle::new(),
            drive: std::env::args().any(|arg| arg == "--drive"),
            fired: State::new(false),
        },
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
