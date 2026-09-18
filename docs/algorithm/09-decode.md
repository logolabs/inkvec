# Stage 09 — Decode

> Re-derives a thin or leaking face's shape and colour together, from a fixed model order,
> instead of trusting a boundary that was fitted against a colour the image never actually
> showed.

**Source:** `crates/inkvec-trace/src/decode.rs`
**Entry point:** `decode_faces()` (`decode.rs:687`)
**Pipeline position:** after `boundary_opt` (stage mark `"boundary_opt"`, `lib.rs:473`),
before `symmetry` enforcement (stage mark `"decode"`, `lib.rs:489`). Called only from
`trace_color_full_with_alpha`, and only when `INKVEC_DECODE` is set to something other than
`"0"` (`lib.rs:478`) — **off by default**.

## What problem this solves

Every other geometry stage in the tracer refines a boundary that already has as many
degrees of freedom as it has points, and decides how many curve segments to spend only at
the very end, in the fitter. The module doc comment (`decode.rs:1-18`) calls that order
"backwards for a face too thin to own a fully covered pixel": such a face has no pixel
that reads its colour directly, so the palette's estimate of its colour is biased toward
the ground by one minus its best coverage, the boundary is then fitted against that wrong
colour, and nothing downstream revisits either.

Measured on a 2 px diagonal stroke (`bench/research/decoding/`): master emits fifteen
cubic arcs, a core colour of `#41557d` where the truth is `#204080`, and keeps 91% of the
ink mass — "violating the first-order condition that a least-squares fill must leave the
coverage-weighted residual orthogonal to its own coverage column." Fixing the model order
first — one quadrilateral, two flat fills — eliminating the fills by least squares and
running damped Gauss–Newton on the eight vertex coordinates reaches the truth to 0.02 px
from starts at a quarter or twice the true width.

This is the **decoding** framing named in the brief for this documentation set. The
Fourier transform of a polygon's indicator function is a sum of exponentials whose
frequencies are the vertices; treated that way, the vertices behave like point sources
seen through a low-pass filter (the pixel box, plus whatever else blurred the image before
it was rasterised), which is the setting of the super-resolution and shape-from-moments
literature (Candès & Fernandez-Granda 2014; Milanfar, Verghese, Karl & Willsky 1995).
Concretely, `bench/research/decoding/REPORT.md` derives (H1) that for a straight stroke of
width `w` and contrast `theta`, the soft (least-identifiable) mode of the Gauss–Newton
Hessian lives entirely above frequency `1/w`, so what the point-spread function does at
high frequency determines what is recoverable at all — a genuine super-resolution
statement, not a metaphor. This stage is the corresponding decoder: fix the order first,
eliminate the fills, run a small, well-conditioned nonlinear least squares in the
vertices, and choose between candidate orders last, by an exact change in the objective.

That is the opposite order to the rest of the pipeline, which builds a free-form boundary
first (`planar::build`, `refine_subpixel`, `boundary_opt`), guesses fills from a palette
second, and decides arc count last, by dynamic programming, in `inkvec-fit`.

## Inputs and outputs

**Input:** the `PlanarMap` (mutated in place), the rendered image `rgb: &[[f32; 3]]`, the
integer `labels: &[u16]` map, `face_fill: &mut [FillFit]` (`gradient.rs:282-290`), and
`lambda: f64` — supplied by the caller as `gradient::bic_lambda(width * height)`
(`lib.rs:484`), the Bayesian-information-criterion choice `0.5 * ln(n)`
(`gradient.rs:292-299`). This is a *different* lambda from the one `candidate_orders` and
`fitted_params` use internally (`FitConfig::from_precision`, described below) — the two
serve different comparisons and are not interchangeable.

**Output:** `Option<Report>`:

```rust
pub struct Report {
    pub considered: usize,   // faces that passed the area/bbox prefilter
    pub attempted: usize,    // faces that passed the thin-or-leaking test
    pub decoded: usize,      // faces actually rewritten
    pub rejected: usize,     // faces attempted but no candidate order beat J
    pub pooled: usize,       // ribbons pooled under share_widths (0 unless INKVEC_DECODE_SHARE)
    pub shared_width: f64,
    pub d_objective: f64,    // sum of (J_after - J_before) over every accepted decode
    pub ms: f64,
}
```

`None` only when no face was ever attempted (`rep.attempted == 0`).

## How it works

### 1. Which faces reach the decoder

`decode_faces` first computes each face's pixel area from the label map, keeps faces with
`0 < area <= MAX_BBOX_PIXELS` (`20_000`), and orders them worst-first by area (smallest
first — "the budget should buy the faces that are most wrong," `decode.rs:721`).

For each candidate, `ring_of` (`decode.rs:326-360`) assembles the face's boundary as a
single ring of points, bailing out if the face's boundary is not exactly one ring, or has
fewer than four or more than `MAX_RING_POINTS = 512` points ("guards against decoding an
entire background," `decode.rs:101-102`).

Two tests decide whether the face is actually a candidate for this stage
(`decode.rs:771-802`):

- **Thin.** `thin = 2.0 * area / perimeter`, the width of a long thin face measured as
  twice its area over its perimeter. Compared against `THIN_PX` (`INKVEC_DECODE_THIN`,
  default `2.5`). The doc comment on `THIN_PX` (`decode.rs:90-95`) states the derivation:
  "`4.0` lets in faces wide enough to own a clean witness pixel and cost more than it
  gains; `2.5` is where the conditioning cliff sits (`h1_exponent.py`) and where the
  corpus probe turns from losing to winning" — i.e. `2.5` is the measured point on H1's
  power-law conditioning curve past which decoding stops paying for itself.
- **Leak**, computed but *not* gated on. `leak = (sse_fixed - sse0) / sse_fixed`: does
  refitting the fills alone, geometry untouched, lower the residual? At a true
  least-squares optimum it cannot — a face that leaks is provably not at one. Leak was
  tried as a second trigger and withdrawn: "it fires on ordinary faces whose fills are
  merely a little off, and decoding one of those as a polygon damaged three case-suite
  shapes that a width test leaves alone (`corner_right`, `edge_axis`, `junction_quad`)"
  (`decode.rs:791-796`). It is retained only as a diagnostic printed under
  `INKVEC_DECODEDBG`.

`bench/research/decoding/REPORT.md`'s leak census (H5) puts this in context: over 112
faces on 30 screen-set icons, only 6.2% leak at all, concentrated (weakly, correlation
+0.157) in the 2–4 px width band. "The disease is real but rare on this corpus," which the
report gives as the main reason the stage came out corpus-neutral (H6, below).

### 2. Model order: `candidate_orders`

`candidate_orders` (`decode.rs:436-511`) proposes several vertex counts for the ring
rather than committing to one, because a single line-segmentation DP run on a long thin
ribbon "happily describes the whole ring as two long lines — geometrically true, and
useless, because the polygon those two lines bound has no area" (`decode.rs:428-430`).
Two independent proposers feed the same candidate list:

- **Price-ladder.** `optimal_polygon` (the fitter's DP) run at five different price
  multipliers (`4.0, 1.0, 0.25, 0.0625, 0.015`) on `FitConfig::from_precision`'s `lambda`,
  producing a ladder of vertex counts from coarse to fine.
- **Turning-based.** `turning_corners` (`decode.rs:383-423`): a corner is where the ring
  changes direction by more than `0.4` radians, measured over a window of `k` points
  (tried at `k = 2` then `k = 1` if fewer than four peaks are found) with non-maximum
  suppression so one corner yields one vertex, then combined with the ring's own junction
  points up to `m` vertices for `m` in `{3, 4, 5, 6, 8, 10}`. The doc comment
  (`decode.rs:375-382`) explains why this second proposer exists at all: "Asked for four
  vertices on a shape whose long sides are 2 px apart, [the line-fitting DP] puts them
  where four straight pieces fit best; one resulting 'segment' then cuts across the ribbon
  and sits 1.3 px off the boundary for 167 consecutive points. Turning has no such failure
  mode."

Every candidate order is required to include every junction point of the ring (a
correctness requirement, not a heuristic — see write-back below) and to have between
`MIN_VERTS = 3` and `MAX_VERTS = 16` vertices, deduplicated against orders already
proposed.

`candidate_orders` also returns `params_before`: what the shipped fitter would actually
spend describing the *current* (undecoded) ring, per edge, using each edge's own measured
`sigma` — "not a made-up constant. The fitter prices a segment against how well the points
are known, so inventing sigma = 0.5 made the shipped fit look four times cheaper than it
is and the comparison below meaningless — it rejected a decode that cut eighteen
parameters to ten" (`decode.rs:445-448`).

### 3. Solving one candidate: variable projection plus damped Gauss–Newton

For each candidate order, `is_polygonal` (`decode.rs:945-1006`) first checks whether the
ring is a polygon the pipeline drew badly, or a genuine curve. The distinguishing test is
the *sign* of how the ring strays from the chord of each proposed segment, not its
magnitude: "A curve leaves its chord on one side all the way along, so the signed offsets
have a large mean. A sawtooth crosses back and forth, so they have a mean near zero and a
large spread. Test the mean against the spread, not the spread against a constant"
(`decode.rs:941-944`). Concretely, a segment with at least `CURVE_MIN_SAMPLES = 8` ring
points along it is rejected as a genuine curve only if `mean.abs() > CURVE_BIAS_PX (0.35)`
**and** `mean.abs() > 0.5 * rms`; segments with fewer samples (an end cap's staircase) are
not tested, since "calling that a curve rejected every proposal on the shape this stage
exists for" (`decode.rs:981-983`). A worst-case deviation past `MAX_DEV = 2.0` px is
rejected regardless of sign pattern.

Free vertices — those not at a junction — are then solved by `gauss_newton`
(`decode.rs:1154-1290`), a Levenberg-damped Gauss–Newton iteration where the fills are
eliminated by least squares (variable projection) at every trial step:

- `Problem::eval` (`decode.rs:658-680`) computes exact fractional pixel coverage of the
  candidate polygon over its bounding band (`coverage`, `decode.rs:266-311`, using exact
  Sutherland–Hodgman clipping — `clip_axis`/`clip_area`, `decode.rs:164-209` — on pixels
  the boundary actually crosses, and a point-in-polygon test for the rest), assembles one
  coverage column per fill (the face itself, plus each edge-neighbour it borders), and
  solves the small linear least-squares system `varpro` (`decode.rs:518-572`) for all
  fills at once, ridge-regularised toward each fill's prior colour so "a column with no
  support keeps the colour it had, and the solve cannot answer a question the pixels did
  not ask" (`decode.rs:541-542`).
- With the fills held at their least-squares values, the residual has a closed-form
  derivative with respect to vertex position: `d(residual)/d(u) = (d(coverage)/d(u)) *
  (c_face - c_other)`. Only the coverage derivative needs finite differences
  (`FD_STEP = 0.01`, central difference); no Jacobian of the fills is ever formed
  (`decode.rs:1146-1153`).
- The normal equations are assembled directly from those coverage derivatives, damped by
  a Levenberg parameter `mu` (starting `1e-3`, multiplied by 4 on a rejected step down to
  `1e-7` divided by 3 on an accepted one, capped at `1e9`), run for `GN_ITERS = 14`
  iterations, each step clamped to `MAX_STEP = 0.35` and leashed to `MAX_TOTAL = 1.0` px
  cumulative from the vertex's starting position — "`boundary_opt` caps its own points the
  same way and for the same reason: topology was decided on the integer lattice, and a
  point that walks far from its lattice evidence is no longer describing the feature it
  was extracted from. Without this cap the solver drifted vertices up to five pixels,
  turning the rounded frame of one emoji into a thirteen-sided polygon whose local
  residual had improved and whose rendered colour error had quadrupled" (`decode.rs:119-125`).
- **Only pixels the boundary actually cuts carry information about where it is** — a
  consequence of the Hadamard structure theorem the same doc comment cites, and the reason
  `PIXELS_PER_UNKNOWN = 4` band pixels are required per free coordinate before a candidate
  order is even attempted: "the reason `boundary_opt` sums over boundary pixels alone
  ... Below this ratio the problem is under-determined and the solver is fitting noise —
  which is what it was doing on faces of one to five square pixels, turning specks into
  triangles for a thousandth of an objective" (`decode.rs:105-111`).

`Problem` also tracks `rest`/`rest_rgb` — every *other* face that reaches into the band,
held fixed at its current geometry and colour. Without them "the fit is biased. A stroke
the palette shattered into three faces has its two ends painted by faces this one shares
no edge with; a model that pretends they are absent explains their ink by shrinking the
middle face, which is exactly what happened: a 2 px stroke came back 1.71 px wide"
(`decode.rs:629-633`).

### 4. Acceptance: a Pareto improvement, judged by exact ΔJ

A solved candidate is accepted only if all of the following hold:

1. `simple(&best)` — the solved polygon does not self-intersect.
2. Either the solve **earned** the right to skip the post-solve shape recheck — defined
   as `sse1 < EVIDENCE_OVERRIDE * sse0` (default `EVIDENCE_OVERRIDE = 0.0`, so this branch
   never fires by default) — or it passes `is_polygonal` against the *original* ring a
   second time. The comment explains why the recheck must not be absolute
   (`decode.rs:840-846`): "The solver moves vertices away from the extracted ring on
   purpose — that ring is the thing being corrected — so measuring the answer against it
   rejects every successful decode ... on a sawtoothed ribbon the corrected boundary is
   *supposed* to sit a pixel off the ring it came from. Strong evidence overrides the
   prior."
3. `fitted_params` (`decode.rs:1404-1420`) — what the shipped fitter would actually spend
   describing the decoded, *written-back* polygon (sampled and re-fitted, not counted as
   two parameters per line by hand) — must not exceed `params_before`.
4. The residual must fall to less than `MIN_GAIN * sse0` (default `0.5`). The doc comment
   (`decode.rs:70-76`) gives the measured reason: "At 1.0 (any improvement at all) the
   stage fires on marginal cases and the screen set comes out 10 icons worse against 7
   better; at 0.5 it fires only where it has something to say, and the same set comes out
   better on every axis."

Both 3 and 4 together are the Pareto rule stated in the module doc comment and reiterated
in-line (`decode.rs:860-865`): "A falling J alone lets the stage buy a cheaper description
with a worse picture, and that is what it did: it polygonised curved thin faces, spending
FEWER parameters and quadrupling the colour error ... Require both — a strictly better fit
and no more parameters."

Among all accepted candidate orders for one face, the winner is whichever has the lowest
`j1 = sse1 + lambda * params_after`.

### 5. Write-back: never replace a ring

`write_back` (`decode.rs:1422-1437`) is deliberately delicate, because **edges are shared
between faces** — replacing a ring outright would silently move a boundary that a
neighbouring face also depends on. Two properties make this safe:

- **Junction nodes are frozen.** They are required to be vertices of every candidate order
  (enforced in `candidate_orders`, `decode.rs:475-477`) and keep their exact positions in
  `decoded_edge_points`. A neighbouring face across a shared edge sees the same boundary
  endpoints it always did.
- **Only interior points move, and each interior point belongs to exactly one edge.** The
  map's own structure — one `Edge` per boundary curve, referenced by exactly the two faces
  either side of it (`planar.rs:24-27`) — means an edge's interior points are never shared
  with any other edge. So rewriting one edge's `points` and `sigma` in place changes
  exactly the geometry that edge owns, and the partition of the image into faces survives
  by construction, without any repair step needing to reconcile two copies of a boundary.

The decoded polygon is **not** written back as bare corner points. `decoded_edge_points`
(`decode.rs:1361-1395`) samples each segment roughly one point per pixel
(`SAMPLE_PX = 1.0`), all carrying a tight `DECODED_SIGMA = 0.05` px. The doc comment
explains why this matters (`decode.rs:1334-1355`): storing only the vertices let a
downstream curve fitter, "judged on boundary error and segment count, not on the image,"
run one cubic through five corners that bulged 20 px off the true boundary at no cost to
its own objective; and separately, a thin ribbon's two long sides fit a pair of lines so
well that the fitter "happily drop[ped] the caps" between them, leaving a self-intersecting
path with no area that was then discarded entirely by the emitter's `MIN_RING_AREA` guard.
Dense samples at a small, honest sigma state the truth — "this boundary is known to a
twentieth of a pixel *everywhere*, not only at its corners" — under which straight lines
are the cheapest description and the caps cannot be deleted.

### `share_widths` — pooling thin ribbons under one shared width

`share_widths` (`decode.rs:1503-1845`, called only when `INKVEC_DECODE_SHARE` is set to
something other than `"0"`; **off** by default) is a second pass over the same map, run
after the per-face loop. It targets a specific identifiability gap the per-face decoder
cannot close on its own: `bench/research/decoding/REPORT.md`'s H3b/H3c measured that an
axis-aligned sub-pixel stroke's raster fixes only the *product* of width and contrast, not
either factor alone — two zero-residual fits to the same 1 px bar can have widths that
differ by two thirds of a pixel while `w * theta` agrees to six decimals. Two strokes at
different sub-pixel phases, forced to share one width, break that ambiguity because a
single width then has to explain two different phases at once (H8).

The function:

1. Collects four-sided ribbon faces below `SHARE_MAX_PX = 1.75` px wide (`ribbon_width`,
   `decode.rs:1455-1487`, the longest side's normal direction and the polygon's extent
   along it), none of whose corners is a junction (a junction is shared with a face this
   pass is not solving and cannot be moved unilaterally).
2. Searches one shared width by a coarse sweep (`0.4 * median` to `2.0 * median`, 40 steps)
   then a local refinement, scoring each candidate width by `joint` — a single least
   squares over **every pooled ribbon's pixels at once**, with one shared-ink column and
   one surround column per ribbon (`decode.rs:1701-1738`).
3. Requires the shared solution to beat the baseline — the same polygons, each keeping its
   own width *and* its own ink, scored through the identical least squares — on **both**
   the residual and the full objective `J`, charging one width and one ink for the whole
   pool against `PARAMS_PER_RIBBON = 10.0` (a move plus four line segments) times the pool
   size individually.
4. On acceptance, writes every pooled ribbon back at the shared width (`set_width`,
   `decode.rs:1489-1501`, moving both sides symmetrically about the ribbon's own
   centreline) and gives every ribbon the one shared ink.

Two earlier, wrong implementations are recorded because "both were the theorem being got
wrong rather than the theorem being wrong" (`decode.rs:1543-1544`): charging the pooled
answer against the *undecoded* ring let it buy a worse fit with the decode's own parameter
saving; and sharing the width while each ribbon still fitted its *own* ink bought nothing
at all, because each ribbon simply slid along its own `w * theta` curve to whatever colour
kept the product right — three 1.5 px bars pooled at 2.311 px with the (wrong) residual
test approving. "One pen means one width *and* one ink, and the second half is not
optional."

**With both fixed, the pass is implemented correctly and still refuted in the pipeline.**
On four parallel bars at four sub-pixel offsets — "the friendliest case it will ever
see" — per-face decoding already recovers the true ink `#204080` at 1.5 px; pooling
replaces it with `#39507e`. The explanation, quoted in full because it is the report's own
verdict and not a paraphrase (`decode.rs:1526-1533`, matching `REPORT.md`'s H8 section):

> **The tracer is not solving the problem the theorem is about.** It reaches this stage
> holding an integer label map, and that labelling is itself a prior — it has already
> committed which pixels belong to the stroke, pinning the width to about a pixel before
> any fitting begins. The gauge has already been broken by something else, so pooling
> arrives with nothing left to contribute.

The pass is kept in the tree, off, "for that" — i.e. for the day the labelling stage
itself stops committing this early.

## `share_widths` vs the rest of the stage

`share_widths` is the one part of `decode.rs` documented as mathematically sound and
practically useless *in this pipeline as it stands*, which is a different verdict from the
main per-face decoder's own corpus-neutral result (below). Both are off by default, but
for different reasons: the per-face decoder is neutral because the disease it targets is
rare on the corpus (H5); `share_widths` is neutral because an upstream stage (labelling)
has already answered the question it exists to ask.

## Corpus result (H6)

`bench/research/decoding/REPORT.md` gives the measured verdict on the default (per-face,
`INKVEC_DECODE_SHARE` off) configuration, screen set (246 icons):

| build | acceptance rule | dE00 | DISTS | params ratio | objective |
|---|---|---|---|---|---|
| master (stage off) | — | 0.19252 | 0.02848 | 1.3958 | **0.47736** |
| final (stage on, as shipped) | v4 + a decode must halve the residual | 0.19256 | 0.02849 | 1.3957 | **0.47746** |

"The final build changes 4 icons of 246: two better, two worse, 242 identical. The
objective differs from master by 0.02%, which is not a win... **Verdict on H6: not
confirmed. Do not merge into the default path.** The stage stays behind `INKVEC_DECODE`,
off, which is where it already is." The report also records a synthetic-stroke win that
does not generalise: on the 2 px diagonal stroke case, the stage recovers the exact core
colour (`#204080` against master's `#41557d`) at 541 bytes against master's 614, and three
"bugs worth recording" that were fixed along the way — the fitter deleting a well-fit
shape, corners alone not specifying a shape, and a line-fitting DP being the wrong corner
detector for a ribbon — all reflected in the `is_polygonal`/write-back design already
described above.

`REPORT.md` also documents a second, more aggressive setting, `INKVEC_DECODE_OVERRIDE=0.5`
(the `EVIDENCE_OVERRIDE` env override, letting a decode move a boundary away from its ring
on weaker evidence): it fixes one more case-suite regression (`ribbon_w1.5`) and the
synthetic stroke's true colour, at a cost of 0.6% worse corpus objective across eleven
extra icons. "The default declines that trade... The corpus and the case suite pull in
opposite directions, and neither is wrong: the case suite contains the disease in
concentrated form, the corpus contains it at 6% of faces (H5)."

H9/H10 in the same report explain *why* the stage came out neutral rather than merely
report that it did: splitting each icon's error into face-interior pixels versus
boundary-crossing pixels shows the interiors are essentially exact (dE00 0.0000–0.0011)
for five of seven families, and all remaining error sits at boundaries already accurate to
between 0.021 and 0.060 px (measured by displacing ground truth by a known amount and
inverting the resulting dE00-per-pixel-of-shift). "The tracer's boundaries are already
right to between a fiftieth and a twentieth of a pixel... The decoder was competing for
tens of a percent of a pixel in a domain that is already at 0.02 px, on the 6% of faces
that leak at all, and it is no surprise it came out neutral." The report's own priority
list places "not more boundary work" above the decoder and above every other boundary
idea in the area (trend filter, junction wedges, a better corner model), and instead points
at a face-splitting proposal for merged flat regions (H12: two flat colours recover 66% of
the remaining error where a linear gradient recovers 13%) as the highest-value next step —
explicitly upstream of this stage rather than inside it.

## Tests

Four tests in `#[cfg(test)] mod tests` (`decode.rs:1847-1937`):

- `half_pixel_is_exactly_half` — a rectangle spanning exactly half a pixel's width reads
  coverage `0.5` in that pixel and sums to `0.5` overall, pinning `coverage`'s exact-area
  arithmetic.
- `triangle_area_is_exact` — a 10x10 right triangle at pixel-aligned corners reads a total
  coverage of exactly `50.0`.
- `decodes_a_thin_diagonal_quad_from_a_wrong_start` — the case the stage exists for: a
  synthetic 2 px-wide diagonal stroke on a uniform ground, started from a quad `0.7` px
  wrong in width and `0.6` px wrong in position (both within `MAX_TOTAL`). Asserts the
  residual falls below `1e-4`, every solved vertex stays within the `MAX_TOTAL` leash of
  its start, and the worst per-vertex coordinate error is under `0.05` px.

There is no unit test in this file for `share_widths`, `varpro`'s ridge regularisation, or
`fitted_params`; those are exercised only through the corpus/case-suite measurements in
`bench/research/decoding/REPORT.md`, not through `cargo test`.

## Constants and thresholds

| name | value | controls | stated derivation |
|---|---|---|---|
| `LEAK_GATE` | `0.05` | diagnostic threshold on `leak` (not gated on; see above) | no numeric derivation; leak itself was withdrawn as a trigger |
| `MIN_VERTS` | `3` | fewest vertices a candidate order may propose | no stated derivation |
| `MAX_VERTS` | `16` | most vertices a candidate order may propose | no stated derivation |
| `MAX_DEV` | `2.0` px | worst-case deviation from a proposed chord before the face is judged "something else entirely" | no numeric derivation |
| `CURVE_BIAS_PX` | `0.35` px | mean-offset threshold in `is_polygonal`'s curve test | no numeric derivation |
| `MIN_GAIN` | `0.5` | a decode must cut the residual to this fraction of `sse0` | **measured**: at `1.0` the screen set is "10 icons worse against 7 better"; at `0.5` "the same set comes out better on every axis" |
| `EVIDENCE_OVERRIDE` | `0.0` (off) | residual-ratio threshold below which the post-solve shape recheck is skipped | **measured trade, not a preference**: at `0.5` the synthetic stroke decodes correctly and one case-suite regression is fixed, at a cost of 0.6% worse screen-set objective across 11 icons; `0.0` is the setting REPORT.md calls "the default declines that trade" |
| `CURVE_MIN_SAMPLES` | `8` | fewest ring samples a segment needs before the curve test applies | no numeric derivation |
| `THIN_PX` | `2.5` px | width (twice area over perimeter) above which a face is not attempted | **measured**: "`4.0` lets in faces wide enough to own a clean witness pixel and cost more than it gains; `2.5` is where the conditioning cliff sits (`h1_exponent.py`) and where the corpus probe turns from losing to winning" |
| `PARAMS_PER_RIBBON` | `10.0` | parameter charge for one pooled ribbon in `share_widths` | stated as "a move plus four line segments" — an arithmetic count, not a sweep |
| `SHARE_MAX_PX` | `1.75` px | widest ribbon `share_widths` will pool | stated purpose ("above this a single stroke is already identifiable"), no numeric derivation shown for `1.75` specifically |
| `MAX_RING_POINTS` | `512` | largest ring `ring_of` will accept | stated purpose (guards decoding a whole background), no numeric derivation |
| `PIXELS_PER_UNKNOWN` | `4` | boundary-cut band pixels required per free coordinate | qualitative: "the problem is under-determined and the solver is fitting noise" below this ratio; no swept value shown |
| `MAX_BBOX_PIXELS` | `20_000` | largest face bounding box attempted | no stated derivation |
| `GN_ITERS` | `14` | Gauss–Newton iteration cap | no stated derivation |
| `FD_STEP` | `0.01` px | finite-difference step for the coverage derivative | no stated derivation |
| `MAX_STEP` | `0.35` px | per-iteration clamp on a vertex's Gauss–Newton step | shared value and shared rationale with `boundary_opt::MAX_STEP` (see `08-boundary-solve.md`); not independently derived here |
| `MAX_TOTAL` | `1.0` px | cumulative leash from a vertex's starting position | **measured regression**: "the solver drifted vertices up to five pixels, turning the rounded frame of one emoji into a thirteen-sided polygon whose local residual had improved and whose rendered colour error had quadrupled" without this cap |
| `DECODED_SIGMA` | `0.05` px | sigma given to written-back samples | matches `coverage.rs`'s `DEFAULT_SIGMA_MODEL` (see `02-coverage.md`) but is not derived from it in this file; stated purpose only ("known to a twentieth of a pixel") |
| `SAMPLE_PX` | `1.0` px | spacing of written-back samples along a decoded edge | no numeric derivation beyond "about one point per pixel" |
| Gauss–Newton damping schedule | `mu` starts `1e-3`, `x4` on reject, `/3` on accept, floor `1e-7`, cap `1e9` | Levenberg step damping | standard Levenberg schedule shape; specific factors not derived |
| ridge in `varpro` | `1e-6 * trace / k` | regularises a fill column with no pixel support toward its prior | stated purpose, numeric factor not derived |
| turning-corner angle floor | `0.4` radians | minimum turning angle counted as a corner | no numeric derivation |

## Failure modes and edge cases

- **A face too narrow for any pixel to read its colour cleanly** is exactly what `THIN_PX`
  is measuring; the label map cannot answer this question on its own, because "a 2 px
  stroke has interior labels all along its length and yet its best pixel is only ~85%
  covered, so its colour was never read off the image" (`decode.rs:704-707`).
- **Axis-aligned sub-pixel strokes are provably unidentifiable from the raster alone.**
  H3b/H3c in `REPORT.md`: at `0`, `0.75`, and `1.0` px width, a stroke exactly aligned with
  the pixel lattice has zero phase spread across the pixels it touches, and the raster
  fixes only `w * theta`. Tilting the stroke by even one degree restores identifiability.
  This is not a solver bug and no per-face fix closes it; `share_widths` is the pipeline's
  attempt to close it by pooling phases across strokes, and it is refuted for the reason
  given above (the label map has already broken the gauge).
- **The fitter can delete a shape it fits well**, if handed bare vertices instead of dense
  samples — see the write-back discussion. Fixed by `decoded_edge_points`'s sampling, not
  by a special case in the fitter.
- **A candidate order can drift a vertex arbitrarily far** without `MAX_TOTAL` — the
  emoji-frame regression above.
- **A decode can win locally and lose globally** if judged on residual alone — the
  Pareto/`fitted_params` requirement exists specifically to prevent trading colour
  accuracy for fewer nominal vertices that the real fitter would not actually realise as
  savings.
- **Very large or background-like faces are excluded outright** by `MAX_BBOX_PIXELS` and
  `MAX_RING_POINTS`, independent of whether they are thin — a face can be thin along most
  of its length and still be skipped if its bounding box is enormous.

## Environment overrides

| variable | default | effect |
|---|---|---|
| `INKVEC_DECODE` | off (`"0"` or unset) | master switch — the whole stage is skipped unless set to something other than `"0"` (read in `lib.rs:478`, not in this file) |
| `INKVEC_DECODE_MS` | `600.0` ms | time budget for the per-face loop |
| `INKVEC_DECODE_LEAK` | `LEAK_GATE = 0.05` | overrides the (diagnostic-only) leak threshold |
| `INKVEC_DECODE_THIN` | `THIN_PX = 2.5` | overrides the width threshold that gates whether a face is attempted |
| `INKVEC_DECODE_GAIN` | `MIN_GAIN = 0.5` | overrides the required residual-fraction cut |
| `INKVEC_DECODE_OVERRIDE` | `EVIDENCE_OVERRIDE = 0.0` | overrides the residual-ratio threshold for skipping the post-solve shape recheck; `0.5` is the trade documented in `REPORT.md` |
| `INKVEC_DECODE_RECHECK` | on (`> 0.5`) | when off, the post-solve `is_polygonal` recheck never runs, whatever `earned` says |
| `INKVEC_DECODE_KEEPFILL` | off | when set, an accepted decode keeps the face's existing `FillModel` instead of replacing it with the decoded flat fill |
| `INKVEC_DECODE_SHARE` | off (`"0"` or unset) | enables `share_widths`, the ribbon-pooling second pass |
| `INKVEC_DECODEDBG` | off | verbose per-face and per-order `eprintln!` tracing through the whole decision chain |

## Open questions

- **`MIN_VERTS`, `MAX_VERTS`, `MAX_DEV`, `CURVE_BIAS_PX`, `CURVE_MIN_SAMPLES`,
  `PIXELS_PER_UNKNOWN`, `MAX_BBOX_PIXELS`, `GN_ITERS`, `FD_STEP`, and the turning-angle
  floor `0.4`** all lack a stated numeric derivation, unlike `THIN_PX`, `MIN_GAIN`, and
  `EVIDENCE_OVERRIDE`, which are explicitly measured. Several of these gate whether the
  stage runs at all on a given face, so an untuned value here could be silently excluding
  or including faces for reasons unrelated to the theory the stage is built on.
- **`SHARE_MAX_PX = 1.75`** is qualitatively justified (identifiable strokes above it) but
  the specific cut is not shown against H8's own measured curve, which reports the
  ambiguity closing "outright from two strokes onward" at 1.5 px and shrinking but not
  vanishing at 1.0 px — it is not stated why `1.75` and not, say, `1.5` or `2.0`, was
  chosen as the pooling ceiling.
- **`DECODED_SIGMA = 0.05`** visually matches `coverage.rs::DEFAULT_SIGMA_MODEL` (see
  `02-coverage.md`), which is itself flagged there as citing a test not found in that
  module. Whether the match is intentional (the same irreducible level-set error) or
  coincidental is not stated in `decode.rs`.
- **The stage is a net corpus wash (+0.02%) and is shipped off.** The report is explicit
  that this is not a defect in the mathematics — H9/H10 show the domain it targets (6% of
  faces, already accurate to 0.02–0.06 px) is nearly exhausted for the objective the corpus
  is scored on. Whether a different objective (parameter count, editability, or a
  case-suite-weighted score) would justify turning it on is raised in the report's own
  priority list but not decided in the code.
- **`share_widths` has no equivalent of `EVIDENCE_OVERRIDE`'s documented trade table.** Its
  refutation is reported as clean (it recovers width correctly and still loses on ink), but
  no environment variable or code path offers a partial or alternative acceptance rule the
  way the main per-face decoder does — it is simply off, with no sweep of intermediate
  settings shown.
- **No test in this file exercises `share_widths`, `fitted_params`, or the
  `INKVEC_DECODE_OVERRIDE` code path.** All three are validated only through the external
  `bench/research/decoding/` scripts and `REPORT.md`'s numbers, not through `cargo test`.
