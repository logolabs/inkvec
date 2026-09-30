//! Speckle removal and faces for fast mode, in two linear passes over row runs.
//!
//! `regions::despeckle` and `regions::split_components` flood-fill pixel by pixel and keep
//! every component's pixel list; at 2048 px that is most of a second of allocation. Here a
//! component is found by union-find over horizontal runs of one label -- a run is joined to
//! the runs above it that overlap it and carry the same label -- so the work is one pass
//! over the pixels plus one over the runs. Components smaller than `min_size` take the
//! label they share the longest border with (the smallest label on a tie), and the faces
//! are the components of the result.
//!
//! The file also holds the label clean-ups that run between the palette and despeckling
//! (stage 2 of the Fast pipeline, called from [`super::front`] in this order):
//! [`absorb_slivers`], [`absorb_rims`] and [`merge_same_inks`], then [`despeckle`] and
//! [`faces`]. In: one ink label per pixel, each pixel's colour (sRGB 0..1 plus opacity) and
//! each ink's colour. Out: cleaned labels, then a face id per pixel and each face's ink.
//! The blend test [`blend_of`] / [`is_blend`] is shared with [`super::palette`].

/// One maximal run of a label image: pixels `x0..x1` (`x1` exclusive) of one row, all of
/// label `label`, with another label or the image edge on either side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    x0: u32,
    x1: u32,
    label: u16,
}

/// Components of equal labels (4-connected), in order of first appearance in scan order:
/// each component's size and label, and on request ([`Components::fill_pixels`]) the
/// component of every pixel.
///
/// One value is made per trace and handed from pass to pass, so its buffers -- above all
/// the 4 bytes per pixel of `comp`, 16 MB at 2048 px -- are allocated and faulted in once
/// instead of once per pass (the shipped code allocated a fresh `comp` in each of its four
/// to five calls, 34,350 page faults per 2048 px image in all).
pub(crate) struct Components {
    /// Component id per pixel; valid only after [`Components::fill_pixels`].
    pub comp: Vec<u32>,
    /// Pixels per component.
    pub size: Vec<usize>,
    /// Label of each component.
    pub label: Vec<u16>,
    /// The maximal runs of every row, in scan order.
    runs: Vec<Run>,
    /// `runs[row_start[y]..row_start[y + 1]]` are row `y`'s runs; `h + 1` entries.
    row_start: Vec<usize>,
    /// Component id of each run.
    run_comp: Vec<u32>,
    /// Union-find parent of each run (scratch).
    parent: Vec<u32>,
    /// Component id of each root run, `u32::MAX` for a run that is no root (scratch).
    root_id: Vec<u32>,
}

/// Union-find root of run `x`, with path halving (each visited node is pointed at its
/// grandparent), so repeated finds stay near constant time without recursion.
fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let p = parent[x as usize];
        parent[x as usize] = parent[p as usize];
        x = p;
    }
    x
}

/// The 4-connected components of equal labels in a `w × h` label image, with the
/// component of every pixel filled in. Allocates; the passes reuse one [`Components`], so
/// only the tests call this.
#[cfg(test)]
pub(crate) fn components(labels: &[u16], w: usize, h: usize) -> Components {
    let mut c = Components::new();
    c.analyse(labels, w, h);
    c.fill_pixels(w, h);
    c
}

impl Components {
    /// Empty buffers, to be filled by [`Components::analyse`].
    pub(crate) fn new() -> Self {
        Self {
            comp: Vec::new(),
            size: Vec::new(),
            label: Vec::new(),
            runs: Vec::new(),
            row_start: Vec::new(),
            run_comp: Vec::new(),
            parent: Vec::new(),
            root_id: Vec::new(),
        }
    }

    /// The components of `labels` (`w × h`, row-major), in the buffers of `self`; the
    /// per-pixel `comp` is left alone (see [`Components::fill_pixels`]).
    ///
    /// Connected-component labelling by union-find over row runs: each row is cut into
    /// maximal runs of one label, and a run is united with every run of the row above that
    /// overlaps it in x (`a.x0 < b.x1 && b.x0 < a.x1`) and has the same label; the two rows
    /// are walked with two pointers, so the pass is linear in the number of runs. The
    /// smaller run index is kept as the root, so every root is the first run of its
    /// component in scan order, and component ids -- given to roots as they are met in run
    /// order -- come out in order of first appearance in scan order, the numbering a pixel
    /// flood fill in scan order gives (the tests check the partition against
    /// `regions::split_components`). An empty image gives no components.
    ///
    /// Cost: one read of the labels (2 bytes per pixel) plus O(R α(R)) on the R runs; R is
    /// 0.24% of the pixels at the median of the 2048 px benchmark images and 3.8-10.5% at
    /// 128 px.
    ///
    /// Method from: Wu, Otoo & Suzuki (2009), "Optimizing two-pass connected-component
    /// labeling algorithms", Pattern Analysis and Applications 12(2):117-135,
    /// <https://doi.org/10.1007/s10044-008-0109-y> (union-find with the smaller index as
    /// root, so provisional labels resolve to scan order), applied to runs rather than
    /// pixels as in He, Chao, Suzuki & Wu (2009), "Fast connected-component labeling",
    /// Pattern Recognition 42(9):1977-1987, <https://doi.org/10.1016/j.patcog.2008.10.013>.
    pub(crate) fn analyse(&mut self, labels: &[u16], w: usize, h: usize) {
        self.runs.clear();
        self.row_start.clear();
        for y in 0..h {
            self.row_start.push(self.runs.len());
            let row = &labels[y * w..(y + 1) * w];
            let mut x = 0;
            while x < w {
                let l = row[x];
                let x0 = x;
                while x < w && row[x] == l {
                    x += 1;
                }
                self.runs.push(Run {
                    x0: x0 as u32,
                    x1: x as u32,
                    label: l,
                });
            }
        }
        self.row_start.push(self.runs.len());
        self.union_and_number(h);
    }

    /// Union-find over `self.runs` (rows `0..h`), then a component id, size and label per
    /// root, and a component id per run. See [`Components::analyse`].
    fn union_and_number(&mut self, h: usize) {
        let (runs, row_start, parent) = (&self.runs, &self.row_start, &mut self.parent);
        parent.clear();
        parent.extend(0..runs.len() as u32);
        for y in 1..h {
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
        self.root_id.clear();
        self.root_id.resize(runs.len(), u32::MAX);
        self.size.clear();
        self.label.clear();
        self.run_comp.clear();
        for r in 0..runs.len() {
            let root = find(parent, r as u32) as usize;
            if self.root_id[root] == u32::MAX {
                self.root_id[root] = self.size.len() as u32;
                self.size.push(0);
                self.label.push(runs[root].label);
            }
            let id = self.root_id[root];
            self.run_comp.push(id);
            self.size[id as usize] += (runs[r].x1 - runs[r].x0) as usize;
        }
    }

    /// Write every pixel's component id into `comp` from the runs of the last
    /// [`Components::analyse`]. The buffer is sized once and every pixel is overwritten
    /// (the runs of a row tile it), so no clearing is needed between passes.
    pub(crate) fn fill_pixels(&mut self, w: usize, h: usize) {
        self.comp.resize(w * h, 0);
        for y in 0..h {
            for r in self.row_start[y]..self.row_start[y + 1] {
                let Run { x0, x1, .. } = self.runs[r];
                self.comp[y * w + x0 as usize..y * w + x1 as usize].fill(self.run_comp[r]);
            }
        }
    }

    /// Which components have an interior pixel: one off the image border whose four
    /// neighbours are all in the same component. Read from the runs of the last
    /// [`Components::analyse`]; `h` is the image height.
    ///
    /// **Reduction to runs.** A 4-neighbour is in a pixel's component exactly when it has
    /// the pixel's label (same-label neighbours are connected; a component holds one
    /// label), so pixel `(x, y)` of label `l` is interior iff `1 <= y <= h - 2` and its left,
    /// right, upper and lower neighbours all have label `l`. In a maximal run `x0..x1` of
    /// row `y` the left and right neighbours match exactly for `x` in `x0 + 1 .. x1 - 1`
    /// (the pixels at `x0 - 1` and `x1` carry other labels or are off the image), which also
    /// keeps `x` off the left and right borders. So the run holds an interior pixel iff some
    /// `x` in `lo..hi = x0 + 1 .. x1 - 1` lies under a run of label `l` in row `y - 1` and
    /// over one in row `y + 1`: the runs of those two rows are walked together over
    /// `lo..hi` (both rows tile `0..w`, so advancing whichever current run ends first visits
    /// every overlapping pair once), looking for such a pair whose common part meets
    /// `lo..hi`. Runs shorter than 3 have no candidate `x`; a component already known to
    /// be interior is skipped. The pointers into rows `y - 1` and `y + 1` only move forward
    /// across row `y`, so the pass is linear in the number of runs.
    ///
    /// Measured on the 254 research dumps: 19.0 ms for the two per-pixel calls at 2048 px,
    /// at most 0.26 ms from runs, with equal results on every dump.
    ///
    /// Not from the literature: interior-ness decided on runs, because the run-based
    /// labelling papers (He et al. 2009; Lemaitre & Lacassagne 2020) compute areas and
    /// bounding boxes on runs but no neighbourhood predicate such as this one.
    /// See also: Lemaitre & Lacassagne (2020), "How to speed Connected Component Labeling
    /// up with SIMD RLE algorithms", <https://arxiv.org/abs/2006.09299>, for features
    /// accumulated per run instead of per pixel.
    fn interiors(&self, h: usize) -> Vec<bool> {
        let (runs, rs) = (&self.runs, &self.row_start);
        let mut interior = vec![false; self.size.len()];
        for y in 1..h.saturating_sub(1) {
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
}

/// Relabel every component smaller than `min_size` pixels with the neighbouring label it
/// shares the longest border with.
///
/// Border length is counted in pixel edges: every 4-neighbour pair across the component's
/// boundary is one vote for the label on the far side. The label with the most votes wins,
/// the smallest label on a tie. Votes read the labels as they were before this pass, so
/// the result does not depend on the order small components are visited; two adjacent
/// speckles may therefore swap into each other's label rather than merge, which is
/// harmless at this size. A component with no neighbour at all (the whole image) keeps
/// its label. `min_size <= 1` does nothing. `c` is the reused component workspace.
pub(crate) fn despeckle(
    labels: &mut [u16],
    w: usize,
    h: usize,
    min_size: usize,
    c: &mut Components,
) {
    if min_size <= 1 {
        return;
    }
    c.analyse(labels, w, h);
    if c.size.iter().all(|&s| s >= min_size) {
        return;
    }
    c.fill_pixels(w, h);
    let mut tally: std::collections::HashMap<u32, Vec<(u16, u32)>> = Default::default();
    for p in 0..w * h {
        let id = c.comp[p];
        if c.size[id as usize] >= min_size {
            continue;
        }
        let (x, y) = (p % w, p / w);
        let t = tally.entry(id).or_default();
        let mut vote = |q: usize| {
            if c.comp[q] != id {
                let l = labels[q];
                match t.iter_mut().find(|e| e.0 == l) {
                    Some(e) => e.1 += 1,
                    None => t.push((l, 1)),
                }
            }
        };
        if x > 0 {
            vote(p - 1);
        }
        if x + 1 < w {
            vote(p + 1);
        }
        if y > 0 {
            vote(p - w);
        }
        if y + 1 < h {
            vote(p + w);
        }
    }
    // New label per component; `u16::MAX` keeps its own.
    let mut to = vec![u16::MAX; c.size.len()];
    for (id, t) in tally {
        if let Some((l, _)) = t
            .into_iter()
            .max_by_key(|&(l, n)| (n, std::cmp::Reverse(l)))
        {
            to[id as usize] = l;
        }
    }
    for (l, &id) in labels.iter_mut().zip(&c.comp) {
        let t = to[id as usize];
        if t != u16::MAX {
            *l = t;
        }
    }
}

/// Each pixel's colour as the clean-up passes read it: sRGB 0..1 over white, plus the source
/// opacity when transparency is traced natively (1 otherwise).
///
/// Read on demand rather than copied. The passes look at a colour only for pixels of
/// strips (components without an interior): at most 2.7% of the pixels of a 2048 px image
/// and 0% at the median (the 7 `big` images of the Fast benchmark). The copy of every
/// pixel as `[f32; 4]` it replaces was 64 MB and 18.3 ms at 2048 px, a quarter of the
/// whole clean-up stage.
///
/// **Why the output is identical:** [`Pixels::get`] builds the same four `f32`s the copy
/// held -- the three stored channels unchanged and `alpha[p]` or the literal `1.0` -- so
/// every blend test downstream sees the same bits.
///
/// Inspired by: Ragan-Kelley, Barnes, Adams, Paris, Durand & Amarasinghe (2013), "Halide:
/// a language and compiler for optimizing parallelism, locality, and recomputation in image
/// processing pipelines", PLDI 2013, <https://doi.org/10.1145/2491956.2462176>. Halide
/// schedules each stage between storing its result and recomputing it at the consumer;
/// here the choice is made by hand at its trivial end: a value that is one load (and one
/// branch) to rebuild, and is read at a few percent of the pixels, is never stored.
#[derive(Clone, Copy)]
pub(crate) struct Pixels<'a> {
    /// sRGB 0..1 per pixel, composited over white.
    pub rgb: &'a [[f32; 3]],
    /// Source opacity per pixel in native mode; `None` reads as 1.
    pub alpha: Option<&'a [f32]>,
}

impl Pixels<'_> {
    /// Pixel `p`'s colour and opacity.
    #[inline]
    pub(crate) fn get(&self, p: usize) -> [f32; 4] {
        let c = self.rgb[p];
        [c[0], c[1], c[2], self.alpha.map_or(1.0, |a| a[p])]
    }
}

/// Largest distance (sRGB and opacity, Euclidean) from a pixel to the line between two
/// neighbouring inks for the pixel to count as a blend of them.
pub(crate) const BLEND_TOL: f32 = 0.04;

/// `col` read as a blend of inks `a` and `b` (sRGB and opacity): how much of `b` it holds,
/// clamped to [0, 1], and its squared distance from the line between them. `None` when the
/// two inks are one colour.
///
/// The orthogonal projection of `col` onto the segment from `a` to `b` in the 4-D space of
/// straight (not linearised) sRGB 0..1 and opacity:
///
/// `t = clamp(((col − a) · (b − a)) / |b − a|², 0, 1)`, `d = |col − (a + t (b − a))|²`.
///
/// `t` is the coverage of `b` an anti-aliased pixel would have under a linear mix; `d` is
/// in squared sRGB units and is compared with `BLEND_TOL²`. Clamping `t` makes a colour
/// beyond either end measure its distance to that end, so a darker-than-black pixel is not
/// read as a blend. `|b − a|² < 1e-9` (the same colour twice) has no line, hence `None`.
pub(crate) fn blend_of(col: [f32; 4], a: [f32; 4], b: [f32; 4]) -> Option<(f32, f32)> {
    let ab: [f32; 4] = std::array::from_fn(|k| b[k] - a[k]);
    let l2: f32 = ab.iter().map(|v| v * v).sum();
    if l2 < 1e-9 {
        return None;
    }
    let t = ((0..4).map(|k| (col[k] - a[k]) * ab[k]).sum::<f32>() / l2).clamp(0.0, 1.0);
    let d = (0..4)
        .map(|k| (col[k] - (a[k] + t * ab[k])).powi(2))
        .sum::<f32>();
    Some((t, d))
}

/// Whether `col` is a blend of inks `a` and `b`: within [`BLEND_TOL`] of the line between
/// them.
pub(crate) fn is_blend(col: [f32; 4], a: [f32; 4], b: [f32; 4]) -> bool {
    blend_of(col, a, b).is_some_and(|(_, d)| d <= BLEND_TOL * BLEND_TOL)
}

/// Put anti-aliasing slivers back where they belong.
///
/// The palette sends a blend to a neighbour's ink only when the blend's colour is not an
/// ink itself; a rim between white and a gradient crosses the gradient's own light bands,
/// and comes out as one-pixel strips of those inks, each a face with a boundary. A
/// component with no interior pixel (none whose four neighbours share its label) whose
/// pixels are each a blend of two inks that meet across it -- or simply one of them -- is
/// such a strip: every pixel takes the nearer of those inks. A hairline is not a blend of
/// what lies either side of it, and stays.
///
/// Per pixel of a component without interior, the candidate inks are the labels of its
/// 4-neighbours that belong to components *with* an interior. Among "the pixel is ink `a`"
/// (squared distance to `a`) and "the pixel is a blend of `a` and `b`" ([`blend_of`],
/// assigned to `a` when `t < 0.5` and to `b` otherwise), the smallest squared distance
/// within `BLEND_TOL²` wins; with none, the pixel keeps its label. Neighbour labels are
/// read from a copy taken before the pass, so the order pixels are visited in does not
/// matter. `px` and `inks` are sRGB 0..1 plus opacity; `labels` must index into `inks`.
/// `c` is the reused component workspace.
pub(crate) fn absorb_slivers(
    labels: &mut [u16],
    px: Pixels<'_>,
    inks: &[[f32; 4]],
    w: usize,
    h: usize,
    c: &mut Components,
) {
    c.analyse(labels, w, h);
    c.fill_pixels(w, h);
    let interior = c.interiors(h);
    let d2 = |a: [f32; 4], b: [f32; 4]| (0..4).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>();
    let src = labels.to_vec();
    for p in 0..w * h {
        let id = c.comp[p] as usize;
        if interior[id] {
            continue;
        }
        let (x, y) = (p % w, p / w);
        let mut around: Vec<u16> = Vec::with_capacity(4);
        for q in [
            (x > 0).then(|| p - 1),
            (x + 1 < w).then(|| p + 1),
            (y > 0).then(|| p - w),
            (y + 1 < h).then(|| p + w),
        ]
        .into_iter()
        .flatten()
        {
            if interior[c.comp[q] as usize] && src[q] != src[p] && !around.contains(&src[q]) {
                around.push(src[q]);
            }
        }
        let col = px.get(p);
        let mut best: Option<(u16, f32)> = None;
        let mut consider = |l: u16, d: f32| {
            if d <= BLEND_TOL * BLEND_TOL && best.is_none_or(|(_, e)| d < e) {
                best = Some((l, d));
            }
        };
        for (i, &a) in around.iter().enumerate() {
            let ia = inks[a as usize];
            consider(a, d2(col, ia));
            for &b in &around[i + 1..] {
                let Some((t, d)) = blend_of(col, ia, inks[b as usize]) else {
                    continue;
                };
                // The blend goes to the ink it is mostly made of.
                consider(if t < 0.5 { a } else { b }, d);
            }
        }
        if let Some((l, _)) = best {
            labels[p] = l;
        }
    }
}

/// Put the rims [`absorb_slivers`] cannot reach back where they belong.
///
/// A component with no interior pixel is a strip at most two pixels wide. When its ink is a
/// blend of two of its neighbours' inks, it is the anti-aliased rim between them, and each
/// of its pixels goes to the side it covers more of. [`absorb_slivers`] only reads
/// neighbours with an interior of their own, so the grey rim between small text and the
/// paper, both strips, stayed a face of its own and cut every glyph's outline into pieces
/// at the junctions it made. A hairline still stays: its ink is no blend of what lies either
/// side of it.
///
/// Unlike [`absorb_slivers`], the test is made once per strip on its *ink*: of all pairs
/// of labels bordering the strip, the pair `(a, b)` whose line the strip's ink lies nearest
/// ([`blend_of`] distance within `BLEND_TOL²`; ties broken by the smaller pair) is the rim's
/// two sides. Each pixel of the strip then goes to `b` when its own colour's projection on
/// that line has `t >= 0.5`, and to `a` otherwise (also when `a` and `b` are one colour).
/// A strip bordering fewer than two inks, or labelled outside the palette, is left alone.
/// `c` is the reused component workspace.
pub(crate) fn absorb_rims(
    labels: &mut [u16],
    px: Pixels<'_>,
    inks: &[[f32; 4]],
    w: usize,
    h: usize,
    c: &mut Components,
) {
    c.analyse(labels, w, h);
    c.fill_pixels(w, h);
    let interior = c.interiors(h);
    let n_inks = inks.len();
    // The labels each strip borders.
    let mut around: Vec<Vec<u16>> = vec![Vec::new(); c.size.len()];
    for p in 0..w * h {
        let id = c.comp[p];
        if interior[id as usize] {
            continue;
        }
        let (x, y) = (p % w, p / w);
        for q in [
            (x > 0).then(|| p - 1),
            (x + 1 < w).then(|| p + 1),
            (y > 0).then(|| p - w),
            (y + 1 < h).then(|| p + w),
        ]
        .into_iter()
        .flatten()
        {
            let l = labels[q];
            let t = &mut around[id as usize];
            if c.comp[q] != id && (l as usize) < n_inks && !t.contains(&l) {
                t.push(l);
            }
        }
    }
    // Per strip, the two inks it is a rim between: the pair its own ink lies nearest the
    // line between, within the blend tolerance.
    let pair: Vec<Option<(u16, u16)>> = around
        .iter()
        .enumerate()
        .map(|(id, t)| {
            let own = c.label[id] as usize;
            if own >= n_inks || t.len() < 2 {
                return None;
            }
            let mut best: Option<(u16, u16, f32)> = None;
            for (i, &a) in t.iter().enumerate() {
                for &b in &t[i + 1..] {
                    let (lo, hi) = (a.min(b), a.max(b));
                    if let Some((_, d)) = blend_of(inks[own], inks[lo as usize], inks[hi as usize])
                    {
                        let better =
                            best.is_none_or(|(l0, h0, e)| d < e || (d == e && (lo, hi) < (l0, h0)));
                        if d <= BLEND_TOL * BLEND_TOL && better {
                            best = Some((lo, hi, d));
                        }
                    }
                }
            }
            best.map(|(a, b, _)| (a, b))
        })
        .collect();
    for (p, l) in labels.iter_mut().enumerate() {
        if let Some((a, b)) = pair[c.comp[p] as usize] {
            *l = match blend_of(px.get(p), inks[a as usize], inks[b as usize]) {
                Some((t, _)) if t >= 0.5 => b,
                _ => a,
            };
        }
    }
}

/// Join each component to a larger neighbour whose ink it cannot be told apart from.
///
/// The fast palette keeps two inks closer than [`crate::color::SAME_INK_DE00`] apart when
/// they are far enough apart in OKLab, which is widest near black: its thin-text ink beside
/// the flat black of large type, 1.3 dE00 from it, which
/// tiled every glyph of a masthead's body text into patches of the two, each patch an
/// outline and each corner of one a junction. Components are taken largest first; each
/// joins the neighbour it shares most border with among the larger ones whose (settled)
/// ink is within the threshold of its own, so no pixel's colour moves further than that
/// however the joins chain -- a ramp of close bands is thinned, not flattened, and the
/// ramp pass after this still sees its bands.
///
/// Two inks are "the same" when their opacities differ by less than [`SAME_ALPHA`] and
/// their CIEDE2000 difference (on sRGB, ignoring opacity) is below `SAME_INK_DE00`.
/// Components are ranked by size (larger first, lower id on a tie); a component may only
/// take the settled ink of a higher-ranked neighbour, choosing the longest shared border in
/// pixel edges (the higher rank on a tie). Returns early, unchanged, when no two inks are
/// the same or no such pair of components touches. `c` is the reused component workspace.
pub(crate) fn merge_same_inks(
    labels: &mut [u16],
    inks: &[[f32; 4]],
    w: usize,
    h: usize,
    c: &mut Components,
) {
    let n_inks = inks.len();
    let same: Vec<bool> = (0..n_inks * n_inks)
        .map(|k| {
            let (ia, ib) = (inks[k / n_inks], inks[k % n_inks]);
            k / n_inks != k % n_inks
                && (ia[3] - ib[3]).abs() < SAME_ALPHA
                && crate::color::de00([ia[0], ia[1], ia[2]], [ib[0], ib[1], ib[2]])
                    < crate::color::SAME_INK_DE00
        })
        .collect();
    if !same.contains(&true) {
        return;
    }
    c.analyse(labels, w, h);
    c.fill_pixels(w, h);
    let n = c.size.len();
    // Border shared by each pair of components whose inks are one ink to the eye.
    let mut border: std::collections::HashMap<(u32, u32), u32> = Default::default();
    let mut touch = |a: u32, b: u32| {
        let (la, lb) = (c.label[a as usize] as usize, c.label[b as usize] as usize);
        if a != b && la < n_inks && lb < n_inks && same[la * n_inks + lb] {
            *border.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    };
    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            if x + 1 < w {
                touch(c.comp[p], c.comp[p + 1]);
            }
            if y + 1 < h {
                touch(c.comp[p], c.comp[p + w]);
            }
        }
    }
    if border.is_empty() {
        return;
    }
    let mut nbrs: Vec<Vec<(u32, u32)>> = vec![Vec::new(); n];
    let mut pairs: Vec<((u32, u32), u32)> = border.into_iter().collect();
    pairs.sort_unstable();
    for ((a, b), len) in pairs {
        nbrs[a as usize].push((b, len));
        nbrs[b as usize].push((a, len));
    }
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_unstable_by_key(|&k| (std::cmp::Reverse(c.size[k as usize]), k));
    let mut rank = vec![0u32; n];
    for (r, &k) in order.iter().enumerate() {
        rank[k as usize] = r as u32;
    }
    let mut to: Vec<u16> = c.label.clone();
    for &k in &order {
        let own = c.label[k as usize] as usize;
        let best = nbrs[k as usize]
            .iter()
            .filter(|&&(o, _)| rank[o as usize] < rank[k as usize])
            .filter(|&&(o, _)| {
                let t = to[o as usize] as usize;
                t != own && t < n_inks && same[own * n_inks + t]
            })
            .max_by_key(|&&(o, len)| (len, std::cmp::Reverse(rank[o as usize])));
        if let Some(&(o, _)) = best {
            to[k as usize] = to[o as usize];
        }
    }
    for (l, &id) in labels.iter_mut().zip(&c.comp) {
        *l = to[id as usize];
    }
}

/// Largest opacity difference between two inks that can be one ink.
const SAME_ALPHA: f32 = 0.02;

/// Faces: the components of `labels`, written over `labels` as a face id per pixel, and
/// each face's label (the returned vector, indexed by face id, holds the ink index of that
/// face). Past `u16::MAX - 1` faces the rest are folded into face 0, as
/// `regions::split_components` does. Face ids are component ids ([`Components::analyse`]),
/// so they follow scan order. `c` is the reused component workspace.
///
/// The ids go straight from the runs into the label buffer, which the runs were read out
/// of first: no per-pixel `u32` component image is written and then narrowed to `u16` in
/// a second buffer, as before. The values are the same -- `min(id, cap)` folding and all --
/// because every pixel of a run gets its run's component id either way.
pub(crate) fn faces(labels: &mut [u16], w: usize, h: usize, c: &mut Components) -> Vec<usize> {
    c.analyse(labels, w, h);
    let cap = (u16::MAX - 1) as usize;
    for y in 0..h {
        for r in c.row_start[y]..c.row_start[y + 1] {
            let Run { x0, x1, .. } = c.runs[r];
            let id = c.run_comp[r] as usize;
            let face = if id < cap { id as u16 } else { 0 };
            labels[y * w + x0 as usize..y * w + x1 as usize].fill(face);
        }
    }
    c.label.iter().take(cap).map(|&l| l as usize).collect()
}

#[cfg(test)]
mod reference_tests;
#[cfg(test)]
mod tests;
