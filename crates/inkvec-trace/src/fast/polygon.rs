//! The optimal polygon of a measured boundary: the fewest straight sides that stay within a
//! tolerance of every point, and among those the one closest to the points.
//!
//! This is the first stage of Selinger's Potrace (2003, section 2.2), restated for sub-pixel
//! input. Potrace asks whether a run of *pixel corners* is straight by an L-infinity test
//! against the integer lattice; the points here have already been moved to their sub-pixel
//! positions, so the question becomes metric: does every point of the run lie within `tol`
//! of the chord between its ends? The run is scanned with an incremental cone of admissible
//! directions (each point narrows it by the angle it subtends at `tol`), which keeps the
//! test O(1) per candidate side, and the dynamic program over sides is Potrace's: fewest
//! sides first, then the smallest sum of squared distances, read in O(1) from prefix sums.

use inkvec_core::{Point, Vec2};

/// Running sums of the points' coordinates, relative to one origin, so the sum of squared
/// distances from any run of points to any line costs O(1).
struct Sums {
    origin: Point,
    /// Prefix sums of x, y, x^2, x*y, y^2; entry `k` covers points `0..k`.
    s: Vec<[f64; 5]>,
}

impl Sums {
    fn new(pts: &[Point]) -> Self {
        let origin = pts.first().copied().unwrap_or(Point::new(0.0, 0.0));
        let mut s = Vec::with_capacity(pts.len() + 1);
        let mut acc = [0.0f64; 5];
        s.push(acc);
        for p in pts {
            let (x, y) = (p.x - origin.x, p.y - origin.y);
            acc[0] += x;
            acc[1] += y;
            acc[2] += x * x;
            acc[3] += x * y;
            acc[4] += y * y;
            s.push(acc);
        }
        Self { origin, s }
    }

    /// Sum of squared distances from points `lo..hi` to the line through `a` and `b`.
    fn sq_dist(&self, lo: usize, hi: usize, a: Point, b: Point) -> f64 {
        if hi <= lo {
            return 0.0;
        }
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len2 = dx * dx + dy * dy;
        if len2 < 1e-18 {
            return 0.0;
        }
        let m = (hi - lo) as f64;
        let d = |k: usize| self.s[hi][k] - self.s[lo][k];
        let (sx, sy, sxx, sxy, syy) = (d(0), d(1), d(2), d(3), d(4));
        // Signed distance times |b - a| is  u x + v y + c  with (u, v) the normal.
        let (u, v) = (-dy, dx);
        let (ax, ay) = (a.x - self.origin.x, a.y - self.origin.y);
        let c = -(u * ax + v * ay);
        let q = u * u * sxx
            + v * v * syy
            + 2.0 * u * v * sxy
            + 2.0 * u * c * sx
            + 2.0 * v * c * sy
            + c * c * m;
        q.max(0.0) / len2
    }
}

/// The cone of directions from one anchor that still pass within `tol` of every point seen
/// so far, as its two bounding unit vectors (`right` clockwise of `left`).
struct Cone {
    right: Vec2,
    left: Vec2,
    open: bool,
}

impl Cone {
    fn full() -> Self {
        Self {
            right: Vec2 { x: 0.0, y: 0.0 },
            left: Vec2 { x: 0.0, y: 0.0 },
            open: false,
        }
    }

    /// True when direction `d` (unit) is admissible.
    fn admits(&self, d: Vec2) -> bool {
        const EPS: f64 = 1e-9;
        !self.open || (self.right.cross(d) >= -EPS && d.cross(self.left) >= -EPS)
    }

    /// Narrow the cone by a point at offset `v`, distance `r > tol` from the anchor.
    /// Returns false when nothing is left.
    fn narrow(&mut self, v: Vec2, r: f64, tol: f64) -> bool {
        let s = (tol / r).min(1.0);
        let c = (1.0 - s * s).max(0.0).sqrt();
        let (ux, uy) = (v.x / r, v.y / r);
        // `u` rotated by -theta (clockwise, in a y-up sense) and by +theta.
        let right = Vec2 {
            x: ux * c + uy * s,
            y: uy * c - ux * s,
        };
        let left = Vec2 {
            x: ux * c - uy * s,
            y: uy * c + ux * s,
        };
        if !self.open {
            *self = Self {
                right,
                left,
                open: true,
            };
            return true;
        }
        if self.right.cross(right) > 0.0 {
            self.right = right;
        }
        if left.cross(self.left) > 0.0 {
            self.left = left;
        }
        self.right.cross(self.left) >= 0.0 && self.right.dot(self.left) > 0.0
    }
}

/// Polygon vertices of an open run, as indices into `pts`: always the first and the last
/// point, and the fewest interior points such that every point lies within `tol` of the
/// side that spans it.
pub(crate) fn open(pts: &[Point], tol: f64) -> Vec<usize> {
    let n = pts.len();
    if n <= 2 {
        return (0..n).collect();
    }
    let sums = Sums::new(pts);
    // (sides, penalty) lexicographically, and the vertex each best path came from.
    let mut best: Vec<(u32, f64)> = vec![(u32::MAX, f64::INFINITY); n];
    let mut prev = vec![usize::MAX; n];
    best[0] = (0, 0.0);
    for i in 0..n - 1 {
        if best[i].0 == u32::MAX {
            continue;
        }
        let a = pts[i];
        let mut cone = Cone::full();
        let mut reach = 0.0f64;
        for j in i + 1..n {
            let v = pts[j] - a;
            let r = v.norm();
            // The run must move away from its anchor: a point that falls back by more than
            // the tolerance is doubling back, and no chord from `a` describes it.
            if r + tol < reach {
                break;
            }
            let d = if r > 1e-12 {
                Vec2 {
                    x: v.x / r,
                    y: v.y / r,
                }
            } else {
                Vec2 { x: 1.0, y: 0.0 }
            };
            if r > 1e-12 && cone.admits(d) {
                let cand = (best[i].0 + 1, best[i].1 + sums.sq_dist(i + 1, j, a, pts[j]));
                if cand.0 < best[j].0 || (cand.0 == best[j].0 && cand.1 < best[j].1) {
                    best[j] = cand;
                    prev[j] = i;
                }
            }
            reach = reach.max(r);
            if r > tol && !cone.narrow(v, r, tol) {
                break;
            }
        }
    }
    let mut out = vec![n - 1];
    let mut k = n - 1;
    while k != 0 {
        k = prev[k];
        if k == usize::MAX {
            // Unreachable: the next point is always admissible from any point.
            return (0..n).collect();
        }
        out.push(k);
    }
    out.reverse();
    out
}

/// Polygon vertices of a closed ring, as indices into `pts` in ring order.
///
/// The ring is cut at its sharpest point, which a polygon of any quality has as a vertex
/// anyway, and solved as an open run back to that point.
pub(crate) fn closed(pts: &[Point], tol: f64) -> Vec<usize> {
    let n = pts.len();
    if n < 4 {
        return (0..n).collect();
    }
    let s = sharpest(pts);
    let run: Vec<Point> = (0..=n).map(|k| pts[(s + k) % n]).collect();
    let mut v = open(&run, tol);
    v.pop(); // the start again
    v.into_iter().map(|k| (s + k) % n).collect()
}

/// The point of a closed ring where it turns hardest over a short window.
fn sharpest(pts: &[Point]) -> usize {
    let n = pts.len();
    let m = 2.min(n / 3).max(1);
    let mut best = (0, f64::NEG_INFINITY);
    for k in 0..n {
        let a = pts[(k + n - m) % n];
        let b = pts[k];
        let c = pts[(k + m) % n];
        let (u, v) = (b - a, c - b);
        let (nu, nv) = (u.norm(), v.norm());
        if nu < 1e-12 || nv < 1e-12 {
            continue;
        }
        let turn = 1.0 - u.dot(v) / (nu * nv);
        if turn > best.1 + 1e-12 {
            best = (k, turn);
        }
    }
    best.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn a_straight_run_is_one_side() {
        let pts: Vec<Point> = (0..50)
            .map(|k| p(k as f64, 0.5 * k as f64 + 0.05 * ((k % 3) as f64)))
            .collect();
        assert_eq!(open(&pts, 0.5), vec![0, 49]);
    }

    #[test]
    fn an_l_is_two_sides_meeting_at_the_corner() {
        let mut pts: Vec<Point> = (0..=20).map(|k| p(k as f64, 0.0)).collect();
        pts.extend((1..=20).map(|k| p(20.0, k as f64)));
        assert_eq!(open(&pts, 0.5), vec![0, 20, 40]);
    }

    #[test]
    fn a_doubling_back_run_is_split() {
        let mut pts: Vec<Point> = (0..=10).map(|k| p(k as f64, 0.0)).collect();
        pts.extend((0..10).rev().map(|k| p(k as f64, 0.2)));
        let v = open(&pts, 0.5);
        assert!(v.len() >= 3, "{v:?}");
    }

    #[test]
    fn a_square_ring_has_its_four_corners() {
        let mut pts = Vec::new();
        for k in 0..10 {
            pts.push(p(k as f64, 0.0));
        }
        for k in 0..10 {
            pts.push(p(10.0, k as f64));
        }
        for k in 0..10 {
            pts.push(p(10.0 - k as f64, 10.0));
        }
        for k in 0..10 {
            pts.push(p(0.0, 10.0 - k as f64));
        }
        let mut v = closed(&pts, 0.5);
        v.sort_unstable();
        assert_eq!(v, vec![0, 10, 20, 30]);
    }

    #[test]
    fn a_circle_gets_a_modest_polygon_within_tolerance() {
        let n = 200;
        let pts: Vec<Point> = (0..n)
            .map(|k| {
                let t = k as f64 / n as f64 * std::f64::consts::TAU;
                p(40.0 + 30.0 * t.cos(), 40.0 + 30.0 * t.sin())
            })
            .collect();
        let v = closed(&pts, 0.5);
        // A regular polygon within 0.5 of a radius-30 circle needs about pi/acos(29.5/30).
        assert!((10..=24).contains(&v.len()), "{}", v.len());
    }

    #[test]
    fn squared_distances_match_the_direct_sum() {
        let pts: Vec<Point> = (0..9)
            .map(|k| p(k as f64, ((k * 7) % 5) as f64 * 0.3))
            .collect();
        let s = Sums::new(&pts);
        let (a, b) = (pts[1], pts[7]);
        let dir = b - a;
        let direct: f64 = pts[2..7]
            .iter()
            .map(|q| {
                let c = dir.cross(*q - a) / dir.norm();
                c * c
            })
            .sum();
        assert!((s.sq_dist(2, 7, a, b) - direct).abs() < 1e-9);
    }
}
