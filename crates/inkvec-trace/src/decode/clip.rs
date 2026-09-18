//! Exact Sutherland-Hodgman polygon clipping, shoelace area, and pixel coverage.

use inkvec_core::predicates::orient2d;
use inkvec_core::Point;

/// Signed shoelace of a closed loop.
/// Exact area of a simple polygon clipped to one pixel, and the shoelace it uses.
/// `centerline`'s stroke solve needs the same exact coverage this module fits
/// against -- a stroke scored by any other rule is optimised towards a picture the
/// emitter will not draw, which is measurably what went wrong the first time.
pub(crate) fn shoelace(pts: &[Point]) -> f64 {
    let n = pts.len();
    let mut s = 0.0;
    for i in 0..n {
        let (p, q) = (pts[i], pts[(i + 1) % n]);
        s += p.x * q.y - q.x * p.y;
    }
    0.5 * s
}

/// Sutherland-Hodgman against one half-plane, in place through two scratch buffers.
pub(crate) fn clip_axis(src: &[Point], dst: &mut Vec<Point>, axis: usize, lim: f64, keep_ge: bool) {
    dst.clear();
    let n = src.len();
    if n == 0 {
        return;
    }
    let val = |p: &Point| if axis == 0 { p.x } else { p.y };
    let inside = |p: &Point| {
        if keep_ge {
            val(p) >= lim
        } else {
            val(p) <= lim
        }
    };
    for i in 0..n {
        let p = src[i];
        let q = src[(i + 1) % n];
        let (ip, iq) = (inside(&p), inside(&q));
        if ip {
            dst.push(p);
        }
        if ip != iq {
            let d = val(&q) - val(&p);
            if d.abs() > 1e-12 {
                let t = (lim - val(&p)) / d;
                dst.push(Point::new(p.x + t * (q.x - p.x), p.y + t * (q.y - p.y)));
            }
        }
    }
}

/// Area of `poly` inside the unit pixel whose lower corner is (x, y).
pub(crate) fn clip_area(
    poly: &[Point],
    x: f64,
    y: f64,
    a: &mut Vec<Point>,
    b: &mut Vec<Point>,
) -> f64 {
    clip_axis(poly, a, 0, x, true);
    if a.is_empty() {
        return 0.0;
    }
    clip_axis(a, b, 0, x + 1.0, false);
    if b.is_empty() {
        return 0.0;
    }
    clip_axis(b, a, 1, y, true);
    if a.is_empty() {
        return 0.0;
    }
    clip_axis(a, b, 1, y + 1.0, false);
    if b.len() < 3 {
        return 0.0;
    }
    shoelace(b).abs()
}

pub(crate) fn point_in_poly(poly: &[Point], x: f64, y: f64) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        if (p.y > y) != (q.y > y) {
            let t = (y - p.y) / (q.y - p.y);
            if x < p.x + t * (q.x - p.x) {
                inside = !inside;
            }
        }
    }
    inside
}

/// A rectangle of pixel indices, clamped to the image.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bbox {
    pub(crate) x0: usize,
    pub(crate) y0: usize,
    pub(crate) x1: usize,
    pub(crate) y1: usize,
}

impl Bbox {
    pub(crate) fn of(poly: &[Point], w: usize, h: usize, pad: f64) -> Option<Bbox> {
        if poly.is_empty() {
            return None;
        }
        let (mut lx, mut ly, mut hx, mut hy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in poly {
            lx = lx.min(p.x);
            ly = ly.min(p.y);
            hx = hx.max(p.x);
            hy = hy.max(p.y);
        }
        let x0 = ((lx - pad).floor().max(0.0)) as usize;
        let y0 = ((ly - pad).floor().max(0.0)) as usize;
        let x1 = (((hx + pad).ceil() + 1.0).min(w as f64)) as usize;
        let y1 = (((hy + pad).ceil() + 1.0).min(h as f64)) as usize;
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(Bbox { x0, y0, x1, y1 })
    }

    pub(crate) fn len(&self) -> usize {
        (self.x1 - self.x0) * (self.y1 - self.y0)
    }
}

/// Exact fractional coverage of `poly` over every pixel of `bb`, row-major.
///
/// Pixels the boundary does not touch are filled by one point-in-polygon test each; only
/// pixels a polygon edge passes through are clipped. `mark` is scratch, reused.
pub(crate) fn coverage(poly: &[Point], bb: Bbox, out: &mut [f64], mark: &mut [bool]) {
    let (bw, bh) = (bb.x1 - bb.x0, bb.y1 - bb.y0);
    out[..bw * bh].fill(0.0);
    mark[..bw * bh].fill(false);
    if poly.len() < 3 {
        return;
    }
    // Walk each edge, marking the pixels it passes through and their 8-neighbours.
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        let len = ((q.x - p.x).powi(2) + (q.y - p.y).powi(2)).sqrt();
        let steps = (len * 4.0).ceil().max(1.0) as usize;
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let x = p.x + t * (q.x - p.x);
            let y = p.y + t * (q.y - p.y);
            let (cx, cy) = (x.floor() as i64, y.floor() as i64);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (ix, iy) = (cx + dx, cy + dy);
                    if ix < bb.x0 as i64
                        || iy < bb.y0 as i64
                        || ix >= bb.x1 as i64
                        || iy >= bb.y1 as i64
                    {
                        continue;
                    }
                    mark[(iy as usize - bb.y0) * bw + (ix as usize - bb.x0)] = true;
                }
            }
        }
    }
    let mut a: Vec<Point> = Vec::with_capacity(poly.len() + 8);
    let mut b: Vec<Point> = Vec::with_capacity(poly.len() + 8);
    for j in 0..bh {
        for i in 0..bw {
            let k = j * bw + i;
            let (x, y) = ((bb.x0 + i) as f64, (bb.y0 + j) as f64);
            out[k] = if mark[k] {
                clip_area(poly, x, y, &mut a, &mut b)
            } else if point_in_poly(poly, x + 0.5, y + 0.5) {
                1.0
            } else {
                0.0
            };
        }
    }
}

pub(crate) fn simple(poly: &[Point]) -> bool {
    let n = poly.len();
    for i in 0..n {
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            if seg_cross(poly[i], poly[(i + 1) % n], poly[j], poly[(j + 1) % n]) {
                return false;
            }
        }
    }
    true
}

pub(crate) fn seg_cross(a: Point, b: Point, c: Point, d: Point) -> bool {
    let (d1, d2) = (orient2d(a, b, c), orient2d(a, b, d));
    let (d3, d4) = (orient2d(c, d, a), orient2d(c, d, b));
    ((d1 > 0.0) != (d2 > 0.0))
        && ((d3 > 0.0) != (d4 > 0.0))
        && d1 != 0.0
        && d2 != 0.0
        && d3 != 0.0
        && d4 != 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bb(w: usize, h: usize) -> Bbox {
        Bbox {
            x0: 0,
            y0: 0,
            x1: w,
            y1: h,
        }
    }

    #[test]
    fn half_pixel_is_exactly_half() {
        let quad = vec![
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(1.0, 0.5),
            Point::new(0.0, 0.5),
        ];
        let mut cov = vec![0.0; 1];
        let mut mark = vec![false; 1];
        coverage(&quad, bb(1, 1), &mut cov, &mut mark);
        assert!((cov[0] - 0.5).abs() < 1e-12, "cov: {}", cov[0]);
    }

    #[test]
    fn triangle_area_is_exact() {
        let tri = vec![
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(0.0, 1.0),
        ];
        let mut cov = vec![0.0; 1];
        let mut mark = vec![false; 1];
        coverage(&tri, bb(1, 1), &mut cov, &mut mark);
        assert!((cov[0] - 0.5).abs() < 1e-12, "cov: {}", cov[0]);
    }

    #[test]
    fn point_in_polygon_and_simple_tests() {
        let square = vec![
            Point::new(0.0, 0.0),
            Point::new(2.0, 0.0),
            Point::new(2.0, 2.0),
            Point::new(0.0, 2.0),
        ];
        assert!(point_in_poly(&square, 1.0, 1.0));
        assert!(!point_in_poly(&square, 3.0, 1.0));
        assert!(simple(&square));

        let self_intersecting = vec![
            Point::new(0.0, 0.0),
            Point::new(2.0, 2.0),
            Point::new(0.0, 2.0),
            Point::new(2.0, 0.0),
        ];
        assert!(!simple(&self_intersecting));
    }
}
