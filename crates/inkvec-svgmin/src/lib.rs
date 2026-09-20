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
    multimodel::{optimal_multimodel, path_cost},
    primitives::fit_primitive_or_arcs,
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
fn sample(run: &[Src], spacing: f64, drop_last: bool) -> Vec<Point> {
    let mut pts = vec![run[0].start()];
    for s in run {
        let n = ((s.rough_length() / spacing).ceil() as usize).clamp(4, 48);
        for i in 1..=n {
            pts.push(s.at(i as f64 / n as f64));
        }
    }
    if drop_last {
        pts.pop();
    }
    pts
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

/// Largest distance from any of `pts` to the fitted path.
fn max_deviation(pts: &[Point], path: &FittedPath) -> f64 {
    let mut starts = Vec::with_capacity(path.segments.len());
    let mut cur = path.start;
    for s in &path.segments {
        starts.push(cur);
        cur = s.end();
    }
    pts.iter()
        .map(|&p| {
            path.segments
                .iter()
                .zip(&starts)
                .map(|(s, &st)| dist_to_segment(p, s, st))
                .fold(f64::INFINITY, f64::min)
        })
        .fold(0.0, f64::max)
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
/// points solved linearly at chord-length parameters, then each parameter moved to its
/// point's nearest place on the new curve and the solve repeated until it settles
/// (Schneider, 1990, with an exact projection in place of his Newton step). The dynamic
/// program's cubic is a tangent-constrained approximation good enough to *choose* a
/// cubic; this is what makes the chosen cubic *exact* where the source was one.
fn ls_cubic(pts: &[Point], p0: Point, p3: Point) -> Option<(Point, Point)> {
    if pts.len() < 3 {
        return None;
    }
    let mut t: Vec<f64> = Vec::with_capacity(pts.len());
    let mut acc = 0.0;
    t.push(0.0);
    for w in pts.windows(2) {
        acc += w[0].dist(w[1]);
        t.push(acc);
    }
    if acc < 1e-12 {
        return None;
    }
    t.iter_mut().for_each(|v| *v /= acc);
    let (mut c1, mut c2) = (p0, p3);
    let mut last_err = f64::INFINITY;
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
        // that no longer brings the points closer.
        let mut err: f64 = 0.0;
        for (&p, tt) in pts.iter().zip(t.iter_mut()) {
            *tt = nearest_t_cubic(p, p0, c1, c2, p3);
            err = err.max(p.dist(cubic_at(p0, c1, c2, p3, *tt)));
        }
        if err > last_err - 1e-9 {
            break;
        }
        last_err = err;
    }
    (c1.x.is_finite() && c1.y.is_finite() && c2.x.is_finite() && c2.y.is_finite())
        .then_some((c1, c2))
}

/// Refit every cubic the dynamic program chose to the samples nearest it, keeping a refit
/// only where it brings those samples closer. The samples are assigned by proximity
/// rather than by the fitter's vertex indices, which refer to its own polygon.
fn refit_cubics(poly: &Polyline, path: &mut FittedPath) {
    let mut starts = Vec::with_capacity(path.segments.len());
    let mut cur = path.start;
    for s in &path.segments {
        starts.push(cur);
        cur = s.end();
    }
    let mut owned: Vec<Vec<Point>> = vec![Vec::new(); path.segments.len()];
    for &p in &poly.points {
        let mut best = (usize::MAX, f64::INFINITY);
        for (k, (s, &st)) in path.segments.iter().zip(&starts).enumerate() {
            let d = dist_to_segment(p, s, st);
            if d < best.1 {
                best = (k, d);
            }
        }
        if best.0 != usize::MAX {
            owned[best.0].push(p);
        }
    }
    for (k, seg) in path.segments.iter_mut().enumerate() {
        let start = starts[k];
        let end = seg.end();
        let Segment::Cubic(c1, c2, _) = seg.clone() else {
            continue;
        };
        let span = &owned[k];
        let Some((n1, n2)) = ls_cubic(span, start, end) else {
            continue;
        };
        let dev = |a: Point, b: Point| {
            span.iter()
                .map(|&p| dist_to_segment(p, &Segment::Cubic(a, b, end), start))
                .fold(0.0, f64::max)
        };
        let (before, after) = (dev(c1, c2), dev(n1, n2));
        if std::env::var_os("INKVEC_SVGMIN_DEBUG").is_some() {
            eprintln!(
                "    refit cubic {k}: {} samples, dev {before:.4} -> {after:.4}",
                span.len()
            );
        }
        if after < before {
            *seg = Segment::Cubic(n1, n2, end);
        }
    }
}

/// The cheapest description of one run within `eps`, or `None` when the source is already
/// as cheap or the fit strayed too far (the caller then keeps the source).
fn fit_run(run: &[Src], eps: f64, closed: bool, cfg: &FitConfig) -> Option<FittedPath> {
    let pts = sample(run, 2.0 * eps, closed);
    if pts.len() < 4 {
        return None;
    }
    let poly = Polyline::new(pts.clone(), vec![eps; pts.len()], closed);
    let mut curve = optimal_multimodel(&poly, cfg);
    refit_cubics(&poly, &mut curve);
    let fitted = match fit_primitive_or_arcs(&poly.points, &poly.sigma, closed, cfg) {
        Some((segs, _, cost)) if cost < path_cost(&poly, &curve, cfg) => FittedPath {
            start: poly.points[0],
            segments: segs,
            closed,
        },
        _ => curve,
    };
    if fitted.segments.is_empty() {
        return None;
    }
    let before: f64 = run.iter().map(Src::params).sum();
    let after: f64 = fitted.segments.iter().map(Segment::params).sum();
    let dev = max_deviation(&pts, &fitted);
    if std::env::var_os("INKVEC_SVGMIN_DEBUG").is_some() {
        let kinds: Vec<&str> = fitted
            .segments
            .iter()
            .map(|s| match s {
                Segment::Line(_) => "L",
                Segment::Cubic(..) => "C",
                Segment::Arc { .. } => "A",
            })
            .collect();
        eprintln!(
            "  run: {} src segs, {} samples, eps {eps:.5}, closed {closed} -> {} segs {} params {before:.0}->{after:.0} dev {dev:.5} ({:.1} eps)",
            run.len(),
            pts.len(),
            fitted.segments.len(),
            kinds.join(""),
            dev / eps
        );
    }
    if after >= before {
        return None;
    }
    // The tolerance is a promise to the reader, not a suggestion to the fitter.
    if dev > 3.0 * eps {
        return None;
    }
    Some(fitted)
}

/// One subpath rewritten: `(start, segments, closed, guarded_runs)`; `None` if nothing in
/// it got cheaper.
fn minify_subpath(
    sp: &Subpath,
    eps: f64,
    cfg: &FitConfig,
    corner_degrees: f64,
) -> Option<(Point, Vec<Segment>, usize)> {
    let n = sp.segs.len();
    if n == 0 {
        return None;
    }
    let is_corner = corners(sp, corner_degrees);
    let n_corners = is_corner.iter().filter(|&&c| c).count();

    // A closed loop with no corner is fitted whole: the dynamic program places its own
    // vertices and a circle or rounded rectangle can describe all of it.
    if sp.closed && n_corners == 0 {
        return fit_run(&sp.segs, eps, true, cfg).map(|f| (f.start, f.segments, 0));
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
            Some(f) => {
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
    improved.then(|| (runs[0][0].start(), segments, guarded))
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

    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for node in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "path")
    {
        let Some(attr) = node.attributes().find(|a| a.name() == "d") else {
            continue;
        };
        rep.paths += 1;
        let (segs0, params0) = text_cost(attr.value());
        rep.segments_before += segs0;
        rep.params_before += params0;
        let subpaths = match parse_d(attr.value()) {
            Ok(s) => s,
            Err(_) => {
                rep.segments_after += segs0;
                rep.params_after += params0;
                continue; // leave what we cannot read exactly as it is
            }
        };
        let eps = eps_units / node_scale(node);
        let cfg = FitConfig::from_precision(ext / node_scale(node), eps, 2.0);

        let mut d = String::new();
        let mut any = false;
        let mut guarded = 0;
        for sp in &subpaths {
            rep.subpaths += 1;
            match minify_subpath(sp, eps, &cfg, opts.corner_degrees) {
                Some((start, segs, g)) => {
                    any = true;
                    guarded += g;
                    fmt_subpath(start, &segs, sp.closed, decimals, &mut d);
                }
                None => {
                    let segs: Vec<Segment> = sp.segs.iter().map(Src::segment).collect();
                    fmt_subpath(sp.segs[0].start(), &segs, sp.closed, decimals, &mut d);
                }
            }
        }
        // The fitter counts a cubic as six numbers whatever the source wrote; the source
        // may have written it as an `S` in four. So the rewrite is judged as text against
        // text, and a path that is not cheaper as written keeps its original bytes.
        let (segs1, params1) = text_cost(&d);
        if any && params1 < params0 {
            rep.rewritten += 1;
            rep.guarded += guarded;
            rep.segments_after += segs1;
            rep.params_after += params1;
            let quote = svg[attr.range()].chars().next_back().unwrap_or('"');
            edits.push((attr.range(), format!("d={quote}{d}{quote}")));
        } else {
            rep.segments_after += segs0;
            rep.params_after += params0;
        }
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
        assert!(rep.segments_after < 16, "{rep:?}");
        assert!(rep.params_after < rep.params_before, "{rep:?}");
        assert!(out.contains("fill=\"#c9754a\"") && out.contains("id=\"keep\""));
        // Every point of the new outline is on the circle to within the tolerance.
        let subs = parse_d(
            roxmltree::Document::parse(&out)
                .unwrap()
                .descendants()
                .find(|n| n.tag_name().name() == "path")
                .unwrap()
                .attribute("d")
                .unwrap(),
        )
        .unwrap();
        let pts = sample(&subs[0].segs, 0.05, false);
        let worst = pts
            .iter()
            .map(|p| (p.dist(Point::new(64.0, 64.0)) - 40.0).abs())
            .fold(0.0, f64::max);
        assert!(
            worst < 3.0 * rep.tolerance_units,
            "worst {worst}, eps {}",
            rep.tolerance_units
        );
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
        let (n1, n2) = ls_cubic(&pts, p0, p3).expect("solvable");
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
