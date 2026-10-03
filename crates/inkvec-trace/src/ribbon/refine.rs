//! The stroke solve: centrelines and width moved together to explain the face's measured
//! boundary, by damped Gauss-Newton (Levenberg-Marquardt).
//!
//! **Why.** The centrelines come out of the curve fitter fitted to *centre samples*, the
//! midpoints of paired sides. Away from the sleeves -- at a corner whose inner side is a
//! sharp notch, at a junction, at a cap -- there are no samples, and the fitter bridges
//! the gap with whatever segment its dynamic program chose. The strokes are then judged
//! on the *boundary* (see [`super::score`]), where those bridges show as residuals of a
//! few tenths of a pixel, and that is enough to lose the description-length comparison
//! against the outline, which was solved against the same boundary. So the strokes are
//! solved against it too.
//!
//! **The problem.** Unknowns `θ`: every centreline's control points (a path's start, each
//! line's end, each cubic's two handles and end, each circular arc's end and radius; a
//! closed path's last end is its start), each circle primitive's centre and radius, and
//! the half-width `h`. Elliptical arcs keep their radii and rotation. The other
//! primitives (ellipse, rounded rectangle) are held fixed. Minimise
//!
//! `E(θ) = Σ_p ((d(p, C(θ)) - h)/σ_p)² + Σ_j ((θ_j - θ_j⁰)/σ_a)²`,
//!
//! the first sum over every measured boundary point (`d` the distance to the nearest
//! centreline -- under miter joins, the miter gauge where the nearest point is a vertex,
//! see [`super::join`] -- and `σ_p` its sigma floored at 1e-3 px), the second a weak anchor
//! (`σ_a` = [`ANCHOR`] px) that only matters for a variable no boundary point sees -- a
//! junction end buried inside the stroke it meets -- and keeps the normal matrix
//! non-singular.
//!
//! **The step.** Each point's residual depends on the few variables of the one segment
//! nearest to it; its derivatives are central differences of that segment's exact point
//! distance (step 1e-5 px). By the envelope theorem the foot point's own movement adds
//! nothing to first order, so the differences measure `-n·∂C/∂θ` at the foot, `n` the unit
//! vector from the foot to the point. The normal equations `(JᵀJ + μ·diag(JᵀJ)) δ = -Jᵀr`
//! are dense (a face has tens to a few hundred variables) and solved by Cholesky. A step
//! that lowers `E` and leaves every cubic free of cusps ([`regular`]) is taken and `μ`
//! shrinks by 3; one that does not is retried with `μ` four times larger. Stops after [`MAX_ITERS`] iterations, or when `E` falls by under
//! 1e-5 relatively, or when no step in [`MAX_RETRIES`] damping increases helps.
//!
//! Method from: Levenberg (1944), A method for the solution of certain non-linear problems
//! in least squares, Quarterly of Applied Mathematics 2(2), doi:10.1090/qam/10666; and
//! Marquardt (1963), An algorithm for least-squares estimation of nonlinear parameters,
//! J. SIAM 11(2), doi:10.1137/0111030 (the diagonal scaling `μ·diag(JᵀJ)` is Marquardt's).
//! Inspired by: Yang, Chao, Zhang, Guo, Yuan, Sun (2016), Effective clipart image
//! vectorization through direct optimization of bezigons, IEEE TVCG 22(2),
//! doi:10.1109/TVCG.2015.2440273, which optimises the final curves against the raster;
//! ours optimises stroke centrelines against the already-solved outline instead of
//! against pixel coverage, which keeps the problem a small least-squares fit. And the
//! line-width model of Steger (1998), An unbiased detector of curvilinear structures,
//! IEEE TPAMI 20(2), doi:10.1109/34.659930, which recovers a line's centre and width
//! jointly rather than from two independent edges, as `h` is solved jointly here.

use inkvec_core::Point;
use inkvec_fit::curves::{arc_ellipse_center, Segment};
use inkvec_fit::primitives::{PrimitiveFit, PrimitiveKind};
use inkvec_fit::FittedPath;

use super::boundary::Boundary;
use super::grid::{point_segment, SegGrid};
use super::join::{miter_gauge, tangents, Join};
use super::score::flatten;
use super::Centreline;

/// Iterations of the solve.
const MAX_ITERS: usize = 30;

/// Damping increases tried before an iteration gives up.
const MAX_RETRIES: usize = 8;

/// Sigma of the anchor holding each variable near its start, px. Wide enough never to
/// hold a variable the boundary sees (those are pinned to hundredths of a pixel by
/// hundreds of points), narrow enough to keep a variable it does not see in place.
const ANCHOR: f64 = 2.0;

/// Sigma of the half-width's anchor, as a fraction of the paired half-width it starts
/// from. The paired width is the median of hundreds of pair distances and lucide's
/// solved widths stay within 0.1% of it; without a tight anchor the solve can shrink a
/// blob read as one thick stroke into a thin stroke hugging its outline, which fits every
/// boundary point and paints none of the interior (simple-icons `elixir`: paired 83 px,
/// solved 4.6 px, dE00 0.1 -> 32).
const ANCHOR_HALF: f64 = 0.05;

/// Above this many variables the dense solve is skipped (cubic cost); the strokes keep
/// their fitted geometry. A face of lucide at 128 px has 20-140.
const MAX_VARS: usize = 400;

/// One segment of a solved path: indices into `θ` for its variables.
#[derive(Clone, Copy, Debug)]
enum SegVar {
    /// A line to `θ[end..end+2]`.
    Line {
        /// Index of the end point's x (y follows).
        end: usize,
    },
    /// A cubic with handles at `θ[c1..]`, `θ[c2..]` and end `θ[end..]`.
    Cubic {
        /// First handle's x index.
        c1: usize,
        /// Second handle's x index.
        c2: usize,
        /// End point's x index.
        end: usize,
    },
    /// An arc to `θ[end..]`; a circular one has its radius at `θ[r]`, an elliptical one
    /// keeps `rx`, `ry`, `phi`.
    Arc {
        /// End point's x index.
        end: usize,
        /// Radius index for a circular arc.
        r: Option<usize>,
        /// Fixed x radius (elliptical arcs).
        rx: f64,
        /// Fixed y radius (elliptical arcs).
        ry: f64,
        /// Fixed rotation (elliptical arcs).
        phi: f64,
        /// SVG large-arc flag.
        large: bool,
        /// SVG sweep flag.
        sweep: bool,
    },
}

/// One centreline in the solve.
#[derive(Clone, Debug)]
enum Shape {
    /// A path: its start point's index and its segments.
    Path {
        /// Start point's x index.
        start: usize,
        /// The segments.
        segs: Vec<SegVar>,
        /// Closes on its start.
        closed: bool,
    },
    /// A circle primitive: `θ[at..at+3]` = centre x, y and radius.
    Circle {
        /// Index of the centre's x.
        at: usize,
        /// The primitive as fitted (for its parameter count).
        prim: PrimitiveFit,
    },
    /// Held fixed.
    Fixed(Centreline),
}

/// The variables and the structure that reads them.
struct Model {
    /// All variables; the last is the half-width.
    theta: Vec<f64>,
    /// The centrelines.
    shapes: Vec<Shape>,
    /// How their segments meet.
    join: Join,
}

/// `(θ[i], θ[i+1])` as a point.
fn pt(theta: &[f64], i: usize) -> Point {
    Point::new(theta[i], theta[i + 1])
}

impl Model {
    /// The variables of `lines` and half-width `h`, in reading order, joined by `join`.
    fn new(lines: &[Centreline], h: f64, join: Join) -> Model {
        let mut theta: Vec<f64> = Vec::new();
        let push = |theta: &mut Vec<f64>, p: Point| -> usize {
            theta.push(p.x);
            theta.push(p.y);
            theta.len() - 2
        };
        let mut shapes = Vec::new();
        for line in lines {
            if let Some(prim) = &line.prim {
                if let PrimitiveKind::Circle { c, r } = prim.kind {
                    let at = push(&mut theta, c);
                    theta.push(r);
                    shapes.push(Shape::Circle { at, prim: *prim });
                } else {
                    shapes.push(Shape::Fixed(line.clone()));
                }
                continue;
            }
            let path = &line.path;
            let start = push(&mut theta, path.start);
            let n = path.segments.len();
            let mut segs = Vec::with_capacity(n);
            for (k, s) in path.segments.iter().enumerate() {
                // A closed path's last segment ends where it started: one variable.
                let end_of = |theta: &mut Vec<f64>, p: Point| -> usize {
                    if path.closed && k + 1 == n && p.dist(path.start) < 1e-6 {
                        start
                    } else {
                        push(theta, p)
                    }
                };
                segs.push(match *s {
                    Segment::Line(p) => SegVar::Line {
                        end: end_of(&mut theta, p),
                    },
                    Segment::Cubic(a, b, p) => {
                        let c1 = push(&mut theta, a);
                        let c2 = push(&mut theta, b);
                        SegVar::Cubic {
                            c1,
                            c2,
                            end: end_of(&mut theta, p),
                        }
                    }
                    Segment::Arc {
                        rx,
                        ry,
                        phi,
                        large_arc,
                        sweep,
                        end,
                    } => {
                        let e = end_of(&mut theta, end);
                        let r = s.is_circular().then(|| {
                            theta.push(rx);
                            theta.len() - 1
                        });
                        SegVar::Arc {
                            end: e,
                            r,
                            rx,
                            ry,
                            phi,
                            large: large_arc,
                            sweep,
                        }
                    }
                });
            }
            shapes.push(Shape::Path {
                start,
                segs,
                closed: path.closed,
            });
        }
        theta.push(h);
        Model {
            theta,
            shapes,
            join,
        }
    }

    /// The half-width.
    fn h(&self) -> f64 {
        self.theta[self.theta.len() - 1]
    }

    /// Segment `k` of path shape `s` as geometry: its start point and the segment.
    fn segment(&self, start: usize, segs: &[SegVar], k: usize) -> (Point, Segment) {
        let t = &self.theta;
        let a = match k {
            0 => pt(t, start),
            _ => pt(t, seg_end(&segs[k - 1])),
        };
        let s = match segs[k] {
            SegVar::Line { end } => Segment::Line(pt(t, end)),
            SegVar::Cubic { c1, c2, end } => Segment::Cubic(pt(t, c1), pt(t, c2), pt(t, end)),
            SegVar::Arc {
                end,
                r,
                rx,
                ry,
                phi,
                large,
                sweep,
            } => match r {
                Some(i) => Segment::circular_arc(t[i], large, sweep, pt(t, end)),
                None => Segment::Arc {
                    rx,
                    ry,
                    phi,
                    large_arc: large,
                    sweep,
                    end: pt(t, end),
                },
            },
        };
        (a, s)
    }

    /// Shape `s` as a fitted path (a circle as two half-circle arcs).
    fn path(&self, s: &Shape) -> FittedPath {
        match s {
            Shape::Path {
                start,
                segs,
                closed,
            } => FittedPath {
                start: pt(&self.theta, *start),
                segments: (0..segs.len())
                    .map(|k| self.segment(*start, segs, k).1)
                    .collect(),
                closed: *closed,
            },
            Shape::Circle { at, .. } => circle_path(pt(&self.theta, *at), self.theta[at + 2]),
            Shape::Fixed(c) => c.path.clone(),
        }
    }

    /// The centrelines as they now stand.
    fn centrelines(&self) -> Vec<Centreline> {
        self.shapes
            .iter()
            .map(|s| match s {
                Shape::Fixed(c) => c.clone(),
                Shape::Circle { at, prim } => {
                    let (c, r) = (pt(&self.theta, *at), self.theta[at + 2]);
                    Centreline {
                        path: circle_path(c, r),
                        prim: Some(PrimitiveFit {
                            kind: PrimitiveKind::Circle { c, r },
                            ..*prim
                        }),
                    }
                }
                Shape::Path { .. } => Centreline {
                    path: self.path(s),
                    prim: None,
                },
            })
            .collect()
    }
}

/// The end-point index of a segment's variables.
fn seg_end(s: &SegVar) -> usize {
    match *s {
        SegVar::Line { end } | SegVar::Cubic { end, .. } | SegVar::Arc { end, .. } => end,
    }
}

/// A circle of centre `c` and radius `r` as a closed path of two half-circle arcs.
fn circle_path(c: Point, r: f64) -> FittedPath {
    let (a, b) = (Point::new(c.x + r, c.y), Point::new(c.x - r, c.y));
    FittedPath {
        start: a,
        segments: vec![
            Segment::circular_arc(r, false, true, b),
            Segment::circular_arc(r, false, true, a),
        ],
        closed: true,
    }
}

/// Distance from `p` to segment `s` starting at `a`, and the foot's parameter (for a
/// cubic, the Newton-polished `t` started from `t_hint`; otherwise unused).
///
/// Lines and circular arcs are closed-form: an arc's nearest point is the radial
/// projection when that lies within the sweep, else the nearer endpoint. A cubic's foot
/// solves `(C(t) - p)·C'(t) = 0` by four Newton steps, clamped to `[0, 1]`, against both
/// endpoints. An elliptical arc takes the best of 64 samples refined by golden section.
fn dist_to(p: Point, a: Point, s: &Segment, t_hint: f64) -> (f64, f64) {
    match *s {
        Segment::Line(b) => {
            let (d, u, _) = point_segment(p, a, b);
            (d, u)
        }
        Segment::Cubic(c1, c2, b) => cubic_dist(p, [a, c1, c2, b], t_hint),
        Segment::Arc { .. } if s.is_circular() => (arc_dist(p, a, s), 0.0),
        Segment::Arc { .. } => (ellipse_arc_dist(p, a, s), 0.0),
    }
}

/// Cubic Bézier `q` at `t`, and its first and second derivatives.
fn cubic_eval(q: &[Point; 4], t: f64) -> (Point, Point, Point) {
    let mt = 1.0 - t;
    let p = Point::new(
        mt * mt * mt * q[0].x
            + 3.0 * mt * mt * t * q[1].x
            + 3.0 * mt * t * t * q[2].x
            + t * t * t * q[3].x,
        mt * mt * mt * q[0].y
            + 3.0 * mt * mt * t * q[1].y
            + 3.0 * mt * t * t * q[2].y
            + t * t * t * q[3].y,
    );
    let d1 = |i: usize, j: usize| 3.0 * (q[j].x - q[i].x);
    let d1y = |i: usize, j: usize| 3.0 * (q[j].y - q[i].y);
    let dp = Point::new(
        mt * mt * d1(0, 1) + 2.0 * mt * t * d1(1, 2) + t * t * d1(2, 3),
        mt * mt * d1y(0, 1) + 2.0 * mt * t * d1y(1, 2) + t * t * d1y(2, 3),
    );
    let ddp = Point::new(
        6.0 * (mt * (q[2].x - 2.0 * q[1].x + q[0].x) + t * (q[3].x - 2.0 * q[2].x + q[1].x)),
        6.0 * (mt * (q[2].y - 2.0 * q[1].y + q[0].y) + t * (q[3].y - 2.0 * q[2].y + q[1].y)),
    );
    (p, dp, ddp)
}

/// Distance from `p` to cubic `q` and the foot's `t`: Newton on `(C - p)·C' = 0` from
/// `t_hint`, clamped, compared with both endpoints.
fn cubic_dist(p: Point, q: [Point; 4], t_hint: f64) -> (f64, f64) {
    let mut t = t_hint.clamp(0.0, 1.0);
    for _ in 0..4 {
        let (c, d1, d2) = cubic_eval(&q, t);
        let (ex, ey) = (c.x - p.x, c.y - p.y);
        let f = ex * d1.x + ey * d1.y;
        let fp = d1.x * d1.x + d1.y * d1.y + ex * d2.x + ey * d2.y;
        if fp.abs() < 1e-12 {
            break;
        }
        t = (t - f / fp).clamp(0.0, 1.0);
    }
    let mut best = (cubic_eval(&q, t).0.dist(p), t);
    for (tt, e) in [(0.0, q[0]), (1.0, q[3])] {
        let d = e.dist(p);
        if d < best.0 {
            best = (d, tt);
        }
    }
    best
}

/// Distance from `p` to the circular arc `s` starting at `a`.
fn arc_dist(p: Point, a: Point, s: &Segment) -> f64 {
    let Segment::Arc {
        rx,
        ry,
        phi,
        large_arc,
        sweep,
        end,
    } = *s
    else {
        return f64::INFINITY;
    };
    let f = arc_ellipse_center(a, rx, ry, phi, large_arc, sweep, end);
    let v = p - f.c;
    let rho = v.norm();
    if f.delta == 0.0 || rho < 1e-12 {
        return a.dist(p).min(end.dist(p));
    }
    let ang = v.y.atan2(v.x);
    let tau = std::f64::consts::TAU;
    // How far round the sweep the point's angle lies, in the sweep's own direction.
    let s_ang = if f.delta > 0.0 {
        (ang - f.theta1).rem_euclid(tau)
    } else {
        (f.theta1 - ang).rem_euclid(tau)
    };
    if s_ang <= f.delta.abs() {
        (rho - f.rx).abs()
    } else {
        a.dist(p).min(end.dist(p))
    }
}

/// Distance from `p` to the elliptical arc `s` starting at `a`: best of 64 samples in the
/// sweep, refined by 24 golden-section steps on the bracketing interval.
fn ellipse_arc_dist(p: Point, a: Point, s: &Segment) -> f64 {
    let Segment::Arc {
        rx,
        ry,
        phi,
        large_arc,
        sweep,
        end,
    } = *s
    else {
        return f64::INFINITY;
    };
    let f = arc_ellipse_center(a, rx, ry, phi, large_arc, sweep, end);
    let d = |u: f64| f.at(f.theta1 + f.delta * u).dist(p);
    let n = 64;
    let mut best = (d(0.0), 0usize);
    for i in 1..=n {
        let v = d(i as f64 / n as f64);
        if v < best.0 {
            best = (v, i);
        }
    }
    let (mut lo, mut hi) = (
        best.1.saturating_sub(1) as f64 / n as f64,
        ((best.1 + 1).min(n)) as f64 / n as f64,
    );
    let g = 0.618_033_988_749_895;
    for _ in 0..24 {
        let (m1, m2) = (hi - g * (hi - lo), lo + g * (hi - lo));
        if d(m1) < d(m2) {
            hi = m2;
        } else {
            lo = m1;
        }
    }
    d(0.5 * (lo + hi)).min(best.0)
}

/// Which variable indices a residual on (shape, segment) depends on, for the
/// finite differences: the segment's start point (the previous end, or the path start),
/// its own handles, end and radius, and the same for segment `with` when the residual is
/// a miter gauge that reads that neighbour's tangent; a circle's centre and radius.
fn locals(model: &Model, shape: usize, seg: usize, with: Option<usize>) -> Vec<usize> {
    match &model.shapes[shape] {
        Shape::Path { start, segs, .. } => {
            let mut v = Vec::with_capacity(16);
            for k in std::iter::once(seg).chain(with) {
                let a = if k == 0 {
                    *start
                } else {
                    seg_end(&segs[k - 1])
                };
                v.extend([a, a + 1]);
                match segs[k] {
                    SegVar::Line { end } => v.extend([end, end + 1]),
                    SegVar::Cubic { c1, c2, end } => {
                        v.extend([c1, c1 + 1, c2, c2 + 1, end, end + 1])
                    }
                    SegVar::Arc { end, r, .. } => {
                        v.extend([end, end + 1]);
                        v.extend(r);
                    }
                }
            }
            v.sort_unstable();
            v.dedup();
            v
        }
        Shape::Circle { at, .. } => vec![*at, at + 1, at + 2],
        Shape::Fixed(_) => Vec::new(),
    }
}

/// Where on a segment a point's nearest location lies.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Foot {
    /// At its start point.
    Start,
    /// Strictly inside.
    Inside,
    /// At its end point.
    End,
}

/// The segment that meets segment `seg` of a path with `n` segments at `foot`: the
/// previous one at the start, the next at the end, wrapping round a closed path; `None`
/// at an open path's ends (caps) and inside a segment.
fn neighbour(n: usize, closed: bool, seg: usize, foot: Foot) -> Option<usize> {
    match foot {
        Foot::Start if seg > 0 => Some(seg - 1),
        Foot::Start if closed => Some(n - 1),
        Foot::End if seg + 1 < n => Some(seg + 1),
        Foot::End if closed => Some(0),
        _ => None,
    }
}

/// The residual's distance term for point `p` on segment `seg` of shape `shape` under
/// the current `θ`, and the neighbouring segment it read, if any.
///
/// Round joins: the exact distance to the segment ([`dist_to`]). Miter joins: the same,
/// unless the nearest point is a vertex where the path turns into another segment; there
/// the [`miter_gauge`] of the two tangents is used. A foot is at an endpoint when the
/// distance equals that endpoint's to 1e-9 px (each distance routine returns the
/// endpoint's own distance when it clamps there).
fn local_eval(
    model: &Model,
    shape: usize,
    seg: usize,
    p: Point,
    t_hint: f64,
) -> (f64, Option<usize>) {
    match &model.shapes[shape] {
        Shape::Path {
            start,
            segs,
            closed,
        } => {
            let (a, s) = model.segment(*start, segs, seg);
            let d = dist_to(p, a, &s, t_hint).0;
            if model.join == Join::Round {
                return (d, None);
            }
            let e = s.end();
            let foot = if (d - p.dist(a)).abs() < 1e-9 {
                Foot::Start
            } else if (d - p.dist(e)).abs() < 1e-9 {
                Foot::End
            } else {
                Foot::Inside
            };
            let Some(k) = neighbour(segs.len(), *closed, seg, foot) else {
                return (d, None);
            };
            let (a2, s2) = model.segment(*start, segs, k);
            let (Some(this), Some(other)) = (tangents(a, &s), tangents(a2, &s2)) else {
                return (d, None);
            };
            let (v, t_in, t_out) = if foot == Foot::Start {
                (a, other.1, this.0)
            } else {
                (e, this.1, other.0)
            };
            (miter_gauge(p, v, t_in, t_out), Some(k))
        }
        Shape::Circle { at, .. } => (
            (pt(&model.theta, *at).dist(p) - model.theta[at + 2]).abs(),
            None,
        ),
        Shape::Fixed(_) => (f64::NAN, None),
    }
}

/// [`local_eval`]'s distance term alone.
fn local_dist(model: &Model, shape: usize, seg: usize, p: Point, t_hint: f64) -> f64 {
    local_eval(model, shape, seg, p, t_hint).0
}

/// Every boundary point's distance term to the centrelines `lines` (half-width `h`,
/// joined by `join`), as the solve measures it; a point with no centreline within
/// `8·h + 4` px gets that reach. The stroke score ([`super::score`]) is computed from
/// these, so the chi-squared that decides the face is the one the solve minimised.
pub(crate) fn distances(lines: &[Centreline], h: f64, b: &Boundary, join: Join) -> Vec<f64> {
    let reach = 8.0 * h + 4.0;
    let (_, rws) = rows(&Model::new(lines, h, join), b, reach);
    rws.iter()
        .map(|r| r.as_ref().map_or(reach, |r| r.d))
        .collect()
}

/// Every flattened piece of every shape, tagged `(shape, segment, t0, t1)`, for the
/// nearest-segment search. Fixed shapes are tagged with segment `usize::MAX`.
#[allow(clippy::type_complexity)]
fn pieces(model: &Model) -> (Vec<(Point, Point)>, Vec<(usize, usize, f64, f64)>) {
    let (mut segs, mut tags) = (Vec::new(), Vec::new());
    for (si, s) in model.shapes.iter().enumerate() {
        match s {
            Shape::Path {
                start, segs: sv, ..
            } => {
                for k in 0..sv.len() {
                    let (a, seg) = model.segment(*start, sv, k);
                    let one = FittedPath {
                        start: a,
                        segments: vec![seg],
                        closed: false,
                    };
                    let before = segs.len();
                    flatten(&one, 0.25, &mut segs);
                    let m = segs.len() - before;
                    for j in 0..m {
                        tags.push((si, k, j as f64 / m as f64, (j + 1) as f64 / m as f64));
                    }
                }
            }
            _ => {
                let before = segs.len();
                flatten(&model.path(s), 0.25, &mut segs);
                let k = if matches!(s, Shape::Circle { .. }) {
                    0
                } else {
                    usize::MAX
                };
                for _ in before..segs.len() {
                    tags.push((si, k, 0.0, 1.0));
                }
            }
        }
    }
    (segs, tags)
}

/// One residual: which shape and segment it belongs to, the foot hint and the data.
struct Row {
    /// Shape index.
    shape: usize,
    /// Segment index (`usize::MAX`: a fixed shape, no variables).
    seg: usize,
    /// Foot parameter hint for a cubic.
    t: f64,
    /// Current distance, px.
    d: f64,
}

/// Each of `pts`'s row under the current model: its nearest segment within `reach` px
/// and the distance term to it ([`local_dist`]), or `None` beyond `reach`.
fn nearest_rows(model: &Model, pts: &[Point], reach: f64) -> Vec<Option<Row>> {
    let (segs, tags) = pieces(model);
    let grid = SegGrid::new(&segs, 2.0);
    pts.iter()
        .map(|&p| {
            let n = grid.nearest(p, reach)?;
            let (shape, seg, t0, t1) = tags[n.seg];
            let t = t0 + n.u * (t1 - t0);
            let d = if seg == usize::MAX {
                n.d
            } else {
                local_dist(model, shape, seg, p, t)
            };
            Some(Row { shape, seg, t, d })
        })
        .collect()
}

/// The data term `Σ ((d_p - h)/σ_p)²` under the current model, and each point's row; a
/// point beyond `reach` contributes `((reach - h)/σ)²`.
fn rows(model: &Model, b: &Boundary, reach: f64) -> (f64, Vec<Option<Row>>) {
    let out = nearest_rows(model, &b.pts, reach);
    let h = model.h();
    let e = out
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let d = r.as_ref().map_or(reach, |r| r.d);
            ((d - h) / b.sigma[i].max(1e-3)).powi(2)
        })
        .sum();
    (e, out)
}

/// The distance term from each of `pts` to the centrelines `lines` (joined by `join`),
/// `f64::INFINITY` beyond `reach` px: for the coverage test ([`super::fit_face`]).
pub(crate) fn point_distances(
    lines: &[Centreline],
    pts: &[Point],
    reach: f64,
    join: Join,
) -> Vec<f64> {
    nearest_rows(&Model::new(lines, 0.0, join), pts, reach)
        .into_iter()
        .map(|r| r.map_or(f64::INFINITY, |r| r.d))
        .collect()
}

/// Solve the strokes `lines` with half-width `h` against boundary `b`; returns the moved
/// centrelines and half-width, or the inputs unchanged when there are more than
/// [`MAX_VARS`] variables or nothing improved. See the module documentation.
pub(crate) fn solve(
    lines: &[Centreline],
    h: f64,
    b: &Boundary,
    join: Join,
) -> (Vec<Centreline>, f64) {
    let mut model = Model::new(lines, h, join);
    let n = model.theta.len();
    if n > MAX_VARS {
        return (lines.to_vec(), h);
    }
    let theta0 = model.theta.clone();
    let reach = 8.0 * h + 4.0;
    let sig = anchor_sigmas(&theta0);
    let anchor = |theta: &[f64]| -> f64 {
        theta
            .iter()
            .zip(&theta0)
            .zip(&sig)
            .map(|((x, y), s)| ((x - y) / s).powi(2))
            .sum()
    };
    let (e0, mut rws) = rows(&model, b, reach);
    let mut energy = e0 + anchor(&model.theta);
    let start_regular = regular(&model);
    let mut mu = 1e-3;
    for _ in 0..MAX_ITERS {
        let (ata, atr) = normal_equations(&mut model, b, &rws, &theta0, &sig);
        let mut improved = false;
        for _ in 0..MAX_RETRIES {
            let mut a = ata.clone();
            for i in 0..n {
                a[i * n + i] += mu * ata[i * n + i].max(1e-9);
            }
            let Some(delta) = cholesky_solve(&mut a, &atr, n) else {
                mu *= 4.0;
                continue;
            };
            let saved = model.theta.clone();
            for (x, d) in model.theta.iter_mut().zip(&delta) {
                *x -= d;
            }
            let (e2, r2) = rows(&model, b, reach);
            let en2 = e2 + anchor(&model.theta);
            // A fit that starts with a cusp (the curve fitter's own) cannot be held to it.
            if en2 < energy && (regular(&model) || !start_regular) {
                let rel = (energy - en2) / energy.max(1e-12);
                energy = en2;
                rws = r2;
                mu = (mu / 3.0).max(1e-9);
                improved = rel > 1e-5;
                break;
            }
            model.theta = saved;
            mu *= 4.0;
        }
        if !improved {
            break;
        }
    }
    (model.centrelines(), model.h())
}

/// Most splits [`solve_adaptive`] tries per face.
const MAX_SPLITS: usize = 12;

/// The stroke solve with structure added where the boundary asks for it.
///
/// The curve fitter chose each centreline's segments from its centre samples, and where a
/// stretch had none (a sharp tip, a tight corner, a junction) it bridged the gap with the
/// cheapest segment that fit the samples it had: often a line, which the solve can move
/// but not bend. So after [`solve`], the segment carrying the most squared residual is
/// split -- a line becomes a cubic on the same chord (+4 parameters), a cubic is cut in
/// two at the foot of its worst point by de Casteljau (+6), an arc is cut in two at that
/// angle (+5) -- the strokes are solved again, and the split is kept only when the face's
/// description length `χ²/2 + λ·k` falls (`λ` = `lambda`, nats per parameter). A refused
/// split is not retried; at most [`MAX_SPLITS`] are tried (`INKVEC_RIBBONS_SPLITS`
/// overrides, 0 to switch it off), and none that would take the strokes to `budget`
/// parameters (what the outline costs, so the face could not win).
///
/// Inspired by: Schneider (1990), An algorithm for automatically fitting digitized curves,
/// in Graphics Gems, Academic Press, pp. 612-626 -- split a curve at its point of maximum
/// error and fit the two halves. Ours splits where the *stroke's painted outline* errs most
/// and accepts the split by description length, not by an error tolerance, so a split
/// that does not pay for its parameters is refused.
pub(crate) fn solve_adaptive(
    lines: &[Centreline],
    h: f64,
    b: &Boundary,
    lambda: f64,
    join: Join,
    budget: f64,
) -> (Vec<Centreline>, f64) {
    let (mut cur, mut h) = solve(lines, h, b, join);
    let max_splits = inkvec_core::env::count("INKVEC_RIBBONS_SPLITS").unwrap_or(MAX_SPLITS);
    let reach = 8.0 * h + 4.0;
    let cost = |ls: &[Centreline], h: f64| -> f64 {
        let (e, _) = rows(&Model::new(ls, h, join), b, reach);
        0.5 * e + lambda * ls.iter().map(Centreline::params).sum::<f64>()
    };
    let mut best = cost(&cur, h);
    let mut refused: Vec<(usize, usize)> = Vec::new();
    for _ in 0..max_splits {
        let Some((shape, seg, t)) = worst_segment(&cur, h, b, reach, &refused, join) else {
            break;
        };
        let Some(trial) = split(&cur, shape, seg, t) else {
            refused.push((shape, seg));
            continue;
        };
        // A split that takes the strokes to the caller's budget cannot win; stop there.
        if trial.iter().map(Centreline::params).sum::<f64>() + 1.0 >= budget {
            break;
        }
        let (tl, th) = solve(&trial, h, b, join);
        let c = cost(&tl, th);
        if c < best && drawable(&tl, join) {
            best = c;
            cur = tl;
            h = th;
            // Segment indices after the split moved; earlier refusals no longer name the
            // same segments.
            refused.clear();
        } else {
            refused.push((shape, seg));
        }
    }
    (cur, h)
}

/// The path segment carrying the most `Σ ((d - h)/σ)²`, with the foot parameter of its
/// worst point (clamped to `[0.15, 0.85]` so neither half is a sliver), skipping the
/// `refused` ones and anything that is not a path segment. `None` when nothing is left.
fn worst_segment(
    lines: &[Centreline],
    h: f64,
    b: &Boundary,
    reach: f64,
    refused: &[(usize, usize)],
    join: Join,
) -> Option<(usize, usize, f64)> {
    let model = Model::new(lines, h, join);
    let (_, rws) = rows(&model, b, reach);
    let mut acc: std::collections::BTreeMap<(usize, usize), (f64, f64, f64)> =
        std::collections::BTreeMap::new();
    for (i, row) in rws.iter().enumerate() {
        let Some(row) = row else { continue };
        if row.seg == usize::MAX || !matches!(model.shapes[row.shape], Shape::Path { .. }) {
            continue;
        }
        let r = ((row.d - h) / b.sigma[i].max(1e-3)).powi(2);
        let e = acc.entry((row.shape, row.seg)).or_insert((0.0, 0.0, 0.5));
        e.0 += r;
        if r > e.1 {
            e.1 = r;
            e.2 = row.t;
        }
    }
    acc.into_iter()
        .filter(|(k, _)| !refused.contains(k))
        .max_by(|x, y| x.1 .0.total_cmp(&y.1 .0))
        .map(|((s, k), (_, _, t))| (s, k, t.clamp(0.15, 0.85)))
}

/// `lines` with segment `seg` of centreline `shape` split at parameter `t` (see
/// [`solve_adaptive`]), or `None` when that segment is not a path segment.
fn split(lines: &[Centreline], shape: usize, seg: usize, t: f64) -> Option<Vec<Centreline>> {
    let line = lines.get(shape)?;
    if line.prim.is_some() {
        return None;
    }
    let path = &line.path;
    let s = path.segments.get(seg)?.clone();
    let a = if seg == 0 {
        path.start
    } else {
        path.segments[seg - 1].end()
    };
    let lerp =
        |p: Point, q: Point, t: f64| Point::new(p.x + t * (q.x - p.x), p.y + t * (q.y - p.y));
    let parts: Vec<Segment> = match s {
        Segment::Line(e) => vec![Segment::Cubic(
            lerp(a, e, 1.0 / 3.0),
            lerp(a, e, 2.0 / 3.0),
            e,
        )],
        Segment::Cubic(c1, c2, e) => {
            let (p01, p12, p23) = (lerp(a, c1, t), lerp(c1, c2, t), lerp(c2, e, t));
            let (p012, p123) = (lerp(p01, p12, t), lerp(p12, p23, t));
            let m = lerp(p012, p123, t);
            vec![Segment::Cubic(p01, p012, m), Segment::Cubic(p123, p23, e)]
        }
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            end,
        } => {
            let f = arc_ellipse_center(a, rx, ry, phi, large_arc, sweep, end);
            let m = f.at(f.theta1 + f.delta * t);
            let pi = std::f64::consts::PI;
            vec![
                Segment::Arc {
                    rx: f.rx,
                    ry: f.ry,
                    phi,
                    large_arc: (f.delta * t).abs() > pi,
                    sweep,
                    end: m,
                },
                Segment::Arc {
                    rx: f.rx,
                    ry: f.ry,
                    phi,
                    large_arc: (f.delta * (1.0 - t)).abs() > pi,
                    sweep,
                    end,
                },
            ]
        }
    };
    let mut out = lines.to_vec();
    let segs = &mut out[shape].path.segments;
    segs.splice(seg..=seg, parts);
    Some(out)
}

/// `JᵀJ` (dense, row-major `n x n`) and `Jᵀr` for the data rows and the anchor, with
/// `J` by central differences of each row's segment distance (see the module docs).
fn normal_equations(
    model: &mut Model,
    b: &Boundary,
    rws: &[Option<Row>],
    theta0: &[f64],
    sig: &[f64],
) -> (Vec<f64>, Vec<f64>) {
    let n = model.theta.len();
    let hi = n - 1;
    let mut ata = vec![0.0; n * n];
    let mut atr = vec![0.0; n];
    let eps = 1e-5;
    for (i, row) in rws.iter().enumerate() {
        let Some(row) = row else { continue };
        let s = b.sigma[i].max(1e-3);
        let r = (row.d - model.h()) / s;
        let mut jac: Vec<(usize, f64)> = vec![(hi, -1.0 / s)];
        if row.seg != usize::MAX {
            let with = local_eval(model, row.shape, row.seg, b.pts[i], row.t).1;
            for v in locals(model, row.shape, row.seg, with) {
                let x = model.theta[v];
                model.theta[v] = x + eps;
                let dp = local_dist(model, row.shape, row.seg, b.pts[i], row.t);
                model.theta[v] = x - eps;
                let dm = local_dist(model, row.shape, row.seg, b.pts[i], row.t);
                model.theta[v] = x;
                jac.push((v, (dp - dm) / (2.0 * eps * s)));
            }
        }
        for &(u, ju) in &jac {
            atr[u] += ju * r;
            for &(v, jv) in &jac {
                ata[u * n + v] += ju * jv;
            }
        }
    }
    for j in 0..n {
        let w = 1.0 / (sig[j] * sig[j]);
        ata[j * n + j] += w;
        atr[j] += w * (model.theta[j] - theta0[j]);
    }
    (ata, atr)
}

/// Shortest segment chord a miter-joined path may have, px.
const MITER_MIN_SEG: f64 = 0.5;

/// Cosine of the sharpest turn allowed at a miter-joined vertex (150°).
const MITER_MAX_TURN_COS: f64 = -0.866;

/// Whether a renderer strokes the model as the residual model assumes.
///
/// * **No cusps** (any join). SVG paints a stroke as the sweep of a line across the path
///   along its normal; where a segment's derivative vanishes the normal is undefined, and
///   a renderer that flattens the curve leaves the half-disc around that point unpainted
///   -- a notch the boundary residual cannot see, because the centreline still passes
///   there (simple-icons `kleinanzeigen`: a solve step turned a cubic's first handle
///   backwards, dE00 +0.13). At 33 samples of `t` on every cubic, the derivative must stay
///   at least 5% of the control polygon's length and turn by under 90° between
///   neighbouring samples.
/// * **No micro-segments or hairpins under miter joins.** A miter is drawn from the two
///   segments' end tangents; on a segment of a fraction of a pixel that reverses
///   direction the renderer draws spikes and notches the miter gauge does not model
///   (material-icons `settings`: two cubics under 0.2 px at one vertex, dE00 +0.31). So
///   every segment's chord must be at least [`MITER_MIN_SEG`] px and no vertex may turn
///   by more than 150° ([`MITER_MAX_TURN_COS`]). Round joins draw a disc at every vertex
///   whatever the tangents, and need neither.
///
/// Not from the literature: a sampled hodograph test and two drawing rules, because the
/// solve needs a cheap yes/no on every candidate step rather than an exact cusp locus.
fn regular(model: &Model) -> bool {
    for shape in &model.shapes {
        let Shape::Path {
            start,
            segs,
            closed,
        } = shape
        else {
            continue;
        };
        if model.join == Join::Miter && !miter_drawable(model, *start, segs, *closed) {
            return false;
        }
        for k in 0..segs.len() {
            let (a, seg) = model.segment(*start, segs, k);
            let Segment::Cubic(c1, c2, b) = seg else {
                continue;
            };
            let q = [a, c1, c2, b];
            let l = a.dist(c1) + c1.dist(c2) + c2.dist(b);
            if l < 1e-9 {
                continue;
            }
            let mut prev: Option<Point> = None;
            for i in 0..=32 {
                let (_, d, _) = cubic_eval(&q, i as f64 / 32.0);
                if d.x.hypot(d.y) < 0.05 * l {
                    return false;
                }
                if prev.is_some_and(|p| p.x * d.x + p.y * d.y <= 0.0) {
                    return false;
                }
                prev = Some(d);
            }
        }
    }
    true
}

/// The miter rules of [`regular`] for one path: every chord at least [`MITER_MIN_SEG`]
/// px, and every vertex (including a closed path's start) turning by at most 150°.
fn miter_drawable(model: &Model, start: usize, segs: &[SegVar], closed: bool) -> bool {
    let n = segs.len();
    let mut tans = Vec::with_capacity(n);
    for k in 0..n {
        let (a, seg) = model.segment(start, segs, k);
        if a.dist(seg.end()) < MITER_MIN_SEG {
            return false;
        }
        match tangents(a, &seg) {
            Some(t) => tans.push(t),
            None => return false,
        }
    }
    let joins = if closed { n } else { n.saturating_sub(1) };
    (0..joins).all(|k| {
        let next = if k + 1 == n { 0 } else { k + 1 };
        tans[k].1.dot(tans[next].0) >= MITER_MAX_TURN_COS
    })
}

/// Whether the centrelines `lines`, joined by `join`, are drawn by a renderer as the
/// residual model assumes ([`regular`]). A hypothesis whose solved strokes are not is
/// refused ([`super::fit_face`]).
pub(crate) fn drawable(lines: &[Centreline], join: Join) -> bool {
    regular(&Model::new(lines, 0.0, join))
}

/// Each variable's anchor sigma: [`ANCHOR`] px, and for the half-width (the last
/// variable) [`ANCHOR_HALF`] of its starting value (at least 0.01 px).
fn anchor_sigmas(theta0: &[f64]) -> Vec<f64> {
    let mut s = vec![ANCHOR; theta0.len()];
    if let Some(h) = theta0.last() {
        let n = s.len();
        s[n - 1] = (ANCHOR_HALF * h).max(0.01);
    }
    s
}

/// Solve `A x = y` for symmetric positive definite `A` (row-major `n x n`, overwritten by
/// its Cholesky factor). `None` when a pivot is not positive.
fn cholesky_solve(a: &mut [f64], y: &[f64], n: usize) -> Option<Vec<f64>> {
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d -= a[j * n + k] * a[j * n + k];
        }
        if d <= 1e-300 {
            return None;
        }
        let d = d.sqrt();
        a[j * n + j] = d;
        for i in j + 1..n {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= a[i * n + k] * a[j * n + k];
            }
            a[i * n + j] = s / d;
        }
    }
    let mut z = y.to_vec();
    for i in 0..n {
        let mut s = z[i];
        for k in 0..i {
            s -= a[i * n + k] * z[k];
        }
        z[i] = s / a[i * n + i];
    }
    for i in (0..n).rev() {
        let mut s = z[i];
        for k in i + 1..n {
            s -= a[k * n + i] * z[k];
        }
        z[i] = s / a[i * n + i];
    }
    Some(z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use inkvec_core::Polyline;

    #[test]
    fn cholesky_solves_a_small_system() {
        let mut a = vec![4.0, 2.0, 2.0, 3.0];
        let x = cholesky_solve(&mut a, &[2.0, 1.0], 2).expect("positive definite");
        assert!((4.0 * x[0] + 2.0 * x[1] - 2.0).abs() < 1e-12);
        assert!((2.0 * x[0] + 3.0 * x[1] - 1.0).abs() < 1e-12);
        let mut bad = vec![0.0, 0.0, 0.0, 1.0];
        assert!(cholesky_solve(&mut bad, &[1.0, 1.0], 2).is_none());
    }

    #[test]
    fn arc_and_cubic_distances_are_exact() {
        // Upper half circle of radius 5 round the origin, from (5,0) to (-5,0).
        let s = Segment::circular_arc(5.0, false, false, Point::new(-5.0, 0.0));
        let a = Point::new(5.0, 0.0);
        let d_in =
            arc_dist(Point::new(0.0, -7.0), a, &s).min(arc_dist(Point::new(0.0, 7.0), a, &s));
        assert!((d_in - 2.0).abs() < 1e-9, "{d_in}");
        // A straight cubic along x: distance is the height.
        let q = [
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(2.0, 0.0),
            Point::new(3.0, 0.0),
        ];
        let (d, t) = cubic_dist(Point::new(1.5, 2.0), q, 0.3);
        assert!((d - 2.0).abs() < 1e-9 && (t - 0.5).abs() < 1e-6, "{d} {t}");
    }

    #[test]
    fn the_solve_pulls_an_offset_centreline_onto_the_stroke() {
        // A straight stroke from (0,0) to (30,0), half-width 3, as its outline.
        let mut pts = Vec::new();
        for i in 0..=120 {
            pts.push(Point::new(i as f64 * 0.25, -3.0));
        }
        for j in 1..40 {
            let t = -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * j as f64 / 40.0;
            pts.push(Point::new(30.0 + 3.0 * t.cos(), 3.0 * t.sin()));
        }
        for i in 0..=120 {
            pts.push(Point::new(30.0 - i as f64 * 0.25, 3.0));
        }
        for j in 1..40 {
            let t = std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * j as f64 / 40.0;
            pts.push(Point::new(3.0 * t.cos(), 3.0 * t.sin()));
        }
        let ring = Polyline::with_uniform_sigma(pts, 0.05, true);
        let inside = |p: Point| Point::new(p.x.clamp(0.0, 30.0), 0.0).dist(p) < 3.0;
        let b = Boundary::new(&[ring], &inside, 2.0).expect("a ring");
        // Start 0.4 px off and 0.5 px short, half-width 2.8.
        let line = Centreline {
            path: FittedPath {
                start: Point::new(0.3, 0.4),
                segments: vec![Segment::Line(Point::new(29.5, 0.4))],
                closed: false,
            },
            prim: None,
        };
        let (out, h) = solve(&[line], 2.8, &b, Join::Round);
        let p = &out[0].path;
        assert!((h - 3.0).abs() < 1e-3, "h {h}");
        assert!(p.start.dist(Point::new(0.0, 0.0)) < 0.01, "{:?}", p.start);
        assert!(p.end().dist(Point::new(30.0, 0.0)) < 0.01, "{:?}", p.end());
    }

    #[test]
    fn a_cusp_is_irregular_and_a_plain_cubic_is_not() {
        let cubic = |c1: Point, c2: Point| Centreline {
            path: FittedPath {
                start: Point::new(0.0, 0.0),
                segments: vec![Segment::Cubic(c1, c2, Point::new(30.0, 0.0))],
                closed: false,
            },
            prim: None,
        };
        let plain = cubic(Point::new(10.0, 5.0), Point::new(20.0, 5.0));
        assert!(regular(&Model::new(&[plain], 2.0, Join::Round)));
        // The first handle points backwards: the derivative reverses near t = 0.
        let cusp = cubic(Point::new(-3.0, 0.0), Point::new(25.0, 0.0));
        assert!(!regular(&Model::new(&[cusp], 2.0, Join::Round)));
    }

    #[test]
    fn splits_keep_the_curve() {
        let a = Point::new(0.0, 0.0);
        let path = FittedPath {
            start: a,
            segments: vec![
                Segment::Line(Point::new(10.0, 0.0)),
                Segment::Cubic(
                    Point::new(14.0, 0.0),
                    Point::new(18.0, 4.0),
                    Point::new(18.0, 8.0),
                ),
            ],
            closed: false,
        };
        let lines = vec![Centreline { path, prim: None }];
        let mut before = Vec::new();
        flatten(&lines[0].path, 0.05, &mut before);
        for (seg, t) in [(0usize, 0.5), (1, 0.3)] {
            let out = split(&lines, 0, seg, t).expect("a path segment");
            assert_eq!(out[0].path.segments.len(), if seg == 0 { 2 } else { 3 });
            let mut after = Vec::new();
            flatten(&out[0].path, 0.05, &mut after);
            let g = SegGrid::new(&before, 1.0);
            for &(p, _) in &after {
                let d = g.nearest(p, 5.0).expect("near the original").d;
                assert!(d < 1e-3, "split {seg} moved the curve by {d}");
            }
        }
        // Primitive centrelines are not split.
        let prim = Centreline {
            prim: Some(PrimitiveFit {
                kind: PrimitiveKind::Circle {
                    c: Point::new(0.0, 0.0),
                    r: 3.0,
                },
                chi2: 0.0,
                params: 3.0,
            }),
            ..lines[0].clone()
        };
        assert!(split(&[prim], 0, 0, 0.5).is_none());
    }
}
