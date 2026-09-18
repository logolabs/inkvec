# Stage 06 — Planar map

> Turns a per-pixel integer label image into a topological structure of faces, edges and
> nodes in which every boundary between two regions is stored exactly once.

**Source:** `crates/inkvec-trace/src/planar.rs`
**Entry point:** `fn build()` (`planar.rs:88`, 247 lines)
**Pipeline position:** after `saddles` (`merge_saddle_faces`, opt-in), before `symmetry_detect` (stage mark `"build_map"`, `lib.rs:453-454`)

## What problem this solves

A tracer that stores each region as an independent closed path has to choose between two
failures, and — per the module doc comment — cannot avoid both:

> "lay regions edge-to-edge and rounding disagreement between the two copies of each
> shared boundary opens hairline **seams**; overlap them and geometry is drawn twice —
> **overdraw** — so every shared boundary exists in two places and editing it means
> editing both." (`planar.rs:4-9`)

`M0-BASELINE.md` §4 is cited in the module doc as measuring VTracer sitting on that
trade-off at overdraw 1.64 (ground truth is 1.00), with seams that grow 2.4x from 1x to
4x zoom (`planar.rs:9`).

`build` avoids the choice altogether: a boundary between two regions is stored once, as
an `Edge` that both neighbouring faces reference by index. Seams become structurally
unrepresentable rather than merely rare, and overdraw is 1.0 by construction, not by
tuning (`planar.rs:11-13`).

The other half of the design is *when* things happen. Topology — who is adjacent to whom
— is read off the integer label map, which is exact: junctions sit at grid nodes, and two
faces meeting at one refer to the same integer id. Geometry (the sub-pixel position of
each point) is refined only afterwards, in stage 07. That ordering is what lets
exactness and sub-pixel accuracy coexist: "moving a vertex cannot change who is adjacent
to whom" (`planar.rs:16-18`).

## Inputs and outputs

```rust
pub fn build(labels: &[u16], w: usize, h: usize, n_labels: usize) -> PlanarMap
```

- `labels`: one face id per pixel, row-major, already split into connected components
  upstream (`split_components`, `lib.rs:706`) and, optionally, coalesced across
  diagonal-only touches by `merge_saddle_faces` (`lib.rs:544`, off by default — see
  Failure modes).
- `n_labels`: number of distinct face ids in `labels`.

```rust
pub struct PlanarMap {
    pub edges: Vec<Edge>,
    pub width: usize,
    pub height: usize,
    pub n_labels: usize,
}

pub struct Edge {
    pub points: Vec<Point>,   // grid-corner positions at build time; sub-pixel after stage 07
    pub sigma: Vec<f64>,      // per-point positional uncertainty; placeholder 0.5 at build time
    pub left: u16,            // face id to the left of the stored traversal direction
    pub right: u16,           // face id to the right
    pub start_node: u32,
    pub end_node: u32,
    pub closed: bool,         // true for a boundary loop with no junction at all
}
```

`left`/`right` are face ids — `u16::MAX` denotes the virtual background outside the image
(`planar.rs:70-76`), so a shape touching the image border still closes.

Every `Edge.points` value at the end of `build` sits exactly on a pixel corner (a
half-integer coordinate, `node_point`, `planar.rs:62-65`). `sigma` is a uniform
placeholder of `0.5`, replaced point-by-point by stage 07 (`refine_subpixel`).

## How it works

### 1. Marching-squares contours become dual-grid segments

`build` does not walk marching-squares cells directly; it enumerates the *dual grid
edges* — the segments between adjacent pixel corners wherever the two pixels they
separate carry different labels:

- Vertical dual edges, node `(i, j) -> (i, j+1)`, separate pixel `(i-1, j)` from `(i, j)`
  (`planar.rs:91-106`).
- Horizontal dual edges, node `(i, j) -> (i+1, j)`, separate pixel `(i, j-1)` from
  `(i, j)` (`planar.rs:107-122`).

Each `Seg` records its two endpoint node ids and which face lies on each side
(`left`/`right`), read directly off the two labels straddling it. Because a label
difference is required to emit a segment at all, a flat interior produces nothing — the
segment set already *is* the full set of pixel-grid boundary cracks, before any topology
is built from it. Node ids are `node_id(i, j, w) = j*(w+1) + i` (`planar.rs:58-60`), so
real node ids span `0 .. (w+1)*(h+1)`.

This is the bilevel case of marching squares specialised to an arbitrary number of
labels: instead of one 0/1 boundary there is one boundary crack per differing label pair,
found by the same 4-neighbour comparison used throughout the crate (`label_at`,
`planar.rs:70-76`).

### 2. The four-way-corner problem, and its exact resolution

Four pixels can meet at one grid corner. If the two pixels on one diagonal are the *same*
face and the two on the other diagonal are *different* faces, the naive dual-grid segment
set gives that face two boundary arms that meet only at a single point — a **bowtie**: the
fill pinches to nothing and reopens, and "no later stage can undo it, because one node has
one refined position however many curves end there" (`planar.rs:131-135`).

`build` resolves this before any chain-walking happens, in a dedicated pass over every
degree-4 node (`planar.rs:154-205`):

1. For each node with exactly four incident segments, read the four surrounding labels
   `nw, ne, sw, se`.
2. If both diagonals are a single face, or neither is (`(nw == se) == (ne == sw)`), there
   is nothing to read off the labels — the node is left as an ordinary junction
   (`planar.rs:173-175`).
3. Otherwise, exactly one diagonal is a single face. The **other** face's two incident
   segments (`{right, below}` if `ne == sw`, `{left, below}` if `nw == se`) are given a
   second copy of the node id, offset by `plane = (w+1)*(h+1)` (`planar.rs:126,
   190-202`).

The comment records the invariant this rests on precisely: `NE and SW being *one face*
means the two segments joined into a chain separate the same pair of faces. Chaining
segments that do not would hand the edge one face's identity while half of it borders
another, and the other face would lose that edge from its ring entirely.` (`planar.rs:
147-151`) — this is **the invariant discovered in this project**: *merging two boundary
segments into one edge is legal only if both separate the same pair of faces.* Everything
else in `build` — and the fact that `left`/`right` can be trusted downstream at all —
follows from respecting it.

The split id is folded back onto the real grid by `node_position` (`planar.rs:772-775`):

```rust
fn node_position(id: u32, w: usize, h: usize) -> Point {
    let id = (id as usize) % ((w + 1) * (h + 1));
    node_point(id % (w + 1), id / (w + 1))
}
```

so both copies of a split corner report the same geometric point, but are distinct nodes
for the purposes of adjacency — able to move independently once stage 07 and 08 refine
them.

Which diagonal is "one face" is decided upstream of `build`, by `merge_saddle_faces`
(`lib.rs:517-...`) when it runs, or simply by whatever `split_components`'s 4-connected
flood fill already produced. `build` itself makes no image-based judgement here — it only
reads face ids.

### 3. Junctions, and walking chains between them

A node is a junction whenever it is *not* a simple two-way pass-through:

```rust
let is_junction = |n: u32| -> bool { inc.get(&n).map(|v| v.len()) != Some(2) };
```

(`planar.rs:219`). Degree 4 is always treated as a junction rather than guessed at —
"splitting there is always topologically safe, whereas picking a diagonal pairing can
weld two regions that should be separate" (`planar.rs:214-218`); what the split in step 2
resolved no longer arrives here as degree 4.

From every junction node, `build` walks each unused incident segment forward, following
the unique unused segment at each intermediate degree-2 node, until it reaches another
junction (or runs out) (`planar.rs:228-282`). The resulting point sequence becomes one
open `Edge`, with `left`/`right` taken from the first segment's orientation at the
starting junction.

Any segment left unused after every junction has been walked belongs to a boundary loop
with no junction anywhere on it at all — an isolated region such as a disc on a flat
background. Those are walked separately and stored with `closed: true`
(`planar.rs:285-326`); the walk is dropped if it has fewer than 3 points.

Walk order is deterministic: junction node ids are sorted before iterating
(`planar.rs:225-226`), so a given label image always produces the same edge list.

### 4. `face_edge_order` — per-face ring assembly

```rust
pub fn face_edge_order(map: &PlanarMap) -> Vec<Vec<Vec<(usize, bool)>>>
```

(`planar.rs:1446`, roughly 78 lines to `planar.rs:1523`). For each face id, this returns
one or more **rings** — a shape's outer boundary and any holes are separate rings — each a
sequence of `(edge index, reversed)` pairs.

It deliberately returns *references into the map*, not copied geometry:

> "the same traversal works whatever alphabet the fitter used — polylines, cubics, or
> later arcs and primitives — and, more importantly, every face that touches a boundary
> names the same edge index. Nothing is copied, so the two sides of a boundary cannot
> drift apart." (`planar.rs:1442-1445`)

Mechanically: every edge is filed under each face it touches, as `(edge index, reversed)`
— `reversed = false` when the face is `left`, `true` when it is `right`
(`planar.rs:1448-1455`). Within one face, rings are assembled by chaining edges whose
start node (in the direction implied by `reversed`) matches the previous edge's end node,
via a `by_start: HashMap<node, Vec<slot>>` lookup (`planar.rs:1461-1519`). A ring closes
when the walk returns to its own starting node, at which point that repeated junction
node is *not* re-emitted as a separate point — the ring is a cycle of edges, not of
points, so the shared junction is implicit at the seam between the last edge and the
first rather than appearing twice in the point list.

A face id that is `u16::MAX`, or otherwise `>= map.n_labels`, is silently skipped when
edges are filed (`planar.rs:1449, 1452`) — this is how `inkvec-cli`'s layer-merging code
removes an edge from every ring without touching its geometry: setting both `left` and
`right` to `u16::MAX` makes the edge "interior" and it drops out of `face_edge_order`'s
output entirely (`inkvec-cli/src/lib.rs:927-930`).

## Constants and thresholds

`build` itself has no free numeric constants. The four-way-corner decision
(`(nw == se) == (ne == sw)`) is an exact equality test on integer face ids, not a
threshold, and the node-id folding (`% ((w+1)*(h+1))`) is an exact bijection, not a
tuned parameter. This is by design — the whole point of the stage is that topology is
decided exactly, with all approximation deferred to stages 07 and 08.

The one constant nearby that this stage's tests exercise indirectly is in the *upstream*
saddle-merge decision:

| name | value | controls | derivation |
|---|---|---|---|
| `SADDLE_SIGMAS` (`lib.rs:515`) | `3.0` | how far a four-pixel corner's coverage reading must sit from 0.5 before `merge_saddle_faces` (upstream of `build`) trusts it enough to merge two faces | stated: "Below that the two readings are indistinguishable, and the corner keeps the junction it has always had" (`lib.rs:512-514`) — a standard statistical significance threshold on propagated coverage noise, not a fitted constant |

## Failure modes and edge cases

- **The bowtie.** Left unhandled, a shape that touches itself (or another shape of the
  same face id) at exactly one pixel corner produces a single junction where two
  unrelated boundary arms are welded — "a fill pinched to nothing and reopened" — and this
  cannot be repaired downstream, because by the time curve fitting runs the topology is
  fixed (`planar.rs:131-145`). The degree-4 split (step 2 above) exists specifically to
  prevent it, but only fires when the two faces on one diagonal already share a face id.
- **`merge_saddle_faces` is off by default.** The stage that decides, from the *image*,
  whether two diagonally-touching-but-currently-separate faces should be read as one
  continuous shape is gated by `INKVEC_SADDLE` and does not run unless that variable is
  set (`lib.rs:561`). Its own doc comment records why: "231 of the 246 screen icons are
  untouched — but it does not yet pay for itself on the set: objective 0.4005 -> 0.4010,
  six icons better and nine worse. The nine are the emitter's containment tree being
  rewritten by a merge into the background..., not the reading being wrong" (`lib.rs:
  555-560`). Practically, this means `build`'s degree-4 split logic mostly protects
  *self-touching* shapes that are already one face by ordinary 4-connectivity (the
  `touching_corner` test case, `planar.rs:1539`) rather than two visually-touching shapes
  of matching colour that never got unioned upstream. A four-way corner where no diagonal
  shares a face id is left as an ordinary junction (`four_inks_meeting_at_a_corner_stay_a_junction`,
  `planar.rs:1556-1562`) — correct when the four regions really are four regions, and a
  missed merge when `INKVEC_SADDLE` would have said otherwise.
- **Chain walking can stall.** If `inc.get(&next_node)` finds no unused candidate segment
  before reaching a junction, the walk simply stops there (`planar.rs:256-262,
  301-307`) rather than panicking; this silently produces a shorter edge than the true
  boundary would warrant. No test in this module exercises that path directly.
- **Loops under 3 points are dropped.** A junction-less closed walk with fewer than three
  points is discarded (`planar.rs:310`) — degenerate single- or two-pixel islands that
  cannot bound any area.

## Environment overrides

None inside `planar::build` itself. The upstream decision that feeds it —
`merge_saddle_faces` — is controlled by `INKVEC_SADDLE` (must be set and not `"0"`,
`lib.rs:561`) and its diagnostic output by `INKVEC_SADDLEDBG` (`lib.rs:564`).

## Open questions

- `merge_saddle_faces` is implemented, tested against real corpus evidence (the coverage
  read at a four-way corner), and shown in its own doc comment to be net-positive on 231
  of 246 icons — yet it ships **off**, because of a downstream interaction with the
  emitter's containment tree on the other 9 (`lib.rs:557-560`). That interaction is not
  analysed further in this file; fixing it would let this stage's corner-splitting logic
  fire on genuinely separate, same-coloured touching shapes rather than only self-touching
  ones.
- `face_edge_order`'s ring assembly is exercised by the saddle-split test
  (`splitting_gives_each_shape_its_own_copy_of_the_corner`, `planar.rs:1575`), which
  checks that both affected faces keep a non-empty ring, but there is no test in this file
  specifically constructing a face with a hole (an outer ring plus an inner ring) to check
  that the two rings are correctly separated rather than merged or lost.
- The chain-walk failure path (`inc.get(&next_node)` returning nothing before a junction
  is reached, `planar.rs:286, 332`) carries no assertion or logged warning. It is dead
  code by construction rather than a latent bug: `inc` is populated once, from both
  endpoints of every segment in `segs` (`planar.rs:238-242`), before either walk begins,
  and `segs` is not mutated afterward. Since `next_node` is always the endpoint of an
  existing segment, it already has an entry in `inc` by the time either loop reaches it —
  the `else` branch cannot fire given how the map is built.
