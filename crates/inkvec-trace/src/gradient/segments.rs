//! Discontinuity-aware smooth segmentation: the regions a gradient *could* fill, found
//! without the palette. Research prototype A10, part `segments` ([`super::gregions`]).
//!
//! # Problem
//!
//! The palette sees a gradient as a stack of bands, one per ink the ramp crosses. At icon
//! resolution the bands are one or two pixels wide: none has an interior, each fits "flat"
//! on the evidence it has, and the band merger's pairwise unions (two bands at a time)
//! rarely show enough ramp to win. The question "which pixels does one gradient fill" is
//! a segmentation question, and it can be asked of the image directly: a gradient region is
//! one where the colour changes a little per pixel, bounded by places where it jumps.
//!
//! # Method
//!
//! Method from: S. Chakraborty et al. (2025), Image Vectorization via Gradient
//! Reconstruction, Computer Graphics Forum 44(2), doi:10.1111/cgf.70055, §3.2
//! ("discontinuity-aware segmentation"). Ported from the unmerged acda7ac
//! (experiment/gradient-segments) by way of research2/gradients (2ca274f). The steps, in
//! the order [`smooth_segments`] runs them:
//!
//! 1. **Discontinuity map `D`**: a pixel is in `D` when the colour steps by more than
//!    [`TAU_D`] to one of its 4-neighbours. On a clean render that is the anti-aliasing at
//!    shape boundaries; a ramp, however large its total change, moves a little per pixel.
//! 2. **Colour-difference segmentation `S0`** of the smooth pixels `D̄`: 4-neighbours both in
//!    `D̄` whose colours differ by less than [`TAU_S`] share a segment (union-find).
//! 3. **Discontinuity relation `A`**: for each pixel `p ∈ D` and each of the directions
//!    right, down and down-right, the first smooth pixel within [`SIGMA`] steps on either
//!    side; when those belong to two different segments `u ≠ v`, `p` counts once towards
//!    `f_A(u, v)`. The pair is in `A` when `f_A(u, v) > τ_a · |∂u ∩ ∂v|` ([`TAU_A`]), the
//!    right-hand side being the number of 4-neighbour pixel pairs the two segments share
//!    directly (paper eq. 3). Such a pair faces itself across a discontinuity, so however
//!    the two connect elsewhere they must not be one region.
//! 4. **Multicut**: the segment adjacency graph, edge weight `exp(−|ū − v̄|² / 2)` on the
//!    segments' mean colours, is cut pair of `A` by pair (most evidence first) along a
//!    minimum cut (Edmonds–Karp max-flow) wherever the pair is still connected (paper eq. 4,
//!    solved as the paper does: one min-cut per pair, "at most n times"). The final regions
//!    are the connected components of what is left.
//!
//! Adapted, and why:
//! * Colours are compared in OKLab scaled by 100 (≈ CIELAB units, which the paper uses),
//!   because OKLab is the engine's perceptual space everywhere else.
//! * `D` is a plain neighbour-step threshold, not the discontinuity set of the paper's
//!   Mumford–Shah pre-smoothing (§3.1, eq. 1–2): that smoothing suppresses resampling noise,
//!   and the images this runs on are clean renders or have been through the noise guards
//!   upstream.
//! * The paper's `τ_s = 10` and `σ = 5 px` are for 512–2048 px photographs and illustrations;
//!   acda7ac set `τ_d = τ_s = 6` and `σ = 3` for 128 px icons, and the round-2 research found
//!   `τ = 4/4` no better (+4.3 % dE00 against +3.8 %). They are kept, not tuned.
//! * `S0` links *pixels* (a step below `τ_s` between two smooth neighbours), where the paper
//!   merges *segments* whose colours differ by less than `τ_s`. A consequence worth knowing:
//!   with `τ_s = τ_d`, two distinct segments of `S0` can only touch through a smooth pixel
//!   pair whose step is exactly `τ_d`, so the segment graph has almost no edges, the
//!   multicut has almost nothing to cut, and the regions are the components of `S0`. A weak
//!   edge (a stretch of the boundary that steps by less than `τ_d`) therefore joins two
//!   regions here, where the paper's multicut would sever it. The prototype leaves that to
//!   the acceptance step ([`super::proposals`]), which is where the round-2 research placed
//!   the fault; making the multicut bind (segment-mean merging, or `τ_s < τ_d`) is the next
//!   experiment if weak-edge merges survive it.
//!
//! # Data layout and cost
//!
//! Pixels are indexed `y·w + x`. The segmentation is one `u32` per pixel: a dense region id
//! from 0, or [`DISCONTINUITY`] for pixels in `D`. Every pass over the image walks rows
//! then columns (no `%` per pixel: the wazero arm64 miscompile met `i32.rem_u` in a hot
//! loop). Steps 1–3 are O(W·H). Step 4 is one max-flow per pair of `A` on the segment graph
//! (tens to hundreds of nodes on a 128 px icon); its work is counted and capped at
//! [`MAX_CUT_VISITS`] node visits, past which no segmentation is returned at all and the
//! band merger runs as if the part were off -- a deterministic bound, not a clock.
//!
//! # Where it sits
//!
//! [`super::proposals`] calls [`smooth_segments`] once per `merge_bands` call (the
//! `merge_bands` stage of Quality, and its native-alpha twin), before the agglomeration.

use std::collections::{BTreeMap, HashMap, VecDeque};

/// Region id of the pixels in the discontinuity map `D`.
pub(crate) const DISCONTINUITY: u32 = u32::MAX;

/// Colour step to a 4-neighbour above which a pixel is in `D`: OKLab × 100.
pub(crate) const TAU_D: f32 = 6.0;
/// Colour step between two smooth 4-neighbours below which they share a segment, OKLab ×
/// 100 (the paper's `τ_s`).
pub(crate) const TAU_S: f32 = 6.0;
/// How far, px, a discontinuity looks along each direction for the segments on its two
/// sides (the paper's `σ`).
pub(crate) const SIGMA: usize = 3;
/// Share of a directly shared boundary that must also be crossed by discontinuity pixels
/// for a pair to be in `A` (the paper's `τ_a`, its value).
pub(crate) const TAU_A: f32 = 0.25;
/// Most node visits all the min-cuts of one image may make. A 128 px emoji needs a few
/// thousand; past this the segmentation is abandoned (see the module docs).
pub(crate) const MAX_CUT_VISITS: usize = 4_000_000;

/// sRGB (0..1) to OKLab scaled by 100, so distances read roughly as CIELAB ΔE.
fn lab100(c: [f32; 3]) -> [f32; 3] {
    let o = crate::color::rgb_to_oklab(c);
    [o.l * 100.0, o.a * 100.0, o.b * 100.0]
}

/// Euclidean distance of two colours.
fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Union-find over `0..n` with path compression. A union keeps the *smaller* root, so the
/// representative of every set is its smallest element whatever order the unions came in:
/// the dense numbering built from it does not depend on that order.
struct Dsu(Vec<u32>);

impl Dsu {
    /// The representative of `a`'s set, compressing the path to it.
    fn find(&mut self, a: u32) -> u32 {
        let mut r = a;
        while self.0[r as usize] != r {
            r = self.0[r as usize];
        }
        let mut x = a;
        while self.0[x as usize] != r {
            let next = self.0[x as usize];
            self.0[x as usize] = r;
            x = next;
        }
        r
    }

    /// Join the sets of `a` and `b`, the smaller representative becoming the root.
    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.0[hi as usize] = lo;
        }
    }
}

/// The smooth segmentation of the sRGB image `rgb` (0..1, `w`×`h`): per pixel a dense
/// region id from 0, or [`DISCONTINUITY`] on `D`. `None` when the min-cuts would exceed
/// [`MAX_CUT_VISITS`]. An image with no smooth pixel is all `D` (and `Some`).
pub(crate) fn smooth_segments(rgb: &[[f32; 3]], w: usize, h: usize) -> Option<Vec<u32>> {
    let n = w * h;
    let lab: Vec<[f32; 3]> = rgb.iter().map(|&c| lab100(c)).collect();
    let step = |p: usize, q: usize| dist(lab[p], lab[q]);

    // 1. D: a step above τ_d to any 4-neighbour.
    let mut in_d = vec![false; n];
    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            in_d[p] = (x > 0 && step(p, p - 1) > TAU_D)
                || (x + 1 < w && step(p, p + 1) > TAU_D)
                || (y > 0 && step(p, p - w) > TAU_D)
                || (y + 1 < h && step(p, p + w) > TAU_D);
        }
    }

    // 2. S0: smooth 4-neighbours closer than τ_s share a segment.
    let mut dsu = Dsu((0..n as u32).collect());
    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            if in_d[p] {
                continue;
            }
            if x + 1 < w && !in_d[p + 1] && step(p, p + 1) < TAU_S {
                dsu.union(p as u32, (p + 1) as u32);
            }
            if y + 1 < h && !in_d[p + w] && step(p, p + w) < TAU_S {
                dsu.union(p as u32, (p + w) as u32);
            }
        }
    }
    // Dense ids in raster order of each segment's first pixel.
    let mut dense: HashMap<u32, u32> = HashMap::new();
    let mut s0 = vec![DISCONTINUITY; n];
    for p in 0..n {
        if !in_d[p] {
            let root = dsu.find(p as u32);
            let next = dense.len() as u32;
            s0[p] = *dense.entry(root).or_insert(next);
        }
    }
    let m = dense.len();
    if m == 0 {
        return Some(s0);
    }

    // Segment mean colours (f64 sums: order-independent to far below a colour step) and
    // the directly shared boundary of every adjacent pair, in 4-neighbour pixel pairs.
    let mut sum = vec![[0f64; 4]; m];
    for p in 0..n {
        if s0[p] != DISCONTINUITY {
            let s = &mut sum[s0[p] as usize];
            for c in 0..3 {
                s[c] += lab[p][c] as f64;
            }
            s[3] += 1.0;
        }
    }
    let mean: Vec<[f32; 3]> = sum
        .iter()
        .map(|s| {
            [
                (s[0] / s[3]) as f32,
                (s[1] / s[3]) as f32,
                (s[2] / s[3]) as f32,
            ]
        })
        .collect();
    let mut shared: HashMap<(u32, u32), u32> = HashMap::new();
    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            let right = (x + 1 < w).then_some(p + 1);
            let down = (y + 1 < h).then_some(p + w);
            for q in [right, down].into_iter().flatten() {
                let (a, b) = (s0[p], s0[q]);
                if a != DISCONTINUITY && b != DISCONTINUITY && a != b {
                    *shared.entry((a.min(b), a.max(b))).or_insert(0) += 1;
                }
            }
        }
    }

    // 3. A, then 4. the multicut.
    let pairs = discontinuity_pairs(&s0, &in_d, w, h, &shared);
    multicut(&s0, m, &mean, &shared, pairs)
}

/// The discontinuity relation `A` with its evidence `f_A`, most evidenced first (ties by
/// the pair, ascending), as the module docs' step 3 defines it. `shared` holds each
/// directly adjacent pair's boundary length, keyed `(min, max)`; a pair absent from it
/// shares no boundary, so any evidence puts it in `A`.
///
/// O(|D| · 3 · SIGMA). The ordering is the paper's "heuristic" for the cut order: the
/// best-evidenced separations are made first, while the graph is still whole.
fn discontinuity_pairs(
    s0: &[u32],
    in_d: &[bool],
    w: usize,
    h: usize,
    shared: &HashMap<(u32, u32), u32>,
) -> Vec<((u32, u32), u32)> {
    // The first smooth segment met within SIGMA steps from (x, y) along (dx, dy).
    let seg_along = |x: usize, y: usize, dx: isize, dy: isize| -> Option<u32> {
        let (mut x, mut y) = (x as isize, y as isize);
        for _ in 0..SIGMA {
            x += dx;
            y += dy;
            if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
                return None;
            }
            let s = s0[y as usize * w + x as usize];
            if s != DISCONTINUITY {
                return Some(s);
            }
        }
        None
    };
    const DIRS: [(isize, isize); 3] = [(1, 0), (0, 1), (1, 1)];
    let mut f_a: HashMap<(u32, u32), u32> = HashMap::new();
    for y in 0..h {
        for x in 0..w {
            if !in_d[y * w + x] {
                continue;
            }
            // A pixel testifies once per pair, however many directions see it.
            let mut seen: Vec<(u32, u32)> = Vec::new();
            for &(dx, dy) in &DIRS {
                if let (Some(u), Some(v)) = (seg_along(x, y, dx, dy), seg_along(x, y, -dx, -dy)) {
                    let key = (u.min(v), u.max(v));
                    if u != v && !seen.contains(&key) {
                        seen.push(key);
                    }
                }
            }
            for key in seen {
                *f_a.entry(key).or_insert(0) += 1;
            }
        }
    }
    let mut pairs: Vec<((u32, u32), u32)> = f_a
        .into_iter()
        .filter(|(k, f)| *f as f32 > TAU_A * shared.get(k).copied().unwrap_or(0) as f32)
        .collect();
    pairs.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    pairs
}

/// The final region of every pixel: the segment graph cut apart along every pair of
/// `pairs`, then labelled by connected component (dense ids by smallest segment). `None`
/// when the cuts exceed [`MAX_CUT_VISITS`].
///
/// Adjacency is a `BTreeMap` per node so every traversal visits neighbours in ascending
/// order: the augmenting paths, and with them the cut chosen among equal-weight cuts, do
/// not depend on a hasher.
fn multicut(
    s0: &[u32],
    m: usize,
    mean: &[[f32; 3]],
    shared: &HashMap<(u32, u32), u32>,
    pairs: Vec<((u32, u32), u32)>,
) -> Option<Vec<u32>> {
    let mut adj: Vec<BTreeMap<u32, f64>> = vec![BTreeMap::new(); m];
    for &(a, b) in shared.keys() {
        let d = dist(mean[a as usize], mean[b as usize]) as f64;
        // Floored so that no edge is free to cut: equal-colour neighbours stay expensive
        // relative to it, distant ones all cost about the same.
        let wgt = (-d * d / 2.0).exp().max(1e-12);
        adj[a as usize].insert(b, wgt);
        adj[b as usize].insert(a, wgt);
    }
    let mut budget = MAX_CUT_VISITS;
    for ((s, t), _) in pairs {
        if !min_cut(&mut adj, s, t, &mut budget) {
            return None;
        }
    }
    let mut dsu = Dsu((0..m as u32).collect());
    for (a, nb) in adj.iter().enumerate() {
        for &b in nb.keys() {
            dsu.union(a as u32, b);
        }
    }
    let mut dense: HashMap<u32, u32> = HashMap::new();
    let final_id: Vec<u32> = (0..m as u32)
        .map(|s| {
            let root = dsu.find(s);
            let next = dense.len() as u32;
            *dense.entry(root).or_insert(next)
        })
        .collect();
    Some(
        s0.iter()
            .map(|&s| {
                if s == DISCONTINUITY {
                    DISCONTINUITY
                } else {
                    final_id[s as usize]
                }
            })
            .collect(),
    )
}

/// Separate `s` from `t` in the undirected weighted graph `adj` by removing a minimum-
/// weight set of edges, if the two are connected. Returns false when `budget` (node visits,
/// decremented) runs out first.
///
/// Edmonds–Karp: augment along shortest residual paths (BFS) until `t` is unreachable; the
/// nodes still reachable from `s` in the residual graph are the source side, and every
/// edge leaving it is saturated and is removed (the max-flow min-cut theorem). An
/// undirected edge of weight `c` is two arcs of capacity `c` with antisymmetric flow, so
/// the residual of `a → b` is `c − flow(a, b)`. Residuals at or below 1e-15 count as
/// saturated. Already disconnected pairs return at once with nothing removed.
fn min_cut(adj: &mut [BTreeMap<u32, f64>], s: u32, t: u32, budget: &mut usize) -> bool {
    let m = adj.len();
    let mut flow: HashMap<(u32, u32), f64> = HashMap::new();
    let residual = |adj: &[BTreeMap<u32, f64>], flow: &HashMap<(u32, u32), f64>, a: u32, b: u32| {
        adj[a as usize].get(&b).copied().unwrap_or(0.0) - flow.get(&(a, b)).copied().unwrap_or(0.0)
    };
    loop {
        let mut prev = vec![u32::MAX; m];
        prev[s as usize] = s;
        let mut queue = VecDeque::from([s]);
        while let Some(a) = queue.pop_front() {
            if a == t {
                break;
            }
            if *budget == 0 {
                return false;
            }
            *budget -= 1;
            for &b in adj[a as usize].keys() {
                if prev[b as usize] == u32::MAX && residual(adj, &flow, a, b) > 1e-15 {
                    prev[b as usize] = a;
                    queue.push_back(b);
                }
            }
        }
        if prev[t as usize] == u32::MAX {
            break;
        }
        let mut bottleneck = f64::INFINITY;
        let mut b = t;
        while b != s {
            let a = prev[b as usize];
            bottleneck = bottleneck.min(residual(adj, &flow, a, b));
            b = a;
        }
        let mut b = t;
        while b != s {
            let a = prev[b as usize];
            *flow.entry((a, b)).or_insert(0.0) += bottleneck;
            *flow.entry((b, a)).or_insert(0.0) -= bottleneck;
            b = a;
        }
    }
    // The source side: everything reachable from s through unsaturated arcs.
    let mut side = vec![false; m];
    side[s as usize] = true;
    let mut queue = VecDeque::from([s]);
    while let Some(a) = queue.pop_front() {
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        for &b in adj[a as usize].keys() {
            if !side[b as usize] && residual(adj, &flow, a, b) > 1e-15 {
                side[b as usize] = true;
                queue.push_back(b);
            }
        }
    }
    if side[t as usize] {
        return true; // never connected: nothing to cut
    }
    for a in 0..m {
        if !side[a] {
            continue;
        }
        let cut: Vec<u32> = adj[a]
            .keys()
            .copied()
            .filter(|&b| !side[b as usize])
            .collect();
        for b in cut {
            adj[a].remove(&b);
            adj[b as usize].remove(&(a as u32));
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w`×`h` image from a colour function of `(x, y)`.
    fn image(w: usize, h: usize, f: impl Fn(usize, usize) -> [f32; 3]) -> Vec<[f32; 3]> {
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| f(x, y))
            .collect()
    }

    #[test]
    fn a_ramp_is_one_segment_and_a_step_splits_it() {
        let (w, h) = (24, 16);
        // A horizontal ramp from dark to light grey over the width: small steps per pixel.
        let ramp = image(w, h, |x, _| [0.2 + 0.4 * x as f32 / w as f32; 3]);
        let seg = smooth_segments(&ramp, w, h).expect("within budget");
        assert!(seg.iter().all(|&s| s == 0), "one smooth region");
        // A hard step in the middle: the step columns are D, the two halves differ.
        let step = image(w, h, |x, _| {
            if x < 12 {
                [0.1, 0.2, 0.8]
            } else {
                [0.9, 0.7, 0.1]
            }
        });
        let seg = smooth_segments(&step, w, h).expect("within budget");
        assert_eq!(seg[11], DISCONTINUITY);
        assert_eq!(seg[12], DISCONTINUITY);
        assert_ne!(seg[0], seg[w - 1]);
    }

    #[test]
    fn a_smooth_gap_in_a_wall_joins_the_two_sides() {
        // Two flats within τ_s of each other, separated by a dark wall with a two-pixel
        // gap at the bottom: S0 links them through the gap (the documented weak-edge leak
        // of the port), while a wall with no gap keeps them apart.
        let (w, h) = (20, 20);
        let walled = |gap: bool| {
            image(w, h, move |x, y| {
                if (9..11).contains(&x) && (y < 18 || !gap) {
                    [0.0; 3]
                } else if x < 10 {
                    [0.50, 0.50, 0.50]
                } else {
                    [0.53, 0.53, 0.53]
                }
            })
        };
        let seg = smooth_segments(&walled(true), w, h).expect("within budget");
        assert_eq!(seg[5 * w + 2], seg[5 * w + 17], "leak through the gap");
        let seg = smooth_segments(&walled(false), w, h).expect("within budget");
        let (l, r) = (seg[5 * w + 2], seg[5 * w + 17]);
        assert!(l != DISCONTINUITY && r != DISCONTINUITY);
        assert_ne!(l, r, "a closed wall separates them");
    }

    #[test]
    fn the_multicut_severs_a_pair_of_the_relation_along_its_cheapest_link() {
        // Three segments in a row, 0 - 1 - 2, where 0 and 2 are a pair of A: the cheaper of
        // the two links (to the more different neighbour) is cut, the other kept.
        let s0 = vec![0u32, 1, 2];
        let mean = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [4.0, 0.0, 0.0]];
        let shared: HashMap<(u32, u32), u32> = [((0, 1), 1), ((1, 2), 1)].into_iter().collect();
        let out = multicut(&s0, 3, &mean, &shared, vec![((0, 2), 5)]).expect("within budget");
        assert_eq!(
            out[0], out[1],
            "the cheap-to-keep link (similar colours) survives"
        );
        assert_ne!(out[1], out[2], "the link between different colours is cut");
        // No pair: one region.
        let out = multicut(&s0, 3, &mean, &shared, vec![]).expect("within budget");
        assert!(out.iter().all(|&r| r == 0));
    }

    #[test]
    fn flat_and_degenerate_images() {
        let seg = smooth_segments(&[[0.3; 3]; 12], 4, 3).expect("within budget");
        assert!(seg.iter().all(|&s| s == 0));
        assert_eq!(smooth_segments(&[], 0, 0), Some(Vec::new()));
        let one = smooth_segments(&[[0.1, 0.9, 0.3]], 1, 1).expect("within budget");
        assert_eq!(one, vec![0]);
        // A single row with a step: both sides of it in D, ends smooth and apart.
        let row = image(6, 1, |x, _| if x < 3 { [0.0; 3] } else { [1.0; 3] });
        let seg = smooth_segments(&row, 6, 1).expect("within budget");
        assert_eq!(&seg[2..4], &[DISCONTINUITY, DISCONTINUITY]);
        assert_ne!(seg[0], seg[5]);
    }

    #[test]
    fn a_min_cut_removes_the_cheapest_separation() {
        // s=0 - 1 - 2=t with a cheap middle edge and a parallel expensive path 0 - 3 - 2.
        let mut adj: Vec<BTreeMap<u32, f64>> = vec![BTreeMap::new(); 4];
        let mut edge = |a: u32, b: u32, c: f64| {
            adj[a as usize].insert(b, c);
            adj[b as usize].insert(a, c);
        };
        edge(0, 1, 5.0);
        edge(1, 2, 0.1);
        edge(0, 3, 1.0);
        edge(3, 2, 0.2);
        let mut budget = 1000;
        assert!(min_cut(&mut adj, 0, 2, &mut budget));
        assert!(!adj[1].contains_key(&2) && !adj[3].contains_key(&2));
        assert!(adj[0].contains_key(&1) && adj[0].contains_key(&3));
        // A budget too small to finish abandons the cut.
        let mut tiny = 1;
        let mut adj2: Vec<BTreeMap<u32, f64>> = vec![BTreeMap::new(); 3];
        adj2[0].insert(1, 1.0);
        adj2[1].insert(0, 1.0);
        adj2[1].insert(2, 1.0);
        adj2[2].insert(1, 1.0);
        assert!(!min_cut(&mut adj2, 0, 2, &mut tiny));
    }
}
