# What Trazor does that Inkvec could take

A read of [`PhenX/Trazor`](https://github.com/PhenX/Trazor) (commit `d5dd561`) against this codebase, to
find ideas worth importing. Trazor is already one of the engines in the README's comparison table, so
this is not a "should we care about it" question — it is a "what did they build that we did not"
question.

Nothing here is a criticism of the core tracer. Trazor's fidelity on the 21-case set is dE00 0.518 at
2.6× our coordinates against our 0.132 at 1×, and the reason is architectural: Trazor is a
**Potrace-class chain** (straightness DP → optimal polygon → least-squares vertex adjustment →
α<sub>max</sub> corner analysis → curve-run merging), applied per colour layer. We are a coverage
inversion, a planar map, a global analysis-by-synthesis boundary solve and an MDL multi-model fit.
Their curve stage cannot beat ours and they know it — their own quality plan
(`plans/vectorization-quality.md`) lists sub-pixel boundaries as "not started (large)" and
fidelity-driven refinement as blocked on not having a deterministic in-engine rasterizer, which is a
thing we already built.

So the value is at the edges: the **serializer**, the **result contract**, one **gradient model** we
do not have, and a **domain** (cut vinyl, plotters, stencils) we do not serve.

---

## Tier 1 — clear wins, low risk

### 1. Shortest-form path-data encoding (`crates/inkvec-cli/src/emit.rs`)

**What they do.** `packages/svg/src/optimize.ts` emits, for every command, the absolute form, the
relative form and (for axis-aligned lines) the `H`/`V` shorthand, measures all three, and renders the
shortest. Leading zeros are dropped (`0.5` → `.5`), separators are omitted where the SVG grammar
allows, and a repeated command letter is elided.

The load-bearing detail is how they avoid drift: **coordinates are quantized to the output grid
(`10^precision` units) once, and every relative delta is an integer difference on that grid**, so a
reader's running sum of deltas reconstructs each absolute position exactly. Naive relative encoding at
two decimals accumulates rounding along a path; theirs cannot.

**What we do today.** `fmt_fitted` and `fmt_ring` write absolute `M`/`L`/`C`/`S`/`A`/`Z` only, with
explicit commas, always. We already do the clever part of this family — `S` for a smooth cubic join,
tested on the *rounded* values so the reader reconstructs the curve we fitted — but nothing else.
`--minify` is documented as "no ids, no groups, no trailing zeros".

**What it is worth.** Measured, not estimated. I re-encoded the `d` attributes of
`docs/assets/github-hero-example.svg` (our own output) and the synthetic corpus files, verifying each
re-encoding decodes to an identical command list on the 2-decimal grid:

| file | raw | absolute + trailing-zero strip | shortest form |
|---|---|---|---|
| `github-hero-example.svg` | 6,595 B | 6,303 B (−4.4%) | 4,892 B (−25.8% raw, **−22.4% vs minify**) |
| synthetic corpus (8 files) | 5,026 B | 3,993 B | 3,626 B (−9.2% vs minify) |
| all 9 | 11,621 B | 10,296 B (−11.4%) | 8,518 B (−26.7% raw, **−17.3% vs minify**) |

Only the first row is our own output; the corpus files are the artists' ground-truth SVGs, re-encoded
at each file's own precision, and are there to show the technique is not specific to how we happen to
write numbers. Trazor measured the same workstream at **−35% path data, −24% whole document** on
their fixtures.
Both numbers say the same thing: this is several times larger than what `--minify` currently earns,
and the geometry is bit-identical on the emitted grid.

**Also worth copying: their safety net.** `npm run test:render` traces the bundled samples in
Chromium and pixel-diffs optimized against baseline output. For a change that rewrites every `d`
string in the repo, a render-equivalence gate is the right guard, and their measured result
("maxΔ=1 level on 0.000% of pixels") is what passing looks like.

Effort: contained — one function in `emit.rs` plus a decoder-equivalence test. Risk: entirely in the
grammar edge cases (`.5` after an integer is one number, after `1.5` it is two; arc flags are single
digits that may be glued to the next number), which is exactly what a round-trip test catches.

### 2. A result contract: fidelity number + typed warnings

**What they do.** Every `VectorizeResult` carries `stats` (path/node/colour counts, bytes, per-stage
timings) and a typed `warnings` array — `stencil-islands`, `node-count`, `empty-result`,
`palette-clamped`, `tiny-features`, `centerline-input`, `gradient-spot-color`, `mode-note` — each with
a severity, an English message and machine-readable `params` so a UI can localize from the code. They
also re-rasterize every result and report a mean Oklab ΔE against the source ("honest fidelity
scoring"), and `@trazor/svg`'s `analyzeSvg` will do the same counting on foreign SVG text.

**What we do today.** `crates/inkvec-trace/src/diag.rs` is better than anything they have for
*developers* — the saturation register, born from four defects that were all "a quantity silently
resting against a limit", is a genuinely good idea and I would not trade it. But it is `INKVEC_DIAG=1`
on stderr. A caller of `trace_image`, the npm package, or the wasm demo gets no structured statement
of how the trace went, and no fidelity number at all.

**Why it is cheap for us.** We already render the model and compare it to the pixels — that is what
the Stage 08 boundary solve *is*, and `harmonize.rs` rasterizes to compare masks. Surfacing the
achieved dE00 alongside the SVG is mostly plumbing, and it turns "trace and hope" into "trace and
know". The warning codes worth having are ours, not theirs: palette clamped against `--colors`, a face
count or node count far above the corpus norm, a region whose gradient fit was declined, a stroke
candidate that failed the constant-width test and fell back to a filled outline.

The asymmetry is worth stating plainly: their workstream E (fidelity-driven refinement) is *blocked*
on building a deterministic in-engine rasterizer. We have one. If we ever want a refinement loop, the
expensive prerequisite is already paid for.

---

## Tier 2 — one real algorithmic gap, and two smaller ones

### 3. Gradients: decomposing a semi-transparent overlay over a ramp

**What they do.** `packages/raster/src/gradient.ts` handles the case where a band's pixels are a
*composite* of two layers — a sun glow, a vignette or a shadow lying across a sky ramp. That
composite is a 2-D colour field that no single linear or radial gradient can paint. They explain it as
**an opacity gradient stacked over an underlay of the ramp**: two 1-D models instead of one 2-D one.
They also keep a specific guard for the case where this fails — if a visible residual is a *function
of the cross-axis position* (a ring or a spot painted as a band), the fit is rejected as a 2-D field
rather than a ramp.

**What we do today.** Our gradient model selection (`crates/inkvec-trace/src/gradient.rs`) is stronger
than theirs in every respect that concerns a *single* ramp: MDL in editable numbers, both sRGB and
linear-light interpolation fitted with the space as a model parameter, an 8-bit quantized-Gaussian
likelihood with a half-LSB dead zone, a bimodal margin that declines a step, `MIN_RAMP_SUPPORT`
against fitting a feature. `gradient/svg.rs` `fade_to_svg` already emits `stop-color` +
`stop-opacity` in one gradient — but that path is driven by the **source's own alpha**
(`crates/inkvec-cli/src/alpha.rs`). An *opaque* input that happens to depict a glow over a ramp is,
per `docs/LIMITATIONS.md`, quantised into bands.

**Why it fits us.** This is not a new subsystem — it is one more entry in the model alphabet, priced
by the MDL machinery already in place: `PARAMS_LINEAR + PARAMS_RADIAL + stops` for the pair against
the sum of the flat bands it replaces. The renderer already knows how to write both halves. And the
cross-axis residual test has no counterpart in our code (nothing in `gradient.rs` or `gradient/`
tests whether a residual correlates with position across the gradient axis) — it is a cheap guard
worth having on its own merits, independent of the overlay model.

### 4. Band-merge screening (performance only — we already do the quality part)

Worth recording accurately because the headline is misleading. Trazor merges posterized bands
**agglomeratively**, closest-union-first, on **connected components of a label** rather than labels
(so a sky band and a hill top sharing one palette colour are not dragged together). We do exactly
that already — `gradient/bands.rs` splits into 4-connected components first, then greedily takes the
pair with the largest saving, caches union fits, and recomputes only those touching the merged pair.

The one thing they have that we do not is a **closed-form O(1) screen** before any pixel pass: per-label
moment sums give the linear axis as the leading eigenvector of the covariance-normalized colour-gradient
scatter, its eigenvalue ratio as the ramp's "1-D-ness", and the radial centre from an isotropic
quadratic fit (colour linear in r²). Only screened pairs pay for a pixel-level union fit. We fit every
adjacent pair — `O(B²)` fits, each linear in the pixels it covers.

So: no quality change, a possible dent in the ~1.2 s per graphic on gradient-heavy inputs. Low
priority unless band merging shows up in a profile.

### 5. Markerless thin-feature rescue

In their region-growing segmenter (`packages/raster/src/segment.ts`), a feature too thin to hold a
flat interior — a hairline glyph like an "&", a fur stroke, a contour line — gets no marker and would
be swallowed by the flood. `rescueMarkerlessFeatures` rescues each such blob **that is a colour
extreme between its sides** as its own marker.

We arrive at thin features differently (coverage inversion, not watershed), but the failure mode is
adjacent to a limitation we already publish: sub-pixel gaps whose anti-aliasing ramps overlap get
merged into one face. The transferable piece is the *test*, not the mechanism: a thin run is real if
its colour is a local extremum along the profile crossing it, rather than a monotone step between its
two sides. That is a cheap thing to evaluate against our coverage field, and `thin_features` is
already in the synthetic corpus to check it against.

---

## Tier 3 — product surface, not algorithm

### 6. A fixed / exact palette path

Trazor's quantizer has exact- and fixed-palette paths, and `@trazor/assist`'s `suggestPalettes`
derives candidate palettes from image statistics.

We have nothing equivalent, and the README's third persona is *"a brand team that needs the logo
exact"* — told, correctly, that "the colours written are the ones measured in the image, so whatever a
JPEG or a screenshot did to them comes along — check them against your brand values." A
`--palette "#rrggbb,…"` that constrains palette extraction to supplied inks and reports the measured
ΔE per ink would close that by construction and hand the user the check we currently ask them to do by
hand. This is a correctness feature for that persona, not a tolerance knob, so it does not conflict
with the no-knobs-because-MDL position.

### 7. The manufacturing domain (vinyl, laser, plotter, stencil)

Trazor's `TARGET_PROFILES` (illustration, logo, vinyl cut, laser, pen plotter, stencil) come with mm
units, gap-fill strokes, and cut-aware layering. Their `stacked` mode is worth reading even if we take
none of it: the most *connective* colour — the one whose regions carry the largest total perimeter — is
pinned as the full-silhouette base sheet, and a pocket buried two or more sheets below its surround (a
base-coloured pupil under the eye white and the face) is relabeled into its surround for the solid
layers and repainted on top as its own island layer, so the lower sheets stay whole instead of each
carrying a floating hole that would drift out of alignment on the cutter. Because a colour can then
recur, their grouped output groups by paint *layer*, not by colour.

That is a real manufacturing insight and it is entirely outside our current scope. Noting it because
our centerline tracer is *stronger* than theirs (see below), which makes the plotter/engraver audience
reachable if we ever want it — and because "stacked" means something different in their build than in
ours.

### 8. Incremental re-fit, if there is ever an interactive path

Their curve chain is split exactly where the settings first matter: `ringPolygon` (straightness DP +
vertex adjustment — a function of the ring and the optional coverage field alone) and
`polygonToCommands` (smoothing, curve optimization, corner threshold). The engine's `StageCache` keys
each stage by the settings that shape it, so changing a smoothing parameter replays the fit **without
re-running the straightness DP**. Their helper-worker parallelism picks its unit per mode with one
invariant — the unit's result must be a function of shared immutable state — and places results
strictly by unit index so a parallel run is byte-identical to a sequential one. Their choice of unit in
bw mode is instructive: a *ring*, not a shape, because one ink silhouette routinely carries most of the
rings in an image and a shape-sized unit would leave the whole run waiting on it.

We are a batch tool at ~1.2 s per graphic and rayon already covers the parallel half. But
`crates/inkvec-wasm` plus `web/` is an interactive surface, and if a "re-trace with one thing changed"
path is ever wanted, this split — and the by-index determinism invariant — is the design to copy
rather than reinvent.

---

## Explicitly not worth importing

| Their thing | Why ours stays |
|---|---|
| Sub-pixel boundaries (`refine.ts`) | They snap polygon vertices onto a coverage zero-contour **after** the lattice polygon's indices are chosen. We invert the coverage physically and then run a global analysis-by-synthesis boundary solve. Their own plan calls sub-pixel "not started (large)". |
| Seam-free partition (`boundary.ts`) | Fit-once shared chains with pinned junctions — the same guarantee our half-edge DCEL planar map gives. Worth reading as a cross-check, not as a replacement. |
| Potrace curve chain | Tolerance-driven segment counts. Ours is an MDL multi-model DP with primitives and arcs; the whole point is that the node count is *derived*, not set. |
| Centerline | Theirs fits whatever the skeleton gives. Ours promotes a region to a stroke only after an aspect test, a constant-width test against the coverage-derived positional sigma, and an area accounting — and returns it for filled tracing when it fails. Keep ours. |
| `@trazor/tune` (objective-weighted settings search) | This is the problem MDL exists to not have. A search over `fidelity / simplicity / fileSize / colorEconomy / cleanliness` weights is a way of asking the user to choose what the description length already decides. |
| `@trazor/assist` `recommendSettings` | Same reason, plus our intake already measures `intake_scale` and `oversample_factor` and adapts λ, min-area and precision from them. |

---

## Suggested order

1. **Shortest-form path encoding** under `--minify`, with a decoder-equivalence test and a rendered
   pixel-diff gate. Measured −17% to −22% on top of today's minify, no geometry change.
2. **Fidelity + warnings in the result**, surfaced through the library API, the npm package and the
   wasm demo. The rasterizer is already there.
3. **Overlay-gradient model** in the MDL alphabet, plus the cross-axis residual guard.
4. **`--palette`** for the brand persona.
5. Thin-feature extremum test as a corpus experiment on `thin_features`.
6. Band-merge screening only if it shows in a profile.
