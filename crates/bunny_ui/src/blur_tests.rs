use super::*;

/// The pass this module shipped before T2-BUNNY-388: a sliding box,
/// transposed on output. The oracle the row-major passes must match.
fn transposed_pass(source: &[[u16; 4]], target: &mut [[u16; 4]], width: usize, height: usize, radius: usize) {
    let divisor = (radius * 2 + 1) as u32;
    for y in 0..height {
        let row = &source[y * width..(y + 1) * width];
        let mut sum = [0u32; 4];
        for pixel in row.iter().take(radius + 1) {
            for channel in 0..4 {
                sum[channel] += u32::from(pixel[channel]);
            }
        }
        for x in 0..width {
            target[x * height + y] = sum.map(|channel| ((channel + divisor / 2) / divisor) as u16);
            if x >= radius {
                for channel in 0..4 {
                    sum[channel] -= u32::from(row[x - radius][channel]);
                }
            }
            if let Some(next) = row.get(x + radius + 1) {
                for channel in 0..4 {
                    sum[channel] += u32::from(next[channel]);
                }
            }
        }
    }
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
    for (width, height, radius) in [
        (1, 1, 0),
        (7, 3, 1),
        (3, 9, 4),
        (33, 17, 2),
        (64, 48, 7),
        (130, 70, 40),
        (12, 200, 100),
    ] {
        let source = noise(width, height, (width * 31 + height) as u64);
        let mut middle = vec![[0; 4]; source.len()];
        let mut expected = vec![[0; 4]; source.len()];
        transposed_pass(&source, &mut middle, width, height, radius);
        transposed_pass(&middle, &mut expected, height, width, radius);
        for (workers, rows) in [(1, height), (2, 1), (3, 2), (4, 5), (8, 16)] {
            let split = Split { workers, rows: rows.min(height) };
            let mut across = vec![[0; 4]; source.len()];
            let mut down = vec![[0; 4]; source.len()];
            across_rows(&source, &mut across, width, radius, split);
            down_columns(&across, &mut down, width, height, radius, split);
            assert!(
                down == expected,
                "{width}x{height}, radius {radius}, {split:?}: the same pixels as the transposed passes"
            );
        }
    }
}

#[test]
fn the_mean_is_the_division_at_every_boundary() {
    // the floor can only change where n crosses a multiple of the divisor:
    // checking both sides of every multiple a sum can reach checks it all
    for divisor in 1..=1100u32 {
        let mean = Mean::over(divisor);
        for q in 0..=65_535u64 {
            for n in [q * u64::from(divisor), q * u64::from(divisor) + u64::from(divisor) - 1] {
                let got = (n * mean.scale) >> Mean::SHIFT;
                assert_eq!(got, n / u64::from(divisor), "divisor {divisor}, n {n}");
            }
        }
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
