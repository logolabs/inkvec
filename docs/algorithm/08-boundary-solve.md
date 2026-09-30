# Stage 08 — Boundary solve

> Moves every boundary point of the planar map at once, so that the geometry's *exact
> rendered coverage* matches the image, instead of refining each point along its own
> one-dimensional normal.

**Source:** `crates/inkvec-trace/src/boundary_opt.rs` (unknowns, priors, energy, fold
guard), `boundary_opt/band.rs` (the data term), `boundary_opt/lbfgs.rs` (the solver),
`boundary_opt/folds.rs` (self-crossing count)
**Entry point:** `optimise_alpha()` (`boundary_opt.rs:579`); `optimise()` is the same
without alpha
**Pipeline position:** after `refine_junc` (`planar::refine_junctions`, `lib.rs:1090`),
before `decode` (stage mark `"boundary_opt"`, `lib.rs:1103`). Called once, from
`trace_color_full_with_alpha` only (`lib.rs:1098-1099`), and only in Quality mode (skipped
when `opts.fast` is set). The bilevel front end (`trace_bilevel`) never calls it.

## What problem this solves

Every stage upstream of this one decides a boundary point on its own. `planar::build`
places it on the integer lattice; `refine_subpixel` slides it along its own normal until
the coverage read there is a half; `refine_junctions` intersects the edges that meet at a
node. Each of those is a one-dimensional argument about one point, and it cannot see two
facts: "A pixel's value is the area coverage of *every* region that touches it, so a
point's neighbours along the boundary change what that pixel should read; and a pixel says
nothing at all about motion *along* the boundary, so a point is free to slide unless
something holds it" (module doc, `boundary_opt.rs:5-9`).

So the boundary is solved as one problem: every point is an unknown in a single
optimisation, and the objective is the actual rendering error — the exact box-filtered
coverage the geometry would paint, compared against the image.

## Inputs and outputs

**Input:** a mutable `PlanarMap` (`edges`, `width`, `height`, `n_labels`), the image
`rgb: &[[f32; 3]]` (sRGB `0..1`, row-major), each face's `FillModel`, an optional
wall-clock budget `budget_ms` (the CLI's `--time-budget`; none by default, and then the
result depends only on the input), and optionally the source alpha and each face's
opacity.

**Output:** the map is edited in place and `optimise_alpha` returns `Option<Report>`:

```rust
pub struct Report {
    pub before: f64,   // energy before the solve
    pub after: f64,    // energy after
    pub iters: usize,  // most iterations any independent part took
    pub moved: usize,  // points whose position changed by more than 1e-6 px
    pub scale: f64,    // fraction of the solved displacement kept by the fold guard
}
```

`None` means nothing changed: the map is empty or has fewer than three unknowns, the
starting data or kink term is zero, the energy did not fall, or the fold guard could not
keep a tenth of the displacement. The working positions live in a local vector until the
whole solve and guard have succeeded.

## How it works

### The unknowns

`build_vars` (`boundary_opt.rs:177`) gives every point of every edge its own unknown,
except that the end points of open edges are keyed by their node id, so all the edges
meeting at a junction share one unknown and move it together. That is what keeps the map a
partition however far the points move. Unknowns are numbered in edge order, then point
order, which fixes every summation order downstream.

`band::pin_frame` then snaps points lying on the image frame (within `1e-3` px of
`x = −½`, `x = w − ½`, `y = −½` or `y = h − ½`, on an edge against the outside) exactly
onto it and lets them move only along it. Not from the literature: a boundary condition,
needed because the coverage accumulation starts from the outside at `x = −½`.

### The objective

```text
E = Σ_{p ∈ B} w_p · ‖ Σ_f cov_f(p)·c_f(p) − t_p ‖²
  + w_kink   · Σ over points sqrt(|p_{i−1} − 2p_i + p_{i+1}|² + 10⁻⁴)
  + w_anchor · Σ over points |p_i − p_i⁰|²
```

`cov_f(p)` is the exact area of face `f` inside pixel `p` (the faces partition the pixel,
so these sum to one), `c_f(p)` the face's fill evaluated at the pixel centre, `t_p` the
image, and `B` a fixed band of pixels. The residual is the Chan–Vese region term with each
face's fill as its constant (T. F. Chan, L. A. Vese (2001), *Active contours without
edges*, IEEE TIP 10(2), <https://doi.org/10.1109/83.902291>; formula as in P. Getreuer
(2012), *Chan–Vese Segmentation*, IPOL, <https://doi.org/10.5201/ipol.2012.g-cv>), with the
fills held fixed during the solve.

### The band

`B` is every pixel within one pixel (Chebyshev, `band::REACH = 1`) of a pixel a boundary
crosses at the start. No point moves more than `MAX_TOTAL` = 1 px, so a piece of boundary
can only ever land inside `B`, and `B` never has to be rebuilt: the narrow band of
D. Adalsteinsson, J. A. Sethian (1995), *A fast level set method for propagating
interfaces*, J. Comput. Phys. 118, <https://doi.org/10.1006/jcph.1995.1098>, fixed for the
whole solve. (`nearest_in_band` handles the one exception: a piece whose midpoint sits
exactly on a pixel border one and a half pixels out is filed under the neighbouring band
pixel, which keeps its carried height, and so every coverage in the row, exact.)

Rendering *every* band pixel, not only the pixels a boundary cuts, is what makes the energy
continuous: leaving a pixel costs exactly what being a pure pixel of the other face costs.
The first form of this stage summed only the cut pixels, deciding in each which side was
which. That made the energy jump whenever a boundary left or entered a pixel, and it put a
piece lying exactly on a pixel border on the wrong side. Measured on the 246-icon screen
set, a quarter of every accepted decrease came from pixels leaving the sum rather than from
fitting them, and 170 of 246 solves stopped because a step of 0.0014 px raised the energy
by a whole pixel's residual.

### Exact coverage by signed-area accumulation

`Problem::bucket_band` walks every edge, cuts each segment where it crosses a pixel
gridline (`crossings`), and files each `Piece` (one fragment of one chain inside one
pixel) into that pixel's list. Each end of a piece carries a `Prov`:

```rust
enum Prov {
    Vertex(u32),                          // a boundary point, moving with its unknown
    CrossV { line: f64, a: u32, b: u32 },  // where segment a→b crosses vertical gridline `line`
    CrossH { line: f64, a: u32, b: u32 },  // where segment a→b crosses horizontal gridline `line`
}
```

`scatter` uses it to push a derivative with respect to a piece end back onto the unknowns
that produced it (a gridline crossing moves with both ends of its segment, in proportion to
where it lies between them), so the gradient is analytic.

`Problem::band_data` then renders each row's band runs left to right. Every piece deposits
into its pixel the signed area between itself and the pixel's right side, for the faces on
its left and right, and carries its height to every pixel further right; a pixel's
coverage of face `f` is its own deposits plus the carry. This is the accumulation-buffer
rasteriser of libart and R. Levien's font-rs (<https://github.com/raphlinus/font-rs>), an
exact box filter as in J. Manson, S. Schaefer (2011), *Wavelet Rasterization*, Computer
Graphics Forum 30(2), <https://doi.org/10.1111/j.1467-8659.2011.01887.x>. Adapted: one carry
per face instead of one winding number, since the faces are a partition; the carry's
derivative is a suffix sum along the row, so the gradient costs one more pass; and a piece
lying on a pixel border simply deposits zero area and a full carry, so no pixel ever
decides which side is which.

A run's *seed* (the face filling everything left of it) is fixed once at the start: the
pixel left of a run is never touched, so it is one face, found by carrying from the left
edge of the image. A run at column 0 also takes the pieces left of the image every time.

Two details keep the evaluation cheap. Consecutive pixels a single straight piece stretches
across are summed at once as a quadratic form in the carry vector (`stretch_energy`, with
per-run prefix sums of the fills, `fill_prefix`). Runs are independent, so large bands are
evaluated in parallel with a fixed partition and a fixed order of summation, and the result
does not depend on the thread count.

The data term and the mixture are computed in f64: in f32 the energy is a staircase the
line search cannot descend.

### Which pixels count

Each band pixel's weight `w_p` is fixed at the start (`band::setup`):

- **Zero where two boundaries meet.** A pixel holding pieces of two or more boundaries
  between modelled faces at the start (a junction, or both sides of a stroke too thin to
  have an interior) is left out of the data term for the whole solve (`exclude_junctions`).
  Its colour is a three-way mixture the fills are least reliable at, and on a thin stroke
  the two sides compete for one pixel's evidence (the sawtooth below). Measured on the
  screen set: objective 0.3713 leaving them out, 0.3908 with them in, and 0.3908 again
  leaving out only the pixels round the junction nodes.
- **Zero where the seed is uncertain.** A run whose seed is not a single face with full
  coverage has weight zero.
- **One elsewhere.**

With alpha (`optimise_alpha` given the source alpha and the faces' opacities), a band pixel
within reach of a boundary whose faces differ in opacity by at least `MIN_CONTRAST` = 2/255
but in colour by less (white paint on the clear ground, the bands of one fade), and of no
boundary whose faces differ in colour, also compares the coverage-weighted opacity with the
source alpha, as a fourth channel (`alpha_channels`).

`setup` also records the starting residual split two ways: the pixels the boundary cuts
(`data0`, what the prior weights are scaled to) and the rest (a constant per run, used by
the stopping rule).

### Priors: kink and anchor

`Problem::priors` adds two terms. **Kink** is the *absolute* value of the discrete second
difference at each interior point of each edge, smoothed by `10⁻⁴` inside the square root:
a corner then costs in proportion to how sharply it turns, so one sharp corner is cheaper
than the many small kinks a squared term would spread it into. **Anchor** is the squared
distance from each point's start, four times heavier at a junction (`JUNCTION_ANCHOR`); it
removes the tangential freedom and holds points the image cannot see where the measurement
put them.

Both weights are relative to the data term (`lbfgs::descend`):

```rust
prob.w_kink   = K_KINK   * data0 / kink0;   // kink starts at 5% of the data term
prob.w_anchor = K_ANCHOR * data0 / n as f64; // 1 px costs a tenth of a point's share
```

so they mean the same thing on a flat two-colour logo and on a crowded emoji.

### Independent parts

`band::components` splits the problem with a union-find: two boundaries are in one part
when they share an unknown (a junction) or when pieces of both lie within reach of one band
run. The energy is exactly the sum of the parts' energies (each run belongs to one part),
so each part is minimised on its own and stops when *it* has converged, instead of every
part paying for the slowest one. A median icon has five parts; a page of text has
hundreds. Parts with no band run (the frame's top, right and bottom edges, which no pixel
reads) are skipped. Block-separable minimisation, as a sparse solver's independent residual
blocks (Ceres Solver documentation, <https://github.com/ceres-solver/ceres-solver>,
`docs/source/nnls_solving.rst`).

### The solver: L-BFGS with a projected Armijo line search

`lbfgs::solve` runs, per part:

1. **Direction:** limited-memory BFGS, the two-loop recursion over the last `MEMORY` = 3
   steps with initial scaling `γ = sᵀy / yᵀy` (D. C. Liu, J. Nocedal (1989), *On the
   limited memory BFGS method for large scale optimization*, Math. Programming 45,
   <https://doi.org/10.1007/BF01589116>; J. Nocedal, S. J. Wright (2006), *Numerical
   Optimization*, Algorithm 7.4, <https://doi.org/10.1007/978-0-387-40065-5>). A pair with
   non-positive curvature is not stored; a direction that is not downhill clears the memory
   and falls back to steepest descent.
2. **Step length:** the unit step (on the first iteration, a move of `MAX_STEP`), capped so
   no point moves more than `MAX_STEP` = 0.35 px, then halved up to `MAX_TRIALS` = 8 times
   until the Armijo condition `E(trial) ≤ E + 10⁻⁴·a·gᵀd` holds (Nocedal & Wright,
   Algorithm 3.1). Every trial point is projected back into the 1 px disc round its start
   and onto the frame where it is pinned. The energy is continuous but only piecewise
   smooth (its slope jumps where a piece meets a gridline), the setting of A. S. Lewis,
   M. L. Overton (2013), *Nonsmooth optimization via quasi-Newton methods*, Math. Program.
   141, <https://doi.org/10.1007/s10107-012-0514-2>, who found BFGS with an inexact line
   search reliable there.
3. **Stop** when no trial is accepted, when a step moves no point more than `PARAM_TOL` =
   0.005 px (half the 0.01 px the SVG writes), when a step lowers the part's energy by less
   than `FUNC_TOL` = 10⁻⁴ of what the geometry can still change (its energy less the
   constant of its untouched pixels), after `MAX_ITERS` = 32 iterations, or when the
   caller's budget runs out.

Nothing is linearised: every trial re-renders the exact coverage of the part's runs.

### The fold guard

A solved displacement can make the boundary self-intersect: two sides of a thin ribbon can
be pulled toward the same ink and pass through each other. Downstream that costs far more
than the boundary error it bought (the repair stage refits the offending rings round after
round, 2.5 s on one logo, and the emitter paints a face over its own interior).
`fold_guard` counts crossing segment pairs at the start and at the solution, and while the
solution adds crossings scales the whole displacement back (`p⁰ + s(p − p⁰)`,
`s = 1, ½, ¼, …` while `s > 0.1`). The count is compared with the start's, not with zero:
an earlier stage may already have left a fold for the repair stage, and refusing to
improve a boundary because of it would give up most of the gain.

The count (`folds::FoldCounter`) is a spatial join after J. Dittrich, B. Seeger (2000),
*Data redundancy and duplicate detection in spatial join processing*, ICDE,
<https://doi.org/10.1109/ICDE.2000.839452>: segments are entered in a coarse grid by their
range swept over every position the guard will ask about, and a pair is reported only in
the cell holding the reference point of the two ranges' intersection. The candidate list
is built once and each count re-applies the exact per-position test, so the count is the
same integer the per-count hash grid it replaced produced (byte-identical output on every
mode, commit `a192c46`).

## Constants and thresholds

| name | value | where | controls | basis |
|---|---|---|---|---|
| `MAX_STEP` | 0.35 px | `boundary_opt.rs:110` | largest displacement of any point in one L-BFGS step | none |
| `MAX_TOTAL` | 1.0 px | `boundary_opt.rs:114` | leash round each point's start; also why the band never moves | "a point a pixel away from its own level set has stopped describing the same piece of the image" |
| `K_KINK` | 0.05 | `boundary_opt.rs:121` | kink weight, fraction of the starting data term | scaling rule derived, value not swept |
| `K_ANCHOR` | 0.10 | `boundary_opt.rs:124` | anchor weight, fraction of a point's share of the starting data term | as above |
| `JUNCTION_ANCHOR` | 4.0 | `boundary_opt.rs:126` | anchor multiplier at a shared end point | not derived |
| `MIN_CONTRAST` | 2/255 | `boundary_opt.rs:128` | colour or opacity difference that counts as a boundary when choosing the pixels where alpha is a fourth channel | none |
| `EPS` (kink) | 1e-4 | `boundary_opt.rs:440` | floor inside the kink term's square root | for differentiability |
| `REACH` | 1 px | `band.rs:71` | band width round the pixels the start crosses | follows from `MAX_TOTAL` |
| `MEMORY` | 3 | `lbfgs.rs:41` | L-BFGS pairs kept | measured: 3 did as well as 7, 15 or 30 |
| `C1` | 1e-4 | `lbfgs.rs:43` | Armijo constant | textbook (Nocedal & Wright) |
| `MAX_TRIALS` | 8 | `lbfgs.rs:45` | halvings per line search | none |
| `MAX_ITERS` | 32 | `lbfgs.rs:49` | iterations per independent part | measured (below) |
| `PARAM_TOL` | 0.005 px | `lbfgs.rs:52` | stop when no point moves more | half the SVG's 0.01 px resolution |
| `FUNC_TOL` | 1e-4 | `lbfgs.rs:55` | stop on relative decrease of the changeable energy | none |
| fold-guard floor | `s > 0.1` | `boundary_opt.rs:658` | how far the guard halves the displacement before giving up | none |
| segment bucket limit | cell range `(x1−x0)(y1−y0) ≤ 64` | `folds.rs` | segments counted by the fold guard (the map never has a larger one) | kept exactly from the grid it replaced |

## Measurements

Against the per-pixel term with Fletcher–Reeves conjugate gradients it replaced (the
behaviour on `4f1fb88`):

| set | objective | mean dE00 | worst tenth dE00 | DISTS | params vs artist | better / worse |
|---|---|---|---|---|---|---|
| screen (246) | 0.3873 → **0.3585** | 0.1563 → 0.1427 | 0.5048 → 0.4669 | 0.0259 → 0.0242 | 1.488 → 1.503 | 155 / 82 |
| held_a (156) | 0.3958 → **0.3366** | 0.1480 → 0.1366 | 0.4626 → 0.4183 | 0.0243 → 0.0218 | 1.477 → 1.472 | 104 / 46 |

Stage time on the four standard inputs (ms, new against old): flat logo 26.5 / 25.0,
Noto gradient emoji 70.2 / 58.1, 2048 px Twemoji globe 193.6 / 185.1, masthead 319.2 /
332.0; about a fifth less on the screen set's icons. Whole-trace times are equal within
noise. Fast mode is untouched (246/246 byte-identical).

## Failure modes and history

### The sawtooth

Area coverage does not determine a boundary. Any wiggle that preserves how much of each
pixel falls on either side leaves the data term unchanged, and on a stroke about two
pixels wide, where both sides compete for the same pixels, a solver can wander into that
null space and return a row of triangular teeth that render almost as well as a straight
edge and look nothing like one (`openmoji/1F3A1` moved by 0.008 in colour error while
turning a smooth grey stroke into a saw).

Measured 2026-09-03, all rejected: twenty times the kink weight (clears the teeth, costs
detail: DISTS 0.0310 to 0.0363 on 246 icons); a length term (barely touches them);
anchoring each point by its measured uncertainty (does not remove them); stopping early
(wins on thin strokes, loses on the rest). A ribbon two pixels wide has no interior, so
its two sides are not two independent boundaries: that is LOG-44, fitting a thin face as a
centreline and a width. Until then its pixels are left out of the data term (above).
**Do not add another knob here.**

### What the solver replaced, and what was tried

On the continuous band energy the Fletcher–Reeves solver moved the furthest point 0.35 px
on every one of its 48 iterations while the energy fell by a hundredth of a percent: it
took the first trial that lowered the energy from a fresh 0.35 px step each time, and
wandered along the energy's flat directions. Measured and dropped while choosing its
replacement:

- **Levenberg–Marquardt** with a Gauss–Newton Hessian (banded Cholesky): the same
  wandering at about a hundred times the cost.
- **Polak–Ribière+ and Hager–Zhang conjugate gradients** with a Wolfe line search: more
  energy evaluations than L-BFGS for the same result.
- **L-BFGS preconditioned** with the priors' exact banded Hessian: steps far too short,
  because the smoothed ℓ1 kink term is a hundred times stiffer at a straight run than
  anywhere a real corner is.
- **Clipping each point's step** separately instead of scaling the whole step: no gain.
- **Iteration cap:** 24 reads 0.3666 on the screen set, 32 reads 0.3585, 48 reads 0.3558;
  48 made the stage a fifth to nine tenths slower on the standard inputs, so 32 ships.
- **Junction pixels in the data term:** measured worse twice, first as the wedge
  partition of the old per-pixel term (objective 0.4328 → 0.4562 on the screen set, with
  noto-emoji 0.389 → 0.418), then in the band (0.3713 → 0.3908, above).

The earlier finding that the Fletcher–Reeves solve was unconverged at 24 iterations (980
icons, objective 0.4519 → 0.4428 at 48) is what the stopping rules above now settle per
part: a part stops when its points stop moving, not at a global count.

## Environment overrides

The engine reads its environment through `inkvec_core::env`; the full list, with what was
removed and why, is [`docs/internal/env-vars.md`](../internal/env-vars.md).

| variable | default | effect |
|---|---|---|
| `INKVEC_BOPT` | on | read in `lib.rs`: `0` disables the whole stage |
| `INKVEC_BOPTDBG` | off | per-iteration `eprintln!` of step, energy, relative decrease and largest move, plus the fold-guard summary |

The former `INKVEC_BOPT_ITERS`, `INKVEC_BOPT_MS`, `INKVEC_BOPT_KINK`,
`INKVEC_BOPT_ANCHOR`, `INKVEC_BOPT_JUNC`, `INKVEC_BOPT_CHUNKS`, `INKVEC_BOPT_CELLS` and
`INKVEC_JUNCDBG` are gone: the stage has one behaviour.

## Open questions

- **`K_KINK`, `K_ANCHOR`, `JUNCTION_ANCHOR`, `MAX_STEP`, `MIN_CONTRAST`** have a reason to
  exist but no swept value. They were tuned for the per-pixel solver; the band energy's
  scale (every band pixel, not only the cut ones) is different, and a joint sweep may pay.
- **The sawtooth's pixels are simply left out.** LOG-44 (a thin face as a centreline and a
  width) is the model-order fix; until then a two-pixel stroke is solved from its ends and
  its priors.
- **Twenty gradients cost more.** The one standard input where the stage is clearly slower
  than before is a Noto emoji with twenty gradient fills (+21%, 58 ms to 70 ms); where that
  time goes has not been profiled.
- **No per-point confidence.** The anchor is the same for every non-junction point; it
  does not consume `refine_subpixel`'s per-point sigma. One use of it was tried against the
  sawtooth and failed; whether another use would help is untested.
