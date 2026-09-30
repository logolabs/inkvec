# Stage 14 — Fast mode

> A second route through the pipeline: the same planar map, sub-pixel refinement and
> emitter as Quality mode, with every expensive search replaced by a one-pass version and
> the curve fitter replaced by a Potrace-class one — and, since the 2026-09-30 round, each
> of its own stages computed on row runs, in parallel where it pays, to the same bytes.

**Source:** `crates/inkvec-trace/src/fast/` — `front.rs` (the front end), `palette.rs`,
`faces.rs` and `faces/runs.rs` (the clean-up), `bands.rs` (ramps), `mod.rs`, `polygon.rs`,
`smooth.rs`, `curve.rs` and `prims.rs` (the fitter); `crates/inkvec-cli/src/fast.rs` (the
command line's side). The stages it shares are documented in `01-intake.md`,
`06-planar-map.md`, `07-subpixel.md`, `10-symmetry.md` and `13-emit.md`.
**Entry points:** `--mode fast` (`TraceMode::Fast`, `crates/inkvec-cli/src/args.rs:21-29`,
default `quality`). The trace crate dispatches to `fast::trace_color` or, for an image
traced with its transparency, `fast::trace_color_native` (`fast/front.rs:17-29`) when
`ColorOptions::fast` is set (`crates/inkvec-trace/src/lib.rs:290-302`); the command line
fits the result with `fast::fit` (`crates/inkvec-cli/src/fast.rs`), which calls
`inkvec_trace::fast::fit_edges` (`fast/mod.rs`).
**Pipeline position:** it replaces stages 03–05 (palette, regions, gradients) with its own
front end; shares stages 01, 06, 07, 10 and 13; skips 08 (the boundary solve,
`lib.rs:1137`) and 09 (decode, `lib.rs:1152`); and replaces 11 (curve fitting) with its own fitter,
without 12 (repair) or shape harmonization (`repair_fits` and `emit_options` in
`crates/inkvec-cli/src/pipeline.rs`).

## What problem this solves

Quality mode spends its time searching: a palette recovered by minimum description length
with spatial evidence tests for every candidate, gradient bands merged pairwise with a refit
after every merge, the whole boundary solved at once against an exact render, and every
boundary fitted by a global multi-model dynamic program. Fast mode keeps what makes the
output *correct* — "the planar map whose shared edges are traced once so neighbouring faces
cannot open a seam, the sub-pixel refinement of every boundary point, and the emitter with
its seam underlap, compound paths and minify" — and replaces the rest with one-pass versions
(the module overview of `fast/mod.rs`). Each stage is linear or near-linear in the number of pixels or
boundary points, and nothing reads a clock, so the output is the same on every machine. The
command line describes the trade as "several times faster, a little less faithful"
(`args.rs:304-307`).

The round of 2026-09-30 rewrote Fast's own stages and the stages it shares for speed,
**with the output held fixed**: every rewrite is exact, keeps the code it replaced as a test
oracle, and was byte-compared on the benchmark sets. This page describes the pipeline as it
stands after that round, stage by stage, with what each stage computes in the field's
standard terms, the method, its citations as the code's doc comments label them, and the
measured costs before and after.

**How the numbers were measured.** Stage timings come from the implementing branches' merge
notes and the research reports of 2026-09-30, on the development machine (an 8-core Ryzen 7
5800X, shared with other work, so milliseconds are approximate and counts and ratios are the
reliable part). The sets: *screen*, 246 icons at 128 × 128 (241 of them RGBA, traced with
native alpha); *held_a*, 156 held-out icons; *s512*, 58 images at 512 px; *big*, 7 opaque
images (six at 2048 × 2048 and a 1672 × 941 masthead); *bigalpha*, 3 transparent images at
2048 px; *flat*, one flat logo at 512 px. "Byte-identical" means the SVG files compared equal
byte for byte against the previous build.

## Inputs and outputs

The same as Quality mode's: a decoded `Rgba` (and the source alpha, when transparency is
traced natively) in; a `ColorTrace` — the planar map with sub-pixel boundaries, one fill and
one palette ink per face — out of the trace crate; one fitted path per map edge out of the
fitter; one SVG document out of the emitter. Two differences in what comes out:

* **fills are flat**, except where the ramp pass (stage 4 below) finds that a run of
  posterised bands is one gradient — on opaque images, with gradients on. The native-alpha
  path has no ramps: "a gradient across opacity is a fade, which is quality mode's"
  (`fast/front.rs:14-16`);
* **the report** opens with a line saying Fast mode ran and naming what it skipped, followed
  by every option the caller changed that only steers a skipped stage (`fast_ignored`,
  `crates/inkvec-cli/src/fast.rs`), so a tuned setting that did nothing is not silent.

## The pipeline, in order

| # | stage | stopwatch marks | code | Quality counterpart | measured before → after (per image) |
|---|---|---|---|---|---|
| 1 | intake | — | `load.rs`, `cli/alpha.rs`, `coverage.rs` | shared (01) | see below; with the palette, wall 282 → 203 ms at 2048 px opaque |
| 2 | histogram palette and labels | `palette` | `fast/palette.rs` | 03 (MDL palette) | 47.7 → 7.1 ms at 2048 px opaque; 1.85 → 0.40 ms at 128 px |
| 3 | region clean-up and faces | `slivers`, `despeckle`, `split` | `fast/faces.rs`, `fast/faces/runs.rs` | 04 (regions) | 76.2 → 3.86 ms at 2048 px; 0.50 → 0.09 ms at 128 px |
| 4 | ramps | `ramps` | `fast/bands.rs` | 05 (gradients) | in the shared totals below |
| 5 | planar map and refinement | `build_map`, `symmetry_detect`, `refine_subpix`, `refine_junc`, `symmetry` | `planar.rs`, `planar/*.rs`, `symmetry.rs` | shared (06, 07, 10) | ramps + map + refinement: 42 → 13 ms at 2048 px opaque |
| 6 | Potrace-class fitter | `fit_dp` | `fast/mod.rs`, `polygon.rs`, `smooth.rs`, `curve.rs`, `prims.rs` | 11, 12 | see §6 |
| 7 | emit | `fills`, `emit` | `cli/pipeline.rs`, `cli/alpha.rs`, `cli/emit.rs` | shared (13) | with stage 5 on transparent images: 130 → 21 ms at 2048 px |

Whole-trace effect, each branch measured on its own against its own baseline run, so the
figures below are not additive: the clean-up round took
`trace_total` at 2048 px from 193 to 116 ms; the palette-and-intake round took it from 179 to
131 ms and the process's wall time from 282 to 203 ms. At 128 px the palette round took
`trace_total` from 4.34 to 2.80 ms.

## How it works

### 1. Intake

**What it computes.** Decode the file to straight RGBA floats, undo an exact
nearest-neighbour upscale, and matte transparency onto an opaque ground; then every stage
reads the image composited over white. Shared with Quality mode and documented in full in
`01-intake.md`; this section lists what the round changed.

**Method.**

* **One read of the file, one widening pass.** `load_image_capped` reads the file into
  memory once and decodes header and pixels from those bytes (`load.rs:229-274`). The bytes
  become floats through a 256-entry table, `UNIT[k] = k / 255` (`load.rs:115`), straight from
  the decoder's own buffer for 8-bit RGB and RGBA, in parallel chunks from 256 × 256 pixels on
  (`from_dynamic`, `widen`, `load.rs:153-200`).
* **Unblock by the gcd of the change positions.** Only the factors that divide
  `gcd(w, h, every column and row where neighbours differ by more than 1/256)` get the block
  test; on ordinary art the gcd reaches 1 a few rows into the content and no block test runs
  (`pixel_grid`, `change_gcd`, `crates/inkvec-cli/src/alpha.rs:388-474`).
* **Matte in place, in parallel.** The transparency scan and the flatten are parallel maps
  from 256 × 256 pixels on, and the flatten writes over the input's own buffer
  (`alpha_source_owned`, `flatten_in_place`, `alpha.rs:797-1036`).
* **Composite over white in parallel** (`Rgba::composited`, `coverage.rs:213-230`).

Each is exact: every output of the parallel maps depends on one input value, the
transparency scan is a pure predicate, and the gcd filter only skips factors that provably
cannot pass the block test; each rewrite keeps the old code as an oracle and is compared bit
for bit.

**Citations** (labels as in the doc comments): the composite and the flatten, "Method from"
Porter & Duff, "Compositing Digital Images", SIGGRAPH '84 (the "over" operator with an opaque
ground, adapted to straight colour); the widening, "Not from the literature", "See also"
Ragan-Kelley et al., "Halide", PLDI 2013; the gcd filter, "Not from the literature", "See
also" Popescu & Farid, "Exposing Digital Forgeries by Detecting Traces of Resampling", IEEE
Trans. Signal Processing 2005; flattening in place, "Not from the literature: buffer reuse",
"See also" Leijen, Zorn & de Moura, "Mimalloc", APLAS 2019; the single file read, "Not from
the literature: plumbing".

**Costs.** Before, at 2048 px (shared-stage research): the load 20 ms (decode 4.9, the RGBA
copy 4.0, the float conversion 10.1), all serial; unblock 7.1 ms; the transparency scan
3.3 ms on an opaque image; the flatten 26.7 ms on a transparent one; the composite 11–18 ms,
serial. After: the palette-and-intake round as a whole took wall time at 2048 px opaque from
282 to 203 ms; its per-stage intake figures were not reported separately. Identity: Fast
493/493 files, including 25 intake variants (JPEG, WebP, GIF, BMP, TIFF, grey, 16-bit,
palette PNG, upscales, over 2048 px); Quality 286/286.

### 2. The histogram palette

**What it computes, in the field's terms** (`fast/palette.rs:30-43`). Every pixel is binned:
5 bits per sRGB channel (15-bit keys) on an opaque image, 4 + 4 + 4 colour bits and 4 opacity
bits on one traced with its transparency. A bin counts as evidence for an ink only through its
*flat* pixels — those whose four neighbours fall in the same bin — so an anti-aliased rim,
one pixel wide, never becomes an ink.

* The histogram is a **colour coherence vector** in its local form: per bucket, the pixels
  whose 4-cross stays in the bucket (the flat pixels of bin `k` are the erosion of
  `X_k = {p : key(p) = k}` by the 4-cross, the image padded by `k`), where Pass, Zabih &
  Miller count coherence by connected-component size.
* Founding inks is **leader clustering** over weighted bins taken in popularity order: a bin
  joins the nearest ink when within `SAME_INK` (0.012 OKLab), or within `merge_distance`
  *and* next to one of that ink's bins on the grid; otherwise it founds an ink, up to
  `max_colors`. Strokes too thin to have a flat pixel get **thin inks** from *paired* pixels
  (at least one neighbour in the bin), unless they are a blend of two other inks.
* The per-bin nearest-ink table is an **inverse colour map** restricted to occupied cells,
  and labelling sure pixels through it is **histogram backprojection**; a pixel whose bin is
  not itself an ink colour (a blend) goes to a neighbour's ink or to the ink it is made of.
* The parallel histogram is a **privatised generalised histogram**, computed on **runs**.

Colours are compared over white and over a second ground (`native::Ink2`), so white paint and
the clear ground are as far apart as white and black; on an opaque image that is plain OKLab.

**Method: four passes** (`fast/palette.rs:58-73`).

1. **Keys and histogram** (`keys_and_histogram`, `palette.rs:683-781`). The image is cut into
   bands of whole rows, processed in parallel. Each band keys its rows once per *colour run*
   (`key_rows`, `:501-521`: the key is a pure function of the pixel's four floats, and 99.56%
   of pixels repeat their left neighbour's colour at 2048 px), then walks *key runs*
   counting pixels, paired and flat pixels and their f64 colour sums (`histogram_rows`,
   `:568-633`): inside a run the horizontal half of the erosion comes from the run's ends, and
   only the rows above and below are read per pixel. A run's sums are held in registers and
   stored once, because adding each pixel into the bin's memory made every addition wait on
   the store before it. Row `r + 1` is keyed just before row `r` is counted, so the count reads
   pixels the keying has just brought into cache. The bands' histograms are merged at the end
   (`Bins::absorb`), which is exact by the lemma below. Rounding to a level is
   `trunc(x + (0.5 − 2⁻²⁵))`, identical to `f32::round` on every one of the 1,065,353,217
   floats in `[0, 1]` for both grids and about twice as fast, because `f32::round` compiles to
   a libm call on the default x86-64 target (`level`, `:174-209`).
2. **Inks** (`found_inks`, `:857-893`; `thin_inks`, `:915-973`) from the table of occupied
   bins, then opacity snapping. Microscopic: at most 36 candidates and 36 inks measured.
3. **Lookup table:** each occupied bin's nearest ink, once, and whether the bin *is* that ink
   (`palette.rs:1213-1228`).
4. **Labels** (`label_rows`, `:1318-1332`): a sure pixel's label is written inline from the
   table; only the others — 0.29% of pixels at 2048 px, 4.4% at 128 px — go to the
   neighbourhood rule (`Blends::blend_label`, `:1037-1081`). Each ink's share is counted per
   run of equal labels, in integers, and converted to the f32 value the old per-pixel float
   count produced (`ink_shares`, `:1377-1384`).

Per-bin state is kept for the *occupied* bins only (`Bins`, `palette.rs:211-307`): a median of
16 on the 128 px screen set and at most 610 on the 2048 px set, out of 65,536 keys. A key
reaches its bin through a small open-addressing hash table (`Slots`, `:309-409`) that starts
at 64 entries and doubles as bins open, so each band's table is as large as the bins it met.
Below 256 × 256 pixels, and whenever rayon has a single worker (the single-threaded
WebAssembly build), every pass runs on the calling thread (`PARALLEL_MIN_PIXELS`, `:1119-1135`).

**The exactness lemma for parallel sums** (`in_exact_set`, `palette.rs:443-483`). Merging band
histograms adds f64 sums in an order the serial pass would not use, and floating-point
addition is not associative in general. It is here:

> Let V be a multiset of f32 values, each 0 or in `[2⁻⁸, 1]`, with `|V| ≤ 2²²`. Then summing
> V into an f64 in any order, grouped in any way, gives the exact real sum.
>
> *Proof.* A float `v ≥ 2⁻⁸` has an exponent of at least −8 and a 24-bit significand, so it
> is an integer multiple of `2⁻³¹`. Every partial sum is then `j · 2⁻³¹` with
> `0 ≤ j · 2⁻³¹ ≤ |V| ≤ 2²²`, so `j ≤ 2⁵³`, and every such number is representable in
> binary64. IEEE 754 addition is "computed exactly and then rounded"; a representable exact
> result is returned unrounded. By induction every partial sum is exact, so the final sum is
> the real sum, independent of order. ∎

Every 8-bit input that has not been resampled satisfies it: a channel is `k / 255 ≥ 1/255 >
2⁻⁸` or 0, and a translucent pixel over white is `fl(fl(s·a) + fl(1 − a)) ≥ 1/255` (0
violations on 253 of 253 measured images). The key pass checks every value it keys (a
repeated colour was already checked); a resampled raster (a `--max-dim` reduction,
`--intake-scale`) carries arbitrary box averages, fails the check, and the histogram is then
recounted serially in raster order over the finished keys. Above `EXACT_SUM_MAX_PIXELS =
2²²` (2048 × 2048, the default `--max-dim`) the histogram is serial too. Counts are integers,
so they merge exactly in any case.

**Citations** (labels as in the doc comments): the sparse per-bin state, "Method from"
Briggs & Torczon, "An Efficient Representation for Sparse Sets", ACM LOPLAS 1993, as
described by Cox, "Using Uninitialized Memory for Fun and Profit", 2008 (adapted: the sparse
side is a hash table); the hash table, "Method from" Knuth, *The Art of Computer
Programming*, vol. 3, §6.4 (linear probing, and multiplicative hashing with the multiplier
`⌊2³²/φ⌋`); working on weighted distinct colours, "Inspired by" Celebi, "Improving the
Performance of K-Means for Color Quantization", Image and Vision Computing 2011; runs,
"Inspired by" Breuel, "Efficient Binary and Run Length Morphology", 2007; flatness,
"Inspired by" Pass, Zabih & Miller, "Comparing Images Using Color Coherence Vectors", ACM
Multimedia '96; the parallel histogram, "Method from" Podlozhnyuk, "Histogram calculation in
CUDA", NVIDIA 2007, and Henriksen, Hellfritzsch, Sadayappan & Oancea, "Compiling Generalized
Histograms for GPU", SC20 (privatised sub-histograms merged at the end; adapted: sparse, and
associativity checked per input rather than assumed); ink founding, "Method from" Hartigan,
*Clustering Algorithms*, 1975, the leader algorithm, as described in Arnold's R package
`leaderCluster`; the lookup table, "Method from" Thomas, "Efficient Inverse Color Map
Computation", Graphics Gems II, 1991; labelling, "Method from" Swain & Ballard, "Color
Indexing", IJCV 1991 (histogram backprojection); the run-wise label count, "Inspired by"
Collet's `HIST_count_parallel_wksp` in FiniteStateEntropy; the rounding, the exactness lemma
and the integer share count, "Not from the literature", each with "See also" Goldberg, "What
Every Computer Scientist Should Know About Floating-Point Arithmetic", ACM Computing Surveys
1991 (the lemma also names Demmel & Nguyen, "Fast Reproducible Floating-Point Summation",
ARITH 2013, as the published approach it does not use, because pre-rounding would change the
values); the serial threshold, "Inspired by" rayon's `with_min_len`. Considered and not
used (`palette.rs:85-96`): accelerated nearest-ink search (Elkan, Hamerly), a distance
transform (Rosenfeld & Pfaltz 1966), memoising blend labels, and VTracer's clustering, which
has no global palette and would change the output.

**Costs.** Before, at 2048 px: 42.6 ms, 99% of it in the four-channel copy (6.9), the keys
(4.3), the serial histogram (17.8), labelling (3.3) and an unread running share (9.8); the
decisions themselves 0.16 ms. At 128 px: 1.75 ms, of which 0.37 ms was spawning rayon's global
pool (the palette was the process's first parallel call) and about 0.65 ms allocating and
scanning 65,536-entry tables for 16,384 pixels. After, in the merge measurement: 47.7 →
7.1 ms at 2048 px opaque, 1.85 → 0.40 ms at 128 px. At 128 px `fit_dp`
rose by 0.35 ms, because it now pays the pool start the palette no longer does: that cost
moved, it did not disappear. Identity: as for intake above; the old palette is kept verbatim
as the test oracle `fast/palette/reference.rs`.

### 3. Region clean-up on row runs

**What it computes, in the field's terms** (`fast/faces.rs:1-30`): *connected-component
analysis of the flat zones* of the label image (a flat zone is a maximal 4-connected set of
pixels of one label), followed by *region merging on the region adjacency graph*. Five
passes, in the order the front end calls them (`fast/front.rs:97-129`):

1. **Slivers** (`absorb_slivers`, `faces.rs:325`, mark `slivers`): pixels of *strips* —
   components with no interior pixel — that are blends of the inks of the thick components
   around them go to the nearer of those inks. Two-endmember linear unmixing, pixel by pixel:
   `t = clamp(((c − a)·(b − a)) / |b − a|², 0, 1)` and distance `|c − (a + t(b − a))|²`
   within `BLEND_TOL² = 0.04²`, in straight sRGB and opacity (`blend_of`, `faces.rs:159-189`).
2. **Rims** (`absorb_rims`, `faces.rs:431`, mark `slivers`): a strip whose own ink is a blend
   of two inks it borders is an anti-aliased rim; each of its pixels goes to the side it
   covers more of.
3. **Same-ink merge** (`merge_same_inks`, `faces.rs:530`, mark `slivers`): a component takes
   the ink of a larger neighbour the eye cannot tell from its own (CIEDE2000 below
   `SAME_INK_DE00`, opacities within 0.02), largest components first, longest shared border
   winning.
4. **Despeckle** (`despeckle`, `faces.rs:614`, mark `despeckle`): components under the speckle
   floor take the label they share the longest border with — an area filter on flat zones.
   The floor is `min_region`, or 4 px per 512 × 512 of image up to 16 px, VTracer's default
   4 × 4 patch (`speckle_floor`, `fast/front.rs:31-46`).
5. **Faces** (`write_faces`, `faces/runs.rs:502`, mark `split`): the components of the result
   are the faces; their ids are written over the label buffer.

**Method: everything on row runs** (`fast/faces/runs.rs`). The label image is held as its
maximal row runs (`RunLabels`); a run is scanned 8 labels at a time while they all equal the
run's label, a compare the compiler turns into one vector instruction (`push_row_runs`,
`runs.rs:124-165`). On runs:

* **components** are union-find over runs: each run is united with the overlapping runs of
  the row above that carry its label, walked with two pointers; the smaller run index is kept
  as the root, so one forward pass numbers the classes in order of first appearance, reading
  each run's parent's id instead of running a find (`components`, `runs.rs:218-290`);
* **interior-ness** is decided per run: pixel `(x, y)` of label `l` is interior iff its four
  neighbours carry `l`, so a run `x0..x1` holds an interior pixel iff some `x` in
  `x0 + 1 .. x1 − 1` lies under a run of `l` in the row above and over one in the row below
  (`interiors`, `runs.rs:292-357`);
* **the region adjacency graph with border lengths** is read off *contacts*: consecutive runs
  of a row touch across one pixel edge, runs of adjacent rows across the length of their
  overlap, and every pixel edge between two components lies in exactly one contact
  (`for_each_contact`, `runs.rs:359-406`);
* an **edit** (a new label per component, or a few single pixels) rebuilds the run list in one
  pass, merging neighbours that end up with one label, so the runs stay maximal
  (`relabel_components`, `apply_pixel_edits`, `runs.rs:408-491`).

The labels are read once, when the runs are built, and written once, with the face ids;
pixel colours are read only at strip pixels, in place, never copied (`Pixels`,
`faces.rs:119-153`).

**Why the output is the per-pixel code's** (`faces.rs:53-77`): maximal runs are a canonical
form of a label image, and every edit keeps them maximal, so the runs after each pass are
exactly the runs the per-pixel code would read off its relabelled image; a 4-neighbour is in
a pixel's component iff it has the pixel's label, so every count summed over contacts is the
per-pixel count; each pass reads the labels as they were before it and chooses maxima or
minima under total orders with explicit tie-breaks, so visiting order does not matter (where
it did, in the sliver candidates' order, it is reproduced); and the floating-point
expressions are the same functions on the same four floats. The per-pixel code is kept as a
test oracle (`faces/reference_tests.rs`), checked on random and degenerate images and on 254
research label dumps.

**Citations** (labels as in the doc comments): labelling on runs, "Method from" Lemaitre &
Lacassagne, "How to speed Connected Component Labeling up with SIMD RLE algorithms", WPMVP
2020 (features on runs; the final per-pixel relabelling is "very expensive" and "should be
avoided"), and Wu, Otoo & Suzuki, "Optimizing two-pass connected-component labeling
algorithms", PAA 2009 (the equivalence table whose root is the smallest index, and FLATTEN);
"See also" He, Chao, Suzuki & Wu, "Fast connected-component labeling", Pattern Recognition
2009; the vector run scan, "Inspired by" Lemaitre & Lacassagne; despeckle and the same-ink
merge, "Method from" Salembier & Serra, "Flat zones filtering, connected operators, and
filters by reconstruction", IEEE TIP 1995 (connected operators: they act on whole flat zones
and never split one); the speckle floor, "Inspired by" Selinger's Potrace `turdsize` and
VTracer's `filter_speckle`, which delete a small region where this gives it to a neighbour;
merging, "See also" Felzenszwalb & Huttenlocher, "Efficient graph-based image segmentation",
IJCV 2004, and Najman & Cousty, "A graph-based mathematical morphology reader", 2014;
unmixing, "See also" Bioucas-Dias et al., "Hyperspectral unmixing overview", 2012; reading
pixels on demand, "Inspired by" Halide (PLDI 2013), store versus recompute. "Not from the
literature": the interior test and the border lengths on runs, the observation that the
component table plus the adjacency graph with border lengths are sufficient statistics for
every pass (colours are needed only at strip pixels), and the strip criterion itself.
Rejected, with the numbers that decided it (`faces.rs:105-112`, `runs.rs:65-72`): caching
sliver decisions (about one distinct key per strip pixel), pixel-scan decision-tree
labellers such as BBDT and Spaghetti (they speed up the union-find, 1.05 ms of the 65.6 ms
stage), dynamic connectivity (the union-find on runs is already 0.08–0.65 ms), and parallel
stripes (at most about 1 ms left to win).

**Costs.** Before, at 2048 px: 18 passes over the image (22 with the same-ink merge), about
118 bytes of traffic per pixel, 65.6 ms, 39% of the whole trace — although all the passes
together change at most 3.6% of the pixels (none at the median) and runs are 0.24% of the
pixels at the median. After (slivers + despeckle + split, per image): 128 px 0.50 → 0.09 ms; 512 px
5.73 → 0.57 ms; 2048 px 76.2 → 3.86 ms; bigalpha 82.3 → 3.9 ms. `trace_total` at 2048 px:
193 → 116 ms. Identity, `--mode fast`: screen 246/246, held_a 156/156, big 7/7, bigalpha 3/3,
s512 58/58, flat 1/1; and on 60 screen icons `--no-background`, `--monochrome` and Quality
60/60 each.

### 4. Ramps

**What it computes** (`fast/bands.rs:1-30`, `merge_ramps`, `:313-495`). A smooth gradient
has no flat colour, so the palette quantises it into bands, and each band is a face with a
boundary of its own. This is the one-shot version of Quality's pairwise band merging:
adjacent faces whose inks are within `RAMP_STEP = 0.09` OKLab and that share at least 3 pixel
edges are joined by union-find into clusters; each cluster of two or more faces and at least
64 pixels is sampled on an even grid, fitted once with Quality's own gradient models
(`gradient::fit_pixels` at the noise floor with the BIC penalty, then `gradient::select`), and
kept when its RMS residual `g` over interior samples satisfies
`g ≤ max(1.25 · f + 1/255, 2/255)`, `f` being the flat bands' residual; two bands already
within 1.5/255 are left flat. Opaque images with gradients on only.

**Method, since 2026-09-30.** Passes 1 to 4 read the palette and the row runs instead of the
pixels (`bands.rs:18-32`):

1. **Palette precheck** (`inks_may_join`, `bands.rs:156-214`): when no two *different* inks lie
   within `RAMP_STEP`, the pass returns before reading a pixel. It is a proof, not a guess:
   the faces are the 4-connected components of the ink map, so two faces that touch always
   carry different inks, and if every pair of different inks is farther apart than
   `RAMP_STEP` no join can happen and nothing would change. Guarded for the one exception:
   past `u16::MAX − 1` components the rest fold into face 0, and the full pass decides.
2. **Row runs** of the face map (`planar::runs::RowRuns`, shared with the planar map).
3. **Contacts** (`contacts`, `bands.rs:72-130`): the border length of every touching pair of
   faces, read off consecutive runs and the overlaps of adjacent rows' runs, keyed and
   summed after a sort; no join means return.
4. **Samples** (`gather_samples`, `bands.rs:249-311`): each cluster's grid sample, read off the
   runs in increasing pixel index, the order the fit sums in; divisibility is tested without
   `%` (wazero's arm64 miscompile).

**Citations** (labels as in the doc comments): contacts and samples, "Method from" He, Chao &
Suzuki, "A Run-Based Two-Scan Labeling Algorithm", IEEE TIP 2008; contacts, "Inspired by"
Ji, Piper & Tang, "Erosion and dilation of binary images by arbitrary structuring elements
using interval coding", Pattern Recognition Letters 1989; the precheck, "Not from the
literature: a necessary condition for a join, checked on the palette before the image",
"See also" He & Chao, "A Very Fast Algorithm for Simultaneously Performing Connected-Component
Labeling and Euler Number Computing", IEEE TIP 2015.

**Costs.** Before, at 2048 px: 14.8 ms (contacts 4.6, gather 5.9, fit 3.8). The precheck
proves the pass empty on 5 of the 7 opaque 2048 px images, 18 of 26 at 512 px and 3 of 5 at
128 px. After: counted in the shared totals of stage 5. The pixel versions are kept as
`contacts_scan` and `gather_samples_scan` and the tests compare them.

### 5. The shared planar map and refinement

**What it computes.** The planar map of the faces — every boundary between two faces stored
once, junctions exact on the lattice — then every boundary point moved to its sub-pixel
position by unmixing the two faces' fills, junctions settled, and mirror symmetry detected and
restored. Shared with Quality; documented in `06-planar-map.md`, `07-subpixel.md` and
`10-symmetry.md`. Fast passes the refinement the noise floor (`coverage::NOISE_FLOOR`) rather
than a noise measurement — "the fast fitter reads no per-point uncertainty, and the refinement
only needs a floor under its contrast test" (`fast/front.rs:152-153`) — and skips the boundary
solve and decode.

**Method, since 2026-09-30.**

* **Cracks from row runs** (`dual_segments`, `planar/cracks.rs:213-348`): vertical cracks at
  run starts, horizontal ones where the runs of two rows overlap with different labels.
* **Incidence by radix sort** with 11-bit digits, in emission order, and **`O(1)` walk
  steps** through each segment end's recorded node (`Incidence`, `planar/cracks.rs:54-211`).
* **Refinement measured in parallel, then applied**: a pure map over edges and, on edges of
  64 points or more, over vertices, with no parallel reduction; serial under 512 boundary
  vertices or while a debug printout or contour dump is on (`measure_subpixel`,
  `planar.rs:502-603`).
* **Symmetry detection beside the measuring phase**, under `rayon::join`; both only read the
  lattice map (`lib.rs:1093-1126`).
* **Junction debug flags read once** into a `OnceLock` (`planar/junctions.rs:54-70`).

**Citations** (labels as in the doc comments): cracks, "Method from" He, Chao & Suzuki 2008 and
He, Chao, Suzuki & Wu 2009, "Inspired by" Ji, Piper & Tang 1989, "See also" Kovalevsky 1989 and
Damiand, Bertrand & Fiorio 2004; the radix sort, "Method from" Knuth, TAOCP vol. 3, §5.2.5;
the parallel refinement, "Method from" Blelloch, Fineman, Gibbons & Shun, "Internally
deterministic parallel algorithms can be fast", PPoPP 2012, with Demmel & Nguyen, ARITH 2013,
as related work; running detection beside it, "Inspired by" Halide (PLDI 2013).

**Costs.** Before, at 2048 px opaque: `build_map` 15.9 ms (the crack scan 9.9, two comparison
sorts 1.5 each), the refinement 10.6 ms serial at 0.44 µs per vertex. After, for the shared
stages the round rewrote — the ramp pass, map and refinement together, plus the face-alpha
pass of stage 7 on transparent images (the merge notes call them "shared stages in total";
their before figures match the research's per-stage costs, 14.8 + 15.9 + 10.6 ms, plus
88.5 ms of face alpha) — per image: 2048 px opaque 42 → 13 ms, 2048 px transparent 130 → 21 ms, 512 px 10.2 → 5.1 ms,
128 px 1.8 → 1.2 ms. Quality gains the map and refinement part too. Identity: 100% in Fast and
Quality on screen, held_a, s512, big, bigalpha, flat and std; also `--no-native-alpha
--cutout`, and `--no-background` / `--monochrome` on 60 icons. The WebAssembly and
WASI builds compile, and the new hot loops use no `%`.

### 6. The Potrace-class fitter

**What it computes.** One fitted path per edge of the map, lines and cubic Béziers — or, for a
closed edge that is a circle or an ellipse, that primitive. Each shared edge is fitted once and
both faces draw the same curve, so the fitter cannot open a seam. In the field's terms it is
the classical pipeline of Selinger's Potrace (2003) — optimal polygon, vertex adjustment,
corner-aware smoothing, curve-run optimisation — "restated for sub-pixel input": Potrace fits
a lattice path of pixel corners, where these points already sit at their measured sub-pixel
positions, so every lattice test becomes a metric one (the module overview of `fast/mod.rs`
and of `polygon.rs`). "Written from the paper; no Potrace or Trazor source was read or used."

What follows describes the fitter as it is on the branch this page was written on (the base
of the 2026-09-30 round). The fitter round of the same date, merged into `integ/fast` later,
changes how several of these steps are computed; see "The fitter round of 2026-09-30" at the
end of this section.

**Per edge, in parallel** (`fit_edges`, `fast/mod.rs`): the edges are fitted as a rayon
parallel map, output in edge order whatever the thread count. Each edge's tolerances depend on
the OKLab contrast between its two faces: at or above `FAINT = 0.12` they are as given; below,
every distance tolerance but `flat` grows as `FAINT / contrast`, up to 3×, because where the
two sides are close in colour a misplaced boundary costs little; a boundary of a gradient face
is fitted as if its contrast were at most `FAINT / 1.6` (`FastFit::for_contrast`). The
defaults (`FastFit::default`): polygon tolerance 0.5 px, vertex box 0.5 px, corner tolerance
0.25 px (Potrace's `alphamax` as a distance), merge tolerance 0.2 px (Potrace's
`opttolerance`), and 0.05 px for a cubic to count as a line. A closed edge of 8 points or
more drops its first point, the lattice node the refinement leaves up to 0.6 px off the edge
(`fit_edge`).

The steps, per edge (`fit_edge`, `fit_points`):

1. **Primitives** (`prims::primitive`), closed edges only, on the denoised points: a cascade
   of cheap tests before expensive fits. Kåsa's algebraic circle fit (one linear solve)
   rejects anything not roughly round; when it lies within 0.6 px of every point, a circle is
   accepted if the orthogonal-distance circle fit (Levenberg–Marquardt) lies within 0.3 px of
   every point and the ring encloses at least 80% of its area; otherwise a bound on the
   ring's span and area rules most boundaries out of an ellipse before any ellipse fit, and
   an ellipse is accepted when Taubin's algebraic ellipse
   lies within 0.6 px of every point and the orthogonal-distance ellipse within 0.3 px, with
   both radii at least 1.5 px. The primitive is drawn as four cubics, the standard
   approximation of a quarter arc with control arms `k = 4/3 · tan(Δt / 4)`. Rounded
   rectangles are left to the curve fit.
2. **Denoise** (`smooth::denoise`): a `[1 2 1] / 4` binomial low-pass along the boundary,
   which removes the 0.1 px alternation the lattice leaves in refined points and moves a
   curve of radius `r` by only `1 / 4r` px; a point that turns sharply over two steps each way
   (more than 50°) is kept as a corner, and the ends of an open boundary, which are junctions,
   stay put.
3. **Optimal polygon** (`polygon::open`, `polygon::closed`; Selinger §2.2): the fewest
   straight sides that stay within `poly_tol` of every point, and among those the one closest
   to the points. A side `i → j` is admissible when the direction `p_j − p_i` lies in the
   *cone* of directions from `p_i` that pass within `tol` of every point between them (each
   point narrows the cone by `asin(tol / r)`), no point has fallen back towards `p_i` by more
   than `tol`, and it spans at most 160 points. A dynamic program over vertices minimises
   `(sides, Σ squared distances)` lexicographically, the distances read in `O(1)` from prefix
   sums. A closed ring is cut at its sharpest point and solved as an open run.
4. **Vertex adjustment** (`smooth::adjust_vertices`; Selinger §2.3.1): each side gets the
   total-least-squares line through the points it spans, and each interior vertex moves to the
   point minimising its two sides' squared distances within a box of half-width `vertex_box`
   around its polygon position, with a small ridge towards that position so parallel sides stay
   well posed. The ends of an open run stay exactly where they are.
5. **Smoothing into pieces** (`smooth::pieces`; Selinger §2.3.2): one join per side, moved off
   the side's line to where the points are (by at most 0.5 px, since a curve drawn through the
   unmoved midpoints would shrink every round shape), and at each vertex the cubic from join to
   join, tangent to both sides, with its two arm lengths fitted to the points between the joins
   by Schneider's tangent-constrained Bézier fit (`curve::fit`) and capped so neither control
   point passes the vertex (Potrace's `alpha ≤ 1`). The vertex is a *corner* — two line pieces —
   when that cubic misses the points by more than `corner_tol` and by more than twice what the
   polyline through the vertex misses them by.
6. **Curve-run optimisation** (`curve::optimise`; Selinger §2.4, Potrace's `opticurve`): runs of
   smooth pieces between corners are merged where one cubic says the same thing — a run that
   turns one way, by less than 3.10 rad in total, of at most 24 pieces, whose merged cubic
   (same end points and tangents, arms fitted to samples of the pieces) stays within `opt_tol`
   of every sample. A dynamic program takes the fewest cubics per run.
7. **Segments** (`curve::to_segments`): a cubic whose control points lie within 0.05 px of its
   chord, and project inside it, is written as a line; consecutive collinear lines are joined;
   the ends are pinned to the junctions exactly.

Boundaries too short for a polygon (fewer than 3 points, or 4 for a ring) are drawn as straight
lines through their points.

**Citations**, as the doc comments on this branch name them: Selinger, "Potrace: a
polygon-based tracing algorithm", 2003 (§2.2, §2.3.1, §2.3.2, §2.4); Schneider's Bézier fit
with fixed end tangents (the research report cites Schneider's 1990 Graphics Gems code,
`FitCurves.c`); Kåsa's algebraic circle fit; Taubin's algebraic ellipse fit (Taubin 1991 in the
research report); Levenberg–Marquardt for the orthogonal-distance fits. The fitter files on
this branch do not yet carry the "Method from" / "Inspired by" labels the other Fast files use;
the fitter round's docs pass added them.

**Where the time went** (fitter research, 2026-09-30, before the fitter round). `fit_dp` took
1.01 ms per 128 px icon (16% of the engine's stages) and 26.75 ms at 2048 px (14%); its wall
time is the slowest edge's (0.81 ms and 23.2 of 23.8 ms). Replayed on one thread, the polygon
was 59% of the fitter at 128 px, 75% at 2048 px and 86% on the flat logo, the primitives 21.8%
and 15.2%. The image-frame ring alone was 31%, 38% and 57% of the fitter's CPU and the slowest
edge on 161 of 246 icons and 7 of 7 big images; its output was always four lines plus one
spurious cubic chamfering the start corner (6 parameters per framed image, on 166 of 246
screen icons), because `fit_edge` drops the ring's first point.

**The fitter round of 2026-09-30.** **Unverified:** merged into `integ/fast` at `5aa2566`
after this page's branch was cut, and not read here; what follows is what the round's merge
notes report, with the citations the research report verified, for whoever brings this page
up to that code to check against its doc comments. Exact, byte-identical changes: the polygon
DP *fathoms* sides and anchors that cannot win (branch and bound, Morin & Marsten 1976); runs
that lie on exact lattice lines are scanned in closed form (reported as "Not from the
literature", inspired by VTracer's walker and Freeman's chain codes, 1961, see also
Debled-Rennesson & Reveillès 1995 on digital straight segments); the admissibility scan of
edges of 2,048 points or more runs in parallel while the relaxation stays sequential (Brent
1974; Blumofe & Leiserson 1999); the primitives no longer compute an unread χ², and the
Levenberg–Marquardt ellipse reuses the Taubin fit (Taubin 1991; Ahn et al. 2001); a ring is
denoised once, not twice; the labels are moved rather than cloned, and Fast builds no
polylines or λ scales. Output-changing, judged by the gate: the image frame is written as the
image rectangle, removing the chamfer cubic (screen dE00 0.3641 → 0.3640, parameter ratio
2.127 → 2.120; held_a dE00 equal, 2.149 → 2.137; only opaque images with a background face
change); and the ellipse fit starts from the algebraic conic alone (Halíř & Flusser 1998; 0 of
464 SVGs changed). Reported `fit_dp` per image: 2048 px opaque 26.2 → 6.1 ms, 2048 px
transparent 23.3 → 9.1 ms, 512 px 5.74 → 1.96 ms, 128 px 1.00 → 0.47 ms; exact tip identical
on Fast 464/464 and Quality 256/256, and all 11,021 replayed edges bitwise equal at 1, 4 and
16 threads.

### 7. Emit

**What it computes.** The SVG document, from the fitted paths, the faces' fills and the
palette: shared with Quality and documented in `13-emit.md`. Fast turns shape harmonization off
(`emit_options`, `crates/inkvec-cli/src/pipeline.rs`). An image traced with its transparency
keeps the native-alpha model: inks carry an opacity and the clear ground is an ink of its own
(`fast/front.rs:14-16`).

**Method, since 2026-09-30.** The face-alpha pass that runs inside the `emit` mark
(`alpha::face_alpha`) gathers every face's alpha statistics in one pass over row runs, and fits
an alpha ramp (under `--cutout`) only for faces where the fit can succeed: a face with fewer than
64 interior pixels, or whose interior alphas are all exactly 1, provably gets `None` — in the
second case the normal equations' right-hand side is bit for bit a column of the matrix, so
Cramer's rule gives an exactly zero gradient. The candidates' pixels are gathered in one more
pass instead of one whole-image scan per face. Details, proof and citations in `13-emit.md`,
"Gradient and fill emission".

**Costs.** Before, at 2048 px on the three transparent images: 88.5 ms of Fast mode's 333 ms,
60.7 ms of it in the per-face scans; 96% of the ramp calls on the research sets were provably
`None`, and none of 3,125 ever returned a ramp. After: inside the transparent total of stage 5,
130 → 21 ms.

## Constants and thresholds

Fast mode's own constants are listed, with their basis, in [`constants.md`](constants.md),
section 14; the shared stages' new ones (`PAR_VERTICES`, `PAR_MAP_VERTICES`, the radix digit,
`RAMP_MIN_INTERIOR`, the intake's serial thresholds) in sections 01, 06, 07 and 13. Every
serial-versus-parallel threshold added in this round (256 × 256 pixels for the palette, the
composite and the intake passes; 512 vertices for the refinement) chooses only the schedule,
never the output.

## Failure modes and edge cases

- **A resampled input takes the slow histogram.** A `--max-dim` reduction or `--intake-scale`
  leaves box averages outside the exact set, so the palette's bands stop counting and the
  histogram is recounted serially in raster order. The result is still identical; only the
  speed is lost.
- **More than 65,534 faces** fold into face 0 (`write_faces`, as `regions::split_components`
  does), which then holds several inks under one colour; the ramp precheck's proof fails there,
  so it defers to the full pass.
- **The rayon pool start moves, it does not vanish.** In the command-line tool the first
  parallel call spawns the global pool (0.37 ms median). With the palette serial at 128 px,
  `fit_dp` pays it instead (+0.35 ms at 128 px); the Studio and the WebAssembly Space keep a warm
  pool.
- **The image frame's spurious chamfer** (four lines plus one cubic per framed image, 6
  parameters) is present on this page's branch; the fitter round's frame-as-rectangle change
  removes it (see §6).
- **Two run encodings exist**: `planar::runs::RowRuns` (the planar map and ramps) and
  `fast/faces/runs.rs` (the clean-up). Unifying them, and letting `RunLabels::for_each_contact`
  feed the ramp contacts, are open opportunities, not defects.
- **Stale overview on this branch.** `fast/mod.rs`'s pipeline overview still names
  `faces::absorb_slivers` and `faces::faces`, which became `faces::RunLabels::{absorb_slivers,
  absorb_rims, merge_same_inks, despeckle, write_faces}`; `integ/fast` corrects it
  (`5aa2566`).

## Environment overrides

On this page's branch, Fast mode's own stages read no environment variable. The shared
stages keep theirs (see `07-subpixel.md`: `INKVEC_SUBPXDBG` and `INKVEC_DUMP_CONTOUR` make
the refinement run serially, in order), and `INKVEC_TIMING` prints the stage marks listed
above. **Unverified:** the fitter round adds an edge-replay harness (`fast/replay.rs`) whose
commit message says it reads its knobs through `inkvec_core::env`; its variables are not
listed here.

## Open questions

- The fitter section above describes the code before the fitter round; §6 needs rewriting
  against `integ/fast`'s `fast/mod.rs`, `polygon.rs` and `prims.rs` (and the fitter constants
  in `constants.md`, section 14, need their line numbers), with the citations and labels its
  doc comments carry.
- The per-stage cost of intake after the round was not measured separately from the palette.
- Fast's front end composites the image over white again (`fast/front.rs:82`); for an image
  the intake already matted (alpha 1 everywhere) that is an identity, about 9 ms at 2048 px
  by the merge notes. Passing the intake's over-white plane through would remove it; this is
  a proposal, not measured as a change.
