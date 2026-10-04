# Stage 12 — Repair

> Finds every ring a face's fitted boundary makes that crosses itself, and refits only the
> guilty edges — each pinned at a measured point beside its crossing, or under a shrinking
> span cap, whichever the fit's own objective prices lower — until none do.

**Source:** `crates/inkvec-cli/src/rings.rs` (detection, pinning and orchestration),
`crates/inkvec-fit/src/simple.rs` (the crossing test, where each crossing is, and the
per-boundary sibling), `crates/inkvec-fit/src/multimodel.rs` (`optimal_multimodel_capped`
and `optimal_multimodel_forced`, the constrained DP)
**Entry point:** `repair_ring_crossings()` (`crates/inkvec-cli/src/rings.rs:276`)
**Pipeline position:** after curve fitting (stage 11), before fill assignment (stage mark
`"fills"`). Stage mark `"repair"` (`crates/inkvec-cli/src/pipeline.rs:489`).

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
*different edges* of the same face ring. `ring_as_path`'s doc (`rings.rs:572-578`):

> "Needed because a self-crossing on a real boundary is almost never inside one edge's fit.
> Repairing per edge — refitting any edge whose own curve crossed itself — changed 5 images
> out of 180 and left the count at 76. The crossings are between *different* edges of the
> same face, so the ring is the smallest unit at which the defect is even visible."

This is why repair operates on assembled *rings*, not on individual edges: it is the smallest
unit the defect can be seen in at all.

**Why a crossing is not cosmetic.** The code's own framing is narrower than "the fill
inverts" — the strongest statement in the source is that a self-crossing ring is "invalid,
resolved arbitrarily by whichever fill rule applies, and unpleasant to edit"
(`simple.rs:24`, restated in `tests/self_intersection.rs:6-7` and in `repair_fits`'s doc,
`crates/inkvec-cli/src/pipeline.rs:803-808`). The mechanism behind that arbitrariness is this
document's reading of the fill rules (SVG 1.1 §11.3), not a code comment. Since 2026-10 the
emitter winds every ring of a compound path by its nesting depth and writes no `fill-rule`, so
the default `nonzero` rule paints exactly what `evenodd` would; `fill-rule="evenodd"` remains
only on a path the winding pass cannot read (`crates/inkvec-cli/src/emit/winding.rs`; see
`13-emit.md`). Both the agreement and the direction each ring is given assume rings that do
not cross: the winding pass decides a ring's direction from its signed area and from which
other rings contain it, and neither question has one answer for a ring that crosses itself.
Where a ring does cross itself, the region between the two crossing stretches is enclosed
twice in one direction or once in the other, and whether it is painted then depends on the
rule the reader applies — `evenodd` leaves a doubly-enclosed region empty where `nonzero`
paints it — so a letter's counter, or any other hole, can show as a hole in one program and as
a solid patch in another. This specific inversion is not spelled out anywhere in the repo;
only the weaker "resolved arbitrarily" claim is. The related complication is that **not every crossing is
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
  reversed)` pairs (`Ring = Vec<(usize, bool)>`, `crates/inkvec-cli/src/faces.rs:18`); edges
  are shared between the two faces on either side of them.
- `fitted: &mut [FittedPath]` — one fitted curve per edge, mutated in place.
- `polys: &[Polyline]` — each edge's *measured* polyline, in content units — the ground
  truth every refit is checked against.
- `cfg: &FitConfig` — the same fitting configuration stage 11 used.
- **Return:** the number of refits performed, pinned or capped (a count of refit operations,
  not of distinct edges).

`repair_ring_crossings`'s own doc (`rings.rs:214-275`), opening:

> "Refit whichever edges take part in a self-crossing, each either pinned where it crosses
> or under a tightening span cap, whichever the objective prices lower, until the assembled
> rings stop crossing themselves. [...] An edge is shared by the two faces either side of it,
> and it is refitted *once* — so both faces continue to reference the same curve and the
> property the planar map exists to guarantee is preserved. The repair cannot fail to
> terminate: an edge gets at most `LOCAL_ROUNDS` pinned refits, every other refit halves its
> cap, and at a cap of one an edge's fit reproduces its measured polyline, and the measured
> boundary of a face on a partition is simple."

## How it works

Each guilty edge is refitted under one of two constraints a round, both handed to the same
dynamic program as stage 11 (its `Limits`, `crates/inkvec-fit/src/multimodel.rs:221`): a
**pin** — a vertex every solution must keep, beside the crossing — or a **halved span cap**.
The program's own objective chooses between them.

### Pinning where the curves cross

The local move (r2-compact #3). For each crossing, `simple::self_crossing_points`
(`crates/inkvec-fit/src/simple.rs:185`) reports the two segments and where they meet (the
intersection of the two flattened pieces, `meet`, `simple.rs:344`). `ring_as_located_path`
(`rings.rs:919`) names each ring segment's edge *and* its index in that edge's own fit,
counted in the edge's direction even where the ring walks it reversed. For each guilty edge,
`pin_crossings` (`rings.rs:614`) takes the first crossing on each of its crossing segments and
pins the measured point nearest it among those strictly inside the segment's measured run.
The runs are recovered from the geometry by `segment_ranges` (`rings.rs:656`): the fit keeps
no record of which points each segment came from once the merge and the corner sharpening
have run, so each join is matched to the nearest measured point ahead of the previous one.

The pinned refit is `optimal_multimodel_forced` (`multimodel.rs:262-316`): the same program
with no span allowed to pass over a pin, so it is the optimum among segmentations that keep
the pins; a closed edge is cut at a pin, exactly; no decimation (a pin is an index of the full
contour); and, uncapped, the usual merge and sharpening afterwards with the pins kept
(`merge_free_cubics_keeping`, `crates/inkvec-fit/src/merge.rs:365`). Since the measured
boundaries of a partition do not cross, two curves each pinned to a measured point beside
their crossing are held apart there.

*Inspired by* the topology-preserving simplification literature, which restores vertices
only where a simplified chain would cross another instead of tightening the whole chain:
de Berg, van Kreveld & Schirra (1998), "Topologically correct subdivision simplification
using the bandwidth criterion", *Cartography and Geographic Information Systems*
25(4):243-257, doi:10.1559/152304098782383007; Saalfeld (1999), "Topologically consistent line
simplification with the Douglas-Peucker algorithm", *Cartography and Geographic Information
Science* 26(1):7-18, doi:10.1559/152304099782424901. Ours keeps the description-length
program and constrains only where it may break.

### Choosing between the pin and the cap

Neither move dominates. A pin near the end of a long curve can cost more segments than a
halved cap (openmoji/1F9B3: 175 numbers pinned against 154 halved), and a halved cap
re-segments the whole edge where one pin would do (openmoji/1F517 at 512 px: 2.87x the
artist's parameters halved, 2.09x with the choice). So each guilty edge with something new
to pin gets both refits, and keeps the one whose `MultimodelFit::cost` — the program's
`½χ² + λ·params + breaks` before refinement — is lower, ties to the pin (`rings.rs:400-411`).
Only the winner's constraint is remembered: its pins, or its halved cap. *Not from the
literature:* the choice by cost, because the papers above add vertices by a fixed rule.

Measured on the 246-icon gate set (gate v2 against v0.2.5, Quality): parameter ratio
-0.77 % at 128 px ("better"), -0.35 % at 512 px, -0.20 % at 512 px opaque; dE00 +0.09,
+0.22 and +0.05 %, turning +0.13, +0.15 and +0.07 % (all non-inferior). With the pins-only
offer of the restoration pass (step 6 below) the tip reads ratio -0.73 / -0.36 / -0.22 % and
dE00 -0.11 / +0.07 / -0.06 %; Fast is byte-identical. The changes sit in openmoji (ratio
-4.5 % at 128 px, -2.3 % at 512 px); the largest single loss is openmoji/1F3C3 runner at
128 px (+0.05 dE00), a junction pinch only a small cap resolves, where the cheaper of two
resolving refits draws a foot's narrow U as a spike. Rings still crossing
after repair (128 px screen set): 11 on 6 icons, against 14 on 9 with halving alone. Tried
and dropped: pinning every crossing found, not one per segment (-0.09 / -0.33 / -0.12 %); the
pin alone without the halving candidate (-0.50 / -0.29 / -0.19 %); requiring a break
anywhere inside the crossing segment instead of at a point (the same ratio, 19 rings left
crossing on 12 icons).

### The span cap

The cap is not a count of spans, and not a single global number — it is a **per-edge,
adaptive limit on how many measured polyline points one *segment* may span**, implemented by
`optimal_multimodel_capped` (`crates/inkvec-fit/src/multimodel.rs:318-328`):

> "As `optimal_multimodel`, but forbidding any single segment from spanning more than
> `max_span` measured points. This exists for the self-intersection repair in
> `crate::simple`. It is a blunt instrument on purpose: the objective has no term for 'the
> assembled ring crosses itself', so rather than trying to teach it one, the repair re-runs
> the same objective under a constraint that provably ends the problem. At `max_span = 1`
> every segment is a single polyline edge, which reproduces the measured contour — and the
> measured contour is a simple closed curve by construction, so the loop always terminates."

The capped path is exempt from the DP's point decimation (`multimodel.rs:393-396`), because
"the self-intersection repair relies on the measured contour being reproducible at
`max_span = 1`, a decimated contour is not simple by construction, and a forced vertex is an
index of the full contour."

### Algorithm

`repair_ring_crossings`, `rings.rs:276-592`, in order:

1. **Snapshot the unconstrained fit.** `full_fit = fitted.to_vec()`. "A cap is a topology
   emergency brake, not a better description of the boundary; after the offending neighbours
   have been repaired we can often put this compact path back without bringing the crossing
   with it" (`rings.rs:283-286`).
2. **Initialise a per-edge cap** at `polys[k].len().max(2)` — effectively unconstrained — and
   no pins (`rings.rs:287-291`).
3. **Flatten every face's rings** into one list, `rings: Vec<&Ring>`.
4. **Round loop, up to `ROUNDS = 10` iterations** (`rings.rs:282`, `:306`). Each round:
   - **Detect**, in parallel, over every ring touching an edge changed last round (all rings
     on round 1). Each ring is assembled into one `FittedPath` with an owner `(edge,
     segment)` per segment (`ring_as_located_path`), and `simple::self_crossing_points(&path,
     32)` returns up to 32 crossing pairs with where they cross; both owning edges of each
     pair are marked guilty (`rings.rs:309-340`).
   - **Drop edges already at cap 1** from the guilty set (`rings.rs:341`).
   - **Exit the round loop if no edges remain guilty.**
   - **Propose pins** for every guilty edge pinned fewer than `LOCAL_ROUNDS = 4` times
     (`rings.rs:345-365`): one per crossing segment, as above.
   - **Refit every guilty edge in parallel**: the halved cap with its existing pins, and,
     where pins were proposed, the pinned refit too; keep the cheaper (`rings.rs:381-413`).
   - Commit the refits, record which edges changed (for next round's detection filter), and
     count each refit toward the returned total.
5. **Post-convergence merge, under a budget** (`rings.rs:437-507`) — see below.
6. **Restoration pass, over every refitted edge, in sorted key order** (`rings.rs:510-584`):
   - **Explosion check** first: if the capped fit has more than 32 segments *and* more than 4x
     the unconstrained fit's segment count, the unconstrained (`full_fit`) path is restored
     unconditionally — crossing or not (see "the exploded case" below).
   - Otherwise, candidates are tried in order — the unconstrained fit first, then the pins
     alone with no cap (for an edge whose cap was halved: its latest proposed pins, fitted
     uncapped and merged with the pins kept, all such edges at once, `rings.rs:511-529`),
     then the merged ("smoothed") capped fit if the budget could afford it — and the first
     candidate that leaves every ring containing this edge free of crossings *involving this
     edge's own segments* is kept; otherwise the refit from the round loop stands. The
     pins-only offer measured dE00 -0.20 / -0.15 / -0.11 % at 128 / 512 / 512 px opaque
     against the repair without it, parameter ratio within ±0.04 %.
7. **Return the refit count.**

### Detecting a crossing

`crossings_located` (`simple.rs:191`, through `self_crossings_inner`, `:165`) is the shared
machinery behind `self_crossings` (all pairs, up to a limit), `self_crossings_touching`
(only pairs touching a given mask of segments) and `self_crossing_points` (all pairs, each
with where it crosses: what the pinning reads). The exact criterion, `simple.rs:259-271`:

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
'repair' turned a 3-segment circle into 61 line segments" (`simple.rs:112-125`). A segment is
also checked against itself (a looping cubic), though "that case does not occur in
practice — zero instances in 2126 rings."

Reporting *every* crossing pair rather than only the first exists because a first-crossing-
only repair needs as many passes as there are crossings to see them all: "measured that way
the repair removed 17% of invalid rings while spending 4.3% more parameters, which is a poor
trade for the axis we are strongest on" (`simple.rs:130-135`). The masked variant,
`self_crossings_touching`, exists purely for performance in the restoration pass: "on a logo
whose two rings carry seven hundred points each, the all-pairs re-check cost five seconds of
a six-second trace" (`simple.rs:141-147`). Arcs are tested by their chord for this purpose,
since "an arc is convex and cannot cross itself; its chord is enough to place it against its
neighbours for this test" (`simple.rs:86-87`).

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

The surviving comment in the DP itself (`multimodel.rs:500-507`) makes the same point: under
a span cap, `merge_free_cubics`/`sharpen_corners` are skipped, "since the merge re-joins short
runs into free cubics that can cross again."

**Re-introduced at the ring level, post-convergence** — commit `91c8497`, roughly 2.5 hours
later. Its reasoning lives in the surviving comment (`rings.rs:146-152`), not in the commit
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

**The merge budget** (`rings.rs:153-168`) exists for the same performance reason as the
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

The in-code counterpart (`rings.rs:220-239`) states the same conclusion and the guard
literally: `c > 32 && c > 4 * f` where `c` is the capped segment count and `f` the
unconstrained one. Measured effect on the 246-icon screen set: "objective 0.4142 -> 0.4140,
parameters 1.39x -> 1.29x, simple-icons 1.13x -> 0.73x, dE00 unchanged. The repair still
fires where it did — 17 of 30 real brands — and still helps there; only the exploded case
changes."

### Where repair sits in the pipeline

`repair_fits` (`crates/inkvec-cli/src/pipeline.rs:800-886`), the call site itself; its doc,
verbatim (`pipeline.rs:803-808`):

> "A self-crossing boundary is invisible to the objective — both curves pass through
> their measured points and the render barely changes — but the ring it produces is
> invalid and unpleasant to edit. Measured over 180 real emoji, 53 (29%) emitted at
> least one, against VTracer's 10 (5.6%), and classifying 2126 rings showed *none*
> were a single cubic looping: every one is two different edges of a face crossing
> after each was fitted within its own tolerance."

```rust
let cfg_repair = scaled(cfg, args.lambda_scale);
let mut repaired = if args.no_repair || fast {
    0
} else {
    repair_ring_crossings(order, &mut fits.fitted, &fits.polys, &cfg_repair)
};
```

(`pipeline.rs:861-866`; `sw.mark("repair")` follows in `fit_and_repair`, `pipeline.rs:489`.)
Fast mode skips the stage. The doc still discusses ordering against a "polish" stage that has
since been deleted (`pipeline.rs:810-814`, "Polish itself has since been removed"); the
underlying ordering principle — repair runs on the geometry that is actually going to be
emitted, after every other geometric transform — still holds, since repair is now the last
geometric stage before mirror symmetrisation, fill assignment and emission.

`--no-repair` (`args.rs:34, 78, 238, 383, 422`) disables the stage entirely. It is parsed but
**does not appear in the CLI's `usage()` help text** — an undocumented flag.

## Constants and thresholds

| name | file:line | value | controls | derivation |
|---|---|---|---|---|
| `ROUNDS` | `rings.rs:282` | 10 | max halving rounds of the outer loop | no stated derivation |
| `MERGE_BUDGET` | `rings.rs:456` | 96 segments | per-boundary merge affordability; `4x` this is the global ceiling | derived from measured cost (~8 ms/segment; `simple-icons/biome` case); the specific 96 not separately justified |
| `RING_SAMPLES` | `rings.rs:907` | 4 | interior samples per curved segment for area/containment | no stated derivation |
| crossing-pair limit | `rings.rs:326`, `:576` | 32 | max crossing pairs reported per ring per call | unnamed literal, no stated derivation |
| explosion thresholds | `rings.rs:542` | `c > 32 && c > 4*f` | when a capped refit is discarded for the unconstrained fit | derived from the `bulma` case; the exact pair (32, 4) not separately justified |
| cap initial value | `rings.rs:287` | `polys[k].len().max(2)` | starting span cap per edge | follows from "cap 1 reproduces the polyline" |
| halving rule | `rings.rs:400`, `:421` | `(cap[k]/2).max(1)` | tightening schedule | keeps the repair logarithmic in the worst case (`simple.rs:35-36`) |
| `LOCAL_ROUNDS` | `rings.rs:600` | 4 | pinned refits per edge before it is only halved | bounds what pinning can add, so the cap still guarantees termination; not derived or swept |
| pin or cap | `rings.rs:405` | lower `MultimodelFit::cost`, ties to the pin | which refit a guilty edge keeps | the program's own objective (see "Choosing between the pin and the cap") |
| `FLATTEN` | `simple.rs:63` | 16 | points per curved segment when flattening for crossing detection | motivated (below render-visibility floor); value none |
| `EPS` | `simple.rs:67` | 1e-6 px | endpoint-coincidence tolerance for adjacency exemption | derived |
| `BLK` | `simple.rs:306` | 16 | bounding-box block size for the all-pairs prune | pure speed optimisation, stated as such |

The sibling `fit_simple` (`simple.rs:363`, used only by tests) no longer has a round count:
it halves until the cap reaches 1, because "a fixed round count never did [reach cap 1] for a
ring longer than 512 points (eight halvings stop at a cap of two)" (`simple.rs:370-375`).

## Failure modes and edge cases

- **Termination within `ROUNDS` is not guaranteed by the theoretical argument alone.** The
  doc's claim ("tightening cannot fail to terminate") is true only if the loop is allowed to
  run until every edge reaches cap 1; in practice it is bounded at `ROUNDS = 10` halvings, and
  edges already at cap 1 are dropped from the guilty set (`cap[k] > 1` filter), so the loop
  can exit with crossings still present on a boundary long enough that ten halvings do not
  reach cap 1. This is not stated in any comment; it follows from reading `ROUNDS`
  (`rings.rs:282`) and the `cap[k] > 1` filter (`rings.rs:341`) together. A pinned refit
  uses a round without halving, so an edge that is pinned and still crosses reaches a small
  cap later; on the 128 px screen set the pinned-or-halved repair leaves 11 rings crossing on
  6 icons where halving alone left 14 on 9.
- **An exploded refit is restored to the unconstrained fit "whether or not that crosses"** —
  an explicit, deliberate abandonment of the simple-ring invariant when the alternative (a
  staircase) is worse on every measured axis (`rings.rs:530-539`).
- **A boundary the merge budget cannot afford keeps a more faceted, but still safe, result**:
  "A boundary it cannot afford keeps its capped refit: chords and G1 cubics with the corner
  chamfers in, which is safe and merely more faceted than it could be" (`rings.rs:453-455`).
- **Pinch points at valid junctions must not be repaired**, and the code has no explicit test
  distinguishing them from a genuine defect — only the span cap running out stops a pointless
  refit from being attempted on one. A pin cannot help there either: the crossing is at a
  junction, a segment's end, so the nearest point inside the segment pins nothing useful,
  and after `LOCAL_ROUNDS` pinned refits the edge is only halved.
- **A pin is placed by geometry, not bookkeeping.** `segment_ranges` matches each join of the
  current fit to the nearest measured point ahead of the last; a corner the refinement moved
  to the intersection of two lines (by at most `3·max(σ_max, 0.25)` px) can match its
  neighbour instead, which moves the pin by a point. A path whose joins cannot be placed in
  order gives no pins, and the edge is halved.
- **The restoration pass's safety re-check is deliberately partial.** `rings.rs:565-568`
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

Since the settings cleanup (CHANGELOG, 0.2.0, *Changed*) the engine reads its environment through one helper (`inkvec_core::env`): a switch is off when unset, empty or `0`, and every variable is read once per process. Variables marked *removed* below are gone (their defaults are constants now); those marked *research build* are read only by a binary built with `--features research`. The full list, with what is left and why, is [`docs/internal/env-vars.md`](../internal/env-vars.md).

No `INKVEC_*` environment variable is specific to this module. The one module-specific
override is `--no-repair`, a CLI flag (not an environment variable) that skips the call to
`repair_ring_crossings` entirely (`args.rs:121`, `:243`, `:624`, checked at
`pipeline.rs:862`). `repair_ring_crossings` itself does read environment, though only for
diagnostics: the generic timing switch `INKVEC_TIMING` — shared with other pipeline stages,
e.g. `pipeline.rs:478` — gates six debug lines inside the function
(`rings.rs:374,428,473,498,546,584`) that print per-round and per-phase timings, and how many
refits were pinned and how many halved, to stderr. It has no effect on the repaired output,
only on what is logged.

## Open questions

- **Whether the loop can exit with crossings still present is not stated or tested.** The
  doc's termination guarantee applies to unbounded halving; `ROUNDS = 10` bounds it in
  practice, and `guilty.retain(|&k| cap[k] > 1)` can remove an edge from further
  consideration before it reaches cap 1.
- **`crates/inkvec-fit/tests/self_intersection.rs` does not test `repair_ring_crossings` at
  all.** It tests the crossing detector (`self_crossing`, `cubic_self_intersects`) directly,
  and a sibling per-boundary repair, `simple::fit_simple`, which has **no caller anywhere in
  the shipping pipeline** outside this test file — the production code path is
  `repair_ring_crossings` in `rings.rs`, at the ring level, which is untested end to end.
  Since 2026-10 `rings.rs` has unit tests for the pinning's bookkeeping (`segment_ranges`,
  `pin_crossings`, `ring_as_located_path`'s reversed indices), `self_intersection.rs` tests
  where `self_crossing_points` says a crossing is, and `multimodel/tests.rs` that forced
  vertices survive the program and the merge.
- **No test exercises the ring-level (multi-edge) crossing case, the choice between the pin
  and the cap, `MERGE_BUDGET`, the `exploded` restoration path, or `self_crossings_touching`.**
- **`LOCAL_ROUNDS = 4` and pinning at the *first* crossing on a segment are not swept.**
- **The specific numeric derivations for `ROUNDS`, `MERGE_BUDGET`, `RING_SAMPLES`, the
  crossing-pair limit of 32, and the exploded-refit thresholds (32, 4x) are not given beyond
  the case study that motivated each.**
