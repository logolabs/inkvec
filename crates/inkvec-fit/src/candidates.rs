//! Primitive candidate generation and scoring for the multi-model alphabet (DESIGN.md S4).
//!
//! Evaluates candidate models — straight lines, circular arcs, elliptical arcs,
//! moment-matched G1 cubic Béziers, and free-tangent cubic Béziers — for any sub-polyline span (i, j).
//!
//! # Where this sits
//!
//! Stage 4 of the shipping fit (see the crate overview). The multimodel dynamic program
//! (`crate::multimodel::scan`) asks, for every candidate span `(i, j)` of the measured
//! polyline, what each model would cost there; this file answers. Inputs are the points
//! (px), their sigmas (px), their cumulative arc lengths `s` (px), the tangents
//! estimated at every point (`crate::tangents`) and prefix sums that make a span's
//! moments O(1). Outputs are a residual `χ²` (dimensionless, in units of each point's
//! sigma), the fitted parameters, and a cost in nats:
//!
//! | model | fitter | cost |
//! |---|---|---|
//! | line | total least squares ([`scatter_min_eigen`]) | `½χ² + 2λ + breaks` ([`line_cost_terms`]) |
//! | G1 cubic | area and moment matching, Levien's quartic ([`best_cubic`]) | `½χ² + 6λ + wobble + over-turn` |
//! | free cubic (research) | linear least squares, ends pinned ([`try_free_cubic`]) | `½χ² + 6λ + wobble + breaks + over-turn` |
//! | circular arc | Kåsa algebraic circle ([`CirclePrefix`], [`try_arc`]) | `½χ² + 5λ + breaks` |
//! | elliptical arc | algebraic ellipse, Sampson distance ([`try_ellipse`]) | `½χ² + 7λ + breaks` |
//!
//! "breaks" is the [`break_cost`] between the model's own end directions and the
//! estimated tangents at whichever ends are joins; the G1 cubic takes the estimated
//! tangents as its end directions and so pays none. `crate::cost` reprices 6, 5 and [`turn`].
//!
//! Every fitter here is closed form or a fixed small number of steps, so a candidate
//! costs O(1) in the span length (or O([`MAX_RESIDUAL_SAMPLES`]) for a cubic's
//! residual), which is what keeps the dynamic program O(n²).

use crate::tangents::{break_cost, turn_angle, Tangents};
use crate::{FitConfig, PARAMS_LINE};
use inkvec_core::{Point, Vec2};
use kurbo::common::{factor_quartic_inner, solve_cubic, solve_quadratic};

mod bounded;
pub(crate) use bounded::{best_cubic_bounded, Abandon, G1Fit};
pub(crate) mod turn;

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
/// price (see [`crate::cost`]).
pub fn params_cubic() -> f64 {
    crate::cost::cubic_params()
}

/// Off unless `INKVEC_FREE_CUBIC` is set in a `research` build, because it does not pay.
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
    cfg!(feature = "research") && inkvec_core::env::flag("INKVEC_FREE_CUBIC")
}

/// Normalize vector to unit length, if non-degenerate: `None` for a length below 1e-12 or
/// a non-finite one.
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
///
/// For the edge `a -> b`, parametrized linearly with `dx = b.x − a.x`, `dy = b.y − a.y`,
/// returns the exact line integrals
///
/// ```text
///     ∫ y dx   = dx·(a.y + dy/2)
///     ∫ x·y dx = dx·(a.x·a.y + (a.x·dy + a.y·dx)/2 + dx·dy/3)
///     ∫ y² dx  = dx·(a.y² + a.y·dy + dy²/3)
/// ```
///
/// in px², px³ and px³. Summed round a closed loop they give its area and first moments
/// (see the `crate::multimodel` overview).
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
///
/// Sums [`edge_terms`] over the edges `i -> i+1 … j−1 -> j`, O(j − i). The dynamic
/// program reads the same quantity as a difference of prefix sums; this is the
/// independent version the tests and `segment_cost_direct` use.
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
///
/// With `w = Σw_k`, `sx = Σw_k·x_k`, `sxx = Σw_k·x_k²` and so on (weights `1/σ²`), the
/// scatter about the weighted centroid is `C = [[Cxx, Cxy], [Cxy, Cyy]]` with
/// `Cxx = sxx − sx²/w` etc., and
///
/// ```text
///     λ_min = ½ (Cxx + Cyy − √((Cxx − Cyy)² + 4·Cxy²))
/// ```
///
/// is the weighted sum of squared perpendicular distances to the total-least-squares
/// line: the line's χ². Clamped at 0 against cancellation; `w` must be positive.
pub fn scatter_min_eigen(w: f64, sx: f64, sy: f64, sxx: f64, syy: f64, sxy: f64) -> f64 {
    let cxx = sxx - sx * sx / w;
    let cyy = syy - sy * sy / w;
    let cxy = sxy - sx * sy / w;
    let tr = cxx + cyy;
    let diff = cxx - cyy;
    let disc = (diff * diff + 4.0 * cxy * cxy).max(0.0).sqrt();
    (0.5 * (tr - disc)).max(0.0)
}

/// Wrap an angle (radians) into `[−π, π]` by subtracting the nearest whole turn.
#[inline]
fn mod_2pi(th: f64) -> f64 {
    let scaled = th * std::f64::consts::FRAC_1_PI * 0.5;
    std::f64::consts::TAU * (scaled - scaled.round())
}

/// A cubic in the frame Levien's quartic is stated in: unit chord on the x-axis.
struct G1Frame {
    /// Angle of the start tangent from the chord, radians in `[−π, π]`.
    th0: f64,
    /// Angle of the chord from the end tangent, radians in `[−π, π]`.
    th1: f64,
    /// Signed area between the points and the chord, divided by chord².
    area: f64,
    /// First moment of that region along the chord, divided by chord⁴.
    mx: f64,
    /// Chord length, in px.
    chord: f64,
}

/// Reduce raw path integrals to the unit-chord frame.
///
/// `raw` holds `(∫ y dx, ∫ x·y dx, ∫ y² dx)` along the points from `p0` to `p1`. The
/// steps:
///
/// 1. subtract the same integrals along the chord `p0 -> p1`, which closes the path into
///    a loop (points forward, chord back) so the integrals become loop integrals:
///    `A = ∮ y dx`, `X = ∮ x·y dx`, `Y = ∮ y² dx`;
/// 2. move the origin to `p0`. Round a closed loop `∮ dx = ∮ x dx = 0`, so
///    `X' = X − x0·A` and `½·Y' = ½·Y − y0·A`;
/// 3. by Green's theorem `X' = −∬ x dA` and `½·Y' = −∬ y dA` (origin at `p0`), so
///    `M = dx·X' + dy·½Y'` is the region's first moment along the chord direction `d`,
///    times `|d|` and with Green's sign;
/// 4. scale to a unit chord: area scales as length², a first moment as length³, and the
///    extra `|d|` from step 3 makes `M / |d|⁴`.
///
/// `(area, mx)` are then the two numbers `kurbo::fit::cubic_fit` consumes, in its own sign
/// convention. `None` for a zero-length or non-finite chord. The tangents need not be
/// unit length; only their angles are used.
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

/// Up to four `(d0, d1)` arm pairs, the real solutions of Levien's quartic. A fixed
/// array rather than a `Vec`, because one is built per candidate span.
pub struct Arms {
    pub items: [(f64, f64); 4],
    pub len: usize,
}

impl Arms {
    /// Append a pair; a fifth is silently dropped (a quartic has at most four roots).
    fn push(&mut self, d: (f64, f64)) {
        if self.len < 4 {
            self.items[self.len] = d;
            self.len += 1;
        }
    }
    /// The pairs pushed so far, in order.
    pub fn iter(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        self.items[..self.len].iter().copied()
    }
}

/// Levien's quartic: arm lengths of the G1 cubics matching signed area and x-moment on
/// a unit chord. Coefficients as in `kurbo::fit::cubic_fit`.
///
/// On a unit chord with end-tangent angles `θ0`, `θ1` fixed, a cubic has two free
/// numbers, its arm lengths `d0` and `d1` (as fractions of the chord). Requiring its
/// signed area to equal `area` gives `d1` in terms of `d0`,
///
/// ```text
///     d1 = (d0·sin θ0 − 10/3·area) / (½·d0·sin(θ0 + θ1) − sin θ1)
/// ```
///
/// and substituting that into the requirement that its x-moment equal `mx` leaves a
/// quartic `a4·d0⁴ + … + a0 = 0` whose coefficients are the expressions below (taken
/// from kurbo, not re-derived). Its real roots are found with kurbo's closed-form
/// solvers, falling back to the cubic or quadratic formula when the leading coefficients
/// vanish; a factor with a complex pair contributes its real part, the nearest real
/// candidate. When every coefficient vanishes the conventional `(1/3, 1/3)` is returned.
///
/// Following kurbo, a negative `d0` is replaced by `(0, sin θ0 / sin(θ0+θ1))` and a
/// non-positive `d1` by `(sin θ1 / sin(θ0+θ1), 0)`: the nearest cubic with one arm
/// collapsed. Pairs that are negative or non-finite after that are dropped, so the result
/// may be empty. The caller scores every pair and keeps the best, which is why all are
/// returned rather than one.
pub fn arms_from_moments(th0: f64, th1: f64, area: f64, mx: f64) -> Arms {
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

/// A cubic Bézier with its four control points, in px: start `p0`, controls `p1` and
/// `p2`, end `p3`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cubic {
    pub(crate) p0: Point,
    pub(crate) p1: Point,
    pub(crate) p2: Point,
    pub(crate) p3: Point,
}

impl Cubic {
    /// The G1 cubic with arms `d0`, `d1` (fractions of the chord) along `t0`, `t1`.
    ///
    /// `p1 = p0 + d0·chord·t0` and `p2 = p3 − d1·chord·t1`. `t0` and `t1` must be unit
    /// vectors pointing along the direction of travel; `chord` is `|p3 − p0|` in px.
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

    /// The point at parameter `t`, Bernstein form (the same expression as
    /// [`crate::curves::eval_cubic`], kept inline here for the projection's hot loop).
    #[inline]
    fn eval(&self, t: f64) -> Point {
        let mt = 1.0 - t;
        let (w0, w1, w2, w3) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
        Point::new(
            w0 * self.p0.x + w1 * self.p1.x + w2 * self.p2.x + w3 * self.p3.x,
            w0 * self.p0.y + w1 * self.p1.y + w2 * self.p2.y + w3 * self.p3.y,
        )
    }

    /// The derivative `B'(t)` at `t`, unnormalised (px per unit `t`).
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
    ///
    /// Newton's method on `f(t) = (B(t) − p)·B'(t)`, dropping the second-derivative term
    /// (Gauss–Newton for the foot of the perpendicular):
    /// `t ← clamp(t − (B(t) − p)·B'(t) / |B'(t)|², 0, 1)`, for [`NEWTON_STEPS`] steps or
    /// until `t` stops moving or `B'` vanishes. Started from the point's chord-length
    /// parameter, which is close, three steps are enough; from a poor start this can land
    /// on a local rather than the global nearest point.
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
                if dd >= 1e-18 && next != t[q] {
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
    ///
    /// A control-polygon test: the cross products of consecutive control-polygon edges
    /// `(p1 − p0) × (p2 − p1)` and `(p2 − p1) × (p3 − p2)` have opposite signs, beyond a
    /// tolerance of `1e-6·max(chord², 1)` px². Cheap, and it catches the S-shaped cubics
    /// the wobble penalty is aimed at; it is not an exact inflection test.
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
    ///
    /// With `A = p2 − 2p1 + p0` and `B = p3 − 2p2 + p1`, the second derivative is
    /// `B''(t) = 6((1 − t)·A + t·B)`, so `∫₀¹ |B''(t)|² dt = 12(|A|² + A·B + |B|²)`;
    /// dividing by the squared chord `L²` makes it scale-free. 0 for a chord shorter than
    /// 1e-6 px.
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
    ///
    /// In nats, with `f` = [`WOBBLE_PENALTY`] and `E` the [`Self::bending_energy`]:
    ///
    /// ```text
    ///     penalty = f·λ·( 2·[has an inflection] + ½·min(10, max(0, E − 2.5)) )
    /// ```
    ///
    /// A gentle arc has `E` below 2.5 and pays nothing; an S-bend pays two parameters'
    /// worth. The residual alone cannot see these shapes, because a wobbly cubic can pass
    /// through noisy points as well as a smooth one.
    pub(crate) fn wobble_penalty(&self, lambda: f64) -> f64 {
        let factor = WOBBLE_PENALTY;
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
/// against the parameter ratio; on this evidence it is worth doing properly. (The sweep
/// used `INKVEC_WOBBLE_PENALTY`, removed since: a sweep edits this constant.)
pub(crate) const WOBBLE_PENALTY: f64 = 1.0;

/// Which interior points of a span are scored, at what parameter, against what noise.
///
/// The points are gathered once per span, in [`LANES`]-wide groups (the tail padded with
/// points never read), since every candidate cubic of the span is scored on them.
struct CubicSamples {
    /// The sampled points, in px.
    pt: [Point; MAX_RESIDUAL_SAMPLES],
    /// Each point's chord-length parameter `(s_k − s_i) / (s_j − s_i)`, Newton's start.
    t: [f64; MAX_RESIDUAL_SAMPLES],
    /// Each point's variance `σ²`, px², sigma floored at 1e-6.
    s2: [f64; MAX_RESIDUAL_SAMPLES],
    /// How many entries are real.
    len: usize,
    /// `interior / len`: each sample stands for this many interior points, so the
    /// weighted sum estimates the residual over all of them.
    weight: f64,
}

/// Points [`Cubic::dist2_lanes`] projects at once.
const LANES: usize = 4;
const _: () = assert!(MAX_RESIDUAL_SAMPLES.is_multiple_of(LANES));

/// The `LANES` elements of `a` from `m`.
#[inline]
fn lanes<T>(a: &[T], m: usize) -> &[T; LANES] {
    a[m..m + LANES].try_into().expect("a whole group of lanes")
}

impl CubicSamples {
    /// Pick up to [`MAX_RESIDUAL_SAMPLES`] interior points of `(i, j)`, evenly spaced by
    /// index: sample `m` of `count` is point `i + 1 + floor((m + ½)·interior / count)`.
    /// `None` when the span has no interior points.
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
            #[cfg(test)]
            count_projections();
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

#[cfg(test)]
thread_local! {
    /// Groups of [`LANES`] points this thread has projected, for the tests that check the
    /// bounds actually skip work.
    pub(crate) static PROJECTIONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Count one projected group of lanes (test builds only).
#[cfg(test)]
fn count_projections() {
    PROJECTIONS.with(|c| c.set(c.get() + 1));
}

/// Weighted residual of the interior points of `(i, j)` against a cubic.
///
/// `χ² = weight · Σ_m d_m² / σ_m²` over the sampled interior points, `d_m` the distance
/// (px) from the point to the curve found by Newton projection (see `Cubic::dist2_from`)
/// from its chord-length parameter. With `subsample` the points are thinned to
/// [`MAX_RESIDUAL_SAMPLES`] as in `CubicSamples` and `weight = interior / count`
/// compensates; without it every interior point is used and `weight = 1`. The end
/// points are not scored: the cubic passes through them by construction. 0 for a span
/// with no interior points.
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

/// Control points of the least-squares cubic through `pts[i..=j]` with its end points
/// pinned to `pts[i]` and `pts[j]`.
///
/// Each interior point `k` is given the chord-length parameter
/// `t_k = (s_k − s_i) / (s_j − s_i)`, and `P1`, `P2` minimise
///
/// ```text
///     Σ_k w_k · |p_k − (b0(t_k)·p0 + b1(t_k)·P1 + b2(t_k)·P2 + b3(t_k)·p3)|²,   w_k = 1/σ_k²
/// ```
///
/// with `b` the Bernstein weights. That is linear least squares: a 2x2 system shared by
/// x and y, solved by Cramer's rule. The parameters are not re-optimised, so this is
/// the first step of Schneider's Bézier fit without its reparametrization loop. `None`
/// for fewer than two interior points, a singular system or a non-finite result.
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
///
/// Returns the two control points `(P1, P2)` in px. `s` is the cumulative arc length of
/// `pts` and `sigma` their uncertainties, both in px. See `free_cubic` for the method.
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
    /// Residual plus twice the wobble penalty, so that `½·chi2` carries the penalty once.
    pub(crate) chi2: f64,
    /// Break costs (nats) between the estimated tangents and the cubic's own end
    /// directions.
    pub(crate) brk: f64,
    /// The cubic's own unit end directions, start and end.
    pub(crate) tans: (Vec2, Vec2),
    /// Its arm lengths as fractions of the chord.
    pub(crate) arms: (f64, f64),
}

/// Fit and score the free-tangent cubic for one span, or refuse it.
///
/// `None` unless the research switch is on ([`free_cubic_enabled`]). Otherwise the cubic
/// from `free_cubic` is refused if either end direction swings [`FREE_MAX_SWING`] degrees
/// or more from the estimated tangent (`t0` outgoing at `i`, `t1` incoming at `j`), if
/// either arm exceeds [`MAX_ARM`] chords, or if the chord or an arm has zero length. The
/// caller's cost is `½·chi2 + λ·params_cubic() + brk`.
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
///
/// The end points are the measured `pts[i]` and `pts[j]`, the end directions the unit
/// tangents `t0` (leaving `i`) and `t1` (arriving at `j`); only the arm lengths are
/// chosen. `raw` is `(∫ y dx, ∫ x y dx, ∫ y² dx)` along the points from `i` to `j`. Every
/// real solution of Levien's quartic (`arms_from_moments`) with both arms at most
/// [`MAX_ARM`] chords is scored by [`chi2_cubic`] and the lowest residual wins; with
/// `subsample` the scoring stops early once a candidate can no longer beat the best so
/// far. `None` for a zero-length chord or when no candidate is admissible, which is not
/// the same as "no cubic fits": the caller may still try a free cubic.
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
///
/// When a circular arc through the same span fits at least four times better
/// (`4·χ²_arc < χ²_line`), the line's residuals are not noise: they bow systematically to
/// one side. The line is then charged `span·ln 2` nats, one bit per point, the price of
/// describing which side of the line each point falls on. Otherwise 0. `span` is `j − i`.
pub(crate) fn bow_penalty(chi2_line: f64, chi2_arc: f64, span: usize) -> f64 {
    if chi2_arc * 4.0 < chi2_line {
        span as f64 * std::f64::consts::LN_2
    } else {
        0.0
    }
}

/// Cost of the line `i -> j`: residual, two parameters, and its disagreement with the
/// polyline tangents at whichever ends are joins.
///
/// ```text
///     cost = ½·χ² + λ·PARAMS_LINE + brk(t_out[i], chord) + brk(chord, t_in[j])
/// ```
///
/// in nats, `brk` being [`break_cost`]. An end is a join unless it is the first or last
/// point of an open polyline; `joins_at_ends` marks both ends of an opened loop as joins.
pub(crate) fn line_cost_terms(
    pts: &[Point],
    tan: &Tangents,
    i: usize,
    j: usize,
    chi2: f64,
    cfg: &FitConfig,
    joins_at_ends: bool,
) -> f64 {
    let chord = pts[j] - pts[i];
    let dev = end_break_cost(
        tan,
        pts.len(),
        (i, j),
        (chord, chord),
        cfg.lambda,
        joins_at_ends,
    );
    0.5 * chi2 + cfg.lambda * PARAMS_LINE + dev
}

/// The break costs (nats) a segment from `i` to `j` pays at its ends: between the
/// estimated tangent leaving `i` and the segment's own start direction `d0`, and between
/// its end direction `d1` and the estimated tangent arriving at `j`.
///
/// An end is charged only where it is a join: not at the first or last point of an open
/// polyline of `n` points, unless `joins_at_ends` says the polyline is an opened loop.
fn end_break_cost(
    tan: &Tangents,
    n: usize,
    (i, j): (usize, usize),
    (d0, d1): (Vec2, Vec2),
    lambda: f64,
    joins_at_ends: bool,
) -> f64 {
    let mut dev = 0.0;
    if i > 0 || joins_at_ends {
        dev += break_cost(tan.outgoing[i], d0, lambda);
    }
    if j + 1 < n || joins_at_ends {
        dev += break_cost(d1, tan.incoming[j], lambda);
    }
    dev
}

/// Weighted moments of `(x, y, x² + y²)` along the polyline, so a circle can be fitted to
/// any span in constant time.
///
/// The circle fit is Kåsa's algebraic fit. Writing `z = x² + y²`, a circle is
/// `z + a·x + b·y + c = 0` with centre `(−a/2, −b/2)` and `r² = (a² + b²)/4 − c`, and the
/// algebraic residual `z + a·x + b·y + c = |p − centre|² − r²` is linear in `(a, b, c)`.
/// Minimising `Σ w·(z + a·x + b·y + c)²` is therefore a 3x3 linear least-squares problem
/// whose normal equations need only the ten weighted moments kept here.
pub(crate) struct CirclePrefix {
    /// `m[k]` = sums over points `0..k` of `w·[1, x, y, z, x², x·y, y², x·z, y·z, z²]`,
    /// `w = 1/σ²` (sigma defaulting to 0.5 where missing, floored at 1e-3).
    m: Vec<[f64; 10]>,
}

impl CirclePrefix {
    /// Accumulate the moments of `pts` (px) with weights from `sigma` (px), O(n).
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

    /// The ten moments over the inclusive range `[i, j]`.
    #[inline]
    fn window(&self, i: usize, j: usize) -> [f64; 10] {
        let (a, b) = (&self.m[i], &self.m[j + 1]);
        let mut out = [0.0f64; 10];
        for k in 0..10 {
            out[k] = b[k] - a[k];
        }
        out
    }

    /// RMS distance (px) of the points `[i, j]` from their weighted centroid, floored at 1:
    /// the span's size, against which an implausibly large radius is judged.
    fn scale(&self, i: usize, j: usize) -> f64 {
        let [s0, sx, sy, _, sxx, _, syy, _, _, _] = self.window(i, j);
        if s0 <= 0.0 {
            return 1.0;
        }
        let var_x = (sxx - sx * sx / s0).max(0.0) / s0;
        let var_y = (syy - sy * sy / s0).max(0.0) / s0;
        (var_x + var_y).sqrt().max(1.0)
    }

    /// χ² of the points `[i, j]` about a given circle (centre `c`, radius `r`, px).
    ///
    /// Uses the algebraic residual `e = |p − c|² − r² = d·(2r + d)`, with `d` the signed
    /// distance to the circle, so `e² / 4r² ≈ d²` for points near it. Summed from the
    /// moments in O(1). Infinite for a radius at or below 1e-12.
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

    /// Kåsa fit to the points `[i, j]`: `(centre, radius, χ²)`, px and dimensionless, the
    /// χ² as in [`Self::residual_about`].
    ///
    /// `None` for zero total weight, a (numerically) singular system, as from fewer than
    /// three distinct points, or a non-positive `r²`. Nearly collinear points give a huge
    /// radius rather than `None`; [`try_arc`] refuses those. The algebraic fit is biased towards
    /// smaller radii on short arcs; callers score the arc they will actually draw, not
    /// this one.
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
    /// `½·chi2 + λ·`[`crate::cost::arc_params`]` + end breaks`, nats.
    pub(crate) cost: f64,
    /// Residual about the arc as it will be drawn.
    pub(crate) chi2: f64,
    /// Radius to write, px: the mean distance of the two end points from the fitted centre.
    pub(crate) radius: f64,
    /// SVG `large-arc-flag`.
    pub(crate) large_arc: bool,
    /// SVG `sweep-flag`: true when the arc turns towards increasing angle.
    pub(crate) sweep: bool,
    /// The arc's own unit tangents at its start and end, in the direction of travel.
    pub(crate) tans: (Vec2, Vec2),
}

/// Fit a circular arc to the span `(i, j)` and cost it, or refuse it.
///
/// 1. Kåsa-fit a circle to the points ([`CirclePrefix::fit`]); refuse a radius over a
///    thousand times the span's own size, which is a straight run in disguise.
/// 2. The points must go round the centre one way only: the cross products of
///    consecutive radius vectors, checked at about eight strides along the span, never
///    change sign.
/// 3. The turn from start to end must agree with that sense and lie between 1e-3 rad and
///    `crate::primitives::MAX_ARC_DEGREES`.
/// 4. Score the arc a renderer would draw: SVG reconstructs the circle from the two end
///    points and a radius, so the radius written is the mean end-point distance from the
///    centre, and χ² is measured about the circle `arc_center` rebuilds from it.
///
/// The span needs at least one interior point (`j ≥ i + 2`). See [`ArcSpan`] for the cost.
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
    let (c, r, _) = pre.fit(i, j)?;
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

    let dev = end_break_cost(tan, pts.len(), (i, j), (t0, t1), cfg.lambda, joins_at_ends);

    let large_arc = turn > std::f64::consts::PI;
    let radius = 0.5 * (start.dist(c) + end.dist(c));
    let (drawn_c, drawn_r, _, _) = crate::curves::arc_center(start, radius, large_arc, ccw, end);
    let chi2 = pre.residual_about(i, j, drawn_c, drawn_r);
    Some(ArcSpan {
        cost: 0.5 * chi2 + cfg.lambda * crate::cost::arc_params() + dev,
        chi2,
        radius,
        large_arc,
        sweep: ccw,
        tans: (t0, t1),
    })
}

/// An elliptical arc fitted to one span.
pub(crate) struct EllipseSpan {
    /// `½·χ² + λ·PARAMS_ELLIPTICAL_ARC + end breaks`, nats.
    pub(crate) cost: f64,
    /// Radius along the ellipse's own x-axis, as drawn, px.
    pub(crate) rx: f64,
    /// Radius along the ellipse's own y-axis, as drawn, px.
    pub(crate) ry: f64,
    /// Rotation of the x-axis, radians.
    pub(crate) phi: f64,
    /// SVG `large-arc-flag`.
    pub(crate) large_arc: bool,
    /// SVG `sweep-flag`.
    pub(crate) sweep: bool,
    /// The arc's own unit tangents at its start and end, in the direction of travel.
    pub(crate) tans: (Vec2, Vec2),
}

/// Weighted Sampson distance of the points from the ellipse `(c, rx, ry, phi)`.
///
/// In the ellipse's own frame `(u, v)` the curve is `Q = u²/rx² + v²/ry² − 1 = 0`, and
/// the Sampson distance is the first-order approximation of the geometric distance,
/// `d ≈ Q / |∇Q|` with `|∇Q| = √(4u²/rx⁴ + 4v²/ry⁴)`. Accurate near the curve, which is
/// where a candidate worth keeping has its points, and closed form where the exact
/// distance to an ellipse needs a quartic. Returns `Σ (d_k / σ_k)²` (sigma defaulting to
/// 0.5, floored at 1e-3); infinite if a point sits at the centre, where `∇Q` vanishes.
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

/// Fit an elliptical arc to the span `(i, j)` and cost it, or refuse it.
///
/// Only spans of at least 24 points whose length is a multiple of 16 are tried: the
/// algebraic fit is O(j − i), so this keeps the ellipse to a sparse grid of candidate
/// ends rather than making the dynamic program cubic. Then:
///
/// 1. fit an ellipse algebraically (Taubin's fit, `crate::primitives::ellipse::taubin_ellipse`,
///    which skips the orthogonal χ² this candidate never reads) and
///    refuse aspect ratios over 12 or a major radius over a thousand times the span's
///    extent;
/// 2. the points must advance round it monotonically ([`ellipse_turn`]) through a total
///    turn between 1e-3 rad and `MAX_ARC_DEGREES`;
/// 3. rescale the radii by the end points' mean normalised radius (refused outside
///    0.5..2), so the arc a renderer rebuilds from the end points is the one scored, and
///    rebuild it with [`crate::curves::arc_ellipse_center`];
/// 4. score by Sampson distance ([`ellipse_sampson_chi2`]) and charge end breaks against
///    the arc's own tangents ([`ellipse_end_tangents`]).
///
/// See [`EllipseSpan`] for the cost.
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
    let fit = crate::primitives::ellipse::taubin_ellipse(span_pts, span_sigma)?;
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

    let (turn, ccw) = ellipse_turn(span_pts, &fit)?;
    let (sp0, cp0) = fit.angle.sin_cos();

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

    let (t0, t1) = ellipse_end_tangents(&frame)?;
    let dev = end_break_cost(tan, pts.len(), (i, j), (t0, t1), cfg.lambda, joins_at_ends);
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

/// How far, and which way, the points go round a fitted ellipse: `(|total turn|, ccw)`,
/// radians, or `None` if they do not go round it monotonically or the turn is outside
/// `[1e-3, MAX_ARC_DEGREES]`.
///
/// Each point's parametric angle is read straight from its projection into the ellipse's
/// frame, `atan2(v/ry, u/rx)` — no iteration, and monotone in the true one for points
/// near the curve, which is all the direction test needs. The sense is set by the first
/// step; any later step backwards by more than 1e-3 rad refuses the arc. Steps are
/// unwrapped into `(−π, π]`. `span_pts` must hold at least two points.
fn ellipse_turn(span_pts: &[Point], fit: &crate::primitives::EllipseFit) -> Option<(f64, bool)> {
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
    Some((turn, ccw))
}

/// Unit tangents of a drawn elliptical arc at its start and end, in the direction of
/// travel: the derivative of `c + R(φ)·(rx cos t, ry sin t)`, i.e.
/// `R(φ)·(−rx sin t, ry cos t)`, reversed when the sweep is negative. `None` if either
/// vanishes.
fn ellipse_end_tangents(frame: &crate::curves::ArcFrame) -> Option<(Vec2, Vec2)> {
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
    Some((t0, t1))
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
