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

// MARK: - Bunny Lit v1
//
// The shading every tier evaluates, written once here for the CPU and
// mirrored by the shaders operation for operation. Inputs are in the
// eye's space; colours are linear light.

pub type V3 = [f32; 3];

pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn mul(a: V3, b: V3) -> V3 {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

pub fn mix(a: V3, b: V3, t: f32) -> V3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

pub fn normalize(a: V3) -> V3 {
    let length = dot(a, a).sqrt();
    if length > 0.0 { scale(a, 1.0 / length) } else { [0.0, 0.0, 1.0] }
}

pub fn saturate(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// `2^x` for `x ≤ 0`, by the exponent bits and a fixed polynomial on
/// the fraction — the same number on every machine, within 3·10⁻⁷ of the
/// truth.
pub fn exp2(x: f32) -> f32 {
    let x = x.max(-126.0);
    let whole = x.floor();
    let f = x - whole;
    let fraction = 0.99999976 + f * (0.6931568 + f * (0.24013178 + f * (0.05587644 + f * (0.00894063 + f * 0.0018943873))));
    let power = f32::from_bits(((whole as i32 + 127) as u32) << 23);
    power * fraction
}

/// The sky over a surface that faces `direction`: the ground's light
/// below, the sky's above, blended by how much it faces up.
pub fn hemisphere(direction: V3, up: V3, sky: V3, ground: V3) -> V3 {
    mix(ground, sky, 0.5 + 0.5 * dot(direction, up))
}

/// How much light a surface reflects from its surroundings at an angle,
/// with no picture of the surroundings: a fitted curve.
pub fn environment_brdf(f0: V3, roughness: f32, n_dot_v: f32) -> V3 {
    let r = [
        1.0 - roughness,
        roughness * -0.0275 + 0.0425,
        roughness * -0.572 + 1.04,
        roughness * 0.022 - 0.04,
    ];
    let a004 = (r[0] * r[0]).min(exp2(-9.28 * n_dot_v)) * r[0] + r[1];
    let (a, b) = (-1.04 * a004 + r[2], 1.04 * a004 + r[3]);
    [f0[0] * a + b, f0[1] * a + b, f0[2] * a + b]
}

/// The tone curve: the identity below the knee, a soft shoulder above it
/// that keeps the hue, and the hottest light bleaching toward white.
pub fn tone(color: V3, exposure: f32, knee: f32) -> V3 {
    let mut c = scale(color, exposure);
    let m = c[0].max(c[1]).max(c[2]);
    if m > knee {
        let f = knee + (1.0 - knee) * (m - knee) / ((1.0 - knee) + (m - knee));
        c = scale(c, f / m);
        let t = saturate((m - 1.0) / 3.0);
        let bleach = 0.6 * t * t * (3.0 - 2.0 * t);
        c = mix(c, [f, f, f], bleach);
    }
    [saturate(c[0]), saturate(c[1]), saturate(c[2])]
}

/// The 4×4 ordered dither at a pixel, in `(−½, ½)`.
pub fn bayer(x: u32, y: u32) -> f32 {
    (super::bloom::bayer_index(x, y) as f32 + 0.5) / 16.0 - 0.5
}

/// A surface point as a shader sees it.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    /// Eye-space position.
    pub position: V3,
    /// Eye-space unit normal, on the side the eye sees.
    pub normal: V3,
    /// Linear base colour (alpha apart).
    pub base: V3,
    pub roughness: f32,
    pub metallic: f32,
    pub emissive: V3,
    /// Baked light from the surroundings, 1 = nothing in the way.
    pub occlusion: f32,
    /// The light groups the surface belongs to.
    pub groups: u32,
    /// How much of the sun reaches the point: 1 in full light, 0 in the
    /// sun's shadow.
    pub shadow: f32,
    /// What the floor map gives the point: linear light added, and in
    /// `[3]` the share of the sky's light that reaches it. `[0, 0, 0, 1]`
    /// for a surface that reads none.
    pub floor: [f32; 4],
    /// Multiplies the light from the sky and the ground (linear).
    pub ambient: V3,
}

/// What a surface that reads no floor map reads.
pub const NO_FLOOR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// The light a surface sends toward the eye, before the tone curve.
pub fn lit(u: &super::wire::FrameUniforms, surface: &Surface) -> V3 {
    let n = surface.normal;
    let parallel = u.params[3] > 0.5;
    let v = if parallel { [0.0, 0.0, 1.0] } else { normalize(scale(surface.position, -1.0)) };
    let n_dot_v = dot(n, v).clamp(1e-4, 1.0);
    let metal = saturate(surface.metallic);
    let diffuse = scale(surface.base, 1.0 - metal);
    let f0 = mix([0.04, 0.04, 0.04], surface.base, metal);
    let r = surface.roughness.clamp(0.045, 1.0);
    let (a, a2) = (r * r, r * r * r * r);
    let up = [u.sky_up[0], u.sky_up[1], u.sky_up[2]];
    let sky = mul([u.sky_color[0], u.sky_color[1], u.sky_color[2]], surface.ambient);
    let ground = mul([u.ground_color[0], u.ground_color[1], u.ground_color[2]], surface.ambient);
    let ao = (1.0 + (surface.occlusion - 1.0) * 0.8) * surface.floor[3];

    let mut light = scale(mul(diffuse, hemisphere(n, up, sky, ground)), ao);
    let reflected = add(scale(n, 2.0 * dot(n, v)), scale(v, -1.0));
    light = add(light, scale(mul(environment_brdf(f0, r, n_dot_v), hemisphere(reflected, up, sky, ground)), ao));

    if u.sun_direction[3] > 0.5 {
        let l = [u.sun_direction[0], u.sun_direction[1], u.sun_direction[2]];
        let n_dot_l = saturate(dot(n, l));
        if n_dot_l > 0.0 {
            let h = normalize(add(l, v));
            let (n_dot_h, v_dot_h) = (saturate(dot(n, h)), saturate(dot(v, h)));
            let q = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
            // π·D cancels the 1/π of the distribution
            let pi_d = a2 / (q * q);
            let visibility = 0.5 / (2.0 * n_dot_l * n_dot_v + (n_dot_l + n_dot_v - 2.0 * n_dot_l * n_dot_v) * a).max(1e-5);
            let x = 1.0 - v_dot_h;
            let x5 = x * x * x * x * x;
            let fresnel = add(f0, scale(add([1.0, 1.0, 1.0], scale(f0, -1.0)), x5));
            let specular = scale(fresnel, pi_d * visibility);
            let sun = [u.sun_color[0], u.sun_color[1], u.sun_color[2]];
            light = add(light, scale(mul(add(diffuse, specular), sun), n_dot_l * surface.shadow));
        }
    }

    let points = u.counts[0] as usize;
    for i in 0..points.min(super::wire::MAX_POINTS) {
        if u.point_groups[i / 4][i % 4] & surface.groups == 0 {
            continue;
        }
        let p = u.point_position[i];
        let to = [p[0] - surface.position[0], p[1] - surface.position[1], p[2] - surface.position[2]];
        let d2 = dot(to, to);
        let range2 = p[3] * p[3];
        if d2 >= range2 || range2 <= 0.0 {
            continue;
        }
        let l = scale(to, 1.0 / d2.max(1e-8).sqrt());
        let w = saturate(1.0 - d2 / range2);
        let r0 = u.point_color[i][3];
        let attenuation = w * w * (r0 * r0) / (d2 + r0 * r0).max(1e-8);
        let color = [u.point_color[i][0], u.point_color[i][1], u.point_color[i][2]];
        light = add(light, scale(mul(diffuse, color), attenuation * saturate(dot(n, l))));
    }

    light = add(light, mul(diffuse, [surface.floor[0], surface.floor[1], surface.floor[2]]));
    add(light, surface.emissive)
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
    fn the_fixed_exp2_follows_the_true_one() {
        for i in 0..=1000 {
            let x = -10.0 * i as f32 / 1000.0;
            let truth = (x as f64).exp2();
            let ours = exp2(x) as f64;
            assert!((ours - truth).abs() <= 3e-7 * truth.max(1e-3), "2^{x}: {ours} vs {truth}");
        }
        assert_eq!(exp2(0.0), 0.99999976);
    }

    #[test]
    fn the_tone_curve_keeps_the_house_colours() {
        // below the knee nothing moves
        assert_eq!(tone([0.57, 0.3, 0.1], 1.0, 0.8), [0.57, 0.3, 0.1]);
        // above it the hue holds and the value stays inside one
        let toned = tone([3.0, 1.5, 0.75], 1.0, 0.8);
        assert!(toned.iter().all(|c| *c <= 1.0));
        assert!(toned[0] > toned[1] && toned[1] > toned[2]);
    }

    #[test]
    fn the_continuous_code_sits_between_its_neighbours() {
        let halfway = (decode(100) + decode(101)) * 0.5;
        let code = encode_code(halfway);
        assert!(code > 100.0 && code < 101.0, "{code}");
    }
}
