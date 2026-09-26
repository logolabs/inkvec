//! Repeated shapes redrawn from one consensus geometry.
//!
//! Marks that repeat across a drawing -- a run of tabs, tiled glyphs -- are clustered by
//! [`inkvec_fit::harmonize`] and each member redrawn from the cluster's consensus, which
//! saves the parameters of drawing near-identical shapes several times. The clustering
//! compares 48x48 masks, where a line a pixel out of place barely changes the overlap, and
//! its consensus is the member with the fewest segments; on its own it stamped one shape's
//! geometry over near-misses and cost releases up to 0.1.3 half their fidelity on the
//! screen set (dE00 0.151 -> 0.299). So nothing here is moved unless its own evidence
//! agrees; see [`harmonize`].

use std::collections::{HashMap, HashSet};

use inkvec_core::Point;
use inkvec_fit::{
    curves::Segment,
    harmonize::{
        closest_point_on_closed_polyline, cluster_compound_shapes, CompoundEquivalenceClass,
        CompoundShape,
    },
    primitives::PrimitiveFit,
    FittedPath,
};

use crate::pathdata::{fmt_segments, ring_to_segments};
use crate::FaceRings;

/// How far a harmonized shape may stray from the boundary its own pixels put it at, in
/// pixels. See [`shape_deviation`]; `INKVEC_HARMONIZE_TOL` overrides it.
///
/// Just above the trace's own accuracy (0.02-0.06 px), so a consensus moves a shape no
/// further than two traces of it disagree anyway. On the screen set 0.25 still let
/// `lucide/closed-caption` trade dE00 0.047 -> 0.107 for 2% of its parameters; at 0.1
/// the two icons that change both get cheaper and neither gets worse.
const HARMONIZE_TOL: f64 = 0.1;

/// The faces of one document, as the emitter holds them once the stacking is settled.
pub(crate) struct Faces<'a> {
    pub order: &'a [FaceRings],
    pub fitted: &'a [FittedPath],
    pub prims: &'a [Option<PrimitiveFit>],
    /// Each face's rings as points.
    pub pts: &'a [Vec<Vec<Point>>],
    /// The rings each face writes.
    pub drawn: &'a [Vec<usize>],
    /// The faces punched out of each face.
    pub holes: &'a [Vec<usize>],
    /// Faces not written at all.
    pub dropped: &'a [bool],
}

/// What harmonizing decided.
#[derive(Default)]
pub(crate) struct Harmonized {
    /// Faces whose `d` is the consensus, drawn in place.
    pub d: HashMap<usize, String>,
    /// Faces written as a `<use>` of a shared symbol: (symbol id, transform).
    pub symbols: HashMap<usize, (String, String)>,
    /// The shared symbols' definitions.
    pub defs: String,
}

/// Cluster the repeated compound shapes among `f` and decide which members take the
/// consensus.
///
/// Harmonizing moves a face's outline and its holes and nothing else, so it may only touch
/// a face no other face is drawn against. Under transparency two kinds are: a face punched
/// out of the faces below it (translucent, faded, or a clear counter whose outline the face
/// around it cuts), and a face with a painted face sitting in one of its holes. Moving one
/// side of such a pair opens a gap onto the ground or paints it twice -- nothing over
/// white, a hairline over a dark page and on the alpha channel. A ring written as a fitted
/// primitive is already the exact shape; a consensus of traced rings can only move it.
///
/// Of what is left, each member is held to its own evidence: the boundary it was traced at
/// -- solved against colour and, where the ground is transparent, against alpha -- and
/// keeps the consensus only where that lands within [`HARMONIZE_TOL`] of it everywhere.
/// The same tolerance in both directions catches a hole paired with the wrong hole. And a
/// path is only worth replacing when the consensus is cheaper than what it replaces: the
/// pass exists to save parameters, and a member already drawn as cheaply gains nothing but
/// a displacement. (Under `--use-symbols` every member becomes one `<use>`, which is the
/// saving, so there only the evidence counts.)
pub(crate) fn harmonize(
    f: &Faces<'_>,
    threshold: f64,
    use_symbols: bool,
    decimals: usize,
) -> Harmonized {
    let (shapes, face_of) = candidates(f);
    let tol = inkvec_core::env::number("INKVEC_HARMONIZE_TOL").unwrap_or(HARMONIZE_TOL);
    let mut out = Harmonized::default();
    for (idx, mut cluster) in cluster_compound_shapes(&shapes, threshold)
        .into_iter()
        .enumerate()
    {
        if cluster.members.len() < 2 {
            continue;
        }
        cluster.members = cluster
            .members
            .iter()
            .copied()
            .filter(|&m| {
                (use_symbols || cheaper(&cluster, &shapes[m]))
                    && faithful(&cluster, &shapes[m], tol)
            })
            .collect();
        if cluster.members.len() <= usize::from(use_symbols) {
            continue;
        }
        if use_symbols {
            let sym_id = format!("glyph_{idx}");
            let mut canon_d = String::new();
            fmt_segments(
                cluster.canonical_outer_start,
                &cluster.canonical_outer_segments,
                decimals,
                &mut canon_d,
            );
            for (h_start, h_segs) in &cluster.canonical_holes {
                fmt_segments(*h_start, h_segs, decimals, &mut canon_d);
            }
            out.defs
                .push_str(&format!("<path id=\"{sym_id}\" d=\"{canon_d}\"/>"));
            for &m in &cluster.members {
                out.symbols.insert(
                    face_of[m],
                    (sym_id.clone(), shapes[m].from_canonical.svg_matrix()),
                );
            }
        } else {
            for &m in &cluster.members {
                let mut d = String::new();
                for (start, segs) in stamped(&cluster, &shapes[m]) {
                    fmt_segments(start, &segs, decimals, &mut d);
                }
                out.d.insert(face_of[m], d);
            }
        }
    }
    out
}

/// The faces harmonizing may move, as compound shapes, and the face each one is.
fn candidates(f: &Faces<'_>) -> (Vec<CompoundShape>, Vec<usize>) {
    let (order, drawn, holes) = (f.order, f.drawn, f.holes);
    let punched: HashSet<usize> = holes.iter().flatten().copied().collect();
    let prim_ring = |i: usize, k: usize| {
        order[i][k].len() == 1 && f.prims.get(order[i][k][0].0).is_some_and(|p| p.is_some())
    };
    let mut shapes = Vec::new();
    let mut face_of = Vec::new();
    for i in 0..order.len() {
        if f.dropped[i] || drawn[i].len() != 1 || punched.contains(&i) || prim_ring(i, drawn[i][0])
        {
            continue;
        }
        if holes[i]
            .iter()
            .any(|&c| !f.dropped[c] || drawn[c].len() != 1 || prim_ring(c, drawn[c][0]))
        {
            continue;
        }
        let (start, segs) = ring_to_segments(&order[i][drawn[i][0]], f.fitted);
        if segs.is_empty() {
            continue;
        }
        let mut hole_rings = Vec::new();
        for &c in &holes[i] {
            for &k in &drawn[c] {
                let (h_start, h_segs) = ring_to_segments(&order[c][k], f.fitted);
                if !h_segs.is_empty() {
                    hole_rings.push((h_start, h_segs, f.pts[c][k].clone()));
                }
            }
        }
        if let Some(shape) =
            CompoundShape::new(start, segs, f.pts[i][drawn[i][0]].clone(), hole_rings)
        {
            shapes.push(shape);
            face_of.push(i);
        }
    }
    (shapes, face_of)
}

/// The cluster's consensus placed where `shape` is: its outline, then its holes.
fn stamped(
    cluster: &CompoundEquivalenceClass,
    shape: &CompoundShape,
) -> Vec<(Point, Vec<Segment>)> {
    let t = &shape.from_canonical;
    let mut rings = vec![(
        t.apply_point(cluster.canonical_outer_start),
        cluster
            .canonical_outer_segments
            .iter()
            .map(|s| t.apply_segment(s))
            .collect(),
    )];
    for (h_start, h_segs) in &cluster.canonical_holes {
        rings.push((
            t.apply_point(*h_start),
            h_segs.iter().map(|s| t.apply_segment(s)).collect(),
        ));
    }
    rings
}

/// Whether the consensus lands within `tol` of the boundary `shape` was traced at.
fn faithful(cluster: &CompoundEquivalenceClass, shape: &CompoundShape, tol: f64) -> bool {
    let own: Vec<Vec<Point>> =
        std::iter::once(run_samples(shape.outer_start, &shape.outer_segments))
            .chain(shape.holes.iter().map(|(h, segs, _)| run_samples(*h, segs)))
            .collect();
    let new: Vec<Vec<Point>> = stamped(cluster, shape)
        .iter()
        .map(|(start, segs)| run_samples(*start, segs))
        .collect();
    shape_deviation(&own, &new, tol) <= tol
}

/// Whether the consensus costs fewer parameters than `shape`'s own drawing.
fn cheaper(cluster: &CompoundEquivalenceClass, shape: &CompoundShape) -> bool {
    let cost = |segs: &[Segment]| segs.iter().map(|s| s.params()).sum::<f64>();
    let own = cost(&shape.outer_segments)
        + shape
            .holes
            .iter()
            .map(|(_, segs, _)| cost(segs))
            .sum::<f64>();
    let new = cost(&cluster.canonical_outer_segments)
        + cluster
            .canonical_holes
            .iter()
            .map(|(_, segs)| cost(segs))
            .sum::<f64>();
    new < own
}

/// Points along a run of segments, half a pixel apart, so the polyline through them is
/// within 0.02 px of the curve down to a two-pixel radius.
fn run_samples(start: Point, segs: &[Segment]) -> Vec<Point> {
    use inkvec_fit::structural::eval_segment;
    let mut out = vec![start];
    let mut cur = start;
    for s in segs {
        let mut len = 0.0;
        let mut prev = cur;
        for k in 1..=8 {
            let p = eval_segment(s, cur, k as f64 / 8.0);
            len += (p.x - prev.x).hypot(p.y - prev.y);
            prev = p;
        }
        let n = ((len / 0.5).ceil() as usize).clamp(1, 1024);
        for k in 1..=n {
            out.push(eval_segment(s, cur, k as f64 / n as f64));
        }
        cur = s.end();
    }
    out
}

/// How far one drawing of a compound shape strays from another: the largest distance from
/// a point of either to the nearest ring of the other. Stops counting once past `limit`.
fn shape_deviation(a: &[Vec<Point>], b: &[Vec<Point>], limit: f64) -> f64 {
    let one_way = |from: &[Vec<Point>], to: &[Vec<Point>]| -> f64 {
        let mut worst: f64 = 0.0;
        for &p in from.iter().flatten() {
            let near = to
                .iter()
                .filter(|r| r.len() >= 2)
                .map(|r| {
                    let q = closest_point_on_closed_polyline(p, r);
                    (p.x - q.x).hypot(p.y - q.y)
                })
                .fold(f64::INFINITY, f64::min);
            worst = worst.max(near);
            if worst > limit {
                break;
            }
        }
        worst
    };
    let ab = one_way(a, b);
    if ab > limit {
        return ab;
    }
    ab.max(one_way(b, a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_consensus_is_held_to_the_member_it_replaces() {
        let square = |x: f64, y: f64, s: f64| -> Vec<Point> {
            let o = Point::new(x, y);
            let segs = [
                Segment::Line(Point::new(x + s, y)),
                Segment::Line(Point::new(x + s, y + s)),
                Segment::Line(Point::new(x, y + s)),
                Segment::Line(o),
            ];
            run_samples(o, &segs)
        };
        let own = vec![square(10.0, 10.0, 20.0)];
        assert!(shape_deviation(&own, &own, HARMONIZE_TOL) < 1e-9);
        // A pixel over: the mask IoU of the two is 0.95, and the guard says no.
        let moved = vec![square(11.0, 10.0, 20.0)];
        assert!((shape_deviation(&own, &moved, 10.0) - 1.0).abs() < 1e-9);
        assert!(shape_deviation(&own, &moved, HARMONIZE_TOL) > HARMONIZE_TOL);
        // A hole the consensus lost is as far as the hole is from the outline.
        let with_hole = vec![square(10.0, 10.0, 20.0), square(18.0, 18.0, 4.0)];
        assert!((shape_deviation(&with_hole, &own, 10.0) - 8.0).abs() < 1e-9);
    }
}
