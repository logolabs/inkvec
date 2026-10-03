# Stage 08 — Boundary solve

> Moves every boundary point of the planar map at once, so that the geometry's *exact
> rendered coverage* matches the image, instead of refining each point along its own
> one-dimensional normal.

**Source:** `crates/inkvec-trace/src/boundary_opt.rs` (unknowns, priors, energy, fold
guard), `boundary_opt/band.rs` (the data term, and the memory budget of its tables),
`boundary_opt/lbfgs.rs` (the solver), `boundary_opt/folds.rs` (self-crossing count)
**Entry point:** `optimise_alpha()` (`boundary_opt.rs:579-634`); `optimise()`
(`boundary_opt.rs:561-568`) is the same without alpha
**Pipeline position:** after `refine_junc` (`planar::refine_junctions`, `lib.rs:1129`),
before `decode` (stage mark `"boundary_opt"`, `lib.rs:1142`). Called once, in
`finish_color_trace_alpha` (`lib.rs:1137-1141`), the geometry tail every colour entry point
shares: the opaque Quality path (`trace_color_full_with_alpha` through `finish_color_trace`,
`lib.rs:712`), the native-alpha path (`native::trace_color`, `native.rs:986`, the only
caller that passes the source alpha) and `trace_color_from_labels` (`lib.rs:966`). Skipped
when `opts.fast` is set (Fast mode reaches the same function from `fast/front.rs:154`) or
when `INKVEC_BOPT=0`. The bilevel front end (`trace_bilevel`) never calls it.

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

`budget_ms` is `ColorOptions::boundary_ms` (`lib.rs:174-177`): `None` by default, and then
the result depends only on the input. The CLI sets it in `color_options`
(`inkvec-cli/src/pipeline.rs:138-149`) to a quarter of `--time-budget`, at least 50 ms,
when `--time-budget` is above zero. The clock starts once the band is set up
(`boundary_opt.rs:624`) and is read before each iteration of each part (`lbfgs.rs:158`):
an iteration under way finishes, and once the budget is spent the parts not yet solved
take no step.

**Output:** the map is edited in place and `optimise_alpha` returns `Option<Report>`
(`boundary_opt.rs:547-558`):

```rust
pub struct Report {
    pub before: f64,   // energy before the solve
    pub after: f64,    // energy after
    pub iters: usize,  // most iterations any independent part took
    pub moved: usize,  // edge points moved by more than 1e-6 px (a shared end once per edge)
    pub scale: f64,    // fraction of the solved displacement kept by the fold guard
}
```

`None` means nothing changed. It is returned when the map has no edges or a zero side, or
the image is smaller than the map (`boundary_opt.rs:587-589`); when there are fewer than
three unknowns (`boundary_opt.rs:593-595`); when the band's tables would pass the memory
budget (below, `boundary_opt.rs:619-623`); when the starting residual of the pixels the
boundary cuts, or the starting kink sum, is zero (`lbfgs.rs:85-87`); when the energy did
not fall or no part took a step (`lbfgs.rs:102-104`); and when the fold guard finds no
scale, down to a sixteenth, that adds no crossing (`boundary_opt.rs:675-677`). The working
positions live in a local vector until the whole solve and guard have succeeded.

## How it works

### The unknowns

`build_vars` (`boundary_opt.rs:177`) gives every point of every edge its own unknown,
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
allocated, and returns `None`; `setup` then returns false (`band.rs:1305-1314`) and
`optimise_alpha` returns `None` (`boundary_opt.rs:619-623`). The solve is skipped and the
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

Each band pixel's weight `w_p` is fixed at the start (`band::setup`, `band.rs:1305-1350`):

- **Zero where two boundaries meet.** A pixel holding pieces of two or more boundaries
  between modelled faces at the start (a junction, or both sides of a stroke too thin to
  have an interior) is left out of the data term for the whole solve (`exclude_junctions`,
  `band.rs:1224-1252`). Its colour is a three-way mixture the fills are least reliable at,
  and on a thin stroke the two sides compete for one pixel's evidence (the sawtooth below).
  Measured on the screen set, with the solver of the time: objective 0.3713 leaving them
  out, 0.3908 with them in, and 0.3908 again leaving out only the pixels round the junction
  nodes.
- **Zero where the seed is uncertain.** A run whose seed is not a single face with full
  coverage has weight zero (`band.rs:962-975`).
- **One elsewhere.**

With alpha (`optimise_alpha` given the source alpha and the faces' opacities), a band pixel
within reach of a boundary whose faces differ in opacity by at least `MIN_CONTRAST` = 2/255
but in colour by less (white paint on the clear ground, the bands of one fade), and of no
boundary whose faces differ in colour, also compares the coverage-weighted opacity with the
source alpha, as a fourth channel (`alpha_channels`, `band.rs:1254-1303`).

`setup` also records the starting residual split two ways: the pixels the boundary cuts
(`data0`, what the prior weights are scaled to) and the rest (a constant per run, used by
the stopping rule).

### Priors: kink and anchor

`Problem::priors` (`boundary_opt.rs:426-505`) adds two terms. **Kink** is the *absolute*
value of the discrete second difference at each interior point of an open edge and at every
point of a closed one (an edge of fewer than three points has none), smoothed by `10⁻⁴`
inside the square root: a corner then costs in proportion to how sharply it turns, so one
sharp corner is cheaper than the many small kinks a squared term would spread it into.
**Anchor** is the squared distance from each point's start, four times heavier at a
junction (`JUNCTION_ANCHOR`); it removes the tangential freedom and holds points the image
cannot see where the measurement put them.

Both weights are relative to the data term (`lbfgs::descend`, `lbfgs.rs:88-89`):

```rust
prob.w_kink   = K_KINK   * data0 / kink0;   // kink starts at 5% of the data term
prob.w_anchor = K_ANCHOR * data0 / n as f64; // 1 px costs a tenth of a point's share
```

so they mean the same thing on a flat two-colour logo and on a crowded emoji.

### Independent parts

`band::components` (`band.rs:1352-1444`) splits the problem with a union-find: two
boundaries are in one part when they share an unknown (a junction) or when pieces of both
lie within reach of one band run. The energy is exactly the sum of the parts' energies
(each run belongs to one part), so each part is minimised on its own and stops when *it*
has converged, instead of every part paying for the slowest one. A median icon has five
parts; a page of text has hundreds. Parts with no band run (the frame's top, right and
bottom edges, which no pixel reads) are skipped. Block-separable minimisation, as a sparse solver's independent residual
blocks (Ceres Solver documentation, <https://github.com/ceres-solver/ceres-solver>,
`docs/source/nnls_solving.rst`).

### The solver: L-BFGS with a projected Armijo line search

`lbfgs::solve` (`lbfgs.rs:117-228`) runs, per part:

1. **Direction:** limited-memory BFGS, the two-loop recursion over the last `MEMORY` = 3
   steps with initial scaling `γ = sᵀy / yᵀy` (D. C. Liu, J. Nocedal (1989), *On the
   limited memory BFGS method for large scale optimization*, Math. Programming 45,
   <https://doi.org/10.1007/BF01589116>; J. Nocedal, S. J. Wright (2006), *Numerical
   Optimization*, Algorithm 7.4, <https://doi.org/10.1007/978-0-387-40065-5>). A pair is
   stored only when its curvature `yᵀs` exceeds `10⁻¹²·‖y‖·‖s‖` (`lbfgs.rs:216-222`); a
   direction that is not downhill clears the memory and falls back to steepest descent.
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
   caller's budget has run out at the start of an iteration.

Nothing is linearised: every trial re-renders the exact coverage of the part's runs.

### The fold guard

A solved displacement can make the boundary self-intersect: two sides of a thin ribbon can
be pulled toward the same ink and pass through each other. Downstream that costs far more
than the boundary error it bought (the repair stage refits the offending rings round after
round, 2.5 s on one logo, and the emitter paints a face over its own interior).
`fold_guard` (`boundary_opt.rs:636-679`) counts crossing segment pairs at the start and at
the solution, and while the solution adds crossings scales the whole displacement back
(`p⁰ + s(p − p⁰)`, halving `s` while `s > 0.1`, so the scales tried are 1, ½, ¼, ⅛ and
1/16; if 1/16 still adds a crossing the stage gives up). The count is compared with the
start's, not with zero: an earlier stage may already have left a fold for the repair
stage, and refusing to improve a boundary because of it would give up most of the gain.

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
| `JUNCTION_ANCHOR` | 4.0 | `boundary_opt.rs:126` | anchor multiplier at a shared end point | not derived (its doc comment gives the reason, `planar::refine_junctions` has already placed the point, but not why fourfold) |
| `MIN_CONTRAST` | 2/255 | `boundary_opt.rs:128` | colour or opacity difference that counts as a boundary when choosing the pixels where alpha is a fourth channel | none |
| `EPS` (kink) | 1e-4 | `boundary_opt.rs:440` | floor inside the kink term's square root | for differentiability |
| `GRID_LIMIT` | 1e9 px | `boundary_opt.rs:222` | largest coordinate whose gridlines `crossings` walks; a segment past it contributes no crossings | "far inside the range where `m += 1.0` is exact, and far outside any image" |
| `GRID_MAX_SPAN` | 2²⁰ gridlines | `boundary_opt.rs:226` | most gridlines `crossings` walks along one axis of one segment | a segment inside the image crosses at most its width or height |
| `REACH` | 1 px | `band.rs:71` | band width round the pixels the start crosses | follows from `MAX_TOTAL` |
| `PARALLEL_CELLS` | 16384 band pixels | `band.rs:151` | below it the runs are evaluated on one thread (the result is the same either way) | none |
| `TABLE_BUDGET_FLOOR` | 256 MiB | `band.rs:852` | floor of the band-table budget `max(TABLE_BUDGET_FLOOR, TABLE_BUDGET_PER_PIXEL · w · h)`; past the budget the solve is skipped | measured: 11 times the gate's largest table (22.2 MB), 2.4 times the largest of 772 stress images (106 MB); the pathological class starts at 116 MB |
| `TABLE_BUDGET_PER_PIXEL` | 32 bytes a pixel | `band.rs:856` | growth of the budget above its floor (binds above about 2900 x 2900 px) | lets an uncapped 8192 px trace keep twice the masthead's 14 bytes a pixel |
| frame snap | 1e-3 px | `band.rs:1196` | how close to a frame line a point of a frame edge is snapped onto it and pinned | none |
| `MEMORY` | 3 | `lbfgs.rs:41` | L-BFGS pairs kept | measured: 3 did as well as 7, 15 or 30 |
| `C1` | 1e-4 | `lbfgs.rs:43` | Armijo constant | textbook (Nocedal & Wright) |
| `MAX_TRIALS` | 8 | `lbfgs.rs:45` | halvings per line search | none |
| `MAX_ITERS` | 32 | `lbfgs.rs:49` | iterations per independent part | measured (below) |
| `PARAM_TOL` | 0.005 px | `lbfgs.rs:52` | stop when no point moves more | half the SVG's 0.01 px resolution |
| `FUNC_TOL` | 1e-4 | `lbfgs.rs:55` | stop on relative decrease of the changeable energy | none |
| fold-guard floor | `s > 0.1` | `boundary_opt.rs:662` | how far the guard halves the displacement before giving up (last scale tried 1/16) | none |
| `COARSE` | 4 px | `folds.rs:33` | side of the fold guard's coarse join grid | about the length of a swept segment's range |
| `MAX_COARSE_CELLS` | 64 | `folds.rs:38` | a swept range over more coarse cells is paired with every segment directly instead of entering the grid | the map never has one; keeps a degenerate segment from filling the grid |
| `MAX_RANGE_AREA` | cell range `(x1−x0)(y1−y0) ≤ 64` | `folds.rs:42` | segments counted by the fold guard (larger ones are never counted) | kept exactly from the grid it replaced |
| boundary share of `--time-budget` | a quarter, at least 50 ms | `inkvec-cli/src/pipeline.rs:145` | `budget_ms` when `--time-budget` is above zero | none |

## Measurements

Against the per-pixel term with Fletcher–Reeves conjugate gradients it replaced (the
behaviour on `4f1fb88`; module doc, `boundary_opt.rs:88-95`):

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
centreline and a width. Until then its pixels are left out of the data term (above).

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
- **Iteration cap:** 24 reads 0.3666 on the screen set, 32 reads 0.3585, 48 reads 0.3558;
  48 made the stage a fifth to nine tenths slower on the standard inputs, so 32 ships (the
  32 and 48 figures are in `lbfgs.rs:46-48`; the 24 figure was recorded on this page with
  commit `7948b38`).
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
| `INKVEC_BOPT` | on | read in `lib.rs:1137` (`env::switch`): `0` disables the whole stage |
| `INKVEC_BOPTDBG` | off | read in `boundary_opt.rs:596` (`env::flag`): per-iteration `eprintln!` of step, energy, relative decrease and largest move (`lbfgs.rs:196-198`), plus the fold-guard summary (`boundary_opt.rs:672-674`) |
| `INKVEC_DIAG` | off | the crate's diagnostics (`diag.rs`); this stage writes the band's size (`bopt` `band runs=… cells=… max_nf=… table_bytes=…`) and, past the table budget, a `band_table_bytes` line marked `AT CAP` and `band over budget … solve skipped` (`band.rs:983-993`, `band.rs:1021-1026`) |

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
- **The table budget skips the whole solve.** Past the budget no part is solved, although
  the parts are independent and most of them may be ordinary art (an icon on a baked
  checkerboard). Solving the parts whose runs fit is untested; splitting a long run instead
  is not exact, since a run's coverage depends on everything left of it (impl2/robust
  report, section 6).
