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
//! * `palette`: inks from a 15-bit histogram of flat pixels (and, for strokes too thin to
//!   have any, of coherent pixels that are no blend of other inks); a blend goes to a
//!   neighbour's ink or to the ink it is made of, never to one it merely lies near;
//! * `faces`: anti-aliased rims and inks the eye cannot tell apart joined to the faces they
//!   belong to, speckles below a floor that grows with the image, and the faces;
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
//!
//! # The pipeline, in order
//!
//! Entered through `trace_color` / `trace_color_native` (from
//! `crate::trace_color_full_with_alpha` when `ColorOptions::fast` is set); the fit is
//! driven by `inkvec-cli`'s `fast::fit`, which calls [`fit_edges`].
//!
//! 1. **Palette and labels** -- `palette::palette_and_labels`: inks from flat bins, then
//!    one ink per pixel, blends sent to the ink they are made of.
//! 2. **Label clean-up**, on the label image's row runs (`faces::RunLabels`):
//!    `absorb_slivers`, `absorb_rims`, `merge_same_inks`, then `despeckle` with the floor
//!    from `front::speckle_floor`, then `write_faces` (one face per 4-connected component,
//!    written once over the label buffer).
//! 3. **Ramps** -- `bands::merge_ramps` (opaque images with gradients on).
//! 4. **Planar map and sub-pixel refinement** -- shared with quality mode, in
//!    `crate::finish_color_trace_alpha`.
//! 5. **Fit**, per edge of the map, in parallel ([`fit_edges`] → [`fit_edge`]):
//!    the image frame, when one face runs round the whole border, is written as the image
//!    rectangle (`frame_rectangle`); a closed edge that is a circle or ellipse becomes one
//!    (`prims::primitive`, on the ring denoised once); every other edge goes through
//!    [`fit_points`]: `smooth::denoise` → `polygon::open` / `polygon::closed` (a
//!    boundary of 2048 points or more scans its anchors in parallel) →
//!    `smooth::adjust_vertices` → `smooth::pieces` → `curve::optimise` →
//!    `curve::to_segments`.
//!
//! Everything after the fit -- fills, seams, the emitter, minify -- is shared with quality
//! mode.

mod bands;
mod curve;
mod faces;
mod front;
mod palette;
mod polygon;
mod prims;
#[cfg(test)]
mod replay;
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
///
/// `pts` are sub-pixel boundary points in px; `closed` says whether the last point joins
/// the first. The stages are the Potrace pipeline of this module's overview:
/// `smooth::denoise`, the optimal polygon, vertex adjustment, smoothing into pieces, and
/// curve-run optimisation, then conversion to path segments. Boundaries too short for a
/// polygon (fewer than 3 points, or 4 for a ring) and any stage that leaves nothing are
/// drawn as straight lines through the (denoised, where it ran) points; an empty input is
/// an empty path at the origin.
pub fn fit_points(pts: &[Point], closed: bool, cfg: &FastFit) -> FittedPath {
    if too_short(pts.len(), closed) {
        return lines(pts, closed);
    }
    fit_denoised(&smooth::denoise(pts, closed), closed, cfg)
}

/// True when a boundary of `n` points is too short for a polygon: fewer than 3 points, or
/// fewer than 4 for a ring. [`fit_points`] draws such a boundary as [`lines`] through its
/// points as measured.
fn too_short(n: usize, closed: bool) -> bool {
    n < 3 || (closed && n < 4)
}

/// Straight lines through `pts`, from the first point, and back to it when `closed`: the
/// fallback for a boundary no stage could fit. An empty input is an empty path at the
/// origin. O(n).
fn lines(pts: &[Point], closed: bool) -> FittedPath {
    let Some(&first) = pts.first() else {
        return FittedPath {
            start: Point::new(0.0, 0.0),
            segments: Vec::new(),
            closed,
        };
    };
    FittedPath {
        start: first,
        segments: pts[1..]
            .iter()
            .copied()
            .chain(closed.then_some(first))
            .map(inkvec_fit::curves::Segment::Line)
            .collect(),
        closed,
    }
}

/// [`fit_points`] after its denoising: the optimal polygon, vertex adjustment, smoothing
/// into pieces, curve-run optimisation and conversion to segments, on points `pts` that
/// [`smooth::denoise`] has already smoothed (at least 3, or 4 for a ring; see
/// [`too_short`]). The fallbacks draw [`lines`] through these denoised points, and an open
/// boundary starts and ends exactly on `pts[0]` and `pts[n − 1]`, which the denoising
/// leaves where they were measured. Cost: that of `polygon::open`, O(n · MAX_SPAN) at
/// worst; the stages after it are linear in the points and the polygon's vertices.
///
/// Split out of [`fit_points`] so that [`fit_edge`] can hand a ring the points it has
/// already denoised for the primitive test instead of denoising them a second time.
fn fit_denoised(pts: &[Point], closed: bool, cfg: &FastFit) -> FittedPath {
    let n = pts.len();
    let vtx = if closed {
        polygon::closed(pts, cfg.poly_tol)
    } else {
        polygon::open(pts, cfg.poly_tol)
    };
    if vtx.len() < 2 {
        return lines(pts, closed);
    }
    let v = smooth::adjust_vertices(pts, &vtx, closed, cfg.vertex_box);
    let pieces = smooth::pieces(pts, &vtx, &v, closed, cfg.corner_tol);
    let curves = curve::optimise(&pieces, closed, cfg.opt_tol);
    let Some(first) = curves.first() else {
        return lines(pts, closed);
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
    /// These tolerances, loosened for a boundary of OKLab `contrast`: every distance
    /// tolerance except `flat` is multiplied by `k = clamp(FAINT / contrast, 1, MAX_LOOSEN)`.
    /// A zero or negative contrast is treated as 1e-6, which gives the largest `k`.
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
///
/// A ring is denoised once, for the primitive test, and the same points go on to the
/// polygon when no primitive fits. [`fit_points`] would denoise them again; `denoise` is a
/// pure function of the points and the closed flag, so the second call could only return
/// the same vector (measured: 996 of 996 rings on the screen set, 238 of 238 at 2048 px),
/// and the output is unchanged. Not from the literature: this only removes a repeated
/// computation.
pub fn fit_edge(pts: &[Point], closed: bool, cfg: &FastFit) -> (FittedPath, Option<PrimitiveFit>) {
    // A closed boundary starts at a lattice node the sub-pixel refinement leaves where it
    // was, up to 0.6 px off the edge; the ring closes just as well without it.
    let pts = if closed && pts.len() >= 8 {
        &pts[1..]
    } else {
        pts
    };
    if !closed {
        return (fit_points(pts, false, cfg), None);
    }
    let den = smooth::denoise(pts, true);
    if let Some((prim, start, segments)) = prims::primitive(&den) {
        let path = FittedPath {
            start,
            segments,
            closed: true,
        };
        return (path, Some(prim));
    }
    // `fit_points(pts, true, cfg)`, without denoising `pts` a second time.
    let path = if too_short(pts.len(), true) {
        lines(pts, true)
    } else {
        fit_denoised(&den, true, cfg)
    };
    (path, None)
}

/// The image frame, written as what it is: the rectangle `[-0.5, w − 0.5] × [-0.5,
/// h − 0.5]` of a `width` × `height` raster (pixel centres at integers), as four lines
/// through its corners in the ring's own order, starting at the first corner the ring
/// reaches. `None` for anything else: an open edge, a ring with a point off that
/// rectangle's border, or one that does not pass each corner exactly once.
///
/// When one face runs round the whole image border (a transparent icon's clear ground,
/// or a background colour), the planar map traces the border as one closed ring. Its
/// points lie on the pixel lattice's outer nodes, which the sub-pixel refinement leaves
/// in place, so the ring *is* the rectangle; fitting it only loses that. The fit also
/// cost the most of any edge: the frame was the slowest boundary on 161 of the 246 screen
/// icons and all 7 of the 2048 px test images (31–38% of the fitter's CPU), and because
/// [`fit_edge`] drops a ring's first point -- the lattice node the refinement did not
/// move, here the corner -- the fit came back as four lines and a spurious cubic that
/// chamfered the start corner: 6 parameters too many on 166 of the 246 screen icons.
///
/// Four lines, 8 parameters, O(n) to recognise. Not from the literature: the border of
/// the raster is known exactly, so there is nothing to estimate. See also: Potrace
/// (Selinger 2003, <https://potrace.sourceforge.net/potrace.pdf>), which traces a bitmap's
/// border like any other boundary.
fn frame_rectangle(pts: &[Point], closed: bool, width: usize, height: usize) -> Option<FittedPath> {
    if !closed || pts.len() < 4 {
        return None;
    }
    let (x0, y0) = (-0.5, -0.5);
    let (x1, y1) = (width as f64 - 0.5, height as f64 - 0.5);
    // Exact comparisons: the border nodes are exact binary fractions, never refined.
    let on_side = |v: f64, lo: f64, hi: f64| v == lo || v == hi;
    let within = |v: f64, lo: f64, hi: f64| (lo..=hi).contains(&v);
    let on_border = |p: &Point| {
        (on_side(p.x, x0, x1) && within(p.y, y0, y1))
            || (on_side(p.y, y0, y1) && within(p.x, x0, x1))
    };
    if !pts.iter().all(on_border) {
        return None;
    }
    let corners: Vec<Point> = pts
        .iter()
        .copied()
        .filter(|p| on_side(p.x, x0, x1) && on_side(p.y, y0, y1))
        .collect();
    // A ring on the border that reaches each corner once is the rectangle.
    let distinct = |a: &Point, b: &Point| a.x != b.x || a.y != b.y;
    if corners.len() != 4 || !(0..4).all(|k| (k + 1..4).all(|m| distinct(&corners[k], &corners[m])))
    {
        return None;
    }
    Some(FittedPath {
        start: corners[0],
        segments: corners[1..]
            .iter()
            .copied()
            .chain([corners[0]])
            .map(inkvec_fit::curves::Segment::Line)
            .collect(),
        closed: true,
    })
}

/// Fit every edge of a planar map, in parallel. Each shared edge is fitted once and both
/// of its faces draw the same curve. `fills` is each face's fill; an edge between two faces
/// of similar colour, or along a gradient, is fitted with looser tolerances.
///
/// An edge's contrast is the OKLab distance between its two faces' representative
/// colours, 1 when either face is outside `fills` (the image border), and at most
/// `FAINT / GRADIENT_LOOSEN` when either face is a gradient. The image frame of a
/// `width` × `height` raster, when one face runs round the whole border, is written as
/// the image rectangle ([`frame_rectangle`]) and not fitted. The output is in edge order,
/// whatever the thread count.
pub fn fit_edges(
    edges: &[Edge],
    fills: &[crate::gradient::FillFit],
    cfg: &FastFit,
    width: usize,
    height: usize,
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
            if let Some(path) = frame_rectangle(&e.points, e.closed, width, height) {
                return (path, None);
            }
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

    /// [`fit_edge`] as it was before a ring's denoised points were reused: the primitive
    /// test on one denoising, [`fit_points`] on a second.
    fn fit_edge_ref(
        pts: &[Point],
        closed: bool,
        cfg: &FastFit,
    ) -> (FittedPath, Option<PrimitiveFit>) {
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

    /// The reuse against [`fit_edge_ref`] on every polygon test case, open and closed, at
    /// the default and the loosest tolerances, plus circles (which the primitive takes).
    #[test]
    fn reusing_the_denoised_ring_keeps_every_fit_bit_for_bit() {
        let mut cases = polygon::tests::cases();
        for n in [0usize, 1, 2, 3, 4, 5, 7, 8, 9, 13] {
            cases.push(
                (0..n)
                    .map(|k| Point::new((k * 3 % 7) as f64, (k * 5 % 11) as f64))
                    .collect(),
            );
        }
        for r in [3.0, 12.0] {
            cases.push(
                (0..64)
                    .map(|k| {
                        let t = k as f64 / 64.0 * std::f64::consts::TAU;
                        Point::new(20.0 + r * t.cos(), 20.0 + r * t.sin())
                    })
                    .collect(),
            );
        }
        for pts in &cases {
            for contrast in [1.0, 0.0] {
                let cfg = FastFit::default().for_contrast(contrast);
                for closed in [false, true] {
                    let new = fit_edge(pts, closed, &cfg);
                    let old = fit_edge_ref(pts, closed, &cfg);
                    assert_eq!(format!("{new:?}"), format!("{old:?}"));
                }
            }
        }
    }

    /// The frame of a raster is its rectangle, four lines from the ring's first corner in
    /// the ring's own direction; an inner rectangle, an open run and a ring with a point
    /// off the border are not frames.
    #[test]
    fn the_image_frame_is_the_image_rectangle() {
        let f = polygon::tests::frame(40, 25);
        let path = frame_rectangle(&f, true, 40, 25).expect("the frame");
        assert_eq!(path.start, Point::new(-0.5, -0.5));
        let ends: Vec<Point> = path.segments.iter().map(|s| s.end()).collect();
        assert_eq!(
            ends,
            [
                Point::new(39.5, -0.5),
                Point::new(39.5, 24.5),
                Point::new(-0.5, 24.5),
                Point::new(-0.5, -0.5)
            ]
        );
        assert!(path.segments.iter().all(|s| matches!(s, Segment::Line(_))));
        // Traced the other way round, from another corner.
        let mut back: Vec<Point> = f.iter().rev().copied().collect();
        back.rotate_left(7);
        let path = frame_rectangle(&back, true, 40, 25).expect("the frame");
        assert_eq!(path.start, Point::new(-0.5, 24.5));
        assert_eq!(path.segments[0].end(), Point::new(39.5, 24.5));
        assert!(frame_rectangle(&f, false, 40, 25).is_none());
        assert!(frame_rectangle(&f, true, 41, 25).is_none());
        let mut off = f.clone();
        off[10].y += 0.25;
        assert!(frame_rectangle(&off, true, 40, 25).is_none());
        let inner: Vec<Point> = f.iter().map(|p| Point::new(p.x + 3.0, p.y + 2.0)).collect();
        assert!(frame_rectangle(&inner, true, 46, 30).is_none());
    }

    #[test]
    fn tiny_boundaries_are_lines() {
        let pts = [Point::new(0.0, 0.0), Point::new(1.0, 0.0)];
        let f = fit_points(&pts, false, &FastFit::default());
        assert_eq!(f.segments.len(), 1);
    }
}
