# Stage 08 — Boundary solve

> Moves every boundary point of the planar map at once, so that the geometry's *exact
> rendered coverage* matches the image, instead of refining each point along its own
> one-dimensional normal.

**Source:** `crates/inkvec-trace/src/boundary_opt.rs`
**Entry point:** `optimise()` (`boundary_opt.rs:973`)
**Pipeline position:** after `refine_junc` (`planar::refine_junctions`, `lib.rs:462`), before
`decode` (stage mark `"boundary_opt"`, `lib.rs:473`). Called once, from
`trace_color_full_with_alpha` only (`lib.rs:469`) — the bilevel front end
(`trace_bilevel`, `lib.rs:156`) never calls it.

## What problem this solves

Every stage upstream of this one decides a boundary point on its own. `planar::build`
places it on the integer lattice; `refine_subpixel` slides it along its own normal until
the coverage read there is a half; `refine_junctions` intersects the edges that meet at a
node. Each of those is a one-dimensional argument about one point, and the module doc
comment (`boundary_opt.rs:1-9`) states plainly what a one-dimensional argument cannot see:
"A pixel's value is the area coverage of *every* region that touches it, so a point's
neighbours along the boundary change what that pixel should read; and a pixel says
nothing at all about motion *along* the boundary, so a point is free to slide unless
something holds it."

So the boundary is solved as one problem: every point is an unknown in a single
optimisation, and the objective being minimised is the actual rendering error — not a
proxy for it, not a per-point residual, but the exact clipped-pixel coverage the geometry
would paint, compared against the image.

## Inputs and outputs

**Input:** a mutable `PlanarMap` (`crates/inkvec-trace/src/planar.rs:47-53` — `edges`,
`width`, `height`, `n_labels`), the rendered image `rgb: &[[f32; 3]]`, the per-face
`face: &[FillModel]` (`gradient.rs:99`), and an optional time budget in milliseconds.

**Output:** the map is edited in place — every boundary point's position may move — and
`optimise` returns `Option<Report>`:

```rust
pub struct Report {
    pub before: f64,   // energy before the solve
    pub after: f64,    // energy after
    pub iters: usize,  // iterations actually taken
    pub moved: usize,  // points whose position changed by more than 1e-6 px
    pub scale: f64,    // fraction of the solved displacement kept, after the fold guard
}
```

`None` means nothing was gained: the map is empty, has fewer than three unknowns, the
initial data or kink term is zero, the solve failed to improve the energy, or the
self-crossing guard (below) could not accept any displacement at all.

## How it works

### The objective

```text
E = sum over boundary pixels || a·c_left + (1-a)·c_right - target ||^2
  + w_kink   * sum over points |p_{i-1} - 2*p_i + p_{i+1}|
  + w_anchor * sum over points |p_i - p_i^0|^2
```

`a` is the **exact area** of the pixel square on the left face's side of the boundary —
the pixel clipped by the chain that crosses it, closed along the pixel's own border. That
is the quantity a rasteriser actually computes, so the data term is the rendering error
itself, and — the reason the whole stage is tractable — its gradient is analytic: area is
a shoelace sum over the clipped polygon, and every vertex of that polygon is one of three
things: a boundary point (moving with its own unknown), a crossing of a pixel gridline
(moving as the two boundary points either side of it move), or a fixed corner of the
pixel.

### `Prov` and the analytic Jacobian

```rust
enum Prov {
    Vertex(u32),                          // a boundary point, moving with its unknown
    CrossV { line: f64, a: u32, b: u32 },  // crosses vertical gridline `line`
    CrossH { line: f64, a: u32, b: u32 },  // crosses horizontal gridline `line`
    Corner,                                // a pixel corner: fixed
}
```
(`boundary_opt.rs:111-120`)

`Prov` records, for every vertex of a clipped polygon, where that vertex came from and
therefore how it depends on the unknowns. This is what makes the derivative closed-form
instead of a finite difference: a finite-difference gradient would need to re-clip and
re-shoelace the polygon once per unknown per pixel, which is exactly the cost this stage
cannot afford at thousands of unknowns and pixels per icon. Instead, `scatter`
(`boundary_opt.rs:309-345`) takes `d(area)/d(vertex)` at one clipped vertex and pushes it
back onto whichever underlying unknowns produced that vertex:

- `Prov::Corner` contributes nothing — its position never varies.
- `Prov::Vertex(v)` passes the gradient straight through to unknown `v`.
- `Prov::CrossV { line, a, b }` is a point on the segment `a -> b` where it crosses a
  fixed vertical gridline `line`. Its `x` is pinned by the gridline, so only its `y`
  gradient propagates; parametrising the crossing as `a.y + t*(b.y - a.y)` with
  `t = (line - a.x)/(b.x - a.x)` and differentiating gives the four terms in
  `boundary_opt.rs:318-330`, split between `a` and `b` in proportion to `t`. `CrossH` is
  the mirror case.

The area itself is the shoelace sum `shoelace(pts)` (`boundary_opt.rs:297-305`), and its
derivative with respect to one vertex `i` is the standard `-0.5*(next.y - prev.y)`,
`-0.5*(prev.x - next.x)` pair (the sign is negative because the stored coverage is minus
the shoelace of the loop as built — `boundary_opt.rs:704-708`), scattered through `Prov`
at every vertex of the loop.

### `build_vars`: one unknown per point, junctions pinned

```rust
struct Vars {
    var: Vec<Vec<u32>>,   // var[edge][i] = the unknown holding point i of that edge
    start: Vec<Point>,
    junction: Vec<bool>,
}
```
(`boundary_opt.rs:135-140`, built by `build_vars`, `boundary_opt.rs:142-177`)

An edge's interior points each get their own unknown. An edge's *endpoints*, where several
edges meet at a shared node, are collapsed: `by_node` maps each planar-map node id to one
unknown, so every edge that touches that junction moves it together rather than each
edge dragging its own copy apart. Those shared endpoints are marked `junction[v] = true`.

Junction points are anchored four times harder (`JUNCTION_ANCHOR = 4.0`, applied in
`priors`, `boundary_opt.rs:927-932`) and are excluded from the data term entirely unless
`INKVEC_BOPT_JUNC` is set (see below). The module doc comment gives the reason
(`boundary_opt.rs:70-75`): "Where three or more faces meet, one chain no longer divides
the pixel in two and the coverages need the full clipped partition; the points there keep
their priors and their anchor, so they move with their neighbours but are not driven by
the image. `planar::refine_junctions` has already placed them by intersecting the
boundaries that meet there, which is better evidence than a single pixel's colour."

### Assembling the data term: `bucket` and `data_cells`

`Problem::bucket` (`boundary_opt.rs:474-530`) walks every edge, finds where each segment
crosses pixel gridlines (`crossings`, `boundary_opt.rs:180-206`), and files each resulting
`Piece` — one chain fragment lying inside one pixel — into that pixel's linked list
(`head`/`next`).

`Problem::data_cells` (`boundary_opt.rs:595-714`) then walks every pixel that has pieces
in it:

- If the pixel's pieces all belong to one edge and chain contiguously from one border
  point to another, the chain divides the pixel cleanly in two. `border_corners`
  (`boundary_opt.rs:240-264`) walks the pixel's own border between the chain's two
  endpoints to close the loop, `shoelace` gives the area of one side (tried both
  orientations, keeping whichever is non-positive so the sign always reads as the `left`
  face's side, `boundary_opt.rs:652-674`), and the mixture `a*c_left + (1-a)*c_right` is
  compared against the pixel's measured colour.
- If several edges' pieces land in the same pixel, that pixel is a **junction pixel** and
  is handled by `junction_pixel` (`boundary_opt.rs:731-891`, described below) only when
  `self.junctions` is set.
- A pixel below `MIN_CONTRAST = 2.0/255.0` (`boundary_opt.rs:99`) across the two faces'
  colours is skipped: "a pixel carries no usable evidence" below that contrast.

The residual is computed in `f64`, not `f32`, and the comment at `boundary_opt.rs:686-688`
explains why: "the mixture in f32 quantises the objective at a hundredth of the coverage
resolution the solver works at, which turns a smooth energy into a staircase the line
search cannot descend."

`Problem::data` (`boundary_opt.rs:542-593`) is the entry point that either runs
`data_cells` over the whole image sequentially, or — if `INKVEC_BOPT_CHUNKS` is set to
more than one — splits the pixel range into that many contiguous chunks, one per rayon
task, each with its own scratch buffer and gradient, summed back in chunk order. The
default is sequential summation, and the doc comment explains the trade explicitly
(`boundary_opt.rs:536-541`): parallel summation changes floating-point addition order,
"the last-bit differences cascade through tie-sensitive fit decisions — on one 300-path
logo they cost 8 paths and 16% more coordinates at the same colour error — so it stays
opt-in until the full set has priced it."

### Junction pixels: wedges, not a two-face split

`junction_pixel` (`boundary_opt.rs:731-891`) handles a pixel where several boundary
chains meet at a shared node, when `INKVEC_BOPT_JUNC` turns it on. It groups the pixel's
pieces into contiguous chains, requires every chain to run outward from the same interior
node to the pixel's border, and sorts them by where they exit. Consecutive chains (by exit
position around the border) bound a **wedge** — one face's exact coverage of that
corner of the pixel — found by walking outward along one chain, along the border to the
next chain's exit, then back along that chain to the shared node. Summing every wedge's
colour, weighted by its area, gives the pixel's modelled colour; the sum of wedge areas is
required to equal 1.0 within `1e-6`, or the construction is judged not to apply and the
pixel contributes nothing (`boundary_opt.rs:849-855`). This is the same statement as
solving a non-negative colour mixture for an anti-aliased pixel, "with the weights
constrained to be areas of an actual partition rather than free numbers"
(`boundary_opt.rs:727-730`).

The diagnostic counters in `pub mod juncstat` (`boundary_opt.rs:270-294`, dumped by
`INKVEC_JUNCDBG`) record why a candidate junction pixel was declined — wrong chain count,
mismatched ends, disagreeing node, an unrecognised face, or a partition that did not sum
to one — because, as the comment above the module says, "the construction below only
handles one shape of junction, and the point of these is to find out which shapes it is
actually meeting" (`boundary_opt.rs:267-269`).

### Priors: kink and anchor

`Problem::priors` (`boundary_opt.rs:894-944`) adds two regularisers.

**Kink** is the *absolute* value of the discrete second difference at each interior point
of each edge, smoothed by a small floor inside the square root
(`sqrt(dx^2 + dy^2 + EPS)`, `EPS = 1e-4`) so it is differentiable where the boundary is
already straight. The module doc comment explains why the absolute value and not the
square (`boundary_opt.rs:29-32`): "a corner then costs in proportion to how sharply it
turns, so one sharp corner is cheaper than the many small kinks a squared term would
spread it into — the staircase is smoothed and the corner survives."

**Anchor** is a plain squared distance from each point's starting position, weighted
`w_anchor` (or `w_anchor * JUNCTION_ANCHOR` at a junction). It "removes the tangential
freedom and holds a point the image cannot see (the interior of a long straight run, a
boundary between two nearly equal colours) where the measurement put it"
(`boundary_opt.rs:33-35`).

Both weights are set *relative to the data term's own initial value* rather than as
absolute numbers (`boundary_opt.rs:1028-1036`):

```rust
prob.w_kink   = env_f64("INKVEC_BOPT_KINK",   K_KINK)   * data0 / kink0;
prob.w_anchor = env_f64("INKVEC_BOPT_ANCHOR", K_ANCHOR) * data0 / n as f64;
```

"Scaling them to the data term is what makes them mean the same thing on a flat
two-colour logo and on a crowded emoji, where the residual differs by orders of
magnitude" (`boundary_opt.rs:91-93`).

### The solve: conjugate gradient with a leashed line search

`optimise` runs Fletcher–Reeves nonlinear conjugate gradient (`boundary_opt.rs:1038-1103`):

1. Compute the energy and its gradient at the start; the initial direction is steepest
   descent.
2. Each iteration, scale the step so the largest per-point displacement is `MAX_STEP`
   (`0.35` px), then leash every trial point back to within `MAX_TOTAL` (`1.0` px) of
   where the measurement originally put it (`boundary_opt.rs:1060-1069`).
3. Backtrack up to six times (`step *= 0.4` each retry) until the trial energy improves;
   stop the whole solve if even the smallest backtrack does not help, or if the relative
   improvement drops below `1e-4` (`boundary_opt.rs:1081`).
4. Otherwise update the conjugate-gradient direction with the Fletcher–Reeves ratio
   `beta = gg_new / gg`, restarting to steepest descent whenever the resulting direction
   is not itself downhill (`boundary_opt.rs:1097-1102`) — a known failure mode of
   Fletcher–Reeves on a non-quadratic objective.
5. Stop when the iteration budget or the time budget (`INKVEC_BOPT_MS`, default `1200` ms,
   or the caller-supplied `budget_ms`) is exhausted.

If the final energy is not below the starting energy, or no iteration was accepted at
all, `optimise` returns `None` and the map is left untouched — the caller passed `&mut
map` by reference, but the mutation only happens after the whole solve-and-guard sequence
below succeeds (the working positions live in a local `pos: Vec<Point>` until then).

### The fold guard

A solved displacement can make the boundary self-intersect: two sides of a thin ribbon can
be pulled toward the same ink between them and pass through each other. `crossings_count`
(`boundary_opt.rs:386-444`) counts segment pairs that cross, using a spatial hash bucketed
by pixel so only segments sharing a pixel are ever compared (a point moves less than a
pixel per solve, so a new crossing is always local). The count is taken **before** the
solve and compared against the count **after**, not against zero — earlier stages can
already have left a fold behind, "which is what the repair stage exists for, and refusing
to improve a boundary because of a crossing that was already there would give up most of
the gain" (`boundary_opt.rs:380-382`). If the solved displacement introduces a *new*
crossing, the whole displacement (every point, uniformly) is scaled back toward the
starting position by successive halving (`scale *= 0.5`) until no new crossing remains or
`scale` drops to `0.1`, at which point the solve is abandoned entirely
(`boundary_opt.rs:1113-1133`). The doc comment on `crossings_count`
(`boundary_opt.rs:373-377`) states the cost of not guarding this: "downstream that costs
far more than the boundary error it bought — the repair stage refits the offending rings
round after round (2.5 s on one logo) and the emitter paints a face over its own
interior."

## Constants and thresholds

| name | value | controls | stated derivation |
|---|---|---|---|
| `MAX_STEP` | `0.35` px | largest per-point displacement in one CG step | none stated beyond its role |
| `MAX_TOTAL` | `1.0` px | total leash from the point's starting (measured) position | "This is a refinement of the boundary, not a search for it: a point a pixel away from its own level set has stopped describing the same piece of the image" (`boundary_opt.rs:88-90`) — qualitative, no swept value |
| `K_KINK` | `0.05` | kink weight, as a fraction of the data term's initial value | scaling rule is derived (relative to `data0`); the specific `0.05` is not swept in this comment |
| `K_ANCHOR` | `0.10` | anchor weight, as a fraction of the data term's initial value | same as above; `0.10` itself not swept here |
| `JUNCTION_ANCHOR` | `4.0` | multiplier on `w_anchor` at a junction point | "anchored harder, for the reason given in the module comment" (junctions need better evidence than one pixel) — the multiplier's own value is not derived |
| `MIN_CONTRAST` | `2.0/255.0` | pixel usable-evidence floor for the data term | no stated derivation |
| `EPS` (in `priors`) | `1e-4` | floor inside the kink term's square root, for differentiability at zero curvature | stated purpose, no numeric derivation |
| `INKVEC_BOPT_ITERS` default | `48` | iteration cap | **measured**: 24 was found unconverged; see Failure modes / Environment overrides |
| `INKVEC_BOPT_MS` default | `1200` ms | time budget | "the 1200 ms budget below was never the binding constraint. Measuring at a 60 s budget gave the same 0.4142" (`boundary_opt.rs:1002-1003`) |
| degenerate-box guard (in `crossings_count`) | `(x1-x0)*(y1-y0) > 64` | skips a segment whose bounding box would touch too many spatial-hash cells | "A degenerate box would put a segment in every cell; the map never has one" (`boundary_opt.rs:406-407`) — asserted, not derived |
| fold-guard floor | `scale > 0.1` | how far the solve will keep halving the accepted displacement before giving up entirely | no stated derivation |
| backtracking factor | `step *= 0.4`, up to 6 tries | line-search backoff | no stated derivation |
| relative-improvement stop | `rel < 1e-4` | early stop once a step buys almost nothing | no stated derivation |
| partition tolerance (`junction_pixel`) | `(sum - 1.0).abs() > 1e-6` | rejects a wedge set that does not sum to one pixel of area | exact geometric identity, not a tuned threshold |

## Failure modes and edge cases

### The sawtooth, and four rejected cures

The module's longest piece of institutional knowledge is its own section on a failure it
could not fix by tuning (`boundary_opt.rs:37-68`), reproduced because it is the most
load-bearing paragraph in the file:

> Area coverage does not determine a boundary. Any wiggle that preserves how much of each
> pixel falls on either side leaves the data term exactly unchanged, and on a stroke about
> two pixels wide — where both of its sides compete for the same pixels, and most of those
> pixels are excluded as junction pixels anyway — the solver wanders into that null space
> and returns a row of triangular teeth. They render almost as well as a straight edge and
> look nothing like one, which is the whole problem: `openmoji/1F3A1` moves by 0.008 in
> colour error while turning a smooth grey stroke into a saw.
>
> Measured 2026-09-03, all rejected, none shipped:
>
> * **More smoothing.** The teeth do clear, at about twenty times the shipped kink weight.
>   They take real detail with them: on 246 icons DISTS goes 0.0310 to 0.0363.
> * **A length term** on the boundary, the textbook cure for this null space, since a
>   zigzag is much longer than the straight edge with the same per-pixel areas. At weights
>   that leave the rest of the corpus alone it barely touches the teeth.
> * **Anchoring each point by its own measured uncertainty**, which is appealing because
>   `refine_subpixel` already marks thin-ribbon points uncertain (its coverage gradient is
>   small there) and it needs no new threshold. It does not remove the teeth either.
> * **Stopping the solver early.** The teeth grow with iteration count: one iteration is
>   clean, three shows them, twenty-four is a saw. On thin-stroke icons four iterations
>   beat twenty-four on every axis (dE00 0.7625 against 0.7852, DISTS 0.0568 against
>   0.0633) — but on the 246-icon screening set the full count wins (0.2249 against
>   0.2293), because everything that is not a thin ribbon is still converging usefully.
>
> The pattern in all four is the same: this is a *local* failure of the model and a global
> knob cannot serve both cases. The model's own domain is the honest place to fix it — a
> ribbon two pixels wide has no interior, so its two sides are not two independent
> boundaries and should not be solved as though they were. That is LOG-44, fitting a thin
> face as a centreline and a width, and it is the same root cause as the lumpy strokes and
> the failure of a wider tangent window in `refine_subpixel`. **Do not add a fifth knob
> here.**

What the sawtooth *is*, in plain terms: area coverage is a many-to-one map from boundary
shape to rendered pixels. A thin, near-symmetric stroke has a large family of boundary
shapes — including jagged ones — that render almost identically, because whatever one side
loses to a wiggle the other side gains back inside the same pixel. The solver has no reason
to prefer the smooth member of that family over a jagged one, and every generic
regulariser tried (more smoothing, a length penalty, per-point uncertainty weighting,
fewer iterations) either failed to separate them or cost real detail elsewhere on the
corpus to do so. `research/decoding`'s later work (see `09-decode.md`) is the model-order
fix this note points at: treat a thin face as a centreline-plus-width rather than as two
independent boundaries.

### The junction wedge term, tested and found worse

`INKVEC_BOPT_JUNC` (default off) turns on `junction_pixel` so junction pixels enter the
data term through the wedge construction above. `bench/research/decoding/REPORT.md`
records a corpus measurement of turning it on (screen set, 246 icons, paired against the
shipped default):

| | dE00 | DISTS | params ratio | objective |
|---|---|---|---|---|
| junctions off (shipped) | 0.1582 | 0.0275 | 1.41 | **0.4328** |
| junctions on | 0.1668 | 0.0289 | 1.46 | 0.4562 |

"noto-emoji 0.389 -> 0.418, openmoji 0.168 -> 0.187, twemoji 0.137 -> 0.151; lucide
unchanged because it never fires. Including junction pixels through the wedge model moves
the boundaries the wrong way. Left off." The same report also corrects an earlier,
mistaken diagnosis that junction pixels carried 88% of the corpus's remaining error — that
number came from a classifier that conflated antialiasing with genuine junctions; measured
directly, junction pixels are 2.07% of the image and about 12.9% of the remaining error,
and `boundary_opt`'s own instrumentation shows the wedge branch is `seen 0` times on the
icons the mistaken classifier was scored on.

### The unconverged iteration count

The comment above the `INKVEC_BOPT_ITERS` default (`boundary_opt.rs:988-1003`) is a second
piece of measured history worth quoting closely:

> 48, not 24: at 24 this solve stops before it has converged, and the boundary it hands on
> is still moving. `gt_diff` attributes 94% of the remaining error to boundaries, so that
> mattered more than any threshold in the tracer.
>
> Full set, 980 icons: objective 0.4519 -> 0.4428, with dE00 0.1660 -> 0.1620, DISTS 0.0286
> -> 0.0281 and parameters against the artist 1.46 -> 1.42. Every family improves or holds;
> simple-icons goes 1.31 -> 1.13 on parameters and material-icons 0.0743 -> 0.0604 on
> dE00. Improving fidelity and cost together is what says this is convergence rather than a
> trade.
>
> It is not monotone past that — 96 reads 0.4157 and 192 reads 0.4160 on the screen split
> against 48's 0.4142 — so the step schedule drifts once the residual stops driving it, and
> more iterations are not better iterations.
>
> Nearly free: 573 ms/icon to 578 ms, and the 1200 ms budget below was never the binding
> constraint. Measuring at a 60 s budget gave the same 0.4142.

Note the two different objective figures quoted are on two different splits — `0.4519 ->
0.4428` is the full 980-icon set (the number that justified 24 -> 48), while `0.4142` /
`0.4157` / `0.4160` are on the 246-icon screen split (used only to show that going past 48
does not help). They should not be conflated.

## Environment overrides

| variable | default | effect |
|---|---|---|
| `INKVEC_BOPT` | on (any value other than `"0"`) | read in `lib.rs`, not in this file: disables the whole stage when set to `"0"` |
| `INKVEC_BOPT_ITERS` | `48` | iteration cap |
| `INKVEC_BOPT_MS` | `1200` | time budget in milliseconds; overridden by the caller's `budget_ms` when supplied |
| `INKVEC_BOPT_KINK` | `K_KINK = 0.05` | kink-weight fraction of the initial data term |
| `INKVEC_BOPT_ANCHOR` | `K_ANCHOR = 0.10` | anchor-weight fraction of the initial data term, divided by point count |
| `INKVEC_BOPT_JUNC` | off | enables the wedge construction for junction pixels — measured worse on the corpus (see Failure modes) |
| `INKVEC_BOPT_CHUNKS` | `1` (sequential) | parallel chunk count for the data-term sum; more than one changes summation order and, downstream, tie-sensitive fit decisions |
| `INKVEC_BOPTDBG` | off | per-iteration `eprintln!` of step, energy, and relative improvement, plus the fold-guard summary |
| `INKVEC_BOPT_CELLS` | off | per-pixel `eprintln!` of the clipped area computed in `data_cells` |
| `INKVEC_JUNCDBG` | off | dumps the `juncstat` counters explaining why candidate junction pixels were accepted or declined |

## Open questions

- **`K_KINK = 0.05`, `K_ANCHOR = 0.10`, `JUNCTION_ANCHOR = 4.0`, `MAX_STEP = 0.35`,
  `MAX_TOTAL = 1.0`, `MIN_CONTRAST = 2.0/255.0`** all have a stated *reason for existing*
  (why a kink term, why an anchor, why a leash, why a contrast floor) but none carries a
  swept numeric derivation the way the iteration count and the merge distance elsewhere in
  the tracer do. These read as engineering choices consistent with the design, not values
  independently justified by a measurement.
- **`INKVEC_BOPT_CHUNKS`'s default of `1`** is justified by a single anecdote — "on one
  300-path logo they cost 8 paths and 16% more coordinates at the same colour error" — and
  the comment says outright that this is provisional ("stays opt-in until the full set has
  priced it"). This is explicitly a one-test-case measurement standing in for a corpus
  sweep that has not yet been run.
- **The fold-guard floor `scale > 0.1`** and the backtracking factor `0.4` have no stated
  derivation; they read as reasonable defaults rather than measured ones.
- **Why does the objective drift upward past 48 iterations** (0.4142 at 48, 0.4157 at 96,
  0.4160 at 192, on the screen split) is observed but not explained mathematically — the
  comment offers "the step schedule drifts once the residual stops driving it" as a
  description, not a mechanism.
- **The degenerate-box guard in `crossings_count`** (`(x1-x0)*(y1-y0) > 64`) is asserted
  ("the map never has one") rather than derived from a bound on segment length or pixel
  size.
- **This module and `refine_subpixel`/`refine_junctions` do not share a common notion of
  positional confidence.** `boundary_opt` anchors every point equally hard except at
  junctions; it does not consume `CoverageField::position_sigma` (see `02-coverage.md`) or
  any per-point sigma from upstream, even though one rejected sawtooth cure was exactly
  "anchoring each point by its own measured uncertainty." Whether a *different* use of
  per-point sigma (not the one tried) would behave differently is not addressed.
