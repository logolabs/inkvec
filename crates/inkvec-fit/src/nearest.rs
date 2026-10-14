//! Exact nearest-piece queries over a chain, pruned by bounding boxes.
//!
//! [`crate::multimodel::path_chi2`] charges every measured point its squared distance to the
//! nearest segment of a path, and an arc enters it as chords every 0.02 px (sampled, so a
//! clean circle is scored as itself and not as its chord). A ring of arcs 300 px long is
//! then some 15 000 pieces, and scanning all of them for each of its points is tens of
//! millions of nearest-point solves for one evaluation. The editability passes price every
//! candidate edit with two such evaluations (`inkvec-cli`'s `editable::price`), which made
//! the trace of a stroked icon sixty times slower.
//!
//! [`BoxTree`] returns the same minimum without visiting the pieces that cannot hold it. The
//! pieces of a chain follow one another, so a run of consecutive pieces has a small bounding
//! box; the tree halves the chain recursively and keeps each run's box. A query walks it
//! nearer box first and skips any run whose box is already further than the best piece
//! found so far: the branch-and-bound nearest-neighbour search over a bounding-volume
//! hierarchy (Friedman, Bentley & Finkel, "An algorithm for finding best matches in
//! logarithmic expected time", ACM TOMS 3(3), 1977, on k-d trees). The answer is unchanged:
//!
//! - every piece the walk skips lies in a box further than the best value already found,
//!   and its own value can only be larger than its box's bound, so it was not the minimum;
//! - the minimum of a set of floats is one of its elements whatever order they are compared
//!   in, so the value returned is bit for bit the one the full scan returns.
//!
//! The first point needs a piece's *computed* value never to fall below its box's bound,
//! which rounding could break by an ulp where the two are equal in exact arithmetic. So each
//! box is grown by [`MARGIN`], far above coordinate rounding at any raster size the program
//! traces, and each bound shrunk by the relative [`SHRINK`]: a bound that is merely close
//! prunes a little less, never wrongly.

use crate::curves::Segment;
use crate::FittedPath;
use inkvec_core::Point;
use kurbo::{CubicBez, Line as KLine, ParamCurveNearest, Point as KPoint};

/// How far (px) every box is grown before a bound is taken from it. Coordinates are
/// rounded at about 1e-13 px on a 4096 px raster; this is a million times that.
pub const MARGIN: f64 = 1e-7;

/// The relative amount every bound is shrunk by, for the rounding of the weighted value
/// (a division by an interpolated sigma) against the bound's.
pub const SHRINK: f64 = 1e-9;

/// The most pieces a leaf of the tree holds; a leaf's pieces are checked one by one.
const LEAF: usize = 4;

/// Marks a leaf in [`Node::left`].
const NO_CHILD: u32 = u32::MAX;

/// One piece of a chain as the tree sees it: the box that holds it, and a weight `w` such
/// that the piece's value at any point `p` is at least `w · d²`, `d` the distance from `p`
/// to the box. A plain squared distance has `w = 1`; a squared distance over a sigma no
/// larger than `s` has `w = 1/s²`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Piece {
    /// The box's smallest corner.
    pub min: Point,
    /// The box's largest corner.
    pub max: Point,
    /// The weight on the squared distance to the box.
    pub weight: f64,
}

impl Piece {
    /// The box around `points` (a segment's ends, a cubic's four control points), with
    /// `weight`.
    pub fn around(points: &[Point], weight: f64) -> Self {
        let mut min = Point::new(f64::INFINITY, f64::INFINITY);
        let mut max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in points {
            min = Point::new(min.x.min(p.x), min.y.min(p.y));
            max = Point::new(max.x.max(p.x), max.y.max(p.y));
        }
        Self { min, max, weight }
    }

    /// A lower bound on the value of anything in this box at `p` (see the module docs for
    /// why it is grown and shrunk). Zero inside the box, and for a `p` with a NaN, which
    /// is then never pruned.
    fn bound(&self, p: Point) -> f64 {
        let dx = (self.min.x - MARGIN - p.x)
            .max(p.x - self.max.x - MARGIN)
            .max(0.0);
        let dy = (self.min.y - MARGIN - p.y)
            .max(p.y - self.max.y - MARGIN)
            .max(0.0);
        self.weight * (dx * dx + dy * dy) * (1.0 - SHRINK)
    }

    /// The box holding `self` and `o`, at the smaller weight, so its bound is a bound for
    /// both.
    fn union(&self, o: &Self) -> Self {
        Self {
            min: Point::new(self.min.x.min(o.min.x), self.min.y.min(o.min.y)),
            max: Point::new(self.max.x.max(o.max.x), self.max.y.max(o.max.y)),
            weight: self.weight.min(o.weight),
        }
    }
}

/// A run of consecutive pieces `lo..hi`: its box, and its two halves unless it is a leaf.
#[derive(Debug, Clone, Copy)]
struct Node {
    /// The box and weight bounding every piece of the run.
    bounds: Piece,
    /// The run's first piece.
    lo: u32,
    /// One past the run's last piece.
    hi: u32,
    /// The first half's node, or [`NO_CHILD`] for a leaf.
    left: u32,
    /// The second half's node.
    right: u32,
}

/// Bounding boxes over the runs of a chain of pieces, for exact minimum queries that skip
/// the runs that cannot hold the minimum. See the module docs.
#[derive(Debug, Clone)]
pub struct BoxTree {
    /// The pieces, in chain order.
    pieces: Vec<Piece>,
    /// The runs; the root is the last.
    nodes: Vec<Node>,
}

impl BoxTree {
    /// The tree over `pieces`, kept in their order: consecutive pieces of a chain lie near
    /// each other, which is what makes a run's box small.
    pub fn new(pieces: Vec<Piece>) -> Self {
        let mut tree = Self {
            nodes: Vec::with_capacity(pieces.len() / 2 + 1),
            pieces,
        };
        if !tree.pieces.is_empty() {
            tree.build(0, tree.pieces.len());
        }
        tree
    }

    /// The number of pieces.
    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    /// Whether there are no pieces.
    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    /// The node for the run `lo..hi` (not empty), its halves pushed first; returns its
    /// index.
    fn build(&mut self, lo: usize, hi: usize) -> u32 {
        let (bounds, left, right) = if hi - lo <= LEAF {
            let b = self.pieces[lo + 1..hi]
                .iter()
                .fold(self.pieces[lo], |acc, p| acc.union(p));
            (b, NO_CHILD, NO_CHILD)
        } else {
            let mid = lo + (hi - lo) / 2;
            let l = self.build(lo, mid);
            let r = self.build(mid, hi);
            let b = self.nodes[l as usize]
                .bounds
                .union(&self.nodes[r as usize].bounds);
            (b, l, r)
        };
        self.nodes.push(Node {
            bounds,
            lo: lo as u32,
            hi: hi as u32,
            left,
            right,
        });
        (self.nodes.len() - 1) as u32
    }

    /// The smallest `value(i)` over every piece `i`, exactly as `(0..len).map(value)
    /// .fold(f64::INFINITY, f64::min)` would return it, calling `value` only on pieces whose
    /// box could hold something smaller than the best so far. `value(i)` must be at least
    /// the piece's weight times its squared distance from `p` to the piece (the contract of
    /// [`Piece`]). Infinite for an empty tree.
    pub fn min_by(&self, p: Point, mut value: impl FnMut(usize) -> f64) -> f64 {
        let mut best = f64::INFINITY;
        let Some(root) = self.nodes.len().checked_sub(1) else {
            return best;
        };
        // Depth-first, nearer half first. A halving tree over fewer than 2^60 pieces is
        // under 60 deep, and the stack never holds more than one entry per level plus one.
        let mut stack = [(0u32, 0.0f64); 64];
        stack[0] = (root as u32, 0.0);
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let (n, b) = stack[top];
            // Strictly further only: a tie could be the minimum's own value.
            if b > best {
                continue;
            }
            let node = self.nodes[n as usize];
            if node.left == NO_CHILD {
                for i in node.lo as usize..node.hi as usize {
                    if self.pieces[i].bound(p) > best {
                        continue;
                    }
                    best = best.min(value(i));
                }
                continue;
            }
            let bl = self.nodes[node.left as usize].bounds.bound(p);
            let br = self.nodes[node.right as usize].bounds.bound(p);
            // The further half goes on first, so the nearer is taken off first.
            let (near, far) = if bl <= br {
                ((node.left, bl), (node.right, br))
            } else {
                ((node.right, br), (node.left, bl))
            };
            stack[top] = far;
            stack[top + 1] = near;
            top += 2;
        }
        best
    }
}

/// A path segment in kurbo's types, for its exact nearest-point queries.
enum KSeg {
    /// A straight segment.
    L(KLine),
    /// A cubic Bézier.
    C(CubicBez),
}

/// A path's segments ready for exact nearest-distance queries: kurbo's segments, arcs
/// replaced by fine chords (0.02 px spacing), under a [`BoxTree`].
pub(crate) struct PathPieces {
    /// The segments, in path order.
    segs: Vec<KSeg>,
    /// Their boxes: a line's two ends, a cubic's four control points (the curve lies in
    /// their convex hull).
    tree: BoxTree,
}

impl PathPieces {
    /// The pieces of `path`; `None` for a path with no segments.
    pub(crate) fn new(path: &FittedPath) -> Option<Self> {
        if path.segments.is_empty() {
            return None;
        }
        let kp = |p: Point| KPoint::new(p.x, p.y);
        let mut segs = Vec::with_capacity(path.segments.len());
        let mut boxes = Vec::with_capacity(path.segments.len());
        let mut cur = path.start;
        for s in &path.segments {
            match *s {
                Segment::Line(p) => {
                    segs.push(KSeg::L(KLine::new(kp(cur), kp(p))));
                    boxes.push(Piece::around(&[cur, p], 1.0));
                    cur = p;
                }
                Segment::Cubic(a, b, p) => {
                    segs.push(KSeg::C(CubicBez::new(kp(cur), kp(a), kp(b), kp(p))));
                    boxes.push(Piece::around(&[cur, a, b, p], 1.0));
                    cur = p;
                }
                // Sampled finely, not chorded. The chord over-states an arc's distance by
                // its own sagitta — which is the entire reason for drawing one — and that was
                // tolerable only while arcs came from elsewhere. Now the program emits them,
                // so scoring one as its chord would report a clean circle as millions of
                // units of cost and hide whatever it is being compared against.
                Segment::Arc { end, .. } => {
                    let pts = crate::curves::sample_run(cur, std::slice::from_ref(s), 0.02);
                    for w in pts.windows(2) {
                        segs.push(KSeg::L(KLine::new(kp(w[0]), kp(w[1]))));
                        boxes.push(Piece::around(w, 1.0));
                    }
                    cur = end;
                }
            }
        }
        let tree = BoxTree::new(boxes);
        Some(Self { segs, tree })
    }

    /// Squared distance (px²) from `p` to the nearest segment, by kurbo's nearest-point
    /// solvers (accuracy 1e-9 on lines, 1e-6 on cubics): the minimum over every segment,
    /// computed on the few whose boxes could hold it.
    pub(crate) fn dist2(&self, p: Point) -> f64 {
        let q = KPoint::new(p.x, p.y);
        self.tree.min_by(p, |i| match &self.segs[i] {
            KSeg::L(l) => l.nearest(q, 1e-9).distance_sq,
            KSeg::C(c) => c.nearest(q, 1e-6).distance_sq,
        })
    }

    /// The same minimum by scanning every segment, for the tests.
    #[cfg(test)]
    pub(crate) fn dist2_scan(&self, p: Point) -> f64 {
        let q = KPoint::new(p.x, p.y);
        self.segs
            .iter()
            .map(|s| match s {
                KSeg::L(l) => l.nearest(q, 1e-9).distance_sq,
                KSeg::C(c) => c.nearest(q, 1e-6).distance_sq,
            })
            .fold(f64::INFINITY, f64::min)
    }
}

#[cfg(test)]
mod tests;
