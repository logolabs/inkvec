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
//! the first sum over every boundary point (one-sided on the canvas frame:
//! [`Boundary::residual`]; `d` the distance to the nearest
//! centreline -- under miter joins, the miter gauge where the nearest point is a vertex,
//! see [`super::join`] -- and `σ_p` its sigma floored at 1e-3 px), the second a weak anchor
//! (`σ_a` = [`ANCHOR`] px) that only matters for a variable no boundary point sees -- a
//! junction end buried inside the stroke it meets -- and keeps the normal matrix
//! non-singular.
//!
//! **The step.** Each point's residual depends on the few variables of the one segment
//! nearest to it. By the envelope theorem the foot point's own movement adds nothing to
//! first order, so the derivative is `-n·∂C/∂θ` at the foot, `n` the unit vector from the
//! foot to the point: written out per segment kind in [`super::dist`] and assembled by
//! [`analytic_row`]; only the miter gauge at a vertex (and degenerate feet) keep central
//! differences of the exact distance (step 1e-5 px). The normal equations `(JᵀJ + μ·diag(JᵀJ)) δ = -Jᵀr`
//! couple each variable only to its own segment's neighbours and to `h`, so they are kept
//! in profile storage and solved by an envelope Cholesky ([`super::skyline`], exactly the
//! dense factorisation's result at a fraction of its cost; [`profile`] gives the shape). A step
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

use inkvec_core::{Point, Vec2};
use inkvec_fit::curves::{arc_ellipse_center, Segment};
use inkvec_fit::primitives::{PrimitiveFit, PrimitiveKind};
use inkvec_fit::FittedPath;
use rayon::prelude::*;

use super::boundary::Boundary;
use super::bvh::PieceTree;
use super::dist::{
    circle_grad, circular_arc_grad, cubic_eval, cubic_grad, dist_to, ellipse_arc_grad, line_grad,
};
use super::grid::point_segment;
use super::join::{miter_gauge, tangents, Join};
use super::score::flatten;
use super::skyline::Skyline;
use super::Centreline;

/// Iterations of the solve.
const MAX_ITERS: usize = 30;

/// Iterations of the solve run on each split trial ([`solve_adaptive`]); the strokes
/// kept are solved to [`MAX_ITERS`] once at the end. A trial only has to show whether the
/// split pays, and the full solves on every trial were most of the stage's time (lucide at
/// 128 px: stage mean 299 ms, worst face 3.5 s, under load).
const TRIAL_ITERS: usize = 8;

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

/// Above this many variables the solve is skipped; the strokes keep their fitted
/// geometry. A face of lucide at 128 px has 20-140. (The cap dates from the dense
/// factorisation, whose cost was cubic in it.)
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
#[derive(Clone)]
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

/// Largest distance, px, between a path segment and the chords that stand in for it in
/// the nearest-segment search ([`flat_pieces`]).
const FLAT_TOL: f64 = 0.02;

/// Path segment `seg` starting at `a` as chords that stay within [`FLAT_TOL`] of it,
/// appended to `out` with the parameter range each covers appended to `range`: the
/// cubic's `t`, the arc's fraction of its sweep, the line's own position.
///
/// The pieces only choose which segment a boundary point belongs to and where its exact
/// foot is polished from (Newton's method in [`super::dist`]); the distance itself is
/// the exact one. So they need not be short, only close: a line is one chord; a cubic is
/// halved by de Casteljau's construction until both inner control points lie within
/// `4/3·FLAT_TOL` of the chord (the curve then lies within `FLAT_TOL` of it, because
/// `B₁(t) + B₂(t) = 3t(1 - t) <= 3/4`); an arc is cut into equal angles whose sagitta
/// `R·(1 - cos(Δθ/2))` at its larger radius `R` is at most `FLAT_TOL`.
///
/// This replaced chords of a quarter pixel along every segment: at 512 px, where a
/// boundary point sits a half-width (~21 px) from its foot, the pieces nearly as close as
/// the nearest ran some 13 px along the curve either way, and the search tested about 70
/// of them per point (lucide `vegan`).
///
/// Inspired by: the flatness test of adaptive Bézier flattening by recursive subdivision
/// (de Casteljau), as surveyed in Lane, Riesenfeld (1980), A theoretical development for
/// the computer generation and display of piecewise polynomial surfaces, IEEE TPAMI 2(1),
/// 35-46, doi:10.1109/TPAMI.1980.4766968; ours stops on the control points' distance to
/// the chord segment.
fn flat_pieces(
    a: Point,
    seg: &Segment,
    out: &mut Vec<(Point, Point)>,
    range: &mut Vec<(f64, f64)>,
) {
    let mut push = |p: Point, q: Point, t0: f64, t1: f64| {
        if p.dist(q) > 1e-12 {
            out.push((p, q));
            range.push((t0, t1));
        }
    };
    match *seg {
        Segment::Line(b) => push(a, b, 0.0, 1.0),
        Segment::Cubic(c1, c2, b) => {
            // Depth-first, left half first, so the chords come out in order.
            let mut stack = vec![([a, c1, c2, b], 0.0, 1.0, 0u32)];
            while let Some((q, t0, t1, depth)) = stack.pop() {
                let flat = point_segment(q[1], q[0], q[3])
                    .0
                    .max(point_segment(q[2], q[0], q[3]).0);
                if depth >= 16 || 0.75 * flat <= FLAT_TOL {
                    push(q[0], q[3], t0, t1);
                    continue;
                }
                let mid = |p: Point, r: Point| Point::new(0.5 * (p.x + r.x), 0.5 * (p.y + r.y));
                let (p01, p12, p23) = (mid(q[0], q[1]), mid(q[1], q[2]), mid(q[2], q[3]));
                let (p012, p123) = (mid(p01, p12), mid(p12, p23));
                let m = mid(p012, p123);
                let tm = 0.5 * (t0 + t1);
                stack.push(([m, p123, p23, q[3]], tm, t1, depth + 1));
                stack.push(([q[0], p01, p012, m], t0, tm, depth + 1));
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
            let f = arc_ellipse_center(a, rx, ry, phi, large_arc, sweep, end);
            let r = f.rx.abs().max(f.ry.abs());
            let n = if f.delta == 0.0 || r <= FLAT_TOL {
                1
            } else {
                let step = 2.0 * (1.0 - FLAT_TOL / r).clamp(-1.0, 1.0).acos();
                ((f.delta.abs() / step).ceil() as usize).clamp(1, 4096)
            };
            let mut prev = a;
            for i in 1..=n {
                let u = i as f64 / n as f64;
                let q = if i == n {
                    end
                } else {
                    f.at(f.theta1 + f.delta * u)
                };
                push(prev, q, (i - 1) as f64 / n as f64, u);
                prev = q;
            }
        }
    }
}

/// Every piece of every shape, tagged `(shape, segment, t0, t1)`, for the nearest-segment
/// search: path segments by [`flat_pieces`]; circles and fixed primitives flattened at a
/// quarter pixel (a fixed shape's distance *is* its nearest piece's, so its pieces stay
/// short), fixed shapes tagged with segment `usize::MAX`.
#[allow(clippy::type_complexity)]
fn pieces(model: &Model) -> (Vec<(Point, Point)>, Vec<(usize, usize, f64, f64)>) {
    let (mut segs, mut tags) = (Vec::new(), Vec::new());
    let mut range = Vec::new();
    for (si, s) in model.shapes.iter().enumerate() {
        match s {
            Shape::Path {
                start, segs: sv, ..
            } => {
                for k in 0..sv.len() {
                    let (a, seg) = model.segment(*start, sv, k);
                    range.clear();
                    flat_pieces(a, &seg, &mut segs, &mut range);
                    tags.extend(range.iter().map(|&(t0, t1)| (si, k, t0, t1)));
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
    let tree = PieceTree::new(&segs);
    // Each point's row depends on the model and that point alone, so the points are
    // measured in parallel; `collect` keeps their order, and nothing is summed here.
    pts.par_iter()
        .map(|&p| {
            let n = tree.nearest(p, reach)?;
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
            b.residual(i, r.as_ref().map_or(reach, |r| r.d), h)
                .0
                .powi(2)
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
    max_iters: usize,
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
    let first = profile(&model);
    let mut mu = 1e-3;
    for _ in 0..max_iters {
        let (ata, atr) = normal_equations(&mut model, b, &rws, &theta0, &sig, &first);
        let mut improved = false;
        for _ in 0..MAX_RETRIES {
            let mut a = ata.clone();
            for i in 0..n {
                a.add_diag(i, mu * ata.diag(i).max(1e-9));
            }
            let Some(delta) = a.cholesky_solve(&atr) else {
                mu *= 4.0;
                continue;
            };
            let saved = model.theta.clone();
            for (x, d) in model.theta.iter_mut().zip(&delta) {
                *x -= d;
            }
            // A step that gives a cubic a cusp is refused whatever it does to `E`, so it is
            // refused before the boundary is measured against it. A fit that starts with a
            // cusp (the curve fitter's own) cannot be held to it.
            if start_regular && !regular(&model) {
                model.theta = saved;
                mu *= 4.0;
                continue;
            }
            let (e2, r2) = rows(&model, b, reach);
            let en2 = e2 + anchor(&model.theta);
            if en2 < energy {
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
    let (mut cur, mut h) = solve(lines, h, b, join, MAX_ITERS);
    let max_splits = inkvec_core::env::count("INKVEC_RIBBONS_SPLITS").unwrap_or(MAX_SPLITS);
    let reach = 8.0 * h + 4.0;
    // The description length of strokes `ls` at half-width `h`, and the rows it was
    // measured with: the rows of the strokes kept so far are what the next worst-segment
    // search reads, so they are kept rather than measured again (same model, same reach,
    // same rows).
    let cost = |ls: &[Centreline], h: f64| -> (f64, Vec<Option<Row>>) {
        let (e, rws) = rows(&Model::new(ls, h, join), b, reach);
        (
            0.5 * e + lambda * ls.iter().map(Centreline::params).sum::<f64>(),
            rws,
        )
    };
    let (mut best, mut cur_rows) = cost(&cur, h);
    let mut refused: Vec<(usize, usize)> = Vec::new();
    let mut accepted_any = false;
    for _ in 0..max_splits {
        let model = Model::new(&cur, h, join);
        let Some((shape, seg, t)) = worst_segment(&model, &cur_rows, h, b, &refused) else {
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
        let (tl, th) = solve(&trial, h, b, join, TRIAL_ITERS);
        let (c, trial_rows) = cost(&tl, th);
        if c < best && drawable(&tl, join) {
            accepted_any = true;
            best = c;
            cur = tl;
            cur_rows = trial_rows;
            h = th;
            // Segment indices after the split moved; earlier refusals no longer name the
            // same segments.
            refused.clear();
        } else {
            refused.push((shape, seg));
        }
    }
    if accepted_any {
        let (polished, ph) = solve(&cur, h, b, join, MAX_ITERS);
        if drawable(&polished, join) && cost(&polished, ph).0 <= best {
            return (polished, ph);
        }
    }
    (cur, h)
}

/// The path segment of `model` carrying the most `Σ ((d - h)/σ)²` over its rows `rws`
/// (measured on `model`), with the foot parameter of its worst point (clamped to
/// `[0.15, 0.85]` so neither half is a sliver), skipping the `refused` ones and anything
/// that is not a path segment. `None` when nothing is left.
fn worst_segment(
    model: &Model,
    rws: &[Option<Row>],
    h: f64,
    b: &Boundary,
    refused: &[(usize, usize)],
) -> Option<(usize, usize, f64)> {
    let mut acc: std::collections::BTreeMap<(usize, usize), (f64, f64, f64)> =
        std::collections::BTreeMap::new();
    for (i, row) in rws.iter().enumerate() {
        let Some(row) = row else { continue };
        if row.seg == usize::MAX || !matches!(model.shapes[row.shape], Shape::Path { .. }) {
            continue;
        }
        let r = b.residual(i, row.d, h).0.powi(2);
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

/// The envelope of the normal matrix `JᵀJ` for `model` ([`Skyline`]): for each variable,
/// the first (lowest-index) variable any residual couples it to.
///
/// A residual reads one segment's variables and its start point ([`locals`]), or under
/// miter joins two neighbouring segments' (the gauge at a vertex, both neighbours of
/// every segment, round a closed path's start too), or a circle's three; every set is
/// listed here, so the envelope holds every nonzero `JᵀJ` can have whatever the rows'
/// assignment to segments. The half-width (last variable) is in every residual: its row
/// is full. Fixed shapes have no variables. Cost O(segments).
fn profile(model: &Model) -> Vec<usize> {
    let n = model.theta.len();
    let mut first: Vec<usize> = (0..n).collect();
    let mut note = |v: &[usize]| {
        if let Some(&lo) = v.iter().min() {
            for &u in v {
                first[u] = first[u].min(lo);
            }
        }
    };
    for (si, s) in model.shapes.iter().enumerate() {
        match s {
            Shape::Path { segs, closed, .. } => {
                let m = segs.len();
                for k in 0..m {
                    note(&locals(model, si, k, None));
                    if model.join == Join::Miter {
                        for foot in [Foot::Start, Foot::End] {
                            if let Some(j) = neighbour(m, *closed, k, foot) {
                                note(&locals(model, si, k, Some(j)));
                            }
                        }
                    }
                }
            }
            Shape::Circle { at, .. } => note(&[*at, at + 1, at + 2]),
            Shape::Fixed(_) => {}
        }
    }
    if let Some(f) = first.last_mut() {
        *f = 0;
    }
    first
}

/// The derivatives of row (`shape`, `seg`)'s distance term at boundary point `p` with
/// respect to the variables it reads, in closed form ([`super::dist`]'s gradients, by the
/// envelope theorem), appended to `out` as `(variable, ∂d/∂θ)` with every variable once.
/// `false` (and `out` untouched) where the closed form is not used: the degenerate cases
/// the gradients decline and -- decided by the caller -- a miter gauge at a vertex; the
/// caller then takes central differences.
///
/// A segment's start point is the previous segment's end variable (the path's start for
/// the first); a closed path's last end *is* its start, so a variable can receive two
/// contributions (a one-segment loop), which are summed.
fn analytic_row(
    model: &Model,
    shape: usize,
    seg: usize,
    p: Point,
    t_hint: f64,
    out: &mut Vec<(usize, f64)>,
) -> bool {
    let t = &model.theta;
    let mut g: Vec<(usize, f64)> = Vec::with_capacity(9);
    let put = |g: &mut Vec<(usize, f64)>, i: usize, v: Vec2| {
        g.push((i, v.x));
        g.push((i + 1, v.y));
    };
    match &model.shapes[shape] {
        Shape::Path { start, segs, .. } => {
            let a_i = if seg == 0 {
                *start
            } else {
                seg_end(&segs[seg - 1])
            };
            let a = pt(t, a_i);
            match segs[seg] {
                SegVar::Line { end } => {
                    let Some((_, d)) = line_grad(p, a, pt(t, end)) else {
                        return false;
                    };
                    put(&mut g, a_i, d[0]);
                    put(&mut g, end, d[1]);
                }
                SegVar::Cubic { c1, c2, end } => {
                    let q = [a, pt(t, c1), pt(t, c2), pt(t, end)];
                    let Some((_, d)) = cubic_grad(p, q, t_hint) else {
                        return false;
                    };
                    for (i, v) in [a_i, c1, c2, end].into_iter().zip(d) {
                        put(&mut g, i, v);
                    }
                }
                SegVar::Arc {
                    end,
                    r: Some(ri),
                    large,
                    sweep,
                    ..
                } => {
                    let Some((_, d, dr)) = circular_arc_grad(p, a, t[ri], large, sweep, pt(t, end))
                    else {
                        return false;
                    };
                    put(&mut g, a_i, d[0]);
                    put(&mut g, end, d[1]);
                    g.push((ri, dr));
                }
                SegVar::Arc {
                    end,
                    r: None,
                    rx,
                    ry,
                    phi,
                    large,
                    sweep,
                } => {
                    let arc = Segment::Arc {
                        rx,
                        ry,
                        phi,
                        large_arc: large,
                        sweep,
                        end: pt(t, end),
                    };
                    let Some((_, d)) = ellipse_arc_grad(p, a, &arc, t_hint) else {
                        return false;
                    };
                    put(&mut g, a_i, d[0]);
                    put(&mut g, end, d[1]);
                }
            }
        }
        Shape::Circle { at, .. } => {
            let Some((_, dc, dr)) = circle_grad(p, pt(t, *at), t[at + 2]) else {
                return false;
            };
            put(&mut g, *at, dc);
            g.push((at + 2, dr));
        }
        Shape::Fixed(_) => return false,
    }
    g.sort_by_key(|e| e.0);
    for (i, v) in g {
        match out.last_mut() {
            Some(last) if last.0 == i => last.1 += v,
            _ => out.push((i, v)),
        }
    }
    true
}

/// `JᵀJ` (lower triangle in the envelope `first`, [`profile`]) and `Jᵀr` for the data
/// rows and the anchor.
///
/// Each row's `J` is the closed form of [`analytic_row`] where it applies -- every row
/// under round joins except at degenerate feet -- and otherwise central
/// differences of the segment's exact distance (step 1e-5 px; see the module docs): the
/// miter gauge at a vertex, whose tangent-dependent derivative is not written out. The
/// closed form costs one foot per row where the differences cost two exact distances per
/// variable (16 Newton solves for a cubic's row); on lucide at 512 px this pass took
/// 10-25 ms per iteration.
fn normal_equations(
    model: &mut Model,
    b: &Boundary,
    rws: &[Option<Row>],
    theta0: &[f64],
    sig: &[f64],
    first: &[usize],
) -> (Skyline, Vec<f64>) {
    let n = model.theta.len();
    let hi = n - 1;
    let mut ata = Skyline::zeros(first.to_vec());
    let mut atr = vec![0.0; n];
    let eps = 1e-5;
    let h = model.h();
    // Each row's residual and Jacobian entries depend on the model and that row alone, so
    // they are computed in parallel (each worker on its own copy of the model, which the
    // central differences perturb and restore exactly); the products are then added into
    // `JᵀJ` and `Jᵀr` in row order, as before, so every sum is the sequential one.
    let jacs: Vec<Option<(f64, Vec<(usize, f64)>)>> = rws
        .par_iter()
        .enumerate()
        .map_init(
            || model.clone(),
            |m, (i, row)| {
                let row = row.as_ref()?;
                let (r, s, active) = b.residual(i, row.d, h);
                if !active {
                    return None;
                }
                let mut jac: Vec<(usize, f64)> = vec![(hi, -1.0 / s)];
                if row.seg != usize::MAX {
                    // Under round joins every row reads one segment's plain distance; under
                    // miter joins a row whose foot is a vertex reads the gauge of two
                    // segments instead.
                    let with = match m.join {
                        Join::Round => None,
                        Join::Miter => local_eval(m, row.shape, row.seg, b.pts[i], row.t).1,
                    };
                    if with.is_none()
                        && analytic_row(m, row.shape, row.seg, b.pts[i], row.t, &mut jac)
                    {
                        for e in jac.iter_mut().skip(1) {
                            e.1 /= s;
                        }
                    } else {
                        for v in locals(m, row.shape, row.seg, with) {
                            let x = m.theta[v];
                            m.theta[v] = x + eps;
                            let dp = local_dist(m, row.shape, row.seg, b.pts[i], row.t);
                            m.theta[v] = x - eps;
                            let dm = local_dist(m, row.shape, row.seg, b.pts[i], row.t);
                            m.theta[v] = x;
                            jac.push((v, (dp - dm) / (2.0 * eps * s)));
                        }
                    }
                }
                Some((r, jac))
            },
        )
        .collect();
    for (r, jac) in jacs.into_iter().flatten() {
        for &(u, ju) in &jac {
            atr[u] += ju * r;
            for &(v, jv) in &jac {
                if v <= u {
                    ata.add(u, v, ju * jv);
                }
            }
        }
    }
    for j in 0..n {
        let w = 1.0 / (sig[j] * sig[j]);
        ata.add_diag(j, w);
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

#[cfg(test)]
mod tests {
    use super::*;
    use inkvec_core::Polyline;

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
        let (out, h) = solve(&[line], 2.8, &b, Join::Round, MAX_ITERS);
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
    fn flat_pieces_stay_within_tolerance_and_cover_the_parameter() {
        let a = Point::new(0.0, 0.0);
        let segs = [
            Segment::Cubic(
                Point::new(40.0, 0.0),
                Point::new(60.0, 50.0),
                Point::new(10.0, 80.0),
            ),
            Segment::circular_arc(30.0, false, true, Point::new(30.0, 30.0)),
            Segment::Arc {
                rx: 500.0,
                ry: 60.0,
                phi: 0.4,
                large_arc: false,
                sweep: false,
                end: Point::new(80.0, -20.0),
            },
            Segment::Line(Point::new(100.0, 3.0)),
        ];
        for seg in segs {
            let (mut out, mut range) = (Vec::new(), Vec::new());
            flat_pieces(a, &seg, &mut out, &mut range);
            assert!(!out.is_empty());
            assert_eq!(range[0].0, 0.0);
            assert_eq!(range[range.len() - 1].1, 1.0);
            for w in range.windows(2) {
                assert_eq!(w[0].1, w[1].0, "contiguous");
            }
            let path = FittedPath {
                start: a,
                segments: vec![seg.clone()],
                closed: false,
            };
            let mut fine = Vec::new();
            flatten(&path, 0.01, &mut fine);
            let tree = PieceTree::new(&out);
            for &(p, _) in &fine {
                let d = tree.nearest(p, 10.0).expect("near").d;
                assert!(d <= FLAT_TOL + 1e-4, "{seg:?}: {d}");
            }
        }
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
            let g = crate::ribbon::grid::SegGrid::new(&before, 1.0);
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
