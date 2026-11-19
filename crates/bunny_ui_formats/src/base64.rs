//! Base64 (RFC 4648, the standard alphabet, with padding): the text
//! form of a key and of a signature.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `bytes` as base64 text, padded with `=`.
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let value = u32::from(group[0]) << 16
            | u32::from(*group.get(1).unwrap_or(&0)) << 8
            | u32::from(*group.get(2).unwrap_or(&0));
        for place in 0..4 {
            if place <= group.len() {
                out.push(char::from(ALPHABET[(value >> (18 - 6 * place) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The bytes of base64 `text`, or `None` when `text` is not base64.
///
/// The text must be padded, and the bits under the padding must be
/// zero: each byte string has exactly one text that decodes to it.
#[must_use]
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let text = text.as_bytes();
    if text.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let groups = text.len() / 4;
    for (index, group) in text.chunks(4).enumerate() {
        let pad = group.iter().rev().take_while(|&&byte| byte == b'=').count();
        if pad > 2 || (pad > 0 && index + 1 != groups) {
            return None;
        }
        let mut value = 0u32;
        for &byte in &group[..4 - pad] {
            value = value << 6 | u32::from(sextet(byte)?);
        }
        value <<= 6 * pad as u32;
        let bytes = value.to_be_bytes();
        let keep = 3 - pad;
        // the bits under the padding are zero in the one true form
        if bytes[1 + keep..].iter().any(|&byte| byte != 0) {
            return None;
        }
        out.extend_from_slice(&bytes[1..1 + keep]);
    }
    Some(out)
}

fn sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rfc_vectors() {
        let pairs = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, coded) in pairs {
            assert_eq!(encode(plain.as_bytes()), coded);
            assert_eq!(decode(coded).as_deref(), Some(plain.as_bytes()));
        }
    }

    #[test]
    fn every_byte_goes_there_and_back() {
        let bytes: Vec<u8> = (0..=255).collect();
        for len in 0..bytes.len() {
            assert_eq!(decode(&encode(&bytes[..len])).as_deref(), Some(&bytes[..len]));
        }
    }

    #[test]
    fn text_that_is_not_base64_is_refused() {
        assert_eq!(decode("Zg"), None, "no padding");
        assert_eq!(decode("Zg=a"), None, "padding in the middle of a group");
        assert_eq!(decode("Zg==Zg=="), None, "padding before the end");
        assert_eq!(decode("Zh=="), None, "bits under the padding");
        assert_eq!(decode("Z!=="), None, "a character outside the alphabet");
        assert_eq!(decode("Z==="), None, "three padding characters");
    }
}
