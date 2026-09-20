//! The fewest segments that draw the same picture.
//!
//! An SVG path can be written a thousand ways: a circle as sixteen cubics or as five, a
//! straight edge as a run of collinear lines, one curve as four exactly-subdivided pieces.
//! They all render identically, and every extra segment is description length that says
//! nothing. This rewrites each path as the cheapest description that stays within a
//! tolerance of the original curve, scored the way the tracer scores everything:
//! `0.5·chi² + λ·params`, the minimum-description-length objective of `inkvec-fit`.
//!
//! What makes this different from a tolerance-based simplifier is where corners come
//! from. A raster tracer has to *infer* corners from pixels; here the source is vector, so
//! a corner is a fact: two consecutive segments whose tangents disagree. Those joins are
//! hard breaks that no fit may smooth across, and everything between two of them is
//! refitted freely. Sharp things stay sharp by construction, not by luck.
//!
//! The tolerance is stated at a viewing size: "invisible at 1024 px" is a property of the
//! picture, where "0.01 units" depends on an arbitrary viewBox. Only `d` attributes are
//! touched; paint, ids, groups, gradients and transforms pass through untouched, and a
//! path that would not get cheaper is left exactly as it was.

use std::ops::Range;

use inkvec_core::{Point, Polyline};
use inkvec_fit::{
    curves::Segment,
    multimodel::optimal_multimodel,
    primitives::{fit_primitive_or_arcs, PrimitiveKind},
    structural::eval_segment,
    FitConfig, FittedPath,
};

/// How the rewrite is judged.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Largest deviation from the original curve, in pixels, when the drawing is viewed at
    /// `judge` pixels on its longer side. 0.1 px at 1024 px is below what a screen shows.
    pub tolerance_px: f64,
    /// The viewing size the tolerance is stated at.
    pub judge: f64,
    /// Turn between two consecutive source segments, in degrees, above which their join
    /// is a corner that must survive exactly.
    pub corner_degrees: f64,
    /// Decimals written per coordinate; `None` derives them from the tolerance.
    pub decimals: Option<usize>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            tolerance_px: 0.1,
            judge: 1024.0,
            corner_degrees: 30.0,
            decimals: None,
        }
    }
}

/// What the rewrite did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// `<path>` elements seen.
    pub paths: usize,
    /// Paths whose `d` was rewritten (the rest got no cheaper and were left alone).
    pub rewritten: usize,
    /// Subpaths seen.
    pub subpaths: usize,
    /// Segments before and after, counting every source segment as one.
    pub segments_before: usize,
    /// Segments after.
    pub segments_after: usize,
    /// Description length before and after, in the fitter's parameter units (a line is
    /// 2, a cubic 6, an arc fewer).
    pub params_before: f64,
    /// Description length after.
    pub params_after: f64,
    /// Runs whose fit strayed past the tolerance and were replaced by their source.
    pub guarded: usize,
    /// Paths written as a `<circle>`, `<ellipse>` or `<rect>` element instead of a path.
    pub primitives: usize,
    /// The tolerance actually used, in the document's units.
    pub tolerance_units: f64,
}

/// One segment of a source path, absolute, with its start point.
#[derive(Debug, Clone, Copy)]
enum Src {
    Line(Point, Point),
    Cubic(Point, Point, Point, Point),
}

impl Src {
    fn start(&self) -> Point {
        match *self {
            Src::Line(a, _) | Src::Cubic(a, _, _, _) => a,
        }
    }
    fn params(&self) -> f64 {
        match self {
            Src::Line(..) => 2.0,
            Src::Cubic(..) => 6.0,
        }
    }
    fn segment(&self) -> Segment {
        match *self {
            Src::Line(_, b) => Segment::Line(b),
            Src::Cubic(_, c1, c2, b) => Segment::Cubic(c1, c2, b),
        }
    }
    /// Direction of travel leaving the start; falls back along the control polygon when a
    /// control point sits on the endpoint (a cusp, or a line drawn as a cubic).
    fn tangent_out(&self) -> Option<Point> {
        match *self {
            Src::Line(a, b) => unit(sub(b, a)),
            Src::Cubic(a, c1, c2, b) => unit(sub(c1, a))
                .or_else(|| unit(sub(c2, a)))
                .or_else(|| unit(sub(b, a))),
        }
    }
    fn tangent_in(&self) -> Option<Point> {
        match *self {
            Src::Line(a, b) => unit(sub(b, a)),
            Src::Cubic(a, c1, c2, b) => unit(sub(b, c2))
                .or_else(|| unit(sub(b, c1)))
                .or_else(|| unit(sub(b, a))),
        }
    }
    fn at(&self, t: f64) -> Point {
        match *self {
            Src::Line(a, b) => Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t),
            Src::Cubic(a, c1, c2, b) => {
                let u = 1.0 - t;
                let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                Point::new(
                    w0 * a.x + w1 * c1.x + w2 * c2.x + w3 * b.x,
                    w0 * a.y + w1 * c1.y + w2 * c2.y + w3 * b.y,
                )
            }
        }
    }
    /// The part of this segment between two parameters, as a segment of its own: the
    /// exact same curve, so a span that falls back to its source loses nothing.
    fn piece(&self, t0: f64, t1: f64) -> Src {
        if t0 <= 0.0 && t1 >= 1.0 {
            return *self;
        }
        match *self {
            Src::Line(a, b) => {
                let at = |t: f64| Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
                Src::Line(at(t0), at(t1))
            }
            Src::Cubic(a, c1, c2, b) => {
                let (a, c1, c2, b) = split_cubic(a, c1, c2, b, t1).0;
                let u = if t1 > 1e-12 { t0 / t1 } else { 0.0 };
                let (a, c1, c2, b) = split_cubic(a, c1, c2, b, u).1;
                Src::Cubic(a, c1, c2, b)
            }
        }
    }
    /// Length of the control polygon: an upper bound on the curve's, and close enough to
    /// decide how densely to sample it.
    fn rough_length(&self) -> f64 {
        match *self {
            Src::Line(a, b) => a.dist(b),
            Src::Cubic(a, c1, c2, b) => a.dist(c1) + c1.dist(c2) + c2.dist(b),
        }
    }
}

fn sub(a: Point, b: Point) -> Point {
    Point::new(a.x - b.x, a.y - b.y)
}

/// De Casteljau's construction: the cubic cut at `t`, as its two halves.
type Cubic = (Point, Point, Point, Point);
fn split_cubic(a: Point, c1: Point, c2: Point, b: Point, t: f64) -> (Cubic, Cubic) {
    let mix = |p: Point, q: Point| Point::new(p.x + (q.x - p.x) * t, p.y + (q.y - p.y) * t);
    let (p01, p12, p23) = (mix(a, c1), mix(c1, c2), mix(c2, b));
    let (p012, p123) = (mix(p01, p12), mix(p12, p23));
    let m = mix(p012, p123);
    ((a, p01, p012, m), (m, p123, p23, b))
}

fn unit(v: Point) -> Option<Point> {
    let n = v.x.hypot(v.y);
    (n > 1e-9).then(|| Point::new(v.x / n, v.y / n))
}

/// A subpath: consecutive segments, and whether it closes back on its start.
#[derive(Debug, Clone)]
struct Subpath {
    segs: Vec<Src>,
    closed: bool,
}

/// Parse a `d` attribute into absolute subpaths of lines and cubics. Arcs and quadratics
/// become cubics (the arc conversion is the only lossy step, at ~1e-6 of the radius).
fn parse_d(d: &str) -> Result<Vec<Subpath>, String> {
    use svgtypes::SimplePathSegment as S;
    let mut out: Vec<Subpath> = Vec::new();
    let mut cur: Vec<Src> = Vec::new();
    let mut start = Point::new(0.0, 0.0);
    let mut pen = start;
    let flush = |cur: &mut Vec<Src>, out: &mut Vec<Subpath>, closed: bool| {
        if !cur.is_empty() {
            out.push(Subpath {
                segs: std::mem::take(cur),
                closed,
            });
        }
    };
    for seg in svgtypes::SimplifyingPathParser::from(d) {
        match seg.map_err(|e| format!("bad path data: {e}"))? {
            S::MoveTo { x, y } => {
                flush(&mut cur, &mut out, false);
                start = Point::new(x, y);
                pen = start;
            }
            S::LineTo { x, y } => {
                let p = Point::new(x, y);
                if p.dist(pen) > 1e-12 {
                    cur.push(Src::Line(pen, p));
                }
                pen = p;
            }
            S::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let p = Point::new(x, y);
                cur.push(Src::Cubic(pen, Point::new(x1, y1), Point::new(x2, y2), p));
                pen = p;
            }
            S::Quadratic { x1, y1, x, y } => {
                let (q, p) = (Point::new(x1, y1), Point::new(x, y));
                let c1 = Point::new(
                    pen.x + 2.0 / 3.0 * (q.x - pen.x),
                    pen.y + 2.0 / 3.0 * (q.y - pen.y),
                );
                let c2 = Point::new(p.x + 2.0 / 3.0 * (q.x - p.x), p.y + 2.0 / 3.0 * (q.y - p.y));
                cur.push(Src::Cubic(pen, c1, c2, p));
                pen = p;
            }
            S::ClosePath => {
                if pen.dist(start) > 1e-9 && !cur.is_empty() {
                    cur.push(Src::Line(pen, start));
                }
                flush(&mut cur, &mut out, true);
                pen = start;
            }
        }
    }
    flush(&mut cur, &mut out, false);
    Ok(out)
}

/// Which joins of a subpath are corners: index `k` means the join entering `segs[k]`
/// (for a closed subpath, `0` is the join from the last segment back to the first).
fn corners(sp: &Subpath, corner_degrees: f64) -> Vec<bool> {
    let n = sp.segs.len();
    let cos_limit = corner_degrees.to_radians().cos();
    (0..n)
        .map(|k| {
            if k == 0 && !sp.closed {
                return true;
            }
            let prev = &sp.segs[(k + n - 1) % n];
            let next = &sp.segs[k];
            match (prev.tangent_in(), next.tangent_out()) {
                (Some(a), Some(b)) => a.x * b.x + a.y * b.y < cos_limit,
                _ => true,
            }
        })
        .collect()
}

/// Points along a run of source segments, spaced about `spacing` apart, starting at the
/// run's first point and ending at its last (unless `drop_last`, for closed loops whose
/// last point is the first).
#[cfg(test)]
fn sample(run: &[Src], spacing: f64, drop_last: bool) -> Vec<Point> {
    sample_mapped(run, spacing, drop_last).0
}

/// The same samples, each with where it came from: the source segment's index plus the
/// parameter within it. That is what lets a span of samples be turned back into the exact
/// source curve it was taken from.
fn sample_mapped(run: &[Src], spacing: f64, drop_last: bool) -> (Vec<Point>, Vec<f64>) {
    let mut pts = vec![run[0].start()];
    let mut at = vec![0.0];
    let most = knob("INKVEC_SVGMIN_PER_SEG", PER_SEGMENT as f64) as usize;
    for (k, s) in run.iter().enumerate() {
        let n = ((s.rough_length() / spacing).ceil() as usize).clamp(4, most);
        for i in 1..=n {
            let t = i as f64 / n as f64;
            pts.push(s.at(t));
            at.push(k as f64 + t);
        }
    }
    if drop_last {
        pts.pop();
        at.pop();
    }
    (pts, at)
}

/// Distance from `p` to one segment of the fitted path: a coarse scan of the curve, then a
/// golden-section search on the parameter around the nearest sample. Sampling alone would
/// report the chord sagitta of the scan as error -- 0.04 units on a 120° arc of radius 40
/// -- and that is three times the tolerance this tool promises.
fn dist_to_segment(p: Point, seg: &Segment, start: Point) -> f64 {
    if let Segment::Line(b) = *seg {
        return point_segment_dist(p, start, b);
    }
    let m = 32;
    let mut best = (0usize, f64::INFINITY);
    for i in 0..=m {
        let d = p.dist(eval_segment(seg, start, i as f64 / m as f64));
        if d < best.1 {
            best = (i, d);
        }
    }
    let (mut lo, mut hi) = (
        (best.0 as f64 - 1.0).max(0.0) / m as f64,
        (best.0 as f64 + 1.0).min(m as f64) / m as f64,
    );
    let phi = 0.618_033_988_749_895;
    let (mut a, mut b) = (hi - phi * (hi - lo), lo + phi * (hi - lo));
    let (mut fa, mut fb) = (
        p.dist(eval_segment(seg, start, a)),
        p.dist(eval_segment(seg, start, b)),
    );
    for _ in 0..28 {
        if fa < fb {
            hi = b;
            b = a;
            fb = fa;
            a = hi - phi * (hi - lo);
            fa = p.dist(eval_segment(seg, start, a));
        } else {
            lo = a;
            a = b;
            fa = fb;
            b = lo + phi * (hi - lo);
            fb = p.dist(eval_segment(seg, start, b));
        }
    }
    best.1.min(fa).min(fb)
}

/// A fitted path sampled once, so that many points can each find their nearest segment
/// with one scan of the table and one refinement, instead of a search on every segment.
/// Refining every segment for every sample made a 40-file batch take 57 s; this takes it
/// back under a second.
struct Curve {
    starts: Vec<Point>,
    segs: Vec<Segment>,
    samples: Vec<Point>,
    seg_of: Vec<usize>,
    /// Per segment: no point of it is farther than this from its nearest table sample.
    reach: Vec<f64>,
}

/// Table samples per segment. Coarse on purpose: the table only nominates a segment, and
/// the exact search on it and its neighbours does the rest.
const COARSE: usize = 8;

impl Curve {
    fn new(path: &FittedPath) -> Self {
        let mut c = Curve {
            starts: Vec::with_capacity(path.segments.len()),
            segs: path.segments.clone(),
            samples: Vec::with_capacity(path.segments.len() * (COARSE + 1)),
            seg_of: Vec::with_capacity(path.segments.len() * (COARSE + 1)),
            reach: Vec::with_capacity(path.segments.len()),
        };
        let mut cur = path.start;
        for (k, s) in path.segments.iter().enumerate() {
            c.starts.push(cur);
            let first = c.samples.len();
            for i in 0..=COARSE {
                c.samples
                    .push(eval_segment(s, cur, i as f64 / COARSE as f64));
                c.seg_of.push(k);
            }
            // Half the widest gap between consecutive samples, with slack for the curve
            // bowing away from its chord: an eighth of a full-circle arc is 2.6% longer
            // than its chord, and a fitted cubic is tamer than that.
            let gap = c.samples[first..]
                .windows(2)
                .map(|w| w[0].dist(w[1]))
                .fold(0.0, f64::max);
            c.reach.push(0.75 * gap);
            cur = s.end();
        }
        c
    }

    /// The segment nearest to `p`, and the distance to it. The table nominates a segment,
    /// which is searched exactly; every other segment is then searched exactly too unless
    /// its table proves it cannot win -- no point of it is nearer than its nearest sample
    /// less its `reach`. Searching only the nominee's neighbours is not enough: on a thin
    /// ring the far side's sample can be nearer than the near side's, the guard then reads
    /// the stroke's width as the deviation (60 tolerances, on a fit 1.4 off), and a third
    /// of one corpus's savings were thrown away on that misreading.
    fn nearest(&self, p: Point) -> (usize, f64) {
        if self.segs.is_empty() {
            return (0, f64::INFINITY);
        }
        let mut best = (0usize, f64::INFINITY);
        for (i, s) in self.samples.iter().enumerate() {
            let d = p.dist(*s);
            if d < best.1 {
                best = (i, d);
            }
        }
        let k = self.seg_of[best.0];
        let mut out = (k, dist_to_segment(p, &self.segs[k], self.starts[k]));
        for (j, chunk) in self.samples.chunks(COARSE + 1).enumerate() {
            if j == k {
                continue;
            }
            let near = chunk
                .iter()
                .map(|s| p.dist(*s))
                .fold(f64::INFINITY, f64::min);
            if near - self.reach[j] < out.1 {
                let d = dist_to_segment(p, &self.segs[j], self.starts[j]);
                if d < out.1 {
                    out = (j, d);
                }
            }
        }
        out
    }
}

/// Largest distance from any of `pts` to the fitted path.
fn max_deviation(pts: &[Point], path: &FittedPath) -> f64 {
    let curve = Curve::new(path);
    pts.iter().map(|&p| curve.nearest(p).1).fold(0.0, f64::max)
}

fn point_segment_dist(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let l2 = dx * dx + dy * dy;
    let t = if l2 < 1e-18 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0)
    };
    p.dist(Point::new(a.x + t * dx, a.y + t * dy))
}

fn cubic_at(p0: Point, c1: Point, c2: Point, p3: Point, t: f64) -> Point {
    let u = 1.0 - t;
    let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(
        w0 * p0.x + w1 * c1.x + w2 * c2.x + w3 * p3.x,
        w0 * p0.y + w1 * c1.y + w2 * c2.y + w3 * p3.y,
    )
}

/// The parameter of the point on a cubic nearest to `p`: a coarse scan, then a
/// golden-section search around the best sample. Exact enough to converge the refit below,
/// where a Newton step from chord-length parameters stalled a pixel off on a deep curve.
fn nearest_t_cubic(p: Point, p0: Point, c1: Point, c2: Point, p3: Point) -> f64 {
    let m = 32;
    let mut best = (0usize, f64::INFINITY);
    for i in 0..=m {
        let d = p.dist(cubic_at(p0, c1, c2, p3, i as f64 / m as f64));
        if d < best.1 {
            best = (i, d);
        }
    }
    let (mut lo, mut hi) = (
        (best.0 as f64 - 1.0).max(0.0) / m as f64,
        (best.0 as f64 + 1.0).min(m as f64) / m as f64,
    );
    let phi = 0.618_033_988_749_895;
    let (mut a, mut b) = (hi - phi * (hi - lo), lo + phi * (hi - lo));
    let (mut fa, mut fb) = (
        p.dist(cubic_at(p0, c1, c2, p3, a)),
        p.dist(cubic_at(p0, c1, c2, p3, b)),
    );
    for _ in 0..30 {
        if fa < fb {
            hi = b;
            b = a;
            fb = fa;
            a = hi - phi * (hi - lo);
            fa = p.dist(cubic_at(p0, c1, c2, p3, a));
        } else {
            lo = a;
            a = b;
            fa = fb;
            b = lo + phi * (hi - lo);
            fb = p.dist(cubic_at(p0, c1, c2, p3, b));
        }
    }
    if fa < fb {
        a
    } else {
        b
    }
}

/// The cubic through `p0` and `p3` closest to `pts` in least squares: the two control
/// points solved linearly at the parameters `t`, then each parameter moved to its point's
/// nearest place on the new curve and the solve repeated until it settles (Schneider,
/// 1990, with an exact projection in place of his Newton step). The dynamic program's
/// cubic is a tangent-constrained approximation good enough to *choose* a cubic; this is
/// what makes the chosen cubic *exact* where the source was one.
///
/// `t` is where each point sits along the source, which the caller knows and chord length
/// only guesses. It matters: a piece of a cubic, walked at its own rate, *is* a cubic and
/// the solve lands on it in one round, where from chord length the alternation settles
/// into a local minimum twenty-five tolerances away -- measured, on one cubic cut at 0.79.
fn ls_cubic_from(pts: &[Point], p0: Point, p3: Point, mut t: Vec<f64>) -> Option<(Point, Point)> {
    if pts.len() < 3 || t.len() != pts.len() {
        return None;
    }
    let (mut c1, mut c2);
    let mut last_err = f64::INFINITY;
    let mut best: Option<(Point, Point)> = None;
    let mut stalled = 0;
    for _ in 0..40 {
        let (mut s11, mut s12, mut s22) = (0.0, 0.0, 0.0);
        let (mut r1, mut r2) = (Point::new(0.0, 0.0), Point::new(0.0, 0.0));
        for (&p, &tt) in pts.iter().zip(&t) {
            let u = 1.0 - tt;
            let (b1, b2) = (3.0 * u * u * tt, 3.0 * u * tt * tt);
            let base = Point::new(
                u * u * u * p0.x + tt * tt * tt * p3.x,
                u * u * u * p0.y + tt * tt * tt * p3.y,
            );
            let r = sub(p, base);
            s11 += b1 * b1;
            s12 += b1 * b2;
            s22 += b2 * b2;
            r1 = Point::new(r1.x + b1 * r.x, r1.y + b1 * r.y);
            r2 = Point::new(r2.x + b2 * r.x, r2.y + b2 * r.y);
        }
        let det = s11 * s22 - s12 * s12;
        if det.abs() < 1e-18 {
            return None;
        }
        c1 = Point::new(
            (s22 * r1.x - s12 * r2.x) / det,
            (s22 * r1.y - s12 * r2.y) / det,
        );
        c2 = Point::new(
            (s11 * r2.x - s12 * r1.x) / det,
            (s11 * r2.y - s12 * r1.y) / det,
        );
        // Move every parameter to its point's nearest place on this cubic, and stop once
        // that no longer brings the points closer. An exact search each round: two Newton
        // steps from the previous parameter were tried and settled a tenth of a unit off on
        // a deep curve, where this reaches 1e-5. The refit is 3% of the run time.
        let mut err: f64 = 0.0;
        for (&p, tt) in pts.iter().zip(t.iter_mut()) {
            *tt = nearest_t_cubic(p, p0, c1, c2, p3);
            err = err.max(p.dist(cubic_at(p0, c1, c2, p3, *tt)));
        }
        // Keep the best round, not the last one: reprojection is not monotone, and a round
        // that overshoots would otherwise be the answer. And one bad round is not the end
        // of the search -- stopping at the first of them left a curve that is exactly a
        // cubic fitted 25 tolerances away from itself.
        if err < last_err {
            last_err = err;
            stalled = 0;
            if c1.x.is_finite() && c1.y.is_finite() && c2.x.is_finite() && c2.y.is_finite() {
                best = Some((c1, c2));
            }
        } else {
            stalled += 1;
            if stalled == 3 {
                break;
            }
        }
    }
    best
}

/// Refit one cubic to the samples it stands for, keeping the refit only where it brings
/// them closer. Anything that is not a cubic, and any cubic the least squares cannot
/// improve, comes back exactly as it went in. `along` is where each sample sits on the
/// source, normalised across the span.
fn refit_cubic(seg: &Segment, start: Point, span: &[Point], along: Vec<f64>) -> Segment {
    let Segment::Cubic(c1, c2, end) = *seg else {
        return seg.clone();
    };
    if span.len() < 3 {
        return seg.clone();
    }
    let Some((n1, n2)) = ls_cubic_from(span, start, end, along) else {
        return seg.clone();
    };
    let dev = |a: Point, b: Point| {
        span.iter()
            .map(|&p| dist_to_segment(p, &Segment::Cubic(a, b, end), start))
            .fold(0.0, f64::max)
    };
    if dev(n1, n2) < dev(c1, c2) {
        Segment::Cubic(n1, n2, end)
    } else {
        seg.clone()
    }
}

/// A tuning knob read from the environment, for measurement; the default is the shipped
/// value.
fn knob(k: &str, d: f64) -> f64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

/// Points the dynamic program is shown on the first pass; the tracer's own cap
/// (`inkvec_fit::multimodel::DP_MAX_POINTS`) on the second.
const COARSE_CAP: usize = 128;
const FINE_CAP: usize = 768;

/// Runs of at most this many samples are *also* fitted whole, at the fine cap, and the
/// cheaper of the two descriptions kept. Past here the program costs seconds rather than
/// milliseconds -- the square of the points it is shown -- and only the coarse view with
/// its repairs is affordable.
const WHOLE_BELOW: usize = 256;

/// Points the whole-run primitive fitter is shown. It costs O(n²) as well, and a circle
/// needs no more witnesses than this to be told from a cubic.
const PRIMITIVE_CAP: usize = 192;

/// Samples taken of one source segment, at most. What a fit needs is enough witnesses to
/// tell one curve from another, which is a property of the drawing; the spacing alone is
/// a property of the tolerance, and at a tight one it asks for hundreds of them per
/// segment -- which the program then pays for squared. Measured on the 40-file batch,
/// coming down from 48 to 24 saved *more* description length (17.3% to 17.6%) in half the
/// time: past two dozen witnesses a cubic has nothing left to say about itself.
const PER_SEGMENT: usize = 24;

/// What a description costs *as this tool writes it*, which is what the minifier is
/// trying to make small. It is not the fitter's parameter count: [`fmt_subpath`] restates
/// a cubic that continues the previous one smoothly as an `S`, four numbers instead of
/// six, so a chain of cubics is cheaper than its segments suggest and an arc -- five,
/// always -- has more to beat than it looks.
fn params_of(segs: &[Segment]) -> f64 {
    let mut total = 0.0;
    let mut prev: Option<(Point, Point)> = None;
    for seg in segs {
        match *seg {
            Segment::Line(_) => {
                total += 2.0;
                prev = None;
            }
            Segment::Cubic(c1, c2, p) => {
                let smooth = prev.is_some_and(|(pc2, pp): (Point, Point)| {
                    Point::new(2.0 * pp.x - pc2.x, 2.0 * pp.y - pc2.y).dist(c1) < 5e-4
                });
                total += if smooth { 4.0 } else { 6.0 };
                prev = Some((c2, p));
            }
            Segment::Arc { .. } => {
                total += 5.0;
                prev = None;
            }
        }
    }
    total
}

/// One run being fitted: the source, the samples taken along it, and where on the source
/// each sample sits. Sample indices are *unwrapped* on a closed run -- `i` and `i + n`
/// are the same point one lap apart -- so a span never has to wrap.
struct RunFit<'a> {
    run: &'a [Src],
    pts: Vec<Point>,
    /// Source position of each sample: segment index plus parameter within it.
    at: Vec<f64>,
    eps: f64,
    cfg: &'a FitConfig,
}

impl RunFit<'_> {
    fn point(&self, i: usize) -> Point {
        self.pts[i % self.pts.len()]
    }

    /// Source position of unwrapped sample `i`, laps included.
    fn source_at(&self, i: usize) -> f64 {
        let n = self.pts.len();
        self.at[i % n] + (i / n) as f64 * self.run.len() as f64
    }

    /// Samples `a..=b`, and where each one sits along the source between them, from 0 to
    /// 1. That is the parameterisation a fit of this span is looking for.
    fn span(&self, a: usize, b: usize) -> (Vec<Point>, Vec<f64>) {
        let (ua, ub) = (self.source_at(a), self.source_at(b));
        let width = if ub > ua { ub - ua } else { 1.0 };
        (
            (a..=b).map(|i| self.point(i)).collect(),
            (a..=b)
                .map(|i| ((self.source_at(i) - ua) / width).clamp(0.0, 1.0))
                .collect(),
        )
    }

    /// The exact source curve between two samples: whole segments where both ends lie
    /// outside, split where a span begins or ends inside one. This is what a span falls
    /// back to, so falling back costs fidelity nothing at all.
    fn source(&self, lo: usize, hi: usize) -> Vec<Segment> {
        let (ua, ub) = (self.source_at(lo), self.source_at(hi));
        let mut out = Vec::new();
        let mut k = ua.floor() as usize;
        while (k as f64) < ub - 1e-12 {
            let t0 = (ua - k as f64).max(0.0);
            let t1 = (ub - k as f64).min(1.0);
            if t1 - t0 > 1e-9 {
                out.push(self.run[k % self.run.len()].piece(t0, t1).segment());
            }
            k += 1;
        }
        out
    }

    /// Which sample each fitted segment ends on. The program places its vertices on the
    /// points it was given, so the nearest sample to each end is that vertex; the walk is
    /// forward-only, and the last end is the span's end by construction. `None` when the
    /// ends do not march through the span -- then the caller does not trust the fit.
    fn locate(&self, curve: &FittedPath, lo: usize, hi: usize, looped: bool) -> Option<Vec<usize>> {
        let nearest = |from: usize, to: usize, p: Point| {
            (from..=to).min_by(|&x, &y| self.point(x).dist(p).total_cmp(&self.point(y).dist(p)))
        };
        let first = if looped {
            nearest(lo, hi - 1, curve.start)?
        } else {
            lo
        };
        let last = first + (hi - lo);
        let mut ends = Vec::with_capacity(curve.segments.len() + 1);
        ends.push(first);
        let mut i = first;
        for (k, seg) in curve.segments.iter().enumerate() {
            let j = if k + 1 == curve.segments.len() {
                last
            } else {
                nearest(i + 1, last, seg.end())?
            };
            if j <= i || j > last {
                return None;
            }
            ends.push(j);
            i = j;
        }
        (i == last).then_some(ends)
    }

    /// Samples `lo..=hi` described within the tolerance: the start point and the segments.
    ///
    /// The program fits the span at `caps[0]` points. Each fitted segment is then judged
    /// against the samples it stands for; one that strays is fitted again on its own at
    /// the next cap -- a short span, where the square in the cost is cheap -- and past the
    /// last cap it is replaced by the exact source it covers.
    ///
    /// Judging the run as a whole instead is what the first version did, and on the
    /// corpus's heaviest file it threw away 62 runs entirely for one bad segment each.
    fn fit(&self, lo: usize, hi: usize, looped: bool, caps: &[usize]) -> (Point, Vec<Segment>) {
        let count = if looped { hi - lo } else { hi - lo + 1 };
        let exact = || (self.point(lo), self.source(lo, hi));
        let Some((&cap, rest)) = caps.split_first() else {
            return exact();
        };
        if count < 4 {
            return exact();
        }
        let points: Vec<Point> = (lo..lo + count).map(|i| self.point(i)).collect();
        let poly = Polyline::new(points, vec![self.eps; count], looped);
        let curve =
            inkvec_fit::multimodel::with_dp_max_points(cap, || optimal_multimodel(&poly, self.cfg));
        // Shown every point it has, the next cap would see exactly the same curve.
        let exhausted = count <= cap;
        let Some(ends) = self.locate(&curve, lo, hi, looped) else {
            return if exhausted {
                exact()
            } else {
                self.fit(lo, hi, looped, rest)
            };
        };
        let mut out = Vec::with_capacity(curve.segments.len());
        let mut start = curve.start;
        for (k, seg) in curve.segments.iter().enumerate() {
            let (a, b) = (ends[k], ends[k + 1]);
            let (span, along) = self.span(a, b);
            let seg = refit_cubic(seg, start, &span, along);
            let dev = span
                .iter()
                .map(|&p| dist_to_segment(p, &seg, start))
                .fold(0.0, f64::max);
            // The tolerance is a promise to the reader, not a suggestion to the fitter.
            if dev <= 3.0 * self.eps {
                out.push(seg.clone());
            } else if exhausted && b - a >= count - 1 {
                out.extend(self.source(a, b));
            } else {
                out.extend(self.fit(a, b, false, rest).1);
            }
            start = seg.end();
        }
        (curve.start, out)
    }

    /// How far the fitted curve wanders from the samples it claims to describe.
    ///
    /// Measuring the other direction -- every sample close to the curve -- is not the same
    /// promise and does not imply this one: a cubic can bow right out of the drawing and
    /// come back, and every sample will still find some part of it nearby. Merging two
    /// segments into one is exactly where that happens, and unchecked it cost 0.68 dE00 on
    /// a corpus the rest of this tool keeps under 0.01.
    fn wanders(&self, seg: &Segment, from: Point, a: usize, b: usize) -> f64 {
        let m = 16;
        (0..=m)
            .map(|i| {
                let p = eval_segment(seg, from, f64::from(i) / f64::from(m));
                (a..b)
                    .map(|j| point_segment_dist(p, self.point(j), self.point(j + 1)))
                    .fold(f64::INFINITY, f64::min)
            })
            .fold(0.0, f64::max)
    }

    /// Merge neighbouring segments into one cubic while it pays, and while the merge stays
    /// inside the tolerance.
    ///
    /// The program calls its own answer optimal, and it is -- for its alphabet, where an
    /// arc costs three parameters and every cubic six. SVG is not that alphabet: an arc is
    /// five numbers there, and a cubic continuing the one before it smoothly is four. So a
    /// description that is optimal to *fit* can still be cheaper to *write*, and this says
    /// so in the only terms that matter here. One cubic exactly subdivided in two comes
    /// back as two arcs and a cubic, sixteen numbers for a curve that needs six.
    fn merge_pairs(&self, path: FittedPath, lo: usize, hi: usize) -> FittedPath {
        if path.segments.len() < 2 {
            return path;
        }
        let debug = std::env::var_os("INKVEC_SVGMIN_DEBUG").is_some();
        let Some(mut ends) = self.locate(&path, lo, hi, path.closed) else {
            if debug {
                eprintln!("    merge: cannot locate {} segments", path.segments.len());
            }
            return path;
        };
        let FittedPath {
            start,
            mut segments,
            closed,
        } = path;
        loop {
            let mut heads = Vec::with_capacity(segments.len());
            let mut cur = start;
            for s in &segments {
                heads.push(cur);
                cur = s.end();
            }
            let mut candidates: Vec<(usize, Segment, f64)> = Vec::new();
            let here = params_of(&segments);
            for k in 0..segments.len() - 1 {
                let (a, b) = (ends[k], ends[k + 2]);
                let (p0, p1) = (heads[k], segments[k + 1].end());
                let (span, along) = self.span(a, b);
                if span.len() < 4 {
                    continue; // too few witnesses to hold a cubic to anything
                }
                let Some((c1, c2)) = ls_cubic_from(&span, p0, p1, along) else {
                    continue;
                };
                let merged = Segment::Cubic(c1, c2, p1);
                let dev = span
                    .iter()
                    .map(|&p| dist_to_segment(p, &merged, p0))
                    .fold(0.0, f64::max);
                if dev > 3.0 * self.eps {
                    continue;
                }
                let mut trial = segments.clone();
                trial.splice(k..=k + 1, [merged.clone()]);
                let saved = here - params_of(&trial);
                if saved > 0.0 {
                    candidates.push((k, merged, saved));
                }
            }
            // The expensive half of the promise is checked on the best candidate first,
            // and only until one keeps it: sweeping the whole curve for every pair, every
            // round, cost more than the fit it was checking.
            candidates.sort_by(|a, b| b.2.total_cmp(&a.2));
            let taken = candidates.into_iter().find(|(k, merged, _)| {
                let (a, b) = (ends[*k], ends[*k + 2]);
                let wander = self.wanders(merged, heads[*k], a, b);
                if debug {
                    eprintln!(
                        "    merge {k}+{}: samples {a}..{b}, wander {wander:.5}, eps {:.5}",
                        k + 1,
                        self.eps
                    );
                }
                wander <= 3.0 * self.eps
            });
            let Some((k, merged, _)) = taken else {
                break;
            };
            segments.splice(k..=k + 1, [merged]);
            ends.remove(k + 1);
        }
        FittedPath {
            start,
            segments,
            closed,
        }
    }
}

/// The cheapest description of one run within `eps`, or `None` when the source is already
/// as cheap (the caller then keeps the source exactly as it was written).
fn fit_run(
    run: &[Src],
    eps: f64,
    closed: bool,
    cfg: &FitConfig,
) -> Option<(FittedPath, Option<PrimitiveKind>)> {
    // The program decides structure; the exact refit and the per-segment guard decide
    // fidelity. Both see the curve sampled two tolerances apart.
    let (pts, at) = sample_mapped(run, 2.0 * eps, closed);
    let n = pts.len();
    if n < 4 {
        return None;
    }
    let source: Vec<Segment> = run.iter().map(Src::segment).collect();
    let before = params_of(&source);
    let fit = RunFit {
        run,
        pts,
        at,
        eps,
        cfg,
    };
    let (lo, hi) = if closed { (0, n) } else { (0, n - 1) };

    // The program costs O(points x span) and its early cut-off never fires on smooth,
    // exact input, so how many points it is shown decides what the run costs to fit. A
    // run a document would actually contain is shown all of them. Only the pathological
    // ones -- a 1,200-segment emoji outline -- are shown at most `COARSE_CAP`, decimated
    // by bends so their corners survive, and repaired span by span afterwards.
    //
    // Decimating is not free: it places the segment *boundaries* by a coarse view of the
    // curve, and repairing a boundary is not something the repair can do. Measured on the
    // 40-file batch, fitting whole where it is affordable is worth 1.6% of the whole
    // description.
    let coarse_cap = knob("INKVEC_SVGMIN_COARSE_CAP", COARSE_CAP as f64) as usize;
    let whole_below = knob("INKVEC_SVGMIN_WHOLE_BELOW", WHOLE_BELOW as f64) as usize;
    let (mut start, mut segments) = fit.fit(lo, hi, closed, &[coarse_cap, FINE_CAP]);
    if n > coarse_cap && n <= whole_below {
        // Neither view dominates: the coarse one can place a boundary where the whole
        // view would not, and the other way about. Both are within the tolerance, so the
        // cheaper one is simply the answer.
        let (s, whole) = fit.fit(lo, hi, closed, &[FINE_CAP]);
        if params_of(&whole) < params_of(&segments) {
            start = s;
            segments = whole;
        }
    }
    if knob("INKVEC_SVGMIN_MERGE", 1.0) > 0.0 {
        let path = fit.merge_pairs(
            FittedPath {
                start,
                segments,
                closed,
            },
            lo,
            hi,
        );
        start = path.start;
        segments = path.segments;
    }

    // The program was offered an arc per span; this offers one description of the whole
    // run, which is how a circle is found at all. It is shown a thinned copy of the
    // points, costing O(n²) as well, and judged the way everything here is judged: by what
    // it costs to write, within the same guard.
    //
    // Not by the fitter's own objective, which was the first draft's mistake: once the
    // cubics are repaired span by span they fit so well that `0.5·chi² + λ·params` prefers
    // them to four arcs that cost a third as much and are inside the tolerance.
    let mut primitive = None;
    if !segments.is_empty() && (closed || knob("INKVEC_SVGMIN_PRIM_OPEN", 1.0) > 0.0) {
        let stride = n.div_ceil(knob("INKVEC_SVGMIN_PRIM_CAP", PRIMITIVE_CAP as f64) as usize);
        let thin: Vec<Point> = fit.pts.iter().copied().step_by(stride.max(1)).collect();
        let sigma = vec![eps; thin.len()];
        if let Some((segs, prim, _)) = fit_primitive_or_arcs(&thin, &sigma, closed, cfg) {
            let candidate = FittedPath {
                start: fit.pts[0],
                segments: segs,
                closed,
            };
            // A whole primitive is written as its own element, cheaper than the path form
            // counted here, so it wins a tie.
            let cost = params_of(&candidate.segments);
            let win = if prim.is_some() {
                cost <= params_of(&segments)
            } else {
                cost < params_of(&segments)
            };
            if win && max_deviation(&fit.pts, &candidate) <= 3.0 * eps {
                primitive = prim.map(|p| p.kind);
                start = candidate.start;
                segments = candidate.segments;
            }
        }
    }

    let after = params_of(&segments);
    if std::env::var_os("INKVEC_SVGMIN_DEBUG").is_some() {
        let kinds: String = segments
            .iter()
            .map(|s| match s {
                Segment::Line(_) => 'L',
                Segment::Cubic(..) => 'C',
                Segment::Arc { .. } => 'A',
            })
            .collect();
        eprintln!(
            "  run: {} src segs, {n} samples, eps {eps:.5}, closed {closed} -> {} segs {kinds} params {before:.0}->{after:.0}",
            run.len(),
            segments.len(),
        );
    }
    if segments.is_empty() || after >= before {
        return None;
    }
    Some((
        FittedPath {
            start,
            segments,
            closed,
        },
        primitive,
    ))
}

/// A closed subpath of four axis-aligned lines is a `<rect>`, whatever the fitter would
/// make of it: its corners are exact, so nothing is fitted -- it is read off the source.
fn sharp_rect(sp: &Subpath, eps: f64) -> Option<PrimitiveKind> {
    if !sp.closed || sp.segs.len() != 4 {
        return None;
    }
    let mut xs = Vec::with_capacity(4);
    let mut ys = Vec::with_capacity(4);
    for s in &sp.segs {
        let Src::Line(a, b) = *s else {
            return None;
        };
        if (a.x - b.x).abs() > eps && (a.y - b.y).abs() > eps {
            return None; // not axis-aligned
        }
        xs.push(a.x);
        ys.push(a.y);
    }
    let (x0, x1) = (
        xs.iter().cloned().fold(f64::INFINITY, f64::min),
        xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
    );
    let (y0, y1) = (
        ys.iter().cloned().fold(f64::INFINITY, f64::min),
        ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
    );
    // Every vertex on the box's corners, and both widths used: a real rectangle, not a
    // degenerate zigzag.
    let on_corner = xs.iter().zip(&ys).all(|(&x, &y)| {
        ((x - x0).abs() <= eps || (x - x1).abs() <= eps)
            && ((y - y0).abs() <= eps || (y - y1).abs() <= eps)
    });
    (on_corner && x1 - x0 > eps && y1 - y0 > eps).then_some(PrimitiveKind::RoundRect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
        rx: 0.0,
    })
}

/// One subpath rewritten: `(start, segments, guarded_runs, primitive)`, the primitive
/// being `Some` when one element describes the whole subpath; `None` if nothing in it
/// got cheaper.
fn minify_subpath(
    sp: &Subpath,
    eps: f64,
    cfg: &FitConfig,
    corner_degrees: f64,
) -> Option<(Point, Vec<Segment>, usize, Option<PrimitiveKind>)> {
    let n = sp.segs.len();
    if n == 0 {
        return None;
    }
    if let Some(rect) = sharp_rect(sp, eps) {
        let segs: Vec<Segment> = sp.segs.iter().map(Src::segment).collect();
        return Some((sp.segs[0].start(), segs, 0, Some(rect)));
    }
    let is_corner = corners(sp, corner_degrees);
    let n_corners = is_corner.iter().filter(|&&c| c).count();

    // A closed loop with no corner is fitted whole: the dynamic program places its own
    // vertices and a circle or rounded rectangle can describe all of it.
    if sp.closed && n_corners == 0 {
        return fit_run(&sp.segs, eps, true, cfg).map(|(f, prim)| (f.start, f.segments, 0, prim));
    }

    // Otherwise the corners cut the subpath into runs, each refitted on its own with both
    // ends pinned. A closed subpath is rotated to begin at a corner so no run wraps.
    let first = is_corner.iter().position(|&c| c).unwrap_or(0);
    let order: Vec<usize> = (0..n).map(|i| (first + i) % n).collect();
    let mut runs: Vec<Vec<Src>> = Vec::new();
    for (i, &k) in order.iter().enumerate() {
        if i == 0 || is_corner[k] {
            runs.push(Vec::new());
        }
        runs.last_mut().expect("a run was opened").push(sp.segs[k]);
    }

    let mut improved = false;
    let mut guarded = 0;
    let mut segments = Vec::new();
    for run in &runs {
        match fit_run(run, eps, false, cfg) {
            Some((f, _)) => {
                improved = true;
                segments.extend(f.segments);
            }
            None => {
                if run.iter().map(Src::params).sum::<f64>() > 0.0 && run.len() > 1 {
                    guarded += 1;
                }
                segments.extend(run.iter().map(Src::segment));
            }
        }
    }
    improved.then(|| (runs[0][0].start(), segments, guarded, None))
}

fn fmt_num(v: f64, decimals: usize, out: &mut String) {
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        &s
    };
    let s = if s == "-0" { "0" } else { s };
    out.push_str(s);
}

fn push_pair(p: Point, decimals: usize, out: &mut String) {
    fmt_num(p.x, decimals, out);
    out.push(',');
    fmt_num(p.y, decimals, out);
}

/// Serialise one subpath as absolute commands, `S` where a cubic continues the previous
/// one smoothly.
fn fmt_subpath(start: Point, segs: &[Segment], closed: bool, decimals: usize, d: &mut String) {
    d.push('M');
    push_pair(start, decimals, d);
    let mut prev_c2: Option<(Point, Point)> = None;
    for seg in segs {
        match *seg {
            Segment::Line(p) => {
                d.push('L');
                push_pair(p, decimals, d);
                prev_c2 = None;
            }
            Segment::Cubic(c1, c2, p) => {
                let smooth = prev_c2.is_some_and(|(pc2, pp)| {
                    Point::new(2.0 * pp.x - pc2.x, 2.0 * pp.y - pc2.y).dist(c1) < 5e-4
                });
                if smooth {
                    d.push('S');
                } else {
                    d.push('C');
                    push_pair(c1, decimals, d);
                    d.push(' ');
                }
                push_pair(c2, decimals, d);
                d.push(' ');
                push_pair(p, decimals, d);
                prev_c2 = Some((c2, p));
            }
            Segment::Arc {
                rx,
                ry,
                phi,
                large_arc,
                sweep,
                end,
            } => {
                d.push('A');
                fmt_num(rx, decimals, d);
                d.push(',');
                fmt_num(ry, decimals, d);
                d.push(' ');
                fmt_num(phi.to_degrees(), 3, d);
                d.push_str(&format!(" {} {} ", u8::from(large_arc), u8::from(sweep)));
                push_pair(end, decimals, d);
                prev_c2 = None;
            }
        }
    }
    if closed {
        d.push('Z');
    }
}

/// The element that states a whole-subpath primitive: its tag, its geometry attributes,
/// and what they cost in numbers. A rotated ellipse would need a `transform` that could
/// collide with the element's own, so it stays a path.
fn primitive_element(kind: &PrimitiveKind, decimals: usize) -> Option<(&'static str, String, f64)> {
    let mut a = String::new();
    let mut attr = |name: &str, v: f64| {
        if !a.is_empty() {
            a.push(' ');
        }
        a.push_str(name);
        a.push_str("=\"");
        fmt_num(v, decimals, &mut a);
        a.push('"');
    };
    match *kind {
        PrimitiveKind::Circle { c, r } => {
            attr("cx", c.x);
            attr("cy", c.y);
            attr("r", r);
            Some(("circle", a, 3.0))
        }
        PrimitiveKind::Ellipse { c, rx, ry, angle } => {
            let a0 = angle.rem_euclid(std::f64::consts::PI);
            let (rx, ry) = if a0 < 1e-3 || a0 > std::f64::consts::PI - 1e-3 {
                (rx, ry)
            } else if (a0 - std::f64::consts::FRAC_PI_2).abs() < 1e-3 {
                (ry, rx)
            } else {
                return None;
            };
            attr("cx", c.x);
            attr("cy", c.y);
            attr("rx", rx);
            attr("ry", ry);
            Some(("ellipse", a, 4.0))
        }
        PrimitiveKind::RoundRect { x, y, w, h, rx } => {
            attr("x", x);
            attr("y", y);
            attr("width", w);
            attr("height", h);
            let mut cost = 4.0;
            if rx > 0.0 {
                attr("rx", rx);
                cost += 1.0;
            }
            Some(("rect", a, cost))
        }
    }
}

/// The longer side of the drawing in its own units: from `viewBox`, else `width`/`height`.
fn extent(root: roxmltree::Node) -> Option<f64> {
    if let Some(vb) = root.attribute("viewBox") {
        let v: Vec<f64> = vb
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();
        if v.len() == 4 && v[2] > 0.0 && v[3] > 0.0 {
            return Some(v[2].max(v[3]));
        }
    }
    let num = |a: &str| -> Option<f64> {
        root.attribute(a)?
            .trim_end_matches(|c: char| c.is_alphabetic() || c == '%')
            .parse()
            .ok()
    };
    match (num("width"), num("height")) {
        (Some(w), Some(h)) if w > 0.0 && h > 0.0 => Some(w.max(h)),
        _ => None,
    }
}

/// The uniform scale a node's accumulated `transform` applies: `sqrt(|det|)`. A tolerance
/// stated on the page has to be divided by this to hold in the path's own coordinates.
fn node_scale(node: roxmltree::Node) -> f64 {
    let mut m = [1.0, 0.0, 0.0, 1.0];
    for anc in node.ancestors() {
        let Some(t) = anc.attribute("transform") else {
            continue;
        };
        for tok in svgtypes::TransformListParser::from(t).flatten() {
            use svgtypes::TransformListToken as T;
            let (a, b, c, d) = match tok {
                T::Matrix { a, b, c, d, .. } => (a, b, c, d),
                T::Translate { .. } => continue,
                T::Scale { sx, sy } => (sx, 0.0, 0.0, sy),
                T::Rotate { angle } => {
                    let (s, co) = angle.to_radians().sin_cos();
                    (co, s, -s, co)
                }
                T::SkewX { angle } => (1.0, 0.0, angle.to_radians().tan(), 1.0),
                T::SkewY { angle } => (1.0, angle.to_radians().tan(), 0.0, 1.0),
            };
            m = [
                m[0] * a + m[2] * b,
                m[1] * a + m[3] * b,
                m[0] * c + m[2] * d,
                m[1] * c + m[3] * d,
            ];
        }
    }
    let det = (m[0] * m[3] - m[1] * m[2]).abs();
    if det > 1e-12 {
        det.sqrt()
    } else {
        1.0
    }
}

/// What a `d` attribute costs as written: segments, and numbers the reader has to store.
/// `S` and `T` are cheaper than the cubic or quadratic they restate, `H` and `V` cheaper
/// than a line, and an arc's two flags are not coordinates. This, not the fitter's
/// alphabet, is the description length a rewrite has to beat.
fn text_cost(d: &str) -> (usize, f64) {
    use svgtypes::PathSegment as P;
    let (mut segs, mut params) = (0usize, 0.0);
    for seg in svgtypes::PathParser::from(d).flatten() {
        let cost = match seg {
            P::MoveTo { .. } => {
                params += 2.0;
                continue;
            }
            P::ClosePath { .. } => continue,
            P::LineTo { .. } | P::SmoothQuadratic { .. } => 2.0,
            P::HorizontalLineTo { .. } | P::VerticalLineTo { .. } => 1.0,
            P::CurveTo { .. } => 6.0,
            P::SmoothCurveTo { .. } | P::Quadratic { .. } => 4.0,
            P::EllipticalArc { .. } => 5.0,
        };
        segs += 1;
        params += cost;
    }
    (segs, params)
}

/// One `<path>` element, read out of the document so it can be fitted on any thread.
struct Job {
    d: String,
    d_range: Range<usize>,
    quote: char,
    node_range: Range<usize>,
    /// The element's other attributes, as written, when it can be replaced whole.
    other_attrs: Option<Vec<String>>,
    scale: f64,
}

/// What became of one path: the text to splice in, if any, and its share of the report.
struct Outcome {
    edit: Option<(Range<usize>, String)>,
    delta: Report,
}

/// Fit one path and decide, as text against text, whether the rewrite is kept.
fn rewrite_path(job: &Job, eps_units: f64, ext: f64, decimals: usize, opts: &Options) -> Outcome {
    let mut delta = Report {
        paths: 1,
        ..Default::default()
    };
    let (segs0, params0) = text_cost(&job.d);
    delta.segments_before = segs0;
    delta.params_before = params0;
    let keep = |mut delta: Report| {
        delta.segments_after = segs0;
        delta.params_after = params0;
        Outcome { edit: None, delta }
    };
    let Ok(subpaths) = parse_d(&job.d) else {
        return keep(delta); // leave what we cannot read exactly as it is
    };
    let eps = eps_units / job.scale;
    let cfg = FitConfig::from_precision(ext / job.scale, eps, 2.0);

    let mut d = String::new();
    let mut any = false;
    let mut guarded = 0;
    let mut whole: Option<PrimitiveKind> = None;
    for sp in &subpaths {
        delta.subpaths += 1;
        match minify_subpath(sp, eps, &cfg, opts.corner_degrees) {
            Some((start, segs, g, prim)) => {
                any = true;
                guarded += g;
                if subpaths.len() == 1 {
                    whole = prim;
                }
                fmt_subpath(start, &segs, sp.closed, decimals, &mut d);
            }
            None => {
                let segs: Vec<Segment> = sp.segs.iter().map(Src::segment).collect();
                fmt_subpath(sp.segs[0].start(), &segs, sp.closed, decimals, &mut d);
            }
        }
    }
    // A path that is one whole primitive becomes the element that says so -- a
    // `<circle>` is three numbers where its path form is seventeen -- with every
    // other attribute carried over byte for byte.
    if let (Some((tag, attrs, cost)), Some(others)) = (
        whole.and_then(|k| primitive_element(&k, decimals)),
        &job.other_attrs,
    ) {
        if cost < params0 {
            let mut el = format!("<{tag}");
            for a in others {
                el.push(' ');
                el.push_str(a);
            }
            el.push(' ');
            el.push_str(&attrs);
            el.push_str("/>");
            delta.rewritten = 1;
            delta.primitives = 1;
            delta.guarded = guarded;
            delta.segments_after = 1;
            delta.params_after = cost;
            return Outcome {
                edit: Some((job.node_range.clone(), el)),
                delta,
            };
        }
    }
    // The fitter counts a cubic as six numbers whatever the source wrote; the source may
    // have written it as an `S` in four. So the rewrite is judged as text against text,
    // and a path that is not cheaper as written keeps its original bytes.
    let (segs1, params1) = text_cost(&d);
    if any && params1 < params0 {
        delta.rewritten = 1;
        delta.guarded = guarded;
        delta.segments_after = segs1;
        delta.params_after = params1;
        let q = job.quote;
        Outcome {
            edit: Some((job.d_range.clone(), format!("d={q}{d}{q}"))),
            delta,
        }
    } else {
        keep(delta)
    }
}

/// Rewrite every `<path d>` in `svg` as its cheapest description within the tolerance.
pub fn minify(svg: &str, opts: &Options) -> Result<(String, Report), String> {
    let doc = roxmltree::Document::parse(svg).map_err(|e| format!("not an SVG document: {e}"))?;
    let root = doc.root_element();
    let ext = extent(root).ok_or("the SVG has no usable viewBox or width/height")?;
    let eps_units = opts.tolerance_px * ext / opts.judge;
    let decimals = opts
        .decimals
        .unwrap_or_else(|| ((1.0 / (0.5 * eps_units)).log10().ceil().max(0.0) as usize).min(6));
    let mut rep = Report {
        tolerance_units: eps_units,
        ..Default::default()
    };

    // Every path is independent, so they are fitted on every core, as the tracer fits its
    // boundaries: the work per path varies by orders of magnitude, which is the shape of
    // problem rayon's work stealing handles. The document is read once, up front, into
    // plain jobs; the results are merged in document order afterwards.
    let jobs: Vec<Job> = doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "path")
        .filter_map(|node| {
            let attr = node.attributes().find(|a| a.name() == "d")?;
            let text = &svg[node.range()];
            let replaceable = !node.has_children() && text.trim_end().ends_with("/>");
            Some(Job {
                d: attr.value().to_string(),
                d_range: attr.range(),
                quote: svg[attr.range()].chars().next_back().unwrap_or('"'),
                node_range: node.range(),
                other_attrs: replaceable.then(|| {
                    node.attributes()
                        .filter(|a| a.name() != "d")
                        .map(|a| svg[a.range()].to_string())
                        .collect()
                }),
                scale: node_scale(node),
            })
        })
        .collect();

    use rayon::prelude::*;
    let outcomes: Vec<Outcome> = jobs
        .par_iter()
        .map(|job| rewrite_path(job, eps_units, ext, decimals, opts))
        .collect();

    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for o in outcomes {
        let r = &o.delta;
        rep.paths += r.paths;
        rep.rewritten += r.rewritten;
        rep.subpaths += r.subpaths;
        rep.segments_before += r.segments_before;
        rep.segments_after += r.segments_after;
        rep.params_before += r.params_before;
        rep.params_after += r.params_after;
        rep.guarded += r.guarded;
        rep.primitives += r.primitives;
        edits.extend(o.edit);
    }

    let mut out = svg.to_string();
    edits.sort_by_key(|e| std::cmp::Reverse(e.0.start));
    for (range, text) in edits {
        out.replace_range(range, &text);
    }
    Ok((out, rep))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn circle_as_cubics(cx: f64, cy: f64, r: f64, n: usize) -> String {
        // A circle as `n` cubic arcs, each exact to the usual 4/3·tan(θ/4) handle.
        let mut d = String::new();
        let step = std::f64::consts::TAU / n as f64;
        let k = 4.0 / 3.0 * (step / 4.0).tan() * r;
        for i in 0..n {
            let (a0, a1) = (i as f64 * step, (i + 1) as f64 * step);
            let p0 = (cx + r * a0.cos(), cy + r * a0.sin());
            let p3 = (cx + r * a1.cos(), cy + r * a1.sin());
            let c1 = (p0.0 - k * a0.sin(), p0.1 + k * a0.cos());
            let c2 = (p3.0 + k * a1.sin(), p3.1 - k * a1.cos());
            if i == 0 {
                d.push_str(&format!("M{},{}", p0.0, p0.1));
            }
            d.push_str(&format!(
                "C{},{} {},{} {},{}",
                c1.0, c1.1, c2.0, c2.1, p3.0, p3.1
            ));
        }
        d.push('Z');
        d
    }

    fn doc(d: &str) -> String {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 128 128\">\
             <path id=\"keep\" fill=\"#c9754a\" d=\"{d}\"/></svg>"
        )
    }

    #[test]
    fn an_oversegmented_circle_gets_cheaper_and_stays_a_circle() {
        let (out, rep) = minify(
            &doc(&circle_as_cubics(64.0, 64.0, 40.0, 16)),
            &Options::default(),
        )
        .expect("minifies");
        assert_eq!(rep.paths, 1);
        assert_eq!(rep.rewritten, 1);
        assert_eq!(rep.primitives, 1, "{rep:?}");
        assert_eq!(rep.params_after, 3.0, "{rep:?}");
        // The element that says so, with the path's other attributes carried over.
        let doc = roxmltree::Document::parse(&out).unwrap();
        let el = doc
            .descendants()
            .find(|n| n.tag_name().name() == "circle")
            .unwrap_or_else(|| panic!("no <circle> in {out}"));
        assert_eq!(el.attribute("fill"), Some("#c9754a"));
        assert_eq!(el.attribute("id"), Some("keep"));
        assert!(el.attribute("d").is_none());
        let num = |a: &str| el.attribute(a).unwrap().parse::<f64>().unwrap();
        let eps = rep.tolerance_units;
        assert!(
            (num("cx") - 64.0).abs() < eps && (num("cy") - 64.0).abs() < eps,
            "{out}"
        );
        assert!((num("r") - 40.0).abs() < eps, "{out}");
    }

    #[test]
    fn the_nearest_segment_lookup_matches_brute_force() {
        let subs = parse_d(&circle_as_cubics(64.0, 64.0, 40.0, 16)).unwrap();
        let eps = 0.0125;
        let cfg = FitConfig::from_precision(128.0, eps, 2.0);
        let pts = sample(&subs[0].segs, 2.0 * eps, true);
        let poly = Polyline::new(pts.clone(), vec![eps; pts.len()], true);
        let (segs, _, _) =
            fit_primitive_or_arcs(&poly.points, &poly.sigma, true, &cfg).expect("arcs");
        let fitted = FittedPath {
            start: pts[0],
            segments: segs,
            closed: true,
        };
        let curve = Curve::new(&fitted);
        let mut starts = Vec::new();
        let mut cur = fitted.start;
        for s in &fitted.segments {
            starts.push(cur);
            cur = s.end();
        }
        for p in sample(&subs[0].segs, 0.5, true) {
            let brute = fitted
                .segments
                .iter()
                .zip(&starts)
                .map(|(s, &st)| dist_to_segment(p, s, st))
                .fold(f64::INFINITY, f64::min);
            let (k, fast) = curve.nearest(p);
            assert!(
                (fast - brute).abs() < 1e-6,
                "point {p:?}: fast {fast} via segment {k}, brute {brute}; segments {}",
                fitted.segments.len()
            );
        }
    }

    /// A thin sliver of long segments: the far side's table sample is nearer to a point
    /// than the near side's, and a lookup that trusts the table reads the sliver's width
    /// (0.2) as the distance to a path the point lies on.
    #[test]
    fn the_nearest_lookup_is_not_fooled_by_the_far_side_of_a_thin_shape() {
        let fitted = FittedPath {
            start: Point::new(0.0, 0.0),
            segments: vec![
                Segment::Line(Point::new(100.0, 0.0)),
                Segment::Line(Point::new(100.0, 0.2)),
                // The far side's samples are offset by half a gap from the near side's.
                Segment::Line(Point::new(-6.25, 0.2)),
                Segment::Line(Point::new(0.0, 0.0)),
            ],
            closed: true,
        };
        let curve = Curve::new(&fitted);
        for i in 0..=1000 {
            let p = Point::new(f64::from(i) * 0.1, 0.0);
            let (k, d) = curve.nearest(p);
            assert!(
                d < 1e-9,
                "point {p:?} is on the path; lookup says {d} via segment {k}"
            );
        }
    }

    #[test]
    fn a_sharp_rectangle_becomes_a_rect_element() {
        let (out, rep) =
            minify(&doc("M10,20L100,20L100,80L10,80Z"), &Options::default()).expect("minifies");
        assert_eq!(rep.primitives, 1, "{rep:?}\n{out}");
        assert_eq!(rep.params_after, 4.0);
        assert!(out.contains("<rect"), "{out}");
        assert!(
            out.contains("x=\"10\"") && out.contains("y=\"20\""),
            "{out}"
        );
        assert!(
            out.contains("width=\"90\"") && out.contains("height=\"60\""),
            "{out}"
        );
        assert!(
            out.contains("fill=\"#c9754a\"") && out.contains("id=\"keep\""),
            "{out}"
        );
        // A rectangle that is not axis-aligned stays a path.
        let (_, rep) =
            minify(&doc("M10,20L100,30L90,80L0,70Z"), &Options::default()).expect("minifies");
        assert_eq!(rep.primitives, 0, "{rep:?}");
    }

    #[test]
    fn a_square_keeps_its_four_corners_exactly() {
        // Each edge drawn as three collinear pieces: twelve lines that should become four.
        let mut d = String::from("M10,10");
        for (x, y) in [
            (40.0, 10.0),
            (70.0, 10.0),
            (100.0, 10.0),
            (100.0, 40.0),
            (100.0, 70.0),
            (100.0, 100.0),
            (70.0, 100.0),
            (40.0, 100.0),
            (10.0, 100.0),
            (10.0, 70.0),
            (10.0, 40.0),
        ] {
            d.push_str(&format!("L{x},{y}"));
        }
        d.push('Z');
        let (out, rep) = minify(&doc(&d), &Options::default()).expect("minifies");
        assert_eq!(rep.segments_after, 4, "{rep:?}\n{out}");
        for corner in ["10,10", "100,10", "100,100", "10,100"] {
            assert!(out.contains(corner), "corner {corner} lost in {out}");
        }
    }

    #[test]
    fn a_subdivided_cubic_collapses_to_one() {
        // One cubic split exactly in two by de Casteljau at t = 0.5.
        let (p0, c1, c2, p3) = ((10.0, 100.0), (30.0, 10.0), (90.0, 10.0), (110.0, 100.0));
        let mid = |a: (f64, f64), b: (f64, f64)| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let (q1, q2, q3) = (mid(p0, c1), mid(c1, c2), mid(c2, p3));
        let (r1, r2) = (mid(q1, q2), mid(q2, q3));
        let s = mid(r1, r2);
        let d = format!(
            "M{},{}C{},{} {},{} {},{}C{},{} {},{} {},{}",
            p0.0, p0.1, q1.0, q1.1, r1.0, r1.1, s.0, s.1, r2.0, r2.1, q3.0, q3.1, p3.0, p3.1
        );
        let (_, rep) = minify(&doc(&d), &Options::default()).expect("minifies");
        assert_eq!(rep.segments_after, 1, "{rep:?}");
    }

    #[test]
    fn least_squares_recovers_a_piece_of_an_exact_cubic() {
        // The same subdivided cubic the minifier sees, sampled as it samples it, with a
        // span that ends part-way through the second half.
        let (p0, c1, c2, p3) = ((10.0, 100.0), (30.0, 10.0), (90.0, 10.0), (110.0, 100.0));
        let mid = |a: (f64, f64), b: (f64, f64)| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let (q1, q2, q3) = (mid(p0, c1), mid(c1, c2), mid(c2, p3));
        let (r1, r2) = (mid(q1, q2), mid(q2, q3));
        let s = mid(r1, r2);
        let pt = |a: (f64, f64)| Point::new(a.0, a.1);
        let run = [
            Src::Cubic(pt(p0), pt(q1), pt(r1), pt(s)),
            Src::Cubic(pt(s), pt(r2), pt(q3), pt(p3)),
        ];
        let pts = sample(&run, 0.025, false);
        let span = &pts[..=38];
        let (a, b) = (span[0], span[span.len() - 1]);
        let along: Vec<f64> = (0..span.len()).map(|i| i as f64 / 38.0).collect();
        let (n1, n2) = ls_cubic_from(span, a, b, along).expect("fits");
        let seg = Segment::Cubic(n1, n2, b);
        let dev = span
            .iter()
            .map(|&p| dist_to_segment(p, &seg, a))
            .fold(0.0, f64::max);
        assert!(dev < 1e-3, "a piece of a cubic is a cubic; missed by {dev}");
    }

    #[test]
    fn least_squares_recovers_an_exact_cubic() {
        let (p0, c1, c2, p3) = (
            Point::new(10.0, 100.0),
            Point::new(30.0, 10.0),
            Point::new(90.0, 10.0),
            Point::new(110.0, 100.0),
        );
        let pts: Vec<Point> = (0..=96)
            .map(|i| cubic_at(p0, c1, c2, p3, i as f64 / 96.0))
            .collect();
        let along: Vec<f64> = (0..=96).map(|i| f64::from(i) / 96.0).collect();
        let (n1, n2) = ls_cubic_from(&pts, p0, p3, along).expect("solvable");
        let worst = pts
            .iter()
            .map(|&p| dist_to_segment(p, &Segment::Cubic(n1, n2, p3), p0))
            .fold(0.0, f64::max);
        assert!(worst < 1e-3, "c1 {n1:?} c2 {n2:?} worst {worst}");
    }

    #[test]
    fn a_path_that_cannot_get_cheaper_is_left_byte_for_byte() {
        let src = doc("M10,10L100,10L100,100Z");
        let (out, rep) = minify(&src, &Options::default()).expect("minifies");
        assert_eq!(rep.rewritten, 0);
        assert_eq!(out, src);
    }

    #[test]
    fn a_transform_scales_the_tolerance() {
        // Under a 10x scale, 0.1 px on the page is 0.01 units in the path.
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1280 1280\">\
                   <g transform=\"scale(10)\"><path d=\"M0,0L10,0L10,10Z\"/></g></svg>";
        let doc = roxmltree::Document::parse(svg).unwrap();
        let node = doc
            .descendants()
            .find(|n| n.tag_name().name() == "path")
            .unwrap();
        assert!((node_scale(node) - 10.0).abs() < 1e-12);
    }
}
