//! Primitive shapes -- circle, ellipse, rounded rectangle -- written as SVG.
//!
//! The fitter (`inkvec_fit::primitives`) replaces a whole closed boundary by one of these
//! when its three to five numbers describe the evidence more cheaply than a chain of
//! cubics. This module writes such a shape three ways, all in the traced image's pixel
//! coordinates (pixel centres at integers, so the canvas spans `-0.5..w-0.5`), rounded to
//! the emitter's decimal count:
//!
//! * as its own element (`<circle>`, `<ellipse>`, `<rect>`), [`primitive_element`], when a
//!   face's entire outline is the primitive;
//! * as path data, [`primitive_d`], when it has to share a `d` with other rings -- a hole
//!   punched in a face, or the monochrome union;
//! * as a stroke, [`stroke_element`], when the ring between two primitives is of one width
//!   ([`annulus_stroke`] decides that).
//!
//! Called from the colour emitter ([`crate::emit`]) and the monochrome emitter
//! ([`crate::mono`]). A leaf: it depends on nothing else in this crate.

use inkvec_core::Point;
use inkvec_fit::primitives::PrimitiveKind;

/// The primitive `kind` as SVG path data, to `decimals` decimals: the same shape
/// [`primitive_element`] writes, for a place where only a `d` will do.
///
/// A transparent face that fitted a primitive is punched out of the face above it, and the
/// punch has to be the *primitive*: the fitted ring it came from is a fraction of a pixel
/// away, and that difference showed as a bright seam along every inner edge of
/// `material-icons/qr_code` (dE00 0.060 -> 0.164) when the ring was punched instead.
///
/// A circle or an ellipse is two half-turn arcs between the ends of a diameter, and that is
/// the worst-conditioned arc there is: the reader places the centre `sqrt(r² - (c/2)²)` off
/// the chord (`r` the written radius, `c` the chord between the written ends), so a radius
/// written 0.005 px longer than half the rounded chord moves it by a third of a pixel.
/// Rounding the ends and the radius separately did exactly that -- the hole in
/// `material-icons/music_note` bulged 0.33 px past the circle it was cut for at top and
/// bottom (dE00 0.009 -> 0.079), `simple-icons/changedetection`'s 0.76 px (dE00 0.23 ->
/// 1.22). So the radius is written a step *shorter* than the half-chord: SVG scales a
/// radius that cannot reach both ends up until it just does (SVG 1.1 F.6.6), which puts the
/// centre on the chord's midpoint in every renderer, and the arc through the two written
/// ends is the half-turn. An ellipse gets the same treatment with both radii shrunk by one
/// factor, which F.6.6 scales back up together, so the axis ratio is kept.
///
/// A rounded rectangle is written from its two rounded *edges*, as [`primitive_element`]
/// explains, with the corner radius capped at half the shorter side. Always `Some`; the
/// `Option` lets callers chain it after a lookup that may fail.
pub(crate) fn primitive_d(kind: &PrimitiveKind, decimals: usize) -> Option<String> {
    let d = decimals;
    let step = 10f64.powi(-(d as i32));
    let q = |v: f64| -> f64 {
        let s = 10f64.powi(d as i32);
        (v * s).round() / s
    };
    // One coordinate pair, `x,y`, each to `d` decimals.
    let p = |x: f64, y: f64| format!("{x:.d$},{y:.d$}");
    Some(match *kind {
        PrimitiveKind::Circle { c, r } => {
            let (cx, cy, r) = (q(c.x), q(c.y), q(r));
            let ra = (r - step).max(0.5 * step);
            let (from, radii, to) = (p(cx - r, cy), p(ra, ra), p(cx + r, cy));
            format!("M{from}A{radii} 0 1 0 {to}A{radii} 0 1 0 {from}Z")
        }
        PrimitiveKind::Ellipse { c, rx, ry, angle } => {
            let (sn, cs) = angle.sin_cos();
            let (ax, ay) = (rx * cs, rx * sn);
            // Two steps: the written ends can each be half a step off along the axis.
            let s = ((rx - 2.0 * step) / rx.max(1e-9)).clamp(0.5, 1.0);
            let (rxa, rya) = (rx * s, ry * s);
            let (from, radii, to) = (p(c.x - ax, c.y - ay), p(rxa, rya), p(c.x + ax, c.y + ay));
            let deg = angle.to_degrees();
            format!("M{from}A{radii} {deg:.3} 1 0 {to}A{radii} {deg:.3} 1 0 {from}Z")
        }
        PrimitiveKind::RoundRect { x, y, w, h, rx } => {
            let (x0, y0) = (q(x), q(y));
            let (w, h) = (q(x + w) - x0, q(y + h) - y0);
            let (x1, y1) = (x0 + w, y0 + h);
            if rx.abs() < 1e-4 {
                format!("M{}L{}L{}L{}Z", p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1))
            } else {
                let r = rx.min(w / 2.0).min(h / 2.0);
                // Clockwise from the top edge: each side, then its corner's quarter arc.
                let rr = p(r, r);
                format!(
                    "M{}L{}A{rr} 0 0 1 {}L{}A{rr} 0 0 1 {}L{}A{rr} 0 0 1 {}L{}A{rr} 0 0 1 {}Z",
                    p(x0 + r, y0),
                    p(x1 - r, y0),
                    p(x1, y0 + r),
                    p(x1, y1 - r),
                    p(x1 - r, y1),
                    p(x0 + r, y1),
                    p(x0, y1 - r),
                    p(x0, y0 + r),
                    p(x0 + r, y0)
                )
            }
        }
    })
}

/// Largest distance, in pixels, that writing an annulus as one stroke may move any part of
/// its two boundaries: a few times the 0.02-0.06 px the boundaries are measured good to (see
/// [`crate::pathdata::EMIT_DECIMALS`]), and far below the pixel or more by which a ring that is not of one
/// width misses.
const STROKE_TOL: f64 = 0.1;

/// The one stroked primitive that paints exactly the ring between `outer` and `inner`, as
/// its midline and its width, when there is one to within [`STROKE_TOL`].
///
/// A face with a hole punched in it cannot be a primitive element, so under the cutout a
/// ring -- the frame of a window, the rim of a button -- became two primitives' worth of
/// path data: a rounded rectangle and its hole cost 72 numbers where the stacked document
/// painted a rectangle and a white one for 12, and lucide's outline icons came out with
/// 1.2x the parameters of the traced-over-white file. But a ring of one width *is* a stroke,
/// which is how the artist drew it, and a stroke leaves its inside empty: the hole stays a
/// hole in six numbers. Circles are offsets of circles and rounded rectangles of rounded
/// rectangles (outer corner `r + t/2`, inner `max(r - t/2, 0)`, square with a mitred join
/// at `r = 0`); an ellipse's offset is not an ellipse, so it is never one.
///
/// The test is how far the stroke's two edges land from the two fitted outlines, in px:
///
/// * **Circles** `(c_o, r_o)` and `(c_i, r_i)`: width `t = r_o - r_i`, midline the circle of
///   radius `(r_o + r_i)/2` about the midpoint of the centres. Moving each centre to that
///   midpoint displaces each ring by `|c_o - c_i| / 2`, which must be within tolerance.
/// * **Rounded rectangles**: the four side thicknesses `s_k` (left, top, right, bottom
///   gaps between the outlines) must all be positive; `t` is their mean, and each side's
///   edges move by `|s_k - t| / 2`. A corner of radius `ρ` has its apex `(√2 - 1)·ρ` in
///   from the corner of its bounding square, so changing a radius by `Δρ` moves the
///   outline there by `(√2 - 1)·|Δρ|`. The stroke's corner radius `r` is the mean of the
///   two where the inner corner is visibly round, else `r_o - t/2` so the outer corner is
///   exact; both outlines under `tol / (√2 - 1)` means square corners (`r = 0`).
///
/// Returns the midline primitive and the width `t` (px), or `None` when any displacement
/// exceeds [`STROKE_TOL`], the shapes are of different kinds, or the rectangle is too small
/// for its corner radius.
pub(crate) fn annulus_stroke(
    outer: &PrimitiveKind,
    inner: &PrimitiveKind,
) -> Option<(PrimitiveKind, f64)> {
    let tol = STROKE_TOL;
    // How far a corner arc's apex moves when its radius changes by one.
    let corner = std::f64::consts::SQRT_2 - 1.0;
    match (*outer, *inner) {
        (PrimitiveKind::Circle { c: co, r: ro }, PrimitiveKind::Circle { c: ci, r: ri }) => {
            let t = ro - ri;
            if t <= 0.0 || co.dist(ci) / 2.0 > tol {
                return None;
            }
            let c = Point::new(0.5 * (co.x + ci.x), 0.5 * (co.y + ci.y));
            Some((
                PrimitiveKind::Circle {
                    c,
                    r: 0.5 * (ro + ri),
                },
                t,
            ))
        }
        (
            PrimitiveKind::RoundRect {
                x: xo,
                y: yo,
                w: wo,
                h: ho,
                rx: ro,
            },
            PrimitiveKind::RoundRect {
                x: xi,
                y: yi,
                w: wi,
                h: hi,
                rx: ri,
            },
        ) => {
            let sides = [
                xi - xo,
                yi - yo,
                (xo + wo) - (xi + wi),
                (yo + ho) - (yi + hi),
            ];
            if sides.iter().any(|&s| s <= 0.0) {
                return None;
            }
            let t = sides.iter().sum::<f64>() / 4.0;
            let mut worst = sides
                .iter()
                .map(|&s| (s - t).abs() / 2.0)
                .fold(0.0f64, f64::max);
            let (ro, ri) = (ro.max(0.0), ri.max(0.0));
            let r = if ro < tol / corner && ri < tol / corner {
                // Square corners, which a stroke gets from its mitred join.
                worst = worst.max(ro.max(ri) * corner);
                0.0
            } else {
                let r = if ri > tol / corner {
                    0.5 * (ro + ri)
                } else {
                    ro - 0.5 * t
                };
                if r <= 0.0 {
                    return None;
                }
                let (po, pi) = (r + 0.5 * t, (r - 0.5 * t).max(0.0));
                worst = worst.max((po - ro).abs().max((pi - ri).abs()) * corner);
                r
            };
            let (x0, y0) = (0.5 * (xo + xi), 0.5 * (yo + yi));
            let (x1, y1) = (0.5 * (xo + wo + xi + wi), 0.5 * (yo + ho + yi + hi));
            if worst > tol || 2.0 * r > (x1 - x0).min(y1 - y0) {
                return None;
            }
            Some((
                PrimitiveKind::RoundRect {
                    x: x0,
                    y: y0,
                    w: x1 - x0,
                    h: y1 - y0,
                    rx: r,
                },
                t,
            ))
        }
        _ => None,
    }
}

/// [`primitive_element`] as a stroke of `width` px in `color` (an SVG paint, `#rrggbb`),
/// with nothing filled.
pub(crate) fn stroke_element(
    kind: &PrimitiveKind,
    width: f64,
    color: &str,
    decimals: usize,
) -> Option<String> {
    let paint = format!(" stroke=\"{color}\" stroke-width=\"{width:.decimals$}\"");
    primitive_element(kind, "none", &paint, decimals)
}

/// The primitive `kind` as its own SVG element -- `<circle>`, `<ellipse>` (with a
/// `rotate` transform when it is tilted) or `<rect>` -- painted with `fill`, with `alpha`
/// appended verbatim after the fill (an opacity attribute, a stroke, or empty).
///
/// A face whose entire boundary is a single primitive edge is written this way rather than
/// as path data: three numbers for a circle instead of twenty-four, and a shape an editor
/// lets you resize by dragging one handle. The element is written without an `id`; the
/// caller inserts one after the tag name. Always `Some`, like [`primitive_d`].
pub(crate) fn primitive_element(
    kind: &PrimitiveKind,
    fill: &str,
    alpha: &str,
    decimals: usize,
) -> Option<String> {
    let d = decimals;
    Some(match *kind {
        PrimitiveKind::Circle { c, r } => format!(
            "<circle cx=\"{:.*}\" cy=\"{:.*}\" r=\"{:.*}\" fill=\"{}\"{}/>",
            d, c.x, d, c.y, d, r, fill, alpha
        ),
        PrimitiveKind::Ellipse { c, rx, ry, angle } => {
            let deg = angle.to_degrees();
            if deg.abs() < 1e-3 {
                format!(
                    "<ellipse cx=\"{:.*}\" cy=\"{:.*}\" rx=\"{:.*}\" ry=\"{:.*}\" fill=\"{}\"{}/>",
                    d, c.x, d, c.y, d, rx, d, ry, fill, alpha
                )
            } else {
                format!(
                    "<ellipse cx=\"{:.*}\" cy=\"{:.*}\" rx=\"{:.*}\" ry=\"{:.*}\" fill=\"{}\"{} transform=\"rotate({:.3} {:.*} {:.*})\"/>",
                    d, c.x, d, c.y, d, rx, d, ry, fill, alpha, deg, d, c.x, d, c.y
                )
            }
        }
        PrimitiveKind::RoundRect { x, y, w, h, rx } => {
            // Round the two *edges*, then take the width from them. Rounding the origin
            // and the size independently moves the far edge by up to a whole step more
            // than the near one, which is how a rectangle centred exactly on a mirror
            // came out a tenth of a pixel wider on one side than the other.
            let q = |v: f64| -> f64 {
                let s = 10f64.powi(d as i32);
                (v * s).round() / s
            };
            let (x0, y0) = (q(x), q(y));
            let (w, h) = (q(x + w) - x0, q(y + h) - y0);
            if rx.abs() < 1e-4 {
                format!(
                    "<rect x=\"{:.*}\" y=\"{:.*}\" width=\"{:.*}\" height=\"{:.*}\" fill=\"{}\"{}/>",
                    d, x0, d, y0, d, w, d, h, fill, alpha
                )
            } else {
                format!(
                    "<rect x=\"{:.*}\" y=\"{:.*}\" width=\"{:.*}\" height=\"{:.*}\" rx=\"{:.*}\" fill=\"{}\"{}/>",
                    d, x0, d, y0, d, w, d, h, d, rx, fill, alpha
                )
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbers(d: &str) -> Vec<f64> {
        d.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect()
    }

    #[test]
    fn a_circle_as_path_is_centred_where_it_was_fitted() {
        // Radius and ends rounded independently put the chord a hair under the diameter,
        // and a half-turn arc then bulges by sqrt(r * error): 0.76 px on this circle.
        let kind = PrimitiveKind::Circle {
            c: Point::new(63.4962, 63.5),
            r: 57.0975,
        };
        let d = primitive_d(&kind, 2).unwrap();
        let v = numbers(&d);
        // M x0,y  A r,r 0 1 0 x1,y  A r,r 0 1 0 x0,y
        let (x0, ra, x1) = (v[0], v[2], v[7]);
        assert!(
            ra < 0.5 * (x1 - x0),
            "{d}: the radius must fall short of the half-chord"
        );
        assert!((0.5 * (x0 + x1) - 63.50).abs() < 1e-9, "{d}");
        assert!((0.5 * (x1 - x0) - 57.10).abs() < 1e-9, "{d}");
    }

    #[test]
    fn a_ring_of_one_width_is_one_stroke() {
        let c = Point::new(63.5, 63.5);
        let (mid, t) = annulus_stroke(
            &PrimitiveKind::Circle { c, r: 21.32 },
            &PrimitiveKind::Circle { c, r: 10.68 },
        )
        .expect("concentric circles are a stroke");
        assert!((t - 10.64).abs() < 1e-9);
        assert_eq!(mid, PrimitiveKind::Circle { c, r: 16.0 });

        // lucide/square-minus: the frame, fitted as two rounded rectangles.
        let outer = PrimitiveKind::RoundRect {
            x: 10.16,
            y: 10.16,
            w: 106.68,
            h: 106.68,
            rx: 16.06,
        };
        let inner = PrimitiveKind::RoundRect {
            x: 20.84,
            y: 20.84,
            w: 85.32,
            h: 85.32,
            rx: 5.29,
        };
        let (mid, t) = annulus_stroke(&outer, &inner).expect("a uniform frame is a stroke");
        assert!((t - 10.68).abs() < 1e-9);
        let PrimitiveKind::RoundRect { x, w, rx, .. } = mid else {
            panic!("a rectangle's stroke is a rectangle")
        };
        assert!((x - 15.5).abs() < 1e-9 && (w - 96.0).abs() < 1e-9 && (rx - 10.675).abs() < 1e-9);
    }

    #[test]
    fn a_ring_that_is_not_uniform_stays_a_path() {
        let c = Point::new(63.5, 63.5);
        // Off centre by a pixel: one side of the ring is two pixels thicker.
        assert!(annulus_stroke(
            &PrimitiveKind::Circle { c, r: 21.32 },
            &PrimitiveKind::Circle {
                c: Point::new(64.5, 63.5),
                r: 10.68
            },
        )
        .is_none());
        // A frame with a thicker bottom bar.
        assert!(annulus_stroke(
            &PrimitiveKind::RoundRect {
                x: 10.0,
                y: 10.0,
                w: 100.0,
                h: 100.0,
                rx: 0.0
            },
            &PrimitiveKind::RoundRect {
                x: 20.0,
                y: 20.0,
                w: 80.0,
                h: 70.0,
                rx: 0.0
            },
        )
        .is_none());
        // An ellipse's offset is not an ellipse.
        let e = |rx: f64, ry: f64| PrimitiveKind::Ellipse {
            c,
            rx,
            ry,
            angle: 0.0,
        };
        assert!(annulus_stroke(&e(30.0, 20.0), &e(20.0, 10.0)).is_none());
    }
}
