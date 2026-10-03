//! Undoing a nearest-neighbour upscale at intake.
//!
//! [`pixel_grid`] finds the factor `k` by which an image was blown up with nearest
//! neighbour (every `k x k` block constant), so the intake in `lib.rs` can trace the original
//! pixels. It lives beside the matting code because both run at intake on the decoded
//! `Rgba`, before any other resampling; the test oracles are in `alpha/intake_tests.rs`.

/// The factor a nearest-neighbour upscale multiplied this image by, if it is one.
///
/// Someone who has a 96-px logo and wants a big SVG resizes the PNG first. Every viewer
/// does that with nearest neighbour on request, and plenty do it without being asked, so
/// what arrives is a grid of k x k constant blocks. The tracer then describes exactly what
/// it is given: on a 96-px logo blown up to 768 the palette shatters from 3 inks to 13, and
/// the boundary comes back as 1568 straight lines walking round pixel corners — the
/// staircase is not an artefact of the fit, it is in the file.
///
/// Replication is exactly invertible: average each block and the original pixels come back
/// bit for bit. So the test is strict — every block constant, no tolerance for "nearly" —
/// because a 1-px tolerance would also catch a genuine drawing of large flat squares, and
/// averaging that away would be a real loss. `k` is the largest factor that passes, tried
/// downwards so an 8x upscale is undone as 8x and not as 2x.
///
/// "Constant" means every channel of every pixel in a block, alpha included, within
/// `1/512` of the block's top-left pixel: under half an 8-bit level, so for 8-bit input it
/// is exact equality and only float round-off is forgiven. A factor must divide both sides
/// exactly, and it is capped at 32 and so that at least 64 px remain on each side; a raster
/// under 128 px on either side is never unblocked.
///
/// Anti-aliased and resampled upscales are a different problem and not this one: their
/// blocks are not constant, they fail here, and `--sr` is what addresses them.
///
/// Called by the intake in `lib.rs` (unless `--no-unblock`), before any other resampling;
/// it only measures, and the caller downsamples by `k`.
///
/// # How: the gcd of the change positions, then the block test
///
/// The factors worth testing are found first, in one early-exiting scan
/// ([`change_gcd`]): `g = gcd(w, h, every x where a pixel differs sharply from its left
/// neighbour, every y where a row differs sharply from the row above)`. Then, from the
/// largest factor down, only the `k` that divide `g` get the block test
/// ([`blocks_constant`], the test this function always ran).
///
/// *Why the answer is the same.* The old loop returned the largest `k ≤ k_max` dividing
/// `w` and `h` whose blocks pass the test. Take any such `k` that passes. Two horizontally
/// adjacent pixels at `x − 1` and `x` with `k ∤ x` lie in one block, so each is within
/// `1/512` of the block's first pixel (the test's comparison is on a rounded f32
/// difference, but `2⁻⁹` is a float and rounding is monotone, so the exact difference is
/// below `2⁻⁹` too) and they differ by less than `2 · 2⁻⁹ = 1/256`. So every position where
/// neighbours differ by more than `1/256` is a multiple of `k`, and so are `w` and `h`:
/// `k` divides `g`. Skipping the `k ∤ g` therefore never skips a passing factor, and the
/// others get the old test itself, so the result is the old result for any input.
///
/// *Why it is fast.* On anything that is not an upscale, two edges at coprime positions
/// appear within the first rows of content and `g` falls to 1: the scan stops there and no
/// block test runs. The old loop ran the block test for every divisor of `w` and `h` from
/// 32 down, each scanning until its first non-constant block -- 7.1 ms at 2048 px, most of it
/// spent on the blank rows above the artwork, once per divisor. For 8-bit input the two
/// views coincide: distinct levels are at least `1/255 > 1/256` apart, so a "sharp change" is
/// any change, the divisors of `g` are exactly the factors whose blocks are constant, and
/// the block test only confirms.
///
/// Not from the literature: the gcd of change positions as a candidate filter for exact
/// block replication, because the published resampling detectors are statistical (they
/// estimate a periodic correlation of an interpolated signal) and would not reproduce this
/// function's exact answer. See also: A. C. Popescu, H. Farid, "Exposing Digital Forgeries by
/// Detecting Traces of Resampling", IEEE Trans. Signal Processing 53(2):758–767, 2005, DOI
/// 10.1109/TSP.2004.839932.
pub(crate) fn pixel_grid(img: &inkvec_trace::Rgba) -> Option<usize> {
    const MAX_FACTOR: usize = 32;
    let (w, h) = (img.width, img.height);
    // An image with a zero side has no blocks, and `smallest.min(w)` below would be a
    // division by zero: a GIF with a zero-wide logical screen reached here as 0 x 30000
    // (intake fuzz, 2026-10-02). The decoder now refuses such files; this guard keeps the
    // function total for any caller that builds an `Rgba` itself.
    if w == 0 || h == 0 {
        return None;
    }
    // Below this there is nothing to gain and something to lose: a 2x undo of a small icon
    // leaves too few pixels for the boundary solve to work with.
    let smallest = 64;
    let k_max = MAX_FACTOR.min(w / smallest.min(w)).min(h / smallest.min(h));
    if k_max < 2 {
        return None;
    }
    let g = change_gcd(img);
    (2..=k_max)
        .rev()
        .find(|&k| divides(k, g) && blocks_constant(img, k))
}

/// Whether `k` divides `n` (`k ≥ 1`), without the remainder operator: wazero's arm64
/// compiler miscompiled `i32.rem_u` in a hot loop last round (the Go binding runs this
/// crate as WebAssembly), so new code here tests divisibility by multiplying back.
pub(super) fn divides(k: usize, n: usize) -> bool {
    (n / k) * k == n
}

/// The greatest common divisor, by Stein's binary algorithm (shifts and subtraction only,
/// for the same reason as [`divides`]). `gcd(0, n) = n`.
pub(super) fn gcd(mut a: usize, mut b: usize) -> usize {
    if a == 0 || b == 0 {
        return a | b;
    }
    let shift = (a | b).trailing_zeros();
    a >>= a.trailing_zeros();
    loop {
        b >>= b.trailing_zeros();
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        b -= a;
        if b == 0 {
            return a << shift;
        }
    }
}

/// `gcd(w, h, X, Y)` for the `w × h` image, where `X` is every column `x ≥ 1` at which some
/// row's pixel differs from its left neighbour by more than `1/256` in some channel, and
/// `Y` every row `y ≥ 1` in which some pixel differs that much from the one above. The
/// scan runs row by row and stops as soon as the gcd reaches 1. NaN differences count as
/// no change (a NaN pixel fails the block test anyway). O(pixels read); on typical art the
/// read stops a few rows into the content.
///
/// Blank rows are the common case before the content starts (a logo on a white page), so a
/// row is first compared with the row above, and with itself shifted by one pixel, bit for
/// bit ([`same_bits`], which vectorises); only a row that differs is examined pixel by
/// pixel. Identical bits mean every difference is 0 (or NaN), which is never sharp, so the
/// shortcut cannot hide a change.
pub(super) fn change_gcd(img: &inkvec_trace::Rgba) -> usize {
    /// Neighbours in one block differ by less than this (see [`pixel_grid`]).
    const SHARP: f32 = 1.0 / 256.0;
    let (w, h) = (img.width, img.height);
    let sharp = |a: &[f32], b: &[f32]| a.iter().zip(b).any(|(p, q)| (p - q).abs() > SHARP);
    let mut g = gcd(w, h);
    for y in 0..h {
        if g < 2 {
            break;
        }
        let row = &img.data[y * w * 4..(y + 1) * w * 4];
        if y > 0 {
            let above = &img.data[(y - 1) * w * 4..y * w * 4];
            if !same_bits(above, row) && sharp(above, row) {
                g = gcd(g, y);
            }
        }
        // Each pixel against its left neighbour: the row against itself one pixel over.
        if same_bits(&row[4..], &row[..row.len() - 4]) {
            continue;
        }
        for x in 1..w {
            if g < 2 {
                break;
            }
            if sharp(&row[(x - 1) * 4..x * 4], &row[x * 4..x * 4 + 4]) {
                g = gcd(g, x);
            }
        }
    }
    g
}

/// Whether two equally long float slices hold the same bits. Compared 64 floats at a time
/// with an OR of XORs, a loop without an early exit that the compiler vectorises; the early
/// exit is per block.
pub(super) fn same_bits(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len()
        && a.chunks(64).zip(b.chunks(64)).all(|(x, y)| {
            x.iter()
                .zip(y)
                .fold(0u32, |acc, (p, q)| acc | (p.to_bits() ^ q.to_bits()))
                == 0
        })
}

/// The block test: every channel of every pixel in every `k × k` block within `1/512` of
/// the block's top-left pixel (so for 8-bit input, exactly equal). `k` must divide both
/// sides. Stops at the first block that fails; a full scan when every block passes.
pub(super) fn blocks_constant(img: &inkvec_trace::Rgba, k: usize) -> bool {
    let (w, h) = (img.width, img.height);
    let px = |x: usize, y: usize| -> &[f32] { &img.data[(y * w + x) * 4..(y * w + x) * 4 + 4] };
    (0..h / k).all(|by| {
        (0..w / k).all(|bx| {
            let first = px(bx * k, by * k);
            (0..k).all(|dy| {
                (0..k).all(|dx| {
                    let p = px(bx * k + dx, by * k + dy);
                    (0..4).all(|c| (p[c] - first[c]).abs() < 1.0 / 512.0)
                })
            })
        })
    })
}
