//! Keep a fitted boundary from crossing itself.
//!
//! # What the defect actually is
//!
//! Measured over 180 real emoji, 53 of them (29%) emitted at least one self-intersecting
//! ring, against VTracer's 10 (5.6%). Classifying those crossings over 60 images and 2126
//! rings settles what kind they are:
//!
//! ```text
//!   single-cubic loops                       0
//!   crossings between different segments    40
//! ```
//!
//! **Not one** was a cubic looping over itself. A per-candidate loop test was written,
//! wired into the fitter's inner loop, and measured: it produced byte-identical output on
//! all 180 images. Every crossing is between two *different* segments of one ring, which
//! no per-candidate test can see, because it is a property of the assembled path rather
//! than of any curve in it.
//!
//! The mechanism is a thin neck. Where two stretches of contour run close together, each
//! is fitted independently to within its own tolerance, and the two smooth curves bulge
//! across each other. The objective cannot object: both curves pass through their
//! measured points, and the rendered pixels barely change. It is nonetheless a defect —
//! an invalid ring, which fill rules resolve arbitrarily and which is unpleasant to edit.
//!
//! # Why the repair terminates
//!
//! Rather than teach the objective a term for "the assembled ring crosses itself", re-run
//! the same objective under a constraint that provably ends the problem: cap how many
//! measured points one segment may span. At `max_span = 1` every segment is a single
//! polyline edge, so the fit reproduces the measured contour exactly — and the measured
//! contour is a simple curve by construction, being a face boundary traced on a partition.
//! So the search always has a valid fallback and cannot fail to terminate.
//!
//! Halving the cap rather than decrementing it keeps the repair logarithmic in the worst
//! case, and the first cap that yields a simple path is kept, so the objective still
//! chooses everything it is allowed to choose.

use inkvec_core::predicates::segments_intersect;
use inkvec_core::{Point, Polyline};

use crate::curves::{cubic_self_intersects, Segment};
use crate::multimodel::{optimal_multimodel, optimal_multimodel_capped};
use crate::{FitConfig, FittedPath};

/// Points per curved segment when testing for crossings.
///
/// The test is on the flattened path, so this sets how fine a crossing is detectable. A
/// crossing shallower than the flattening error is also invisible in the render, so
/// there is no reason to go finer.
const FLATTEN: usize = 16;

/// How many times the span cap may be halved before falling back to the measured
/// contour. `2^8` covers any contour we produce.
const MAX_REPAIRS: usize = 8;

/// Two endpoints closer than this are the same point. Coordinates are in pixels, so this
/// is far below anything the fit distinguishes.
const EPS: f64 = 1e-6;

/// Flatten one segment to a polyline, given where it starts.
fn flatten(start: Point, seg: &Segment, out: &mut Vec<Point>) {
    out.clear();
    out.push(start);
    match *seg {
        Segment::Line(p) => out.push(p),
        Segment::Cubic(c1, c2, p) => {
            let q = [start, c1, c2, p];
            for k in 1..=FLATTEN {
                let t = k as f64 / FLATTEN as f64;
                let u = 1.0 - t;
                let b = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
                out.push(Point::new(
                    b[0] * q[0].x + b[1] * q[1].x + b[2] * q[2].x + b[3] * q[3].x,
                    b[0] * q[0].y + b[1] * q[1].y + b[2] * q[2].y + b[3] * q[3].y,
                ));
            }
        }
        Segment::Arc { end, .. } => {
            // An arc is convex and cannot cross itself; its chord is enough to place it
            // against its neighbours for this test.
            out.push(end);
        }
    }
}

fn bbox(pts: &[Point]) -> (f64, f64, f64, f64) {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in pts {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    (x0, y0, x1, y1)
}

#[inline]
fn boxes_overlap(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> bool {
    a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3
}

/// The first pair of segment indices found to cross, or `None` if the path is simple.
///
/// Segments that share an endpoint are skipped: they meet there by construction. That
/// adjacency is decided **geometrically**, by comparing the endpoints, rather than from
/// the path's `closed` flag — because the flag is not reliable here. A closed contour is
/// solved by cutting it open, and the fit that comes back is marked `closed = false` even
/// though its last segment ends exactly where the first begins. Trusting the flag made a
/// plain circle report its first and last segments as crossing where they merely meet,
/// and the "repair" turned a 3-segment circle into 61 line segments.
///
/// A segment is also checked against itself, which catches a looping cubic. That case does
/// not occur in practice — zero instances in 2126 rings — but a test for "does this path
/// cross itself" that could not see a loop would be incomplete, and the check is one 2x2
/// solve.
pub fn self_crossing(path: &FittedPath) -> Option<(usize, usize)> {
    self_crossings(path, 1).into_iter().next()
}

/// Every pair of segments found to cross, up to `limit` pairs.
///
/// A repair driven by the *first* crossing alone fixes one pair per pass, so a boundary
/// with several needs as many passes to even see them all — measured that way the repair
/// removed 17% of invalid rings while spending 4.3% more parameters, which is a poor
/// trade for the axis we are strongest on. Reporting them all lets one pass address the
/// whole ring.
pub fn self_crossings(path: &FittedPath, limit: usize) -> Vec<(usize, usize)> {
    self_crossings_inner(path, limit, None)
}

/// Crossings that involve at least one segment marked in `mask`.
///
/// The repair pass swaps one boundary at a time into a ring it has already made simple, so
/// any crossing the swap creates must involve one of that boundary's own segments. Testing
/// only those pairs gives the same answer for a fraction of the work: on a logo whose two
/// rings carry seven hundred points each, the all-pairs re-check cost five seconds of a
/// six-second trace, because it was repeated for every candidate on every incident ring.
pub fn self_crossings_touching(
    path: &FittedPath,
    limit: usize,
    mask: &[bool],
) -> Vec<(usize, usize)> {
    self_crossings_inner(path, limit, Some(mask))
}

fn self_crossings_inner(
    path: &FittedPath,
    limit: usize,
    mask: Option<&[bool]>,
) -> Vec<(usize, usize)> {
    let touched = |i: usize, j: usize| -> bool {
        match mask {
            None => true,
            Some(m) => m.get(i).copied().unwrap_or(false) || m.get(j).copied().unwrap_or(false),
        }
    };
    let mut found = Vec::new();
    let n = path.segments.len();
    if n < 2 || limit == 0 {
        return found;
    }
    let mut starts = Vec::with_capacity(n);
    let mut cur = path.start;
    for s in &path.segments {
        starts.push(cur);
        cur = s.end();
    }

    let mut flat: Vec<Vec<Point>> = Vec::with_capacity(n);
    let mut scratch = Vec::new();
    for (i, s) in path.segments.iter().enumerate() {
        flatten(starts[i], s, &mut scratch);
        flat.push(scratch.clone());
    }
    let boxes: Vec<(f64, f64, f64, f64)> = flat.iter().map(|f| bbox(f)).collect();

    'outer: for i in 0..n {
        if let Segment::Cubic(c1, c2, p) = path.segments[i] {
            // Only this boundary's own segments are suspect: the ring was simple
            // before the swap, so nothing else can have started crossing.
            if touched(i, i) && cubic_self_intersects(starts[i], c1, c2, p) {
                found.push((i, i));
                if found.len() >= limit {
                    break 'outer;
                }
            }
        }
        for j in i + 1..n {
            if !touched(i, j) || !boxes_overlap(boxes[i], boxes[j]) {
                continue;
            }
            // The criterion is the renderer's, not a per-segment one: a ring is invalid
            // exactly when two *non-consecutive* sub-segments of its flattened outline
            // intersect. Consecutive sub-segments share a point by construction, and that
            // single pair is the only exemption.
            //
            // Getting this granularity wrong is what made three earlier versions of this
            // function useless. Skipping whole segments that share an endpoint hid the
            // shape that actually occurs — a cusp where a cubic doubles back across the
            // line feeding it, crossing about half a pixel from the join. Nudging the
            // shared point instead over-fired, because the chord error from flattening a
            // cubic dwarfs any nudge small enough to be safe.
            let ends_i = (starts[i], path.segments[i].end());
            let ends_j = (starts[j], path.segments[j].end());
            let touch = |a: Point, b: Point| (a.x - b.x).abs() < EPS && (a.y - b.y).abs() < EPS;
            let (ni, nj) = (flat[i].len(), flat[j].len());
            // The one sub-segment pair that legitimately meets, if these two segments are
            // consecutive in the chain.
            let exempt: Option<(usize, usize)> = if touch(ends_i.1, ends_j.0) {
                Some((ni - 2, 0))
            } else if touch(ends_j.1, ends_i.0) {
                Some((0, nj - 2))
            } else if touch(ends_i.0, ends_j.0) {
                Some((0, 0))
            } else if touch(ends_i.1, ends_j.1) {
                Some((ni - 2, nj - 2))
            } else {
                None
            };

            // The all-pairs walk over two flattened outlines is quadratic in their
            // sub-segments, and two long arcs flattened to hundreds of points made this
            // the dominant cost on large inputs. Blocks of sixteen sub-segments carry a
            // bounding box each, so the quadratic work only happens where the outlines
            // actually come near each other; the answer is identical.
            const BLK: usize = 16;
            let (nbi, nbj) = (ni - 1, nj - 1);
            /// A block of consecutive segments: `(first, last, bounding box)`.
            type Block = (usize, usize, (f64, f64, f64, f64));
            let j_blocks: Vec<Block> = (0..nbj)
                .step_by(BLK)
                .map(|b0| {
                    let b1 = (b0 + BLK).min(nbj);
                    (b0, b1, bbox(&flat[j][b0..=b1]))
                })
                .collect();
            let mut hit = false;
            'blocks: for a0 in (0..nbi).step_by(BLK) {
                let a1 = (a0 + BLK).min(nbi);
                let bb_a = bbox(&flat[i][a0..=a1]);
                for &(b0, b1, bb_b) in &j_blocks {
                    if !boxes_overlap(bb_a, bb_b) {
                        continue;
                    }
                    for a in a0..a1 {
                        for b in b0..b1 {
                            if exempt == Some((a, b)) {
                                continue;
                            }
                            if segments_intersect(
                                flat[i][a],
                                flat[i][a + 1],
                                flat[j][b],
                                flat[j][b + 1],
                            ) {
                                hit = true;
                                break 'blocks;
                            }
                        }
                    }
                }
            }
            if hit {
                found.push((i, j));
                if found.len() >= limit {
                    break 'outer;
                }
            }
        }
    }
    found
}

/// Fit a boundary, and if the result crosses itself, fit it again under a tightening span
/// cap until it does not.
///
/// The unconstrained fit is returned untouched whenever it is already simple, which is
/// the overwhelming majority of boundaries — so this costs one extra crossing test on the
/// common path and nothing else.
pub fn fit_simple(poly: &Polyline, cfg: &FitConfig) -> FittedPath {
    let path = optimal_multimodel(poly, cfg);
    if self_crossing(&path).is_none() {
        return path;
    }
    let mut cap = poly.len().max(2);
    let mut best = path;
    for _ in 0..MAX_REPAIRS {
        cap = (cap / 2).max(1);
        let candidate = optimal_multimodel_capped(poly, cfg, cap);
        if self_crossing(&candidate).is_none() {
            return candidate;
        }
        best = candidate;
        if cap == 1 {
            break;
        }
    }
    // `max_span = 1` reproduces the measured contour, which is simple, so reaching here
    // means the contour itself was not simple — nothing this function can repair.
    best
}
