# Stage 00 — Overview

> The whole pipeline: raster pixels in, an SVG document out, by treating every stage as a
> model-selection problem under one description-length objective.

**Source:** the whole tree; this page indexes it.
**Entry points:** `inkvec_trace::trace_color_full_with_alpha` (`crates/inkvec-trace/src/lib.rs:285`,
called via `trace_color_full` at `lib.rs:249`) for the raster-to-planar-map half;
`inkvec_cli::trace_image` (`crates/inkvec-cli/src/lib.rs:193`) for the whole command, intake
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
(`coverage.rs:20-33`, `CoverageField::position_sigma`, `coverage.rs:154-171`). A faint edge
says, honestly, that it was measured badly, and that number is what the curve fitter reads to
decide how hard to try: each point's squared miss is weighed by `1/sigma²` in the
chi-squared term of the fit's cost (`crates/inkvec-fit/src/lib.rs:82-90`; "chi2 weights are
`1/sigma²`", `coverage.rs:73-74`), and `--simplify-faint` inflates it further on faint
boundaries. The `tau * sigma` admissibility cone the fitter's module header once described is
no longer on the shipping path (`crates/inkvec-fit/src/lib.rs:92-98`), and the boundary
solve does not read sigma at all (`08-boundary-solve.md`). Nothing downstream invents its
own tolerance parameter; it reads this one.

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

`lambda` is not tuned by taste. `FitConfig::from_precision` (`crates/inkvec-fit/src/lib.rs:162`)
derives it from what a coordinate actually costs to write down: a value confined to a range
`extent` and stored to resolution `precision` carries `ln(extent / precision)` nats of
information (clamped so the ratio is never below `e`, i.e. `lambda >= 1`). For a 256px
canvas at 0.1px precision that is `ln(2560) ≈ 7.85` nats per coordinate pair — not the 1.0 a
first guess suggests, and the difference is "roughly a factor of two in emitted segment
count" per the doc comment. The fill-selection variant of the same idea is
`gradient::bic_lambda(n) = 0.5 * ln(n)` (`crates/inkvec-trace/src/gradient.rs:325`), the
Bayesian information criterion value: a large region has to earn a gradient with
proportionally more evidence than a small one does.

`FitConfig::lambda`'s own doc comment (`crates/inkvec-fit/src/lib.rs:136-144`) calls it "the constant
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
each timed by the `Stopwatch` (`crates/inkvec-trace/src/lib.rs:1203`, `mark` at `:1280`, printed under
`INKVEC_TIMING`):

| mark | line | stage | what it decides |
|---|---|---|---|
| — | `crates/inkvec-cli/src/lib.rs:292` (`intake`) | **intake** | decode (format from the file's signature, EXIF orientation applied, an ICC profile converted to sRGB; `crates/inkvec-trace/src/load.rs`), undo a nearest-neighbour upscale by any factor of 2 or more (exact), reduce a resampled or blurred raster to its detail (soft intake, Quality), optional SR clean-up, resolution normalisation, alpha matting — doc `01-intake.md` |
| `palette` | `crates/inkvec-trace/src/lib.rs:409` (`color::extract_palette_mdl_ids` :395) | palette | how many inks, and which colours, by MDL against measured pixel noise |
| `labels` | `crates/inkvec-trace/src/lib.rs:528` (`color::label_image_ids` :413) | labels | which ink each pixel is assigned to |
| `despeckle` | `crates/inkvec-trace/src/lib.rs:532` | despeckle | absorb regions below `min_region` into their most common neighbour |
| `blend_absorb` | `crates/inkvec-trace/src/lib.rs:570` (`absorb_blend_slivers` :548 / `reassign_blend_pixels` :557) | blend absorption | anti-aliased pixels between two inks are not a third ink; stop them minting sliver faces |
| `merge_bands` | `crates/inkvec-trace/src/lib.rs:599` (`gradient::merge_gradient_bands_with_ink` :586) | gradient bands | whether adjacent palette bands are really one gradient |
| `carve` | `crates/inkvec-trace/src/lib.rs:669` (`gradient::carve_residual_features_with_detail_noise` :652) | carve | cut out a feature the palette quantised into its surroundings before a gradient is asked to explain it |
| `split` | `crates/inkvec-trace/src/lib.rs:683` (`split_components` :682) | split | a face is a *connected* region, not "everywhere this colour appears"; a map with more components than `u16` face ids can number (`MAX_FACES`, 65,535) first has its smallest merged into a neighbour (`regions::cap_components`) |
| `saddles` | `crates/inkvec-trace/src/lib.rs:1073` (`merge_saddle_faces` :1062) | saddle join | resolve the one ambiguity labels cannot: four pixels meeting diagonally at one corner |
| `build_map` | `crates/inkvec-trace/src/lib.rs:1077` (`planar::build` :1076) | planar map | shared edges between exactly two faces, from the exact integer label grid, read off its row runs (`planar/cracks.rs`, `planar/runs.rs`) |
| `symmetry_detect` | `crates/inkvec-trace/src/lib.rs:1092` | refinement setup | since 2026-09-30 this mark times only the setup of the refinement's inputs (each face's fill model and opacity); `symmetry::detect` itself runs inside the next mark |
| `refine_subpix` | `crates/inkvec-trace/src/lib.rs:1127` (`symmetry::detect` beside `planar::measure_subpixel`, `lib.rs:1121-1125`; `Refined::apply`, `:1126`) | symmetry detect + sub-pixel | find mirror pairs on the label lattice, where the comparison is exact, and, at the same time, measure where each boundary point sits along its local normal (the 0.5-coverage level); both only read the lattice map, so they run side by side under `rayon::join`, and the measured points are written back afterwards |
| `refine_junc` | `crates/inkvec-trace/src/lib.rs:1130` (`planar::refine_junctions` :1129) | junctions | settle shared endpoints |
| `boundary_opt` | `crates/inkvec-trace/src/lib.rs:1142` (`boundary_opt::optimise_alpha` :1138) | boundary solve | move every boundary point at once so the *rendered* partition matches the image; not run when its band tables would pass a memory budget (`boundary_opt/band.rs`) |
| `decode` | `crates/inkvec-trace/src/lib.rs:1164` (`decode::decode_faces` :1153) | decode | order-first colour/geometry fix for faces too thin to own a fully-covered pixel; off unless `INKVEC_DECODE` (*research build*) is set |
| `symmetry` | `crates/inkvec-trace/src/lib.rs:1174` (`symmetry::enforce` :1172) | symmetry enforce | put back the exactness every upstream tie-break quietly broke |
| — | `crates/inkvec-cli/src/pipeline.rs:246` (`trace_total`) | — | end of the `inkvec_trace` half |
| — | `crates/inkvec-cli/src/pipeline.rs:480` (`fit_dp`) | curve fit | one global DP per boundary over lines, cubics and arcs, MDL cost; a whole-boundary primitive is offered beside it and taken when it costs less (`inkvec_fit::choice`) |
| — | `crates/inkvec-cli/src/pipeline.rs:489` (`repair`) | repair | close self-crossing rings the independent per-edge fits can produce |
| — | `crates/inkvec-cli/src/pipeline.rs:387` (`fills`) | fills | per-face fill model already chosen upstream; a gradient whose every stop lies within a just-noticeable difference of every other is painted flat (`pipeline/demote.rs`) |
| — | `crates/inkvec-cli/src/pipeline.rs:433` (`emit`) | emit | fitted geometry to SVG text, each ring wound by nesting depth so no `fill-rule` is needed; layers vs. flat form costed against each other |
| — | `crates/inkvec-cli/src/post.rs` | post | viewBox retarget, background knock-out, margin, minify |

Fast mode (`--mode fast`) takes another route through the same table. Its front end
(`crates/inkvec-trace/src/fast/front.rs`) replaces the marks from `palette` to `split` with
its own `palette`, `slivers`, `despeckle`, `split` and `ramps`; it shares `build_map`,
`refine_subpix`, `refine_junc` and `symmetry`, and skips `boundary_opt` and `decode`
(`lib.rs:1137`, `:1152`). Under `fit_dp` it runs a Potrace-class fitter instead of the DP,
and it skips `repair` and shape harmonization (`repair_fits` and `emit_options` in
`crates/inkvec-cli/src/pipeline.rs`). [`14-fast-mode.md`](14-fast-mode.md) follows that
route end to end.

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
  -> Rgba                    (crates/inkvec-trace/src/coverage.rs:174; straight RGBA f32, [0,1])
  -> Palette                 (crates/inkvec-trace/src/color.rs:615; Oklab colours + rgb + weight + alpha)
  -> Vec<u16> labels         (per-pixel face id, NOT a palette index — split_components makes that so)
  -> PlanarMap { edges: Vec<Edge>, n_labels, width, height }
       Edge { points: Vec<Point>, sigma: Vec<f64>, left: u16, right: u16,
              start_node, end_node, closed }   (crates/inkvec-trace/src/planar.rs:71)
  -> FillFit { model: FillModel, chi2, params, cost }   (crates/inkvec-trace/src/gradient.rs:308)
       FillModel::Flat | Linear | Radial       (gradient.rs:129)
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
(`merge_map`, `crates/inkvec-cli/src/pipeline.rs:1077-1092`).

## Reading order

`01-intake.md` covers everything before the palette runs — decode, the unblock pre-pass,
the SR pre-pass, resolution-invariant tolerances, alpha matting. Stages 02 onward (numbered
per the pipeline table above) go module by module through `inkvec-trace` and `inkvec-fit`.
`14-fast-mode.md` then covers Fast mode: which of those stages it replaces, which it shares,
and how each of its own stages works. `constants.md` collects every named constant across the
tree in one table.

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
  `tools/` (`build_upscaler` and `sr_tools_dir`, `crates/inkvec-cli/src/lib.rs:756-797`). `inkvec-sr` itself is not in DESIGN.md's
  list at all; it was added afterwards as the super-resolution pre-pass.
* **S0, image-formation-model estimation.** DESIGN.md §"S0" calls for estimating
  compositing gamma and the anti-aliasing kernel per image by fitting the edge-spread
  function. No such per-image gamma/AA-kernel estimator exists in this repository.
  What exists instead is narrower and more targeted — `coverage::intake_scale` measures
  edge *width* (`coverage.rs:832`) and `lossy_container` reads the file's codec
  (`trace/load.rs:77`) — which is a container-format and resampling detector, not a
  general image-formation-model fit. Whether this is a deliberate narrowing or an unbuilt
  piece of S0 is not stated anywhere in the code comments. The intake also converts an
  embedded ICC profile to sRGB (`trace/load/icc.rs`), which reads the colour space the file
  declares; it estimates neither the compositing gamma nor the kernel.
* **S2, joint analysis-by-synthesis boundary solve.** This part of DESIGN.md is built and
  matches closely: `boundary_opt::optimise` (`trace/boundary_opt.rs`) is exactly the
  "parametrize the boundary, forward-render, minimise residual against the observed image"
  design DESIGN.md specifies, run by default (`trace/lib.rs:1137-1141`; skipped in Fast mode,
  and when its band tables would pass a memory budget, `trace/boundary_opt.rs:619-623`).
* **S4, primitives inside one global DP.** DESIGN.md insists primitives must be *members of*
  the segmentation alphabet, decided by the same dynamic program as lines and cubics,
  because "once cubics are fitted they have already absorbed the error a primitive would
  have explained." The code does this for part of the alphabet only. Circular and
  elliptical arcs *are* states of the dynamic program (`multimodel::optimal_multimodel` fits
  lines, cubics and arcs per span; see `11-fitting.md`), but whole-boundary primitives —
  circle, ellipse, rounded rectangle, a run of arcs — are a separate fit
  (`primitives::fit_primitive_or_arcs`), and `inkvec_fit::choice` keeps whichever of the two
  descriptions costs less under the shared MDL cost (`choice::choose`,
  `crates/inkvec-fit/src/choice.rs:211-226`). That is model *selection between* two fits,
  not primitives as states inside one DP. `choice::describe` (called per boundary from
  `fit_boundaries`, `crates/inkvec-cli/src/pipeline.rs:694-710`) also orders the work: for
  the image frame it tries the rectangle first and skips the dynamic program when the
  rectangle is below a lower bound on any fitted path (`choice::cost_floor`); for every
  other boundary it runs the program and the primitive search side by side under
  `rayon::join`. Both are exact: the choice made is the one the full comparison makes. The
  cost of the gap has since been measured where the primitive fit lives
  (`crates/inkvec-fit/src/primitives.rs:697-719`): over the 246-icon gate set a primitive is
  offered on 606 of 6,898 boundaries and wins 595 of the offers (98.2%), with no population
  of near-misses for a unified DP to rescue; what the comparison cannot see is a boundary
  that is only *partly* a primitive, which is never offered one.
* **S5, structured refinement with topology not frozen.** DESIGN.md §S5 explicitly reverses
  an earlier decision to freeze topology during polish, arguing for a structured move set
  searched to convergence. The code has gone the other way again: `repair_fits`
  (`crates/inkvec-cli/src/pipeline.rs:825-845`) records that both `polish` (per-cubic control-point refinement against the coverage
  field) and the "adjudication" re-scoring stage were *removed*, with a measured result —
  "removing them takes the objective from 0.5477 to 0.4960 and dE00 from 0.2503 to 0.2146...
  Polish alone made 197 of 246 better by not running." The stated reason is that
  `boundary_opt::optimise` now solves the whole boundary jointly *before* the fit runs, so a
  local per-cubic refinement afterwards partly undoes a better global answer. This is a
  genuine, measured reversal of DESIGN.md's S5, not an oversight — but DESIGN.md itself
  still describes the removed stages as the specified design.
* **Symmetry as a first-class constraint.** DESIGN.md's region graph (§4) models symmetry as
  a constraint enforced *during* optimisation. The code detects symmetry once, early
  (`symmetry::detect` at `trace/lib.rs:1121-1125`, on the exact label lattice, before any
  geometry moves: it runs beside the sub-pixel refinement's measuring phase, which writes
  nothing to the map until detection has returned), and enforces it twice more downstream —
  once on the map (`symmetry::enforce`, `trace/lib.rs:1172`) and once on the fitted curves, by reflecting one boundary's fit onto
  its mirror rather than re-solving both (`apply_mirrors`, `crates/inkvec-cli/src/pipeline.rs:913`). That is "detect once,
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
* `inkvec_fit::fit_path` (`crates/inkvec-fit/src/lib.rs:955`, documented in its own doc
  comment as "two passes: a line-only DP... then cubics are fitted to the runs between
  them") is not reachable from the production pipeline. Every call site is an example or a
  test — `crates/inkvec-cli/examples/fitdbg.rs`, `crates/inkvec-fit/examples/multimodel_demo.rs`,
  `crates/inkvec-fit/examples/primitives_demo.rs`, `crates/inkvec-fit/tests/multimodel.rs`,
  `crates/inkvec-fit/tests/primitives.rs` — that exercise the line-then-cubic fit directly.
  The CLI pipeline instead calls `multimodel::optimal_multimodel`
  (`crates/inkvec-cli/src/pipeline.rs:702`, and `optimal_multimodel_without_structural` at
  `:732` for the research baseline) directly. `fit_path` exists for
  examples and tests, not as part of the shipped trace.
