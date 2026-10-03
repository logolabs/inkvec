# Stage 13 — Emit

> Turns settled geometry, fills and stacking order into SVG text — the only stage that
> decides how a number is written down, not what it means.

**Source:** `crates/inkvec-cli/src/emit.rs` (`emit_color`, in nine stages) with
`emit/winding.rs` (ring direction), `crates/inkvec-cli/src/pathdata.rs` (path data),
`primitive.rs` (circle, ellipse and rectangle elements), `rings.rs` (nesting),
`crates/inkvec-cli/src/post.rs` (output options), and `inkvec_svgmin::compact` for `--minify`.
**Entry points:** `emit_color()` (`crates/inkvec-cli/src/emit.rs:151`); post-processing via
`post_process()` (`crates/inkvec-cli/src/post.rs:394`).
**Pipeline position:** after fill assignment (stage mark `"fills"`,
`crates/inkvec-cli/src/pipeline.rs:387`), through the stage mark `"emit"`
(`crates/inkvec-cli/src/pipeline.rs:433`); `post_process` then runs separately, outside the
timed pipeline, just before the file is written.

Crate header, `crates/inkvec-cli/src/lib.rs:6-21`, gives the whole pipeline shape:

```
image ─▶ intake ─▶ trace ─▶ fit ─▶ repair ─▶ emit ─▶ post ─▶ SVG
```

> "* **emit** — `emit`: fitted geometry to SVG text.
> * **post** — `post`: viewBox, margin, background knock-out, minification.
>
> The objective is the same at every stage — squared residual against the image plus
> `lambda` per parameter written — so a stage only keeps what it can pay for."

## What problem this solves

By the time this stage runs, every decision about the *image* has been made: curves, fills,
holes and stacking order are settled. What remains is turning that into bytes, and the module
header (`emit.rs:30-47`) names the two things that make that more than a neutral step:

> "* **Coordinates are written to [`crate::pathdata::EMIT_DECIMALS`], not to `--precision`.**
>   The geometry arrives good to hundredths of a pixel and rounding it to tenths threw away 7%
>   of the fidelity on the full corpus, which is more than most of the fitter earns. [...]
> * **Same-coloured siblings become one compound path.** An artist draws a letter and its
>   counter as one path with a hole; emitting them as two costs a shape and leaves a seam
>   along the shared edge. What a compound path paints is decided by parity -- a point inside
>   an odd number of its rings is painted -- and the parity rules in [`crate::rings`] and
>   [`stack_faces`] are what make the rings right. Each ring is then wound by its nesting
>   depth ([`winding`]), so the default `nonzero` fill rule paints exactly that parity and no
>   `fill-rule` is written: fonts, cutters and old Android read the same shape a browser
>   does. Until 2026-10 the paths said `fill-rule="evenodd"` instead, and 30% of the screen
>   set's files filled their holes back in for any consumer that ignored it."

## Inputs and outputs

`emit_color` takes a `ColorDoc` (`emit.rs:72-112`): the rings of each face as walks over
shared edges, every fitted edge and whole-edge primitive, every face's fill model, the
palette and per-face colour index, per-face clear/opacity, alpha ramps and native fades,
optional recovered layers, the matte, and the raster size; and an `EmitOptions`
(`emit.rs:114-131`): `--cutout`, native transparency, `--no-background`, `--precision`, and
the harmonize settings. Output is a `String` — the complete `<svg>...</svg>` document, viewBox
`-0.5 -0.5 w h`, before `post_process`. The colour pipeline calls it once per candidate
document (flat, and with translucent layers when there are any); the bilevel pipeline calls
`emit_bilevel` (`emit.rs:1317`).

## How it works

`emit_color` runs in stages, each a function with its own reasons (`emit.rs:12-28`):
nesting (`rings::nesting`), stacking (`stack_faces`), harmonizing, paint (`face_paint`), paint
order and names (`paint_tree`, `face_ids`), strokes (`annulus_strokes`), writing (`Writer`,
with same-coloured siblings merged into one compound path), seams (`seam_overrides`), and
layers (`write_layers`).

### The path grammar actually emitted

Four serialisers exist:

| function | file:line | used for |
|---|---|---|
| `fmt_fitted` | `pathdata.rs:60` | a single open or closed stroke (`run_strokes`) |
| `fmt_path` | `pathdata.rs:136` | a plain point sequence |
| `fmt_ring` / `fmt_ring_with` | `pathdata.rs:152`, `:164-279` | a face ring assembled from shared planar-map edges (`fmt_ring_with` takes the seam pass's replacement edges) |
| `primitive_d` | `primitive.rs:46` | circle/ellipse/rounded-rect/rect primitives written as path data |

Commands the emitter writes:

- **`M`** — once per subpath, at its start.
- **`L`** — for `Segment::Line`, and inside the rect/rounded-rect primitives' straight edges.
- **`C`** — for `Segment::Cubic`, when the `S` shorthand test (below) fails.
- **`S`** — the smooth-cubic shorthand; see below.
- **`A`** — for `Segment::Arc`, and for the arcs of a circle, ellipse or rounded rectangle
  written as path data.
- **`Z`** — closes a subpath; `fmt_ring_with` only writes a ring once it has at least two
  segments (`pathdata.rs:275-278`).
- **`H` and `V` are never written by the emitter.** `--minify` respells the finished paths
  (see below), and its writer uses `H`, `V`, relative commands and dropped repeats where they
  say the same thing in fewer bytes.

`fmt_fitted` writes the two arc flags space-separated (`A rx,ry phi large sweep x,y`,
`pathdata.rs:114`), `fmt_ring_with` comma-separated (`:258`) — both are legal SVG. Arc rotation
(`phi`) is always written at a fixed **3 decimals** in degrees, independent of the coordinate
precision. A one-segment ring (a walk out and straight back) is discarded entirely;
`fmt_path` refuses fewer than 3 points outright (`pathdata.rs:136-139`).

### Coordinate precision — the headline fact

`pub(crate) const EMIT_DECIMALS: usize = 2;` (`pathdata.rs:44`). Its doc comment
(`pathdata.rs:20-43`) is the single most load-bearing piece of prose in this stage:

> "This used to be derived from `precision`, which at the default of 0.1 wrote one decimal
> and so rounded every coordinate to a tenth of a pixel. That was throwing the geometry away.
> Displacing the ground truth by a known sub-pixel amount and inverting the error it produces
> (via sub-pixel boundary calibration) measures the boundaries this tracer produces as
> accurate to **0.021 px on lucide and 0.060 px at worst** -- between two and five times finer
> than the grid they were being written on, so up to half the emitted error was quantisation
> of an answer that was already right.
>
> A coordinate should not be rounded more coarsely than the geometry is accurate, and there
> is no reason to write it finer either. Two decimals, 0.01 px, is the first grid below the
> measured accuracy; the full 980-icon devset agrees, and stops agreeing immediately
> afterwards, which is what a bound being reached looks like:
>
> ```
>   1 decimal   objective 0.4922      2 decimals  objective 0.4601
>   3 decimals  objective 0.4599 -- a fortieth of the gain, for another digit everywhere
> ```
>
> Every family improves at two decimals, material-icons by 40 % and simple-icons by 16 %, and
> the parameter count against the artist does not move at all: this buys accuracy with
> digits, not with shapes."

Why quantising an accurate answer is pure loss: the fitted geometry (stage 11) is accurate to
roughly 0.02-0.06 px. Rounding to 0.1 px introduces a *quantisation* error up to 0.05 px in
the worst case — comparable to or larger than the geometric error itself — for no benefit.
Two decimals sits below the measured accuracy floor; a third buys almost nothing, which is
exactly the pattern the objective numbers above show.

**Git provenance for the "7.2%" figure.** Commit `ebc7534`, "Write coordinates to 0.01 px, not
0.1 px":

> "Separating rounding from the segment price on the screen set: as shipped, 1 decimal
> 0.4760; finer price, 1 decimal 0.4790 (worse); 2 decimals 0.4451; 3 decimals 0.4449 (a
> fortieth more, for a digit). The whole gain is the rounding, and a finer price with the old
> rounding makes the corpus worse, so this is the emitter's bound and not the fitter's...
> Full devset against master, with the merge-distance change: full 980 0.4960 -> 0.4601
> (-7.2%), held_a 156 0.4979 -> 0.4661 (-6.4%), held_b 156 0.5002 -> 0.4570 (-8.6%). 874 icons
> better, 105 worse, 1 unchanged. Every family improves, material-icons by 40%. Parameters
> against the artist do not move: this buys accuracy with digits, not shapes. Cost is 12.5%
> more bytes; the 95th-percentile trace time improves slightly."

The **-7.2%** figure is the full 980-icon devset, measured together with an unrelated
merge-distance change in the same commit; the isolated screen-set effect of the rounding
change alone is 0.4760 to 0.4451.

**The accessor deliberately ignores its `precision` argument** (`pathdata.rs:46-51`):

```rust
pub(crate) fn emit_decimals(_precision: f64) -> usize {
    inkvec_core::env::count("INKVEC_EMIT_DECIMALS").unwrap_or(EMIT_DECIMALS)
}
```

`--precision` (default `0.1`) still sets `lambda` for the MDL objective (stage 11); it does
not touch the number of emitted digits. Precision comes from Rust's `{:.*}` runtime-width
formatting; the explicit rounding helpers are the `S`-shorthand test (below) and the
rect/rounded-rect corner rounding in `primitive.rs`.

### The `S` shorthand — tested in rounded space, and why

**Two implementations exist, with two different tolerances.**

**`fmt_ring_with`** (the production ring path) tests in the space the *reader* will
reconstruct from, not the fitter's raw floats (`pathdata.rs:195-226`):

> "`S` restates a cubic whose first control point is the reflection of the previous one's
> second -- the same curve in two fewer numbers, which is what `merge::snap_smooth_joins`
> arranges and what artists write (60% of their smooth cubic joins are exactly this).
>
> Tested on the *rounded* values, not the floats: those are what the reader gets, and a
> reflection that holds only before rounding would decode to a slightly different curve than
> the one that was fitted."

```rust
let r = |v: f64| {
    let m = 10f64.powi(decimals as i32);
    (v * m).round() / m
};
let (wx, wy) = (2.0 * r(pp3.x) - r(pc2.x), 2.0 * r(pp3.y) - r(pc2.y));
let tol = 10f64.powi(-(decimals as i32));
if (a.x - wx).abs() < tol && (a.y - wy).abs() < tol {
    // write S ...
}
```

`prev_c2` carries `(pc2, pp3)` — the previous cubic's second control point and its endpoint —
*across edge boundaries* within a ring, since a ring is several edges' fits concatenated and a
smooth join can fall on the seam between two of them. It resets to `None` on any non-cubic
segment.

**Why the test must be done in rounded space**: the decoder reconstructs the reflected control
point as `2*r(pp3) - r(pc2)` from the numbers actually written to the file. In general
`round(2p - c) != 2*round(p) - round(c)`: rounding does not commute with the linear
reflection. Testing against `wx, wy` computed from the *rounded* `pp3, pc2` — and comparing
them to the raw fitted `a.x, a.y` within one step of the emitted grid (`tol = 10^-decimals`,
0.01 px at the default) — matches what the SVG reader will actually reconstruct.

**`fmt_fitted`** (the stroke path) is the older, stricter form: it compares `2*pp3 - pc2`
against `c1` on raw floats, with a Euclidean tolerance `5e-4` px (`pathdata.rs:57-58`, code at
`:76`). This has no stated derivation, and the two paths can disagree on when a join is smooth
enough.

**Provenance of the "60%" figure** — commit `81959fd`, "Offer the smooth cubic as a candidate,
opt-in": "of the joins between consecutive cubics that are smooth to within a thousandth of a
degree, 60% have equal handle lengths either side, the ratio's median being exactly 1.00 with
its whole interquartile range at 1.00." The fitter-side producer, `merge::snap_smooth_joins`,
is compiled only into research builds (behind `INKVEC_G1`, see `11-fitting.md`), because it
does not pay on the objective ("objective 0.4005 -> 0.4019, fourteen icons better and
seventy-four worse", same commit). The emitter's `S` *detection* is unconditional — a curve
that happens to come out smooth for any reason gets the short form.

### Compound paths, wound by nesting depth

`Writer::emit_level` (`emit.rs:1055-1066`) writes the faces of one nesting level:

> "Siblings are disjoint by construction -- every pixel belongs to one face -- so all the
> siblings of one flat colour can be one compound path under even-odd, which is how an artist
> draws them: a whole word is one `<path>`, not one per letter. Before this the tracer wrote
> one element per connected face and came back with 3.5x the artist's path count on a
> detailed wordmark at the same colour error. Primitives stay their own element and gradient
> faces each own a gradient, so neither merges; nor does a harmonized `<use>`. The merged
> path takes the first member's place in the paint order and its id; every member's children
> follow it."

The merge condition has gates, no numeric threshold: a face must not be a primitive element,
a stroke or a harmonized `<use>`; its fill must be a flat hex colour (a gradient's `url(#...)`
never merges); and its fill *and* `fill-opacity` must match another sibling's exactly.

**Which rule paints the holes.** A compound path's holes — a letter's counter, a face punched
out of the faces under it (`stack_faces`) — are decided by parity: a point inside an odd
number of the path's rings is painted. Until 2026-10 every path carried
`fill-rule="evenodd"` to say so, and its rings ran in whatever direction the planar map's walk
gave them. Under SVG's default `nonzero` rule — the only rule of TrueType and OpenType
outlines, Fontello and the icon-font chain, many cutter and CAD importers, and Android vector
drawables before API 24 — such a hole fills back in; on the 246-icon screen set 73 files
(30%) rendered differently under `nonzero` (`emit/winding.rs:6-13`).

Now every path is passed through `winding::for_nonzero` (`emit/winding.rs:104-119`) when it is
written (`Writer::path_element`, `emit.rs:1040-1052`). The method (`emit/winding.rs:15-34`):
for rings that do not cross each other, a point inside `k` nested rings is crossed once by
each, so if the ring at depth `d` runs the way `(−1)^d` says, the winding number is
`1 − 1 + 1 − …` over `k` terms — 1 for odd `k`, 0 for even, the even-odd answer at every
point. So the two rules agree and no `fill-rule` is written. In the SVG's y-down coordinates a
ring at even depth runs with a negative shoelace sum, the way the planar map already walks an
outline (2,107 of 2,123 outer rings on the screen set), so the convention reverses 154 rings
where the opposite would reverse 2,152. Per compound path (`emit/winding.rs:36-50`):

1. **Read** the `d` as the emitters write it (absolute `M`, `L`, `C`, `S`, `A`, `Z`, numbers
   kept as text). Anything else and the path is returned unread, with
   `fill-rule="evenodd"` (`EVENODD`, `emit/winding.rs:102`): the fallback is the old output,
   never a wrong picture. On the tracer's own output this does not happen.
2. **Measure** each ring on the polygon that follows its curves (`rings::ring_points`): its
   signed area and up to five interior probes (`rings::interior_probes`).
3. **Depth parity**: the parity of all ray crossings with the larger rings' edges (a
   row-bucketed index, `DepthIndex`), the majority over the probes deciding.
4. **Write** each ring that runs the wrong way reversed and every other ring as its original
   text, byte for byte. A reversed cubic swaps its control points; an arc keeps its radii,
   rotation and large-arc flag with the sweep flag flipped, which by SVG 1.1 F.6.5 selects the
   same centre; an `S` is expanded with its reflected control point computed in exact decimal
   arithmetic and written as `S` again only where that reflection holds exactly.

The same pass winds the recovered `--layers` paths (`write_layers`), the harmonized symbols
(`harmonize.rs:133`) and the `--monochrome` and bilevel documents (`mono.rs:540`); a `<use>`
of a harmonized symbol carries no `fill-rule`, since the symbol itself was wound
(`emit.rs:998-999`).

Measured (`emit/winding.rs:66-80`; the 246-icon screen set at 512 px, against v0.2.4 rendered
under `evenodd`): with `evenodd` forced back on, the wound files render identically in resvg,
246 of 246 — the reversal moves nothing; under the default rule 237 of 246 are identical, and
the other 9 differ in 1 to 100 pixels of 262,144, all where two rings of one path overlap by a
sliver (two same-coloured siblings whose fitted outlines cross where they touch): even-odd cut
the overlap out, the wound path paints it, closer to the artist's file in 6 of the 9 (the
gate: 3 icons better, 1 worse by 0.001 dE00). No ring direction can make two overlapping rings
agree under both rules. Chromium agrees on 215 of 246, since Skia's anti-aliasing depends on
edge direction (12 files differ by at most 10 pixels even with `evenodd` forced). Read under
`nonzero`, v0.2.4's files differed from themselves in 85 of 246 (2.95 million pixels). The
code labels its sources: "Method from" W3C SVG 1.1 (Second Edition) §11.3 (`fill-rule`) and
Appendix F.6; "Inspired by" the TrueType Reference Manual, ch. 1 (contour direction under the
non-zero rule) and Fontello's icon-font advice; the depth-parity ray cast is the
crossing-number test of `rings::point_in_ring` summed over rings.

**The measured figures for the merge itself are in commit history.** Commit `b13bed6`, "Emit
the pieces of one colour as one compound path":

> "The tracer wrote one `<path>` per connected face; an artist draws all the disconnected
> pieces of one colour as one compound path, so a word is one path and not one per letter. On
> a detailed wordmark that was 156 paths against the artist's 44 at the same colour error...
> Siblings are disjoint by construction, so the union is exact. The only pixels that change
> are the anti-aliasing seams between same-colour neighbours, which disappear: at most 0.007%
> of pixels, in components of 17 pixels or fewer, colour error equal or slightly better. Four
> detailed logos at 2048: 311 -> 45 paths (artist 434), 114 -> 27 (298), 227 -> 40 (173), 156
> -> 51 (44). Twenty brands at 1024: mean path ratio **1.85x -> 0.75x**, logos over 1.5x from
> nine to two. Gate PASS."

The "under 1.5x author paths" project rule is recorded only in that commit narrative, not
enforced as a hard gate: `bench/ci_gate.py`'s `MARGINS` (`ci_gate.py:138`) caps the
parameter-vs-artist *ratio's regression* at 3% relative, judged at the one-sided 95% upper
bound of a paired bootstrap, which is a change-detection gate, not an absolute ceiling.

### Gradient and fill emission

Each face's fill attribute comes from `face_fill` (`emit.rs:690-754`), in order of precedence:
under a recovered layer, the ground colour; a fade traced natively, written by
`gradient::fade_to_svg` (id `f{i}`); an alpha ramp, written as the gradient an editor would
use — one colour, two `stop-opacity` values, along the axis the fade was measured to run (id
`a{i}`, endpoints at two decimals and opacities at three, `emit.rs:724`); its fill model,
delegated to `gradient::fill_to_svg` (`crates/inkvec-trace/src/gradient/svg.rs:74`); and with
no fill model, its palette ink, else black. A translucent face's flat colour is "un-matted"
before writing, since it was measured over the compositing matte. `fill_to_svg` returns a
`(defs fragment, fill attribute)` pair: `FillModel::Flat` needs no defs;
`FillModel::Linear`/`Radial` write a `<linearGradient>`/`<radialGradient>` with
`gradientUnits="userSpaceOnUse"`. No empty `<defs>` block is written when there is nothing to
put in it (`emit.rs:227-231`).

Which gradients reach this point is decided in the `fills` mark (below): a gradient whose
stops a viewer could not tell apart is painted flat first.

**Where the alpha ramps come from, and which faces are fitted.** The ramps are measured just
before writing, inside the `emit` mark: `face_transparency` (`pipeline.rs:1008`) calls
`alpha::face_alpha` (`crates/inkvec-cli/src/alpha.rs:941`), which under `--cutout` tries, for
each face that is neither clear nor already one flat opacity, to fit a plane to its interior
alpha (`fit_alpha_ramp`, `alpha.rs:170`) and keeps it only when it fades by at least
`RAMP_MIN_FADE = 0.15`, with an RMS residual of at most `RAMP_MAX_RESIDUAL = 0.06`, over at
least `RAMP_MIN_INTERIOR = 64` interior pixels (`alpha.rs:87-93`). Since 2026-09-30 the pass is
linear in the pixel count (`face_alpha`'s "Passes", `alpha.rs:912-923`):

1. one pass over each row's runs gathers every face's alpha statistics (`face_stats`,
   `alpha.rs:1112`); each f64 sum receives the same terms in the same order as the per-pixel
   loop it replaced;
2. the clear and opacity verdicts come from those sums alone;
3. a face goes to the fit only when `ramp_candidate` (`alpha.rs:1273`) says the fit can return
   anything, and the candidates' interior pixels are gathered in one more pass
   (`interior_pixels`, `alpha.rs:1297`).

The skip is a proof, not a heuristic (`alpha.rs:1231-1258`): a face with fewer than 64 interior
pixels fails the fit's own first test, and a face whose interior alphas are all exactly `1.0`
gets an exactly flat plane — the right-hand side of the normal equations is bit for bit the
matrix's first column, so Cramer's rule (`solve3x3`, `alpha.rs:298`) evaluates `det(M_1)` and
`det(M_2)` to exactly `0`, and the fit returns `None`. On the research sets 96% of the 3,125
calls were such provable `None`s, and none ever returned a ramp. The old whole-image fit is kept
as `fit_alpha_ramp_scan` (`alpha/ramp_tests.rs:13`). Citations, as the doc comments give them:
the run-based statistics "Method from" He, Chao & Suzuki 2008, "A Run-Based Two-Scan Labeling
Algorithm", IEEE TIP 17(5); the skip "Not from the literature: a proof that a least-squares fit
is degenerate", with "See also" He & Chao 2015.

### Stage 9: the recovered layers, back to front

With `--layers`, `write_layers` (`emit.rs:1270-1312`) paints each recovered translucent layer
over everything as one compound path: the union of its faces, from the rings they had before
the ground under them was merged, wound by depth so the default rule paints exactly the union,
and one path rather than one per face so the translucent paint is composited once and no seam
appears where two pieces met. The layers are written **back to front**
(`emit.rs:1279-1286`): the decomposition's list is in peel order, frontmost first, because a
layer can only be taken off once nothing lies over it. Written in that order the frontmost
layer went down first and every layer behind it was composited over it: on
`synthetic/stack_overlap` the blue disc came out on top of the green one that covers it, dE00
0.019 -> 2.90 with `--layers`. Over a face both cover, the document now draws
`a₀·C₀ + (1 − a₀)·(a₁·C₁ + (1 − a₁)·G)`, the stack the decomposition peeled; the ids keep the
peel index, `layer-0` frontmost. Which layers are recovered at all (sRGB only, flat faces only,
and only when the stack reproduces every covered face within dE00 1.0) is
`alpha::layers::recover_layers`; see `01-intake.md`.

### `--minify` and `--no-background`

`post_process` (`post.rs:378-406`) composes the output options in a fixed order: background
knock-out, then either `--minify` (`minify_svg`, then `compact_paths`) or, without it, the
generator comment and metadata (`annotate`), then the margin.

`knock_out_background` (`post.rs:94-185`, for `--no-background`) removes the element that
paints the whole canvas. A `<rect>` is matched by its numbers: all four of its edges within
`CANVAS_TOL = 0.25` px of the canvas's, because a fitted rectangle lands a hundredth of a pixel
or two off them. A `<path>` is matched by text: its outer ring must contain all four canvas
corners, written at the emitter's own decimal count (`emit_decimals`) — until 2026-09-08 those
digits were hard-coded and an `INKVEC_EMIT_DECIMALS` override made the match fail silently
(`post.rs:104-110`). Only the first matching element is removed; children of that face remain;
nothing is removed when nothing matches. It runs before minify, which changes how the numbers
are written. Under `--monochrome` it stands aside, since each emitter then leaves the ground
out itself.

`minify_svg` (`post.rs:187-202`) strips every `id="..."` that nothing references (a substring
test for `#id`, so a false positive costs bytes, never a broken reference), removes `<g>`
wrappers left with no attributes, and strips trailing zeros from decimal numbers (`12.50` ->
`12.5`, `3.00` -> `3`), guarding hex colours and URLs.

`compact_paths` (`post.rs:408-434`) then hands every `d` to `inkvec_svgmin::compact`
(`crates/inkvec-svgmin/src/lib.rs:117-131`): the same numbers, written in the fewest bytes —
relative where that is shorter, repeated letters and needless separators dropped, `H`/`V`/`S`
where they say the same thing — with nothing rounded and nothing moved. **Arcs stay arcs**
since 2026-10: the path is read as written (`path::parse_d_written`,
`crates/inkvec-svgmin/src/path.rs:226-252`, "Method from" SVG 1.1 §8.3 and Appendix F.6, the
command resolution following `svgtypes`' `SimplifyingPathParser` minus its arc-to-cubic step),
where the fitter's reading turned each arc into the cubics that approximate it and made every
path holding an arc too long to replace. Measured on the 246-icon screen set (2026-10-02):
`--minify` writes 478,421 bytes where the default writes 710,367, every file pixel-identical
to the default at 512 px; before the arcs were kept it stopped at 541,427; SVGO 4.1's default
preset, which rounds, takes a further 1.1% off (`post.rs:410-420`). Rounding is deliberately
left out ("ten points left on the table on purpose": two decimals would take 18% instead of
8.5% but move pixels on gradient-heavy traces); `inkvec-svgmin --decimals` is where precision
is spent for bytes. If the writer fails to parse the document, or its result is not strictly
shorter, the input is returned as it was.

### viewBox, margin, and `fit_viewbox`

The emitted header is `<svg ... viewBox="-0.5 -0.5 {w} {h}" width="{w}" height="{h}">`. The
origin is `(-0.5, -0.5)` because coordinates are **pixel centres** — pixel `(0,0)`'s centre is
at `0,0`, so the canvas spans `-0.5` to `w-0.5`.

`post::retarget` (`post.rs:63-90`) rewrites only the root `width`/`height` (leaving `viewBox`
untouched) to make a document traced at one size render at another — used whenever the raster
was resampled or capped (`--max-dim`). `post::with_margin` (`post.rs:287-349`) grows the
viewBox by `margin · max(w, h)` on every edge and scales the presented size in proportion,
writing all six numbers at two decimals; a document whose root is not the emitter's header is
left as it was.

**`fit_viewbox` is not part of the Rust emitter.** It lives in the Python benchmark harness,
`bench/inkvec_bench/render.py:101`, and exists to make a non-square SVG render at a requested
square size without Lanczos-stretching it — its docstring records the regression this fixed:
"a 1984x400 logo asked for at 512x512 came back 512x104 and was then stretched 4.9x vertically
with Lanczos [...] The tracer then found a phantom ink in the soft ramps and the lettering came
out wrong (2026-09-05)."

### The `fills` and `emit` stage marks

Both stage marks are inside `finish_color` (`pipeline.rs:341`), timed by a `Stopwatch`
(`crates/inkvec-trace/src/lib.rs:1203`) that prints only under `INKVEC_TIMING`.

**`fills`** (`pipeline.rs:385-387`) times the mirror-symmetry reconciliation after repair
(`apply_mirrors`, `pipeline.rs:913`: one boundary's fit reflected onto its mirror) and
`final_fills` (`pipeline.rs:951-985`): `--no-gradients` paints every face its palette ink, and
otherwise `demote_imperceptible_gradient` (`crates/inkvec-cli/src/pipeline/demote.rs:38`)
paints flat a gradient no viewer could see. The test is the profile's colour range, the
largest OKLab distance between *any two of its stops*, ends and interior stops alike, against
`JND = 0.02`; a demoted gradient is painted the mean of its two ends. Until 2026-10 only the two
ends were compared, which painted flat a light-dark-light shading — a highlight across a cheek,
a crease in a sleeve — whose contrast is all in the middle: on the 168 corpus icons with artist
gradients, 18 of the 49 demoted gradient faces had interior stops, and keeping the ones whose
stops differ made 7 icons better and none worse (`pipeline/demote.rs:20-26`). With no interior stops the
test is the old end-to-end one exactly. The gradient *fitting* itself happens earlier, inside
`inkvec_trace`, and is charged to `trace_total`. The demotion is "Not from the literature: a
fix to the guard's own definition", with "See also" Ottosson 2020 (OKLab).

**`emit`** (`pipeline.rs:388-433`) covers alpha-layer recovery (`alpha::recover_layers`), the
per-face transparency above, and **two competing documents, priced against each other**
(`write_colour`, `pipeline.rs:1095-1104`):

> "Both forms of the document, costed against each other.
>
> A layer is only worth having if it says the same thing in fewer marks. It repaints the faces
> it covers with the ground and composites itself over them, so the image is identical either
> way and the whole decision is a parameter count — the same test every fill model and every
> arc has to pass, with the residual term equal on both sides. Where the layer does not pay,
> the flat form is what is written."

The flat document is always emitted; if layers were recovered, a second document is emitted
with the covered faces merged into their ground (`merge_map`, `pipeline.rs:1077`), and the two
are priced by counting shape elements and bytes in each:

```rust
let pays = pl < pf && bl <= bf;  // fewer shapes AND not more bytes
```

(`pipeline.rs:1188`). Whichever wins becomes the output.

## Constants and thresholds

| name | file:line | value | controls | derivation |
|---|---|---|---|---|
| `EMIT_DECIMALS` | `pathdata.rs:44` | 2 | coordinate decimal places | fully derived — see Coordinate precision above |
| `MIN_RING_AREA` | `rings.rs:33` | 0.25 px² | smallest ring area worth emitting | motivated (far below a pixel so thin features survive); the 0.25 not swept |
| `S`-shorthand tolerance (`fmt_ring_with`) | `pathdata.rs:216` | `10^-decimals` (0.01 px default) | rounded-space reflection test | derived from the emitted grid itself |
| `S`-shorthand tolerance (`fmt_fitted`) | `pathdata.rs:76` | `5e-4` px, raw-space Euclidean | stroke-path reflection test | no stated derivation |
| ring segment minimum | `pathdata.rs:275` | 2 | a ring is discarded below this | it encloses nothing |
| path point minimum | `pathdata.rs:137` | 3 | `fmt_path` early return | degenerate-polygon guard |
| arc rotation precision | `pathdata.rs:114`, `:258`, `:338` | 3 decimals (fixed) | arc `phi` formatting | no stated derivation |
| rounded-rect radius floor | `primitive.rs:76`, `:281` | `1e-4` | below this, written as a plain rect | no stated derivation |
| ellipse rotation floor | `primitive.rs:258` | `1e-3` | below this, no `transform` written | no stated derivation |
| alpha-ramp endpoint precision | `emit.rs:724` | 2 decimals (fixed) | independent of `EMIT_DECIMALS` | no stated derivation |
| opacity precision | `emit.rs:724`, `:766`, `:772` | 3 decimals | `fill-opacity`/`stop-opacity` | no stated derivation |
| `INKVEC_EMIT_DECIMALS` | `pathdata.rs:50` | env override | overrides `EMIT_DECIMALS` | the mechanism used to isolate rounding from the segment price in the 7.2% measurement |
| `EVENODD` | `emit/winding.rs:102` | ` fill-rule="evenodd"` | written only on a `d` the winding pass cannot read | the old output as a fallback |
| `CANVAS_TOL` | `post.rs:129` | 0.25 px | `--no-background` rect match | motivated (a fitted canvas rect lands 0.01-0.02 px off) |
| margin viewBox precision | `post.rs:336` | 2 decimals | `--margin` growth | no stated derivation |
| `JND` | `pipeline/demote.rs:41` | 0.02 (OKLab) | gradient demotion | "a conservative multiple of a just-noticeable difference" |
| `LAYER_MAX_DE00` | `alpha/layers.rs:32` | 1.0 dE00 | `--layers` reproduction guard | about one just-noticeable difference, the tolerance the merge after it already spends |
| `ci_gate.py` ratio margin | `bench/ci_gate.py:138` | 3% relative, at the one-sided 95% upper bound | regression gate on parameter count vs artist | change detection, not the 1.5x absolute rule cited in commit history |

## Failure modes and edge cases

- **Zero-area rings are dropped outright.** The planar map can produce faces one pixel wide
  whose boundary walks out along a chain and straight back; these were emitted as paths that
  "paint nothing at any resolution and cost coordinates, an id, and a line in the document a
  person has to read past" (`rings.rs:106-115`) — "twenty-seven of them on a plain green
  circle."
- **Rounding a rectangle's origin and size independently moves the far edge more than the
  near one.** The fix rounds the two *edges* and derives width as their difference
  (`primitive.rs:270-276`): "how a rectangle centred exactly on a mirror came out a tenth of a
  pixel wider on one side than the other."
- **Punching a fitted ring instead of the primitive it was fitted from leaves a visible
  seam** — measured on `material-icons/qr_code`, dE00 0.060 -> 0.164 (`primitive.rs:24-28`).
- **Parity means punching a transparent face out of its painted ancestors only up to the first
  transparent one.** A second ring inside an already-punched hole flips it back solid — the
  counter of a heart inside a page inside a book cover came out solid black, dE00 0.19 -> 3.33
  on `lucide/book-heart` (`emit.rs:262-269`).
- **Containment for cutting must be certain, not merely likely.** `ring_inside`'s majority vote
  is right for paint-order stacking and wrong for deciding what to punch — a black wedge beside
  the head on `noto-emoji/emoji_u1f3cb_200d_2642`, dE00 0.58 -> 1.78 (`emit.rs:312-320`). A
  punch needs every probe inside (`strictly_inside`).
- **`fill-opacity` in a stacked document blends against whatever painted underneath it, not
  the page** — the film frames on `noto-emoji/emoji_u1f39e`, dE00 0.46 -> 1.43
  (`emit.rs:482-493`).
- **A primitive element cannot carry a hole.** A face that must show a hole through it is
  written as a path even when its outline would otherwise fit a circle (`emit.rs:1019-1020`).
- **Drawing every face's own holes (making paths self-contained so any face could drop
  freely) was tried and reverted** — parameters rose (the arrow 166 to 248) and DISTS regressed
  on three of six probes, "because a ring contained inside another of the same face is not
  reliably a hole, and punching it makes one anyway" (`emit.rs:157-162`).
- **Two rings of one path that overlap by a sliver** paint the overlap under `nonzero` where
  `evenodd` cut it out; no ring direction can make the two rules agree there (9 of 246 screen
  icons at 512 px, 170 px in all; see the winding section).
- **Layers painted in peel order** put the frontmost layer underneath (`synthetic/stack_overlap`,
  dE00 0.019 -> 2.90); they are now written back to front.
- **`--no-background` is a match, and a no-op if it misses** — it returns the input unchanged
  when no element matches; it must run before minify, which rewrites the numbers.

## Environment overrides

Since the settings cleanup (CHANGELOG, *Unreleased*) the engine reads its environment through one helper (`inkvec_core::env`): a switch is off when unset, empty or `0`, and every variable is read once per process. Variables marked *removed* below are gone (their defaults are constants now); those marked *research build* are read only by a binary built with `--features research`. The full list, with what is left and why, is [`docs/internal/env-vars.md`](../internal/env-vars.md).

| variable | effect |
|---|---|
| `INKVEC_EMIT_DECIMALS` | overrides `EMIT_DECIMALS` (coordinate decimal places); `--no-background`'s path match follows it |
| `INKVEC_ALPHADBG` | per-face emit debug dump (rings, areas, outer/clear/parent/drop, holes; `emit.rs:604`) |
| `INKVEC_TIMING` | enables the `Stopwatch` printout that surfaces the `fills`/`emit` marks |

## Open questions

- **`fmt_fitted`'s `S`-shorthand test uses raw floats and a `5e-4` Euclidean tolerance, while
  `fmt_ring_with` tests in rounded space against `10^-decimals`.** The stroke and face-ring
  outputs can disagree on when a join is smooth enough, and the `5e-4` figure carries no
  derivation.
- **The "1.85x -> 0.75x" compound-path figure and the "under 1.5x" project rule live only in a
  commit message (`b13bed6`), not in any code comment or enforced gate.**
- **Overlapping rings.** Where two same-coloured siblings' fitted outlines cross by a sliver,
  the wound path paints the overlap (closer to the artist in 6 of 9 measured cases); the rings
  themselves still overlap, and no fill rule can hide that.
- **Several formatting-precision constants (arc rotation at 3 decimals, opacity at 3 decimals,
  alpha-ramp endpoints at 2 decimals independent of `EMIT_DECIMALS`, the rounded-rect radius
  floor `1e-4`, the ellipse rotation floor `1e-3`) have no stated derivation** and were not
  swept the way the headline coordinate precision was.
- **`MIN_RING_AREA = 0.25 px²` and the ring segment minimum of 2 are motivated but not swept
  against the corpus** the way `EMIT_DECIMALS` was.
