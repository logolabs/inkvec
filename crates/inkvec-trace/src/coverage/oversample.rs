//! How many times more pixels a raster has than its drawing needs: the downsampling
//! round trip ([`oversample_factor`]) and the yardsticks it is held to.
//!
//! `inkvec-cli`'s intake reads the factor to price `--min-area` and lambda in the raster's
//! own units above 128 px (`price_in_raster_units`), and `--content-units` reads it too.
//! Moved out of `coverage.rs` unchanged on 2026-10-02, when the relative test
//! ([`OVERSAMPLE_KEEP`], [`flat_error`]) took that file past its length budget; it is
//! re-exported there.

/// Mean absolute round-trip error, in 8-bit levels, above which a downsample has lost
/// something.
///
/// This cannot separate native from oversampled on its own, and it was measured trying:
/// across seventy corpus rasters the lowest native reading at /2 is 1.29 (a synthetic
/// gradient, which really is band-limited and really does survive halving), while the 4x
/// upscale this exists for reads 2.12 at /4. The distributions overlap, so a threshold
/// permissive enough to catch the upscale would also rewrite smooth native artwork.
///
/// So this is not a gate and must not be used as one. The caller decides whether a raster
/// is native -- `intake_scale` against `color::SOFT_INTAKE_EDGE` does that, and its
/// margin is real (native maximum 1.50 against a 1.75 threshold) -- and only then asks
/// this by how much. Inside that gate the value can be generous, because nothing native
/// reaches it.
const OVERSAMPLE_TOL: f64 = 3.0;

/// Largest share of the image's detail a round trip may lose and still count as lossless:
/// its error over [`flat_error`], the error of the best single colour.
///
/// [`OVERSAMPLE_TOL`] is a mean over every pixel, so it is diluted by flat area. A
/// near-empty raster passes it at every factor because its detail is too small a share of
/// the image to move the mean, not because the detail survived: a 38 px² disc on a 144 px
/// canvas, erased outright, costs 0.47 levels against the 3.0 allowed, so it read as 8x
/// oversampled, the speckle floor went up 64x to 128 px² and the disc was removed. A
/// round trip that keeps the drawing loses a small share of it; one that erases a shape
/// loses about all of it. This asks for at most half, and the two populations sit far
/// apart (measured 2026-10-02, the round-trip error over the flat error at every factor
/// the absolute test accepts):
///
/// * the corpus icons of the screen, held_a and held_b sets at 512 and 1024 px read at
///   most 0.31 and 0.17 (both `twemoji/1f7eb`, a square that fills the canvas), every
///   other icon at most 0.18; at 256 px the same square reads 0.56 at /8 and is the one
///   corpus icon this rule moves (8x to 4x; its rounded corners are about 3 px at /8);
/// * 897 stress and test inputs over 128 px move only in three variants of that square;
/// * the 38 px² disc reads 0.36 at /2, 0.77 at /4 and 1.60 at /8, so it is 2x, not 8x.
///
/// Inspired by: the relative error measures of forecast evaluation, a method's error
/// divided by a benchmark method's (Hyndman, R. J. & Koehler, A. B. (2006), "Another look
/// at measures of forecast accuracy", *International Journal of Forecasting*
/// 22(4):679-688, doi:10.1016/j.ijforecast.2006.03.001). Here the method is the round
/// trip and the benchmark the best constant image under the same absolute error, the
/// per-channel median. The half is our choice: a majority of the detail kept, with the
/// margins above on both sides.
const OVERSAMPLE_KEEP: f64 = 0.5;

/// The mean absolute error of the best single colour for this image, in 8-bit levels: the
/// per-channel median `m_c`, and `Σ_p Σ_c |rgb_p,c − m_c| · 255 / (3 · width · height)`.
///
/// The median minimises the mean absolute error over constants, so this is how much an
/// image loses when everything but its one most typical colour is erased: the yardstick
/// [`oversample_factor`] measures a round trip's loss against. 0 for a flat image. With an
/// even pixel count any value between the two middle ones minimises the sum, and gives the
/// same sum, so taking the upper middle is exact. Needs `rgb.len() >= width * height > 0`.
/// O(n) time (selection, not a sort) and 4n bytes of scratch per channel.
fn flat_error(rgb: &[[f32; 3]], width: usize, height: usize) -> f64 {
    let n = width * height;
    let mut err = 0.0f64;
    let mut v: Vec<f32> = Vec::with_capacity(n);
    for c in 0..3 {
        v.clear();
        v.extend(rgb[..n].iter().map(|p| p[c]));
        let (_, med, _) = v.select_nth_unstable_by(n / 2, |a, b| a.total_cmp(b));
        let med = *med as f64;
        err += rgb[..n]
            .iter()
            .map(|p| (p[c] as f64 - med).abs())
            .sum::<f64>();
    }
    err * 255.0 / (n * 3) as f64
}

/// By what factor this raster carries the same drawing on more pixels than it needs.
///
/// [`intake_scale`](super::intake_scale) answers a related question by measuring how wide
/// an edge transition is, and it is the right measure for the palette's noise guard: a
/// soft edge really does
/// put intermediate colours on the ramp. It is the wrong measure for *tolerances*,
/// because a super-resolution model defeats it -- it returns a sharp edge at high
/// resolution, so the raster reads as barely oversampled when it carries four times the
/// pixels the drawing needs. Measured on a real brand mark upscaled 4x: edge width 2.00,
/// where the answer is 4.
///
/// This asks the question directly instead. An oversampled raster has a property that
/// sharpening cannot fake: its pixels can be thrown away and put back. Halve it, restore
/// it, and compare -- if nothing was lost, the halved version already carried the whole
/// drawing. Repeated, that gives the factor, and it is indifferent to whether the surplus
/// pixels are crisp or blurred, asking only whether they say anything.
///
/// For `k` in 2, 4, 8: box-average `k x k` blocks (the trailing `width mod k` columns and
/// rows are dropped), resample back to full size bilinearly — pixel centre `x` maps to
/// `(x + 0.5)/k − 0.5` in the small image, clamped to its edge — and take the mean absolute
/// error over all pixels and channels, in 8-bit levels. A `k` passes when that error stays
/// under `OVERSAMPLE_TOL` *and* is at most `OVERSAMPLE_KEEP` (half) of [`flat_error`],
/// the error of erasing everything but the median colour. The largest `k` that passes,
/// with every smaller `k` also passing, is the answer. The search stops once the small
/// image would be under 8 px on a side.
///
/// The second test is there for sparse rasters. The first is a mean over the whole image,
/// and a lone small shape is too small a share of it to fail it even when the round trip
/// erases the shape (see [`OVERSAMPLE_KEEP`] for the case and the margins). It only ever
/// lowers the answer, and on a flat image both errors are 0 and the answer is unchanged.
/// [`flat_error`] is computed once, when the first `k` passes the absolute test.
///
/// Returns 1 for most native renders at the corpus's 128 px (212 of the 246 screen icons;
/// the caller does not scale at or below 128 px anyway, see `inkvec-cli`'s
/// `price_in_raster_units`); at 512 px nearly every native render reads 2 to 8, which is
/// the speckle floor scaling that caller wants. Also 1 for anything under 16x16 or a
/// buffer shorter than `width * height`.
pub fn oversample_factor(rgb: &[[f32; 3]], width: usize, height: usize) -> usize {
    if width < 16 || height < 16 || rgb.len() < width * height {
        return 1;
    }
    let mut flat: Option<f64> = None;
    let mut best = 1usize;
    for k in [2usize, 4, 8] {
        let (sw, sh) = (width / k, height / k);
        if sw < 8 || sh < 8 {
            break;
        }
        // Box down, bilinear back, and compare against what we started with.
        let mut small = vec![[0.0f32; 3]; sw * sh];
        for y in 0..sh {
            for x in 0..sw {
                let mut acc = [0.0f64; 3];
                for dy in 0..k {
                    for dx in 0..k {
                        let p = rgb[(y * k + dy) * width + (x * k + dx)];
                        for c in 0..3 {
                            acc[c] += p[c] as f64;
                        }
                    }
                }
                let n = (k * k) as f64;
                small[y * sw + x] = [
                    (acc[0] / n) as f32,
                    (acc[1] / n) as f32,
                    (acc[2] / n) as f32,
                ];
            }
        }
        let mut err = 0.0f64;
        for y in 0..height {
            for x in 0..width {
                // Bilinear sample of `small` at this pixel's centre.
                let fx = ((x as f64 + 0.5) / k as f64 - 0.5).clamp(0.0, sw as f64 - 1.0);
                let fy = ((y as f64 + 0.5) / k as f64 - 0.5).clamp(0.0, sh as f64 - 1.0);
                let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
                let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
                let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
                for c in 0..3 {
                    let a = small[y0 * sw + x0][c] as f64 * (1.0 - tx)
                        + small[y0 * sw + x1][c] as f64 * tx;
                    let b = small[y1 * sw + x0][c] as f64 * (1.0 - tx)
                        + small[y1 * sw + x1][c] as f64 * tx;
                    err += (a * (1.0 - ty) + b * ty - rgb[y * width + x][c] as f64).abs();
                }
            }
        }
        err = err * 255.0 / (width * height * 3) as f64;
        if err >= OVERSAMPLE_TOL {
            break;
        }
        // Lost more than half of what there was to lose: the mean above was diluted.
        let flat = *flat.get_or_insert_with(|| flat_error(rgb, width, height));
        if err > OVERSAMPLE_KEEP * flat {
            break;
        }
        best = k;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A black disc of `area` px² centred on an `n × n` white canvas, its rim a linear
    /// ramp `ramp` px wide: 1 for a native render, `k` for one upscaled `k` times.
    fn disc_on_white(n: usize, area: f64, ramp: f64) -> Vec<[f32; 3]> {
        let r = (area / std::f64::consts::PI).sqrt();
        let c = n as f64 / 2.0;
        (0..n * n)
            .map(|i| {
                let d = ((i % n) as f64 - c).hypot((i / n) as f64 - c);
                let v = 1.0 - ((r + ramp / 2.0 - d) / ramp).clamp(0.0, 1.0);
                [v as f32; 3]
            })
            .collect()
    }

    /// The 2026-10-02 repro: a native 38 px² disc on a 144 px canvas. Erasing it costs
    /// 0.47 levels over the whole image, under `OVERSAMPLE_TOL` at every factor, so it
    /// read as 8x oversampled and the intake raised the speckle floor 64-fold, past the
    /// disc. The round trip loses 35 % of the image's detail at /2, 77 % at /4.
    #[test]
    fn a_lone_small_shape_is_not_read_as_eight_times_oversampled() {
        let img = disc_on_white(144, 38.0, 1.0);
        assert!(oversample_factor(&img, 144, 144) <= 2);
        // An upscaled drawing still reads as one: the same kind of disc upscaled 4x.
        let up = disc_on_white(144, 38.0 * 16.0, 4.0);
        assert!(oversample_factor(&up, 144, 144) >= 4);
        // A flat image has nothing to lose and reads as before, the largest factor.
        assert_eq!(oversample_factor(&vec![[0.3f32; 3]; 64 * 64], 64, 64), 8);
    }

    /// The flat error is the mean absolute deviation from the per-channel median, in
    /// 8-bit levels: one black pixel among three white ones is 255 / 4 per channel.
    #[test]
    fn flat_error_is_the_mean_deviation_from_the_median() {
        let px = [[0.0f32; 3], [1.0; 3], [1.0; 3], [1.0; 3]];
        assert!((flat_error(&px, 2, 2) - 255.0 / 4.0).abs() < 1e-9);
        assert_eq!(flat_error(&[[0.5f32; 3]; 6], 3, 2), 0.0);
        // Even count, two values: any point between the middle two gives the same sum.
        let half = [[0.0f32; 3], [0.0; 3], [1.0; 3], [1.0; 3]];
        assert!((flat_error(&half, 4, 1) - 255.0 / 2.0).abs() < 1e-9);
    }
}
