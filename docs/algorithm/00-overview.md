# Stage 00 — Overview

> The whole pipeline: raster pixels in, an SVG document out, by treating every stage as a
> model-selection problem under one description-length objective.

**Source:** the whole tree; this page indexes it.
**Entry points:** `inkvec_trace::trace_color_full_with_alpha` (`crates/inkvec-trace/src/lib.rs:275`,
called via `trace_color_full` at `lib.rs:264`) for the raster-to-planar-map half;
`inkvec_cli::trace_image` (`crates/inkvec-cli/src/lib.rs:242`) for the whole command, intake
through SVG text.
**Pipeline position:** none — this is the front door. Every numbered stage document assumes
the reader has this one.

## The core idea

`crates/inkvec-trace/src/coverage.rs:1-33` states it better than a paraphrase would, so this
is close to verbatim:

> An anti-aliased pixel is not a blurry approximation of the shape — it is a *measurement*
> of how much of that pixel the shape covers. Reading it as a measurement rather than as
> noise to be thresholded away is the single largest lever in this project.

For a boundary between a foreground colour `F` and a background colour `B`, an observed
pixel is `P = a*F + (1-a)*B`. Projecting onto the `F-B` axis inverts that:

```text
a = dot(P - B, F - B) / |F - B|^2
```

a least-squares estimate that uses all three colour channels, not one luminance projection
(`coverage.rs:11-18`). The reason this matters in practice, not just in principle, is stated
in the same file: on the `thin_features` case in `docs/M0-BASELINE.md` §3, VTracer recovers
2 of 11 elements and *no parameter setting recovers more*, because the coverage information
was destroyed before any tunable stage ran (`coverage.rs:6-8`).

The corollary the whole codebase is built on: uncertainty comes out of the same equation.
Pixel noise `sigma_pixel` gives coverage uncertainty `sigma_a = sigma_pixel / |F-B|`; the
boundary is the level set `a = 0.5`, so positional uncertainty is `sigma_a / |grad a|`
(`coverage.rs:20-33`, `CoverageField::position_sigma`, `coverage.rs:102-117`). A faint edge
says, honestly, that it was measured badly, and that number is what later stages read to
decide how hard to try — `tau * sigma` admissibility in the curve fitter
(`crates/inkvec-fit/src/multimodel.rs:13-19`), fit tolerance in the boundary solve, node
budget in `--simplify-faint`. Nothing downstream invents its own tolerance parameter; it
reads this one.

## The objective the whole system serves

Every stage that decides to keep, drop, merge or spend a parameter answers the same
question, phrased as one Occam's-razor cost:

```text
cost = 0.5 * chi2 + lambda * params
```

`chi2` is squared error scaled by the measured per-point uncertainty (so a well-localized
boundary is held to a tighter standard than a badly-localized one); `params` counts the
numbers written into the SVG; `lambda` is the exchange rate between a nat of residual and
a nat of description length.

`lambda` is not tuned by taste. `FitConfig::from_precision` (`crates/inkvec-fit/src/lib.rs:95`)
derives it from what a coordinate actually costs to write down: a value confined to a range
`extent` and stored to resolution `precision` carries `ln(extent / precision)` nats of
information (clamped so the ratio is never below `e`, i.e. `lambda >= 1`). For a 256px
canvas at 0.1px precision that is `ln(2560) ≈ 7.85` nats per coordinate pair — not the 1.0 a
first guess suggests, and the difference is "roughly a factor of two in emitted segment
count" per the doc comment. The fill-selection variant of the same idea is
`gradient::bic_lambda(n) = 0.5 * ln(n)` (`crates/inkvec-trace/src/gradient.rs:328-330`), the
Bayesian information criterion value: a large region has to earn a gradient with
proportionally more evidence than a small one does.

`FitConfig::lambda`'s own doc comment (`crates/inkvec-fit/src/lib.rs:73-82`) calls it "the constant
most likely to be mis-set in a way that looks like a pipeline defect" — worth remembering
when a stage's output looks wrong and the fix turns out to be a units problem, not a logic
one.

## The pipeline

Two crates carry the geometry. `inkvec_trace` decides *what is in the image*: palette,
labels, the planar map, fills. `inkvec_fit` turns a measured boundary into curves.
`inkvec_cli` drives both and turns the result into SVG text (`crates/inkvec-cli/src/lib.rs:1-22`):

```text
image -> intake -> trace -> fit -> repair -> emit -> post -> SVG
```

The colour path proper — `trace_color_full_with_alpha` — runs the marks below, in order,
each timed by the `Stopwatch` (`crates/inkvec-trace/src/lib.rs:1030-1055`, printed under
`INKVEC_TIMING`):

| mark | line | stage | what it decides |
|---|---|---|---|
| — | `crates/inkvec-cli/src/lib.rs:242` | **intake** | decode, unblock a nearest-neighbour upscale, optional SR clean-up, resolution normalisation, alpha matting — doc `01-intake.md` |
| `palette` | `crates/inkvec-trace/src/lib.rs:385` (`color::extract_palette_mdl` :372) | palette | how many inks, and which colours, by MDL against measured pixel noise |
| `labels` | `crates/inkvec-trace/src/lib.rs:502` (`color::label_image` :387) | labels | which ink each pixel is assigned to |
| `despeckle` | `crates/inkvec-trace/src/lib.rs:505` | despeckle | absorb regions below `min_region` into their most common neighbour |
| `blend_absorb` | `crates/inkvec-trace/src/lib.rs:542` (`absorb_blend_slivers` / `reassign_blend_pixels`) | blend absorption | anti-aliased pixels between two inks are not a third ink; stop them minting sliver faces |
| `merge_bands` | `crates/inkvec-trace/src/lib.rs:570` (`gradient::merge_gradient_bands_with_ink`) | gradient bands | whether adjacent palette bands are really one gradient |
| `carve` | `crates/inkvec-trace/src/lib.rs:633` (`gradient::carve_residual_features`) | carve | cut out a feature the palette quantised into its surroundings before a gradient is asked to explain it |
| `split` | `crates/inkvec-trace/src/lib.rs:646` (`split_components`) | split | a face is a *connected* region, not "everywhere this colour appears" |
| `saddles` | `crates/inkvec-trace/src/lib.rs:965` (`merge_saddle_faces`) | saddle join | resolve the one ambiguity labels cannot: four pixels meeting diagonally at one corner |
| `build_map` | `crates/inkvec-trace/src/lib.rs:968` (`planar::build`) | planar map | shared edges between exactly two faces, from the exact integer label grid |
| `symmetry_detect` | `crates/inkvec-trace/src/lib.rs:972` (`symmetry::detect`) | symmetry detect | find mirror/rotation pairs on the label lattice, where the comparison is exact |
| `refine_subpix` | `crates/inkvec-trace/src/lib.rs:975` (`planar::refine_subpixel`) | sub-pixel | slide each boundary point along its local normal to the measured 0.5-coverage level |
| `refine_junc` | `crates/inkvec-trace/src/lib.rs:977` (`planar::refine_junctions`) | junctions | settle shared endpoints |
| `boundary_opt` | `crates/inkvec-trace/src/lib.rs:987` (`boundary_opt::optimise`) | boundary solve | move every boundary point at once so the *rendered* partition matches the image |
| `decode` | `crates/inkvec-trace/src/lib.rs:1003` (`decode::decode_faces`) | decode | order-first colour/geometry fix for faces too thin to own a fully-covered pixel; off unless `INKVEC_DECODE` is set |
| `symmetry` | `crates/inkvec-trace/src/lib.rs:1013` (`symmetry::enforce`) | symmetry enforce | put back the exactness every upstream tie-break quietly broke |
| — | `crates/inkvec-cli/src/pipeline.rs:363` (`trace_total`) | — | end of the `inkvec_trace` half |
| — | `crates/inkvec-cli/src/pipeline.rs:585` (`fit_dp`) | curve fit | one global DP per boundary, `{line, cubic}` alphabet, MDL cost, primitives offered as an alternative and taken when they cost less |
| — | `crates/inkvec-cli/src/pipeline.rs:699` (`repair`) | repair | close self-crossing rings the independent per-edge fits can produce |
| — | `crates/inkvec-cli/src/pipeline.rs:774` (`fills`) | fills | per-face fill model already chosen upstream; demote imperceptible gradients to flat here |
| — | `crates/inkvec-cli/src/pipeline.rs:894` (`emit`) | emit | fitted geometry to SVG text; layers vs. flat form costed against each other |
| — | `crates/inkvec-cli/src/post.rs` | post | viewBox retarget, background knock-out, margin, minify |

Crates: `inkvec-core` (geometry primitives), `inkvec-trace` (raster → planar map),
`inkvec-fit` (points → curves), `inkvec-sr` (super-resolution pre-pass), `inkvec-cli`
(driver, fills, emit), `inkvec-wasm` (browser build). `inkvec-trace/src/lib.rs:1-27` gives
the same table from the trace crate's own point of view, with one line worth repeating
verbatim because it is the whole design in one sentence:

> One rule governs every stage: a model is kept only when it lowers squared residual
> against the image by more than `lambda` times the parameters it adds.

## Data structures that flow between stages

```text
bytes
  -> Rgba                    (crates/inkvec-trace/src/coverage.rs:128; straight RGBA f32, [0,1])
  -> Palette                 (crates/inkvec-trace/src/color.rs:679; Oklab colours + rgb + weight + alpha)
  -> Vec<u16> labels         (per-pixel face id, NOT a palette index — split_components makes that so)
  -> PlanarMap { edges: Vec<Edge>, n_labels, width, height }
       Edge { points: Vec<Point>, sigma: Vec<f64>, left: u16, right: u16,
              start_node, end_node, closed }   (crates/inkvec-trace/src/planar.rs:29)
  -> FillFit { model: FillModel, chi2, params, cost }   (crates/inkvec-trace/src/gradient.rs:311)
       FillModel::Flat | Linear | Radial       (gradient.rs:101)
  -> Polyline (per edge, in content units)  -> FittedPath { start, segments: Vec<Segment>, closed }
       Segment::Line | Cubic | (Primitive fits carried alongside: PrimitiveFit)
  -> FaceRings (which edges each face walks, and which way)  -> SVG path strings (emit.rs)
```

The structural claim that makes this different from "trace the edges" is in
`crates/inkvec-trace/src/planar.rs:1-18`: a boundary between two regions is stored **once**.
Both faces reference the same `Edge`. A tracer that stores each region as an independent
closed path has to choose between two failures — lay regions edge-to-edge and rounding
disagreement between the two copies of the shared boundary opens a seam; overlap them and
the boundary is drawn twice (overdraw). `docs/M0-BASELINE.md` §4, quoted in the module doc,
measures VTracer sitting on that trade at overdraw 1.64 against a ground truth of 1.00. Here
seams are not merely rare, they are unrepresentable, because there is only one copy of the
edge to move.

`Edge.sigma` is the per-point positional uncertainty from the coverage inversion, carried
all the way to the curve fitter; `Edge.left` / `Edge.right` are face ids, `u16::MAX` when an
edge has been merged into the interior of a layer and no longer belongs to any ring
(`crates/inkvec-cli/src/lib.rs:921-937`).

## Reading order

`01-intake.md` covers everything before the palette runs — decode, the unblock pre-pass,
the SR pre-pass, resolution-invariant tolerances, alpha matting. Stages 02 onward (numbered
per the pipeline table above) go module by module through `inkvec-trace` and `inkvec-fit`.
`constants.md` collects every named constant across the tree in one table.

## `docs/DESIGN.md` against the code as it stands

`docs/DESIGN.md` is dated 2026-08-31 and states its own status as "M0 built and measured; M1
designed." The code in this repository has moved well past that milestone marker — most of
what DESIGN.md specifies for M1 (S0–S4) is built and running by default — so the design
document is best read as *rationale for decisions already taken*, not as a roadmap of what
is still to come. Specific points where the two disagree, worth a reader's attention because
disagreements are where the real design lives:

* **Crate layout.** DESIGN.md §6 specifies `inkvec-render`, `inkvec-io` and `inkvec-py` as
  separate crates. The tree that exists has none of them: rasterisation for the SR detector
  lives in `inkvec-sr::detect` (via `resvg`), SVG emission lives in `inkvec-cli::emit`, and
  there is no `inkvec-py` — Python involvement is limited to the packaged SR fallback in
  `tools/` (`crates/inkvec-cli/src/lib.rs:442-467`). `inkvec-sr` itself is not in DESIGN.md's
  list at all; it was added afterwards as the super-resolution pre-pass.
* **S0, image-formation-model estimation.** DESIGN.md §"S0" calls for estimating
  compositing gamma and the anti-aliasing kernel per image by fitting the edge-spread
  function. No such per-image gamma/AA-kernel estimator exists in this repository.
  What exists instead is narrower and more targeted — `coverage::intake_scale` measures
  edge *width* (`coverage.rs:306-337`) and `lossy_container` reads the file's codec
  (`trace/lib.rs:94-113`) — which is a container-format and resampling detector, not a
  general image-formation-model fit. Whether this is a deliberate narrowing or an unbuilt
  piece of S0 is not stated anywhere in the code comments.
* **S2, joint analysis-by-synthesis boundary solve.** This part of DESIGN.md is built and
  matches closely: `boundary_opt::optimise` (`trace/boundary_opt.rs`) is exactly the
  "parametrize the boundary, forward-render, minimise residual against the observed image"
  design DESIGN.md specifies, run by default (`trace/lib.rs:468-473`).
* **S4, primitives inside one global DP.** DESIGN.md insists primitives must be *members of*
  the segmentation alphabet, decided by the same dynamic program as lines and cubics,
  because "once cubics are fitted they have already absorbed the error a primitive would
  have explained." The code does not do this. `cli/lib.rs:863-890` runs
  `multimodel::optimal_multimodel` (the `{line, cubic}` DP) and
  `fit_primitive_or_arcs` (a separate primitive/arc fit) independently, per boundary, and
  keeps whichever scores lower under the shared MDL cost. That is model *selection between*
  two fits, not primitives as a state inside one DP with a shared alphabet. Whether this
  costs anything in practice is not measured in the comments; it is a real architectural gap
  from the stated design, not a rewording of it.
* **S5, structured refinement with topology not frozen.** DESIGN.md §S5 explicitly reverses
  an earlier decision to freeze topology during polish, arguing for a structured move set
  searched to convergence. The code has gone the other way again: `cli/lib.rs:939-960`
  records that both `polish` (per-cubic control-point refinement against the coverage
  field) and the "adjudication" re-scoring stage were *removed*, with a measured result —
  "removing them takes the objective from 0.5477 to 0.4960 and dE00 from 0.2503 to 0.2146...
  Polish alone made 197 of 246 better by not running." The stated reason is that
  `boundary_opt::optimise` now solves the whole boundary jointly *before* the fit runs, so a
  local per-cubic refinement afterwards partly undoes a better global answer. This is a
  genuine, measured reversal of DESIGN.md's S5, not an oversight — but DESIGN.md itself
  still describes the removed stages as the specified design.
* **Symmetry as a first-class constraint.** DESIGN.md's region graph (§4) models symmetry as
  a constraint enforced *during* optimisation. The code detects symmetry once, early
  (`symmetry_detect` at `trace/lib.rs:458`, on the exact label lattice, before any geometry
  moves), and enforces it twice more downstream — once on the map (`symmetry::enforce`,
  `trace/lib.rs:494`) and once on the fitted curves, by reflecting one boundary's fit onto
  its mirror rather than re-solving both (`cli/lib.rs:1009-1039`). That is "detect once,
  then copy," which is cheaper and exact by construction, but it is a different design from
  "enforced as a constraint during re-optimisation."
* **§0 governing directive ("best algorithm at every stage, even substantially slower").**
  The S5 removal above is evidence the project has since prioritised the measured objective
  over that directive where the two conflicted. Worth flagging for anyone using DESIGN.md's
  §0 as a standing instruction: the code's own commit history (via the doc comments) shows
  at least one considered decision to trade the "best algorithm" for a better-measured
  cheaper one.

## Open questions

* Whether DESIGN.md's S0 (per-image gamma / AA-kernel estimation) is intentionally narrowed
  to the two detectors that exist, or genuinely unbuilt, is not stated anywhere in the
  tree. **Unverified.**
* `docs/M0-BASELINE.md` and `docs/M1-PROGRESS.md` (referenced from DESIGN.md §9.5) exist
  in the repository. The measurement scripts cited throughout the code comments are
  development-time artefacts that are not committed, so the numbers quoted above come from
  source-code doc comments rather than being independently re-derived from a committed
  harness. A reader auditing a specific number should trace it to the comment that states
  it.
* `inkvec_fit::fit_path` (`crates/inkvec-fit/src/lib.rs:770`, documented in its own doc
  comment as "two passes: a line-only DP... then cubics are fitted to the runs between
  them") is not reachable from the production pipeline. Every call site is an example or a
  test — `crates/inkvec-cli/examples/fitdbg.rs`, `crates/inkvec-fit/examples/multimodel_demo.rs`,
  `crates/inkvec-fit/examples/primitives_demo.rs`, `crates/inkvec-fit/tests/multimodel.rs`,
  `crates/inkvec-fit/tests/primitives.rs` — that exercise the line-then-cubic fit directly.
  The CLI pipeline instead calls `multimodel::optimal_multimodel`
  (`crates/inkvec-cli/src/pipeline.rs:103` and `:492`) directly. `fit_path` exists for
  examples and tests, not as part of the shipped trace.
