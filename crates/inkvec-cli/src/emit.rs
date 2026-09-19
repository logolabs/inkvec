//! Writing the document.
//!
//! Everything here turns fitted geometry into SVG text, and nothing here decides anything
//! about the image — by the time these run, the curves, the fills and the stacking order
//! are settled. Two things are worth knowing before reading:
//!
//! * **Coordinates are written to `EMIT_DECIMALS`, not to `--precision`.** The geometry
//!   arrives good to hundredths of a pixel and rounding it to tenths threw away 7% of the
//!   fidelity on the full corpus, which is more than most of the fitter earns. The digit
//!   count is deliberately the emitter's own and not the fitter's: pricing coordinates more
//!   finely while still rounding coarsely made the corpus *worse*, so `--precision` sets
//!   lambda and `INKVEC_EMIT_DECIMALS` sets the digits. This header said `--precision`
//!   until 2026-09-08.
//! * **Same-coloured siblings become one even-odd path.** An artist draws a letter and its
//!   counter as one path with a hole; emitting them as two costs a shape and leaves a seam
//!   along the shared edge. `fill-rule="evenodd"` says the same thing in fewer marks, and
//!   the parity rules in [`crate::rings`] are what make it correct.

use inkvec_core::Point;
use inkvec_fit::{
    curves::Segment,
    primitives::{PrimitiveFit, PrimitiveKind},
    FittedPath,
};
use inkvec_trace::gradient;

use crate::alpha::{unmatte, AlphaRamp};
use crate::colour_name;
use crate::rings::{
    containment, interior_probes, point_in_ring, ring_area, ring_inside, ring_points,
};
use crate::{FaceRings, Layers, Ring};

/// Decimals written per coordinate.
///
/// This used to be derived from `precision`, which at the default of 0.1 wrote one
/// decimal and so rounded every coordinate to a tenth of a pixel. That was throwing the
/// geometry away. Displacing the ground truth by a known sub-pixel amount and inverting
/// the error it produces (via sub-pixel boundary calibration) measures
/// the boundaries this tracer produces as accurate to **0.021 px on lucide and 0.060 px at
/// worst** -- between two and five times finer than the grid they were being written on,
/// so up to half the emitted error was quantisation of an answer that was already right.
///
/// A coordinate should not be rounded more coarsely than the geometry is accurate, and
/// there is no reason to write it finer either. Two decimals, 0.01 px, is the first grid
/// below the measured accuracy; the full 980-icon devset agrees, and stops agreeing
/// immediately afterwards, which is what a bound being reached looks like:
///
///   1 decimal   objective 0.4922      2 decimals  objective 0.4601
///   3 decimals  objective 0.4599 -- a fortieth of the gain, for another digit everywhere
///
/// Every family improves at two decimals, material-icons by 40 % and simple-icons by 16 %,
/// and the parameter count against the artist does not move at all: this buys accuracy
/// with digits, not with shapes. `INKVEC_EMIT_DECIMALS` still overrides, which is how the
/// rounding was separated from the segment price in the first place -- a finer price with
/// the old rounding makes the corpus *worse* (0.4790 against 0.4760), so this is the
/// emitter's bound and not the fitter's.
const EMIT_DECIMALS: usize = 2;

/// Smallest area, in square pixels, that a ring has to enclose to be worth emitting.
const MIN_RING_AREA: f64 = 0.25;

/// Serialise one fitted path. `fmt_ring` does this for a closed ring of planar
/// edges; a stroke is a single open or closed curve and needs no ring walk.
pub(crate) fn fmt_fitted(path: &FittedPath, closed: bool, decimals: usize, d: &mut String) {
    d.push_str(&format!(
        "M{:.*},{:.*}",
        decimals, path.start.x, decimals, path.start.y
    ));
    // `S` restates a cubic whose first control point is the reflection of the previous
    // one's second, which is exactly what `merge::snap_smooth_joins` arranges. Written
    // out it is the same curve in two fewer numbers; the reader reconstructs the handle.
    // Detected here from the geometry rather than carried on the segment, so nothing
    // upstream has to track it and a curve that happens to be smooth gets the short form
    // for free.
    let mut prev_c2: Option<(Point, Point)> = None; // (that segment's c2, its end)
    for seg in &path.segments {
        if let Segment::Cubic(c1, c2, p) = *seg {
            if let Some((pc2, pp3)) = prev_c2 {
                let want = Point::new(2.0 * pp3.x - pc2.x, 2.0 * pp3.y - pc2.y);
                if want.dist(c1) < 5e-4 {
                    d.push_str(&format!(
                        "S{:.*},{:.*} {:.*},{:.*}",
                        decimals, c2.x, decimals, c2.y, decimals, p.x, decimals, p.y
                    ));
                    prev_c2 = Some((c2, p));
                    continue;
                }
            }
            prev_c2 = Some((c2, p));
        } else {
            prev_c2 = None;
        }
        match *seg {
            Segment::Line(p) => d.push_str(&format!("L{:.*},{:.*}", decimals, p.x, decimals, p.y)),
            Segment::Cubic(a, b, p) => d.push_str(&format!(
                "C{:.*},{:.*} {:.*},{:.*} {:.*},{:.*}",
                decimals,
                a.x,
                decimals,
                a.y,
                decimals,
                b.x,
                decimals,
                b.y,
                decimals,
                p.x,
                decimals,
                p.y
            )),
            Segment::Arc {
                rx,
                ry,
                phi,
                large_arc,
                sweep,
                end,
            } => d.push_str(&format!(
                "A{:.*},{:.*} {:.3} {} {} {:.*},{:.*}",
                decimals,
                rx,
                decimals,
                ry,
                phi.to_degrees(),
                u8::from(large_arc),
                u8::from(sweep),
                decimals,
                end.x,
                decimals,
                end.y
            )),
        }
    }
    if closed {
        d.push('Z');
    }
}

pub(crate) fn emit_decimals(_precision: f64) -> usize {
    std::env::var("INKVEC_EMIT_DECIMALS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(EMIT_DECIMALS)
}

pub(crate) fn fmt_path(pts: &[Point], decimals: usize, d: &mut String) {
    if pts.len() < 3 {
        return;
    }
    d.push('M');
    for (i, p) in pts.iter().enumerate() {
        if i > 0 {
            d.push('L');
        }
        d.push_str(&format!("{:.*},{:.*}", decimals, p.x, decimals, p.y));
    }
    d.push('Z');
}

/// Serialize one ring, assembled from the shared edges that bound it.
pub(crate) fn fmt_ring(ring: &Ring, fitted: &[FittedPath], decimals: usize, d: &mut String) {
    let mut first = true;
    let mut total = 0usize;
    let mut buf = String::new();
    // (previous cubic's second control point, its end) -- carried across edge boundaries,
    // since a ring is several edges' fits concatenated and a smooth join can fall on the
    // seam between two of them.
    let mut prev_c2: Option<(Point, Point)> = None;
    for &(k, rev) in ring {
        let path = if rev {
            fitted[k].reversed()
        } else {
            fitted[k].clone()
        };
        if path.segments.is_empty() {
            continue;
        }
        if first {
            buf.push_str(&format!(
                "M{:.*},{:.*}",
                decimals, path.start.x, decimals, path.start.y
            ));
            first = false;
        }
        for seg in &path.segments {
            // `S` restates a cubic whose first control point is the reflection of the
            // previous one's second -- the same curve in two fewer numbers, which is what
            // `merge::snap_smooth_joins` arranges and what artists write (60% of their
            // smooth cubic joins are exactly this).
            //
            // Tested on the *rounded* values, not the floats: those are what the reader
            // gets, and a reflection that holds only before rounding would decode to a
            // slightly different curve than the one that was fitted.
            if let Segment::Cubic(a, b, p) = *seg {
                if let Some((pc2, pp3)) = prev_c2 {
                    let r = |v: f64| {
                        let m = 10f64.powi(decimals as i32);
                        (v * m).round() / m
                    };
                    // What the reader will reconstruct from the numbers actually
                    // written. Rounding does not commute with the reflection, so testing
                    // for equality there rejects joins that are exactly reflective in
                    // full precision; what matters is only that the curve the reader
                    // rebuilds is the curve that was fitted, to within the rounding this
                    // output already accepts everywhere else.
                    let (wx, wy) = (2.0 * r(pp3.x) - r(pc2.x), 2.0 * r(pp3.y) - r(pc2.y));
                    let tol = 10f64.powi(-(decimals as i32));
                    if (a.x - wx).abs() < tol && (a.y - wy).abs() < tol {
                        buf.push_str(&format!(
                            "S{:.*},{:.*} {:.*},{:.*}",
                            decimals, b.x, decimals, b.y, decimals, p.x, decimals, p.y
                        ));
                        total += 1;
                        prev_c2 = Some((b, p));
                        continue;
                    }
                }
                prev_c2 = Some((b, p));
            } else {
                prev_c2 = None;
            }
            match *seg {
                Segment::Line(p) => {
                    buf.push_str(&format!("L{:.*},{:.*}", decimals, p.x, decimals, p.y))
                }
                Segment::Cubic(a, b, p) => buf.push_str(&format!(
                    "C{:.*},{:.*} {:.*},{:.*} {:.*},{:.*}",
                    decimals,
                    a.x,
                    decimals,
                    a.y,
                    decimals,
                    b.x,
                    decimals,
                    b.y,
                    decimals,
                    p.x,
                    decimals,
                    p.y
                )),
                Segment::Arc {
                    rx,
                    ry,
                    phi,
                    large_arc,
                    sweep,
                    end,
                } => buf.push_str(&format!(
                    "A{:.*},{:.*} {:.3} {},{} {:.*},{:.*}",
                    decimals,
                    rx,
                    decimals,
                    ry,
                    phi.to_degrees(),
                    u8::from(large_arc),
                    u8::from(sweep),
                    decimals,
                    end.x,
                    decimals,
                    end.y
                )),
            }
            total += 1;
        }
    }
    if total >= 2 {
        buf.push('Z');
        d.push_str(&buf);
    }
}

/// Assembles a ring's fitted edges into a starting point and a list of segments.
pub(crate) fn ring_to_segments(ring: &Ring, fitted: &[FittedPath]) -> (Point, Vec<Segment>) {
    let mut start = Point::new(0.0, 0.0);
    let mut segments = Vec::new();
    for &(k, rev) in ring {
        let path = if rev {
            fitted[k].reversed()
        } else {
            fitted[k].clone()
        };
        if segments.is_empty() && !path.segments.is_empty() {
            start = path.start;
        }
        segments.extend(path.segments);
    }
    (start, segments)
}

/// Serializes a sequence of fitted segments starting from `start` into SVG path data.
pub(crate) fn fmt_segments(start: Point, segments: &[Segment], decimals: usize, d: &mut String) {
    if segments.is_empty() {
        return;
    }
    d.push_str(&format!(
        "M{:.*},{:.*}",
        decimals, start.x, decimals, start.y
    ));
    for seg in segments {
        match *seg {
            Segment::Line(p) => d.push_str(&format!("L{:.*},{:.*}", decimals, p.x, decimals, p.y)),
            Segment::Cubic(a, b, p) => d.push_str(&format!(
                "C{:.*},{:.*} {:.*},{:.*} {:.*},{:.*}",
                decimals,
                a.x,
                decimals,
                a.y,
                decimals,
                b.x,
                decimals,
                b.y,
                decimals,
                p.x,
                decimals,
                p.y
            )),
            Segment::Arc {
                rx,
                ry,
                phi,
                large_arc,
                sweep,
                end,
            } => d.push_str(&format!(
                "A{:.*},{:.*} {:.3} {},{} {:.*},{:.*}",
                decimals,
                rx,
                decimals,
                ry,
                phi.to_degrees(),
                u8::from(large_arc),
                u8::from(sweep),
                decimals,
                end.x,
                decimals,
                end.y
            )),
        }
    }
    d.push('Z');
}

/// Emit the map as a **stacked** document: faces painted back to front, each drawing only
/// its outermost rings.
///
/// SVG has no way to share one curve between two fills, so a boundary between two faces
/// must appear twice in the file even though the planar map stores it once. Stacking
/// recovers most of that cost wherever faces nest: a face painted on top *supplies* the
/// hole in the face beneath, so the lower face never draws that hole at all. Concentric
/// rings become discs rather than annuli, and a shape on a background reduces to a
/// canvas-sized rectangle plus the shape.
///
/// The faces still tile exactly, so this changes the file and not the rendering — and
/// because the curve an upper face paints is bit-identical to the one the lower face
/// would have drawn, no seam can appear between them.
/// A face whose entire boundary is a single primitive edge can be written as the SVG
/// element itself rather than as path data — three numbers for a circle instead of
/// twenty-four, and a shape an editor lets you resize by dragging one handle.
/// The same shape [`primitive_element`] writes, as path data.
///
/// A transparent face that fitted a primitive is punched out of the face above it, and the
/// punch has to be the *primitive*: the fitted ring it came from is a fraction of a pixel
/// away, and that difference showed as a bright seam along every inner edge of
/// `material-icons/qr_code` (dE00 0.060 -> 0.164) when the ring was punched instead.
///
/// A circle or an ellipse is two half-turn arcs between the ends of a diameter, and that is
/// the worst-conditioned arc there is: the reader places the centre `sqrt(r² - (c/2)²)` off
/// the chord, so a radius written 0.005 px longer than half the rounded chord moves it by a
/// third of a pixel. Rounding the ends and the radius separately did exactly that -- the
/// hole in `material-icons/music_note` bulged 0.33 px past the circle it was cut for at top
/// and bottom (dE00 0.009 -> 0.079), `simple-icons/changedetection`'s 0.76 px (dE00 0.23 ->
/// 1.22). So the radius is
/// written a step *shorter* than the half-chord: SVG scales a radius that cannot reach both
/// ends up until it just does (SVG 1.1 F.6.6), which puts the centre on the chord's
/// midpoint in every renderer, and the arc through the two written ends is the half-turn.
pub(crate) fn primitive_d(kind: &PrimitiveKind, decimals: usize) -> Option<String> {
    let d = decimals;
    let step = 10f64.powi(-(d as i32));
    let q = |v: f64| -> f64 {
        let s = 10f64.powi(d as i32);
        (v * s).round() / s
    };
    Some(match *kind {
        PrimitiveKind::Circle { c, r } => {
            let (cx, cy, r) = (q(c.x), q(c.y), q(r));
            let ra = (r - step).max(0.5 * step);
            format!(
                "M{:.*},{:.*}A{:.*},{:.*} 0 1 0 {:.*},{:.*}A{:.*},{:.*} 0 1 0 {:.*},{:.*}Z",
                d,
                cx - r,
                d,
                cy,
                d,
                ra,
                d,
                ra,
                d,
                cx + r,
                d,
                cy,
                d,
                ra,
                d,
                ra,
                d,
                cx - r,
                d,
                cy
            )
        }
        PrimitiveKind::Ellipse { c, rx, ry, angle } => {
            let (sn, cs) = angle.sin_cos();
            let (ax, ay) = (rx * cs, rx * sn);
            // Two steps: the written ends can each be half a step off along the axis.
            let s = ((rx - 2.0 * step) / rx.max(1e-9)).clamp(0.5, 1.0);
            let (rxa, rya) = (rx * s, ry * s);
            format!(
                "M{:.*},{:.*}A{:.*},{:.*} {:.3} 1 0 {:.*},{:.*}A{:.*},{:.*} {:.3} 1 0 {:.*},{:.*}Z",
                d,
                c.x - ax,
                d,
                c.y - ay,
                d,
                rxa,
                d,
                rya,
                angle.to_degrees(),
                d,
                c.x + ax,
                d,
                c.y + ay,
                d,
                rxa,
                d,
                rya,
                angle.to_degrees(),
                d,
                c.x - ax,
                d,
                c.y - ay
            )
        }
        PrimitiveKind::RoundRect { x, y, w, h, rx } => {
            let (x0, y0) = (q(x), q(y));
            let (w, h) = (q(x + w) - x0, q(y + h) - y0);
            let (x1, y1) = (x0 + w, y0 + h);
            if rx.abs() < 1e-4 {
                format!(
                    "M{:.*},{:.*}L{:.*},{:.*}L{:.*},{:.*}L{:.*},{:.*}Z",
                    d, x0, d, y0, d, x1, d, y0, d, x1, d, y1, d, x0, d, y1
                )
            } else {
                let r = rx.min(w / 2.0).min(h / 2.0);
                format!(
                    "M{:.*},{:.*}L{:.*},{:.*}A{:.*},{:.*} 0 0 1 {:.*},{:.*}L{:.*},{:.*}\
A{:.*},{:.*} 0 0 1 {:.*},{:.*}L{:.*},{:.*}A{:.*},{:.*} 0 0 1 {:.*},{:.*}L{:.*},{:.*}\
A{:.*},{:.*} 0 0 1 {:.*},{:.*}Z",
                    d,
                    x0 + r,
                    d,
                    y0,
                    d,
                    x1 - r,
                    d,
                    y0,
                    d,
                    r,
                    d,
                    r,
                    d,
                    x1,
                    d,
                    y0 + r,
                    d,
                    x1,
                    d,
                    y1 - r,
                    d,
                    r,
                    d,
                    r,
                    d,
                    x1 - r,
                    d,
                    y1,
                    d,
                    x0 + r,
                    d,
                    y1,
                    d,
                    r,
                    d,
                    r,
                    d,
                    x0,
                    d,
                    y1 - r,
                    d,
                    x0,
                    d,
                    y0 + r,
                    d,
                    r,
                    d,
                    r,
                    d,
                    x0 + r,
                    d,
                    y0
                )
            }
        }
    })
}

/// Largest distance, in pixels, that writing an annulus as one stroke may move any part of
/// its two boundaries: a few times the 0.02-0.06 px the boundaries are measured good to (see
/// [`EMIT_DECIMALS`]), and far below the pixel or more by which a ring that is not of one
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
pub(crate) fn annulus_stroke(
    outer: &PrimitiveKind,
    inner: &PrimitiveKind,
) -> Option<(PrimitiveKind, f64)> {
    let tol = std::env::var("INKVEC_STROKE_TOL")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(STROKE_TOL);
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

/// [`primitive_element`] as a stroke of `width` in `color`, with nothing filled.
pub(crate) fn stroke_element(
    kind: &PrimitiveKind,
    width: f64,
    color: &str,
    decimals: usize,
) -> Option<String> {
    let paint = format!(" stroke=\"{color}\" stroke-width=\"{width:.decimals$}\"");
    primitive_element(kind, "none", &paint, decimals)
}

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

/// Takes the whole document state; bundling it into a struct would only rename the
/// arguments, not reduce them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_color(
    order: &[FaceRings],
    fitted: &[FittedPath],
    prims: &[Option<PrimitiveFit>],
    fill_fits: &[gradient::FillFit],
    pal: &inkvec_trace::Palette,
    face_color: &[usize],
    // Faces whose source pixels are transparent; they are not painted.
    clear: &[bool],
    // Punch those faces out of the faces above them (`--cutout`).
    cutout: bool,
    // Transparency was traced natively: a clear face is the ground, never paint.
    native: bool,
    no_background: bool,
    // What the source drew each face at, 1.0 for opaque. Below it, the face carries
    // `fill-opacity` and its colour is un-matted.
    opacity: &[f32],
    // Faces whose opacity fades across them, as a gradient of `stop-opacity`.
    alpha_ramps: &[Option<AlphaRamp>],
    // Faces traced natively as fades: an opacity profile (stops are greys equal to the
    // opacity) and the colour profile on the same geometry and stops.
    fades: &[Option<(gradient::FillModel, gradient::FillModel)>],
    // Translucent layers to paint over the faces once those carry the ground's colour,
    // with one ring set per layer: its own outline, taken before the merge that removed
    // its faces from the map. A face can lie under two layers, so these cannot be indexed
    // by face.
    layers: Option<Layers>,
    matte: [f32; 3],
    w: usize,
    h: usize,
    precision: f64,
    harmonize: bool,
    harmonize_threshold: f64,
    use_symbols: bool,
) -> String {
    let decimals = emit_decimals(precision);

    let pts: Vec<Vec<Vec<Point>>> = order
        .iter()
        .map(|face| face.iter().map(|r| ring_points(r, fitted)).collect())
        .collect();

    // Painted rings per face (holes dropped: in a partition every hole is another
    // face's outer boundary, painted later and on top).
    //
    // A ring enclosing no area is dropped outright. The planar map can produce faces one
    // pixel wide whose boundary walks out along a chain and straight back, and those were
    // being emitted as paths like `M35.5,11.5 L37.5,10.5 Z` — twenty-seven of them on a
    // plain green circle. They paint nothing at any resolution and cost coordinates, an
    // id, and a line in the document a person has to read past.
    //
    // The threshold is far below a pixel so that genuinely thin features survive: a
    // sliver forty pixels long and a third of a pixel wide still encloses about 13px^2.
    let solid: Vec<Vec<usize>> = (0..order.len())
        .map(|i| {
            (0..order[i].len())
                .filter(|&k| pts[i][k].len() >= 3 && ring_area(&pts[i][k]) > MIN_RING_AREA)
                .collect()
        })
        .collect();
    // `outer` is the face's outline: the rings not contained in another of its own. It
    // decides containment and paint order.
    let outer: Vec<Vec<usize>> = (0..order.len())
        .map(|i| {
            solid[i]
                .iter()
                .copied()
                .filter(|&k| {
                    !solid[i]
                        .iter()
                        .any(|&m| m != k && ring_inside(&pts[i][k], &pts[i][m]))
                })
                .collect()
        })
        .collect();

    // Drawing the holes too — making each path self-contained so any face could be
    // dropped freely — was tried and reverted. It fixes interior transparency and costs
    // more than it buys: parameters rose (the arrow 166 to 248) and DISTS regressed on
    // three of six probes (the arrow 0.0054 to 0.0273), because a ring contained inside
    // another of the same face is not reliably a hole, and punching it makes one anyway.
    let drawn = &outer;

    let parent = containment(&pts, &outer);
    // A transparent face is a hole, not a colour.
    //
    // This emitter is *stacked*: a face's hole is drawn by painting the face inside it on
    // top, so withholding paint from a transparent face shows whatever lies under it.
    // That is why only the outermost transparent faces used to be dropped, and why every
    // interior one — the counter of a letter O, the wedge inside a rounded badge — came
    // out painted in the matte colour. Innocuous while the matte was always white and on
    // a white ground; wrong everywhere else, and glaring now that the matte is chosen for
    // contrast rather than fixed.
    //
    // So the face is dropped and its outline is punched, under `evenodd`, out of the faces
    // that would otherwise paint over it: every painted ancestor up to the first ancestor
    // that is itself transparent. Stopping there is what parity requires and not a
    // shortcut — the transparent ancestor is punched out of the faces above in its own
    // right, and its ring already carries this one inside it. Punching the whole chain
    // instead re-paints the inner region, because a second ring inside an even-odd hole
    // flips it back: the counter of a heart inside a page inside a book cover came out
    // solid black that way, dE00 0.19 -> 3.33 on `lucide/book-heart`.
    // Containment here has to be certain, not likely. `ring_inside` settles the paint
    // order on a majority of probes, which is right for stacking and wrong for cutting: a
    // wedge that merely runs alongside a shape can win that vote, and punching its ring
    // into a face it is not inside paints the wedge in that face's colour instead of
    // removing it — a black wedge beside the head on `noto-emoji/emoji_u1f3cb_200d_2642`,
    // dE00 0.58 -> 1.78. So a punch needs every probe inside, and a transparent face that
    // cannot prove where it belongs is painted as it always was rather than cut by guess.
    let strictly_inside = |c: usize, p: usize| -> bool {
        outer[c].iter().any(|&kc| {
            let probes = interior_probes(&pts[c][kc]);
            !probes.is_empty()
                && outer[p].iter().any(|&kp| {
                    pts[p][kp].len() >= 3 && probes.iter().all(|&q| point_in_ring(q, &pts[p][kp]))
                })
        })
    };
    let mut holes: Vec<Vec<usize>> = vec![Vec::new(); order.len()];
    let mut dropped = vec![false; order.len()];

    let get_flat_color = |i: usize| -> Option<[f32; 3]> {
        if let Some(f) = fill_fits.get(i) {
            if let gradient::FillModel::Flat(c) = f.model {
                return Some(c);
            }
        }
        face_color.get(i).and_then(|&ci| pal.rgb.get(ci)).copied()
    };

    let (x0, y0) = (-0.5, -0.5);
    let (x1, y1) = (w as f64 - 0.5, h as f64 - 0.5);
    let canvas_bg = if no_background {
        (0..order.len()).find(|&i| {
            if parent[i].is_some() {
                return false;
            }
            if let Some(pf) = (drawn[i].len() == 1 && order[i][drawn[i][0]].len() == 1)
                .then(|| {
                    prims
                        .get(order[i][outer[i][0]][0].0)
                        .and_then(|p| p.as_ref())
                })
                .flatten()
            {
                if let PrimitiveKind::RoundRect {
                    x, y, w: rw, h: rh, ..
                } = pf.kind
                {
                    if (x + 0.5).abs() <= 0.25
                        && (y + 0.5).abs() <= 0.25
                        && (x + rw - x1).abs() <= 0.25
                        && (y + rh - y1).abs() <= 0.25
                    {
                        return true;
                    }
                }
            }
            outer[i].iter().any(|&k| {
                let ring = &pts[i][k];
                let has_corner = |cx: f64, cy: f64| {
                    ring.iter()
                        .any(|p| (p.x - cx).abs() <= 0.5 && (p.y - cy).abs() <= 0.5)
                };
                has_corner(x0, y0) && has_corner(x1, y0) && has_corner(x1, y1) && has_corner(x0, y1)
            })
        })
    } else {
        None
    };

    // Faces the colour of the canvas are background showing through -- a guess, and the
    // right one for an opaque file. Where the source was transparent under the canvas face
    // its colour is only the matte, and whether a face of that colour is paint is not a
    // guess: `clear` says so face by face, and the pass below punches the clear ones. Taking
    // every face that merely matches the matte deleted white paint: a white ring inside a
    // copper disc on a transparent PNG came out as a hole in the disc.
    let bg_known_clear = canvas_bg.is_some_and(|bg| clear.get(bg).copied().unwrap_or(false));
    if let Some(bg) = canvas_bg {
        dropped[bg] = true;
    }
    if let Some(bg) = canvas_bg.filter(|_| !bg_known_clear) {
        if let Some(bg_c) = get_flat_color(bg) {
            for c in 0..order.len() {
                if c == bg || outer[c].is_empty() {
                    continue;
                }
                if let Some(cc) = get_flat_color(c) {
                    let matches_bg = (cc[0] - bg_c[0]).abs() < 0.02
                        && (cc[1] - bg_c[1]).abs() < 0.02
                        && (cc[2] - bg_c[2]).abs() < 0.02;
                    if !matches_bg {
                        continue;
                    }
                    let mut chain = Vec::new();
                    let mut k = c;
                    let mut sure = true;
                    while let Some(p) = parent[k] {
                        if Some(p) == canvas_bg || clear.get(p).copied().unwrap_or(false) {
                            break;
                        }
                        if !strictly_inside(c, p) {
                            sure = false;
                            break;
                        }
                        chain.push(p);
                        k = p;
                    }
                    if sure && chain.len() == 1 {
                        let p = chain[0];
                        // Only punch out holes from a parent `p` if `p` is a glyph / simple container
                        // whose children are all background cutouts, NOT a complex illustration figure
                        // containing various colored foreground elements (clothes, eyes, teeth, tools).
                        let mut p_has_non_bg_children = false;
                        for other in 0..order.len() {
                            if parent[other] == Some(p) && other != p {
                                if let Some(oc) = get_flat_color(other) {
                                    let matches = (oc[0] - bg_c[0]).abs() < 0.02
                                        && (oc[1] - bg_c[1]).abs() < 0.02
                                        && (oc[2] - bg_c[2]).abs() < 0.02;
                                    if !matches {
                                        p_has_non_bg_children = true;
                                        break;
                                    }
                                } else {
                                    p_has_non_bg_children = true;
                                    break;
                                }
                            }
                        }
                        if p_has_non_bg_children {
                            continue;
                        }

                        let is_hole = outer[c].iter().any(|&kc| {
                            let c_ring = &order[c][kc];
                            order[p].iter().enumerate().any(|(kp, pr)| {
                                !outer[p].contains(&kp)
                                    && pr
                                        .iter()
                                        .any(|&(pe, _)| c_ring.iter().any(|&(ce, _)| ce == pe))
                            })
                        });
                        if is_hole {
                            dropped[c] = true;
                            if !holes[p].contains(&c) {
                                holes[p].push(c);
                            }
                        }
                    }
                }
            }
        }
    }

    // A translucent face is written as one only if nothing of ours paints under it. In a
    // stacked document its parent is painted across the whole area first, so `fill-opacity`
    // would blend with the parent instead of the page — the film frames on
    // `noto-emoji/emoji_u1f39e` came out over the strip they sit in, dE00 0.46 -> 1.43. It
    // is honest when the face already sits on the page, and when the cutout punches it out
    // of the faces below; otherwise the face keeps the colour it was measured at, baked
    // against the matte, exactly as before.
    let mut thin: Vec<f32> = opacity.to_vec();
    let no_punch = !cutout && !no_background;
    for c in 0..order.len() {
        if dropped[c] {
            continue;
        }
        let is_clear = clear.get(c).copied().unwrap_or(false);
        let is_thin = thin.get(c).copied().unwrap_or(1.0) < 1.0;
        if (!is_clear && !is_thin) || outer[c].is_empty() {
            continue;
        }
        // The faces that would paint under this one: up the chain to the first ancestor
        // that is itself transparent.
        let mut chain = Vec::new();
        let mut k = c;
        let mut sure = true;
        while let Some(p) = parent[k] {
            if clear.get(p).copied().unwrap_or(false) {
                break;
            }
            if !strictly_inside(c, p) {
                sure = false;
                break;
            }
            chain.push(p);
            k = p;
        }
        if is_clear {
            if !sure {
                // Traced natively, a clear face is the ground itself, and painting it would
                // put a matte colour where the source had nothing. Drop it and punch it out
                // of the ancestors that do contain it; without native alpha the old rule
                // stands and it is left as it was.
                if native {
                    dropped[c] = true;
                    for p in chain {
                        holes[p].push(c);
                    }
                }
                continue;
            }
            // Without the cutout the old rule stands: drop only what has nothing painted
            // under it, and let the rest paint the matte as it always did.
            if no_punch {
                dropped[c] = chain.is_empty();
                continue;
            }
            dropped[c] = true;
            for p in chain {
                holes[p].push(c);
            }
        } else if no_punch {
            // Translucency is part of what the cutout promises: writing it as
            // `fill-opacity` needs the faces underneath removed, and without the flag they
            // are not. Bake it against the matte, as before.
            thin[c] = 1.0;
        } else if chain.is_empty() {
            continue; // already on the page
        } else if sure {
            for p in chain {
                holes[p].push(c);
            }
        } else {
            thin[c] = 1.0;
        }
    }
    // A face is written with its holes under even-odd, and even-odd counts crossings: a
    // hole inside another hole of the same face fills back in, and the same hole twice
    // cancels. Every clear or translucent face is punched out of *all* its ancestors, so a
    // stack of them -- the bands of a fade -- gave the outermost band every inner outline
    // as a hole and painted alternate bands twice over. A hole already inside another of
    // the face's holes is removed with it and needs no hole of its own.
    for p in 0..holes.len() {
        if holes[p].len() < 2 {
            continue;
        }
        let set: std::collections::HashSet<usize> = holes[p].iter().copied().collect();
        let mut seen = std::collections::HashSet::new();
        holes[p].retain(|&h| {
            if !seen.insert(h) {
                return false;
            }
            let mut k = h;
            while let Some(q) = parent[k] {
                if q == p {
                    break;
                }
                if set.contains(&q) {
                    return false;
                }
                k = q;
            }
            true
        });
    }
    let drop = |i: usize| -> bool { dropped[i] };
    // A dropped face must not take its children with it. Walk up past any dropped
    // ancestor so a shape sitting inside a transparent region is still painted, at the
    // level of the nearest ancestor that survives.
    let surviving_parent = |mut i: usize| -> Option<usize> {
        while let Some(p) = parent[i] {
            if !drop(p) {
                return Some(p);
            }
            i = p;
        }
        None
    };
    if std::env::var_os("INKVEC_ALPHADBG").is_some() {
        for i in 0..order.len() {
            eprintln!(
                "  emit {i}: rings {} areas {:?} outer {} clear {:?} parent {:?} drop {} holes {:?}",
                order.get(i).map_or(0, |o| o.len()),
                pts[i].iter().map(|r| (r.len(), ring_area(r).round())).collect::<Vec<_>>(),
                outer.get(i).map_or(0, |o| o.len()),
                clear.get(i),
                parent.get(i).copied().flatten(),
                drop(i),
                holes.get(i)
            );
            if outer.get(i).is_some_and(|o| o.is_empty()) {
                for r in &order[i] {
                    for &(k, rev) in r {
                        eprintln!(
                            "      edge {k} rev {rev} start {:?} segs {:?}",
                            fitted[k].start, fitted[k].segments
                        );
                    }
                }
            }
        }
    }
    // Repeated shapes redrawn from one consensus, where their own evidence agrees; see
    // `crate::harmonize`. Its symbols come first among the definitions.
    let mut defs = String::new();
    let harmonized = if harmonize {
        crate::harmonize::harmonize(
            &crate::harmonize::Faces {
                order,
                fitted,
                prims,
                pts: &pts,
                drawn,
                holes: &holes,
                dropped: &dropped,
            },
            harmonize_threshold,
            use_symbols,
            decimals,
        )
    } else {
        crate::harmonize::Harmonized::default()
    };
    defs.push_str(&harmonized.defs);
    let (harmonized_d, symbol_use) = (harmonized.d, harmonized.symbols);

    let mut fills: Vec<String> = Vec::with_capacity(order.len());
    // A translucent face was measured *over the matte*, so its colour is un-matted before
    // it is written. Only flat fills: a gradient's stops would each need the same
    // treatment, and a translucent gradient is rare enough that keeping today's behaviour
    // beats a half-done one.
    let face_opacity = |i: usize| -> f32 { thin.get(i).copied().unwrap_or(1.0) };
    // A face that lies under a recovered layer is painted with what is *underneath* the
    // layer, not with what was observed through it: the layer itself is painted on top,
    // once, at its own opacity. That is what makes the ground continuous — the pieces a
    // layer cut a shape into become one colour again, and the emitter's own sibling
    // merging then writes them as one path.
    let base_of: Vec<Option<[f32; 3]>> = (0..order.len())
        .map(|i| {
            let (an, _) = layers?;
            an.layers
                .iter()
                .any(|l| l.faces.binary_search(&i).is_ok())
                .then(|| an.base_rgb.get(i).copied())
                .flatten()
        })
        .collect();

    for i in 0..order.len() {
        let a = face_opacity(i);
        if let Some(base) = base_of[i] {
            fills.push(inkvec_trace::color::to_hex(base));
            continue;
        }
        // A fade traced natively: linear, radial or elliptical, with the stops the fitter
        // found, all in one colour at their own opacities.
        if let Some((model, color)) = fades.get(i).and_then(|f| f.as_ref()) {
            let (frag, attr) = gradient::fade_to_svg(model, color, &format!("f{i}"));
            defs.push_str(&frag);
            fills.push(attr);
            continue;
        }
        // A fade is written as the gradient an editor would use: one colour, two
        // `stop-opacity` values, along the axis the alpha was measured to run.
        if let Some(r) = alpha_ramps.get(i).copied().flatten() {
            let hex = inkvec_trace::color::to_hex(r.color);
            defs.push_str(&format!(
                "<linearGradient id=\"a{i}\" gradientUnits=\"userSpaceOnUse\" x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\"><stop offset=\"0\" stop-color=\"{hex}\" stop-opacity=\"{:.3}\"/><stop offset=\"1\" stop-color=\"{hex}\" stop-opacity=\"{:.3}\"/></linearGradient>",
                r.p0.x, r.p0.y, r.p1.x, r.p1.y, r.a0, r.a1
            ));
            fills.push(format!("url(#a{i})"));
            continue;
        }
        match fill_fits.get(i) {
            Some(f) => match (&f.model, a < 1.0) {
                (gradient::FillModel::Flat(c), true) => {
                    fills.push(inkvec_trace::color::to_hex(unmatte(*c, a, matte)))
                }
                _ => {
                    let (frag, attr) = gradient::fill_to_svg(&f.model, &format!("g{i}"));
                    defs.push_str(&frag);
                    fills.push(attr);
                }
            },
            None => fills.push(
                face_color
                    .get(i)
                    .and_then(|&ci| pal.rgb.get(ci))
                    .map(|c| {
                        let c = if a < 1.0 { unmatte(*c, a, matte) } else { *c };
                        inkvec_trace::color::to_hex(c)
                    })
                    .unwrap_or_else(|| "#000000".into()),
            ),
        }
    }
    // The attribute each face adds after its fill. Empty for everything opaque, which is
    // every face of an image that had no alpha channel to begin with.
    let opac: Vec<String> = (0..order.len())
        .map(|i| {
            let a = face_opacity(i);
            // A fade's opacity is in its gradient's stops; stating it again would apply it
            // twice. A flat profile is the exception: its colour is written plain, so its
            // one opacity goes on the attribute as for any wash.
            if let Some((model, _)) = fades.get(i).and_then(|f| f.as_ref()) {
                return match model {
                    gradient::FillModel::Flat(c) if c[0] < 1.0 => {
                        format!(" fill-opacity=\"{:.3}\"", c[0])
                    }
                    _ => String::new(),
                };
            }
            if a < 1.0 {
                format!(" fill-opacity=\"{a:.3}\"")
            } else {
                String::new()
            }
        })
        .collect();

    let mut children: Vec<Vec<usize>> = vec![Vec::new(); order.len()];
    let mut roots: Vec<usize> = Vec::new();
    for i in 0..order.len() {
        if order[i].is_empty() || i >= fills.len() || outer[i].is_empty() || drop(i) {
            continue;
        }
        match surviving_parent(i) {
            Some(p) => children[p].push(i),
            None => roots.push(i),
        }
    }
    // Paint order within a level: larger first, so nothing hides its own children.
    let by_area = |v: &mut Vec<usize>| {
        v.sort_by(|&a, &b| {
            let fa: f64 = outer[a].iter().map(|&k| ring_area(&pts[a][k])).sum();
            let fb: f64 = outer[b].iter().map(|&k| ring_area(&pts[b][k])).sum();
            fb.partial_cmp(&fa).unwrap_or(std::cmp::Ordering::Equal)
        });
    };
    by_area(&mut roots);
    for c in children.iter_mut() {
        by_area(c);
    }

    let mut used: std::collections::HashMap<&'static str, usize> = std::collections::HashMap::new();
    let mut ids: Vec<String> = Vec::with_capacity(order.len());
    for i in 0..order.len() {
        // Name a gradient face from its own colours, not the palette: a gradient face
        // has no single palette entry, and falling through to "shape" throws away the
        // most useful half of the name.
        let base = match fill_fits.get(i).map(|f| &f.model) {
            Some(gradient::FillModel::Flat(c)) => colour_name(*c),
            Some(gradient::FillModel::Linear { c0, c1, .. })
            | Some(gradient::FillModel::Radial { c0, c1, .. }) => colour_name([
                0.5 * (c0[0] + c1[0]),
                0.5 * (c0[1] + c1[1]),
                0.5 * (c0[2] + c1[2]),
            ]),
            None => face_color
                .get(i)
                .and_then(|&ci| pal.rgb.get(ci))
                .map(|&c| colour_name(c))
                .unwrap_or("shape"),
        };
        let n = used.entry(base).or_insert(0);
        *n += 1;
        ids.push(format!("{base}-{n}"));
    }

    /// The `d` of a face: its drawn rings, then the transparent faces punched out of it,
    /// for an even-odd fill.
    #[allow(clippy::too_many_arguments)]
    fn face_d(
        i: usize,
        order: &[FaceRings],
        fitted: &[FittedPath],
        prims: &[Option<PrimitiveFit>],
        drawn: &[Vec<usize>],
        holes: &[Vec<usize>],
        decimals: usize,
        harmonized_d: &std::collections::HashMap<usize, String>,
    ) -> String {
        if let Some(h_d) = harmonized_d.get(&i) {
            return h_d.clone();
        }
        let mut d = String::new();
        // A face's own outline is its primitive too, when it fitted one, and for the same
        // reason as a hole's: it is the geometry every neighbour was drawn against. The
        // fitted ring starts wherever the edge walk did, off the primitive by as much as
        // 0.6 px, and a face only lands here with a primitive outline when a hole was
        // punched in it -- `material-icons/qr_code`'s finder squares came out with a
        // slanted top edge, `simple-icons/phpstorm`'s square with a notch in one corner.
        let mut ring_d = |ring: &Ring| match (ring.len() == 1)
            .then(|| prims.get(ring[0].0).and_then(|p| p.as_ref()))
            .flatten()
            .and_then(|pf| primitive_d(&pf.kind, decimals))
        {
            Some(p) => d.push_str(&p),
            None => fmt_ring(ring, fitted, decimals, &mut d),
        };
        for &k in &drawn[i] {
            ring_d(&order[i][k]);
        }
        for &c in &holes[i] {
            for &k in &drawn[c] {
                ring_d(&order[c][k]);
            }
        }
        d
    }

    /// The element a face is written as on its own: a primitive when its whole
    /// boundary is one, else a path.
    #[allow(clippy::too_many_arguments)]
    fn face_element(
        i: usize,
        order: &[FaceRings],
        outer: &[Vec<usize>],
        fitted: &[FittedPath],
        prims: &[Option<PrimitiveFit>],
        fills: &[String],
        opac: &[String],
        ids: &[String],
        drawn: &[Vec<usize>],
        holes: &[Vec<usize>],
        decimals: usize,
        harmonized_d: &std::collections::HashMap<usize, String>,
        symbol_use: &std::collections::HashMap<usize, (String, String)>,
        strokes: &std::collections::HashMap<usize, String>,
    ) -> Option<(String, bool)> {
        let fill = &fills[i];
        let alpha = opac[i].as_str();
        let id = &ids[i];
        if let Some((sym_id, matrix)) = symbol_use.get(&i) {
            return Some((
                format!("<use id=\"{id}\" href=\"#{sym_id}\" transform=\"{matrix}\" fill=\"{fill}\"{alpha} fill-rule=\"evenodd\"/>"),
                false,
            ));
        }
        if let Some(el) = strokes.get(&i) {
            let mut el = el.clone();
            if let Some(sp) = el.find(' ') {
                el.insert_str(sp, &format!(" id=\"{id}\""));
            }
            return Some((el, true));
        }
        // A primitive element cannot carry a hole, so a face that has to show one through
        // is written as a path even when its outline would have fitted a circle.
        if drawn[i].len() == 1 && order[i][drawn[i][0]].len() == 1 && holes[i].is_empty() {
            let (k, _) = order[i][outer[i][0]][0];
            if let Some(pf) = prims.get(k).and_then(|p| p.as_ref()) {
                if let Some(mut el) = primitive_element(&pf.kind, fill, alpha, decimals) {
                    if let Some(sp) = el.find(' ') {
                        el.insert_str(sp, &format!(" id=\"{id}\""));
                    }
                    return Some((el, true));
                }
            }
        }
        let d = face_d(
            i,
            order,
            fitted,
            prims,
            drawn,
            holes,
            decimals,
            harmonized_d,
        );
        if d.is_empty() {
            return None;
        }
        Some((
            format!("<path id=\"{id}\" d=\"{d}\" fill=\"{fill}\"{alpha} fill-rule=\"evenodd\"/>"),
            false,
        ))
    }

    /// Emit the faces of one nesting level.
    ///
    /// Siblings are disjoint by construction -- every pixel belongs to one face -- so
    /// all the siblings of one flat colour can be one compound path under even-odd,
    /// which is how an artist draws them: a whole word is one `<path>`, not one per
    /// letter. Before this the tracer wrote one element per connected face and came
    /// back with 3.5x the artist's path count on a detailed wordmark at the same
    /// colour error. Primitives stay their own element and gradient faces each own a
    /// gradient, so neither merges. The merged path takes the first member's place in
    /// the paint order and its id; every member's children follow it.
    #[allow(clippy::too_many_arguments)]
    fn emit_level(
        members: &[usize],
        order: &[FaceRings],
        outer: &[Vec<usize>],
        fitted: &[FittedPath],
        prims: &[Option<PrimitiveFit>],
        fills: &[String],
        opac: &[String],
        ids: &[String],
        children: &[Vec<usize>],
        drawn: &[Vec<usize>],
        holes: &[Vec<usize>],
        decimals: usize,
        harmonized_d: &std::collections::HashMap<usize, String>,
        symbol_use: &std::collections::HashMap<usize, (String, String)>,
        strokes: &std::collections::HashMap<usize, String>,
        out: &mut String,
    ) {
        let mut done = vec![false; members.len()];
        for a in 0..members.len() {
            if done[a] {
                continue;
            }
            done[a] = true;
            let i = members[a];
            let Some((element, is_prim)) = face_element(
                i,
                order,
                outer,
                fitted,
                prims,
                fills,
                opac,
                ids,
                drawn,
                holes,
                decimals,
                harmonized_d,
                symbol_use,
                strokes,
            ) else {
                continue;
            };
            let mut group = vec![i];
            if !is_prim && fills[i].starts_with('#') && !symbol_use.contains_key(&i) {
                for b in a + 1..members.len() {
                    if done[b]
                        || fills[members[b]] != fills[i]
                        || opac[members[b]] != opac[i]
                        || symbol_use.contains_key(&members[b])
                    {
                        continue;
                    }
                    let j = members[b];
                    let Some((_, prim_b)) = face_element(
                        j,
                        order,
                        outer,
                        fitted,
                        prims,
                        fills,
                        opac,
                        ids,
                        drawn,
                        holes,
                        decimals,
                        harmonized_d,
                        symbol_use,
                        strokes,
                    ) else {
                        done[b] = true;
                        continue;
                    };
                    if prim_b {
                        continue;
                    }
                    done[b] = true;
                    group.push(j);
                }
            }
            let element = if group.len() == 1 {
                element
            } else {
                let mut d = String::new();
                for &j in &group {
                    d.push_str(&face_d(
                        j,
                        order,
                        fitted,
                        prims,
                        drawn,
                        holes,
                        decimals,
                        harmonized_d,
                    ));
                }
                format!(
                    "<path id=\"{}\" d=\"{d}\" fill=\"{}\"{} fill-rule=\"evenodd\"/>",
                    ids[i], fills[i], opac[i]
                )
            };
            let has_children = group.iter().any(|&j| !children[j].is_empty());
            if !has_children {
                out.push_str(&element);
                continue;
            }
            // A face with things inside it becomes a group, so selecting the group
            // selects the shape and its contents together.
            out.push_str(&format!("<g id=\"{}-group\">", ids[i]));
            out.push_str(&element);
            for &j in &group {
                emit_level(
                    &children[j],
                    order,
                    outer,
                    fitted,
                    prims,
                    fills,
                    opac,
                    ids,
                    children,
                    drawn,
                    holes,
                    decimals,
                    harmonized_d,
                    symbol_use,
                    strokes,
                    out,
                );
            }
            out.push_str("</g>");
        }
    }

    // Rings written as strokes: a face with exactly one hole punched in it, where the face's
    // outline and the hole are both primitives and one is the other offset by a uniform
    // width. See `annulus_stroke`. Only where transparency is traced natively; opaque paint
    // only, since a stroke carries no `fill-opacity` of its own.
    let single_prim = |f: usize| -> Option<&PrimitiveKind> {
        let &[k] = drawn[f].as_slice() else {
            return None;
        };
        let &[(e, _)] = order[f][k].as_slice() else {
            return None;
        };
        prims.get(e).and_then(|p| p.as_ref()).map(|p| &p.kind)
    };
    let strokes: std::collections::HashMap<usize, String> = if native {
        (0..order.len())
            .filter(|&i| {
                !drop(i)
                    && holes[i].len() == 1
                    && fills[i].starts_with('#')
                    && opac[i].is_empty()
                    && !harmonized_d.contains_key(&i)
                    && !symbol_use.contains_key(&i)
            })
            .filter_map(|i| {
                let (o, h) = (single_prim(i)?, single_prim(holes[i][0])?);
                let (mid, width) = annulus_stroke(o, h)?;
                Some((i, stroke_element(&mid, width, &fills[i], decimals)?))
            })
            .collect()
    } else {
        std::collections::HashMap::new()
    };

    let mut body = String::new();
    emit_level(
        &roots,
        order,
        &outer,
        fitted,
        prims,
        &fills,
        &opac,
        &ids,
        &children,
        drawn,
        &holes,
        decimals,
        &harmonized_d,
        &symbol_use,
        &strokes,
        &mut body,
    );

    // The layers, over everything, each as one compound path. Its faces are disjoint, so
    // even-odd paints their union; and because it is one path rather than one per face,
    // the renderer composites the translucent paint once and no seam appears where two of
    // them met.
    if let Some((an, shape_order)) = layers {
        for (k, l) in an.layers.iter().enumerate() {
            // The union of the layer's faces, from the rings they had before the ground
            // under them was merged. They are disjoint, so even-odd paints the union, and
            // one path means the translucent paint is composited exactly once.
            let mut d = String::new();
            if let Some(rings) = shape_order.get(k) {
                for ring in rings {
                    fmt_ring(ring, fitted, decimals, &mut d);
                }
            }
            let _ = l;
            if d.is_empty() {
                continue;
            }
            body.push_str(&format!(
                "<path id=\"layer-{k}\" d=\"{d}\" fill=\"{}\" fill-opacity=\"{:.3}\" fill-rule=\"evenodd\"/>",
                inkvec_trace::color::to_hex(l.color),
                l.alpha
            ));
        }
    }

    let defs_block = if defs.is_empty() {
        String::new()
    } else {
        format!("<defs>{defs}</defs>")
    };
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-0.5 -0.5 {w} {h}\" width=\"{w}\" height=\"{h}\">{defs_block}{body}</svg>"
    )
}

/// Bilevel output: every contour as one path with `evenodd`, so holes fall out of the
/// winding rather than needing to be detected and paired up.
pub(crate) fn emit_bilevel(paths: &[Vec<Point>], w: usize, h: usize, precision: f64) -> String {
    let decimals = emit_decimals(precision);
    let mut d = String::new();
    for pts in paths {
        fmt_path(pts, decimals, &mut d);
    }
    // See emit_color: pixel-centre coordinates, so the canvas origin is (-0.5, -0.5).
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-0.5 -0.5 {w} {h}\" width=\"{w}\" height=\"{h}\"><rect x=\"-0.5\" y=\"-0.5\" width=\"{w}\" height=\"{h}\" fill=\"#ffffff\"/><path d=\"{d}\" fill=\"#000000\" fill-rule=\"evenodd\"/></svg>"
    )
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
