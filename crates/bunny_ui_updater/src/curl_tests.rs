//! The real `curl`, against a server on the loopback and against files.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;

use bunny_ui_release::{Transfer, UpdateError};

use crate::{CurlFetch, Fetch};

/// A server on 127.0.0.1 that answers each request by its path, on a
/// thread of its own, until the test ends.
fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            if reader.read_line(&mut request).is_err() {
                continue;
            }
            // read the headers to the empty line
            let mut header = String::new();
            while reader.read_line(&mut header).is_ok_and(|read| read > 2) {
                header.clear();
            }
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
            let (status, body): (&str, Vec<u8>) = match path.as_str() {
                "/up-to-date" => ("204 No Content", Vec::new()),
                "/release" => ("200 OK", br#"{"version":"2.0.0"}"#.to_vec()),
                "/line-breaks" => ("200 OK", b"one\ntwo\n".to_vec()),
                "/big" => ("200 OK", vec![b'z'; 3 << 20]),
                "/moved" => ("302 Found", Vec::new()),
                _ => ("404 Not Found", b"no".to_vec()),
            };
            let mut stream = stream;
            let location = if path == "/moved" { "Location: /release\r\n" } else { "" };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{location}Connection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    format!("http://{address}")
}

fn folder(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("bunny-updater-curl-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn a_get_reads_the_status_and_the_body() {
    let base = serve();
    let curl = CurlFetch::default();
    let reply = curl.get(&format!("{base}/release")).unwrap();
    assert_eq!((reply.status, reply.body.as_slice()), (200, &br#"{"version":"2.0.0"}"#[..]));
    let none = curl.get(&format!("{base}/up-to-date")).unwrap();
    assert_eq!((none.status, none.body.len()), (204, 0));
    let missing = curl.get(&format!("{base}/nothing")).unwrap();
    assert_eq!(missing.status, 404, "a status is an answer, not an error");
    // line breaks in the body stay; only the status line goes
    assert_eq!(curl.get(&format!("{base}/line-breaks")).unwrap().body, b"one\ntwo\n");
    // a redirect is followed
    assert_eq!(curl.get(&format!("{base}/moved")).unwrap().status, 200);
}

#[test]
fn a_download_goes_to_its_file_and_reports_what_arrived() {
    let base = serve();
    let dir = folder("download");
    let to = dir.join("artifact.part");
    let mut seen = Vec::new();
    let status = CurlFetch::default()
        .download(&format!("{base}/big"), &to, &mut |transfer: Transfer| seen.push(transfer.received()))
        .unwrap();
    assert_eq!(status, 200);
    assert_eq!(std::fs::metadata(&to).unwrap().len(), 3 << 20);
    assert_eq!(seen.first(), Some(&0));
    assert_eq!(seen.last(), Some(&(3 << 20)));
    assert!(seen.windows(2).all(|pair| pair[0] <= pair[1]), "it only grows");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_file_url_reads_as_a_success() {
    let dir = folder("file");
    let path = dir.join("latest.json");
    std::fs::write(&path, b"{}").unwrap();
    // a Windows path (`C:\\...`) needs a third slash: `file:///C:/...`
    let path_text = path.display().to_string().replace('\\', "/");
    let url = if path_text.starts_with('/') { format!("file://{path_text}") } else { format!("file:///{path_text}") };
    let reply = CurlFetch::default().get(&url).unwrap();
    assert_eq!((reply.status, reply.body.as_slice()), (200, &b"{}"[..]));
    assert!(matches!(CurlFetch::default().get(&format!("{url}.missing")), Err(UpdateError::Transport(_))));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_url_that_is_not_http_https_or_file_never_reaches_curl() {
    let curl = CurlFetch::at("/no/such/curl");
    for url in ["-o/etc/passwd", "ftp://host/x", "scp://host/x", "", "https://"] {
        let error = curl.get(url).unwrap_err();
        assert!(matches!(&error, UpdateError::Transport(why) if why.contains("is not https")), "{url}: {error}");
    }
}

#[test]
fn a_missing_curl_is_a_transport_error_that_names_it() {
    let error = CurlFetch::at("/no/such/curl").get("https://example.invalid").unwrap_err();
    assert!(matches!(&error, UpdateError::Transport(why) if why.contains("/no/such/curl")), "{error}");
}

#[test]
fn a_connection_refused_is_a_transport_error() {
    // a port that was free a moment ago
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let error = CurlFetch::default().get(&format!("http://127.0.0.1:{port}/x")).unwrap_err();
    assert!(matches!(error, UpdateError::Transport(_)));
}
