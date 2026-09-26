//! The source path: how a `d` attribute is read, and how points are taken
//! along it for the fitter to judge.

use crate::fit::PER_SEGMENT;
use inkvec_core::Point;
use inkvec_fit::curves::Segment;

/// One segment of a source path, absolute, with its start point.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Src {
    Line(Point, Point),
    Cubic(Point, Point, Point, Point),
}

impl Src {
    pub(crate) fn start(&self) -> Point {
        match *self {
            Src::Line(a, _) | Src::Cubic(a, _, _, _) => a,
        }
    }
    pub(crate) fn params(&self) -> f64 {
        match self {
            Src::Line(..) => 2.0,
            Src::Cubic(..) => 6.0,
        }
    }
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
    pub(crate) fn tangent_in(&self) -> Option<Point> {
        match *self {
            Src::Line(a, b) => unit(sub(b, a)),
            Src::Cubic(a, c1, c2, b) => unit(sub(b, c2))
                .or_else(|| unit(sub(b, c1)))
                .or_else(|| unit(sub(b, a))),
        }
    }
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
pub(crate) fn sub(a: Point, b: Point) -> Point {
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
pub(crate) struct Subpath {
    pub(crate) segs: Vec<Src>,
    pub(crate) closed: bool,
}
/// Parse a `d` attribute into absolute subpaths of lines and cubics. Arcs and quadratics
/// become cubics (the arc conversion is the only lossy step, at ~1e-6 of the radius).
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
/// Which joins of a subpath are corners: index `k` means the join entering `segs[k]`
/// (for a closed subpath, `0` is the join from the last segment back to the first).
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
