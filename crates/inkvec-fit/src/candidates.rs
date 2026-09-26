//! Primitive candidate generation and scoring for the multi-model alphabet (DESIGN.md S4).
//!
//! Evaluates candidate models — straight lines, circular arcs, elliptical arcs,
//! moment-matched G1 cubic Béziers, and free-tangent cubic Béziers — for any sub-polyline span (i, j).

use crate::tangents::{break_cost, turn_angle, Tangents};
use crate::{FitConfig, PARAMS_LINE};
use inkvec_core::{Point, Vec2};
use kurbo::common::{factor_quartic_inner, solve_cubic, solve_quadratic};
use std::sync::OnceLock;

/// Maximum points a cubic's residual is evaluated on.
pub const MAX_RESIDUAL_SAMPLES: usize = 32;

/// Newton projection steps when measuring point distance to a cubic Bézier.
const NEWTON_STEPS: usize = 3;

/// Maximum control arm length as a multiple of chord length.
pub const MAX_ARM: f64 = 1.0;

/// How far a free cubic's end tangent may depart from the estimated one before the fit is
/// treated as describing something other than this boundary.
const FREE_MAX_SWING: f64 = 75.0;

/// Parameters charged to a cubic segment: 6 unless the trace in progress asked for another
/// price (see [`crate::cost`]), or an experiment set `INKVEC_PARAMS_CUBIC`.
pub fn params_cubic() -> f64 {
    crate::cost::cubic_params()
}

/// The elliptical candidate, separately from the circular one, so the two can be priced
/// against each other.
pub(crate) fn ellipses_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("INKVEC_NO_ELLIPSE").is_none())
}

/// The per-span arc candidate. `INKVEC_NO_ARCS=1` takes it out of the alphabet, which is
/// how the fitter is measured with and without it.
pub(crate) fn arcs_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("INKVEC_NO_ARCS").is_none())
}

/// Off unless `INKVEC_FREE_CUBIC` is set, because it does not pay.
///
/// What it costs is one axis, not three. On the 246-icon gate set it is *better* on
/// dE00 (-0.21%) and on parameter ratio (-1.08%), and fails only turning (+5.44%). The
/// wobble is real and not a metric artefact: `turning` is the sawtooth detector
/// (`svgeval.py:538`), which exists to catch a boundary that renders well and is shaped
/// wrong.
///
/// Swept against [`wobble_penalty_factor`], which had not been done. There is no
/// setting that keeps the parameter win and the wobble both:
///
/// | wobble | dE00 | turning | ratio |
/// |---|---|---|---|
/// | 1.0 | -0.21% | +5.44% | **-1.08%** |
/// | 1.5 | -0.52% | +2.48% | +0.35% |
/// | 2.0 | -0.34% | +1.58% | +0.93% |
/// | 2.5 | -0.77% | +1.17% | +1.11% |
/// | 3.5 | -0.63% | **+0.34%** | +1.35% |
/// | 5.0 | -0.73% | **+0.01%** | +1.53% |
///
/// By the time turning is inside the gate the ratio is worse than baseline, so the free
/// cubic is available as a *colour* win costing parameters (3.5 passes all three gates
/// at dE00 -0.63%), which is the wrong direction for a project whose loose axis is the
/// parameter ratio. That is why it is still off.
pub(crate) fn free_cubic_enabled() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var_os("INKVEC_FREE_CUBIC").is_some())
}

/// Normalize vector to unit length, if non-degenerate.
#[inline]
pub(crate) fn unit(v: Vec2) -> Option<Vec2> {
    let n = v.norm();
    if n < 1e-12 || !n.is_finite() {
        None
    } else {
        Some(Vec2 {
            x: v.x / n,
            y: v.y / n,
        })
    }
}

/// Green's-theorem contributions of one straight edge.
#[inline]
pub(crate) fn edge_terms(a: Point, b: Point) -> (f64, f64, f64) {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    (
        dx * (a.y + 0.5 * dy),
        dx * (a.x * a.y + 0.5 * (a.x * dy + a.y * dx) + dx * dy / 3.0),
        dx * (a.y * a.y + a.y * dy + dy * dy / 3.0),
    )
}

/// Raw (∫ y dx, ∫ x y dx, ∫ y² dx) computed directly over points without prefix sums.
pub(crate) fn raw_moments_direct(pts: &[Point], i: usize, j: usize) -> (f64, f64, f64) {
    let mut a = 0.0;
    let mut x = 0.0;
    let mut y = 0.0;
    for k in i..j {
        let (da, dx, dy) = edge_terms(pts[k], pts[k + 1]);
        a += da;
        x += dx;
        y += dy;
    }
    (a, x, y)
}

/// Smaller eigenvalue of the weighted scatter matrix, from raw sums.
pub(crate) fn scatter_min_eigen(w: f64, sx: f64, sy: f64, sxx: f64, syy: f64, sxy: f64) -> f64 {
    let cxx = sxx - sx * sx / w;
    let cyy = syy - sy * sy / w;
    let cxy = sxy - sx * sy / w;
    let tr = cxx + cyy;
    let diff = cxx - cyy;
    let disc = (diff * diff + 4.0 * cxy * cxy).max(0.0).sqrt();
    (0.5 * (tr - disc)).max(0.0)
}

#[inline]
fn mod_2pi(th: f64) -> f64 {
    let scaled = th * std::f64::consts::FRAC_1_PI * 0.5;
    std::f64::consts::TAU * (scaled - scaled.round())
}

/// A cubic in the frame Levien's quartic is stated in: unit chord on the x-axis.
struct G1Frame {
    th0: f64,
    th1: f64,
    area: f64,
    mx: f64,
    chord: f64,
}

/// Reduce raw path integrals to the unit-chord frame.
fn g1_frame(p0: Point, p1: Point, t0: Vec2, t1: Vec2, raw: (f64, f64, f64)) -> Option<G1Frame> {
    let d = p1 - p0;
    let chord2 = d.dot(d);
    if chord2 < 1e-18 || !chord2.is_finite() {
        return None;
    }
    let th = d.angle();
    let th0 = mod_2pi(t0.angle() - th);
    let th1 = mod_2pi(th - t1.angle());

    let (mut area, mut x, mut y) = raw;
    let (x0, y0) = (p0.x, p0.y);
    let (dx, dy) = (d.x, d.y);
    area -= dx * (y0 + 0.5 * dy);
    let dy_3 = dy / 3.0;
    x -= dx * (x0 * y0 + 0.5 * (x0 * dy + y0 * dx) + dy_3 * dx);
    y -= dx * (y0 * y0 + y0 * dy + dy_3 * dy);
    x -= x0 * area;
    y = 0.5 * y - y0 * area;
    let moment = dx * x + dy * y;
    let inv = chord2.recip();
    Some(G1Frame {
        th0,
        th1,
        area: area * inv,
        mx: moment * inv * inv,
        chord: chord2.sqrt(),
    })
}

/// Up to four `(d0, d1)` arm pairs.
struct Arms {
    items: [(f64, f64); 4],
    len: usize,
}

impl Arms {
    fn push(&mut self, d: (f64, f64)) {
        if self.len < 4 {
            self.items[self.len] = d;
            self.len += 1;
        }
    }
    fn iter(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        self.items[..self.len].iter().copied()
    }
}

/// Levien's quartic: arm lengths of the G1 cubics matching signed area and x-moment on
/// a unit chord. Coefficients as in `kurbo::fit::cubic_fit`.
fn arms_from_moments(th0: f64, th1: f64, area: f64, mx: f64) -> Arms {
    let mut out = Arms {
        items: [(0.0, 0.0); 4],
        len: 0,
    };
    let (s0, c0) = th0.sin_cos();
    let (s1, c1) = th1.sin_cos();
    let a4 = -9.
        * c0
        * (((2. * s1 * c1 * c0 + s0 * (2. * c1 * c1 - 1.)) * c0 - 2. * s1 * c1) * c0
            - c1 * c1 * s0);
    let a3 = 12.
        * ((((c1 * (30. * area * c1 - s1) - 15. * area) * c0 + 2. * s0
            - c1 * s0 * (c1 + 30. * area * s1))
            * c0
            + c1 * (s1 - 15. * area * c1))
            * c0
            - s0 * c1 * c1);
    let a2 = 12.
        * ((((70. * mx + 15. * area) * s1 * s1 + c1 * (9. * s1 - 70. * c1 * mx - 5. * c1 * area))
            * c0
            - 5. * s0 * s1 * (3. * s1 - 4. * c1 * (7. * mx + area)))
            * c0
            - c1 * (9. * s1 - 70. * c1 * mx - 5. * c1 * area));
    let a1 = 16.
        * (((12. * s0 - 5. * c0 * (42. * mx - 17. * area)) * s1
            - 70. * c1 * (3. * mx - area) * s0
            - 75. * c0 * c1 * area * area)
            * s1
            - 75. * c1 * c1 * area * area * s0);
    let a0 = 80. * s1 * (42. * s1 * mx - 25. * area * (s1 - c1 * area));

    let mut roots = [0.0f64; 4];
    let mut n_roots = 0usize;
    {
        let mut push = |r: f64| {
            if n_roots < roots.len() {
                roots[n_roots] = r;
                n_roots += 1;
            }
        };
        const EPS: f64 = 1e-12;
        if a4.abs() > EPS {
            let (a, b, c, d) = (a3 / a4, a2 / a4, a1 / a4, a0 / a4);
            if let Some(quads) = factor_quartic_inner(a, b, c, d, false) {
                for (qc1, qc0) in quads {
                    let qroots = solve_quadratic(qc0, qc1, 1.0);
                    if qroots.is_empty() {
                        push(-0.5 * qc1);
                    } else {
                        for r in qroots.iter().copied() {
                            push(r);
                        }
                    }
                }
            }
        } else if a3.abs() > EPS {
            for r in solve_cubic(a0, a1, a2, a3).iter().copied() {
                push(r);
            }
        } else if a2.abs() > EPS || a1.abs() > EPS || a0.abs() > EPS {
            for r in solve_quadratic(a0, a1, a2).iter().copied() {
                push(r);
            }
        } else {
            out.push((1.0 / 3.0, 1.0 / 3.0));
            return out;
        }
    }

    let s01 = s0 * c1 + s1 * c0;
    for &d0 in &roots[..n_roots] {
        let (d0, d1) = if d0 > 0.0 {
            let d1 = (d0 * s0 - area * (10. / 3.)) / (0.5 * d0 * s01 - s1);
            if d1 > 0.0 {
                (d0, d1)
            } else {
                (s1 / s01, 0.0)
            }
        } else {
            (0.0, s0 / s01)
        };
        if d0 >= 0.0 && d1 >= 0.0 && d0.is_finite() && d1.is_finite() {
            out.push((d0, d1));
        }
    }
    out
}

/// A cubic Bézier with its four control points.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cubic {
    pub(crate) p0: Point,
    pub(crate) p1: Point,
    pub(crate) p2: Point,
    pub(crate) p3: Point,
}

impl Cubic {
    /// The G1 cubic with arms `d0`, `d1` (fractions of the chord) along `t0`, `t1`.
    pub(crate) fn from_arms(
        p0: Point,
        p3: Point,
        t0: Vec2,
        t1: Vec2,
        chord: f64,
        d0: f64,
        d1: f64,
    ) -> Self {
        Cubic {
            p0,
            p1: Point::new(p0.x + t0.x * d0 * chord, p0.y + t0.y * d0 * chord),
            p2: Point::new(p3.x - t1.x * d1 * chord, p3.y - t1.y * d1 * chord),
            p3,
        }
    }

    #[inline]
    fn eval(&self, t: f64) -> Point {
        let mt = 1.0 - t;
        let (w0, w1, w2, w3) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
        Point::new(
            w0 * self.p0.x + w1 * self.p1.x + w2 * self.p2.x + w3 * self.p3.x,
            w0 * self.p0.y + w1 * self.p1.y + w2 * self.p2.y + w3 * self.p3.y,
        )
    }

    #[inline]
    fn deriv(&self, t: f64) -> Vec2 {
        let mt = 1.0 - t;
        let (w0, w1, w2) = (3.0 * mt * mt, 6.0 * mt * t, 3.0 * t * t);
        Vec2 {
            x: w0 * (self.p1.x - self.p0.x)
                + w1 * (self.p2.x - self.p1.x)
                + w2 * (self.p3.x - self.p2.x),
            y: w0 * (self.p1.y - self.p0.y)
                + w1 * (self.p2.y - self.p1.y)
                + w2 * (self.p3.y - self.p2.y),
        }
    }

    /// Squared distance from `p` to the curve, starting Newton from parameter `t`: the
    /// reference [`Self::dist2_lanes`] reproduces bit for bit.
    #[cfg(test)]
    fn dist2_from(&self, p: Point, t_init: f64) -> f64 {
        let mut t = t_init.clamp(0.0, 1.0);
        for _ in 0..NEWTON_STEPS {
            let b = self.eval(t);
            let d = self.deriv(t);
            let dd = d.dot(d);
            if dd < 1e-18 {
                break;
            }
            let r = b - p;
            let next = (t - r.dot(d) / dd).clamp(0.0, 1.0);
            if next == t {
                break;
            }
            t = next;
        }
        let r = self.eval(t) - p;
        r.dot(r)
    }

    /// Squared distances from [`LANES`] points to the curve, each starting Newton from
    /// its own parameter: `dist2_from`'s results, bit for bit.
    ///
    /// Every lane runs the scalar iteration's own expressions in the same order. Where
    /// the scalar loop stops early (a vanishing derivative, or Newton landing on the
    /// parameter it started from) the lane keeps its parameter, and since the step is a
    /// pure function of the parameter, the steps that follow reach the same decision and
    /// keep it too. With the early exits gone the lanes are straight-line code the
    /// compiler can run side by side, where the scalar loop was one chain of dependent
    /// operations per point with an unpredictable branch in it. This projection was a
    /// quarter to a third of all trace time on flat art.
    #[inline]
    fn dist2_lanes(&self, p: &[Point; LANES], t_init: &[f64; LANES]) -> [f64; LANES] {
        let mut t = t_init.map(|t| t.clamp(0.0, 1.0));
        for _ in 0..NEWTON_STEPS {
            for q in 0..LANES {
                let b = self.eval(t[q]);
                let d = self.deriv(t[q]);
                let dd = d.dot(d);
                let r = b - p[q];
                let next = (t[q] - r.dot(d) / dd).clamp(0.0, 1.0);
                if !(dd < 1e-18) && next != t[q] {
                    t[q] = next;
                }
            }
        }
        let mut out = [0.0; LANES];
        for q in 0..LANES {
            let r = self.eval(t[q]) - p[q];
            out[q] = r.dot(r);
        }
        out
    }

    /// Internal inflection check: does the cubic reverse its turning direction?
    pub(crate) fn has_inflection(&self) -> bool {
        let d0 = self.p1 - self.p0;
        let d1 = self.p2 - self.p1;
        let d2 = self.p3 - self.p2;
        let cross0 = d0.x * d1.y - d0.y * d1.x;
        let cross1 = d1.x * d2.y - d1.y * d2.x;
        let chord2 = (self.p3.x - self.p0.x).powi(2) + (self.p3.y - self.p0.y).powi(2);
        cross0 * cross1 < -1e-6 * chord2.max(1.0)
    }

    /// Dimensionless normalized bending energy: 12 * (|A|^2 + A.B + |B|^2) / L^2
    pub(crate) fn bending_energy(&self) -> f64 {
        let chord2 = (self.p3.x - self.p0.x).powi(2) + (self.p3.y - self.p0.y).powi(2);
        if chord2 <= 1e-12 {
            return 0.0;
        }
        let ax = self.p2.x - 2.0 * self.p1.x + self.p0.x;
        let ay = self.p2.y - 2.0 * self.p1.y + self.p0.y;
        let bx = self.p3.x - 2.0 * self.p2.x + self.p1.x;
        let by = self.p3.y - 2.0 * self.p2.y + self.p1.y;
        let aa = ax * ax + ay * ay;
        let ab = ax * bx + ay * by;
        let bb = bx * bx + by * by;
        12.0 * (aa + ab + bb) / chord2
    }

    /// Penalty charged against wobbly/inflecting cubics to suppress micro-oscillations.
    pub(crate) fn wobble_penalty(&self, lambda: f64) -> f64 {
        let factor = wobble_penalty_factor();
        if factor <= 0.0 {
            return 0.0;
        }
        let mut penalty = 0.0;
        if self.has_inflection() {
            penalty += 2.0 * lambda * factor;
        }
        let ebend = self.bending_energy();
        if ebend > 2.5 {
            penalty += ((ebend - 2.5).min(10.0)) * lambda * factor * 0.5;
        }
        penalty
    }
}

/// The most efficient parameter-ratio lever measured on this tree, and it is already
/// here. Swept on the 246-icon gate set against the default 1.0 (dE00 0.148324, turning
/// 0.042094, ratio 1.481829):
///
/// | factor | dE00 | turning | ratio |
/// |---|---|---|---|
/// | 0.0 | -0.48% | +42.40% | -9.96% |
/// | 0.5 | +2.86% | +11.38% | -8.35% |
/// | 0.8 | **+0.40%** | +2.54% | **-1.94%** |
/// | 1.0 | — | — | — |
/// | 2.0 | +0.03% | -3.74% | +2.20% |
///
/// The response either side of the default is steep and very asymmetric, and 0.8 is the
/// interesting point: nearly 2% of the parameter ratio for 0.4% of dE00. It still fails
/// the turning gate (+2.54% against a +1% limit), so it is not a free win — but it is a
/// better exchange than anything else tried here, including `--lambda-scale` and the
/// correlated-noise chi2 reverted in e3746b0, and it costs one constant rather than a
/// new model. Whether 1.0 is the right default does not appear to have been swept
/// against the parameter ratio; on this evidence it is worth doing properly.
pub(crate) fn wobble_penalty_factor() -> f64 {
    static V: OnceLock<f64> = OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("INKVEC_WOBBLE_PENALTY")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(1.0)
    })
}

/// Which interior points of a span are scored, at what parameter, against what noise.
///
/// The points are gathered once per span, in [`LANES`]-wide groups (the tail padded with
/// points never read), since every candidate cubic of the span is scored on them.
struct CubicSamples {
    pt: [Point; MAX_RESIDUAL_SAMPLES],
    t: [f64; MAX_RESIDUAL_SAMPLES],
    s2: [f64; MAX_RESIDUAL_SAMPLES],
    len: usize,
    weight: f64,
}

/// Points [`Cubic::dist2_lanes`] projects at once.
const LANES: usize = 4;
const _: () = assert!(MAX_RESIDUAL_SAMPLES % LANES == 0);

/// The `LANES` elements of `a` from `m`.
#[inline]
fn lanes<T>(a: &[T], m: usize) -> &[T; LANES] {
    a[m..m + LANES].try_into().expect("a whole group of lanes")
}

impl CubicSamples {
    fn new(pts: &[Point], sigma: &[f64], s: &[f64], i: usize, j: usize) -> Option<Self> {
        let interior = j.saturating_sub(i + 1);
        if interior == 0 {
            return None;
        }
        let count = interior.min(MAX_RESIDUAL_SAMPLES);
        let span = (s[j] - s[i]).max(1e-12);
        let mut out = Self {
            pt: [Point::new(0.0, 0.0); MAX_RESIDUAL_SAMPLES],
            t: [0.0; MAX_RESIDUAL_SAMPLES],
            s2: [0.0; MAX_RESIDUAL_SAMPLES],
            len: count,
            weight: interior as f64 / count as f64,
        };
        for m in 0..count {
            let k = i + 1 + (((m as f64 + 0.5) * interior as f64) / count as f64).floor() as usize;
            let k = k.min(j - 1);
            // Floored like every other chi2 term: a vanishing sigma would otherwise divide
            // by zero (or a subnormal) and blow the residual up.
            let sg = sigma[k].max(1e-6);
            (out.pt[m], out.t[m], out.s2[m]) = (pts[k], (s[k] - s[i]) / span, sg * sg);
        }
        Some(out)
    }

    /// The weighted residual, summed in sample order; returned as soon as the partial sum
    /// reaches `bound` (a candidate that can no longer win).
    fn chi2(&self, cb: &Cubic, bound: f64) -> f64 {
        let mut acc = 0.0;
        for m in (0..self.len).step_by(LANES) {
            let d2 = cb.dist2_lanes(lanes(&self.pt, m), lanes(&self.t, m));
            for (q, d2) in d2.iter().enumerate().take(self.len - m) {
                acc += self.weight * d2 / self.s2[m + q];
                if acc >= bound {
                    return acc;
                }
            }
        }
        acc
    }
}

/// Weighted residual of the interior points of `(i, j)` against a cubic.
pub(crate) fn chi2_cubic(
    pts: &[Point],
    sigma: &[f64],
    s: &[f64],
    i: usize,
    j: usize,
    cb: &Cubic,
    subsample: bool,
) -> f64 {
    let interior = j.saturating_sub(i + 1);
    if interior == 0 {
        return 0.0;
    }
    let count = if subsample {
        interior.min(MAX_RESIDUAL_SAMPLES)
    } else {
        interior
    };
    let weight = interior as f64 / count as f64;
    let span = (s[j] - s[i]).max(1e-12);
    let mut acc = 0.0;
    for m0 in (0..count).step_by(LANES) {
        let (mut p, mut t, mut sg) = ([Point::new(0.0, 0.0); LANES], [0.0; LANES], [1.0; LANES]);
        let n = LANES.min(count - m0);
        for q in 0..n {
            let m = m0 + q;
            let k = i + 1 + (((m as f64 + 0.5) * interior as f64) / count as f64).floor() as usize;
            let k = k.min(j - 1);
            (p[q], t[q], sg[q]) = (pts[k], (s[k] - s[i]) / span, sigma[k].max(1e-6));
        }
        let d2 = cb.dist2_lanes(&p, &t);
        for q in 0..n {
            acc += weight * d2[q] / (sg[q] * sg[q]);
        }
    }
    acc
}

fn free_cubic(
    pts: &[Point],
    sigma: &[f64],
    s: &[f64],
    i: usize,
    j: usize,
) -> Option<(Point, Point)> {
    if j < i + 3 {
        return None;
    }
    let span = (s[j] - s[i]).max(1e-12);
    let (p0, p3) = (pts[i], pts[j]);
    let (mut a00, mut a01, mut a11) = (0.0, 0.0, 0.0);
    let (mut bx0, mut bx1, mut by0, mut by1) = (0.0, 0.0, 0.0, 0.0);
    for k in i + 1..j {
        let t = ((s[k] - s[i]) / span).clamp(0.0, 1.0);
        let mt = 1.0 - t;
        let (w0, w1, w2, w3) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
        let w = 1.0 / (sigma[k] * sigma[k]);
        a00 += w * w1 * w1;
        a01 += w * w1 * w2;
        a11 += w * w2 * w2;
        let rx = pts[k].x - w0 * p0.x - w3 * p3.x;
        let ry = pts[k].y - w0 * p0.y - w3 * p3.y;
        bx0 += w * w1 * rx;
        bx1 += w * w2 * rx;
        by0 += w * w1 * ry;
        by1 += w * w2 * ry;
    }
    let det = a00 * a11 - a01 * a01;
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let solve = |b0: f64, b1: f64| ((a11 * b0 - a01 * b1) / det, (a00 * b1 - a01 * b0) / det);
    let (p1x, p2x) = solve(bx0, bx1);
    let (p1y, p2y) = solve(by0, by1);
    let (p1, p2) = (Point::new(p1x, p1y), Point::new(p2x, p2y));
    (p1.x.is_finite() && p1.y.is_finite() && p2.x.is_finite() && p2.y.is_finite())
        .then_some((p1, p2))
}

/// Free-tangent least-squares cubic through points `i..=j` of `pts`, endpoints pinned.
pub fn free_cubic_fit(
    pts: &[Point],
    sigma: &[f64],
    s: &[f64],
    i: usize,
    j: usize,
) -> Option<(Point, Point)> {
    free_cubic(pts, sigma, s, i, j)
}

/// A free-tangent candidate, scored.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FreeFit {
    pub(crate) chi2: f64,
    pub(crate) brk: f64,
    pub(crate) tans: (Vec2, Vec2),
    pub(crate) arms: (f64, f64),
}

/// Fit and score the free-tangent cubic for one span, or refuse it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn try_free_cubic(
    pts: &[Point],
    sigma: &[f64],
    s: &[f64],
    i: usize,
    j: usize,
    t0: Vec2,
    t1: Vec2,
    lambda: f64,
    subsample: bool,
) -> Option<FreeFit> {
    if !free_cubic_enabled() {
        return None;
    }
    let (q1, q2) = free_cubic(pts, sigma, s, i, j)?;
    let chord = (pts[j] - pts[i]).norm();
    let (v0, v1) = (q1 - pts[i], pts[j] - q2);
    if chord <= 1e-9 || v0.norm() <= 1e-9 || v1.norm() <= 1e-9 {
        return None;
    }
    let f0 = Vec2 {
        x: v0.x / v0.norm(),
        y: v0.y / v0.norm(),
    };
    let f1 = Vec2 {
        x: v1.x / v1.norm(),
        y: v1.y / v1.norm(),
    };
    let (d0, d1) = (v0.norm() / chord, v1.norm() / chord);
    let swing = FREE_MAX_SWING.to_radians();
    if turn_angle(t0, f0) >= swing || turn_angle(f1, t1) >= swing || d0 > MAX_ARM || d1 > MAX_ARM {
        return None;
    }
    let cb = Cubic {
        p0: pts[i],
        p1: q1,
        p2: q2,
        p3: pts[j],
    };
    let wobble = cb.wobble_penalty(lambda);
    Some(FreeFit {
        chi2: chi2_cubic(pts, sigma, s, i, j, &cb, subsample) + 2.0 * wobble,
        brk: break_cost(t0, f0, lambda) + break_cost(f1, t1, lambda),
        tans: (f0, f1),
        arms: (d0, d1),
    })
}

/// Best G1 cubic for `(i, j)` given tangents and raw moments: `(chi2, d0, d1)`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn best_cubic(
    pts: &[Point],
    sigma: &[f64],
    s: &[f64],
    i: usize,
    j: usize,
    t0: Vec2,
    t1: Vec2,
    raw: (f64, f64, f64),
    subsample: bool,
) -> Option<(f64, f64, f64)> {
    let fr = g1_frame(pts[i], pts[j], t0, t1, raw)?;
    let plan = subsample
        .then(|| CubicSamples::new(pts, sigma, s, i, j))
        .flatten();
    let mut best: Option<(f64, f64, f64)> = None;
    for (d0, d1) in arms_from_moments(fr.th0, fr.th1, fr.area, fr.mx).iter() {
        if d0 > MAX_ARM || d1 > MAX_ARM {
            continue;
        }
        let cb = Cubic::from_arms(pts[i], pts[j], t0, t1, fr.chord, d0, d1);
        let chi2 = match &plan {
            Some(p) => p.chi2(&cb, best.map_or(f64::INFINITY, |b| b.0)),
            None => chi2_cubic(pts, sigma, s, i, j, &cb, subsample),
        };
        if best.map(|b| chi2 < b.0).unwrap_or(true) {
            best = Some((chi2, d0, d1));
        }
    }
    best
}

/// G1 cubics matching the area and first moment of a point run, with the given end
/// tangents. Returns every real candidate's control points, in the order the quartic
/// produced them. Exposed so the reduction can be checked against a known cubic.
pub fn fit_cubic_moments(pts: &[Point], t0: Vec2, t1: Vec2) -> Vec<(Point, Point)> {
    if pts.len() < 2 {
        return Vec::new();
    }
    let raw = raw_moments_direct(pts, 0, pts.len() - 1);
    let Some(fr) = g1_frame(pts[0], pts[pts.len() - 1], t0, t1, raw) else {
        return Vec::new();
    };
    arms_from_moments(fr.th0, fr.th1, fr.area, fr.mx)
        .iter()
        .map(|(d0, d1)| {
            let c = Cubic::from_arms(pts[0], pts[pts.len() - 1], t0, t1, fr.chord, d0, d1);
            (c.p1, c.p2)
        })
        .collect()
}

/// What a line's residual sign pattern says against it.
pub(crate) fn bow_penalty(chi2_line: f64, chi2_arc: f64, span: usize) -> f64 {
    if chi2_arc * 4.0 < chi2_line {
        span as f64 * std::f64::consts::LN_2
    } else {
        0.0
    }
}

/// Cost of the line `i -> j`: residual, two parameters, and its disagreement with the
/// polyline tangents at whichever ends are joins.
pub(crate) fn line_cost_terms(
    pts: &[Point],
    tan: &Tangents,
    i: usize,
    j: usize,
    chi2: f64,
    cfg: &FitConfig,
    joins_at_ends: bool,
) -> f64 {
    let n = pts.len();
    let chord = pts[j] - pts[i];
    let mut dev = 0.0;
    if i > 0 || joins_at_ends {
        dev += break_cost(tan.outgoing[i], chord, cfg.lambda);
    }
    if j + 1 < n || joins_at_ends {
        dev += break_cost(chord, tan.incoming[j], cfg.lambda);
    }
    0.5 * chi2 + cfg.lambda * PARAMS_LINE + dev
}

/// Weighted moments of `(x, y, x² + y²)` along the polyline, so a circle can be fitted to
/// any span in constant time.
pub(crate) struct CirclePrefix {
    m: Vec<[f64; 10]>,
}

impl CirclePrefix {
    pub(crate) fn new(pts: &[Point], sigma: &[f64]) -> Self {
        let mut m = Vec::with_capacity(pts.len() + 1);
        let mut acc = [0.0f64; 10];
        m.push(acc);
        for (k, p) in pts.iter().enumerate() {
            let sg = sigma.get(k).copied().unwrap_or(0.5).max(1e-3);
            let w = 1.0 / (sg * sg);
            let (x, y) = (p.x, p.y);
            let z = x * x + y * y;
            acc[0] += w;
            acc[1] += w * x;
            acc[2] += w * y;
            acc[3] += w * z;
            acc[4] += w * x * x;
            acc[5] += w * x * y;
            acc[6] += w * y * y;
            acc[7] += w * x * z;
            acc[8] += w * y * z;
            acc[9] += w * z * z;
            m.push(acc);
        }
        Self { m }
    }

    #[inline]
    fn window(&self, i: usize, j: usize) -> [f64; 10] {
        let (a, b) = (&self.m[i], &self.m[j + 1]);
        let mut out = [0.0f64; 10];
        for k in 0..10 {
            out[k] = b[k] - a[k];
        }
        out
    }

    fn scale(&self, i: usize, j: usize) -> f64 {
        let [s0, sx, sy, _, sxx, _, syy, _, _, _] = self.window(i, j);
        if s0 <= 0.0 {
            return 1.0;
        }
        let var_x = (sxx - sx * sx / s0).max(0.0) / s0;
        let var_y = (syy - sy * sy / s0).max(0.0) / s0;
        (var_x + var_y).sqrt().max(1.0)
    }

    pub(crate) fn residual_about(&self, i: usize, j: usize, c: Point, r: f64) -> f64 {
        if r <= 1e-12 {
            return f64::INFINITY;
        }
        let [s0, sx, sy, sz, sxx, sxy, syy, sxz, syz, szz] = self.window(i, j);
        let (a, b) = (-2.0 * c.x, -2.0 * c.y);
        let cc = c.x * c.x + c.y * c.y - r * r;
        let resid = szz
            + a * a * sxx
            + b * b * syy
            + cc * cc * s0
            + 2.0 * (a * sxz + b * syz + cc * sz + a * b * sxy + a * cc * sx + b * cc * sy);
        resid.max(0.0) / (4.0 * r * r)
    }

    pub(crate) fn fit(&self, i: usize, j: usize) -> Option<(Point, f64, f64)> {
        let [s0, sx, sy, sz, sxx, sxy, syy, sxz, syz, szz] = self.window(i, j);
        if s0 <= 0.0 {
            return None;
        }
        let m = [[sxx, sxy, sx], [sxy, syy, sy], [sx, sy, s0]];
        let r = [-sxz, -syz, -sz];
        let [a, b, c] = crate::tangents::solve3(m, r)?;
        let centre = Point::new(-0.5 * a, -0.5 * b);
        let r2 = 0.25 * (a * a + b * b) - c;
        if !(r2.is_finite() && r2 > 1e-12) {
            return None;
        }
        let radius = r2.sqrt();
        let resid = szz
            + a * a * sxx
            + b * b * syy
            + c * c * s0
            + 2.0 * (a * sxz + b * syz + c * sz + a * b * sxy + a * c * sx + b * c * sy);
        let chi2 = (resid.max(0.0)) / (4.0 * r2);
        Some((centre, radius, chi2))
    }
}

/// A circular arc fitted to one span, with what it costs to use it there.
pub(crate) struct ArcSpan {
    pub(crate) cost: f64,
    pub(crate) chi2: f64,
    pub(crate) radius: f64,
    pub(crate) large_arc: bool,
    pub(crate) sweep: bool,
    pub(crate) tans: (Vec2, Vec2),
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_arc(
    pts: &[Point],
    tan: &Tangents,
    pre: &CirclePrefix,
    i: usize,
    j: usize,
    cfg: &FitConfig,
    joins_at_ends: bool,
) -> Option<ArcSpan> {
    const DIRECTION_SAMPLES: usize = 8;
    if j < i + 2 {
        return None;
    }
    let (c, r, chi2_fit) = pre.fit(i, j)?;
    let _ = chi2_fit;
    if !r.is_finite() || r <= 1e-6 {
        return None;
    }
    if r > 1e3 * pre.scale(i, j).max(1.0) {
        return None;
    }
    let n_span = j - i;
    let stride = (n_span / DIRECTION_SAMPLES).max(1);
    let start = pts[i];
    let end = pts[j];
    let mut sign = 0.0f64;
    let mut prev = start - c;
    let mut k = i;
    while k < j {
        k = (k + stride).min(j);
        let cur = pts[k] - c;
        let cross = prev.x * cur.y - prev.y * cur.x;
        if cross.abs() > 1e-12 {
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross.signum() != sign {
                return None;
            }
        }
        prev = cur;
    }
    if sign == 0.0 {
        return None;
    }
    let (us, ue) = (start - c, end - c);
    let turn = (us.x * ue.y - us.y * ue.x).atan2(us.x * ue.x + us.y * ue.y);
    if turn == 0.0 || turn.signum() != sign {
        return None;
    }
    let turn = turn.abs();
    if !(1e-3..=crate::primitives::MAX_ARC_DEGREES.to_radians()).contains(&turn) {
        return None;
    }
    let ccw = sign > 0.0;

    let tangent_at = |p: Point| -> Option<Vec2> {
        let radial = p - c;
        let t = if ccw {
            Vec2 {
                x: -radial.y,
                y: radial.x,
            }
        } else {
            Vec2 {
                x: radial.y,
                y: -radial.x,
            }
        };
        unit(t)
    };
    let t0 = tangent_at(start)?;
    let t1 = tangent_at(end)?;

    let n = pts.len();
    let mut dev = 0.0;
    if i > 0 || joins_at_ends {
        dev += break_cost(tan.outgoing[i], t0, cfg.lambda);
    }
    if j + 1 < n || joins_at_ends {
        dev += break_cost(t1, tan.incoming[j], cfg.lambda);
    }

    let large_arc = turn > std::f64::consts::PI;
    let radius = 0.5 * (start.dist(c) + end.dist(c));
    let (drawn_c, drawn_r, _, _) = crate::curves::arc_center(start, radius, large_arc, ccw, end);
    let chi2 = pre.residual_about(i, j, drawn_c, drawn_r);
    Some(ArcSpan {
        cost: 0.5 * chi2 + cfg.lambda * crate::curves::PARAMS_ARC + dev,
        chi2,
        radius,
        large_arc,
        sweep: ccw,
        tans: (t0, t1),
    })
}

/// An elliptical arc fitted to one span.
pub(crate) struct EllipseSpan {
    pub(crate) cost: f64,
    pub(crate) rx: f64,
    pub(crate) ry: f64,
    pub(crate) phi: f64,
    pub(crate) large_arc: bool,
    pub(crate) sweep: bool,
    pub(crate) tans: (Vec2, Vec2),
}

/// Weighted Sampson distance of the points from the ellipse `(c, rx, ry, phi)`.
pub(crate) fn ellipse_sampson_chi2(
    pts: &[Point],
    sigma: &[f64],
    c: Point,
    rx: f64,
    ry: f64,
    phi: f64,
) -> f64 {
    let (sp, cp) = phi.sin_cos();
    let (ax, ay) = (1.0 / (rx * rx), 1.0 / (ry * ry));
    let mut sum = 0.0;
    for (k, p) in pts.iter().enumerate() {
        let (dx, dy) = (p.x - c.x, p.y - c.y);
        let u = cp * dx + sp * dy;
        let v = -sp * dx + cp * dy;
        let q = u * u * ax + v * v * ay - 1.0;
        let g = (4.0 * u * u * ax * ax + 4.0 * v * v * ay * ay).sqrt();
        if g < 1e-12 {
            return f64::INFINITY;
        }
        let d = q / g;
        let sg = sigma.get(k).copied().unwrap_or(0.5).max(1e-3);
        sum += (d / sg) * (d / sg);
    }
    sum
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_ellipse(
    pts: &[Point],
    sigma: &[f64],
    tan: &Tangents,
    i: usize,
    j: usize,
    cfg: &FitConfig,
    joins_at_ends: bool,
) -> Option<EllipseSpan> {
    const MAX_ASPECT: f64 = 12.0;
    const MIN_POINTS: usize = 24;
    const LENGTH_STRIDE: usize = 16;
    if j < i + MIN_POINTS || !(j - i).is_multiple_of(LENGTH_STRIDE) {
        return None;
    }
    let span_pts = &pts[i..=j];
    let span_sigma = &sigma[i..=j];
    let fit = crate::primitives::fit_ellipse_algebraic(span_pts, span_sigma)?;
    if !(fit.rx.is_finite() && fit.ry.is_finite()) || fit.rx <= 1e-6 || fit.ry <= 1e-6 {
        return None;
    }
    let (major, minor) = (fit.rx.max(fit.ry), fit.rx.min(fit.ry));
    if major / minor > MAX_ASPECT {
        return None;
    }
    let extent = span_pts
        .iter()
        .map(|p| p.dist(span_pts[0]))
        .fold(0.0f64, f64::max);
    if major > 1e3 * extent.max(1.0) {
        return None;
    }

    // The parametric angle, straight from the projected point — no iteration, and monotone
    // in the true one for points near the curve, which is all the direction test needs.
    let (sp0, cp0) = fit.angle.sin_cos();
    let angle_of = |p: Point| -> f64 {
        let (dx, dy) = (p.x - fit.c.x, p.y - fit.c.y);
        let u = cp0 * dx + sp0 * dy;
        let v = -sp0 * dx + cp0 * dy;
        (v / fit.ry).atan2(u / fit.rx)
    };
    let step = |a: f64, prev: f64| -> f64 {
        let mut d = a - prev;
        if d > std::f64::consts::PI {
            d -= std::f64::consts::TAU;
        } else if d < -std::f64::consts::PI {
            d += std::f64::consts::TAU;
        }
        d
    };
    let mut prev = angle_of(span_pts[0]);
    let ccw = step(angle_of(span_pts[1]), prev) > 0.0;
    let mut total = 0.0;
    for p in &span_pts[1..] {
        let a = angle_of(*p);
        let d = step(a, prev);
        if (ccw && d < -1e-3) || (!ccw && d > 1e-3) {
            return None;
        }
        total += d;
        prev = a;
    }
    let turn = total.abs();
    if !(1e-3..=crate::primitives::MAX_ARC_DEGREES.to_radians()).contains(&turn) {
        return None;
    }

    // The radii that are *drawn*. As with the circle, the arc a renderer reconstructs
    // passes through the two endpoints, so the fitted radii are rescaled to put them on
    // the ellipse before the arc is scored.
    let start = span_pts[0];
    let end = span_pts[span_pts.len() - 1];
    let scale_at = |p: Point| -> f64 {
        let (dx, dy) = (p.x - fit.c.x, p.y - fit.c.y);
        let (u, v) = (cp0 * dx + sp0 * dy, -sp0 * dx + cp0 * dy);
        ((u / fit.rx).powi(2) + (v / fit.ry).powi(2)).sqrt()
    };
    let k = 0.5 * (scale_at(start) + scale_at(end));
    if !(0.5..=2.0).contains(&k) {
        return None; // the endpoints are nowhere near the fitted ellipse
    }
    let large_arc = turn > std::f64::consts::PI;
    let frame = crate::curves::arc_ellipse_center(
        start,
        fit.rx * k,
        fit.ry * k,
        fit.angle,
        large_arc,
        ccw,
        end,
    );
    if frame.delta.abs() <= 1e-6 {
        return None;
    }
    let chi2 = ellipse_sampson_chi2(span_pts, span_sigma, frame.c, frame.rx, frame.ry, frame.phi);
    if !chi2.is_finite() {
        return None;
    }

    // End tangents: the derivative of the parametrization, in the direction of travel.
    let tangent_at = |t: f64| -> Option<Vec2> {
        let (sp, cp) = frame.phi.sin_cos();
        let (dx, dy) = (-frame.rx * t.sin(), frame.ry * t.cos());
        let v = Vec2 {
            x: cp * dx - sp * dy,
            y: sp * dx + cp * dy,
        };
        let v = if frame.delta > 0.0 {
            v
        } else {
            Vec2 { x: -v.x, y: -v.y }
        };
        unit(v)
    };
    let t0 = tangent_at(frame.theta1)?;
    let t1 = tangent_at(frame.theta1 + frame.delta)?;

    let n = pts.len();
    let mut dev = 0.0;
    if i > 0 || joins_at_ends {
        dev += break_cost(tan.outgoing[i], t0, cfg.lambda);
    }
    if j + 1 < n || joins_at_ends {
        dev += break_cost(t1, tan.incoming[j], cfg.lambda);
    }
    Some(EllipseSpan {
        cost: 0.5 * chi2 + cfg.lambda * crate::curves::PARAMS_ELLIPTICAL_ARC + dev,
        rx: frame.rx,
        ry: frame.ry,
        phi: frame.phi,
        large_arc,
        sweep: ccw,
        tans: (t0, t1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unit_and_edge_terms() {
        let v = Vec2 { x: 3.0, y: 4.0 };
        let u = unit(v).unwrap();
        assert!((u.norm() - 1.0).abs() < 1e-9);
        assert!((u.x - 0.6).abs() < 1e-9);
        assert!((u.y - 0.8).abs() < 1e-9);

        let (a, x, y) = edge_terms(Point::new(0.0, 0.0), Point::new(2.0, 2.0));
        assert!(a > 0.0 && x > 0.0 && y > 0.0);
    }

    #[test]
    fn test_circle_prefix_and_bow_penalty() {
        let pts = vec![
            Point::new(0.0, 0.0),
            Point::new(1.0, 1.0),
            Point::new(2.0, 0.0),
        ];
        let sigmas = vec![0.1, 0.1, 0.1];
        let pre = CirclePrefix::new(&pts, &sigmas);
        let fit = pre.fit(0, 2);
        assert!(fit.is_some());

        let penalty = bow_penalty(10.0, 1.0, 5);
        assert!(penalty > 0.0);
    }

    #[test]
    fn test_dist2_lanes_is_dist2_from_bit_for_bit() {
        let mut st = 3u64;
        let mut rnd = || {
            st = st
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (st >> 11) as f64 / (1u64 << 53) as f64
        };
        for case in 0..400 {
            let mut pt = || Point::new(40.0 * rnd() - 20.0, 40.0 * rnd() - 20.0);
            let (p0, p1, p2, p3) = (pt(), pt(), pt(), pt());
            // Degenerate curves too: every control point on one spot (a vanishing
            // derivative everywhere), and cusps where two coincide.
            let cb = match case % 5 {
                0 => Cubic {
                    p0,
                    p1: p0,
                    p2: p0,
                    p3: p0,
                },
                1 => Cubic { p0, p1: p0, p2, p3 },
                _ => Cubic { p0, p1, p2, p3 },
            };
            let mut p = [Point::new(0.0, 0.0); LANES];
            let mut t = [0.0; LANES];
            for q in 0..LANES {
                p[q] = Point::new(40.0 * rnd() - 20.0, 40.0 * rnd() - 20.0);
                // Starts outside [0, 1] and exactly on its ends as well as inside it.
                t[q] = match (case + q) % 4 {
                    0 => 0.0,
                    1 => 1.0,
                    2 => 1.4 * rnd() - 0.2,
                    _ => rnd(),
                };
            }
            let lanes = cb.dist2_lanes(&p, &t);
            for q in 0..LANES {
                assert_eq!(lanes[q].to_bits(), cb.dist2_from(p[q], t[q]).to_bits());
            }
        }
    }
}
