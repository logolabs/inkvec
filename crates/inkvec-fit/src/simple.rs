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
//!
//! # Where this sits
//!
//! The crossing test ([`self_crossings`], [`self_crossings_touching`], and
//! [`self_crossing_points`], which also says where) is what `inkvec-cli`'s ring assembly
//! calls on every assembled ring, and it drives its own repair there: each crossing curve
//! pinned at a measured point beside the crossing
//! ([`crate::multimodel::optimal_multimodel_forced`]) or refitted under the halved cap
//! ([`crate::multimodel::optimal_multimodel_capped`]), whichever the objective prices lower.
//! The cap is what guarantees termination, as above; pinning is what keeps the repair
//! local. [`fit_simple`] is the self-contained, cap-only version of that loop for one
//! boundary. Paths are in px.

use inkvec_core::predicates::segments_intersect;
use inkvec_core::{Point, Polyline};

use crate::curves::{cubic_self_intersects, eval_cubic, Segment};
use crate::multimodel::{optimal_multimodel, optimal_multimodel_capped};
use crate::{FitConfig, FittedPath};

/// Points per curved segment when testing for crossings.
///
/// The test is on the flattened path, so this sets how fine a crossing is detectable. A
/// crossing shallower than the flattening error is also invisible in the render, so
/// there is no reason to go finer.
const FLATTEN: usize = 16;

/// Two endpoints closer than this are the same point. Coordinates are in pixels, so this
/// is far below anything the fit distinguishes.
const EPS: f64 = 1e-6;

/// Flatten one segment to a polyline, given where it starts.
///
/// Into `out` (cleared first): the start, then a line's end, [`FLATTEN`] points of a
/// cubic evenly spaced in its parameter, or an arc's end.
fn flatten(start: Point, seg: &Segment, out: &mut Vec<Point>) {
    out.clear();
    out.push(start);
    match *seg {
        Segment::Line(p) => out.push(p),
        Segment::Cubic(c1, c2, p) => {
            let q = [start, c1, c2, p];
            for k in 1..=FLATTEN {
                let t = k as f64 / FLATTEN as f64;
                out.push(eval_cubic(q, t));
            }
        }
        Segment::Arc { end, .. } => {
            // An arc is convex and cannot cross itself; its chord is enough to place it
            // against its neighbours for this test.
            out.push(end);
        }
    }
}

/// Axis-aligned bounding box of `pts` as `(x0, y0, x1, y1)`; inverted (`x0 > x1`) for
/// an empty slice, which then overlaps nothing.
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

/// Whether two `(x0, y0, x1, y1)` boxes overlap or touch.
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

/// The crossing test behind [`self_crossings`] and [`self_crossings_touching`]: up to
/// `limit` crossing pairs `(i, j)`, `i ≤ j`, in order of `i` then `j`, testing only pairs
/// with a segment in `mask` when one is given.
///
/// Every segment is flattened (`flatten`) and boxed. A cubic is tested against itself
/// exactly ([`cubic_self_intersects`]); two different segments whose boxes overlap cross
/// when any two of their flattened sub-segments intersect ([`segments_intersect`], an
/// exact predicate), except the one pair that legitimately meets where they join
/// (`exempt_join`). O(n²) in segments, pruned by the boxes.
fn self_crossings_inner(
    path: &FittedPath,
    limit: usize,
    mask: Option<&[bool]>,
) -> Vec<(usize, usize)> {
    crossings_located(path, limit, mask)
        .into_iter()
        .map(|(i, j, _)| (i, j))
        .collect()
}

/// Every pair of segments found to cross, up to `limit` pairs, as [`self_crossings`] finds
/// them, each with a point where the two cross (px).
///
/// The point is where the first pair of flattened sub-segments found to intersect meet:
/// the intersection of the two lines through them, `a + t·(b − a)` with
/// `t = ((c − a) × (d − c)) / ((b − a) × (d − c))`, or the midpoint of the shared stretch
/// when they are collinear. A cubic that loops on itself reports its own midpoint
/// (`B(½)`), the one place the loop is sure to be near. The local crossing repair reads
/// this to choose where to pin each curve (`inkvec-cli`'s `rings::repair_ring_crossings`).
pub fn self_crossing_points(path: &FittedPath, limit: usize) -> Vec<(usize, usize, Point)> {
    crossings_located(path, limit, None)
}

/// The body of [`self_crossings_inner`] and [`self_crossing_points`]: the pairs and where
/// each crosses.
fn crossings_located(
    path: &FittedPath,
    limit: usize,
    mask: Option<&[bool]>,
) -> Vec<(usize, usize, Point)> {
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
                found.push((i, i, eval_cubic([starts[i], c1, c2, p], 0.5)));
                if found.len() >= limit {
                    break 'outer;
                }
            }
        }
        for j in i + 1..n {
            if !touched(i, j) || !boxes_overlap(boxes[i], boxes[j]) {
                continue;
            }
            let exempt = exempt_join(
                (starts[i], path.segments[i].end()),
                (starts[j], path.segments[j].end()),
                flat[i].len(),
                flat[j].len(),
            );
            if let Some((a, b)) = outlines_cross(&flat[i], &flat[j], exempt) {
                let at = meet(flat[i][a], flat[i][a + 1], flat[j][b], flat[j][b + 1]);
                found.push((i, j, at));
                if found.len() >= limit {
                    break 'outer;
                }
            }
        }
    }
    found
}

/// The one pair of flattened sub-segments that legitimately meets, if segments `i` and
/// `j` (given by their end points, flattened to `ni` and `nj` points) are consecutive
/// in the chain: `(sub-segment of i, sub-segment of j)`.
///
/// The criterion is the renderer's, not a per-segment one: a ring is invalid exactly when
/// two *non-consecutive* sub-segments of its flattened outline intersect. Consecutive
/// sub-segments share a point by construction, and that single pair is the only
/// exemption.
///
/// Getting this granularity wrong is what made three earlier versions of the crossing
/// test useless. Skipping whole segments that share an endpoint hid the shape that
/// actually occurs — a cusp where a cubic doubles back across the line feeding it,
/// crossing about half a pixel from the join. Nudging the shared point instead
/// over-fired, because the chord error from flattening a cubic dwarfs any nudge small
/// enough to be safe.
///
/// Ends meet when both coordinates agree to within [`EPS`]; all four pairings are
/// checked, end-to-start first.
fn exempt_join(
    ends_i: (Point, Point),
    ends_j: (Point, Point),
    ni: usize,
    nj: usize,
) -> Option<(usize, usize)> {
    let touch = |a: Point, b: Point| (a.x - b.x).abs() < EPS && (a.y - b.y).abs() < EPS;
    if touch(ends_i.1, ends_j.0) {
        Some((ni - 2, 0))
    } else if touch(ends_j.1, ends_i.0) {
        Some((0, nj - 2))
    } else if touch(ends_i.0, ends_j.0) {
        Some((0, 0))
    } else if touch(ends_i.1, ends_j.1) {
        Some((ni - 2, nj - 2))
    } else {
        None
    }
}

/// The first sub-segment of the flattened outline `fi` found to intersect one of `fj`, other
/// than the `exempt` pair, as `(index into fi, index into fj)`; `None` when none does.
///
/// The all-pairs walk over two flattened outlines is quadratic in their sub-segments, and
/// two long arcs flattened to hundreds of points made this the dominant cost on large
/// inputs. Blocks of sixteen sub-segments carry a bounding box each, so the quadratic
/// work only happens where the outlines actually come near each other; the answer is
/// identical.
fn outlines_cross(
    fi: &[Point],
    fj: &[Point],
    exempt: Option<(usize, usize)>,
) -> Option<(usize, usize)> {
    const BLK: usize = 16;
    let (nbi, nbj) = (fi.len() - 1, fj.len() - 1);
    /// A block of consecutive segments: `(first, last, bounding box)`.
    type Block = (usize, usize, (f64, f64, f64, f64));
    let j_blocks: Vec<Block> = (0..nbj)
        .step_by(BLK)
        .map(|b0| {
            let b1 = (b0 + BLK).min(nbj);
            (b0, b1, bbox(&fj[b0..=b1]))
        })
        .collect();
    for a0 in (0..nbi).step_by(BLK) {
        let a1 = (a0 + BLK).min(nbi);
        let bb_a = bbox(&fi[a0..=a1]);
        for &(b0, b1, bb_b) in &j_blocks {
            if !boxes_overlap(bb_a, bb_b) {
                continue;
            }
            for a in a0..a1 {
                for b in b0..b1 {
                    if exempt == Some((a, b)) {
                        continue;
                    }
                    if segments_intersect(fi[a], fi[a + 1], fj[b], fj[b + 1]) {
                        return Some((a, b));
                    }
                }
            }
        }
    }
    None
}

/// Where the segments `a–b` and `c–d` (px), already known to intersect, meet: the
/// intersection of their lines, `a + t·(b − a)` with
/// `t = ((c − a) × (d − c)) / ((b − a) × (d − c))` clamped to `[0, 1]`, or, when they are
/// parallel (the cross product below 1e-18 px²), the midpoint of the middle two of the four
/// end points along `b − a`, which lies on the stretch they share.
fn meet(a: Point, b: Point, c: Point, d: Point) -> Point {
    let (r, s) = (b - a, d - c);
    let cross = |u: inkvec_core::Vec2, v: inkvec_core::Vec2| u.x * v.y - u.y * v.x;
    let den = cross(r, s);
    if den.abs() > 1e-18 {
        let t = (cross(c - a, s) / den).clamp(0.0, 1.0);
        return Point::new(a.x + t * r.x, a.y + t * r.y);
    }
    let mut ends = [a, b, c, d];
    ends.sort_by(|p, q| (p.x * r.x + p.y * r.y).total_cmp(&(q.x * r.x + q.y * r.y)));
    Point::new(0.5 * (ends[1].x + ends[2].x), 0.5 * (ends[1].y + ends[2].y))
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
    // Halve the span cap until the fit is simple. `max_span = 1` reproduces the measured
    // contour, which is simple by construction, so the search always has a valid fallback —
    // but only if the loop actually reaches cap 1. A fixed round count never did for a ring
    // longer than 512 points (eight halvings stop at a cap of two), so it returned a
    // still-crossing ring as `best`. Loop until the cap is exhausted instead.
    while cap > 1 {
        cap = (cap / 2).max(1);
        let candidate = optimal_multimodel_capped(poly, cfg, cap);
        if self_crossing(&candidate).is_none() {
            return candidate;
        }
        best = candidate;
    }
    // `max_span = 1` reproduces the measured contour, which is simple, so reaching here
    // means the contour itself was not simple — nothing this function can repair.
    best
}
