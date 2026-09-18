# Stage 10 — Symmetry

> Detects the exact mirror symmetry a hand-drawn logo's label map carries, and — after
> every stage that could break a tie has run — restores it, by averaging each paired
> boundary point back into exact agreement.

**Source:** `crates/inkvec-trace/src/symmetry.rs` (467 lines)
**Entry points:** `fn detect()` (`symmetry.rs:253`, stage mark `"symmetry_detect"`, `lib.rs:457-458`)
and `fn enforce()` (`symmetry.rs:285`, stage mark `"symmetry"`, `lib.rs:494-495`)
**Pipeline position:** `detect` runs immediately after `build_map`, before `refine_subpix`
(`lib.rs:453-461`); `enforce` runs last of all, after `decode` (`lib.rs:489-495`)

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
face was cut from (`lib.rs:457`).

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
produced, where the comparison is exact" (`lib.rs:455-456`) — this has to happen before
any sub-pixel refinement moves points off that exact lattice, or the point-identity
matching in `pair_edges` would need a tolerance instead of exact equality.

`enforce` is deferred to the very end of `trace_color_full_with_alpha`, run "once every
stage that can break a tie has had its turn" (`lib.rs:456`) — after `refine_subpixel`,
`refine_junctions`, `boundary_opt::optimise`, and `decode::decode_faces` have all had a
chance to nudge points asymmetrically. The comment at the call site makes the intent
explicit: "The label map is exactly symmetric whenever the artist's drawing was, and every
stage above breaks that symmetry a little by breaking ties. Put it back." (`lib.rs:
492-493`)

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

## Constants and thresholds

There are none in the numeric-threshold sense. Every test in this module is exact
equality on integer pixel coordinates or exact half-integer lattice points — `detect`
either finds a mirror that holds on *every* pixel or it does not exist at all
(`symmetry.rs:19-21, 156-163`), and `pair_edges` either finds an exact point-for-point
match or the whole mirror is discarded (`symmetry.rs:243-245, 250-252`). This is a
deliberate design choice recorded in the module doc, not an omission — see "What problem
this solves" above.

## Failure modes and edge cases

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
