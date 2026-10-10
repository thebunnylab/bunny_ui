//! The page's server during `bunny run -d web`: a folder over HTTP, with
//! the right type for `.wasm` (the browser refuses to stream it
//! otherwise), nothing cached, and two doors of its own — a long poll
//! that tells the page a new build landed, and a mailbox where the page
//! drops its console errors for the terminal.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

/// The script every page `bunny run` serves loads first.
pub const DEV_JS: &str = include_str!("serve/dev.js");

/// The build the page is on, and a wake-up for the pages waiting on it.
#[derive(Default)]
struct Build {
    id: Mutex<u64>,
    landed: Condvar,
}

/// A running server.
pub struct Server {
    pub address: SocketAddr,
    build: Arc<Build>,
}

impl Server {
    /// Serves `root` on `host`, at `port` or the next free one.
    pub fn start(root: PathBuf, host: &str, port: u16) -> std::io::Result<Server> {
        let listener = (port..port.saturating_add(20))
            .find_map(|port| TcpListener::bind((host, port)).ok())
            .map_or_else(|| TcpListener::bind((host, 0)), Ok)?;
        let address = listener.local_addr()?;
        let build = Arc::new(Build::default());
        let shared = Arc::clone(&build);
        thread::spawn(move || {
            for stream in listener.incoming().map_while(Result::ok) {
                let root = root.clone();
                let build = Arc::clone(&shared);
                thread::spawn(move || {
                    let _ = handle(stream, &root, &build);
                });
            }
        });
        Ok(Server { address, build })
    }

    /// A new build landed: every page waiting reloads.
    pub fn reload(&self) {
        let mut id = self.build.id.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *id += 1;
        self.build.landed.notify_all();
    }

    /// The address a person types, `localhost` for the loopback.
    pub fn url(&self) -> String {
        if self.address.ip().is_loopback() {
            format!("http://localhost:{}/", self.address.port())
        } else {
            format!("http://{}/", self.address)
        }
    }
}

fn handle(stream: TcpStream, root: &Path, build: &Build) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut stream = stream;
    match (method, path) {
        ("GET", "/__bunny/dev.js") => respond(&mut stream, "200 OK", "text/javascript", DEV_JS.as_bytes(), true),
        ("GET", "/__bunny/build") => {
            let since = query.split('&').find_map(|pair| pair.strip_prefix("since=")).and_then(|v| v.parse::<u64>().ok());
            let mut id = build.id.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(since) = since {
                // wait for a new build, but answer within half a minute —
                // what proxies and browsers keep a request open for
                let (next, _) = build
                    .landed
                    .wait_timeout_while(id, Duration::from_secs(25), |current| *current == since)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                id = next;
            }
            let body = id.to_string();
            drop(id);
            respond(&mut stream, "200 OK", "text/plain", body.as_bytes(), true)
        }
        ("POST", "/__bunny/log") => {
            let mut body = vec![0u8; length.min(64 * 1024)];
            reader.read_exact(&mut body)?;
            let text = String::from_utf8_lossy(&body);
            let (level, message) = text.split_once(' ').unwrap_or(("log", &text));
            eprintln!("{} {message}", if level == "error" { "[page error]" } else { "[page]" });
            respond(&mut stream, "204 No Content", "text/plain", b"", false)
        }
        ("GET" | "HEAD", _) => match file(root, path) {
            Some(file) => {
                let bytes = fs::read(&file).unwrap_or_default();
                respond(&mut stream, "200 OK", mime(&file), &bytes, method == "GET")
            }
            None => respond(&mut stream, "404 Not Found", "text/plain", b"not found", method == "GET"),
        },
        _ => respond(&mut stream, "405 Method Not Allowed", "text/plain", b"", false),
    }
}

fn respond(stream: &mut TcpStream, status: &str, kind: &str, body: &[u8], with_body: bool) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    if with_body {
        stream.write_all(body)?;
    }
    stream.flush()
}

/// The file a URL path names under `root` — never one outside it.
fn file(root: &Path, path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(path)?;
    let relative = Path::new(decoded.trim_start_matches('/'));
    if relative.components().any(|part| !matches!(part, Component::Normal(_))) {
        return None;
    }
    let mut full = root.join(relative);
    if full.is_dir() {
        full = full.join("index.html");
    }
    full.is_file().then_some(full)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript",
        "wasm" => "application/wasm",
        "css" => "text/css",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn get(address: SocketAddr, target: &str) -> String {
        let mut stream = TcpStream::connect(address).unwrap();
        write!(stream, "GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
    }

    fn served(name: &str) -> (Server, PathBuf) {
        let root = std::env::temp_dir().join(format!("bunny-serve-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::write(root.join("index.html"), "<h1>app</h1>").unwrap();
        fs::write(root.join("app.wasm"), b"\0asm").unwrap();
        fs::write(root.join("assets/a b.css"), "body{}").unwrap();
        (Server::start(root.clone(), "127.0.0.1", 0).unwrap(), root)
    }

    #[test]
    fn files_go_out_with_their_type_and_nothing_else_does() {
        let (server, root) = served("files");
        let page = get(server.address, "/");
        assert!(page.starts_with("HTTP/1.1 200") && page.contains("text/html") && page.ends_with("<h1>app</h1>"));
        assert!(get(server.address, "/app.wasm").contains("Content-Type: application/wasm"));
        assert!(get(server.address, "/assets/a%20b.css").contains("text/css"));
        assert!(get(server.address, "/../etc/passwd").starts_with("HTTP/1.1 404"));
        assert!(get(server.address, "/%2e%2e/secret").starts_with("HTTP/1.1 404"));
        assert!(get(server.address, "/missing.js").starts_with("HTTP/1.1 404"));
        assert!(get(server.address, "/__bunny/dev.js").contains("text/javascript"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_waiting_page_hears_the_next_build() {
        let (server, root) = served("poll");
        let now = get(server.address, "/__bunny/build");
        assert!(now.ends_with("\r\n\r\n0"), "{now}");
        let address = server.address;
        let waiting = thread::spawn(move || {
            let started = Instant::now();
            (get(address, "/__bunny/build?since=0"), started.elapsed())
        });
        thread::sleep(Duration::from_millis(200));
        server.reload();
        let (answer, waited) = waiting.join().unwrap();
        assert!(answer.ends_with("\r\n\r\n1"), "{answer}");
        assert!(waited < Duration::from_secs(5));
        let _ = fs::remove_dir_all(root);
    }
}
