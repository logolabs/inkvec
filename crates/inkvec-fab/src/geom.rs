//! Polygon geometry: the few boolean and offset operations fabrication needs, over
//! `i_overlay`, in millimetres.
//!
//! A [`Region`] is a set of shapes, each an outer contour followed by its holes, the outer
//! counterclockwise and the holes clockwise (the orientation `i_overlay` returns). Every
//! operation here takes regions in that form and returns them in that form, so results can
//! be fed straight back in.

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::simplify::SimplifyShape;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::stroke::offset::StrokeOffset;
use i_overlay::mesh::float::style::{LineCap, LineJoin, OutlineStyle, StrokeStyle};

/// A point in millimetres.
pub type Pt = [f64; 2];
/// A closed contour.
pub type Contour = Vec<Pt>;
/// A shape: its outer contour, then its holes.
pub type Shape = Vec<Contour>;
/// A set of shapes.
pub type Region = Vec<Shape>;

/// Chord tolerance of round joins, as `segment length / radius`.
const ROUND_STEP: f64 = 0.25;

fn all_contours(r: &Region) -> Vec<Contour> {
    r.iter().flat_map(|s| s.iter().cloned()).collect()
}

/// Normalise raw contours (any orientation, self-intersecting) under `even_odd` or nonzero.
pub fn normalise(contours: &[Contour], even_odd: bool) -> Region {
    if contours.is_empty() {
        return Vec::new();
    }
    let rule = if even_odd {
        FillRule::EvenOdd
    } else {
        FillRule::NonZero
    };
    contours.to_vec().simplify_shape(rule)
}

/// Everything in either region.
pub fn union(a: &Region, b: &Region) -> Region {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Vec::new(),
        (true, false) => b.clone(),
        (false, true) => a.clone(),
        _ => all_contours(a).overlay(&all_contours(b), OverlayRule::Union, FillRule::NonZero),
    }
}

/// The union of many regions.
pub fn union_all<'a>(regions: impl IntoIterator<Item = &'a Region>) -> Region {
    let contours: Vec<Contour> = regions.into_iter().flat_map(all_contours).collect();
    if contours.is_empty() {
        return Vec::new();
    }
    contours.simplify_shape(FillRule::NonZero)
}

/// What is in `a` and not in `b`.
pub fn difference(a: &Region, b: &Region) -> Region {
    if a.is_empty() || b.is_empty() {
        return a.clone();
    }
    all_contours(a).overlay(&all_contours(b), OverlayRule::Difference, FillRule::NonZero)
}

/// What is in both.
pub fn intersection(a: &Region, b: &Region) -> Region {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    all_contours(a).overlay(&all_contours(b), OverlayRule::Intersect, FillRule::NonZero)
}

/// Grow (`d > 0`) or shrink (`d < 0`) a region by `d` millimetres, with round joins so a
/// grown corner stays a corner's distance away and does not spike.
pub fn offset(r: &Region, d: f64) -> Region {
    if r.is_empty() || d == 0.0 {
        return r.clone();
    }
    let style = OutlineStyle::new(d).line_join(LineJoin::Round(ROUND_STEP));
    r.outline(&style)
}

/// Morphological opening: what survives shrinking by `w / 2` and growing back. Parts and
/// necks narrower than `w` are what it removes.
pub fn opening(r: &Region, w: f64) -> Region {
    offset(&offset(r, -w / 2.0), w / 2.0)
}

/// Morphological closing: grow by `w / 2` and shrink back, which fills gaps and notches
/// narrower than `w`.
pub fn closing(r: &Region, w: f64) -> Region {
    offset(&offset(r, w / 2.0), -w / 2.0)
}

/// The outline of an open or closed polyline drawn with a round-capped pen of `width`.
pub fn stroke(path: &[Pt], width: f64, closed: bool) -> Region {
    if path.len() < 2 || width <= 0.0 {
        return Vec::new();
    }
    let style = StrokeStyle::new(width)
        .line_join(LineJoin::Round(ROUND_STEP))
        .start_cap(LineCap::Round(ROUND_STEP))
        .end_cap(LineCap::Round(ROUND_STEP));
    path.to_vec().stroke(style, closed)
}

/// Signed area of a contour (positive counterclockwise in a y-up frame).
pub fn contour_area(c: &[Pt]) -> f64 {
    let n = c.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..n {
        let (p, q) = (c[i], c[(i + 1) % n]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    0.5 * a
}

/// Area of a shape: its outer contour less its holes.
pub fn shape_area(s: &Shape) -> f64 {
    s.iter().map(|c| contour_area(c)).sum::<f64>().abs()
}

/// Area of a region.
pub fn area(r: &Region) -> f64 {
    r.iter().map(shape_area).sum()
}

/// Axis-aligned bounds `[x0, y0, x1, y1]`, or `None` when empty.
pub fn bounds(r: &Region) -> Option<[f64; 4]> {
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for p in r.iter().flatten().flatten() {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    (b[0] <= b[2]).then_some(b)
}

/// True when `p` is inside `r` (even-odd over all its contours).
pub fn contains(r: &Region, p: Pt) -> bool {
    let mut inside = false;
    for c in r.iter().flatten() {
        let n = c.len();
        for i in 0..n {
            let (a, b) = (c[i], c[(i + 1) % n]);
            if (a[1] > p[1]) != (b[1] > p[1]) {
                let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                if p[0] < x {
                    inside = !inside;
                }
            }
        }
    }
    inside
}

/// An axis-aligned rectangle as a region.
pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Region {
    vec![vec![vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]]]
}

/// The region with its holes dropped: every shape becomes its outer contour alone.
pub fn fill_holes(r: &Region) -> Region {
    let outers: Vec<Contour> = r.iter().filter_map(|s| s.first().cloned()).collect();
    normalise(&outers, false)
}

/// Mirror horizontally about `x = axis`.
pub fn mirror_x(r: &Region, axis: f64) -> Region {
    let flipped: Vec<Contour> = r
        .iter()
        .flatten()
        .map(|c| c.iter().rev().map(|p| [2.0 * axis - p[0], p[1]]).collect())
        .collect();
    normalise(&flipped, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn booleans_and_offsets_measure_as_expected() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 0.0, 15.0, 10.0);
        assert!((area(&union(&a, &b)) - 150.0).abs() < 1e-6);
        assert!((area(&intersection(&a, &b)) - 50.0).abs() < 1e-6);
        assert!((area(&difference(&a, &b)) - 50.0).abs() < 1e-6);
        // Growing a 10 mm square by 1 mm with round corners: 12x12 less the four corner
        // squares plus the four quarter-discs.
        let g = area(&offset(&a, 1.0));
        let want = 144.0 - 4.0 + std::f64::consts::PI;
        assert!((g - want).abs() < 0.2, "{g} vs {want}");
        assert!((area(&offset(&a, -1.0)) - 64.0).abs() < 1e-3);
    }

    #[test]
    fn opening_removes_a_thin_neck_but_keeps_the_body() {
        // A 10 mm square with a 0.4 mm wide, 10 mm long tail.
        let body = rect(0.0, 0.0, 10.0, 10.0);
        let tail = rect(10.0, 4.8, 20.0, 5.2);
        let r = union(&body, &tail);
        let o = opening(&r, 0.8);
        let b = bounds(&o).unwrap();
        assert!(b[2] < 10.5, "the tail is gone: {b:?}");
        assert!(area(&o) > 95.0, "the body stays");
    }
}
