//! Fast mode: a Potrace-class tracer on the planar map's shared edges.
//!
//! Quality mode recovers its palette by minimum description length, merges gradient bands
//! pairwise, solves the whole boundary against the image and fits every boundary with a
//! global multi-model dynamic program; that is where its time goes. Fast mode keeps what
//! makes the output correct -- the planar map whose shared edges are traced once so
//! neighbouring faces cannot open a seam, the sub-pixel refinement of every boundary point,
//! and the emitter with its seam underlap, compound paths and minify -- and replaces the
//! rest with one-pass versions:
//!
//! * `palette`: inks from a 15-bit histogram of flat pixels; blends go to a neighbour's ink;
//! * `front`: faces and the planar map, without blend absorption or the boundary solve;
//! * `bands`: posterised ramps put back together, one gradient fit per ramp;
//! * `prims`: a closed boundary that is a circle or an ellipse is written as one;
//! * and for everything else the classical pipeline of Selinger's Potrace (2003), restated
//!   for sub-pixel input: the optimal polygon (`polygon`), least-squares vertex placement
//!   and corner-aware smoothing into Bézier pieces (`smooth`), and curve-run
//!   optimisation (`curve`).
//!
//! Each stage is linear or near-linear in the number of pixels or boundary points, and
//! nothing reads a clock, so the output is the same on every machine. Written from the
//! paper; no Potrace or Trazor source was read or used.

mod bands;
mod curve;
mod faces;
mod front;
mod palette;
mod polygon;
mod prims;
mod smooth;

pub(crate) use front::{trace_color, trace_color_native};

use crate::planar::Edge;
use inkvec_core::Point;
use inkvec_fit::primitives::PrimitiveFit;
use inkvec_fit::FittedPath;

/// The fast fitter's tolerances, in pixels.
#[derive(Debug, Clone, Copy)]
pub struct FastFit {
    /// How far a point may lie from the polygon side that spans it.
    pub poly_tol: f64,
    /// Half-width of the box a polygon vertex may move within when it is placed.
    pub vertex_box: f64,
    /// A vertex is a corner when the smoothed curve misses the points around it by more
    /// than this, and the two sides through the vertex miss them by much less: Potrace's
    /// `alphamax`, as a distance.
    pub corner_tol: f64,
    /// How far a merged cubic may stray from the pieces it replaces (Potrace's
    /// `opttolerance`).
    pub opt_tol: f64,
    /// A cubic whose control points lie this close to its chord is written as a line.
    pub flat: f64,
}

impl Default for FastFit {
    fn default() -> Self {
        Self {
            poly_tol: 0.5,
            vertex_box: 0.5,
            corner_tol: 0.25,
            opt_tol: 0.2,
            flat: 0.05,
        }
    }
}

/// Fit one measured boundary. The ends of an open boundary are kept exactly: they are
/// junctions every boundary meeting there shares.
pub fn fit_points(pts: &[Point], closed: bool, cfg: &FastFit) -> FittedPath {
    let n = pts.len();
    let lines = |pts: &[Point]| FittedPath {
        start: pts[0],
        segments: pts[1..]
            .iter()
            .copied()
            .chain(closed.then_some(pts[0]))
            .map(inkvec_fit::curves::Segment::Line)
            .collect(),
        closed,
    };
    if n < 3 || (closed && n < 4) {
        if n == 0 {
            return FittedPath {
                start: Point::new(0.0, 0.0),
                segments: Vec::new(),
                closed,
            };
        }
        return lines(pts);
    }
    let pts = &smooth::denoise(pts, closed)[..];
    let vtx = if closed {
        polygon::closed(pts, cfg.poly_tol)
    } else {
        polygon::open(pts, cfg.poly_tol)
    };
    if vtx.len() < 2 {
        return lines(pts);
    }
    let v = smooth::adjust_vertices(pts, &vtx, closed, cfg.vertex_box);
    let pieces = smooth::pieces(pts, &vtx, &v, closed, cfg.corner_tol);
    let curves = curve::optimise(&pieces, closed, cfg.opt_tol);
    let Some(first) = curves.first() else {
        return lines(pts);
    };
    let start = first[0];
    let mut segments = curve::to_segments(&curves, cfg.flat);
    // Pin the ends: an open boundary ends exactly on its junction, a closed one on its
    // own start, whatever rounding the stages above did.
    let end = if closed { start } else { pts[n - 1] };
    if let Some(last) = segments.last_mut() {
        match last {
            inkvec_fit::curves::Segment::Line(e) => *e = end,
            inkvec_fit::curves::Segment::Cubic(_, _, e) => *e = end,
            inkvec_fit::curves::Segment::Arc { end: e, .. } => *e = end,
        }
    }
    FittedPath {
        start: if closed { start } else { pts[0] },
        segments,
        closed,
    }
}

/// OKLab contrast at or above which a boundary gets the tolerances as given. Below it they
/// grow as `FAINT / contrast`, up to [`MAX_LOOSEN`] times: where the two sides are close in
/// colour a misplaced boundary costs little, and where the two sides are two bands of one
/// ramp there is no sharp edge to place at all.
const FAINT: f64 = 0.12;
/// How much a boundary of a gradient face is loosened, at the least.
const GRADIENT_LOOSEN: f64 = 1.6;
/// Most a faint boundary's tolerances are loosened.
const MAX_LOOSEN: f64 = 3.0;

impl FastFit {
    /// These tolerances, loosened for a boundary of OKLab `contrast`.
    fn for_contrast(&self, contrast: f64) -> FastFit {
        let k = (FAINT / contrast.max(1e-6)).clamp(1.0, MAX_LOOSEN);
        FastFit {
            poly_tol: self.poly_tol * k,
            vertex_box: self.vertex_box * k,
            corner_tol: self.corner_tol * k,
            opt_tol: self.opt_tol * k,
            flat: self.flat,
        }
    }
}

/// Fit one boundary of the map: as a circle or an ellipse when a closed boundary is one,
/// and with [`fit_points`] otherwise.
pub fn fit_edge(pts: &[Point], closed: bool, cfg: &FastFit) -> (FittedPath, Option<PrimitiveFit>) {
    // A closed boundary starts at a lattice node the sub-pixel refinement leaves where it
    // was, up to 0.6 px off the edge; the ring closes just as well without it.
    let pts = if closed && pts.len() >= 8 {
        &pts[1..]
    } else {
        pts
    };
    if closed {
        if let Some((prim, start, segments)) = prims::primitive(&smooth::denoise(pts, true)) {
            let path = FittedPath {
                start,
                segments,
                closed: true,
            };
            return (path, Some(prim));
        }
    }
    (fit_points(pts, closed, cfg), None)
}

/// Fit every edge of a planar map, in parallel. Each shared edge is fitted once and both
/// of its faces draw the same curve. `fills` is each face's fill; an edge between two faces
/// of similar colour, or along a gradient, is fitted with looser tolerances.
pub fn fit_edges(
    edges: &[Edge],
    fills: &[crate::gradient::FillFit],
    cfg: &FastFit,
) -> Vec<(FittedPath, Option<PrimitiveFit>)> {
    use rayon::prelude::*;
    let lab = |f: u16| {
        fills
            .get(f as usize)
            .map(|fit| crate::color::rgb_to_oklab(fit.model.representative()))
    };
    let graded = |f: u16| {
        fills
            .get(f as usize)
            .is_some_and(|fit| fit.model.is_gradient())
    };
    edges
        .par_iter()
        .map(|e| {
            let mut contrast = match (lab(e.left), lab(e.right)) {
                (Some(a), Some(b)) => a.dist(b) as f64,
                _ => 1.0,
            };
            // A boundary is placed against each side's fill model, and a fitted gradient is
            // a coarser model of its pixels than a flat ink is of its own: its boundary
            // comes back rougher, and is fitted as if its contrast were lower.
            if graded(e.left) || graded(e.right) {
                contrast = contrast.min(FAINT / GRADIENT_LOOSEN);
            }
            fit_edge(&e.points, e.closed, &cfg.for_contrast(contrast))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use inkvec_fit::curves::Segment;

    #[test]
    fn a_square_ring_is_four_lines() {
        let mut pts = Vec::new();
        for k in 0..10 {
            pts.push(Point::new(k as f64, 0.0));
        }
        for k in 0..10 {
            pts.push(Point::new(10.0, k as f64));
        }
        for k in 0..10 {
            pts.push(Point::new(10.0 - k as f64, 10.0));
        }
        for k in 0..10 {
            pts.push(Point::new(0.0, 10.0 - k as f64));
        }
        let f = fit_points(&pts, true, &FastFit::default());
        assert_eq!(f.segments.len(), 4, "{:?}", f.segments);
        assert!(f.segments.iter().all(|s| matches!(s, Segment::Line(_))));
        assert_eq!(f.end(), f.start);
    }

    #[test]
    fn a_circle_is_a_few_cubics_close_to_the_circle() {
        let n = 190;
        let pts: Vec<Point> = (0..n)
            .map(|k| {
                let t = k as f64 / n as f64 * std::f64::consts::TAU;
                Point::new(40.0 + 30.0 * t.cos(), 40.0 + 30.0 * t.sin())
            })
            .collect();
        let f = fit_points(&pts, true, &FastFit::default());
        assert!(f.segments.len() <= 8, "{}", f.segments.len());
        let mut at = f.start;
        for s in &f.segments {
            if let Segment::Cubic(a, b, c) = *s {
                for t in [0.25, 0.5, 0.75] {
                    let q = inkvec_fit::curves::eval_cubic([at, a, b, c], t);
                    let r = (q.x - 40.0).hypot(q.y - 40.0);
                    assert!((r - 30.0).abs() < 0.35, "r = {r}");
                }
            }
            at = s.end();
        }
    }

    #[test]
    fn an_open_boundary_keeps_its_junctions_exactly() {
        let pts: Vec<Point> = (0..30)
            .map(|k| Point::new(k as f64 * 0.7 + 0.13, (k as f64 * 0.3).sin() * 4.0 + 0.37))
            .collect();
        let f = fit_points(&pts, false, &FastFit::default());
        assert_eq!(f.start, pts[0]);
        assert_eq!(f.end(), pts[29]);
    }

    #[test]
    fn a_long_straight_boundary_is_one_line() {
        let pts: Vec<Point> = (0..1000)
            .map(|k| Point::new(k as f64 * 0.8 + 0.1, k as f64 * 0.6 + 0.3))
            .collect();
        let f = fit_points(&pts, false, &FastFit::default());
        assert_eq!(f.segments.len(), 1, "{:?}", f.segments);
        assert_eq!(f.end(), pts[999]);
    }

    #[test]
    fn tiny_boundaries_are_lines() {
        let pts = [Point::new(0.0, 0.0), Point::new(1.0, 0.0)];
        let f = fit_points(&pts, false, &FastFit::default());
        assert_eq!(f.segments.len(), 1);
    }
}
