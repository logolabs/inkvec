//! Row runs of a label image: the interval coding the planar map and the ramp pass read
//! the label map through.
//!
//! # The problem
//!
//! Several stages ask questions about the *boundaries* of a label map — which pixel sides
//! separate two labels ([`super::build`]'s dual segments), and how long two faces' shared
//! border is (`fast::bands`' contacts). Asked pixel by pixel, each question costs a pass over
//! every pixel side, `2 · w · h` comparisons with bounds checks, although boundaries are
//! sparse: at 2048 px the boundary is 0.6% of the pixels and the runs 0.24% (median over
//! the seven opaque 2048 px test images, research 2026-09-30). This module codes each row
//! as its maximal runs of one label once, and the questions are answered on runs.
//!
//! # Data layout
//!
//! A **run** is a maximal horizontal interval of one label in one row: `x0..x1` (`x1`
//! exclusive) with every `labels[y · w + x]` for `x0 ≤ x < x1` equal to `label`, and the
//! pixels just outside it (if inside the row) carrying a different label. A row of width
//! `w ≥ 1` is the concatenation of its runs, so the runs of a row partition `0..w`, in
//! increasing `x`, and two consecutive runs of a row always carry different labels. All
//! runs of the image are stored in one vector, row after row; `start[y]..start[y + 1]` are
//! row `y`'s runs. An empty row (`w = 0`) has no runs.
//!
//! # Passes
//!
//! 1. [`RowRuns::new`]: one pass over the rows. A row equal to the row above (a `memcmp`)
//!    copies that row's runs instead of scanning; otherwise each run's end is found by
//!    comparing 16 labels at a time against the run's label, then one at a time.
//! 2. [`overlaps`]: the runs of two consecutive rows, merged by two pointers into the
//!    maximal `x` intervals over which both rows are constant. Every vertical pixel side
//!    between the two rows lies in exactly one of them, so a question about vertical
//!    neighbours is answered per interval, not per pixel.
//!
//! Method from: He, Chao & Suzuki 2008, "A Run-Based Two-Scan Labeling Algorithm", IEEE
//! Transactions on Image Processing 17(5) 749–756, <https://doi.org/10.1109/TIP.2008.919369>,
//! and He, Chao, Suzuki & Wu 2009, "Fast connected-component labeling", Pattern Recognition
//! 42(9) 1977–1987, <https://doi.org/10.1016/j.patcog.2008.10.013>: rows coded as runs, and
//! rows compared run against run with two pointers. Adapted: they label binary images and
//! connect foreground runs that touch; here the image has many labels, every run is kept
//! (there is no background), and the merge reports each pair of overlapping runs with their
//! overlap instead of uniting them.
//!
//! Inspired by: Ji, Piper & Tang 1989, "Erosion and dilation of binary images by arbitrary
//! structuring elements using interval coding", Pattern Recognition Letters 9(3) 201–209,
//! <https://doi.org/10.1016/0167-8655(89)90055-X>, which computes on interval-coded rows
//! rather than on pixels; the operations here (boundary extraction and contact lengths) are
//! ours, not theirs.

/// One maximal run of a label in a row: pixels `x0..x1` (exclusive) all carry `label`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Run {
    /// First pixel of the run.
    pub(crate) x0: u32,
    /// One past the last pixel of the run.
    pub(crate) x1: u32,
    /// The label every pixel of the run carries.
    pub(crate) label: u16,
}

/// Every row of a `w x h` label image as its runs; see the module docs for the layout.
pub(crate) struct RowRuns {
    /// All runs, row after row, each row's in increasing `x`.
    runs: Vec<Run>,
    /// `start[y]..start[y + 1]` index row `y`'s runs; `h + 1` entries.
    start: Vec<usize>,
    /// Whether row `y`'s labels equal row `y − 1`'s exactly (always `false` for row 0).
    same_as_above: Vec<bool>,
}

impl RowRuns {
    /// Code every row of `labels` (row-major, `w · h`) as its maximal runs.
    ///
    /// `O(w · h)` label reads in the worst case, but with two shortcuts that make the common
    /// case much cheaper: a row equal to the one above is detected with one slice compare
    /// and takes a copy of that row's runs, and inside a row a run is extended 16 labels at
    /// a time (`row[x..x + 16] == [label; 16]`, which compiles to two 128-bit compares on
    /// x86-64) before the last few are compared one by one. Neither shortcut changes the
    /// result: both only establish, faster, that labels equal the run's label.
    ///
    /// Edge cases: `w = 0` or `h = 0` gives rows without runs. Panics if `labels` is shorter
    /// than `w · h` or `w` does not fit in `u32` (the planar map's node ids need
    /// `(w + 1)(h + 1) < 2^32` anyway).
    pub(crate) fn new(labels: &[u16], w: usize, h: usize) -> Self {
        const LANES: usize = 16;
        let mut runs: Vec<Run> = Vec::new();
        let mut start = Vec::with_capacity(h + 1);
        let mut same_as_above = Vec::with_capacity(h);
        for y in 0..h {
            let row = &labels[y * w..(y + 1) * w];
            let same = y > 0 && row == &labels[(y - 1) * w..y * w];
            same_as_above.push(same);
            let begin = runs.len();
            start.push(begin);
            if same {
                // Row y − 1's runs are `start[y − 1]..begin`; copy them.
                runs.extend_from_within(start[y - 1]..begin);
                continue;
            }
            let mut x = 0usize;
            while x < w {
                let label = row[x];
                let x0 = x;
                x += 1;
                let splat = [label; LANES];
                while x + LANES <= w && row[x..x + LANES] == splat {
                    x += LANES;
                }
                while x < w && row[x] == label {
                    x += 1;
                }
                runs.push(Run {
                    x0: x0 as u32,
                    x1: x as u32,
                    label,
                });
            }
        }
        start.push(runs.len());
        Self {
            runs,
            start,
            same_as_above,
        }
    }

    /// Row `y`'s runs, in increasing `x`. Panics if `y ≥ h`.
    pub(crate) fn row(&self, y: usize) -> &[Run] {
        &self.runs[self.start[y]..self.start[y + 1]]
    }

    /// Whether row `y` has exactly the labels of row `y − 1` (`false` for row 0). Two equal
    /// rows have no vertical boundary between them. Panics if `y ≥ h`.
    pub(crate) fn same_as_above(&self, y: usize) -> bool {
        self.same_as_above[y]
    }

    /// Total number of runs over all rows.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.runs.len()
    }
}

/// Call `f(lo, hi, above, below)` for every maximal interval `lo..hi` (exclusive, `lo < hi`)
/// over which row `above_runs` is constant (label `above`) and row `below_runs` is constant
/// (label `below`), in increasing `x`.
///
/// Both rows must be the runs of one image's rows (each partitioning `0..w`). Two pointers
/// walk them: the current pair overlaps on `max(x0) .. min(x1)`, and whichever run ends
/// first is left behind (the upper one on a tie, which leaves an empty overlap next step;
/// empty intervals are not reported). Every `x` in `0..w` falls in exactly one reported
/// interval, so the pixel pairs `(x, y − 1)`, `(x, y)` are covered once each.
/// `O(|above_runs| + |below_runs|)`.
///
/// Method from: He, Chao & Suzuki 2008 (see the module docs), whose second scan compares a
/// row's runs with the row above's in the same two-pointer merge.
pub(crate) fn overlaps(
    above_runs: &[Run],
    below_runs: &[Run],
    mut f: impl FnMut(u32, u32, u16, u16),
) {
    let (mut i, mut j) = (0usize, 0usize);
    while i < above_runs.len() && j < below_runs.len() {
        let (a, b) = (above_runs[i], below_runs[j]);
        let lo = a.x0.max(b.x0);
        let hi = a.x1.min(b.x1);
        if hi > lo {
            f(lo, hi, a.label, b.label);
        }
        if a.x1 <= b.x1 {
            i += 1;
        } else {
            j += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic label maps: random, blocky, one label, a checkerboard.
    fn maps() -> Vec<(Vec<u16>, usize, usize)> {
        let mut s = 0x1234_5678_9abc_def0u64;
        let mut next = move |n: u64| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s % n
        };
        let mut out = Vec::new();
        for (w, h) in [(0, 3), (3, 0), (1, 1), (1, 9), (9, 1), (17, 13), (40, 33)] {
            out.push(((0..w * h).map(|_| next(3) as u16).collect(), w, h));
            out.push((vec![5u16; w * h], w, h));
            out.push((
                (0..w * h)
                    .map(|p| ((p % w.max(1) + p / w.max(1)) % 2) as u16)
                    .collect(),
                w,
                h,
            ));
            let bands: Vec<u16> = (0..w * h)
                .map(|p| ((p / w.max(1)) / 4 + (p % w.max(1)) / 7) as u16)
                .collect();
            out.push((bands, w, h));
        }
        out
    }

    #[test]
    fn runs_reassemble_every_row() {
        for (labels, w, h) in maps() {
            let r = RowRuns::new(&labels, w, h);
            let mut total = 0;
            for y in 0..h {
                let mut x = 0u32;
                let mut prev: Option<u16> = None;
                for run in r.row(y) {
                    assert_eq!(run.x0, x, "runs must tile the row");
                    assert!(run.x1 > run.x0);
                    assert_ne!(Some(run.label), prev, "runs must be maximal");
                    for xx in run.x0..run.x1 {
                        assert_eq!(labels[y * w + xx as usize], run.label);
                    }
                    prev = Some(run.label);
                    x = run.x1;
                }
                assert_eq!(x as usize, w);
                total += r.row(y).len();
                assert_eq!(
                    r.same_as_above(y),
                    y > 0 && labels[y * w..(y + 1) * w] == labels[(y - 1) * w..y * w]
                );
            }
            assert_eq!(total, r.len());
        }
    }

    #[test]
    fn overlaps_cover_each_column_once_with_the_right_labels() {
        for (labels, w, h) in maps() {
            let r = RowRuns::new(&labels, w, h);
            for y in 1..h {
                let mut seen = vec![0u8; w];
                let mut last = 0u32;
                overlaps(r.row(y - 1), r.row(y), |lo, hi, a, b| {
                    assert!(lo >= last && hi > lo);
                    last = hi;
                    for x in lo..hi {
                        seen[x as usize] += 1;
                        assert_eq!(labels[(y - 1) * w + x as usize], a);
                        assert_eq!(labels[y * w + x as usize], b);
                    }
                });
                assert!(seen.iter().all(|&c| c == 1));
            }
        }
    }
}
