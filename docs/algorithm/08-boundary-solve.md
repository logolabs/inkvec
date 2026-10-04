# Stage 08 — Boundary solve

> Moves every boundary point of the planar map at once, so that the geometry's *exact
> rendered coverage* matches the image, instead of refining each point along its own
> one-dimensional normal.

**Source:** `crates/inkvec-trace/src/boundary_opt.rs` (unknowns, priors, energy, fold
guard), `boundary_opt/band.rs` (the data term, and the memory budget of its tables),
`boundary_opt/lbfgs.rs` (the solver and its stopping rules), `boundary_opt/linesearch.rs`
(the Moré–Thuente line search), `boundary_opt/folds.rs` (finding self-crossings)
**Entry point:** `optimise_alpha_capped()` (`boundary_opt.rs:627-683`), reached from the
pipeline through `optimise_for()` (`boundary_opt.rs:598-611`); `optimise_alpha()`
(`boundary_opt.rs:583-591`) is it without an iteration cap and `optimise()`
(`boundary_opt.rs:565-572`) the same without alpha
**Pipeline position:** after `refine_junc` (`planar::refine_junctions`, `lib.rs:1158`),
before `decode` (stage mark `"boundary_opt"`, `lib.rs:1167`). Called once, in
`finish_color_trace_alpha` (`optimise_for`, `lib.rs:1166`), the geometry tail every colour entry point
shares: the opaque Quality path (`trace_color_full_with_alpha` through `finish_color_trace`,
`lib.rs:740`), the native-alpha path (`native::trace_color`, `native.rs:1001`, the only
caller that passes the source alpha) and `trace_color_from_labels` (`lib.rs:994`). Skipped
when `opts.fast` is set without an iteration cap (Fast mode reaches the same function from
`fast/front.rs:154`) or when `INKVEC_BOPT=0` (`boundary_opt.rs:605-608`); `--mode balanced`
sets `ColorOptions::boundary_iters` to 8 and runs it capped at 8 iterations per part
(`boundary_opt.rs:612-626`, see [14-fast-mode.md](14-fast-mode.md)). The bilevel front end
(`trace_bilevel`) never calls it.

## What problem this solves

Every stage upstream of this one decides a boundary point on its own. `planar::build`
places it on the integer lattice; `refine_subpixel` slides it along its own normal until
the coverage read there is a half; `refine_junctions` intersects the edges that meet at a
node. Each of those is a one-dimensional argument about one point, and it cannot see two
facts: "A pixel's value is the area coverage of *every* region that touches it, so a
point's neighbours along the boundary change what that pixel should read; and a pixel says
nothing at all about motion *along* the boundary, so a point is free to slide unless
something holds it" (module doc, `boundary_opt.rs:6-9`).

So the boundary is solved as one problem: every point is an unknown in a single
optimisation, and the objective is the actual rendering error — the exact box-filtered
coverage the geometry would paint, compared against the image.

## Inputs and outputs

**Input:** a mutable `PlanarMap` (`edges`, `width`, `height`, `n_labels`), the image
`rgb: &[[f32; 3]]` (sRGB `0..1`, row-major), each face's `FillModel`, an optional
wall-clock budget `budget_ms`, and optionally the source alpha and each face's opacity
(passed only by the native-alpha path).

`budget_ms` is `ColorOptions::boundary_ms` (`lib.rs:178-181`): `None` by default, and then
the result depends only on the input. The CLI sets it in `color_options`
(`inkvec-cli/src/pipeline.rs:145-153`) to a quarter of `--time-budget`, at least 50 ms,
when `--time-budget` is above zero, in Quality only: balanced caps its solve by iterations
instead, so its output does not depend on the machine. The clock starts once the band is set up
(`boundary_opt.rs:673`) and is read before each iteration of each part (`lbfgs.rs:239`):
an iteration under way finishes, and once the budget is spent the parts not yet solved
take no step.

**Output:** the map is edited in place and `optimise_alpha` returns `Option<Report>`
(`boundary_opt.rs:549-563`):

```rust
pub struct Report {
    pub before: f64,   // energy before the solve
    pub after: f64,    // energy after (before the fold guard backs anything off)
    pub iters: usize,  // most iterations any independent part took
    pub moved: usize,  // edge points moved by more than 1e-6 px (a shared end once per edge)
    pub scale: f64,    // share of the solved displacement the fold guard kept
}
```

`None` means nothing changed. It is returned when the map has no edges or a zero side, or
the image is smaller than the map (`boundary_opt.rs:636-638`); when there are fewer than
three unknowns (`boundary_opt.rs:642-644`); when the band's tables would pass the memory
budget (below, `boundary_opt.rs:668-672`); when the starting residual of the pixels the
boundary cuts, or the starting kink sum, is zero (`lbfgs.rs:175-177`); and when the energy
did not fall or no part took a step (`lbfgs.rs:200-202`). The fold guard never discards the
solve: at worst it puts the boundaries in a new crossing back where they started
(*The fold guard*, below). The working positions live in a local vector until the whole
solve and guard are done.

## How it works

### The unknowns

`build_vars` (`boundary_opt.rs:179`) gives every point of every edge its own unknown,
except that the end points of open edges are keyed by their node id, so all the edges
meeting at a junction share one unknown and move it together. That is what keeps the map a
partition however far the points move. Unknowns are numbered in edge order, then point
order, which fixes every summation order downstream.

`band::pin_frame` (`band.rs:1186-1217`) then snaps points lying on the image frame
(within `1e-3` px of `x = −½`, `x = w − ½`, `y = −½` or `y = h − ½`, on an edge against the
outside) exactly onto it and lets them move only along it. Not from the literature: a
boundary condition, needed because the coverage accumulation starts from the outside at
`x = −½`.

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

Two details keep the evaluation cheap. Only the pixels holding pieces are rendered one by
one; between two of them the carry is constant, so a stretch of pixels holding no piece is
summed at once as a quadratic form in the carry vector, `κᵀ(ΣA)κ − 2κᵀ(Σb) + Σc`, from
per-run prefix sums of the fills fixed at the start (`stretch_energy`, `fill_prefix`;
`Problem::band_run`, `band.rs:598-695`). Runs are independent, so a band of at least
`PARALLEL_CELLS` = 16384 pixels is evaluated in parallel with a fixed partition and a fixed
order of summation, and the result does not depend on the thread count.

The tables those two passes read are filled once, by `fill_colours` and `fill_prefix`
(`band.rs:1092-1184`), after `band::build` (`band.rs:919-1028`) has found the runs and the
faces of each: for every run of `len` pixels and the `nf` faces that can appear in it, each
face's colour at each pixel (and its opacity where alpha is a channel), and the run's
prefix sums, `nf(nf + 1)/2 + nf + 1` of them per pixel. They grow
as `len · nf²` per run, and `nf` is bounded by nothing but the image (see *The band-table
budget* below).

The data term and the mixture are computed in f64: in f32 the energy is a staircase the
line search cannot descend.

### The band-table budget

`band::build` (`band.rs:919-1028`) counts the tables' bytes run by run as it finds the
runs, with `table_bytes` (`band.rs:907-917`):

```rust
pub(super) fn table_bytes(len: u64, nf: u64, alpha: bool) -> u64 {
    let colour = (24 + if alpha { 8 } else { 0 }) * nf * len;
    let kk = nf * (nf + 1) / 2 + nf + 1;
    colour.saturating_add((len + 1).saturating_mul(kk).saturating_mul(8))
}
```

that is `24·nf·len` bytes of colours (three f64 per face per pixel), `8·nf·len` more of
opacities when alpha is a channel, and `8·(len + 1)·kk` of prefix sums (`kk` per pixel and
one leading row of zeros), counted in u64 with saturating arithmetic. The budget is
`table_budget(w, h)` (`band.rs:858-905`):

```text
budget = max(TABLE_BUDGET_FLOOR, TABLE_BUDGET_PER_PIXEL · w · h) = max(256 MiB, 32 B · w · h)
```

which is 256 MiB up to about 2900 x 2900 px and 32 bytes a pixel beyond. When the running
total passes it (`band.rs:981-995`), `build` stops at that run, before any table is
allocated, and returns `None`; `setup` then returns false (`band.rs:1275-1290`) and
`optimise_alpha_capped` returns `None` (`boundary_opt.rs:668-672`). The solve is skipped and the
map keeps the boundary `refine_subpixel` and `refine_junctions` measured, as when the solve
finds nothing to gain. A band of exactly the budget is solved; one byte over is not. The
count covers the per-run tables, the part that grows with `nf`; the solve's per-pixel
arrays (one entry per image pixel) are not in it.

Why (`band.rs:864-880`): "On art whose boundaries are dense enough that the band covers
whole rows, a run is a whole row and `nf` is every face the row crosses". One-pixel
vertical stripes at 512 px made `nf = 513` and asked for a 265 GB prefix table, and the
process aborted. "What the solve buys on such an image is small (on the stripes it reports
no gain at all: every pixel is a mixture of two faces whatever the boundary does)".
Halftone screens, hatched or engraved logos and dense labyrinth textures build bands of the
same shape. The measured sizes are under *Measurements* below.

Not from the literature: "a resource bound on our own data structure. See also: the
narrow-band level set of D. Adalsteinsson, J. A. Sethian (1995), *A fast level set method
for propagating interfaces*, J. Comput. Phys. 118, <https://doi.org/10.1006/jcph.1995.1098>,
whose band is what grows here; the paper bounds the band's width, not the number of regions
meeting inside it" (`band.rs:896-900`).

Tests (`boundary_opt/band_tests.rs`): `the_table_budget_counts_exactly_what_is_allocated`
(`boundary_opt/band_tests.rs:90-114`) checks that `table_bytes`, summed over the runs,
equals the bytes `fill_colours` and `fill_prefix` allocate, with and without alpha;
`a_band_over_the_table_budget_is_not_set_up` (`boundary_opt/band_tests.rs:116-146`) that a
band of exactly the budget is set up and one byte over is refused with nothing set up;
`the_table_budget_has_a_floor_and_grows_with_the_image`
(`boundary_opt/band_tests.rs:148-156`) the floor, the per-pixel growth and saturation; and
`an_ordinary_large_map_is_far_inside_the_production_budget`
(`boundary_opt/band_tests.rs:158-180`) that a 600 x 400 map of a few dozen discs fits in a
sixteenth of the floor.

With `INKVEC_DIAG` set, `build` writes the band's size as a `bopt` line (`band runs=…
cells=… max_nf=… table_bytes=…`, `band.rs:1021-1026`) and, past the budget, a saturation
line for `band_table_bytes` marked `AT CAP` and `band over budget at row=… of … max_nf=…:
solve skipped` (`band.rs:983-993`).

### Which pixels count

Each band pixel's weight `w_p` is fixed at the start (`band::setup`, `band.rs:1275-1332`):

- **Zero where the seed is uncertain.** A run whose seed is not a single face with full
  coverage has weight zero (`band.rs:965-975`).
- **One everywhere else**, including the pixels where two boundaries meet (`band.rs:1301-1313`).

Pixels holding pieces of two or more boundaries between modelled faces at the start (a
junction, or both sides of a stroke too thin to have an interior) used to be left out of the
data term for the whole solve: their colour is a three-way mixture the fills are least
reliable at, and on a thin stroke the two sides compete for one pixel's evidence (the
sawtooth below). With the solver of the time that measured better (objective 0.3713 leaving
them out, 0.3908 with them in, 0.3908 again leaving out only the pixels round the junction
nodes). Under the converged solve it no longer does: leaving them out took the only evidence
a two-pixel stroke has away from both of its sides (`simple-icons/luanti`'s strokes are about
two pixels wide, so nearly every stroke pixel had weight zero, r2-fidelity research section
2.5 (c)). Re-measured 2026-10-03 on the 246-icon screen set against v0.2.5 (128 iterations,
local fold guard): dE00 −4.37 % keeping them against −4.35 % leaving them out at 512 px,
−8.40 % against −7.58 % at 128 px. The gate's own verdict is under *Measurements*.

With alpha (`optimise_alpha` given the source alpha and the faces' opacities), a band pixel
within reach of a boundary whose faces differ in opacity by at least `MIN_CONTRAST` = 2/255
but in colour by less (white paint on the clear ground, the bands of one fade), and of no
boundary whose faces differ in colour, also compares the coverage-weighted opacity with the
source alpha, as a fourth channel (`alpha_channels`, `band.rs:1224-1273`).

`setup` also records the starting residual split two ways: the pixels the boundary cuts
(`data0`, what the prior weights are scaled to) and the rest (`band_norm.1`, the residual
no boundary can change, which the stopping rule subtracts from the energy it measures
decreases against; `band.rs:1316-1329`).

### Priors: kink and anchor

`Problem::priors` (`boundary_opt.rs:428-507`) adds two terms. **Kink** is the *absolute*
value of the discrete second difference at each interior point of an open edge and at every
point of a closed one (an edge of fewer than three points has none), smoothed by `10⁻⁴`
inside the square root: a corner then costs in proportion to how sharply it turns, so one
sharp corner is cheaper than the many small kinks a squared term would spread it into.
**Anchor** is the squared distance from each point's start, four times heavier at a
junction (`JUNCTION_ANCHOR`); it removes the tangential freedom and holds points the image
cannot see where the measurement put them.

Both weights are relative to the data term (`lbfgs::descend`, `lbfgs.rs:178-179`):

```rust
prob.w_kink   = K_KINK   * data0 / kink0;   // kink starts at 5% of the data term
prob.w_anchor = K_ANCHOR * data0 / n as f64; // 1 px costs a tenth of a point's share
```

so they mean the same thing on a flat two-colour logo and on a crowded emoji.

### Independent parts

`band::components` (`band.rs:1334-1424`) splits the problem with a union-find: two
boundaries are in one part when they share an unknown (a junction) or when pieces of both
lie within reach of one band run. The energy is exactly the sum of the parts' energies
(each run belongs to one part), so each part is minimised on its own and stops when *it*
has converged, instead of every part paying for the slowest one. A median icon has five
parts; a page of text has hundreds. Parts with no band run (the frame's top, right and
bottom edges, which no pixel reads) are skipped. Block-separable minimisation, as a sparse solver's independent residual
blocks (Ceres Solver documentation, <https://github.com/ceres-solver/ceres-solver>,
`docs/source/nnls_solving.rst`).

### The solver: L-BFGS with a Moré–Thuente line search on the projected path

The problem: minimise the band energy over the boundary points, each held in the disc of
radius `MAX_TOTAL` round its start and, where it is pinned, on the image frame. The energy
is continuous but only piecewise smooth (its slope jumps where a piece meets a gridline).

`descend` (`lbfgs.rs:164-213`) first evaluates the whole problem's energy and gradient at the
start once, for the stopping tests' denominators (`Scales`, `lbfgs.rs:132-150`), then solves
the parts one after another. `solve` (`lbfgs.rs:223-298`) runs, per part:

1. **Stop test.** Stop when the projected gradient's largest point
   (`projected_gradient_norm`, `lbfgs.rs:492-513`: the gradient with the outward normal
   component removed at a point on the edge of its disc) is below `PG_TOL` = 10⁻⁶ of the
   whole problem's largest gradient at the start (`lbfgs.rs:245`); when the last step
   lowered the energy by less than `FUNC_TOL` = 10⁻⁷ of the whole problem's changeable
   energy at the start, its energy less the residual of the band pixels no boundary touches
   (`lbfgs.rs:284`); after `MAX_ITERS` = 64 iterations; when the line search finds no step of
   sufficient decrease; or when the caller's budget has run out. Never on the length of a
   step. Inspired by R. H. Byrd, P. Lu, J. Nocedal, C. Zhu (1995), *A limited memory
   algorithm for bound constrained optimization*, SIAM J. Sci. Comput. 16(5):1190–1208,
   <https://doi.org/10.1137/0916069> (L-BFGS-B), which stops on the projected gradient and
   the relative reduction of the objective; adapted to discs and pinned coordinates instead
   of boxes, and with both thresholds relative to the whole problem, whose scale differs by
   orders of magnitude between a two-colour logo and a crowded emoji. The whole problem
   rather than the part, because the parts are one problem split only for speed: measured
   against its own energy, a part with almost nothing to gain (a frame edge, a speck) keeps
   iterating for nothing, and a large part looks converged early (on 21 icons of the 512 px
   screen set, a part-relative 10⁻⁶ left 72 of 107 parts running at 128 iterations, the
   whole-problem 10⁻⁷ only 25, median 68 iterations, and read −4.35 % dE00 against −4.13 %
   on the full set). The projected-gradient test rarely fires on this energy, whose gradient
   does not vanish at a minimum on a kink: none of 13,285 steps on 41 icons of the 128 px
   screen set stopped on it (`lbfgs.rs:117-122`).
2. **Direction.** Limited-memory BFGS, the two-loop recursion over the last `MEMORY` = 3
   steps with initial scaling `γ = sᵀy / yᵀy` (method from D. C. Liu, J. Nocedal (1989),
   *On the limited memory BFGS method for large scale optimization*, Math. Programming 45,
   <https://doi.org/10.1007/BF01589116>; J. Nocedal, S. J. Wright (2006), *Numerical
   Optimization*, Algorithm 7.4, <https://doi.org/10.1007/978-0-387-40065-5>), projected onto
   the tangent cone of the active discs: the outward normal component of a point already on
   the edge of its disc is removed (`tangent_cone`, `lbfgs.rs:470-490`). A direction that is
   not downhill clears the memory and falls back to the projected steepest descent
   (`descent_direction`, `lbfgs.rs:416-438`). A pair is stored only when its curvature `yᵀs`
   exceeds `10⁻¹²·‖y‖·‖s‖` (`curvature_pair`, `lbfgs.rs:440-458`).
3. **Step length.** The Moré–Thuente line search (`linesearch.rs`; method from J. J. Moré,
   D. J. Thuente (1994), *Line search algorithms with guaranteed sufficient decrease*, ACM
   TOMS 20(3):286–307, <https://doi.org/10.1145/192115.192132>), from the unit quasi-Newton
   step (on the first iteration, a move of `MAX_STEP` for the furthest point), never beyond a
   move of `MAX_STEP` = 0.35 px, along the projected path `a ↦ P(x + a·d)` (`Part::search`,
   `lbfgs.rs:361-413`). It looks for a step satisfying the strong Wolfe conditions,
   `φ(a) ≤ φ(0) + μ·a·φ'(0)` and `|φ'(a)| ≤ η·|φ'(0)|` with `μ = FTOL` = 10⁻⁴ and
   `η = GTOL` = 0.9, by safeguarded cubic and quadratic interpolation (`step`,
   `next_trial`, `linesearch.rs:352-505`), extrapolating up to four times the last step while
   the slope is still steep; the slope `φ'(a)` is the gradient at the trial dotted with the
   path's exact derivative, including where the projection onto a disc is active
   (`path_slope`, `lbfgs.rs:515-542`). Up to `MAX_EVALS` = 10 trials; when the curvature
   condition cannot be met (the minimum sits on a kink, the setting of A. S. Lewis,
   M. L. Overton (2013), *Nonsmooth optimization via quasi-Newton methods*, Math. Program.
   141:135–163, <https://doi.org/10.1007/s10107-012-0514-2>) the best trial of sufficient
   decrease is taken. Measured: 96.3 % of 13,285 accepted steps took one trial, 99.7 % at
   most two, none more than four (`linesearch.rs:90-94`).

Nothing is linearised: every trial re-renders the exact coverage of the part's runs.

Why not the backtracking search it replaced: the curvature condition rules out a step at
which the slope is still as steep as at the start, which is exactly the step a halving
search accepts first; and stopping on the length of an accepted step read that short step
as convergence (`linesearch.rs:1-25`; *What the solver replaced*, below).

### The fold guard

A solved displacement can make the boundary self-intersect: two sides of a thin ribbon can
be pulled toward the same ink and pass through each other. Downstream that costs far more
than the boundary error it bought (the repair stage refits the offending rings round after
round, 2.5 s on one logo, and the emitter paints a face over its own interior).

A fold is a property of one pair of segments, so the guard backs off only the boundaries
that take part in one (`fold_guard_local`, `boundary_opt.rs:685-790`). Each edge `k` carries
a scale `s_k`, starting at 1; an unknown `v` sits at `p⁰_v + σ_v·(p_v − p⁰_v)` with
`σ_v = min s_k` over the edges it belongs to, so a junction follows the most cautious of its
edges and an edge at scale 0 is exactly where it started, ends included. While some pair of
segments crosses that did not cross at the start, the scale of every edge with a segment in
such a pair is halved, from ½ down to 1/16 and then to 0. Every round lowers at least one
scale, and a pair whose two edges are both at 0 is the start's own, so the loop ends with no
new crossing after at most six rounds per edge; the solve is never discarded. Crossings
already there at the start (left by `refine_subpixel` or `refine_junctions` for the repair
stage) may stay: refusing to improve a boundary because of one would give up most of the
gain. `Report::scale` is the share of the solved displacement kept,
`Σ_v σ_v·|p_v − p⁰_v| / Σ_v |p_v − p⁰_v|`.

The guard it replaced scaled the *whole* displacement back by halves (to 1/16, else it
discarded the solve) until the crossing count was back to the start's: one crossing anywhere
gave up the solve's gain on every boundary of the image. The r2-fidelity research measured
it scaling 46 of 362 icons back and discarding one solve outright (`looker`), 23 of the 64
icons v0.2.4 made worse being scaled back where the solve before had been kept in full, and
the local rule alone worth −2.9 % dE00 on the 128 px screen set and −2.3 % on `held_a`.
With the Wolfe-search solver (2026-10-03, screen set against v0.2.5, every band pixel kept,
128 iterations): −4.35 % with the local guard against −3.88 % with the global one at 512 px,
−7.58 % against −7.51 % at 128 px.

Inspired by J. Smith, S. Schaefer (2015), *Bijective parameterization with free
boundaries*, ACM TOG 34(4), <https://doi.org/10.1145/2766947>, and M. Li, Z. Ferguson,
T. Schneider, T. Langlois, D. Zorin, D. Panozzo, C. Jiang, D. M. Kaufman (2020),
*Incremental potential contact*, ACM TOG 39(4), <https://doi.org/10.1145/3386569.3392425>,
which never take a step that makes two boundary elements pass through each other, deciding
it per pair of elements. Adapted: they bound the step inside the line search and add a
barrier energy that keeps a colliding pair apart so the descent can go on; this energy has
no barrier (two sides of a thin ribbon are *meant* to approach), and a step bound without
one stops the whole part's descent at the first contact, so the per-pair decision is taken
once, after the solve (`boundary_opt.rs:722-733`).

The crossings (`folds::FoldCounter`) are found by a spatial join after J. Dittrich,
B. Seeger (2000), *Data redundancy and duplicate detection in spatial join processing*,
ICDE, <https://doi.org/10.1109/ICDE.2000.839452>: segments are entered in a coarse grid by
their range swept over every position the guard will ask about (every point anywhere on the
segment from its start to its solution, `folds.rs:94-120`), and a pair is reported only in
the cell holding the reference point of the two ranges' intersection. The candidate list is
built once; `new_crossings` (`folds.rs:161-175`) re-applies the exact per-position test of
the module docs at the current and the starting positions and returns the pairs that cross
now and did not then. `count`, the number of crossing pairs at one position, is kept as the
reference the tests compare the join with (`folds.rs:121-131`).

## Constants and thresholds

| name | value | where | controls | basis |
|---|---|---|---|---|
| `MAX_STEP` | 0.35 px | `boundary_opt.rs:112` | largest displacement of any point in one L-BFGS step | none |
| `MAX_TOTAL` | 1.0 px | `boundary_opt.rs:116` | leash round each point's start; also why the band never moves | "a point a pixel away from its own level set has stopped describing the same piece of the image" |
| `K_KINK` | 0.05 | `boundary_opt.rs:123` | kink weight, fraction of the starting data term | scaling rule derived, value not swept |
| `K_ANCHOR` | 0.10 | `boundary_opt.rs:126` | anchor weight, fraction of a point's share of the starting data term | as above |
| `JUNCTION_ANCHOR` | 4.0 | `boundary_opt.rs:128` | anchor multiplier at a shared end point | not derived (its doc comment gives the reason, `planar::refine_junctions` has already placed the point, but not why fourfold) |
| `MIN_CONTRAST` | 2/255 | `boundary_opt.rs:130` | colour or opacity difference that counts as a boundary when choosing the pixels where alpha is a fourth channel | none |
| `EPS` (kink) | 1e-4 | `boundary_opt.rs:442` | floor inside the kink term's square root | for differentiability |
| `GRID_LIMIT` | 1e9 px | `boundary_opt.rs:224` | largest coordinate whose gridlines `crossings` walks; a segment past it contributes no crossings | "far inside the range where `m += 1.0` is exact, and far outside any image" |
| `GRID_MAX_SPAN` | 2²⁰ gridlines | `boundary_opt.rs:228` | most gridlines `crossings` walks along one axis of one segment | a segment inside the image crosses at most its width or height |
| `REACH` | 1 px | `band.rs:71` | band width round the pixels the start crosses | follows from `MAX_TOTAL` |
| `PARALLEL_CELLS` | 16384 band pixels | `band.rs:151` | below it the runs are evaluated on one thread (the result is the same either way) | none |
| `TABLE_BUDGET_FLOOR` | 256 MiB | `band.rs:852` | floor of the band-table budget `max(TABLE_BUDGET_FLOOR, TABLE_BUDGET_PER_PIXEL · w · h)`; past the budget the solve is skipped | measured: 11 times the gate's largest table (22.2 MB), 2.4 times the largest of 772 stress images (106 MB); the pathological class starts at 116 MB |
| `TABLE_BUDGET_PER_PIXEL` | 32 bytes a pixel | `band.rs:856` | growth of the budget above its floor (binds above about 2900 x 2900 px) | lets an uncapped 8192 px trace keep twice the masthead's 14 bytes a pixel |
| frame snap | 1e-3 px | `band.rs:1196` | how close to a frame line a point of a frame edge is snapped onto it and pinned | none |
| `MEMORY` | 3 | `lbfgs.rs:102` | L-BFGS pairs kept | measured: 3 did as well as 7, 15 or 30 with the Armijo search; with the Wolfe search 7 read −4.26 % against −4.37 % for 3 at 512 px |
| `MAX_ITERS` | 64 | `lbfgs.rs:110` | iterations per independent part | measured: −4.15 % dE00 at 512 px and −6.35 % at 128 px against −4.37 % and −8.40 % at 128 iterations; whole trace 1.13× v0.2.5 at 512 px against 1.28–1.30× |
| `FUNC_TOL` | 1e-7 | `lbfgs.rs:116` | stop a part on a relative decrease, of the whole problem's changeable energy at the start | measured: against a part-relative 1e-6, a quarter as many parts run to the cap, −4.35 % against −4.13 % at 512 px |
| `PG_TOL` | 1e-6 | `lbfgs.rs:122` | stop a part on its projected gradient, relative to the whole problem's largest gradient at the start | none (rarely fires: 0 of 13,285 steps) |
| `FTOL` | 1e-4 | `linesearch.rs:82` | sufficient-decrease constant `μ` | textbook (Nocedal & Wright's `c1`), the value of the Armijo search it replaced |
| `GTOL` | 0.9 | `linesearch.rs:86` | curvature constant `η` | textbook value for quasi-Newton directions (Nocedal & Wright §3.1; Moré & Thuente §1) |
| `XTOL` | 0.1 | `linesearch.rs:89` | relative width of the interval of uncertainty below which a search stops | MINPACK-2's |
| `MAX_EVALS` | 10 | `linesearch.rs:94` | trials per line search | measured: 96.3 % of steps take one trial, none more than four |
| `XTRAPL`, `XTRAPU` | 1.1, 4.0 | `linesearch.rs:98-99` | extrapolation window before a bracket exists | MINPACK-2's |
| `SHRINK` | 0.66 | `linesearch.rs:102` | required shrink of a bracket over two trials, else bisect | Moré & Thuente §4 |
| fold-guard halving | ½ per round to 1/16, then 0 | `boundary_opt.rs:753-757` | how a boundary in a new crossing is backed off | none |
| `COARSE` | 4 px | `folds.rs:36` | side of the fold guard's coarse join grid | about the length of a swept segment's range |
| `MAX_COARSE_CELLS` | 64 | `folds.rs:41` | a swept range over more coarse cells is paired with every segment directly instead of entering the grid | the map never has one; keeps a degenerate segment from filling the grid |
| `MAX_RANGE_AREA` | cell range `(x1−x0)(y1−y0) ≤ 64` | `folds.rs:45` | segments counted by the fold guard (larger ones are never counted) | kept exactly from the grid it replaced |
| boundary share of `--time-budget` | a quarter, at least 50 ms | `inkvec-cli/src/pipeline.rs:152` | `budget_ms` when `--time-budget` is above zero | none |

## Measurements

### The converged solve (2026-10-03, against v0.2.5)

The regression gate (`bench/ci_gate.py`, the 246-icon screen set judged at 1024 px against
the artist's file, family-macro dE00, paired bootstrap against v0.2.5's numbers on the same
machine), one change at a time; each row includes the rows above it:

| change | Quality 128ss dE00 | Quality 512ss dE00 | Quality 512ssop dE00 | params ratio 128 / 512 / 512op | verdict |
|---|---|---|---|---|---|
| Wolfe search, whole-problem stopping rules, 64 iterations | −3.57 % | −3.59 % | −2.30 % | −0.33 / −1.60 / −1.34 % | better on every dE00, nothing worse |
| + local fold guard | −4.61 % | −4.19 % | −3.20 % | −0.69 / −1.55 / −1.26 % | better on every dE00, nothing worse |
| + band pixels two boundaries share kept | −6.35 % | −4.15 % | −2.87 % | −0.37 / −1.22 / −0.96 % | better on every dE00, nothing worse |
| + the intake's border pad ([01-intake.md](01-intake.md)) | −12.17 % | −6.22 % | −2.87 % | −2.65 / −2.92 / −0.96 % | better on every dE00, nothing worse |

On the held-out `held_a` set (156 icons, 128 px, the same scorer), family-macro dE00 goes
0.1310 → 0.1233 → 0.1195 → 0.1168 → 0.1105 down the four rows (−15.6 % in all), its worst
tenth 0.420 → 0.369, parameters against the artist 1.471 → 1.428.

Fast mode is untouched by all four (identical on its three conditions). Against the row
above it, keeping the shared pixels reads −1.83 % at 128 px, +0.04 % at 512 px and +0.34 %
on 512 px opaque (within the noise the set resolves). By family at 512 px with all four,
lucide −7.8 %, material-icons −4.9 %, noto-emoji −4.2 %, openmoji −2.0 %, simple-icons
−18.1 %, synthetic +0.7 %, twemoji −12.7 %: material-icons, which the v0.2.4 solve had made
14 % worse at 512 px (r2-eval), now gains. The largest single-icon losses at 512 px are
`material-icons/laptop` 0.064 → 0.079 and `simple-icons/playerfm` 0.035 → 0.050.

Whole-trace time at 512 px (screen icons, two interleaved passes, default threads, under
load): 1.13× v0.2.5 for the solve and 1.17–1.19× with the border pad in the measurement that
chose the iteration cap (30 icons, 512 px transparent); the finished branch read 1.22–1.24×
on 512 px transparent icons (30) and 1.21–1.23× on 512 px opaque ones (60), with the machine
more heavily loaded (2026-10-04).

### The solve it replaced

Against the per-pixel term with Fletcher–Reeves conjugate gradients it replaced (the
behaviour on `4f1fb88`):

| set | objective | mean dE00 | worst tenth dE00 | DISTS | params vs artist | better / worse |
|---|---|---|---|---|---|---|
| screen (246) | 0.3873 → **0.3585** | 0.1563 → 0.1427 | 0.5048 → 0.4669 | 0.0259 → 0.0242 | 1.488 → 1.503 | 155 / 82 |
| held_a (156) | 0.3958 → **0.3366** | 0.1480 → 0.1366 | 0.4626 → 0.4183 | 0.0243 → 0.0218 | 1.477 → 1.472 | 104 / 46 |

Stage time on the four standard inputs (ms, new against old, from the message of commit
`7948b38`): flat logo 26.5 / 25.0, Noto gradient emoji 70.2 / 58.1, 2048 px Twemoji globe
193.6 / 185.1, masthead 319.2 / 332.0; about a fifth less on the screen set's icons.
Whole-trace times are equal within noise. Fast mode is untouched (246/246 byte-identical).

### Band-table sizes

Largest band tables per image, measured 2026-10-02 with `INKVEC_DIAG` (the `table_bytes`
of the largest solve per image; `band.rs:884-894`), against the 256 MiB floor:

| set | largest tables |
|---|---|
| screen set, 246 icons at 128 px | 2.8 MB |
| the same icons at 512 px | 5.7 MB |
| `held_a` | 2.0 MB |
| 51-image 512 px set | 5.9 MB |
| seven opaque 2048 px inputs | 22.2 MB (the masthead, 14 bytes a pixel) |
| three transparent 2048 px inputs | 16.3 MB |
| 772 non-pathological images of the r2-inputs stress set | 106 MB (a 4x bicubic upscale of a dense wordmark at 512 x 313 px) |

The pathological class (measured 2026-10-02 by the r2-inputs research, `bandstats.py`;
`band.rs:866-875`): one-pixel vertical stripes at 512 px, `nf = 513`, a 265 GB prefix
table asked for (the process aborted on a 34.7 GB allocation after 19.7 GB of working set);
a 512 px pixel checkerboard 398 MB (`nf = 259`); a baked grey-and-white "transparency"
checkerboard behind an icon 116–711 MB; a 3.5x nearest-neighbour upscale of a wordmark
497 MB.

Effect of the budget, v0.2.4 against the tip of the branch that added it (impl2/robust
report, sections 3 and 4, 2026-10-03; peak memory of the process):

- `pixel_stripes_512` in Quality: crashed before; now traces with the band solve skipped,
  89 MB peak.
- `pixel_checker_512` in Quality: 557 MB peak before, 35 MB after.
- the `bg_checker` stress group (28 icons and logos on a baked checkerboard): largest peak
  863 MB before, 444 MB after; 21 of 28 traces identical, 7 skip the band solve, and the
  group's mean dE00 goes from 0.127 to 0.131.
- the branch's identity check: 1,656 of 1,656 SVGs byte-identical (the screen, `held_a`,
  512 px, 2048 px and stress sets, in both modes), so the budget never binds there.

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
centreline and a width. Its pixels used to be left out of the data term; under the converged
solve they are kept, and the kink prior and the fold guard hold its sides apart (*Which
pixels count*, above).

### Dense boundaries: the band tables

Before the budget, the band's tables were allocated whatever their size. On art whose
band covers whole rows (one-pixel stripes, pixel checkerboards, a baked "transparency"
checkerboard behind an icon, a nearest-neighbour upscale), a run is a whole row and `nf`
is every face the row crosses, so the prefix table grows as `len · nf²`: the 512 px stripes
asked for 265 GB and the Quality trace crashed. Since commit `e246f1e` the tables are
counted before anything is allocated and the solve is skipped past the budget (*The
band-table budget*, above). The skip is all or nothing: the whole map keeps its measured
boundary, including the ordinary art beside the dense part (7 of the 28 `bg_checker`
icons, mean dE00 0.127 to 0.131, Measurements above).

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
- **Iteration cap, with the Armijo search:** 24 reads 0.3666 on the screen set, 32 reads
  0.3585, 48 reads 0.3558; 48 made the stage a fifth to nine tenths slower on the standard
  inputs, so 32 shipped in v0.2.4 and v0.2.5 (the 24 figure was recorded on this page with
  commit `7948b38`).
- **Junction pixels in the data term:** measured worse twice, first as the wedge
  partition of the old per-pixel term (objective 0.4328 → 0.4562 on the screen set, with
  noto-emoji 0.389 → 0.418), then in the band (0.3713 → 0.3908, above); measured better a
  third time, under the converged solve, and kept since (above).

The earlier finding that the Fletcher–Reeves solve was unconverged at 24 iterations (980
icons, objective 0.4519 → 0.4428 at 48) happened again after the L-BFGS rewrite. The v0.2.4
and v0.2.5 solve halved the step from the unit step until the Armijo condition held and
stopped when an accepted step moved no point more than 0.005 px, or after 32 iterations, or
on a relative decrease of 10⁻⁴ of the part's own changeable energy. The r2-fidelity
research (2026-10-02, section 2.5) found both rules wrong for this energy: a backtracked step
is short because the search halved it, not because the minimum is near, so the step-length
test fired while the energy could still fall a long way (lucide `equal` stopped after two
iterations with both bars 0.05 px thin, dE00 0.067 where the converged solve reaches 0.002);
elsewhere the cap was hit while points still moved 0.03–0.13 px a step (`luanti`, `no_food`,
`navigation-2-off`). Run to convergence with the old line search (192 iterations and tighter
tolerances) the stage was worth −7.6 % dE00 on the 128 px screen set and −9.6 % on `held_a`,
and 384 iterations added nothing. The Wolfe search and the whole-problem stopping rules are
the fix. Measured at 128 px, 192 iterations: −7.55 % with the Wolfe search against −6.94 %
with the Armijo search at the same stopping rules, for about 7 % more trace time.

## Environment overrides

The engine reads its environment through `inkvec_core::env`; the full list, with what was
removed and why, is [`docs/internal/env-vars.md`](../internal/env-vars.md).

| variable | default | effect |
|---|---|---|
| `INKVEC_BOPT` | on | read in `boundary_opt.rs:606` (`env::switch`): `0` disables the whole stage |
| `INKVEC_BOPTDBG` | off | read in `boundary_opt.rs:645` (`env::flag`): per-iteration `eprintln!` of step, line-search trials, energy, relative decrease, largest move and projected gradient (`lbfgs.rs:273-275`), a line when a part stops on its projected gradient (`lbfgs.rs:247-250`), plus the fold guard's rounds, the edges it backed off and the share of the displacement kept (`boundary_opt.rs:781-786`) |
| `INKVEC_DIAG` | off | the crate's diagnostics (`diag.rs`); this stage writes the band's size (`bopt` `band runs=… cells=… max_nf=… table_bytes=…`) and, past the table budget, a `band_table_bytes` line marked `AT CAP` and `band over budget … solve skipped` (`band.rs:983-993`, `band.rs:1021-1026`) |

The former `INKVEC_BOPT_ITERS`, `INKVEC_BOPT_MS`, `INKVEC_BOPT_KINK`,
`INKVEC_BOPT_ANCHOR`, `INKVEC_BOPT_JUNC`, `INKVEC_BOPT_CHUNKS`, `INKVEC_BOPT_CELLS` and
`INKVEC_JUNCDBG` are gone: the stage has one behaviour.

## Open questions

- **The iteration cap is a time decision.** At 64 iterations a quarter of the parts of a
  512 px icon still stop on the cap rather than on a tolerance; 128 iterations read a fifth
  of a percent better at 512 px and two points better at 128 px, for a trace about 15 %
  slower. A cheaper iteration (each one renders the part's band once per trial) would let
  the cap rise within the same time.
- **The fitter after a converged solve.** With 128 iterations, `simple-icons/debridlink`
  traced on the border pad's canvas came back with a closed ring whose first segment skips
  the corner its seam sits on (a sliver 5 px wide, dE00 0.033 → 0.216 at 512 px); at the
  shipped 64 iterations it does not occur. The ring fit (stage 11) should not produce that
  spike from any boundary; it was not chased further here.
- **The projected-gradient test seldom stops anything** on this piecewise-smooth energy
  (0 of 13,285 steps): the relative decrease and the cap do the stopping. A stationarity
  measure made for nonsmooth functions would give the stop a meaning at a kink; not looked
  into.

- **`K_KINK`, `K_ANCHOR`, `JUNCTION_ANCHOR`, `MAX_STEP`, `MIN_CONTRAST`** have a reason to
  exist but no swept value. They were tuned for the per-pixel solver; the band energy's
  scale (every band pixel, not only the cut ones) is different, and a joint sweep may pay.
- **A two-pixel stroke is still two boundaries.** Its shared pixels are back in the data
  term, and the kink prior and the fold guard keep its sides apart; LOG-44 (a thin face as a
  centreline and a width) remains the model-order fix the r2-fidelity research ranks next
  (its P2).
- **Twenty gradients cost more.** The one standard input where the stage is clearly slower
  than before is a Noto emoji with twenty gradient fills (+21%, 58 ms to 70 ms); where that
  time goes has not been profiled.
- **No per-point confidence.** The anchor is the same for every non-junction point; it
  does not consume `refine_subpixel`'s per-point sigma. One use of it was tried against the
  sawtooth and failed; whether another use would help is untested.
- **The table budget skips the whole solve.** Past the budget no part is solved, although
  the parts are independent and most of them may be ordinary art (an icon on a baked
  checkerboard). Solving the parts whose runs fit is untested; splitting a long run instead
  is not exact, since a run's coverage depends on everything left of it (impl2/robust
  report, section 6).
