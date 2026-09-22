//! The codecs of the house: PNG and JPEG decoded in safe Rust, for a
//! platform with no codec of its own. The other shells ask their
//! platform (ImageIO, WIC, `AImageDecoder`, the browser); the Linux
//! shell asks here. Behind the `codec` feature — off by default, the
//! shipped binary carries no decoder it does not use.
//!
//! One contract for both: bytes in, straight RGBA out at the picture's
//! own size, `None` (or a refusal by name) on anything malformed or
//! outside the scope. Nothing here allocates more than the picture
//! asks for, and a picture beyond [`MAX_PIXELS`] is refused before
//! the first row.

pub mod inflate;
pub mod jpeg;
pub mod png;

/// A decoded picture: straight (not premultiplied) RGBA, row-major,
/// `width × height × 4` bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The most pixels a picture may declare: 64 megapixels, a quarter
/// gigabyte of RGBA — beyond it, the header is a refusal, not a
/// request for memory.
pub const MAX_PIXELS: u64 = 64 << 20;

/// Which codec the bytes ask for, by their signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Png,
    Jpeg,
}

/// The codec the bytes belong to, or `None` for bytes neither knows.
pub fn kind(bytes: &[u8]) -> Option<Kind> {
    const PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.starts_with(&PNG) {
        Some(Kind::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        Some(Kind::Jpeg)
    } else {
        None
    }
}

/// The picture's size out of its header alone — the cheap answer an
/// `intrinsic` wants before anyone decodes.
pub fn header(bytes: &[u8]) -> Option<(u32, u32)> {
    match kind(bytes)? {
        Kind::Png => png::header(bytes),
        Kind::Jpeg => jpeg::header(bytes),
    }
}

/// The whole picture. A JPEG the decoder refuses by name (arithmetic,
/// lossless, CMYK …) answers `None` here; [`jpeg::decode`] says why.
pub fn decode(bytes: &[u8]) -> Option<Image> {
    match kind(bytes)? {
        Kind::Png => png::decode(bytes),
        Kind::Jpeg => jpeg::decode(bytes).ok(),
    }
}

/// True when a declared size fits the cap.
pub(crate) fn fits(width: u32, height: u32) -> bool {
    width > 0 && height > 0 && (width as u64) * (height as u64) <= MAX_PIXELS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signature_names_the_codec() {
        assert_eq!(kind(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]), Some(Kind::Png));
        assert_eq!(kind(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Kind::Jpeg));
        assert_eq!(kind(b"bnyr"), None);
        assert_eq!(kind(&[]), None);
    }

    #[test]
    fn the_cap_refuses_before_the_first_row() {
        assert!(fits(1, 1));
        assert!(fits(8192, 8192));
        assert!(!fits(0, 10));
        assert!(!fits(u32::MAX, u32::MAX));
    }
}
