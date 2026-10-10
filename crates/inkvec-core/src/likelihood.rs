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

impl Piece {
    /// The point at parameter `t ∈ [0, 1]`.
    pub fn point(&self, t: f64) -> Point {
        match *self {
            Piece::Line([a, b]) => Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y)),
            Piece::Quad([a, b, c]) => {
                let m = 1.0 - t;
                Point::new(
                    m * m * a.x + 2.0 * m * t * b.x + t * t * c.x,
                    m * m * a.y + 2.0 * m * t * b.y + t * t * c.y,
                )
            }
            Piece::Cubic([a, b, c, d]) => {
                let m = 1.0 - t;
                let (k0, k1, k2, k3) = (m * m * m, 3.0 * m * m * t, 3.0 * m * t * t, t * t * t);
                Point::new(
                    k0 * a.x + k1 * b.x + k2 * c.x + k3 * d.x,
                    k0 * a.y + k1 * b.y + k2 * c.y + k3 * d.y,
                )
            }
            Piece::Arc(arc) => arc.at(arc.start_angle + t * arc.sweep_angle),
        }
    }

    /// An SVG elliptical arc in endpoint form (`A rx ry phi large-arc sweep x y` from
    /// `start`), converted to centre form as SVG's implementation notes (F.6.5) do, with
    /// out-of-range radii scaled up (F.6.6); a line where the arc degenerates. `phi` in
    /// radians; `sweep` true runs towards increasing angle (clockwise on a y-down screen).
    pub fn from_svg_arc(
        start: Point,
        rx: f64,
        ry: f64,
        phi: f64,
        large_arc: bool,
        sweep: bool,
        end: Point,
    ) -> Piece {
        let (mut rx, mut ry) = (rx.abs(), ry.abs());
        if start.dist(end) < 1e-12 || rx < 1e-12 || ry < 1e-12 {
            return Piece::Line([start, end]);
        }
        let (sp, cp) = phi.sin_cos();
        let (hx, hy) = ((start.x - end.x) / 2.0, (start.y - end.y) / 2.0);
        let (x1, y1) = (cp * hx + sp * hy, -sp * hx + cp * hy);
        let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
        if lambda > 1.0 {
            let k = lambda.sqrt();
            rx *= k;
            ry *= k;
        }
        let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
        let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
        let mut coef = (num.max(0.0) / den.max(1e-300)).sqrt();
        if large_arc == sweep {
            coef = -coef;
        }
        let (cx1, cy1) = (coef * rx * y1 / ry, -coef * ry * x1 / rx);
        let centre = Point::new(
            cp * cx1 - sp * cy1 + (start.x + end.x) / 2.0,
            sp * cx1 + cp * cy1 + (start.y + end.y) / 2.0,
        );
        let angle = |ux: f64, uy: f64, vx: f64, vy: f64| -> f64 {
            (ux * vy - uy * vx).atan2(ux * vx + uy * vy)
        };
        let (ux, uy) = ((x1 - cx1) / rx, (y1 - cy1) / ry);
        let (vx, vy) = ((-x1 - cx1) / rx, (-y1 - cy1) / ry);
        let start_angle = angle(1.0, 0.0, ux, uy);
        let mut sweep_angle = angle(ux, uy, vx, vy);
        if !sweep && sweep_angle > 0.0 {
            sweep_angle -= 2.0 * PI;
        } else if sweep && sweep_angle < 0.0 {
            sweep_angle += 2.0 * PI;
        }
        Piece::Arc(EllipticalArc {
            centre,
            radii: (rx, ry),
            x_rotation: phi,
            start_angle,
            sweep_angle,
        })
    }
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
    /// Draw arcs as usvg's cubics at this tolerance (px: 0.1 times the pixels per user unit
    /// of the artist's file), as resvg does; `None` (the default) draws them exactly. The
    /// engine's model is the exact geometry, with whatever a renderer adds left to the
    /// intake's calibrated floor; the cubics remain for measuring a known renderer.
    pub arc_tolerance_px: Option<f64>,
    /// The intake's blur, as its second moment along a window's strip, px² (0 on a native
    /// render): a window's sum is the face's area profile convolved with the blur, which
    /// moves it by `½ μ₂ A''` on a curve (`chain-boundary.md`, "Blur").
    pub psf_mu2: f64,
}

impl Default for RenderModel {
    /// Exact geometry on a native render.
    fn default() -> Self {
        Self {
            arc_tolerance_px: None,
            psf_mu2: 0.0,
        }
    }
}

impl RenderModel {
    /// usvg's model: arcs as its cubics at 0.1 user units, one user unit per pixel.
    pub fn usvg() -> Self {
        Self {
            arc_tolerance_px: Some(0.1),
            psf_mu2: 0.0,
        }
    }

    /// The pieces as the renderer draws them: with an arc tolerance, every arc replaced by
    /// usvg's cubics; the rest unchanged.
    pub fn as_rendered(&self, pieces: &[Piece]) -> Vec<Piece> {
        let Some(tol) = self.arc_tolerance_px else {
            return pieces.to_vec();
        };
        let mut out = Vec::with_capacity(pieces.len());
        for p in pieces {
            match p {
                Piece::Arc(a) => out.extend(a.to_cubics(tol)),
                other => out.push(*other),
            }
        }
        out
    }

    /// The area the drawn curve `drawn` gives its left face in window `w`, as this intake
    /// measures it: [`left_area`], plus `½ μ₂ A''` under a blur, `A''` the second difference
    /// of the window's area profile across its strip (the curve moved one pixel either way).
    pub fn window_area(&self, drawn: &[Piece], w: &Window) -> f64 {
        let a = left_area(drawn, w);
        if self.psf_mu2 <= 0.0 {
            return a;
        }
        let (dx, dy) = match w.axis {
            Axis::Column => (1.0, 0.0),
            Axis::Row => (0.0, 1.0),
        };
        let plus = left_area(&translated(drawn, -dx, -dy), w);
        let minus = left_area(&translated(drawn, dx, dy), w);
        if !(plus.is_finite() && minus.is_finite()) {
            return a;
        }
        a + 0.5 * self.psf_mu2 * (plus - 2.0 * a + minus)
    }
}

/// `pieces` moved by `(dx, dy)`.
pub fn translated(pieces: &[Piece], dx: f64, dy: f64) -> Vec<Piece> {
    let m = |p: Point| Point::new(p.x + dx, p.y + dy);
    pieces
        .iter()
        .map(|p| match *p {
            Piece::Line(q) => Piece::Line(q.map(m)),
            Piece::Quad(q) => Piece::Quad(q.map(m)),
            Piece::Cubic(q) => Piece::Cubic(q.map(m)),
            Piece::Arc(a) => Piece::Arc(EllipticalArc {
                centre: m(a.centre),
                ..a
            }),
        })
        .collect()
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
    /// Variance of `sum`: quantisation of the partial pixels and the intake's per-window
    /// floor (not the per-edge offset, [`BoundaryLikelihood::edge_offset_var`]), times the
    /// window's replica count (see `share`).
    pub var: f64,
    /// Whether the left face lies on the window's low side (above a column, west of a row)
    /// on the starting geometry.
    pub left_low: bool,
    /// This window's share of one independent measurement: `1/k` in a stretch of `k`
    /// replicas (neighbouring windows that see the edge at the same sub-pixel phase repeat one
    /// measurement and its error, as on an axis or a diagonal), else 1. `var` is already `k`
    /// times one window's, so that the stretch weighs as one measurement.
    pub share: f64,
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

/// A score: `χ²` and the number of measurements it sums, so that at the truth
/// `E χ² = dof` (`chain-boundary.md` B2.4).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Chi2 {
    /// `Σ r² / V` over the windows, with each window's own variance.
    pub chi2: f64,
    /// Number of windows (constant over any segmentation of a run: the partition).
    pub m: usize,
    /// `χ²` with the per-edge offset as well: the windows of one edge share an unknown offset
    /// of variance [`BoundaryLikelihood::edge_offset_var`] (A3), so this is
    /// `χ² − (b² (Σ r/V)²) / (1 + b² Σ 1/V)`, the Sherman–Morrison form of the covariance
    /// `V + b² 1 1ᵀ` ([`chi2_correlated`] with `ρ = 1`). Equal to `chi2` where it is zero.
    pub chi2_floor: f64,
    /// Independent measurements among the windows: `Σ share` (replicas count once).
    pub dof: f64,
    /// The robust cost: Huber's at [`crate::noise::huber_kappa`] of the intake's tail, summed
    /// over the standardised innovations of `chi2_floor`. Equal to `chi2_floor` while every
    /// innovation is within the threshold (always, on a clean intake's truth).
    pub cost: f64,
}

/// Sum of two scores over disjoint measurements.
impl std::ops::Add for Chi2 {
    type Output = Chi2;

    fn add(self, o: Chi2) -> Chi2 {
        Chi2 {
            chi2: self.chi2 + o.chi2,
            m: self.m + o.m,
            chi2_floor: self.chi2_floor + o.chi2_floor,
            dof: self.dof + o.dof,
            cost: self.cost + o.cost,
        }
    }
}

/// Score residuals `r` (one per window, variances `v`, shares `share`) with an offset of
/// variance `offset_var` shared by all of them and Huber's threshold `kappa`.
pub fn score(r: &[f64], v: &[f64], share: &[f64], offset_var: f64, kappa: f64) -> Chi2 {
    let c = Correlated {
        tau2: offset_var.max(0.0),
        rho: 1.0,
    };
    let cs = vec![c; r.len()];
    let z = innovations(r, v, &cs);
    let chi2_floor: f64 = z.iter().map(|z| z * z).sum();
    Chi2 {
        chi2: r.iter().zip(v).map(|(r, v)| r * r / v.max(1e-300)).sum(),
        m: r.len(),
        chi2_floor,
        dof: share.iter().sum(),
        cost: z.iter().map(|&z| crate::noise::huber(z, kappa)).sum(),
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

/// The correlated part of the renderer floor at one run window: a stationary first-order
/// autoregression along the run, with variance `tau2` (px²) and correlation `rho` with the
/// window before (`rho = 1`: an offset the windows share, A3; `tau2 = 0`: none).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Correlated {
    /// Variance of the correlated part, px².
    pub tau2: f64,
    /// Correlation with the previous window's correlated part, in `[0, 1]`.
    pub rho: f64,
}

/// `−2 log L` of residuals `r` whose errors are independent with variances `v` plus a
/// correlated part `c` (per window), split into its quadratic part `rᵀ Σ⁻¹ r` and
/// `log det Σ`: a scalar Kalman filter along the run, `O(n)`. With `rho = 1` and one `tau2`
/// this is the Sherman–Morrison form of `V + τ² 1 1ᵀ`; with `tau2 = 0`, `Σ r²/v`.
///
/// The correlated part follows `g_i = ρ_i (τ_i/τ_{i−1}) g_{i−1} + η_i`, so that its variance
/// is `τ_i²` at every window and a class change along the run (a line meeting a curve) needs
/// no special case.
pub fn chi2_correlated(r: &[f64], v: &[f64], c: &[Correlated]) -> (f64, f64) {
    let (mut chi2, mut logdet) = (0.0, 0.0);
    filter(r, v, c, |nu, f| {
        chi2 += nu * nu / f;
        logdet += f.ln();
    });
    (chi2, logdet)
}

/// The standardised innovations `νᵢ/√Fᵢ` of [`chi2_correlated`]'s filter: independent unit
/// normals at the truth, whatever the correlation, so a robust cost applies to them one by
/// one.
pub fn innovations(r: &[f64], v: &[f64], c: &[Correlated]) -> Vec<f64> {
    let mut out = Vec::with_capacity(r.len());
    filter(r, v, c, |nu, f| out.push(nu / f.sqrt()));
    out
}

/// The scalar Kalman filter of [`chi2_correlated`], calling `each(νᵢ, Fᵢ)` per window.
fn filter(r: &[f64], v: &[f64], c: &[Correlated], mut each: impl FnMut(f64, f64)) {
    // State: the correlated part's mean and variance given the windows so far.
    let (mut m, mut p) = (0.0f64, 0.0f64);
    let mut tau_prev = 0.0f64;
    for i in 0..r.len() {
        let ci = c.get(i).copied().unwrap_or_default();
        let tau = ci.tau2.max(0.0).sqrt();
        if i == 0 || tau_prev == 0.0 {
            m = 0.0;
            p = tau * tau;
        } else {
            let a = ci.rho.clamp(0.0, 1.0) * tau / tau_prev;
            m *= a;
            p = a * a * p + tau * tau * (1.0 - ci.rho.clamp(0.0, 1.0).powi(2));
        }
        let f = (p + v[i]).max(1e-300);
        let nu = r[i] - m;
        each(nu, f);
        let k = p / f;
        m += k * nu;
        p *= 1.0 - k;
        tau_prev = tau;
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
                dof: r.len() as f64,
                cost: chi2,
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
    /// Variance of the offset all of edge `e`'s windows share, px² (A3): by default the
    /// floor's `edge_var` on an edge along the lattice, else 0.
    fn edge_offset_var(&self, e: usize) -> f64 {
        if self.edge_on_lattice(e) {
            self.floor().edge_var
        } else {
            0.0
        }
    }
    /// Student-t degrees of freedom of the intake's window errors (`f64::INFINITY`:
    /// Gaussian), which set the robust cost's threshold.
    fn tail_nu(&self) -> f64 {
        f64::INFINITY
    }
    /// The score of edge `e`'s run windows `range` against a candidate curve (pieces in
    /// order, in px, drawn as the renderer draws them, oriented as the edge). Each window is
    /// scored with the candidate's area there, so a span's piece must reach past the span's
    /// ends to cover every window of `range`.
    fn chi2_run(&self, e: usize, range: Range<usize>, curve: &[Piece]) -> Chi2 {
        let rm = self.render_model();
        let drawn = rm.as_rendered(curve);
        let obs = &self.runs(e)[range];
        let r: Vec<f64> = obs
            .iter()
            .map(|o| o.sum - rm.window_area(&drawn, &o.window))
            .collect();
        let v: Vec<f64> = obs.iter().map(|o| o.var.max(1e-300)).collect();
        let share: Vec<f64> = obs.iter().map(|o| o.share).collect();
        score(
            &r,
            &v,
            &share,
            self.edge_offset_var(e),
            crate::noise::huber_kappa(self.tail_nu()),
        )
    }
    /// The standardised residuals `(S_W − A_W)/√V_W` of the windows `range`.
    fn residuals_run(&self, e: usize, range: Range<usize>, curve: &[Piece]) -> Vec<f64> {
        let rm = self.render_model();
        let drawn = rm.as_rendered(curve);
        self.runs(e)[range]
            .iter()
            .map(|o| (o.sum - rm.window_area(&drawn, &o.window)) / o.var.max(1e-300).sqrt())
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
    /// Per run window of edge `e`, where its centre falls along the edge's starting polyline
    /// (`map.edges[e].points`), as a fractional point index in `[0, n)`: non-decreasing along
    /// the run. By default, none (an evaluator that cannot say).
    fn window_point_index(&self, _e: usize) -> Vec<f64> {
        Vec::new()
    }
    /// A2: the score of a stroke band on the thin-feature pixels edges `pair` share (both
    /// reach them), the band's inks given as `[left of it, the band, right of it]` relative to
    /// its centreline's direction; each pixel's colour is its three areas' mix. By default
    /// nothing is scored.
    fn chi2_band(&self, _pair: (usize, usize), _band: &StrokeBand, _inks: [[f32; 3]; 3]) -> Chi2 {
        Chi2::default()
    }
    /// A2: the score of a junction's pixels under per-layer compositing: `layers` in paint
    /// order, each a closed boundary (the layer's whole shape near the junction, not only its
    /// visible part) with its colour, drawn over `ground`. Where shapes overlap this is what a
    /// renderer computes, which the visible partition of [`chi2_local`] is not, by up to
    /// 0.04 colour units at a junction (`renderer_floor.py`). By default nothing is scored.
    ///
    /// [`chi2_local`]: BoundaryLikelihood::chi2_local
    fn chi2_local_layers(
        &self,
        _node: u32,
        _layers: &[(&[Piece], [f32; 3])],
        _ground: [f32; 3],
    ) -> Chi2 {
        Chi2::default()
    }
}

/// A stroke: a centreline drawn with a width, its sides half the width either way (caps and
/// joins are the representation chain's to add). A2's thin features: the pixels its two
/// edges share are scored against it whole ([`BoundaryLikelihood::chi2_band`]).
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeBand {
    /// The centreline, in order.
    pub centre: Vec<Piece>,
    /// The stroke's width, px.
    pub width: f64,
}

impl StrokeBand {
    /// The band's left and right sides (left of the centreline's direction), the centreline
    /// sampled every `step` px or so and offset along its normal.
    pub fn sides(&self, step: f64) -> (Vec<Point>, Vec<Point>) {
        let mut pts: Vec<Point> = Vec::new();
        for p in &self.centre {
            let (a, b) = (p.point(0.0), p.point(1.0));
            let n = ((a.dist(b) + 1.0) / step.max(1e-3))
                .ceil()
                .clamp(1.0, 4096.0) as usize;
            for k in 0..=n {
                if k == 0 && !pts.is_empty() {
                    continue;
                }
                pts.push(p.point(k as f64 / n as f64));
            }
        }
        let h = 0.5 * self.width;
        let m = pts.len();
        let (mut left, mut right) = (Vec::with_capacity(m), Vec::with_capacity(m));
        for i in 0..m {
            let (a, b) = (pts[i.saturating_sub(1)], pts[(i + 1).min(m - 1)]);
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let l = dx.hypot(dy).max(1e-12);
            // Left of direction d is (d.y, −d.x).
            let (nx, ny) = (dy / l, -dx / l);
            left.push(Point::new(pts[i].x + h * nx, pts[i].y + h * ny));
            right.push(Point::new(pts[i].x - h * nx, pts[i].y - h * ny));
        }
        (left, right)
    }
}

/// One edge's windows as the representation chain's dynamic program reads them
/// (`chain-boundary.md`, "The adapter"). The windows are **partitioned** among any
/// segmentation of the edge's points: a window belongs to the span `[i, j)` of point indices
/// that holds its centre, so the windows scored, and their number, do not depend on where
/// the breakpoints fall (the corner and junction terms are fixed per edge in the same way).
/// A span's piece is scored on all of its windows, so it must reach past the span's ends far
/// enough to cross them.
pub struct EdgeScorer<'a, L: BoundaryLikelihood + ?Sized> {
    lik: &'a L,
    edge: usize,
    /// Fractional point index of each window's centre.
    at: Vec<f64>,
    /// Number of points of the edge's polyline (for the wrap of a closed edge).
    n_points: usize,
    /// Least-squares graphs of degree 1, 2, 3 over any range.
    moments: [RunMoments; 3],
    /// Prefix sums of `Σ w u^p`, `p = 0..6`, `w = 1/var`, `u` the line less the first
    /// window's.
    weights: Vec<[f64; 7]>,
}

impl<'a, L: BoundaryLikelihood + ?Sized> EdgeScorer<'a, L> {
    /// The scorer of edge `edge`, whose starting polyline has `n_points` points.
    pub fn new(lik: &'a L, edge: usize, n_points: usize) -> Self {
        let obs = lik.runs(edge);
        let mut at = lik.window_point_index(edge);
        if at.len() != obs.len() {
            at = vec![0.0; obs.len()];
        }
        let u_ref = obs.first().map_or(0.0, |o| o.window.line as f64);
        let mut weights = Vec::with_capacity(obs.len() + 1);
        let mut acc = [0.0f64; 7];
        weights.push(acc);
        for o in obs {
            let w = 1.0 / o.var.max(1e-300);
            let u = o.window.line as f64 - u_ref;
            let mut up = 1.0;
            for a in acc.iter_mut() {
                *a += w * up;
                up *= u;
            }
            weights.push(acc);
        }
        EdgeScorer {
            lik,
            edge,
            at,
            n_points,
            moments: [
                RunMoments::new(obs, 1),
                RunMoments::new(obs, 2),
                RunMoments::new(obs, 3),
            ],
            weights,
        }
    }

    /// The run windows whose centres lie in the span of point indices `[i, j)`; on a closed
    /// edge a span with `j ≤ i` runs through the seam, and its windows come as two ranges
    /// (the second empty otherwise).
    pub fn windows_between_points(&self, i: usize, j: usize) -> (Range<usize>, Range<usize>) {
        let pos = |p: usize| self.at.partition_point(|&t| t < p as f64);
        if j > i {
            (pos(i)..pos(j), 0..0)
        } else {
            (pos(i)..self.at.len(), 0..pos(j.min(self.n_points)))
        }
    }

    /// The score of a candidate on windows `r` (pieces in px, oriented as the edge).
    pub fn chi2(&self, r: Range<usize>, pieces: &[Piece]) -> Chi2 {
        self.lik.chi2_run(self.edge, r, pieces)
    }

    /// The least-squares polynomial graph of `degree` (1 to 3) over windows `r`, in the
    /// windows' strip frame (`mean_position = Σ θ_k · mean over the strip of (u − u_ref)^k`,
    /// `u_ref` the first window's line of the edge), with its `χ²`, in `O(1)`; `None` across
    /// an axis switch or with fewer windows than coefficients.
    pub fn best_graph(&self, r: Range<usize>, degree: usize) -> Option<(Vec<f64>, Chi2)> {
        let m = &self.moments[degree.clamp(1, 3) - 1];
        m.fit(r)
    }

    /// `Σ w u^p` over windows `r`, `p = 0..6`, with `w = 1/var` (the window's whole variance,
    /// replicas deflated) and `u` its line less the edge's first window's: the moments a
    /// degree ≤ 3 graph's Fisher matrix is built from, in `O(1)`. Meaningful on a range of one
    /// axis.
    pub fn weight_moments(&self, r: Range<usize>) -> [f64; 7] {
        let (a, b) = (&self.weights[r.end], &self.weights[r.start]);
        std::array::from_fn(|k| a[k] - b[k])
    }
}

#[cfg(test)]
#[path = "likelihood_tests.rs"]
mod tests;
