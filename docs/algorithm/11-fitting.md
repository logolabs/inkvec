# Stage 11 — Curve fitting

> Turns a measured boundary — a polyline with a per-point uncertainty — into the smallest
> description-length combination of lines, cubics, arcs and constrained primitives that
> still explains the measurement.

**Source:** `crates/inkvec-fit/src/` — `lib.rs` (the objective, corners, the line-only
program), `multimodel.rs` and `multimodel/scan.rs` (the dynamic program), `candidates.rs`
(the per-span fitters), `tangents.rs`, `decimate.rs`, `cost.rs`, `primitives.rs`,
`curves.rs`, `merge.rs` with `merge/residual.rs`, `merge/grid.rs` and `merge/snap.rs` (the
post-fit passes), `choice.rs` (curve or primitive), `simple.rs` (self-crossing test).
**Entry point:** `optimal_multimodel()` (`crates/inkvec-fit/src/multimodel.rs:205`), called
for every boundary of the colour path from `fit_boundaries`
(`crates/inkvec-cli/src/pipeline.rs:653`) through `inkvec_fit::choice::describe`
(`pipeline.rs:714-738`), and for strokes from `crates/inkvec-cli/src/strokes.rs:263`.
**Pipeline position:** after decode and symmetry enforcement (stages 9–10), before repair
(stage 12). Stage mark `"fit_dp"` (`crates/inkvec-cli/src/pipeline.rs:489`, in
`fit_and_repair`).

The crate's own overview (`crates/inkvec-fit/src/lib.rs:1-58`) lists the seven steps of the
shipping fit in order; this page follows them and adds what the code comments measured.

## What problem this solves

Every earlier stage produces a *measured* boundary: a polyline of points, each with a
positional uncertainty `sigma` inherited from stage 2's coverage measurement and stage 7–8's
sub-pixel and boundary-solve refinement. This stage turns that polyline into an SVG path —
lines, cubics, arcs — choosing both the segmentation (where the breakpoints fall) and, per
segment, which primitive to spend parameters on.

A line-only alphabet has a hard floor no amount of segmentation quality can lift.
`curves.rs:3-7`, verbatim:

> "The line-only alphabet has a hard floor that no amount of segmentation quality can lift.
> A circle approximated by chords within tolerance `e` needs about `pi / sqrt(e / 2r)`
> segments — 36 of them for a 46px radius at 0.1px — while four cubics track the same circle
> to about 0.02% error. The segmentation was already optimal; it was optimal over the wrong
> alphabet."

So this stage does two things at once: it picks where to break the boundary, and it picks
what each piece is drawn with — under one objective, so the two decisions cannot fight each
other.

## Inputs and outputs

**Input:** an `inkvec_core::Polyline` — points plus a per-point `sigma: Vec<f64>` — and a
`FitConfig`.

**`FitConfig`** (`lib.rs:129-145`) has exactly two fields:

| field | meaning |
|---|---|
| `tau` | confidence multiplier on the per-point sigma; the default `2.0` "admits a chord that stays within ~2 standard deviations of every measurement" |
| `lambda` | cost per emitted parameter, in nats — the exchange rate between fidelity and description length |

**Output:** `FittedPath` (a start point and a sequence of `curves::Segment`: `Line`,
`Cubic`, `Arc`) from `optimal_multimodel`, or the richer `MultimodelFit` from
`optimal_multimodel_full` (`multimodel.rs:189-202`):

```rust
pub struct MultimodelFit {
    pub path: FittedPath,
    /// Indices into the source polyline chosen as vertices. For a closed input the
    /// first and last are the same index.
    pub vertices: Vec<usize>,
    /// Type of each segment `vertices[k] -> vertices[k+1]`.
    pub kinds: Vec<SegKind>,
    /// Value of the dynamic program's objective at the optimum, *before* the continuous
    /// refinement of `refine`. This is what the exhaustive test verifies.
    pub cost: f64,
}
```

`SegKind` (`multimodel.rs:178-187`) is `Line`, `Cubic`, or `Arc` (circular and elliptical
arcs share the tag; only the stored parameters differ). `optimal_multimodel_capped` /
`_capped_full` (`multimodel.rs:224`, `:235`) add a `max_span` argument — the point-span cap
stage 12's repair uses — and `optimal_multimodel_forced` (`multimodel/limits.rs:94`) adds vertices
every solution must keep, the repair's pins; see `12-repair.md`.

Each boundary's fitted path then competes with a whole-boundary primitive (circle,
ellipse, rounded rectangle, or a run of arcs); `inkvec_fit::choice` makes that choice — see
*Curve or primitive* below.

## How it works

### The objective, and where `lambda` comes from

The cost of a candidate description is (`lib.rs:11-20`)

```
E = ½·χ² + λ·P,        χ² = Σ_k (d_k / σ_k)²
```

`d_k` is the distance (px) from measured point `k` to the emitted geometry, `σ_k` its
uncertainty, `P` the count of numbers the description writes, and `½·χ²` the Gaussian
negative log-likelihood, so both terms are in nats. `FitConfig::from_precision`
(`lib.rs:148-169`) derives `lambda`, verbatim:

> "MDL measures description length in nats. A coordinate confined to a range `extent` and
> stored to resolution `precision` carries `ln(extent / precision)` nats of information.
> For a 256px canvas written at 0.1px precision that is `ln(2560) ~ 7.85` — not the 1.0 a
> first guess suggests, and the difference is roughly a factor of two in emitted segment
> count."

```rust
pub fn from_precision(extent: f64, precision: f64, tau: f64) -> Self {
    let ratio = (extent / precision.max(f64::MIN_POSITIVE)).max(std::f64::consts::E);
    Self {
        tau,
        lambda: ratio.ln(),
    }
}
```

The `.max(std::f64::consts::E)` floor means `lambda` is never below `1.0`, however small
`extent/precision` gets. The default (`lib.rs:171-176`) is a 256 px canvas at 0.1 px output
precision, `tau = 2.0`.

In the shipping CLI, `extent` is the intake raster's `max(width, height)` in pixels, and
`precision`/`tau` come from `--precision` (default `0.1`) and `--tau` (default `2.0`)
(`crates/inkvec-cli/src/args.rs:234-235`), combined in `fit_config`
(`crates/inkvec-cli/src/units.rs:92-105`). Under `--content-units`, `fit_config` multiplies
`precision` by the content scale `s` before deriving `lambda` and then multiplies `lambda`
by `s` again; see `01-intake.md`.

At a 512 px intake with defaults: `lambda = ln(5120) ≈ 8.541`. At 2048 px: `ln(20480) ≈
9.927`. Note how little `lambda` moves for a 4x change in image size — this is the first
sign of the fact below.

What a cubic costs is no longer a fixed 6 for every caller: `cost.rs` holds a `CostModel`
that a caller sets for one trace (`--bezier-cost`, `--corner-angle` on the command line);
the defaults are 6 parameters per cubic and a 10-degree corner, and a trace that asks for
nothing is unchanged (`cost.rs:1-21`). The code reads the price through
`params_cubic()` (`candidates.rs:54-58`).

### The effective geometric tolerance — and why the square root matters

No line in the repo writes `d = sigma * sqrt(2*lambda*k/n)` as a formula — this is a
derivation, not a quotation — but every quantity in it is pinned to code: the cost function
is `E = ½·χ² + λ·P` (`lib.rs:11-20`), applied identically to every candidate the dynamic
program prices (the cost column of `candidates.rs:16-26`), at the merge pass's accept test
(`merge.rs:425-431`, code at `:501-542`) and at the curve-or-primitive choice
(`choice.rs:106-118`); and `chi2` is the sum of squared residuals each weighted by
`1/sigma^2` (`lib.rs:178-190`, `chi2_line` at `lib.rs:254`). Given those two facts, the
algebra below is exact, not speculative.

The accept/reject test used throughout this crate is: spend `k` extra parameters only if
`0.5 * delta_chi2 < lambda * k`. Suppose a `k`-parameter-cheaper description differs from
the `n`-point-richer one by a roughly uniform positional deviation `d` over `n` points, each
of measured uncertainty `sigma`. Then `delta_chi2 ~ n * d^2 / sigma^2`, and the break-even
point is

```
0.5 * n * d^2 / sigma^2 = lambda * k
d = sigma * sqrt(2 * lambda * k / n)
```

`d` is the effective geometric tolerance the objective is willing to accept in exchange for
`k` fewer parameters spread over `n` points — the practical answer to "how far can the curve
drift from the measurement before it costs more than it saves."

**`lambda` enters under a square root.** Doubling `lambda` (say, by moving `precision` from
0.1 to 0.01, a ten-fold tightening) widens `d` by only `sqrt(2) ≈ 1.41`. Combined with
`lambda` itself being a *logarithm* of `extent/precision`, the tolerance the fitter actually
uses is doubly damped against the two things a user might naively expect to control it
directly: image size and output precision. A ten-fold change in `precision` moves `lambda`
by `ln(10) ≈ 2.30` — roughly 27% at `lambda ≈ 8.5` — and the square root halves that again to
about 13% in `d`. This is the reason `docs/DESIGN.md:873` records, of a `lambda` sweep, "That
range is lambda 8.76 to 5.36, a factor of 1.6, and far too narrow to say" — the geometric
behaviour of the fitter is genuinely flat across a wide range of plausible `lambda` values.
Anyone hunting for a quality regression by adjusting `precision`/`lambda` is pulling the
weakest lever in the system; `sigma` (the measurement model) and the per-primitive parameter
counts (`PARAMS_LINE`, the cubic price, …) move the output far more per unit of change.

### The `tau * sigma` envelope

The crate header describes admissibility as a statistical test on the chord (`lib.rs:74-81`):

> "We ask whether every point is *statistically consistent* with the chord:
> `|d_k| <= tau * sigma_k`. Where the boundary was well localized, sigma is small and we
> demand tightness; where it was faint, sigma is large and we simplify hard."

The shipping dynamic program does not test that with an angular cone. The header says so
itself (`lib.rs:92-98`): it described the cut-off as "straightness by incremental cone
intersection" until 2026-09-08; the cone was removed as a correctness bug, and
`DirectionCone` (`lib.rs:300-375`) with `is_admissible` (`lib.rs:535`) survive "as a
reference implementation used by the tests and by `examples/lambda_sweep.rs`" — whose loop
at `examples/lambda_sweep.rs:59` is the only non-test caller. The regression that removed it,
`lib.rs:460-466`:

> "An earlier version pruned with an angular cone on the chord direction instead. That was a
> correctness bug wearing an optimization's clothes: on a 49px straight edge the cone
> excluded the whole-edge segment, and the dynamic program dutifully returned the best of the
> *remaining* options — splitting straight edges at collinear points and returning 12
> segments for a hexagon. A bound tied to the cost function cannot fail that way, because it
> discards only segments the cost function would have rejected anyway."

What `tau` gates today: the tangent windows, which must satisfy `χ²/dof ≤ τ²`
(`tau2 = cfg.tau * cfg.tau` at `tangents.rs:178` and `:224`); the primitive and arc-run
offers, whose chi-squared gate is `tau² · n` (`primitives.rs:736`, documented at
`primitives.rs:688-690`); and `MAX_REDUCED_CHI2 = 4.0`, "`tau²` with the default `tau = 2`"
(`primitives.rs:413-416`). So `tau` still means how many standard deviations of the
measurement a candidate may spend.

### The dynamic program: `optimal_multimodel`

**State.** A bare vertex index into the (decimated, possibly opened) point sequence.
`multimodel.rs:54-70`, verbatim:

> "The state is the **vertex index alone**. The tangent at each vertex comes from the
> polyline itself, estimated once, before the program runs (`crate::tangents`). [...]
> This was chosen over the alternative of a `(vertex, tangent-bin)` state because it is
> **exact**: there is no quantization, the objective is a sum of per-segment and
> per-vertex terms, and the dynamic program is verified against exhaustive enumeration
> of every segmentation and type assignment. The price is that the tangent at a join is
> *estimated* rather than *optimized* jointly with its two segments"

Where a local quadratic centred on a point fits within the noise, both one-sided tangents
are its derivative; across a true corner they come from quadratics fitted to the points
before and after, "because a symmetric window straddling a true corner returns the
bisector, which is the wrong tangent for both neighbouring segments" (`multimodel.rs:55-63`).
The window is the widest whose quadratic fit still satisfies `χ²/dof ≤ τ²`.

**Transitions.** For every ordered pair `(i, j)` with `i < j`, `j − i ≤ max_span`, the
candidate models of `candidates.rs:16-26` are priced:

| model | fitter | cost |
|---|---|---|
| line | total least squares (`scatter_min_eigen`) | `½χ² + 2λ + breaks` (`line_cost_terms`) |
| G1 cubic | area and moment matching, Levien's quartic (`best_cubic`) | `½χ² + 6λ + wobble + over-turn` |
| free cubic (research) | linear least squares, ends pinned (`try_free_cubic`) | `½χ² + 6λ + wobble + breaks + over-turn` |
| circular arc | Kåsa algebraic circle (`CirclePrefix`, `try_arc`) | `½χ² + 5λ + breaks` (7λ under the written-arcs prices) |
| elliptical arc | algebraic ellipse, Sampson distance (`try_ellipse`) | `½χ² + 7λ + breaks` |

"6" is the cubic price in force (`params_cubic`), "5" the arc price in force
(`cost::arc_params`, `cost.rs:182`). "over-turn" is 0 at the default prices and `6λ` under
the experimental written-arcs prices for a cubic whose end directions turn more than 90°
(`over_turn_params`, `candidates/turn.rs:54`; see "Arcs as written" below). A curved model is fitted only when the line
(for the ellipse, the best candidate so far) already costs more than the curved model's
parameter floor `λ·P` — "a proof, not a heuristic, that it could not otherwise win"
(`multimodel/scan.rs:26-30`).

**Recurrence** (`multimodel/scan.rs:8-18`):

```
best[0] = 0
base(i) = best[i] + (i > 0 ? vertex_cost(i) : 0)
best[j] = min over admissible i < j of  base(i) + min_model cost_model(i, j)
```

where a span is admissible when `Limits::allows` it (`multimodel/limits.rs:23`): at most
`max_span` points, and no forced vertex strictly inside. Both are unconstrained in normal
fitting; stage 12's repair sets them. Both conditions are monotone in `j`, so a start refused
once is dropped for good (`multimodel/scan.rs:825`), and the pull and push fills stay
identical bit for bit (the tests in `multimodel/scan.rs` compare them under caps and forced
vertices). Ties go to the smallest `i`; `from[j]`, `kind[j]` and the fitted parameters record the
winner, and the answer is read back from `from[n − 1]` to 0. The per-vertex turn cost
(`vertex_cost`, `tangents.rs:313-318`) is charged once, on leaving `i`: leaving a point
"makes it a vertex, which pays its turn" (`multimodel/scan.rs:848-849`).

**Why the DP is a global optimum over segmentations for a fixed alphabet.** The total cost is
additive over spans and vertices: `sum(segcost(kind, v_q, v_{q+1})) + sum(vertex_cost(v))`.
No term couples two different spans, because the tangent array is fixed *before* the
program runs and does not depend on the segmentation chosen. That is exactly the condition
optimal substructure requires: any optimal segmentation ending at `j` whose last break is at
`i` must itself use an optimal segmentation of `0..i` — otherwise substituting the cheaper
prefix would produce a strictly cheaper whole. This is *verified*, not merely argued:
`tests/multimodel.rs:241` (`matches_exhaustive_search_on_small_inputs`) and `:301`
(`...on_random_inputs`) compare the DP's cost against exhaustive enumeration of every
segmentation and every per-segment type choice.

**Closed loops.** `solve_closed` (`multimodel.rs:1327`) fixes a cut point, solves the open
problem, and tries once more from a better cut; its doc says plainly
(`multimodel.rs:1314-1323`): "The true optimum is a minimum-cost *cycle*; fixing a cut vertex
is an approximation that can cost one segment when the cut lands mid-curve [...] Potrace
solves the cyclic problem exactly; that remains future work." On the small loops of
`closed_program_matches_brute_force_over_cyclic_segmentations`
(`multimodel/tests.rs:251-254`) the two cuts reach the exhaustive optimum over every cyclic
segmentation; that is a test on those loops, not a proof for every ring.

### Complexity and pruning

Nominally O(n²) in the number of measured points, times a constant per candidate span.
These bounds keep it tractable:

1. **The scan cut-off.** The scan from a start `i` stops after `PRUNE_PATIENCE`
   consecutive spans whose line and cubic fidelity terms both exceed
   `PRUNE_SLACK·λ·PARAMS_LINE·(j − i)` (`multimodel/scan.rs:24-26`). `PRUNE_SLACK = 4.0`
   (`lib.rs:127`) and `PRUNE_PATIENCE = 8` (`multimodel.rs:141-147`), whose doc records why
   one exceedance is not enough: "the cubic residual is not [monotone] — the end tangent
   changes with `j`, and a span that fits badly can be followed by a longer one that fits
   well. That was measured, not supposed: with a single-exceedance cut-off the program
   returned a non-optimal S-curve segmentation that exhaustive enumeration caught." The
   value 8 itself is not derived.
2. **Bounds inside the table** (`multimodel/scan.rs:32-68`): a candidate whose price floor
   already reaches the best value at its end needs no residual, and a residual being summed
   stops once the partial sum settles the three questions the program asks of it. The
   module cites Morin & Marsten 1976 (branch and bound in a DP), Killick, Fearnhead & Eckley
   2012 (PELT's per-candidate test, without its permanent pruning, whose condition these
   costs break) and Rakthanmanon et al. 2012 (early abandoning), and marks its handling of
   the cut-off's unknown answers as not from the literature. The result is identical to the
   unbounded sequential fill.
3. **Point decimation**, `DP_MAX_POINTS = 768` (`multimodel.rs:149-152`): longer boundaries
   are decimated for the program only, "one point per sampling cell of `stride` points, with
   sigma divided by `√stride`", keeping significant bends (`crate::decimate`); "A ring of a
   512px image is 1,500 points; eleven of them took eleven seconds" (`multimodel.rs:321-335`).
   The capped and forced variants used by repair are exempt: "the self-intersection repair
   relies on the measured contour being reproducible at `max_span = 1`, a decimated contour is
   not simple by construction, and a forced vertex is an index of the full contour"
   (`multimodel.rs:290-293`).
4. **`max_span` and forced vertices** (`Limits`, `multimodel/limits.rs:23`) — none in normal
   fitting; a finite cap, pins, or both under stage 12's repair. A closed boundary with a
   forced vertex is cut there, which is exact (every admissible solution has that vertex),
   instead of the two-cut heuristic (`multimodel.rs:1346`). An uncapped forced fit runs the
   post-fit passes with the forced vertices kept (`merge_free_cubics_keeping`,
   `merge.rs:365`; `multimodel.rs:412`).

Per-candidate O(1) pricing comes from prefix sums (`Prefix`, `multimodel.rs:486`;
`CirclePrefix`, `candidates.rs:990`) and a residual-sample cap `MAX_RESIDUAL_SAMPLES = 32`
for cubics (`candidates.rs:42`). The elliptical arc is the one candidate that is not O(1):
"Only spans of at least 24 points whose length is a multiple of 16 are tried: the algebraic
fit is O(j − i), so this keeps the ellipse to a sparse grid of candidate ends rather than
making the dynamic program cubic" (`candidates.rs:1271-1273`; constants `MIN_POINTS = 24`,
`LENGTH_STRIDE = 16` at `candidates.rs:1299-1300`). Long polylines (from
`DP_PAR_MIN_POINTS = 128`, `multimodel/scan.rs:78`) share an endpoint's candidates between
idle threads; this "decides only where spans are evaluated, never what is chosen"
(`multimodel/scan.rs:86-92`).

### Why the DP scans every endpoint, and what four cheaper alternatives cost

`multimodel.rs:608-670` records four attempts to make the O(n²) scan cheaper, three refuted:

> "The scan is the tracer's largest single cost — 978,236 spans evaluated on a 768-px
> wordmark to keep 448 segments, a ratio of two thousand to one [...] Four ways of being
> cleverer were tried on the 246-icon gate. Three are refuted, and the fourth says where the
> real constraint is.
>
> - **Cap the reach.** [...] at 64 it saves more but forces extra segments, and the parameter
>   ratio goes to 1.390 against a limit of 1.353. The reach is not the waste.
> - **Assume the optimal predecessor is monotone** — the Knuth/quadrangle condition [...]
>   It does not hold: 11.7% of transitions move the predecessor backwards, by as much as
>   253 points. A window would be wrong, not merely approximate.
> - **Propose breakpoints from a cheap line-only polygon.** Poor recall [...] at a quarter of
>   lambda the polygon proposes 18% of the points and contains only 65% of the breaks this
>   program chooses.
> - **Coarsen where a break may fall.** [...] costs dE00 0.1497 -> 0.2207 at almost
>   unchanged parameter count. Half the resolution, half again the error."

The same comment then follows its own conclusion — a local model with free tangents and a
noise model that knows boundary error is correlated — and records that it was built and is
wrong: "An AR(1)-whitened cubic residual [...] *raises* turning monotonically (+1.42% at
rho=0.25, +3.98% at rho=0.50, +10.02% with the free cubic) where it was predicted to lower
it", the damage concentrated at corners, and a single constant (`INKVEC_WOBBLE_PENALTY=0.8`,
see `candidates::wobble_penalty_factor`) beats it on every axis (`multimodel.rs:643-670`).

### The alphabet

| primitive | params | constant | derivation |
|---|---|---|---|
| Line | 2 | `PARAMS_LINE` (`lib.rs:119`) | "its endpoint (x, y). The start point is shared with the previous segment, so it is not charged twice." |
| Axis-constrained line (research) | 1 | `PARAMS_AXIS_LINE` (`merge/snap.rs:17`) | "A line the drawing constrains to an axis costs one number, not two: the artist writes `h16`, not `L 16,0`." |
| G1 / free cubic | 6 by default | `PARAMS_CUBIC` (`multimodel.rs:131`); the price in force is `params_cubic()` (`candidates.rs:54-58`, `cost.rs`) | "two control points and an endpoint" |
| Smooth (`S`) cubic (research) | 4 | `PARAMS_SMOOTH_CUBIC` (`merge/snap.rs:204`) | the reflection is implied |
| Circular arc | 5 | `PARAMS_ARC` (`curves.rs:194`) | one radius + two flags charged as full parameters + endpoint; see below |
| Circular arc, written-arcs prices (experimental) | 7 | `PARAMS_ARC_WRITTEN` (`curves.rs:220`) | the seven numbers the emitter writes and the benchmark counts; see "Arcs as written" |
| Elliptical arc | 7 | `PARAMS_ELLIPTICAL_ARC` (`curves.rs:227`) | "One more than a cubic, so an ellipse has to be a materially better description of its span, not merely an equal one." |
| `<circle>` | 3 | `PARAMS_CIRCLE` (`primitives.rs:42`) | `cx cy r` |
| `<ellipse>` | 5 | `PARAMS_ELLIPSE` (`primitives.rs:44`) | `cx cy rx ry` plus rotation |
| `<rect>` rounded | 6 | `PARAMS_ROUND_RECT` (`primitives.rs:47`) | "charged as six even though we force `ry = rx`, because the emitted element carries both" |
| `<rect>` plain | 4 | `PARAMS_RECT` (`primitives.rs:49`) | corner radius collapsed to zero |

`PARAMS_ARC`'s derivation (`curves.rs:182-194`) explains a deliberate over-charge:

> "SVG writes seven numbers (`rx ry rotation large-arc sweep x y`), but for a *circular* arc
> `rx == ry` and the rotation is meaningless, so the document carries one radius, two one-bit
> flags and an endpoint. We charge the flags a full parameter each — they are a choice the
> designer has to make when editing — which gives 1 + 2 + 2 = 5. It is deliberately not 3
> (radius + endpoint): a description length that ignored the flags would let an arc undercut
> a cubic on every short run where the two are indistinguishable, which is not the
> compactness the objective is meant to reward."

`FittedPath::params()` (`lib.rs:859`) adds a further `2.0` for the path's own start point;
`multimodel::path_cost` (`multimodel.rs:1493-1498`) does not, since the start point is
shared with whatever precedes it. `choice::boundary_cost` (`choice.rs:106-118`) uses
`path.params()`, start point included.

**Free-tangent cubics are in the alphabet but compiled only into research builds**
(`INKVEC_FREE_CUBIC`, `candidates.rs:60-85`). Their doc records the measured reason: on the
246-icon gate set the free cubic is better on dE00 (-0.21%) and parameter ratio (-1.08%) but
worse on turning (+5.44%), the sawtooth detector; swept against the wobble penalty there is
"no setting that keeps the parameter win and the wobble both", and "by the time turning is
inside the gate the ratio is worse than baseline".

**Per-span arcs.** `try_arc` (`candidates.rs:1113-1125`) Kåsa-fits a circle to the span
(`CirclePrefix`), refuses a radius over a thousand times the span's own size, requires the
points to go round the centre one way only (about eight strides checked), a turn between
1e-3 rad and `MAX_ARC_DEGREES`, and then scores "the arc a renderer would draw: SVG
reconstructs the circle from the two end points and a radius, so the radius written is the
mean end-point distance from the centre, and χ² is measured about the circle `arc_center`
rebuilds from it."

`MAX_ARC_DEGREES = 120.0` (`primitives.rs:51-60`) bounds the longest single-arc sweep:

> "SVG arcs are endpoint-parametrized, and near 180° that parametrization is badly
> conditioned: the centre offset from the chord midpoint is `sqrt(r² − h²)` in the
> half-chord `h`, whose derivative diverges as `h → r`. Shortening the chord of a 40px
> half-circle by 0.05px — one sigma of extraction noise on an endpoint — moves its centre by
> 2px. At 120° the same derivative is 0.58, so the arc shape is as stable as its endpoints."

### Arcs as written (an experiment, not exposed)

At five numbers an arc undercuts a six-number cubic on every span where both fit, but the
document writes it as seven (`A rx,ry rot large,sweep x,y`) and the benchmark counts seven
(`bench/inkvec_bench/svgmodel.py`). On the 246-icon screen set arcs are 35 % of the segments
the tracer writes and 5.6 % of the artists' (r2-compact). `CostModel::with_written_arcs`
(`cost.rs:114`) charges what the document carries. It was measured as an option
(`--arcs-as-written`, with two companions found necessary) and **withdrawn before release**
(2026-10-04, held_a below): no flag or option selects it, and the default prices are
unchanged. The cost model is kept so the experiment can be rerun:

1. **The arc at seven** (`PARAMS_ARC_WRITTEN`, `curves.rs:196-220`), read wherever the arc
   price is read (`cost::arc_params`): `Segment::params`, `try_arc`, the scan's arc floor and
   `segment_cost_direct`, so the branch-and-bound floors stay floors. *Not from the
   literature: a price-consistency fix. See also* Maier, Janda & Schindler (2012), "Minimum
   description length arc spline approximation of digital curves", ICIP 2012,
   doi:10.1109/icip.2012.6467248.
2. **One cubic charged as two past 90° of turn** (`CAP_TURN_DEGREES`,
   `candidates/turn.rs:14-43`; `over_turn_params`, `candidates/turn.rs:54`). Two arcs at seven cost
   `8λ` more than one cubic, about 57 nats at 128 px, against about 15 nats of `½χ²` for the
   0.1 px a single cubic misses a 5.3 px round cap by, so the cap went to one cubic and
   lucide's dE00 rose 12 % (r2-compact). A cubic's radial error on a circular arc grows as
   the sixth power of the sweep: 2.7e-4 of the radius at 90°, 1.5e-3 at 120°, 1.8e-2 at 180°
   (evaluated for the `(4/3)·tan(θ/4)` arms). *Inspired by* Goldapp (1991), "Approximation
   of circular arcs by cubic polynomials", CAGD 8(3):227-238,
   doi:10.1016/0167-8396(91)90007-X; the charge on the cubic's own end tangents, where the
   circle is not known, is not from the literature. Charged in the scan (G1 and free
   cubics), in `segment_cost_direct` and on both sides of the free-cubic merge
   (`merge.rs:491`, `:540`). `tests/cost_scope.rs` reproduces the collapse on two caps and
   its repair.
3. **The parameter price scaled 0.8** (in the withdrawn option, applied as `--lambda-scale 0.8`
   on top of the cost model). Without it the arc change trades colour for parameters.

Measured on gate v2 (Quality, 246 icons, against the v0.2.5 Windows baseline; turning on
the drawn curves, below):

| arc / λ scale / 90° limit | dE00 128 / 512 / 512 opaque | ratio 128 / 512 / 512 opaque | drawn turning, 128 px |
|---|---|---|---|
| 7 / 1.0 / on | +4.1 / +1.7 / +1.7 % (worse) | -5.9 / -3.0 / -2.9 % | -0.9 % |
| 7 / 0.8 / on (the option as measured) | -1.5 / -0.5 / +0.8 % (n.s.) | -4.1 / -1.4 / -1.4 % (better) | -0.04 % |
| 7 / 0.7 / on | -4.1 / -0.9 / -0.6 % | -2.4 / -0.6 / -0.6 % | +0.6 % |
| 7 / 0.6 / on (the proposal) | -7.3 / -2.3 / -1.9 % | -1.4 / +0.3 / +0.2 % | +1.2 % |
| 6 / 1.0 / on | +2.0 / -0.1 / +0.0 % | -4.4 / -1.2 / -1.1 % | — |
| 5 / 1.0 / on (limit alone) | -0.8 / -0.2 / -0.1 % | +1.0 / +0.9 / +0.9 % | -0.1 % |

On top of stage 12's pinned repair (the branch tip, `d0f3343` plus the option) the shipped
row reads dE00 -1.9 / -0.4 / +0.8 % and ratio -4.3 / -1.6 / -1.5 %. The mean hides real
losses on single drawings, mostly where art touches the frame: the boundary points along
the image edge carry little weight, so once an arc costs seven one long cubic over a corner
and the side beside it wins (twemoji/1f7eb at 512 px opaque, dE00 0.003 -> 0.197, the
rounded square's right side bowed; simple-icons/notion at 128 px, 0.50 -> 0.80). lucide reads
+7 % dE00 at 512 px. On the held-out set (held_a, 156 icons, 128 px) it costs colour:
parameter ratio 1.471 -> 1.403 (-4.6 %), but dE00 +2.2 % (macro), worst tenth 0.420 -> 0.447
(+6.5 %), DISTS +11 %, five icons worse by more than 0.1 (openmoji/1F382 +0.41,
simple-icons/picxy +0.40, the two the r2-compact proposal also named). Those come from the
arc price itself, not its companions: arc 7 alone reproduces both (picxy 0.127 -> 0.536,
1F382 0.337 -> 0.756), a corner the arc described becoming a spike or a wedge of lines.
Gating the turn charge on the circle's size (Goldapp's error at the half circle against the
0.05 px sigma) left them unchanged and was dropped. Those losses withdrew the option;
the frame cases should be re-measured after the 2 px border pad (stage 1).

Independently, the gate's turning axis reads every arm with arcs at six or seven
as **worse** by 8-26 %, and that reading is an artefact. `svgeval.structure_signals`
(`bench/svgeval.py:699-713`) takes every `x,y` pair in a path's data as an anchor, so an arc
contributes its radii `rx,ry` and its flags `large,sweep` as two fictitious anchors, with
the turns and lengths between them. The same circle reads 0.157 as two arcs and 0.098 as
four cubics. Arc 7 alone moves the gate's turning +37.4 % at 128 px; read without the
radii and flags +2.1 %; read as the total absolute turning of the drawn, flattened curves
(the same circle 0.1002 and 0.1001, `1/r`) -0.7 %. The table's last column is that last
reading. A fix to the axis (skipping an arc's radii and flags, or flattening) would change
the baseline, so it is the gate owner's to make; the scripts are in the branch report
(`tmp/w3-arcs/turncheck.py`).

### The bow term

`bow_penalty` (`candidates.rs:914-926`) charges the *line* candidate when a circular arc fits
the same span far better:

> "When a circular arc through the same span fits at least four times better
> (`4·χ²_arc < χ²_line`), the line's residuals are not noise: they bow systematically to one
> side. The line is then charged `span·ln 2` nats, one bit per point, the price of describing
> which side of the line each point falls on. Otherwise 0."

```rust
pub(crate) fn bow_penalty(chi2_line: f64, chi2_arc: f64, span: usize) -> f64 {
    if chi2_arc * 4.0 < chi2_line {
        span as f64 * std::f64::consts::LN_2
    } else {
        0.0
    }
}
```

The `4.0` threshold has no stated derivation; the `ln 2` per point does (one bit per
independently-agreeing sign). The circle is fitted wherever the line's residual exceeds one
per point, even below the arc's price floor, "because it is the evidence for the line's bow
penalty" (`multimodel/scan.rs:29-30`).

### The post-fit passes: `merge.rs`, and their gating

They run from one place, `post_fit_passes` (`multimodel.rs:387-445`), on the centred
polyline the fit was computed against:

```rust
if max_span == usize::MAX {
    crate::merge::merge_free_cubics(&mut fit.path, shifted, &fit.vertices, cfg);
    crate::merge::sharpen_corners(&mut fit.path);
    #[cfg(feature = "research")]
    {
        if research::axis() { crate::merge::snap_axis_aligned(/* ... */); }
        if research::g1() { crate::merge::snap_smooth_joins(/* ... */); }
        if structural { crate::structural::simplify_with_poly(/* ... */); }
    }
}
```

| pass | default | gate |
|---|---|---|
| `merge_free_cubics` | **on** | every uncapped fit (`max_span == usize::MAX`) |
| `sharpen_corners` | **on** | same |
| `snap_axis_aligned` | **off** | compiled only with `--features research` (`merge.rs:811-816`), and then only with `INKVEC_AXIS` set |
| `snap_smooth_joins` | **off** | research builds only, with `INKVEC_G1` set |
| structural simplifier | **off** | research builds only, with `INKVEC_STRUCTURAL` set |

Not under a span cap, because the merge re-joins short runs into free cubics that can cross
again and the repair never converged: "1-2 s per round on a 250-point ring at span 7, against
0.2 ms for the program itself (family emoji: 11.8 s in repair)" (`multimodel.rs:397-401`).
`merge_free_cubics` and `sharpen_corners` run a second time inside stage 12's ring-level
repair (`crates/inkvec-cli/src/rings.rs:556-557`), under a segment budget — see `12-repair.md`.

**`merge_free_cubics`** (`merge.rs:351`) replaces short runs of chord/cubic/chord with one
free-tangent cubic, because the DP's own cubic is G1 — its tangent *directions* are inherited
from the vertex estimator, only the arm lengths are fitted — and at a corner the inherited
tangent is wrong. The module doc (`merge.rs:17-27`) gives measured numbers on a traced
rounded square:

```
                             chi2     cost
  G1, tangents inherited    284-303   174-183
  free tangents              95-101    80-83
  the split it chose        111-115   109-111
```

"So a free cubic beats the split by about 25% and beats the constrained cubic by more than
half." Each sweep (`merge_round`, doc at `merge.rs:420-431`) tries, at every segment, runs of
`MAX_RUN` (4) down to 2 segments, longest first; a run qualifies if it covers more than three
and at most `MAX_SPAN` (96) measured points and holds no arc. The first run whose free cubic,
fitted through the run's actual start and end on the path, satisfies

```
½·χ²_new + λ·(params_cubic + BREAK_PARAMS)  <  ½·Σχ²_old + λ·Σparams_old (+ SMOOTH_SLACK·λ)
```

and does not cross itself is spliced in. `BREAK_PARAMS = 2.0` charges the two joins the free
cubic no longer meets smoothly; `SMOOTH_SLACK = 0.0` leaves the objective in charge
(`merge.rs:84-115`). Up to `MAX_ROUNDS` (6) sweeps run, stopping when one merges nothing; the
doc of `MAX_ROUNDS` records why one sweep was not enough: "Traced output was 21% curved where
the ground truth is 78%" (`merge.rs:101-109`). Two pieces of bookkeeping skip work without
changing a merge: a run that already costs no more than the free cubic's parameter floor
`λ·(params_cubic + BREAK_PARAMS)` is never searched (`merge.rs:512-525`, "the dynamic
program's own price-floor argument [...] applied to the merge. Not from the literature"),
and a run turned down once is not tried again with the same vertices (`RejectedRuns`,
`merge.rs:392-409`, local invalidation after Garland & Heckbert 1997; 2,467 of 12,248
attempts on the screen set were such repeats).

### How the free-cubic search stays fast, exactly

`free_cubic` (`merge.rs:134-160`) searches the cubic from the run's fixed start `P0` to its
fixed end `P3` over four numbers: the rotations `r0`, `r1` of its end directions away from
the contour's own directions (chords to the second point in from each end) and the arm
lengths `d0`, `d1` in chords. A coarse grid picks the basin and a compass search
(`FreeCubicSearch::refine`) finishes; cubics that cross themselves, arms outside
`[0.02, MAX_ARM]` and rotations beyond `SEARCH_DEGREES` (100) score infinity. Every candidate
is scored by its residual against the run (`merge/residual.rs:7-19`):

```
χ²_n(B) = Σ_{k=a..=b} (d_k / s_k)²,   d_k = |p_k − B(j*/n)|,   s_k = max(σ_k, 1e-6)
```

with `B(j*/n)` the nearest of the `n + 1` samples of the curve, `n = SAMPLES` (96) for the
residual that decides a merge and `COARSE_SAMPLES` (24) on the grid.

This search is the hottest code of the fit: r2-qspeed (2026-10-01) measured the post-fit
merge at 29 % of all boundary-fit work on the 246-icon screen set, and the residual as the
single hottest function (17.6 % self time) (`merge.rs:55-57`). Since 2026-10 two modules cut
that work **without moving a bit of output**:

- **`merge/residual.rs` — screen, then confirm.** The grid and the compass search only ask
  whether a candidate beats the best so far, `χ² < bound`. The answer is reached in three
  layers (`merge/residual.rs:30-49`): (1) early abandoning — the terms are non-negative, so a
  partial sum that reaches the bound settles it; the points are visited middle-first and
  compared against `bound · REORDER_MARGIN` (`1 + 1e-12`) so the reordered sum also proves the
  in-order one; (2) a screen on a lower bound of each term, `ℓ_k = fl(fl(dist²)/fl(s²)) ·
  (1 − 1e-12)`, from the squared distance the nearest-sample search computes anyway, so a
  rejected candidate never pays for `hypot` (about 6.9 times its native cost per call in the
  wasm32 build); (3) exact confirmation of a survivor: every term recomputed exactly with the
  same sample and summed in point order, the operations of `chi2` itself. Why the answers
  are the same bits (`merge/residual.rs:51-80`): the 1e-12 shrink is about 4,500 ulp, so
  `ℓ_k < T_k` for any `hypot` within about 2,000 ulp while every quantity is a normal number;
  outside `[1e-290, 1e300]` (`SCREEN_FLOOR`, `SCREEN_CEIL`) the screen uses 0; rounded addition
  is monotone, so a screen exit implies the old exit; and a candidate the old code would have
  cut early comes back at least `bound`, which every caller only tests with `<`.
- **`merge/grid.rs` — the coarse grid from cached partial sums.** The grid tries every
  combination of 9 rotations (`ANGLES`, ±90°) at each end by 5 arm lengths (`ARMS`, 0.15–0.8)
  at each: 2,025 candidates, each on 25 samples. `eval_cubic` computes a sample as
  `((b0·P0 + b1·P1) + b2·P2) + b3·P3`; `P1` depends only on `(r0, d0)` and `P2` only on
  `(r1, d1)`, so the first partial sum is cached per start choice and sample, `b2·P2` per end
  choice and sample, `b3·P3` per sample, and a candidate's sample is `(head + tail) + last` —
  the same operations, operands and order, so the same bits (`merge/grid.rs:21-36`). The
  self-crossing test now runs only on a candidate whose residual would make it the new best:
  a crossing candidate still never becomes the best, and the bound evolves identically
  (`merge/grid.rs:39-45`).

Measured (`merge.rs:66-71`, `merge/residual.rs:82-92`): in r2-qspeed's prototype the two cut
the post-fit merge by 46 % (sum over the screen set, one thread), byte-identical on all 246
icons; as shipped, `fit_dp` -5 % and process CPU -7 % at 128 px, -4 % and -4 % at 512 px, and
the repair stage, which runs the same pass on refitted rings, -19 % at 128 px (16 threads,
under heavy load, interleaved against v0.2.4). Literature, as the code labels it: early
abandoning is "Method from" Bei & Gray 1985 (doi:10.1109/TCOM.1985.1096214); the reorder
margin "Method from" Higham 1993 (doi:10.1137/0914050, eq. 2.6); the lower-bound cascade
"Inspired by" Rakthanmanon et al. 2012 (doi:10.1145/2339530.2339576, §4.2.2 and §4.2.4);
the exact confirmation and the partial-sum cache are "Not from the literature", with "See
also" Borges 2019 (arXiv:1904.09481, the cost of `hypot`) and Kolesnikov & Fränti 2007
(doi:10.1016/j.patcog.2006.09.002, §2.4, reuse of a first run's results, there of DP span
costs). Unit tests hold the 0.2.4 implementations as references
(`lower_term_never_exceeds_the_exact_term`, `merge/residual.rs:347`;
`cached_samples_are_the_evaluated_ones`, `merge/grid.rs:236`; and `merge/tests.rs`).

**`sharpen_corners`** (`merge.rs:599`) — takes only the path, no polyline or config — replaces
a short cubic bridging two lines with the lines' actual intersection. Its doc
(`merge.rs:582-598`) explains why the DP produces these chamfer cubics:

> "The contour samples around a corner lie on the coverage level set, which rounds the corner
> off by about a pixel. To the residual those samples *are* a small fillet, so a cubic through
> them beats two lines that miss them — the residual cannot tell a rasterised sharp corner
> from a sub-pixel fillet. The prior settles it: icon and logo artists draw corners; a fillet
> under two pixels at this scale is not something they draw, it is something the renderer
> did."

Guarded geometrically, not by residual: the two lines must turn by at least
`SHARPEN_MIN_TURN`, their intersection must lie ahead of the first line and behind the
second, and it must sit within a chamfer allowance of the cubic's endpoints. Two shapes are
recognised: a cubic that *is* the chamfer (chord ≤ `SHARPEN_MAX_CHORD = 2.5`), or a short
straight edge with a chamfer cubic at each end (chord ≤ `SHARPEN_MAX_EDGE = 8.0`)
(`merge.rs:565-574`). `SHARPEN_MIN_TURN` *is* `crate::CORNER_TURN_MIN` (`merge.rs:575-580`):
"They were independent copies of `PI / 6.0` until 2026-09-08."

**The research snaps** (`merge/snap.rs`, compiled only with `--features research`):

- **`snap_axis_aligned`** (`merge/snap.rs:52`) puts a line the measurement cannot
  distinguish from horizontal or vertical exactly onto the axis, at `PARAMS_AXIS_LINE = 1.0`
  instead of 2. Corpus motivation: "68% of lucide's [straight segments]" are exactly
  axis-aligned (`merge/snap.rs:26`). Accepted when
  `0.5 * (chi2_axis - chi2_free) < lambda * (PARAMS_LINE - PARAMS_AXIS_LINE)`
  (`merge/snap.rs:41`), and guarded by line-line joins only (a move caught on
  `simple-icons/atlassian`, `:91`), a displacement bound of the moved point's own sigma
  (`room`, `:124`), and `MAX_AXIS_DEV_SIGMA = 3.0` per sample (`:22`; caught on
  `lucide/bath`, `:136`). Measured: "snapping a third of all lines moved mean dE00 by
  -0.001" (`:51`).
- **`snap_smooth_joins`** (`merge/snap.rs:236`) offers the SVG `S` shorthand at
  `PARAMS_SMOOTH_CUBIC = 4.0`; corpus motivation: "60% have equal handle lengths either side"
  of joins smooth to a thousandth of a degree (`:209`). Its accept budget reads the cubic
  price in force (`2λ·(params_cubic − 4)`, `:248`, `:309`); a join is refitted only when its
  handles are within 20 degrees of smooth (an inline literal, `:344`, a filter rather than
  the decision), by a four-coordinate `compass_search` (`:354`).

### Curve or primitive: `inkvec_fit::choice`

Every boundary gets two candidate descriptions (`choice.rs:4-16`): the **curve**, the
dynamic program's path, scored by `boundary_cost` (`choice.rs:106-118`, `½χ² + λ·P` with
`χ²` sampled every quarter pixel and `P = path.params()`); and the **primitive**, the
cheapest whole-ring circle, ellipse or (rounded) rectangle, or a run of arcs
(`fit_primitive_or_arcs`, `primitives.rs:720`), which prices itself. The primitive wins when
its cost is strictly below the curve's (`choose`, `choice.rs:211-226`); a tie or a NaN keeps
the curve. The CLI reaches the choice through `describe` (`choice.rs:243-272`), called from
`fit_boundaries` (`crates/inkvec-cli/src/pipeline.rs:710-738`), which avoids work the choice
discards:

1. **The image frame.** When one face touches every border pixel (the background of 166 of
   the 246 screen icons), that boundary is a closed ring on the image rectangle
   (`lies_on_frame`, `choice.rs:196-209`: every point on `x = −0.5`, `x = W − 0.5`,
   `y = −0.5` or `y = H − 0.5`). Its dynamic program "ran both of its cuts over hundreds of
   points and was then thrown away: the rectangle won 166 times out of 166"
   (`choice.rs:20-26`). For the frame the primitive search runs first, and when its cost is
   below `cost_floor`, a lower bound on what *any* fitted path of the polyline can cost, the
   dynamic program is not run at all; otherwise the program runs and the choice is made as
   before.
2. **Every other boundary.** The dynamic program and the primitive search run side by side
   under `rayon::join`: the program on the calling thread, the search offered to an idle one,
   so the boundary's wall time falls from the sum of the two towards the longer.

`cost_floor` (`choice.rs:120-149`) is `min(6λ, (2 + c)λ, 4λ + ½·L)`, with `c` the cubic price
in force and `L` a lower bound on the χ² of any single straight segment — Pearson's smallest
eigenvalue of the points' weighted scatter matrix, halved and shrunk by an absolute
`1e-9·(tr S + Σw·C²)` slack for rounding (`line_chi2_floor`, `choice.rs:151-194`): every
segment costs at least two parameters and the start point two, so two or more segments cost
at least `6λ`, one cubic `(2 + c)λ`, one arc at least `7λ`, and one line `4λ` plus its
residual. Both steps are exact: step 2 computes the same two pure functions and compares
them as before; step 1 skips the program only when the primitive provably wins, assuming
only that the program's path has finite coordinates (`choice.rs:34-43`). Measured
(`choice.rs:44-74`; 16 threads, under heavy load, `fit_dp` stage time against v0.2.4,
including the merge search above): -40 % sum / -37 % median at 128 px (246 screen icons),
-20 % / -22 % at 512 px (51 images), -10 % / -19 % on three transparent 2048 px images, and
+10 % / +2 % on seven opaque 2048 px images, which the comment reads as noise: there hundreds
of rings keep every thread busy. Literature, as labelled in the code: "Inspired by" Morin & Marsten 1976
(doi:10.1287/opre.24.4.611, branch and bound in dynamic programming), "Method from" Pearson
1901 (doi:10.1080/14786440109462720) and Blumofe & Leiserson 1999
(doi:10.1145/324133.324234, work stealing); the parameter-count floor itself is "Not from the
literature".

How much the primitive path is worth, measured where it is computed
(`primitives.rs:697-720`): on the 246-icon gate set a primitive is offered on 606 of 6,898
boundaries and wins 595 of the offers (98.2 %); removing the path entirely costs 30.52 % of
the parameter ratio (1.4818 -> 1.9341) and 9.01 % of dE00.

### Corner handling: `adjust_vertices_at`, `spans_loop`, the 3-sigma cap

Marching squares cannot represent a sharp corner — the level set that produced the measured
polyline cuts across the corner pixel, chamfering it over a point or two.
`adjust_vertices_at` (`lib.rs:648`, doc at `lib.rs:609-647`) moves a chosen vertex to the
intersection of its two adjacent fitted lines instead of trusting the measured point:

> "Marching squares cannot represent a sharp corner: the level set cuts across the corner
> pixel, chamfering it over a point or two. Trusting those measured points as vertices both
> rounds the corner and costs an extra segment to cross the chamfer — measured effect, a
> hexagon coming back with 12 vertices instead of 6." (`lib.rs:592-595`)

Restricted to actual corners — a correctness requirement, because intersecting two
nearly-parallel fitted lines is ill-conditioned (`lib.rs:611-621`):

> "On a smooth curve they are nearly parallel — 9.7 degrees apart on a 37-segment circle —
> and the intersection runs off far from the curve. [...] Displaced vertices made the
> polygon's turn angles erratic, corner detection then fired every few vertices, a circle was
> cut into sixteen short runs, and no run was long enough for a cubic to be worth its
> parameters. Curve fitting looked broken; the actual fault was here."

In the multimodel fit only a vertex between two `Line` segments moves (`adjust_line_corners`,
`multimodel.rs:967-993`): "A vertex touching a cubic or an arc stays where it was measured,
because the curve was fitted to pass through it." The cap there is
`max_shift = 3·max(σ_max, 0.25)` px (`multimodel.rs:991`), plus, where the two lines turn by at
least `CORNER_TURN_MIN` (30 degrees), a chamfer allowance
`min(3, CORNER_CHAMFER / max(0.2, sin(½(π − turn))))` (`lib.rs:634-637`, code at `:703-709`).
Its comment (`lib.rs:694-701`) gives the measurement behind the allowance:

> "Measured on a 6 px bar at 20 degrees: samples 1.0 px from each true corner, chords 0.12 px
> too far in. A cap of 3 sigma (0.86 px there) refused every one of those intersections."

The `0.25` floor, the `0.2` divisor floor and the `3` cap have no stated derivation.

**The loop-cut index bug**: `spans_loop` (`lib.rs:560-582`) exists because a closed ring
solved by the DP is *opened at a cut* before solving, and comparing `v.first() == v.last()`
alone silently exempted the cut vertex — usually the sharpest corner in the shape — from
corner adjustment and smooth-join treatment:

> "That second spelling is every closed contour in the pipeline, and the cut is placed at the
> sharpest corner; comparing indices alone therefore exempted exactly the most corner-like
> vertex of every shape from corner adjustment and from smooth joins. Measured on a rotated
> bar: the cut corner stayed 0.77 px inside the true corner while its neighbours were
> recovered to within 0.1 px."

### `solve_open` and `refine`

`solve_open` (`multimodel.rs:671`) is the DP body described above, filled by
`multimodel/scan.rs`. `refine` (`multimodel.rs:944`, doc at `:930-943`) is the post-DP
continuous-parameter pass, run once the discrete decisions are fixed:

> "1. line–line corners move to the intersection of their fitted lines
> ([`adjust_vertices_at`], as in [`crate::fit_path`]);
> 2. each cubic's arms are polished against the full residual ([`polish_arms`]);
> 3. at joins the program left smooth (break below [`G1_BREAK_DEGREES`]) the shared
> tangent is re-estimated symmetrically, or set to the adjacent line's direction, so the
> emitted path is exactly G1 there; accepted only when it does not cost more residual than
> the break it removes."

## Constants and thresholds

Values that carry a stated numeric derivation are marked **derived**; values whose existence
is explained but whose specific number is not are marked **motivated**; values with neither
are marked **none**. Paths are under `crates/inkvec-fit/src/`.

| name | file:line | value | controls | derivation |
|---|---|---|---|---|
| `PARAMS_LINE` | `lib.rs:119` | 2.0 | line parameter cost | derived |
| `PRUNE_SLACK` | `lib.rs:127` (shared by `multimodel.rs:139`) | 4.0 | safety factor on the scan cut-off | motivated; value none |
| `CORNER_CHAMFER` | `lib.rs:585` | 1.0 px | corner-adjustment chamfer allowance | derived (one pixel = level-set sampling step) |
| `CORNER_TURN_MIN` | `lib.rs:588` | π/6 (30°) | when a vertex meeting is treated as a corner | none |
| `CORNER_DEGREES` | `lib.rs:921` | 45.0 | corner-vs-smooth-join threshold of `fit_path` | none |
| max-shift factor | `multimodel.rs:991` (inline) | 3 × max(σ), floor 0.25 | corner intersection displacement cap | motivated; 3 and 0.25 none |
| `PARAMS_CUBIC` | `multimodel.rs:131` | 6.0 | default cubic parameter cost (`--bezier-cost` overrides per trace) | derived |
| `PRUNE_PATIENCE` | `multimodel.rs:147` | 8 | consecutive over-budget spans before the scan stops | empirical; value none |
| `DP_MAX_POINTS` | `multimodel.rs:152` | 768 | decimation threshold | none |
| `DP_PAR_MIN_POINTS` | `multimodel/scan.rs:78` | 128 | shortest polyline whose scan is shared between threads | motivated |
| `G1_BREAK_DEGREES` | `tangents.rs:23` | 10.0 | tangent break below which a join is nearly free (`--corner-angle` overrides) | none; swept empirically |
| `TANGENT_WINDOW_MAX` | `tangents.rs:26` | 16 | widest one-sided tangent window | none |
| `MAX_RESIDUAL_SAMPLES` | `candidates.rs:42` | 32 | cubic residual evaluation points (O(1) cap) | none |
| `NEWTON_STEPS` | `candidates.rs:45` | 3 | Newton steps for point-to-cubic projection | none |
| `MAX_ARM` | `candidates.rs:48` (used by `merge.rs:132`) | 1.0 | largest control arm as a fraction of chord | derived ("more than a half turn") |
| `FREE_MAX_SWING` | `candidates.rs:52` | 75.0° | how far a free cubic's tangent may depart from the estimate | none |
| `DIRECTION_SAMPLES` | `candidates.rs:1137` | 8 | monotone-sweep samples for arc validity | asserted, not derived |
| `MAX_ASPECT` | `candidates.rs:1298` | 12.0 | most elongated ellipse worth fitting | none |
| `MIN_POINTS` (ellipse) | `candidates.rs:1299` | 24 | minimum points to try an ellipse | none |
| `LENGTH_STRIDE` | `candidates.rs:1300` | 16 | ellipse candidate-length sampling | motivated (keeps the O(n) fit to a sparse grid) |
| bow-penalty factor | `candidates.rs:921` (inline) | 4.0 | arc-vs-line residual ratio that triggers the bow penalty | none |
| `PARAMS_ARC` | `curves.rs:194` | 5.0 | circular arc cost (default prices) | derived |
| `PARAMS_ARC_WRITTEN` | `curves.rs:220` | 7.0 | circular arc cost under the written-arcs prices (experimental, not exposed) | the numbers SVG writes |
| `CAP_TURN_DEGREES` | `candidates/turn.rs:43` | 90.0 | turn past which one cubic is charged as two, under the written-arcs prices | derived (cubic circle error ∝ sweep⁶, Goldapp 1991); the 90 not swept |
| `PARAMS_ELLIPTICAL_ARC` | `curves.rs:227` | 7.0 | elliptical arc cost | derived |
| `PARAMS_CIRCLE` | `primitives.rs:42` | 3.0 | `<circle>` cost | derived |
| `PARAMS_ELLIPSE` | `primitives.rs:44` | 5.0 | `<ellipse>` cost | derived |
| `PARAMS_ROUND_RECT` | `primitives.rs:47` | 6.0 | rounded `<rect>` cost | derived |
| `PARAMS_RECT` | `primitives.rs:49` | 4.0 | plain `<rect>` cost | derived |
| `MAX_ARC_DEGREES` | `primitives.rs:60` | 120.0 | longest single-arc sweep | derived (conditioning argument) |
| `MAX_REDUCED_CHI2` | `primitives.rs:416` | 4.0 | primitive acceptance gate | derived (`tau²` at default `tau = 2`) |
| `BREAK_PARAMS` | `merge.rs:88` | 2.0 | joins a free cubic no longer meets smoothly | derived |
| `MAX_SPAN` | `merge.rs:92` | 96 | longest merge run attempted, in measured points | motivated (bounds the pass at O(n · span)) |
| `MAX_RUN` | `merge.rs:99` | 4 | segments a merge run may absorb | none (once overridable, never swept) |
| `MAX_ROUNDS` | `merge.rs:109` | 6 | merge sweeps | motivated; value none |
| `SMOOTH_SLACK` | `merge.rs:115` | 0.0 | parameters a merged curve may lose by and still be taken | measured (the `INKVEC_SMOOTH` experiment; the default never moved) |
| `SAMPLES` | `merge.rs:118` | 96 | curve samples of the residual that decides a merge | none |
| `SEARCH_DEGREES` | `merge.rs:126` | 100.0 | free-cubic rotation search width | motivated (a 60° clamp put the optimum outside the search); value none |
| `COARSE_SAMPLES` | `merge.rs:129` | 24 | curve samples while ranking grid candidates | none |
| `ANGLES` | `merge/grid.rs:83` | ±90, ±65, ±45, ±22, 0 (°) | the grid's rotations of each end direction | none |
| `ARMS` | `merge/grid.rs:86` | 0.15, 0.3, 0.45, 0.6, 0.8 | the grid's arm lengths, in chords | none |
| `SCREEN_FLOOR`, `SCREEN_CEIL` | `merge/residual.rs:204`, `:208` | 1e-290, 1e300 | range where the lower-bound screen is used (0 outside) | derived (normal-number range of the error bound) |
| `SCREEN_SHRINK` | `merge/residual.rs:213` | 1 − 1e-12 | lower-bound margin below the exact term | derived (~4,500 ulp, covers `hypot` errors up to ~2,000 ulp) |
| `REORDER_MARGIN` | `merge/residual.rs:246` | 1 + 1e-12 | reordered partial sum against the bound | derived (Higham 1993, eq. 2.6) |
| `SHARPEN_MAX_CHORD` | `merge.rs:569` | 2.5 | chamfer-cubic chord ceiling | semi-derived (chamfer ~1 px per side) |
| `SHARPEN_MAX_EDGE` | `merge.rs:574` | 8.0 | short-edge-with-chamfers ceiling | none |
| `SHARPEN_MIN_TURN` | `merge.rs:580` | `CORNER_TURN_MIN` | corner-vs-smooth threshold | shared with `CORNER_TURN_MIN` by design |
| `PARAMS_AXIS_LINE` (research) | `merge/snap.rs:17` | 1.0 | axis-snapped line cost | derived |
| `MAX_AXIS_DEV_SIGMA` (research) | `merge/snap.rs:22` | 3.0 | per-sample axis-snap deviation cap | none |
| `PARAMS_SMOOTH_CUBIC` (research) | `merge/snap.rs:204` | 4.0 | `S`-shorthand cost | derived |
| G1 pre-filter angle (research) | `merge/snap.rs:344` (inline) | 20° | when a smooth-join candidate is worth the refit | none |
| cost-floor slack | `choice.rs:187` (inline) | `1e-9·(tr S + Σw·C²)` | absolute shrink of the single-line χ² floor | derived (dwarfs the rounding of the scatter and the sampled χ² below 2^20 points) |
| `FLATTEN` | `simple.rs:63` | 16 | flattening resolution for the self-crossing test | motivated (below render-visibility floor); value none |
| `EPS` (endpoint coincidence) | `simple.rs:67` | 1e-6 px | adjacency exemption tolerance | derived |

## Failure modes and edge cases

- **The hexagon regression**: an angular-cone pruning bound that was not tied to the cost
  function doubled a hexagon's segment count (12 instead of 6) before being replaced by the
  cost-derived cut-off — `lib.rs:460-466`, quoted above.
- **Free-tangent cubics** buy colour and parameters but fail the turning axis at every wobble
  setting (`candidates.rs:60-83`), and a correlated-noise model for the cubic residual made
  turning worse, not better (`multimodel.rs:643-670`).
- **A merge pass misaligned with its vertices** raised parameters 34 % "on a pass whose whole
  purpose is to remove segments" before the vertex list was kept in step (`merge.rs:375-378`).
- **Fitting a free cubic through the raw contour point** instead of the refined vertex scored
  a curve that is not the one emitted: "DISTS worse on 125 of 180 real emoji, with the
  parameter count going *up*" (`merge.rs:137-142`).
- **`snap_axis_aligned` regressions on `simple-icons/atlassian`** (a moved shared vertex) and
  **`lucide/bath`** (a trend hidden under the aggregate chi2 budget) — why that research pass
  carries three independent guards rather than one chi2 test.
- **A closed loop is only approximately optimal.** The DP's exactness argument does not
  extend to `solve_closed`'s cut heuristic, though the two cuts match brute force on the
  test loops.

## Environment overrides

Since the settings cleanup (CHANGELOG, 0.2.0, *Changed*) the engine reads its environment through one helper (`inkvec_core::env`): a switch is off when unset, empty or `0`, and every variable is read once per process. Variables marked *removed* below are gone (their defaults are constants now); those marked *research build* are read only by a binary built with `--features research`. The full list, with what is left and why, is [`docs/internal/env-vars.md`](../internal/env-vars.md).

| variable | effect | default when unset |
|---|---|---|
| `INKVEC_AXIS` (*research build*) | enables `snap_axis_aligned` | off |
| `INKVEC_G1` (*research build*) | enables `snap_smooth_joins` | off |
| `INKVEC_FREE_CUBIC` (*research build*) | enables the free-tangent cubic candidate in the DP | off |
| `INKVEC_STRUCTURAL` (*research build*) | enables the structural simplifier after the merge passes | off |
| `INKVEC_G1DBG` (*research build*) | prints per-join accept/reject diagnostics for `snap_smooth_joins` | off |
| `INKVEC_DPDBG` | prints the dynamic program's decisions (`multimodel.rs:133-136`) | off |
| `INKVEC_NO_ARCS` (*removed*) | disabled per-span arcs | arcs on |
| `INKVEC_MERGE_BREAK` (*removed*) | overrode `BREAK_PARAMS` | `2.0` |
| `INKVEC_PARAMS_CUBIC`, `INKVEC_G1_BREAK` (*removed*) | the cubic price and the corner angle, now `--bezier-cost` and `--corner-angle` (`cost.rs:15-21`) | 6, 10° |

`--precision` and `--tau` (`crates/inkvec-cli/src/args.rs`) set `FitConfig` via
`from_precision`; see the objective section above.

## Open questions

- **The `d = sigma*sqrt(2*lambda*k/n)` tolerance formula is not written anywhere in the
  source.** It is derived here from the objective the code implements
  (`0.5*delta_chi2 < lambda*delta_params`), and should be read as this document's own
  derivation, not a quoted fact.
- **Closed loops**: `solve_closed` is a two-cut heuristic; the cyclic problem Potrace solves
  exactly "remains future work" (`multimodel.rs:1323`).
- **`snap_axis_aligned`, `snap_smooth_joins`, the free cubic and the structural simplifier
  are research-only.** Each was measured on the corpus and does not ship; a release build
  does not compile the snaps.
- **Several constants central to the search bound and alphabet gates
  (`MAX_RESIDUAL_SAMPLES`, `DP_MAX_POINTS`, `TANGENT_WINDOW_MAX`, `MAX_RUN`, `SEARCH_DEGREES`,
  `ANGLES`, `ARMS`, `SHARPEN_MAX_EDGE`, the bow-penalty `4.0`) have no stated numeric
  derivation** — the need for the guard is explained, but not why this particular number
  rather than a nearby one.
