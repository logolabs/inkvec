//! Counting the boundary's self-crossings for the fold guard, as a spatial join.
//!
//! The fold guard asks one question several times: how many pairs of boundary segments
//! cross at the start, at the solution, and at a few points on the straight path between
//! the two (the displacement scaled by ½, ¼, …). The pairs worth testing hardly change
//! along that path, since no point moves more than `MAX_TOTAL`. So the candidate pairs are
//! found once and tested at each position, instead of rebuilding a hash grid per count.
//!
//! **The definition counted.** A segment is *bucketed* at a position when its cell range
//! (below) spans `(x1 − x0)·(y1 − y0) ≤ 64`. Two segments are counted when both are
//! bucketed, their cell ranges overlap on both axes, they share no unknown, and
//! [`super::segments_cross`] holds. The cell range of a segment with ends `p`, `q` is
//! `x0 = ⌊min(p.x, q.x) − ½⌋ … x1 = ⌈max(p.x, q.x) + ½⌉`, and likewise in y. Each crossing
//! pair counts once. This is exactly what the per-cell hash grid it replaces counted: two
//! ranges overlap if and only if they have a cell in common, which is when that grid tested
//! the pair, and it deduplicated the pairs it found.
//!
//! **The join.** Following Dittrich & Seeger (2000), *Data redundancy and duplicate
//! detection in spatial join processing*, ICDE, <https://doi.org/10.1109/ICDE.2000.839452>:
//! every segment is entered in each cell of a coarse grid its (swept) range touches, the
//! candidates in each grid cell are compared pairwise, and a pair is reported only in the
//! one cell that holds the *reference point* of the intersection of the two ranges (its
//! low corner). That reports every pair exactly once without a deduplication pass.
//! Adapted: the ranges are *swept* over every position the guard will ask about, and grown
//! by one cell, so one candidate list serves every count; each count then re-applies the
//! exact per-position test above, so the result is the same integer as before.

use super::{segments_cross, PlanarMap, Point, Vars};

/// Side of the coarse grid used to find candidate pairs, in pixel cells. Four pixels is
/// about the length of a swept segment's range, so most ranges touch one to four coarse
/// cells and most coarse cells hold a few dozen entries.
const COARSE: i64 = 4;

/// A swept range spanning more coarse cells than this is paired against every segment
/// directly instead of being entered in the grid. The planar map never has one; it keeps a
/// degenerate segment from filling the grid.
const MAX_COARSE_CELLS: i64 = 64;

/// Segments larger than this (the `(x1 − x0)·(y1 − y0)` of their cell range) are not
/// bucketed, and so never counted. The limit of the grid this replaces.
const MAX_RANGE_AREA: i64 = 64;

/// A segment's cell range, `[x0, x1] × [y0, y1]`, at the positions given.
#[inline]
fn cell_range(p: Point, q: Point) -> [i64; 4] {
    [
        (p.x.min(q.x) - 0.5).floor() as i64,
        (p.x.max(q.x) + 0.5).ceil() as i64,
        (p.y.min(q.y) - 0.5).floor() as i64,
        (p.y.max(q.y) + 0.5).ceil() as i64,
    ]
}

/// Whether two cell ranges have a cell in common.
#[inline]
fn overlap(a: &[i64; 4], b: &[i64; 4]) -> bool {
    a[0] <= b[1] && b[0] <= a[1] && a[2] <= b[3] && b[2] <= a[3]
}

/// The boundary's segments as pairs of unknowns, in edge order.
pub(super) fn segments(map: &PlanarMap, vars: &Vars) -> Vec<(u32, u32)> {
    let mut segs: Vec<(u32, u32)> = Vec::new();
    for (k, e) in map.edges.iter().enumerate() {
        let ids = &vars.var[k];
        let n = ids.len();
        if n < 2 {
            continue;
        }
        let last = if e.closed { n } else { n - 1 };
        for i in 0..last {
            segs.push((ids[i], ids[(i + 1) % n]));
        }
    }
    segs
}

/// The pairs of segments that can cross anywhere on the path between two sets of
/// positions, found once; [`FoldCounter::count`] then counts the crossings at any position
/// on that path.
pub(super) struct FoldCounter {
    segs: Vec<(u32, u32)>,
    /// Candidate pairs `(i, j)`, `i < j`, sharing no unknown and with overlapping swept
    /// ranges. Every pair that can be counted at a position on the path is here.
    cand: Vec<(u32, u32)>,
}

impl FoldCounter {
    /// Candidates for every position `a + s·(b − a)`, `s ∈ [0, 1]`.
    ///
    /// Such a point lies in the box spanned by its two ends (up to rounding, which the
    /// one-cell growth covers many times over), so a segment's cell range there lies inside
    /// the union of its ranges at `a` and at `b`, grown by one cell: the swept range.
    pub(super) fn new(map: &PlanarMap, vars: &Vars, a: &[Point], b: &[Point]) -> Self {
        let segs = segments(map, vars);
        let swept: Vec<[i64; 4]> = segs
            .iter()
            .map(|&(u, v)| {
                let (u, v) = (u as usize, v as usize);
                let ra = cell_range(a[u], a[v]);
                let rb = cell_range(b[u], b[v]);
                [
                    ra[0].min(rb[0]).saturating_sub(1),
                    ra[1].max(rb[1]).saturating_add(1),
                    ra[2].min(rb[2]).saturating_sub(1),
                    ra[3].max(rb[3]).saturating_add(1),
                ]
            })
            .collect();
        let cand = candidates(&segs, &swept);
        FoldCounter { segs, cand }
    }

    /// The number of crossing pairs at `pos` (see the module docs for the exact rule).
    pub(super) fn count(&self, pos: &[Point]) -> usize {
        let ranges: Vec<Option<[i64; 4]>> = self
            .segs
            .iter()
            .map(|&(u, v)| {
                let r = cell_range(pos[u as usize], pos[v as usize]);
                ((r[1] - r[0]) * (r[3] - r[2]) <= MAX_RANGE_AREA).then_some(r)
            })
            .collect();
        let mut n = 0usize;
        for &(i, j) in &self.cand {
            let (Some(ri), Some(rj)) = (&ranges[i as usize], &ranges[j as usize]) else {
                continue;
            };
            if !overlap(ri, rj) {
                continue;
            }
            let (s, t) = (self.segs[i as usize], self.segs[j as usize]);
            if segments_cross(
                pos[s.0 as usize],
                pos[s.1 as usize],
                pos[t.0 as usize],
                pos[t.1 as usize],
            ) {
                n += 1;
            }
        }
        n
    }
}

/// Pairs `(i, j)`, `i < j`, whose ranges overlap and whose segments share no unknown,
/// each once, by a coarse-grid join with reference-point duplicate avoidance.
fn candidates(segs: &[(u32, u32)], range: &[[i64; 4]]) -> Vec<(u32, u32)> {
    let share = |i: usize, j: usize| {
        let (s, t) = (segs[i], segs[j]);
        s.0 == t.0 || s.0 == t.1 || s.1 == t.0 || s.1 == t.1
    };
    let coarse = |r: &[i64; 4]| {
        [
            r[0].div_euclid(COARSE),
            r[1].div_euclid(COARSE),
            r[2].div_euclid(COARSE),
            r[3].div_euclid(COARSE),
        ]
    };
    let mut big: Vec<usize> = Vec::new();
    let mut small: Vec<usize> = Vec::new();
    let (mut gx0, mut gx1, mut gy0, mut gy1) = (i64::MAX, i64::MIN, i64::MAX, i64::MIN);
    for (i, r) in range.iter().enumerate() {
        let c = coarse(r);
        let cells = (c[1].saturating_sub(c[0]).saturating_add(1))
            .saturating_mul(c[3].saturating_sub(c[2]).saturating_add(1));
        if cells > MAX_COARSE_CELLS || cells <= 0 {
            big.push(i);
            continue;
        }
        small.push(i);
        gx0 = gx0.min(c[0]);
        gx1 = gx1.max(c[1]);
        gy0 = gy0.min(c[2]);
        gy1 = gy1.max(c[3]);
    }
    let mut out: Vec<(u32, u32)> = Vec::new();
    if !small.is_empty() {
        let gw = (gx1 - gx0 + 1) as usize;
        let gh = (gy1 - gy0 + 1) as usize;
        let cell_of = |x: i64, y: i64| (y - gy0) as usize * gw + (x - gx0) as usize;
        // Counting sort of (cell, segment) entries into a compressed row layout.
        let mut start = vec![0u32; gw * gh + 1];
        for &i in &small {
            let c = coarse(&range[i]);
            for y in c[2]..=c[3] {
                for x in c[0]..=c[1] {
                    start[cell_of(x, y) + 1] += 1;
                }
            }
        }
        for k in 0..gw * gh {
            start[k + 1] += start[k];
        }
        let mut fill = start.clone();
        let mut list = vec![0u32; start[gw * gh] as usize];
        for &i in &small {
            let c = coarse(&range[i]);
            for y in c[2]..=c[3] {
                for x in c[0]..=c[1] {
                    let k = cell_of(x, y);
                    list[fill[k] as usize] = i as u32;
                    fill[k] += 1;
                }
            }
        }
        for y in gy0..=gy1 {
            for x in gx0..=gx1 {
                let k = cell_of(x, y);
                let bucket = &list[start[k] as usize..start[k + 1] as usize];
                for (ai, &a) in bucket.iter().enumerate() {
                    let (a, ca) = (a as usize, coarse(&range[a as usize]));
                    for &b in &bucket[ai + 1..] {
                        let b = b as usize;
                        let cb = coarse(&range[b]);
                        // The reference point: the low corner of the two coarse ranges'
                        // intersection. Only the cell holding it reports the pair.
                        if ca[0].max(cb[0]) != x || ca[2].max(cb[2]) != y {
                            continue;
                        }
                        if !overlap(&range[a], &range[b]) || share(a, b) {
                            continue;
                        }
                        out.push((a.min(b) as u32, a.max(b) as u32));
                    }
                }
            }
        }
    }
    // The few segments too large for the grid are paired with everything directly.
    for (bi, &a) in big.iter().enumerate() {
        let others = small.iter().chain(big[bi + 1..].iter());
        for &b in others {
            if overlap(&range[a], &range[b]) && !share(a, b) {
                out.push((a.min(b) as u32, a.max(b) as u32));
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "folds_tests.rs"]
mod tests;
