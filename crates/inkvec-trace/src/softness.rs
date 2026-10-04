//! Is this raster soft -- resampled, upscaled or blurred -- and by how much?
//!
//! The soft intake in `inkvec-cli` (`soft_intake.rs`) reduces a raster that carries its
//! drawing on more pixels than the drawing needs back to the detail it carries. This module
//! is its measurement; it decides nothing.
//!
//! # The problem
//!
//! [`crate::coverage::intake_scale`] estimates edge width as the median ratio of first to
//! second differences, and that estimator is blind where it matters most: on a *linear*
//! ramp the second difference vanishes, the pixel is skipped, and a bilinear 2x upscale
//! reads exactly 1.00. A bicubic 4x reads about 2.4. Both are far from the truth.
//!
//! # What is measured, in one pass over every row and every column
//!
//! A *transition* is a maximal monotone run of first differences above [`FLAT_EPS`] whose
//! summed rise reaches [`MIN_CONTRAST`] (a strong edge, not texture). For each:
//!
//! * **its spread** ([`RampEvidence::soft_fraction`], [`RampEvidence::width`]), when it runs
//!   between two flat stretches ([`PLATEAU`] pixels either side moving by at most
//!   [`PLATEAU_SLACK`] of the contrast) and its Sobel gradient points along the scan (within
//!   about 23 degrees, `cos ≥ COS_MIN`, so the profile is the edge's own cross-section,
//!   corrected by `cos²`). The spread is the second moment of the transition's first
//!   differences about their centroid. A native box-filtered render puts an edge into at
//!   most one partial pixel, so its differences sit on two neighbouring taps with weights
//!   `c` and `1 − c`: a variance of `c(1 − c)`, never above 1/4, and 1/6 on average over edge
//!   phase. Resampling from a smaller raster, or blurring, spreads *every* edge wider.
//! * **its core** ([`RampEvidence::sharp_fraction`]), for every strong transition that
//!   points along the scan, plateaus or not: the differences around the largest one that
//!   are at least [`CORE_SHARE`] of it, and their second moment. A native edge's core is its
//!   one or two taps, variance at most 1/4 ([`NATIVE_EDGE_VAR`]); on a resampled raster
//!   every edge's core is wide too. This is what tells a *blurred raster* from a *sharp
//!   drawing with a blurred element* -- the glow or drop shadow round crisp artwork. The
//!   spread above only reads plateau-to-plateau transitions, and the artwork's own edges,
//!   which run into the glow's ramp on one side, never qualify: on the r2-inputs glow set
//!   98 % of the qualifying transitions were the glow's (5.97 px wide), and the first soft
//!   intake reduced a native 512 px render with a glow to 64 px (dE00 0.690 -> 1.013).
//!   Measured with the core (r2-inputs stress set, 28 sources per group): every glow and
//!   shadow image reads a sharp fraction of 0.80 or more, every bicubic 4x, bicubic 2x and
//!   Lanczos 3x upscale 0.20 or less, every clean render 1.00.
//! * **the distance to the previous opposite transition** along the line
//!   ([`RampEvidence::feature_p10`]), for every strong transition: the width of a stroke or
//!   a gap, which a reduction must not squeeze out.
//!
//! [`grid_contrast`] and [`shading_share`] answer two narrower questions the soft intake
//! asks before it chooses a factor: whether the raster repeats with an upscale's period, and
//! whether its flat areas are actually shaded.
//!
//! # Literature
//!
//! Inspired by: P. Marziliano, F. Dufaux, S. Winkler, T. Ebrahimi, "A no-reference
//! perceptual blur metric", Proc. ICIP 2002, DOI 10.1109/ICIP.2002.1038902: blur read as the
//! width of edges along scan lines. Ours differs in three ways, each for a measured reason:
//! the width is a second moment of the difference profile rather than the distance between
//! the extrema either side (an upscale's ramps are long and flat-topped, and the moment
//! normalises to a box-filter width); only strong, isolated, near-axis edges count (texture
//! and corners are not cross-sections); and the decision is on the *share* of edges wider
//! than any native render makes, not on a mean, because soft artwork (noto emoji drawn with
//! blurred shading) has a wide mean too, with only some of its edges soft. The core test is
//! not from the literature: a global edge-width statistic cannot separate a blurred image
//! from a sharp image with blurred elements, which is the known weakness of that family of
//! metrics, and the core is the smallest measurement that does on the stress set.
//!
//! Ported from the unmerged `engine/soft-input` branch (2026-09-25) by way of the r2-inputs
//! research port (`research2/inputs` f72095d); the core measurement is new here.

/// A first difference below this is flat (two 8-bit levels).
const FLAT_EPS: f64 = 2.0 / 255.0;
/// A transition must rise at least this far (of 1.0) to count as a strong edge.
const MIN_CONTRAST: f64 = 0.2;
/// Pixels either side of the transition that must stay (nearly) flat for its spread to be
/// measured.
const PLATEAU: usize = 3;
/// How much of the contrast the plateaus may still move by (bicubic overshoot, noise).
const PLATEAU_SLACK: f64 = 0.12;
/// Least cosine between the gradient and the scan axis (about 23 degrees).
const COS_MIN: f64 = 0.92;
/// A transition whose spread variance exceeds this is wider than any native render can make
/// one: a single partial pixel gives at most 0.25.
pub const SOFT_EDGE_VAR: f64 = 0.3;
/// A core whose variance is at most this is a native edge: one partial pixel, `c(1 − c)`.
pub const NATIVE_EDGE_VAR: f64 = 0.25;
/// The differences of a transition's core are those at least this share of its largest.
/// A quarter keeps a bilinear 2x ramp's `[1/4, 1/2, 1/4]` whole (variance 1/2) and drops the
/// long, shallow tail a glow adds behind a crisp edge.
const CORE_SHARE: f64 = 0.25;
/// Fewer measured edges than this and nothing is claimed.
pub const MIN_EDGES: usize = 16;
/// At least this share of the measured edges wider than native and the raster is soft:
/// resampled or blurred. Over the corpus -- 1406 icons at 128/128ss, the 246 screen icons at
/// 256/512/1024, 14 brand wordmarks at 256 to 2048 -- native renders reach at most 0.18;
/// every resampled or blurred variant measured (blur sigma 0.7 to 2, bilinear 2x, bicubic
/// 2x/3x/4x/8x, Lanczos 4x) reads 1.00, because resampling softens every edge alike; JPEG
/// reads at most 0.67 (`engine/soft-input`, 2026-09-25).
pub const SOFT_FRACTION_GATE: f64 = 0.9;
/// Share of the spread variances dropped from the top before averaging them into a width:
/// features that are not isolated steps (a stroke meeting another, a corner, a gradient).
const TRIM_TOP: f64 = 0.2;

/// What the edge ramps say about the intake. See the module documentation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RampEvidence {
    /// Plateau-to-plateau, near-axis strong edges whose spread was measured.
    pub edges: usize,
    /// Share of them wider than a native render can produce, in `[0, 1]`.
    pub soft_fraction: f64,
    /// Box-filter-equivalent edge width of those edges, in pixels; 1.0 on a native render
    /// (median 0.96, max 1.14 over the corpus), about 2.1 for a bilinear 2x or a blur of
    /// sigma 1, 3.3 for a bicubic 4x.
    pub width: f64,
    /// The 10th percentile of the distance, along a row or column, between the centroids of
    /// two consecutive strong transitions of opposite sign: the width of the thinnest strokes
    /// and gaps, in pixels. Infinite when fewer than [`MIN_EDGES`] were seen.
    pub feature_p10: f64,
    /// Near-axis strong transitions whose core was measured, plateaus or not.
    pub cores: usize,
    /// Share of those whose core is native-sharp (variance at most [`NATIVE_EDGE_VAR`]), in
    /// `[0, 1]`; 0 when none was measured.
    pub sharp_fraction: f64,
}

impl RampEvidence {
    /// Nothing measured: a native-looking verdict with no sharp edge either.
    pub const NONE: RampEvidence = RampEvidence {
        edges: 0,
        soft_fraction: 0.0,
        width: 1.0,
        feature_p10: f64::INFINITY,
        cores: 0,
        sharp_fraction: 0.0,
    };
}

/// Rec. 709 luma of an sRGB-encoded pixel, `0.2126 R + 0.7152 G + 0.0722 B`, in `[0, 1]`:
/// the scalar every measurement here walks.
fn luma(c: &[f32; 3]) -> f64 {
    0.2126 * c[0] as f64 + 0.7152 * c[1] as f64 + 0.0722 * c[2] as f64
}

/// Measure the edge ramps of an RGB raster (straight colour composited over a background,
/// row-major `width × height`). O(pixels): one luma buffer, then every row and every column
/// walked once ([`scan_profile`]), with a Sobel gradient evaluated only at transition centres.
/// A raster under `2·PLATEAU + 4` on either side, or a short buffer, gives
/// [`RampEvidence::NONE`].
pub fn ramp_evidence(rgb: &[[f32; 3]], width: usize, height: usize) -> RampEvidence {
    if width < 2 * PLATEAU + 4 || height < 2 * PLATEAU + 4 || rgb.len() < width * height {
        return RampEvidence::NONE;
    }
    let lum: Vec<f64> = rgb[..width * height].iter().map(luma).collect();
    let mut acc = Scan::default();
    let mut profile: Vec<f64> = Vec::with_capacity(width.max(height));
    for axis in [Axis::Rows, Axis::Columns] {
        let (n_outer, n_inner) = match axis {
            Axis::Rows => (height, width),
            Axis::Columns => (width, height),
        };
        for o in 0..n_outer {
            profile.clear();
            match axis {
                Axis::Rows => profile.extend_from_slice(&lum[o * width..(o + 1) * width]),
                Axis::Columns => profile.extend((0..n_inner).map(|k| lum[k * width + o])),
            }
            scan_profile(
                &profile,
                |c| {
                    // The transition's centre pixel in image coordinates.
                    let (x, y) = match axis {
                        Axis::Rows => (c, o),
                        Axis::Columns => (o, c),
                    };
                    axis_cosine(&lum, width, height, x, y, axis)
                },
                &mut acc,
            );
        }
    }
    acc.summarise()
}

/// Which lines a scan walks: rows (the gradient's x component is the one along the scan)
/// or columns (y).
#[derive(Debug, Clone, Copy)]
enum Axis {
    Rows,
    Columns,
}

/// Cosine between the Sobel gradient at `(x, y)` and the scan axis, or `None` at the image
/// border, on a vanishing gradient, or when the edge is too oblique (`cos < COS_MIN`) for the
/// scan to read its cross-section.
fn axis_cosine(lum: &[f64], w: usize, h: usize, x: usize, y: usize, axis: Axis) -> Option<f64> {
    if x < 1 || y < 1 || x + 1 >= w || y + 1 >= h {
        return None;
    }
    let p = |xx: usize, yy: usize| lum[yy * w + xx];
    let gx = (p(x + 1, y - 1) + 2.0 * p(x + 1, y) + p(x + 1, y + 1))
        - (p(x - 1, y - 1) + 2.0 * p(x - 1, y) + p(x - 1, y + 1));
    let gy = (p(x - 1, y + 1) + 2.0 * p(x, y + 1) + p(x + 1, y + 1))
        - (p(x - 1, y - 1) + 2.0 * p(x, y - 1) + p(x + 1, y - 1));
    let g = gx.hypot(gy);
    if g < 1e-9 {
        return None;
    }
    let along = match axis {
        Axis::Rows => gx.abs(),
        Axis::Columns => gy.abs(),
    };
    let cos = along / g;
    (cos >= COS_MIN).then_some(cos)
}

/// What the scans collect: spread variances (plateau-to-plateau edges), core variances (all
/// near-axis strong edges) and opposite-transition spacings, all `cos²`-corrected where an
/// orientation applies.
#[derive(Debug, Default)]
struct Scan {
    spreads: Vec<f64>,
    cores: Vec<f64>,
    spacings: Vec<f64>,
}

/// Walk one row or column `prof` (luma, one value per pixel) and record every strong
/// transition in `acc`; `orient(c)` gives the cosine to the axis at profile pixel `c`, or
/// `None` to leave the transition's spread and core unmeasured.
///
/// The differences are `d(i) = prof[i+1] − prof[i]`. A transition is `d[s..e]` of one sign
/// with every `|d| > FLAT_EPS`; with `total = Σ |d|`, its centroid is
/// `μ = Σ |d_i|·i / total` and its spread `Σ |d_i|·(i − μ)² / total`. Its centre pixel, at
/// which the gradient is read, is `(s + e)/2 + 1`. O(len).
fn scan_profile(prof: &[f64], mut orient: impl FnMut(usize) -> Option<f64>, acc: &mut Scan) {
    let m = prof.len().saturating_sub(1);
    let d = |i: usize| prof[i + 1] - prof[i];
    // The previous strong transition's centroid and sign.
    let mut last: Option<(f64, f64)> = None;
    let mut k = 0;
    while k < m {
        if d(k).abs() <= FLAT_EPS {
            k += 1;
            continue;
        }
        let sgn = d(k).signum();
        let s = k;
        while k < m && d(k) * sgn > FLAT_EPS {
            k += 1;
        }
        let e = k;
        let total: f64 = (s..e).map(|i| d(i) * sgn).sum();
        if total < MIN_CONTRAST {
            continue;
        }
        let centre: f64 = (s..e).map(|i| d(i) * sgn / total * i as f64).sum();
        if let Some((c0, s0)) = last {
            if s0 != sgn {
                acc.spacings.push(centre - c0);
            }
        }
        last = Some((centre, sgn));
        let Some(cos) = orient((s + e) / 2 + 1) else {
            continue;
        };
        acc.cores.push(core_variance(&d, s, e, sgn) * cos * cos);
        if s < PLATEAU || e + PLATEAU > m {
            continue;
        }
        let pre = (prof[s] - prof[s - PLATEAU]).abs();
        let post = (prof[e + PLATEAU] - prof[e]).abs();
        if pre > PLATEAU_SLACK * total || post > PLATEAU_SLACK * total {
            continue;
        }
        let spread: f64 = (s..e)
            .map(|i| d(i) * sgn / total * (i as f64 - centre).powi(2))
            .sum();
        acc.spreads.push(spread * cos * cos);
    }
}

/// The core of the transition `d[s..e]` (sign `sgn`): from its largest difference `j`,
/// extend left and right while the differences are at least [`CORE_SHARE`] of `|d_j|`, and
/// return the second moment of that window about its own centroid (as for the spread).
/// Ties for the largest go to the first. O(e − s).
fn core_variance(d: &impl Fn(usize) -> f64, s: usize, e: usize, sgn: f64) -> f64 {
    let mut j = s;
    for i in s..e {
        if d(i) * sgn > d(j) * sgn {
            j = i;
        }
    }
    let floor = CORE_SHARE * d(j) * sgn;
    let mut a = j;
    while a > s && d(a - 1) * sgn >= floor {
        a -= 1;
    }
    let mut b = j + 1;
    while b < e && d(b) * sgn >= floor {
        b += 1;
    }
    let total: f64 = (a..b).map(|i| d(i) * sgn).sum();
    let mu: f64 = (a..b).map(|i| d(i) * sgn / total * i as f64).sum();
    (a..b)
        .map(|i| d(i) * sgn / total * (i as f64 - mu).powi(2))
        .sum()
}

impl Scan {
    /// The evidence: counts, the soft and sharp shares, the trimmed-mean width and the 10th
    /// percentile spacing. Below [`MIN_EDGES`] spreads the spread fields are
    /// [`RampEvidence::NONE`]'s; the core fields are reported whatever their number.
    fn summarise(mut self) -> RampEvidence {
        let cores = self.cores.len();
        let sharp_fraction = if cores == 0 {
            0.0
        } else {
            self.cores.iter().filter(|&&v| v <= NATIVE_EDGE_VAR).count() as f64 / cores as f64
        };
        let n = self.spreads.len();
        if n < MIN_EDGES {
            return RampEvidence {
                edges: n,
                cores,
                sharp_fraction,
                ..RampEvidence::NONE
            };
        }
        let feature_p10 = if self.spacings.len() < MIN_EDGES {
            f64::INFINITY
        } else {
            let i = self.spacings.len() / 10;
            *self.spacings.select_nth_unstable_by(i, f64::total_cmp).1
        };
        let soft = self.spreads.iter().filter(|&&v| v > SOFT_EDGE_VAR).count();
        self.spreads.sort_by(f64::total_cmp);
        let keep = (((n as f64) * (1.0 - TRIM_TOP)).ceil() as usize).clamp(1, n);
        let mean = self.spreads[..keep].iter().sum::<f64>() / keep as f64;
        // A box of width `s` pixels, seen through the pixel's own box and one difference,
        // spreads a step with variance (s² + 1)/12 plus a phase term; the native average is
        // 1/6. Normalised so that 1/6 reads exactly 1.
        let width = ((12.0 * mean + 1.0) / 3.0).sqrt();
        RampEvidence {
            edges: n,
            soft_fraction: soft as f64 / n as f64,
            width,
            feature_p10,
            cores,
            sharp_fraction,
        }
    }
}

/// How strongly the raster's second differences repeat with period `k` pixels, along the
/// weaker of the two axes: the spread `(max − min)/mean` of the per-phase means of
/// `|second difference|`, summed across each column and then each row.
///
/// An integer upscale by `k` resamples every source pixel through the same kernel, so the
/// curvature of every edge ramp peaks at the same `k` phases; a native render or a blur has
/// no preferred phase. Measured over 20 screen icons: bicubic 3x reads 0.77-1.52 at `k = 3`,
/// bicubic 4x 0.68-1.20 at `k = 4`, Lanczos 4x 0.32-0.91, while blurs and 2x upscales stay
/// under 0.39 at `k = 3` (`engine/soft-input`). At `k = 2` the kernel is symmetric about the
/// half pixel and nothing shows, so a 2x upscale cannot be recognised this way.
///
/// Inspired by: A. C. Popescu, H. Farid, "Exposing Digital Forgeries by Detecting Traces of
/// Resampling", IEEE Trans. Signal Processing 53(2):758–767, 2005, DOI
/// 10.1109/TSP.2004.839932 -- resampling leaves periodic correlations between neighbouring
/// pixels. They estimate the correlation by expectation-maximisation and read its period
/// from a spectrum; here the period is known up to a few candidates (the edge width says
/// roughly which), so a per-phase mean of the second difference is enough to compare them.
///
/// 0 when `k < 2`, the raster is under `4k` on a side, or there is no curvature at all.
/// O(pixels). The phase counter wraps by comparison, not `%` (wazero's arm64 compiler
/// miscompiled `i32.rem_u` in a hot loop; the Go binding runs this crate as WebAssembly).
pub fn grid_contrast(rgb: &[[f32; 3]], width: usize, height: usize, k: usize) -> f64 {
    if k < 2 || width < 4 * k || height < 4 * k || rgb.len() < width * height {
        return 0.0;
    }
    let lum = |i: usize| luma(&rgb[i]);
    let mut worst = f64::INFINITY;
    for axis in [Axis::Rows, Axis::Columns] {
        let (n_along, n_across) = match axis {
            Axis::Rows => (width, height),
            Axis::Columns => (height, width),
        };
        let at = |a: usize, c: usize| match axis {
            Axis::Rows => c * width + a,
            Axis::Columns => a * width + c,
        };
        let mut phase = vec![0.0f64; k];
        let mut count = vec![0usize; k];
        // `ph` is `a mod k`, advanced with `a`.
        let mut ph = 1;
        for a in 1..n_along - 1 {
            let mut sum = 0.0;
            for c in 0..n_across {
                sum += (lum(at(a + 1, c)) - 2.0 * lum(at(a, c)) + lum(at(a - 1, c))).abs();
            }
            phase[ph] += sum;
            count[ph] += 1;
            ph += 1;
            if ph == k {
                ph = 0;
            }
        }
        let means: Vec<f64> = phase
            .iter()
            .zip(&count)
            .map(|(s, &n)| s / n.max(1) as f64)
            .collect();
        let mean = means.iter().sum::<f64>() / k as f64;
        if mean <= 1e-12 {
            return 0.0;
        }
        let (lo, hi) = means
            .iter()
            .fold((f64::INFINITY, 0.0f64), |(l, h), &m| (l.min(m), h.max(m)));
        worst = worst.min((hi - lo) / mean);
    }
    worst
}

/// Share of the raster, away from its strong edges, that is smoothly shaded rather than
/// flat: among the pixels more than `reach` pixels (chessboard distance) from any pixel
/// whose luma gradient exceeds 12 levels per pixel, the share whose own gradient exceeds 0.4
/// levels per pixel. Gradients are central differences, `√(g_x² + g_y²)` in 8-bit levels,
/// zero on the border.
///
/// Resampling and blur only touch the neighbourhood of an edge, so on flat artwork this is
/// zero however soft the edges are; shading -- a gradient fill, an airbrushed highlight --
/// raises it. Measured on the degraded benchmark with `reach` three times the reduction:
/// every lucide, material and simple-icons input reads 0.000, openmoji at most 0.016, noto
/// emoji 0.009 to 0.22 (`engine/soft-input`). Not from the literature: a flat-area test for
/// this tracer's one question (would a reduction cost shading it then has to refit); see
/// also the edge-masked noise estimators the palette uses (`color::estimate_noise`).
///
/// 0 when fewer than 100 pixels are that far from an edge, or the raster is under 3 px on a
/// side. O(pixels): the "near an edge" mask is the strong mask dilated by a square, as two
/// running-count passes.
pub fn shading_share(rgb: &[[f32; 3]], width: usize, height: usize, reach: usize) -> f64 {
    const STRONG: f64 = 12.0;
    const FAINT: f64 = 0.4;
    let (w, h) = (width, height);
    if w < 3 || h < 3 || rgb.len() < w * h {
        return 0.0;
    }
    let lum: Vec<f64> = rgb[..w * h].iter().map(|c| 255.0 * luma(c)).collect();
    let mut grad = vec![0.0f64; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let gx = if x > 0 && x + 1 < w {
                (lum[i + 1] - lum[i - 1]) / 2.0
            } else {
                0.0
            };
            let gy = if y > 0 && y + 1 < h {
                (lum[i + w] - lum[i - w]) / 2.0
            } else {
                0.0
            };
            grad[i] = gx.hypot(gy);
        }
    }
    let strong: Vec<bool> = grad.iter().map(|&g| g > STRONG).collect();
    let near = dilate(&dilate(&strong, w, h, reach, true), w, h, reach, false);
    let (mut far, mut shaded) = (0usize, 0usize);
    for (&g, &n) in grad.iter().zip(&near) {
        if !n {
            far += 1;
            if g > FAINT {
                shaded += 1;
            }
        }
    }
    if far < 100 {
        return 0.0;
    }
    shaded as f64 / far as f64
}

/// `mask` dilated by `reach` pixels along rows (`along_x`) or columns: `out[k]` is set when
/// any of `mask[k − reach ..= k + reach]` on the same line is. A running count per line.
fn dilate(mask: &[bool], w: usize, h: usize, reach: usize, along_x: bool) -> Vec<bool> {
    let (n_line, len) = if along_x { (h, w) } else { (w, h) };
    let at = |l: usize, k: usize| if along_x { l * w + k } else { k * w + l };
    let mut out = vec![false; w * h];
    for l in 0..n_line {
        let mut count = 0usize;
        // The window [k − reach, k + reach]; seeded with its part right of k = 0.
        for k in 0..reach.min(len) {
            count += mask[at(l, k)] as usize;
        }
        for k in 0..len {
            if k + reach < len {
                count += mask[at(l, k + reach)] as usize;
            }
            if k > reach {
                count -= mask[at(l, k - reach - 1)] as usize;
            }
            out[at(l, k)] = count > 0;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dark disc of radius `r` at the centre of a white `size²` raster, box-filtered with
    /// `ss`x supersampling: a native render.
    fn disc(size: usize, r: f64, ss: usize) -> Vec<[f32; 3]> {
        let c = size as f64 / 2.0;
        let mut out = vec![[1.0f32; 3]; size * size];
        for y in 0..size {
            for x in 0..size {
                let mut cov = 0.0;
                for sy in 0..ss {
                    for sx in 0..ss {
                        let px = x as f64 + (sx as f64 + 0.5) / ss as f64 - c;
                        let py = y as f64 + (sy as f64 + 0.5) / ss as f64 - c;
                        if px * px + py * py < r * r {
                            cov += 1.0;
                        }
                    }
                }
                out[y * size + x] = [(1.0 - cov / (ss * ss) as f64) as f32; 3];
            }
        }
        out
    }

    /// Bilinear upscale of an `n²` raster by an integer factor.
    fn bilinear_up(src: &[[f32; 3]], n: usize, k: usize) -> Vec<[f32; 3]> {
        let m = n * k;
        let mut out = vec![[0.0f32; 3]; m * m];
        for y in 0..m {
            for x in 0..m {
                let fx = ((x as f64 + 0.5) / k as f64 - 0.5).clamp(0.0, n as f64 - 1.0);
                let fy = ((y as f64 + 0.5) / k as f64 - 0.5).clamp(0.0, n as f64 - 1.0);
                let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
                let (x1, y1) = ((x0 + 1).min(n - 1), (y0 + 1).min(n - 1));
                let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
                let v = |xx: usize, yy: usize| src[yy * n + xx][0] as f64;
                let a = v(x0, y0) * (1.0 - tx) + v(x1, y0) * tx;
                let b = v(x0, y1) * (1.0 - tx) + v(x1, y1) * tx;
                out[y * m + x] = [(a * (1.0 - ty) + b * ty) as f32; 3];
            }
        }
        out
    }

    /// `img` (an `n²` raster) with a soft halo: a Gaussian-ish blur of its darkness, of
    /// radius `r`, laid under it, so the disc's own edge stays sharp and a wide ramp runs
    /// out from it -- the r2-inputs glow, in grey.
    fn with_glow(img: &[[f32; 3]], n: usize, r: usize) -> Vec<[f32; 3]> {
        let dark: Vec<f64> = img.iter().map(|c| 1.0 - c[0] as f64).collect();
        let mut blur = dark.clone();
        // Three box passes per axis approximate a Gaussian.
        for _ in 0..3 {
            for along_x in [true, false] {
                let src = blur.clone();
                for l in 0..n {
                    for k in 0..n {
                        let (lo, hi) = (k.saturating_sub(r), (k + r).min(n - 1));
                        let s: f64 = (lo..=hi)
                            .map(|t| {
                                if along_x {
                                    src[l * n + t]
                                } else {
                                    src[t * n + l]
                                }
                            })
                            .sum();
                        let i = if along_x { l * n + k } else { k * n + l };
                        blur[i] = s / (hi - lo + 1) as f64;
                    }
                }
            }
        }
        // Black art (coverage `d`) over a grey halo that falls from 0.4 to white.
        blur.iter()
            .zip(&dark)
            .map(|(&g, &d)| [((1.0 - d) * (1.0 - 0.6 * (1.5 * g).min(1.0))) as f32; 3])
            .collect()
    }

    #[test]
    fn a_native_render_reads_native_and_sharp() {
        let img = disc(256, 90.0, 8);
        let e = ramp_evidence(&img, 256, 256);
        assert!(e.edges >= MIN_EDGES, "{e:?}");
        assert!(e.soft_fraction < 0.2, "{e:?}");
        assert!((0.7..1.25).contains(&e.width), "{e:?}");
        assert!(e.sharp_fraction > 0.9, "{e:?}");
    }

    #[test]
    fn a_linear_ramp_upscale_is_caught_and_has_no_sharp_edge() {
        // The case `intake_scale` reads as exactly 1.00: bilinear 2x makes linear ramps.
        let small = disc(128, 45.0, 8);
        let img = bilinear_up(&small, 128, 2);
        let e = ramp_evidence(&img, 256, 256);
        assert!(e.soft_fraction > 0.9, "{e:?}");
        assert!(e.width > 1.6, "{e:?}");
        assert!(e.sharp_fraction < 0.2, "{e:?}");
    }

    #[test]
    fn a_sharp_disc_with_a_glow_keeps_its_sharp_edges() {
        let img = with_glow(&disc(256, 60.0, 8), 256, 6);
        let e = ramp_evidence(&img, 256, 256);
        assert!(e.sharp_fraction > 0.5, "{e:?}");
    }

    #[test]
    fn width_grows_with_the_upscale_factor() {
        let small = disc(64, 22.0, 8);
        let w2 = ramp_evidence(&bilinear_up(&small, 64, 2), 128, 128).width;
        let w4 = ramp_evidence(&bilinear_up(&small, 64, 4), 256, 256).width;
        assert!(w4 > 1.6 * w2, "{w2} {w4}");
    }

    #[test]
    fn a_4x_upscale_repeats_every_4_pixels() {
        let small = disc(64, 22.0, 8);
        let up = bilinear_up(&small, 64, 4);
        assert!(grid_contrast(&up, 256, 256, 4) > 0.5);
        assert!(grid_contrast(&disc(256, 90.0, 8), 256, 256, 4) < 0.5);
        assert_eq!(grid_contrast(&up, 256, 256, 1), 0.0);
        assert_eq!(grid_contrast(&up[..100], 10, 10, 4), 0.0);
    }

    /// The phase counter against `a % k`, the form it replaces.
    #[test]
    fn the_phase_counter_is_a_mod_k() {
        let img = bilinear_up(&disc(40, 13.0, 4), 40, 3);
        for k in 2..9 {
            let reference = {
                let lum = |i: usize| luma(&img[i]);
                let n = 120;
                let mut worst = f64::INFINITY;
                for rows in [true, false] {
                    let at = |a: usize, c: usize| if rows { c * n + a } else { a * n + c };
                    let mut phase = vec![0.0f64; k];
                    let mut count = vec![0usize; k];
                    for a in 1..n - 1 {
                        let s: f64 = (0..n)
                            .map(|c| {
                                (lum(at(a + 1, c)) - 2.0 * lum(at(a, c)) + lum(at(a - 1, c))).abs()
                            })
                            .sum();
                        phase[a % k] += s;
                        count[a % k] += 1;
                    }
                    let means: Vec<f64> = phase
                        .iter()
                        .zip(&count)
                        .map(|(s, &c)| s / c.max(1) as f64)
                        .collect();
                    let mean = means.iter().sum::<f64>() / k as f64;
                    let lo = means.iter().cloned().fold(f64::INFINITY, f64::min);
                    let hi = means.iter().cloned().fold(0.0, f64::max);
                    worst = worst.min((hi - lo) / mean);
                }
                worst
            };
            assert_eq!(grid_contrast(&img, 120, 120, k), reference, "k {k}");
        }
    }

    #[test]
    fn a_soft_edge_is_not_shading_but_a_ramp_fill_is() {
        let small = disc(64, 22.0, 8);
        let up = bilinear_up(&small, 64, 4);
        assert_eq!(shading_share(&up, 256, 256, 12), 0.0);
        // A horizontal ramp across the whole canvas, no edges at all.
        let ramp: Vec<[f32; 3]> = (0..128 * 128)
            .map(|i| [((i % 128) as f32 / 255.0) + 0.2; 3])
            .collect();
        assert!(shading_share(&ramp, 128, 128, 6) > 0.9);
        assert_eq!(shading_share(&ramp[..4], 2, 2, 1), 0.0);
    }

    #[test]
    fn flat_or_tiny_input_claims_nothing() {
        let flat = vec![[0.5f32; 3]; 64 * 64];
        assert_eq!(ramp_evidence(&flat, 64, 64), RampEvidence::NONE);
        assert_eq!(ramp_evidence(&flat[..16], 4, 4), RampEvidence::NONE);
        assert_eq!(ramp_evidence(&flat[..10], 64, 64), RampEvidence::NONE);
    }
}
