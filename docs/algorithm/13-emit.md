# Stage 13 — Emit

> Turns settled geometry, fills and stacking order into SVG text — the only stage that
> decides how a number is written down, not what it means.

**Source:** `crates/inkvec-cli/src/emit.rs` (`emit_color`, the longest function in the
codebase at ~536 lines), `crates/inkvec-cli/src/post.rs`
**Entry points:** `emit_color()` (`crates/inkvec-cli/src/emit.rs:385-920`); post-processing
via `post_process()` (`crates/inkvec-cli/src/post.rs:153-158`)
**Pipeline position:** after fill assignment (stage mark `"fills"`,
`crates/inkvec-cli/src/lib.rs:1065`), through the stage mark `"emit"`
(`crates/inkvec-cli/src/lib.rs:1179`); `post_process` then runs separately, outside the
timed pipeline, just before the file is written.

Crate header, `crates/inkvec-cli/src/lib.rs:6-21`, gives the whole pipeline shape:

```
image -> intake -> trace -> fit -> repair -> emit -> post -> SVG
```

> "**emit** — [`emit`]: fitted geometry to SVG text. **post** — [`post`]: viewBox, margin,
> background knock-out, minification. The objective is the same at every stage — squared
> residual against the image plus `lambda` per parameter written — so a stage only keeps
> what it can pay for."

## What problem this solves

By the time this stage runs, every decision about the *image* has been made: curves, fills,
holes and stacking order are settled. What remains is turning that into bytes, and the module
header states plainly that this is not a neutral step:

`emit.rs:1-13`, verbatim:

> "Writing the document. Everything here turns fitted geometry into SVG text, and nothing
> here decides anything about the image — by the time these run, the curves, the fills and
> the stacking order are settled. Two things are worth knowing before reading:
>
> * **Coordinates are written to `--precision`, not to taste.** The geometry arrives good to
>   hundredths of a pixel and rounding it to tenths threw away 7% of the fidelity on the full
>   corpus, which is more than most of the fitter earns.
> * **Same-coloured siblings become one even-odd path.** An artist draws a letter and its
>   counter as one path with a hole; emitting them as two costs a shape and leaves a seam
>   along the shared edge."

(The first bullet's phrasing — "written to `--precision`" — is stale; see Coordinate
precision below. `--precision` no longer sets the emitted digit count.)

## Inputs and outputs

`emit_color` takes the whole document state: the face traversal order, every fitted path and
primitive, every face's fill model, the palette, per-face colour indices, clear/opacity/matte
alpha state, alpha ramps, an optional recovered-layer document, and the canvas dimensions and
precision. Its own doc explains why it is one function with many arguments rather than a
struct: "bundling it into a struct would only rename the arguments, not reduce them"
(`emit.rs:382-384`). Output is a `String` — the complete `<svg>...</svg>` document, before
`post_process`.

## How it works

### `fmt_ring` and the SVG path grammar actually emitted

Four serialisers exist:

| function | file:line | used for |
|---|---|---|
| `fmt_fitted` | `emit.rs:61-123` | a single open or closed stroke (`run_strokes`) |
| `fmt_path` | `emit.rs:132-144` | a plain point sequence |
| `fmt_ring` | `emit.rs:147-256` | a face ring assembled from shared planar-map edges |
| `primitive_d` | `emit.rs:280-328` | circle/ellipse/rounded-rect/rect primitives written as path data |

Commands actually written, confirmed against every literal command-character emission in
`emit.rs`:

- **`M`** — once per subpath, at its start.
- **`L`** — for `Segment::Line`, and inside the rect/rounded-rect primitives' straight edges.
- **`C`** — for `Segment::Cubic`, when the `S` shorthand test (below) fails.
- **`S`** — the smooth-cubic shorthand; see below.
- **`A`** — for `Segment::Arc`, and for the two half-arcs of a `<circle>`-as-path, the
  `<ellipse>`-as-path, and rounded-rect corners.
- **`Z`** — closes a subpath; `fmt_ring` only emits it once the accumulated ring has at least
  two segments.
- **`H` and `V` are never emitted anywhere in the codebase.** There is no axis-aligned path
  shorthand in the emitter; a line snapped to an axis by stage 11's (off-by-default)
  `snap_axis_aligned` is still written as a plain `L`.

Two minor syntactic inconsistencies exist between the two arc writers: `fmt_fitted` writes the
two arc flags space-separated (`A rx,ry phi large,sweep x,y`), `fmt_ring` writes them
comma-separated — both are legal SVG. Arc rotation (`phi`) is always written at a fixed
**3 decimals** in degrees, independent of the coordinate precision setting.

`fmt_ring` buffers a whole ring and only commits it if it accumulates at least 2 segments
(`emit.rs:252-255`) — a one-segment ring (a walk out and straight back) is discarded
entirely. `fmt_path` refuses fewer than 3 points outright.

### Coordinate precision — the headline fact

`const EMIT_DECIMALS: usize = 2;` (`emit.rs:54`). Its doc comment (`emit.rs:30-53`) is the
single most load-bearing piece of prose in this stage:

> "This used to be derived from `precision`, which at the default of 0.1 wrote one decimal
> and so rounded every coordinate to a tenth of a pixel. That was throwing the geometry away.
> Displacing the ground truth by a known sub-pixel amount and inverting the error it produces
> measures the boundaries this tracer produces as accurate to **0.021 px on lucide and
> 0.060 px at worst** — between two and five times finer than the grid they were being
> written on, so up to half the emitted error was quantisation of an answer that was already
> right.
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
> Every family improves at two decimals, material-icons by 40% and simple-icons by 16%, and
> the parameter count against the artist does not move at all: this buys accuracy with
> digits, not with shapes."

Why quantising an accurate answer is pure loss: the fitted geometry (stage 11) is accurate to
roughly 0.02-0.06 px — a claim independently corroborated by project history recorded
elsewhere in this documentation set (boundaries right to 0.02-0.06 px once geometry work was
finished). Rounding to 0.1 px introduces a *quantisation* error up to 0.05 px in the worst
case — comparable to or larger than the geometric error itself — for no benefit, since nothing
downstream needed the extra digit of compactness. Two decimals sits below the measured
accuracy floor; a third decimal buys almost nothing because the floor has already been
reached, which is exactly the pattern the objective numbers above show (0.4601 to 0.4599, "a
fortieth of the gain, for another digit everywhere").

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

**The accessor deliberately ignores its own `precision` argument** — the historical coupling
to `--precision` is preserved only as a comment, not as behaviour:

```rust
pub(crate) fn emit_decimals(_precision: f64) -> usize {
    std::env::var("INKVEC_EMIT_DECIMALS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(EMIT_DECIMALS)
}
```

`--precision` (default `0.1`) still sets `lambda` for the MDL objective (stage 11); it no
longer touches the number of emitted digits. There is no explicit `round()` call on the
general coordinate path — precision comes from Rust's `{:.*}` runtime-width formatting.
Three places do call an explicit rounding helper with a `10f64.powi(decimals)` multiplier:
the `S`-shorthand test (below), and the rect/rounded-rect primitive corner rounding.

### The `S` shorthand — tested in rounded space, and why

**Two implementations exist, with two different tolerances — a documented divergence.**

**`fmt_ring`** (the production ring path) tests in the space the *reader* will reconstruct
from, not the fitter's raw floats. `emit.rs:172-191`, verbatim comment:

> "`S` restates a cubic whose first control point is the reflection of the previous one's
> second — the same curve in two fewer numbers, which is what `merge::snap_smooth_joins`
> arranges and what artists write (60% of their smooth cubic joins are exactly this). Tested
> on the *rounded* values, not the floats: those are what the reader gets, and a reflection
> that holds only before rounding would decode to a slightly different curve than the one
> that was fitted... Rounding does not commute with the reflection, so testing for equality
> there rejects joins that are exactly reflective in full precision; what matters is only
> that the curve the reader rebuilds is the curve that was fitted."

```rust
let r = |v: f64| {
    let m = 10f64.powi(decimals as i32);
    (v * m).round() / m
};
let (wx, wy) = (2.0 * r(pp3.x) - r(pc2.x), 2.0 * r(pp3.y) - r(pc2.y));
let tol = 10f64.powi(-(decimals as i32));
if (a.x - wx).abs() < tol && (a.y - wy).abs() < tol {
    // emit S ...
}
```

`prev_c2` carries `(pc2, pp3)` — the previous cubic's second control point and its endpoint —
*across edge boundaries* within a ring, since a ring is several edges' fits concatenated and
a smooth join can fall on the seam between two of them. It resets to `None` on any non-cubic
segment.

**Why the test must be done in rounded space**: the decoder reconstructs the reflected
control point as `2*r(pp3) - r(pc2)` from the numbers actually written to the file — the
already-rounded ones. In general `round(2p - c) != 2*round(p) - round(c)`: rounding does not
commute with the linear reflection. A join that is exactly reflective in full floating-point
precision can therefore decode to a *slightly different* curve once its endpoints are
rounded for output, and conversely a join that is only reflective after rounding — because the
rounding of `pp3` and `pc2` happened to land the reflection exactly on the rounded `c1` — would
be wrongly rejected by a test done on unrounded values. Testing against `wx, wy` computed from
the *rounded* `pp3, pc2` — and comparing them to the raw fitted `a.x, a.y` within one step of
the emitted grid (`tol = 10^-decimals`, i.e. 0.01 px at the default) — matches what the SVG
reader will actually reconstruct.

**`fmt_fitted`** (the stroke path) is the older, stricter form: it compares
`2*pp3 - pc2` against `c1` on raw floats, with a hard-coded Euclidean tolerance `5e-4`
(`emit.rs:76-77`). This has no stated derivation, and is a genuine divergence from `fmt_ring`
that was not carried over when `fmt_ring`'s test was upgraded.

**Provenance of the "60%" figure** — commit `81959fd`, "Offer the smooth cubic as a candidate,
opt-in": "Artists constrain these and the corpus says so plainly — of the joins between
consecutive cubics that are smooth to within a thousandth of a degree, 60% have equal handle
lengths either side, the ratio's median being exactly 1.00 with its whole interquartile range
at 1.00." The same commit records that the *fitter-side* producer, `merge::snap_smooth_joins`,
ships off by default behind `INKVEC_G1` (*research build*) because it does not pay on the objective ("objective
0.4005 -> 0.4019, fourteen icons better and seventy-four worse") even though it does what it
claims at the file level ("42 `S` commands over 48 icons, 82 fewer coordinates, 0.45% fewer
bytes... `svgmodel` counts geometric segments after normalising `S` away, so `ratio` moved by
exactly 0.00% while the files genuinely shrank"). The important distinction: `INKVEC_G1` (*research build*) gates
the *fitter's* deliberate production of reflective joins (stage 11); the emitter's `S`
*detection* here is unconditional — a curve that happens to come out smooth for any reason
gets the short form regardless of that flag.

### Compound paths: same-colour siblings, even-odd

`emit_level` (a nested function inside `emit_color`, `emit.rs:801-877`) replaces the older
one-face-per-element scheme. Its doc, `emit.rs:790-799`:

> "Emit the faces of one nesting level. Siblings are disjoint by construction — every pixel
> belongs to one face — so all the siblings of one flat colour can be one compound path under
> even-odd, which is how an artist draws them: a whole word is one `<path>`, not one per
> letter. Before this the tracer wrote one element per connected face and came back with 3.5x
> the artist's path count on a detailed wordmark at the same colour error. Primitives stay
> their own element and gradient faces each own a gradient, so neither merges. The merged
> path takes the first member's place in the paint order and its id; every member's children
> follow it."

The merge condition has three gates, no numeric threshold: a face must not be a primitive
element (`<circle>`/`<ellipse>`/`<rect>` never merge, since they cannot carry a compound
path); its fill string must start with `#` (a flat hex colour — a gradient's `url(#...)` fill
never merges, since each gradient face owns its own `<linearGradient>`/`<radialGradient>`);
and its fill *and* `fill-opacity` strings must match another sibling's exactly. Every emitted
`<path>` carries `fill-rule="evenodd"` unconditionally, merged or not.

**The measured figures are not in code — they are in commit history.** Commit `b13bed6`,
"Emit the pieces of one colour as one compound path":

> "The tracer wrote one `<path>` per connected face; an artist draws all the disconnected
> pieces of one colour as one compound path, so a word is one path and not one per letter. On
> a detailed wordmark that was 156 paths against the artist's 44 at the same colour error...
> Siblings are disjoint by construction, so the union is exact. The only pixels that change
> are the anti-aliasing seams between same-colour neighbours, which disappear: at most 0.007%
> of pixels, in components of 17 pixels or fewer, colour error equal or slightly better. Four
> detailed logos at 2048: 311 -> 45 paths (artist 434), 114 -> 27 (298), 227 -> 40 (173), 156
> -> 51 (44). Twenty brands at 1024: mean path ratio **1.85x -> 0.75x**, logos over 1.5x from
> nine to two. Gate PASS."

The **1.85x -> 0.75x** figure and the **1.5x** figure both come from this commit message —
neither is a comment in the current source. The "under 1.5x author paths" project rule is
recorded only in that commit narrative, not enforced as a hard gate: `bench/ci_gate.py`'s
`LIMITS` dictionary caps the parameter-vs-artist *ratio's regression* at 5% relative
(`"ratio": 0.05`), which is a change-detection gate, not an absolute 1.5x ceiling.

### Gradient and fill emission

Built in one loop in `emit_color` (`emit.rs:590-665`). A translucent face's flat colour is
"un-matted" before writing, since it was measured over the compositing matte
(`emit.rs:593-596`). An alpha *ramp* — a measured fade — is written as the gradient an editor
would use: one colour, two `stop-opacity` values, along the axis the fade was measured to run
(`emit.rs:620-629`); its endpoints are hard-coded to 2 decimals regardless of
`EMIT_DECIMALS`. Everything else — flat opaque colours and every genuine gradient — is
delegated to `gradient::fill_to_svg` (`crates/inkvec-trace/src/gradient.rs:1974`), which
returns a `(defs fragment, fill attribute)` pair: `FillModel::Flat` needs no defs at all;
`FillModel::Linear`/`Radial` write a `<linearGradient>`/`<radialGradient>` with
`gradientUnits="userSpaceOnUse"`, and an elliptical radial gradient carries a
`gradientTransform` built as translate-squash-rotate-translate back, "so it reads right to
left." No empty `<defs>` block is written when there is nothing to put in it.

### `--minify` and `--no-background`

`post.rs` composes three independent, optional rewrites in a fixed order
(`post.rs:153-158`): background knock-out, then minify, then margin.

`knock_out_background` (`post.rs:41-80`, for `--no-background`) removes the element that
paints the whole canvas — a `<rect>` spanning it, or a path whose outer ring visits all four
canvas corners — by **matching literal formatted text at exactly two decimals**
(`post.rs:42-52`). This is hard-coupled to `EMIT_DECIMALS == 2`: setting
`INKVEC_EMIT_DECIMALS` to anything else silently breaks `--no-background`, since the string
signature it searches for is built assuming 2-decimal formatting. It removes only the first
matching element; children of that face remain. Running before minify matters, since minify
strips the trailing zeros the matcher depends on.

`minify_svg` (`post.rs:83-130`, for `--minify`) strips every `id="..."` attribute, removes
now-empty `<g></g>` wrappers left behind by that removal, and strips trailing zeros from
decimal numbers (`12.50` -> `12.5`, `3.00` -> `3`, `-0.50` -> `-0.5`), guarding against
matching inside a hex colour (`#1428a0`) or a URL (`www.w3.org`) by requiring digits on both
sides of the dot before treating a token as a number. It does not touch whitespace between
attributes, shorten to relative path commands, or merge path data — "no ids or groups, no
trailing zeros. Same geometry, typically about a tenth smaller."

### viewBox, margin, and `fit_viewbox`

The emitted header is `<svg ... viewBox="-0.5 -0.5 {w} {h}" width="{w}" height="{h}">`. The
origin is `(-0.5, -0.5)` because coordinates are **pixel centres** — pixel `(0,0)`'s centre is
at `0,0`, so the canvas spans `-0.5` to `w-0.5`.

`post::retarget` (`post.rs:12-32`) rewrites only the root `width`/`height` (leaving `viewBox`
untouched) to make a document traced at one size render at another — used for `--max-dim`
downscaling. `post::with_margin` (`post.rs:137-151`) grows the viewBox by a fraction of the
larger side on every edge, via a single literal string replacement of the exact header text,
so it must run before nothing else has altered that text.

**`fit_viewbox` is not part of the Rust emitter.** It lives in the Python benchmark harness,
`bench/inkvec_bench/render.py:75`, and exists to make a non-square SVG render at a requested
square size without Lanczos-stretching it — its docstring records the regression this fixed:
"a 1984x400 logo asked for at 512x512 came back 512x104 and was then stretched 4.9x vertically
with Lanczos... The tracer then found a phantom ink in the soft ramps and the lettering came
out wrong (2026-09-05)." This function operates on already-emitted SVG text at benchmark time,
not inside the tracer itself.

### The `fills` and `emit` stage marks

Both stage marks are inside `run_color`, timed by a `Stopwatch`
(`crates/inkvec-trace/src/lib.rs:1241-1260`) that only prints under `INKVEC_TIMING`.

**`fills`** (`crates/inkvec-cli/src/lib.rs:1042-1065`) times the interval from `repair`
through the `--no-gradients` flattening pass and `demote_imperceptible_gradient`, plus the
mirror-symmetry reconciliation that runs immediately after repair
(`lib.rs:1009-1013`: "a dynamic program walks one boundary forwards and its reflection
backwards, and can segment them differently. So the fits are made to agree here, after
everyone else has finished"). Note the gradient *fitting* itself happens earlier, inside
`inkvec_trace`, and is charged to `trace_total`; what `fills` actually times is the
flattening/demotion decision plus the symmetry reconciliation.

**`emit`** (`lib.rs:1067-1179`) covers alpha-layer recovery and then **two competing
documents, priced against each other** (`lib.rs:1073-1079`):

> "Both forms of the document, costed against each other. A layer is only worth having if it
> says the same thing in fewer marks. It repaints the faces it covers with the ground and
> composites itself over them, so the image is identical either way and the whole decision is
> a parameter count — the same test every fill model and every arc has to pass."

The flat document is always emitted; if alpha layers were recovered, a second `layered`
document is also emitted, and the two are priced by literally counting shape-element
substrings and byte length in each rendered SVG string:

```rust
let pays = pl < pf && bl <= bf;  // fewer shapes AND not more bytes
```

Whichever wins becomes the output. `sw.mark("emit")` follows.

## Constants and thresholds

| name | file:line | value | controls | derivation |
|---|---|---|---|---|
| `EMIT_DECIMALS` | `emit.rs:54` | 2 | coordinate decimal places | fully derived — see Coordinate precision above |
| `MIN_RING_AREA` | `emit.rs:57` | 0.25 px² | smallest ring area worth emitting | motivated ("far below a pixel so that genuinely thin features survive: a sliver forty pixels long and a third of a pixel wide still encloses about 13px^2"); the specific 0.25 not swept |
| `S`-shorthand tolerance (`fmt_ring`) | `emit.rs:193` | `10^-decimals` (0.01 px default) | rounded-space reflection test | derived from the emitted grid itself |
| `S`-shorthand tolerance (`fmt_fitted`) | `emit.rs:77` | `5e-4`, raw-space Euclidean | stroke-path reflection test | no stated derivation |
| ring segment minimum | `emit.rs:252` | 2 | a ring is discarded below this | no stated derivation |
| path point minimum | `emit.rs:133` | 3 | `fmt_path` early return | degenerate-polygon guard |
| arc rotation precision | `emit.rs:105, 235, 291` | 3 decimals (fixed) | arc `phi` formatting | no stated derivation |
| rounded-rect radius floor | `emit.rs:305, 367` | `1e-4` | below this, written as a plain rect | no stated derivation |
| ellipse rotation floor | `emit.rs:344` | `1e-3` | below this, no `transform` written | no stated derivation |
| alpha-ramp endpoint precision | `emit.rs:625` | 2 decimals (fixed) | independent of `EMIT_DECIMALS` | no stated derivation |
| opacity precision | `emit.rs:626, 660, 905` | 3 decimals | `fill-opacity`/`stop-opacity` | no stated derivation |
| `INKVEC_EMIT_DECIMALS` | `emit.rs:126` | env override | overrides `EMIT_DECIMALS` | the mechanism used to isolate rounding from the segment price in the 7.2% measurement |
| background-match precision | `post.rs:42-52` | hard-coded 2 decimals | `--no-background` element matching | coupled to `EMIT_DECIMALS`, not derived independently — breaks silently if the two diverge |
| margin viewBox precision | `post.rs:147` | 2 decimals | `--margin` growth | no stated derivation |
| `ci_gate.py` ratio limit | `bench/ci_gate.py:33` | 5% relative | regression gate on parameter count vs artist | project's compactness regression budget, not the 1.5x absolute rule cited in commit history |

## Failure modes and edge cases

- **Zero-area rings are dropped outright.** The planar map can produce faces one pixel wide
  whose boundary walks out along a chain and straight back; these were being emitted as
  paths that "paint nothing at any resolution and cost coordinates, an id, and a line in the
  document a person has to read past" (`emit.rs:418-428`) — "twenty-seven of them on a plain
  green circle."
- **Rounding a rectangle's origin and size independently moves the far edge more than the
  near one.** The fix rounds the two *edges* and derives width as their difference
  (`emit.rs:357-360`): "how a rectangle centred exactly on a mirror came out a tenth of a
  pixel wider on one side than the other."
- **Punching a fitted ring instead of the primitive it was fitted from leaves a visible
  seam.** A transparent face's punch has to use the primitive, not the ring it was fitted to,
  "since the fitted ring it came from is a fraction of a pixel away" — measured on
  `material-icons/qr_code`, dE00 0.060 -> 0.164 (`emit.rs:276-279`).
- **Even-odd parity means punching a whole ancestor chain, not just the immediate parent.** A
  second ring inside an already-punched hole flips it back solid — measured on
  `lucide/book-heart`, dE00 0.19 -> 3.33 (`emit.rs:474-477`).
- **Containment for cutting must be certain, not merely likely.** `ring_inside`'s majority
  vote is right for paint-order stacking and wrong for deciding what to punch: a shape that
  merely runs alongside another can win the vote and get punched into a face it does not
  belong to — measured on `noto-emoji/emoji_u1f3cb_200d_2642`, dE00 0.58 -> 1.78
  (`emit.rs:478-484`). The fix requires every containment probe to agree, not a majority.
- **`fill-opacity` in a stacked document blends against whatever painted underneath it, not
  the page** — measured on `noto-emoji/emoji_u1f39e`, dE00 0.46 -> 1.43 (`emit.rs:496-502`).
- **A primitive element cannot carry a hole.** A face that must show a hole through it is
  written as a path even when its outline would otherwise fit a circle (`emit.rs:767-768`).
- **Drawing every face's own holes (making paths self-contained so any face could drop
  freely) was tried and reverted** — parameters rose and DISTS regressed on 3 of 6 probes,
  "because a ring contained inside another of the same face is not reliably a hole, and
  punching it makes one anyway" (`emit.rs:452-456`).
- **`--no-background` is a text match, and silently a no-op if it misses** — it returns the
  input unchanged when no element matches the expected two-decimal signature, including after
  a precision change or if run in the wrong order relative to minify.

## Environment overrides

Since the settings cleanup (CHANGELOG, *Unreleased*) the engine reads its environment through one helper (`inkvec_core::env`): a switch is off when unset, empty or `0`, and every variable is read once per process. Variables marked *removed* below are gone (their defaults are constants now); those marked *research build* are read only by a binary built with `--features research`. The full list, with what is left and why, is [`docs/internal/env-vars.md`](../internal/env-vars.md).

| variable | effect |
|---|---|
| `INKVEC_EMIT_DECIMALS` | overrides `EMIT_DECIMALS` (coordinate decimal places); breaks `--no-background`'s text matching if changed from 2 |
| `INKVEC_ALPHADBG` | per-face emit debug dump (rings, areas, outer/clear/parent/drop, holes) |
| `INKVEC_TIMING` | enables the `Stopwatch` printout that surfaces the `fills`/`emit` marks |

## Open questions

- **The module header's claim "coordinates are written to `--precision`" is stale.** The
  emitted digit count is controlled by `EMIT_DECIMALS`/`INKVEC_EMIT_DECIMALS` alone;
  `--precision` only sets the fitter's `lambda`. This was a deliberate separation (see the
  `ebc7534` commit message: "a finer price with the old rounding makes the corpus *worse*...
  so this is the emitter's bound and not the fitter's"), but the header comment was not
  updated to say so.
- **`fmt_fitted`'s `S`-shorthand test uses raw floats and a hard-coded `5e-4` Euclidean
  tolerance, while `fmt_ring`'s equivalent test was upgraded to rounded-space, per-axis
  comparison against `10^-decimals`.** The two paths (stroke output vs. face-ring output) now
  disagree on when a join is "smooth enough," and the `5e-4` figure carries no derivation.
- **The "1.85x -> 0.75x" compound-path figure and the "under 1.5x" project rule live only in
  a commit message (`b13bed6`), not in any code comment or enforced gate.** `ci_gate.py`
  enforces a 5%-relative regression budget on the parameter ratio, which is a different and
  weaker constraint than an absolute 1.5x ceiling.
- **`--no-background`'s background-face detection is a literal string match hard-coded to
  two decimals**, coupled to but not derived from `EMIT_DECIMALS`; changing
  `INKVEC_EMIT_DECIMALS` breaks it silently, with no error and no warning.
- **Several formatting-precision constants (arc rotation at 3 decimals, opacity at 3
  decimals, alpha-ramp endpoints hard-coded at 2 decimals independent of `EMIT_DECIMALS`,
  the rounded-rect radius floor `1e-4`, the ellipse rotation floor `1e-3`) have no stated
  derivation** and were not swept the way the headline coordinate precision was.
- **`MIN_RING_AREA = 0.25 px²` and the ring segment minimum of 2 are both motivated but not
  swept against the corpus** the way `EMIT_DECIMALS` was.
