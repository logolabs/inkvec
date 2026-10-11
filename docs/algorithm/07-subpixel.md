# Stage 07 — Sub-pixel refinement

> Moves every boundary point from an exact pixel-grid corner to the sub-pixel position
> the image actually supports, and attaches an honest uncertainty to each one.

**Source:** `crates/inkvec-trace/src/planar.rs`, `crates/inkvec-trace/src/planar/junctions.rs`
**Entry points:** `fn refine_subpixel_alpha()` (`planar.rs:461-470`; `refine_subpixel`,
`planar.rs:438`, is its opaque form), which is `measure_subpixel` (`planar.rs:521-596`)
followed by `Refined::apply` (`planar.rs:480-493`), stage mark `"refine_subpix"`; and
`fn refine_junctions()` (`planar/junctions.rs:340`, stage mark `"refine_junc"`)
**Pipeline position:** after `build_map`, before `boundary_opt`. The measuring phase runs
side by side with symmetry detection under `rayon::join` (`lib.rs:1097-1131`); both read the
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
(`planar.rs:711-713`). `refine_subpixel_alpha` takes the same arguments plus the source
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
edge sharing a node keeps bit-identical coordinates afterwards (`planar/junctions.rs:392-396`).

## How it works

### `refine_subpixel`: per-point normal search

For each edge point `p` (`planar.rs:788-839`):

1. **Local tangent.** Estimated from neighbours `window` points to either side
   (`INKVEC_SUBPX_WIN` (*removed*), default 1 — see Environment overrides), unless the turning angle
   between the two chords exceeds `CORNER_COS` (60°, `planar/chord.rs:15`), in which case the
   narrow one-point window is used instead so a wide window does not smear a real corner.
   The normal is perpendicular to the tangent.
2. **Unmix colours.** `gradient::unmix_pair(fa, fb, p.x, p.y)` (`gradient.rs:574`)
   returns the two colours to project against and their separation `contrast`. If
   `contrast` is below `min_contrast = max(3*sigma_noise, MIN_UNMIX_CONTRAST)`
   (`planar.rs:417, 586`), the point is left on the grid at placeholder sigma `0.5` — there
   is nothing to unmix across.
3. **Sample coverage along the normal.** `UnmixAxis::coverage(x, y)` bilinearly samples the
   source image and projects onto the unmixed colour axis (`planar.rs:613-640, 961-969`),
   giving a 1-D coverage profile along the normal.
4. **Classify the profile and invert it.** Two inversion strategies:
   - **Step inversion** (`edge_offset`, `planar.rs:982-992`): closed-form inversion of the
     *exact* half-plane coverage of a unit square, used when the profile is `step_like` —
     both flat fills, monotone (not a one-pixel-wide ridge), saturating on both sides
     within the probe span, and contrast at least `2 * min_contrast`
     (`planar.rs:1099-1136`).
   - **Root-find** (`planar.rs:1141-1157`): a 9-step scan for the `0.5` crossing along the
     normal, `[-1, 1]` pixels either side of `p`, used otherwise — including at any
     corner, at any gradient boundary, or wherever the profile is not step-like.
5. **Positional uncertainty.** Combines statistical uncertainty
   `sigma = (sigma_noise / contrast) / |grad alpha|` with the level-set extraction's own
   resolution limit (`crate::coverage::DEFAULT_SIGMA_MODEL`, `= 0.05`, `coverage.rs:81`)
   in quadrature, floors it at `crate::contour::SIGMA_FLOOR` (zero, so no floor), applies curvature
   inflation (`crate::contour::inflate_for_curvature`, shared with the bilevel front end —
   see stage 08/09 docs for that mechanism), and clamps to `[0.02, 2.0]`
   (`planar.rs:1174-1217`).

### Measure, then apply: the parallel schedule

The per-point search above reads only the image, the two faces' fills and its own edge's
*original* points (its lattice neighbours, for the normal), and writes only its own
result: it is a pure map over vertices. The curvature correction of a vertex's sigma then
reads the edge's *moved* points, all of which are known by then: a pure map again. So since
2026-09-30 both run as parallel maps (`measure_subpixel`, `planar.rs:521-596`;
`refine_edge`, `planar.rs:690-752`):

- **edges in parallel**, and, inside an edge of at least `PAR_VERTICES = 64` points
  (`planar.rs:495-499`), **vertices in parallel too**, in chunks of at least 64, so a task
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
(`planar.rs:530-560`). The serial loop is kept as `refine_serial` in
`planar/refine_tests.rs`, and `parallel_refinement_equals_the_serial_loop` compares points
and sigmas bit for bit on anti-aliased scenes with flat and gradient fills, with and without
the alpha channel, down to one pixel and one row.

**When it stays serial.** Three cases run everything on the calling thread, in edge and
vertex order, as before:

- a small map: fewer than `PAR_MAP_VERTICES = 512` boundary vertices in all
  (`refine_in_parallel`, `planar.rs:501-519`). Waking rayon's sleeping workers only pays
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
measuring phase under `rayon::join` or before it (`lib.rs:1123-1129`); on a single-thread
build (WebAssembly) `join` runs the two in turn with no work added. Measured at 2048 px
before the change, the refinement took 10.6 ms, serial, at 0.44 µs per vertex (shared-stage
research, 2026-09-30). The shared stages that round rewrote (the ramp pass, the planar map
and this refinement, plus the face-alpha pass on transparent images) went from 42 to 13 ms
per 2048 px opaque image and from 130 to 21 ms per transparent one, output byte-identical
in Fast and Quality; Quality gains the planar-map and refinement part too.

### The strip reading: exact column sums, before the probes

Since 2026-10-10, a vertex between two flat fills with contrast of at least
`2 * min_contrast` (and not on an alpha axis) is first read by `planar/strip.rs`
(`strip_offset`, tried in `refine_vertex` before `probe_pixel_centres`). The per-point probes
below decide only where it declines.

It rests on three theorems proved in Lean in `formal/InkvecTheory` (see
[`docs/theory`](../theory/README.md)):

* **A column's coverages sum to the boundary's mean height over the column**
  (`column_sum_eq_average`), for any boundary that is a graph over the column, whatever its
  slope or curvature. No normal, threshold or interpolation enters. This is the identity of
  volume-of-fluid reconstruction (Puckett 2010) and of the partial-area edge detector
  (Trujillo-Pino et al. 2013).
* **Four adjacent column means determine a unique cubic** (`poly_eq_of_averages`), and
  `histopolate`'s rational constants return it exactly (`histopolate_cubic`).
* **The ½-crossing of interpolated coverage is biased by up to `3/2 − √2 ≈ 0.086 px`** on a
  straight edge (`levelSetBias_max`), the reading the root-find and the two-probe bracket use.

The procedure (`strip_along`):

1. Columns are read when the lattice normal is nearer vertical (the boundary is then a graph
   over x), rows otherwise; when one axis declines the other is tried.
2. In each of the six columns whose centres lie within 2.5 px of the vertex's pixel border,
   `line_mean` finds the window from a pixel saturated in one face (unmixed coverage within
   `SATURATED` = 0.06 of 0 or 1) to the nearest pixel saturated in the other, with only
   partial pixels between, and sums the window's coverages *unclamped*, flanks included
   (`UnmixAxis::coverage_unclamped`). Summing the flanks' own values keeps the sum unbiased
   under noise (`strip_unbiased`). A pixel whose colour lies more than
   `max(4σ, 0.06·contrast, 2/255)` off the line between the two faces' colours (a third
   colour, a junction) voids its window. Columns are read from the vertex outwards, each
   searched within `REACH` = 3 px of its inner neighbour's mean, so a 45° boundary is followed.
3. The orientation (which face is on the low side) must agree across the six, and adjacent
   means may differ by at most `MAX_STEP` = 1.5 px.
4. The cubic through the four central means is histopolated. The cubics through the left
   four and the right four must agree with it at the vertex's border to within
   `SIDE_TOL` = 0.05 px: every cubic passes (`side_stencil_cubic`), a corner within reach
   does not.
5. The vertex slides along its lattice normal onto the cubic (Newton, at most 8 steps,
   declining if the normal runs within about 17° of the cubic's tangent or the shift exceeds
   1 px). The edge keeps one point per vertex; the vertex's sigma is computed as before.
6. The shift is certified: a checker generated from the Lean definition its theorem is
   about (`stripResidual_on_cubic`; `crates/inkvec-verified`) evaluates the residual
   `q(s₀ + t·n_u) − t·n_v` in outward-rounded intervals and accepts the shift only if it is
   provably within `CERT_TOL` = 1e-9 px of zero for the exact real numbers. The cubic
   stencil `histopolate` is the generated one too, bit for bit the formula it replaced
   (`docs/theory/verification.md`).

Measured, stage-07 points against exactly known geometry (`bench/theory/inkvec_compare.py`:
circles, ellipses, straight edges at random angles, rotated squares and regular polygons,
64 px, 8-bit), distance to the true boundary in px:

| renderer | before: mean / p99 / max | after: mean / p99 / max |
|---|---|---|
| exact area coverage | 0.0734 / 0.319 / 0.516 | 0.0106 / 0.180 / 0.368 |
| 8x8 point supersampling | 0.0732 / 0.318 / 0.443 | 0.0159 / 0.194 / 0.351 |

The tail that remains is vertices the strip reading declines (corners, junctions, windows a
third colour enters). The regression gate's verdict is under *Measurements* in
[`docs/theory`](../theory/README.md#5-what-changed).

### The exact inversion, and why it replaced a biased root-find

The step-inversion path (`edge_offset`) is the more heavily documented piece of this
function, because it replaced an earlier, biased method, and the doc comments record the
measurements that justified each change:

- **The original bug.** Root-finding on a *bilinear interpolation between pixel centres*
  systematically pulled every edge towards the `.0`/`.5` grid: "interpolating from a pure
  neighbour (alpha 0) to the partial pixel (alpha 0.67) puts the 0.5 crossing at 0.25 of
  the way, not 0.33" — measured 0.19px off on straight runs, 2.5% of ink missing on a
  Material icon whose edges fall at multiples of 5.33px (`planar.rs:1008-1014`).
- **The chord-length fix.** Coverage changes with the chord length the normal cuts through
  a pixel per unit of edge movement (`1` on an axis, `sqrt(2)` on a diagonal), so the edge
  sits at `(alpha - 0.5) / chord` from the centre, with `chord = 1 / max(|nx|, |ny|)`
  (`planar.rs:1015-1017`). Dividing by `max` rather than the alternative was itself measured:
  the wrong denominator "pushed every diagonal and curved edge out by up to 2x: rounded
  outlines came back uniformly fat" (`planar.rs:1018-1019`).
- **The quadratic correction.** That linear rule is exact only for an axis-aligned edge.
  At 45°, a pixel's coverage curve is quadratic throughout, and the linear rule put a
  90%-covered pixel's edge at 0.57px from centre where the truth is 0.39 — "slanted
  strokes came out fat by 0.1-0.2px" (`planar.rs:1023-1024`). `edge_offset` therefore
  inverts the *exact* half-plane coverage of a unit square rather than a linear
  approximation (`planar.rs:982-992`).
- **Bracketing, not averaging.** Only the two probes that straddle the `0.5` crossing are
  used; averaging in a third, already-saturated probe was measured to pull a point half a
  pixel into a stroke under two pixels wide, because the far probe's partial coverage came
  from the stroke's *other* edge (`planar.rs:1049-1053`).
- **Why the root-find survives at all.** A soft boundary (two gradient bands, a shadow
  edge) is a ramp, not a step, and reading it as a step "throws the point up to a pixel
  off" (synthetic gradients 0.15 → 0.35 dE00 when the step inversion was applied there,
  `planar.rs:1106-1112`). A one-pixel stroke is a ridge with background on both sides, and
  reading a ridge as a step likewise throws the point out (synthetic `thin_features` 0.91
  → 3.00 dE00, `planar.rs:1113-1116`). The `monotone` test excludes both (`planar.rs:
  1156-1161`), and `both_flat` additionally excludes gradient boundaries, where the
  unmixing colours are only the model's local *prediction* and a small model error
  misplaces an inverted edge (colour families +0.015 dE00 when the inversion was applied
  there too, `planar.rs:820-825`).

Two switches this history left behind are recorded as no longer switchable:
`INKVEC_SUBPX_MODE=inv` (*removed*) (force step-inversion everywhere) and `=root` (force root-find
everywhere) were both measured worse than the classifier and are dead code paths in
intent even though the comments describing them remain (`planar.rs:1045-1047, 1166-1167`).

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
`coverage::intake_scale` (`coverage.rs:832-882`), which measures how many raster pixels
one unit of genuine edge detail occupies and is used upstream (in
`trace_color_full_with_alpha`, `lib.rs:360`) to decide whether the *palette's* noise
guard should widen its same-ink tolerance — not to change anything in `refine_subpixel`
itself. `refine_subpixel` runs identically regardless of `intake_scale`.

### `simplify_faint`: spending coordinates where they can be seen

Positional uncertainty at each point is inflated below a reference contrast when
`--simplify-faint` is set (`planar.rs:1186-1213`):

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
g`, `planar.rs:1185`), so a faint boundary is already measured with wider uncertainty than
a crisp one. `simplify_faint`'s inflation is a second, independent adjustment layered on
top, and the doc comment is explicit about why it is a separate knob rather than folded
into the physical sigma:

> "the *statistical* uncertainty... on clean art it is swamped by the method's own
> resolution limit — measured on two identical wavy edges, one at 90% contrast and one at
> 7%, the tracer spent 100 and 103 segments. That is the right answer to 'where is the
> boundary' and the wrong answer to 'how much description is this boundary worth': the
> error a viewer sees is the position error times the contrast across it, so at a tenth of
> the contrast a coordinate buys a tenth of the visible accuracy." (`planar.rs:1189-1195`)

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
> and not the gain — the same blindness that keeps `--cutout` off." (`planar.rs:1201-1205`)

### `refine_junctions`: why a junction cannot be refined like an edge point

`refine_subpixel` cannot localize a junction, because the pixel there is a mixture of
three or more colours and the two-colour unmixing it relies on is not defined
(`planar/junctions.rs:10-12`). So junction nodes are still on their pixel corners when
`refine_junctions` starts, and every face touching a junction would keep a corner up to
half a pixel from where its boundaries actually meet; this pass moves each junction there
and sets every incident edge's end point to that one position, "so the edges still share
it exactly" (`planar/junctions.rs:12-15`).

`refine_junctions` instead estimates the junction as the point where the incident
boundaries meet, in two stages:

1. **`end_tangent`** (`planar/junctions.rs:112-199`) fits each incident edge's end tangent as a
   line, from its already-refined interior points nearest the junction — skipping the
   `junction_skip()` points closest to the junction itself (1 point, `planar/junctions.rs:43`),
   because that region is contaminated by the three-way mixture, and using up to
   `junction_curvature_points()` (16, `planar/junctions.rs:50`) points, weighted by `1/sigma^2`
   from stage 07's own uncertainty estimates. It reports both the line and the variance of
   its position at the junction — inflated by the reduced chi-square when the points are
   not actually collinear (`fit_end_polynomial`, `planar/junctions.rs:214-290`). A straight-line
   extrapolation of an arc is systematically biased outward by about `t_mean^2 / 2R`; when
   the curvature term is statistically significant (`CURVATURE_SIGNIFICANCE = 3.0`,
   `planar/junctions.rs:216`) with enough points (`MIN_QUADRATIC_POINTS = 5`, `planar/junctions.rs:215`), a
   quadratic model is fitted instead, decided on the whole window (where curvature is best
   determined) but extrapolated from the nearer `junction_fit_points()` (6,
   `planar/junctions.rs:36-38`) points.
2. **`solve_junction`** (`planar/junctions.rs:610-647`) intersects those lines by weighted
   least-squares — closed form `p = (sum w_i n_i n_i^T)^-1 (sum w_i n_i c_i)`, `w_i = 1 /
   var_i` — but refuses when fewer than two lines are usable, when the lines are too
   nearly parallel to localize a crossing (`JUNCTION_MIN_CONDITION = 0.02`, roughly 16°,
   `planar/junctions.rs:78`), or when the solution would move the node further than
   `JUNCTION_MAX_MOVE = 1.5` px (`planar/junctions.rs:73`).

Where the boundaries meet **tangentially** rather than transversally, intersection is not
merely imprecise — it is ill-posed, with condition number `1/sin(theta)`
(`taper.rs:5-9`). `taper_junction` (`planar/junctions.rs:489-594`) handles that case instead, by
fitting a circle tangent to the "through" boundary from the branching boundary's own
points (see `crate::taper`, and stage-08-adjacent material for the geometry). The ordering
between the two is deliberate: `refine_junctions` "tries the tangent intersection first
and the taper fit only when that fails" (`planar/junctions.rs:336-338`), so a taper never
overrules an intersection that was well posed. (Earlier revisions recorded the
measurement behind that order, a taper-first run that relocated 1129 junctions over 180
images and cost colour accuracy on 75 of them; the current source no longer carries it.)

When a taper *does* fire, it can slide the junction along a boundary and past points that
boundary's edge list still holds; `trim_passed_over` (`planar/junctions.rs:415-464`) removes
them, so the edge does not double back through itself (`planar/junctions.rs:407-414`).

## Constants and thresholds

| name | value | controls | derivation |
|---|---|---|---|
| `MIN_UNMIX_CONTRAST` (`planar.rs:417`) | `0.02` | floor on unmixing contrast below which a point is not moved at all | no stated derivation |
| `CORNER_COS` (`planar/chord.rs:15`) | `0.5` (60°) | turning angle above which the tangent window narrows to 1 point | stated: "A staircase at any slope turns by at most 45 degrees between chords two points long, so slanted edges stay smooth" (`planar/chord.rs:12-14`) — a geometric bound, not a sweep |
| `PAR_VERTICES` (`planar.rs:499`) | `64` | fewest points an edge needs before its vertices are refined in parallel, and the smallest chunk of a long edge one thread takes | motivated by a measured cost: a task of at least ~30 µs (0.44 µs per vertex at 2048 px) against rayon's few-µs cost per split (`planar.rs:495-498`); chooses only the schedule, never the result |
| `PAR_MAP_VERTICES` (`planar.rs:513`) | `512` | fewest boundary vertices in the whole map for the refinement (and symmetry detection beside it) to use threads at all | measured: per-icon serial / parallel timings by vertex count, parallel already ahead in the smallest bucket (512–1,024: 0.263 / 0.206 ms), so the cutoff sits at its lower end (`planar.rs:501-512`); chooses only the schedule, never the result |
| `INKVEC_SUBPX_WIN` (*removed*) (`planar/chord.rs:7-10`) | default `1`, range `1..=8` | width of the tangent-estimation window | measured trade-off: widening to 2 improved dE00 0.2663→0.2534 on a 620-icon subset but cost DISTS 0.0418→0.0435 and caused a face-order regression on one icon (0.23→3.20 dE00); default left at 1 "until that is understood (LOG-43)" (`planar/chord.rs:47-58`) |
| `REACH` (`planar/strip.rs`) | `3` px | how far along a column the strip reading searches for a window's flanks, from the inner neighbour's mean | geometric: a boundary within 45° of the column's normal crosses at most two pixels of it |
| `SATURATED` (`planar/strip.rs`) | `0.06` | unmixed coverage within this of 0 or 1 makes a pixel a window flank | not swept; the flanks' own values are summed, so a near-saturated pixel classed as a flank costs nothing in bias (`strip_unbiased`) |
| `SIDE_TOL` (`planar/strip.rs`) | `0.05` px | largest disagreement at the vertex's border between the central and the one-sided cubics before the strip reading declines (the corner test) | each one-sided difference carries 0.7× the noise of one column mean; every cubic passes exactly (`side_stencil_cubic`); a 45° turn at the border moves it by 0.04 px, so gentler corners are rounded, by up to about 0.1 px |
| `MAX_STEP` (`planar/strip.rs`) | `1.5` px | largest difference between adjacent column means: the steepest boundary the strip reading follows, about 56° | not swept |
| `DEFAULT_SIGMA_MODEL` (`coverage.rs:81`) | `0.05` px | irreducible resolution limit of level-set extraction, added in quadrature to statistical noise | stated: "Measured on analytic circles..., level-set extraction lands within roughly 0.05px" (`coverage.rs:104-105`) |
| `CONTRAST_REF` (`planar.rs:1207`) | `0.25` | reference contrast for `simplify_faint`'s inflation | no stated derivation |
| `MAX_INFLATION` (`planar.rs:1208`) | `4.0` | cap on `simplify_faint`'s sigma multiplier | no stated derivation |
| `JUNCTION_MAX_MOVE` (`planar/junctions.rs:73`) | `1.5` px | reject an intersection solution beyond this move | no stated derivation: the doc comment says only "Farthest a junction may move from its grid node before the solution is distrusted" (`planar/junctions.rs:72`). Earlier revisions recorded a rejected 0.5 px limit (one test phase 0.178px → 1.006px residual); the current source no longer does |
| `JUNCTION_MIN_CONDITION` (`planar/junctions.rs:78`) | `0.02` | minimum eigenvalue ratio admitted for a junction intersection | stated as equivalent to rejecting crossings below about 16°, from `tan^2(theta/2)` (`planar/junctions.rs:75-77`) |
| `junction_fit_points()` (`planar/junctions.rs:36`) | `6` | interior points used for the near-junction extrapolation | no stated derivation |
| `junction_skip()` (`planar/junctions.rs:43`) | `1` | points nearest the junction excluded from the tangent fit | stated: the pixel there is a three-way mixture the two-colour unmixing does not know about (`planar/junctions.rs:40-42`) |
| `junction_curvature_points()` (`planar/junctions.rs:50`) | `16` | points used to test for significant curvature | no stated derivation |
| `MIN_QUADRATIC_POINTS` (`planar/junctions.rs:215`) | `5` | minimum points before a quadratic tangent model is tried | no stated derivation (a plausible minimum for a 3-parameter fit, but not stated as such) |
| `CURVATURE_SIGNIFICANCE` (`planar/junctions.rs:216`) | `3.0` | sigma threshold for preferring a quadratic over a linear tangent | no stated derivation (a conventional "3-sigma" significance bar) |
| `TAPER_DEGREES` (`planar/junctions.rs:468`) | `55.0` | how far from the through-direction a branch may lie and still be tested as a taper | no stated derivation in the current source (`planar/junctions.rs:466-467` says only what it bounds); earlier revisions explained it as deliberately loose, because the angle is measured *at the misplaced junction* (31° observed on the motivating rounded square, where a 20° gate rejected the case) |
| `TAPER_MAX_MOVE` (`planar/junctions.rs:470`) | `8.0` px | largest move a taper estimate may make | no stated derivation: "Farthest the taper fit may move a junction, in px" (`planar/junctions.rs:469`) |
| `TAPER_MAX_CONSUMED` (`planar/junctions.rs:472`) | `0.35` | fraction of the shortest incident boundary a taper move may consume | no derivation comment at the current declaration; an earlier revision carried one describing the motivating failure ("a single 5.3px move on a leaf icon left thirteen rings self-crossing... 78 segments became 355"), but it was dropped when this constant moved into `planar/junctions.rs` during the module split and has not been restored |
| `TAPER_MAX_SIGMA` (`planar/junctions.rs:474`) | `1.0` px | largest standard error a taper estimate may carry and still be preferred | no stated derivation: "Largest acceptable uncertainty of the vanishing point, in px" (`planar/junctions.rs:473`) |
| `TRIM_MIN_POINTS` (`planar/junctions.rs:402`) | `4` | fewest points an edge keeps after trimming passed-over points | stated: "Below this it has no shape left to fit" (`planar/junctions.rs:401`) |
| `TRIM_LOOK` (`planar/junctions.rs:405`) | `3` | how far ahead to look when deciding an edge's direction of travel | no stated derivation: "How far along an edge to look when asking which way it leaves the junction, in points" (`planar/junctions.rs:404`) |
| `MIN_WIDTH` / `MAX_WIDTH` (`taper.rs:104, 108`) | `0.05` / `6.0` px | taper sample admission band | stated: below `MIN_WIDTH` the measurement is noise-dominated; beyond `MAX_WIDTH` the tapering region is no longer bounded by a single arc (`taper.rs:102-108`) |
| `MIN_SAMPLES` (`taper.rs:111`) | `4` | fewest taper samples trusted | no stated derivation |
| `MAX_DEFECT` (`taper.rs:121`) | `0.15` px | largest tangent-circle residual admitted | calibrated against measured cases: a true taper measured 0.05px, two boundaries actually crossing measured 0.21, a constant-width strip measured 1.33 (`taper.rs:116-120`) |

## Failure modes and edge cases

- **The seam of a closed ring was never refined** (fixed 2026-10-10). `probe_chord` clamped
  its neighbour indices at the ends of an edge, and a closed edge stores its first point
  once, so the first and last vertices of every closed ring took their chord from one
  neighbour. On a ring whose seam lies on a horizontal run that is a horizontal normal on a
  horizontal boundary: the probe ran along the boundary and the vertex stayed on the
  lattice, 0.30 px off on an exactly rendered disc. Quality's boundary solve repaired it;
  Fast mode kept it. The chord now wraps round the seam of a closed edge. On the gate the
  fix alone is within noise everywhere (the corpus's closed rings mostly start at a
  junction or a corner); on exact discs it removes the worst stage-07 point.
- **`refine_subpixel`'s misclassification cost is asymmetric and recorded exactly once
  each way.** Every branch of the `step_like` classifier (`planar.rs:1129-1135`) exists
  because the *other* choice was tried and measured worse on a named case (gradients,
  thin features, script wordmarks — see How it works above). There is no held-out ablation
  covering all combinations at once.
- **A two-pixel saturation requirement was tried and rejected**, even though it "carried
  the thin-feature case" better in isolation: "objective 0.6703 vs 0.6816 but dE00 +0.004
  and parameters +9%: it starved the very edges the inversion is for" (`planar.rs:
  1162-1165`).
- **`refine_junctions` ordering hazard.** The taper runs only where the intersection
  fails (`planar/junctions.rs:336-338`). Earlier revisions recorded why: run first, the
  taper overruled well-posed intersections and cost accuracy on 75 of 180 images, and the
  harmful relocations could not be told from the harmless ones by their move, sigma or
  residual. The current source keeps the order but no longer carries that record.
- **Only a taper move trims edge points.** `refine_junctions` calls `trim_passed_over`
  only when the taper placed the node (`planar/junctions.rs:333-338`): a taper slides the
  node *along* a boundary, past points that boundary still holds.

## Environment overrides

Since the settings cleanup (CHANGELOG, 0.2.0, *Changed*) the engine reads its environment through one helper (`inkvec_core::env`): a switch is off when unset, empty or `0`, and every variable is read once per process. Variables marked *removed* below are gone (their defaults are constants now); those marked *research build* are read only by a binary built with `--features research`. The full list, with what is left and why, is [`docs/internal/env-vars.md`](../internal/env-vars.md).

| variable | effect |
|---|---|
| `INKVEC_SUBPX_WIN` (*removed*) | tangent-window width for `refine_subpixel`, `1..=8`, default `1` (`planar/chord.rs:7-10`) |
| `INKVEC_SUBPXDBG` | per-point debug trace of the subpixel search; read once per map into the refinement context (`planar.rs:581`), and while set the whole refinement runs serially, in edge and vertex order, so the lines come out as before (`planar.rs:584`) |
| `INKVEC_SIGMA_FLOOR` (*removed*) | overrode the sigma floor shared with the bilevel front end; now the constant `contour::SIGMA_FLOOR`, zero (`contour.rs:325-327`) |
| `INKVEC_CURV_GAIN` (*removed*) | overrode the curvature gain `inflate_for_curvature` uses; now the constant `NONLINEARITY_GAIN = 0.35`, chosen by sweep (`contour.rs:248-261`) |
| `INKVEC_DUMP_CONTOUR` | appends every refined edge's points and sigmas to a file, for offline study of extraction error structure (`dump_contour`, `planar.rs:762-774`); the path is read once per map (`planar.rs:582`; it used to be read per edge), and while it is set the refinement runs serially so the file keeps edge order |
| `INKVEC_NO_TAPER` | disables `taper_junction` (`planar/junctions.rs:490-491`); read once per process into a `OnceLock` through `inkvec_core::env::flag`, so an empty value counts as unset |
| `INKVEC_TAPERDBG` | per-node taper fit debug trace; read once per process into a `OnceLock` (`taperdbg`, `planar/junctions.rs:66-70`) |
| `INKVEC_TAPER_SKIP=<n>` (*removed*) | skipped the n-th accepted taper relocation, to attribute a corpus-level change to one junction; nothing reads it now |
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
- `TAPER_MAX_CONSUMED = 0.35` (`planar/junctions.rs:472`) was originally motivated by a
  single named failure case (the leaf icon: a 5.3px taper move consumed enough of the
  shortest incident boundary to fold thirteen rings through themselves), but no sweep or
  comparison against nearby values is recorded anywhere in the source or its history.
  Whether 0.35 sits with comfortable margin above the value that would let that failure
  recur, or close to it, is not something the repository records either way.
