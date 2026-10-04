//! The nearest of many short line pieces to a query point, by a bounding-box hierarchy
//! built along the pieces' own order.
//!
//! **What it is for.** The stroke solve measures every boundary point against the
//! flattened centrelines thousands of times per face ([`super::refine`]): which piece is
//! nearest decides the segment a point belongs to and the parameter its exact foot is
//! polished from. The boundary points sit about a half-width `h` from the centrelines, so
//! a uniform grid of fixed cells ([`super::grid::SegGrid`], 2 px) has to search `h/2` rings
//! of cells before it can stop -- at 512 px (h about 21 px) some 500 cells per query, and a
//! measurement of lucide `vegan`'s 5300 boundary points took 20 ms. A hierarchy of boxes
//! prunes by distance instead of by cells, so its cost does not grow with `h`.
//!
//! **Layout.** The pieces are kept in the order they were flattened -- along each
//! segment, segment after segment -- so consecutive pieces are neighbours in the plane.
//! Level 0 cuts them into leaves of [`LEAF`] consecutive pieces, each with its bounding box;
//! every level above boxes two consecutive nodes of the one below, up to a single root.
//! Building is `O(n)`; nothing is sorted.
//!
//! **Query** ([`PieceTree::nearest`]). Depth first from the root, the nearer child first;
//! a node whose box is at least as far as the best piece found so far (or beyond the reach)
//! is skipped, as no piece inside it can be nearer. The answer is the nearest piece, as
//! the grid's was; the two can differ only between pieces at exactly the same distance
//! (the grid kept the first in its cell order, this keeps the first in its visiting order).
//! About `O(log n)` boxes and a few leaves per query.
//!
//! Inspired by: the bounding volume hierarchies of ray tracing and collision detection;
//! the build by merging neighbours along a sequence follows the linear BVH of Lauterbach,
//! Garland, Sengupta, Luebke, Manocha (2009), Fast BVH construction on GPUs, Computer
//! Graphics Forum 28(2), 375-384, doi:10.1111/j.1467-8659.2009.01377.x, which orders
//! primitives along a space-filling curve and groups neighbours in that order. Ours needs no
//! curve: flattening already lists the pieces along the centrelines. The pruned depth-first
//! search with the nearer child first is the classic branch-and-bound nearest neighbour of
//! Fukunaga, Narendra (1975), A branch and bound algorithm for computing k-nearest
//! neighbors, IEEE Transactions on Computers C-24(7), 750-753, doi:10.1109/T-C.1975.224297.

use inkvec_core::Point;

use super::grid::{point_segment, Near};

/// Pieces per leaf.
const LEAF: usize = 8;

/// An axis-aligned box.
#[derive(Clone, Copy, Debug)]
struct Aabb {
    /// Left, px.
    x0: f64,
    /// Top, px.
    y0: f64,
    /// Right, px.
    x1: f64,
    /// Bottom, px.
    y1: f64,
}

impl Aabb {
    /// The box of segment `a -> b`.
    fn of(a: Point, b: Point) -> Aabb {
        Aabb {
            x0: a.x.min(b.x),
            y0: a.y.min(b.y),
            x1: a.x.max(b.x),
            y1: a.y.max(b.y),
        }
    }

    /// The smallest box holding both.
    fn union(self, o: Aabb) -> Aabb {
        Aabb {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }

    /// Squared distance from `p` to the box (0 inside): a lower bound on the squared
    /// distance to anything in it.
    fn dist2(&self, p: Point) -> f64 {
        let dx = (self.x0 - p.x).max(p.x - self.x1).max(0.0);
        let dy = (self.y0 - p.y).max(p.y - self.y1).max(0.0);
        dx * dx + dy * dy
    }
}

/// Line pieces `a[k] -> b[k]` under a bounding-box hierarchy (see the module docs).
pub(crate) struct PieceTree {
    /// First endpoint of each piece.
    a: Vec<Point>,
    /// Second endpoint of each piece.
    b: Vec<Point>,
    /// `levels[0][i]` boxes pieces `LEAF·i ..`; `levels[k][i]` boxes nodes `2i` and
    /// `2i + 1` of level `k - 1`. The last level has one node. Empty for no pieces.
    levels: Vec<Vec<Aabb>>,
}

impl PieceTree {
    /// The hierarchy over `segs`, in their order. `O(n)`.
    pub(crate) fn new(segs: &[(Point, Point)]) -> PieceTree {
        let mut levels: Vec<Vec<Aabb>> = Vec::new();
        if !segs.is_empty() {
            let leaves: Vec<Aabb> = segs
                .chunks(LEAF)
                .map(|c| {
                    c.iter()
                        .map(|&(a, b)| Aabb::of(a, b))
                        .reduce(Aabb::union)
                        .expect("chunks are never empty")
                })
                .collect();
            levels.push(leaves);
            while levels.last().is_some_and(|l| l.len() > 1) {
                let up: Vec<Aabb> = levels
                    .last()
                    .expect("a level")
                    .chunks(2)
                    .map(|c| if c.len() == 2 { c[0].union(c[1]) } else { c[0] })
                    .collect();
                levels.push(up);
            }
        }
        PieceTree {
            a: segs.iter().map(|s| s.0).collect(),
            b: segs.iter().map(|s| s.1).collect(),
            levels,
        }
    }

    /// The nearest piece point to `q` within `max_d` px, or `None` when there is none:
    /// branch and bound over the hierarchy, nearer child first. Ties keep the first piece
    /// visited.
    pub(crate) fn nearest(&self, q: Point, max_d: f64) -> Option<Near> {
        let top = self.levels.len().checked_sub(1)?;
        let mut best: Option<Near> = None;
        let mut bound2 = max_d * max_d;
        // (level, index) of the nodes still to visit; at most two per level are pending.
        let mut stack: Vec<(usize, usize)> = Vec::with_capacity(2 * self.levels.len() + 2);
        stack.push((top, 0));
        while let Some((lv, i)) = stack.pop() {
            if self.levels[lv][i].dist2(q) > bound2 {
                continue;
            }
            if lv == 0 {
                let end = (LEAF * (i + 1)).min(self.a.len());
                for k in LEAF * i..end {
                    let (d, u, p) = point_segment(q, self.a[k], self.b[k]);
                    if d * d <= bound2 && best.is_none_or(|b| d < b.d) {
                        best = Some(Near { d, seg: k, u, p });
                        bound2 = d * d;
                    }
                }
                continue;
            }
            let below = &self.levels[lv - 1];
            let (c0, c1) = (2 * i, 2 * i + 1);
            if c1 >= below.len() {
                stack.push((lv - 1, c0));
                continue;
            }
            // Push the farther child first so the nearer one is searched first.
            if below[c0].dist2(q) <= below[c1].dist2(q) {
                stack.push((lv - 1, c1));
                stack.push((lv - 1, c0));
            } else {
                stack.push((lv - 1, c0));
                stack.push((lv - 1, c1));
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ribbon::grid::SegGrid;

    #[test]
    fn the_tree_finds_the_nearest_distance_the_grid_finds() {
        // A wavy polyline of short pieces, a circle of them, and queries all round.
        let mut segs = Vec::new();
        let mut prev = Point::new(0.0, 0.0);
        for i in 1..300 {
            let t = i as f64 * 0.1;
            let p = Point::new(t * 3.0, 10.0 * (t * 0.7).sin());
            segs.push((prev, p));
            prev = p;
        }
        for i in 0..64 {
            let (a, b) = (
                std::f64::consts::TAU * i as f64 / 64.0,
                std::f64::consts::TAU * (i + 1) as f64 / 64.0,
            );
            segs.push((
                Point::new(30.0 + 8.0 * a.cos(), 20.0 + 8.0 * a.sin()),
                Point::new(30.0 + 8.0 * b.cos(), 20.0 + 8.0 * b.sin()),
            ));
        }
        let (tree, grid) = (PieceTree::new(&segs), SegGrid::new(&segs, 2.0));
        for i in 0..600 {
            let q = Point::new(
                -15.0 + 0.2 * i as f64,
                -25.0 + (i as f64 * 0.37).sin() * 50.0,
            );
            for max_d in [1.0, 6.0, 80.0] {
                match (tree.nearest(q, max_d), grid.nearest(q, max_d)) {
                    (None, None) => {}
                    (Some(t), Some(g)) => {
                        assert_eq!(t.d.to_bits(), g.d.to_bits(), "{q:?}");
                        // Ties aside, the same piece.
                        let (dt, _, _) = point_segment(q, segs[g.seg].0, segs[g.seg].1);
                        assert_eq!(dt.to_bits(), t.d.to_bits());
                    }
                    (t, g) => panic!("{q:?} {max_d}: {t:?} vs {g:?}"),
                }
            }
        }
    }

    #[test]
    fn an_empty_tree_and_a_single_piece() {
        assert!(PieceTree::new(&[])
            .nearest(Point::new(0.0, 0.0), 10.0)
            .is_none());
        let one = PieceTree::new(&[(Point::new(0.0, 0.0), Point::new(4.0, 0.0))]);
        let n = one.nearest(Point::new(2.0, 3.0), 10.0).expect("in reach");
        assert!((n.d - 3.0).abs() < 1e-12 && n.seg == 0 && (n.u - 0.5).abs() < 1e-12);
        assert!(one.nearest(Point::new(2.0, 3.0), 2.9).is_none());
    }
}
