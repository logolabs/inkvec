//! Measuring how far one curve is from another, exactly: the guards that
//! decide whether a rewrite kept the tolerance promise, and the point-on-cubic
//! searches the least-squares refit is built on.

use inkvec_core::Point;
use inkvec_fit::curves::Segment;
use inkvec_fit::structural::eval_segment;
use inkvec_fit::FittedPath;

/// Distance from `p` to one segment of the fitted path: a coarse scan of the curve, then a
/// golden-section search on the parameter around the nearest sample. Sampling alone would
/// report the chord sagitta of the scan as error -- 0.04 units on a 120° arc of radius 40
/// -- and that is three times the tolerance this tool promises.
pub(crate) fn dist_to_segment(p: Point, seg: &Segment, start: Point) -> f64 {
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
pub(crate) struct Curve {
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
    pub(crate) fn new(path: &FittedPath) -> Self {
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
    pub(crate) fn nearest(&self, p: Point) -> (usize, f64) {
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
pub(crate) fn max_deviation(pts: &[Point], path: &FittedPath) -> f64 {
    let curve = Curve::new(path);
    pts.iter().map(|&p| curve.nearest(p).1).fold(0.0, f64::max)
}
pub(crate) fn point_segment_dist(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let l2 = dx * dx + dy * dy;
    let t = if l2 < 1e-18 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0)
    };
    p.dist(Point::new(a.x + t * dx, a.y + t * dy))
}
pub(crate) fn cubic_at(p0: Point, c1: Point, c2: Point, p3: Point, t: f64) -> Point {
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
pub(crate) fn nearest_t_cubic(p: Point, p0: Point, c1: Point, c2: Point, p3: Point) -> f64 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use inkvec_fit::curves::Segment;

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
}
