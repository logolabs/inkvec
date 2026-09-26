//! Multi-stop interior knot fitting for gradients (DESIGN.md S1).
//!
//! Adds interior color stops piecewise-linearly using IRLS (Iteratively Reweighted
//! Least Squares) with a Huber loss to robustly fit non-linear gradient profiles.

use super::{
    fit_cap, from_space, to_lin, FillModel, Interp, Samples, MAX_MID_STOPS, MIN_GRADIENT_PIXELS,
};

impl FillModel {
    /// The gradient coordinate `t` of a position: 0 at the first stop, 1 at the last.
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

    /// The same geometry with the colour profile replaced.
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

/// Rounds of reweighting when a stop count's final profile is fitted.
const IRLS_ROUNDS: usize = 2;

/// How much worse, in median residual, a sliver of samples cut off by a new stop may be
/// fitted than the rest of its segment; see [`fit_mid_stops`].
const MAX_SLIVER_MISFIT: f64 = 4.0;

const KNOT_GRID: usize = 16;

/// Solve the small system `A·x = b` for three right-hand sides at once (Gaussian
/// elimination with partial pivoting). `None` when singular.
fn solve_small(mut a: Vec<Vec<f64>>, mut b: Vec<[f64; 3]>) -> Option<Vec<[f64; 3]>> {
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
) -> (Vec<Vec<f64>>, Vec<[f64; 3]>) {
    let mut diag = vec![0.0; m];
    // `off[j]` couples nodes `j - 1` and `j`.
    let mut off = vec![0.0; m];
    let mut atb = vec![[0.0; 3]; m];
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
    let ata = (0..m)
        .map(|i| {
            let mut row = vec![0.0; m];
            row[i] = diag[i] + 1e-9;
            if i > 0 {
                row[i - 1] = off[i];
            }
            if i + 1 < m {
                row[i + 1] = off[i + 1];
            }
            row
        })
        .collect();
    (ata, atb)
}

/// Robust least squares of a piecewise-linear colour profile in `t` with nodes at
/// `0, knots.., 1` (hat-function basis). Returns the Huber objective — quadratic within
/// `delta` of the fit, linear beyond — and the colour at each node, first to last.
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
        x = solve_small(ata, atb)?;
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
pub(crate) fn fit_mid_stops(
    model: &FillModel,
    s: &Samples,
    cols: &[[f64; 3]],
    space: Interp,
) -> Vec<FillModel> {
    let n = s.len();
    let stride = (n / fit_cap()).max(1);
    let idx: Vec<usize> = (0..n).step_by(stride).collect();
    if idx.len() < 4 * MIN_GRADIENT_PIXELS {
        return Vec::new();
    }
    let t: Vec<f64> = idx.iter().map(|&i| model.t_at(s.x[i], s.y[i])).collect();
    let c: Vec<[f64; 3]> = idx.iter().map(|&i| cols[i]).collect();
    let parent = model.eval();
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
        return Vec::new();
    }
    let (k_lo, k_hi) = (q_lo + 0.05 * span, q_hi - 0.05 * span);
    let mut out = Vec::new();
    let mut knots: Vec<f64> = Vec::new();
    for _ in 0..MAX_MID_STOPS {
        let resid_with = |k: f64| -> f64 {
            let mut ks = knots.clone();
            ks.push(k);
            ks.sort_by(f64::total_cmp);
            fit_piecewise(&c, &t, &ks, &weight, delta, 0).map_or(f64::MAX, |(r, _)| r)
        };
        let mut best = (f64::NAN, f64::MAX);
        let step = (k_hi - k_lo) / KNOT_GRID as f64;
        // The grid's fits are independent, and on a large gradient region they are the
        // stage's longest serial stretch: fit them side by side, then pick in grid order
        // exactly as the one-at-a-time loop did.
        let grid: Vec<f64> = {
            use rayon::prelude::*;
            (0..=KNOT_GRID)
                .into_par_iter()
                .map(|g| resid_with(k_lo + g as f64 * step))
                .collect()
        };
        for (g, &r) in grid.iter().enumerate() {
            let k = k_lo + g as f64 * step;
            if r < best.1 {
                best = (k, r);
            }
        }
        if !best.0.is_finite() {
            break;
        }
        let (mut lo, mut hi) = ((best.0 - step).max(k_lo), (best.0 + step).min(k_hi));
        let phi = 0.5 * (5.0f64.sqrt() - 1.0);
        let (mut a, mut b) = (hi - phi * (hi - lo), lo + phi * (hi - lo));
        let (mut fa, mut fb) = (resid_with(a), resid_with(b));
        for _ in 0..16 {
            if fa < fb {
                hi = b;
                b = a;
                fb = fa;
                a = hi - phi * (hi - lo);
                fa = resid_with(a);
            } else {
                lo = a;
                a = b;
                fa = fb;
                b = lo + phi * (hi - lo);
                fb = resid_with(b);
            }
        }
        let k = 0.5 * (lo + hi);
        if knots.iter().any(|&q| (q - k).abs() < 0.05 * span) {
            break;
        }
        knots.push(k);
        knots.sort_by(f64::total_cmp);
        let Some((_, x)) = fit_piecewise(&c, &t, &knots, &weight, delta, IRLS_ROUNDS) else {
            break;
        };
        let m = x.len();
        let mids: Vec<(f64, [f32; 3])> = knots
            .iter()
            .zip(&x[1..m - 1])
            .map(|(&off, &col)| (off, from_space(col, space)))
            .collect();
        let cand = model.with_stops(from_space(x[0], space), mids, from_space(x[m - 1], space));
        let seg = knots.iter().position(|&q| q == k).unwrap();
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
        for (j, &i) in idx.iter().enumerate() {
            if t[j] < lo_t || t[j] > hi_t {
                continue;
            }
            let p = cand_eval.color_at(s.x[i], s.y[i]);
            let r = (0..3)
                .map(|q| (p[q] - s.srgb[i][q]).powi(2))
                .sum::<f32>()
                .sqrt() as f64;
            resid[usize::from(t[j] >= k)].push(r);
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
        let sliver = resid[short].len() < sorted.len() / 10;
        let (m_short, m_long) = (median(&mut resid[short]), median(&mut resid[1 - short]));
        if sliver && m_short > MAX_SLIVER_MISFIT * m_long.max(1.0 / 255.0) {
            break;
        }
        out.push(cand);
    }
    out
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
            }
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
}
