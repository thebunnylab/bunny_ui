//! A WebSocket client — the part of RFC 6455 a debugging protocol on the
//! loopback needs: one handshake, masked text frames out, text frames in
//! (whole or in fragments), pings answered, a close honoured.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::formats::base64;
use crate::formats::sha::{Hasher, Sha1};

/// The GUID RFC 6455 appends to the key before it hashes the answer.
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// One open connection.
pub struct Socket {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    /// The next frame's mask: a client masks every frame it sends, and a
    /// server on the loopback only checks that it did.
    mask: u32,
}

fn invalid(why: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.into())
}

/// The key's answer: SHA-1 of the key and the GUID, in base64.
fn accept_for(key: &str) -> String {
    let mut hasher = Hasher::<Sha1>::default();
    hasher.update(key.as_bytes());
    hasher.update(GUID.as_bytes());
    base64::encode(&hasher.finish())
}

impl Socket {
    /// Opens `ws://host:port/path` and completes the handshake.
    ///
    /// # Errors
    ///
    /// An address that is not `ws://`, a refused connection, or a server
    /// that answers the handshake with anything but its key's answer.
    pub fn connect(url: &str) -> io::Result<Self> {
        let rest = url.strip_prefix("ws://").ok_or_else(|| invalid(format!("not a ws:// address: {url}")))?;
        let (host, path) =
            rest.split_once('/').map_or_else(|| (rest, String::from("/")), |(host, path)| (host, format!("/{path}")));
        let stream = TcpStream::connect(host)?;
        stream.set_nodelay(true)?;
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9E37_79B9, |since| since.subsec_nanos() | 1);
        let key = base64::encode(&u128::from(seed).wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
        let mut writer = stream.try_clone()?;
        write!(
            writer,
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )?;
        let mut reader = BufReader::new(stream);
        let mut status = String::new();
        reader.read_line(&mut status)?;
        if !status.starts_with("HTTP/1.1 101") {
            return Err(invalid(format!("the handshake was answered `{}`", status.trim())));
        }
        let mut accept = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.trim().eq_ignore_ascii_case("sec-websocket-accept")
            {
                accept = Some(value.trim().to_owned());
            }
        }
        if accept.as_deref() != Some(accept_for(&key).as_str()) {
            return Err(invalid("the handshake's answer does not match its key"));
        }
        Ok(Self { reader, writer, mask: seed })
    }

    /// How long a read waits before it gives up; `None` waits forever.
    ///
    /// # Errors
    ///
    /// A zero duration, which the platform refuses.
    pub fn set_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.writer.set_read_timeout(timeout)
    }

    /// Sends one text message as a single masked frame.
    ///
    /// # Errors
    ///
    /// The connection's own.
    pub fn send(&mut self, text: &str) -> io::Result<()> {
        self.frame(0x1, text.as_bytes())
    }

    fn frame(&mut self, opcode: u8, payload: &[u8]) -> io::Result<()> {
        let mut out = Vec::with_capacity(payload.len() + 14);
        out.push(0x80 | opcode);
        let len = payload.len();
        if let Ok(short) = u8::try_from(len)
            && short <= 125
        {
            out.push(0x80 | short);
        } else if let Ok(medium) = u16::try_from(len) {
            out.push(0x80 | 0x7E);
            out.extend_from_slice(&medium.to_be_bytes());
        } else {
            out.push(0x80 | 0x7F);
            out.extend_from_slice(&u64::try_from(len).unwrap_or(u64::MAX).to_be_bytes());
        }
        self.mask ^= self.mask << 13;
        self.mask ^= self.mask >> 17;
        self.mask ^= self.mask << 5;
        let mask = self.mask.to_be_bytes();
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(index, byte)| byte ^ mask[index % 4]));
        self.writer.write_all(&out)?;
        self.writer.flush()
    }

    /// The next text message, its fragments joined; `None` once the peer
    /// closed. Pings are answered on the way.
    ///
    /// # Errors
    ///
    /// A read that timed out (`WouldBlock` or `TimedOut`), a broken
    /// connection, or a text frame that is not UTF-8.
    pub fn receive(&mut self) -> io::Result<Option<String>> {
        let mut message = Vec::new();
        loop {
            let mut head = [0u8; 2];
            self.reader.read_exact(&mut head)?;
            let last = head[0] & 0x80 != 0;
            let opcode = head[0] & 0x0F;
            let masked = head[1] & 0x80 != 0;
            let len = match head[1] & 0x7F {
                126 => {
                    let mut bytes = [0u8; 2];
                    self.reader.read_exact(&mut bytes)?;
                    u64::from(u16::from_be_bytes(bytes))
                }
                127 => {
                    let mut bytes = [0u8; 8];
                    self.reader.read_exact(&mut bytes)?;
                    u64::from_be_bytes(bytes)
                }
                len => u64::from(len),
            };
            let mut mask = [0u8; 4];
            if masked {
                self.reader.read_exact(&mut mask)?;
            }
            let len = usize::try_from(len).map_err(|_| invalid("a frame longer than this machine holds"))?;
            let mut payload = vec![0u8; len];
            self.reader.read_exact(&mut payload)?;
            if masked {
                for (index, byte) in payload.iter_mut().enumerate() {
                    *byte ^= mask[index % 4];
                }
            }
            match opcode {
                0x0..=0x2 => {
                    message.extend_from_slice(&payload);
                    if last {
                        return String::from_utf8(message).map(Some).map_err(|_| invalid("a text frame that is not UTF-8"));
                    }
                }
                0x8 => {
                    let _ = self.frame(0x8, &payload);
                    return Ok(None);
                }
                0x9 => self.frame(0xA, &payload)?,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    /// The RFC's own example key and answer.
    #[test]
    fn the_handshake_answer_is_the_rfcs() {
        assert_eq!(accept_for("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    /// A server that answers the handshake, unmasks what it hears, and
    /// sends back a ping, a reply in two fragments, a long reply, and a
    /// close.
    #[test]
    fn a_message_goes_out_masked_and_comes_back_whole() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let mut key = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                if let Some(value) = line.strip_prefix("Sec-WebSocket-Key: ") {
                    key = value.trim().to_owned();
                }
            }
            write!(writer, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", accept_for(&key)).unwrap();
            let mut head = [0u8; 2];
            reader.read_exact(&mut head).unwrap();
            assert_eq!(head[0], 0x81);
            assert!(head[1] & 0x80 != 0, "a client masks");
            let len = usize::from(head[1] & 0x7F);
            let mut mask = [0u8; 4];
            reader.read_exact(&mut mask).unwrap();
            let mut payload = vec![0u8; len];
            reader.read_exact(&mut payload).unwrap();
            let unmasked: Vec<u8> = payload.iter().enumerate().map(|(index, byte)| byte ^ mask[index % 4]).collect();
            assert_eq!(unmasked, b"hello");
            writer.write_all(&[0x89, 2, b'h', b'i']).unwrap();
            writer.write_all(&[0x01, 3, b'w', b'o', b'r']).unwrap();
            writer.write_all(&[0x80, 2, b'l', b'd']).unwrap();
            let long = "x".repeat(70_000);
            writer.write_all(&[0x81, 127]).unwrap();
            writer.write_all(&(long.len() as u64).to_be_bytes()).unwrap();
            writer.write_all(long.as_bytes()).unwrap();
            writer.write_all(&[0x88, 0]).unwrap();
            let mut answer = [0u8; 2];
            reader.read_exact(&mut answer).unwrap();
            assert_eq!(answer[0], 0x8A, "the ping is answered");
        });
        let mut socket = Socket::connect(&format!("ws://{address}/devtools/page/1")).unwrap();
        socket.send("hello").unwrap();
        assert_eq!(socket.receive().unwrap().as_deref(), Some("world"));
        assert_eq!(socket.receive().unwrap().map(|text| text.len()), Some(70_000));
        assert_eq!(socket.receive().unwrap(), None);
        server.join().unwrap();
    }
}
