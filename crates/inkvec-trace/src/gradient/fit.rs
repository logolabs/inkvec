//! The ramp fitters: flat, linear, circular and elliptical radial.
//!
//! Each fitter takes a region's [`Samples`] and the samples' colours in one fitting space
//! (`cols`: linear light or sRGB, both 0..1, see [`Interp`]) and returns one
//! [`FillModel`] with two end stops, or `None` when the region cannot support that model.
//! Scoring, the choice between models and the multi-stop refinement happen in the parent
//! module ([`super::fit_samples`]); these functions only place the geometry and the
//! stops. Every ramp reduces to the same one-dimensional problem once its geometry is
//! fixed: a scalar coordinate `t` per sample (distance along an axis, or from a centre)
//! and a least-squares line `colour = a + g·t` per channel ([`fit_1d`]). The fitters
//! differ in how they search for the geometry that makes that line fit best.
//!
//! Positions are pixel centres, px. Moved out of `gradient.rs` unchanged, to keep that
//! file under the workspace's line cap.

use super::*;

/// Least squares of `colour = a + g·t` per channel.
///
/// Ordinary least squares on centred moments: with `t̄`, `c̄` the means,
/// `S_tt = Σ (t_i − t̄)²`, `S_tc = Σ (t_i − t̄)(c_i − c̄)` and `S_cc = Σ (c_i − c̄)²`
/// per channel, the slope is `g = S_tc / S_tt`, the intercept `a = c̄ − g·t̄`, and the
/// residual sum of squares `S_cc − g²·S_tt` (clamped at 0 against rounding).
///
/// Returns the residual summed over the three channels, `a` and `g`. When `t` has no
/// spread (`S_tt ≤ 1e-12`) the slope is 0 and the line is the mean colour. An empty
/// input divides by zero and yields NaN; callers guarantee at least
/// [`MIN_GRADIENT_PIXELS`] samples.
pub(super) fn fit_1d(cols: &[[f64; 3]], t: &[f64]) -> (f64, [f64; 3], [f64; 3]) {
    let n = cols.len() as f64;
    let tbar = t.iter().sum::<f64>() / n;
    let cbar = mean3(cols);
    let mut stt = 0.0;
    let mut stc = [0.0; 3];
    let mut scc = [0.0; 3];
    for (c, &ti) in cols.iter().zip(t) {
        let dt = ti - tbar;
        stt += dt * dt;
        for k in 0..3 {
            let dc = c[k] - cbar[k];
            stc[k] += dt * dc;
            scc[k] += dc * dc;
        }
    }
    let mut g = [0.0; 3];
    let mut a = [0.0; 3];
    let mut resid = 0.0;
    for k in 0..3 {
        g[k] = if stt > 1e-12 { stc[k] / stt } else { 0.0 };
        a[k] = cbar[k] - g[k] * tbar;
        resid += (scc[k] - g[k] * g[k] * stt).max(0.0);
    }
    (resid, a, g)
}

/// The half of [`fit_1d`] a centre search recomputes for nothing.
///
/// `fit_1d` forms five sums. Two of them -- the colour mean and the per-channel colour
/// variance -- are properties of the samples alone and do not mention `t`, so a search
/// that moves a centre, an angle or an aspect around recomputes the same two numbers on
/// every evaluation. There are up to 600 of those for a circular gradient and 836 for an
/// elliptical one, on a subsample of a thousand pixels: the elliptical fit was 663 ms of
/// the 683 ms `merge_bands` spent on one emoji.
///
/// So they are formed once, here, over the same slice in the same order -- and the sums
/// that do depend on the geometry are formed exactly as `fit_1d` forms them. Every number
/// is bit for bit the one the search would have arrived at anyway; only the arithmetic
/// that was redundant is gone. The distances land in a buffer the struct owns, which also
/// retires one allocation per evaluation.
pub(super) struct Resid1d<'a> {
    /// The sample coordinates, gathered contiguous: the search reads them once per
    /// evaluation and the stride made every read a scattered one.
    xs: Vec<f64>,
    ys: Vec<f64>,
    /// The subsample's colours in the fitting space, parallel to `xs`/`ys`.
    cols: &'a [[f64; 3]],
    /// Per-channel mean colour `c̄`.
    cbar: [f64; 3],
    /// Per-channel `S_cc = Σ (c_i − c̄)²`.
    scc: [f64; 3],
    /// Scratch: the geometric coordinate `t_i` of each sample for the current candidate.
    t: Vec<f64>,
}

impl<'a> Resid1d<'a> {
    /// Precompute the geometry-free sums over the samples `idx` of `s`, whose colours
    /// `cols` must be given in the same order (`cols[j]` belongs to sample `idx[j]`).
    pub(super) fn new(s: &Samples, idx: &[usize], cols: &'a [[f64; 3]]) -> Self {
        let cbar = mean3(cols);
        let mut scc = [0.0; 3];
        for c in cols {
            for k in 0..3 {
                let dc = c[k] - cbar[k];
                scc[k] += dc * dc;
            }
        }
        Self {
            xs: idx.iter().map(|&i| s.x[i]).collect(),
            ys: idx.iter().map(|&i| s.y[i]).collect(),
            cols,
            cbar,
            scc,
            t: vec![0.0; idx.len()],
        }
    }

    /// The 1-D residual with `t_i = |P_i − c|`, the Euclidean distance to a candidate
    /// centre `c` (px): the residual a circular gradient centred there would leave.
    pub(super) fn radial(&mut self, c: (f64, f64)) -> f64 {
        for j in 0..self.t.len() {
            self.t[j] = ((self.xs[j] - c.0).powi(2) + (self.ys[j] - c.1).powi(2)).sqrt();
        }
        self.finish()
    }

    /// The 1-D residual with `t_i` the elliptical distance about `c` in the frame
    /// `(angle, aspect)`: `u = dx·cos + dy·sin`, `v = (−dx·sin + dy·cos)·k`,
    /// `t = √(u² + v²)`, where `(dx, dy) = P_i − c`, `sn`/`cs` are the angle's sine and
    /// cosine, and `k` is the aspect. This is [`super::eval::radial_t_rot`] before its
    /// division by the radius, which a line fit absorbs into its slope.
    pub(super) fn elliptic(&mut self, c: (f64, f64), sn: f64, cs: f64, k: f64) -> f64 {
        for j in 0..self.t.len() {
            let (dx, dy) = (self.xs[j] - c.0, self.ys[j] - c.1);
            let u = dx * cs + dy * sn;
            let v = (-dx * sn + dy * cs) * k;
            self.t[j] = (u * u + v * v).sqrt();
        }
        self.finish()
    }

    /// The residual of the least-squares line through `(t, colour)`, the `.0` of `fit_1d`:
    /// `Σ_ch max(S_cc − S_tc²/S_tt, 0)` with `S_tc`, `S_tt` formed from the current `t`.
    fn finish(&self) -> f64 {
        let n = self.cols.len() as f64;
        let tbar = self.t.iter().sum::<f64>() / n;
        let mut stt = 0.0;
        let mut stc = [0.0; 3];
        for (c, &ti) in self.cols.iter().zip(&self.t) {
            let dt = ti - tbar;
            stt += dt * dt;
            for k in 0..3 {
                stc[k] += dt * (c[k] - self.cbar[k]);
            }
        }
        let mut resid = 0.0;
        for k in 0..3 {
            let g = if stt > 1e-12 { stc[k] / stt } else { 0.0 };
            resid += (self.scc[k] - g * g * stt).max(0.0);
        }
        resid
    }
}

/// The flat fill: the per-channel median of the samples' sRGB colours, over a strided
/// subsample of at most [`fit_cap`] samples. No samples gives black.
///
/// Not the mean in linear light, the average of the light the region emits, although
/// that is the physically natural choice. The flat model is scored by the same sRGB
/// residual as every other candidate, so it has to be the colour that minimises that
/// residual, not the linear-light mean converted back: for a dark region with a few light outliers (blend pixels the evidence test
/// let through, a lost dot) the linear mean lands at a grey no pixel has, every pure
/// pixel then pays for the outliers, and a gradient that puts the true colour in the
/// middle and the outliers at its rim wins on solid black (luanti: flat fitted at 0.16
/// on a region whose interior is 0.00). The per-channel sRGB median is a robust estimate
/// of the sRGB residual's optimum: a minority of outliers cannot move it, and on a clean
/// region it agrees with the sRGB mean that would minimise the squared residual exactly.
pub(super) fn fit_flat(s: &Samples) -> FillModel {
    let n = s.len();
    if n == 0 {
        return FillModel::Flat([0.0; 3]);
    }
    let stride = (n / fit_cap()).max(1);
    let mut c = [0.0f32; 3];
    for k in 0..3 {
        let mut v: Vec<f32> = (0..n).step_by(stride).map(|i| s.srgb[i][k]).collect();
        let m = v.len() / 2;
        let (_, med, _) = v.select_nth_unstable_by(m, |a, b| a.total_cmp(b));
        c[k] = *med;
    }
    FillModel::Flat(c)
}

/// Linear gradient: axis by PCA of the colour-versus-position slope, direction refined
/// by golden-section search on the exact 1-D residual, stops by least squares.
///
/// The full affine model `colour = a + B·(x, y)` is a 3x2 slope matrix `B`; a linear
/// gradient is the rank-1 case where every channel varies along one direction. The top
/// right-singular vector of `B` is that direction's PCA estimate. Given the direction,
/// the residual of the 1-D fit is a closed form in the second moments, so refining the
/// angle costs nothing per step: a coarse scan around the PCA angle guards against the
/// estimate being poor when the channels disagree, and golden section finishes it.
///
/// In symbols, with positions centred on the centroid `(x̄, ȳ)` and colours on `c̄`:
///
/// * second moments `Sxx, Sxy, Syy` of position and `Sxc, Syc, Scc` per channel;
/// * affine slope per channel `(Bx, By) = [Sxx Sxy; Sxy Syy]⁻¹ (Sxc, Syc)`;
/// * seed angle `θ0 = ½·atan2(2·M01, M00 − M11)` with `M = Σ_ch (Bx, By)ᵀ(Bx, By)`,
///   the leading eigenvector of the 2x2 matrix `M`;
/// * residual along direction `d = (cos θ, sin θ)`:
///   `R(θ) = Σ_ch Scc − Σ_ch (d·(Sxc, Syc))² / (dᵀ S d)`, the unexplained variance of
///   the best line in `t = d·(P − P̄)`;
/// * scan `θ0 ± 45°` in 1° steps, then 40 golden-section steps within ±1° of the best.
///
/// The stops are the line's values at the extreme projections `t_min`, `t_max`, and the
/// axis runs between the corresponding points, so the gradient spans exactly the region.
/// `None` when there are fewer than [`MIN_GRADIENT_PIXELS`] samples, the positions are
/// collinear (a 1-pixel line has no 2-D axis), or the projections have no spread.
pub(super) fn fit_linear(s: &Samples, cols: &[[f64; 3]], space: Interp) -> Option<FillModel> {
    let n = s.len();
    if n < MIN_GRADIENT_PIXELS {
        return None;
    }
    let (xc, yc) = s.centroid();
    let cbar = mean3(cols);
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    let mut sxc = [0.0; 3];
    let mut syc = [0.0; 3];
    let mut scc = [0.0; 3];
    for (i, c) in cols.iter().enumerate() {
        let (dx, dy) = (s.x[i] - xc, s.y[i] - yc);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
        for k in 0..3 {
            let dc = c[k] - cbar[k];
            sxc[k] += dx * dc;
            syc[k] += dy * dc;
            scc[k] += dc * dc;
        }
    }
    let det = sxx * syy - sxy * sxy;
    if det <= 1e-9 * (sxx + syy).powi(2) {
        return None; // positions collinear: no 2-D axis to find
    }

    // PCA of the affine slope matrix.
    let (mut m00, mut m01, mut m11) = (0.0, 0.0, 0.0);
    for k in 0..3 {
        let bx = (syy * sxc[k] - sxy * syc[k]) / det;
        let by = (sxx * syc[k] - sxy * sxc[k]) / det;
        m00 += bx * bx;
        m01 += bx * by;
        m11 += by * by;
    }
    let theta0 = 0.5 * (2.0 * m01).atan2(m00 - m11);

    // Exact residual of the 1-D fit along direction theta, from the moments.
    let resid = |theta: f64| -> f64 {
        let (c, sn) = (theta.cos(), theta.sin());
        let sss = c * c * sxx + 2.0 * c * sn * sxy + sn * sn * syy;
        if sss <= 1e-12 {
            return f64::MAX;
        }
        let mut explained = 0.0;
        for k in 0..3 {
            let ssc = c * sxc[k] + sn * syc[k];
            explained += ssc * ssc / sss;
        }
        scc.iter().sum::<f64>() - explained
    };

    let deg = std::f64::consts::PI / 180.0;
    let mut best = (theta0, resid(theta0));
    for k in -45..=45 {
        let th = theta0 + k as f64 * deg;
        let r = resid(th);
        if r < best.1 {
            best = (th, r);
        }
    }
    let (mut lo, mut hi) = (best.0 - deg, best.0 + deg);
    let phi = 0.5 * (5.0f64.sqrt() - 1.0);
    let (mut a, mut b) = (hi - phi * (hi - lo), lo + phi * (hi - lo));
    let (mut fa, mut fb) = (resid(a), resid(b));
    for _ in 0..40 {
        if fa < fb {
            hi = b;
            b = a;
            fb = fa;
            a = hi - phi * (hi - lo);
            fa = resid(a);
        } else {
            lo = a;
            a = b;
            fa = fb;
            b = lo + phi * (hi - lo);
            fb = resid(b);
        }
    }
    let theta = 0.5 * (lo + hi);
    let (dc, ds) = (theta.cos(), theta.sin());

    let t: Vec<f64> = (0..n)
        .map(|i| (s.x[i] - xc) * dc + (s.y[i] - yc) * ds)
        .collect();
    let smin = t.iter().cloned().fold(f64::MAX, f64::min);
    let smax = t.iter().cloned().fold(f64::MIN, f64::max);
    if smax - smin < 1e-6 {
        return None;
    }
    let (_, a, g) = fit_1d(cols, &t);
    let at = |sv: f64| [a[0] + g[0] * sv, a[1] + g[1] * sv, a[2] + g[2] * sv];
    Some(FillModel::Linear {
        p0: (xc + smin * dc, yc + smin * ds),
        p1: (xc + smax * dc, yc + smax * ds),
        c0: from_space(at(smin), space),
        c1: from_space(at(smax), space),
        interp: space,
        mids: Vec::new(),
    })
}

/// Principal direction of the colours, by power iteration on their covariance.
///
/// Forms the 3x3 scatter matrix `C = Σ_i (c_i − c̄)(c_i − c̄)ᵀ` of the colours (in
/// whatever space `cols` is in) and iterates `v ← C v / |C v|` thirty times from the grey
/// direction `(1, 1, 1)/√3`. The result is a unit vector; its sign is arbitrary. `None`
/// when the colours have no variance (trace of `C` ≤ 1e-12) or the iterate collapses,
/// i.e. every colour difference is orthogonal to the start vector's image. Used by
/// [`fit_radial`] to reduce colour to one scalar whose spatial gradient points at the
/// centre.
pub(super) fn color_axis(cols: &[[f64; 3]]) -> Option<[f64; 3]> {
    let cbar = mean3(cols);
    let mut cov = [[0.0f64; 3]; 3];
    for c in cols {
        let d = [c[0] - cbar[0], c[1] - cbar[1], c[2] - cbar[2]];
        for (i, row) in cov.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v += d[i] * d[j];
            }
        }
    }
    let trace = cov[0][0] + cov[1][1] + cov[2][2];
    if trace <= 1e-12 {
        return None;
    }
    let mut v = [1.0 / 3.0f64.sqrt(); 3];
    for _ in 0..30 {
        let mut nv = [0.0; 3];
        for i in 0..3 {
            for j in 0..3 {
                nv[i] += cov[i][j] * v[j];
            }
        }
        let norm = (nv[0] * nv[0] + nv[1] * nv[1] + nv[2] * nv[2]).sqrt();
        if norm <= 1e-15 {
            return None;
        }
        v = [nv[0] / norm, nv[1] / norm, nv[2] / norm];
    }
    Some(v)
}

/// The weighted least-squares point nearest every gradient line of the scalar field `f`
/// (one value per sample), the seed of [`fit_radial`]'s centre search; see there for the
/// formula. `None` when the 2x2 system is near singular (`det ≤ 1e-9·trace²`), which
/// happens when the lines are all parallel (the data is a linear ramp) or there are none.
fn gradient_line_centre(s: &Samples, f: &[f64], w: usize) -> Option<(f64, f64)> {
    let n = s.len();
    // Weighted least squares for the point nearest all gradient lines.
    let (mut a00, mut a01, mut a11, mut b0, mut b1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let stride = (n / fit_cap()).max(1);
    for i in (0..n).step_by(stride) {
        let p = s.px[i];
        let x = p % w;
        if x == 0 || p < w {
            continue;
        }
        let (Some(l), Some(r), Some(u), Some(d)) = (
            s.sample_at(p - 1),
            s.sample_at(p + 1),
            s.sample_at(p - w),
            s.sample_at(p + w),
        ) else {
            continue;
        };
        let gx = 0.5 * (f[r] - f[l]);
        let gy = 0.5 * (f[d] - f[u]);
        let mag = (gx * gx + gy * gy).sqrt();
        if mag <= 1e-9 {
            continue;
        }
        // Perpendicular to the gradient: distance of C from the line through P along g.
        let (nx, ny) = (-gy / mag, gx / mag);
        let rhs = nx * s.x[i] + ny * s.y[i];
        a00 += mag * nx * nx;
        a01 += mag * nx * ny;
        a11 += mag * ny * ny;
        b0 += mag * nx * rhs;
        b1 += mag * ny * rhs;
    }
    let det = a00 * a11 - a01 * a01;
    if det > 1e-9 * (a00 + a11).powi(2) {
        Some(((a11 * b0 - a01 * b1) / det, (a00 * b1 - a01 * b0) / det))
    } else {
        None
    }
}

/// Radial gradient: centre by a weighted least-squares intersection of the gradient
/// lines, refined by pattern search on the exact 1-D residual; stops by least squares.
///
/// In a radial gradient the spatial gradient of any scalar function of colour points at
/// (or away from) the centre, so every interior pixel with a measurable gradient
/// contributes a line the centre must lie on. The centre that minimises the weighted
/// squared distance to all of those lines is a 2x2 linear solve. That estimate is then
/// polished by pattern search on the residual of `colour = a + g·|P - C|`, which is a
/// 1-D least squares per candidate centre.
///
/// The centre is confined to within one bounding-box width of the region. Any radial
/// gradient with its centre farther out than that is, across this region, a linear
/// gradient — and should be described as one.
///
/// In symbols: `f_i = c_i·v` is each colour projected on the principal colour axis `v`
/// ([`color_axis`]); `g_i = (∂f/∂x, ∂f/∂y)` its central difference (only at samples whose
/// four neighbours are samples); `n_i = (−g_y, g_x)/|g_i|` the normal to the gradient
/// line through `P_i`. The centre minimises `Σ_i |g_i| (n_i·(C − P_i))²`, whose normal
/// equations are the 2x2 system `A C = b` with `A = Σ |g| n nᵀ`, `b = Σ |g| n (n·P)`.
/// When `A` is near singular (all lines parallel: the data is a linear ramp) the start
/// is the centroid. The pattern search then tries steps of 4 px along the eight compass
/// directions, halves the step whenever none improves the residual, and stops below
/// 0.03 px or after 600 evaluations, each on at most [`CENTRE_SEARCH_SAMPLES`] samples.
///
/// The radius `r` is the largest sample distance from the centre, so the whole region
/// lies inside `t ∈ [0, 1]`; the stops are the fitted line at `ρ = 0` and `ρ = r`.
/// `None` below [`MIN_GRADIENT_PIXELS`] samples, with no colour variance, or when
/// `r < 0.5` px. `w` is the image width, to find neighbours by pixel index.
pub(super) fn fit_radial(
    s: &Samples,
    cols: &[[f64; 3]],
    space: Interp,
    w: usize,
) -> Option<FillModel> {
    let n = s.len();
    if n < MIN_GRADIENT_PIXELS {
        return None;
    }
    let axis = color_axis(cols)?;
    let f: Vec<f64> = cols
        .iter()
        .map(|c| c[0] * axis[0] + c[1] * axis[1] + c[2] * axis[2])
        .collect();

    let mut c = gradient_line_centre(s, &f, w).unwrap_or_else(|| s.centroid());

    let xmin = s.x.iter().cloned().fold(f64::MAX, f64::min);
    let xmax = s.x.iter().cloned().fold(f64::MIN, f64::max);
    let ymin = s.y.iter().cloned().fold(f64::MAX, f64::min);
    let ymax = s.y.iter().cloned().fold(f64::MIN, f64::max);
    let (bw, bh) = ((xmax - xmin).max(4.0), (ymax - ymin).max(4.0));
    let clamp = |p: (f64, f64)| {
        (
            p.0.clamp(xmin - bw, xmax + bw),
            p.1.clamp(ymin - bh, ymax + bh),
        )
    };
    c = clamp(c);

    let radii = |c: (f64, f64)| -> Vec<f64> {
        (0..n)
            .map(|i| ((s.x[i] - c.0).powi(2) + (s.y[i] - c.1).powi(2)).sqrt())
            .collect()
    };
    // The centre search evaluates its residual up to six hundred times; on a strided
    // subsample each evaluation is O(MAX_FIT_SAMPLES) instead of O(n). The final fit
    // below still uses every pixel.
    let cstride = (n / CENTRE_SEARCH_SAMPLES).max(1);
    let idx: Vec<usize> = (0..n).step_by(cstride).collect();
    let cols_sub: Vec<[f64; 3]> = idx.iter().map(|&i| cols[i]).collect();
    let mut rz = Resid1d::new(s, &idx, &cols_sub);

    let mut best = rz.radial(c);
    let mut step = 4.0;
    let mut evals = 0;
    const DIRS: [(f64, f64); 8] = [
        (1.0, 0.0),
        (-1.0, 0.0),
        (0.0, 1.0),
        (0.0, -1.0),
        (1.0, 1.0),
        (1.0, -1.0),
        (-1.0, 1.0),
        (-1.0, -1.0),
    ];
    while step > 0.03 && evals < 600 {
        let mut improved = false;
        for (dx, dy) in DIRS {
            let cand = clamp((c.0 + dx * step, c.1 + dy * step));
            let r = rz.radial(cand);
            evals += 1;
            if r < best - 1e-12 {
                best = r;
                c = cand;
                improved = true;
                break;
            }
        }
        if !improved {
            step *= 0.5;
        }
    }

    let rho = radii(c);
    let r = rho.iter().cloned().fold(f64::MIN, f64::max);
    if r < 0.5 {
        return None;
    }
    let (_, a, g) = fit_1d(cols, &rho);
    Some(FillModel::Radial {
        c,
        r,
        c0: from_space(a, space),
        c1: from_space([a[0] + g[0] * r, a[1] + g[1] * r, a[2] + g[2] * r], space),
        interp: space,
        aspect: 1.0,
        angle: 0.0,
        mids: Vec::new(),
    })
}

/// Largest aspect ratio an elliptical gradient may take. Beyond this the gradient is,
/// across any region it could plausibly fill, a linear one.
const MAX_ASPECT: f64 = 8.0;

/// Elliptical radial gradient: the circular fit's centre, then a pattern search over
/// centre, orientation and aspect on the exact 1-D residual.
///
/// The centre search of [`fit_radial`] is a good start even when the truth is
/// elliptical - the weighted line intersection lands near the middle of the ellipse -
/// but its residual is left with the whole anisotropy. Four coordinates, `(cx, cy,
/// angle, ln aspect)`, are then searched together: with the aspect free the centre
/// often moves, so the two cannot be settled one after the other. Each evaluation is a
/// 1-D least squares on the strided subsample; the final stops use every pixel.
///
/// The search is a compass (coordinate pattern) search: an angle sweep every 15° at
/// aspects 2 and 3 seeds the orientation, then each coordinate is stepped by
/// `±step·scale` with scales `(1 px, 1 px, 10°, 0.25)`, the first improvement taken,
/// and the step halved when none improves, from 4 down to 0.03 or 800 evaluations. The
/// centre stays within one bounding-box size of the region and `ln aspect` within
/// `[0, ln MAX_ASPECT]`. The residual minimised is [`Resid1d::elliptic`].
///
/// `circular` must be the [`fit_radial`] result for the same samples (anything else
/// gives `None`). Also `None` below `2·MIN_GRADIENT_PIXELS` samples, when the best
/// aspect is under 1.02 (the circle already describes it), or when the radius is under
/// 0.5 px.
pub(super) fn fit_radial_elliptic(
    s: &Samples,
    cols: &[[f64; 3]],
    space: Interp,
    circular: &FillModel,
) -> Option<FillModel> {
    let FillModel::Radial { c: c_start, .. } = *circular else {
        return None;
    };
    let n = s.len();
    if n < 2 * MIN_GRADIENT_PIXELS {
        return None;
    }
    let cstride = (n / CENTRE_SEARCH_SAMPLES).max(1);
    let idx: Vec<usize> = (0..n).step_by(cstride).collect();
    let cols_sub: Vec<[f64; 3]> = idx.iter().map(|&i| cols[i]).collect();
    let mut rz = Resid1d::new(s, &idx, &cols_sub);

    let xmin = s.x.iter().cloned().fold(f64::MAX, f64::min);
    let xmax = s.x.iter().cloned().fold(f64::MIN, f64::max);
    let ymin = s.y.iter().cloned().fold(f64::MAX, f64::min);
    let ymax = s.y.iter().cloned().fold(f64::MIN, f64::max);
    let (bw, bh) = ((xmax - xmin).max(4.0), (ymax - ymin).max(4.0));

    // State: centre, angle, ln(aspect). Radius is not searched: the residual of a
    // 1-D fit on unnormalised elliptical distance is what the search minimises, and
    // the radius falls out of the final fit as the largest distance seen.
    let mut st = [c_start.0, c_start.1, 0.0, 0.0];
    let clamp = |st: [f64; 4]| -> [f64; 4] {
        [
            st[0].clamp(xmin - bw, xmax + bw),
            st[1].clamp(ymin - bh, ymax + bh),
            st[2],
            st[3].clamp(0.0, MAX_ASPECT.ln()),
        ]
    };
    let resid = |rz: &mut Resid1d, st: [f64; 4]| -> f64 {
        let (sn, cs) = st[2].sin_cos();
        rz.elliptic((st[0], st[1]), sn, cs, st[3].exp())
    };

    // Seed the orientation: a coarse sweep at a moderate aspect is cheap and keeps the
    // pattern search out of the wrong local minimum of the angle.
    let mut best = resid(&mut rz, st);
    for deg in (0..180).step_by(15) {
        // Aspects 2 and 3. There is no need for the reciprocals: the angle sweep
        // already reaches a 1:2 ellipse as a 2:1 one turned ninety degrees. (A fourth
        // seed used to sit here written `0.5f64.ln() * -1.0`, which is ln 2 — the same
        // candidate as the second, scored twice for nothing.)
        for &la in &[2.0f64.ln(), 3.0f64.ln()] {
            let cand = clamp([st[0], st[1], (deg as f64).to_radians(), la]);
            let r = resid(&mut rz, cand);
            if r < best - 1e-12 {
                best = r;
                st = cand;
            }
        }
    }
    let scale = [1.0, 1.0, 10f64.to_radians(), 0.25];
    let mut step = 4.0;
    let mut evals = 0;
    while step > 0.03 && evals < 800 {
        let mut improved = false;
        'axes: for ax in 0..4 {
            for sign in [1.0, -1.0] {
                let mut cand = st;
                cand[ax] += sign * step * scale[ax];
                let cand = clamp(cand);
                let r = resid(&mut rz, cand);
                evals += 1;
                if r < best - 1e-12 {
                    best = r;
                    st = cand;
                    improved = true;
                    break 'axes;
                }
            }
        }
        if !improved {
            step *= 0.5;
        }
    }
    let aspect = st[3].exp();
    if aspect < 1.02 {
        return None; // the circle already has it
    }
    let (sn, cs) = st[2].sin_cos();
    let rho: Vec<f64> = (0..n)
        .map(|i| {
            let (dx, dy) = (s.x[i] - st[0], s.y[i] - st[1]);
            let u = dx * cs + dy * sn;
            let v = (-dx * sn + dy * cs) * aspect;
            (u * u + v * v).sqrt()
        })
        .collect();
    let r = rho.iter().cloned().fold(f64::MIN, f64::max);
    if r < 0.5 {
        return None;
    }
    let (_, a, g) = fit_1d(cols, &rho);
    Some(FillModel::Radial {
        c: (st[0], st[1]),
        r,
        c0: from_space(a, space),
        c1: from_space([a[0] + g[0] * r, a[1] + g[1] * r, a[2] + g[2] * r], space),
        interp: space,
        aspect,
        angle: st[2],
        mids: Vec::new(),
    })
}
