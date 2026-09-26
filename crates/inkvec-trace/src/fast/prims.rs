//! Whole-boundary primitives for fast mode: a closed boundary that is a circle or an
//! ellipse is written as one, three or five numbers where the curve fit spends twenty-six.
//!
//! Quality mode searches circles, ellipses and rounded rectangles under its MDL objective,
//! against the line-only optimum. Fast mode asks the cheaper question: do the quality
//! fitters' circle or ellipse pass within [`TOL`] of every measured point, and do the
//! points go all the way round it? Rounded rectangles are left to the curve fit.

use inkvec_core::{Point, Vec2};
use inkvec_fit::curves::Segment;
use inkvec_fit::primitives::{
    fit_circle, fit_circle_kasa, fit_ellipse, fit_ellipse_algebraic, PrimitiveFit, PrimitiveKind,
};

/// Largest distance, in pixels, from any measured point to the primitive.
const TOL: f64 = 0.3;
/// How far from its best circle, as a share of the radius, a ring may be before an
/// ellipse is not worth fitting.
const ROUGHLY_ROUND: f64 = 0.6;
/// Fewest points on a boundary worth trying.
const MIN_POINTS: usize = 12;
/// Smallest radius, in pixels.
const MIN_RADIUS: f64 = 1.5;
/// The ring must enclose at least this share of the primitive's area: a thin sliver
/// running along an arc and back fits a circle well and encloses nothing.
const MIN_AREA_SHARE: f64 = 0.8;

fn signed_area(pts: &[Point]) -> f64 {
    let n = pts.len();
    (0..n)
        .map(|k| {
            let (a, b) = (pts[k], pts[(k + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum::<f64>()
        * 0.5
}

/// Four cubics round the ellipse `c + R(angle) (rx cos t, ry sin t)`, starting at the
/// point nearest `from` and running the way `ccw` says (increasing `t`).
fn ellipse_cubics(
    c: Point,
    rx: f64,
    ry: f64,
    angle: f64,
    from: Point,
    ccw: bool,
) -> (Point, Vec<Segment>) {
    let (s, co) = angle.sin_cos();
    let at = |t: f64| {
        let (x, y) = (rx * t.cos(), ry * t.sin());
        Point::new(c.x + co * x - s * y, c.y + s * x + co * y)
    };
    let tangent = |t: f64| {
        let (x, y) = (-rx * t.sin(), ry * t.cos());
        Vec2 {
            x: co * x - s * y,
            y: s * x + co * y,
        }
    };
    let (dx, dy) = (from.x - c.x, from.y - c.y);
    let (qx, qy) = (co * dx + s * dy, -s * dx + co * dy);
    let t0 = (rx * qy).atan2(ry * qx);
    let step = if ccw { 1.0 } else { -1.0 } * std::f64::consts::FRAC_PI_2;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let start = at(t0);
    let mut segs = Vec::with_capacity(4);
    for i in 0..4 {
        let (ta, tb) = (t0 + step * i as f64, t0 + step * (i + 1) as f64);
        let (pa, pb) = (at(ta), if i == 3 { start } else { at(tb) });
        let (da, db) = (tangent(ta), tangent(tb));
        segs.push(Segment::Cubic(
            Point::new(pa.x + k * da.x, pa.y + k * da.y),
            Point::new(pb.x - k * db.x, pb.y - k * db.y),
            pb,
        ));
    }
    (start, segs)
}

/// The circle or ellipse `pts` (a closed boundary) is, with the path that draws it.
pub(crate) fn primitive(pts: &[Point]) -> Option<(PrimitiveFit, Point, Vec<Segment>)> {
    let n = pts.len();
    if n < MIN_POINTS {
        return None;
    }
    let area = signed_area(pts);
    // `signed_area` is positive for a ring that turns with increasing angle in these
    // coordinates, which is the direction the parametrisation above runs.
    let ccw = area > 0.0;
    let sigma = vec![0.5; n];
    let worst_of = |c: Point, r: f64| {
        pts.iter()
            .map(|p| (p.dist(c) - r).abs())
            .fold(0.0f64, f64::max)
    };
    // The one-pass algebraic circle first: most boundaries are nowhere near round, and the
    // iterative fits below are only worth running on the ones that are.
    let k = fit_circle_kasa(pts, &sigma)?;
    if !(k.r.is_finite() && k.r >= MIN_RADIUS) {
        return None;
    }
    let rough = worst_of(k.c, k.r);
    if rough > ROUGHLY_ROUND * k.r {
        return None;
    }
    if rough <= 2.0 * TOL {
        let f = fit_circle(pts, &sigma)?;
        let ok = f.r.is_finite()
            && worst_of(f.c, f.r) <= TOL
            && area.abs() >= MIN_AREA_SHARE * std::f64::consts::PI * f.r * f.r;
        if ok {
            let (start, segs) = ellipse_cubics(f.c, f.r, f.r, 0.0, pts[0], ccw);
            let prim = PrimitiveFit {
                kind: PrimitiveKind::Circle { c: f.c, r: f.r },
                chi2: f.chi2,
                params: 3.0,
            };
            return Some((prim, start, segs));
        }
    }
    let alg = fit_ellipse_algebraic(pts, &sigma)?;
    if !(alg.rx.is_finite() && alg.ry.is_finite())
        || pts.iter().any(|&p| alg.contact(p).0.abs() > 2.0 * TOL)
    {
        return None;
    }
    let e = fit_ellipse(pts, &sigma)?;
    let span = pts.iter().map(|p| p.dist(pts[0])).fold(0.0f64, f64::max);
    let ok = e.rx.is_finite()
        && e.ry.is_finite()
        && e.rx.min(e.ry) >= MIN_RADIUS
        && e.rx.max(e.ry) <= span.max(1.0)
        && area.abs() >= MIN_AREA_SHARE * std::f64::consts::PI * e.rx * e.ry
        && pts.iter().all(|&p| e.contact(p).0.abs() <= TOL);
    if !ok {
        return None;
    }
    let (start, segs) = ellipse_cubics(e.c, e.rx, e.ry, e.angle, pts[0], ccw);
    let prim = PrimitiveFit {
        kind: PrimitiveKind::Ellipse {
            c: e.c,
            rx: e.rx,
            ry: e.ry,
            angle: e.angle,
        },
        chi2: e.chi2,
        params: 5.0,
    };
    Some((prim, start, segs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(n: usize, rx: f64, ry: f64, noise: f64) -> Vec<Point> {
        (0..n)
            .map(|k| {
                let t = k as f64 / n as f64 * std::f64::consts::TAU;
                let wob = noise * ((k * 7) % 5) as f64 / 4.0;
                Point::new(50.0 + (rx + wob) * t.cos(), 40.0 + (ry + wob) * t.sin())
            })
            .collect()
    }

    #[test]
    fn a_circle_is_a_circle() {
        let (p, start, segs) = primitive(&ring(120, 20.0, 20.0, 0.1)).expect("a circle");
        assert!(matches!(p.kind, PrimitiveKind::Circle { r, .. } if (r - 20.05).abs() < 0.1));
        assert_eq!(segs.len(), 4);
        assert_eq!(segs[3].end(), start);
    }

    #[test]
    fn an_ellipse_is_an_ellipse() {
        let (p, _, _) = primitive(&ring(160, 30.0, 12.0, 0.0)).expect("an ellipse");
        assert!(matches!(p.kind, PrimitiveKind::Ellipse { .. }));
    }

    #[test]
    fn a_square_is_neither() {
        let mut pts = Vec::new();
        for k in 0..20 {
            pts.push(Point::new(k as f64, 0.0));
        }
        for k in 0..20 {
            pts.push(Point::new(20.0, k as f64));
        }
        for k in 0..20 {
            pts.push(Point::new(20.0 - k as f64, 20.0));
        }
        for k in 0..20 {
            pts.push(Point::new(0.0, 20.0 - k as f64));
        }
        assert!(primitive(&pts).is_none());
    }
}
