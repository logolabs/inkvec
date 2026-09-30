# Stage 07 — Sub-pixel refinement

> Moves every boundary point from an exact pixel-grid corner to the sub-pixel position
> the image actually supports, and attaches an honest uncertainty to each one.

**Source:** `crates/inkvec-trace/src/planar.rs`, `crates/inkvec-trace/src/planar/junctions.rs`
**Entry points:** `fn refine_subpixel_alpha()` (`planar.rs:468-477`; `refine_subpixel`,
`planar.rs:445`, is its opaque form), which is `measure_subpixel` (`planar.rs:528-603`)
followed by `Refined::apply` (`planar.rs:487-500`), stage mark `"refine_subpix"`; and
`fn refine_junctions()` (`planar/junctions.rs:340`, stage mark `"refine_junc"`)
**Pipeline position:** after `build_map`, before `boundary_opt`. The measuring phase runs
side by side with symmetry detection under `rayon::join` (`lib.rs:1093-1127`); both read the
lattice map and neither writes it, and the measured points are written back afterwards.
Shared by Quality and Fast mode.

## What problem this solves

`planar::build` (stage 06) fixes topology exactly but leaves every point sitting on an
integer pixel-grid corner — the boundary as marching squares over the *label* image sees
it, not as the anti-aliased *image* sees it. This stage moves each point to where the
image says the boundary actually is, without ever changing which faces are adjacent
(that was already decided, immutably, in stage 06).

The governing idea, stated in `coverage.rs`'s module doc: an anti-aliased pixel is not
noise to threshold away, it is "a *measurement* of how much of that pixel the shape
covers" (`coverage.rs:3-4`). For a boundary between two colours `F` and `B`, the observed
pixel `P = a*F + (1-a)*B` inverts to a coverage value `a`, and the boundary itself is the
`a = 0.5` level set of that field (`coverage.rs:10-18`, `contour.rs:3`). `refine_subpixel`
locates that level set along the local boundary normal at every point; `refine_junctions`
handles the points where the two-colour model underlying that inversion breaks down.

## Inputs and outputs

```rust
pub fn refine_subpixel(
    map: &mut PlanarMap,
    rgb: &[[f32; 3]],
    face_fill: &[gradient::FillModel],
    sigma_noise: f64,
    simplify_faint: bool,
)
```

Replaces every open and closed edge's `points` and `sigma`. `face_fill[f]` is the
fill model for face `f` (`Flat` or a gradient) — not the palette entry, because several
faces can share one ink and a gradient face has no single palette colour at all
(`planar.rs:718-720`). `refine_subpixel_alpha` takes the same arguments plus the source
alpha and each face's opacity, for unmixing in four channels on a transparent image.

Since 2026-09-30 the work is split in two phases:

```rust
pub(crate) fn measure_subpixel(map: &PlanarMap, /* same inputs */) -> Refined
impl Refined { pub(crate) fn apply(self, map: &mut PlanarMap) }
```

`measure_subpixel` reads the map and returns, per edge in edge order, the moved points and
their sigmas, or `None` for an edge the refinement leaves as it is (one side outside the
image, or a face without a fill model); `apply` moves those vectors into the map, `O(edges)`.

```rust
pub fn refine_junctions(map: &mut PlanarMap)
```

Mutates every junction node's shared endpoint across all edges that meet there, so every
edge sharing a node keeps bit-identical coordinates afterwards (`planar.rs:1029-1031`).

## How it works

### `refine_subpixel`: per-point normal search

For each edge point `p` (`planar.rs:427-699`):

1. **Local tangent.** Estimated from neighbours `window` points to either side
   (`INKVEC_SUBPX_WIN` (*removed*), default 1 — see Environment overrides), unless the turning angle
   between the two chords exceeds `CORNER_COS` (60°, `planar.rs:367`), in which case the
   narrow one-point window is used instead so a wide window does not smear a real corner.
   The normal is perpendicular to the tangent.
2. **Unmix colours.** `gradient::unmix_pair(fa, fb, p.x, p.y)` (`gradient.rs:1222`)
   returns the two colours to project against and their separation `contrast`. If
   `contrast` is below `min_contrast = max(3*sigma_noise, MIN_UNMIX_CONTRAST)`
   (`planar.rs:337, 377`), the point is left on the grid at placeholder sigma `0.5` — there
   is nothing to unmix across.
3. **Sample coverage along the normal.** `alpha(x, y)` bilinearly samples the source
   image and projects onto the unmixed colour axis (`planar.rs:379-406, 466-470`), giving
   a 1-D coverage profile along the normal.
4. **Classify the profile and invert it.** Two inversion strategies:
   - **Step inversion** (`edge_offset`, `planar.rs:505-517`): closed-form inversion of the
     *exact* half-plane coverage of a unit square, used when the profile is `step_like` —
     both flat fills, monotone (not a one-pixel-wide ridge), saturating on both sides
     within the probe span, and contrast at least `2 * min_contrast`
     (`planar.rs:616-629`).
   - **Root-find** (`planar.rs:630-647`): a 9-step scan for the `0.5` crossing along the
     normal, `[-1, 1]` pixels either side of `p`, used otherwise — including at any
     corner, at any gradient boundary, or wherever the profile is not step-like.
5. **Positional uncertainty.** Combines statistical uncertainty
   `sigma = (sigma_noise / contrast) / |grad alpha|` with the level-set extraction's own
   resolution limit (`crate::coverage::DEFAULT_SIGMA_MODEL`, `= 0.05`, `coverage.rs:39`)
   in quadrature, floors it at `crate::contour::sigma_floor()`, applies curvature
   inflation (`crate::contour::inflate_for_curvature`, shared with the bilevel front end —
   see stage 08/09 docs for that mechanism), and clamps to `[0.02, 2.0]`
   (`planar.rs:658-699`).

### Measure, then apply: the parallel schedule

The per-point search above reads only the image, the two faces' fills and its own edge's
*original* points (its lattice neighbours, for the normal), and writes only its own
result: it is a pure map over vertices. The curvature correction of a vertex's sigma then
reads the edge's *moved* points, all of which are known by then: a pure map again. So since
2026-09-30 both run as parallel maps (`measure_subpixel`, `planar.rs:528-603`;
`refine_edge`, `planar.rs:697-759`):

- **edges in parallel**, and, inside an edge of at least `PAR_VERTICES = 64` points
  (`planar.rs:502-506`), **vertices in parallel too**, in chunks of at least 64, so a task
  is at least about 30 µs of work (0.44 µs per vertex measured at 2048 px) against rayon's
  few-µs cost per split. The second level matters because one edge can hold most of an
  image's vertices: longest edge 8,192 points, median edge 450, on the opaque `big` set;
- each result goes to its own slot of an indexed output (rayon's `collect` and `unzip` over
  an indexed iterator keep positions);
- the measuring phase does not write the map, so no vertex can see another's moved
  position. The serial loop could not either: it wrote an edge's points only after the whole
  edge was measured.

**Why the output is bit-for-bit the serial loop's.** Every vertex is computed from the same
inputs by the same operations; scheduling changes only *when* a value is computed. No value
is combined across vertices or threads — there is no parallel reduction, so no
floating-point sum whose rounding depends on how the work was split. "Method from"
Blelloch, Fineman, Gibbons & Shun, "Internally deterministic parallel algorithms can be
fast", PPoPP 2012: a parallel loop whose iterations are independent and write disjoint,
indexed outputs computes the serial result on every schedule; adapted to two nested levels
with a minimum task size. The doc comment names Demmel & Nguyen, "Fast Reproducible
Floating-Point Summation", ARITH 2013, as the related work behind the no-reductions rule;
nothing here sums across vertices, so no reproducible summation is needed
(`planar.rs:537-567`). The serial loop is kept as `refine_serial` in
`planar/refine_tests.rs`, and `parallel_refinement_equals_the_serial_loop` compares points
and sigmas bit for bit on anti-aliased scenes with flat and gradient fills, with and without
the alpha channel, down to one pixel and one row.

**When it stays serial.** Three cases run everything on the calling thread, in edge and
vertex order, as before:

- a small map: fewer than `PAR_MAP_VERTICES = 512` boundary vertices in all
  (`refine_in_parallel`, `planar.rs:508-526`). Waking rayon's sleeping workers only pays
  when there is enough to share; measured per icon (refinement and symmetry detection, the
  minimum of 3 interleaved runs, serial / parallel, ms): 512–1,024 vertices 0.263 / 0.206
  (16 screen icons), 1,024–2,048 0.506 / 0.253 (187), 2,048–4,096 0.998 / 0.351 (43),
  4,096–8,192 2.162 / 0.627 (37 icons at 512 px), over 8,192 4.300 / 0.930 (10). The
  parallel form already wins in the smallest bucket measured, so the cutoff sits at its lower
  end; 512 vertices are about 0.23 ms of serial work;
- `INKVEC_SUBPXDBG` set: it prints a line per vertex, which must come out in order;
- `INKVEC_DUMP_CONTOUR` set: it appends a block per edge to a file, which must keep edge
  order. The path is now read once per map (it was read per edge).

The same `refine_in_parallel` test decides whether symmetry detection runs beside the
measuring phase under `rayon::join` or before it (`lib.rs:1119-1125`); on a single-thread
build (WebAssembly) `join` runs the two in turn with no work added. Measured at 2048 px
before the change, the refinement took 10.6 ms, serial, at 0.44 µs per vertex (shared-stage
research, 2026-09-30). The shared stages that round rewrote (the ramp pass, the planar map
and this refinement, plus the face-alpha pass on transparent images) went from 42 to 13 ms
per 2048 px opaque image and from 130 to 21 ms per transparent one, output byte-identical
in Fast and Quality; Quality gains the planar-map and refinement part too.

### The exact inversion, and why it replaced a biased root-find

The step-inversion path (`edge_offset`) is the more heavily documented piece of this
function, because it replaced an earlier, biased method, and the doc comments record the
measurements that justified each change:

- **The original bug.** Root-finding on a *bilinear interpolation between pixel centres*
  systematically pulled every edge towards the `.0`/`.5` grid: "interpolating from a pure
  neighbour (alpha 0) to the partial pixel (alpha 0.67) puts the 0.5 crossing at 0.25 of
  the way, not 0.33" — measured 0.19px off on straight runs, 2.5% of ink missing on a
  Material icon whose edges fall at multiples of 5.33px (`planar.rs:483-487`).
- **The chord-length fix.** Coverage changes with the chord length the normal cuts through
  a pixel per unit of edge movement (`1` on an axis, `sqrt(2)` on a diagonal), so the edge
  sits at `(alpha - 0.5) / chord` from the centre, with `chord = 1 / max(|nx|, |ny|)`
  (`planar.rs:488-492`). Dividing by `max` rather than the alternative was itself measured:
  the wrong denominator "pushed every diagonal and curved edge out by up to 2x: rounded
  outlines came back uniformly fat" (`planar.rs:492`).
- **The quadratic correction.** That linear rule is exact only for an axis-aligned edge.
  At 45°, a pixel's coverage curve is quadratic throughout, and the linear rule put a
  90%-covered pixel's edge at 0.57px from centre where the truth is 0.39 — "slanted
  strokes came out fat by 0.1-0.2px" (`planar.rs:493-497`). `edge_offset` therefore
  inverts the *exact* half-plane coverage of a unit square rather than a linear
  approximation (`planar.rs:505-517`).
- **Bracketing, not averaging.** Only the two probes that straddle the `0.5` crossing are
  used; averaging in a third, already-saturated probe was measured to pull a point half a
  pixel into a stroke under two pixels wide, because the far probe's partial coverage came
  from the stroke's *other* edge (`planar.rs:556-560`).
- **Why the root-find survives at all.** A soft boundary (two gradient bands, a shadow
  edge) is a ramp, not a step, and reading it as a step "throws the point up to a pixel
  off" (synthetic gradients 0.15 → 0.35 dE00 when the step inversion was applied there,
  `planar.rs:596-599`). A one-pixel stroke is a ridge with background on both sides, and
  reading a ridge as a step likewise throws the point out (synthetic `thin_features` 0.91
  → 3.00 dE00, `planar.rs:602-604`). The `monotone` test excludes both (`planar.rs:
  605-610`), and `both_flat` additionally excludes gradient boundaries, where the
  unmixing colours are only the model's local *prediction* and a small model error
  misplaces an inverted edge (colour families +0.015 dE00 when the inversion was applied
  there too, `planar.rs:611-615`).

Two switches this history left behind are recorded as no longer switchable:
`INKVEC_SUBPX_MODE=inv` (*removed*) (force step-inversion everywhere) and `=root` (force root-find
everywhere) were both measured worse than the classifier and are dead code paths in
intent even though the comments describing them remain (`planar.rs:552-554, 620-622`).

### Corpus intake and 8x supersampling

`refine_subpixel` has no notion of supersampling: `planar.rs`, `coverage.rs` and
`contour.rs` contain no reference to an 8x factor or corpus intake. The 8x supersampling
is entirely a property of the benchmark's corpus generator, not of this stage: the corpus
builder renders each ground-truth SVG at `SUPERSAMPLE = 8` times the target tier and
box-filters it down to an ordinary raster before Inkvec ever sees it (`bench/build_corpus_v2.py:61-66`),
and the case suite's own rasteriser copies the same method for the same reason
(`bench/cases/_common.py:50-53, 113-119`). By the time `refine_subpixel` runs, the image
is just a pixel grid like any other; it treats it at face value regardless of how that
grid was produced.

The one *runtime* mechanism that reasons about raster resolution is
`coverage::intake_scale` (`coverage.rs:337-381`), which measures how many raster pixels
one unit of genuine edge detail occupies and is used upstream (in
`trace_color_full_with_alpha`, `lib.rs:282-297`) to decide whether the *palette's* noise
guard should widen its same-ink tolerance — not to change anything in `refine_subpixel`
itself. `refine_subpixel` runs identically regardless of `intake_scale`.

### `simplify_faint`: spending coordinates where they can be seen

Positional uncertainty at each point is inflated below a reference contrast when
`--simplify-faint` is set (`planar.rs:667-699`):

```rust
const CONTRAST_REF: f64 = 0.25;
const MAX_INFLATION: f64 = 4.0;
let visibility = if simplify_faint {
    (CONTRAST_REF / contrast.max(1e-6)).clamp(1.0, MAX_INFLATION)
} else {
    1.0
};
```

This follows directly from the `sigma = sigma_pixel / (|F-B| * |grad a|)` relation in
`coverage.rs:30` — contrast (`|F-B|`) already appears in the denominator of the
*statistical* uncertainty term computed a few lines above (`s = (sigma_noise / contrast) /
g`, `planar.rs:666`), so a faint boundary is already measured with wider uncertainty than
a crisp one. `simplify_faint`'s inflation is a second, independent adjustment layered on
top, and the doc comment is explicit about why it is a separate knob rather than folded
into the physical sigma:

> "the *statistical* uncertainty... on clean art it is swamped by the method's own
> resolution limit — measured on two identical wavy edges, one at 90% contrast and one at
> 7%, the tracer spent 100 and 103 segments. That is the right answer to 'where is the
> boundary' and the wrong answer to 'how much description is this boundary worth': the
> error a viewer sees is the position error times the contrast across it, so at a tenth of
> the contrast a coordinate buys a tenth of the visible accuracy." (`planar.rs:670-676`)

In other words: the physical sigma answers *how precisely is this point located*;
`simplify_faint`'s visibility multiplier answers *how much does mislocating it matter*,
which is a different question — a faint edge can be located just as precisely as a sharp
one (if the noise is low) and still not be worth many coordinates, because the viewer
cannot see the error either way. This is principled model selection under the project's
description-length objective (`cost = 0.5*chi2 + lambda*params`): inflating sigma lowers
`chi2`'s sensitivity to that point, which is exactly the lever that should move when a
point's fidelity is worth less.

It ships **off by default**, and the doc comment is candid about why:

> "the corpus disagrees with the argument: on the 246-icon screen set it saves 0.3% of the
> parameters and costs 0.2% of the colour error, objective 0.3992 -> 0.4023. Those icons
> are high-contrast art where the faint case barely arises, and the metric sees the loss
> and not the gain — the same blindness that keeps `--cutout` off." (`planar.rs:682-687`)

### `refine_junctions`: why a junction cannot be refined like an edge point

`refine_subpixel` cannot localize a junction, because the pixel there is a mixture of
three or more colours and the two-colour unmixing it relies on is not defined
(`planar.rs:1005-1006`). Left alone, each incident edge's endpoint would be moved along
its own normal independently, so "the copies of a shared vertex disagreed with each other
and all of them still sat within the half-pixel neighbourhood of the grid corner. Every
face touching the junction was pulled to the wrong place by up to half a pixel."
(`planar.rs:1007-1009`)

`refine_junctions` instead estimates the junction as the point where the incident
boundaries meet, in two stages:

1. **`end_tangent`** (`planar.rs:786-873`) fits each incident edge's end tangent as a
   line, from its already-refined interior points nearest the junction — skipping the
   `junction_skip()` points closest to the junction itself (1 point, `planar.rs:738`),
   because that region is contaminated by the three-way mixture, and using up to
   `junction_curvature_points()` (16, `planar.rs:742`) points, weighted by `1/sigma^2`
   from stage 07's own uncertainty estimates. It reports both the line and the variance of
   its position at the junction — inflated by the reduced chi-square when the points are
   not actually collinear (`fit_end_polynomial`, `planar.rs:884-960`). A straight-line
   extrapolation of an arc is systematically biased outward by about `t_mean^2 / 2R`; when
   the curvature term is statistically significant (`CURVATURE_SIGNIFICANCE = 3.0`,
   `planar.rs:886`) with enough points (`MIN_QUADRATIC_POINTS = 5`, `planar.rs:885`), a
   quadratic model is fitted instead, decided on the whole window (where curvature is best
   determined) but extrapolated from the nearer `junction_fit_points()` (6,
   `planar.rs:730-733`) points.
2. **`solve_junction`** (`planar.rs:1389-1429`) intersects those lines by weighted
   least-squares — closed form `p = (sum w_i n_i n_i^T)^-1 (sum w_i n_i c_i)`, `w_i = 1 /
   var_i` — but refuses when fewer than two lines are usable, when the lines are too
   nearly parallel to localize a crossing (`JUNCTION_MIN_CONDITION = 0.02`, roughly 16°,
   `planar.rs:757`), or when the solution would move the node further than
   `JUNCTION_MAX_MOVE = 1.5` px (`planar.rs:753`).

Where the boundaries meet **tangentially** rather than transversally, intersection is not
merely imprecise — it is ill-posed, with condition number `1/sin(theta)`
(`taper.rs:6-9`). `taper_junction` (`planar.rs:1253-1385`) handles that case instead, by
fitting a circle tangent to the "through" boundary from the branching boundary's own
points (see `crate::taper`, and stage-08-adjacent material for the geometry). The ordering
between the two is deliberate and itself the subject of a measured regression:

> "The intersection first, and the taper only where it declines... Running the taper
> first instead lets it overrule intersections that were perfectly well posed, and
> measurement says that is not free: it relocated 1129 junctions over 180 images and cost
> colour accuracy on 75 of them." (`planar.rs:1063-1069`)

When a taper *does* fire, it can slide the junction along a boundary and past points that
boundary's edge list still holds; `trim_passed_over` (`planar.rs:1136-1193`) removes them
to prevent the polyline from folding back through itself, which the doc comment reports
as the taper's entire net parameter cost across the 180-image measurement
(`planar.rs:1122-1125`).

## Constants and thresholds

| name | value | controls | derivation |
|---|---|---|---|
| `MIN_UNMIX_CONTRAST` (`planar.rs:414`) | `0.02` | floor on unmixing contrast below which a point is not moved at all | no stated derivation |
| `CORNER_COS` (`planar.rs:424`) | `0.5` (60°) | turning angle above which the tangent window narrows to 1 point | stated: "A staircase at any slope turns by at most 45 degrees between chords two points long, so slanted edges stay smooth" (`planar.rs:421-423`) — a geometric bound, not a sweep |
| `PAR_VERTICES` (`planar.rs:506`) | `64` | fewest points an edge needs before its vertices are refined in parallel, and the smallest chunk of a long edge one thread takes | motivated by a measured cost: a task of at least ~30 µs (0.44 µs per vertex at 2048 px) against rayon's few-µs cost per split (`planar.rs:502-505`); chooses only the schedule, never the result |
| `PAR_MAP_VERTICES` (`planar.rs:520`) | `512` | fewest boundary vertices in the whole map for the refinement (and symmetry detection beside it) to use threads at all | measured: per-icon serial / parallel timings by vertex count, parallel already ahead in the smallest bucket (512–1,024: 0.263 / 0.206 ms), so the cutoff sits at its lower end (`planar.rs:508-519`); chooses only the schedule, never the result |
| `INKVEC_SUBPX_WIN` (*removed*) (`planar.rs:353-362`) | default `1`, range `1..=8` | width of the tangent-estimation window | measured trade-off: widening to 2 improved dE00 0.2663→0.2534 on a 620-icon subset but cost DISTS 0.0418→0.0435 and caused a face-order regression on one icon (0.23→3.20 dE00); default left at 1 "until that is understood (LOG-43)" (`planar.rs:432-442`) |
| `DEFAULT_SIGMA_MODEL` (`coverage.rs:39`) | `0.05` px | irreducible resolution limit of level-set extraction, added in quadrature to statistical noise | stated: "Measured on analytic circles..., level-set extraction lands within roughly 0.05px" (`coverage.rs:59-60`) |
| `CONTRAST_REF` (`planar.rs:688`) | `0.25` | reference contrast for `simplify_faint`'s inflation | no stated derivation |
| `MAX_INFLATION` (`planar.rs:689`) | `4.0` | cap on `simplify_faint`'s sigma multiplier | no stated derivation |
| `JUNCTION_MAX_MOVE` (`planar.rs:753`) | `1.5` px | reject an intersection solution beyond this move | tried at 0.5 px diagonal first and found to make results worse (one test phase 0.178px → 1.006px residual) because it rejected genuinely helpful corrections; "the real limit is elsewhere" (`planar.rs:746-752`) — the current value has no positive derivation of its own, only a record of a rejected alternative |
| `JUNCTION_MIN_CONDITION` (`planar.rs:757`) | `0.02` | minimum eigenvalue ratio admitted for a junction intersection | stated as equivalent to rejecting crossings below about 16°, from `tan^2(theta/2)` (`planar.rs:754-756`) |
| `junction_fit_points()` (`planar.rs:733`) | `6` | interior points used for the near-junction extrapolation | no stated derivation |
| `junction_skip()` (`planar.rs:738`) | `1` | points nearest the junction excluded from the tangent fit | stated: the pixel there is a three-way mixture the two-colour unmixing does not know about (`planar.rs:736-737`) |
| `junction_curvature_points()` (`planar.rs:742`) | `16` | points used to test for significant curvature | no stated derivation |
| `MIN_QUADRATIC_POINTS` (`planar.rs:885`) | `5` | minimum points before a quadratic tangent model is tried | no stated derivation (a plausible minimum for a 3-parameter fit, but not stated as such) |
| `CURVATURE_SIGNIFICANCE` (`planar.rs:886`) | `3.0` | sigma threshold for preferring a quadratic over a linear tangent | no stated derivation (a conventional "3-sigma" significance bar) |
| `TAPER_DEGREES` (`planar.rs:1203`) | `55.0` | how far from the through-direction a branch may lie and still be tested as a taper | stated as deliberately loose, because the angle is measured *at the misplaced junction* and is inflated by the very error being corrected — on the motivating rounded square, 31° was observed for a boundary that is truly tangent, so a 20° gate rejected the case it was written for (`planar.rs:1198-1202`) |
| `TAPER_MAX_MOVE` (`planar.rs:1208`) | `8.0` px | largest move a taper estimate may make | stated as "generous — the error it corrects is several pixels — but not unbounded" (`planar.rs:1205-1207`), no numeric derivation |
| `TAPER_MAX_CONSUMED` (`planar/junctions.rs:387`) | `0.35` | fraction of the shortest incident boundary a taper move may consume | no derivation comment at the current declaration; an earlier revision carried one describing the motivating failure ("a single 5.3px move on a leaf icon left thirteen rings self-crossing... 78 segments became 355"), but it was dropped when this constant moved into `planar/junctions.rs` during the module split and has not been restored |
| `TAPER_MAX_SIGMA` (`planar.rs:1224`) | `1.0` px | largest standard error a taper estimate may carry and still be preferred | stated: "At a pixel of standard error the estimate has stopped being an improvement on what it replaces" (`planar.rs:1221-1223`) |
| `TRIM_MIN_POINTS` (`planar.rs:1112`) | `4` | fewest points an edge keeps after trimming passed-over points | stated: "Below this it has no shape left to fit" (`planar.rs:1111`) |
| `TRIM_LOOK` (`planar.rs:1116`) | `3` | how far ahead to look when deciding an edge's direction of travel | stated: "Far enough not to be reading one pixel of staircase" (`planar.rs:1114-1115`) |
| `MIN_WIDTH` / `MAX_WIDTH` (`taper.rs:98, 102`) | `0.05` / `6.0` px | taper sample admission band | stated: below `MIN_WIDTH` the measurement is noise-dominated; beyond `MAX_WIDTH` the tapering region is no longer bounded by a single arc (`taper.rs:96-101`) |
| `MIN_SAMPLES` (`taper.rs:105`) | `4` | fewest taper samples trusted | no stated derivation |
| `MAX_DEFECT` (`taper.rs:115`) | `0.15` px | largest tangent-circle residual admitted | calibrated against measured cases: a true taper measured 0.05px, two boundaries actually crossing measured 0.21, a constant-width strip measured 1.33 (`taper.rs:110-114`) |

## Failure modes and edge cases

- **`refine_subpixel`'s misclassification cost is asymmetric and recorded exactly once
  each way.** Every branch of the `step_like` classifier (`planar.rs:623-629`) exists
  because the *other* choice was tried and measured worse on a named case (gradients,
  thin features, script wordmarks — see How it works above). There is no held-out ablation
  covering all combinations at once.
- **A two-pixel saturation requirement was tried and rejected**, even though it "carried
  the thin-feature case" better in isolation: "objective 0.6703 vs 0.6816 but dE00 +0.004
  and parameters +9%: it starved the very edges the inversion is for" (`planar.rs:
  618-620`).
- **`refine_junctions` ordering hazard.** Running the taper before the intersection was
  tried and found to overrule perfectly well-posed intersections, costing accuracy on 75
  of 180 images even though the fits it overrode were not individually wrong — "relocations
  with identical move, sigma and residual differed in whether they hurt — so it could not
  be filtered out by demanding better evidence. It is filtered out by not asking the
  question where the existing method has a good answer." (`planar.rs:1066-1071`)
- **Applying intersection refinement's re-cut logic to the taper case (or vice versa) is
  explicitly not symmetric**: "Only a taper slides the node *along* a boundary... An
  intersection refinement moves it across the boundaries instead... and re-cutting there
  discards good measurements: applied to both, it made the untouched baseline worse."
  (`planar.rs:1078-1081`)

## Environment overrides

Since the settings cleanup (CHANGELOG, *Unreleased*) the engine reads its environment through one helper (`inkvec_core::env`): a switch is off when unset, empty or `0`, and every variable is read once per process. Variables marked *removed* below are gone (their defaults are constants now); those marked *research build* are read only by a binary built with `--features research`. The full list, with what is left and why, is [`docs/internal/env-vars.md`](../internal/env-vars.md).

| variable | effect |
|---|---|
| `INKVEC_SUBPX_WIN` (*removed*) | tangent-window width for `refine_subpixel`, `1..=8`, default `1` (`planar.rs:356-360`) |
| `INKVEC_SUBPXDBG` | per-point debug trace of the subpixel search; read once per map into the refinement context (`planar.rs:588`), and while set the whole refinement runs serially, in edge and vertex order, so the lines come out as before (`planar.rs:591`) |
| `INKVEC_SIGMA_FLOOR` (*removed*) | overrides `contour::sigma_floor()`, shared with the bilevel front end (`contour.rs:320-329`) |
| `INKVEC_CURV_GAIN` (*removed*) | overrides `contour::curv_gain()` used by `inflate_for_curvature` (`contour.rs:231-243`) |
| `INKVEC_DUMP_CONTOUR` | appends every refined edge's points and sigmas to a file, for offline study of extraction error structure (`dump_contour`, `planar.rs:769-781`); the path is read once per map (`planar.rs:589`; it used to be read per edge), and while it is set the refinement runs serially so the file keeps edge order |
| `INKVEC_NO_TAPER` | disables `taper_junction` when set to a non-empty value (`planar.rs:1260-1269`) — cached in a `OnceLock`, and the doc comment records a real bug where an *empty* value used to be treated as "set": "a shell that exports `VAR=` should not silently turn the estimator off, which it did once here and made an A/B compare a binary to itself" (`planar.rs:1261-1262`) |
| `INKVEC_TAPERDBG` | per-node taper fit debug trace; read once per process into a `OnceLock` (`taperdbg`, `planar/junctions.rs:66-70`) |
| `INKVEC_TAPER_SKIP=<n>` (*removed*) | skips the n-th accepted taper relocation, to attribute a corpus-level change to one junction (`planar.rs:1350-1357`) |
| `INKVEC_JDBG` | debug trace for `end_tangent`'s polynomial fit and `refine_junctions`'s per-node solve. Since 2026-09-30 read once per process into a `OnceLock` (`jdbg`, `planar/junctions.rs:54-64`), so the check on every junction node and twice per open edge, in both modes, is one atomic load; it used to go through `inkvec_core::env::flag` each time, which takes the environment cache's mutex and searches it. `env::flag` caches its first read, so the value seen is the same |

## Open questions

- `MIN_UNMIX_CONTRAST`, `CONTRAST_REF` and `MAX_INFLATION` carry no stated derivation in
  the source. `CONTRAST_REF = 0.25` in particular sits inside a feature (`simplify_faint`)
  that the corpus measurement says currently loses on the screen set — it is possible the
  reference value was chosen for a different corpus or use case than the one it is
  currently measured against.
- `junction_fit_points()` (6), `junction_curvature_points()` (16), `MIN_QUADRATIC_POINTS`
  (5) and `CURVATURE_SIGNIFICANCE` (3.0) have no stated derivation; `CURVATURE_SIGNIFICANCE`
  in particular looks like a conventional statistical-significance constant rather than one
  swept against this corpus.
- `TAPER_MAX_CONSUMED = 0.35` (`planar/junctions.rs:387`) was originally motivated by a
  single named failure case (the leaf icon: a 5.3px taper move consumed enough of the
  shortest incident boundary to fold thirteen rings through themselves), but no sweep or
  comparison against nearby values is recorded anywhere in the source or its history.
  Whether 0.35 sits with comfortable margin above the value that would let that failure
  recur, or close to it, is not something the repository records either way.
