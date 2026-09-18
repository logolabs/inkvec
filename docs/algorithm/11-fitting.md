# Stage 11 — Curve fitting

> Turns a measured boundary — a polyline with a per-point uncertainty — into the smallest
> description-length combination of lines, cubics, arcs and constrained primitives that
> still explains the measurement.

**Source:** `crates/inkvec-fit/src/lib.rs`, `multimodel.rs`, `primitives.rs`, `curves.rs`,
`simple.rs`, `merge.rs`
**Entry point:** `optimal_multimodel()` (`crates/inkvec-fit/src/multimodel.rs:202`), called
from `crates/inkvec-cli/src/lib.rs:868` for the colour path and `lib.rs:607` for strokes.
**Pipeline position:** after decode and symmetry enforcement (stages 9–10), before repair
(stage 12). Stage mark `"fit_dp"` (`crates/inkvec-cli/src/lib.rs:899`).

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

**`FitConfig`** (`lib.rs:47-61`) has exactly two fields:

| field | meaning |
|---|---|
| `tau` | confidence multiplier on the per-point sigma; the default `2.0` "admits a chord that stays within ~2 standard deviations of every measurement" |
| `lambda` | cost per emitted parameter, in nats — the exchange rate between fidelity and description length |

**Output:** `FittedPath` (a sequence of `curves::Segment`: `Line`, `Cubic`, `Arc`) from
`optimal_multimodel`, or the richer `MultimodelFit` from `optimal_multimodel_full`
(`multimodel.rs:187-199`):

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

`SegKind` is `Line`, `Cubic`, or `Arc` (circular and elliptical arcs share the tag; only the
stored parameters differ). `optimal_multimodel_capped` / `_capped_full` add a `max_span`
argument — the point-span cap stage 12's repair uses; see `12-repair.md`.

## How it works

### The objective, and where `lambda` comes from

The cost of a candidate description is

```
cost = 0.5 * chi2 + lambda * params
```

`chi2` is the weighted sum of squared residuals against the measured points (each weighted
by its own `1/sigma^2`); `params` is the count of numbers the description would write.
`FitConfig::from_precision` (`lib.rs:63-81`) derives `lambda`, verbatim:

> "MDL measures description length in nats. A coordinate confined to a range `extent` and
> stored to resolution `precision` carries `ln(extent / precision)` nats of information.
> For a 256px canvas written at 0.1px precision that is `ln(2560) ~ 7.85` — not the 1.0 a
> first guess suggests, and the difference is roughly a factor of two in emitted segment
> count."

```rust
pub fn from_precision(extent: f64, precision: f64, tau: f64) -> Self {
    let ratio = (extent / precision.max(f64::MIN_POSITIVE)).max(std::f64::consts::E);
    Self { tau, lambda: ratio.ln() }
}
```

The `.max(std::f64::consts::E)` floor means `lambda` is never below `1.0`, however small
`extent/precision` gets. The default (`lib.rs:83-88`) is a 256 px canvas at 0.1 px output
precision, `tau = 2.0`.

In the shipping CLI, `extent` is the intake raster's `max(width, height)` in pixels, and
`precision`/`tau` come from `--precision` (default `0.1`) and `--tau` (default `2.0`)
(`crates/inkvec-cli/src/args.rs:62-63`, `crates/inkvec-cli/src/lib.rs:374`). The
content-units variant (`crates/inkvec-cli/src/lib.rs:130-136`) scales both `precision` and
`lambda` by a further factor `s` so that "lambda [stays] at `ln(REF_EXTENT / precision)`
whatever the extent" while the point-count term absorbs the scale.

At a 512 px intake with defaults: `lambda = ln(5120) ≈ 8.541`. At 2048 px: `ln(20480) ≈
9.927`. Note how little `lambda` moves for a 4x change in image size — this is the first
sign of the fact below.

### The effective geometric tolerance — and why the square root matters

No line in the repo writes `d = sigma * sqrt(2*lambda*k/n)` as a formula — this is a
derivation, not a quotation — but every quantity in it is pinned to code: the cost function is
`cost = 0.5 * chi2 + lambda * params` (`lib.rs:73-77`), applied identically at the DP's
per-candidate comparisons (`lib.rs:896,909`, `multimodel.rs:795,808,1310`, `primitives.rs:691`)
and at merge's pairwise accept test (`merge.rs:684-687`, `merge.rs:862`); and `chi2` is the sum
of squared residuals each weighted by `1/sigma^2` (`lib.rs:137`, `chi2_line` at `lib.rs:162`).
Given those two facts, the algebra below is exact, not speculative.

The accept/reject test used throughout this crate is: spend `k` extra parameters only if
`0.5 * delta_chi2 < lambda * k` (see e.g. `merge.rs`'s repeated
`0.5 * (chi2_a - chi2_b) < lambda * (params_a - params_b)` pattern, and the DP's own
per-candidate cost comparison). Suppose a `k`-parameter-cheaper description differs from the
`n`-point-richer one by a roughly uniform positional deviation `d` over `n` points, each of
measured uncertainty `sigma`. Then `delta_chi2 ~ n * d^2 / sigma^2`, and the break-even point
is

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
about 13% in `d`. This is the reason `docs/DESIGN.md:872` records, of a `lambda` sweep, "that
range is lambda 8.76 to 5.36, a factor of 1.6, and far too narrow to say" — the geometric
behaviour of the fitter is genuinely flat across a wide range of plausible `lambda` values.
Anyone hunting for a quality regression by adjusting `precision`/`lambda` is pulling the
weakest lever in the system; `sigma` (the measurement model) and the per-primitive parameter
counts (`PARAMS_LINE`, `PARAMS_CUBIC`, …) move the output far more per unit of change.

### The `tau * sigma` admissibility envelope — and a gap between the module doc and the code

The crate's top-of-file doc (`lib.rs:13-19`) frames admissibility as a geometric test:

> "We ask whether every point is *statistically consistent* with the chord:
> `|d_k| <= tau * sigma_k`. Where the boundary was well localized, sigma is small and we
> demand tightness; where it was faint, sigma is large and we simplify hard."

and describes the mechanism as "straightness by incremental cone intersection"
(`lib.rs:28`), implemented by `DirectionCone` (`lib.rs:186-260`), whose own doc explains the
incremental-cone-intersection algorithm in detail. **This is no longer how the shipping
dynamic program tests admissibility.** `DirectionCone` is constructed in exactly one
production function, `is_admissible` (`lib.rs:398-412`), whose own doc says: "Exposed because
an independent, obviously-correct implementation of the admissibility rule is what lets the
tests verify the fast path" — and its only caller outside tests is
`crates/inkvec-fit/examples/lambda_sweep.rs:59`, a reference greedy segmenter, not
`optimal_multimodel`.

The reason the cone left the DP is recorded as a correctness regression, `lib.rs:322-329`:

> "An earlier version pruned with an angular cone on the chord direction instead. That was a
> correctness bug wearing an optimization's clothes: on a 49px straight edge the cone
> excluded the whole-edge segment, and the dynamic program dutifully returned the best of the
> *remaining* options — splitting straight edges at collinear points and returning 12
> segments for a hexagon. A bound tied to the cost function cannot fail that way, because it
> discards only segments the cost function would have rejected anyway."

What actually gates admissibility today is the objective itself, expressed through `tau` in
four places: the tangent-window acceptance test in the tangent estimator
(`tau2 = cfg.tau * cfg.tau`, `multimodel.rs:495`, `:2272`), the primitive-fit acceptance gate
(`primitives.rs:1341`), and `MAX_REDUCED_CHI2 = 4.0`, documented as "`tau²` with the default
`tau = 2`" (`primitives.rs:1078-1081`). So `tau` still means the same thing — how many
standard deviations of the measurement a candidate is allowed to spend — but the module doc's
description of the mechanism is stale for the alphabet actually in use.

### The dynamic program: `optimal_multimodel`

**State.** A bare vertex index into the (decimated, possibly opened) point sequence.
`multimodel.rs:52-69`, verbatim:

> "The state is the **vertex index alone**. The tangent at each vertex comes from the
> polyline itself, estimated once, one-sidedly, before the program runs... This was chosen
> over the alternative of a `(vertex, tangent-bin)` state because it is **exact**: there is
> no quantization, the objective is a sum of per-segment and per-vertex terms, and the
> dynamic program is verified against exhaustive enumeration of every segmentation and type
> assignment. The price is that the tangent at a join is *estimated* rather than *optimized*
> jointly with its two segments."

The tangent at each vertex is estimated one-sidedly — `t-` from a local quadratic fitted to
the points *before* the vertex, `t+` likewise from the points *after* — because "a symmetric
window straddling a true corner returns the bisector, which is the wrong tangent for both
neighbouring segments." The window itself is the widest whose quadratic fit still satisfies
`chi2/dof <= tau^2`, so it is wide on smooth runs and narrow beside corners.

**Transitions.** For every ordered pair `(i, j)` with `i < j <= min(i + max_span, n-1)`, up
to five candidate primitives are priced (`multimodel.rs:1880-2003`):

1. **Line** — cost `0.5*chi2 + lambda*PARAMS_LINE + break_cost(t+_i, chord) + break_cost(chord, t-_j)`, plus the bow penalty (below).
2. **G1 cubic** — tangent directions inherited from the vertex estimator, only the two arm
   lengths fitted; gated on `line_cost > cubic_floor` so a cubic is only tried where a line
   is already expensive.
3. **Free-tangent cubic** — both end tangents fitted freely; off by default (see below).
4. **Circular arc** — an O(1) algebraic (Kåsa) fit re-scored about the drawn circle.
5. **Elliptical arc** — a heavier, non-O(1) conic fit tried only every 16th candidate length.

**Recurrence.**

```
best[0] = 0
base(i)  = best[i] + (i > 0 ? vertex_cost(tan, i, cfg) : 0)
best[j]  = min over i<j, over kind in {Line, Cubic, Arc, Ellipse} of
           base(i) + segcost(kind, i, j)
```

The per-vertex turn cost is charged once, on *leaving* `i` — "leaving `i` makes it a vertex;
that is when its turn is paid" (`multimodel.rs:1875`).

**Backpointers.** Five parallel arrays (`from`, `kind`, `arms`, `tans`, `arcs`) are written
whenever a cheaper cost is found at `j`; reconstruction walks backwards from `n-1` to `0` and
reverses.

**Why the DP is a global optimum over segmentations for a fixed alphabet.** The total cost is
additive over spans and vertices: `sum(segcost(kind, v_q, v_{q+1})) + sum(vertex_cost(v))`.
No term couples two different spans, because the tangent array `tan` is fixed *before* the
program runs and does not depend on the segmentation chosen (`multimodel.rs:54`, quoted
above). That is exactly the condition optimal substructure requires: the optimal cost to
reach `j` decomposes as `min_i [ opt(i) + vertex_cost(i) + min_kind segcost(kind, i, j) ]`,
because any optimal segmentation ending at `j` whose last break is at `i` must itself use an
optimal segmentation of `0..i` — otherwise substituting the cheaper prefix would produce a
strictly cheaper whole, a contradiction. This is *verified*, not merely argued:
`tests/multimodel.rs:221` (`matches_exhaustive_search_on_small_inputs`) and `:281`
(`...on_random_inputs`) compare the DP's cost against exhaustive enumeration of every
segmentation and every per-segment type choice, to `1e-7` relative tolerance.

**The one place the argument is approximate: closed loops.** `solve_closed`
(`multimodel.rs:2575`) fixes a cut point, solves the open problem, and tries once more from a
better cut; its own doc says plainly (`multimodel.rs:2565-2574`): "The true optimum is a
minimum-cost *cycle*; fixing a cut vertex is an approximation that can cost one segment when
the cut lands mid-curve... Potrace solves the cyclic problem exactly; that remains future
work."

### Complexity and pruning

Nominally O(n^2) in the number of measured points, times a constant per candidate span (two
cubic evaluations). Three separate bounds keep this tractable:

1. **The scan cut-off** (`multimodel.rs:2035-2048`): "covering `i..j` with the finest
   segmentation costs at least `2*lambda*(j-i)`, so once both models' fidelity terms alone
   exceed that by the slack, no longer span from `i` can win." Controlled by `PRUNE_SLACK =
   4.0` and `PRUNE_PATIENCE = 8` consecutive over-budget candidates before the scan from a
   given start gives up. `PRUNE_PATIENCE`'s value is empirical, not derived from first
   principles — `multimodel.rs:152-158` records that a *single*-exceedance cut-off produced a
   non-optimal S-curve segmentation caught by exhaustive enumeration, because "the cubic
   residual is not [monotone in the span] — the end tangent changes with `j`, and a span that
   fits badly can be followed by a longer one that fits well."
2. **Point decimation**, `DP_MAX_POINTS = 768` (`multimodel.rs:163`): boundaries longer than
   this are decimated for the program only, every `stride`-th point, with `sigma` rescaled by
   `1/sqrt(stride)` so each kept point carries the weight of the run it stands for. "A ring of
   a 512px image is 1,500 points; eleven of them took eleven seconds." The capped variant used
   by repair is exempt, "since the self-intersection repair relies on the measured contour
   being reproducible at `max_span = 1`, and a decimated contour is not simple by
   construction."
3. **`max_span`** — `usize::MAX` in normal fitting; a finite cap under stage 12's repair.

Per-candidate O(1) pricing comes from prefix sums (`Prefix`, `CirclePrefix`) maintained
incrementally, and a residual-sample cap `MAX_RESIDUAL_SAMPLES = 32` for cubics. The one
candidate that is *not* O(1) is the elliptical-arc fit — a full conic scatter matrix would
need 21 prefix sums and a 6x6 eigenproblem — so it is tried only at every `LENGTH_STRIDE`-th
candidate length. **`LENGTH_STRIDE = 16`, but the doc comment above it says "only every
fourth length" — a mismatch between the comment and the constant** (`multimodel.rs:1670-1676`).

### Why the DP scans every endpoint, and what four cheaper alternatives cost

This is one of the most substantial doc comments in the crate, `multimodel.rs:1812-1848`,
because it records four attempts to make the O(n^2) scan cheaper, three of which were
refuted:

> "The scan is the tracer's largest single cost — 978,236 spans evaluated on a 768-px
> wordmark to keep 448 segments, a ratio of two thousand to one... Four ways of being
> cleverer were tried on the 246-icon gate. Three are refuted, and the fourth says where the
> real constraint is.
>
> - **Cap the reach.** ...at 64 it saves more but forces extra segments, and the parameter
>   ratio goes to 1.390 against a limit of 1.353. The reach is not the waste.
> - **Assume the optimal predecessor is monotone** — the Knuth/quadrangle condition... It
>   does not hold: 11.7% of transitions move the predecessor backwards, by as much as 253
>   points. A window would be wrong, not merely approximate.
> - **Propose breakpoints from a cheap line-only polygon.** Poor recall... at a quarter of
>   lambda the polygon proposes 18% of the points and contains only 65% of the breaks this
>   program chooses.
> - **Coarsen where a break may fall.** ...costs dE00 0.1497 -> 0.2207 at almost unchanged
>   parameter count. Half the resolution, half again the error.
>
> That last one is the finding... The program is searching every endpoint to compensate for a
> local model that is too rigid. So the way to make this cheap is not a better order
> estimator. It is a local model whose tangents do not have to be guessed — which is exactly
> the model `try_free_cubic` already implements and which is switched off because it fits
> correlated contour noise too faithfully."

### The alphabet

| primitive | params | constant | derivation |
|---|---|---|---|
| Line | 2 | `PARAMS_LINE` (`lib.rs:41`) | "its endpoint (x, y). The start point is shared with the previous segment, so it is not charged twice." |
| Axis-constrained line | 1 | `PARAMS_AXIS_LINE` (`merge.rs:646`) | "A line the drawing constrains to an axis costs one number, not two: the artist writes `h16`, not `L 16,0`." |
| G1 / free cubic | 6 | `PARAMS_CUBIC` (`multimodel.rs:102`) | "two control points and an endpoint" |
| Smooth (`S`) cubic | 4 | `PARAMS_SMOOTH_CUBIC` (`merge.rs:831`) | "the reflection is implied" |
| Circular arc | 5 | `PARAMS_ARC` (`curves.rs:158`) | one radius + two flags charged as full parameters + endpoint; see below |
| Elliptical arc | 7 | `PARAMS_ELLIPTICAL_ARC` (`curves.rs:165`) | "one more than a cubic, so an ellipse has to be a materially better description of its span, not merely an equal one" |
| `<circle>` | 3 | `PARAMS_CIRCLE` (`primitives.rs:32`) | `cx cy r` |
| `<ellipse>` | 5 | `PARAMS_ELLIPSE` (`primitives.rs:34`) | `cx cy rx ry` plus rotation |
| `<rect>` rounded | 6 | `PARAMS_ROUND_RECT` (`primitives.rs:37`) | "charged as six even though we force `ry = rx`, because the emitted element carries both" |
| `<rect>` plain | 4 | `PARAMS_RECT` (`primitives.rs:39`) | corner radius collapsed to zero |

`PARAMS_ARC`'s derivation (`curves.rs:149-158`) is worth quoting in full, because it explains
a deliberate over-charge:

> "SVG writes seven numbers... but for a *circular* arc `rx == ry` and the rotation is
> meaningless, so the document carries one radius, two one-bit flags and an endpoint. We
> charge the flags a full parameter each — they are a choice the designer has to make when
> editing — which gives 1 + 2 + 2 = 5. It is deliberately not 3 (radius + endpoint): a
> description length that ignored the flags would let an arc undercut a cubic on every short
> run where the two are indistinguishable, which is not the compactness the objective is
> meant to reward."

`FittedPath::params()` (`lib.rs:663-665`) adds a further `2.0` for the path's own start
point; the DP's internal cost accounting (`multimodel::path_cost`) does not, since the start
point is shared with whatever precedes it.

**Free-tangent cubics are in the alphabet but off by default** (`INKVEC_FREE_CUBIC`), and the
refutation is a genuinely useful negative result, `multimodel.rs:1172-1188`:

> "The model is sound and does what it promised: freeing the tangent directions halves the
> residual per point... It still made the output *worse* against the truth — on 24 images
> rendered at 1024 against their ground-truth SVGs, DISTS went from 0.0926 to 0.0932 and
> runtime rose 25%. ...Chi-squared is measured against extracted contour points, and those
> carry the staircase error of reading a boundary off a pixel grid — error that is strongly
> correlated along the boundary, not independent as the diagonal weighting assumes. A model
> with more freedom spends it tracking that error more faithfully. Fitting the measurements
> better is not the same as being closer to the shape."

**Per-span arcs** exist because a boundary that mixes straight and curved parts previously
had no way to spend an arc mid-ring — arcs were only ever proposed for a *whole* ring.
`ArcSpan`'s doc, `multimodel.rs:1452-1466`:

> "Arcs were already in the alphabet, but only ever proposed for a *whole* ring... Measured
> across thirty corpus icons at 128 px, the artists' own files are 61% curve commands and
> ours were 35%: at small sizes a shallow arc's chord sits inside the anti-aliasing noise, and
> a line costs 2 parameters where a cubic costs 6. An arc costs 5 and, unlike a cubic, is
> *exactly* the shape a circular logo is drawn from, so it wins on residual as well as on
> price."

Fitting is O(1) via `CirclePrefix`, an algebraic (Kåsa) approximation to the geometric chi2
(`multimodel.rs:1329-1340`) — "with the iterative fitter called per span, a 768-px logo took
3.6 s against 0.9 s without arcs; the moments take it back." The final radius used for scoring
is the one the SVG arc *renders* with (endpoint-parametrised), not the fitted least-squares
one — a distinct earlier bug, `multimodel.rs:1571-1576`: "emitting the fitted radius and
scoring the fitted circle is how the alphabet came out smoother and a third worse in colour:
the cost was measured on a curve nobody draws."

`MAX_ARC_DEGREES = 120.0` (`primitives.rs:41-50`) bounds the longest single-arc sweep; its
derivation is the best-supported constant in the crate:

> "SVG arcs are endpoint-parametrized, and near 180 degrees that parametrization is badly
> conditioned: the centre offset from the chord midpoint is `sqrt(r^2 - h^2)` in the
> half-chord `h`, whose derivative diverges as `h -> r`. Shortening the chord of a 40px
> half-circle by 0.05px — one sigma of extraction noise on an endpoint — moves its centre by
> 2px. At 120 degrees the same derivative is 0.58, so the arc shape is as stable as its
> endpoints."

### The bow term

`bow_penalty` (`multimodel.rs:1291-1306`) charges the *line* candidate a penalty when a
circular arc fits the same points far better, on the reasoning that a systematic sign pattern
in the line's residuals is itself evidence against the line model:

> "Under the line model each residual sign is a coin flip, so all of them agreeing is
> `(n-1) ln 2` of information against the model — evidence the chi2 alone discards, and the
> reason a shallow curve at a small size came back faceted however the tolerances were set. It
> is charged only where a circle explains the same points four times better, which is what
> separates a systematic bow from a fluke."

```rust
fn bow_penalty(chi2_line: f64, chi2_arc: f64, span: usize) -> f64 {
    if chi2_arc * 4.0 < chi2_line {
        span as f64 * std::f64::consts::LN_2
    } else {
        0.0
    }
}
```

The `4.0` threshold for "four times better" has no stated derivation; the `ln 2` per point
does (one bit per independently-agreeing sign). Note also a doc-attachment defect: the
paragraph documenting `line_cost_terms` (residual + parameters + tangent-break cost) sits
directly above this function and rustdoc will attach it here instead
(`multimodel.rs:1291-1296`).

### `merge.rs` — the four post-DP passes, and their default gating

All four run from one place, `optimal_multimodel_capped_full` (`multimodel.rs:333-348`), and
their gating is worth stating precisely rather than assuming, because two of the four are
silently off:

```rust
if max_span == usize::MAX {
    crate::merge::merge_free_cubics(&mut fit.path, &shifted, &fit.vertices, cfg);
    crate::merge::sharpen_corners(&mut fit.path);
    if std::env::var("INKVEC_AXIS").is_ok_and(|v| v != "0") {
        crate::merge::snap_axis_aligned(&mut fit.path, &shifted, &fit.vertices, cfg);
    }
    if std::env::var("INKVEC_G1").is_ok_and(|v| v != "0") {
        crate::merge::snap_smooth_joins(&mut fit.path, &shifted, &fit.vertices, cfg);
    }
}
```

| pass | default | gate |
|---|---|---|
| `merge_free_cubics` | **on** | unconditional, but only outside the repair's span cap (`max_span == usize::MAX`) |
| `sharpen_corners` | **on** | same |
| `snap_axis_aligned` | **off** | `INKVEC_AXIS` must be set and not equal to `"0"` — absence means off |
| `snap_smooth_joins` | **off** | `INKVEC_G1` must be set and not equal to `"0"` — absence means off |

`merge_free_cubics` and `sharpen_corners` also run a second time, unconditionally, inside
stage 12's ring-level repair (`crates/inkvec-cli/src/rings.rs:193-194`), under a segment
budget — see `12-repair.md`. `snap_axis_aligned` and `snap_smooth_joins` have **no other
caller anywhere in the workspace** — with both env variables unset (the shipping default),
they are dead code, and neither has a test exercising it.

**`merge_free_cubics`** (`merge.rs:344`) replaces short runs of chord/cubic/chord with one
free-tangent cubic, because the DP's own cubic is G1 — its tangent *directions* are inherited
from the vertex estimator, only the arm lengths are fitted — and at a corner the inherited
tangent is wrong. The module doc gives measured numbers on a traced rounded square:

```
                             chi2     cost
  G1, tangents inherited    284-303   174-183
  free tangents              95-101    80-83
  the split it chose        111-115   109-111
```

"a free cubic beats the split by about 25% and beats the constrained cubic by more than
half." It searches run lengths longest-first (a corner is usually three segments), refuses
runs containing an `Arc`, and fits with a coarse grid (9 angles x 5 arm fractions) followed by
pattern search. Accepted only if `new_cost < old_cost + smooth_slack() * cfg.lambda`.

**`sharpen_corners`** (`merge.rs:498`) — takes only the path, no polyline or config — replaces
a short cubic bridging two lines with the lines' actual intersection. Its doc explains why the
DP produces these chamfer cubics in the first place:

> "The contour samples around a corner lie on the coverage level set, which rounds the corner
> off by about a pixel. To the residual those samples *are* a small fillet, so a cubic through
> them beats two lines that miss them — the residual cannot tell a rasterised sharp corner
> from a sub-pixel fillet. The prior settles it: icon and logo artists draw corners; a fillet
> under two pixels at this scale is not something they draw, it is something the renderer
> did."

Guarded geometrically, not by residual: the two lines must turn by at least
`SHARPEN_MIN_TURN = pi/6` (30 degrees — the same value as `lib.rs`'s `CORNER_TURN_MIN`,
independently defined), their intersection must lie ahead of the first line and behind the
second, and it must sit within a chamfer allowance of the cubic's endpoints. Two shapes are
recognised: a cubic that *is* the chamfer (chord <= `SHARPEN_MAX_CHORD = 2.5`), or a short
straight edge with a chamfer cubic at each end (chord <= `SHARPEN_MAX_EDGE = 8.0`).

**`snap_axis_aligned`** (`merge.rs:681`, off by default) puts a line the measurement cannot
distinguish from horizontal or vertical exactly onto the axis, at a cost of
`PARAMS_AXIS_LINE = 1.0` instead of `PARAMS_LINE = 2.0`. Corpus motivation, `merge.rs:653-660`:

> "Artists constrain lines to the axes and the corpus says so plainly: 68% of lucide's
> straight segments and 46% of simple-icons' are *exactly* horizontal or vertical, and
> widening the tolerance does not find more (68.3% -> 68.4% out to a quarter pixel). That is
> the signature of a constraint rather than a coincidence. Our own output is smeared instead:
> 12% exactly axis-aligned, but a third of all lines within 0.05 px of it."

The accept test is `0.5 * (chi2_axis - chi2_free) < lambda * (PARAMS_LINE - PARAMS_AXIS_LINE)`
— but it is guarded by *three* separate mechanisms, each added after a specific regression:

1. **Line-line joins only.** A cubic's control points are fixed in absolute coordinates, so
   moving a shared vertex without them "silently distorts whatever comes next." Caught on
   `simple-icons/atlassian`: a 0.21 px move "clipped a whole pixel row from full coverage to
   partial at the shape's edge, for +0.023 dE00."
2. **A displacement bound (`room`)** no larger than the moved point's own measured sigma.
   Without it, "icons gained (lucide dE00 0.090 -> 0.085) while emoji lost (noto 0.389 ->
   0.401), which is a neighbouring cubic being dragged off its own evidence."
3. **`MAX_AXIS_DEV_SIGMA = 3.0`** — no single sample may sit more than this many sigma from
   the axis candidate, independent of the aggregate chi2 budget. Caught on `lucide/bath`: "two
   points 91 units apart differed by 0.02 px, well inside the chi2 budget summed over ~90
   samples, and flattening it turned a 90 px stretch of a correctly grey row solid black."

Aggregate measured effect when this pass is on: "snapping a third of all lines moved mean
dE00 by -0.001" — essentially neutral, consistent with it shipping off.

**`snap_smooth_joins`** (`merge.rs:854`, off by default) offers the SVG `S` (smooth cubic)
shorthand as a candidate, at `PARAMS_SMOOTH_CUBIC = 4.0` against `PARAMS_CUBIC = 6.0`. Corpus
motivation: "of the joins between consecutive cubics that are smooth to within a thousandth
of a degree, 60% have equal handle lengths either side — the ratio's median is exactly 1.00
and its whole interquartile range is 1.00." Two mechanisms worth flagging precisely:

- **The 20-degree pre-filter is an inline literal with no name**, `merge.rs:902`:
  `if ang.abs() > 20.0_f64.to_radians() { continue; }`. It compares the angle between the
  incoming handle direction (`pp3 - pc2`) and the outgoing handle direction (`c1 - q1`); the
  comment calls it "a filter, not the decision" needed to avoid refitting every adjacent
  cubic pair including every genuine corner. The value has no stated derivation and is not
  the same constant as `G1_BREAK_DEGREES = 10.0`.
- **The joint refit is a coordinate-descent / pattern search, not the "Damped Gauss-Newton"
  its own comment claims** (`merge.rs:915-919` says "Damped Gauss-Newton on those four,
  numerically differenced"; the code that follows is a four-coordinate compass search with
  step halving on stall, 12 outer rounds, no Jacobian formed). The four free coordinates are
  the previous segment's second control point and the current segment's second control point;
  the first control point of the current segment is *not* free — it is always
  `reflect(pc2) = 2*q1 - pc2`, which is what enforces the `S` constraint during the search.

Both `snap_axis_aligned` and `snap_smooth_joins` use `PARAMS_LINE`/`PARAMS_CUBIC` — the
constants, not `params_cubic()`, the accessor other code in `merge.rs` uses that can be
overridden by an env var — so an override of the cubic parameter price would not reach
`snap_smooth_joins`'s budget calculation. This is an inconsistency, not a documented design
choice.

### Corner handling: `adjust_vertices_at`, `spans_loop`, the 3-sigma cap

Marching squares cannot represent a sharp corner — the level set that produced the measured
polyline cuts across the corner pixel, chamfering it over a point or two.
`adjust_vertices_at` (`lib.rs:479-484`) moves a chosen vertex to the intersection of its two
adjacent fitted lines instead of trusting the measured (chamfered) point:

> "Marching squares cannot represent a sharp corner: the level set cuts across the corner
> pixel, chamfering it over a point or two. Trusting those measured points as vertices both
> rounds the corner and costs an extra segment to cross the chamfer — measured effect, a
> hexagon coming back with 12 vertices instead of 6."

Restricted to actual corners (`is_corner` selects which vertices move) — not an optimisation,
a correctness requirement, because intersecting two nearly-parallel fitted lines is
ill-conditioned:

> "On a smooth curve they are nearly parallel — 9.7 degrees apart on a 37-segment circle —
> and the intersection runs off far from the curve... Displaced vertices made the polygon's
> turn angles erratic, corner detection then fired every few vertices, a circle was cut into
> sixteen short runs, and no run was long enough for a cubic to be worth its parameters. Curve
> fitting looked broken; the actual fault was here."

**The 3-sigma cap**, `lib.rs:561-576` — the exact bound on how far the intersection may move
the vertex:

> "How far the measured vertex may legitimately sit from the true corner. The anti-aliased
> boundary of a corner with interior angle theta is a level set of the coverage field, which
> rounds the corner off: it passes about half a pixel inside a right angle along the bisector
> and further for sharper ones, and the nearest *sample* of it can be another pixel away.
> Measured on a 6 px bar at 20 degrees: samples 1.0 px from each true corner, chords 0.12 px
> too far in. A cap of 3 sigma (0.86 px there) refused every one of those intersections."

The bound is `max_shift = 3.0 * max(sigma) (floored at 0.25)`, plus, only where the turn is at
least `CORNER_TURN_MIN` (30 degrees), a chamfer allowance
`min(CORNER_CHAMFER / sin(half_interior).max(0.2), 3.0)`. Both the `.max(0.2)` divisor floor
and the `.min(3.0)` cap have no stated derivation.

**The loop-cut index bug**: `spans_loop` (`lib.rs:444-453`) exists because a closed ring
solved by the DP is *opened at a cut* before solving, and comparing `v.first() == v.last()`
alone silently exempted the cut vertex — usually the sharpest corner in the shape — from
corner adjustment and smooth-join treatment:

> "That second spelling is every closed contour in the pipeline, and the cut is placed at the
> sharpest corner; comparing indices alone therefore exempted exactly the most corner-like
> vertex of every shape from corner adjustment and from smooth joins. Measured on a rotated
> bar: the cut corner stayed 0.77 px inside the true corner while its neighbours were
> recovered to within 0.1 px."

The fix compares the actual polyline indices and positions rather than trusting the vertex
list's own endpoints.

### `solve_open` and `refine`

`solve_open` (`multimodel.rs:1849`) is the DP body described above. `refine`
(`multimodel.rs:2327`) is the post-DP continuous-parameter pass, run once the discrete
decisions are fixed:

> "1. line–line corners move to the intersection of their fitted lines (`adjust_vertices_at`);
> 2. each cubic's arms are polished against the full residual (`polish_arms`);
> 3. at joins the program left smooth (break below `G1_BREAK_DEGREES`) the shared tangent is
> re-estimated symmetrically, or set to the adjacent line's direction, so the emitted path is
> exactly G1 there; accepted only when it does not cost more residual than the break it
> removes."

`is_corner` fires only where *both* adjacent segments are `SegKind::Line`. Arcs are exempt
from smooth-join adjustment entirely: "An arc's direction is its own: it is the circle the
points fit, and turning its end to meet a neighbour would move geometry the residual already
settled."

## Constants and thresholds

Values that carry a stated numeric derivation are marked **derived**; values whose existence
is explained but whose specific number is not are marked **motivated**; values with neither
are marked **none**.

| name | file:line | value | controls | derivation |
|---|---|---|---|---|
| `PARAMS_LINE` | `lib.rs:41` | 2.0 | line parameter cost | derived |
| `PRUNE_SLACK` | `lib.rs:45`, also `multimodel.rs:150` | 4.0 | safety factor on scan cut-off | motivated; value none |
| `CORNER_CHAMFER` | `lib.rs:457` | 1.0 px | corner-adjustment chamfer allowance | derived (one pixel = level-set sampling step) |
| `CORNER_TURN_MIN` | `lib.rs:460` | pi/6 (30 deg) | when a vertex meeting is treated as a corner | none |
| `CORNER_DEGREES` | `lib.rs:722` | 45.0 | corner-vs-smooth-join threshold | none |
| max-shift factor | `lib.rs:800` (inline) | 3.0 x max(sigma), floor 0.25 | corner intersection displacement cap | motivated; 3.0 and 0.25 none |
| `PARAMS_CUBIC` | `multimodel.rs:102` | 6.0 | cubic parameter cost | derived |
| `G1_BREAK_DEGREES` | `multimodel.rs:137` | 10.0 | tangent break below which a join is nearly free | none; swept empirically |
| `MAX_ARM` | `multimodel.rs:142`, also `merge.rs:117` | 1.0 | largest admissible control arm as a fraction of chord | derived |
| `MAX_RESIDUAL_SAMPLES` | `multimodel.rs:147` | 32 | cubic residual evaluation points (O(1) cap) | none |
| `PRUNE_PATIENCE` | `multimodel.rs:158` | 8 | consecutive over-budget candidates before scan stops | empirical; value none |
| `DP_MAX_POINTS` | `multimodel.rs:163` | 768 | decimation threshold | none |
| `TANGENT_WINDOW_MAX` | `multimodel.rs:166` | 16 | widest one-sided tangent window | none |
| `NEWTON_STEPS` | `multimodel.rs:170` | 3 | Newton steps for arc-length projection | none |
| `FREE_MAX_SWING` | `multimodel.rs:1103` | 75.0 deg | how far a free cubic's tangent may depart from the estimate | none |
| `DIRECTION_SAMPLES` | `multimodel.rs:1491` | 8 | monotone-sweep samples for arc validity | asserted, not derived |
| `MAX_ASPECT` | `multimodel.rs:1666` | 12.0 | most elongated ellipse worth fitting | none |
| `MIN_POINTS` (ellipse) | `multimodel.rs:1669` | 24 | minimum points to try an ellipse | none |
| `LENGTH_STRIDE` | `multimodel.rs:1676` | 16 | ellipse candidate-length sampling | doc says "every fourth length" — mismatch |
| bow-penalty factor | `multimodel.rs:1301` (inline) | 4.0 | arc-vs-line residual ratio that triggers the bow penalty | none |
| `PARAMS_ARC` | `curves.rs:158` | 5.0 | circular arc cost | derived |
| `PARAMS_ELLIPTICAL_ARC` | `curves.rs:165` | 7.0 | elliptical arc cost | derived |
| `MAX_ARC_DEGREES` | `primitives.rs:50` | 120.0 | longest single-arc sweep | derived (conditioning argument) |
| `MAX_REDUCED_CHI2` | `primitives.rs:1081` | 4.0 | primitive acceptance gate | derived (`tau^2` at default `tau=2`) |
| `PARAMS_CIRCLE` | `primitives.rs:32` | 3.0 | derived |
| `PARAMS_ELLIPSE` | `primitives.rs:34` | 5.0 | derived |
| `PARAMS_ROUND_RECT` | `primitives.rs:37` | 6.0 | derived |
| `PARAMS_RECT` | `primitives.rs:39` | 4.0 | derived |
| `BREAK_PARAMS` | `merge.rs:53` | 2.0 | joins a free cubic no longer meets smoothly | derived; overridable `INKVEC_MERGE_BREAK` |
| `MAX_SPAN` | `merge.rs:57` | 96 | longest merge run attempted | none |
| `MAX_RUN` | `merge.rs:65` | 4 | segments a merge run may absorb | none (once overridable, no longer swept) |
| `MAX_ROUNDS` | `merge.rs:75` | 6 | merge sweep passes | motivated; value none |
| `SEARCH_DEGREES` | `merge.rs:111` | 100.0 | free-cubic angle search width | motivated (a 60-degree clamp put the optimum outside the search); value none |
| `SHARPEN_MAX_CHORD` | `merge.rs:472` | 2.5 | chamfer-cubic chord ceiling | semi-derived (chamfer ~1px/side) |
| `SHARPEN_MAX_EDGE` | `merge.rs:477` | 8.0 | short-edge-with-chamfers ceiling | none |
| `SHARPEN_MIN_TURN` | `merge.rs:479` | pi/6 (30 deg) | corner-vs-smooth threshold | none; duplicates `CORNER_TURN_MIN` |
| `PARAMS_AXIS_LINE` | `merge.rs:646` | 1.0 | axis-snapped line cost | derived |
| `MAX_AXIS_DEV_SIGMA` | `merge.rs:651` | 3.0 | per-sample axis-snap deviation cap | none |
| `PARAMS_SMOOTH_CUBIC` | `merge.rs:831` | 4.0 | `S`-shorthand cost | derived |
| G1 pre-filter angle | `merge.rs:902` (inline, unnamed) | 20 deg | when a smooth-join candidate is worth the exact refit | none |
| `FLATTEN` | `simple.rs:51` | 16 | flattening resolution for the self-crossing test | motivated (below render-visibility floor); value none |
| `MAX_REPAIRS` | `simple.rs:55` | 8 | span-cap halvings before falling back | derived (`2^8` covers any contour produced) |
| `EPS` (endpoint coincidence) | `simple.rs:59` | 1e-6 px | adjacency exemption tolerance | derived |

## Failure modes and edge cases

- **The hexagon regression**: an angular-cone pruning bound that was not tied to the cost
  function silently doubled a hexagon's segment count (12 instead of 6) before being replaced
  by the cost-derived cut-off — see `lib.rs:322-329`, quoted above.
- **Free-tangent cubics, closed-form arm least-squares, and moment-method free cubics were
  all tried and refuted** on measured DISTS/runtime grounds — see the quotes above and
  `multimodel.rs:1210-1221` ("the reason is the same one that sinks free tangents... a closer
  fit to contour points that carry correlated extraction error is not a closer fit to the
  shape") and `multimodel.rs:1055-1059` (the moment-method's sixth-power differencing "loses
  every significant digit" on short spans).
- **Scoring the fitted circle instead of the drawn one**: `multimodel.rs:1571-1576`, "the
  cost was measured on a curve nobody draws" — the alphabet came out "a third worse in
  colour" until the score was moved to the endpoint-parametrised (drawn) radius.
- **`break_cost` numerical identity**: `x.powf(2.0)` and `x*x` were checked bit-for-bit equal
  over an exhaustive practical sweep so the hot per-cell path could use the cheaper multiply
  (`multimodel.rs:565-573`).
- **`snap_axis_aligned` regressions on `simple-icons/atlassian`** (a cubic's fixed control
  points silently distorted by a moved shared vertex) and **`lucide/bath`** (a trend hidden
  under the aggregate chi2 budget flattened a 90 px stretch of partial coverage to solid) —
  both are why the pass carries three independent guards rather than one chi2 test.
- **A degenerate closed loop is only approximately optimal.** The DP's exactness argument
  (§ "why the DP is a global optimum") does not extend to `solve_closed`'s cut heuristic.

## Environment overrides

| variable | effect | default when unset |
|---|---|---|
| `INKVEC_AXIS` | enables `snap_axis_aligned` when set and not `"0"` | off |
| `INKVEC_G1` | enables `snap_smooth_joins` when set and not `"0"` | off |
| `INKVEC_FREE_CUBIC` | enables the free-tangent cubic candidate in the DP | off |
| `INKVEC_NO_ARCS` | referenced in a doc comment near `arcs_enabled()` as disabling per-span arcs when set to `1` | arcs on |
| `INKVEC_MERGE_BREAK` | overrides `BREAK_PARAMS` (`merge.rs:94`) | `2.0` |
| `INKVEC_G1DBG` | prints per-join accept/reject diagnostics for `snap_smooth_joins` | off |

`--precision` and `--tau` (`crates/inkvec-cli/src/args.rs`) set `FitConfig` via
`from_precision`; see the objective section above.

## Open questions

- **The `d = sigma*sqrt(2*lambda*k/n)` tolerance formula is not written anywhere in the
  source.** It is derived here from the objective the code actually implements
  (`0.5*delta_chi2 < lambda*delta_params`), and should be understood as this document's own
  derivation, not a quoted fact.
- **The module doc's description of admissibility ("straightness by incremental cone
  intersection") is stale.** `DirectionCone`/`is_admissible` is a reference implementation
  used only by tests and `examples/lambda_sweep.rs`; the shipping DP tests admissibility
  through `tau^2` chi2 gates in the tangent estimator and primitive fitters instead. Whether
  this divergence is intentional (the cone was a correctness liability, per `lib.rs:322-329`)
  or simply an un-updated doc was not established.
- **Three doc-comment misattachments**, each caused by a missing blank line before the next
  item, leave the wrong function documented in rustdoc: `multimodel.rs:1172-1192` (three
  separate doc blocks land on `ellipses_enabled`, leaving `arcs_enabled` and
  `free_cubic_enabled` themselves undocumented in generated docs); `multimodel.rs:1291-1299`
  (the `line_cost_terms` doc lands on `bow_penalty`); `lib.rs:432-443` (a `spans_loop` doc
  lands appended to `adjust_vertices`'s).
- **`LENGTH_STRIDE`'s doc says "every fourth length"; the constant is 16, not 4.**
- **`snap_smooth_joins`'s doc calls its refit "Damped Gauss-Newton"; the code is a
  four-coordinate compass/pattern search** with step halving, no Jacobian ever formed.
- **`snap_smooth_joins`'s budget reads the `PARAMS_CUBIC` constant, not the `params_cubic()`
  accessor** other code in `merge.rs` uses, so an env override of the cubic parameter price
  would silently not reach this pass's own accept test.
- **`snap_axis_aligned` and `snap_smooth_joins` are both untested and, with default
  environment, unreachable.** Both were built and measured against the real corpus (68%/46%
  axis-aligned lines; 60% equal-handle-length smooth joins) but ship off, with no other
  caller in the workspace and no test exercising either.
- **Duplicate, independently-defined constants**: `CORNER_TURN_MIN` (`lib.rs:460`) and
  `SHARPEN_MIN_TURN` (`merge.rs:479`) are both `pi/6`; `MAX_ARM` is defined identically in
  `multimodel.rs:142` and `merge.rs:117`; `PRUNE_SLACK` is defined identically in `lib.rs:45`
  and `multimodel.rs:150`. None of the three pairs is shared via a common constant.
- **Several constants central to the search bound and alphabet gates
  (`MAX_RESIDUAL_SAMPLES`, `DP_MAX_POINTS`, `TANGENT_WINDOW_MAX`, `MAX_AXIS_DEV_SIGMA`,
  `MAX_SPAN`, `MAX_RUN`, `SEARCH_DEGREES`, `SHARPEN_MAX_EDGE`, the bow-penalty `4.0`, the
  20-degree G1 pre-filter) have no stated numeric derivation** — the need for the guard is
  explained, but not why this particular number rather than a nearby one.
