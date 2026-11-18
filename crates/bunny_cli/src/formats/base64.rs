//! Base64 (RFC 4648, the standard alphabet with padding) — what a
//! WebSocket handshake keys with and what a debugging protocol sends
//! a screenshot in.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `bytes` as base64 text.
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let word = (u32::from(group[0]) << 16)
            | (u32::from(group.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(group.get(2).copied().unwrap_or(0));
        for (index, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if index <= group.len() {
                out.push(char::from(ALPHABET[(word >> shift & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The bytes base64 `text` holds; `None` for a character outside the
/// alphabet or a length no encoder writes.
#[must_use]
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let digits = text.trim_end_matches('=').as_bytes();
    if !text.len().is_multiple_of(4) || text.len() - digits.len() > 2 {
        return None;
    }
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    let mut word = 0u32;
    for (index, &digit) in digits.iter().enumerate() {
        let value = match digit {
            b'A'..=b'Z' => digit - b'A',
            b'a'..=b'z' => digit - b'a' + 26,
            b'0'..=b'9' => digit - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        word = word << 6 | u32::from(value);
        if index % 4 == 3 {
            out.extend_from_slice(&word.to_be_bytes()[1..]);
            word = 0;
        }
    }
    match digits.len() % 4 {
        2 => out.push((word >> 4).to_be_bytes()[3]),
        3 => out.extend_from_slice(&(word >> 2).to_be_bytes()[2..]),
        _ => {}
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The RFC's own vectors, both ways.
    #[test]
    fn the_rfc_vectors_round_trip() {
        for (plain, coded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode(plain.as_bytes()), coded);
            assert_eq!(decode(coded).as_deref(), Some(plain.as_bytes()));
        }
        assert_eq!(decode("Zm9v!A=="), None);
        assert_eq!(decode("Zm9"), None);
    }
}
