//! Stroke outline generation, circular joins/caps, pixel clipping, and coverage cost.

use inkvec_core::Point;

use super::Stroke;
use crate::coverage::CoverageField;

#[inline]
pub(crate) fn cross(a: f32, b: f32) -> f64 {
    let d = (b - a) as f64;
    if d.abs() < 1e-12 {
        0.5
    } else {
        ((0.5 - a as f64) / d).clamp(0.0, 1.0)
    }
}

/// Marching-squares edge pairs. Edge order is top, right, bottom, left.
pub(crate) fn ms_pairs(case: u8, centre_in: bool) -> &'static [(usize, usize)] {
    match case {
        1 | 14 => &[(3, 0)],
        2 | 13 => &[(0, 1)],
        3 | 12 => &[(3, 1)],
        4 | 11 => &[(1, 2)],
        6 | 9 => &[(0, 2)],
        7 => &[(3, 2)],
        8 => &[(2, 3)],
        // Saddles. The cell centre decides whether the two like corners are joined; the
        // same resolution contour.rs uses, and for the same reason.
        5 => {
            if centre_in {
                &[(0, 1), (2, 3)]
            } else {
                &[(3, 0), (1, 2)]
            }
        }
        10 => {
            if centre_in {
                &[(3, 0), (1, 2)]
            } else {
                &[(0, 1), (2, 3)]
            }
        }
        _ => &[],
    }
}

pub(crate) fn point_seg(p: Point, a: Point, b: Point) -> (f64, Point) {
    let ab = b - a;
    let l2 = ab.dot(ab);
    if l2 <= 1e-24 {
        return (p.dist(a), a);
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    let q = Point::new(a.x + ab.x * t, a.y + ab.y * t);
    (p.dist(q), q)
}

/// Points per quarter turn when a round cap or join is turned into polygon edges.
const ARC_STEPS: usize = 8;

pub(crate) fn arc_into(out: &mut Vec<Point>, c: Point, from: f64, to: f64, r: f64) {
    let mut d = to - from;
    while d <= -std::f64::consts::PI {
        d += std::f64::consts::TAU;
    }
    while d > std::f64::consts::PI {
        d -= std::f64::consts::TAU;
    }
    let n = ((d.abs() / std::f64::consts::FRAC_PI_2) * ARC_STEPS as f64).ceil() as usize;
    for k in 1..=n.max(1) {
        let a = from + d * (k as f64) / (n.max(1) as f64);
        out.push(Point::new(c.x + r * a.cos(), c.y + r * a.sin()));
    }
}

/// The outline a round-capped, round-joined stroke actually paints.
pub(crate) fn offset_outline(
    path: &[Point],
    closed: bool,
    hw: f64,
) -> (Vec<Point>, Option<Vec<Point>>) {
    let n = path.len();
    if n < 2 || hw <= 0.0 {
        return (Vec::new(), None);
    }
    let dir = |i: usize, j: usize| -> Option<Point> {
        let d = path[j] - path[i];
        let l = (d.x * d.x + d.y * d.y).sqrt();
        (l > 1e-12).then(|| Point::new(d.x / l, d.y / l))
    };

    let side = |fwd: bool| -> Vec<Point> {
        let mut out: Vec<Point> = Vec::with_capacity(n * 2);
        let idx: Vec<usize> = if fwd {
            (0..n).collect()
        } else {
            (0..n).rev().collect()
        };
        let m = idx.len();
        let seg = if closed { m } else { m - 1 };
        let mut prev_nrm: Option<Point> = None;
        for k in 0..seg {
            let (a, b) = (idx[k], idx[(k + 1) % m]);
            let Some(t) = dir(a, b) else { continue };
            let nrm = Point::new(-t.y, t.x);
            let pa = path[a];
            if let Some(pn) = prev_nrm {
                let a0 = pn.y.atan2(pn.x);
                let a1 = nrm.y.atan2(nrm.x);
                arc_into(&mut out, pa, a0, a1, hw);
            } else {
                out.push(Point::new(pa.x + nrm.x * hw, pa.y + nrm.y * hw));
            }
            let pb = path[b];
            out.push(Point::new(pb.x + nrm.x * hw, pb.y + nrm.y * hw));
            prev_nrm = Some(nrm);
        }
        out
    };

    if closed {
        let a = side(true);
        let b = side(false);
        if a.len() < 3 || b.len() < 3 {
            return (if a.len() >= 3 { a } else { b }, None);
        }
        let (aa, ab) = (
            crate::decode::shoelace(&a).abs(),
            crate::decode::shoelace(&b).abs(),
        );
        let (outer, inner) = if aa >= ab { (a, b) } else { (b, a) };
        return (outer, Some(inner));
    }

    // Open: up one side, round cap, back the other, round cap.
    let mut out = side(true);
    if out.len() < 2 {
        return (Vec::new(), None);
    }
    let back = side(false);
    if let (Some(&e), Some(&s2)) = (out.last(), back.first()) {
        let c = path[n - 1];
        arc_into(
            &mut out,
            c,
            (e.y - c.y).atan2(e.x - c.x),
            (s2.y - c.y).atan2(s2.x - c.x),
            hw,
        );
    }
    out.extend_from_slice(&back);
    if let (Some(&e), Some(&s2)) = (out.last(), out.first()) {
        let c = path[0];
        arc_into(
            &mut out,
            c,
            (e.y - c.y).atan2(e.x - c.x),
            (s2.y - c.y).atan2(s2.x - c.x),
            hw,
        );
    }
    (out, None)
}

/// Exact coverage the stroke's own outline puts on the pixel with lower corner
/// `(x, y)`, using the same clip the rest of the tracer fits against.
pub(crate) fn outline_cover(
    outer: &[Point],
    inner: Option<&Vec<Point>>,
    x: f64,
    y: f64,
    a: &mut Vec<Point>,
    b: &mut Vec<Point>,
) -> f64 {
    if outer.len() < 3 {
        return 0.0;
    }
    let mut c = crate::decode::clip_area(outer, x, y, a, b);
    if let Some(i) = inner {
        if i.len() >= 3 {
            c -= crate::decode::clip_area(i, x, y, a, b);
        }
    }
    c.clamp(0.0, 1.0)
}

/// Squared coverage residual over a window, for one stroke against the drawing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stroke_cost(
    coverage: &CoverageField,
    others: &[f32],
    path: &[Point],
    closed: bool,
    hw: f64,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
) -> f64 {
    let w = coverage.width;
    let ww = x1 - x0;
    let (outer, inner) = offset_outline(path, closed, hw);
    if outer.len() < 3 {
        return f64::INFINITY;
    }
    let (mut ca, mut cb) = (Vec::with_capacity(32), Vec::with_capacity(32));
    let mut s = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let mine = outline_cover(
                &outer,
                inner.as_ref(),
                x as f64 - 0.5,
                y as f64 - 0.5,
                &mut ca,
                &mut cb,
            );
            let other = others[(y - y0) * ww + (x - x0)] as f64;
            let d = mine.max(other) - coverage.data[y * w + x] as f64;
            s += d * d;
        }
    }
    s
}

/// Coverage every stroke except `skip` puts on a window.
pub(crate) fn others_cover(
    strokes: &[Stroke],
    skip: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
) -> Vec<f32> {
    let mut out = vec![0.0f32; (x1.saturating_sub(x0)) * (y1.saturating_sub(y0))];
    for (k, st) in strokes.iter().enumerate() {
        if k == skip || st.path.is_empty() {
            continue;
        }
        let hw = st.width * 0.5;
        let (outer, inner) = offset_outline(&st.path, st.closed, hw);
        if outer.len() < 3 {
            continue;
        }
        let ww = x1 - x0;
        let (mut ca, mut cb) = (Vec::with_capacity(32), Vec::with_capacity(32));
        for y in y0..y1 {
            for x in x0..x1 {
                let c = outline_cover(
                    &outer,
                    inner.as_ref(),
                    x as f64 - 0.5,
                    y as f64 - 0.5,
                    &mut ca,
                    &mut cb,
                ) as f32;
                let idx = (y - y0) * ww + (x - x0);
                out[idx] = out[idx].max(c);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point_seg_distance() {
        let a = Point::new(0.0, 0.0);
        let b = Point::new(10.0, 0.0);
        let p = Point::new(5.0, 3.0);
        let (d, q) = point_seg(p, a, b);
        assert!((d - 3.0).abs() < 1e-12);
        assert!((q.x - 5.0).abs() < 1e-12 && (q.y - 0.0).abs() < 1e-12);
    }

    #[test]
    fn test_offset_outline_open_stroke() {
        let path = vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0)];
        let (outer, inner) = offset_outline(&path, false, 1.0);
        assert!(inner.is_none());
        assert!(outer.len() >= 4);
    }
}
