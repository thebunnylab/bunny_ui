use super::*;

/// A pass computed the plain way: every pixel's weighted sum taken whole,
/// divided, and written transposed. The oracle the sliding, row-major,
/// banded passes must match.
fn transposed_pass(source: &[[u16; 4]], target: &mut [[u16; 4]], width: usize, height: usize, pass: Pass) {
    let divisor = u64::from(pass.divisor());
    let radius = pass.radius as isize;
    for y in 0..height {
        let row = &source[y * width..(y + 1) * width];
        let sample = |x: isize| usize::try_from(x).ok().and_then(|x| row.get(x)).copied().unwrap_or_default();
        for x in 0..width as isize {
            let mut total = [0u64; 4];
            for (offset, weight) in (-radius - 1..=radius + 1).map(|k| {
                (k, if k.abs() > radius { u64::from(pass.edge) } else { u64::from(pass.unit) })
            }) {
                for (sum, channel) in total.iter_mut().zip(sample(x + offset)) {
                    *sum += weight * u64::from(channel);
                }
            }
            target[x as usize * height + y] = total.map(|sum| ((sum + divisor / 2) / divisor) as u16);
        }
    }
}

/// Three passes' variance, from the extended box's closed form.
fn variance(pass: Pass) -> f64 {
    let (r, alpha) = (pass.radius as f64, f64::from(pass.edge) / f64::from(pass.unit));
    3.0 * (r * (r + 1.0) * 2.0f64.mul_add(r, 1.0) / 3.0 + 2.0 * alpha * (r + 1.0) * (r + 1.0))
        / 2.0f64.mul_add(alpha, 2.0f64.mul_add(r, 1.0))
}

/// Premultiplied-looking noise: a deterministic generator, each pixel's
/// colour no brighter than its alpha.
fn noise(width: usize, height: usize, seed: u64) -> Vec<[u16; 4]> {
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    (0..width * height)
        .map(|_| {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            let alpha = ((state >> 33) & 0xff) as u16 * 256;
            let bits = state >> 8;
            let channel = |shift: u32| (((bits >> shift) & 0xff) as u16 * 256).min(alpha);
            [channel(0), channel(8), channel(16), alpha]
        })
        .collect()
}

#[test]
fn the_row_major_passes_match_the_transposed_ones_bit_for_bit() {
    for (width, height, radius, edge) in [
        (1, 1, 0, 0),
        (7, 3, 1, 0),
        (7, 3, 0, 1),
        (3, 9, 4, 255),
        (33, 17, 2, 128),
        (64, 48, 7, 13),
        (130, 70, 40, 200),
        (12, 200, 100, 77),
    ] {
        let pass = Pass { radius, edge, unit: Pass::unit(radius) };
        let source = noise(width, height, (width * 31 + height) as u64);
        let mut middle = vec![[0; 4]; source.len()];
        let mut expected = vec![[0; 4]; source.len()];
        transposed_pass(&source, &mut middle, width, height, pass);
        transposed_pass(&middle, &mut expected, height, width, pass);
        for (workers, rows) in [(1, height), (2, 1), (3, 2), (4, 5), (8, 16)] {
            let split = Split { workers, rows: rows.min(height) };
            let mut across = vec![[0; 4]; source.len()];
            let mut down = vec![[0; 4]; source.len()];
            across_rows(&source, &mut across, width, pass, split);
            down_columns(&across, &mut down, width, height, pass, split);
            assert!(
                down == expected,
                "{width}x{height}, {pass:?}, {split:?}: the same pixels as the transposed passes"
            );
        }
    }
}

#[test]
fn the_mean_is_the_division_at_every_boundary() {
    // the floor can only change where n crosses a multiple of the divisor:
    // checking both sides of every multiple a sum can reach checks it all.
    // Every divisor of a pass up to four pixels wide, and wide ones around
    // where the unit starts to coarsen (radius 126) and past the widest a
    // blur asks for (radius 383)
    let pass = |radius, edge| Pass { radius, edge, unit: Pass::unit(radius) };
    let narrow = (0..4).flat_map(|radius| (0..Pass::FINEST).map(move |edge| pass(radius, edge)));
    let wide = [50, 125, 126, 383, 1000].into_iter().flat_map(|radius| {
        let unit = Pass::unit(radius);
        [0, 1, unit / 2, unit - 1].map(|edge| pass(radius, edge))
    });
    for pass in narrow.chain(wide) {
        let divisor = u64::from(pass.divisor());
        assert!(divisor < 1 << 16, "{pass:?} divides by {divisor}");
        let mean = Mean::over(pass.divisor());
        for q in 0..=65_535u64 {
            for n in [q * divisor, q * divisor + divisor - 1] {
                let got = (n * mean.scale) >> Mean::SHIFT;
                assert_eq!(got, n / divisor, "{pass:?}, n {n}");
            }
        }
    }
}

#[test]
fn a_blur_follows_its_sigma_without_notches() {
    // a word clearing out of a blur asks a little less every frame: a
    // hundredth of a pixel less never jumps the blur by more than the
    // lightest edge does out of none (0.15 px), never backs it up, and
    // past half a pixel the three passes blur by sigma
    let mut last = 0.0;
    for hundredths in 0..=38_400 {
        let sigma = f64::from(hundredths) / 100.0;
        let pass = Pass::for_sigma(sigma);
        assert!(pass.edge < pass.unit, "{sigma}: {pass:?} is a wider plain box");
        let blurs = variance(pass).sqrt();
        assert!(blurs >= last, "{sigma}: {blurs} backs up from {last}");
        assert!(blurs - last < 0.16, "{sigma}: {blurs} jumps from {last}");
        assert!(sigma < 0.5 || (blurs - sigma).abs() < sigma * 0.015, "{sigma}: blurs by {blurs}");
        last = blurs;
    }
}

#[test]
fn a_big_picture_splits_its_rows_and_a_small_one_does_not() {
    assert_eq!(Split::for_picture(64, 64).workers, 1, "below the threshold one core does it");
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let big = Split::for_picture(1440, 900);
    assert_eq!(big.workers, if cores < 2 { 1 } else { cores.min(WORKERS) });
    assert!(big.rows >= BAND_ROWS, "never a band thinner than sixteen rows");
    assert_eq!(Split::for_picture(65_536, 8).workers, 1, "too few rows to split");
}
