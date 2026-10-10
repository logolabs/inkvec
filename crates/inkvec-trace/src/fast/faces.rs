//! Stage 2 of the Fast pipeline: the label clean-up and the faces, on row runs.
//!
//! # The problem
//!
//! The palette (stage 1, [`super::palette`]) gives every pixel an ink label. Before the
//! planar map is built, the label image is cleaned, and then cut into faces. In the field's
//! terms this is *connected-component analysis of the flat zones* of the label image (a
//! flat zone is a maximal 4-connected set of pixels of one label), followed by *region
//! merging on the region adjacency graph*: each pass decides, from statistics of the
//! components and of the borders between them, which components take another label.
//! The passes, in the order [`super::front`] calls them, with the stopwatch stage each is
//! timed under:
//!
//! 1. [`RunLabels::absorb_slivers`] (`slivers`): pixels of *strips* -- components with no
//!    interior pixel -- that are blends of the inks of the thick components around them go
//!    to the nearer of those inks. Two-endmember linear unmixing, pixel by pixel.
//! 2. [`RunLabels::absorb_rims`] (`slivers`): strips whose own ink is a blend of two inks
//!    they border are anti-aliased rims; each of their pixels goes to the side it covers
//!    more of.
//! 3. [`RunLabels::absorb_halos`] (`slivers`), on a lossy intake only: thin components
//!    whose colour lies between two inks around them in JPEG's colour space are the
//!    codec's halos, and each of their pixels goes to the ink it covers more of
//!    ([`halos`]).
//! 4. [`RunLabels::merge_same_inks`] (`slivers`): a component takes the ink of a larger
//!    neighbour whose ink the eye cannot tell from its own (region merging on the graph);
//!    on a lossy intake a thin one takes one up to the soft floor.
//! 5. [`RunLabels::despeckle`] (`despeckle`): components under the speckle floor take the
//!    label they share the longest border with (an area filter on flat zones, a
//!    connected operator).
//! 6. [`RunLabels::write_faces`] (`split`): the components of the result are the faces;
//!    their ids are written over the label buffer.
//!
//! In: one ink label per pixel, each pixel's colour ([`Pixels`], sRGB 0..1 plus opacity)
//! and each ink's colour. Out: a face id per pixel (in the label buffer) and each face's
//! ink. The blend test [`blend_of`] / [`is_blend`] is shared with [`super::palette`].
//!
//! # Data layout: runs
//!
//! Every pass works on the image's maximal row runs ([`RunLabels`], see [`runs`]): per
//! row, the maximal stretches `x0..x1` of one label, in scan order. The component analysis
//! is union-find over runs; sizes, interiors and border lengths are read off the runs;
//! and a pass that changes labels rebuilds the run list rather than the pixels. The pixel
//! labels are read once (when the runs are built) and a pixel is written once (its face
//! id). Colours are read only at strip pixels.
//!
//! What that saves, measured on the research dumps of 2026-09-30 (the 7 `big` images at
//! 2048 px): the per-pixel stage made 18 passes over the image (22 with the same-ink
//! merge), about 118 bytes of traffic per pixel, and took 65.6 ms, 39% of the whole trace;
//! of that the union-find itself was 1.05 ms. Yet all the passes together change at most
//! 3.6% of the pixels (none at the median), strip pixels are at most 2.7%, and runs are
//! 0.24% of the pixels at the median. The necessary traffic -- read the labels, write the
//! faces -- is about 16 MB, roughly 1 ms on one thread. On runs the whole stage takes
//! 2.1-7.4 ms on the 2048 px dumps (reading the runs and writing the faces about 1 ms
//! each) and 11-13 ms on the 1672 x 941 masthead, whose text makes 42k strip pixels,
//! against 86-137 ms and 46-75 ms for the per-pixel code in the same runs (best of 5,
//! `faces/tests.rs`'s timing test, on a loaded machine).
//!
//! # Why the output is the same as the per-pixel code's
//!
//! 1. *Canonical runs.* Maximal runs are a unique encoding of a label image, and every
//!    edit ([`RunLabels::relabel_components`], [`RunLabels::apply_pixel_edits`]) keeps the
//!    runs maximal. So after each pass the runs are exactly the runs the per-pixel code
//!    would read off its relabelled image, and the union-find it ran on those runs gives
//!    the same component ids, sizes and labels here.
//! 2. *Labels decide adjacency.* A 4-neighbour is in a pixel's component iff it has the
//!    pixel's label. So a pixel's neighbours inside its own run are always in its
//!    component, the neighbours that can lie outside are at the run's two ends and in the
//!    runs of the rows above and below that overlap it, and every pixel edge between two
//!    components lies between two consecutive runs of a row or under the overlap of two
//!    runs of adjacent rows ([`RunLabels::for_each_contact`]). Counts summed over those
//!    are the per-pixel counts.
//! 3. *Order-free decisions.* Each pass reads the labels as they were before it, as the
//!    per-pixel code did (from a copy, or by deciding everything before writing), and its
//!    choices are maxima or minima under total orders with explicit tie-breaks, so the
//!    order in which runs are visited does not matter. Where the per-pixel code's order
//!    did matter (the candidate list of [`RunLabels::absorb_slivers`]), it is reproduced.
//! 4. *Same arithmetic.* The floating-point expressions ([`blend_of`], squared distances)
//!    are the same functions on the same four floats, in the same order.
//!
//! The per-pixel code is kept as a test oracle (`faces/reference_tests.rs`), and
//! `faces/tests.rs` checks every pass against it on random and degenerate images and on
//! the 254 research label dumps.
//!
//! # Literature
//!
//! - Method from: Lemaitre & Lacassagne (2020) and Wu, Otoo & Suzuki (2009) for the
//!   labelling on runs; see [`runs`].
//! - Method from: Salembier & Serra (1995), "Flat zones filtering, connected operators, and
//!   filters by reconstruction", IEEE Transactions on Image Processing 4(8):1153-1160,
//!   <https://doi.org/10.1109/83.403422>, for the frame: [`RunLabels::despeckle`] and
//!   [`RunLabels::merge_same_inks`] are connected operators -- they act on whole flat
//!   zones and never split one, so no new contour appears.
//! - Not from the literature: the interior test and the border lengths on runs, and the
//!   observation that the component table plus the adjacency graph with border lengths are
//!   sufficient statistics for every pass here (pixel colours are needed only at strip
//!   pixels), because the labelling papers stop at component features. See also: Najman &
//!   Cousty (2014), "A graph-based mathematical morphology reader",
//!   <https://arxiv.org/abs/1404.7748>, for morphology on the graph of flat zones.
//! - Inspired by: Selinger (2003), "Potrace: a polygon-based tracing algorithm",
//!   <https://potrace.sourceforge.net/potrace.pdf>, whose `turdsize` drops paths under an
//!   area, and VTracer's `filter_speckle` (<https://github.com/visioncortex/vtracer>),
//!   whose default of 4 (a 4 x 4 patch) sets the largest speckle floor in
//!   `front::speckle_floor`. Both delete a small region; [`RunLabels::despeckle`] gives it
//!   to the neighbour it shares most border with, so no hole is left in the planar map.
//! - See also: Felzenszwalb & Huttenlocher (2004), "Efficient graph-based image
//!   segmentation", IJCV 59(2):167-181,
//!   <https://doi.org/10.1023/B:VISI.0000022288.19776.77>: region merging with union-find
//!   under a pairwise predicate, as [`RunLabels::merge_same_inks`] merges under "one ink to
//!   the eye"; theirs merges by edge weight order, this one by component size.
//! - Rejected: caching the sliver decision per (colour, candidate inks) key -- measured
//!   about one distinct key per strip pixel, so nothing to reuse; and evaluating slivers
//!   per stretch of equal candidates -- 82% of the masthead's strip pixels have
//!   candidates, and their scattered colour reads, not the candidate search, are the cost.
//!   Threads (rayon over pixel passes, or over image stripes as OpenCV's
//!   `connectedComponents` does) are not used: once on runs the stage is a few
//!   milliseconds, about half of it the one read of the labels and the one write of the
//!   faces, which are bound by memory bandwidth rather than by one core.

mod halos;
mod runs;

use runs::Run;
pub(crate) use runs::RunLabels;

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
///
/// See also: Bioucas-Dias, Plaza, Dobigeon, Parente, Du, Gader & Chanussot (2012),
/// "Hyperspectral unmixing overview: geometrical, statistical, and sparse regression-based
/// approaches", <https://arxiv.org/abs/1202.6294>: this is the linear mixing model with two
/// endmembers and the abundances constrained to sum to one and be non-negative, solved in
/// closed form.
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

/// Largest opacity difference between two inks that can be one ink.
const SAME_ALPHA: f32 = 0.02;

/// The ink a sliver pixel of colour `col` should take, given the distinct labels `around`
/// of its 4-neighbours that lie in thick components (with an interior) and differ from its
/// own, in the order left, right, up, down of first appearance; `None` keeps its label.
///
/// Candidates are "the pixel is ink `a`", scored by the squared distance
/// `|col − ink_a|² = Σₖ (colₖ − aₖ)²` over sRGB and opacity, and for every pair `a` before
/// `b` in `around`, "the pixel is a blend of `a` and `b`", scored by [`blend_of`]'s `d` and
/// given to `a` when `t < 0.5` (mostly `a`) and to `b` otherwise. The lowest score within
/// `BLEND_TOL²` wins; on an exact tie the candidate met first wins (`d < e` is strict),
/// which is why `around` keeps the per-pixel code's order. At most 4 labels, so at most 4
/// distances and 6 blends.
fn sliver_ink(col: [f32; 4], around: &[u16], inks: &[[f32; 4]]) -> Option<u16> {
    let d2 = |a: [f32; 4], b: [f32; 4]| (0..4).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>();
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
    best.map(|(l, _)| l)
}

/// The pair of inks a strip of ink `own` is the anti-aliased rim between, from the sorted,
/// distinct in-palette labels `sides` of the components it borders; `None` when the strip
/// is no rim.
///
/// The pair `(lo, hi)`, `lo < hi`, minimising `d` of [`blend_of`]`(ink_own, ink_lo,
/// ink_hi)` within `BLEND_TOL²`, the smaller pair on an exact tie of `d`: the minimum of
/// `(d, lo, hi)` in lexicographic order. A minimum under a total order does not depend on
/// the order the pairs are tried in (a `NaN` distance is never within tolerance, so it
/// never competes), which is why `sides` may come sorted here where the per-pixel code
/// met them in scan order. `own` outside the palette, or fewer than two sides, is no rim.
/// At most `k (k − 1) / 2` blend tests for `k` sides.
fn rim_pair(own: usize, sides: &[u16], inks: &[[f32; 4]]) -> Option<(u16, u16)> {
    if own >= inks.len() || sides.len() < 2 {
        return None;
    }
    let mut best: Option<(u16, u16, f32)> = None;
    for (i, &lo) in sides.iter().enumerate() {
        for &hi in &sides[i + 1..] {
            if let Some((_, d)) = blend_of(inks[own], inks[lo as usize], inks[hi as usize]) {
                let better =
                    best.is_none_or(|(l0, h0, e)| d < e || (d == e && (lo, hi) < (l0, h0)));
                if d <= BLEND_TOL * BLEND_TOL && better {
                    best = Some((lo, hi, d));
                }
            }
        }
    }
    best.map(|(a, b, _)| (a, b))
}

/// Move the distinct values of the sorted slice `v` to its front, in order, and return how
/// many there are (`Vec::dedup` for a slice). Empty gives 0.
fn dedup_sorted(v: &mut [u16]) -> usize {
    let mut k = 0;
    for i in 0..v.len() {
        if k == 0 || v[i] != v[k - 1] {
            v[k] = v[i];
            k += 1;
        }
    }
    k
}

/// `same[a · n + b]`: whether inks `a` and `b` (of `n`) are one ink to the eye -- distinct
/// indices, opacities within [`SAME_ALPHA`], and CIEDE2000 (on sRGB, ignoring opacity)
/// below [`crate::color::SAME_INK_DE00`]. `n²` colour differences; the palette has tens
/// of inks. Computed for both orders, as CIEDE2000 is evaluated with `a` first.
fn same_ink_table(inks: &[[f32; 4]], de00: f32) -> Vec<bool> {
    let n_inks = inks.len();
    (0..n_inks * n_inks)
        .map(|k| {
            let (ia, ib) = (inks[k / n_inks], inks[k % n_inks]);
            k / n_inks != k % n_inks
                && (ia[3] - ib[3]).abs() < SAME_ALPHA
                && crate::color::de00([ia[0], ia[1], ia[2]], [ib[0], ib[1], ib[2]]) < de00
        })
        .collect()
}

impl RunLabels {
    /// Put anti-aliasing slivers back where they belong.
    ///
    /// The palette sends a blend to a neighbour's ink only when the blend's colour is not
    /// an ink itself; a rim between white and a gradient crosses the gradient's own light
    /// bands, and comes out as one-pixel strips of those inks, each a face with a boundary.
    /// A component with no interior pixel (none whose four neighbours share its label)
    /// whose pixels are each a blend of two inks that meet across it -- or simply one of
    /// them -- is such a strip: every pixel takes the nearer of those inks
    /// ([`sliver_ink`]). A hairline is not a blend of what lies either side of it, and
    /// stays.
    ///
    /// Per pixel of a component without interior, the candidate inks are the labels of
    /// its 4-neighbours that belong to components *with* an interior and differ from its
    /// own. Labels are read as they were before the pass: every new label is decided
    /// before any is written ([`RunLabels::apply_pixel_edits`]). `inks` are sRGB 0..1 plus
    /// opacity, and every label must index into them.
    ///
    /// **On runs.** For pixel `x` of a strip run `x0..x1` in row `y`, the left neighbour is
    /// in the same run (same label, so no candidate) unless `x = x0`, when it is the
    /// previous run of the row; likewise the right neighbour is the next run when
    /// `x = x1 − 1`; the upper and lower neighbours are the runs of rows `y − 1` and
    /// `y + 1` containing `x`, found by cursors that only move right across the row. The
    /// candidates are gathered in the per-pixel code's order (left, right, up, down), so
    /// ties resolve as before. Work: O(R) for the cursors plus O(1) per strip pixel; only
    /// strip pixels whose candidate list is not empty read a colour.
    ///
    /// Inspired by: the linear mixing model of spectral unmixing with two endmembers (see
    /// [`blend_of`]), restricted to thin strips where a pixel can only be a mix of what
    /// lies on either side of it. Not from the literature: the strip criterion (no
    /// interior pixel), because unmixing papers unmix every pixel against a global
    /// endmember set.
    pub(crate) fn absorb_slivers(&mut self, px: Pixels<'_>, inks: &[[f32; 4]]) {
        self.components();
        let interior = self.interiors();
        let edits = self.sliver_edits(px, inks, &interior);
        self.apply_pixel_edits(&edits);
    }

    /// The pixel edits of [`RunLabels::absorb_slivers`], as `(run, x, new label)` in scan
    /// order, the order [`RunLabels::apply_pixel_edits`] needs. Needs fresh components;
    /// `interior` is [`RunLabels::interiors`].
    ///
    /// Edge cases: on the first row there is no upper neighbour and on the last no lower
    /// one (their cursors are never read there); a run starting at `x = 0` has no left
    /// neighbour and one ending at `w` no right one, so in a one-column image only the
    /// rows above and below offer candidates. A strip pixel without any candidate reads no
    /// colour. Measured (masthead, 1672 x 941): 41,884 strip pixels, 34,189 with
    /// candidates, 5,853 edits, 1.8 ms -- mostly the colour reads, scattered over a 19 MB
    /// image, so grouping pixels with equal candidates would not pay.
    fn sliver_edits(
        &self,
        px: Pixels<'_>,
        inks: &[[f32; 4]],
        interior: &[bool],
    ) -> Vec<(u32, u32, u16)> {
        let (w, h, runs, rs) = (self.w, self.h, &self.runs, &self.row_start);
        let thick = |r: usize| interior[self.run_comp[r] as usize];
        let mut edits = Vec::new();
        for y in 0..h {
            // Cursors on the run above and below the current pixel. Unused (and never
            // read) on the first and last row.
            let mut up = if y > 0 { rs[y - 1] } else { 0 };
            let mut down = if y + 1 < h { rs[y + 1] } else { 0 };
            for r in rs[y]..rs[y + 1] {
                let Run { x0, x1, label } = runs[r];
                if thick(r) {
                    continue;
                }
                for x in x0..x1 {
                    // The distinct thick-neighbour labels, left, right, up, down.
                    let mut around = [0u16; 4];
                    let mut n = 0;
                    let mut offer = |q: usize| {
                        let l = runs[q].label;
                        if thick(q) && l != label && !around[..n].contains(&l) {
                            around[n] = l;
                            n += 1;
                        }
                    };
                    if x == x0 && x0 > 0 {
                        offer(r - 1);
                    }
                    if x + 1 == x1 && (x1 as usize) < w {
                        offer(r + 1);
                    }
                    if y > 0 {
                        // Row `y - 1` tiles `0..w` and `x < w`, so this stops in the row.
                        while runs[up].x1 <= x {
                            up += 1;
                        }
                        offer(up);
                    }
                    if y + 1 < h {
                        while runs[down].x1 <= x {
                            down += 1;
                        }
                        offer(down);
                    }
                    if n == 0 {
                        continue;
                    }
                    let col = px.get(y * w + x as usize);
                    if let Some(l) = sliver_ink(col, &around[..n], inks) {
                        edits.push((r as u32, x, l));
                    }
                }
            }
        }
        edits
    }

    /// Put the rims [`RunLabels::absorb_slivers`] cannot reach back where they belong.
    ///
    /// A component with no interior pixel is a strip at most two pixels wide. When its ink
    /// is a blend of two of its neighbours' inks, it is the anti-aliased rim between them,
    /// and each of its pixels goes to the side it covers more of.
    /// [`RunLabels::absorb_slivers`] only reads neighbours with an interior of their own,
    /// so the grey rim between small text and the paper, both strips, stayed a face of its
    /// own and cut every glyph's outline into pieces at the junctions it made. A hairline
    /// still stays: its ink is no blend of what lies either side of it.
    ///
    /// Unlike [`RunLabels::absorb_slivers`], the test is made once per strip on its *ink*:
    /// of all pairs of in-palette labels bordering the strip, the pair `(a, b)` whose line
    /// the strip's ink lies nearest ([`rim_pair`]) is the rim's two sides. Each pixel of
    /// the strip then goes to `b` when its own colour's projection on that line has
    /// `t >= 0.5`, and to `a` otherwise (also when `a` and `b` are one colour). A strip
    /// bordering fewer than two inks, or labelled outside the palette, is left alone.
    ///
    /// **On runs.** The labels a strip borders are read off the run contacts
    /// ([`RunLabels::for_each_contact`]): the set of labels of other components touching
    /// the strip's runs is the set the per-pixel code gathered from each strip pixel's
    /// 4-neighbours (every such neighbour is in a touching run, and every touching run
    /// holds such a neighbour). Only the set matters, as [`rim_pair`] is order-free. The
    /// sides are grouped by strip with a counting sort on the component id and each
    /// strip's few labels sorted and deduplicated in place ([`dedup_sorted`]). Work:
    /// O(R + E + C) for the E strip contacts and C components, plus one blend test per
    /// pixel of a strip that is a rim.
    pub(crate) fn absorb_rims(&mut self, px: Pixels<'_>, inks: &[[f32; 4]]) {
        self.components();
        let interior = self.interiors();
        let n_inks = inks.len();
        // (strip component, label across the border) for every contact of a strip.
        let mut sides: Vec<(u32, u16)> = Vec::new();
        self.for_each_contact(|i, j, _| {
            let (ci, cj) = (self.run_comp[i], self.run_comp[j]);
            if ci == cj {
                return;
            }
            let (li, lj) = (self.runs[i].label, self.runs[j].label);
            if !interior[ci as usize] && (lj as usize) < n_inks {
                sides.push((ci, lj));
            }
            if !interior[cj as usize] && (li as usize) < n_inks {
                sides.push((cj, li));
            }
        });
        // Group the sides by strip with a counting sort on the component id, O(E + C), then
        // sort and deduplicate each strip's few labels in place. (A comparison sort of all
        // E sides was 1.6 ms of the 4.9 ms pass on the masthead, E = 53,087.)
        let n = self.size.len();
        let mut start = vec![0usize; n + 1];
        for &(c, _) in &sides {
            start[c as usize + 1] += 1;
        }
        for c in 0..n {
            start[c + 1] += start[c];
        }
        let mut next = start.clone();
        let mut by_strip = vec![0u16; sides.len()];
        for &(c, l) in &sides {
            by_strip[next[c as usize]] = l;
            next[c as usize] += 1;
        }
        let mut pair: Vec<Option<(u16, u16)>> = vec![None; n];
        let mut any = false;
        for c in 0..n {
            let group = &mut by_strip[start[c]..start[c + 1]];
            if group.len() < 2 {
                continue;
            }
            group.sort_unstable();
            let distinct = dedup_sorted(group);
            pair[c] = rim_pair(self.label[c] as usize, &group[..distinct], inks);
            any |= pair[c].is_some();
        }
        if !any {
            return;
        }
        let mut edits = Vec::new();
        for y in 0..self.h {
            for r in self.row_start[y]..self.row_start[y + 1] {
                let Some((a, b)) = pair[self.run_comp[r] as usize] else {
                    continue;
                };
                let Run { x0, x1, .. } = self.runs[r];
                for x in x0..x1 {
                    let col = px.get(y * self.w + x as usize);
                    let to = match blend_of(col, inks[a as usize], inks[b as usize]) {
                        Some((t, _)) if t >= 0.5 => b,
                        _ => a,
                    };
                    edits.push((r as u32, x, to));
                }
            }
        }
        self.apply_pixel_edits(&edits);
    }

    /// Join each component to a larger neighbour whose ink it cannot be told apart from.
    ///
    /// The fast palette keeps two inks closer than [`crate::color::SAME_INK_DE00`] apart
    /// when they are far enough apart in OKLab, which is widest near black: its thin-text
    /// ink beside the flat black of large type, 1.3 dE00 from it, which tiled every glyph
    /// of a masthead's body text into patches of the two, each patch an outline and each
    /// corner of one a junction. Components are taken largest first; each joins the
    /// neighbour it shares most border with among the larger ones whose (settled) ink is
    /// within the threshold of its own, so no pixel's colour moves further than that
    /// however the joins chain -- a ramp of close bands is thinned, not flattened, and the
    /// ramp pass after this still sees its bands.
    ///
    /// Two inks are "the same" by [`same_ink_table`]. Components are ranked by size (larger
    /// first, lower id on a tie); a component may only take the settled ink of a
    /// higher-ranked neighbour, choosing the longest shared border in pixel edges (the
    /// higher rank on a tie). Returns early, unchanged, when no two inks are the same or
    /// no such pair of components touches.
    ///
    /// **Thin components on a lossy intake** (`thin_de00`, `None` otherwise). A JPEG splits
    /// one ink into several a few dE00 apart wherever its blocks and its half-resolution
    /// chroma shift the colour (a black outline came back as #010101, #010009 and #080000
    /// on `openmoji/1F9D1`, `web` tier), and the variants tile the ink into fragments, none
    /// of which has an interior. A component with no pixel whose window of radius
    /// [`halos::HALO_REACH`] is all its own ([`RunLabels::deep_components`]) then counts an
    /// ink within `thin_de00` (CIEDE2000; [`crate::color::SOFT_SAME_INK_DE00`], the soft
    /// floor Quality's palette uses on such an intake) as its own; a component with an
    /// interior keeps the clean threshold, so two flat inks a few dE00 apart stay two.
    /// With `None` the decisions are exactly the clean ones.
    ///
    /// **On runs.** The border lengths are summed over run contacts
    /// ([`RunLabels::for_each_contact`], equal to the per-pixel count), oriented as the
    /// per-pixel code met them (left or upper component first, which decides the order
    /// CIEDE2000 is evaluated in), and summed per pair after a sort instead of in a hash
    /// map; the sorted pair list, and everything decided from it, is the same. The result
    /// is one label per component ([`RunLabels::relabel_components`]). Work: O(R + E log E)
    /// for E same-ink contacts, plus a sort of the components by size.
    ///
    /// Method from: Salembier & Serra (1995), <https://doi.org/10.1109/83.403422> -- a
    /// connected operator: flat zones are merged whole, never split.
    pub(crate) fn merge_same_inks(&mut self, inks: &[[f32; 4]], thin_de00: Option<f32>) {
        let n_inks = inks.len();
        let same = same_ink_table(inks, crate::color::SAME_INK_DE00);
        let thin_same = thin_de00.map(|de00| same_ink_table(inks, de00));
        if !same.contains(&true) && !thin_same.as_ref().is_some_and(|t| t.contains(&true)) {
            return;
        }
        self.components();
        let deep = thin_de00.map(|_| self.deep_components(halos::HALO_REACH));
        // Whether component `k`, of ink `a`, may take ink `b`.
        let same_for = |k: usize, a: usize, b: usize| -> bool {
            match (&thin_same, &deep) {
                (Some(t), Some(d)) if !d[k] => t[a * n_inks + b],
                _ => same[a * n_inks + b],
            }
        };
        let n = self.size.len();
        // Border shared by each pair of components whose inks are one ink to the eye.
        let mut touching: Vec<(u32, u32, u32)> = Vec::new();
        self.for_each_contact(|i, j, len| {
            let (a, b) = (self.run_comp[i], self.run_comp[j]);
            let (la, lb) = (
                self.label[a as usize] as usize,
                self.label[b as usize] as usize,
            );
            if a != b
                && la < n_inks
                && lb < n_inks
                && (same[la * n_inks + lb]
                    || (thin_de00.is_some()
                        && (same_for(a as usize, la, lb) || same_for(b as usize, lb, la))))
            {
                touching.push((a.min(b), a.max(b), len));
            }
        });
        if touching.is_empty() {
            return;
        }
        touching.sort_unstable();
        let mut pairs: Vec<((u32, u32), u32)> = Vec::new();
        for (a, b, len) in touching {
            match pairs.last_mut() {
                Some((ab, sum)) if *ab == (a, b) => *sum += len,
                _ => pairs.push(((a, b), len)),
            }
        }
        let mut nbrs: Vec<Vec<(u32, u32)>> = vec![Vec::new(); n];
        for ((a, b), len) in pairs {
            nbrs[a as usize].push((b, len));
            nbrs[b as usize].push((a, len));
        }
        let mut order: Vec<u32> = (0..n as u32).collect();
        order.sort_unstable_by_key(|&k| (std::cmp::Reverse(self.size[k as usize]), k));
        let mut rank = vec![0u32; n];
        for (r, &k) in order.iter().enumerate() {
            rank[k as usize] = r as u32;
        }
        let mut to: Vec<u16> = self.label.clone();
        for &k in &order {
            let own = self.label[k as usize] as usize;
            let best = nbrs[k as usize]
                .iter()
                .filter(|&&(o, _)| rank[o as usize] < rank[k as usize])
                .filter(|&&(o, _)| {
                    let t = to[o as usize] as usize;
                    t != own && t < n_inks && same_for(k as usize, own, t)
                })
                .max_by_key(|&&(o, len)| (len, std::cmp::Reverse(rank[o as usize])));
            if let Some(&(o, _)) = best {
                to[k as usize] = to[o as usize];
            }
        }
        if to != self.label {
            self.relabel_components(&to);
        }
    }

    /// Relabel every component smaller than `min_size` pixels with the neighbouring label
    /// it shares the longest border with.
    ///
    /// Border length is counted in pixel edges: every 4-neighbour pair across the
    /// component's boundary is one vote for the label on the far side. The label with the
    /// most votes wins, the smallest label on a tie. Votes read the labels as they were
    /// before this pass, so the result does not depend on the order small components are
    /// visited; two adjacent speckles may therefore swap into each other's label rather
    /// than merge, which is harmless at this size. A component with no neighbour at all
    /// (the whole image) keeps its label. `min_size <= 1` does nothing.
    ///
    /// **On runs.** A contact of `len` edges between runs of two components is `len` votes
    /// for each side that is small, for the label of the other ([`RunLabels::for_each_contact`];
    /// every pixel edge is in exactly one contact). Votes are summed per (component,
    /// label) after a sort; the winner is the maximum of (votes, reversed label), unique
    /// because each label appears once. Work: O(R + V log V) for the V contacts of small
    /// components; an image with nothing small stops after the component sizes.
    ///
    /// Method from: Salembier & Serra (1995), <https://doi.org/10.1109/83.403422> -- an
    /// area filter on flat zones, a connected operator. Inspired by: Potrace's `turdsize`
    /// and VTracer's `filter_speckle` (see the module documentation), which delete small
    /// regions where this gives them to a neighbour.
    pub(crate) fn despeckle(&mut self, min_size: usize) {
        if min_size <= 1 {
            return;
        }
        self.components();
        if self.size.iter().all(|&s| s >= min_size) {
            return;
        }
        let small = |c: u32| self.size[c as usize] < min_size;
        // (small component, label across the border, pixel edges).
        let mut votes: Vec<(u32, u16, u32)> = Vec::new();
        self.for_each_contact(|i, j, len| {
            let (ci, cj) = (self.run_comp[i], self.run_comp[j]);
            if ci == cj {
                return;
            }
            if small(ci) {
                votes.push((ci, self.runs[j].label, len));
            }
            if small(cj) {
                votes.push((cj, self.runs[i].label, len));
            }
        });
        votes.sort_unstable_by_key(|&(c, l, _)| (c, l));
        let mut to = self.label.clone();
        for group in votes.chunk_by(|a, b| a.0 == b.0) {
            // (label, votes) per label, then the most votes, the smallest label on a tie.
            let best = group
                .chunk_by(|a, b| a.1 == b.1)
                .map(|g| (g[0].1, g.iter().map(|v| v.2).sum::<u32>()))
                .max_by_key(|&(l, n)| (n, std::cmp::Reverse(l)));
            if let Some((l, _)) = best {
                // `u16::MAX` meant "keep" in the per-pixel code; kept for bit-equality.
                if l != u16::MAX {
                    to[group[0].0 as usize] = l;
                }
            }
        }
        if to != self.label {
            self.relabel_components(&to);
        }
    }
}

#[cfg(test)]
mod reference_tests;
#[cfg(test)]
mod tests;
