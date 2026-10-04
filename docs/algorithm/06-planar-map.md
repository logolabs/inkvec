# Stage 06 — Planar map

> Turns a per-pixel integer label image into a topological structure of faces, edges and
> nodes in which every boundary between two regions is stored exactly once.

**Source:** `crates/inkvec-trace/src/planar.rs`, with the cracks and their incidence in
`planar/cracks.rs` and the row-run coding in `planar/runs.rs`
**Entry point:** `fn build()` (`planar.rs:163-180`), which calls `dual_segments`
(`planar/cracks.rs:273`), `split_saddle_corners` (`planar.rs:211`), `Incidence::new`
(`planar/cracks.rs:96`), `walk_open_chains` (`planar.rs:286`) and `walk_closed_loops`
(`planar.rs:360`)
**Pipeline position:** after `saddles` (`merge_saddle_faces`, research build only), before
symmetry detection and the sub-pixel refinement, which run side by side on the map it
builds (stage mark `"build_map"`, `lib.rs:1077`). Shared by Quality and Fast mode.

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

### 1. Cracks, read off the label map's row runs

`build` does not walk marching-squares cells directly; it enumerates the *cracks* — the
pixel sides between two pixels of different labels, the 1-cells of the image's cell
complex — as *dual grid edges*, unit segments between adjacent pixel corners:

- Vertical dual edges, node `(i, j) -> (i, j+1)`, separate pixel `(i-1, j)` from `(i, j)`.
- Horizontal dual edges, node `(i, j) -> (i+1, j)`, separate pixel `(i, j-1)` from
  `(i, j)`.

Each `Seg` (`planar/cracks.rs:42-52`) records its two endpoint node ids and which face lies
on each side (`left`/`right`, oriented so `left` is on the left walking from `a` to `b`, y
down), read directly off the two labels straddling it. Because a label difference is
required to emit a segment at all, a flat interior produces nothing — the segment set
already *is* the full set of pixel-grid boundary cracks, before any topology is built from
it. Node ids are `node_id(i, j, w) = j*(w+1) + i` (`planar.rs:129-131`), so real node ids
span `0 .. (w+1)*(h+1)`. This is the bilevel case of marching squares specialised to an
arbitrary number of labels: instead of one 0/1 boundary there is one boundary crack per
differing label pair.

**How the cracks are found** (`dual_segments`, `planar/cracks.rs:273-348`, rewritten
2026-09-30). Boundaries are sparse — at 2048 px the cracks are 0.6% of the pixels and the
row runs 0.24% (median, opaque `big` set) — so the label map is first coded once as its
maximal row runs (`RowRuns::new`, `planar/runs.rs:83-124`): per row, the maximal stretches
`x0..x1` of one label, stored row after row. A row equal to the row above is detected with
one slice compare and copies that row's runs; inside a row a run is extended 16 labels at a
time before the last few are compared one by one. The cracks are then read off the runs:

- **Vertical cracks of row `j`**, left to right: the image's left border unless the row
  starts with the outside label `u16::MAX`; one crack at every run start `x0 > 0`, where the
  label changes by the definition of a run; and the right border unless the row ends with
  `u16::MAX`.
- **Horizontal cracks of node row `j`**: along the top and bottom borders, every pixel of the
  first (last) row whose label is not `u16::MAX`; between rows `j − 1` and `j`, the
  *overlaps* of the two rows' runs (`overlaps`, `planar/runs.rs:157-176`, a two-pointer merge
  into the maximal `x` intervals over which both rows are constant), one crack per pixel of
  every overlap whose two labels differ. Two equal rows have none and are skipped whole.

The output is element for element the vector the pixel scan produced: the same positions
are visited in the same order (vertical cracks row by row, then horizontal ones node row by
node row, each left to right), and a crack is pushed exactly where the two labels differ.
The scan is kept as `dual_segments_scan` in `planar/cracks_tests.rs`, and
`segments_from_runs_equal_the_pixel_scan` compares the two on random and degenerate maps.
Cost: `O(w · h)` label reads to code the runs, most of them 16 at a time, then
`O(runs + segments)`, where the scan made `2 · w · h` bounds-checked comparisons; it took
9.9 ms of `build_map`'s 15.9 ms at 2048 px (mean, opaque `big` set), in Fast and Quality
alike (`planar/cracks.rs:250-255`).

Citations, as the doc comments give them: "Method from" He, Chao & Suzuki 2008, "A Run-Based
Two-Scan Labeling Algorithm", IEEE TIP 17(5), and He, Chao, Suzuki & Wu 2009, "Fast
connected-component labeling", Pattern Recognition 42(9) — rows as runs, each row's runs
merged with the row above's, adapted to report the cracks between runs of different labels
instead of connecting runs of the same one; "Inspired by" Ji, Piper & Tang 1989 (computing
on the interval code rather than the pixels, `planar/runs.rs:43-47`); "See also" Kovalevsky
1989 (cracks as the 1-cells between pixels) and Damiand, Bertrand & Fiorio 2004 (a planar map
of a label image in one scan; read in metadata only, its single-scan claim not checked).
The same row runs serve Fast mode's ramp pass (`fast/bands.rs`), which reads contact lengths
off them.

### 2. The four-way-corner problem, and its exact resolution

Four pixels can meet at one grid corner. If the two pixels on one diagonal are the *same*
face and the two on the other diagonal are *different* faces, the naive dual-grid segment
set gives that face two boundary arms that meet only at a single point — a **bowtie**: the
fill pinches to nothing and reopens, and "no later stage can undo it, because one node has
one refined position however many curves end there" (`planar.rs:185-189`).

`build` resolves this before any chain-walking happens, in a dedicated pass over every
degree-4 node (`split_saddle_corners`, `planar.rs:211-267`, which finds each node's
segments through its own `Incidence`, the structure of step 3):

1. For each node with exactly four incident segments, read the four surrounding labels
   `nw, ne, sw, se`.
2. If both diagonals are a single face, or neither is (`(nw == se) == (ne == sw)`), there
   is nothing to read off the labels — the node is left as an ordinary junction
   (`planar.rs:232-234`).
3. Otherwise, exactly one diagonal is a single face. The **other** face's two incident
   segments (`{right, below}` if `ne == sw`, `{left, below}` if `nw == se`) are given a
   second copy of the node id, offset by `plane = (w+1)*(h+1)` (`planar.rs:214-215,
   249-265`).

The comment records the invariant this rests on precisely: `NE and SW being *one face*
means the two segments joined into a chain separate the same pair of faces. Chaining
segments that do not would hand the edge one face's identity while half of it borders
another, and the other face would lose that edge from its ring entirely.` (`planar.rs:
201-204`) — this is **the invariant discovered in this project**: *merging two boundary
segments into one edge is legal only if both separate the same pair of faces.* Everything
else in `build` — and the fact that `left`/`right` can be trusted downstream at all —
follows from respecting it.

The split id is folded back onto the real grid by `node_position` (`planar/junctions.rs:93`):

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

### 3. Incidence, junctions, and walking chains between them

**Incidence** (`Incidence`, `planar/cracks.rs:62-167`) lists, for every node that has any
segment, its segments in segment order: the nodes in increasing id, each node's segments in
one flat array, in sorted arrays rather than a hash map of vectors. Since 2026-09-30 it is
built by a radix sort instead of a comparison sort (`Incidence::new`,
`planar/cracks.rs:96-123`). Both ends of every segment are listed in *emission order* — end
`a` of segment `k` as entry `2k`, end `b` as `2k + 1` — and stable-sorted by node id with a
least-significant-digit radix sort in digits of 11 bits (2,048 buckets, 16 KiB of counters
that stay in L1 cache; `radix_sort_by_node`, `planar/cracks.rs:184-211`), passing only over
the digits the largest id uses: three at 2048 px. A stable sort keeps, inside each node's
group, the emission order, which is increasing `k` — exactly the order the comparison sort
of distinct `(node, k)` pairs produced. Cost `O(passes · (m + 2^11))` for `m` segment ends,
against `O(m log m)`: the comparison sort took 1.5 ms at 2048 px, and as much again in
`split_saddle_corners`, which builds one too (mean, opaque `big` set). "Method from" Knuth
1998, *The Art of Computer Programming*, vol. 3, §5.2.5 (sorting by distribution, least
significant digit first), adapted only in the digit width. The comparison-sort construction
is kept as `Incidence::new_sorted`, and
`incidence_from_the_radix_sort_equals_the_comparison_sort` and
`the_radix_sort_is_a_stable_sort_by_node` (`planar/cracks_tests.rs:130, 151`) hold the two
equal.

While grouping, `Incidence::new` also records which node each segment end belongs to
(`node_of`), so a walk steps from a segment to the list at its far node in `O(1)`, one array
read (`Incidence::end`, `planar/cracks.rs:137-144`), where the old walk did a binary search
over the nodes — a cache miss per halving — on every point of every chain.

**Junctions.** A node is a junction whenever it is *not* a simple two-way pass-through:

```rust
fn is_junction(segs_here: Option<&[usize]>) -> bool {
    segs_here.map(<[usize]>::len) != Some(2)
}
```

(`planar.rs:276-278`). Degree 4 is always treated as a junction rather than guessed at —
"splitting there is always topologically safe, whereas picking a diagonal pairing can
weld two regions that should be separate" (`planar.rs:271-275`); what the split in step 2
resolved no longer arrives here as degree 4.

From every junction node, `build` walks each unused incident segment forward, following
the unique unused segment at each intermediate degree-2 node, until it reaches another
junction (or runs out) (`walk_open_chains`, `planar.rs:286-354`). The resulting point
sequence becomes one open `Edge`, with `left`/`right` taken from the first segment's
orientation at the starting junction.

Any segment left unused after every junction has been walked belongs to a boundary loop
with no junction anywhere on it at all — an isolated region such as a disc on a flat
background. Those are walked separately and stored with `closed: true`
(`walk_closed_loops`, `planar.rs:360-411`); the walk is dropped if it has fewer than 3
points.

Walk order is deterministic: junctions are visited in increasing node id, the order
`Incidence` holds them in (`planar.rs:294-296`), so a given label image always produces the
same edge list.

### 4. `face_edge_order` — per-face ring assembly

```rust
pub fn face_edge_order(map: &PlanarMap) -> Vec<Vec<Vec<(usize, bool)>>>
```

(`planar.rs:1272-1349`). For each face id, this returns
one or more **rings** — a shape's outer boundary and any holes are separate rings — each a
sequence of `(edge index, reversed)` pairs.

It deliberately returns *references into the map*, not copied geometry:

> "the same traversal works whatever alphabet the fitter used — polylines, cubics, or
> later arcs and primitives — and, more importantly, every face that touches a boundary
> names the same edge index. Nothing is copied, so the two sides of a boundary cannot
> drift apart." (`planar.rs:1261-1264`)

Mechanically: every edge is filed under each face it touches, as `(edge index, reversed)`
— `reversed = false` when the face is `left`, `true` when it is `right`
(`planar.rs:1273-1280`). Within one face, rings are assembled by chaining edges whose
start node (in the direction implied by `reversed`) matches the previous edge's end node,
via a `by_start: HashMap<node, Vec<slot>>` lookup (`planar.rs:1286-1348`). A ring closes
when the walk returns to its own starting node, at which point that repeated junction
node is *not* re-emitted as a separate point — the ring is a cycle of edges, not of
points, so the shared junction is implicit at the seam between the last edge and the
first rather than appearing twice in the point list.

A face id that is `u16::MAX`, or otherwise `>= map.n_labels`, is silently skipped when
edges are filed (`planar.rs:1274, 1277`) — this is how `inkvec-cli`'s layer-merging code
removes an edge from every ring without touching its geometry: setting both `left` and
`right` to `u16::MAX` makes the edge "interior" and it drops out of `face_edge_order`'s
output entirely (`inkvec-cli/src/lib.rs:954-957`).

## Constants and thresholds

`build` itself has no free numeric constants. The four-way-corner decision
(`(nw == se) == (ne == sw)`) is an exact equality test on integer face ids, not a
threshold, and the node-id folding (`% ((w+1)*(h+1))`) is an exact bijection, not a
tuned parameter. This is by design — the whole point of the stage is that topology is
decided exactly, with all approximation deferred to stages 07 and 08.

Two sizes in the rewritten crack and incidence code choose only speed, never the result:

| name | value | controls | derivation |
|---|---|---|---|
| `DIGIT` (`radix_sort_by_node`, `planar/cracks.rs:185`) | 11 bits (2,048 buckets) | digit width of the incidence radix sort | motivated: 16 KiB of counters "stay in L1 cache"; three passes at 2048 px (`planar/cracks.rs:87-89, 169-170`) |
| `LANES` (`RowRuns::new`, `planar/runs.rs:84`) | 16 labels | how many labels a run is extended by at once before the last few are compared singly | motivated: "compiles to two 128-bit compares on x86-64" (`planar/runs.rs:75-78`) |

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
  fixed (`planar.rs:182-199`). The degree-4 split (step 2 above) exists specifically to
  prevent it, but only fires when the two faces on one diagonal already share a face id.
- **`merge_saddle_faces` is off by default.** The stage that decides, from the *image*,
  whether two diagonally-touching-but-currently-separate faces should be read as one
  continuous shape is gated by `INKVEC_SADDLE` (*research build*) and does not run unless that variable is
  set (`lib.rs:561`). Its own doc comment records why: "231 of the 246 screen icons are
  untouched — but it does not yet pay for itself on the set: objective 0.4005 -> 0.4010,
  six icons better and nine worse. The nine are the emitter's containment tree being
  rewritten by a merge into the background..., not the reading being wrong" (`lib.rs:
  555-560`). Practically, this means `build`'s degree-4 split logic mostly protects
  *self-touching* shapes that are already one face by ordinary 4-connectivity (the
  `touching_corner` test case, `planar.rs:1365`) rather than two visually-touching shapes
  of matching colour that never got unioned upstream. A four-way corner where no diagonal
  shares a face id is left as an ordinary junction (`four_inks_meeting_at_a_corner_stay_a_junction`,
  `planar.rs:1389`) — correct when the four regions really are four regions, and a
  missed merge when `INKVEC_SADDLE` (*research build*) would have said otherwise.
- **Chain walking can stall.** If the list at the next node (`inc.end`) holds no unused
  candidate segment before a junction is reached, the walk simply stops there
  (`planar.rs:329-332, 387-389`) rather than panicking; this silently produces a shorter
  edge than the true boundary would warrant. No test in this module exercises that path
  directly.
- **Loops under 3 points are dropped.** A junction-less closed walk with fewer than three
  points is discarded (`planar.rs:393`) — degenerate single- or two-pixel islands that
  cannot bound any area.

## Environment overrides

Since the settings cleanup (CHANGELOG, 0.2.0, *Changed*) the engine reads its environment through one helper (`inkvec_core::env`): a switch is off when unset, empty or `0`, and every variable is read once per process. Variables marked *removed* below are gone (their defaults are constants now); those marked *research build* are read only by a binary built with `--features research`. The full list, with what is left and why, is [`docs/internal/env-vars.md`](../internal/env-vars.md).

None inside `planar::build` itself. The upstream decision that feeds it —
`merge_saddle_faces` — is controlled by `INKVEC_SADDLE` (*research build*) (must be set and not `"0"`,
`lib.rs:561`) and its diagnostic output by `INKVEC_SADDLEDBG` (*research build*) (`lib.rs:564`).

## Open questions

- `merge_saddle_faces` is implemented, tested against real corpus evidence (the coverage
  read at a four-way corner), and shown in its own doc comment to be net-positive on 231
  of 246 icons — yet it ships **off**, because of a downstream interaction with the
  emitter's containment tree on the other 9 (`lib.rs:557-560`). That interaction is not
  analysed further in this file; fixing it would let this stage's corner-splitting logic
  fire on genuinely separate, same-coloured touching shapes rather than only self-touching
  ones.
- `face_edge_order`'s ring assembly is exercised by the saddle-split test
  (`splitting_gives_each_shape_its_own_copy_of_the_corner`, `planar.rs:1411`), which
  checks that both affected faces keep a non-empty ring, but there is no test in this file
  specifically constructing a face with a hole (an outer ring plus an inner ring) to check
  that the two rings are correctly separated rather than merged or lost.
- Resolved on 2026-09-30: the walks used to look the next node up with `inc.get`, whose
  `None` branch (no list at all for that node) could never fire, because `inc` is built
  from both ends of every segment before either walk begins and `next_node` is always an
  end of an existing segment. The walks now read that node's list through `Incidence::end`,
  which returns it directly, so the dead branch is gone; the comment at the call site
  records that `get` found every such node (`planar.rs:321-324`).
