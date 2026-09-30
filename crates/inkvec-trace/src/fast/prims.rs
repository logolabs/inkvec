//! Whole-boundary primitives for fast mode: a closed boundary that is a circle or an
//! ellipse is written as one, three or five numbers where the curve fit spends twenty-six.
//!
//! Quality mode searches circles, ellipses and rounded rectangles under its MDL objective,
//! against the line-only optimum. Fast mode asks the cheaper question: do the quality
//! fitters' circle or ellipse pass within [`TOL`] of every measured point, and do the
//! points go all the way round it? Rounded rectangles are left to the curve fit.
//!
//! Part of the Fast fit (stage 5), tried first on every closed boundary by
//! [`super::fit_edge`]. In: the boundary's denoised sub-pixel points, in px. Out: the
//! primitive and the four cubics that draw it, or `None` to fall through to the polygon
//! and curve fit.

use inkvec_core::{Point, Vec2};
use inkvec_fit::curves::Segment;
use inkvec_fit::primitives::ellipse::{fit_ellipse_seeded, taubin_ellipse};
use inkvec_fit::primitives::{fit_circle, fit_circle_kasa, PrimitiveFit, PrimitiveKind};

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

/// Signed area of the closed polygon through `pts`, in px², by the shoelace formula
/// `A = ½ Σ_k (x_k y_{k+1} − x_{k+1} y_k)` with the index taken mod `n`. Positive when the
/// ring turns towards increasing angle in image coordinates (y down), which is clockwise
/// on screen. Zero for fewer than three distinct points.
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
///
/// Each quarter from `t_a` to `t_b = t_a ± π/2` is the standard cubic approximation of an
/// elliptic arc: end points `P(t_a)`, `P(t_b)` and control points `P(t_a) + k P'(t_a)`,
/// `P(t_b) − k P'(t_b)` with `k = 4/3 · tan(Δt / 4)` and `Δt = ±π/2`; `k` is negative
/// when running backwards, which moves the control points along `−P'`. The radial error
/// is about 0.03 % of the radius. `angle` is in radians. The start parameter
/// `t_0 = atan2(rx q_y, ry q_x)`, with `q` = `from − c` rotated by `−angle`, is the
/// parameter whose point lies on the ray from the centre through `from` -- "nearest" in
/// angle, which is what matters for keeping the path's start where the boundary's was.
/// The last cubic ends exactly on the start point so the ring closes without a gap.
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
///
/// A cascade of cheap tests before expensive fits, every point weighted with σ = 0.5 px:
///
/// 1. Kåsa's algebraic circle fit (`fit_circle_kasa`, one linear least-squares solve).
///    Reject when its radius is under [`MIN_RADIUS`] or not finite, or when some point is
///    more than `ROUGHLY_ROUND · r` from the circle: not round at all.
/// 2. When the Kåsa circle is within `2 · TOL` everywhere, the orthogonal-distance circle
///    fit (`fit_circle`, Levenberg–Marquardt) must be within [`TOL`] of every point and the
///    ring must enclose `MIN_AREA_SHARE · π r²`: accepted as a circle (3 parameters).
/// 3. Otherwise an ellipse. A bound rules most boundaries out before any ellipse fit: with
///    `span` the largest distance from the first point, an ellipse within `TOL` of all
///    points has `rx >= span/2 − TOL`, while the area test with `ry >= MIN_RADIUS` needs
///    `rx <= |A| / (MIN_AREA_SHARE · π · MIN_RADIUS)`. Then Taubin's algebraic ellipse
///    (`taubin_ellipse`) must be within `2 · TOL` of every point, and the
///    orthogonal-distance ellipse (`fit_ellipse_seeded`, Levenberg–Marquardt) within
///    `TOL`, with both radii at least `MIN_RADIUS`, the larger at most `span`, and the
///    area test passed: accepted as an ellipse (5 parameters).
///
/// Step 3 computes nothing it does not read, and nothing twice:
///
/// * the algebraic ellipse is Taubin's conic alone. The library's `fit_ellipse_algebraic`
///   also returns the ellipse's orthogonal χ², a Newton foot-point solve per point that
///   nothing here reads: measured on the phase-1 replay of the 246-icon screen set it was
///   5.0% of the whole fast fit's CPU (2.0% at 2048 px), most of it on the image-frame
///   ring, which reaches this test (a square is roughly round) and fails it;
/// * the orthogonal fit is seeded with that same conic and with the orthogonal circle
///   of step 2 when step 2 ran, where `fit_ellipse` would fit both again.
///
/// The result is bit for bit what `fit_ellipse_algebraic` then `fit_ellipse` gave: the
/// conic's geometry is the same computation with or without its χ² (the library proves
/// this in its own tests), the tests here read only the geometry, and
/// `fit_ellipse_seeded` given `taubin_ellipse(pts, σ)` and `fit_circle(pts, σ)` runs the
/// same starts in the same order as `fit_ellipse(pts, σ)`.
///
/// Method from: Taubin, G. (1991), "Estimation of planar curves, surfaces, and nonplanar
/// space curves defined by implicit equations with applications to edge and range image
/// segmentation", IEEE TPAMI 13(11):1115–1138, doi:10.1109/34.103273, for the algebraic
/// conic; Ahn, S. J., Rauh, W. & Warnecke, H.-J. (2001), "Least-squares orthogonal
/// distances fitting of circle, sphere, ellipse, hyperbola, and parabola", *Pattern
/// Recognition* 34:2283–2303, doi:10.1016/S0031-3203(00)00152-7, for the orthogonal fit
/// (both implemented in `inkvec_fit::primitives`). Not from the literature: skipping the
/// χ² and the repeated seeds, because it only removes work nothing reads.
///
/// Returns the primitive, the path's start point, and its four cubics
/// ([`ellipse_cubics`]), running in the ring's own direction from near `pts[0]`. Fewer
/// than [`MIN_POINTS`] points, a fit that fails, or a non-finite radius give `None`.
/// Cost: O(n) for the algebraic fits and tests, and O(n) per Levenberg–Marquardt
/// iteration for the orthogonal fits (up to 100 for the circle, 200 per ellipse start).
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
    // The orthogonal circle, when step 2 fits it: it is also one of the ellipse's seeds.
    let mut circle = None;
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
        circle = Some(f);
    }
    let span = pts.iter().map(|p| p.dist(pts[0])).fold(0.0f64, f64::max);
    // The orthogonal fit is iterative and costs as much as the rest of the fast fit put
    // together on a long boundary (a 3,000-point rule across a masthead: 330 ms), so it runs
    // only where the test below could pass. Every point within TOL of the ellipse puts the
    // two points `span` apart within 2 (rx + TOL) of each other, and the area test with
    // ry >= MIN_RADIUS bounds rx from above: when the two bounds cross, no ellipse passes.
    if span / 2.0 - TOL > area.abs() / (MIN_AREA_SHARE * std::f64::consts::PI * MIN_RADIUS) {
        return None;
    }
    // Taubin's conic without the orthogonal χ² `fit_ellipse_algebraic` adds: only its
    // geometry is read, here and as the orthogonal fit's first start.
    let alg = taubin_ellipse(pts, &sigma)?;
    if !(alg.rx.is_finite() && alg.ry.is_finite())
        || pts.iter().any(|&p| alg.contact(p).0.abs() > 2.0 * TOL)
    {
        return None;
    }
    // `fit_ellipse(pts, σ)` is `fit_ellipse_seeded(pts, σ, taubin_ellipse(pts, σ),
    // fit_circle(pts, σ))`; both seeds are already in hand when step 2 ran.
    let circle = circle.or_else(|| fit_circle(pts, &sigma));
    let e = fit_ellipse_seeded(pts, &sigma, Some(alg), circle)?;
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

    /// [`primitive`] as it was before its ellipse step reused its seeds: the algebraic
    /// ellipse with its unread χ², and `fit_ellipse` fitting both seeds again.
    fn primitive_ref(pts: &[Point]) -> Option<(PrimitiveFit, Point, Vec<Segment>)> {
        use inkvec_fit::primitives::{fit_ellipse, fit_ellipse_algebraic};
        let n = pts.len();
        if n < MIN_POINTS {
            return None;
        }
        let area = signed_area(pts);
        let ccw = area > 0.0;
        let sigma = vec![0.5; n];
        let worst_of = |c: Point, r: f64| {
            pts.iter()
                .map(|p| (p.dist(c) - r).abs())
                .fold(0.0f64, f64::max)
        };
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
        let span = pts.iter().map(|p| p.dist(pts[0])).fold(0.0f64, f64::max);
        if span / 2.0 - TOL > area.abs() / (MIN_AREA_SHARE * std::f64::consts::PI * MIN_RADIUS) {
            return None;
        }
        let alg = fit_ellipse_algebraic(pts, &sigma)?;
        if !(alg.rx.is_finite() && alg.ry.is_finite())
            || pts.iter().any(|&p| alg.contact(p).0.abs() > 2.0 * TOL)
        {
            return None;
        }
        let e = fit_ellipse(pts, &sigma)?;
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

    /// A rotated, noisy ellipse ring, or a rounded square when `square` is set.
    fn shape(n: usize, rx: f64, ry: f64, angle: f64, noise: f64, seed: u64) -> Vec<Point> {
        let mut rng = super::super::polygon::tests::Rng(seed);
        let (s, c) = angle.sin_cos();
        (0..n)
            .map(|k| {
                let t = k as f64 / n as f64 * std::f64::consts::TAU;
                let (x, y) = (
                    rx * t.cos() + noise * (rng.unit() - 0.5),
                    ry * t.sin() + noise * (rng.unit() - 0.5),
                );
                Point::new(60.0 + c * x - s * y, 50.0 + s * x + c * y)
            })
            .collect()
    }

    #[test]
    fn reusing_the_seeds_keeps_every_primitive_bit_for_bit() {
        let mut rings: Vec<Vec<Point>> = Vec::new();
        let mut seed = 1u64;
        for n in [12usize, 40, 150] {
            for (rx, ry) in [(8.0, 8.0), (20.0, 12.0), (30.0, 6.0), (3.0, 2.0)] {
                for angle in [0.0, 0.4, 1.2] {
                    for noise in [0.0, 0.3, 0.9] {
                        seed += 1;
                        rings.push(shape(n, rx, ry, angle, noise, seed));
                    }
                }
            }
        }
        // The image frame and a square: roughly round, and no ellipse.
        let mut frame = Vec::new();
        for x in 0..40 {
            frame.push(Point::new(x as f64 - 0.5, -0.5));
        }
        for y in 0..36 {
            frame.push(Point::new(39.5, y as f64 - 0.5));
        }
        for x in (1..=40).rev() {
            frame.push(Point::new(x as f64 - 0.5, 35.5));
        }
        for y in (1..=36).rev() {
            frame.push(Point::new(-0.5, y as f64 - 0.5));
        }
        rings.push(frame);
        let mut hits = 0;
        for r in &rings {
            let (new, old) = (primitive(r), primitive_ref(r));
            assert_eq!(format!("{new:?}"), format!("{old:?}"));
            hits += usize::from(new.is_some());
        }
        // The cases cover both outcomes.
        assert!(
            hits > 10 && hits < rings.len() - 10,
            "{hits} of {}",
            rings.len()
        );
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
