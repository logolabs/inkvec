//! The strip reading of a boundary vertex: exact column sums, a histopolated cubic, and
//! the vertex slid along its normal onto that cubic.
//!
//! A pixel's coverage is the area of the shape inside it, so the coverages of a column of
//! pixels, summed from a pixel wholly on one side of a boundary to a pixel wholly on the
//! other, are *exactly* the column mean of the boundary's height, whatever its slope or
//! curvature (`column_sum_eq_average` in `formal/InkvecTheory/InkvecTheory/Coverage.lean`).
//! Four adjacent column means determine a unique cubic (`poly_eq_of_averages`,
//! `Identifiability.lean`: histopolation is unisolvent), and that cubic *is* the boundary
//! whenever the boundary is a cubic, so a vertex slid along its normal onto it lands on the
//! boundary with no normal-direction, threshold or interpolation error. Measured on circles,
//! ellipses, straight edges, rotated squares and regular polygons at 64 px
//! (`bench/theory/inkvec_compare.py`), stage 07's points, every vertex counted including
//! those this declines, lie 0.011 px from the true boundary on average against 0.073 px with
//! the per-vertex probes alone on exact area renders, and 0.016 px against 0.073 px on 8x8
//! supersampled ones (the corpus's renderer).
//!
//! Method from: the partial area effect of A. Trujillo-Pino, K. Krissian,
//! M. Alemán-Flores, D. Santana-Cedrés (2013), *Accurate subpixel edge location based on
//! partial area effect*, Image and Vision Computing 31(1):72-90,
//! <https://doi.org/10.1016/j.imavis.2012.10.005>, which recovers a quadratic edge from
//! three column sums; and the cell-average reconstruction of finite-volume schemes
//! (e.g. P. McCorquodale, P. Colella (2011), *A high-order finite-volume method for
//! conservation laws on locally refined grids*, CAMCoS 6(1),
//! <https://doi.org/10.2140/camcos.2011.6.1>). Adapted: the column window runs from a
//! saturated pixel of one face to a saturated pixel of the other (the flanks' own values
//! are summed, which keeps the sum unbiased under noise); the cubic is histopolated from
//! four columns and checked against two more, which is what refuses a corner; and the
//! vertex keeps its lattice normal and only chooses where along it to sit, so the edge
//! keeps its points.

use inkvec_core::Point;
use inkvec_verified::generated::strip as generated;
use inkvec_verified::iv::Iv;

use super::{RefineCtx, Source, UnmixAxis};
use crate::gradient::FillModel;

/// Pixels either side of the vertex searched along a column for the window's flanks. A
/// boundary within 45 degrees of the column's normal crosses at most two pixels of it, so
/// three reach both flanks from any vertex on it.
const REACH: isize = 3;

/// Coverage within this of 0 or 1 makes a pixel a flank: wholly one face.
const SATURATED: f64 = 0.06;

/// Largest disagreement, in px, at the vertex's pixel border between the cubic through the
/// four central columns and either one-sided cubic (columns -3..=0 or -1..=2). Every cubic
/// boundary makes all three agree exactly; a corner within reach does not. Each one-sided
/// difference carries 0.7 times the noise of one column mean, so on 8x8 supersampled
/// renders, whose column means scatter by a few hundredths of a pixel, the test still
/// passes a smooth boundary.
const SIDE_TOL: f64 = 0.05;

/// The one-sided test in standard deviations of its own noise, on a soft intake only:
/// `|D|/12` exceeds `SIDE_Z·√(70 V)/12` (`fourthDiff_side`: the one-sided difference is the
/// fourth difference of the six column means over 12, whose variance is `70 V` for column
/// means of variance `V`). It replaces [`SIDE_TOL`] where the intake's noise makes it the
/// larger, so a noisy intake does not have every smooth vertex declined as a corner; the
/// evidence threshold moves, the certificate does not.
///
/// Only on an intake the front end found soft or lossy (`RefineCtx::soft`). On a clean render
/// a low-contrast edge (shading) makes `σ/|d|` large too, and there `SIDE_TOL` stays: it is
/// what the clean tiers were tuned on, and they keep their bytes.
const SIDE_Z: f64 = 3.0;

/// Largest difference, in px, between adjacent column means: the boundary may be at most
/// this steep across the scan, about 56 degrees.
const MAX_STEP: f64 = 1.5;

/// The projection of pixel `(x, y)` onto the vertex's unmixing axis, *unclamped*, and the
/// squared distance of its colour from the line through the two faces' colours (how far the
/// pixel is from being a mixture of the two). Unclamped because the column sums must stay
/// unbiased under noise. Read straight from the pixel: the strip reading only ever asks at
/// pixel centres, where the bilinear sampler the probes use returns the pixel itself. The
/// caller keeps `(x, y)` inside the image.
fn unmix(axis: &UnmixAxis, src: &Source, x: usize, y: usize) -> (f64, f64) {
    let (cb, d) = (axis.cb, axis.d);
    let p = src.rgb[y * src.w + x];
    let q = [p[0] - cb[0], p[1] - cb[1], p[2] - cb[2]];
    let a = (q[0] * d[0] + q[1] * d[1] + q[2] * d[2]) as f64 / axis.dd;
    let r2: f64 = (0..3)
        .map(|c| {
            let e = q[c] as f64 - a * d[c] as f64;
            e * e
        })
        .sum();
    (a, r2)
}

/// The column mean of the boundary's height across one column (or row, `transpose`), whether
/// the vertex's left face (coverage 1) lies on the low-index side, and how many pixels the
/// window summed.
///
/// `line` is the column's pixel index, `v0` the vertex's height. Pixels within [`REACH`]
/// of `v0` are read through the unmixing axis; the window is the run from a saturated
/// pixel of one face to the nearest saturated pixel of the other with only partial pixels
/// between, and its sum is the boundary's column mean measured from the window's top edge
/// (`column_sum_eq_average`). Of several such windows the one whose mean lies nearest `v0`
/// is taken. `None` when no window exists within reach, when a pixel of it is not a
/// mixture of the two faces (a third colour, a junction), or when the column is outside
/// the image.
fn line_mean(
    ctx: &RefineCtx,
    axis: &UnmixAxis,
    line: isize,
    v0: f64,
    transpose: bool,
) -> Option<(f64, bool, usize)> {
    let (w, h) = (ctx.src.w as isize, ctx.src.h as isize);
    let (n_line, n_along) = if transpose { (h, w) } else { (w, h) };
    if line < 0 || line >= n_line {
        return None;
    }
    let r0 = (v0.floor() as isize - REACH).max(0);
    let r1 = (v0.ceil() as isize + REACH).min(n_along - 1);
    if r1 - r0 < 2 {
        return None;
    }
    // Coverage and kind of every pixel in reach (at most 2·REACH + 2 of them): 1 or 0 for a
    // flank, -1 partial, -2 a pixel that is not a mixture of the two faces.
    let len = (r1 - r0 + 1) as usize;
    let mut a = [0.0f64; 2 * REACH as usize + 2];
    let mut kind = [0i8; 2 * REACH as usize + 2];
    let tol = (4.0 * ctx.sigma_noise)
        .max(0.06 * axis.contrast)
        .max(2.0 / 255.0);
    let tol2 = tol * tol;
    for (k, r) in (r0..=r1).enumerate() {
        let (x, y) = if transpose { (r, line) } else { (line, r) };
        let (cov, resid2) = unmix(axis, &ctx.src, x as usize, y as usize);
        a[k] = cov;
        kind[k] = if resid2 > tol2 {
            -2
        } else if cov >= 1.0 - SATURATED {
            1
        } else if cov <= SATURATED {
            0
        } else {
            -1
        };
    }
    let mut best: Option<(f64, bool, usize)> = None;
    let mut j = 0usize;
    while j < len {
        if kind[j] < 0 {
            j += 1;
            continue;
        }
        let mut k = j + 1;
        while k < len && kind[k] == -1 {
            k += 1;
        }
        if k >= len {
            break;
        }
        if kind[k] >= 0 && kind[k] != kind[j] {
            let top_is_left = kind[j] == 1;
            let sum: f64 = a[j..=k]
                .iter()
                .map(|&c| if top_is_left { c } else { 1.0 - c })
                .sum();
            let mean = (r0 + j as isize) as f64 - 0.5 + sum;
            if best.is_none_or(|(b, _, _)| (mean - v0).abs() < (b - v0).abs()) {
                best = Some((mean, top_is_left, k - j + 1));
            }
        }
        j = k;
    }
    best.filter(|(m, _, _)| (m - v0).abs() <= MAX_STEP)
}

/// The cubic `c₀ + c₁s + c₂s² + c₃s³` whose means over the cells `[-2,-1]`, `[-1,0]`,
/// `[0,1]`, `[1,2]` are `m`: the inverse of the histopolation matrix, exact rationals.
/// Its constant term is the classical fourth-order face value
/// `(7(m₁ + m₂) - (m₀ + m₃))/12`.
///
/// Generated from the Lean definition its exactness theorem is about (`histopolateK`, with
/// `histopolate_cubic` and `strip_reading_exact`), in the operation order it was written in
/// here, so the reading is unchanged bit for bit.
fn histopolate(m: &[f64; 4]) -> [f64; 4] {
    generated::histopolate_f64(m)
}

/// Largest residual, px, the shift's certificate may leave: `q(s₀ + t·n_u) − t·n_v` for the
/// cubic `q` the column sums determine ([`certified`]). Newton stops at steps of 1e-12, so a
/// converged shift is ten thousand times inside it.
const CERT_TOL: f64 = 1e-9;

/// Whether the shift `t` along `(nu, nv)` from `s0` provably puts the vertex on the cubic
/// histopolated from the four central means `m[1..5]`, to [`CERT_TOL`]: the generated
/// interval checker (`stripResidualK`; `stripResidual_on_cubic` says the residual is zero
/// exactly on that cubic), evaluated in outward-rounded intervals, so a `true` holds for the
/// exact real numbers and not only for their floating-point rounding. A refusal sends the
/// vertex to the probes.
fn certified(m: &[f64; 6], s0: f64, nu: f64, nv: f64, t: f64) -> bool {
    let x = [m[1], m[2], m[3], m[4], s0, nu, nv, t];
    let mut ivs = [Iv { lo: 0.0, hi: 0.0 }; 8];
    for (slot, v) in ivs.iter_mut().zip(x) {
        match Iv::point(v) {
            Some(p) => *slot = p,
            None => return false,
        }
    }
    generated::strip_residual_iv(&ivs).is_some_and(|r| r[0].within(CERT_TOL))
}

/// Mean of the cubic `c` over the cell `[k, k+1]`.
#[cfg(test)]
fn cell_mean(c: &[f64; 4], k: f64) -> f64 {
    let p =
        |s: f64| c[0] * s + c[1] * s * s / 2.0 + c[2] * s.powi(3) / 3.0 + c[3] * s.powi(4) / 4.0;
    p(k + 1.0) - p(k)
}

fn eval(c: &[f64; 4], s: f64) -> f64 {
    c[0] + s * (c[1] + s * (c[2] + s * c[3]))
}

fn slope(c: &[f64; 4], s: f64) -> f64 {
    c[1] + s * (2.0 * c[2] + s * 3.0 * c[3])
}

/// The shift along the unit normal `(nx, ny)` that puts vertex `p` on the boundary the
/// column sums around it describe, or `None` where the strip reading does not apply (see
/// the module doc); the caller then falls back to the probe inversion.
///
/// It applies only between two flat fills (`fa`, `fb`): against a gradient the unmixing
/// colours are the model's local prediction, which the probes' root-find tolerates and a
/// sum of coverages does not. Not at a `corner`, not on an alpha axis, and only where the
/// contrast is at least twice the unmixing threshold, as for the probes' step inversion.
/// Measured on the 246-icon gate against the probes alone: dE00 -10.9 % Fast and -4.3 %
/// Quality at 128 px, -8.7 % and -2.3 % at 512 px (`docs/theory/README.md`).
///
/// Columns are scanned first when the normal is nearer vertical than horizontal (the
/// boundary is then a graph over x with slope at most 1 at the vertex), rows first
/// otherwise; when the first reading declines, the other is tried. A lattice normal is
/// quantised to multiples of 45 degrees, so near the diagonal either axis may be the one
/// over which the boundary is a graph across all six lines.
pub(super) fn strip_offset(
    ctx: &RefineCtx,
    axis: &UnmixAxis,
    fa: &FillModel,
    fb: &FillModel,
    corner: bool,
    p: Point,
    (nx, ny): (f64, f64),
) -> Option<f64> {
    if axis.with_alpha
        || corner
        || fa.is_gradient()
        || fb.is_gradient()
        || axis.contrast < 2.0 * ctx.min_contrast
    {
        return None;
    }
    let first = nx.abs() > ny.abs();
    strip_along(ctx, axis, p, nx, ny, first).or_else(|| strip_along(ctx, axis, p, nx, ny, !first))
}

/// [`strip_offset`] reading columns (`transpose` false) or rows.
fn strip_along(
    ctx: &RefineCtx,
    axis: &UnmixAxis,
    p: Point,
    nx: f64,
    ny: f64,
    transpose: bool,
) -> Option<f64> {
    let (pu, pv, nu, nv) = if transpose {
        (p.y, p.x, ny, nx)
    } else {
        (p.x, p.y, nx, ny)
    };
    // The pixel border nearest the vertex: cells k = -3..=2 of the local coordinate
    // s = u - border are the columns whose centres are at border + k + 1/2.
    let border = (pu - 0.5).round() + 0.5;
    let mut means = [0.0f64; 6];
    let mut widest = 0usize;
    let mut orient: Option<bool> = None;
    let why = |r: std::fmt::Arguments| {
        if ctx.debug {
            eprintln!(
                "  [strip] p=({:.2},{:.2}) n=({:.2},{:.2}) declined: {r}",
                p.x, p.y, nx, ny
            );
        }
    };
    // Columns are read from the vertex outwards, each searched around its inner
    // neighbour's mean: at 45 degrees the outer columns' crossings are two to three pixels
    // above or below the vertex.
    for k in [-1isize, 0, -2, 1, -3, 2] {
        let slot = (k + 3) as usize;
        let near = match k {
            -1 | 0 => pv,
            k if k < 0 => pv + means[slot + 1],
            _ => pv + means[slot - 1],
        };
        let line = (border + k as f64 + 0.5).round() as isize;
        let Some((m, o, n)) = line_mean(ctx, axis, line, near, transpose) else {
            why(format_args!("no window in line {k}"));
            return None;
        };
        widest = widest.max(n);
        if *orient.get_or_insert(o) != o {
            why(format_args!("orientation"));
            return None;
        }
        means[slot] = m - pv;
    }
    if means.windows(2).any(|w| (w[1] - w[0]).abs() > MAX_STEP) {
        why(format_args!("step"));
        return None;
    }
    let c = histopolate(&[means[1], means[2], means[3], means[4]]);
    // The one-sided cubics, read at the border s = 0 only (their constant terms are taken
    // in their own coordinates, shifted by one cell).
    let left = eval(&histopolate(&[means[0], means[1], means[2], means[3]]), 1.0);
    let right = eval(
        &histopolate(&[means[2], means[3], means[4], means[5]]),
        -1.0,
    );
    let side_tol = side_tol(ctx.soft, widest, ctx.sigma_noise, axis.dd);
    if (left - c[0]).abs() > side_tol || (right - c[0]).abs() > side_tol {
        why(format_args!("sides {:.3} {:.3}", left - c[0], right - c[0]));
        return None;
    }
    // Newton on q(s0 + t·nu) - t·nv = 0: where the normal line meets the cubic.
    let s0 = pu - border;
    let mut t = 0.0f64;
    for _ in 0..8 {
        let s = s0 + t * nu;
        let f = eval(&c, s) - t * nv;
        let df = slope(&c, s) * nu - nv;
        if df.abs() < 0.3 {
            why(format_args!("newton slope"));
            return None;
        }
        let step = f / df;
        t -= step;
        if step.abs() < 1e-12 {
            break;
        }
    }
    if !(t.is_finite() && t.abs() <= 1.0) {
        why(format_args!("shift {t:.3}"));
        return None;
    }
    if !certified(&means, s0, nu, nv, t) {
        why(format_args!("certificate {t:.4}"));
        return None;
    }
    if ctx.debug {
        eprintln!(
            "  [strip] p=({:.2},{:.2}) n=({:.2},{:.2}) t={t:.4}",
            p.x, p.y, nx, ny
        );
    }
    Some(t)
}

/// The side test's threshold, px: [`SIDE_TOL`] on a clean intake; on a soft one the larger of
/// it and [`SIDE_Z`] standard deviations of the one-sided difference, `√(70 V)/12`. `V` is a
/// column mean's variance: `widest` pixels, each with its weight's variance `σ²/|d|²`
/// (`sigma` the per-channel noise, `dd` the squared colour separation of the two faces).
fn side_tol(soft: bool, widest: usize, sigma: f64, dd: f64) -> f64 {
    if !soft {
        return SIDE_TOL;
    }
    let col_var = widest as f64 * sigma * sigma / dd.max(1e-300);
    SIDE_TOL.max(SIDE_Z * (70.0 * col_var).sqrt() / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clean intake keeps `SIDE_TOL` whatever its noise and contrast; a soft one moves to
    /// three standard deviations of the one-sided difference once that is the larger, and
    /// not before.
    #[test]
    fn the_side_test_scales_with_noise_on_a_soft_intake_only() {
        let (sigma, faint, strong) = (2.0 / 255.0, 0.03f64.powi(2) * 3.0, 3.0);
        assert_eq!(side_tol(false, 4, sigma, faint), SIDE_TOL);
        assert_eq!(side_tol(false, 4, 0.1, 1e-6), SIDE_TOL);
        // Black on white at two levels of noise: the statistical term is far below.
        assert_eq!(side_tol(true, 4, sigma, strong), SIDE_TOL);
        // A faint edge (3 % per channel): sqrt(70 · 4 σ²/dd)/12 · 3.
        let want = 3.0 * (70.0 * 4.0 * sigma * sigma / faint).sqrt() / 12.0;
        assert!(want > SIDE_TOL);
        assert!((side_tol(true, 4, sigma, faint) - want).abs() < 1e-12);
    }

    /// The histopolated cubic reproduces any cubic from its four cell means, and its
    /// constant term is the value at the border (`cubic_point_from_means` and
    /// `poly_eq_of_averages` in the Lean development, checked here on numbers).
    #[test]
    fn histopolation_is_exact_on_cubics() {
        let q = [0.3, -0.7, 0.25, 0.04];
        let m = [
            cell_mean(&q, -2.0),
            cell_mean(&q, -1.0),
            cell_mean(&q, 0.0),
            cell_mean(&q, 1.0),
        ];
        let c = histopolate(&m);
        for (a, b) in c.iter().zip(q.iter()) {
            assert!((a - b).abs() < 1e-12, "{c:?} vs {q:?}");
        }
        assert!((cell_mean(&c, -3.0) - cell_mean(&q, -3.0)).abs() < 1e-12);
        // Shifted one cell, the same means describe the same cubic one unit along.
        let m2 = [
            cell_mean(&q, -3.0),
            cell_mean(&q, -2.0),
            cell_mean(&q, -1.0),
            cell_mean(&q, 0.0),
        ];
        assert!((eval(&histopolate(&m2), 1.0) - eval(&q, 0.0)).abs() < 1e-12);
    }
}
