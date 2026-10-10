//! The boundary likelihood: the interface between recovering the picture (boundary chain)
//! and writing it with the fewest coordinates (representation chain).
//!
//! An anti-aliased pixel holds the area of each face inside it, so summed over any set of
//! whole pixels a face's unmixed weights are its exact area there, whatever the shape
//! (`docs/theory/chain-boundary.md`, B2.1; `column_sum_eq_average` in
//! `formal/InkvecTheory`). A candidate description is therefore scored against the pixels by
//! computing, for each *window* of pixels, the area its curves give the face, and comparing
//! that with the measured sum: no boundary points are extracted first. This module holds the
//! vocabulary both chains share:
//!
//! * [`Piece`]: a candidate curve's pieces (lines, quadratic and cubic Béziers, elliptical
//!   arcs), in pixels, with pixel centres on integers;
//! * [`RenderModel`]: how the renderer draws them (arcs as the cubics usvg writes);
//! * [`Window`] and [`left_area`]: a window of whole pixels along one grid line, and the exact
//!   area a curve crossing it gives its left face there;
//! * [`RunObs`], [`Chi2`], [`Floor`], [`RunMoments`]: one measured window, a score, the
//!   renderer floor, and least-squares fits of polynomial graphs over ranges of windows in
//!   `O(p²)` per range;
//! * [`JunctionReport`], [`CornerProposal`]: what the boundary chain reports where runs end;
//! * [`BoundaryLikelihood`]: the evaluator the representation chain scores descriptions with.
//!
//! Coordinates are inkvec's: pixel `(x, y)` covers `[x − ½, x + ½] × [y − ½, y + ½]`, `y`
//! down. A curve's *left* face lies on `(d.y, −d.x)` of its direction `d` (walking right, the
//! face above; walking down, the face to the east), the planar map's convention.

use crate::Point;
use std::f64::consts::PI;
use std::ops::Range;

/// One piece of a candidate curve, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Piece {
    /// A straight segment.
    Line([Point; 2]),
    /// A quadratic Bézier.
    Quad([Point; 3]),
    /// A cubic Bézier.
    Cubic([Point; 4]),
    /// An elliptical arc, drawn as the renderer draws it ([`RenderModel::as_rendered`]).
    Arc(EllipticalArc),
}

/// An elliptical arc in centre form: the points `centre + R(x_rotation)·(rx cos θ, ry sin θ)`
/// for `θ` from `start_angle` through `start_angle + sweep_angle` (kurbo's `Arc`, which usvg
/// builds from SVG's endpoint form).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EllipticalArc {
    /// Centre, px.
    pub centre: Point,
    /// Radii along the rotated axes, px.
    pub radii: (f64, f64),
    /// Rotation of the ellipse's first axis from `+x` towards `+y`, radians.
    pub x_rotation: f64,
    /// Parameter angle at the start, radians.
    pub start_angle: f64,
    /// Signed parameter sweep, radians.
    pub sweep_angle: f64,
}

impl EllipticalArc {
    /// The point at parameter angle `a`.
    pub fn at(&self, a: f64) -> Point {
        let (u, v) = rotated(self.radii, self.x_rotation, a);
        Point::new(self.centre.x + u, self.centre.y + v)
    }

    /// The cubics kurbo's `Arc::append_iter(tolerance)` writes for this arc, which is how
    /// usvg converts every SVG arc, circle, ellipse and rounded corner before resvg draws
    /// it (`usvg::parser::shapes`, tolerance 0.1 user units). `tolerance` is in the arc's own
    /// units (px here). Port of kurbo 0.13.1 `arc.rs`; the subdivision count is
    /// `ceil(max((1.1163·r_max/tolerance)^(1/6), 4)·|sweep|/2π)`, so an arc of under a quarter
    /// turn is one cubic for every radius under about 3600 tolerances.
    pub fn to_cubics(&self, tolerance: f64) -> Vec<Piece> {
        let (rx, ry) = self.radii;
        if rx <= 0.0 || ry <= 0.0 || self.sweep_angle == 0.0 || tolerance <= 0.0 {
            return vec![Piece::Line([
                self.at(self.start_angle),
                self.at(self.start_angle + self.sweep_angle),
            ])];
        }
        let sign = self.sweep_angle.signum();
        let scaled_err = rx.max(ry) / tolerance;
        let n_err = (1.1163 * scaled_err).powf(1.0 / 6.0).max(3.999_999);
        let n = (n_err * self.sweep_angle.abs() * (1.0 / (2.0 * PI)))
            .ceil()
            .max(1.0);
        let step = self.sweep_angle / n;
        let arm = (4.0 / 3.0) * (0.25 * step).abs().tan() * sign;
        let mut out = Vec::with_capacity(n as usize);
        let mut a0 = self.start_angle;
        let mut p0 = rotated(self.radii, self.x_rotation, a0);
        let c = self.centre;
        for _ in 0..n as usize {
            let a1 = a0 + step;
            let d0 = rotated(self.radii, self.x_rotation, a0 + PI / 2.0);
            let p3 = rotated(self.radii, self.x_rotation, a1);
            let d3 = rotated(self.radii, self.x_rotation, a1 + PI / 2.0);
            out.push(Piece::Cubic([
                Point::new(c.x + p0.0, c.y + p0.1),
                Point::new(c.x + p0.0 + arm * d0.0, c.y + p0.1 + arm * d0.1),
                Point::new(c.x + p3.0 - arm * d3.0, c.y + p3.1 - arm * d3.1),
                Point::new(c.x + p3.0, c.y + p3.1),
            ]));
            a0 = a1;
            p0 = p3;
        }
        out
    }
}

/// `R(rot)·(rx cos a, ry sin a)`.
fn rotated(radii: (f64, f64), rot: f64, a: f64) -> (f64, f64) {
    let (u, v) = (radii.0 * a.cos(), radii.1 * a.sin());
    let (s, c) = rot.sin_cos();
    (u * c - v * s, u * s + v * c)
}

/// How the gate's renderer draws a description: resvg 0.48 (usvg converts arcs to cubics at
/// 0.1 user units; tiny-skia then rasterises at 8× with 4 × 4 samples per device pixel,
/// which the corpus builder box-filters to the intake). The window areas of this module are
/// exact for the cubics; the sampling lattice and tiny-skia's own curve flattening are the
/// renderer floor ([`Floor`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderModel {
    /// usvg's arc tolerance, px: 0.1 times the pixels per user unit of the artist's file.
    pub arc_tolerance_px: f64,
}

impl Default for RenderModel {
    /// One user unit per pixel, inkvec's own output frame.
    fn default() -> Self {
        Self {
            arc_tolerance_px: 0.1,
        }
    }
}

impl RenderModel {
    /// The pieces as the renderer draws them: every arc replaced by usvg's cubics, the rest
    /// unchanged.
    pub fn as_rendered(&self, pieces: &[Piece]) -> Vec<Piece> {
        let mut out = Vec::with_capacity(pieces.len());
        for p in pieces {
            match p {
                Piece::Arc(a) => out.extend(a.to_cubics(self.arc_tolerance_px)),
                other => out.push(*other),
            }
        }
        out
    }
}

/// Which grid lines a window runs along.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Axis {
    /// A column of pixels: the boundary crosses it as a graph over `x`, within 45° of
    /// horizontal.
    Column,
    /// A row of pixels: the boundary crosses it as a graph over `y`.
    Row,
}

/// A window: the pixels `lo..=hi` along one grid line (`x = line` for a column, `y = line`
/// for a row). Its strip is `[line − ½, line + ½]` across and `[lo − ½, hi + ½]` along.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Window {
    /// Column or row.
    pub axis: Axis,
    /// The column's `x` (or the row's `y`).
    pub line: i32,
    /// First pixel along the line.
    pub lo: i32,
    /// Last pixel along the line, inclusive.
    pub hi: i32,
}

impl Window {
    /// Number of pixels.
    pub fn len(&self) -> usize {
        (self.hi - self.lo + 1).max(0) as usize
    }

    /// Whether the window holds no pixel.
    pub fn is_empty(&self) -> bool {
        self.hi < self.lo
    }

    /// The pixels `(x, y)` in order along the line.
    pub fn pixels(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        (self.lo..=self.hi).map(move |k| match self.axis {
            Axis::Column => (self.line, k),
            Axis::Row => (k, self.line),
        })
    }
}

/// A cubic polynomial `c0 + c1 t + c2 t² + c3 t³`.
#[derive(Debug, Clone, Copy)]
struct Poly([f64; 4]);

impl Poly {
    fn eval(&self, t: f64) -> f64 {
        let c = self.0;
        c[0] + t * (c[1] + t * (c[2] + t * c[3]))
    }
}

/// Power-basis coordinates of a piece over `t ∈ [0, 1]`; `None` for an arc (convert first).
fn power(p: &Piece) -> Option<(Poly, Poly)> {
    let comp = |f: &dyn Fn(Point) -> f64| -> Poly {
        match p {
            Piece::Line([a, b]) => Poly([f(*a), f(*b) - f(*a), 0.0, 0.0]),
            Piece::Quad([a, b, c]) => Poly([
                f(*a),
                2.0 * (f(*b) - f(*a)),
                f(*a) - 2.0 * f(*b) + f(*c),
                0.0,
            ]),
            Piece::Cubic([a, b, c, d]) => Poly([
                f(*a),
                3.0 * (f(*b) - f(*a)),
                3.0 * (f(*a) - 2.0 * f(*b) + f(*c)),
                -f(*a) + 3.0 * f(*b) - 3.0 * f(*c) + f(*d),
            ]),
            Piece::Arc(_) => Poly([0.0; 4]),
        }
    };
    if matches!(p, Piece::Arc(_)) {
        return None;
    }
    Some((comp(&|q| q.x), comp(&|q| q.y)))
}

/// The roots in `(0, 1)` of `q(t) = level`, appended to `out`. Monotone pieces between the
/// critical points of `q` are bisected; a double root (a tangency) changes no sign and is
/// not reported, which is what the integrals below need.
fn roots_in_unit(q: &Poly, level: f64, out: &mut Vec<f64>) {
    let c = q.0;
    let (a, b, cc) = (3.0 * c[3], 2.0 * c[2], c[1]);
    let mut cuts = [0.0, 1.0, 1.0, 1.0];
    let mut n = 1;
    let scale = c.iter().fold(0.0f64, |m, v| m.max(v.abs())).max(1e-300);
    if a.abs() > 1e-14 * scale {
        let disc = b * b - 4.0 * a * cc;
        if disc > 0.0 {
            let s = disc.sqrt();
            let q0 = -0.5 * (b + if b >= 0.0 { s } else { -s });
            for r in [q0 / a, if q0 != 0.0 { cc / q0 } else { f64::NAN }] {
                if r > 0.0 && r < 1.0 {
                    cuts[n] = r;
                    n += 1;
                }
            }
        }
    } else if b.abs() > 1e-14 * scale {
        let r = -cc / b;
        if r > 0.0 && r < 1.0 {
            cuts[n] = r;
            n += 1;
        }
    }
    cuts[n] = 1.0;
    n += 1;
    let cuts = &mut cuts[..n];
    cuts.sort_by(f64::total_cmp);
    for w in cuts.windows(2) {
        let (mut lo, mut hi) = (w[0], w[1]);
        if hi - lo <= 0.0 {
            continue;
        }
        let (flo, fhi) = (q.eval(lo) - level, q.eval(hi) - level);
        if flo == 0.0 && lo > 0.0 {
            out.push(lo);
            continue;
        }
        if flo * fhi >= 0.0 {
            continue;
        }
        let up = fhi > flo;
        for _ in 0..80 {
            let mid = 0.5 * (lo + hi);
            if mid <= lo || mid >= hi {
                break;
            }
            if (q.eval(mid) - level > 0.0) == up {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        out.push(0.5 * (lo + hi));
    }
}

/// One pass of a curve through a strip: the integral `∫ (clamp(v) − v_lo) du`, the net advance
/// `∫ du`, and the range of `v` it reached.
#[derive(Debug, Clone, Copy)]
struct Pass {
    area: f64,
    adv: f64,
    vmin: f64,
    vmax: f64,
}

/// The passes of one piece through the strip `u ∈ [u0, u1]`: per maximal `t`-interval inside
/// it, `∫ (clamp(v(t), vlo, vhi) − vlo) u'(t) dt` and `∫ u'(t) dt`, with whether the interval
/// touches `t = 0` and `t = 1` (so that passes continue across pieces). Exact up to the root
/// finding: between breakpoints the integrand is a polynomial integrated in closed form.
fn passes(
    u: &Poly,
    v: &Poly,
    u0: f64,
    u1: f64,
    vlo: f64,
    vhi: f64,
    out: &mut Vec<(Pass, bool, bool)>,
) {
    let mut ts: Vec<f64> = Vec::with_capacity(16);
    ts.push(0.0);
    for (q, lv) in [(u, u0), (u, u1), (v, vlo), (v, vhi)] {
        roots_in_unit(q, lv, &mut ts);
    }
    ts.push(1.0);
    ts.sort_by(f64::total_cmp);
    // (v − vlo)·u' as a degree-5 polynomial, and its antiderivative.
    let va = [v.0[0] - vlo, v.0[1], v.0[2], v.0[3]];
    let ud = [u.0[1], 2.0 * u.0[2], 3.0 * u.0[3]];
    let mut prod = [0.0f64; 6];
    for (i, a) in va.iter().enumerate() {
        for (j, b) in ud.iter().enumerate() {
            prod[i + j] += a * b;
        }
    }
    let anti = |t: f64| -> f64 {
        let mut acc = 0.0;
        for k in (0..6).rev() {
            acc = acc * t + prod[k] / (k as f64 + 1.0);
        }
        acc * t
    };
    let mut cur: Option<(Pass, bool)> = None;
    for w in ts.windows(2) {
        let (ta, tb) = (w[0], w[1]);
        if tb <= ta {
            continue;
        }
        let tm = 0.5 * (ta + tb);
        let um = u.eval(tm);
        if um < u0 || um > u1 {
            if let Some((p, at0)) = cur.take() {
                out.push((p, at0, false));
            }
            continue;
        }
        let du = u.eval(tb) - u.eval(ta);
        let (va, vm, vb) = (v.eval(ta), v.eval(tm), v.eval(tb));
        let add = if vm <= vlo {
            0.0
        } else if vm >= vhi {
            (vhi - vlo) * du
        } else {
            anti(tb) - anti(ta)
        };
        let p = cur.get_or_insert((
            Pass {
                area: 0.0,
                adv: 0.0,
                vmin: f64::INFINITY,
                vmax: f64::NEG_INFINITY,
            },
            ta == 0.0,
        ));
        p.0.area += add;
        p.0.adv += du;
        p.0.vmin = p.0.vmin.min(va.min(vm).min(vb));
        p.0.vmax = p.0.vmax.max(va.max(vm).max(vb));
    }
    if let Some((p, at0)) = cur.take() {
        out.push((p, at0, true));
    }
}

/// The area the curve `pieces` gives its *left* face inside window `w`, px².
///
/// Exact for lines and Béziers (arcs are integrated through their renderer cubics at a
/// tolerance of `1e-6` px, so convert them with [`RenderModel::as_rendered`] first to score
/// a candidate as drawn). By the window identity this is what the left face's unmixed
/// weights summed over the window measure. The curve must cross the window's strip once
/// (enter through one side, leave through the other), as a run's curve does; it may wiggle
/// or fold inside it, and leave the window along the line, which is the clamp.
///
/// With `I = ∫ (clamp(v) − v_lo) du` along the curve's pass through the strip and `Δ = ±1` its
/// net advance (`u` across the strip, `v` along it): a column's left face has area
/// `I + h(1 − Δ)/2`, a row's `−I + h(1 + Δ)/2`, `h` the window's length (Green's theorem with
/// the form `(v − v_lo) du`; `chain-boundary.md` B2.1). Only the passes that reach the
/// window (within a pixel of it along the line) count, so a curve that crosses the same
/// column twice, a U, is scored at each window by its own pass. A curve with no pass near
/// the window leaves it wholly to one face: the left one when the nearest pass has the
/// window on its left.
pub fn left_area(pieces: &[Piece], w: &Window) -> f64 {
    let (u0, u1) = (w.line as f64 - 0.5, w.line as f64 + 0.5);
    let (vlo, vhi) = (w.lo as f64 - 0.5, w.hi as f64 + 0.5);
    let h = vhi - vlo;
    // The passes of every piece, joined across piece boundaries.
    let mut all: Vec<Pass> = Vec::new();
    let mut open = false;
    let mut scratch = Vec::with_capacity(4);
    let mut acc = |p: &Piece, all: &mut Vec<Pass>, open: &mut bool| {
        let Some((px, py)) = power(p) else { return };
        let (u, v) = match w.axis {
            Axis::Column => (px, py),
            Axis::Row => (py, px),
        };
        let (umin, umax) = u_range(p, w.axis);
        if umax < u0 || umin > u1 {
            *open = false;
            return;
        }
        scratch.clear();
        passes(&u, &v, u0, u1, vlo, vhi, &mut scratch);
        if scratch.is_empty() {
            *open = false;
        }
        for (k, &(ps, at0, at1)) in scratch.iter().enumerate() {
            match all.last_mut() {
                Some(last) if *open && k == 0 && at0 => {
                    last.area += ps.area;
                    last.adv += ps.adv;
                    last.vmin = last.vmin.min(ps.vmin);
                    last.vmax = last.vmax.max(ps.vmax);
                }
                _ => all.push(ps),
            }
            *open = at1;
        }
    };
    for p in pieces {
        match p {
            Piece::Arc(a) => {
                for c in a.to_cubics(1e-6) {
                    acc(&c, &mut all, &mut open);
                }
            }
            other => acc(other, &mut all, &mut open),
        }
    }
    let near: Vec<&Pass> = all
        .iter()
        .filter(|p| p.vmax >= vlo - 1.0 && p.vmin <= vhi + 1.0)
        .collect();
    let (i_sum, adv) = if near.is_empty() {
        // The window is one face's: the nearest pass's side decides.
        let Some(p) = all.iter().min_by(|a, b| {
            let da = (a.vmin - vhi).max(vlo - a.vmax);
            let db = (b.vmin - vhi).max(vlo - b.vmax);
            da.total_cmp(&db)
        }) else {
            return f64::NAN;
        };
        let below = p.vmin > vhi; // the curve passes beyond the window's high end
        let forward = p.adv > 0.0;
        // Column: walking +x the left face is on the low side (towards v_lo): it covers a
        // window that lies on the low side of the pass, i.e. when the pass is below it.
        let left_has_it = match w.axis {
            Axis::Column => below == forward,
            Axis::Row => below != forward,
        };
        return if left_has_it { h } else { 0.0 };
    } else {
        near.iter()
            .fold((0.0, 0.0), |(a, d), p| (a + p.area, d + p.adv))
    };
    match w.axis {
        Axis::Column => i_sum + h * (1.0 - adv) / 2.0,
        Axis::Row => -i_sum + h * (1.0 + adv) / 2.0,
    }
}

fn u_range(p: &Piece, axis: Axis) -> (f64, f64) {
    let pts: &[Point] = match p {
        Piece::Line(q) => q,
        Piece::Quad(q) => q,
        Piece::Cubic(q) => q,
        Piece::Arc(_) => return (f64::NEG_INFINITY, f64::INFINITY),
    };
    let f = |q: &Point| if axis == Axis::Column { q.x } else { q.y };
    pts.iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| {
            (lo.min(f(q)), hi.max(f(q)))
        })
}

/// One measured run window of an edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunObs {
    /// The window.
    pub window: Window,
    /// Position along the edge's starting geometry, px of arclength from its start.
    pub s: f64,
    /// Measured area of the edge's left face in the window: the sum of its unmixed weights
    /// over the window's pixels, unclamped.
    pub sum: f64,
    /// Variance of `sum`: quantisation of the partial pixels and the renderer's per-window
    /// floor (not the per-edge part, which is [`Floor::edge_var`]).
    pub var: f64,
    /// Whether the left face lies on the window's low side (above a column, west of a row)
    /// on the starting geometry.
    pub left_low: bool,
}

impl RunObs {
    /// The measured mean position of the boundary across the window's strip, along the
    /// line (a column's `y`, a row's `x`), px: the window identity read as a point
    /// measurement for curves that are graphs over the strip.
    pub fn mean_position(&self) -> f64 {
        let lo = self.window.lo as f64 - 0.5;
        if self.left_low {
            lo + self.sum
        } else {
            lo + self.window.len() as f64 - self.sum
        }
    }
}

/// A score: `χ²` and the number of measurements `M` it sums, so that at the truth
/// `E χ² = M` (`chain-boundary.md` B2.4).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Chi2 {
    /// `Σ r² / V` over the windows, with each window's own variance.
    pub chi2: f64,
    /// Number of measurements.
    pub m: usize,
    /// `χ²` with the renderer floor's per-edge term as well: the windows of one edge share
    /// an unknown offset of variance [`Floor::edge_var`] (A3), so this is
    /// `χ² − (b² (Σ r/V)²) / (1 + b² Σ 1/V)`, the Sherman–Morrison form of the covariance
    /// `V + b² 1 1ᵀ`. Equal to `chi2` where the floor is zero.
    pub chi2_floor: f64,
}

/// Sum of two scores over disjoint measurements.
impl std::ops::Add for Chi2 {
    type Output = Chi2;

    fn add(self, o: Chi2) -> Chi2 {
        Chi2 {
            chi2: self.chi2 + o.chi2,
            m: self.m + o.m,
            chi2_floor: self.chi2_floor + o.chi2_floor,
        }
    }
}

/// The renderer floor: what the renderer adds to the window sums beyond quantisation
/// (`chain-boundary.md` B2.3; measured by `bench/theory/renderer_floor.py`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Floor {
    /// Samples per pixel along each axis of the renderer's sampling lattice (32 for the
    /// corpus intake: 8× rendering, 4 × 4 samples per device pixel).
    pub lattice: u32,
    /// Variance per window that behaves as independent noise (generic slopes), px².
    pub window_var: f64,
    /// Variance of the offset shared by every window of one edge running within a lattice
    /// step of an axis or a diagonal, px²: `1/(12 n²)` for an `n`-lattice. Different edges are
    /// independent (A3). Add it to tests of absolute positions; leave it out of tests along
    /// one edge.
    pub edge_var: f64,
}

impl Floor {
    /// The floor of an `n × n` point-sampling lattice: `1/(12 n³)` per window at generic
    /// slopes, `1/(12 n²)` per near-axis edge.
    pub fn lattice(n: u32) -> Self {
        let n = n.max(1) as f64;
        Self {
            lattice: n as u32,
            window_var: 1.0 / (12.0 * n * n * n),
            edge_var: 1.0 / (12.0 * n * n),
        }
    }
}

/// A place where a run may end in a corner, proposed for the representation to test
/// (`chain-boundary.md` B3.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CornerProposal {
    /// The edge.
    pub edge: u32,
    /// Index of the run observation at the centre of the five whose fourth difference fired.
    pub index: usize,
    /// `|D| / √(70 V)`: the fourth difference in standard deviations.
    pub z: f64,
    /// Where, on the starting geometry.
    pub at: Point,
}

/// One arm of a junction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arm {
    /// The edge.
    pub edge: u32,
    /// Whether the edge starts at the junction (else it ends there).
    pub at_start: bool,
    /// Direction leaving the junction, radians from `+x` towards `+y`.
    pub direction: f64,
    /// Standard deviation of `direction`, radians.
    pub direction_sd: f64,
}

/// A junction where three or more faces meet, its arms in cyclic order, and the pairs of
/// arms that may be one curve continuing through it (`chain-representation.md` A4).
#[derive(Debug, Clone, PartialEq)]
pub struct JunctionReport {
    /// Node id in the planar map.
    pub node: u32,
    /// Starting position.
    pub at: Point,
    /// Arms in increasing `direction` (counter-clockwise on screen is decreasing; this is
    /// the order of angles from `+x` towards `+y`).
    pub arms: Vec<Arm>,
    /// `(i, j, z)`: arms `i` and `j` (indices into `arms`) leave in opposite directions to
    /// within `z` standard deviations of their difference, so they may continue one another.
    pub continuations: Vec<(usize, usize, f64)>,
}

/// Owner of a local term: the vertex or thin stretch whose pixels it scores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Owner {
    /// A junction node of the planar map.
    Junction(u32),
    /// A corner proposal, by index.
    Corner(u32),
}

/// Least-squares fits of polynomial graphs over ranges of an edge's run windows from prefix
/// sums (`chain-boundary.md` B2.4): the window identity read as a linear model,
/// `mean_position = Σ_k θ_k · mean over the strip of (u − u_ref)^k`.
#[derive(Debug, Clone, PartialEq)]
pub struct RunMoments {
    /// Polynomial degree.
    pub degree: usize,
    /// Reference abscissa `u_ref`.
    pub u_ref: f64,
    /// Axis of every window (`None` past an axis switch: such a range cannot be fitted).
    axis: Vec<Axis>,
    /// Prefix sums: per window, `G = Σ g gᵀ/V` (upper triangle), `b = Σ g h/V`, `c = Σ h²/V`.
    pre: Vec<Vec<f64>>,
}

impl RunMoments {
    /// Build from an edge's run windows.
    pub fn new(obs: &[RunObs], degree: usize) -> Self {
        let degree = degree.min(3);
        let p = degree + 1;
        let u_ref = obs.first().map_or(0.0, |o| o.window.line as f64);
        let width = p * (p + 1) / 2 + p + 1;
        let mut pre = vec![vec![0.0; width]];
        let mut axis = Vec::with_capacity(obs.len());
        for o in obs {
            let (u0, u1) = (
                o.window.line as f64 - 0.5 - u_ref,
                o.window.line as f64 + 0.5 - u_ref,
            );
            // g_k = mean of (u − u_ref)^k over the strip.
            let g: Vec<f64> = (0..p)
                .map(|k| (u1.powi(k as i32 + 1) - u0.powi(k as i32 + 1)) / (k as f64 + 1.0))
                .collect();
            let h = o.mean_position();
            let wv = 1.0 / o.var.max(1e-300);
            let last = pre.last().expect("seeded").clone();
            let mut row = last;
            let mut idx = 0;
            for i in 0..p {
                for j in i..p {
                    row[idx] += g[i] * g[j] * wv;
                    idx += 1;
                }
            }
            for i in 0..p {
                row[idx + i] += g[i] * h * wv;
            }
            row[idx + p] += h * h * wv;
            pre.push(row);
            axis.push(o.window.axis);
        }
        Self {
            degree,
            u_ref,
            axis,
            pre,
        }
    }

    /// The weighted least-squares polynomial over windows `r` and its `χ²`, or `None` when the
    /// range mixes columns and rows, has fewer windows than coefficients, or is singular.
    #[allow(clippy::needless_range_loop)] // a 4 × 4 symmetric matrix filled from its triangle
    pub fn fit(&self, r: Range<usize>) -> Option<(Vec<f64>, Chi2)> {
        let p = self.degree + 1;
        if r.end > self.axis.len()
            || r.len() < p
            || self.axis[r.clone()]
                .iter()
                .any(|a| *a != self.axis[r.start])
        {
            return None;
        }
        let (a, b) = (&self.pre[r.end], &self.pre[r.start]);
        let d: Vec<f64> = a.iter().zip(b).map(|(x, y)| x - y).collect();
        let mut g = vec![vec![0.0; p]; p];
        let mut idx = 0;
        for i in 0..p {
            for j in i..p {
                g[i][j] = d[idx];
                g[j][i] = d[idx];
                idx += 1;
            }
        }
        let rhs: Vec<f64> = (0..p).map(|i| d[idx + i]).collect();
        let c = d[idx + p];
        let theta = solve_spd(&g, &rhs)?;
        let fit: f64 = theta.iter().zip(&rhs).map(|(t, b)| t * b).sum();
        let chi2 = (c - fit).max(0.0);
        Some((
            theta,
            Chi2 {
                chi2,
                m: r.len(),
                chi2_floor: chi2,
            },
        ))
    }
}

/// Solve `G θ = b` for a small symmetric positive definite `G` (Cholesky); `None` when not
/// positive definite to working precision.
fn solve_spd(g: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    let mut l = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..=i {
            let s: f64 = (0..j).map(|k| l[i][k] * l[j][k]).sum();
            if i == j {
                let d = g[i][i] - s;
                if d <= 1e-14 * g[i][i].abs().max(1e-300) {
                    return None;
                }
                l[i][i] = d.sqrt();
            } else {
                l[i][j] = (g[i][j] - s) / l[j][j];
            }
        }
    }
    let mut y = vec![0.0; n];
    for i in 0..n {
        y[i] = (b[i] - (0..i).map(|k| l[i][k] * y[k]).sum::<f64>()) / l[i][i];
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        x[i] = (y[i] - (i + 1..n).map(|k| l[k][i] * x[k]).sum::<f64>()) / l[i][i];
    }
    Some(x)
}

/// The evaluator the representation chain scores candidate descriptions with. Every score
/// is `−2 log L` up to a constant, calibrated so that `E χ² = M` at the truth.
pub trait BoundaryLikelihood {
    /// Number of edges of the starting map.
    fn edge_count(&self) -> usize;
    /// The run observations of edge `e`, in order along it.
    fn runs(&self, e: usize) -> &[RunObs];
    /// How the renderer draws candidates.
    fn render_model(&self) -> RenderModel;
    /// The renderer floor.
    fn floor(&self) -> Floor;
    /// Whether edge `e`'s windows share the per-edge floor offset (it runs within a lattice
    /// step of an axis or a diagonal).
    fn edge_on_lattice(&self, e: usize) -> bool;
    /// `χ²` of edge `e`'s run windows `range` against a candidate curve (pieces in order, in
    /// px, drawn as the renderer draws them). Every window of the range must be crossed by
    /// the curve (a dynamic program charges a segment only the windows wholly its own).
    fn chi2_run(&self, e: usize, range: Range<usize>, curve: &[Piece]) -> Chi2 {
        let rm = self.render_model();
        let drawn = rm.as_rendered(curve);
        let obs = &self.runs(e)[range];
        let mut out = Chi2::default();
        let (mut sr, mut sw) = (0.0, 0.0);
        for o in obs {
            let r = o.sum - left_area(&drawn, &o.window);
            let wv = 1.0 / o.var.max(1e-300);
            out.chi2 += r * r * wv;
            sr += r * wv;
            sw += wv;
            out.m += 1;
        }
        let b2 = if self.edge_on_lattice(e) {
            self.floor().edge_var
        } else {
            0.0
        };
        out.chi2_floor = out.chi2 - b2 * sr * sr / (1.0 + b2 * sw);
        out
    }
    /// The standardised residuals `(S_W − A_W)/√V_W` of the windows `range`.
    fn residuals_run(&self, e: usize, range: Range<usize>, curve: &[Piece]) -> Vec<f64> {
        let drawn = self.render_model().as_rendered(curve);
        self.runs(e)[range]
            .iter()
            .map(|o| (o.sum - left_area(&drawn, &o.window)) / o.var.max(1e-300).sqrt())
            .collect()
    }
    /// Prefix moments of edge `e` for polynomial graphs of `degree` (≤ 3).
    fn run_moments(&self, e: usize, degree: usize) -> RunMoments {
        RunMoments::new(self.runs(e), degree)
    }
    /// `χ²` of a vertex neighbourhood's per-pixel terms, given each incident edge's candidate
    /// curve near it (oriented as the edge).
    fn chi2_local(&self, owner: Owner, curves: &[(usize, &[Piece])]) -> Chi2;
    /// Junction reports (A4).
    fn junctions(&self) -> &[JunctionReport];
    /// Corner proposals along runs (B3.3).
    fn corners(&self) -> &[CornerProposal];
    /// Information density along edge `e`: `(s, ρ)` per run window, `ρ = ℓ_W / V_W` with the
    /// window's boundary length from the starting geometry, px⁻³ (B2.5).
    fn density(&self, e: usize) -> Vec<(f64, f64)>;
}

#[cfg(test)]
#[path = "likelihood_tests.rs"]
mod tests;
