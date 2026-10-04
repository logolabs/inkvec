//! Multi-stop interior knot fitting for gradients (DESIGN.md S1).
//!
//! Adds interior colour stops piecewise-linearly using IRLS (Iteratively Reweighted
//! Least Squares) with a Huber loss to robustly fit non-linear gradient profiles.
//!
//! Called from [`super::fit_samples`] once per two-stop ramp candidate. Input: the
//! candidate (its geometry fixes each sample's gradient coordinate `t`), the samples and
//! their colours in the candidate's interpolation space. Output: up to
//! [`MAX_MID_STOPS`] variants of the same geometry with one, then two, interior stops;
//! the caller scores each against the plain ramp by MDL, so a stop is only kept when it
//! pays for its [`super::PARAMS_STOP`] parameters.

use super::{
    fit_cap, from_space, to_lin, FillModel, Interp, Samples, MAX_MID_STOPS, MIN_GRADIENT_PIXELS,
};

impl FillModel {
    /// The gradient coordinate `t` of a position: 0 at the first stop, 1 at the last,
    /// padded to `[0, 1]`; always 0 for a flat fill.
    pub(crate) fn t_at(&self, x: f64, y: f64) -> f64 {
        match *self {
            FillModel::Flat(_) => 0.0,
            FillModel::Linear { p0, p1, .. } => super::linear_t(x, y, p0, p1),
            FillModel::Radial {
                c,
                r,
                aspect,
                angle,
                ..
            } => super::radial_t(x, y, c, r, aspect, angle),
        }
    }

    /// The same geometry with the colour profile replaced (sRGB stops, `mids` sorted by
    /// offset). A flat fill is returned unchanged.
    pub(crate) fn with_stops(
        &self,
        c0: [f32; 3],
        mids: Vec<(f64, [f32; 3])>,
        c1: [f32; 3],
    ) -> FillModel {
        let mut m = self.clone();
        match &mut m {
            FillModel::Flat(_) => {}
            FillModel::Linear {
                c0: a,
                c1: b,
                mids: mm,
                ..
            }
            | FillModel::Radial {
                c0: a,
                c1: b,
                mids: mm,
                ..
            } => {
                *a = c0;
                *b = c1;
                *mm = mids;
            }
        }
        m
    }
}

mod knots;

/// Rounds of reweighting when a stop count's final profile is fitted.
const IRLS_ROUNDS: usize = 2;

/// How much worse, in median residual, a sliver of samples cut off by a new stop may be
/// fitted than the rest of its segment; see [`fit_mid_stops`].
const MAX_SLIVER_MISFIT: f64 = 4.0;

/// Most profile nodes a fit has: the two end stops and [`MAX_MID_STOPS`] interior ones.
const MAX_NODES: usize = MAX_MID_STOPS + 2;

/// Passes of the repartition of two interior stops ([`repartition`]): each pass moves each
/// stop to its best offset with the other held. One pass; see `repartition` for what it
/// was measured on.
const REPARTITION_PASSES: usize = 1;

/// A normal-equations matrix on the stack: the leading `n×n` block is used.
type Mat = [[f64; MAX_NODES]; MAX_NODES];
/// Its right-hand sides, one per colour channel.
type Rhs = [[f64; 3]; MAX_NODES];

/// Solve the small system `A·x = b` for three right-hand sides at once (Gaussian
/// elimination with partial pivoting). `None` when singular (a pivot below 1e-12).
/// Only the leading `n×n` block of `a` and the first `n` rows of `b` are read; here `n`
/// is the number of profile nodes, at most four, so a dense solve is cheaper than
/// anything cleverer.
///
/// The matrix lives on the stack: this runs once per candidate knot, tens of thousands
/// of times per image, and the heap `Vec<Vec<f64>>` it used to take cost an allocation
/// per row. The elimination is the same one, operation for operation, so the solution is
/// the same bits. Not from the literature: an allocation removed.
fn solve_small(mut a: Mat, mut b: Rhs, n: usize) -> Option<Vec<[f64; 3]>> {
    for i in 0..n {
        let piv = (i..n).max_by(|&p, &q| a[p][i].abs().total_cmp(&a[q][i].abs()))?;
        if a[piv][i].abs() < 1e-12 {
            return None;
        }
        a.swap(i, piv);
        b.swap(i, piv);
        let (pivot_row, pivot_b) = (a[i], b[i]);
        for r in i + 1..n {
            let f = a[r][i] / pivot_row[i];
            if f == 0.0 {
                continue;
            }
            for c in i..n {
                a[r][c] -= f * pivot_row[c];
            }
            for k in 0..3 {
                b[r][k] -= f * pivot_b[k];
            }
        }
    }
    let mut x = vec![[0.0; 3]; n];
    for i in (0..n).rev() {
        for k in 0..3 {
            let mut s = b[i][k];
            for c in i + 1..n {
                s -= a[i][c] * x[c][k];
            }
            x[i][k] = s / a[i][i];
        }
    }
    Some(x)
}

/// The normal equations of [`fit_piecewise`]'s hat-function basis: `AᵀWA` (tridiagonal,
/// with the 1e-9 ridge on its diagonal) and `AᵀWc`.
///
/// Each entry is summed over the samples in their order, exactly as accumulating
/// straight into the matrix does; the sums of the piece the current sample falls in are
/// only held in locals until the samples move on to another piece. Consecutive samples
/// mostly share a piece, so the accumulation no longer waits on a store and a reload per
/// sample, and not a bit of the result changes. (`AᵀWA` is symmetric, and its two
/// off-diagonal entries per piece were two identical sums: one is summed and mirrored.)
fn normal_equations(
    cols: &[[f64; 3]],
    basis: &[(usize, f64)],
    weight: &[f64],
    m: usize,
) -> (Mat, Rhs) {
    let mut diag = [0.0; MAX_NODES];
    // `off[j]` couples nodes `j - 1` and `j`.
    let mut off = [0.0; MAX_NODES];
    let mut atb: Rhs = [[0.0; 3]; MAX_NODES];
    // The piece whose sums are in the locals; pieces are numbered from 1, so 0 is none.
    let mut cur = 0usize;
    let (mut d0, mut d1, mut o) = (0.0, 0.0, 0.0);
    let (mut b0, mut b1) = ([0.0; 3], [0.0; 3]);
    for ((c, &(j, u)), &wt) in cols.iter().zip(basis).zip(weight) {
        if j != cur {
            if cur != 0 {
                (diag[cur - 1], diag[cur], off[cur]) = (d0, d1, o);
                (atb[cur - 1], atb[cur]) = (b0, b1);
            }
            cur = j;
            (d0, d1, o) = (diag[j - 1], diag[j], off[j]);
            (b0, b1) = (atb[j - 1], atb[j]);
        }
        let (w0, w1) = (1.0 - u, u);
        d0 += wt * w0 * w0;
        o += wt * w0 * w1;
        d1 += wt * w1 * w1;
        for k in 0..3 {
            b0[k] += wt * w0 * c[k];
            b1[k] += wt * w1 * c[k];
        }
    }
    if cur != 0 {
        (diag[cur - 1], diag[cur], off[cur]) = (d0, d1, o);
        (atb[cur - 1], atb[cur]) = (b0, b1);
    }
    let mut ata: Mat = [[0.0; MAX_NODES]; MAX_NODES];
    for (i, row) in ata.iter_mut().enumerate().take(m) {
        row[i] = diag[i] + 1e-9;
        if i > 0 {
            row[i - 1] = off[i];
        }
        if i + 1 < m {
            row[i + 1] = off[i + 1];
        }
    }
    (ata, atb)
}

/// Robust least squares of a piecewise-linear colour profile in `t` with nodes at
/// `0, knots.., 1` (hat-function basis). Returns the Huber objective — quadratic within
/// `delta` of the fit, linear beyond — and the colour at each node, first to last.
///
/// Model: a sample at `t` in the piece `[τ_{j−1}, τ_j]`, at `u = (t − τ_{j−1}) /
/// (τ_j − τ_{j−1})`, is predicted as `(1 − u)·x_{j−1} + u·x_j`, where `x` are the node
/// colours. Each round solves the weighted normal equations `AᵀWA x = AᵀWc`
/// ([`normal_equations`]); after the first, the weights are reset by the Huber rule
/// `w_i = 1` when the residual norm `r_i ≤ δ`, else `δ / r_i` (iteratively reweighted
/// least squares, `rounds` extra rounds). The objective is `Σ_i ρ(r_i)` with
/// `ρ(r) = r²` for `r ≤ δ` and `δ(2r − δ)` beyond. `weight` is the starting weight per
/// sample; `knots` must be sorted and inside `(0, 1)`. `None` when [`solve_small`] meets
/// a pivot below 1e-12; the 1e-9 ridge on the diagonal keeps even a piece with no
/// samples from doing that in practice.
fn fit_piecewise(
    cols: &[[f64; 3]],
    t: &[f64],
    knots: &[f64],
    weight: &[f64],
    delta: f64,
    rounds: usize,
) -> Option<(f64, Vec<[f64; 3]>)> {
    let m = knots.len() + 2;
    let mut nodes = Vec::with_capacity(m);
    nodes.push(0.0);
    nodes.extend_from_slice(knots);
    nodes.push(1.0);
    let basis: Vec<(usize, f64)> = t
        .iter()
        .map(|&ti| {
            let j = nodes.partition_point(|&k| k < ti).clamp(1, m - 1);
            let (lo, hi) = (nodes[j - 1], nodes[j]);
            let u = if hi > lo {
                ((ti - lo) / (hi - lo)).clamp(0.0, 1.0)
            } else {
                1.0
            };
            (j, u)
        })
        .collect();
    let mut weight = std::borrow::Cow::Borrowed(weight);
    let mut x: Vec<[f64; 3]> = Vec::new();
    for round in 0..=rounds {
        if round > 0 {
            for ((c, &(j, u)), wt) in cols.iter().zip(&basis).zip(weight.to_mut().iter_mut()) {
                let r = residual(c, &x[j - 1], &x[j], u);
                *wt = if r > delta { delta / r } else { 1.0 };
            }
        }
        let (ata, atb) = normal_equations(cols, &basis, &weight, m);
        x = solve_small(ata, atb, m)?;
    }
    let mut objective = 0.0;
    for (c, &(j, u)) in cols.iter().zip(&basis) {
        let r = residual(c, &x[j - 1], &x[j], u);
        objective += if r > delta {
            delta * (2.0 * r - delta)
        } else {
            r * r
        };
    }
    return Some((objective, x));

    fn residual(c: &[f64; 3], a: &[f64; 3], b: &[f64; 3], u: f64) -> f64 {
        let mut s = 0.0;
        for k in 0..3 {
            let d = c[k] - (a[k] + (b[k] - a[k]) * u);
            s += d * d;
        }
        s.sqrt()
    }
}

/// Fit multi-stop interior knots for a candidate model.
///
/// The idea: a two-stop ramp is a straight line in colour against the gradient
/// coordinate `t`; an artist's three- or four-stop gradient is a polyline. With the
/// candidate's geometry held fixed, each sample has a `t`, and the question reduces to
/// fitting a piecewise-linear profile with one or two free break points ("knots") to
/// the points `(t_i, c_i)`. Knots are added greedily, one per round: the new knot is the
/// stop offset (a multiple of 1/1000, the precision the SVG carries) found by the exact
/// binned scan of [`knots`] -- a segmented-regression search on moments, reweighted
/// towards the Huber loss -- and the profile is then refitted on the samples with
/// [`IRLS_ROUNDS`] of reweighting.
///
/// Robustness: the Huber threshold is `δ = max(3 · 1.4826 · median_i r_i, 1/255)`, where
/// `r_i` is each sample's colour distance to the two-stop parent and `1.4826·median` is
/// the median absolute deviation scaled to a Gaussian σ. Samples the parent misfits by
/// more than `δ` start down-weighted as `δ / r_i`.
///
/// A round stops the search, keeping what was found so far, when: no knot position gives
/// a finite objective; the new knot lands within 5 % of the `t` span of an existing one;
/// the refit is singular; or the knot cuts off a sliver (under a tenth of the samples)
/// that is fitted more than [`MAX_SLIVER_MISFIT`] times worse, in median residual, than
/// the rest of its segment — a stop bought to explain a feature at one end, not a
/// shading. Knots are confined to the central 90 % of the 2–98 % quantile range of `t`.
///
/// Returns the variants in order of stop count (one stop, then two). Empty when fewer
/// than `4·MIN_GRADIENT_PIXELS` samples survive the stride or the samples span under 0.1
/// of `t`.
pub(crate) fn fit_mid_stops(
    model: &FillModel,
    s: &Samples,
    cols: &[[f64; 3]],
    space: Interp,
) -> Vec<FillModel> {
    let Some(p) = StopProblem::new(model, s, cols, space) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut knots: Vec<f64> = Vec::new();
    for _ in 0..MAX_MID_STOPS {
        let Some(k) = p.best_knot(&knots) else {
            break;
        };
        if knots.iter().any(|&q| (q - k).abs() < 0.05 * p.span) {
            break;
        }
        knots.push(k);
        knots.sort_by(f64::total_cmp);
        if knots.len() == 2 {
            repartition(&p, &mut knots);
        }
        let Some((_, x)) = fit_piecewise(&p.c, &p.t, &knots, &p.weight, p.delta, IRLS_ROUNDS)
        else {
            break;
        };
        let m = x.len();
        let mids: Vec<(f64, [f32; 3])> = knots
            .iter()
            .zip(&x[1..m - 1])
            .map(|(&off, &col)| (off, from_space(col, space)))
            .collect();
        let cand = model.with_stops(from_space(x[0], space), mids, from_space(x[m - 1], space));
        // The knot just added -- or, after a repartition, both knots, since both moved.
        let moved: &[f64] = if knots.len() == 2 { &knots } else { &[k] };
        if moved
            .iter()
            .any(|&q| p.cuts_a_misfit_sliver(&cand, &knots, q))
        {
            break;
        }
        out.push(cand);
    }
    out
}

/// Re-place two greedily found interior stops: each in turn is moved to the offset the
/// exact scan ([`StopProblem::best_knot`]) picks with the other one held, for
/// [`REPARTITION_PASSES`] passes. `knots` holds two offsets, ascending, and still does
/// afterwards; a move that would land within 5 % of the `t` span of the other stop (the
/// distance [`fit_mid_stops`] refuses) is not made.
///
/// Why: the stops are found one at a time, and the first is placed as if it were the only
/// one. On a clamped artist profile -- a flat core and a ramp with two bends -- the lone best
/// knot sits at the steeper bend, and the second then splits the rim, leaving the core
/// fitted by a slope: on `noto-emoji/emoji_u1f36a` at 512 px the body's knots landed at 0.804
/// and 0.923 of a profile that is flat to 0.56, and the core read 0.2 dE00 off over 40 % of
/// the icon. Re-placing the first with the second held finds the core's end.
///
/// Method from: J. Bai (1997), Estimating Multiple Breaks One at a Time, Econometric Theory
/// 13(3):315–352, doi:10.1017/S0266466600005831 -- breaks estimated one at a time, then each
/// re-estimated with the others held (the paper's refinement, which brings the sequential
/// estimates to the limiting distribution of the simultaneous ones). Adapted: one pass, on
/// the exact binned scan and its Huber reweighting, which already prices every offset of a
/// knot given the others. See also: J. Bai, P. Perron (2003), doi:10.1002/jae.659, for the
/// global dynamic-programming search over all breaks at once, which costs `O(G²)` scans of
/// the `G = 1000` offsets here and is not used for that reason.
///
/// Complexity: two more knot searches per two-stop variant, each `O(n + G)` per Huber
/// round.
fn repartition(p: &StopProblem, knots: &mut Vec<f64>) {
    debug_assert_eq!(knots.len(), 2);
    for _ in 0..REPARTITION_PASSES {
        for i in 0..2 {
            let other = knots[1 - i];
            let Some(k) = p.best_knot(&[other]) else {
                continue;
            };
            if (k - other).abs() < 0.05 * p.span {
                continue;
            }
            knots[i] = k;
            knots.sort_by(f64::total_cmp);
        }
    }
}

/// The fixed data of one [`fit_mid_stops`] search: the strided subsample, each sample's
/// gradient coordinate and colour, the Huber threshold and starting weights, and the
/// range new knots may take.
struct StopProblem<'a> {
    /// The region's samples.
    s: &'a Samples,
    /// Indices into `s` of the strided subsample.
    idx: Vec<usize>,
    /// Gradient coordinate `t` of each subsample, under the parent's geometry.
    t: Vec<f64>,
    /// Colour of each subsample in the fitting space.
    c: Vec<[f64; 3]>,
    /// Starting IRLS weight of each subsample.
    weight: Vec<f64>,
    /// Huber threshold `δ`, in fitting-space colour units.
    delta: f64,
    /// Width of the 2–98 % quantile range of `t`.
    span: f64,
    /// Lowest position a knot may take.
    k_lo: f64,
    /// Highest position a knot may take.
    k_hi: f64,
}

impl<'a> StopProblem<'a> {
    /// Build the search for `model`'s samples; `None` when there are too few samples or
    /// too little spread in `t` to place a stop (see [`fit_mid_stops`]).
    fn new(model: &FillModel, s: &'a Samples, cols: &[[f64; 3]], space: Interp) -> Option<Self> {
        let n = s.len();
        let stride = (n / fit_cap()).max(1);
        let idx: Vec<usize> = (0..n).step_by(stride).collect();
        if idx.len() < 4 * MIN_GRADIENT_PIXELS {
            return None;
        }
        let parent = model.eval();
        let t: Vec<f64> = idx.iter().map(|&i| parent.t_at(s.x[i], s.y[i])).collect();
        let c: Vec<[f64; 3]> = idx.iter().map(|&i| cols[i]).collect();
        let parent_r: Vec<f64> = idx
            .iter()
            .map(|&i| {
                let p = parent.color_at(s.x[i], s.y[i]);
                let p = match space {
                    Interp::LinearRgb => to_lin(p),
                    Interp::Srgb => [p[0] as f64, p[1] as f64, p[2] as f64],
                };
                cols[i]
                    .iter()
                    .zip(&p)
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum::<f64>()
                    .sqrt()
            })
            .collect();
        let delta = {
            let mut r = parent_r.clone();
            let mid = r.len() / 2;
            let (_, med, _) = r.select_nth_unstable_by(mid, f64::total_cmp);
            (3.0 * 1.4826 * *med).max(1.0 / 255.0)
        };
        let weight: Vec<f64> = parent_r
            .iter()
            .map(|&r| if r > delta { delta / r } else { 1.0 })
            .collect();
        let mut sorted = t.clone();
        sorted.sort_by(f64::total_cmp);
        let q = |f: f64| sorted[((sorted.len() - 1) as f64 * f).round() as usize];
        let (q_lo, q_hi) = (q(0.02), q(0.98));
        let span = q_hi - q_lo;
        if span < 0.1 {
            return None;
        }
        let (k_lo, k_hi) = (q_lo + 0.05 * span, q_hi - 0.05 * span);
        Some(Self {
            s,
            idx,
            t,
            c,
            weight,
            delta,
            span,
            k_lo,
            k_hi,
        })
    }

    /// Where one more knot, added to `knots`, goes: the stop offset in `[k_lo, k_hi]` (a
    /// multiple of 1/1000) chosen by [`knots::best_knot`], the exact binned scan
    /// reweighted towards the Huber loss. `knots` must hold offsets on that grid, as this
    /// returns them. `None` when the range holds no grid offset or no offset gives a
    /// solvable profile.
    fn best_knot(&self, knots: &[f64]) -> Option<f64> {
        let steps = knots::OFFSET_STEPS as f64;
        let fixed: Vec<usize> = knots
            .iter()
            .map(|&k| (k * steps).round() as usize)
            .collect();
        // A whisker of slack so an end of the range that is itself a grid offset counts.
        let lo = ((self.k_lo * steps - 1e-9).ceil().max(1.0)) as usize;
        let hi = ((self.k_hi * steps + 1e-9).floor() as usize).min(knots::OFFSET_STEPS - 1);
        if lo > hi {
            return None;
        }
        knots::best_knot(&self.t, &self.c, &self.weight, self.delta, &fixed, (lo, hi))
            .map(|j| j as f64 / steps)
    }

    /// Whether the knot `k` (already in the sorted `knots`) cuts its segment into a
    /// sliver that `cand` fits much worse than the rest.
    ///
    /// The segment is `[previous knot, next knot]` (open-ended at the profile's ends).
    /// Its samples are split at `k`, each side's residuals taken as the sRGB distance
    /// between `cand` and the observed colour, and the smaller side is a sliver when it
    /// holds under a tenth of all subsamples. The knot is rejected when the sliver's
    /// median residual exceeds [`MAX_SLIVER_MISFIT`] times the other side's (floored at
    /// 1/255). An empty side has median 0.
    fn cuts_a_misfit_sliver(&self, cand: &FillModel, knots: &[f64], k: f64) -> bool {
        let s = self.s;
        let seg = knots
            .iter()
            .position(|&q| q == k)
            .expect("k was just inserted into knots and is finite");
        let lo_t = if seg == 0 {
            f64::NEG_INFINITY
        } else {
            knots[seg - 1]
        };
        let hi_t = if seg + 1 < knots.len() {
            knots[seg + 1]
        } else {
            f64::INFINITY
        };
        let mut resid: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
        let cand_eval = cand.eval();
        for (j, &i) in self.idx.iter().enumerate() {
            if self.t[j] < lo_t || self.t[j] > hi_t {
                continue;
            }
            let p = cand_eval.color_at(s.x[i], s.y[i]);
            let r = (0..3)
                .map(|q| (p[q] - s.srgb[i][q]).powi(2))
                .sum::<f32>()
                .sqrt() as f64;
            resid[usize::from(self.t[j] >= k)].push(r);
        }
        let median = |v: &mut Vec<f64>| -> f64 {
            if v.is_empty() {
                return 0.0;
            }
            let m = v.len() / 2;
            v.select_nth_unstable_by(m, f64::total_cmp);
            v[m]
        };
        let short = usize::from(resid[1].len() < resid[0].len());
        let sliver = resid[short].len() < self.t.len() / 10;
        let (m_short, m_long) = (median(&mut resid[short]), median(&mut resid[1 - short]));
        sliver && m_short > MAX_SLIVER_MISFIT * m_long.max(1.0 / 255.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fit_piecewise_linear() {
        let t: Vec<f64> = (0..=10).map(|i| i as f64 / 10.0).collect();
        let cols: Vec<[f64; 3]> = t.iter().map(|&x| [x, x * 0.5, 1.0 - x]).collect();
        let weight = vec![1.0; t.len()];
        let knots = [0.5];
        let (obj, x) = fit_piecewise(&cols, &t, &knots, &weight, 1.0, 1).expect("piecewise fit");
        assert!(obj < 1e-6);
        assert_eq!(x.len(), 3);
        assert!((x[0][0] - 0.0).abs() < 1e-4);
        assert!((x[1][0] - 0.5).abs() < 1e-4);
        assert!((x[2][0] - 1.0).abs() < 1e-4);
    }

    /// A small deterministic generator, uniform in `[0, 1)`.
    fn lcg(state: &mut u64) -> f64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (*state >> 11) as f64 / (1u64 << 53) as f64
    }

    #[test]
    fn test_fill_eval_is_color_at_bit_for_bit() {
        let mut st = 7u64;
        let col = |st: &mut u64| [lcg(st) as f32, lcg(st) as f32, lcg(st) as f32];
        for case in 0..48 {
            let interp = if case % 2 == 0 {
                Interp::LinearRgb
            } else {
                Interp::Srgb
            };
            let mut mids: Vec<(f64, [f32; 3])> = (0..case % 3)
                .map(|_| (0.1 + 0.8 * lcg(&mut st), col(&mut st)))
                .collect();
            mids.sort_by(|a, b| a.0.total_cmp(&b.0));
            let (c0, c1) = (col(&mut st), col(&mut st));
            let model = match (case / 2) % 4 {
                0 => FillModel::Linear {
                    p0: (3.0, 4.0),
                    p1: (40.0 * lcg(&mut st), 25.0),
                    c0,
                    c1,
                    interp,
                    mids,
                },
                1 => FillModel::Flat(c0),
                k => FillModel::Radial {
                    c: (20.0, 18.0),
                    r: 5.0 + 20.0 * lcg(&mut st),
                    c0,
                    c1,
                    interp,
                    aspect: if k == 2 { 1.0 } else { 0.4 + lcg(&mut st) },
                    angle: 3.0 * lcg(&mut st),
                    mids,
                },
            };
            let eval = model.eval();
            for _ in 0..200 {
                let (x, y) = (48.0 * lcg(&mut st), 48.0 * lcg(&mut st));
                let (a, b) = (model.color_at(x, y), eval.color_at(x, y));
                assert_eq!(
                    a.map(f32::to_bits),
                    b.map(f32::to_bits),
                    "{model:?} at {x},{y}"
                );
                assert_eq!(
                    model.t_at(x, y).to_bits(),
                    eval.t_at(x, y).to_bits(),
                    "t of {model:?} at {x},{y}"
                );
            }
        }
    }

    /// The heap-allocated solver `solve_small` replaced, verbatim.
    fn solve_small_vec(mut a: Vec<Vec<f64>>, mut b: Vec<[f64; 3]>) -> Option<Vec<[f64; 3]>> {
        let n = b.len();
        for i in 0..n {
            let piv = (i..n).max_by(|&p, &q| a[p][i].abs().total_cmp(&a[q][i].abs()))?;
            if a[piv][i].abs() < 1e-12 {
                return None;
            }
            a.swap(i, piv);
            b.swap(i, piv);
            let (top, rest) = a.split_at_mut(i + 1);
            let (btop, brest) = b.split_at_mut(i + 1);
            let (pivot_row, pivot_b) = (&top[i], &btop[i]);
            for (row, brow) in rest.iter_mut().zip(brest.iter_mut()) {
                let f = row[i] / pivot_row[i];
                if f == 0.0 {
                    continue;
                }
                for (x, &y) in row[i..].iter_mut().zip(&pivot_row[i..]) {
                    *x -= f * y;
                }
                for (x, &y) in brow.iter_mut().zip(pivot_b) {
                    *x -= f * y;
                }
            }
        }
        let mut x = vec![[0.0; 3]; n];
        for i in (0..n).rev() {
            for k in 0..3 {
                let mut s = b[i][k];
                for c in i + 1..n {
                    s -= a[i][c] * x[c][k];
                }
                x[i][k] = s / a[i][i];
            }
        }
        Some(x)
    }

    #[test]
    fn stack_solve_matches_the_heap_solve_bit_for_bit() {
        let mut st = 3u64;
        for case in 0..400 {
            let n = 1 + case % MAX_NODES;
            let mut a: Mat = [[0.0; MAX_NODES]; MAX_NODES];
            let mut b: Rhs = [[0.0; 3]; MAX_NODES];
            for i in 0..n {
                for j in 0..n {
                    // Dense, tridiagonal, pivot-hungry (tiny diagonal) and singular cases.
                    let v = lcg(&mut st) - 0.5;
                    a[i][j] = match case % 4 {
                        0 => v,
                        1 if i.abs_diff(j) > 1 => 0.0,
                        2 if i == j => 1e-14 * v,
                        3 if i + 1 == n => a[0][j],
                        _ => v,
                    };
                }
                b[i] = [lcg(&mut st), lcg(&mut st) - 0.5, 3.0 * lcg(&mut st)];
            }
            let av: Vec<Vec<f64>> = (0..n).map(|i| a[i][..n].to_vec()).collect();
            let bv: Vec<[f64; 3]> = b[..n].to_vec();
            let (x1, x2) = (solve_small(a, b, n), solve_small_vec(av, bv));
            let bits = |x: &Option<Vec<[f64; 3]>>| {
                x.as_ref().map(|v| {
                    v.iter()
                        .flat_map(|r| r.map(f64::to_bits))
                        .collect::<Vec<_>>()
                })
            };
            assert_eq!(bits(&x1), bits(&x2), "case {case}");
        }
    }

    #[test]
    fn test_normal_equations_match_dense_accumulation_bit_for_bit() {
        let mut st = 11u64;
        let m = 4;
        let n = 500;
        let cols: Vec<[f64; 3]> = (0..n)
            .map(|_| [lcg(&mut st), lcg(&mut st), lcg(&mut st)])
            .collect();
        // Runs of one piece, and jumps between arbitrary pieces, as samples in pixel order give.
        let basis: Vec<(usize, f64)> = (0..n)
            .map(|i| {
                (
                    1 + (i / 7 + usize::from(lcg(&mut st) < 0.2)) % (m - 1),
                    lcg(&mut st),
                )
            })
            .collect();
        let weight: Vec<f64> = (0..n).map(|_| 0.2 + lcg(&mut st)).collect();
        let mut ata = vec![vec![0.0; m]; m];
        let mut atb = vec![[0.0; 3]; m];
        for ((c, &(j, u)), &wt) in cols.iter().zip(&basis).zip(&weight) {
            let (w0, w1) = (1.0 - u, u);
            ata[j - 1][j - 1] += wt * w0 * w0;
            ata[j - 1][j] += wt * w0 * w1;
            ata[j][j - 1] += wt * w0 * w1;
            ata[j][j] += wt * w1 * w1;
            for k in 0..3 {
                atb[j - 1][k] += wt * w0 * c[k];
                atb[j][k] += wt * w1 * c[k];
            }
        }
        for (i, row) in ata.iter_mut().enumerate() {
            row[i] += 1e-9;
        }
        let (a2, b2) = normal_equations(&cols, &basis, &weight, m);
        let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        for i in 0..m {
            assert_eq!(bits(&ata[i]), bits(&a2[i]));
            assert_eq!(bits(&atb[i]), bits(&b2[i]));
        }
    }

    /// A clamped profile along a linear axis: flat to `t = 0.55`, then two ramps of
    /// different slope meeting at `t = 0.85`. The first knot found alone sits at the
    /// steeper bend; the repartition moves the other to the core's end, so the two-stop
    /// variant has knots at both bends.
    #[test]
    fn two_stops_land_on_both_bends_of_a_clamped_profile() {
        let (w, h) = (200usize, 6usize);
        let profile = |t: f64| {
            let a = (t - 0.55).clamp(0.0, 0.30) * 0.25;
            let b = (t - 0.85).max(0.0) * 3.0;
            0.80 - a - b
        };
        let mut s = Samples {
            px: vec![],
            x: vec![],
            y: vec![],
            srgb: vec![],
            lin: vec![],
        };
        for y in 0..h {
            for x in 0..w {
                let t = x as f64 / (w - 1) as f64;
                let v = profile(t);
                let c = [v, 0.5 * v, 0.3];
                s.px.push(y * w + x);
                s.x.push(x as f64);
                s.y.push(y as f64);
                s.srgb.push([c[0] as f32, c[1] as f32, c[2] as f32]);
                s.lin.push(to_lin([c[0] as f32, c[1] as f32, c[2] as f32]));
            }
        }
        let model = FillModel::Linear {
            p0: (0.0, 0.0),
            p1: ((w - 1) as f64, 0.0),
            c0: [0.8, 0.4, 0.3],
            c1: [0.2, 0.1, 0.3],
            interp: Interp::Srgb,
            mids: vec![],
        };
        let cols = s.colors(Interp::Srgb);
        let variants = fit_mid_stops(&model, &s, &cols, Interp::Srgb);
        let Some(FillModel::Linear { mids, .. }) = variants.get(1) else {
            panic!("no two-stop variant: {variants:?}");
        };
        let offs: Vec<f64> = mids.iter().map(|m| m.0).collect();
        assert!((offs[0] - 0.55).abs() < 0.02, "knots {offs:?}");
        assert!((offs[1] - 0.85).abs() < 0.02, "knots {offs:?}");
    }
}
