//! The merge pass's residual: how far a candidate cubic lies from the measured points, and
//! how the search asks "is this candidate below the best so far?" without computing all of
//! it.
//!
//! # The quantity
//!
//! For a cubic `B` and the measured run `p_a..=p_b` with uncertainties `σ_a..=σ_b` (px),
//!
//! ```text
//!     χ²_n(B) = Σ_{k=a..=b} (d_k / s_k)²,   d_k = |p_k − B(j*/n)|,   s_k = max(σ_k, 1e-6)
//! ```
//!
//! where `B(j*/n)` is the nearest of the `n + 1` samples `B(0), B(1/n), …, B(1)` (the first
//! one at the smallest squared distance), `d_k` is computed with `hypot`, and the sum is
//! taken in point order. `n` is [`SAMPLES`] (96) for the residual that decides a merge and
//! [`COARSE_SAMPLES`](super::COARSE_SAMPLES) (24) on the search's coarse grid. The nearest
//! *sample*, not the nearest point of the curve: every description in a comparison is
//! scored the same way, so the comparison stays fair, and the search's every decision is a
//! comparison between two such sums.
//!
//! # Who asks what
//!
//! - The merge decision and the search's answer need the value itself: [`chi2`].
//! - The coarse grid and the compass search only ask whether a candidate beats the best
//!   found so far, `χ² < bound`: [`chi2_n_below`] and [`score_below`]. They may answer
//!   "no" with infinity, or with any number at least `bound`; a "yes" comes back as the
//!   exact `χ²_n`, bit for bit, because the winner's value becomes the next bound and, in
//!   the end, the merge's price.
//!
//! # How a "no" is reached cheaply: three layers
//!
//! 1. **Early abandoning** (partial distance elimination, Bei & Gray 1985). The terms are
//!    non-negative, so once a partial sum reaches the bound the whole sum does. The points
//!    are visited middle-first, where a bad candidate is furthest off (the run's ends are
//!    pinned to the curve's ends), and a partial sum taken in that order is compared with
//!    `bound · REORDER_MARGIN` so that it also proves the in-order sum reaches `bound`
//!    (Higham 1993, eq. 2.6). This layer shipped in 0.2.4.
//! 2. **Screening on lower bounds, without `hypot`** (added after 0.2.4). Most candidates are
//!    rejected, so most terms are only ever *compared*, never summed into an answer. The
//!    screen therefore uses a lower bound of each term, `ℓ_k = fl(fl(dist²)/fl(s²)) ·
//!    (1 − 1e-12)`, from the squared distance the nearest-sample search computes anyway.
//!    `hypot` is the expensive part of a term: correctly scaled, it is a library call on
//!    every target, and a software one on wasm32, where it measured about 6.9 times its
//!    native cost per call (r2-qspeed's V8 tick profile on the wasip1 build; 70 % of the
//!    WASM build's `hypot` calls came from this residual).
//! 3. **Exact confirmation**. A candidate that survives the screen is a new best, or
//!    nearly one. Each of its terms is then taken exactly, `(hypot(p_k − q_k)/s_k)²` with
//!    `q_k` the very sample the screen found nearest, and summed in point order: the same
//!    operations, on the same operands, in the same order as [`chi2`].
//!
//! # Why the answers are unchanged, bit for bit
//!
//! Write `T_k` for the exact term as [`chi2`] computes it and `r_k` for the true distance
//! `√(dx² + dy²)` of the rounded differences both share. With `u = 2⁻⁵³`:
//!
//! - `fl(dist²) ≤ r²(1 + 2u)` and `fl(s²) ≥ s²(1 − u)`, so with the division and the
//!   product rounded, `ℓ_k ≤ (r/s)²(1 + 5u)(1 − 1e-12)`;
//! - if `hypot` errs by at most `e` ulp, `T_k ≥ (r/s)²(1 − (2e + 3)u)`. IEEE 754 recommends
//!   a correctly rounded `hypot` and common libms stay within about one ulp, but the
//!   margin does not depend on that: `1e-12` is about 4,500 `u`, so `ℓ_k < T_k` for any
//!   `e` up to about 2,000, as long as every quantity is a normal number;
//! - [`lower_term`] returns `0 ≤ T_k` outside the range where that holds: a quotient
//!   under 1e-290 or over 1e300, an overflowed square, or NaN.
//! - Rounded addition is monotone in each operand, so a partial sum of `ℓ`s never exceeds
//!   the partial sum of `T`s taken in the same order. If the screen's partial sum reaches
//!   `bound · REORDER_MARGIN`, the 0.2.4 middle-first exit would have fired at the same
//!   step or earlier: both return infinity.
//! - If the screen never fires, the exact terms are summed in point order, which is
//!   [`chi2`]'s own value. The 0.2.4 code might instead have exited early, returning
//!   infinity where this returns that value; but its exit proves the value is at least
//!   `bound`, and every caller only tests `value < bound`, so neither answer is taken.
//!
//! So every comparison the search makes comes out the same, the search visits the same
//! candidates, and the cubic and residual it returns are the same bits. Unit tests:
//! `lower_term_never_exceeds_the_exact_term` and the 0.2.4 tests in `super::tests`, which
//! hold the old implementations as references.
//!
//! # Measured
//!
//! r2-qspeed (2026-10-01, the 246-icon screen set, one thread): 7,959 free-cubic searches,
//! 146 million point terms, each a brute-force nearest of 25 or 97 samples plus one
//! `hypot`. The screen alone cut the post-fit merge by 39 % (sum over icons) and `fit_dp`
//! by 11 %; with the cached grid of `super::grid`, by 46 % and 12 %. In WASM (Node, the
//! wasip1 build, 8 icons) `fit_dp` fell 8.5 %. Byte-identical on all 246 icons.
//!
//! # Literature
//!
//! - Method from: Bei, C.-D. & Gray, R. M. (1985), "An improvement of the minimum
//!   distortion encoding algorithm for vector quantization", *IEEE Trans. Commun.*
//!   33(10), doi:10.1109/TCOM.1985.1096214 -- stop summing a candidate's
//!   distortion once it exceeds the best so far (layer 1).
//! - Method from: Higham, N. J. (1993), "The accuracy of floating point summation", *SIAM
//!   J. Sci. Comput.* 14(4):783–799, doi:10.1137/0914050, eq. 2.6 -- how far a reordered
//!   sum of non-negative terms can fall below the in-order one (`REORDER_MARGIN`).
//! - Inspired by: Rakthanmanon, T. et al. (2012), "Searching and mining trillions of time
//!   series subsequences under dynamic time warping", *KDD '12* 262–270,
//!   doi:10.1145/2339530.2339576, §4.2.2 and §4.2.4 -- cascade a cheap lower bound in
//!   front of the exact distance and compute the exact one only for candidates the bound
//!   cannot reject. Ours differs in what is cheap: their bounds (LB_Kim, LB_Keogh) skip
//!   whole DTW computations; ours is the same term without its square root, and the bound
//!   is made provably below the exact *floating-point* term so that no decision changes.
//! - Not from the literature: the exact-confirmation step (re-summing a survivor from the
//!   same samples in the original order), because the published cascades only need the
//!   right *ranking*, while this search must reproduce the earlier code's bits. See also:
//!   Borges, C. F. (2019), "An improved algorithm for hypot(a,b)", arXiv:1904.09481, on
//!   why a correctly scaled `hypot` costs what it does; this module avoids the call rather
//!   than replacing it.

use inkvec_core::{Point, Polyline};

use super::{MAX_SPAN, SAMPLES};
use crate::curves::eval_cubic;

/// Weighted sum of squared distances from the measured points `a..=b` to a curve.
///
/// `χ² = Σ_k (d_k/σ_k)²` (sigma floored at 1e-6 px), with `d_k` the distance from point
/// `k` to the nearest of `SAMPLES`` + 1` points evenly spaced in the curve parameter.
/// Nearest-sample distance overstates the true distance by up to half the sample spacing;
/// every description in a comparison is scored the same way, so the comparison stays
/// fair. A line is passed as the degenerate cubic `[start, start, end, end]`.
pub fn chi2(c: &[Point; 4], poly: &Polyline, a: usize, b: usize) -> f64 {
    chi2_n(c, poly, a, b, SAMPLES)
}

/// `chi2_n` is called millions of times per icon, and both call sites pass one of exactly
/// two compile-time constants, `SAMPLES` or `COARSE_SAMPLES`, neither exceeding
/// `SAMPLES`. A stack buffer sized to `SAMPLES` therefore always has room, which keeps a
/// malloc/free pair out of each call without changing which points are sampled or in what
/// order the distances are folded.
///
/// [`chi2`] with `n + 1` samples instead of `SAMPLES + 1`; `n` must not exceed `SAMPLES`.
/// O(m · n) for a run of `m = b − a + 1` points.
pub(super) fn chi2_n(c: &[Point; 4], poly: &Polyline, a: usize, b: usize, n: usize) -> f64 {
    let mut buf = [Point::new(0.0, 0.0); SAMPLES + 1];
    in_order_sum(samples_of(c, n, &mut buf), poly, a, b)
}

/// The `n + 1` samples `B(k/n)`, `k = 0..=n`, of the cubic `c`, written into the front of
/// `buf` and returned as a slice. `n ≤ SAMPLES`.
fn samples_of<'a>(c: &[Point; 4], n: usize, buf: &'a mut [Point; SAMPLES + 1]) -> &'a [Point] {
    debug_assert!(n <= SAMPLES);
    for (k, s) in buf.iter_mut().enumerate().take(n + 1) {
        *s = eval_cubic(*c, k as f64 / n as f64);
    }
    &buf[..=n]
}

/// `Σ_{k=a..=b} T_k` in point order, starting from `0.0`: [`chi2_n`]'s own fold, given
/// its samples. O(m · samples.len()).
fn in_order_sum(samples: &[Point], poly: &Polyline, a: usize, b: usize) -> f64 {
    let mut total = 0.0;
    for i in a..=b {
        total += point_term(poly.points[i], poly.sigma[i], samples);
    }
    total
}

/// One measured point's share of [`chi2_n`]: `(d/σ)²`, with `d` the distance to the
/// nearest of `samples` and `σ` floored at 1e-6 px. Never negative. O(samples.len()).
#[inline(always)]
fn point_term(p: Point, sigma: f64, samples: &[Point]) -> f64 {
    let (k, _) = nearest_sample(p, samples);
    exact_term(p, sigma, samples[k])
}

/// The nearest of `samples` to `p`: its index and its squared distance `dx² + dy²` (px²).
///
/// The first index at the smallest squared distance wins (a later tie does not replace
/// it), and index 0 if every distance is NaN. Squared distance ranks candidates as the
/// distance would without a `hypot` per candidate: only the winner's distance is ever
/// taken, by [`exact_term`]. This is exactly the search the 0.2.4 `point_term` made, so
/// the chosen sample is the same. O(samples.len()); `samples` must not be empty.
#[inline(always)]
fn nearest_sample(p: Point, samples: &[Point]) -> (usize, f64) {
    let mut best_dist2 = f64::INFINITY;
    let mut best_k = 0;
    for (k, q) in samples.iter().enumerate() {
        let dx = p.x - q.x;
        let dy = p.y - q.y;
        let dist2 = dx * dx + dy * dy;
        if dist2 < best_dist2 {
            best_dist2 = dist2;
            best_k = k;
        }
    }
    (best_k, best_dist2)
}

/// `T = (d/s)²` for the measured point `p` and its chosen sample `q`: `d = |p − q|` via
/// `hypot` (`Point::dist`), `s = max(σ, 1e-6)` px. The term [`chi2`] sums. O(1).
#[inline(always)]
fn exact_term(p: Point, sigma: f64, q: Point) -> f64 {
    let d = p.dist(q);
    let s = sigma.max(1e-6);
    (d / s) * (d / s)
}

/// Below this a lower-bound quotient is replaced by zero: `(r/s)²` this small may involve
/// subnormal intermediates, whose relative error is unbounded. `s ≥ 1e-6` px, so a quotient
/// above it also has `dist² ≥ 1e-302`, a normal number.
const SCREEN_FLOOR: f64 = 1e-290;

/// Above this a lower-bound quotient is replaced by zero, so that no exact term it is
/// compared with can have overflowed while the bound did not, or the reverse.
const SCREEN_CEIL: f64 = 1e300;

/// The factor that keeps a lower bound below the exact term: `1 − 1e-12`, about 4,500
/// units of rounding below one, against the at most twelve that separate the two
/// computations (see the module comment).
const SCREEN_SHRINK: f64 = 1.0 - 1e-12;

/// A lower bound `ℓ ≤ T` of [`exact_term`] from the squared distance `dist2` the nearest
/// search already computed: `fl(dist2 / fl(s²)) · SCREEN_SHRINK`, `s = max(σ, 1e-6)`.
///
/// Zero, which bounds any term from below, when the quotient lies outside
/// `[SCREEN_FLOOR, SCREEN_CEIL]` or is NaN: there the relative error bounds of the module
/// comment need not hold. A NaN coordinate therefore screens as zero; the exact sum it then
/// reaches is NaN, which is not below any bound, as before. O(1), no `hypot`.
#[inline(always)]
fn lower_term(dist2: f64, sigma: f64) -> f64 {
    let s = sigma.max(1e-6);
    let q = dist2 / (s * s);
    if (SCREEN_FLOOR..=SCREEN_CEIL).contains(&q) {
        q * SCREEN_SHRINK
    } else {
        0.0
    }
}

/// Most measured points a merge run spans, `MAX_SPAN + 1`: the size of the stack buffers
/// [`score_below`] keeps its sample choices in.
const RUN_POINTS: usize = MAX_SPAN + 1;

/// A chosen sample's index is kept in a byte; `SAMPLES` must leave room.
const _: () = assert!(SAMPLES < 256);

/// Safety factor on the early-exit bound when the partial sum is taken in a different
/// order from [`chi2_n`]'s. Higham (1993) eq. 2.6 bounds the error of any recursive
/// summation of `m` non-negative terms by `γ_{m−1} = (m−1)u/(1−(m−1)u)` times the exact
/// sum; with `m ≤ RUN_POINTS = 97` that is under 1.1e-14, so a partial sum in any order
/// that reaches `bound·(1 + 1e-12)` proves the in-order sum reaches `bound`, with room to
/// spare for the rounding of the product itself.
const REORDER_MARGIN: f64 = 1.0 + 1e-12;

/// [`chi2_n`], except that it may stop early once the sum is certain to be at least
/// `bound`, and then returns infinity or some value `≥ bound`. A sum below `bound` is
/// returned bit for bit as [`chi2_n`] returns it. The samples are those of [`chi2_n`];
/// the scoring is [`score_below`]'s. O(m · n) at worst.
pub(super) fn chi2_n_below(
    c: &[Point; 4],
    poly: &Polyline,
    a: usize,
    b: usize,
    n: usize,
    bound: f64,
) -> f64 {
    let mut buf = [Point::new(0.0, 0.0); SAMPLES + 1];
    score_below(samples_of(c, n, &mut buf), poly, a, b, bound)
}

/// The run `a..=b` scored against the given curve samples, answering only "is the residual
/// below `bound`?": the exact in-order residual (as [`chi2_n`] computes it from these
/// samples) when it is, and infinity or a value `≥ bound` when it is not.
///
/// The three layers of the module comment, in order:
///
/// 1. `bound` infinite or NaN: no candidate can be rejected, so the exact in-order sum.
///    A run longer than [`RUN_POINTS`], or a `bound` under 1e-250 (too small for the
///    margin's product to be exact): the in-order sum, stopping as soon as it reaches
///    `bound`, which needs no margin because adding a non-negative term never makes a sum
///    smaller.
/// 2. Otherwise the points are visited middle-first -- `mid = ⌊(m−1)/2⌋`, then
///    alternately one step right and one step left, which visits each index once since
///    `mid` points lie left of the middle and `m − 1 − mid` (the same or one more) right
///    of it -- and the lower bounds [`lower_term`] of their terms are summed. Once that
///    partial sum reaches `bound · REORDER_MARGIN`, the answer is infinity.
/// 3. A survivor's exact terms are summed in point order, each from the sample its screen
///    found nearest.
///
/// `samples` must not be empty. O(m · samples.len()) for the nearest-sample searches, the
/// survivor's `m` extra `hypot`s, and no allocation. Edge cases: NaN coordinates screen as
/// zero and sum to NaN, never below `bound`; a one-point run is its middle.
pub(super) fn score_below(
    samples: &[Point],
    poly: &Polyline,
    a: usize,
    b: usize,
    bound: f64,
) -> f64 {
    if bound.is_nan() || bound == f64::INFINITY {
        return in_order_sum(samples, poly, a, b);
    }
    let m = b + 1 - a;
    if m > RUN_POINTS || bound < 1e-250 {
        let mut total = 0.0;
        for i in a..=b {
            total += point_term(poly.points[i], poly.sigma[i], samples);
            if total >= bound {
                return f64::INFINITY;
            }
        }
        return total;
    }
    let exit = bound * REORDER_MARGIN;
    // The sample each point's screen found nearest, by position in the run.
    let mut nearest = [0u8; RUN_POINTS];
    let mut partial = 0.0;
    let mid = (m - 1) / 2;
    for step in 0..m {
        let half = step.div_ceil(2);
        // Odd steps go right of the middle, even steps left. `& 1` rather than `% 2`: new
        // hot loops avoid `%`, since wazero's arm64 compiler miscompiled `i32.rem_u` in one.
        let q = if step & 1 == 1 {
            mid + half
        } else {
            mid - half
        };
        let i = a + q;
        let (k, dist2) = nearest_sample(poly.points[i], samples);
        // `k ≤ SAMPLES < 256`, checked at compile time above.
        nearest[q] = k as u8;
        partial += lower_term(dist2, poly.sigma[i]);
        if partial >= exit {
            return f64::INFINITY;
        }
    }
    let mut total = 0.0;
    for (q, &k) in nearest[..m].iter().enumerate() {
        let i = a + q;
        total += exact_term(poly.points[i], poly.sigma[i], samples[usize::from(k)]);
    }
    total
}

#[cfg(test)]
mod tests {
    //! The screen's one claim: a lower bound never exceeds the exact term it stands for.
    use super::*;

    /// `lower_term(dist², σ) ≤ exact_term` for the same point and sample, over distances
    /// from zero through subnormal-squared to overflowing, sigmas from zero to huge and
    /// NaN, and coordinates far from the origin.
    #[test]
    fn lower_term_never_exceeds_the_exact_term() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let scales = [
            0.0, 1e-300, 1e-200, 1e-160, 1e-150, 1e-140, 1e-20, 1e-9, 1e-3, 0.37, 1.0, 3.0, 97.0,
            1e4, 1e140, 1e150, 1e154, 1e160, 1e200,
        ];
        let sigmas = [
            0.0,
            -1.0,
            1e-9,
            1e-6,
            1e-3,
            0.05,
            0.5,
            1.0,
            7.0,
            1e10,
            1e160,
            f64::NAN,
        ];
        let origins = [0.0, 1.0, 1e4, -1e6];
        let mut checked = 0;
        for &scale in &scales {
            for &sigma in &sigmas {
                for &o in &origins {
                    for k in 0..40 {
                        // Half the pairs near `o`, half with both points at the scale's own
                        // magnitude, where tiny and huge differences are not absorbed.
                        let p = if k % 2 == 0 {
                            Point::new(o + 3.0 * (next() - 0.5), o - 2.0 * next())
                        } else {
                            Point::new(scale * (next() - 0.5), scale * next())
                        };
                        let q =
                            Point::new(p.x + scale * (next() - 0.5), p.y + scale * (next() - 0.5));
                        let (dx, dy) = (p.x - q.x, p.y - q.y);
                        let lower = lower_term(dx * dx + dy * dy, sigma);
                        let exact = exact_term(p, sigma, q);
                        assert!(lower >= 0.0, "{lower} at {scale} {sigma}");
                        assert!(
                            lower <= exact,
                            "lower {lower:e} > exact {exact:e} at scale {scale:e}, sigma {sigma:e}"
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 30_000);
    }

    /// The nearest search keeps the first of tied samples, as the 0.2.4 `point_term` did.
    #[test]
    fn nearest_sample_keeps_the_first_of_a_tie() {
        let p = Point::new(0.0, 0.0);
        let samples = [
            Point::new(2.0, 0.0),
            Point::new(0.0, 1.0),
            Point::new(-1.0, 0.0),
            Point::new(1.0, 0.0),
        ];
        assert_eq!(nearest_sample(p, &samples), (1, 1.0));
        let nan = [Point::new(f64::NAN, 0.0), Point::new(f64::NAN, 1.0)];
        assert_eq!(nearest_sample(p, &nan).0, 0);
    }

    /// The screened score against the plain in-order sum on runs whose terms span many
    /// orders of magnitude, with bounds just around the exact value: below its bound it is
    /// the exact sum bit for bit, and it never answers "below" when the sum is not.
    #[test]
    fn screened_score_agrees_with_the_in_order_sum() {
        let mut state = 17u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        for case in 0..400 {
            let m = 1 + case % RUN_POINTS;
            let spread = [1e-8, 1e-3, 0.5, 30.0][case % 4];
            let pts: Vec<Point> = (0..m)
                .map(|k| Point::new(k as f64 * 0.7, spread * (next() - 0.5)))
                .collect();
            let sigma: Vec<f64> = (0..m)
                .map(|_| [1e-7, 0.05, 0.5][case % 3] + next())
                .collect();
            let poly = Polyline::new(pts, sigma, false);
            let n = [8, 24, 96][case % 3];
            let samples: Vec<Point> = (0..=n)
                .map(|k| Point::new(k as f64 * 0.7 * (m as f64) / n as f64, 0.0))
                .collect();
            let exact = in_order_sum(&samples, &poly, 0, m - 1);
            for bound in [
                exact,
                f64::from_bits(exact.to_bits() + 1),
                f64::from_bits(exact.to_bits().saturating_sub(1)),
                exact * (1.0 + 1e-13),
                exact * (1.0 - 1e-13),
                exact * 0.5,
                exact * 2.0,
                f64::INFINITY,
            ] {
                let got = score_below(&samples, &poly, 0, m - 1, bound);
                assert_eq!(got < bound, exact < bound, "case {case}, bound {bound:e}");
                if got < bound || !got.is_infinite() {
                    assert_eq!(
                        got.to_bits(),
                        exact.to_bits(),
                        "case {case}, bound {bound:e}"
                    );
                }
            }
        }
    }
}
