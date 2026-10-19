//! Default extended grapheme boundaries keep editing from tearing a visible character apart.
//!
//! Unicode 17.0.0, UAX #29 revision 47, rules GB3–GB999. Properties are generated
//! from pinned Unicode data; the official corpus exercises the public edit API.
//! Random access inspects the adjacent cluster, with additional lookbehind only
//! for Indic linking, emoji ZWJ and regional-indicator parity. Ordinary ASCII
//! tail editing never scans the document prefix. These are logical boundaries,
//! not visual BiDi movement, word segmentation or shaping.

#[path = "grapheme_data.rs"]
mod data;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Gcb {
    Other,
    Cr,
    Lf,
    Control,
    Extend,
    Zwj,
    RegionalIndicator,
    Prepend,
    SpacingMark,
    L,
    V,
    T,
    Lv,
    Lvt,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Incb {
    None,
    Consonant,
    Extend,
    Linker,
}

fn property<T: Copy>(table: &[(u32, u32, T)], ch: char, default: T) -> T {
    let cp = u32::from(ch);
    let index = table.partition_point(|&(_, end, _)| end < cp);
    table
        .get(index)
        .filter(|&&(start, _, _)| start <= cp)
        .map_or(default, |&(_, _, value)| value)
}

fn gcb(ch: char) -> Gcb {
    match ch {
        '\r' => Gcb::Cr,
        '\n' => Gcb::Lf,
        '\0'..='\u{1f}' | '\u{7f}' => Gcb::Control,
        ' '..='~' => Gcb::Other,
        '\u{ac00}'..='\u{d7a3}' => {
            if (u32::from(ch) - 0xac00).is_multiple_of(28) {
                Gcb::Lv
            } else {
                Gcb::Lvt
            }
        }
        _ => property(data::GCB, ch, Gcb::Other),
    }
}

fn incb(ch: char) -> Incb {
    property(data::INCB, ch, Incb::None)
}

fn pictographic(ch: char) -> bool {
    let cp = u32::from(ch);
    let index = data::EXTENDED_PICTOGRAPHIC.partition_point(|&(_, end)| end < cp);
    data::EXTENDED_PICTOGRAPHIC
        .get(index)
        .is_some_and(|&(start, _)| start <= cp)
}

/// Whether a UTF-8 boundary also separates extended grapheme clusters.
fn is_boundary(text: &str, index: usize) -> bool {
    let Some(right_char) = text[index..].chars().next() else {
        return true;
    };
    let mut left_chars = text[..index].chars().rev();
    let Some(left_char) = left_chars.next() else {
        return true;
    };
    let left = gcb(left_char);
    let right = gcb(right_char);
    use Gcb::{
        Control, Cr, Extend, L, Lf, Lv, Lvt, Prepend, RegionalIndicator, SpacingMark, T, V, Zwj,
    };
    match (left, right) {
        (Cr, Lf) => return false,                                       // GB3
        (Cr | Lf | Control, _) | (_, Cr | Lf | Control) => return true, // GB4–5
        (L, L | V | Lv | Lvt) | (Lv | V, V | T) | (Lvt | T, T) => return false, // GB6–8
        (_, Extend | Zwj | SpacingMark) | (Prepend, _) => return false, // GB9–9b
        _ => (),
    }
    // GB9c: consonant, any linking/extension run containing a linker, consonant.
    if incb(right_char) == Incb::Consonant {
        let mut linker = false;
        for ch in text[..index].chars().rev() {
            match incb(ch) {
                Incb::Linker => linker = true,
                Incb::Extend => (),
                Incb::Consonant if linker => return false,
                _ => break,
            }
        }
    }
    // GB11: an extended pictograph, zero or more Extend, ZWJ, pictograph.
    if left == Zwj
        && pictographic(right_char)
        && left_chars
            .find(|&ch| gcb(ch) != Extend)
            .is_some_and(pictographic)
    {
        return false;
    }
    // GB12–13: pair regional indicators from the beginning of their run.
    if left == RegionalIndicator && right == RegionalIndicator {
        let before = text[..index]
            .chars()
            .rev()
            .take_while(|&ch| gcb(ch) == RegionalIndicator)
            .count();
        return before.is_multiple_of(2);
    }
    true // GB999
}

/// The closest grapheme boundary strictly before a UTF-8 boundary, or zero.
pub(crate) fn previous(text: &str, index: usize) -> usize {
    text[..index]
        .char_indices()
        .rev()
        .map(|(at, _)| at)
        .find(|&at| is_boundary(text, at))
        .unwrap_or(0)
}

/// The closest grapheme boundary strictly after a UTF-8 boundary, or the end.
pub(crate) fn next(text: &str, index: usize) -> usize {
    text[index..]
        .char_indices()
        .skip(1)
        .map(|(at, _)| index + at)
        .find(|&at| is_boundary(text, at))
        .unwrap_or(text.len())
}

/// The boundary at or before a UTF-8 boundary (native ranges may sit inside a cluster).
pub(crate) fn floor(text: &str, index: usize) -> usize {
    if is_boundary(text, index) {
        index
    } else {
        previous(text, index)
    }
}

/// The boundary at or after a UTF-8 boundary.
pub(crate) fn ceil(text: &str, index: usize) -> usize {
    if is_boundary(text, index) {
        index
    } else {
        next(text, index)
    }
}

/// All extended grapheme boundaries in one forward pass. Geometry needs the
/// complete line; repeated backward RI/Indic scans would make that quadratic.
pub fn boundaries(text: &str) -> Vec<usize> {
    use Gcb::{
        Control, Cr, Extend, L, Lf, Lv, Lvt, Prepend, RegionalIndicator, SpacingMark, T, V, Zwj,
    };
    let mut result = vec![0];
    let mut left = None;
    let mut regional_run = 0usize;
    let mut pictograph_extend = false;
    let mut pictograph_zwj = false;
    let mut indic_consonant = false;
    let mut indic_linker = false;
    for (byte, character) in text.char_indices() {
        let right = gcb(character);
        let indic = incb(character);
        let picture = pictographic(character);
        let boundary = match (left, right) {
            (None, _) | (Some(Cr), Lf) => false,
            (Some(Cr | Lf | Control), _) | (_, Cr | Lf | Control) => true,
            _ => {
                let hangul = matches!(
                    (left, right),
                    (Some(L), L | V | Lv | Lvt) | (Some(Lv | V), V | T) | (Some(Lvt | T), T)
                );
                let extension = matches!(
                    (left, right),
                    (_, Extend | Zwj | SpacingMark) | (Some(Prepend), _)
                );
                let conjunct = indic == Incb::Consonant && indic_consonant && indic_linker;
                let emoji = left == Some(Zwj) && pictograph_zwj && picture;
                let regional = left == Some(RegionalIndicator)
                    && right == RegionalIndicator
                    && !regional_run.is_multiple_of(2);
                !(hangul || extension || conjunct || emoji || regional)
            }
        };
        if boundary {
            result.push(byte);
        }
        regional_run = if right == RegionalIndicator {
            regional_run + 1
        } else {
            0
        };
        pictograph_zwj = right == Zwj && pictograph_extend;
        pictograph_extend = picture || (right == Extend && pictograph_extend);
        match indic {
            Incb::Consonant => {
                indic_consonant = true;
                indic_linker = false;
            }
            Incb::Linker if indic_consonant => indic_linker = true,
            Incb::Extend => (),
            _ => {
                indic_consonant = false;
                indic_linker = false;
            }
        }
        left = Some(right);
    }
    if !text.is_empty() {
        result.push(text.len());
    }
    result
}
