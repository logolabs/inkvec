//! Morphological skeleton thinning (Zhang-Suen), branch tracing, spur pruning,
//! and junction pair collapse into geometric stroke chains.

use inkvec_core::Point;

use super::{median, LevelSet, RegionGrid, CAP_FRACTION};

/// Zhang–Suen thinning to a one-pixel skeleton.
pub(crate) fn zhang_suen(mask: &[bool], w: usize, h: usize) -> Vec<bool> {
    let mut m = mask.to_vec();
    if w < 3 || h < 3 {
        return m;
    }
    let mut doomed: Vec<usize> = Vec::new();
    loop {
        let mut changed = false;
        for pass in 0..2 {
            doomed.clear();
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let p = y * w + x;
                    if !m[p] {
                        continue;
                    }
                    // p2..p9 clockwise from north.
                    let n = [
                        m[p - w],
                        m[p - w + 1],
                        m[p + 1],
                        m[p + w + 1],
                        m[p + w],
                        m[p + w - 1],
                        m[p - 1],
                        m[p - w - 1],
                    ];
                    let b = n.iter().filter(|&&v| v).count();
                    if !(2..=6).contains(&b) {
                        continue;
                    }
                    let mut a = 0;
                    for k in 0..8 {
                        if !n[k] && n[(k + 1) % 8] {
                            a += 1;
                        }
                    }
                    if a != 1 {
                        continue;
                    }
                    let (p2, p4, p6, p8) = (n[0], n[2], n[4], n[6]);
                    let ok = if pass == 0 {
                        !(p2 && p4 && p6) && !(p4 && p6 && p8)
                    } else {
                        !(p2 && p4 && p8) && !(p2 && p6 && p8)
                    };
                    if ok {
                        doomed.push(p);
                    }
                }
            }
            for &p in &doomed {
                m[p] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    m
}

/// The skeleton as a graph over its own pixels.
pub(crate) struct SkelGraph {
    /// Skeleton pixel indices into the region grid, raster order.
    pub(crate) pix: Vec<usize>,
    /// Adjacency in `pix` indices, with shadowed diagonals removed.
    pub(crate) adj: Vec<Vec<usize>>,
}

impl SkelGraph {
    /// Build the graph, dropping any diagonal link that an orthogonal pair already
    /// carries. Without that reduction a simple corner reads as degree 3 and every
    /// corner in the drawing becomes a spurious junction.
    pub(crate) fn build(skel: &[bool], w: usize, h: usize) -> SkelGraph {
        let mut id = vec![usize::MAX; skel.len()];
        let mut pix: Vec<usize> = Vec::new();
        for (p, &s) in skel.iter().enumerate() {
            if s {
                id[p] = pix.len();
                pix.push(p);
            }
        }
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); pix.len()];
        for (k, &p) in pix.iter().enumerate() {
            let (x, y) = ((p % w) as isize, (p / w) as isize);
            let get = |xx: isize, yy: isize| -> bool {
                xx >= 0
                    && yy >= 0
                    && xx < w as isize
                    && yy < h as isize
                    && skel[yy as usize * w + xx as usize]
            };
            for (dx, dy) in [
                (0isize, -1isize),
                (1, 0),
                (0, 1),
                (-1, 0),
                (1, -1),
                (1, 1),
                (-1, 1),
                (-1, -1),
            ] {
                let (nx, ny) = (x + dx, y + dy);
                if !get(nx, ny) {
                    continue;
                }
                if dx != 0 && dy != 0 && (get(x + dx, y) || get(x, y + dy)) {
                    continue; // shadowed diagonal
                }
                adj[k].push(id[ny as usize * w + nx as usize]);
            }
        }
        SkelGraph { pix, adj }
    }

    #[inline]
    pub(crate) fn degree(&self, k: usize) -> usize {
        self.adj[k].len()
    }
}

/// One traced branch of the skeleton graph.
pub(crate) struct Branch {
    /// `pix` indices along the branch, node to node inclusive.
    pub(crate) chain: Vec<usize>,
    pub(crate) a: usize,
    pub(crate) b: usize,
    pub(crate) closed: bool,
}

pub(crate) fn trace_branches(g: &SkelGraph) -> Vec<Branch> {
    let mut out: Vec<Branch> = Vec::new();
    let n = g.pix.len();
    let mut step_used: Vec<Vec<bool>> = g.adj.iter().map(|a| vec![false; a.len()]).collect();
    let mut on_branch = vec![false; n];

    let mark = |step_used: &mut Vec<Vec<bool>>, g: &SkelGraph, from: usize, to: usize| {
        if let Some(s) = g.adj[from].iter().position(|&v| v == to) {
            step_used[from][s] = true;
        }
        if let Some(s) = g.adj[to].iter().position(|&v| v == from) {
            step_used[to][s] = true;
        }
    };

    for start in 0..n {
        if g.degree(start) == 2 {
            continue;
        }
        on_branch[start] = true;
        for s in 0..g.adj[start].len() {
            if step_used[start][s] {
                continue;
            }
            let mut prev = start;
            let mut cur = g.adj[start][s];
            mark(&mut step_used, g, prev, cur);
            let mut chain = vec![start, cur];
            on_branch[cur] = true;
            while g.degree(cur) == 2 {
                let next = g.adj[cur].iter().copied().find(|&v| v != prev);
                let Some(next) = next else { break };
                mark(&mut step_used, g, cur, next);
                chain.push(next);
                on_branch[next] = true;
                prev = cur;
                cur = next;
            }
            out.push(Branch {
                a: start,
                b: cur,
                chain,
                closed: false,
            });
        }
    }

    // Whatever is left is a pure cycle: every pixel degree 2, no node anywhere on it.
    for start in 0..n {
        if on_branch[start] || g.degree(start) != 2 {
            continue;
        }
        let mut chain = vec![start];
        on_branch[start] = true;
        let mut prev = start;
        let mut cur = g.adj[start][0];
        while cur != start {
            on_branch[cur] = true;
            chain.push(cur);
            let Some(next) = g.adj[cur].iter().copied().find(|&v| v != prev) else {
                break;
            };
            prev = cur;
            cur = next;
        }
        out.push(Branch {
            a: start,
            b: start,
            chain,
            closed: true,
        });
    }
    out
}

/// Iteratively remove dead-end branches shorter than `spur_factor` local stroke widths,
/// or that end without a cap.
pub(crate) fn prune_spurs(
    mut skel: Vec<bool>,
    g: &RegionGrid,
    ls: &LevelSet,
    w: usize,
    h: usize,
    spur_factor: f64,
) -> Vec<bool> {
    for _ in 0..16 {
        let graph = SkelGraph::build(&skel, w, h);
        if graph.pix.is_empty() {
            break;
        }
        let branches = trace_branches(&graph);
        let mut cut = false;
        for br in &branches {
            if br.closed {
                continue;
            }
            let (da, db) = (graph.degree(br.a), graph.degree(br.b));
            // A branch whose both ends are free is a whole stroke, not a spur.
            let (tip, root) = match (da == 1, db == 1) {
                (true, false) => (br.a, br.b),
                (false, true) => (br.b, br.a),
                _ => continue,
            };
            if graph.degree(root) < 3 {
                continue;
            }
            let len = chain_length(&graph, g, &br.chain, w);
            let d_tip = ls.nearest(pixel_point(g, graph.pix[tip], w)).0;
            let mut ds: Vec<f64> = br
                .chain
                .iter()
                .map(|&k| ls.nearest(pixel_point(g, graph.pix[k], w)).0)
                .collect();
            let d_med = median(&mut ds);
            let local_width = 2.0 * ds.iter().cloned().fold(0.0f64, f64::max);
            let short = len < spur_factor * local_width;
            let no_cap = d_med > 0.0 && d_tip < CAP_FRACTION * d_med;
            if short || no_cap {
                for &k in &br.chain {
                    if k != root {
                        skel[graph.pix[k]] = false;
                    }
                }
                cut = true;
            }
        }
        if !cut {
            break;
        }
    }
    skel
}

#[inline]
pub(crate) fn pixel_point(g: &RegionGrid, p: usize, w: usize) -> Point {
    g.point_of(p % w, p / w)
}

pub(crate) fn chain_length(graph: &SkelGraph, g: &RegionGrid, chain: &[usize], w: usize) -> f64 {
    chain
        .windows(2)
        .map(|s| pixel_point(g, graph.pix[s[0]], w).dist(pixel_point(g, graph.pix[s[1]], w)))
        .sum()
}

/// A branch as geometry, before sub-pixel refinement.
pub(crate) struct RawChain {
    pub(crate) pts: Vec<Point>,
    pub(crate) closed: bool,
}

/// Turn the pruned skeleton into geometric chains, collapsing junction pairs that sit
/// closer together than the width can resolve.
pub(crate) fn geometric_chains(
    skel: &[bool],
    g: &RegionGrid,
    ls: &LevelSet,
    w: usize,
    h: usize,
) -> Vec<RawChain> {
    let graph = SkelGraph::build(skel, w, h);
    if graph.pix.is_empty() {
        return Vec::new();
    }
    let branches = trace_branches(&graph);

    // Which junction nodes get merged, and where to.
    let mut merged_to: Vec<Option<Point>> = vec![None; graph.pix.len()];
    let mut collapsed: Vec<bool> = vec![false; branches.len()];
    for (bi, br) in branches.iter().enumerate() {
        if br.closed || graph.degree(br.a) < 3 || graph.degree(br.b) < 3 || br.a == br.b {
            continue;
        }
        let pa = pixel_point(g, graph.pix[br.a], w);
        let pb = pixel_point(g, graph.pix[br.b], w);
        let len = chain_length(&graph, g, &br.chain, w);
        let local = 2.0 * ls.nearest(pa).0.min(ls.nearest(pb).0);
        if len <= local {
            let m = Point::new(0.5 * (pa.x + pb.x), 0.5 * (pa.y + pb.y));
            merged_to[br.a] = Some(m);
            merged_to[br.b] = Some(m);
            collapsed[bi] = true;
        }
    }

    let mut out = Vec::new();
    for (bi, br) in branches.iter().enumerate() {
        if collapsed[bi] || br.chain.len() < 2 {
            continue;
        }
        let mut pts: Vec<Point> = br
            .chain
            .iter()
            .map(|&k| pixel_point(g, graph.pix[k], w))
            .collect();
        if let Some(m) = merged_to[br.a] {
            pts[0] = m;
        }
        if let Some(m) = merged_to[br.b] {
            let last = pts.len() - 1;
            pts[last] = m;
        }
        if br.closed {
            // A cycle repeats its start pixel implicitly; Polyline does not.
            if pts.len() > 2 && pts[0].dist(pts[pts.len() - 1]) < 1e-9 {
                pts.pop();
            }
        }
        out.push(RawChain {
            pts,
            closed: br.closed,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zhang_suen_horizontal_bar() {
        let w = 10;
        let h = 7;
        let mut mask = vec![false; w * h];
        // 3-pixel tall bar from y=2..=4, x=1..=8
        for y in 2..=4 {
            for x in 1..=8 {
                mask[y * w + x] = true;
            }
        }
        let thinned = zhang_suen(&mask, w, h);
        let spine_count = (1..=8).filter(|&x| thinned[3 * w + x]).count();
        assert!(
            (4..=8).contains(&spine_count),
            "thinned spine count was {}",
            spine_count
        );
        // Outer rows y=2 and y=4 should have been completely eroded
        let edge_count = (1..=8)
            .filter(|&x| thinned[2 * w + x] || thinned[4 * w + x])
            .count();
        assert_eq!(edge_count, 0, "outer rows were not eroded");
    }

    #[test]
    fn test_skel_graph_and_branches() {
        let w = 5;
        let h = 5;
        let mut skel = vec![false; w * h];
        // A 3-pixel horizontal line at y=2: (1, 2), (2, 2), (3, 2)
        skel[2 * w + 1] = true;
        skel[2 * w + 2] = true;
        skel[2 * w + 3] = true;
        let graph = SkelGraph::build(&skel, w, h);
        assert_eq!(graph.pix.len(), 3);
        let branches = trace_branches(&graph);
        assert_eq!(branches.len(), 1);
        assert_eq!(branches[0].chain.len(), 3);
        assert!(!branches[0].closed);
    }
}
