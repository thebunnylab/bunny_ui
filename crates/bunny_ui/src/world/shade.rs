//! The arithmetic of light that the CPU rasterizer and every shader share.
//!
//! The oracle must give the same bytes on every machine, so nothing here
//! calls a transcendental function of the platform's math library per
//! pixel: only `+ − × ÷`, `sqrt`, `min`, `max`, `floor`, fixed tables and
//! fixed polynomials. `sqrt` is exactly rounded everywhere; `powf` is not.

/// sRGB code → linear light, exactly: `((c/255 + 0.055) / 1.055)^2.4`
/// above the knee, `c/255 / 12.92` below. Stored as bit patterns, so the
/// table is the same number on every machine that reads it.
pub const SRGB_DECODE: [f32; 256] = [
    f32::from_bits(0x00000000), f32::from_bits(0x399F22B4), f32::from_bits(0x3A1F22B4), f32::from_bits(0x3A6EB40E), f32::from_bits(0x3A9F22B4), f32::from_bits(0x3AC6EB61), f32::from_bits(0x3AEEB40E), f32::from_bits(0x3B0B3E5D),
    f32::from_bits(0x3B1F22B4), f32::from_bits(0x3B33070A), f32::from_bits(0x3B46EB61), f32::from_bits(0x3B5B518E), f32::from_bits(0x3B70F18F), f32::from_bits(0x3B83E1C6), f32::from_bits(0x3B8FE616), f32::from_bits(0x3B9C87FD),
    f32::from_bits(0x3BA9C9B6), f32::from_bits(0x3BB7AD6F), f32::from_bits(0x3BC6354A), f32::from_bits(0x3BD56360), f32::from_bits(0x3BE539C1), f32::from_bits(0x3BF5BA71), f32::from_bits(0x3C0373B6), f32::from_bits(0x3C0C6153),
    f32::from_bits(0x3C15A705), f32::from_bits(0x3C1F45BE), f32::from_bits(0x3C293E6B), f32::from_bits(0x3C3391F7), f32::from_bits(0x3C3E4149), f32::from_bits(0x3C494D44), f32::from_bits(0x3C54B6C9), f32::from_bits(0x3C607EB4),
    f32::from_bits(0x3C6CA5DF), f32::from_bits(0x3C792D22), f32::from_bits(0x3C830AA9), f32::from_bits(0x3C89AF9F), f32::from_bits(0x3C9085DC), f32::from_bits(0x3C978DC6), f32::from_bits(0x3C9EC7C2), f32::from_bits(0x3CA63433),
    f32::from_bits(0x3CADD37D), f32::from_bits(0x3CB5A602), f32::from_bits(0x3CBDAC21), f32::from_bits(0x3CC5E63A), f32::from_bits(0x3CCE54AC), f32::from_bits(0x3CD6F7D5), f32::from_bits(0x3CDFD010), f32::from_bits(0x3CE8DDBA),
    f32::from_bits(0x3CF2212D), f32::from_bits(0x3CFB9AC3), f32::from_bits(0x3D02A56A), f32::from_bits(0x3D0798DD), f32::from_bits(0x3D0CA7E6), f32::from_bits(0x3D11D2AF), f32::from_bits(0x3D171964), f32::from_bits(0x3D1C7C30),
    f32::from_bits(0x3D21FB3C), f32::from_bits(0x3D2796B2), f32::from_bits(0x3D2D4EBB), f32::from_bits(0x3D332381), f32::from_bits(0x3D39152B), f32::from_bits(0x3D3F23E4), f32::from_bits(0x3D454FD2), f32::from_bits(0x3D4B991D),
    f32::from_bits(0x3D51FFEC), f32::from_bits(0x3D588468), f32::from_bits(0x3D5F26B6), f32::from_bits(0x3D65E6FD), f32::from_bits(0x3D6CC563), f32::from_bits(0x3D73C20E), f32::from_bits(0x3D7ADD24), f32::from_bits(0x3D810B65),
    f32::from_bits(0x3D84B793), f32::from_bits(0x3D88732E), f32::from_bits(0x3D8C3E48), f32::from_bits(0x3D9018F4), f32::from_bits(0x3D940344), f32::from_bits(0x3D97FD49), f32::from_bits(0x3D9C0715), f32::from_bits(0x3DA020BA),
    f32::from_bits(0x3DA44A4A), f32::from_bits(0x3DA883D6), f32::from_bits(0x3DACCD6F), f32::from_bits(0x3DB12727), f32::from_bits(0x3DB5910F), f32::from_bits(0x3DBA0B38), f32::from_bits(0x3DBE95B3), f32::from_bits(0x3DC33090),
    f32::from_bits(0x3DC7DBE0), f32::from_bits(0x3DCC97B4), f32::from_bits(0x3DD1641D), f32::from_bits(0x3DD6412B), f32::from_bits(0x3DDB2EEE), f32::from_bits(0x3DE02D76), f32::from_bits(0x3DE53CD4), f32::from_bits(0x3DEA5D18),
    f32::from_bits(0x3DEF8E51), f32::from_bits(0x3DF4D090), f32::from_bits(0x3DFA23E5), f32::from_bits(0x3DFF885E), f32::from_bits(0x3E027F06), f32::from_bits(0x3E05427F), f32::from_bits(0x3E080EA2), f32::from_bits(0x3E0AE377),
    f32::from_bits(0x3E0DC104), f32::from_bits(0x3E10A753), f32::from_bits(0x3E13966A), f32::from_bits(0x3E168E51), f32::from_bits(0x3E198F0F), f32::from_bits(0x3E1C98AC), f32::from_bits(0x3E1FAB30), f32::from_bits(0x3E22C6A1),
    f32::from_bits(0x3E25EB07), f32::from_bits(0x3E29186A), f32::from_bits(0x3E2C4ED0), f32::from_bits(0x3E2F8E42), f32::from_bits(0x3E32D6C5), f32::from_bits(0x3E362862), f32::from_bits(0x3E39831F), f32::from_bits(0x3E3CE703),
    f32::from_bits(0x3E405417), f32::from_bits(0x3E43CA60), f32::from_bits(0x3E4749E6), f32::from_bits(0x3E4AD2AF), f32::from_bits(0x3E4E64C3), f32::from_bits(0x3E520029), f32::from_bits(0x3E55A4E7), f32::from_bits(0x3E595305),
    f32::from_bits(0x3E5D0A89), f32::from_bits(0x3E60CB7A), f32::from_bits(0x3E6495DF), f32::from_bits(0x3E6869BE), f32::from_bits(0x3E6C471F), f32::from_bits(0x3E702E07), f32::from_bits(0x3E741E7E), f32::from_bits(0x3E78188B),
    f32::from_bits(0x3E7C1C33), f32::from_bits(0x3E8014BF), f32::from_bits(0x3E822039), f32::from_bits(0x3E84308B), f32::from_bits(0x3E8645B8), f32::from_bits(0x3E885FC3), f32::from_bits(0x3E8A7EB0), f32::from_bits(0x3E8CA281),
    f32::from_bits(0x3E8ECB3B), f32::from_bits(0x3E90F8DF), f32::from_bits(0x3E932B72), f32::from_bits(0x3E9562F6), f32::from_bits(0x3E979F6F), f32::from_bits(0x3E99E0E0), f32::from_bits(0x3E9C274C), f32::from_bits(0x3E9E72B6),
    f32::from_bits(0x3EA0C321), f32::from_bits(0x3EA31890), f32::from_bits(0x3EA57307), f32::from_bits(0x3EA7D288), f32::from_bits(0x3EAA3716), f32::from_bits(0x3EACA0B6), f32::from_bits(0x3EAF0F68), f32::from_bits(0x3EB18332),
    f32::from_bits(0x3EB3FC15), f32::from_bits(0x3EB67A14), f32::from_bits(0x3EB8FD34), f32::from_bits(0x3EBB8576), f32::from_bits(0x3EBE12DE), f32::from_bits(0x3EC0A56E), f32::from_bits(0x3EC33D2A), f32::from_bits(0x3EC5DA14),
    f32::from_bits(0x3EC87C30), f32::from_bits(0x3ECB2380), f32::from_bits(0x3ECDD008), f32::from_bits(0x3ED081CA), f32::from_bits(0x3ED338C9), f32::from_bits(0x3ED5F508), f32::from_bits(0x3ED8B68A), f32::from_bits(0x3EDB7D52),
    f32::from_bits(0x3EDE4963), f32::from_bits(0x3EE11ABF), f32::from_bits(0x3EE3F169), f32::from_bits(0x3EE6CD65), f32::from_bits(0x3EE9AEB5), f32::from_bits(0x3EEC955B), f32::from_bits(0x3EEF815C), f32::from_bits(0x3EF272B8),
    f32::from_bits(0x3EF56974), f32::from_bits(0x3EF86593), f32::from_bits(0x3EFB6716), f32::from_bits(0x3EFE6E00), f32::from_bits(0x3F00BD2B), f32::from_bits(0x3F02460C), f32::from_bits(0x3F03D1A5), f32::from_bits(0x3F055FF7),
    f32::from_bits(0x3F06F104), f32::from_bits(0x3F0884CD), f32::from_bits(0x3F0A1B54), f32::from_bits(0x3F0BB499), f32::from_bits(0x3F0D509F), f32::from_bits(0x3F0EEF65), f32::from_bits(0x3F1090EF), f32::from_bits(0x3F12353D),
    f32::from_bits(0x3F13DC50), f32::from_bits(0x3F15862A), f32::from_bits(0x3F1732CC), f32::from_bits(0x3F18E237), f32::from_bits(0x3F1A946E), f32::from_bits(0x3F1C4970), f32::from_bits(0x3F1E0140), f32::from_bits(0x3F1FBBDE),
    f32::from_bits(0x3F21794D), f32::from_bits(0x3F23398C), f32::from_bits(0x3F24FC9F), f32::from_bits(0x3F26C285), f32::from_bits(0x3F288B41), f32::from_bits(0x3F2A56D2), f32::from_bits(0x3F2C253C), f32::from_bits(0x3F2DF67F),
    f32::from_bits(0x3F2FCA9C), f32::from_bits(0x3F31A194), f32::from_bits(0x3F337B6A), f32::from_bits(0x3F35581D), f32::from_bits(0x3F3737B0), f32::from_bits(0x3F391A24), f32::from_bits(0x3F3AFF7A), f32::from_bits(0x3F3CE7B2),
    f32::from_bits(0x3F3ED2CF), f32::from_bits(0x3F40C0D2), f32::from_bits(0x3F42B1BC), f32::from_bits(0x3F44A58E), f32::from_bits(0x3F469C49), f32::from_bits(0x3F4895EF), f32::from_bits(0x3F4A9280), f32::from_bits(0x3F4C91FF),
    f32::from_bits(0x3F4E946C), f32::from_bits(0x3F5099C9), f32::from_bits(0x3F52A216), f32::from_bits(0x3F54AD56), f32::from_bits(0x3F56BB88), f32::from_bits(0x3F58CCAF), f32::from_bits(0x3F5AE0CC), f32::from_bits(0x3F5CF7DF),
    f32::from_bits(0x3F5F11EA), f32::from_bits(0x3F612EEF), f32::from_bits(0x3F634EEE), f32::from_bits(0x3F6571E9), f32::from_bits(0x3F6797E0), f32::from_bits(0x3F69C0D5), f32::from_bits(0x3F6BECCA), f32::from_bits(0x3F6E1BBF),
    f32::from_bits(0x3F704DB5), f32::from_bits(0x3F7282AE), f32::from_bits(0x3F74BAAB), f32::from_bits(0x3F76F5AE), f32::from_bits(0x3F7933B6), f32::from_bits(0x3F7B74C6), f32::from_bits(0x3F7DB8DE), f32::from_bits(0x3F800000),
];

/// Linear light of an sRGB code.
pub fn decode(code: u8) -> f32 {
    SRGB_DECODE[code as usize]
}

/// Linear light → a CONTINUOUS sRGB code in `[0, 255]`: the exact code
/// below it in the table, plus how far the light reaches toward the next
/// one. A shader computes the same curve with its own power function;
/// the two agree to a hundredth of a code.
pub fn encode_code(linear: f32) -> f32 {
    if linear.is_nan() || linear <= 0.0 {
        return 0.0;
    }
    if linear >= 1.0 {
        return 255.0;
    }
    let (mut low, mut high) = (0usize, 255usize);
    while high - low > 1 {
        let middle = (low + high) / 2;
        if SRGB_DECODE[middle] <= linear {
            low = middle;
        } else {
            high = middle;
        }
    }
    let (a, b) = (SRGB_DECODE[low], SRGB_DECODE[high]);
    low as f32 + (linear - a) / (b - a)
}

/// Linear light → the nearest sRGB code.
pub fn encode(linear: f32) -> u8 {
    quantize(encode_code(linear), 0.0)
}

/// A continuous code to a byte, with `dither` in `(−½, ½)` added first.
pub fn quantize(code: f32, dither: f32) -> u8 {
    (code + 0.5 + dither).floor().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_srgb_round_trip_is_exact() {
        for code in 0..=255u8 {
            assert_eq!(encode(decode(code)), code, "code {code}");
        }
        assert!(SRGB_DECODE.windows(2).all(|pair| pair[0] < pair[1]), "the table climbs");
        assert_eq!(decode(255), 1.0);
        assert_eq!(encode(-1.0), 0);
        assert_eq!(encode(f32::NAN), 0);
        assert_eq!(encode(2.0), 255);
    }

    #[test]
    fn the_continuous_code_sits_between_its_neighbours() {
        let halfway = (decode(100) + decode(101)) * 0.5;
        let code = encode_code(halfway);
        assert!(code > 100.0 && code < 101.0, "{code}");
    }
}
