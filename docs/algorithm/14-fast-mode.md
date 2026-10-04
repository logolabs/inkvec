# Stage 14 — Fast mode

> A second route through the pipeline: the same planar map, sub-pixel refinement and
> emitter as Quality mode, with every expensive search replaced by a one-pass version and
> the curve fitter replaced by a Potrace-class one — and, since the 2026-09-30 round, each
> of its own stages computed on row runs, in parallel where it pays, to the same bytes, but
> for the image frame, which is now written as the image rectangle.

**Source:** `crates/inkvec-trace/src/fast/` — `front.rs` (the front end), `palette.rs`,
`faces.rs` and `faces/runs.rs` (the clean-up), `bands.rs` (ramps), `mod.rs`, `polygon.rs`,
`smooth.rs`, `curve.rs` and `prims.rs` (the fitter), `replay.rs` (a test-only replay of
dumped fitter inputs); `crates/inkvec-trace/src/regions.rs` (`cap_components`, which the
clean-up calls past the face-id limit); `crates/inkvec-fit/src/primitives/ellipse.rs`
(`taubin_ellipse`, `fit_ellipse_seeded`, which the fitter's primitive test calls);
`crates/inkvec-cli/src/fast.rs` (the command line's side). The stages it shares are
documented in `01-intake.md`,
`06-planar-map.md`, `07-subpixel.md`, `10-symmetry.md` and `13-emit.md`.
**Entry points:** `--mode fast` (`TraceMode::Fast`, `crates/inkvec-cli/src/args.rs:21-29`,
default `quality`). The trace crate dispatches to `fast::trace_color` or, for an image
traced with its transparency, `fast::trace_color_native` (`fast/front.rs:17-29`) when
`ColorOptions::fast` is set (`crates/inkvec-trace/src/lib.rs:290-302`); the command line
fits the result with `fast::fit` (`crates/inkvec-cli/src/fast.rs:28-45`), which calls
`inkvec_trace::fast::fit_edges` (`fast/mod.rs:318-365`) with the map's width and height.
**Pipeline position:** it replaces stages 03–05 (palette, regions, gradients) with its own
front end; shares stages 01, 06, 07, 10 and 13; skips 08 (the boundary solve,
`lib.rs:1137`) and 09 (decode, `lib.rs:1152`); and replaces 11 (curve fitting) with its own
fitter, without 12 (repair) or shape harmonization (`repair_fits`,
`crates/inkvec-cli/src/pipeline.rs:885`; `emit_options`, `pipeline.rs:515-526`).

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
(`args.rs:305-308`).

The round of 2026-09-30 rewrote Fast's own stages and the stages it shares for speed,
**with the output held fixed**: every rewrite is exact, keeps the code it replaced as a test
oracle, and was byte-compared on the benchmark sets. Two changes in the fitter are the
exceptions, and the quality gate judged them instead (§6): the image frame is written as the
image rectangle, which changes opaque images with a background face, and the orthogonal
ellipse starts from the algebraic conic alone, which changed no file on the gate but is not
provably identical. This page describes the pipeline as it
stands after that round, stage by stage, with what each stage computes in the field's
standard terms, the method, its citations as the code's doc comments label them, and the
measured costs before and after. It also covers two fixes merged on 2026-10-03 that came
after the round: a run of curve pieces that U-turns is no longer collapsed to its last
piece, and a ring of pieces is never one cubic (§6, step 6; `3513beb`, judged by the
gate); and an image with more components than face ids has its smallest components merged
into their neighbours instead of folded into face 0 (§3; `be5abad`, byte-identical on the
gate's sets).

**How the numbers were measured.** Stage timings come from the implementing branches' merge
notes and the research reports of 2026-09-30, on the development machine (an 8-core Ryzen 7
5800X, shared with other work, so milliseconds are approximate and counts and ratios are the
reliable part). The combined figures — every branch of the round together — come from one
run of `main` at `55ee4e0` against the merged `integ/fast` tip, per image, mean ms. The
fitter's stage figures marked "replay" come from the phase-1 replay: 11,021 map edges dumped
by the research build and fitted again on one thread without the engine (`fast/replay.rs`).
The sets: *screen*, 246 icons at 128 × 128 (241 of them RGBA, traced with
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
* **the report** opens with a line saying Fast mode ran and naming what it skipped — "fast
  mode     Potrace-class fit, flat fills; not run: boundary solve, curve DP, gradients,
  ring repair, harmonization" (`report`, `crates/inkvec-cli/src/fast.rs:93-105`) — followed
  by every option the caller moved off its default that only steers a skipped stage
  (`fast_ignored`, `fast.rs:47-91`): `--tau`, `--content-units`, `--bezier-cost`,
  `--corner-angle`, `--time-budget`, `--simplify-faint`, `--harmonize-threshold`,
  `--use-symbols`, `--no-repair` and `--harmonize` / `--no-harmonize`, in that order. Not
  `--no-gradients`, because Fast still merges ramps, and not `--precision` or
  `--lambda-scale`, which the intake rescales on an oversampled raster, so a changed value
  is no evidence the caller set them. So a tuned setting that did nothing is not silent.

## The pipeline, in order

| # | stage | stopwatch marks | code | Quality counterpart | measured before → after (per image) |
|---|---|---|---|---|---|
| 1 | intake | — | `load.rs`, `cli/alpha.rs`, `coverage.rs` | shared (01) | see below; with the palette, wall 282 → 203 ms at 2048 px opaque |
| 2 | histogram palette and labels | `palette` | `fast/palette.rs` | 03 (MDL palette) | 47.7 → 7.1 ms at 2048 px opaque; 1.85 → 0.40 ms at 128 px |
| 3 | region clean-up and faces | `slivers`, `despeckle`, `split` | `fast/faces.rs`, `fast/faces/runs.rs` (and `regions.rs` past the face-id limit) | 04 (regions) | 76.2 → 3.86 ms at 2048 px; 0.50 → 0.09 ms at 128 px |
| 4 | ramps | `ramps` | `fast/bands.rs` | 05 (gradients) | in the shared totals below |
| 5 | planar map and refinement | `build_map`, `symmetry_detect`, `refine_subpix`, `refine_junc`, `symmetry` | `planar.rs`, `planar/*.rs`, `symmetry.rs` | shared (06, 07, 10) | ramps + map + refinement: 42 → 13 ms at 2048 px opaque |
| 6 | Potrace-class fitter | `fit_dp` | `fast/mod.rs`, `polygon.rs`, `smooth.rs`, `curve.rs`, `prims.rs` | 11, 12 | combined: 26.27 → 5.42 ms at 2048 px opaque; 1.00 → 0.46 ms at 128 px (§6) |
| 7 | emit | `fills`, `emit` | `cli/pipeline.rs`, `cli/alpha.rs`, `cli/emit.rs` | shared (13) | with stage 5 on transparent images: 130 → 21 ms at 2048 px |

Whole-trace effect, each branch measured on its own against its own baseline run, so the
figures below are not additive: the clean-up round took
`trace_total` at 2048 px from 193 to 116 ms; the palette-and-intake round took it from 179 to
131 ms and the process's wall time from 282 to 203 ms. At 128 px the palette round took
`trace_total` from 4.34 to 2.80 ms.

Measured together, `main` `55ee4e0` against the merged tip, per image, mean ms.
`trace_total` is the stopwatch mark after the trace crate returns (stages 2–5; set in
`run_color_impl`, `crates/inkvec-cli/src/pipeline.rs:247`); `fit_dp` is the fit (stage 6)
plus what `finish_color` does before it (`pipeline.rs:360-363`, the mark at `:484`):

| set | `fit_dp` | `trace_total` |
|---|---|---|
| 2048 px opaque (7 images) | 26.27 → 5.42 | 161.5 → 31.0 |
| 2048 px transparent (3) | 23.03 → 7.49 | 157.5 → 26.2 |
| 512 px (51) | 5.66 → 1.79 | 17.1 → 6.6 |
| 128 px (246 icons) | 1.00 → 0.46 | 4.29 → 2.48 |

## How it works

### 1. Intake

**What it computes.** Decode the file to straight RGBA floats, undo an exact
nearest-neighbour upscale, and matte transparency onto an opaque ground; then every stage
reads the image composited over white. Shared with Quality mode and documented in full in
`01-intake.md`; this section lists what the round changed. Since the robustness merge of
2026-10-03 (`d8a3650`) the shared intake also picks the decoder by the file's signature,
converts an embedded ICC profile to sRGB, turns the image upright by its EXIF orientation,
lets a decode that will be capped allocate more (`load.rs:8-26`), and refuses an image with
a zero side (`has_pixels`, `load.rs:255-272`); those steps are documented in
`01-intake.md`, and Fast takes them as Quality does.

**Method.**

* **One read of the file, one widening pass.** `load_image_capped` reads the file into
  memory once and decodes header and pixels from those bytes (`load_image_capped`,
  `load_file_bytes_capped`, `load.rs:392-433`). The bytes become floats through a 256-entry
  table, `UNIT[k] = k / 255` (`load.rs:282-296`), straight from the decoder's own buffer for
  8-bit RGB and RGBA, in parallel chunks from 256 × 256 pixels on (`from_dynamic`, `widen`,
  `load.rs:298-373`).
* **Unblock by the gcd of the change positions.** Only the factors that divide
  `gcd(w, h, every column and row where neighbours differ by more than 1/256)` get the block
  test; on ordinary art the gcd reaches 1 a few rows into the content and no block test runs
  (`pixel_grid`, `change_gcd`, `crates/inkvec-cli/src/alpha/unblock.rs:8-161`).
* **Matte in place, in parallel.** The transparency scan and the flatten are parallel maps
  from 256 × 256 pixels on, and the flatten writes over the input's own buffer
  (`alpha_source_owned`, `crates/inkvec-cli/src/alpha.rs:598-624`; `flatten_in_place`,
  `has_transparency`, `alpha.rs:792-852`).
* **Composite over white in parallel** (`Rgba::composited`, `coverage.rs:198-232`).

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
   (`key_rows`, `palette.rs:501-521`: the key is a pure function of the pixel's four floats,
   and 99.56% of pixels repeat their left neighbour's colour at 2048 px), then walks *key
   runs* counting pixels, paired and flat pixels and their f64 colour sums (`histogram_rows`,
   `palette.rs:568-633`): inside a run the horizontal half of the erosion comes from the
   run's ends, and only the rows above and below are read per pixel. A run's sums are held
   in registers and stored once, because adding each pixel into the bin's memory made every
   addition wait on the store before it. Row `r + 1` is keyed just before row `r` is
   counted, so the count reads pixels the keying has just brought into cache. The bands'
   histograms are merged at the end (`Bins::absorb`), which is exact by the lemma below.
   Rounding to a level is `trunc(x + (0.5 − 2⁻²⁵))`, identical to `f32::round` on every one
   of the 1,065,353,217 floats in `[0, 1]` for both grids and about twice as fast, because
   `f32::round` compiles to a libm call on the default x86-64 target (`level`,
   `palette.rs:174-209`).
2. **Inks** (`found_inks`, `palette.rs:857-893`; `thin_inks`, `:915-973`) from the table of
   occupied bins, then opacity snapping. Microscopic: at most 36 candidates and 36 inks
   measured.
3. **Lookup table:** each occupied bin's nearest ink, once, and whether the bin *is* that ink
   (`palette.rs:1213-1228`).
4. **Labels** (`label_rows`, `palette.rs:1318-1332`): a sure pixel's label is written inline
   from the table; only the others — 0.29% of pixels at 2048 px, 4.4% at 128 px — go to the
   neighbourhood rule (`Blends::blend_label`, `palette.rs:1037-1081`). Each ink's share is
   counted per run of equal labels, in integers, and converted to the f32 value the old
   per-pixel float count produced (`ink_shares`, `palette.rs:1377-1384`).

Per-bin state is kept for the *occupied* bins only (`Bins`, `palette.rs:211-307`): a median of
16 on the 128 px screen set and at most 610 on the 2048 px set, out of 65,536 keys. A key
reaches its bin through a small open-addressing hash table (`Slots`, `palette.rs:309-409`)
that starts at 64 entries and doubles as bins open, so each band's table is as large as the
bins it met. Below 256 × 256 pixels, and whenever rayon has a single worker (the
single-threaded WebAssembly build), every pass runs on the calling thread
(`PARALLEL_MIN_PIXELS`, `palette.rs:1119-1135`).

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
5. **Faces** (`write_faces`, `faces/runs.rs:493-534`, mark `split`): the components of the
   result are the faces; their ids are written over the label buffer. Ids are `u16`, and
   Fast numbers at most 65,534 faces (`cap = u16::MAX − 1`, `runs.rs:513`). Past that,
   since `be5abad` (merged at `d8a3650`, 2026-10-03), the label image is written out, its
   smallest components are merged into their neighbours until 65,534 remain, and the runs
   are read again before the ids are written. The merge is `regions::cap_components`
   (`regions.rs:293-416`), the same function Quality's `split_components` calls past its
   own limit of 65,535 (`MAX_FACES`, see `04-regions.md`). In rounds: take the
   `count − cap` smallest components (by size, then by the raster order of their first
   pixel), and in that order give each the neighbouring label it shares the most pixel
   edges with (the lower label on a tie), unless a neighbour of it was already relabelled
   this round or it was itself chosen as another's target. Each relabelling removes a
   component, so the rounds end; every face is then still one 4-connected component of one
   ink, the smallest specks drawn in their neighbour's ink. Before, every component past the
   limit was folded into face 0, which then held pixels of many inks under one colour. An
   image with fewer components takes exactly the old path. Tested on a 300 × 300
   checkerboard, 90,000 components (`faces/tests.rs:36-53`); the implementing branch
   (impl2/robust) found no real image that reaches the limit in Fast, whose palette and
   despeckle keep two-ink noise at 18,000 to 35,000 faces.

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

The labels are read once, when the runs are built, and written once, with the face ids
(past the face-id limit, once more: written out for the merge and read back); pixel colours
are read only at strip pixels, in place, never copied (`Pixels`, `faces.rs:119-153`).

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
pixels on demand, "Inspired by" Halide (PLDI 2013), store versus recompute; the merge past
the face-id limit (on `regions::cap_components`), "Method from" Haris, Efstratiadis,
Maglaveras & Katsaggelos, "Hybrid image segmentation using watersheds and fast region
merging", IEEE TIP 1998 (region merging on the region adjacency graph, smallest and most
similar first; adapted: the order is size alone, "most similar" is the longest shared
border, since the labels carry no colour there, and the stop is the face-id limit rather
than a dissimilarity threshold). "Not from the literature": the interior test and the
border lengths on runs, the observation that the component table plus the adjacency graph
with border lengths are sufficient statistics for every pass (colours are needed only at
strip pixels), and the strip criterion itself.
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

**What it computes** (`fast/bands.rs:1-32`, `merge_ramps`, `:318-500`). A smooth gradient
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

1. **Palette precheck** (`inks_may_join`, `bands.rs:156-219`): when no two *different* inks
   lie within `RAMP_STEP`, the pass returns before reading a pixel. It is a proof, not a
   guess: the faces are the 4-connected components of the ink map, so two faces that touch
   always carry different inks, and if every pair of different inks is farther apart than
   `RAMP_STEP` no join can happen and nothing would change. The proof holds past the face-id
   limit too: there `write_faces` relabels the smallest components with a neighbouring ink
   and reads the runs again (stage 3), so every face still has one ink. With 65,534 faces or
   more (`CAPPED`, `bands.rs:200`) the precheck returns `true` all the same and the full pass
   decides, as a conservative check only: it covers `write_faces`' fold of leftover ids into
   face 0, reached only if `cap_components` stopped short of the cap, which its proof rules
   out.
2. **Row runs** of the face map (`planar::runs::RowRuns`, shared with the planar map).
3. **Contacts** (`contacts`, `bands.rs:72-130`): the border length of every touching pair of
   faces, read off consecutive runs and the overlaps of adjacent rows' runs, keyed and
   summed after a sort; no join means return.
4. **Samples** (`gather_samples`, `bands.rs:254-316`): each cluster's grid sample, read off the
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

This section describes the fitter as it stands after the fitter round of 2026-09-30, which
changed how the polygon and the primitive test are computed and writes the image frame as
the image rectangle; "The fitter round of 2026-09-30", at the end of the section, lists each
change and how it was checked.

**Per edge, in parallel** (`fit_edges`, `fast/mod.rs:318-365`): the edges are fitted as a
rayon parallel map, output in edge order whatever the thread count. The image frame is
recognised first and written without fitting (below). Every other edge's tolerances
depend on the OKLab contrast between its two faces' representative colours (1 when either
face is outside the fills, at the image border): at or above `FAINT = 0.12` they are as
given; below, every distance tolerance but `flat` grows as `FAINT / contrast`, up to
`MAX_LOOSEN = 3`×, because where the two sides are close in colour a misplaced boundary costs
little; a boundary of a gradient face is fitted as if its contrast were at most
`FAINT / GRADIENT_LOOSEN = FAINT / 1.6`, because a fitted gradient is a coarser model of its
pixels than a flat ink and its boundary comes back rougher (`FastFit::for_contrast`,
`mod.rs:195-219`). The defaults (`FastFit::default`, `mod.rs:91-101`): polygon tolerance
0.5 px, vertex box 0.5 px, corner tolerance 0.25 px (Potrace's `alphamax` as a distance),
merge tolerance 0.2 px (Potrace's `opttolerance`), and 0.05 px for a cubic to count as a
line. A closed edge of 8 points or more drops its first point, the lattice node the
refinement leaves up to 0.6 px off the edge (`fit_edge`, `mod.rs:230-237`).

The steps follow the order of the module overview (`fast/mod.rs:43-50`): `fit_edges` →
`frame_rectangle`, then `fit_edge` → `prims::primitive`, then `fit_denoised`.

**First, the image frame** (`frame_rectangle`, `mod.rs:259-316`). When one face runs round
the whole image border — a transparent icon's clear ground, or a background colour — the
planar map traces the border as one closed ring on the lattice's outer nodes, which the
sub-pixel refinement leaves in place, so the ring *is* the rectangle
`[-0.5, w − 0.5] × [-0.5, h − 0.5]` (pixel centres at integers). A closed edge of at
least 4 points whose every point lies on that rectangle's border, and which passes each
of its four corners exactly once, is written as four lines through the corners, in the
ring's own direction, from the first corner the ring reaches: 8 parameters, `O(n)` to
recognise. The comparisons are exact, because the border nodes are exact binary
fractions the refinement never moves. Anything else — an open edge, a ring with a point
off the border, an inner rectangle — goes on to the fit.

**Then, for every other edge** (`fit_edge`, `fit_denoised`):

1. **Primitives** (`prims::primitive`, `prims.rs:99-242`), closed edges of at least 12
   points only, on the ring's denoised points, every point weighted with σ = 0.5 px: a
   cascade of cheap tests before expensive fits.
    * Kåsa's algebraic circle fit (`fit_circle_kasa`, one linear least-squares solve)
      rejects a ring whose radius is under 1.5 px or not finite, or that has a point more
      than `0.6 r` from the circle: not round at all.
    * When the Kåsa circle lies within 0.6 px (`2 · TOL`) of every point, the
      orthogonal-distance circle (`fit_circle`, Levenberg–Marquardt) is accepted, with 3
      parameters, if it lies within 0.3 px (`TOL`) of every point and the ring encloses at
      least 80% of `π r²`.
    * Otherwise an ellipse. A bound rules most rings out before any ellipse fit: with `span`
      the largest distance from the first point, an ellipse within `TOL` of every point has
      `rx ≥ span / 2 − TOL`, while the area test with `ry ≥ 1.5` px needs
      `rx ≤ |A| / (0.8 · π · 1.5)`; when the bounds cross, no ellipse can pass. Then Taubin's
      algebraic conic (`taubin_ellipse`) must lie within 0.6 px of every point, and the
      orthogonal-distance ellipse within 0.3 px, with both radii at least 1.5 px, the larger
      at most `span`, and the area test passed: accepted with 5 parameters. The algebraic
      conic is Taubin's geometry alone: the library's `fit_ellipse_algebraic` also computes
      the ellipse's orthogonal χ², a Newton foot-point solve per point that nothing here
      reads. Levenberg–Marquardt then starts from that conic alone (`fit_ellipse_seeded`,
      `crates/inkvec-fit/src/primitives/ellipse.rs:346-389`; `taubin_ellipse`, `:197-219`).
      The library's `fit_ellipse` also starts from four near-circles about the orthogonal
      circle (radii 1.02 r and 0.98 r, at 0°, 45°, 90° and 135°) and keeps the lowest χ² of
      the five; here those four run only when the algebraic start fails, reusing the circle
      the round test already fitted when it fitted one.

    The primitive is drawn as four cubics, the standard approximation of a quarter arc with
    control arms `k = 4/3 · tan(Δt / 4)`, starting on the ray from the centre through the
    ring's first point and running the ring's own way (`ellipse_cubics`, `prims.rs:47-97`).
    Rounded rectangles are left to the curve fit.

2. **Denoise** (`smooth::denoise`, `smooth.rs:51-105`): a `[1 2 1] / 4` binomial low-pass
   along the boundary, which removes the 0.1 px alternation the lattice leaves in refined
   points and moves a curve of radius `r` by only `1 / 4r` px; a point that turns sharply
   over two steps each way (more than 50°) is kept as a corner, and the ends of an open
   boundary, which are junctions, stay put. A ring is denoised once: `fit_edge` hands the
   points it denoised for the primitive test straight on to `fit_denoised`
   (`mod.rs:150-193`, `:221-257`), because `denoise` is a pure function of the points and
   the closed flag and a second call could only return the same vector.

3. **Optimal polygon** (`polygon::open`, `polygon.rs:250-392`; `polygon::closed`,
   `polygon.rs:638-652`; Selinger §2.2) — in the field's terms the *min-#* problem of
   polygonal approximation: the fewest straight sides that stay within `poly_tol` of every
   point, and among those the one closest to the points. A side `i → j` is admissible when
   the direction `p_j − p_i` lies in the *cone* of directions from `p_i` that pass within
   `tol` of every point between them (each point narrows the cone by `asin(tol / r)`;
   `Cone`, `polygon.rs:135-199`), no point has fallen back towards `p_i` by more than `tol`,
   and it spans at most `MAX_SPAN = 160` points. A dynamic program over vertices minimises
   `(sides, Σ squared distances)` lexicographically, the distances read in `O(1)` from
   prefix sums taken relative to the first point, so that the squares of large coordinates
   do not swamp the differences (`Sums`, `polygon.rs:69-133`). A closed ring is cut at its
   sharpest point and solved as an open run back to it. Three exact speed-ups, all keeping
   the vertex lists bit for bit those of the plain program, which the tests keep as
   `tests::open_ref`:
    * **Fathoming** (`polygon.rs:272-298`): branch and bound inside the dynamic program,
      with the side count as an exact integer bound. A side whose count `best[i].sides + 1`
      already exceeds `best[j].sides` cannot win at `j` whatever its penalty, so the penalty
      is not computed (89% of the admitted sides on the screen set, 55% at 2048 px, replay);
      an anchor with `best[i].sides ≥ best[n−1].sides` is not scanned, since every path
      through it ends with more sides than one the end already has (18.9% of the scan steps
      on the screen set start at such an anchor). The doc comment proves by induction over
      the index that every entry with fewer sides than the final count is decided by the
      same anchors through the same IEEE expressions in the same order, so the path is
      unchanged.
    * **Lattice runs in closed form** (`lattice_runs`, `polygon.rs:215-240`;
      `scan_anchor`, `polygon.rs:552-636`): 62–72% of the scan steps, and 98% of the image
      frame's, lie on a run of exactly equal steps, counted for every step in one backwards
      pass. Once the scan is under way on one — cone open, the last point more than `2 tol`
      from the anchor, the step at most `RUN_MAX_STEP = 4` px, `tol ≥ RUN_MIN_TOL = 1/16`
      px — every point to the run's end is admitted, and the cone and reach after it are the
      last point's alone, so the per-point cone work (a `hypot`, three divisions, a square
      root and four cross products on a loop-carried dependency) is skipped. The sides are
      still offered one by one, in order, because their penalties break ties by rounding.
      The proof (`polygon.rs:313-333`): rounding moves each run point's computed direction
      and cone bounds by less than 10⁻¹⁴ rad; the distances grow by `|s|` per point, so the
      reach test never fires; each point's cone is strictly tighter than the last's by at
      least 6·10⁻⁷ rad; every direction lies inside the previous point's cone by at least
      9·10⁻⁵, far beyond the admission slack of 10⁻⁹; and `r > 2 tol` keeps the cone within
      60°, so it never empties.
    * **Parallel admissibility** (`admitted_sides`, `polygon.rs:507-550`): which sides an
      anchor admits depends only on the points and the tolerance, never on the table. A
      boundary of `PARALLEL_MIN = 2048` points or more scans every anchor on all cores
      first, into a 160-bit set per anchor (`SideSet`, `polygon.rs:448-470`), then relaxes
      the table sequentially in anchor order, offering the same sides in the same order;
      the sequential scan's skipped cone tests are exactly the sides whose offer is a
      no-op, so the polygon is the same on any thread count. The price: anchors the table would have fathomed are scanned too,
      and 24 bytes per point. Such boundaries are rare — none of the 6,645 edges of the
      screen set, 16 of the 564 at 2048 px, which include the image frame and set the fit's
      wall time (replay). The set's bits are written by shift and mask, not `/ 64` and
      `% 64`, so that no remainder appears in a hot loop after wazero's arm64 `i32.rem_u`
      miscompile.

4. **Vertex adjustment** (`smooth::adjust_vertices`, `smooth.rs:209-253`; Selinger
   §2.3.1): each side gets the total-least-squares line through the points it spans, and
   each interior vertex moves to the point minimising its two sides' squared distances
   within a box of half-width `vertex_box` around its polygon position, with a small ridge
   (`RIDGE = 1e-3`) towards that position so parallel sides stay well posed. The ends of an
   open run stay exactly where they are.

5. **Smoothing into pieces** (`smooth::pieces`, `smooth.rs:314-432`; Selinger §2.3.2): one
   join per side, moved off the side's line to where the points are (by at most
   `JOIN_MAX = 0.5` px, since a curve drawn through the unmoved midpoints would shrink every
   round shape), and at each vertex the cubic from join to join, tangent to both sides, with
   its two arm lengths fitted to the points between the joins by Schneider's
   tangent-constrained Bézier fit (`curve::fit`, `curve.rs:79-173`) and capped so neither
   control point passes the vertex (Potrace's `alpha ≤ 1`). The vertex is a *corner* — two
   line pieces — when that cubic misses the points by more than `corner_tol` and by more
   than twice what the polyline through the vertex misses them by, or when the fit fails.

6. **Curve-run optimisation** (`curve::optimise`, `curve.rs:334-365`; `optimise_run`,
   `curve.rs:240-332`; Selinger §2.4, Potrace's `opticurve`): runs of smooth pieces between
   corners are merged where one cubic says the same thing. A closed boundary is first
   rotated to start at a corner, when it has one, so no run is cut in two where the ring
   happens to start; each maximal run of smoothly joined pieces is then optimised on its
   own, so a corner is never smoothed over. Within a run a dynamic program over piece
   boundaries takes the fewest cubics: `best[j+1] = min_i best[i] + 1`, over `i = j` (the
   piece alone) and every `i` for which pieces `i..=j` turn one way (a piece turning less
   than 10⁻⁶ rad counts as either), by at most `MAX_TURN = 3.10` rad in total, number at
   most `MAX_RUN = 24`, and merge into one cubic within `opt_tol` of every sample. The
   merged cubic keeps the run's end points and end tangents, and its two arm lengths are
   fitted (`curve::fit`) to samples of the pieces at `t = ¼, ½, ¾` and at every join
   between them. On a tie the smallest `i` is kept, and the cubics are read back from the
   last piece. `O(n · MAX_RUN)` candidate runs for `n` pieces, each merge linear in its run.

   Two rules, since `3513beb` (merged at `46e92e4`, 2026-10-03), keep every run drawn
   (`curve.rs:251-268`):
    * **A single piece is always its own cubic, whatever it turns.** The piece alone is
      admitted before the turn test, and only merges of two or more pieces are held to it.
      It used to be admitted only after the turn test, so a piece turning more than
      `MAX_TURN` — a U-turn at a vertex whose sides double back, |turn| ≈ π — left
      `best[j+1]` unreachable, and every later prefix with it; the read-back then started
      from an unset `best[n]` and returned the run's last piece alone as if it were the
      whole run. On a ring that is one cubic, which the fit then closes on itself, and the
      face is not drawn. The doc comment says why Potrace has no such failure and this
      program can: Potrace's polygon never doubles back, while here
      `smooth::adjust_vertices` moves the vertices off the points within loosened boxes.
    * **A run that is the whole ring is never one cubic.** A ring with no corner, or with
      one, is a single run that starts and ends at the same join, and one cubic with both
      ends there can only draw a loop or a sliver; the one candidate that covers every
      piece (`i = 0`, `j = n − 1`) is not tried, so a closed boundary of two or more pieces
      comes back as two or more cubics. A ring turns a full turn, so with honest turns
      `MAX_TURN` already rules that merge out; the guard holds where the measured turns do
      not add up (each is read in (−π, π], so a piece that turns further reads short).

   Where every piece turns at most `MAX_TURN` and no whole ring merges into one cubic —
   every boundary the old code drew correctly — the loop makes the same choices in the same
   order, so those fits are unchanged bit for bit. Measured by the implementing branch
   (impl2/bugs report: SVG hashes, and the gate's A/B of `3513beb` against the commit
   before it at the gate's 128ss and 512ss tiers): the fix changes the output of 9 of the
   402 gate icons at 128ss, 3 at 512ss, and the 1672 × 941 masthead. Gate dE00 at 128ss:
   screen 0.3640 → 0.3639 (3 better, 1 worse), held_a 0.3632 → 0.3630 (2 better, none
   worse); at 512ss: screen 0.1106, unchanged (1 better). Parameter ratio 2.120 → 2.121
   (128ss screen) and 3.192 → 3.193 (512ss screen). The largest changes:
   simple-icons/visa at 512 px, dE00 0.0479 → 0.0381, and openmoji 1F9D1-200D-1F393,
   −0.026. Off the gate, the masthead goes from 0.9051 to 0.9044 with 8441 → 8491
   parameters (thin rings that had collapsed are drawn again, by the commit message). The
   case that found it was `synthetic/gradient_radial` at 512 px traced in Fast with the
   research boundary solve switched on (research build r2-fastq): piece 37 of 38 turned
   −3.109 rad, the ring collapsed to one cubic, and dE00 was 10.62; with the fix built on
   that research commit it traces at 0.0587 (plain Fast 0.0586).

7. **Segments** (`curve::to_segments`, `curve.rs:382-416`): a cubic whose control points lie
   within 0.05 px of its chord, and project inside it, is written as a line; consecutive
   collinear lines are joined; the ends are pinned to the junctions exactly
   (`fit_denoised`, `mod.rs:177-187`).

Boundaries too short for a polygon (fewer than 3 points, or 4 for a ring), and any stage that
leaves nothing, are drawn as straight lines through their points (`too_short`, `lines`,
`mod.rs:120-148`).

**The fitter round of 2026-09-30** (branch `impl/fast-fit`, merged into `integ/fast` at
`e30fa8e`). Byte-identical changes: fathoming (`ebf9ecb`), lattice runs in closed form
(`0cb5017`), parallel admissibility (`b9660c6`), the primitive test without the unread χ²
and with its seeds reused (`37a4e40`), the ring denoised once (`d3ea154`), and the side
bits by shift and mask (`e183cac`). Outside the fitter's files, and inside the `fit_dp`
mark: `finish_color` moves the traced labels instead of cloning them, and Fast builds no
content-unit polylines or λ multipliers unless `--editability` asks for them (`9f290b6`;
`crates/inkvec-cli/src/pipeline.rs:361-363`, `:635-658`; see `01-intake.md`). Each rewrite keeps the code it
replaced as a test reference (`polygon::tests::open_ref` and `closed_ref`,
`prims::tests::primitive_ref`, `tests::fit_edge_ref`). Identity of the exact tip against
`main` `55ee4e0`: Fast 464/464 files, Quality 256/256, `--no-background` and `--monochrome`
60 each, `--editability` 30; and the polygons of all 11,021 replayed edges (21,072 polygon
runs, at each edge's own tolerance and at the loosest) equal to the reference at 1, 4 and
16 threads. Quality's own ellipse fit is unchanged: `fit_ellipse` now hands both of its
seeds to `fit_ellipse_seeded`, which given those seeds is bit-identical to it.

Two changes alter the output, and the gate (`--mode fast`) judged them:

* **The frame as the image rectangle** (`5520e6b`). Screen: dE00 0.3641 → 0.3640 (1 image
  better, 0 worse), worst tenth 0.9218 → 0.9218, DISTS 0.0571 → 0.0570, parameter ratio
  2.127 → 2.120. held_a: dE00 0.3632 → 0.3632, worst tenth equal, DISTS 0.0538 → 0.0538,
  parameter ratio 2.149 → 2.137. 12 of 253 screen and 2048 px outputs change, all opaque
  images with a background face: on a transparent icon the frame bounds the clear ground,
  which is not drawn, so only the fitting time goes.
* **The orthogonal ellipse from the algebraic start alone** (`d403d3b`). It changes the
  output wherever a near-circle start would have reached a lower χ², so it is not provably
  identical. On the gate 0 of 464 SVGs changed (screen, held_a, the 2048 px set, s512, the
  2048 px transparent set, the flat logo), and a unit test checks that the single start
  finds the same primitives as the five on 109 rings (circles, rotated noisy ellipses and
  the frame), to 10⁻⁶ px. It agrees with the library's own measurement: every one of the
  93 ellipses the Quality search chose on the screen set and a poster came from the
  algebraic start (`fit_ellipse_screened`). Kept as a speed-up.

The final tip was checked again on Quality: 256/256 identical.

After the round, one more fitter change alters the output, on purpose: the curve-run fix of
`3513beb` (step 6 above), judged by the gate on its own.

**Citations** (labels as in the doc comments):

* the optimal polygon, "Method from" Selinger, "Potrace: a polygon-based tracing
  algorithm", 2003, §2.2 (the optimal polygon as a shortest path, fewest sides then least
  squared distance, with capped spans); "See also" Imai & Iri, "Computational-geometric
  methods for polygonal approximations of a curve", CVGIP 1986 (min-# as a shortest path
  over the admissible sides), and Chan & Chin, "Approximation of polygonal curves with
  minimum number of line segments or minimum error", IJCGA 1996 (min-# in `O(n²)`; the span
  cap makes this program `O(n · MAX_SPAN)`); "Inspired by" the cone-intersection test of
  Williams, "An efficient algorithm for the piecewise linear approximation of planar
  curves", CGIP 1978, and Sklansky & Gonzalez, "Fast polygonal approximation of digitized
  curves", Pattern Recognition 1980, used here only to decide which sides are admissible.
  Not used, with the reasons in `polygon.rs:26-36`: the greedy scan-along fitters
  themselves (faster, but not min-#, so the polygon would change), Agarwal & Varadarajan,
  "Efficient algorithms for approximating polygonal chains", DCG 2000 (its subquadratic
  bound is for a different error metric), and an approximate multiresolution program
  (refuted by the Quality fitter's research for changing the output);
* fathoming, "Method from" Morin & Marsten, "Branch-and-bound strategies for dynamic
  programming", Operations Research 1976 (adapted to the lexicographic value, where the side
  count is an exact integer bound, so no relaxation has to be solved to get one);
* lattice runs: counting them, "Inspired by" VTracer's straight-run walker (visioncortex)
  and Freeman, "On the encoding of arbitrary geometric configurations", IRE Trans.
  Electronic Computers 1961 (here the runs are only looked up, not compressed, because the
  dynamic program must still see every point); the closed form, "Not from the literature"
  (the published speed-ups for straight runs change what is fitted), "Inspired by" VTracer's
  walker, which compresses runs before fitting instead, "See also" Debled-Rennesson &
  Reveillès, "A linear algorithm for segmentation of digital curves", IJPRAI 1995 (digital
  straight segments of any slope; on sub-pixel points only an exactly repeated step is a
  run, so the simpler test suffices). A tangential-cover bound on how far a scan can reach
  (Faure, Buzer & Feschet 2009) was measured and dropped: it would have saved at most 9.5%
  of the steps;
* parallel admissibility, "Inspired by" Brent, "The parallel evaluation of general
  arithmetic expressions", J. ACM 1974 (the part with no dependencies in parallel, the short
  dependent chain in order), scheduled by rayon's work stealing (Blumofe & Leiserson,
  "Scheduling multithreaded computations by work stealing", J. ACM 1999), which also lets
  the cores the per-edge loop leaves idle join in;
* the primitives, "Method from" Taubin, "Estimation of planar curves, surfaces, and
  nonplanar space curves defined by implicit equations with applications to edge and range
  image segmentation", IEEE TPAMI 1991 (the algebraic conic), Ahn, Rauh & Warnecke,
  "Least-squares orthogonal distances fitting of circle, sphere, ellipse, hyperbola, and
  parabola", Pattern Recognition 2001 (the orthogonal fit), and Halíř & Flusser,
  "Numerically stable direct least squares fitting of ellipses", WSCG 1998, for starting
  the orthogonal fit from the algebraic one alone, which they recommend as "a fast and
  robust estimator of a good initial solution" (their direct fit; Taubin's conic plays that
  part here); dropping the unread χ², "Not from the literature". Kåsa's circle fit and
  Levenberg–Marquardt are named in `prims.rs` without a citation;
* the image frame, "Not from the literature" (the border of the raster is known exactly,
  so there is nothing to estimate), "See also" Selinger 2003, which traces a bitmap's border
  like any other boundary; the ring denoised once, "Not from the literature" (it only
  removes a repeated computation);
* the curve-run optimisation (`curve::optimise_run`, since `3513beb`), "Method from"
  Selinger, "Potrace: a polygon-based tracing algorithm", 2003, §2.4 (`opticurve`: the
  fewest curves over runs of consistent convexity and less than a half turn); adapted:
  Potrace's own program has no read-back failure because its polygon never doubles back,
  while here the vertices are moved off the points by `smooth::adjust_vertices` with
  loosened boxes, and can;
* the smoothing stages (`smooth.rs`) and the tangent-constrained Bézier fit (`curve::fit`),
  which the round did not change, name their sources in the text without the labels:
  Selinger 2003 §2.3.1 and §2.3.2, and Schneider's Bézier fit with fixed end tangents (the
  research report cites Schneider's 1990 Graphics Gems code, `FitCurves.c`).

**Where the time went.** Before the round (fitter research, 2026-09-30, on `main`):
`fit_dp` took 1.01 ms per 128 px icon (16% of the engine's stages) and 26.75 ms at 2048 px
(14%); its wall time is the slowest edge's (0.81 ms and 23.2 of 23.8 ms). Replayed on one
thread, the polygon was 59% of the fitter at 128 px, 75% at 2048 px and 86% on the flat
logo, the primitives 21.8% and 15.2%. The image-frame ring alone was 31%, 38% and 57% of the
fitter's CPU and the slowest edge on 161 of 246 icons and 7 of 7 big images; its output was
always four lines plus one spurious cubic chamfering the start corner (6 parameters too
many, on 166 of 246 screen icons), because `fit_edge` drops the ring's first point, here the
corner. In the primitive test, the unread χ² was 5.0% of the fast fit's CPU on the screen
set (2.0% at 2048 px), most of it on the frame, which is roughly round and reaches the
ellipse test; the five-start ellipse fit was 13.9% (10.9% at 2048 px).

After, combined (all four branches, `main` `55ee4e0` against the merged tip; the table under
"The pipeline, in order"), `fit_dp` per image: 26.27 → 5.42 ms at 2048 px opaque (4.8×),
23.03 → 7.49 ms at 2048 px transparent (3.1×), 5.66 → 1.79 ms at 512 px (3.2×) and
1.00 → 0.46 ms at 128 px (2.2×). The fit branch measured alone against its own baseline
(merge notes): 26.2 → 6.1, 23.3 → 9.1, 5.74 → 1.96 and 1.00 → 0.47 ms on the same four
sets. By stage, on the replay, one thread:

* **the frame** is no longer fitted at all; the closed-form runs had already cut its
  polygon to a sixth of the other edges' (`0cb5017`);
* **the polygon stage**: fathoming took it from 381 to 211 ms on the screen set and from
  622 to 504 ms on the 2048 px set (`ebf9ecb`); the closed-form lattice runs, measured on
  top of fathoming under a different load, from 318 to 104 ms and from 467 to 201 ms
  (`0cb5017`). The module overview (`polygon.rs:54-58`) quotes the second pair for all
  three speed-ups together;
* **long boundaries** (2048 points or more, 2048 px set, smallest of 25 runs): the scan
  takes 52 ms on one thread and 11 ms on all cores, the sequential relaxation, which prices
  the 5.8 M live sides of the 12 M admitted, stays at 40 ms, and the polygon of those edges
  goes from 91 to 53–58 ms (`polygon.rs:521-524`). That doc comment counts 16 such edges in
  the set, and so does the one on `PARALLEL_MIN`, 16 of 564 (`polygon.rs:497-502`; it said
  34 until `542a0da` recounted them from the replay dumps);
* **the primitive test**: 59 → 15 ms on the screen set and 54 → 13 ms on the 2048 px set
  for the single ellipse start, and the whole `fit_edge` 202 → 158 ms and 227 → 175 ms at
  that commit (`d403d3b`);
* **inside `fit_dp`, outside the fitter**: the label clone was 1.28 ms at 2048 px and the
  polylines and λ multipliers 0.47 ms (`9f290b6`).

What is left on a long boundary is the sequential relaxation. Pricing every admitted side in
the parallel pass would take it off the critical path, but needs 8 bytes per admitted side
(96 MB on that set) and twice the pricing work, so it is not done (`polygon.rs:525-527`). At
128 px, `fit_dp` also pays the rayon pool start the palette no longer pays (stage 2).

### 7. Emit

**What it computes.** The SVG document, from the fitted paths, the faces' fills and the
palette: shared with Quality and documented in `13-emit.md`. Fast turns shape harmonization
off (`emit_options`, `crates/inkvec-cli/src/pipeline.rs:515-526`). An image traced with its
transparency keeps the native-alpha model: inks carry an opacity and the clear ground is an
ink of its own (`fast/front.rs:14-16`).

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
composite and the intake passes; 512 vertices for the refinement; 2048 points for the
polygon's parallel scan) chooses only the schedule, never the output. The polygon's
`RUN_MAX_STEP` and `RUN_MIN_TOL` only decide where the closed form for lattice runs applies,
which by its proof gives the same sides either way. Fast's face-id limit, 65,534 faces
(`u16::MAX − 1`, `faces/runs.rs:513`, and the same count as `CAPPED` in `bands.rs:200`), is
one below Quality's `MAX_FACES` (section 04); past it the smallest components are merged
by the rule the two modes share (`regions::cap_components`, stage 3).

## Failure modes and edge cases

- **A resampled input takes the slow histogram.** A `--max-dim` reduction or `--intake-scale`
  leaves box averages outside the exact set, so the palette's bands stop counting and the
  histogram is recounted serially in raster order. The result is still identical; only the
  speed is lost.
- **More than 65,534 components** are merged, smallest first, into the neighbouring ink each
  shares the most border with, until 65,534 remain (`write_faces` calling
  `regions::cap_components`, the rule Quality's `split_components` uses past its own limit;
  stage 3). Every face stays one connected component of one ink; the merged specks are
  drawn in their neighbour's ink. The ramp precheck steps aside at that count and the full
  pass decides. Before `be5abad` the extra components were folded into face 0, which then
  held pixels of many inks under one colour. The implementing branch found no real image
  that reaches the limit in Fast; a 300 × 300 checkerboard (90,000 components) does, and is
  the unit test.
- **A run that U-turns, or a whole ring of smooth pieces, is never collapsed.** Since
  `3513beb` a piece turning more than `MAX_TURN` is always its own cubic, and a closed
  boundary of two or more pieces always comes back as two cubics or more (step 6 of §6).
  Before, a run with such a piece was read back as its last piece alone; on a ring that is
  one cubic closed on itself, and the face was not drawn.
- **The rayon pool start moves, it does not vanish.** In the command-line tool the first
  parallel call spawns the global pool (0.37 ms median). With the palette serial at 128 px,
  `fit_dp` pays it instead (+0.35 ms at 128 px); the Studio and the WebAssembly Space keep a warm
  pool.
- **The image frame is written, not fitted, and the chamfer is gone.** Since `5520e6b` the
  ring that runs round the whole image border is written as the image rectangle, four lines
  and 8 parameters (`frame_rectangle`, `fast/mod.rs:259-316`), so the spurious cubic that
  chamfered its start corner (6 parameters too many, on 166 of 246 screen icons) no longer
  appears. The test compares coordinates exactly, which holds because the refinement never
  moves the lattice's outer nodes; a frame ring with any point off the border, or that does
  not pass each corner exactly once, falls back to the ordinary fit. Only opaque images with
  a background face change: on a transparent icon the frame bounds the clear ground, which
  is not drawn.
- **The single ellipse start is judged, not proven.** Levenberg–Marquardt now starts from
  Taubin's conic alone and tries the four near-circles only when that start fails
  (`d403d3b`). Where a near-circle would have reached a lower χ² the ellipse, or whether one
  is accepted at all, could differ; on the gate no SVG changed (0 of 464).
- **Two run encodings exist**: `planar::runs::RowRuns` (the planar map and ramps) and
  `fast/faces/runs.rs` (the clean-up). Unifying them, and letting `RunLabels::for_each_contact`
  feed the ramp contacts, are open opportunities, not defects.

## Environment overrides

Fast mode's own stages read no environment variable that changes what they compute. One
writes a diagnostic: with `INKVEC_DIAG` set, `write_faces` reports under the stage name
`split` how many components it merged past the face-id limit (`faces/runs.rs:517-520`).
The shared stages keep theirs (see `07-subpixel.md`: `INKVEC_SUBPXDBG` and
`INKVEC_DUMP_CONTOUR` make the refinement run serially, in order), and `INKVEC_TIMING`
prints the stage marks listed above.

Two test-only harnesses read variables, through `inkvec_core::env`. Both are compiled only
under `cargo test` (`replay` and `faces::tests` are declared `#[cfg(test)]`,
`fast/mod.rs:62-63`, `fast/faces.rs:660-661`), and their tests are `#[ignore]`d, so they run
only when asked for:

| variable | read in | what it sets |
|---|---|---|
| `INKVEC_FFD_DIR` | `fast/replay.rs:65-85` (`dump_files`); also `replay_timing`, `:326` | directory searched recursively for `*.ffd` edge dumps. Unset, the three replay tests print a note and return. The dump sets for `replay_timing` are its first-level subdirectories |
| `INKVEC_FFD_REPS` | `fast/replay.rs:168-172` (`reps`) | how many timed runs a replay keeps the smallest of: default 30 in `replay_long_edges`, 5 in `replay_timing` |
| `INKVEC_FFD_MIN` | `fast/replay.rs:187-199` (`replay_long_edges`) | fewest points an edge needs to be timed there; default 2048, the parallel scan's threshold |
| `INKVEC_FACES_DUMPS` | `fast/faces/tests.rs:476-517`, `:519-529` | directory of the clean-up's research label dumps (`*.bin`) for its differential test and its per-pass timing |
| `INKVEC_FACES_BENCH_MIN` | `fast/faces/tests.rs:529` | smallest dump, in pixels, the per-pass timing measures; default 1 (all) |

The replay tests are run as `INKVEC_FFD_DIR=<dir> cargo test --release -p inkvec-trace
replay -- --ignored --nocapture` (`replay.rs:5-7`). They compare the polygon with its kept
reference on every dumped edge, at the edge's own tolerance and the loosest
(`replay_polygon_matches_the_reference_on_every_dumped_edge`), time the long edges
(`replay_long_edges`), and time each stage per dump set (`replay_timing`: the whole
`fit_edge`, the primitive test, the polygon on the frame, on the other edges and through the
reference, and the sum of each image's slowest edge). A dump ("FFD1", little endian: an edge
count, then per edge a closed flag, the contrast and the points as f64 pairs) is written by
the phase-1 research build's `INKVEC_FFDUMP`, which is not in this tree.

## Open questions

- The per-stage cost of intake after the round was not measured separately from the palette.
- Fast's front end composites the image over white again (`fast/front.rs:82`); for an image
  the intake already matted (alpha 1 everywhere) that is an identity, about 9 ms at 2048 px
  by the merge notes. Passing the intake's over-white plane through would remove it; this is
  a proposal, not measured as a change.
