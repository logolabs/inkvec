//! The chordal axis of a face: a constrained Delaunay triangulation of its measured
//! boundary, its triangles read as terminals, sleeves and junctions, and the graph through
//! the midpoints of the chords.
//!
//! **Why.** The raster skeleton ([`super::graph`]'s Zhang-Suen thinning) gives the
//! topology of thin strokes well, but a stroke that is thick against its own length
//! thins to the wrong graph: lucide `list-checks` at 512 px draws its check marks with a
//! 42.7 px stroke on arms of 60 and 121 px, and thinning erodes the short arm into a
//! stub on a short horizontal line, so the V is lost (the A8 prototype's report). The
//! chordal axis is read from the outline's own geometry instead, at any width.
//!
//! **The triangulation** ([`Cdt::new`]). The boundary points (every ring of the face,
//! after the boundary solve: about one per pixel of outline) are inserted one by one
//! into a Delaunay triangulation inside a large enclosing triangle: the containing
//! triangle is found by walking from the last one created, split in three (or, for a
//! point on an edge, the two triangles on that edge in four), and every edge opposite the
//! new point is flipped while the point across it lies inside its circumcircle (Lawson's
//! flips). The predicates are exact (`inkvec_core::predicates`: orientation and
//! in-circle by adaptive precision). Then every boundary segment that is not yet an
//! edge is forced in by flipping the edges that cross it (Sloan's procedure). At
//! this density almost every boundary segment is already Delaunay.
//!
//! **The axis** ([`axis`]). Triangles inside the face are found by a flood from the inner
//! side of each boundary segment that never crosses one. An inside triangle with two
//! boundary sides is a *terminal* (the tip of a stroke), with one a *sleeve* (a piece of
//! stroke between two chords), with none a *junction*. Every chord (an inside edge that is
//! not a boundary segment) gives a node at its midpoint; a sleeve links its two chords'
//! nodes, a junction links its three to a node at its centroid. Nodes of degree 1 are
//! stroke ends, of degree 3 junctions; the branches between them are what the topology
//! passes read ([`super::graph::Axis`]).
//!
//! Method from: Prasad (2005), Rectification of the chordal axis transform and a new
//! criterion for shape decomposition, Discrete Geometry for Computer Imagery (DGCI),
//! Lecture Notes in Computer Science, 263-275, doi:10.1007/978-3-540-31965-8_25, and
//! Prasad (2007), Rectification of the chordal axis transform skeleton and criteria for
//! shape decomposition, Image and Vision Computing 25(10), 1557-1571,
//! doi:10.1016/j.imavis.2006.06.025 -- the chordal axis transform from a
//! constrained Delaunay triangulation of the outline, with terminal, sleeve and junction
//! triangles. Adapted: the axis only supplies topology here; positions and widths are
//! read from the paired boundary as before ([`super::graph`]), and Prasad's rectification
//! of junction and terminal regions is played by the existing junction rebuild and cap
//! walk. Method from: Chew (1989), Constrained Delaunay triangulations, Algorithmica
//! 4(1-4), 97-108, doi:10.1007/BF01553881 (the triangulation); Lawson's incremental
//! insertion with edge flips, as in Guibas, Stolfi (1985), Primitives for the
//! manipulation of general subdivisions and the computation of Voronoi diagrams, ACM
//! Transactions on Graphics 4(2), 74-123, doi:10.1145/282918.282923; Sloan (1993), A fast
//! algorithm for generating constrained Delaunay triangulations, Computers & Structures
//! 47(3), 441-450, doi:10.1016/0045-7949(93)90239-A (forcing segments in by flips); and
//! Shewchuk (1997), Adaptive precision floating-point arithmetic and fast robust
//! geometric predicates, Discrete & Computational Geometry 18(3), 305-363,
//! doi:10.1007/PL00009321 (the predicates). See also: Zhang et al. (2022) and Berio et al.
//! (2022), cited in [`super::graph`].

use inkvec_core::predicates::{incircle, orient2d};
use inkvec_core::Point;

use super::boundary::Boundary;
use crate::centerline::skeleton::{trace_branches, SkelGraph};

/// No triangle.
const NONE: u32 = u32::MAX;

/// `(i + 1) mod 3` and `(i + 2) mod 3` by table: no `%` in the hot loops (wazero's arm64
/// compiler once miscompiled `i32.rem_u` in one, the Go binding).
const NEXT: [usize; 3] = [1, 2, 0];
/// See [`NEXT`].
const PREV: [usize; 3] = [2, 0, 1];

/// A triangulation: vertices, triangles (counter-clockwise, `orient2d > 0`) and the
/// neighbour across each triangle's edge. Edge `i` of a triangle is the one opposite its
/// vertex `i`, from vertex `NEXT[i]` to vertex `PREV[i]`.
pub(crate) struct Cdt {
    /// Vertex positions; the last three are the enclosing triangle's.
    pts: Vec<Point>,
    /// Each triangle's vertices.
    tv: Vec<[u32; 3]>,
    /// Each triangle's neighbour across edge `i`, or [`NONE`].
    tn: Vec<[u32; 3]>,
    /// A triangle holding each vertex.
    vt: Vec<u32>,
    /// The triangle the last search ended in, where the next one starts.
    last: u32,
}

impl Cdt {
    /// The constrained Delaunay triangulation of `pts` with the segments `segs` (pairs of
    /// indices into `pts`) forced in, or `None` when a point repeats another exactly, a
    /// point lies inside a segment, or a segment cannot be forced in -- the caller then
    /// keeps its raster skeleton.
    pub(crate) fn new(pts: &[Point], segs: &[(usize, usize)]) -> Option<Cdt> {
        let n = pts.len();
        if n < 3 {
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in pts {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        let (cx, cy) = (0.5 * (x0 + x1), 0.5 * (y0 + y1));
        let m = 16.0 * (x1 - x0).max(y1 - y0).max(1.0);
        let mut all = pts.to_vec();
        // A triangle far enough out that its corners never become Delaunay neighbours of
        // anything inside the face; counter-clockwise.
        all.push(Point::new(cx - 2.0 * m, cy - m));
        all.push(Point::new(cx + 2.0 * m, cy - m));
        all.push(Point::new(cx, cy + 2.0 * m));
        let mut t = Cdt {
            pts: all,
            tv: vec![[n as u32, n as u32 + 1, n as u32 + 2]],
            tn: vec![[NONE; 3]],
            vt: vec![0; n + 3],
            last: 0,
        };
        if orient2d(t.pts[n], t.pts[n + 1], t.pts[n + 2]) <= 0.0 {
            return None;
        }
        for i in 0..n {
            t.insert(i as u32)?;
        }
        for &(a, b) in segs {
            t.force(a as u32, b as u32)?;
        }
        Some(t)
    }

    /// Position of vertex `v`.
    fn p(&self, v: u32) -> Point {
        self.pts[v as usize]
    }

    /// The triangle containing point `q`, and where: `Ok((t, None))` strictly inside,
    /// `Ok((t, Some(i)))` on its edge `i`, `Err(v)` on vertex `v`. A walk from the last
    /// triangle: step across any edge with `q` strictly on its far side, starting the edge
    /// scan at a rotating index so the walk cannot cycle on a Delaunay triangulation.
    fn locate(&mut self, q: Point) -> Result<(u32, Option<usize>), u32> {
        let mut t = self.last;
        let mut start = 0usize;
        let limit = 4 * self.tv.len() + 16;
        for _ in 0..limit {
            let tv = self.tv[t as usize];
            let mut moved = false;
            let mut on_edge = None;
            for k in 0..3 {
                let i = [start, NEXT[start], PREV[start]][k];
                let (a, b) = (self.p(tv[NEXT[i]]), self.p(tv[PREV[i]]));
                let o = orient2d(a, b, q);
                if o < 0.0 {
                    let u = self.tn[t as usize][i];
                    if u == NONE {
                        return Err(NONE);
                    }
                    t = u;
                    start = NEXT[start];
                    moved = true;
                    break;
                }
                if o == 0.0 {
                    on_edge = Some(i);
                }
            }
            if moved {
                continue;
            }
            self.last = t;
            if let Some(i) = on_edge {
                // On a vertex when it is on two edges at once.
                for v in tv {
                    if self.p(v).x == q.x && self.p(v).y == q.y {
                        return Err(v);
                    }
                }
                return Ok((t, Some(i)));
            }
            return Ok((t, None));
        }
        Err(NONE)
    }

    /// In triangle `u`, the index of the edge whose neighbour is `t`.
    fn back(&self, u: u32, t: u32) -> usize {
        let n = self.tn[u as usize];
        if n[0] == t {
            0
        } else if n[1] == t {
            1
        } else {
            2
        }
    }

    /// Make triangle `u`'s pointer to `from` point to `to` (no-op for [`NONE`]).
    fn relink(&mut self, u: u32, from: u32, to: u32) {
        if u != NONE {
            let i = self.back(u, from);
            self.tn[u as usize][i] = to;
        }
    }

    /// Set triangle `t` (an index to reuse, or a new one when `t == NONE`) to vertices
    /// `v` and neighbours `n`; returns its index and records it for its vertices.
    fn put(&mut self, t: u32, v: [u32; 3], n: [u32; 3]) -> u32 {
        let t = if t == NONE {
            self.tv.push(v);
            self.tn.push(n);
            (self.tv.len() - 1) as u32
        } else {
            self.tv[t as usize] = v;
            self.tn[t as usize] = n;
            t
        };
        for x in v {
            self.vt[x as usize] = t;
        }
        t
    }

    /// Insert vertex `v` and restore the Delaunay property around it. `None` when it lands
    /// on an existing vertex or the walk fails.
    fn insert(&mut self, v: u32) -> Option<()> {
        let q = self.p(v);
        let (t, edge) = self.locate(q).ok()?;
        let mut stack: Vec<(u32, usize)> = Vec::new();
        match edge {
            None => {
                // Split in three: [v0, v1, P], [v1, v2, P], [v2, v0, P].
                let [v0, v1, v2] = self.tv[t as usize];
                let [n0, n1, n2] = self.tn[t as usize];
                let tb = self.put(NONE, [v1, v2, v], [NONE; 3]);
                let tc = self.put(NONE, [v2, v0, v], [NONE; 3]);
                self.put(t, [v0, v1, v], [tb, tc, n2]);
                self.tn[tb as usize] = [tc, t, n0];
                self.tn[tc as usize] = [t, tb, n1];
                self.relink(n0, t, tb);
                self.relink(n1, t, tc);
                stack.extend([(t, 2), (tb, 2), (tc, 2)]);
            }
            Some(i) => {
                // `v` on edge i of t = [c, a, b] (rotated so c is vertex i), shared with u.
                let tv = self.tv[t as usize];
                let tn = self.tn[t as usize];
                let (c, a, b) = (tv[i], tv[NEXT[i]], tv[PREV[i]]);
                let (tn_a, tn_b) = (tn[NEXT[i]], tn[PREV[i]]);
                let u = tn[i];
                if u == NONE {
                    return None;
                }
                let j = self.back(u, t);
                let uv = self.tv[u as usize];
                let un = self.tn[u as usize];
                let d = uv[j];
                // In u = [d, b, a]: across (b, a) is t; opposite b is (a, d), opposite a
                // is (d, b).
                let (un_b, un_a) = if uv[NEXT[j]] == b {
                    (un[NEXT[j]], un[PREV[j]])
                } else {
                    (un[PREV[j]], un[NEXT[j]])
                };
                let t2 = self.put(NONE, [c, v, b], [NONE; 3]);
                let u2 = self.put(NONE, [d, v, a], [NONE; 3]);
                self.put(t, [c, a, v], [u2, t2, tn_b]);
                self.tn[t2 as usize] = [u, tn_a, t];
                self.put(u, [d, b, v], [t2, u2, un_a]);
                self.tn[u2 as usize] = [t, un_b, u];
                self.relink(tn_a, t, t2);
                self.relink(un_b, u, u2);
                stack.extend([(t, 2), (t2, 1), (u, 2), (u2, 1)]);
            }
        }
        // Lawson's flips: (triangle, index of the new vertex in it); the edge opposite it
        // is checked against the vertex across.
        while let Some((t, i)) = stack.pop() {
            let u = self.tn[t as usize][i];
            if u == NONE {
                continue;
            }
            let tv = self.tv[t as usize];
            let j = self.back(u, t);
            let d = self.tv[u as usize][j];
            if incircle(self.p(tv[0]), self.p(tv[1]), self.p(tv[2]), self.p(d)) <= 0.0 {
                continue;
            }
            let (t2, u2) = self.flip(t, i);
            // After the flip t = [P, a, d] and u = [P, d, b]: their edges opposite P.
            stack.push((t2, 0));
            stack.push((u2, 0));
        }
        Some(())
    }

    /// Flip the edge opposite vertex `i` of triangle `t` (with `P = tv[i]`, the edge
    /// `(a, b)`, and `d` across it in the neighbour `u`): `t` becomes `[P, a, d]` and `u`
    /// becomes `[P, d, b]`. The caller makes sure the quadrilateral is convex. Returns
    /// `(t, u)`.
    fn flip(&mut self, t: u32, i: usize) -> (u32, u32) {
        let tv = self.tv[t as usize];
        let tn = self.tn[t as usize];
        let (p, a, b) = (tv[i], tv[NEXT[i]], tv[PREV[i]]);
        // Across (b, P), opposite a; across (P, a), opposite b.
        let (tn_a, tn_b) = (tn[NEXT[i]], tn[PREV[i]]);
        let u = tn[i];
        let j = self.back(u, t);
        let uv = self.tv[u as usize];
        let un = self.tn[u as usize];
        let d = uv[j];
        // u = [d, b, a] counter-clockwise: opposite b is (a, d), opposite a is (d, b).
        let (un_b, un_a) = if uv[NEXT[j]] == b {
            (un[NEXT[j]], un[PREV[j]])
        } else {
            (un[PREV[j]], un[NEXT[j]])
        };
        self.put(t, [p, a, d], [un_b, u, tn_b]);
        self.put(u, [p, d, b], [un_a, tn_a, t]);
        self.relink(un_b, u, t);
        self.relink(tn_a, t, u);
        (t, u)
    }

    /// Whether `a -> b` is an edge; a walk round `a`'s triangles.
    fn has_edge(&self, a: u32, b: u32) -> bool {
        let start = self.vt[a as usize];
        let mut t = start;
        for _ in 0..self.tv.len() {
            let tv = self.tv[t as usize];
            if tv.contains(&b) {
                return true;
            }
            // Rotate round `a`: across the edge from `a` to the next vertex.
            let k = (0..3).find(|&k| tv[k] == a).unwrap_or(0);
            let u = self.tn[t as usize][PREV[k]];
            if u == NONE || u == start {
                break;
            }
            t = u;
        }
        // The other way round, for a vertex on the hull.
        let mut t = start;
        for _ in 0..self.tv.len() {
            let tv = self.tv[t as usize];
            if tv.contains(&b) {
                return true;
            }
            let k = (0..3).find(|&k| tv[k] == a).unwrap_or(0);
            let u = self.tn[t as usize][NEXT[k]];
            if u == NONE || u == start {
                break;
            }
            t = u;
        }
        false
    }

    /// Force the segment `a -> b` into the triangulation by flipping every edge that
    /// crosses it (Sloan 1993): a crossing edge whose quadrilateral is strictly convex is
    /// flipped, and its new diagonal queued again if it still crosses; a non-convex one
    /// waits its turn. `None` when a vertex lies on the open segment or the flips do not
    /// finish within a bound.
    fn force(&mut self, a: u32, b: u32) -> Option<()> {
        if self.has_edge(a, b) {
            return Some(());
        }
        let (pa, pb) = (self.p(a), self.p(b));
        // Every edge crossing the open segment, as (triangle, edge index).
        let mut queue: std::collections::VecDeque<(u32, u32)> = std::collections::VecDeque::new();
        for t in 0..self.tv.len() as u32 {
            let tv = self.tv[t as usize];
            for i in 0..3 {
                let (x, y) = (tv[NEXT[i]], tv[PREV[i]]);
                if x > y || x == a || x == b || y == a || y == b {
                    continue; // each undirected edge once; edges at a or b do not cross
                }
                if self.crosses(pa, pb, x, y)? {
                    queue.push_back((x, y));
                }
            }
        }
        let mut budget = 64 * (queue.len() + 4);
        while let Some((x, y)) = queue.pop_front() {
            budget = budget.checked_sub(1)?;
            let Some((t, i)) = self.edge_of(x, y) else {
                continue;
            };
            let u = self.tn[t as usize][i];
            if u == NONE {
                return None;
            }
            let p = self.tv[t as usize][i];
            let d = self.tv[u as usize][self.back(u, t)];
            // Convex when the new diagonal p-d separates x and y strictly.
            let (pp, pd, px, py) = (self.p(p), self.p(d), self.p(x), self.p(y));
            let convex = orient2d(pp, pd, px) * orient2d(pp, pd, py) < 0.0;
            if !convex {
                queue.push_back((x, y));
                continue;
            }
            self.flip(t, i);
            if p != a && p != b && d != a && d != b && self.crosses(pa, pb, p, d)? {
                queue.push_back((p.min(d), p.max(d)));
            }
        }
        self.has_edge(a, b).then_some(())
    }

    /// The triangle and edge index of the undirected edge `x - y`, if it exists.
    fn edge_of(&self, x: u32, y: u32) -> Option<(u32, usize)> {
        let start = self.vt[x as usize];
        let mut t = start;
        for dir in [PREV, NEXT] {
            t = start;
            for _ in 0..self.tv.len() {
                let tv = self.tv[t as usize];
                let k = (0..3).find(|&k| tv[k] == x)?;
                if let Some(m) = (0..3).find(|&m| tv[m] == y) {
                    // The edge x-y is opposite the third vertex.
                    let i = 3 - k - m;
                    return Some((t, i));
                }
                let u = self.tn[t as usize][dir[k]];
                if u == NONE || u == start {
                    break;
                }
                t = u;
            }
        }
        let _ = t;
        None
    }

    /// Whether the edge `x - y` crosses the open segment `pa - pb` properly; `None` when
    /// a vertex lies exactly on the segment's line between its ends (the segment cannot
    /// then be an edge).
    fn crosses(&self, pa: Point, pb: Point, x: u32, y: u32) -> Option<bool> {
        let (px, py) = (self.p(x), self.p(y));
        let (o1, o2) = (orient2d(pa, pb, px), orient2d(pa, pb, py));
        let on = |o: f64, q: Point| {
            o == 0.0 && {
                let (dx, dy) = (pb.x - pa.x, pb.y - pa.y);
                let s = (q.x - pa.x) * dx + (q.y - pa.y) * dy;
                s > 0.0 && s < dx * dx + dy * dy
            }
        };
        if on(o1, px) || on(o2, py) {
            return None;
        }
        if o1 * o2 >= 0.0 {
            return Some(false);
        }
        let (o3, o4) = (orient2d(px, py, pa), orient2d(px, py, pb));
        Some(o3 * o4 < 0.0)
    }
}

/// The chordal axis of the face with boundary `b` (see the module documentation): node
/// positions, the graph, and its branches, or `None` when the triangulation fails (the
/// caller keeps its raster skeleton).
pub(crate) fn axis(
    b: &Boundary,
) -> Option<(
    Vec<Point>,
    SkelGraph,
    Vec<crate::centerline::skeleton::Branch>,
)> {
    let n = b.len();
    let segs: Vec<(usize, usize)> = (0..n).map(|i| (i, b.next[i])).collect();
    let cdt = Cdt::new(&b.pts, &segs)?;
    let nt = cdt.tv.len();
    // A boundary segment, either way round.
    let is_seg = |x: u32, y: u32| -> bool {
        let (x, y) = (x as usize, y as usize);
        x < n && y < n && (b.next[x] == y || b.next[y] == x)
    };
    // Inside triangles: seeded on the inner side of every boundary segment, flooded
    // across every edge that is not one.
    let mut inside = vec![false; nt];
    let mut stack: Vec<u32> = Vec::new();
    for t in 0..nt {
        let tv = cdt.tv[t];
        for i in 0..3 {
            let (x, y) = (tv[NEXT[i]] as usize, tv[PREV[i]] as usize);
            // The ring segment this edge is, whichever way round the triangle runs it.
            let seg = if x < n && y < n && b.next[x] == y {
                x
            } else if x < n && y < n && b.next[y] == x {
                y
            } else {
                continue;
            };
            // The triangle is on the segment's inner side when its third vertex is.
            let v = cdt.p(tv[i]) - b.pts[seg];
            if v.x * b.seg_n[seg].x + v.y * b.seg_n[seg].y > 0.0 && !inside[t] {
                inside[t] = true;
                stack.push(t as u32);
            }
        }
    }
    while let Some(t) = stack.pop() {
        let tv = cdt.tv[t as usize];
        for i in 0..3 {
            if is_seg(tv[NEXT[i]], tv[PREV[i]]) {
                continue;
            }
            let u = cdt.tn[t as usize][i];
            // Reaching the enclosing triangle means the flood leaked out: the boundary
            // was not closed off. Give up rather than read the outside as stroke.
            if u == NONE || cdt.tv[u as usize].iter().any(|&v| v as usize >= n) {
                return None;
            }
            if !inside[u as usize] {
                inside[u as usize] = true;
                stack.push(u);
            }
        }
    }
    // Nodes: one per chord (inside edge that is not a boundary segment), one per junction.
    let mut node_of_edge: std::collections::HashMap<(u32, u32), usize> =
        std::collections::HashMap::new();
    let mut pts: Vec<Point> = Vec::new();
    let mut adj: Vec<Vec<usize>> = Vec::new();
    let mut node = |x: u32, y: u32, pts: &mut Vec<Point>, adj: &mut Vec<Vec<usize>>| -> usize {
        let key = (x.min(y), x.max(y));
        *node_of_edge.entry(key).or_insert_with(|| {
            let (p, q) = (cdt.p(x), cdt.p(y));
            pts.push(Point::new(0.5 * (p.x + q.x), 0.5 * (p.y + q.y)));
            adj.push(Vec::new());
            pts.len() - 1
        })
    };
    for t in 0..nt {
        if !inside[t] {
            continue;
        }
        let tv = cdt.tv[t];
        let chords: Vec<usize> = (0..3)
            .filter(|&i| !is_seg(tv[NEXT[i]], tv[PREV[i]]))
            .map(|i| node(tv[NEXT[i]], tv[PREV[i]], &mut pts, &mut adj))
            .collect();
        match chords.len() {
            2 => {
                adj[chords[0]].push(chords[1]);
                adj[chords[1]].push(chords[0]);
            }
            3 => {
                let (p, q, r) = (cdt.p(tv[0]), cdt.p(tv[1]), cdt.p(tv[2]));
                pts.push(Point::new((p.x + q.x + r.x) / 3.0, (p.y + q.y + r.y) / 3.0));
                adj.push(chords.clone());
                let c = pts.len() - 1;
                for &k in &chords {
                    adj[k].push(c);
                }
            }
            _ => {}
        }
    }
    let g = SkelGraph {
        pix: (0..pts.len()).collect(),
        adj,
    };
    let branches = trace_branches(&g);
    Some((pts, g, branches))
}

#[cfg(test)]
mod tests {
    use super::*;
    use inkvec_core::Polyline;

    /// Every triangle counter-clockwise, every neighbour link mutual, and (with no
    /// segments forced) every edge locally Delaunay.
    fn check(t: &Cdt, delaunay: bool) {
        for k in 0..t.tv.len() {
            let v = t.tv[k];
            assert!(
                orient2d(t.p(v[0]), t.p(v[1]), t.p(v[2])) > 0.0,
                "triangle {k} not ccw"
            );
            for i in 0..3 {
                let u = t.tn[k][i];
                if u == NONE {
                    continue;
                }
                assert_eq!(
                    t.tn[u as usize][t.back(u, k as u32)],
                    k as u32,
                    "link {k}-{u}"
                );
                let uv = t.tv[u as usize];
                let (a, b) = (v[NEXT[i]], v[PREV[i]]);
                assert!(
                    uv.contains(&a) && uv.contains(&b),
                    "edge {k}/{i} not shared"
                );
                if delaunay {
                    let d = uv[t.back(u, k as u32)];
                    assert!(incircle(t.p(v[0]), t.p(v[1]), t.p(v[2]), t.p(d)) <= 0.0);
                }
            }
        }
    }

    #[test]
    fn a_grid_of_points_triangulates_delaunay_with_collinear_and_cocircular_points() {
        let mut pts = Vec::new();
        for y in 0..9 {
            for x in 0..11 {
                pts.push(Point::new(x as f64, y as f64 * 0.5));
            }
        }
        let t = Cdt::new(&pts, &[]).expect("a triangulation");
        check(&t, true);
        // Euler: a triangulation of n points with h on the hull has 2n - h - 2 triangles;
        // with the three enclosing corners, n + 3 points, 3 on the hull.
        assert_eq!(t.tv.len(), 2 * (pts.len() + 3) - 3 - 2);
    }

    #[test]
    fn segments_are_forced_in() {
        // Two rows of points far apart and a long diagonal segment across the gap that the
        // Delaunay triangulation would not contain.
        let mut pts = Vec::new();
        for x in 0..20 {
            pts.push(Point::new(x as f64, 0.0));
            pts.push(Point::new(x as f64 + 0.37, 5.0));
        }
        let seg = (0usize, 39usize);
        let t = Cdt::new(&pts, &[seg]).expect("a triangulation");
        assert!(t.has_edge(0, 39));
        check(&t, false);
    }

    /// The outline of a V-shaped stroke (two capsules meeting at `v`), sampled every
    /// ~`step` px, as one ring, and its inside test.
    fn v_stroke(h: f64, step: f64) -> (Polyline, impl Fn(Point) -> bool) {
        let (a, v, c) = (
            Point::new(0.0, 40.0),
            Point::new(40.0, 80.0),
            Point::new(120.0, 0.0),
        );
        let seg_d = |p: Point, s: Point, e: Point| crate::ribbon::grid::point_segment(p, s, e).0;
        let inside = move |p: Point| seg_d(p, a, v).min(seg_d(p, v, c)) <= h;
        // March round the outline: points where the distance field crosses h, by a scan
        // of rays from inside points... simpler: sample the boundary of the union by
        // walking a fine circle of directions from a dense set of axis points and keeping
        // the boundary crossings in angular order round the shape's centre is fragile; use
        // a polygon offset instead: the outline of each capsule, clipped by the other.
        let mut ring = Vec::new();
        let n = 2000;
        // Polar scan round a point inside both arms near the corner, radius by bisection.
        let o = Point::new(40.0, 72.0);
        for k in 0..n {
            let ang = std::f64::consts::TAU * k as f64 / n as f64;
            let dir = (ang.cos(), ang.sin());
            let (mut lo, mut hi) = (0.0, 200.0);
            for _ in 0..60 {
                let mid = 0.5 * (lo + hi);
                if inside(Point::new(o.x + mid * dir.0, o.y + mid * dir.1)) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            ring.push(Point::new(o.x + lo * dir.0, o.y + lo * dir.1));
        }
        let _ = step;
        (Polyline::with_uniform_sigma(ring, 0.05, true), inside)
    }

    #[test]
    fn a_thick_v_has_one_branch_from_tip_to_tip() {
        let (ring, inside) = v_stroke(21.0, 1.0);
        let b = Boundary::new(&[ring], &inside, 2.0).expect("a ring");
        let (pts, g, branches) = axis(&b).expect("an axis");
        assert!(!pts.is_empty());
        assert!(!branches.is_empty());
        // A free end near each cap, and a path between them along the axis about as long
        // as the centreline (56.6 + 113.1 px, less the caps' reach into the tips): the V
        // is one stroke through its corner. Spurs into the convex outer corner are allowed
        // (the topology passes drop branches with no sleeve under them).
        let end_near = |q: Point| -> usize {
            (0..pts.len())
                .filter(|&k| g.degree(k) == 1)
                .min_by(|&i, &j| pts[i].dist(q).total_cmp(&pts[j].dist(q)))
                .expect("free ends")
        };
        let (s, t) = (
            end_near(Point::new(0.0, 40.0)),
            end_near(Point::new(120.0, 0.0)),
        );
        assert!(pts[s].dist(Point::new(0.0, 40.0)) < 25.0, "{:?}", pts[s]);
        assert!(pts[t].dist(Point::new(120.0, 0.0)) < 25.0, "{:?}", pts[t]);
        // Dijkstra by Euclidean length.
        let mut dist = vec![f64::INFINITY; pts.len()];
        let mut done = vec![false; pts.len()];
        dist[s] = 0.0;
        for _ in 0..pts.len() {
            let Some(u) = (0..pts.len())
                .filter(|&k| !done[k] && dist[k].is_finite())
                .min_by(|&i, &j| dist[i].total_cmp(&dist[j]))
            else {
                break;
            };
            done[u] = true;
            for &v in &g.adj[u] {
                let d = dist[u] + pts[u].dist(pts[v]);
                if d < dist[v] {
                    dist[v] = d;
                }
            }
        }
        assert!((120.0..230.0).contains(&dist[t]), "path {}", dist[t]);
    }
}
