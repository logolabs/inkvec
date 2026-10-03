//! A uniform grid over line segments: nearest-segment queries and ray casts.
//!
//! The stroke stage asks two geometric questions thousands of times per face, and both are
//! about short line segments:
//!
//! * **Across the stroke** ([`SegGrid::ray`]): starting on one side of a stroke and walking
//!   along the inward normal, where is the other side? That is how the two sides of a
//!   stroke are paired and its width is read ([`super::boundary`]).
//! * **To the stroke** ([`SegGrid::nearest`]): how far is a measured boundary point from
//!   the fitted centrelines? That distance minus the half-width is the residual the stroke
//!   model is judged by ([`super::score`]).
//!
//! The segments are bucketed into square cells of side `cs` px: every segment is listed in
//! every cell its bounding box overlaps, so a cell's list holds every segment that has a
//! point inside it. Boundary segments are about one pixel long, so each sits in one to four
//! cells.
//!
//! Not from the literature: a uniform bucket grid is the textbook spatial index for
//! uniformly sized primitives, and the ray walk is the standard cell-by-cell traversal of a
//! line through a grid (a 2-D DDA, in the order the line crosses cell walls). Neither is a
//! research method; they are named here so a reader knows nothing subtler is going on.
//!
//! Coordinates are the traced raster's pixels (pixel centres at integers), as everywhere
//! in the tracer.

use inkvec_core::{Point, Vec2};

/// Segments `a[k] -> b[k]` bucketed into square cells of side [`SegGrid::cs`] px.
pub(crate) struct SegGrid {
    /// First endpoint of each segment.
    a: Vec<Point>,
    /// Second endpoint of each segment.
    b: Vec<Point>,
    /// Image x of the grid's left wall, px.
    ox: f64,
    /// Image y of the grid's top wall, px.
    oy: f64,
    /// Cell side, px.
    cs: f64,
    /// Columns.
    nx: usize,
    /// Rows.
    ny: usize,
    /// Per cell (row-major), the segments with a point in it.
    cells: Vec<Vec<u32>>,
}

/// The nearest point of a segment set to a query point.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Near {
    /// Euclidean distance from the query, px.
    pub(crate) d: f64,
    /// Index of the segment the nearest point lies on.
    pub(crate) seg: usize,
    /// Position along that segment, 0 at `a` and 1 at `b`.
    pub(crate) u: f64,
    /// The nearest point itself.
    pub(crate) p: Point,
}

/// The point of segment `a -> b` nearest to `q`, as `(distance px, u in [0, 1], point)`.
///
/// `u = clamp(((q - a)·(b - a)) / |b - a|², 0, 1)`; a segment shorter than 1e-12 px is
/// treated as its first endpoint (`u = 0`).
pub(crate) fn point_segment(q: Point, a: Point, b: Point) -> (f64, f64, Point) {
    let e = b - a;
    let l2 = e.x * e.x + e.y * e.y;
    let u = if l2 <= 1e-24 {
        0.0
    } else {
        (((q.x - a.x) * e.x + (q.y - a.y) * e.y) / l2).clamp(0.0, 1.0)
    };
    let p = Point::new(a.x + u * e.x, a.y + u * e.y);
    (q.dist(p), u, p)
}

/// Where the ray `o + t·d` crosses segment `a -> b`: `Some((t, u))` with `u in [0, 1]` the
/// position along the segment, or `None` when they are parallel (|d × e| < 1e-12) or the
/// line misses the segment. `t` may be negative; the caller bounds it.
///
/// Solving `o + t·d = a + u·e` with `e = b - a` by Cramer's rule:
/// `t = ((a - o) × e) / (d × e)` and `u = ((a - o) × d) / (d × e)`.
fn ray_segment(o: Point, d: Vec2, a: Point, b: Point) -> Option<(f64, f64)> {
    let e = b - a;
    let den = d.cross(e);
    if den.abs() < 1e-12 {
        return None;
    }
    let ao = a - o;
    let t = ao.cross(e) / den;
    let u = ao.cross(d) / den;
    (-1e-9..=1.0 + 1e-9)
        .contains(&u)
        .then_some((t, u.clamp(0.0, 1.0)))
}

impl SegGrid {
    /// Bucket `segs` into cells of side `cs` px (at least 0.25 px).
    ///
    /// The grid covers the segments' bounding box with one empty cell of margin on every
    /// side. Building is O(segments + cells); an empty input gives a 2 x 2 grid of empty
    /// cells, on which every query returns `None`.
    pub(crate) fn new(segs: &[(Point, Point)], cs: f64) -> SegGrid {
        let cs = cs.max(0.25);
        let (mut x0, mut y0, mut x1, mut y1) =
            (f64::INFINITY, f64::INFINITY, -f64::INFINITY, -f64::INFINITY);
        for &(a, b) in segs {
            for p in [a, b] {
                x0 = x0.min(p.x);
                y0 = y0.min(p.y);
                x1 = x1.max(p.x);
                y1 = y1.max(p.y);
            }
        }
        if segs.is_empty() {
            (x0, y0, x1, y1) = (0.0, 0.0, 0.0, 0.0);
        }
        let (ox, oy) = (x0 - cs, y0 - cs);
        let nx = ((x1 - ox) / cs).floor() as usize + 2;
        let ny = ((y1 - oy) / cs).floor() as usize + 2;
        let mut g = SegGrid {
            a: segs.iter().map(|s| s.0).collect(),
            b: segs.iter().map(|s| s.1).collect(),
            ox,
            oy,
            cs,
            nx,
            ny,
            cells: vec![Vec::new(); nx * ny],
        };
        for (k, &(a, b)) in segs.iter().enumerate() {
            let (cx0, cy0) = g.cell_of(a.x.min(b.x), a.y.min(b.y));
            let (cx1, cy1) = g.cell_of(a.x.max(b.x), a.y.max(b.y));
            for cy in cy0..=cy1 {
                for cx in cx0..=cx1 {
                    g.cells[cy * nx + cx].push(k as u32);
                }
            }
        }
        g
    }

    /// The (column, row) of the cell holding image point `(x, y)`, clamped into the grid.
    fn cell_of(&self, x: f64, y: f64) -> (usize, usize) {
        let cx = ((x - self.ox) / self.cs)
            .floor()
            .clamp(0.0, (self.nx - 1) as f64) as usize;
        let cy = ((y - self.oy) / self.cs)
            .floor()
            .clamp(0.0, (self.ny - 1) as f64) as usize;
        (cx, cy)
    }

    /// Number of segments in the grid.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.a.len()
    }

    /// The nearest segment point to `q` within `max_d` px, or `None` when there is none.
    ///
    /// Searches square rings of cells around `q`'s cell, nearest ring first. Every segment
    /// is listed in each cell it has a point in, so once ring `r` is done any segment not
    /// yet seen lies wholly in cells at least `r + 1` rings out, at least `r·cs` px away:
    /// the search stops as soon as the best distance found is within that bound. Ties keep
    /// the first segment seen. Cost O((d / cs)² · bucket size) for a nearest distance `d`.
    pub(crate) fn nearest(&self, q: Point, max_d: f64) -> Option<Near> {
        let qx = ((q.x - self.ox) / self.cs).floor() as isize;
        let qy = ((q.y - self.oy) / self.cs).floor() as isize;
        let r_max = (max_d / self.cs).ceil() as isize + 1;
        let mut best: Option<Near> = None;
        for r in 0..=r_max {
            for cy in qy - r..=qy + r {
                if cy < 0 || cy >= self.ny as isize {
                    continue;
                }
                for cx in qx - r..=qx + r {
                    // Only the cells exactly `r` rings out; the inner ones were done.
                    if cx < 0
                        || cx >= self.nx as isize
                        || ((cx - qx).abs().max((cy - qy).abs()) != r)
                    {
                        continue;
                    }
                    for &k in &self.cells[cy as usize * self.nx + cx as usize] {
                        let k = k as usize;
                        let (d, u, p) = point_segment(q, self.a[k], self.b[k]);
                        if best.is_none_or(|b| d < b.d) {
                            best = Some(Near { d, seg: k, u, p });
                        }
                    }
                }
            }
            if best.is_some_and(|b| b.d <= r as f64 * self.cs) {
                break;
            }
        }
        best.filter(|b| b.d <= max_d)
    }

    /// The first crossing of the ray `o + t·d` (`d` a unit vector) with a segment, for `t`
    /// in `[t_min, t_max]`, skipping segments `skip` accepts. Returns `(t, segment, u)`.
    ///
    /// Walks the cells the ray passes through in the order it enters them (the step to the
    /// next cell is across whichever wall, vertical or horizontal, the ray reaches first)
    /// and tests every segment listed there. A crossing found in one cell can lie in a
    /// later one, so the walk continues until the next cell starts beyond the best `t` found;
    /// nothing nearer can then remain. A ray leaving the grid ends the walk.
    pub(crate) fn ray(
        &self,
        o: Point,
        d: Vec2,
        t_min: f64,
        t_max: f64,
        skip: &dyn Fn(usize) -> bool,
    ) -> Option<(f64, usize, f64)> {
        let (mut cx, mut cy) = {
            let (x, y) = self.cell_of(o.x, o.y);
            (x as isize, y as isize)
        };
        // Distance along the ray to the next vertical / horizontal cell wall, and the
        // distance between successive walls of each kind.
        let wall = |c: isize, o: f64, origin: f64, dir: f64| -> (f64, f64, isize) {
            if dir > 0.0 {
                (
                    ((c + 1) as f64 * self.cs + origin - o) / dir,
                    self.cs / dir,
                    1,
                )
            } else if dir < 0.0 {
                ((c as f64 * self.cs + origin - o) / dir, -self.cs / dir, -1)
            } else {
                (f64::INFINITY, f64::INFINITY, 0)
            }
        };
        let (mut tx, dtx, sx) = wall(cx, o.x, self.ox, d.x);
        let (mut ty, dty, sy) = wall(cy, o.y, self.oy, d.y);
        let mut best: Option<(f64, usize, f64)> = None;
        loop {
            for &k in &self.cells[cy as usize * self.nx + cx as usize] {
                let k = k as usize;
                if skip(k) {
                    continue;
                }
                if let Some((t, u)) = ray_segment(o, d, self.a[k], self.b[k]) {
                    if t >= t_min && t <= t_max && best.is_none_or(|b| t < b.0) {
                        best = Some((t, k, u));
                    }
                }
            }
            let t_exit = tx.min(ty);
            if t_exit > t_max || best.is_some_and(|b| t_exit > b.0) {
                break;
            }
            if tx < ty {
                cx += sx;
                tx += dtx;
            } else {
                cy += sy;
                ty += dty;
            }
            if cx < 0 || cy < 0 || cx >= self.nx as isize || cy >= self.ny as isize {
                break;
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square of side 10 as four segments.
    fn square() -> Vec<(Point, Point)> {
        let p = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(0.0, 10.0),
        ];
        (0..4).map(|k| (p[k], p[(k + 1) & 3])).collect()
    }

    #[test]
    fn nearest_matches_brute_force() {
        let segs = square();
        let g = SegGrid::new(&segs, 1.5);
        for &(x, y) in &[
            (5.0, 5.0),
            (3.0, 1.0),
            (-2.0, 4.0),
            (12.0, 13.0),
            (9.9, 0.2),
        ] {
            let q = Point::new(x, y);
            let brute = segs
                .iter()
                .map(|&(a, b)| point_segment(q, a, b).0)
                .fold(f64::INFINITY, f64::min);
            let got = g.nearest(q, 100.0).expect("a segment is in range");
            assert!((got.d - brute).abs() < 1e-12, "{q:?}: {} vs {brute}", got.d);
        }
        assert!(g.nearest(Point::new(5.0, 5.0), 1.0).is_none());
    }

    #[test]
    fn ray_finds_the_far_side() {
        let segs = square();
        let g = SegGrid::new(&segs, 1.0);
        // From the bottom side straight up: the top side is 10 px away.
        let hit = g
            .ray(
                Point::new(4.0, 0.0),
                Vec2 { x: 0.0, y: 1.0 },
                1e-6,
                50.0,
                &|k| k == 0,
            )
            .expect("the top side is hit");
        assert!((hit.0 - 10.0).abs() < 1e-12 && hit.1 == 2);
        // Diagonal from a corner region: first crossing is the right side.
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let hit = g
            .ray(
                Point::new(5.0, 2.0),
                Vec2 { x: s, y: s },
                1e-6,
                50.0,
                &|_| false,
            )
            .expect("a side is hit");
        assert_eq!(hit.1, 1);
        assert!((hit.0 - 5.0 * std::f64::consts::SQRT_2).abs() < 1e-9);
        // Too short a ray finds nothing.
        assert!(g
            .ray(
                Point::new(4.0, 0.0),
                Vec2 { x: 0.0, y: 1.0 },
                1e-6,
                9.0,
                &|k| k == 0
            )
            .is_none());
    }

    #[test]
    fn empty_grid_answers_none() {
        let g = SegGrid::new(&[], 1.0);
        assert_eq!(g.len(), 0);
        assert!(g.nearest(Point::new(0.0, 0.0), 10.0).is_none());
        assert!(g
            .ray(
                Point::new(0.0, 0.0),
                Vec2 { x: 1.0, y: 0.0 },
                0.0,
                10.0,
                &|_| false
            )
            .is_none());
    }
}
