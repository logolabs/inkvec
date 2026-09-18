# Stage 12 — Repair

> Finds every ring a face's fitted boundary makes that crosses itself, and refits only the
> guilty edges — under a shrinking span cap — until none do.

**Source:** `crates/inkvec-cli/src/rings.rs` (detection and orchestration),
`crates/inkvec-fit/src/simple.rs` (the crossing test and its per-boundary sibling),
`crates/inkvec-fit/src/multimodel.rs` (`optimal_multimodel_capped`, the constrained DP)
**Entry point:** `repair_ring_crossings()` (`crates/inkvec-cli/src/rings.rs:58-63`)
**Pipeline position:** after curve fitting (stage 11), before fill assignment (stage mark
`"fills"`). Stage mark `"repair"` (`crates/inkvec-cli/src/lib.rs:992`).

## What problem this solves

Stage 11 fits each edge of the planar map independently, and the dynamic program's cost is a
sum over that edge's own points — nothing in `0.5*chi2 + lambda*params` knows or cares what
any other edge of the ring is doing. Two edges that pass close together, each correctly
fitted to within its own measured tolerance, can smoothly bulge across one another. The fit
is not wrong: both curves still pass through their measured points and the rendered pixels
barely move.

`crates/inkvec-fit/src/simple.rs:1-24` (module doc), states the mechanism and the measured
scale of the defect:

> "Keep a fitted boundary from crossing itself... Measured over 180 real emoji, 53 of them
> (29%) emitted at least one self-intersecting ring, against VTracer's 10 (5.6%).
> Classifying those crossings over 60 images and 2126 rings settles what kind they are:
>
> ```
>   single-cubic loops                       0
>   crossings between different segments    40
> ```
>
> **Not one** was a cubic looping over itself... Every crossing is between two *different*
> segments of one ring, which no per-candidate test can see, because it is a property of the
> assembled path rather than of any curve in it. The mechanism is a thin neck. Where two
> stretches of contour run close together, each is fitted independently to within its own
> tolerance, and the two smooth curves bulge across each other. The objective cannot
> object: both curves pass through their measured points and the rendered pixels barely
> change."

The crossing is almost never contained inside one edge's own fit — it is between two
*different edges* of the same face ring. `ring_as_path`'s doc (`rings.rs:458-464`):

> "Needed because a self-crossing on a real boundary is almost never inside one edge's fit.
> Repairing per edge — refitting any edge whose own curve crossed itself — changed 5 images
> out of 180 and left the count at 76. The crossings are between *different* edges of the
> same face, so the ring is the smallest unit at which the defect is even visible."

This is why repair operates on assembled *rings*, not on individual edges: it is the smallest
unit the defect can be seen in at all.

**Why a crossing is not cosmetic.** The code's own framing is narrower than "the fill
inverts" — the strongest statement in the source is that a self-crossing ring is "invalid,
resolved arbitrarily by whichever fill rule applies, and unpleasant to edit"
(`simple.rs:24`, restated in `tests/self_intersection.rs:6-7` and
`crates/inkvec-cli/src/lib.rs:976-986`). The geometric mechanism behind that arbitrariness —
this follows from the fill rules the emitter actually uses, though it is not spelled out
verbatim as a code comment: every emitted path is drawn `fill-rule="evenodd"` (Inkvec emits no
`nonzero` paths; see `emit.rs:884,955,1015,1042` and the ring nesting discussion at
`rings.rs:3,355`). Under even-odd, fill state alternates each time a ray crosses the boundary,
so the lobe cut off by a self-crossing flips crossing parity relative to the rest of the
ring's interior and renders with the *opposite* fill state from what was intended — a letter's
counter, or any other hole, can flip from empty to solid or the reverse. This specific
inversion is not spelled out anywhere in the repo; only the weaker "resolved arbitrarily"
claim is. The related complication is that **not every crossing is
a defect**: about a fifth of the residual is pinch points at junctions, which are valid
topology and must not be repaired.

## Inputs and outputs

```rust
pub(crate) fn repair_ring_crossings(
    order: &[FaceRings],
    fitted: &mut [FittedPath],
    polys: &[inkvec_core::Polyline],
    cfg: &FitConfig,
) -> usize
```

- `order: &[FaceRings]` — every face's rings, each ring a sequence of `(edge_index,
  reversed)` pairs (`Ring = Vec<(usize, bool)>`, `crates/inkvec-cli/src/lib.rs:43-51`); edges
  are shared between the two faces on either side of them.
- `fitted: &mut [FittedPath]` — one fitted curve per edge, mutated in place.
- `polys: &[Polyline]` — each edge's *measured* polyline, in content units — the ground
  truth every refit is checked against.
- `cfg: &FitConfig` — the same fitting configuration stage 11 used.
- **Return:** the number of capped refits performed (a count of refit operations, not of
  distinct edges).

`repair_ring_crossings`'s own doc (`rings.rs:50-57`):

> "Refit whichever edges take part in a self-crossing, under a tightening span cap, until the
> assembled rings stop crossing themselves. An edge is shared by the two faces either side of
> it, and it is refitted *once* — so both faces continue to reference the same curve and the
> property the planar map exists to guarantee is preserved. Tightening cannot fail to
> terminate: at a cap of one, an edge's fit reproduces its measured polyline, and the
> measured boundary of a face on a partition is simple."

## How it works

### The span cap

The cap is not a count of spans, and not a single global number — it is a **per-edge,
adaptive limit on how many measured polyline points one *segment* may span**, implemented by
`optimal_multimodel_capped` (`crates/inkvec-fit/src/multimodel.rs:206-221`):

> "As `optimal_multimodel`, but forbidding any single segment from spanning more than
> `max_span` measured points. This exists for the self-intersection repair in
> `crate::simple`. It is a blunt instrument on purpose: the objective has no term for 'the
> assembled ring crosses itself', so rather than trying to teach it one, the repair re-runs
> the same objective under a constraint that provably ends the problem. At `max_span = 1`
> every segment is a single polyline edge, which reproduces the measured contour — and the
> measured contour is a simple closed curve by construction, so the loop always terminates."

The capped path is exempt from the DP's point decimation (`multimodel.rs:257-264`), because
"the self-intersection repair relies on the measured contour being reproducible at
`max_span = 1`, and a decimated contour is not simple by construction."

### Algorithm

`repair_ring_crossings`, `rings.rs:58-273`, in order:

1. **Snapshot the unconstrained fit.** `full_fit = fitted.to_vec()`. "A cap is a topology
   emergency brake, not a better description of the boundary; after the offending neighbours
   have been repaired we can often put this compact path back without bringing the crossing
   with it" (`rings.rs:66-68`).
2. **Initialise a per-edge cap** at `polys[k].len().max(2)` — effectively unconstrained.
3. **Flatten every face's rings** into one list, `rings: Vec<&Ring>`.
4. **Round loop, up to `ROUNDS = 10` iterations** (`rings.rs:64`). Each round:
   - **Detect**, in parallel, over every ring touching an edge changed last round (all rings
     on round 1). Each ring is assembled into one `FittedPath` with an `owner: Vec<usize>`
     recording which edge each segment came from, and `simple::self_crossings(&path, 32)`
     returns up to 32 crossing pairs; both owning edges of each pair are marked guilty.
   - **Drop edges already at cap 1** from the guilty set (further halving is impossible).
   - **Exit the round loop if no edges remain guilty.**
   - **Refit every guilty edge in parallel, at half its current cap**:
     `cap[k] = (cap[k] / 2).max(1)`, `multimodel::optimal_multimodel_capped_full(&polys[k],
     cfg, cap[k])`.
   - Commit the refits, record which edges changed (for next round's detection filter), and
     count each refit toward the returned total.
5. **Post-convergence merge, under a budget** (`rings.rs:145-209`) — see below.
6. **Restoration pass, over every refitted edge, in sorted key order** (`rings.rs:210-267`):
   - **Explosion check** first: if the capped fit has more than 32 segments *and* more than 4x
     the unconstrained fit's segment count, the unconstrained (`full_fit`) path is restored
     unconditionally — crossing or not (see "the exploded case" below).
   - Otherwise, candidates are tried in order — the unconstrained fit first, then the merged
     ("smoothed") capped fit if the budget could afford it — and the first candidate that
     leaves every ring containing this edge free of crossings *involving this edge's own
     segments* is kept; otherwise the capped fit from the round loop stands.
7. **Return the refit count.**

### Detecting a crossing

`self_crossings_inner` (`simple.rs:147-273`) is the shared machinery behind both
`self_crossings` (all pairs, up to a limit) and `self_crossings_touching` (only pairs
touching a given mask of segments). The exact criterion, `simple.rs:193-203`:

> "The criterion is the renderer's, not a per-segment one: a ring is invalid exactly when two
> *non-consecutive* sub-segments of its flattened outline intersect. Consecutive
> sub-segments share a point by construction, and that single pair is the only exemption.
> Getting this granularity wrong is what made three earlier versions of this function
> useless. Skipping whole segments that share an endpoint hid the shape that actually
> occurs — a cusp where a cubic doubles back across the line feeding it, crossing about half
> a pixel from the join. Nudging the shared point instead over-fired, because the chord error
> from flattening a cubic dwarfs any nudge small enough to be safe."

Adjacency is decided **geometrically** — by comparing endpoints — rather than from the path's
`closed` flag, because a closed contour is solved by cutting it open and the returned fit is
marked `closed = false` even though it geometrically closes: "Trusting the flag made a plain
circle report its first and last segments as crossing where they merely meet, and the
'repair' turned a 3-segment circle into 61 line segments" (`simple.rs:103-116`). A segment is
also checked against itself (a looping cubic), though "that case does not occur in
practice — zero instances in 2126 rings."

Reporting *every* crossing pair rather than only the first exists because a first-crossing-
only repair needs as many passes as there are crossings to see them all: "measured that way
the repair removed 17% of invalid rings while spending 4.3% more parameters, which is a poor
trade for the axis we are strongest on" (`simple.rs:121-127`). The masked variant,
`self_crossings_touching`, exists purely for performance in the restoration pass: "on a logo
whose two rings carry seven hundred points each, the all-pairs re-check cost five seconds of
a six-second trace" (`simple.rs:132-138`). Arcs are tested by their chord for this purpose,
since "an arc is convex and cannot cross itself; its chord is enough to place it against its
neighbours for this test" (`simple.rs:79-83`).

### Recorded history: the merge pass, removed and then re-added

Two commits, roughly two and a half hours apart, record a genuine back-and-forth that both
halves of which remain in the current tree.

**Removed from the capped path** — commit `5a218b0`, "Repair: refit under a span cap without
the merge pass":

> "The family emoji spent 11.8 s of a 12.8 s trace in the self-crossing repair, and the
> rounds got slower as the cap shrank... The program itself was never the cost — at span 7 it
> solves a 250-point ring in 0.2 ms — the free-cubic merge that follows every fit was: its
> work grows with the segment count the cap produces, and the cubics it re-joins can cross
> again, which is why three boundaries stayed guilty for eight rounds. A capped fit is the
> constrained program's answer and that is what the repair needs; merge and corner sharpening
> now run only on uncapped fits. Family emoji 11.46 s -> 0.65 s at unchanged quality (dE00
> 0.342 -> 0.344)."

The surviving comment in the DP itself (`multimodel.rs:327-337`) makes the same point: under
a span cap, `merge_free_cubics`/`sharpen_corners` are skipped, "since the merge re-joins short
runs into free cubics that can cross again."

**Re-introduced at the ring level, post-convergence** — commit `91c8497`, roughly 2.5 hours
later. Its reasoning lives in the surviving comment (`rings.rs:145-151`), not in the commit
message:

> "A capped refit is the constrained program's raw answer: chords and G1 cubics with the
> corner chamfers left in. Under the cap the merge pass was skipped because it re-joined runs
> into cubics that crossed again and its cost grew with the segment count. Now that the rings
> are simple, merge and sharpen each refitted boundary once, and keep the result only where
> the rings it belongs to stay simple. Without this, 1f9d1-1f3ff-200d-1f680 and 1f640
> (twemoji) came back faceted at +0.12 and +0.10 dE00."

The distinction that lets both coexist: the *capped* path's own internal fitting (inside the
DP, per-round) never merges — it stays a "blunt instrument." Only *after* the round loop has
converged and every ring is simple does the ring-level repair run `merge_free_cubics` and
`sharpen_corners` once more on each refitted edge, and keep the merged result only if it does
not reopen a crossing.

**The merge budget** (`rings.rs:152-167`) exists for the same performance reason as the
original removal:

> "The merge searches a grid of tangent directions and arm lengths per candidate run, which
> costs about eight milliseconds for every segment it looks at. That is affordable on the
> handful of segments a normal repair touches and not affordable at all on a capped refit of
> a boundary with hundreds of them: on simple-icons/biome it was 4.8 s of a 5.8 s trace. So the
> pass gets a budget, counted in segments and spent shortest boundary first, which is
> deterministic and keeps the case it was written for."

`MERGE_BUDGET = 96` segments per boundary, and a further global ceiling of `4 * MERGE_BUDGET =
384` segments across all boundaries in one round: "no single boundary may exceed the budget on
its own: the pathological case *is* one boundary with hundreds of segments, so an escape
hatch that always merges the first one would keep paying exactly the cost this avoids."

**The exploded case** — commit `ddd151a`, "Repair: reject a refit that exploded, and offer
every refitted edge its full fit." A logo (`bulma`, "a seven-line polygon the artist wrote in
22 numbers") grew to 484 line segments at one intake size and 3,840 at another, because the
span cap halves every round toward 1, and at cap 1 a refit reproduces the measured polyline
point by point — a staircase along every diagonal:

> "So every refitted edge now gets the full fit offered back, and a refit that came back with
> more than four times the segments of the unconstrained fit (and more than 32) takes the
> full fit whether or not that crosses. A crossing that survived halving to a cap of one is
> not one that capping fixes, and the staircase is the worst answer on every axis: bulma's
> discarded 15-segment fit scores dE00 0.0880 against the staircase's 0.0934, at 44 parameters
> against 1,920. The crossing it refused to ship renders better than the cure."

The in-code counterpart (`rings.rs:218-231`) states the same conclusion and the guard
literally: `c > 32 && c > 4 * f` where `c` is the capped segment count and `f` the
unconstrained one. Measured effect on the 246-icon screen set: "objective 0.4142 -> 0.4140,
parameters 1.39x -> 1.29x, simple-icons 1.13x -> 0.73x, dE00 unchanged. The repair still
fires where it did — 17 of 30 real brands — and still helps there; only the exploded case
changes."

### Where repair sits in the pipeline

`crates/inkvec-cli/src/lib.rs:976-992`, the call site itself, verbatim:

> "A self-crossing boundary is invisible to the objective — both curves pass through their
> measured points and the render barely changes — but the ring it produces is invalid and
> unpleasant to edit. Measured over 180 real emoji, 53 (29%) emitted at least one, against
> VTracer's 10 (5.6%), and classifying 2126 rings showed *none* were a single cubic looping:
> every one is two different edges of a face crossing after each was fitted within its own
> tolerance."

```rust
let repaired = if args.no_repair {
    0
} else {
    repair_ring_crossings(&order, &mut fitted, &polys, cfg)
};
sw.mark("repair");
```

Note: the surrounding comment block (`lib.rs:908`, `:983-986`) still discusses ordering
against a "polish" stage that has since been deleted from the pipeline; the underlying
ordering principle — repair runs on the geometry that is actually going to be emitted, after
every other geometric transform — still holds, since repair is now the last geometric stage
before mirror symmetrisation, fill assignment and emission.

`--no-repair` (`args.rs:34, 78, 238, 383, 422`) disables the stage entirely. It is parsed but
**does not appear in the CLI's `usage()` help text** — an undocumented flag.

## Constants and thresholds

| name | file:line | value | controls | derivation |
|---|---|---|---|---|
| `ROUNDS` | `rings.rs:64` | 10 | max halving rounds of the outer loop | no stated derivation; one in-code comment refers to "15 rounds of halving" (`rings.rs:222`), inconsistent with this value — see Open questions |
| `MERGE_BUDGET` | `rings.rs:164` | 96 segments | per-boundary merge affordability; `4x` this is the global ceiling | derived from measured cost (~8 ms/segment; `simple-icons/biome` case); the specific 96 not separately justified |
| `RING_SAMPLES` | `rings.rs:456` | 4 | interior samples per curved segment for area/containment | no stated derivation |
| crossing-pair limit | `rings.rs:96`, `:260` | 32 | max crossing pairs reported per ring per call | unnamed literal, no stated derivation |
| explosion thresholds | `rings.rs:230` | `c > 32 && c > 4*f` | when a capped refit is discarded for the unconstrained fit | derived from the `bulma` case; the exact pair (32, 4) not separately justified |
| cap initial value | `rings.rs:69` | `polys[k].len().max(2)` | starting span cap per edge | follows from "cap 1 reproduces the polyline" |
| halving rule | `rings.rs:126, 131` | `(cap[k]/2).max(1)` | tightening schedule | keeps the repair logarithmic in the worst case (`simple.rs:35-36`) |
| `FLATTEN` | `simple.rs:51` | 16 | points per curved segment when flattening for crossing detection | motivated (below render-visibility floor); value none |
| `MAX_REPAIRS` | `simple.rs:55` | 8 | halvings in the (unused-in-production) `fit_simple` sibling | derived: `2^8` covers any contour produced |
| `EPS` | `simple.rs:59` | 1e-6 px | endpoint-coincidence tolerance for adjacency exemption | derived |
| `BLK` | `simple.rs:227` | 16 | bounding-box block size for the all-pairs prune | pure speed optimisation, stated as such |

## Failure modes and edge cases

- **Termination within `ROUNDS` is not guaranteed by the theoretical argument alone.** The
  doc's claim ("tightening cannot fail to terminate") is true only if the loop is allowed to
  run until every edge reaches cap 1; in practice it is bounded at `ROUNDS = 10` halvings, and
  edges already at cap 1 are dropped from the guilty set (`cap[k] > 1` filter), so the loop
  can exit with crossings still present on a boundary long enough that ten halvings do not
  reach cap 1. This is not stated in any comment; it follows from reading `rings.rs:64, 78,
  110` together.
- **An exploded refit is restored to the unconstrained fit "whether or not that crosses"** —
  an explicit, deliberate abandonment of the simple-ring invariant when the alternative (a
  staircase) is worse on every measured axis (`rings.rs:225-227`).
- **A boundary the merge budget cannot afford keeps a more faceted, but still safe, result**:
  "A boundary it cannot afford keeps its capped refit: chords and G1 cubics with the corner
  chamfers in, which is safe and merely more faceted than it could be" (`rings.rs:161-163`).
- **Pinch points at valid junctions must not be repaired**, and the code has no explicit test
  distinguishing them from a genuine defect — only the span cap running out stops a pointless
  refit from being attempted on one.
- **The restoration pass's safety re-check is deliberately partial.** `rings.rs:249-253`
  reasons that "every ring is simple at this point... so a crossing this candidate introduces
  has to involve one of edge `k`'s own segments, and only those pairs are worth testing" — an
  assumption that does not hold in the two cases above (round-loop exhaustion, or an
  explosion bypass), where a ring can still contain a crossing the masked check cannot see.
- **The crossing test operates on flattened geometry** (`FLATTEN = 16` points per curve), so a
  crossing shallower than the flattening error is invisible by construction — argued as
  acceptable because such a crossing would also be invisible in the render.
- **Repair costs parameters.** The shipped version measured `+4.3%` parameters for its gain
  (commit `62c841e`); the rejected first-crossing-only variant traded 17% fewer invalid rings
  for the same 4.3%, "a poor trade for the axis we are strongest on."
- **Global boundary optimisation (stage 8) can itself introduce folds** that repair then has
  to clean up — commit `39935a3` records the boundary solve's displacement being scaled back
  "until it adds no fold of its own... (the repair stage refitting the same rings round after
  round, 2.5 s on one logo)."

## Environment overrides

No `INKVEC_*` environment variable is specific to this module. The one module-specific
override is `--no-repair`, a CLI flag (not an environment variable) that skips the call to
`repair_ring_crossings` entirely (`args.rs:58,138,340,509,548`, checked at `pipeline.rs:620`).
`repair_ring_crossings` itself does read environment, though only for diagnostics: the
generic timing switch `INKVEC_TIMING` — shared with other pipeline stages, e.g.
`pipeline.rs:478` — gates six `eprintln!` calls inside the function
(`rings.rs:114,139,184,206,237,271`) that print per-round and per-phase timings to stderr. It
has no effect on the repaired output, only on what is logged.

## Open questions

- **Whether the loop can exit with crossings still present is not stated or tested.** The
  doc's termination guarantee applies to unbounded halving; `ROUNDS = 10` bounds it in
  practice, and `guilty.retain(|&k| cap[k] > 1)` can remove an edge from further
  consideration before it reaches cap 1.
- **`rings.rs:222`'s comment says "15 rounds of halving"; `ROUNDS` is 10.** Either the
  constant or the comment changed without the other being updated.
- **`crates/inkvec-fit/tests/self_intersection.rs` does not test `repair_ring_crossings` at
  all.** It tests the crossing detector (`self_crossing`, `cubic_self_intersects`) directly,
  and a sibling per-boundary repair, `simple::fit_simple`, which has **no caller anywhere in
  the shipping pipeline** outside this test file — the production code path is
  `repair_ring_crossings` in `rings.rs`, at the ring level, which is untested. Both
  `crates/inkvec-cli/src/rings.rs` and `crates/inkvec-fit/src/simple.rs` are listed in
  `bench/quality_budget.json`'s grandfathered `untested_modules`.
- **No test exercises the ring-level (multi-edge) crossing case, `MERGE_BUDGET`, the
  `exploded` restoration path, or `self_crossings_touching`** — only the single-edge detector
  and the unused `fit_simple` sibling are covered.
- **The specific numeric derivations for `ROUNDS`, `MERGE_BUDGET`, `RING_SAMPLES`, the
  crossing-pair limit of 32, and the exploded-refit thresholds (32, 4x) are not given beyond
  the case study that motivated each.**
