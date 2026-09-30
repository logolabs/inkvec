//! The cracks of a label map and their incidence: the first two steps of [`super::build`].
//!
//! # The problem
//!
//! A planar map of a label image is built from its *cracks*: the pixel sides that separate
//! two different labels (the 1-cells between pixels in the cell-complex view of a digital
//! image). [`super::build`] needs every crack as a unit segment between two grid nodes, with
//! the labels on its two sides ([`dual_segments`]), and then, for every node, the cracks
//! that meet there ([`Incidence`]), to walk them into edges.
//!
//! # Data layout
//!
//! Grid node `(i, j)`, `0 ≤ i ≤ w`, `0 ≤ j ≤ h`, is the top-left corner of pixel `(i, j)` and
//! has id `j · (w + 1) + i` ([`super::node_id`]); a node split at a saddle has a second id one
//! whole grid further on. A [`Seg`] is one crack, oriented so its `left` label is on the left
//! walking from `a` to `b` (y down); `u16::MAX` stands for the outside of the image. The
//! label map is read through its row runs ([`RowRuns`]): maximal horizontal intervals of
//! one label.
//!
//! # Passes
//!
//! 1. [`dual_segments`]: code the rows as runs, then read the vertical cracks off the run
//!    boundaries and the horizontal ones off the overlaps of consecutive rows' runs.
//!    Output order: vertical cracks row by row, then horizontal ones node row by node row,
//!    each left to right — the order the pixel scan it replaced produced.
//! 2. [`Incidence::new`]: list both ends of every crack in emission order and stable-sort
//!    them by node with a radix sort ([`radix_sort_by_node`]); record each end's node so a
//!    walk steps from a crack to its far node's list in `O(1)` ([`Incidence::end`]).
//!
//! Both were rewritten on 2026-09-30 from a pixel scan and a comparison sort; the old forms
//! are kept in `cracks_tests` and the tests assert element-for-element equality. At 2048 px
//! the two old forms took 9.9 ms and 2 x 1.5 ms of `build_map`'s 15.9 ms (mean, opaque
//! `big` set), in Fast and Quality alike.

use super::node_id;
use super::runs::{overlaps, RowRuns};

/// A boundary segment on the dual grid, separating two pixels of differing label.
///
/// It runs from node `a` to node `b`, one pixel side long; `left` and `right` are the face
/// labels on either side when walking from `a` to `b` (`u16::MAX` outside the image).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Seg {
    /// Node the segment starts at.
    pub(super) a: u32,
    /// Node the segment ends at.
    pub(super) b: u32,
    /// Label on the left, walking from `a` to `b`.
    pub(super) left: u16,
    /// Label on the right, walking from `a` to `b`.
    pub(super) right: u16,
}

/// The segments incident to each node, in segment order: every node that has any, in
/// increasing id, with its segments in one flat list. A map from node to segment list, in
/// sorted arrays rather than a hash map of vectors -- the same lists, without an allocation
/// and a hash per boundary node.
///
/// Built by a radix sort rather than a comparison sort, and it also records, for each
/// segment end, which node it is, so a walk from a segment to the node at its far end
/// ([`Incidence::end`]) is an array read instead of a binary search over the nodes.
pub(super) struct Incidence {
    /// Nodes with at least one segment, increasing.
    pub(super) nodes: Vec<u32>,
    /// Where each node's segments start in `segs`; one more entry than `nodes`.
    pub(super) start: Vec<usize>,
    /// Segment indices, grouped by node and increasing within a node.
    pub(super) segs: Vec<usize>,
    /// For segment `k`, `node_of[2k]` is the index in `nodes` of its end `a` and
    /// `node_of[2k + 1]` that of its end `b`.
    node_of: Vec<u32>,
}

impl Incidence {
    /// Index `segs` by endpoint: each segment is listed under both of its nodes, in segment
    /// order within a node.
    ///
    /// # Method
    ///
    /// The ends are listed in *emission order*: end `a` of segment `k` as entry `2k` and end
    /// `b` as `2k + 1`, each carrying its node id. A stable sort of that list by node id
    /// groups the ends by node and keeps, inside a group, the emission order — which is
    /// increasing `k`, because entry `2k + e` comes before `2k' + e'` whenever `k < k'`. That
    /// is exactly the order the previous comparison sort of `(node, k)` pairs produced (the
    /// pairs are distinct, since a unit segment's two ends are different nodes, so that
    /// sort's order was total and unique). The stable sort is a least-significant-digit
    /// radix sort ([`radix_sort_by_node`]), `O(passes · (m + 2^11))` for `m = 2 · segs`
    /// ends and `⌈bits / 11⌉` passes over the node id's significant bits (three at 2048 px),
    /// where the comparison sort was `O(m log m)`: 1.5 ms at 2048 px, plus as much again in
    /// `split_saddle_corners`, which builds one too (mean over the opaque `big` set).
    ///
    /// Method from: Knuth 1998, "The Art of Computer Programming, Vol. 3: Sorting and
    /// Searching", 2nd ed., Addison-Wesley, ISBN 0-201-89685-0, §5.2.5 (sorting by
    /// distribution, least significant digit first, stable per pass). The research report
    /// proposed "a radix sort or the emission order"; this uses both.
    pub(super) fn new(segs: &[Seg]) -> Self {
        let mut ends: Vec<(u32, usize)> = Vec::with_capacity(2 * segs.len());
        for (k, s) in segs.iter().enumerate() {
            ends.push((s.a, 2 * k));
            ends.push((s.b, 2 * k + 1));
        }
        let ends = radix_sort_by_node(ends);
        let mut nodes = Vec::new();
        let mut start = Vec::new();
        let mut node_of = vec![0u32; ends.len()];
        for (i, &(n, e)) in ends.iter().enumerate() {
            if nodes.last() != Some(&n) {
                nodes.push(n);
                start.push(i);
            }
            // `nodes.len() - 1` is this end's node's index; node ids are u32, so there are
            // fewer than 2^32 distinct nodes and the index fits.
            node_of[e] = (nodes.len() - 1) as u32;
        }
        start.push(ends.len());
        Self {
            nodes,
            start,
            // Entry `e` belongs to segment `e / 2` (a shift, not a division by a variable).
            segs: ends.into_iter().map(|(_, e)| e >> 1).collect(),
            node_of,
        }
    }

    /// The segments at the `i`-th node of `nodes`.
    pub(super) fn at(&self, i: usize) -> &[usize] {
        &self.segs[self.start[i]..self.start[i + 1]]
    }

    /// The segments at node `n`, if it has any. `O(log nodes)`; the walks use
    /// [`Incidence::end`] instead.
    #[cfg(test)]
    pub(super) fn get(&self, n: u32) -> Option<&[usize]> {
        self.nodes.binary_search(&n).ok().map(|i| self.at(i))
    }

    /// The segments at one end of segment `k`: its end `b` when `at_b`, else its end `a`.
    ///
    /// The same list as `get(segs[k].b)` (or `.a`), since that node certainly has segment
    /// `k`; `O(1)`, one array read. The walks step from a segment to its far node on every
    /// point of every chain, and a binary search there cost a cache miss per halving.
    pub(super) fn end(&self, k: usize, at_b: bool) -> &[usize] {
        self.at(self.node_of[2 * k + usize::from(at_b)] as usize)
    }

    /// The comparison-sort construction [`Incidence::new`] replaced, kept as its test
    /// reference: `(node, segment)` pairs sorted, then grouped.
    #[cfg(test)]
    pub(super) fn new_sorted(segs: &[Seg]) -> (Vec<u32>, Vec<usize>, Vec<usize>) {
        let mut pairs: Vec<(u32, usize)> = Vec::with_capacity(2 * segs.len());
        for (k, s) in segs.iter().enumerate() {
            pairs.push((s.a, k));
            pairs.push((s.b, k));
        }
        pairs.sort_unstable();
        let mut nodes = Vec::new();
        let mut start = Vec::new();
        for (i, &(n, _)) in pairs.iter().enumerate() {
            if nodes.last() != Some(&n) {
                nodes.push(n);
                start.push(i);
            }
        }
        start.push(pairs.len());
        (nodes, start, pairs.into_iter().map(|(_, k)| k).collect())
    }
}

/// Stable sort of `(node, payload)` pairs by `node`: a least-significant-digit radix sort
/// in digits of 11 bits (2,048 buckets, 16 KiB of counters, which stay in L1 cache).
///
/// Each pass is a counting sort on one digit: count the digit's values, turn the counts
/// into starting offsets (an exclusive prefix sum), then place every pair at its digit's
/// next offset, visiting the pairs in their current order — so equal digits keep their
/// order and the pass is stable. After the passes over every digit that can be non-zero
/// (from the least significant up), the pairs are sorted by the whole node id, and pairs
/// with equal ids are in their input order. Passes stop at the highest set bit of the
/// largest id; an input whose ids are all 0 is returned as it is.
///
/// `O(passes · (m + 2^11))` time and one extra buffer of `m` pairs, for `m` pairs.
///
/// Method from: Knuth 1998, TAOCP Vol. 3 (2nd ed.), §5.2.5, least-significant-digit radix
/// sorting; adapted only in the digit width.
pub(super) fn radix_sort_by_node(mut src: Vec<(u32, usize)>) -> Vec<(u32, usize)> {
    const DIGIT: u32 = 11;
    const MASK: u32 = (1 << DIGIT) - 1;
    let max = src.iter().map(|p| p.0).max().unwrap_or(0);
    let bits = u32::BITS - max.leading_zeros();
    let mut dst = vec![(0u32, 0usize); src.len()];
    let mut shift = 0;
    while shift < bits {
        let mut offset = [0usize; 1 << DIGIT];
        for p in &src {
            offset[((p.0 >> shift) & MASK) as usize] += 1;
        }
        let mut sum = 0;
        for o in offset.iter_mut() {
            let c = *o;
            *o = sum;
            sum += c;
        }
        for p in &src {
            let d = ((p.0 >> shift) & MASK) as usize;
            dst[offset[d]] = *p;
            offset[d] += 1;
        }
        std::mem::swap(&mut src, &mut dst);
        shift += DIGIT;
    }
    src
}

/// Every pixel side separating two labels, as a unit segment on the dual grid.
///
/// Vertical sides first (row by row), then horizontal ones. Each is oriented so that the
/// label it records as `left` is on the left when walking from `a` to `b` in image
/// coordinates (y down).
///
/// # Method
///
/// The pixel sides are the *cracks* of the image's cell complex, and a crack separates two
/// labels exactly where the label changes across it. The label map is first coded as row
/// runs ([`RowRuns`]), and the separating cracks are read off the runs:
///
/// * **vertical cracks of row `j`**, left to right: the image's left border (node column
///   0) unless the row starts with the outside label `u16::MAX`; one crack at every run
///   start `x0 > 0`, where the label changes by the definition of a run, with the new run's
///   label on the left (walking down, `+y`) and the previous run's on the right; and the
///   right border (node column `w`) unless the row ends with `u16::MAX`;
/// * **horizontal cracks of node row `j`**, left to right: along the top border (`j = 0`)
///   and the bottom border (`j = h`) every pixel of row 0, respectively `h − 1`, whose label
///   is not `u16::MAX`; between rows `j − 1` and `j`, the [`overlaps`] of the two rows,
///   one crack per pixel of every overlap whose two labels differ, the upper label on the
///   left (walking right, `+x`). Two equal rows have no such overlap and are skipped whole.
///
/// # Why the output is identical to the pixel scan
///
/// The scan visited node columns `i = 0..=w` of each row `j` for the vertical cracks, then
/// pixels `i = 0..w` of each node row `j = 0..=h` for the horizontal ones, and pushed a
/// segment wherever the two labels differed, with the outside of the image read as
/// `u16::MAX`. The run form visits the same positions in the same order and pushes a
/// segment exactly where they differ: at `i = 0` and `i = w` the outside is compared with
/// the row's first and last label; for `0 < i < w` the labels differ exactly at run starts,
/// runs being maximal; and every horizontal crack of an inner node row lies in exactly one
/// overlap, whose two labels are the two pixels'. Each segment is built from the same
/// `node_id` calls and labels, so the vector is element for element the same.
/// `dual_segments_scan`, kept in the test module `cracks_tests`, is the scan, and the tests compare the
/// two on random and degenerate maps. An empty image (`w = 0` or `h = 0`) has no crack.
///
/// # Cost
///
/// `O(w · h)` label reads to code the runs, most of them 16 at a time, then `O(runs +
/// segments)`, where the scan made `2 · w · h` bounds-checked comparisons. At 2048 px the
/// segments are 0.6% of the pixels and the runs 0.24% (median, opaque `big` set); the scan
/// took 9.9 ms there (mean), of `build_map`'s 15.9 ms, in Fast and Quality alike.
///
/// Method from: He, Chao & Suzuki 2008, "A Run-Based Two-Scan Labeling Algorithm", IEEE
/// TIP 17(5) 749–756, <https://doi.org/10.1109/TIP.2008.919369>, and He, Chao, Suzuki & Wu
/// 2009, "Fast connected-component labeling", Pattern Recognition 42(9) 1977–1987,
/// <https://doi.org/10.1016/j.patcog.2008.10.013>: rows as runs, each row's runs merged
/// with the row above's. Adapted: the merge reports the cracks between runs of different
/// labels instead of connecting runs of the same one.
///
/// See also: Kovalevsky 1989, "Finite topology as applied to image analysis", Computer
/// Vision, Graphics, and Image Processing 46(2) 141–161,
/// <https://doi.org/10.1016/0734-189X(89)90165-5>, for cracks as the 1-cells between
/// pixels; and Damiand, Bertrand & Fiorio 2004, "Topological model for two-dimensional
/// image representation: definition and optimal extraction algorithm", Computer Vision and
/// Image Understanding 93(2) 111–154, <https://doi.org/10.1016/j.cviu.2003.09.001>, for
/// extracting a planar map of a label image in one scan (read in metadata only; its
/// single-scan claim is not checked here). The walk into edges (in `planar`) stays as it
/// was.
pub(super) fn dual_segments(labels: &[u16], w: usize, h: usize) -> Vec<Seg> {
    let runs = RowRuns::new(labels, w, h);
    let mut segs: Vec<Seg> = Vec::new();

    // Vertical dual edges: node (i, j) -> (i, j+1) separates pixel (i-1, j) from (i, j).
    // Walking downward (+y), the pixel on the left in screen terms is (i, j).
    for j in 0..h {
        let row = runs.row(j);
        let (Some(first), Some(last)) = (row.first(), row.last()) else {
            continue; // w = 0: no pixel, no crack.
        };
        if first.label != u16::MAX {
            segs.push(Seg {
                a: node_id(0, j, w),
                b: node_id(0, j + 1, w),
                left: first.label,
                right: u16::MAX,
            });
        }
        for pair in row.windows(2) {
            let i = pair[1].x0 as usize;
            segs.push(Seg {
                a: node_id(i, j, w),
                b: node_id(i, j + 1, w),
                left: pair[1].label,
                right: pair[0].label,
            });
        }
        if last.label != u16::MAX {
            segs.push(Seg {
                a: node_id(w, j, w),
                b: node_id(w, j + 1, w),
                left: u16::MAX,
                right: last.label,
            });
        }
    }

    // Horizontal dual edges: node (i, j) -> (i+1, j) separates pixel (i, j-1) from (i, j).
    // Walking rightward (+x), the pixel above is on the left.
    let push_run = |segs: &mut Vec<Seg>, j: usize, x0: u32, x1: u32, up: u16, down: u16| {
        for i in x0 as usize..x1 as usize {
            segs.push(Seg {
                a: node_id(i, j, w),
                b: node_id(i + 1, j, w),
                left: up,
                right: down,
            });
        }
    };
    if h > 0 {
        // Node row 0: the outside above row 0.
        for run in runs.row(0) {
            if run.label != u16::MAX {
                push_run(&mut segs, 0, run.x0, run.x1, u16::MAX, run.label);
            }
        }
        for j in 1..h {
            if runs.same_as_above(j) {
                continue; // equal rows: every overlap has one label on both sides.
            }
            overlaps(runs.row(j - 1), runs.row(j), |lo, hi, up, down| {
                if up != down {
                    push_run(&mut segs, j, lo, hi, up, down);
                }
            });
        }
        // Node row h: row h-1 above the outside.
        for run in runs.row(h - 1) {
            if run.label != u16::MAX {
                push_run(&mut segs, h, run.x0, run.x1, run.label, u16::MAX);
            }
        }
    }
    segs
}
