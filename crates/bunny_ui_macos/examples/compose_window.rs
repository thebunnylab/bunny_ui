//! A composer in a bunny box — the EDITABLE document of the webview.
//!
//! The composer of a mail client edits html in place: the engine's
//! own editing works the body, the toolbar speaks through the
//! allowlist, every change comes back as html, and the network policy
//! holds while editing (`docs/webview.md`). The witness — a server on
//! the loopback that remembers every path it is asked for — says
//! whether anything the draft carries reached the network.
//!
//! What to check by hand:
//! - the composer opens READY: type without clicking, and the footer
//!   shows the body's html change by change;
//! - the toolbar works the selection: bold, italic, a list, a link
//!   (to a fixed url — an app asks in its own dialog), undo;
//! - "quote the original" replaces the body (the app's own write —
//!   the footer does not change until you type); "insert" lands html
//!   at the caret; "read the html" asks and the answer lands below;
//! - paste something: the app OWNS it here — the footer shows what the
//!   clipboard held, and the plain text is what lands, escaped;
//! - "dark"/"light" reloads the document in the other colours;
//! - the witness stays EMPTY: an inserted or pasted image from the web
//!   never loads under the policy.
//!
//! `--drive`: the hand runs the whole sheet itself once the document
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

/// The draft as the composer opens: a greeting, room to type, and the
/// original quoted below a signature.
const DRAFT: &str = "<p>Hi,</p><p><br></p><p>-- <br>Ada</p>\
<blockquote>On Monday, the sender wrote:<br>Could you send the figures?</blockquote>";

/// What a reply becomes when the app quotes the original itself.
const QUOTED: &str =
    "<p>Sure — attached.</p><blockquote>Could you send the figures?</blockquote>";

/// A paste the hand makes in `--drive`: html and text, each carrying a
/// TAB, because the wire escapes tabs and the proof reads them back.
const PASTE: &str = "(function() { \
    var data = new DataTransfer(); \
    data.setData('text/html', '<b>pasted</b>\\tx'); \
    data.setData('text/plain', 'pasted\\ttab'); \
    document.dispatchEvent(new ClipboardEvent('paste', \
        { clipboardData: data, bubbles: true, cancelable: true })); \
    return 1; })()";

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

/// Text as html: the four characters that would otherwise be markup.
fn escaped(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[derive(Clone)]
struct Composer {
    port: u16,
    hits: Arc<Mutex<Vec<String>>>,
    scheme: State<ColorScheme>,
    /// The body's html, as the last change reported it.
    body: State<String>,
    /// What the last paste held, `html|text`.
    pasted: State<String>,
    /// What `get_html` answered.
    read: State<String>,
    fetched: State<String>,
    commits: State<usize>,
    handle: WebviewHandle,
    drive: bool,
    fired: State<bool>,
}

impl Component for Composer {
    fn body(self, _ctx: &Context) -> impl View {
        let (scheme, body, pasted, read) = (self.scheme, self.body, self.pasted, self.read);
        let (fetched, commits) = (self.fetched, self.commits);
        let (port, hits, handle) = (self.port, self.hits, self.handle);

        let chip = |label: &str, on: bool| {
            text(label)
                .padding_length(8.0)
                .background_color(if on { theme::control_hovered() } else { theme::control() })
                .corner_radius(6.0)
        };
        let command = |label: &str, what: EditorCommand| {
            let handle = handle.clone();
            chip(label, false).on_click(move || handle.exec(what))
        };
        let link = {
            let handle = handle.clone();
            chip("link", false).on_click(move || handle.exec_link("https://example.com/"))
        };
        let quote = {
            let handle = handle.clone();
            chip("quote the original", false).on_click(move || handle.set_html(QUOTED))
        };
        let insert = {
            let handle = handle.clone();
            chip("insert", false).on_click(move || handle.insert_html("<i>inserted</i> "))
        };
        let ask = {
            let handle = handle.clone();
            chip("read the html", false).on_click(move || {
                let read = read;
                handle.get_html(move |answer| {
                    read.set(match answer {
                        Ok(html) => html,
                        Err(why) => format!("refused: {why}"),
                    });
                });
            })
        };
        let focus = {
            let handle = handle.clone();
            chip("focus", false).on_click(move || handle.focus())
        };
        let dark = chip("dark", scheme.get() == ColorScheme::Dark)
            .on_click(move || scheme.set(ColorScheme::Dark));
        let light = chip("light", scheme.get() == ColorScheme::Light)
            .on_click(move || scheme.set(ColorScheme::Light));

        let toolbar = hstack!(
            command("bold", EditorCommand::Bold),
            command("italic", EditorCommand::Italic),
            command("list", EditorCommand::UnorderedList),
            command("quote", EditorCommand::Blockquote),
            link,
            command("undo", EditorCommand::Undo),
            spacer(),
            dark,
            light
        )
        .spacing(6.0)
        .alignment(VerticalAlignment::Center);
        let doors = hstack!(quote, insert, ask, focus).spacing(6.0);

        let clip = |text: &str| -> String {
            let mut shown: String = text.chars().take(240).collect();
            if shown.len() < text.len() {
                shown.push('…');
            }
            shown
        };
        let footer = vstack!(
            text("the body, as the last change reported it")
                .foreground_color(theme::fg_secondary()),
            text(clip(&body.get())),
            text("the last paste held").foreground_color(theme::fg_secondary()),
            text(clip(&pasted.get())),
            text("read the html answered").foreground_color(theme::fg_secondary()),
            text(clip(&read.get())),
            text(format!("the witness heard: {}  ·  commits: {}", fetched.get(), commits.get()))
                .foreground_color(theme::fg_secondary())
        )
        .spacing(4.0)
        .alignment(HorizontalAlignment::Leading)
        // the witness is read on a beat
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

        let (driving, fired) = (self.drive, self.fired);
        let armed = Composer {
            port,
            hits: Arc::clone(&hits),
            scheme,
            body,
            pasted,
            read,
            fetched,
            commits,
            handle: handle.clone(),
            drive: false,
            fired,
        };
        let paste_handle = handle.clone();
        let pane = webview_html(DRAFT, "", NetworkPolicy::Deny)
            .editable()
            .focus_on_appear()
            .color_scheme(scheme.get())
            .handle(&handle)
            .on_html_change(move |html| body.set(html.to_string()))
            // the app OWNS the paste: what lands is the text, escaped
            // — the html the clipboard held never meets the document
            .on_paste(move |html, text| {
                println!("[{}] paste: html={html:?} text={text:?}", stamp());
                pasted.set(format!("{html}|{text}"));
                paste_handle.insert_html(escaped(text));
            })
            .on_navigate(move |url| {
                println!("[{}] commit: {url}", stamp());
                commits.update(|count| *count += 1);
                if driving && !fired.get() {
                    fired.set(true);
                    drive_the_hand(armed.clone());
                }
            })
            .on_navigate_failed(|url, why| println!("[{}] refused: {url} — {why}", stamp()));

        vstack!(
            toolbar.padding_length(8.0).background_color(theme::panel()),
            doors.padding_length(8.0),
            pane,
            footer.padding_length(8.0).background_color(theme::panel())
        )
        .spacing(0.0)
        .alignment(HorizontalAlignment::Leading)
    }
}

/// The hand runs the sheet: typing without a click (the keyboard was
/// taken on appearing), the allowlist on the selection, the app's own
/// write and read, an insert, a paste the app owns, and the policy
/// holding through all of it. Each line prints as it is measured, and
/// the process answers 0 or 1.
fn drive_the_hand(composer: Composer) {
    task::spawn(async move {
        let mut passed = true;
        let mut check = |name: &str, held: bool| {
            println!("[{}] {} — {name}", stamp(), if held { "ok" } else { "FAILED" });
            passed &= held;
        };

        settle().await;
        composer.handle.type_text("Dear reader, ");
        beat().await;
        println!("[{}] body: {}", stamp(), composer.body.get());
        check(
            "typing without a click lands: the keyboard was taken on appearing",
            composer.body.get().contains("Dear reader,"),
        );

        composer.handle.exec(EditorCommand::SelectAll);
        composer.handle.exec(EditorCommand::Bold);
        beat().await;
        check("bold from the allowlist wraps the selection", composer.body.get().contains("<b>"));

        composer.handle.exec_link("https://example.com/");
        beat().await;
        check(
            "a link from the app's own dialog lands on the selection",
            composer.body.get().contains("href=\"https://example.com/\""),
        );

        let before = composer.body.get();
        composer.handle.set_html(QUOTED);
        beat().await;
        check("the app's own write is not reported as a change", composer.body.get() == before);
        composer.handle.get_html({
            let read = composer.read;
            move |answer| read.set(answer.unwrap_or_else(|why| format!("refused: {why}")))
        });
        beat().await;
        println!("[{}] read: {}", stamp(), composer.read.get());
        check(
            "read the html answers the body, as html and not as JSON",
            composer.read.get() == QUOTED,
        );

        let pixel =
            format!("<i>inserted</i><img src=\"http://127.0.0.1:{}/pixel.gif\">", composer.port);
        composer.handle.insert_html(pixel);
        settle().await;
        check("an insert is the change it is", composer.body.get().contains("<i>inserted</i>"));
        let heard = composer.hits.lock().expect("the list").clone();
        println!("[{}] the witness heard {heard:?}", stamp());
        check("the policy holds while editing: an inserted image never loads", heard.is_empty());

        composer.handle.eval(PASTE, |_| {});
        beat().await;
        check(
            "a paste the app owns lands in on_paste, tabs intact",
            composer.pasted.get() == "<b>pasted</b>\tx|pasted\ttab",
        );
        check(
            "and what the app inserted is what landed — the text, not the html",
            composer.body.get().contains("pasted\ttab")
                && !composer.body.get().contains("<b>pasted"),
        );

        let commits = composer.commits.get();
        composer.scheme.set(ColorScheme::Dark);
        settle().await;
        check("the other colours reload the document once", composer.commits.get() == commits + 1);

        println!(
            "[{}] {}",
            stamp(),
            if passed { "the sheet holds" } else { "the sheet has a hole" }
        );
        std::process::exit(if passed { 0 } else { 1 });
    })
    .detach();
}

/// Long enough for a load to settle.
async fn settle() {
    task::sleep(std::time::Duration::from_millis(1500)).await;
}

/// Long enough for a change to be reported.
async fn beat() {
    task::sleep(std::time::Duration::from_millis(500)).await;
}

/// Milliseconds since the composer opened.
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
        "a composer",
        Size { width: 960.0, height: 640.0 },
        runtime,
        Composer {
            port,
            hits,
            scheme: State::new(ColorScheme::Light),
            body: State::new(String::from("nothing yet")),
            pasted: State::new(String::from("nothing yet")),
            read: State::new(String::from("nothing yet")),
            fetched: State::new(String::from("nothing")),
            commits: State::new(0),
            handle: WebviewHandle::new(),
            drive: std::env::args().any(|arg| arg == "--drive"),
            fired: State::new(false),
        },
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
