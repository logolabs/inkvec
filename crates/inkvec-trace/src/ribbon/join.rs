//! How a stroke's segments meet: SVG's `stroke-linejoin`, as a residual model.
//!
//! **Round** joins paint a disc of radius `h` (the half-width) at every vertex, so a round
//! stroke is exactly the set of points within `h` of its centreline and a boundary point's
//! residual is `d(p, C) - h` ([`super::score`]).
//!
//! **Miter** joins extend the two offset edges at a convex vertex `V` until they meet. A
//! point `p` whose nearest centreline point is `V` lies in the outer wedge of the corner;
//! with `n1`, `n2` the unit normals of the incoming and outgoing tangents on `p`'s side,
//! the painted region there is `max(n1·(p-V), n2·(p-V)) <= h` -- a kite whose two outer
//! edges are the offset lines. So [`miter_gauge`] replaces `d(p, C)` by that maximum,
//! which equals the perpendicular distance to whichever offset line `p` is nearer the far
//! side of, and the same residual `gauge - h` applies. Past SVG's default miter limit
//! (the miter length over the stroke width, `1/cos(γ/2)` for a turn of `γ`, above
//! [`MITER_LIMIT`]) a renderer bevels instead: the corner is cut by the chord between the
//! two offset points, at distance `h·cos(γ/2)` from `V` along the bisector `b`, and the
//! gauge is `b·(p-V)/cos(γ/2)`, which reaches `h` on that chord.
//!
//! **Caps** ([`Cap`]): a round cap paints a half disc round each open end, so its residual
//! is the distance to the end point, as for a round join; a butt cap stops the stroke
//! square at the end point, measured by [`butt_gauge`].
//!
//! Method from: the SVG 1.1 specification, section 11.4 (stroke properties:
//! `stroke-linejoin` miter, round and bevel, and `stroke-miterlimit`, default 4),
//! <https://www.w3.org/TR/SVG11/painting.html#StrokeProperties> -- the geometry the renderer
//! paints, written as a distance-like gauge so the stroke solve can fit it.
//! See also: Berio, Leymarie, Asente, Echevarria (2022), StrokeStyles, ACM TOG 41(3),
//! doi:10.1145/3505246, which classifies glyph corners and junctions before choosing
//! strokes; here the join style is chosen per face by description length instead.

use inkvec_core::{Point, Vec2};
use inkvec_fit::curves::{arc_ellipse_center, Segment};

use super::boundary::unit;

/// SVG's default `stroke-miterlimit`: when the miter length exceeds four stroke widths,
/// i.e. `1/cos(γ/2) > 4` for a turn of `γ` (sharper than about 151°, an interior angle
/// under about 29°), a miter join is drawn bevelled.
pub(crate) const MITER_LIMIT: f64 = 4.0;

/// The join style of a face's strokes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Join {
    /// `stroke-linejoin="round"`: lucide, openmoji's lines, most line icon sets.
    Round,
    /// `stroke-linejoin="miter"` (default limit 4): sharp-cornered outline styles.
    Miter,
}

impl Join {
    /// The SVG attribute value.
    pub fn svg(self) -> &'static str {
        match self {
            Join::Round => "round",
            Join::Miter => "miter",
        }
    }
}

/// How a stroke's open ends are drawn: SVG's `stroke-linecap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    /// `stroke-linecap="round"`: a half disc of the half-width round each end.
    Round,
    /// `stroke-linecap="butt"` (SVG's default): the stroke stops square at the end point.
    Butt,
}

impl Cap {
    /// The SVG attribute value.
    pub fn svg(self) -> &'static str {
        match self {
            Cap::Round => "round",
            Cap::Butt => "butt",
        }
    }
}

/// A face's stroke style: how its segments meet and how its open ends are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    /// `stroke-linejoin`.
    pub join: Join,
    /// `stroke-linecap`.
    pub cap: Cap,
}

impl From<Join> for Style {
    /// `join` with round caps.
    fn from(join: Join) -> Style {
        Style {
            join,
            cap: Cap::Round,
        }
    }
}

/// Unit tangents of segment `s` (starting at `a`) at its start and its end, in the
/// direction of travel; `None` for a degenerate segment.
///
/// A line's direction; a cubic's first handle direction (falling back to the second
/// handle, then the chord, when handles coincide with the endpoint) and likewise at its
/// end; an arc's tangent `±R(φ)(-rx·sin θ, ry·cos θ)` at its first and last angle, signed
/// by the sweep.
pub(crate) fn tangents(a: Point, s: &Segment) -> Option<(Vec2, Vec2)> {
    match *s {
        Segment::Line(b) => {
            let t = unit(b - a)?;
            Some((t, t))
        }
        Segment::Cubic(c1, c2, b) => {
            let t0 = unit(c1 - a)
                .or_else(|| unit(c2 - a))
                .or_else(|| unit(b - a))?;
            let t1 = unit(b - c2)
                .or_else(|| unit(b - c1))
                .or_else(|| unit(b - a))?;
            Some((t0, t1))
        }
        Segment::Arc {
            rx,
            ry,
            phi,
            large_arc,
            sweep,
            end,
        } => {
            let f = arc_ellipse_center(a, rx, ry, phi, large_arc, sweep, end);
            if f.delta == 0.0 {
                let t = unit(end - a)?;
                return Some((t, t));
            }
            let sgn = f.delta.signum();
            let (sp, cp) = f.phi.sin_cos();
            let tan = |th: f64| -> Option<Vec2> {
                let (x, y) = (-f.rx * th.sin() * sgn, f.ry * th.cos() * sgn);
                unit(Vec2 {
                    x: cp * x - sp * y,
                    y: sp * x + cp * y,
                })
            };
            Some((tan(f.theta1)?, tan(f.theta1 + f.delta)?))
        }
    }
}

/// The butt-cap gauge of point `p` beyond the open end `e` of a centreline leaving along
/// the unit tangent `t_out` (pointing out of the stroke), for half-width `h`, and whether
/// the end face (rather than a side) decides it.
///
/// A butt cap stops the stroke on the line through `e` across `t_out`: near `e` the
/// painted region is `{q : (q - e)·t_out <= 0, |(q - e)×t_out| <= h}`. The gauge
/// `g = max((p - e)·t_out + h, |(p - e)×t_out|)` equals `h` exactly on that region's edge
/// -- on the end face the residual `g - h` is the signed distance to the face, and beyond
/// the sides it is the distance to the side's line -- so the same residual `g - h` as for
/// a round stroke applies. On the end face `g - h` does not depend on `h` (the face does
/// not move with the width), which the caller's Jacobian needs to know.
///
/// Method from: the SVG 1.1 specification, section 11.4 (`stroke-linecap="butt"`: the
/// stroke ends flush with the path's end point), written as a gauge like the miter's.
pub(crate) fn butt_gauge(p: Point, e: Point, t_out: Vec2, h: f64) -> (f64, bool) {
    let q = p - e;
    let along = q.dot(t_out) + h;
    let side = q.cross(t_out).abs();
    if along >= side {
        (along, true)
    } else {
        (side, false)
    }
}

/// The miter-join gauge of point `p` at vertex `v`, where a centreline arriving along
/// `t_in` leaves along `t_out` (unit tangents): see the module documentation. Comparable
/// with a distance: the painted outline is where it equals the half-width.
///
/// Each normal is taken on `p`'s side; a turn of 180° (the two tangents opposite) has no
/// corner and falls back to the distance `|p - v|`.
pub(crate) fn miter_gauge(p: Point, v: Point, t_in: Vec2, t_out: Vec2) -> f64 {
    let q = p - v;
    let side = |t: Vec2| -> Vec2 {
        let n = Vec2 { x: -t.y, y: t.x };
        if n.dot(q) < 0.0 {
            Vec2 { x: -n.x, y: -n.y }
        } else {
            n
        }
    };
    let (n1, n2) = (side(t_in), side(t_out));
    let cos_half = (0.5 * (1.0 + n1.dot(n2).clamp(-1.0, 1.0))).sqrt();
    if cos_half < 1e-6 {
        return q.norm();
    }
    if 1.0 / cos_half > MITER_LIMIT {
        let Some(b) = unit(Vec2 {
            x: n1.x + n2.x,
            y: n1.y + n2.y,
        }) else {
            return q.norm();
        };
        return b.dot(q) / cos_half;
    }
    n1.dot(q).max(n2.dot(q))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_right_angle_corner_is_a_square_miter() {
        // Arrive going +x, leave going +y, at the origin: the outer corner is towards
        // (+x, -y). With h = 1 the miter tip is at (1, -1).
        let (t_in, t_out) = (Vec2 { x: 1.0, y: 0.0 }, Vec2 { x: 0.0, y: 1.0 });
        let v = Point::new(0.0, 0.0);
        assert!((miter_gauge(Point::new(1.0, -1.0), v, t_in, t_out) - 1.0).abs() < 1e-12);
        // On the outer edge beyond the vertex, the gauge is the offset distance.
        assert!((miter_gauge(Point::new(0.5, -1.0), v, t_in, t_out) - 1.0).abs() < 1e-12);
        // A round join would put (1, -1) at sqrt 2.
        assert!((Point::new(1.0, -1.0).dist(v) - 2f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn a_butt_end_is_a_square_face() {
        // End at the origin, leaving along +x, half-width 1.
        let (e, t) = (Point::new(0.0, 0.0), Vec2 { x: 1.0, y: 0.0 });
        // Half a pixel beyond the face: residual +0.5, decided by the face (h-free).
        let (g, f) = butt_gauge(Point::new(0.5, 0.2), e, t, 1.0);
        assert!((g - 1.5).abs() < 1e-12 && f);
        // On the face itself: exactly h.
        assert!((butt_gauge(Point::new(0.0, 0.7), e, t, 1.0).0 - 1.0).abs() < 1e-12);
        // Behind the face beside the stroke: the side's distance.
        let (g, f) = butt_gauge(Point::new(-2.0, 1.3), e, t, 1.0);
        assert!((g - 1.3).abs() < 1e-12 && !f);
    }

    #[test]
    fn a_hairpin_bevels_past_the_limit() {
        // A left turn of 170°: 1/cos(85°) = 11.5 > 4, so bevelled. The outer side is the
        // right of travel, normals (t.y, -t.x); the bevel is the chord between the two
        // offset points at h = 1, and its midpoint must read exactly h.
        let a = 170f64.to_radians();
        let (t_in, t_out) = (
            Vec2 { x: 1.0, y: 0.0 },
            Vec2 {
                x: a.cos(),
                y: a.sin(),
            },
        );
        let (n1, n2) = (
            Vec2 {
                x: t_in.y,
                y: -t_in.x,
            },
            Vec2 {
                x: t_out.y,
                y: -t_out.x,
            },
        );
        let m = Point::new(0.5 * (n1.x + n2.x), 0.5 * (n1.y + n2.y));
        let g = miter_gauge(m, Point::new(0.0, 0.0), t_in, t_out);
        assert!((g - 1.0).abs() < 1e-9, "{g}");
    }

    #[test]
    fn tangents_of_lines_cubics_and_arcs() {
        let a = Point::new(0.0, 0.0);
        let (t0, t1) = tangents(a, &Segment::Line(Point::new(3.0, 4.0))).expect("a line");
        assert!((t0.x - 0.6).abs() < 1e-12 && (t1.y - 0.8).abs() < 1e-12);
        let c = Segment::Cubic(
            Point::new(1.0, 0.0),
            Point::new(2.0, 1.0),
            Point::new(2.0, 2.0),
        );
        let (t0, t1) = tangents(a, &c).expect("a cubic");
        assert!((t0.x - 1.0).abs() < 1e-12 && (t1.y - 1.0).abs() < 1e-12);
        // Quarter circle from (1,0) to (0,1) round the origin, sweep positive (increasing
        // angle in the image's own axes): starts going +y, ends going -x.
        let arc = Segment::circular_arc(1.0, false, true, Point::new(0.0, 1.0));
        let (t0, t1) = tangents(Point::new(1.0, 0.0), &arc).expect("an arc");
        assert!(
            (t0.y - 1.0).abs() < 1e-9 && (t1.x + 1.0).abs() < 1e-9,
            "{t0:?} {t1:?}"
        );
    }
}
