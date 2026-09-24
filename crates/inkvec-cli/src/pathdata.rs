//! Path data: fitted geometry written as the `d` attribute of an SVG path.
//!
//! A face's outline is a ring of edges from the planar map, each stored once and walked
//! forwards or backwards by the faces either side of it; these assemble a ring from its
//! edges and write it, a stroke, or a bare polygon, to a fixed number of decimals (see
//! [`crate::emit`] for why the count is the emitter's own).

use inkvec_core::Point;
use inkvec_fit::{curves::Segment, FittedPath};

use crate::Ring;

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
    fmt_ring_with(ring, fitted, &|_| None, decimals, d);
}

/// [`fmt_ring`], with some edges replaced by geometry already oriented as this ring walks
/// them (see [`crate::seams`]).
pub(crate) fn fmt_ring_with(
    ring: &Ring,
    fitted: &[FittedPath],
    over: &dyn Fn(usize) -> Option<FittedPath>,
    decimals: usize,
    d: &mut String,
) {
    let mut first = true;
    let mut total = 0usize;
    let mut buf = String::new();
    // (previous cubic's second control point, its end) -- carried across edge boundaries,
    // since a ring is several edges' fits concatenated and a smooth join can fall on the
    // seam between two of them.
    let mut prev_c2: Option<(Point, Point)> = None;
    for &(k, rev) in ring {
        let path = match over(k) {
            Some(p) => p,
            None if rev => fitted[k].reversed(),
            None => fitted[k].clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn line(a: (f64, f64), b: (f64, f64)) -> FittedPath {
        FittedPath {
            start: Point::new(a.0, a.1),
            segments: vec![Segment::Line(Point::new(b.0, b.1))],
            closed: false,
        }
    }

    /// A triangle of three shared edges, the second walked backwards.
    fn triangle() -> (Ring, Vec<FittedPath>) {
        let fitted = vec![
            line((0.0, 0.0), (4.0, 0.0)),
            line((4.0, 4.0), (4.0, 0.0)),
            line((4.0, 4.0), (0.0, 0.0)),
        ];
        (vec![(0, false), (1, true), (2, false)], fitted)
    }

    #[test]
    fn a_ring_walks_its_edges_in_their_own_directions() {
        let (ring, fitted) = triangle();
        let mut d = String::new();
        fmt_ring(&ring, &fitted, 1, &mut d);
        assert_eq!(d, "M0.0,0.0L4.0,0.0L4.0,4.0L0.0,0.0Z");
    }

    #[test]
    fn an_override_replaces_one_edge_as_the_ring_walks_it() {
        let (ring, fitted) = triangle();
        // Edge 1 as the ring walks it (upward from (4,0)), bowed out through (4.5,2).
        let moved = FittedPath {
            start: Point::new(4.0, 0.0),
            segments: vec![
                Segment::Line(Point::new(4.5, 2.0)),
                Segment::Line(Point::new(4.0, 4.0)),
            ],
            closed: false,
        };
        let mut d = String::new();
        fmt_ring_with(
            &ring,
            &fitted,
            &|k| (k == 1).then(|| moved.clone()),
            1,
            &mut d,
        );
        assert_eq!(d, "M0.0,0.0L4.0,0.0L4.5,2.0L4.0,4.0L0.0,0.0Z");
    }

    #[test]
    fn a_polygon_of_fewer_than_three_points_is_not_written() {
        let mut d = String::new();
        fmt_path(&[Point::new(0.0, 0.0), Point::new(1.0, 1.0)], 2, &mut d);
        assert!(d.is_empty());
    }
}
