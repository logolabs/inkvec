//! How well a set of stroked centrelines explains a face's measured boundary.
//!
//! **The model.** An SVG stroke with round caps and round joins paints exactly the
//! points within half its width `h` of its centreline: the Minkowski sum of the path with
//! a disc of radius `h`. A union of such strokes, `C` the union of their centrelines,
//! therefore has its outline where the distance to `C` equals `h`. So every measured
//! boundary point `p` of the face has a residual
//!
//! `r_p = d(p, C) - h`,
//!
//! the same quantity the outline fit is judged by (the distance from a measured point to
//! the curve drawn through it), now with the drawn curve being the stroke's edge. A blob
//! the strokes do not cover puts boundary points beyond `h`; a hole they paint over puts
//! them inside `h`; so this one residual sees both kinds of failure.
//!
//! **The width.** For fixed centrelines, `Σ (r_p/σ_p)²` is a quadratic in `h` with the
//! closed-form minimiser `h = Σ (d_p/σ_p²) / Σ (1/σ_p²)`, taken over the points within
//! `max(0.25·w0, 1)` px of the paired half-width `w0/2` so a junction corner or a cap the
//! centrelines missed cannot drag the width of every stroke. The chi-squared itself is
//! summed over *every* point: what is not explained is not forgiven.
//!
//! Method from: the stroke model is SVG's own (round `stroke-linecap` / `stroke-linejoin`
//! paint the Minkowski sum with a disc), and the criterion is the same weighted
//! least-squares likelihood `χ²/2` the curve fitter and the primitive search minimise, so
//! the two descriptions of one face are priced in one currency.
//! Inspired by: Favreau, Lafarge, Bousseau (2016), Fidelity vs. simplicity: a global
//! approach to line drawing vectorization, ACM TOG 35(4), doi:10.1145/2897824.2925946,
//! whose energy weighs the fitting error of centrelines against their number and degree;
//! ours measures the fit on the outline the strokes paint, not on a skeleton.

use inkvec_core::Point;
use inkvec_fit::curves::{arc_ellipse_center, Segment};
use inkvec_fit::FittedPath;

use super::boundary::Boundary;
use super::grid::SegGrid;
use super::join::Join;
use super::refine::distances;
use super::Centreline;

/// The fitted centrelines of a face, scored against its boundary.
#[derive(Debug, Clone, Copy)]
pub struct Score {
    /// `Σ_p ((d(p, C) - h)/σ_p)²` over every measured boundary point, σ floored at 1e-3
    /// px as the curve fitter's chi-squared does.
    pub chi2: f64,
    /// The least-squares half-width `h`, px.
    pub half: f64,
    /// Root mean square of `d(p, C) - h`, px.
    pub rms: f64,
    /// Largest `|d(p, C) - h|`, px.
    pub worst: f64,
    /// Boundary points with `|d(p, C) - h| > 0.5` px.
    pub outliers: usize,
}

/// Append `path` to `out` as line segments no longer than `step` px (closed paths get
/// their closing segment when the last segment does not already end at the start).
///
/// Lines are one segment; cubics are evaluated at `n = ceil(L/step)` equal steps of `t`,
/// with `L` the control polygon's length (an upper bound on the arc length); arcs are
/// converted to centre form (SVG appendix F.6.5, [`arc_ellipse_center`]) and stepped in
/// angle by the same rule on their span.
pub(crate) fn flatten(path: &FittedPath, step: f64, out: &mut Vec<(Point, Point)>) {
    let mut cur = path.start;
    let mut push = |a: Point, b: Point| {
        if a.dist(b) > 1e-12 {
            out.push((a, b));
        }
    };
    for s in &path.segments {
        match *s {
            Segment::Line(p) => {
                push(cur, p);
                cur = p;
            }
            Segment::Cubic(c1, c2, p) => {
                let l = cur.dist(c1) + c1.dist(c2) + c2.dist(p);
                let n = ((l / step).ceil() as usize).clamp(1, 4096);
                let mut prev = cur;
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    let mt = 1.0 - t;
                    let (b0, b1, b2, b3) =
                        (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
                    let q = Point::new(
                        b0 * cur.x + b1 * c1.x + b2 * c2.x + b3 * p.x,
                        b0 * cur.y + b1 * c1.y + b2 * c2.y + b3 * p.y,
                    );
                    push(prev, q);
                    prev = q;
                }
                cur = p;
            }
            Segment::Arc {
                rx,
                ry,
                phi,
                large_arc,
                sweep,
                end,
            } => {
                let f = arc_ellipse_center(cur, rx, ry, phi, large_arc, sweep, end);
                let n = ((f.span() / step).ceil() as usize).clamp(1, 4096);
                let mut prev = cur;
                for i in 1..n {
                    let q = f.at(f.theta1 + f.delta * i as f64 / n as f64);
                    push(prev, q);
                    prev = q;
                }
                push(prev, end);
                cur = end;
            }
        }
    }
    if path.closed {
        push(cur, path.start);
    }
}

/// Score the centrelines `lines` (joined by `join`) against boundary `b`, starting from
/// the paired width `w0` px. See the module documentation for the residual, the width and
/// the chi-squared. With `half` given (the stroke solve's half-width) the width is not
/// re-estimated.
///
/// The distances are the stroke solve's own ([`super::refine::distances`]): exact
/// distances to each segment (a cubic's foot found by Newton's method, an arc's in closed
/// form), the miter gauge at a vertex under miter joins, and `8·h + 4` px for a point no
/// centreline comes near. So the chi-squared that decides the face is the one the solve
/// minimised. Cost O(boundary points x local segments).
pub(crate) fn score(
    b: &Boundary,
    lines: &[Centreline],
    w0: f64,
    half: Option<f64>,
    join: Join,
) -> Score {
    let d = distances(lines, half.unwrap_or(0.5 * w0), b, join);
    let band = (0.25 * w0).max(1.0);
    let (mut num, mut den) = (0.0, 0.0);
    for (i, &di) in d.iter().enumerate() {
        if !b.frame[i] && (di - 0.5 * w0).abs() <= band {
            let s = b.sigma[i].max(1e-3);
            num += di / (s * s);
            den += 1.0 / (s * s);
        }
    }
    let half = half.unwrap_or(if den > 0.0 { num / den } else { 0.5 * w0 });
    let (mut chi2, mut ss, mut worst, mut outliers) = (0.0, 0.0, 0.0f64, 0usize);
    for (i, &di) in d.iter().enumerate() {
        let (r, s, _) = b.residual(i, di, half);
        chi2 += r * r;
        let dist = r * s;
        ss += dist * dist;
        worst = worst.max(dist.abs());
        if dist.abs() > 0.5 {
            outliers += 1;
        }
    }
    Score {
        chi2,
        half,
        rms: (ss / d.len().max(1) as f64).sqrt(),
        worst,
        outliers,
    }
}

/// `Σ_k (d_k/σ_k)²` of measured points `pts` (sigmas `sigma`, floored at 1e-3 px; 0.5 past
/// the end of `sigma`) against `path`, with `d_k` the exact distance to the path flattened
/// at 0.1 px.
///
/// The outline's side of the stroke-versus-fill comparison. It is the chi-squared the
/// curve fitter's `curves::chi2` estimates, but without that function's assumption that
/// the points and the path run the same way (its nearest-sample search walks a window
/// forward along the path): after the mirror stage a fit can be its partner's reflection
/// run backwards, and the windowed search then reads distances of whole pixels.
///
/// A path with nothing to flatten (no segments, or only zero-length ones: the fitter's
/// answer for an edge of a point or two) is measured against its start point, so one
/// degenerate edge does not make a whole face's outline cost infinite (openmoji `E056`).
pub(crate) fn path_chi2(pts: &[Point], sigma: &[f64], path: &FittedPath) -> f64 {
    let mut segs = Vec::new();
    flatten(path, 0.1, &mut segs);
    if segs.is_empty() {
        segs.push((path.start, path.start));
    }
    let grid = SegGrid::new(&segs, 2.0);
    pts.iter()
        .enumerate()
        .map(|(k, &p)| {
            let d = grid.nearest(p, 1e6).map_or(1e6, |n| n.d);
            let s = sigma.get(k).copied().unwrap_or(0.5).max(1e-3);
            (d / s) * (d / s)
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ribbon::boundary::Boundary;
    use inkvec_core::Polyline;

    /// The outline of a straight round-capped stroke from (0,0) to (`len`,0), half-width
    /// `h`, sampled every ~0.25 px, as one ring.
    fn stadium(len: f64, h: f64) -> Polyline {
        let mut pts = Vec::new();
        let n = (len / 0.25) as usize;
        for i in 0..=n {
            pts.push(Point::new(i as f64 * len / n as f64, -h));
        }
        let m = 40;
        for j in 1..m {
            let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * j as f64 / m as f64;
            pts.push(Point::new(len + h * a.cos(), h * a.sin()));
        }
        for i in 0..=n {
            pts.push(Point::new(len - i as f64 * len / n as f64, h));
        }
        for j in 1..m {
            let a = std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * j as f64 / m as f64;
            pts.push(Point::new(h * a.cos(), h * a.sin()));
        }
        Polyline::with_uniform_sigma(pts, 0.05, true)
    }

    #[test]
    fn the_true_centreline_explains_a_stadium_exactly() {
        let ring = stadium(30.0, 3.0);
        let inside = |p: Point| {
            let x = p.x.clamp(0.0, 30.0);
            Point::new(x, 0.0).dist(p) < 3.0
        };
        let b = Boundary::new(&[ring], &inside, 2.0).expect("a ring");
        let path = FittedPath {
            start: Point::new(0.0, 0.0),
            segments: vec![Segment::Line(Point::new(30.0, 0.0))],
            closed: false,
        };
        let line = |p: &FittedPath| Centreline {
            path: p.clone(),
            prim: None,
        };
        let s = score(&b, &[line(&path)], 6.0, None, Join::Round);
        assert!((s.half - 3.0).abs() < 1e-6, "half {}", s.half);
        assert!(s.worst < 1e-6 && s.outliers == 0, "{s:?}");
        // A centreline 0.2 px short at one end leaves that cap's points about 0.2 px
        // off; the least-squares width takes a little of it up (the cap's tip moves in by
        // 0.2, the rest of the outline by nothing), so the worst point is a shade under.
        let short = FittedPath {
            segments: vec![Segment::Line(Point::new(29.8, 0.0))],
            ..path
        };
        let s2 = score(&b, &[line(&short)], 6.0, None, Join::Round);
        assert!(
            s2.chi2 > s.chi2 + 100.0 && (0.15..0.21).contains(&s2.worst),
            "{s2:?}"
        );
    }

    #[test]
    fn flatten_closes_and_follows_arcs() {
        let c = FittedPath {
            start: Point::new(10.0, 0.0),
            segments: vec![
                Segment::circular_arc(10.0, false, true, Point::new(-10.0, 0.0)),
                Segment::circular_arc(10.0, false, true, Point::new(10.0, 0.0)),
            ],
            closed: true,
        };
        let mut segs = Vec::new();
        flatten(&c, 0.5, &mut segs);
        for &(a, _) in &segs {
            assert!((a.dist(Point::new(0.0, 0.0)) - 10.0).abs() < 1e-9);
        }
        let len: f64 = segs.iter().map(|s| s.0.dist(s.1)).sum();
        assert!((len - std::f64::consts::TAU * 10.0).abs() < 0.05, "{len}");
    }

    /// The outline of an L-shaped stroke with a miter corner: centreline (0,0) -> (20,0)
    /// -> (20,20), half-width 2, round caps, sampled every ~0.25 px.
    fn mitred_l() -> (Polyline, impl Fn(Point) -> bool) {
        let mut pts = Vec::new();
        let mut run = |a: Point, b: Point| {
            let n = (a.dist(b) / 0.25).ceil() as usize;
            for i in 0..n {
                let t = i as f64 / n as f64;
                pts.push(Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y)));
            }
        };
        run(Point::new(0.0, -2.0), Point::new(22.0, -2.0));
        run(Point::new(22.0, -2.0), Point::new(22.0, 20.0));
        let mut cap = |c: Point, from: f64| {
            for j in 0..40 {
                let a = from + std::f64::consts::PI * j as f64 / 40.0;
                pts.push(Point::new(c.x + 2.0 * a.cos(), c.y + 2.0 * a.sin()));
            }
        };
        cap(Point::new(20.0, 20.0), 0.0);
        let mut run2 = |a: Point, b: Point| {
            let n = (a.dist(b) / 0.25).ceil() as usize;
            for i in 0..n {
                let t = i as f64 / n as f64;
                pts.push(Point::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y)));
            }
        };
        run2(Point::new(18.0, 20.0), Point::new(18.0, 2.0));
        run2(Point::new(18.0, 2.0), Point::new(0.0, 2.0));
        for j in 0..40 {
            let a = std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * j as f64 / 40.0;
            pts.push(Point::new(2.0 * a.cos(), 2.0 * a.sin()));
        }
        let inside = |p: Point| {
            (p.x > 0.0 && p.x < 22.0 && p.y > -2.0 && p.y < 2.0)
                || (p.x > 18.0 && p.x < 22.0 && p.y > -2.0 && p.y < 20.0)
                || p.dist(Point::new(0.0, 0.0)) < 2.0
                || p.dist(Point::new(20.0, 20.0)) < 2.0
        };
        (Polyline::with_uniform_sigma(pts, 0.05, true), inside)
    }

    #[test]
    fn a_miter_corner_is_explained_by_miter_joins_and_not_by_round_ones() {
        let (ring, inside) = mitred_l();
        let b = Boundary::new(&[ring], &inside, 2.0).expect("a ring");
        let line = Centreline {
            path: FittedPath {
                start: Point::new(0.0, 0.0),
                segments: vec![
                    Segment::Line(Point::new(20.0, 0.0)),
                    Segment::Line(Point::new(20.0, 20.0)),
                ],
                closed: false,
            },
            prim: None,
        };
        let m = score(&b, std::slice::from_ref(&line), 4.0, Some(2.0), Join::Miter);
        assert!(m.worst < 1e-6, "miter {m:?}");
        // A round join leaves the outer corner (22, -2) at sqrt(8) - 2 = 0.83 px.
        let r = score(&b, &[line], 4.0, Some(2.0), Join::Round);
        assert!((r.worst - (8f64.sqrt() - 2.0)).abs() < 1e-6, "round {r:?}");
    }
}
