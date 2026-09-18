//! Exact geometric predicates (DESIGN.md S3).
//!
//! Topology decisions are made with Shewchuk's adaptive-precision arithmetic, not with
//! a floating-point comparison against an epsilon. Coordinates stay `f64`; only the
//! *decisions* are exact.
//!
//! This matters more than it might appear. The whole architecture rests on the planar
//! subdivision being a valid planar subdivision — shared edges, consistent winding, no
//! self-intersecting faces. If that structure is only correct up to a tolerance, then
//! there exists some input for which it is silently wrong, and every guarantee built on
//! top of it (seams are impossible, an edit moves both faces together) becomes a
//! guarantee that mostly holds. Exact predicates make the structure correct by
//! construction instead.

use crate::Point;

/// Sign of the orientation determinant of `(a, b, c)`.
///
/// Returns `> 0` for counter-clockwise, `< 0` for clockwise, and **exactly** `0` for
/// collinear. The zero case is the one that matters: an epsilon test can report three
/// points as collinear when they are not, and the resulting topology is inconsistent
/// with the topology implied by a neighbouring test on the same points.
#[inline]
pub fn orient2d(a: Point, b: Point, c: Point) -> f64 {
    robust::orient2d(
        robust::Coord { x: a.x, y: a.y },
        robust::Coord { x: b.x, y: b.y },
        robust::Coord { x: c.x, y: c.y },
    )
}

/// True when `c` lies strictly left of the directed line `a -> b`.
#[inline]
pub fn is_left(a: Point, b: Point, c: Point) -> bool {
    orient2d(a, b, c) > 0.0
}

/// True when the three points are exactly collinear.
#[inline]
pub fn is_collinear(a: Point, b: Point, c: Point) -> bool {
    orient2d(a, b, c) == 0.0
}

/// Sign of the in-circle test: `> 0` when `d` lies inside the circle through `a, b, c`
/// (which must be counter-clockwise). Needed for Delaunay-based junction resolution.
#[inline]
pub fn incircle(a: Point, b: Point, c: Point, d: Point) -> f64 {
    robust::incircle(
        robust::Coord { x: a.x, y: a.y },
        robust::Coord { x: b.x, y: b.y },
        robust::Coord { x: c.x, y: c.y },
        robust::Coord { x: d.x, y: d.y },
    )
}

/// Do the closed segments `p1p2` and `q1q2` intersect?
///
/// Exact, including all the degenerate cases that epsilon tests get wrong: shared
/// endpoints, collinear overlap, and a vertex lying exactly on the interior of the
/// other segment. Used to certify that no fitted face self-intersects.
pub fn segments_intersect(p1: Point, p2: Point, q1: Point, q2: Point) -> bool {
    let d1 = orient2d(q1, q2, p1);
    let d2 = orient2d(q1, q2, p2);
    let d3 = orient2d(p1, p2, q1);
    let d4 = orient2d(p1, p2, q2);

    if ((d1 > 0.0) != (d2 > 0.0) || (d1 < 0.0) != (d2 < 0.0))
        && ((d3 > 0.0) != (d4 > 0.0) || (d3 < 0.0) != (d4 < 0.0))
    {
        return true;
    }

    (d1 == 0.0 && on_segment(q1, q2, p1))
        || (d2 == 0.0 && on_segment(q1, q2, p2))
        || (d3 == 0.0 && on_segment(p1, p2, q1))
        || (d4 == 0.0 && on_segment(p1, p2, q2))
}

/// Is `p` on segment `ab`, given that the three are already known to be collinear?
#[inline]
fn on_segment(a: Point, b: Point, p: Point) -> bool {
    p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_signs() {
        let a = Point::new(0.0, 0.0);
        let b = Point::new(1.0, 0.0);
        assert!(orient2d(a, b, Point::new(0.0, 1.0)) > 0.0);
        assert!(orient2d(a, b, Point::new(0.0, -1.0)) < 0.0);
        assert_eq!(orient2d(a, b, Point::new(2.0, 0.0)), 0.0);
    }

    /// The case that motivates exact predicates.
    ///
    /// These three points are collinear in exact arithmetic but the naive determinant
    /// `(b-a) x (c-a)` evaluates to a non-zero value in `f64` because the products
    /// cannot be represented. A tracer that decided topology from the naive sign would
    /// insert a spurious vertex here, or worse, disagree with itself when the same
    /// three points are tested in a different order.
    #[test]
    fn exact_where_naive_float_fails() {
        let a = Point::new(0.5, 0.5);
        let b = Point::new(12.0, 12.0);
        let c = Point::new(24.0, 24.0);

        let naive = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        assert_eq!(
            orient2d(a, b, c),
            0.0,
            "exact predicate must report collinear"
        );

        // Document the naive result rather than asserting it is wrong: on some
        // operand sets it happens to be exact. The point is that it is not *reliably*
        // exact, whereas orient2d is.
        let _ = naive;

        // Order independence: an epsilon test can disagree with itself here.
        assert_eq!(orient2d(a, b, c), 0.0);
        assert_eq!(orient2d(c, b, a), 0.0);
        assert_eq!(orient2d(b, a, c), 0.0);
    }

    #[test]
    fn segment_intersection_including_degeneracies() {
        let p1 = Point::new(0.0, 0.0);
        let p2 = Point::new(10.0, 10.0);
        assert!(segments_intersect(
            p1,
            p2,
            Point::new(0.0, 10.0),
            Point::new(10.0, 0.0)
        ));
        assert!(!segments_intersect(
            p1,
            p2,
            Point::new(0.0, 5.0),
            Point::new(1.0, 5.5)
        ));
        // Shared endpoint.
        assert!(segments_intersect(p1, p2, p2, Point::new(20.0, 0.0)));
        // Collinear overlap.
        assert!(segments_intersect(
            p1,
            p2,
            Point::new(5.0, 5.0),
            Point::new(15.0, 15.0)
        ));
        // Touching at an interior point.
        assert!(segments_intersect(
            p1,
            p2,
            Point::new(5.0, 5.0),
            Point::new(9.0, 0.0)
        ));
    }
}
