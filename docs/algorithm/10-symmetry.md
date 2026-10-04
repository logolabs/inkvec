# Stage 10 — Symmetry

> Detects the exact mirror symmetry a hand-drawn logo's label map carries, and — after
> every stage that could break a tie has run — restores it, by averaging each paired
> boundary point back into exact agreement.

**Source:** `crates/inkvec-trace/src/symmetry.rs` (467 lines)
**Entry points:** `fn detect()` (`symmetry.rs:293`, called at `lib.rs:1121-1125` and timed
under the stage mark `"refine_subpix"`, `lib.rs:1127`) and `fn enforce()` (`symmetry.rs:342`,
stage mark `"symmetry"`, `lib.rs:1169-1174`)
**Pipeline position:** `detect` runs right after `build_map`, *beside* the measuring phase of
the sub-pixel refinement: on a map of at least 512 boundary vertices the two run side by side
under `rayon::join`, on a smaller one `detect` runs first and the refinement after it, on
the same thread (`lib.rs:1093-1125`). The measured points are written back only once both
have returned (`lib.rs:1126`). `enforce` runs last of all, after `decode`
(`lib.rs:1166-1174`). Since 2026-09-30 the `"symmetry_detect"` mark (`lib.rs:1092`) times
only the setup of the refinement's inputs; detection's time is reported with the
refinement's. Shared by Quality and Fast mode.

## What problem this solves

A large fraction of real logos are mirror-symmetric by construction, and the artist's
symmetry is exact: the module doc states that rendering one such icon and comparing it
with its own mirror gives a residual "around 1e-5, which is floating point and nothing
else" (`symmetry.rs:4-5`). Before this stage existed, the tracer's own output came back
"around 4e-3, two to four hundred times worse, and it is the kind of error a person sees
immediately and a colour metric barely registers — one bar of a three-bar glyph a tenth
of a pixel wider than the other two." (`symmetry.rs:5-8`)

Crucially, the module doc establishes that none of that asymmetry is present in the
*evidence*: "Measured through the pipeline, the input raster is symmetric to 1e-6 and
**the label map is symmetric to exactly zero**." (`symmetry.rs:10-11`) Every bit of the
error is introduced downstream, by ties broken arbitrarily wherever the code happens to
walk one way rather than another — "which pixel a boundary piece exactly on a pixel
border is assigned to, which direction a chain is traversed, which of two equal
candidates a search reaches first." (`symmetry.rs:12-14`) Those are genuine coin flips,
present throughout a pipeline this size, and the doc comment rejects fixing them
individually: "the stage responsible differs from icon to icon." (`symmetry.rs:15-16`)

So symmetry is enforced structurally, as a dedicated pass, rather than defended stage by
stage.

## Inputs and outputs

```rust
pub fn detect(map: &PlanarMap, labels: &[u16], ink: &[usize]) -> Symmetry
```

`ink[label]` maps a label to a comparable "ink identity" — necessary because face ids are
per-connected-component, and a symmetric shape's two mirrored halves are never the same
component; comparing face ids directly would reject every real symmetry there is
(`symmetry.rs:140-142`). In the pipeline this is `face_color` — the palette entry each
face was cut from (`lib.rs:1122`, `:1124`).

```rust
pub struct Symmetry {
    pub mirrors: Vec<Mirror>,
    partner: Vec<Vec<Option<Pairing>>>,   // partner[m][e]: where edge e reflects under mirrors[m]
}
```

```rust
pub fn enforce(map: &mut PlanarMap, sym: &Symmetry) -> usize   // returns points moved
```

`enforce` mutates `map` in place, replacing each boundary point that has a partner with
the midpoint of itself and its mirrored partner point.

## How it works

### `Mirror`: only the exact centre is a candidate

```rust
pub enum Mirror {
    V(i64),   // pixel x -> k - x
    H(i64),   // pixel y -> k - y
}
```

Only integer `k` can be a symmetry of a pixel grid, which is what keeps the test exact —
the axis then falls either on a pixel centre (`k` even) or the border between two (`k`
odd), and every pixel has a partner (`symmetry.rs:41-45`).

`mirrors_of` (`symmetry.rs:143-178`) does not sweep every candidate `k`. A short argument
in the comment shows only the exact image centre can possibly hold: treating an
out-of-bounds pixel as `None` and an in-bounds one as `Some`, "for every x both x and k-x
must be inside: x=0 needs k<w and x=w-1 needs k>=w-1. That leaves k=w-1" — and
symmetrically `k=h-1` for the horizontal axis (`symmetry.rs:166-170`). This is recorded as
**an optimisation, made centre-only for speed**: the loop that used to try every `k` cost
"1.1s of a 4s trace at 2048px" because "each off-centre horizontal candidate matched white
margin rows against white margin rows before it could fail," i.e. `O(h^2 w)`
(`symmetry.rs:169-170`).

`mirrors_of` tests candidacy pixel-by-pixel: `at(x, y) != at(mx, my)` for any pixel fails
the whole mirror (`symmetry.rs:153-163`) — no tolerance, no threshold. The module doc is
explicit that this exactness is a feature, not an oversight: "a mirror is admitted only
when *every* pixel maps to a pixel of the same ink. That test cannot be fooled into
snapping a logo the artist drew asymmetric, which is the failure that would matter."
(`symmetry.rs:19-21`)

### `pair_edges`: matching boundaries by their points, not their endpoints

A closed boundary (a ring) has no meaningful endpoint — the extractor "starts its walk at
whichever crack it reached first, and that choice is not mirror-equivariant"
(`symmetry.rs:183-185`). So a ring and its reflection agree only up to an unknown
rotation offset and an unknown direction, and `pair_edges` (`symmetry.rs:188-246`) has to
recover both:

1. Shortlist candidate partners by point count (`by_len`, `symmetry.rs:194-197`).
2. For each candidate, find where the reflected first point of `e` lands in the
   candidate's point list (all matching positions for a closed ring; only the two ends for
   an open one, since an open boundary can only meet its partner end to end,
   `symmetry.rs:213-219`).
3. Try both directions (`rev in [false, true]`) from that offset, and accept the first
   full point-by-point match (`symmetry.rs:220-241`).

This runs on lattice points from stage 06's extractor, where "every coordinate is an
exact half-integer, so the comparison is exact and needs no tolerance" (`symmetry.rs:
186-187`) — the same discipline as the label-map test in `mirrors_of`. `detect` returns
an **empty** `Symmetry` unless *every* edge finds a partner (or is its own partner) —
"a mirror that holds on the pixels but not on the extracted boundaries would be a bug, and
acting on half of one would be worse than ignoring it." (`symmetry.rs:250-252`)

An edge can pair with itself (`Pairing.edge == e`) — `self_mirrors` (`symmetry.rs:
110-122`) reports which mirrors carry an edge onto itself, needed because "a shape lying
across the axis is its own reflection, so it never gets a partner to copy from and has to
be made symmetric in its own right." (`symmetry.rs:112-113`) — see `centre_primitive`
below.

### `enforce`: averaging, not copying

```rust
let q = m.point(other[pr.index(i, n)]);
let mid = Point::new(0.5 * (e.points[i].x + q.x), 0.5 * (e.points[i].y + q.y));
e.points[i] = mid;
```

(`symmetry.rs:300-305`). Every paired point is replaced with the midpoint of itself and
its mirrored partner's reflection. The doc comment states why averaging is chosen over
letting one side win: "the two sides carry the same evidence and disagree only by
whatever tie was broken between them, so the midpoint is the estimate both of them
support." (`symmetry.rs:282-284`)

### Why detection happens early and enforcement happens last

`detect` is run once, immediately after `build_map`, "on the lattice the extractor
produced, where the comparison is exact" (`lib.rs:1094-1095`) — it has to see the map
before any sub-pixel refinement moves points off that exact lattice, or the point-identity
matching in `pair_edges` would need a tolerance instead of exact equality.

Since 2026-09-30 "before the refinement" means *before the refinement writes*, not before it
starts. The refinement was split into a measuring phase, which reads the lattice map and
writes nothing (`planar::measure_subpixel`), and a write-back (`Refined::apply`). Detection
and the measuring phase both only read the map, so they run side by side under
`rayon::join`, and the moved points are applied after both have returned
(`lib.rs:1093-1126`):

```rust
let (sym, refined) = if planar::refine_in_parallel(&map) {
    rayon::join(|| symmetry::detect(&map, &labels, &face_color), measure)
} else {
    (symmetry::detect(&map, &labels, &face_color), measure())
};
refined.apply(&mut map);
```

Each computes exactly what it computed alone, since neither sees the other's output:
`detect` still compares exact half-integer lattice points, because nothing has been written
back when it reads them. A small map (under 512 boundary vertices,
`planar::refine_in_parallel`) is detected and then measured on the calling thread, as
before, because handing it to the pool costs more than it saves; where rayon has a single
thread (the WebAssembly build) `join` runs the two in turn with no work added. The comment
at the call site cites Ragan-Kelley et al., "Halide", PLDI 2013 ("Inspired by":
independent pipeline stages scheduled to run concurrently, here by hand for one pair of
stages). Detection's time is therefore reported inside `"refine_subpix"`; the
`"symmetry_detect"` mark now only closes the setup before it (`lib.rs:1092`).

`enforce` is deferred to the very end of `trace_color_full_with_alpha`, run "once every
stage that can break a tie has had its turn" (`lib.rs:1095-1096`) — after `refine_subpixel`,
`refine_junctions`, `boundary_opt::optimise`, and `decode::decode_faces` have all had a
chance to nudge points asymmetrically. The comment at the call site makes the intent
explicit: "The label map is exactly symmetric whenever the artist's drawing was, and every
stage above breaks that symmetry a little by breaking ties. Put it back." (`lib.rs:
1167-1168`)

### The interaction hazard: a stage between `detect` and `enforce` can be undone

Because `enforce` unconditionally averages each paired point with its partner's
reflection, **any stage that runs between `detect` and `enforce` and treats a symmetric
pair asymmetrically has its asymmetric work erased** at the `enforce` call — the two
sides are forced back to agreement regardless of which one, if either, was "more right."
This is implicit in `enforce`'s mechanics (it has no notion of which side to trust more
than the other) rather than stated as a warning anywhere in `symmetry.rs`, but it is the
direct consequence of the ordering the pipeline comment describes, and it means a stage
inserted between the two calls needs to either respect the symmetry itself or accept that
`enforce` will flatten whatever it did asymmetrically.

### What `enforce` does not fix: the fit itself

The module doc is explicit that `enforce` only makes the *geometry* symmetric, not the
downstream *fit*: "Two mirror-paired boundaries with exactly mirrored points can still be
fitted differently, because the dynamic program walks a chain in one direction and its
partner in the other." (`symmetry.rs:25-27`) That is handled separately, in the CLI's
fitting stage, by fitting only one edge of each mirrored pair and reflecting the result
onto its partner — both exact and half the work (`symmetry.rs:28-29`; see `mirror_of`,
`symmetry.rs:126-135`, which finds the earlier-indexed partner of a given edge so that
following the map always terminates).

### Reflecting fitted geometry: `reflect_path`, `reflect_primitive`, `centre_primitive`

Three further functions support the fitting-side half of the symmetry story:

- `reflect_path` (`symmetry.rs:317-342`) reflects a fitted `FittedPath` — lines and cubics
  transform point-by-point; a circular arc keeps its radius and large-arc flag but flips
  its sweep, "same circle, same side of the chord, the other way round the plane"
  (`symmetry.rs:315-316`).
- `reflect_primitive` (`symmetry.rs:347-366`) reflects a fitted circle, ellipse, or
  rounded rectangle; only an ellipse's axis angle needs adjustment, "because a mirror
  negates the angle it makes with the mirror's own axis" (`symmetry.rs:345-346`).
- `centre_primitive` (`symmetry.rs:375-405`) is for a primitive that is its own
  reflection (via `self_mirrors`) — being symmetric is then one exact equation on a couple
  of its numbers (its centre lies on the axis; an ellipse's angle snaps to whichever
  right-angle-multiple it is already nearer). The doc comment gives the motivating case
  directly: "This is where 'make the circle round' actually happens: three bars of a
  glyph come back 21.3, 21.4 and 21.3 pixels wide because each was fitted alone, and the
  middle one straddles the axis." (`symmetry.rs:372-374`)

### A boundary that is its own mirror image: `mirror_fit`

`mirror_of` and `centre_primitive` leave one case open: a boundary paired with *itself*
that is fitted as a path, not a primitive — a closed outline lying across the axis, or an
open boundary running from a junction on one side to its mirror junction on the other. Its
points are exactly symmetric after `enforce`, but the dynamic program places its vertices
where the objective's ties fall, which is not symmetrically. On `lucide/beaker` (the
`real_symmetry` case of `bench/cases.py`) both outlines straddle the axis and the left top
corner came back at x = 21.66 against 22.51 for the reflection of the right one; the
render's mirror residual rose from 7.5e-5 to 2.1e-4 (over the case's 1e-4) when the
converged boundary solve of 0.2.6 moved the points slightly and the program broke its ties
differently.

`crates/inkvec-cli/src/mirror_fit.rs` fits such a boundary on one side of the axis and
reflects the result. `pipeline::fit_boundaries` calls `mirror_fit::choose`
(`crates/inkvec-cli/src/pipeline.rs:718-726`) for every edge `self_mirrors` names:

1. **The ordinary fit, when it is already symmetric** (every anchor's reflection is an
   anchor, to 1e-6 px; `choose`, `mirror_fit.rs:217-236`). The program finds the
   symmetric optimum by itself on most such boundaries.
2. **Otherwise the fundamental domain** (`half`, `mirror_fit.rs:334-388`): a closed
   boundary's reflection reverses its direction of travel, so its points pair as
   `π(i) = s − i mod n` and the involution has exactly two fixed sites, the axis crossings,
   each a point (`π(i) = i`) or the middle of a piece (`π(i) = i + 1`); an open one pairs as
   `π(i) = n − 1 − i` and crosses once. The points from one crossing to the next (or from
   the start to the crossing) are taken, the crossings snapped exactly onto the axis.
3. **Fitted by the ordinary fitter and reflected** (`fit_side` and `assemble`,
   `mirror_fit.rs:144-174`, `:398-412`): the half's fit, then its reflection run
   backwards (`reflect_path`). A half that is itself symmetric under the second mirror is
   fitted as a quarter, recursively. Both halves are tried (`fit`, `mirror_fit.rs:109-140`;
   a closed boundary is rotated to start at its other crossing, an open one is reversed),
   and the one with the lower description length is kept, so neither side is favoured by
   the order the points happen to be stored in.
4. **The joins at the axis** (`polish_joins`, `mirror_fit.rs:483-556`): two mirrored lines
   are tried as one line square to the axis, two circular arcs as one arc of twice the angle
   with its centre on the axis, and a cubic arriving within `SNAP_DEGREES` = 10° of the axis
   normal is given exactly that end tangent, so the halves join smoothly. A merge is kept
   when the description length `½χ² + λ·k` of the whole boundary (`multimodel::path_cost`)
   does not rise, a tangent snap when it rises by at most λ.
5. **Kept only when it is no dearer** than the ordinary fit by that same description length
   (a tie goes to the symmetric fit). Measured on the gate (2026-10-04, the screen set
   judged at 1024 px, dE00 against v0.2.5): always taking the symmetric fit read −15.41 % /
   −11.77 % / −7.23 % at 128 / 512 / 512 px opaque, discounting its reflected half's
   parameters −15.72 % / −11.84 % / −7.23 %, this rule −16.11 % / −11.85 % / −7.24 % on the
   first half alone and −16.37 % / −11.82 % / −7.24 % with both halves tried, against
   −15.95 % / −11.68 % / −7.15 % without any of it.

Citations, as the code gives them: the selection is "Method from" J. Rissanen (1978),
*Modeling by shortest data description*, Automatica 14(5),
<https://doi.org/10.1016/0005-1098(78)90005-5>; fitting the fundamental domain is "Not from
the literature" (the rule this module already applies to mirror-paired boundaries, extended
to a boundary that is its own pair), with "See also" N. J. Mitra, L. J. Guibas, M. Pauly
(2007), *Symmetrization*, ACM TOG 26(3), <https://doi.org/10.1145/1276377.1276456>, which
makes approximate symmetries exact by deforming the shape, as `enforce` does to the points.

## Constants and thresholds

There are none in the numeric-threshold sense. Every test in this module is exact
equality on integer pixel coordinates or exact half-integer lattice points — `detect`
either finds a mirror that holds on *every* pixel or it does not exist at all
(`symmetry.rs:19-21, 156-163`), and `pair_edges` either finds an exact point-for-point
match or the whole mirror is discarded (`symmetry.rs:243-245, 250-252`). This is a
deliberate design choice recorded in the module doc, not an omission — see "What problem
this solves" above.

## Failure modes and edge cases

- **A self-symmetric boundary whose symmetric fit is dearer keeps its asymmetric ordinary
  fit** (`mirror_fit::choose`): typically a smooth crossing the ordinary fit spans with one
  segment that the halves cannot merge back. And the repair stage may refit a boundary
  whose ring crosses another without regard to its mirror. Fast mode fits nothing through
  `mirror_fit` (`real_symmetry` fails there, and is not on its ratchet).
- **Point symmetry (a half turn) is not a mirror** and is neither detected nor kept: the
  synthetic case `mirror_r` of `bench/cases.py` fails in both modes.

- **A symmetric-looking icon with even one pixel of genuine hand-drawn asymmetry finds no
  mirror at all**, by design — `mirrors_of`'s per-pixel test has no tolerance
  (`symmetry.rs:153-163`), and this is exactly the intended behaviour (see the module doc
  quote above about not "snapping a logo the artist drew asymmetric").
- **A mirror that holds on pixels but not on extracted boundaries is treated as a bug and
  silently dropped**, per-mirror, in `detect` (`symmetry.rs:266-276`): each candidate
  mirror is paired independently, and only mirrors whose `pair_edges` succeeds are kept
  in the returned `Symmetry` (`symmetry.rs:272-275`) — the other candidate mirrors are
  simply not present in the result, with no error surfaced to the caller.
- **Only two mirror axes are ever considered** — a single vertical and a single horizontal
  candidate at the exact image centre (`symmetry.rs:171-176`). A diagonal mirror, a
  4-fold rotational symmetry, or an off-centre local symmetry (e.g. within one glyph of a
  wordmark rather than across the whole canvas) is out of scope for this stage entirely.
- **The ordering hazard** described above (a stage between `detect` and `enforce`
  overwritten by `enforce`) is a live property of the current pipeline (`boundary_opt`,
  `decode` both run in that window, `lib.rs:465-489`) and is not defended against by any
  assertion; it is simply accepted as the intended behaviour, per the "Put it back"
  comment at the `enforce` call site.

## Environment overrides

Since the settings cleanup (CHANGELOG, 0.2.0, *Changed*) the engine reads its environment through one helper (`inkvec_core::env`): a switch is off when unset, empty or `0`, and every variable is read once per process. Variables marked *removed* below are gone (their defaults are constants now); those marked *research build* are read only by a binary built with `--features research`. The full list, with what is left and why, is [`docs/internal/env-vars.md`](../internal/env-vars.md).

`INKVEC_TIMING` (checked as `std::env::var_os`) turns on per-call timing output inside
`detect` for `mirrors_of` and `pair_edges` (`symmetry.rs:260, 263-264, 267, 269-271`).
No other stage-specific overrides exist in this file.

## Tests as specification

`mod tests` (`symmetry.rs:407-467`) exercises the two central guarantees concretely, on a
synthetic three-bar glyph built to be exactly mirror-symmetric (`bars`, `symmetry.rs:
414-423`):

- `finds_the_mirror_and_pairs_the_boundaries` (`symmetry.rs:425-433`): for a 16-wide
  image, confirms the detected axis is `Mirror::V(15)` (pixel `x` maps to `15 - x`, so the
  axis sits between columns 7 and 8) and that `sym` is non-empty.
- `averages_a_nudge_back_out` (`symmetry.rs:435-466`): pushes every point of one boundary
  0.1px to the right, then calls `enforce`, and asserts every paired point afterwards is
  an exact reflection of its partner (`dist < 1e-9`) — a direct, minimal demonstration
  that averaging restores exact agreement regardless of which side was perturbed, and that
  edges without a partner are left untouched (`map.edges.len() == before.len()`,
  `symmetry.rs:465`).

## Open questions

- The interaction hazard between `detect` and `enforce` (any intervening stage's
  asymmetric adjustment being erased) is a real property of the pipeline but is not
  documented as a warning anywhere in `symmetry.rs` itself, nor guarded by any test that
  specifically inserts an asymmetric perturbation between the two calls and confirms it is
  cleanly undone rather than leaving, say, mismatched point counts.
- `mirrors_of` only ever tests the exact-centre vertical and horizontal axes, and the file
  itself explains why for the off-centre case: an axis anywhere other than `w - 1` or
  `h - 1` cannot satisfy the full-image reflection check, since a pixel inside the image
  has no counterpart once its mirror falls outside it. An earlier version looped over
  every candidate `k` and paid for it — 1.1s of a 4s trace at 2048px, matching white
  margin rows against white margin rows before failing — for a check that could only ever
  hold at the centre, so the loop was replaced with the direct computation
  (`symmetry.rs:174-179`). Diagonal symmetry is a different matter: `Mirror` has only `V`
  and `H` variants (`symmetry.rs:52-57`), so a diagonal axis has no representation in the
  type at all, and neither `symmetry.rs` nor `docs/DESIGN.md` states whether omitting it
  was a deliberate scope decision or simply the first version implemented — that remains
  **Unverified**.
- No constant in this module lacks a stated derivation, which is unusual among the stages
  in this pipeline — the entire design rests on exactness rather than tuned thresholds, by
  the same reasoning documented in stage 06.
