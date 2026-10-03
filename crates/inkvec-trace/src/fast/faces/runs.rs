//! A label image held as its maximal row runs, with its connected components computed on
//! the runs: the data structure behind the Fast clean-up in [`super`].
//!
//! # Layout
//!
//! A *run* is a maximal horizontal stretch of one label: pixels `x0..x1` (`x1` exclusive)
//! of one row, with another label or the image edge on either side. Row `y`'s runs are
//! `runs[row_start[y]..row_start[y + 1]]`, left to right, and tile `0..w` exactly; the
//! whole image is `runs` in scan order (`row_start` has `h + 1` entries). Maximality makes
//! the run list a *canonical* form of the label image: one image has one run list, so any
//! computation on runs that only depends on the image gives the same answer whether the
//! runs were read off the pixels or rebuilt after an edit.
//!
//! On the images the Fast pipeline sees, runs are few: R/n (runs per pixel) is 0.24% at
//! the median and 2.3% at the 95th percentile of the 2048 px benchmark images, 1.8-4.3% at
//! 512 px and 3.8-10.5% at 128 px (research dumps of 2026-09-30, 254 label images).
//!
//! # Components
//!
//! Two pixels are in one component when a path of 4-neighbours of their label joins them
//! (the connected components of the *flat zones* of the label image, in the words of
//! Salembier & Serra). On runs: two runs of adjacent rows with the same label whose x
//! ranges overlap are joined, and the components are the classes of that relation
//! ([`RunLabels::components`], union-find). Horizontally adjacent runs never share a label
//! (they are maximal), so they are never joined.
//!
//! Component ids are numbered in order of first appearance in scan order, exactly as the
//! per-pixel code and a scan-order flood fill number them.
//!
//! # Edits
//!
//! The passes change labels in two ways, and each rebuilds the run list in one pass over
//! the runs, merging neighbours that end up with one label so the runs stay maximal:
//! a new label per component ([`RunLabels::relabel_components`]), or new labels for a few
//! single pixels ([`RunLabels::apply_pixel_edits`]). The components are then out of date
//! and are recomputed, on the runs, the next time a pass asks for them. The per-pixel
//! label image is read once, when the runs are built, and written once, when
//! [`RunLabels::write_faces`] writes the face ids.
//!
//! # Literature
//!
//! Method from: Lemaitre & Lacassagne (2020), "How to speed Connected Component Labeling up
//! with SIMD RLE algorithms", WPMVP 2020, <https://arxiv.org/abs/2006.09299>. Their Light
//! Speed Labeling family labels the run-length encoding of each row, joins overlapping runs
//! of adjacent rows through an equivalence table, and computes component features (area,
//! bounding box) on the runs; they note that the final pass writing a label into every
//! pixel is "very expensive" and "should be avoided" when only features are needed. Here
//! the features are sizes, interiors and the region adjacency graph, and the one per-pixel
//! write left is the face image the rest of the pipeline reads. Adapted: 16-bit ink labels
//! (not a binary image), and the labelling is repeated after each edit instead of once.
//!
//! Method from: Wu, Otoo & Suzuki (2009), "Optimizing two-pass connected-component
//! labeling algorithms", Pattern Analysis and Applications 12(2):117-135,
//! <https://doi.org/10.1007/s10044-008-0109-y>, for the equivalence table: union-find in
//! a flat array whose root is always the smallest index of its class, so provisional
//! labels resolve to scan order without a sort, and their FLATTEN pass, which numbers the
//! classes in one forward sweep reading each entry's parent (its parent has a smaller
//! index and is already numbered). Path halving replaces their path compression; both
//! keep finds near constant time.
//!
//! See also: He, Chao, Suzuki & Wu (2009), "Fast connected-component labeling", Pattern
//! Recognition 42(9):1977-1987, <https://doi.org/10.1016/j.patcog.2008.10.013>, the
//! two-scan scheme (provisional labels, then resolution) this follows.
//!
//! Rejected, with the numbers that decided it (2048 px benchmark images):
//! - pixel-scan decision-tree labellers (BBDT, Spaghetti; compared in the YACCLAB
//!   benchmark, <https://github.com/prittt/YACCLAB>): they speed up the union-find part,
//!   which was 1.05 ms of the 65.6 ms stage, not the per-pixel passes around it;
//! - dynamic connectivity (updating components under edits instead of relabelling): the
//!   union-find on runs is already 0.08-0.65 ms per call;
//! - parallel stripes merged at their seams (OpenCV `connectedComponents`, `mergeLabels`):
//!   at most about 1 ms left to win once the work is on runs.

/// One maximal run: pixels `x0..x1` (`x1` exclusive) of one row, all of label `label`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Run {
    /// First pixel of the run.
    pub x0: u32,
    /// One past the last pixel of the run.
    pub x1: u32,
    /// The label of every pixel in it.
    pub label: u16,
}

/// A `w × h` label image as its maximal row runs, with the 4-connected components of equal
/// labels computed on the runs. See the module documentation for the layout.
pub(crate) struct RunLabels {
    /// Image width in pixels.
    pub(super) w: usize,
    /// Image height in pixels.
    pub(super) h: usize,
    /// The maximal runs of every row, in scan order.
    pub(super) runs: Vec<Run>,
    /// `runs[row_start[y]..row_start[y + 1]]` are row `y`'s runs; `h + 1` entries.
    pub(super) row_start: Vec<usize>,
    /// Component id of each run. Valid only while `fresh`.
    pub(super) run_comp: Vec<u32>,
    /// Pixels per component. Valid only while `fresh`.
    pub(super) size: Vec<usize>,
    /// Label of each component. Valid only while `fresh`.
    pub(super) label: Vec<u16>,
    /// Whether `run_comp`, `size` and `label` describe the current runs.
    fresh: bool,
    /// Union-find parent of each run (scratch, reused).
    parent: Vec<u32>,
    /// The next run list while an edit rebuilds it (scratch, swapped with `runs`).
    spare: Vec<Run>,
    /// The next `row_start` while an edit rebuilds it (scratch).
    spare_rows: Vec<usize>,
}

/// Union-find root of run `x`, with path halving (each visited node is pointed at its
/// grandparent), so repeated finds stay near constant time without recursion. Roots are
/// the smallest index of their class, so `parent[x] <= x` for every `x`.
fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let p = parent[x as usize];
        parent[x as usize] = parent[p as usize];
        x = p;
    }
    x
}

/// How many labels [`push_row_runs`] compares per step while it scans a run: one 128-bit
/// vector of `u16`s.
const SCAN_LANES: usize = 8;

/// Append the maximal runs of one row of labels to `out`, left to right.
///
/// A run is scanned `SCAN_LANES` labels at a time while all of them equal the run's label,
/// then one at a time to its exact end. The block test folds with `&` over a fixed-size
/// block, with no early exit inside it, so the compiler turns it into one vector compare
/// (SSE2 on x86-64, simd128 in the WebAssembly build); on long runs, which is most of every
/// row (runs are 0.24% of the pixels at the median at 2048 px), that is about eight times
/// fewer steps than a label-by-label scan. The runs found are the same either way: a block
/// is skipped only when all its labels are the run's.
///
/// Inspired by: Lemaitre & Lacassagne (2020), <https://arxiv.org/abs/2006.09299>, whose
/// SIMD run-length encoders find the ends of runs with vector compares. Theirs work on
/// binary images with explicit intrinsics; here the compare is on 16-bit labels and left to
/// the auto-vectoriser, so the code stays portable to every target the engine builds for.
fn push_row_runs(row: &[u16], out: &mut Vec<Run>) {
    let w = row.len();
    let mut x = 0;
    while x < w {
        let l = row[x];
        let x0 = x;
        x += 1;
        while x + SCAN_LANES <= w
            && row[x..x + SCAN_LANES]
                .iter()
                .fold(true, |all, &v| all & (v == l))
        {
            x += SCAN_LANES;
        }
        while x < w && row[x] == l {
            x += 1;
        }
        out.push(Run {
            x0: x0 as u32,
            x1: x as u32,
            label: l,
        });
    }
}

/// Append `run` to the run list being built, joining it to the last run when that run is
/// in the same row (`out.len() > row0`, `row0` being where the row began) and has the same
/// label. Pieces are pushed left to right and tile the row, so the last run ends where
/// `run` begins, and joining equal neighbours as they arrive leaves every run maximal.
#[inline]
fn push_merged(out: &mut Vec<Run>, row0: usize, run: Run) {
    if out.len() > row0 {
        if let Some(last) = out.last_mut() {
            if last.label == run.label {
                last.x1 = run.x1;
                return;
            }
        }
    }
    out.push(run);
}

impl RunLabels {
    /// The runs of `labels`, a `w × h` label image in row-major order (`labels.len()`
    /// must be `w · h`). One read of the labels, 2 bytes per pixel: about 1 ms at 2048 px,
    /// most of the stage's remaining cost together with [`RunLabels::write_faces`]. The
    /// components are computed on first use. A zero width gives `h` empty rows, a zero
    /// height no rows; both have no runs and no components.
    ///
    /// Rows are walked as rows, so no pixel index is ever split into `(p % w, p / w)`: the
    /// per-pixel code paid a division per pixel for that, and `%` in a hot loop is also
    /// what wazero's arm64 compiler miscompiled in the Go binding (see
    /// `boundary_opt/band.rs`).
    pub(crate) fn new(labels: &[u16], w: usize, h: usize) -> Self {
        let mut runs = Vec::new();
        let mut row_start = Vec::with_capacity(h + 1);
        for y in 0..h {
            row_start.push(runs.len());
            push_row_runs(&labels[y * w..(y + 1) * w], &mut runs);
        }
        row_start.push(runs.len());
        Self {
            w,
            h,
            runs,
            row_start,
            run_comp: Vec::new(),
            size: Vec::new(),
            label: Vec::new(),
            fresh: false,
            parent: Vec::new(),
            spare: Vec::new(),
            spare_rows: Vec::new(),
        }
    }

    /// Bring `run_comp`, `size` and `label` up to date with the runs, if an edit has made
    /// them stale.
    ///
    /// Connected-component labelling by union-find over the runs: each run is united with
    /// every run of the row above that overlaps it in x (`a.x0 < b.x1 && b.x0 < a.x1`) and
    /// has the same label. The two rows are walked with two pointers (advance whichever
    /// current run ends first; both rows tile `0..w`, so every overlapping pair is met
    /// once), so the pass is linear in the number of runs R. The smaller run index is kept
    /// as the root, so every root is the first run of its class in scan order; ids are
    /// then given to roots as they are met in run order, which is the order of first
    /// appearance in scan order.
    ///
    /// **Same ids as the per-pixel code:** that code ran this very union-find on the runs
    /// it read off the pixels. The runs here are the maximal runs of the same image (the
    /// edits keep them maximal), and maximal runs are unique, so the input and hence every
    /// id, size and label are the same. The numbering pass differs -- the per-pixel code
    /// ran a find per run and looked the root up in a table, this one reads the parent's
    /// id (Wu, Otoo & Suzuki's FLATTEN) -- but both give each class, in order of its
    /// smallest run index, the next id.
    ///
    /// Cost O(R α(R)) for the unions plus O(R) for the numbering; 0.08-0.65 ms at 2048 px
    /// on the benchmark images before the numbering lost its finds. An empty image gives no
    /// components.
    pub(super) fn components(&mut self) {
        if self.fresh {
            return;
        }
        let (runs, row_start, parent) = (&self.runs, &self.row_start, &mut self.parent);
        parent.clear();
        parent.extend(0..runs.len() as u32);
        for y in 1..self.h {
            let (mut i, mut j) = (row_start[y - 1], row_start[y]);
            let (ie, je) = (row_start[y], row_start[y + 1]);
            while i < ie && j < je {
                let (a, b) = (runs[i], runs[j]);
                if a.x0 < b.x1 && b.x0 < a.x1 && a.label == b.label {
                    let (ra, rb) = (find(parent, i as u32), find(parent, j as u32));
                    if ra != rb {
                        // The earlier run is the root, so ids follow scan order.
                        parent[ra.max(rb) as usize] = ra.min(rb);
                    }
                }
                if a.x1 <= b.x1 {
                    i += 1;
                } else {
                    j += 1;
                }
            }
        }
        // Flatten: one forward pass numbers the classes, with no find. `parent[r] <= r`
        // always holds (a union points the larger root at the smaller, and path halving
        // only moves a parent pointer to an ancestor, which has a smaller index), and a
        // root is the smallest index of its class. So walking the runs in order, a root is
        // the first run of its class met and takes the next id, and any other run's parent
        // is an earlier run of the same class whose id is already known.
        self.size.clear();
        self.label.clear();
        self.run_comp.clear();
        for r in 0..runs.len() {
            let p = parent[r] as usize;
            let id = if p == r {
                let id = self.size.len() as u32;
                self.size.push(0);
                self.label.push(runs[r].label);
                id
            } else {
                self.run_comp[p]
            };
            self.run_comp.push(id);
            self.size[id as usize] += (runs[r].x1 - runs[r].x0) as usize;
        }
        self.fresh = true;
    }

    /// Which components have an interior pixel: one off the image border whose four
    /// neighbours are all in the same component. Needs fresh components.
    ///
    /// **Reduction to runs.** A 4-neighbour is in a pixel's component exactly when it has
    /// the pixel's label (same-label neighbours are connected; a component holds one
    /// label), so pixel `(x, y)` of label `l` is interior iff `1 <= y <= h - 2` and its
    /// left, right, upper and lower neighbours all have label `l`. In a maximal run
    /// `x0..x1` of row `y` the left and right neighbours match exactly for `x` in
    /// `x0 + 1 .. x1 - 1` (the pixels at `x0 - 1` and `x1` carry other labels or are off the
    /// image), which also keeps `x` off the left and right borders. So the run holds an
    /// interior pixel iff some `x` in `lo..hi = x0 + 1 .. x1 - 1` lies under a run of label
    /// `l` in row `y - 1` and over one in row `y + 1`: the runs of those two rows are walked
    /// together over `lo..hi` (both rows tile `0..w`, so advancing whichever current run
    /// ends first visits every overlapping pair once), looking for such a pair whose common
    /// part meets `lo..hi`. Runs shorter than 3 have no candidate `x`; a component already
    /// known to be interior is skipped. The pointers into rows `y - 1` and `y + 1` only
    /// move forward across row `y`, so the pass is linear in the number of runs. Images
    /// less than three pixels high or wide have no interior at all.
    ///
    /// Measured on the 254 research dumps: 19.0 ms for the two per-pixel calls at 2048 px,
    /// at most 0.26 ms from runs, with equal results on every dump.
    ///
    /// Not from the literature: interior-ness decided on runs, because the run-based
    /// labelling papers compute areas and bounding boxes on runs but no neighbourhood
    /// predicate such as this one. See also: Lemaitre & Lacassagne (2020),
    /// <https://arxiv.org/abs/2006.09299>, for features accumulated per run.
    pub(super) fn interiors(&self) -> Vec<bool> {
        debug_assert!(self.fresh);
        let (runs, rs) = (&self.runs, &self.row_start);
        let mut interior = vec![false; self.size.len()];
        for y in 1..self.h.saturating_sub(1) {
            let (a1, b1) = (rs[y], rs[y + 2]);
            let (mut ia, mut ib) = (rs[y - 1], rs[y + 1]);
            for r in rs[y]..rs[y + 1] {
                let Run { x0, x1, label } = runs[r];
                let c = self.run_comp[r] as usize;
                if x1 - x0 < 3 || interior[c] {
                    continue;
                }
                let (lo, hi) = (x0 + 1, x1 - 1);
                // Skip the runs above and below that end at or before `lo`.
                while ia < a1 && runs[ia].x1 <= lo {
                    ia += 1;
                }
                while ib < b1 && runs[ib].x1 <= lo {
                    ib += 1;
                }
                let (mut i, mut j) = (ia, ib);
                while i < a1 && j < b1 && runs[i].x0 < hi && runs[j].x0 < hi {
                    let (above, below) = (runs[i], runs[j]);
                    let s = lo.max(above.x0).max(below.x0);
                    let e = hi.min(above.x1).min(below.x1);
                    if above.label == label && below.label == label && s < e {
                        interior[c] = true;
                        break;
                    }
                    if above.x1 <= below.x1 {
                        i += 1;
                    } else {
                        j += 1;
                    }
                }
            }
        }
        interior
    }

    /// Call `f(i, j, len)` once for every pair of distinct runs `i < j` that touch across
    /// pixel edges, with `len` the number of such edges: consecutive runs of a row touch
    /// across one edge, and runs of adjacent rows across the length of their x overlap.
    ///
    /// This is the region adjacency graph at run level. Summing `len` over the runs of two
    /// components gives the length of their common border in pixel edges -- exactly the
    /// count a per-pixel pass over every right and lower neighbour makes -- because every
    /// pixel edge between two different runs lies between two consecutive runs of a row or
    /// under the overlap of two runs of adjacent rows, and is counted once there. Pairs
    /// within one component (overlapping runs of one label) are reported too; callers
    /// that want borders compare components.
    ///
    /// Cost O(R). Measured (research dumps, as a border table): equal to the per-pixel
    /// count on all 254 dumps, 4.6-5.4 ms per pixel pass against 0.09-1.6 ms from runs.
    ///
    /// Not from the literature: border lengths of the adjacency graph read off runs,
    /// because the region-merging literature builds the graph from pixels. See also:
    /// Salembier & Serra (1995), "Flat zones filtering, connected operators, and filters by
    /// reconstruction", IEEE Transactions on Image Processing 4(8):1153-1160,
    /// <https://doi.org/10.1109/83.403422>, and Najman & Cousty (2014), "A graph-based
    /// mathematical morphology reader", <https://arxiv.org/abs/1404.7748>, for merging
    /// on the graph of flat zones.
    pub(super) fn for_each_contact(&self, mut f: impl FnMut(usize, usize, u32)) {
        let (runs, rs) = (&self.runs, &self.row_start);
        for y in 0..self.h {
            // Consecutive runs of a row: one edge between the last pixel of one and the
            // first of the next.
            for r in rs[y] + 1..rs[y + 1] {
                f(r - 1, r, 1);
            }
            if y + 1 < self.h {
                let (mut i, mut j) = (rs[y], rs[y + 1]);
                let (ie, je) = (rs[y + 1], rs[y + 2]);
                while i < ie && j < je {
                    let (a, b) = (runs[i], runs[j]);
                    let (s, e) = (a.x0.max(b.x0), a.x1.min(b.x1));
                    if s < e {
                        f(i, j, e - s);
                    }
                    if a.x1 <= b.x1 {
                        i += 1;
                    } else {
                        j += 1;
                    }
                }
            }
        }
    }

    /// Give every run the label `to[c]` of its component `c` (needs fresh components;
    /// `to` has one entry per component), rebuilding the run list with equal neighbours
    /// merged. One pass over the runs, O(R).
    ///
    /// **Invariant kept:** the rebuilt runs are the maximal runs of the relabelled image.
    /// Each row's pieces are pushed left to right and tile the row, and
    /// [`push_merged`] joins a piece to the previous one whenever their labels agree, so no
    /// two consecutive runs of a row end up with one label. The components are marked
    /// stale; the next [`RunLabels::components`] recomputes them from the new runs.
    pub(super) fn relabel_components(&mut self, to: &[u16]) {
        debug_assert!(self.fresh);
        self.spare.clear();
        self.spare_rows.clear();
        for y in 0..self.h {
            let row0 = self.spare.len();
            self.spare_rows.push(row0);
            for r in self.row_start[y]..self.row_start[y + 1] {
                let run = self.runs[r];
                let label = to[self.run_comp[r] as usize];
                push_merged(&mut self.spare, row0, Run { label, ..run });
            }
        }
        self.finish_rebuild();
    }

    /// Set single pixels to new labels: `edits` holds `(run, x, label)` for pixel `x` of
    /// run `run`, sorted by run then `x`, at most one edit per pixel. The run list is
    /// rebuilt in one pass over the runs, each edited run cut into its unchanged stretches
    /// and its edited pixels, with equal neighbours merged (the invariant of
    /// [`RunLabels::relabel_components`] holds the same way). An edit may give a pixel its
    /// own label; it then simply merges back. An empty list changes nothing and keeps the
    /// components fresh. O(R + number of edits).
    pub(super) fn apply_pixel_edits(&mut self, edits: &[(u32, u32, u16)]) {
        if edits.is_empty() {
            return;
        }
        self.spare.clear();
        self.spare_rows.clear();
        let mut e = 0;
        for y in 0..self.h {
            let row0 = self.spare.len();
            self.spare_rows.push(row0);
            for r in self.row_start[y]..self.row_start[y + 1] {
                let Run { x0, x1, label } = self.runs[r];
                let mut x = x0;
                while e < edits.len() && edits[e].0 as usize == r {
                    let (_, ex, el) = edits[e];
                    if x < ex {
                        push_merged(
                            &mut self.spare,
                            row0,
                            Run {
                                x0: x,
                                x1: ex,
                                label,
                            },
                        );
                    }
                    let one = Run {
                        x0: ex,
                        x1: ex + 1,
                        label: el,
                    };
                    push_merged(&mut self.spare, row0, one);
                    x = ex + 1;
                    e += 1;
                }
                if x < x1 {
                    push_merged(&mut self.spare, row0, Run { x0: x, x1, label });
                }
            }
        }
        self.finish_rebuild();
    }

    /// Close the last row of the rebuilt run list (`spare_rows` gets its `h + 1`-th
    /// entry), swap it in for `runs` -- the old buffers become the next scratch, so no
    /// allocation after the first rebuild -- and mark the components stale.
    fn finish_rebuild(&mut self) {
        self.spare_rows.push(self.spare.len());
        std::mem::swap(&mut self.runs, &mut self.spare);
        std::mem::swap(&mut self.row_start, &mut self.spare_rows);
        self.fresh = false;
    }

    /// Write every pixel's face id -- its component id -- into `out` (`w × h`, row-major),
    /// and return each face's label, indexed by face id.
    ///
    /// This is the one per-pixel write of the clean-up: each run is one `fill` of its
    /// component id. It gives the same ids as the per-pixel `u32` component image the code
    /// used to fill and then narrow, since every pixel of a run gets its run's id either
    /// way. Every pixel is written (the runs tile each row), so `out` needs no clearing and
    /// may still hold the palette's labels. An empty image writes nothing and has no faces.
    ///
    /// # More components than face ids
    ///
    /// Ids are `u16` and Fast keeps them below `u16::MAX - 1`. Past that this used to fold
    /// every further component into face 0 (as `regions::split_components` did), which
    /// left face 0 holding pixels of many inks under one colour. Now the label image is
    /// written out, its smallest components are merged into their neighbours until the rest
    /// fit (`regions::cap_components`, the region-merging rule and its proof are there), and
    /// the runs are read again before the ids are written. An image with fewer components
    /// takes exactly the old path.
    pub(crate) fn write_faces(&mut self, out: &mut [u16]) -> Vec<usize> {
        self.components();
        let cap = (u16::MAX - 1) as usize;
        if self.size.len() > cap {
            self.write_labels(out);
            let absorbed = crate::regions::cap_components(out, self.w, self.h, cap);
            crate::diag!(
                "split",
                "fast components over the face-id limit: {absorbed} smallest merged into neighbours to fit {cap}"
            );
            *self = RunLabels::new(out, self.w, self.h);
            self.components();
        }
        let w = self.w;
        for y in 0..self.h {
            for r in self.row_start[y]..self.row_start[y + 1] {
                let Run { x0, x1, .. } = self.runs[r];
                let id = self.run_comp[r] as usize;
                let face = if id < cap { id as u16 } else { 0 };
                out[y * w + x0 as usize..y * w + x1 as usize].fill(face);
            }
        }
        self.label.iter().take(cap).map(|&l| l as usize).collect()
    }

    /// The label image the runs describe, one label per pixel (tests only).
    #[cfg(test)]
    pub(super) fn to_labels(&self) -> Vec<u16> {
        let mut out = vec![0u16; self.w * self.h];
        self.write_labels(&mut out);
        out
    }

    /// Write the label image the runs describe into `out` (`w × h`, row-major): each run is
    /// one `fill` of its label, and the runs tile every row, so every pixel is written.
    fn write_labels(&self, out: &mut [u16]) {
        for y in 0..self.h {
            for r in self.row_start[y]..self.row_start[y + 1] {
                let Run { x0, x1, label } = self.runs[r];
                out[y * self.w + x0 as usize..y * self.w + x1 as usize].fill(label);
            }
        }
    }

    /// The layout invariants (tests only): `h + 1` row starts, each row's runs tiling
    /// `0..w` left to right, and no two consecutive runs of a row with one label.
    #[cfg(test)]
    pub(super) fn assert_canonical(&self) {
        assert_eq!(self.row_start.len(), self.h + 1);
        assert_eq!(*self.row_start.last().unwrap(), self.runs.len());
        for y in 0..self.h {
            let row = &self.runs[self.row_start[y]..self.row_start[y + 1]];
            let mut x = 0;
            for (k, r) in row.iter().enumerate() {
                assert_eq!(r.x0, x, "row {y}: runs tile the row");
                assert!(r.x1 > r.x0, "row {y}: no empty run");
                if k > 0 {
                    assert_ne!(r.label, row[k - 1].label, "row {y}: runs are maximal");
                }
                x = r.x1;
            }
            assert_eq!(x as usize, self.w, "row {y}: runs cover the row");
        }
    }
}
