//! The profile-aware radial search: circular and elliptical gradient geometries placed
//! under the artist's kind of colour profile, not under a straight one.
//!
//! # The problem
//!
//! A radial or elliptical gradient is `colour = s(t(x; θ))`: a geometry `θ` (centre
//! `(cx, cy)`, and for an ellipse its angle `φ` and `ln` aspect `a`) maps each sample to a
//! coordinate `t`, and a profile `s` maps `t` to colour. The line-scored fitters
//! ([`super::fit_radial`], [`super::fit_radial_elliptic`]) choose `θ` by the residual of a
//! *straight* profile `s(t) = a + g·t`, but on the 168 corpus icons whose artist SVG has
//! gradients, 73 % of the artist's visible gradients are clamped -- their stops do not span
//! 0..1, so the profile is a flat core and a ramp at the rim, or a bump on a pad -- and they
//! were painted flat 38 % of the time against 10 % for full-range ones (round-2 gradient
//! research, problem A). A straight-line score pulls the centre to wherever a straight ramp
//! fits the region best, not to the ramp's real centre, and the interior stops fitted
//! afterwards (`fit_mid_stops`) can only reshape the profile at that wrong geometry.
//!
//! So the geometry is searched a second time under a profile that can be clamped: a
//! continuous piecewise-linear function with [`SPLINE_KNOTS`] uniform knots over the range
//! of `t`, linear in its knot values `X` (a `K × 3` matrix, one column per channel) once `θ`
//! is fixed. Fitting both is a *separable* nonlinear least-squares problem,
//!
//! `min_{θ, X} ‖C − A(θ)·X‖²`,
//!
//! with `C` the centred sample colours and `A(θ)` the hat-basis matrix (`A_ij` the weight of
//! knot `j` at sample `i`'s `t`). Variable projection eliminates `X` in closed form,
//! `X(θ) = A⁺C`, and leaves the functional `F(θ) = ‖P⊥(θ)·C‖²`, `P⊥ = I − A·A⁺`, of the
//! geometry alone: exactly the residual [`Resid1d::finish_spline`] returns.
//!
//! The geometries found are offered as *extra* candidates, re-stopped in each fitting space
//! ([`super::restop_radial`]) and appended after the line-scored ones by `fit_samples`:
//! nothing is replaced, so a region whose profile is straight keeps the fit it had (the
//! line-scored candidate comes first and wins ties), and model selection prices the
//! newcomers like any other. Replacing the line search instead was measured worse in an
//! earlier attempt (4304ba3).
//!
//! # The search ([`profile_geometries`])
//!
//! 1. **Seed**: the weighted least-squares point nearest every gradient line of the
//!    principal colour axis, the seed of [`super::fit_radial`], clamped to within one
//!    bounding-box size of the region; the centroid when the lines are parallel.
//! 2. **Circle**: Levenberg–Marquardt over the centre ([`Resid1d::refine_spline`], `p = 2`).
//! 3. **Ellipse**, from the circle: the orientation sweep of [`super::fit_radial_elliptic`]
//!    (every 15° at aspects 2 and 3, kept when it lowers the score), then
//!    Levenberg–Marquardt over all four coordinates (`p = 4`); kept from an aspect of 1.02.
//!
//! The search runs once, on the sRGB colours, and serves both interpolation spaces: a free
//! piecewise-linear profile absorbs the per-channel transfer curve between the spaces, so
//! the level sets it finds, and with them the geometry, do not depend on the space.
//!
//! # The method
//!
//! Method from: L. Kaufman (1975), A variable projection method for solving separable
//! nonlinear least squares problems, BIT 15(1):49–57, doi:10.1007/BF01932995 -- the
//! Gauss–Newton step on the variable-projection functional with Kaufman's Jacobian
//! `J = −P⊥·(∂A/∂θ)·X`, which drops the second, smaller term of the exact Jacobian of
//! G. H. Golub, V. Pereyra (1973), The differentiation of pseudo-inverses and nonlinear
//! least squares problems whose variables separate, SIAM J. Numer. Anal. 10(2):413–432,
//! doi:10.1137/0710036. With the residual `r = P⊥·C` (which `P⊥` leaves unchanged) the
//! step `δ` solves
//!
//! `(Dᵀ·P⊥·D + μ·diag)·δ = Dᵀ·r`,  `D = (∂A/∂θ)·X`,
//!
//! damped as K. Levenberg (1944), A method for the solution of certain non-linear problems
//! in least squares, Quart. Appl. Math. 2(2):164–168, doi:10.1090/qam/10666, and
//! D. W. Marquardt (1963), An algorithm for least-squares estimation of nonlinear
//! parameters, J. SIAM 11(2):431–441, doi:10.1137/0111030 (the diagonal of `Dᵀ·P⊥·D` scales
//! the damping, so pixels, radians and `ln` aspect need no common unit). Under Kaufman's
//! approximation the gradient `∇F = −2·Dᵀr` is exact (the dropped term is orthogonal to
//! `r`), which the tests check against finite differences.
//!
//! What was adapted:
//! * `∂A/∂θ` follows the hat basis: a sample at knot coordinate `U = (t − lo)/h`, in piece
//!   `j` at fraction `f`, is predicted `(1 − f)·X_j + f·X_{j+1}`, so `D_i = ΔX_j·∂U_i/∂θ`
//!   with `ΔX_j = X_{j+1} − X_j`. The knots span the samples' range `[lo, hi]`, which moves
//!   with `θ` (`lo` and `hi` are the `t` of two samples), so `∂U_i/∂θ = ((∂t_i − ∂t_lo) −
//!   U_i·(∂t_hi − ∂t_lo)/(K − 1)) / h` -- the profile stretches with its range.
//! * `P⊥·D` is never formed: `Dᵀ·P⊥·D = DᵀD − (AᵀD)ᵀ(AᵀA)⁻¹(AᵀD)`, and `AᵀA` is the same
//!   tridiagonal matrix the score solves, so each column costs one Thomas sweep of `K`.
//! * The geometry is clamped after each step to the box the compass searches keep (the
//!   centre within one bounding-box size of the region, `ln` aspect in `[0, ln 8]`); a
//!   clamped step is judged like any other, on the true `F`.
//!
//! Inspired by: S. Chakraborty et al. (2025), Image Vectorization via Gradient
//! Reconstruction, Computer Graphics Forum 44(2), doi:10.1111/cgf.70055, §3.3, whose radial
//! geometry does not depend on the profile shape at all; ours keeps a profile in the score
//! but lets it bend, and leaves the stops to the fitters that follow. See also:
//! D. P. O'Leary, B. W. Rust (2013), Variable projection for nonlinear least squares
//! problems, Comput. Optim. Appl. 54(3):579–593, doi:10.1007/s10589-012-9492-9, for the
//! Kaufman step's place among the variable-projection variants; M. Lukáč et al., US
//! 12,340,441 B2 (2025), Reconstructing concentric radial gradients, a profile-agnostic
//! centre from the orthogonality of the colour gradient and the position vector, which is
//! what the gradient-line seed already is.
//!
//! # Cost, measured
//!
//! The research prototype minimised `F` by the compass search the line-scored fitters use
//! (steps of 4 px halved down to 0.03 px, up to 600 evaluations for a circle and 800 after
//! a 24-point sweep for an ellipse). On `noto-emoji/emoji_u1f36a` at 128 px (single thread,
//! `INKVEC_TIMING`, 2026-10-03) its 1,328 searches made 14.5 evaluations each with
//! Levenberg–Marquardt, and the searches took 1.3 s instead of 2.3 s; on the 168 gradient
//! icons the result was as good (dE00 0.3459 against the compass's 0.3468, as-one 48 %
//! against 47 %). Most of what the part costs is not the search but the four extra
//! candidates each fit then scores (their interior stops, `fit_mid_stops`): 10.6 s of the
//! 15.7 s that icon's band merger took. One pass of the refinement forms the system
//! (`O(n·p + K·p²)`, `p` free coordinates, `n ≤ CENTRE_SEARCH_SAMPLES`) and evaluates `F`
//! once per trial step. Sequential and deterministic: the same numbers on every machine
//! and thread count.

use super::*;

/// Most evaluations of the score one refinement may make (trial steps included). The
/// compass search it replaced allowed 600 (circle) and 800 after a 24-point sweep
/// (ellipse); the refinement uses 14.5 on average (module docs).
pub(super) const LM_MAX_EVALS: usize = 60;

/// An accepted step that lowers the score by less than this fraction of it ends the
/// refinement.
pub(super) const LM_REL_TOL: f64 = 1e-9;

/// The damping at which the refinement gives up looking for a lower score.
pub(super) const LM_MU_MAX: f64 = 1e10;

/// An accepted step smaller than this in every coordinate (centre px, angle rad,
/// `ln` aspect) ends the refinement: the compass search it replaced stopped at 0.03 px.
pub(super) const LM_STEP_TOL: [f64; 4] = [0.01, 0.01, 1e-3, 1e-3];

/// A radial geometry: centre `x`, centre `y` (px), angle (rad), `ln` aspect.
pub(super) type Geom = [f64; 4];

/// The profile-aware circular and elliptical geometries of the samples `s` (colours `cols`,
/// in the space the search runs in; `w` the image width, for the seed's neighbours), as
/// [`FillModel::Radial`] values whose centre, aspect and angle are the search's and whose
/// radius and stops are placeholders: [`super::restop_radial`] fits those per space.
///
/// Steps 1–3 of the module docs. Empty below [`MIN_GRADIENT_PIXELS`] samples or without
/// colour variance; no circle when its radius (the largest sample distance) is under
/// 0.5 px, and then no ellipse; no ellipse below `2·MIN_GRADIENT_PIXELS` samples or under an
/// aspect of 1.02, or with a radius under 0.5 px. The search reads a strided subsample of at
/// most [`CENTRE_SEARCH_SAMPLES`] samples; the radius test reads every sample.
pub(crate) fn profile_geometries(s: &Samples, cols: &[[f64; 3]], w: usize) -> Vec<FillModel> {
    let n = s.len();
    if n < MIN_GRADIENT_PIXELS {
        return Vec::new();
    }
    let Some(axis) = color_axis(cols) else {
        return Vec::new();
    };
    let f: Vec<f64> = cols
        .iter()
        .map(|c| c[0] * axis[0] + c[1] * axis[1] + c[2] * axis[2])
        .collect();
    let seed = gradient_line_centre(s, &f, w).unwrap_or_else(|| s.centroid());
    let xmin = s.x.iter().cloned().fold(f64::MAX, f64::min);
    let xmax = s.x.iter().cloned().fold(f64::MIN, f64::max);
    let ymin = s.y.iter().cloned().fold(f64::MAX, f64::min);
    let ymax = s.y.iter().cloned().fold(f64::MIN, f64::max);
    let (bw, bh) = ((xmax - xmin).max(4.0), (ymax - ymin).max(4.0));
    // The box of the line-scored searches: the centre within one bounding-box size of the
    // region, `ln` aspect in `[0, ln MAX_ASPECT]`.
    let clamp = |g: Geom| -> Geom {
        [
            g[0].clamp(xmin - bw, xmax + bw),
            g[1].clamp(ymin - bh, ymax + bh),
            g[2],
            g[3].clamp(0.0, MAX_ASPECT.ln()),
        ]
    };
    let cstride = (n / CENTRE_SEARCH_SAMPLES).max(1);
    let idx: Vec<usize> = (0..n).step_by(cstride).collect();
    let cols_sub: Vec<[f64; 3]> = idx.iter().map(|&i| cols[i]).collect();
    let mut rz = Resid1d::new(s, &idx, &cols_sub);
    // The largest distance of any sample from the geometry's centre, in its own metric.
    let radius = |g: &Geom| -> f64 {
        let (sn, cs) = g[2].sin_cos();
        let k = g[3].exp();
        (0..n)
            .map(|i| {
                let (dx, dy) = (s.x[i] - g[0], s.y[i] - g[1]);
                let u = dx * cs + dy * sn;
                let v = (-dx * sn + dy * cs) * k;
                (u * u + v * v).sqrt()
            })
            .fold(f64::MIN, f64::max)
    };
    let as_model = |g: Geom| FillModel::Radial {
        c: (g[0], g[1]),
        r: 1.0,
        c0: [0.0; 3],
        c1: [0.0; 3],
        interp: Interp::Srgb,
        aspect: g[3].exp(),
        angle: g[2],
        mids: Vec::new(),
    };
    // Step 2: the circle; angle and aspect are held at 0 (aspect 1).
    let start = clamp([seed.0, seed.1, 0.0, 0.0]);
    let circle_box = |g: Geom| {
        let q = clamp(g);
        [q[0], q[1], 0.0, 0.0]
    };
    let (circle, _) = rz.refine_spline(start, 2, circle_box);
    if radius(&circle) < 0.5 {
        return Vec::new();
    }
    let mut out = vec![as_model(circle)];
    if n < 2 * MIN_GRADIENT_PIXELS {
        return out;
    }
    // Step 3: the orientation sweep about the circle's centre, then the four coordinates.
    let mut st = clamp(circle);
    let mut best = rz.score_at(&st);
    for deg in (0..180).step_by(15) {
        // Aspects 2 and 3; the sweep reaches a 1:2 ellipse as a 2:1 one turned 90°.
        for &la in &[2.0f64.ln(), 3.0f64.ln()] {
            let cand = clamp([st[0], st[1], (deg as f64).to_radians(), la]);
            let r = rz.score_at(&cand);
            if r < best - 1e-12 {
                best = r;
                st = cand;
            }
        }
    }
    let (ellipse, _) = rz.refine_spline(st, 4, clamp);
    if ellipse[3].exp() >= 1.02 && radius(&ellipse) >= 0.5 {
        out.push(as_model(ellipse));
    }
    out
}

/// The spline fit at the current `t` of a [`Resid1d`]: the knot range, the knot values and
/// the factored normal matrix.
struct SplineFit {
    /// Smallest `t` and the sample attaining it (first on ties).
    lo: (f64, usize),
    /// Largest `t` and the sample attaining it (first on ties).
    hi: (f64, usize),
    /// Knot spacing `(hi − lo)/(K − 1)`.
    h: f64,
    /// Knot values per channel (centred colours).
    x: [[f64; 3]; SPLINE_KNOTS],
    /// Thomas pivots of `AᵀA` (ridge included).
    m: [f64; SPLINE_KNOTS],
    /// Thomas upper coefficients of `AᵀA`.
    cp: [f64; SPLINE_KNOTS],
    /// Off-diagonal of `AᵀA`.
    e: [f64; SPLINE_KNOTS],
}

/// Knot coordinate of `t` in a range `lo..lo + (K − 1)·h`: the piece `j` and the fraction
/// `f` in it, with `t = hi` falling in the last piece at `f = 1`. The same rule as
/// [`Resid1d::finish_spline`].
fn piece(t: f64, lo: f64, h: f64) -> (usize, f64, f64) {
    let u_max = (SPLINE_KNOTS - 1) as f64 - 1e-12;
    let u = ((t - lo) / h).min(u_max);
    let j = u as usize;
    (j, u - j as f64, u)
}

/// Solve `AᵀA·z = b` for one right-hand side by the Thomas algorithm with the pivots of
/// [`SplineFit`]. `O(K)`.
fn thomas(fit: &SplineFit, b: &[f64; SPLINE_KNOTS]) -> [f64; SPLINE_KNOTS] {
    const K: usize = SPLINE_KNOTS;
    let mut dp = [0.0f64; K];
    dp[0] = b[0] / fit.m[0];
    for i in 1..K {
        dp[i] = (b[i] - fit.e[i - 1] * dp[i - 1]) / fit.m[i];
    }
    let mut z = [0.0f64; K];
    z[K - 1] = dp[K - 1];
    for i in (0..K - 1).rev() {
        z[i] = dp[i] - fit.cp[i] * z[i + 1];
    }
    z
}

/// Solve the `p × p` system `a·δ = b` (`p ≤ 4`) by Gaussian elimination with partial
/// pivoting. `None` when a pivot vanishes (relative to the largest diagonal).
fn solve_small(mut a: [[f64; 4]; 4], mut b: [f64; 4], p: usize) -> Option<[f64; 4]> {
    let scale = (0..p).map(|i| a[i][i].abs()).fold(0.0, f64::max);
    if scale <= 0.0 || !scale.is_finite() {
        return None;
    }
    for col in 0..p {
        let piv = (col..p)
            .max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))
            .unwrap_or(col);
        if a[piv][col].abs() <= 1e-14 * scale {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..p {
            let f = a[row][col] / a[col][col];
            for k in col..p {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0f64; 4];
    for row in (0..p).rev() {
        let mut s = b[row];
        for k in row + 1..p {
            s -= a[row][k] * x[k];
        }
        x[row] = s / a[row][row];
    }
    x.iter().take(p).all(|v| v.is_finite()).then_some(x)
}

impl Resid1d<'_> {
    /// The spline score ([`Resid1d::finish_spline`]) at geometry `g`, leaving the samples'
    /// `t` in `self.t`. With angle 0 and aspect 1 the elliptical distance is the Euclidean
    /// one, bit for bit (`u = dx`, `v = dy`).
    fn score_at(&mut self, g: &Geom) -> f64 {
        let (sn, cs) = g[2].sin_cos();
        self.elliptic_t((g[0], g[1]), sn, cs, g[3].exp());
        self.finish_spline()
    }

    /// The least-squares spline through the current `(t, colour)`: [`SplineFit`]. The
    /// arithmetic of [`Resid1d::finish_spline`], keeping what the Jacobian needs. `None`
    /// when `t` has no spread.
    fn spline_fit(&self) -> Option<SplineFit> {
        const K: usize = SPLINE_KNOTS;
        let (mut lo, mut hi) = ((f64::MAX, 0usize), (f64::MIN, 0usize));
        for (i, &t) in self.t.iter().enumerate() {
            if t < lo.0 {
                lo = (t, i);
            }
            if t > hi.0 {
                hi = (t, i);
            }
        }
        if hi.0 - lo.0 <= 1e-9 {
            return None;
        }
        let h = (hi.0 - lo.0) / (K - 1) as f64;
        let mut d = [0.0f64; K];
        let mut e = [0.0f64; K];
        let mut b = [[0.0f64; 3]; K];
        for (c, &t) in self.cols.iter().zip(&self.t) {
            let (j, f, _) = piece(t, lo.0, h);
            let (w0, w1) = (1.0 - f, f);
            d[j] += w0 * w0;
            d[j + 1] += w1 * w1;
            e[j] += w0 * w1;
            for ch in 0..3 {
                let dc = c[ch] - self.cbar[ch];
                b[j][ch] += w0 * dc;
                b[j + 1][ch] += w1 * dc;
            }
        }
        let mut m = [0.0f64; K];
        let mut cp = [0.0f64; K];
        m[0] = d[0] + 1e-9;
        cp[0] = e[0] / m[0];
        for i in 1..K {
            m[i] = d[i] + 1e-9 - e[i - 1] * cp[i - 1];
            cp[i] = if i + 1 < K { e[i] / m[i] } else { 0.0 };
        }
        let mut fit = SplineFit {
            lo,
            hi,
            h,
            x: [[0.0; 3]; K],
            m,
            cp,
            e,
        };
        for ch in 0..3 {
            let rhs: [f64; K] = std::array::from_fn(|j| b[j][ch]);
            let z = thomas(&fit, &rhs);
            for j in 0..K {
                fit.x[j][ch] = z[j];
            }
        }
        Some(fit)
    }

    /// `∂t_i/∂θ` of sample `i` at geometry `g` (whose `sin φ`, `cos φ` and `k = e^a` are
    /// `trig`), for the first `p` coordinates of [`Geom`]. With `(dx, dy) = P_i − c`,
    /// `u = dx·cos φ + dy·sin φ`,
    /// `v = (−dx·sin φ + dy·cos φ)·k`, `k = e^a`, `t = √(u² + v²)`:
    /// `∂t/∂cx = (−u·cos φ + v·k·sin φ)/t`, `∂t/∂cy = (−u·sin φ − v·k·cos φ)/t`,
    /// `∂t/∂φ = u·v·(1/k − k)/t`, `∂t/∂a = v²/t`. Zero at `t = 0`, where `t` is not
    /// differentiable (the centre on a sample).
    fn dt(&self, i: usize, g: &Geom, trig: (f64, f64, f64), p: usize) -> [f64; 4] {
        let (sn, cs, k) = trig;
        let (dx, dy) = (self.xs[i] - g[0], self.ys[i] - g[1]);
        let u = dx * cs + dy * sn;
        let v = (-dx * sn + dy * cs) * k;
        let t = (u * u + v * v).sqrt();
        if t <= 1e-12 {
            return [0.0; 4];
        }
        let all = [
            (-u * cs + v * k * sn) / t,
            (-u * sn - v * k * cs) / t,
            u * v * (1.0 / k - k) / t,
            v * v / t,
        ];
        std::array::from_fn(|q| if q < p { all[q] } else { 0.0 })
    }

    /// Kaufman's undamped system at geometry `g`, whose `t` must be in `self.t`: the
    /// matrix `Dᵀ·P⊥·D` and the vector `Dᵀ·r` over the first `p` coordinates (see the
    /// module docs). `None` when `t` has no spread.
    fn kaufman_system(&self, g: &Geom, p: usize) -> Option<([[f64; 4]; 4], [f64; 4])> {
        const K: usize = SPLINE_KNOTS;
        let fit = self.spline_fit()?;
        let (sn, cs) = g[2].sin_cos();
        let trig = (sn, cs, g[3].exp());
        let dt_lo = self.dt(fit.lo.1, g, trig, p);
        let dt_hi = self.dt(fit.hi.1, g, trig, p);
        let mut dd = [[0.0f64; 4]; 4];
        let mut dr = [0.0f64; 4];
        // AᵀD per channel and coordinate: atd[ch][q][j].
        let mut atd = [[[0.0f64; K]; 4]; 3];
        for (i, (c, &t)) in self.cols.iter().zip(&self.t).enumerate() {
            let (j, f, u) = piece(t, fit.lo.0, fit.h);
            let dti = self.dt(i, g, trig, p);
            let du: [f64; 4] = std::array::from_fn(|q| {
                ((dti[q] - dt_lo[q]) - u * (dt_hi[q] - dt_lo[q]) / (K - 1) as f64) / fit.h
            });
            for ch in 0..3 {
                let slope = fit.x[j + 1][ch] - fit.x[j][ch];
                let pred = (1.0 - f) * fit.x[j][ch] + f * fit.x[j + 1][ch];
                let r = (c[ch] - self.cbar[ch]) - pred;
                for q in 0..p {
                    let dq = slope * du[q];
                    dr[q] += dq * r;
                    atd[ch][q][j] += (1.0 - f) * dq;
                    atd[ch][q][j + 1] += f * dq;
                    for l in 0..=q {
                        dd[q][l] += dq * slope * du[l];
                    }
                }
            }
        }
        for ch in 0..3 {
            let z: [[f64; K]; 4] = std::array::from_fn(|q| {
                if q < p {
                    thomas(&fit, &atd[ch][q])
                } else {
                    [0.0; K]
                }
            });
            for q in 0..p {
                for l in 0..=q {
                    let proj: f64 = (0..K).map(|j| atd[ch][q][j] * z[l][j]).sum();
                    dd[q][l] -= proj;
                }
            }
        }
        for q in 0..p {
            for l in q + 1..p {
                dd[q][l] = dd[l][q];
            }
        }
        Some((dd, dr))
    }

    /// Minimise the spline score over the first `p` coordinates of the geometry (2: the
    /// centre of a circle, angle and aspect held; 4: an ellipse), from `start`, by
    /// Levenberg–Marquardt on Kaufman's variable-projection step (module docs). `clamp`
    /// keeps a trial geometry in the search box. Returns the best geometry found and its
    /// score; never worse than `start` (clamped).
    ///
    /// Each pass forms the system at the current geometry, then tries damped steps,
    /// multiplying the damping `μ` by 4 after a step that does not lower the score (or a
    /// singular system) and by 0.3 after one that does; an accepted step ends the pass.
    /// The damping starts at `1e-3` and is floored at `1e-12`. Stops on: an accepted step
    /// smaller than [`LM_STEP_TOL`] in every coordinate, or one lowering the score by at
    /// most [`LM_REL_TOL`] of it (converged); `μ` past [`LM_MU_MAX`] (no lower score
    /// nearby); [`LM_MAX_EVALS`] evaluations; `t` without spread (no system).
    /// Ties: a trial must lower the score strictly to be accepted.
    pub(super) fn refine_spline(
        &mut self,
        start: Geom,
        p: usize,
        clamp: impl Fn(Geom) -> Geom,
    ) -> (Geom, f64) {
        let mut g = clamp(start);
        let mut best = self.score_at(&g);
        let mut evals = 1usize;
        let mut mu = 1e-3;
        // Each pass of this loop starts with `self.t` holding the `t` of `g`: the first
        // evaluation above, then the accepted trial that ended the previous pass.
        while evals < LM_MAX_EVALS {
            let Some((a, b)) = self.kaufman_system(&g, p) else {
                break;
            };
            let floor = 1e-12 * (0..p).map(|q| a[q][q]).fold(0.0, f64::max);
            let mut accepted = false;
            while evals < LM_MAX_EVALS && mu <= LM_MU_MAX {
                let mut damped = a;
                for q in 0..p {
                    damped[q][q] += mu * (a[q][q] + floor);
                }
                let Some(step) = solve_small(damped, b, p) else {
                    mu *= 4.0;
                    continue;
                };
                let mut trial = g;
                for q in 0..p {
                    trial[q] += step[q];
                }
                let trial = clamp(trial);
                let score = self.score_at(&trial);
                evals += 1;
                if score < best {
                    let gained = best - score;
                    // The largest step, in units of each coordinate's tolerance.
                    let moved = (0..p)
                        .map(|q| (trial[q] - g[q]).abs() / LM_STEP_TOL[q])
                        .fold(0.0, f64::max);
                    g = trial;
                    best = score;
                    mu = (mu * 0.3).max(1e-12);
                    accepted = true;
                    if gained <= LM_REL_TOL * best.max(f64::MIN_POSITIVE) || moved < 1.0 {
                        return (g, best);
                    }
                    break;
                }
                mu *= 4.0;
            }
            if !accepted {
                break;
            }
        }
        (g, best)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples of a `w × w` grid inside the ellipse `(centre, r, angle, aspect)` with the
    /// clamped profile of an artist's radial: flat out to `t = 0.4`, a ramp to the rim.
    fn clamped(w: usize, centre: (f64, f64), r: f64, angle: f64, aspect: f64) -> Samples {
        let mut s = Samples {
            px: vec![],
            x: vec![],
            y: vec![],
            srgb: vec![],
            lin: vec![],
        };
        let (sn, cs) = angle.sin_cos();
        for p in 0..w * w {
            let (x, y) = ((p % w) as f64, (p / w) as f64);
            let (dx, dy) = (x - centre.0, y - centre.1);
            let u = dx * cs + dy * sn;
            let v = (-dx * sn + dy * cs) * aspect;
            let t = (u * u + v * v).sqrt() / r;
            if t > 1.0 {
                continue;
            }
            let k = ((t - 0.4) / 0.6).max(0.0);
            let c = [0.85 - 0.5 * k, 0.6 - 0.3 * k, 0.3 + 0.2 * k];
            s.px.push(p);
            s.x.push(x);
            s.y.push(y);
            s.lin.push(c);
            s.srgb.push([c[0] as f32, c[1] as f32, c[2] as f32]);
        }
        s
    }

    fn free(g: Geom) -> Geom {
        g
    }

    #[test]
    fn kaufmans_gradient_is_the_scores_gradient() {
        // The variable-projection gradient is exact under Kaufman's approximation:
        // ∇F = −2·Dᵀr. Checked against central differences of the score, every coordinate
        // of an ellipse, away from the optimum (so the gradient is not zero).
        let s = clamped(40, (19.3, 21.7), 15.0, 0.4, 1.5);
        let idx: Vec<usize> = (0..s.px.len()).collect();
        let cols = s.colors(Interp::Srgb);
        let mut rz = Resid1d::new(&s, &idx, &cols);
        let g = [18.1, 22.9, 0.25, 0.3];
        rz.score_at(&g);
        let (_, dr) = rz.kaufman_system(&g, 4).expect("spread");
        for q in 0..4 {
            let h = 1e-5;
            let (mut a, mut b) = (g, g);
            a[q] += h;
            b[q] -= h;
            let fd = (rz.score_at(&a) - rz.score_at(&b)) / (2.0 * h);
            let an = -2.0 * dr[q];
            assert!(
                (fd - an).abs() <= 1e-3 * fd.abs().max(an.abs()) + 1e-9,
                "coordinate {q}: finite difference {fd}, analytic {an}"
            );
        }
    }

    #[test]
    fn a_clamped_circle_is_centred_from_a_wrong_start() {
        let centre = (20.3, 17.6);
        let s = clamped(48, centre, 16.0, 0.0, 1.0);
        let idx: Vec<usize> = (0..s.px.len()).collect();
        let cols = s.colors(Interp::Srgb);
        let mut rz = Resid1d::new(&s, &idx, &cols);
        let start = [23.0, 21.0, 0.0, 0.0];
        let at_start = rz.score_at(&start);
        let (g, f) = rz.refine_spline(start, 2, free);
        let err = (g[0] - centre.0).hypot(g[1] - centre.1);
        assert!(err < 0.1, "centre {g:?} off by {err}");
        assert!(f < 1e-3 * at_start, "score {f} from {at_start}");
        assert_eq!((g[2], g[3]), (0.0, 0.0), "angle and aspect held");
    }

    #[test]
    fn a_clamped_ellipse_is_found_in_all_four_coordinates() {
        let (centre, angle, aspect) = ((24.4, 22.1), 0.5, 1.6);
        let s = clamped(48, centre, 18.0, angle, aspect);
        let idx: Vec<usize> = (0..s.px.len()).collect();
        let cols = s.colors(Interp::Srgb);
        let mut rz = Resid1d::new(&s, &idx, &cols);
        let (g, _) = rz.refine_spline([23.5, 23.0, 0.3, 1.4f64.ln()], 4, free);
        let err = (g[0] - centre.0).hypot(g[1] - centre.1);
        assert!(err < 0.2, "centre {g:?} off by {err}");
        assert!((g[2] - angle).abs().to_degrees() < 2.0, "angle {}", g[2]);
        assert!(
            (g[3].exp() / aspect - 1.0).abs() < 0.03,
            "aspect {}",
            g[3].exp()
        );
    }

    #[test]
    fn the_result_is_never_worse_than_the_start_and_respects_the_clamp() {
        let s = clamped(32, (15.2, 16.8), 12.0, 0.0, 1.0);
        let idx: Vec<usize> = (0..s.px.len()).collect();
        let cols = s.colors(Interp::Srgb);
        let mut rz = Resid1d::new(&s, &idx, &cols);
        // A box that excludes the true centre: the answer sits on its edge.
        let boxed = |g: Geom| [g[0].clamp(0.0, 12.0), g[1].clamp(0.0, 12.0), g[2], g[3]];
        let start = [6.0, 6.0, 0.0, 0.0];
        let at_start = rz.score_at(&start);
        let (g, f) = rz.refine_spline(start, 2, boxed);
        assert!(f <= at_start);
        assert!(g[0] <= 12.0 && g[1] <= 12.0);
        // One sample: no spread in t, nothing to do.
        let one = Samples {
            px: vec![0],
            x: vec![0.0],
            y: vec![0.0],
            srgb: vec![[0.5; 3]],
            lin: vec![[0.5; 3]],
        };
        let cols1 = one.colors(Interp::Srgb);
        let mut r1 = Resid1d::new(&one, &[0], &cols1);
        let (g1, _) = r1.refine_spline([3.0, 4.0, 0.0, 0.0], 2, free);
        assert_eq!(g1, [3.0, 4.0, 0.0, 0.0]);
    }

    #[test]
    fn the_search_finds_a_clamped_ellipse_from_the_gradient_line_seed() {
        // The artist's elliptical radial with a flat core: the search starts at the
        // gradient-line seed and returns the circle it refined and the ellipse.
        let (centre, angle, aspect) = ((25.3, 23.6), 0.6, 1.5);
        let s = clamped(52, centre, 20.0, angle, aspect);
        let cols = s.colors(Interp::Srgb);
        let found = profile_geometries(&s, &cols, 52);
        assert_eq!(found.len(), 2, "a circle and an ellipse");
        let FillModel::Radial {
            c,
            aspect: k,
            angle: a,
            ..
        } = found[1]
        else {
            panic!("not radial");
        };
        let err = (c.0 - centre.0).hypot(c.1 - centre.1);
        assert!(err < 0.3, "centre {c:?} off by {err}");
        assert!((k / aspect - 1.0).abs() < 0.05, "aspect {k}");
        assert!((a - angle).abs().to_degrees() < 3.0, "angle {a}");
        // Too few samples: nothing to search.
        let tiny = clamped(52, centre, 1.5, 0.0, 1.0);
        assert!(profile_geometries(&tiny, &tiny.colors(Interp::Srgb), 52).is_empty());
    }

    /// Samples of a disc of radius `r` about `(cx, cy)` on a `w × w` grid, coloured by `f`.
    fn disc(w: usize, cx: f64, cy: f64, r: f64, f: impl Fn(f64, f64) -> [f64; 3]) -> Samples {
        let mut s = Samples {
            px: vec![],
            x: vec![],
            y: vec![],
            srgb: vec![],
            lin: vec![],
        };
        for p in 0..w * w {
            let (x, y) = ((p % w) as f64, (p / w) as f64);
            if (x - cx).hypot(y - cy) > r {
                continue;
            }
            let c = f(x, y);
            s.px.push(p);
            s.x.push(x);
            s.y.push(y);
            s.lin.push(c);
            s.srgb.push([c[0] as f32, c[1] as f32, c[2] as f32]);
        }
        s
    }

    #[test]
    fn the_spline_score_never_exceeds_the_line_score() {
        // The hat basis spans every line in t (its knots cover the samples' range), so its
        // least-squares residual can only be lower; on a field that is not a function of
        // the distance at all, both are positive.
        let s = disc(24, 11.5, 11.5, 11.0, |x, y| {
            [0.1 + 0.02 * x, 0.3 + 0.01 * y, 0.5 + 0.001 * x * y]
        });
        let idx: Vec<usize> = (0..s.px.len()).collect();
        let cols = s.colors(Interp::Srgb);
        let mut rz = Resid1d::new(&s, &idx, &cols);
        for &c in &[(3.0, 4.0), (11.5, 11.5), (20.0, 7.5), (-6.0, 30.0)] {
            let l = rz.radial(c);
            let p = rz.score_at(&[c.0, c.1, 0.0, 0.0]);
            assert!(p <= l + 1e-9, "{c:?}: spline {p} line {l}");
            assert!(p > 0.0);
        }
        // A single abscissa (one sample): the total variance, which is 0 for one colour.
        let one = disc(3, 1.0, 1.0, 0.0, |_, _| [0.2, 0.4, 0.6]);
        let cols1 = one.colors(Interp::Srgb);
        let mut r1 = Resid1d::new(&one, &[0], &cols1);
        assert_eq!(r1.score_at(&[0.0, 0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn a_clamped_circle_is_centred_by_the_spline_score_and_not_by_the_line() {
        // A flat core of radius 7 and a ramp from there to the rim, centred off the
        // region's middle: the artist's clamped radial. The search finds the centre, and
        // the spline residual at the truth is far below the line's there.
        let centre = (20.3, 17.6);
        let s = disc(48, 23.0, 21.0, 16.0, move |x, y| {
            let d = (x - centre.0).hypot(y - centre.1);
            let k = (d - 7.0).max(0.0);
            [0.85 - 0.03 * k, 0.6 - 0.02 * k, 0.3 + 0.01 * k]
        });
        let cols = s.colors(Interp::LinearRgb);
        let found = profile_geometries(&s, &cols, 48);
        let Some(FillModel::Radial { c, .. }) = found.first() else {
            panic!("no circle");
        };
        let err = (c.0 - centre.0).hypot(c.1 - centre.1);
        assert!(err < 0.5, "centre {c:?} off by {err}");
        let idx: Vec<usize> = (0..s.px.len()).collect();
        let mut rz = Resid1d::new(&s, &idx, &cols);
        let line = rz.radial(centre);
        assert!(rz.score_at(&[centre.0, centre.1, 0.0, 0.0]) < 0.05 * line);
    }

    #[test]
    fn the_small_solver_solves_and_refuses_singular_systems() {
        let a = [
            [4.0, 1.0, 0.0, 0.0],
            [1.0, 3.0, 0.0, 0.0],
            [0.0; 4],
            [0.0; 4],
        ];
        let x = solve_small(a, [1.0, 2.0, 0.0, 0.0], 2).expect("regular");
        assert!((4.0 * x[0] + x[1] - 1.0).abs() < 1e-12);
        assert!((x[0] + 3.0 * x[1] - 2.0).abs() < 1e-12);
        let sing = [
            [1.0, 2.0, 0.0, 0.0],
            [2.0, 4.0, 0.0, 0.0],
            [0.0; 4],
            [0.0; 4],
        ];
        assert!(solve_small(sing, [1.0, 1.0, 0.0, 0.0], 2).is_none());
    }
}
