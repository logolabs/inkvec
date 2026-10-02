//! The source path: how a `d` attribute is read, and how points are taken
//! along it for the fitter to judge.

use crate::fit::PER_SEGMENT;
use inkvec_core::Point;
use inkvec_fit::curves::Segment;

/// One segment of a source path, absolute, with its start point.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Src {
    /// From the first point to the second.
    Line(Point, Point),
    /// Start, first control point, second control point, end.
    Cubic(Point, Point, Point, Point),
}

impl Src {
    /// Where the segment begins.
    pub(crate) fn start(&self) -> Point {
        match *self {
            Src::Line(a, _) | Src::Cubic(a, _, _, _) => a,
        }
    }
    /// What the segment costs in the fitter's parameter units: 2 for a line, 6 for a cubic.
    pub(crate) fn params(&self) -> f64 {
        match self {
            Src::Line(..) => 2.0,
            Src::Cubic(..) => 6.0,
        }
    }
    /// The same segment in the fitter's form, which stores only the end (the start is the
    /// previous segment's end).
    pub(crate) fn segment(&self) -> Segment {
        match *self {
            Src::Line(_, b) => Segment::Line(b),
            Src::Cubic(_, c1, c2, b) => Segment::Cubic(c1, c2, b),
        }
    }
    /// Direction of travel leaving the start; falls back along the control polygon when a
    /// control point sits on the endpoint (a cusp, or a line drawn as a cubic).
    pub(crate) fn tangent_out(&self) -> Option<Point> {
        match *self {
            Src::Line(a, b) => unit(sub(b, a)),
            Src::Cubic(a, c1, c2, b) => unit(sub(c1, a))
                .or_else(|| unit(sub(c2, a)))
                .or_else(|| unit(sub(b, a))),
        }
    }
    /// Direction of travel arriving at the end, with the same fallbacks from the other side.
    /// `None` for a segment of zero length.
    pub(crate) fn tangent_in(&self) -> Option<Point> {
        match *self {
            Src::Line(a, b) => unit(sub(b, a)),
            Src::Cubic(a, c1, c2, b) => unit(sub(b, c2))
                .or_else(|| unit(sub(b, c1)))
                .or_else(|| unit(sub(b, a))),
        }
    }
    /// The point at parameter `t ∈ [0, 1]`: linear interpolation for a line, the Bernstein
    /// form for a cubic.
    pub(crate) fn at(&self, t: f64) -> Point {
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
    ///
    /// For a cubic, two de Casteljau cuts: keep `[0, t1]`, then cut that at `t0 / t1` (where
    /// `t0` falls on the kept part's own parameter) and keep the second half. `0 ≤ t0 ≤ t1 ≤ 1`
    /// is assumed; the whole segment comes back unchanged when the range covers it.
    pub(crate) fn piece(&self, t0: f64, t1: f64) -> Src {
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
    pub(crate) fn rough_length(&self) -> f64 {
        match *self {
            Src::Line(a, b) => a.dist(b),
            Src::Cubic(a, c1, c2, b) => a.dist(c1) + c1.dist(c2) + c2.dist(b),
        }
    }
}
/// `a − b`, kept as a [`Point`] (this crate uses points for vectors too).
pub(crate) fn sub(a: Point, b: Point) -> Point {
    Point::new(a.x - b.x, a.y - b.y)
}

/// A cubic's four control points: start, first control, second control, end.
type Cubic = (Point, Point, Point, Point);
/// De Casteljau's construction: the cubic cut at `t`, as its two halves. Each level mixes
/// neighbouring points at ratio `t`; the three levels give the new control points and the
/// cut point `m`, which both halves share. Exact: the two halves trace the same curve.
fn split_cubic(a: Point, c1: Point, c2: Point, b: Point, t: f64) -> (Cubic, Cubic) {
    let mix = |p: Point, q: Point| Point::new(p.x + (q.x - p.x) * t, p.y + (q.y - p.y) * t);
    let (p01, p12, p23) = (mix(a, c1), mix(c1, c2), mix(c2, b));
    let (p012, p123) = (mix(p01, p12), mix(p12, p23));
    let m = mix(p012, p123);
    ((a, p01, p012, m), (m, p123, p23, b))
}

/// `v` scaled to length 1, or `None` when it is shorter than 1e-9.
fn unit(v: Point) -> Option<Point> {
    let n = v.x.hypot(v.y);
    (n > 1e-9).then(|| Point::new(v.x / n, v.y / n))
}
/// A subpath: consecutive segments, and whether it closes back on its start.
#[derive(Debug, Clone)]
pub(crate) struct Subpath {
    /// The segments, each starting where the one before ended. Never empty.
    pub(crate) segs: Vec<Src>,
    /// Ended by `Z`; the closing line, if it had length, is the last segment.
    pub(crate) closed: bool,
}
/// Parse a `d` attribute into absolute subpaths of lines and cubics. Arcs and quadratics
/// become cubics (the arc conversion is the only lossy step, at ~1e-6 of the radius).
///
/// `svgtypes`' simplifying parser resolves relative, `H`/`V`, smooth and arc commands; a
/// quadratic with control point `q` is raised exactly to the cubic with controls
/// `p0 + 2/3·(q − p0)` and `p + 2/3·(q − p)`. Zero-length lines are dropped, a `Z` adds the
/// closing line when the pen is away from the start, and a subpath with no segments (a bare
/// `M`) is not returned. Fails with the parser's message on malformed data.
pub(crate) fn parse_d(d: &str) -> Result<Vec<Subpath>, String> {
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
/// A subpath as it was written, for respelling only: absolute, with arcs kept as arcs.
///
/// [`parse_d`] reads for the fitter, which wants one curve type to measure, so it turns
/// every arc into cubics. That is the right input for refitting and the wrong one for
/// [`crate::compact`], which only changes the spelling: a half-circle that the file says in
/// one `A` (seven numbers, two of them one-character flags) comes back as two cubics of six
/// numbers each, longer than what it replaces, so every path holding an arc was left as it
/// was. 57% of the tracer's `--minify` paths hold one, and 72% of their path bytes
/// (measured on the 246-icon screen set, 2026-10-02).
#[derive(Debug, Clone)]
pub(crate) struct WrittenSubpath {
    /// Where the subpath starts, absolute.
    pub(crate) start: Point,
    /// Its segments, absolute, each starting where the one before ended. Never empty.
    pub(crate) segs: Vec<Segment>,
    /// Ended by `Z`; the closing line, if it had length, is the last segment.
    pub(crate) closed: bool,
}

/// Parse a `d` attribute into absolute subpaths with every arc kept as an arc: the input of
/// a respelling that must not move anything ([`crate::compact`]).
///
/// The same reading as [`parse_d`] in every other respect, command by command, as SVG 1.1
/// §8.3 defines it and `svgtypes`' simplifying parser implements it:
///
/// * relative coordinates are resolved against the pen; `H` and `V` become lines; a `Z`
///   followed by anything but a move starts the next subpath at the last move's point;
/// * `S` takes the reflection of the previous cubic's second control point about the pen
///   (the pen itself after anything but `C` or `S`), and `T` likewise for quadratics;
/// * a quadratic with control point `q` is raised exactly to the cubic with controls
///   `p0 + 2/3·(q − p0)` and `p + 2/3·(q − p)`;
/// * an arc keeps its radii, rotation (read in degrees, held in radians as
///   [`Segment::Arc`] does) and flags, with its end resolved to absolute. An arc that ends
///   where it starts is dropped, as SVG 1.1 F.6.2 says a renderer omits it; one with a zero
///   radius is kept, because a renderer draws it as the straight line (F.6.2) and writing
///   it back unchanged keeps exactly that;
/// * zero-length lines are dropped, `Z` adds the closing line when the pen is away from the
///   start, and a subpath with no segments is not returned.
///
/// Fails with the parser's message on malformed data.
///
/// Method from: W3C, "Scalable Vector Graphics (SVG) 1.1 (Second Edition)", 2011, §8.3
/// (path data grammar) and Appendix F.6 (elliptical arc implementation notes),
/// <https://www.w3.org/TR/SVG11/>; the command resolution follows `svgtypes`'
/// `SimplifyingPathParser` line for line, minus its arc-to-cubic step.
pub(crate) fn parse_d_written(d: &str) -> Result<Vec<WrittenSubpath>, String> {
    use svgtypes::PathSegment as P;
    let mut out: Vec<WrittenSubpath> = Vec::new();
    let mut segs: Vec<Segment> = Vec::new();
    let mut start = Point::new(0.0, 0.0);
    let mut pen = start;
    // The previous command's reflected control point, for `S` (cubic) and `T` (quadratic):
    // `Some` only right after a command of the matching family.
    let mut last_c2: Option<Point> = None;
    let mut last_q: Option<Point> = None;
    // After a `Z` the pen is back at `start`, so a drawing command that follows without a
    // move of its own begins the next subpath there -- the implicit move SVG 1.1 §8.3.3
    // asks for -- and a relative move is taken from that same point.
    let flush = |segs: &mut Vec<Segment>, out: &mut Vec<WrittenSubpath>, s: Point, closed| {
        if !segs.is_empty() {
            out.push(WrittenSubpath {
                start: s,
                segs: std::mem::take(segs),
                closed,
            });
        }
    };
    for seg in svgtypes::PathParser::from(d) {
        let seg = seg.map_err(|e| format!("bad path data: {e}"))?;
        // Resolve one coordinate pair against the pen when the command is relative.
        let abs_pt = |abs: bool, x: f64, y: f64| {
            if abs {
                Point::new(x, y)
            } else {
                Point::new(pen.x + x, pen.y + y)
            }
        };
        if let Some(c) = curve_command(&seg, pen, last_c2, last_q) {
            segs.push(c.seg);
            (pen, last_c2, last_q) = (c.end, c.c2, c.q);
            continue;
        }
        match seg {
            P::MoveTo { abs, x, y } => {
                flush(&mut segs, &mut out, start, false);
                start = abs_pt(abs, x, y);
                pen = start;
            }
            P::LineTo { abs, x, y } => {
                let p = abs_pt(abs, x, y);
                push_line(&mut segs, pen, p);
                pen = p;
            }
            P::HorizontalLineTo { abs, x } => {
                let p = Point::new(if abs { x } else { pen.x + x }, pen.y);
                push_line(&mut segs, pen, p);
                pen = p;
            }
            P::VerticalLineTo { abs, y } => {
                let p = Point::new(pen.x, if abs { y } else { pen.y + y });
                push_line(&mut segs, pen, p);
                pen = p;
            }
            // The four curve commands were taken by `curve_command` above.
            P::CurveTo { .. }
            | P::SmoothCurveTo { .. }
            | P::Quadratic { .. }
            | P::SmoothQuadratic { .. } => {}
            P::EllipticalArc {
                abs,
                rx,
                ry,
                x_axis_rotation,
                large_arc,
                sweep,
                x,
                y,
            } => {
                let p = abs_pt(abs, x, y);
                if p.dist(pen) > 1e-12 {
                    segs.push(Segment::Arc {
                        rx,
                        ry,
                        phi: x_axis_rotation.to_radians(),
                        large_arc,
                        sweep,
                        end: p,
                    });
                }
                pen = p;
            }
            P::ClosePath { .. } => {
                if pen.dist(start) > 1e-9 && !segs.is_empty() {
                    segs.push(Segment::Line(start));
                }
                flush(&mut segs, &mut out, start, true);
                pen = start;
            }
        }
        // Anything but a curve command leaves nothing for an `S` or `T` to reflect.
        (last_c2, last_q) = (None, None);
    }
    flush(&mut segs, &mut out, start, false);
    Ok(out)
}

/// One curve command resolved to absolute: the cubic it draws, where the pen ends, and the
/// control point the next `S` (`c2`) or `T` (`q`) reflects.
struct Curve {
    /// The segment, a cubic (a quadratic is raised to one exactly).
    seg: Segment,
    /// The command's end point.
    end: Point,
    /// The cubic's second control point, after `C` or `S`.
    c2: Option<Point>,
    /// The quadratic's control point, after `Q` or `T`.
    q: Option<Point>,
}

/// `C`, `S`, `Q` or `T` resolved against the pen, for [`parse_d_written`]; `None` for any
/// other command. `last_c2` and `last_q` are the previous command's reflected points, `Some`
/// only right after a command of the same family: `S` reflects `last_c2` about the pen and
/// `T` reflects `last_q`, each falling back to the pen itself (SVG 1.1 §8.3.6, §8.3.7).
fn curve_command(
    seg: &svgtypes::PathSegment,
    pen: Point,
    last_c2: Option<Point>,
    last_q: Option<Point>,
) -> Option<Curve> {
    use svgtypes::PathSegment as P;
    let at = |abs: bool, x: f64, y: f64| {
        if abs {
            Point::new(x, y)
        } else {
            Point::new(pen.x + x, pen.y + y)
        }
    };
    let mirror =
        |c: Option<Point>| c.map_or(pen, |c| Point::new(2.0 * pen.x - c.x, 2.0 * pen.y - c.y));
    Some(match *seg {
        P::CurveTo {
            abs,
            x1,
            y1,
            x2,
            y2,
            x,
            y,
        } => {
            let (c2, end) = (at(abs, x2, y2), at(abs, x, y));
            Curve {
                seg: Segment::Cubic(at(abs, x1, y1), c2, end),
                end,
                c2: Some(c2),
                q: None,
            }
        }
        P::SmoothCurveTo { abs, x2, y2, x, y } => {
            let (c2, end) = (at(abs, x2, y2), at(abs, x, y));
            Curve {
                seg: Segment::Cubic(mirror(last_c2), c2, end),
                end,
                c2: Some(c2),
                q: None,
            }
        }
        P::Quadratic { abs, x1, y1, x, y } => {
            let (q, end) = (at(abs, x1, y1), at(abs, x, y));
            Curve {
                seg: raise_quadratic(pen, q, end),
                end,
                c2: None,
                q: Some(q),
            }
        }
        P::SmoothQuadratic { abs, x, y } => {
            let (q, end) = (mirror(last_q), at(abs, x, y));
            Curve {
                seg: raise_quadratic(pen, q, end),
                end,
                c2: None,
                q: Some(q),
            }
        }
        _ => return None,
    })
}

/// A line from `pen` to `p`, unless it has no length.
fn push_line(segs: &mut Vec<Segment>, pen: Point, p: Point) {
    if p.dist(pen) > 1e-12 {
        segs.push(Segment::Line(p));
    }
}

/// The quadratic `pen → q → p` as the cubic that draws it exactly (degree elevation):
/// controls `pen + 2/3·(q − pen)` and `p + 2/3·(q − p)`.
fn raise_quadratic(pen: Point, q: Point, p: Point) -> Segment {
    Segment::Cubic(
        Point::new(
            pen.x + 2.0 / 3.0 * (q.x - pen.x),
            pen.y + 2.0 / 3.0 * (q.y - pen.y),
        ),
        Point::new(p.x + 2.0 / 3.0 * (q.x - p.x), p.y + 2.0 / 3.0 * (q.y - p.y)),
        p,
    )
}

/// Which joins of a subpath are corners: index `k` means the join entering `segs[k]`
/// (for a closed subpath, `0` is the join from the last segment back to the first).
///
/// A join is a corner when the turn between the incoming and outgoing unit tangents `a`, `b`
/// exceeds `corner_degrees`, tested as `a·b < cos(corner_degrees)`. The start of an open
/// subpath, and any join next to a zero-length segment (no tangent), is a corner.
pub(crate) fn corners(sp: &Subpath, corner_degrees: f64) -> Vec<bool> {
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
pub(crate) fn sample(run: &[Src], spacing: f64, drop_last: bool) -> Vec<Point> {
    sample_mapped(run, spacing, drop_last).0
}
/// The same samples, each with where it came from: the source segment's index plus the
/// parameter within it. That is what lets a span of samples be turned back into the exact
/// source curve it was taken from.
pub(crate) fn sample_mapped(run: &[Src], spacing: f64, drop_last: bool) -> (Vec<Point>, Vec<f64>) {
    let mut pts = vec![run[0].start()];
    let mut at = vec![0.0];
    let most = PER_SEGMENT;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A square drawn as four lines: every join breaks the tangent, so every join is a
    /// corner the fitter must respect.
    #[test]
    fn every_join_of_a_square_is_a_corner() {
        let sp = parse_d("M10,10L100,10L100,100L10,100Z").unwrap();
        assert_eq!(sp.len(), 1);
        let c = corners(&sp[0], 30.0);
        assert_eq!(c.len(), 4);
        assert!(c.iter().all(|&c| c), "{c:?}");
    }

    /// The same loop with smooth joins: no corner anywhere, so a closed run may be fitted
    /// whole.
    #[test]
    fn a_loop_with_matching_tangents_has_no_corner() {
        let sp = parse_d("M50,10C10,10 10,90 50,90C90,90 90,10 50,10Z").unwrap();
        let c = corners(&sp[0], 30.0);
        assert!(c.iter().all(|&c| !c), "{c:?}");
    }

    /// The written reading agrees with the fitter's on every command but the arc: the same
    /// subpaths, the same segments, the same points, for relative, `H`/`V`, `S` after a
    /// cubic and after a line, `Q`/`T`, a `Z` followed by a drawing command, and a relative
    /// move after a `Z`.
    #[test]
    fn the_written_reading_matches_the_fitters_without_arcs() {
        for d in [
            "M10,10L100,10L100,100Z",
            "m10 10h90v90H10z",
            "M10,100C30,10 90,10 110,100S190,190 160,110l5-5",
            "M0,0L10,0S20,10 30,0",
            "M0,0Q10,10 20,0T40,0t20,0Z",
            "M5,5L15,5L15,15ZL25,25L5,25Z",
            "M5,5l10,0l0,10zm20,0l10,0l0,10z",
            "M1 1 2 2 3 1",
            "M1,1L1,1L4,4Z",
        ] {
            let fitter = parse_d(d).unwrap();
            let written = parse_d_written(d).unwrap();
            assert_eq!(fitter.len(), written.len(), "{d}");
            for (f, w) in fitter.iter().zip(&written) {
                assert_eq!(f.closed, w.closed, "{d}");
                assert_eq!(f.segs.len(), w.segs.len(), "{d}");
                assert!(f.segs[0].start().dist(w.start) < 1e-12, "{d}");
                for (a, b) in f.segs.iter().zip(&w.segs) {
                    let pts = |s: &Segment| -> Vec<Point> {
                        match *s {
                            Segment::Line(p) => vec![p],
                            Segment::Cubic(c1, c2, p) => vec![c1, c2, p],
                            Segment::Arc { end, .. } => vec![end],
                        }
                    };
                    let (pa, pb) = (pts(&a.segment()), pts(b));
                    assert_eq!(pa.len(), pb.len(), "{d}");
                    for (x, y) in pa.iter().zip(&pb) {
                        assert!(x.dist(*y) < 1e-9, "{d}: {x:?} vs {y:?}");
                    }
                }
            }
        }
    }

    /// Arcs come through as arcs, with their end resolved to absolute and their rotation in
    /// radians; one that ends where it starts is omitted, as a renderer omits it.
    #[test]
    fn the_written_reading_keeps_arcs() {
        let sp = parse_d_written("M10,20a5,5 30 1,0 10,0A5 5 0 0 1 10,20a3,3 0 0 0 0,0Z").unwrap();
        assert_eq!(sp.len(), 1);
        assert!(sp[0].closed);
        assert_eq!(sp[0].segs.len(), 2, "{:?}", sp[0].segs);
        match sp[0].segs[0] {
            Segment::Arc {
                rx,
                ry,
                phi,
                large_arc,
                sweep,
                end,
            } => {
                assert_eq!((rx, ry, large_arc, sweep), (5.0, 5.0, true, false));
                assert!((phi - 30f64.to_radians()).abs() < 1e-15);
                assert!(end.dist(Point::new(20.0, 20.0)) < 1e-12);
            }
            ref other => panic!("not an arc: {other:?}"),
        }
        assert!(matches!(sp[0].segs[1], Segment::Arc { sweep: true, .. }));
    }

    /// Each sample lands on the exact point of the source segment its `at` names: that is
    /// the contract a span's fallback relies on when it returns the source unchanged.
    #[test]
    fn samples_carry_where_they_came_from() {
        let sp = parse_d("M0,0L30,0L30,30Z").unwrap();
        let (pts, at) = sample_mapped(&sp[0].segs, 10.0, true);
        assert_eq!(pts[0], sp[0].segs[0].start());
        for (i, &a) in at.iter().enumerate().skip(1) {
            // A sample may sit exactly on a segment boundary: it is then the new
            // segment's own start, which `at(0)` returns exactly.
            let (k, t) = (a.floor() as usize, a.fract());
            let expected = sp[0].segs[k].at(t);
            assert!(
                pts[i].dist(expected) < 1e-9,
                "sample {i} at {a}: {:?} vs {:?}",
                pts[i],
                expected
            );
        }
    }
}
