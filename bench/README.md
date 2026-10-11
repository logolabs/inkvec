# bench

Everything used to measure Inkvec, in two layers:

- **The regression gate and dev loop** (`ci_gate.py`, `full_eval.py`, `quality.py`, and
  friends) — what you run locally before and after a change. Fast, real-icon corpus,
  no external dependencies beyond the workspace itself.
- **`inkvec_bench`** — the slower, broader harness that compares Inkvec against other
  raster-to-vector tracers on fidelity, structural economy and editability, not just
  pixels. This is what produced the cross-tracer numbers cited in the top-level README.

A benchmark claim is only as good as the benchmark behind it, and the specific claim
this project makes — *more editable output at equal or better fidelity* — is not one
any published pixel-only benchmark can check. Both layers exist to make that claim
falsifiable.

## Regression gate

```bash
python bench/ci_gate.py --exe target/release/inkvec
python bench/ci_gate.py --exe target/release/inkvec --conditions quality-512ss,fast-512ss  # a subset
python bench/ci_gate.py --exe target/release/inkvec --sample 0.25    # a quick read, never a verdict
python bench/ci_gate.py --exe target/release/inkvec --write-baseline   # re-baseline: a decision
python bench/ci_gate.py --exe target/release/inkvec --bypass-gate "reason"  # explicit, justified override
python tools/prepush.py --gate                                       # build, hard cases, gate
```

Scores the committed 246-icon screen set (under `bench/data`, so this runs from a bare
checkout) under eight conditions: Quality and Fast, each at 128 px (`128ss`), 512 px
(`512ss`), 512 px flattened onto white (`512ssop`, an opaque logo) and `web`, the logo as it
usually reaches a tracer: on white, resized to 400 px (bicubic) and saved as a JPEG at
quality 80 with 4:2:0 chroma (`bench/build_web_tier.py`; the JPEG files are committed, so
every machine reads the same bytes). The first three are exact renders; `web` carries the
blur, ringing, block artefacts and half-resolution colour of real inputs. Each condition's
four gated axes are compared icon by icon with the per-platform baseline
`bench/gate/baselines/<os>-<arch>.json`:

| axis | aggregate | margin at the one-sided 95 % upper bound |
|---|---|---|
| dE00 (colour error vs. the artist's file) | family-macro mean | 1 % |
| turning gap (distance from the artist's control-polygon turning, per canvas side; `ci_gate.with_gaps`, `inkvec_bench/turning.py`) | family-macro mean | 2 % |
| parameter gap (absolute log of parameters over the artist's: twice and half the artist's count are equally far) | family-macro mean | 3 % |
| geom (mean edge displacement from the artist's file, input px; `inkvec_bench/geomatch.py`) | family-macro mean | 2 % |

`geom` compares the trace with the artist's file as geometry: both drawn flat (no
anti-aliasing) at four times the input's size, every pixel given the nearest of the
artist's colours, and the area where they disagree divided by the length of the artist's
edges. That is the mean distance between the two files' edges: an edge moved by `δ` reads
`δ` in any colour, where dE00 reads the colour contrast as much as the displacement
(`bench/theory/geom_calibration.py`: a disc grown by 0.1 px reads geom 0.097 in dark red
and in pale grey, dE00 0.021 and 0.0018). `geom_far`, the part of it more than an input
pixel from any artist edge (missing or extra features), and `self_res` are reported.

The change of each aggregate gets a paired, family-stratified bootstrap interval
(`bench/gate_stats.py`). The gate passes an axis when the interval's one-sided upper bound
is below the margin, or, where the 246 icons cannot resolve the margin (a broad edit at
512 px), below the minimum detectable effect; that pass is reported as `within-noise`.
There is no "inconclusive" verdict: the margin is floored at what the set can detect, so a
change fails only when it is worse by more than the margin and the set can see it. Icons whose SVG is
byte-identical to the baseline's keep its numbers, and are not scored again at all: the gate
hashes each SVG as soon as it is traced and skips the renders when the hash is the baseline's
(`--rescore` scores them anyway, to check the scorer). A baseline moves only by
`--write-baseline` or, locally, on a demonstrable gain. Until a platform's baseline file is
committed, the gate falls back to the old scalar rule against `bench/gate/baseline.json`
on quality-128ss. CI uploads every run as the `gate-baseline` artifact, in the format to
commit. The scorer is a pure function of the SVGs: the reference renders are the same
8-bit images whether the cache is warm or cold.

### What a run costs

Each condition's console line says where the CPU went, per icon: the tracer, the gate's
signals and the design battery (below), and how many icons were byte-identical to the
baseline. What the artist's file contributes is computed once and kept in
`bench/data/_cache`, keyed by the file's hash and the renderer's and libraries' versions:
the 1024 px reference render (`gt1024/`), the artist's side of `geom` (`geomatch/`: the
flat render's palette, labels, edge length and near-edge mask, at each size and page) and
its design profile (`design/`). dE00 converts and compares only the sampled pixels where
the two renders differ (a pixel equal in both has a CIEDE2000 of exactly 0), and the flat
renders are labelled run by run; both give bit-identical numbers. One process pool serves
every condition. On the full set (246 icons, `--workers 2`, CPU per icon):

| condition | before | every icon scored, cold cache | every icon scored, warm cache | a normal run against the baseline |
|---|---|---|---|---|
| quality-512ssop | 3.99 s | 2.84 s | 2.15 s | 1.43 s (246 identical) |
| fast-web | 1.75 s | 1.03 s | 0.64 s | 0.35 s (133 identical) |
| quality-web | 3.0 s (30-icon sample) | | | 1.53 s (246 identical) |

The tracer itself is 1.41 s of quality-512ssop's and 0.05 s of fast-web's; the gate's own
scoring of a changed 512 px icon fell from 2.58 to 0.72 s (the artist's flat render and
labels, about 1.1 s, now come off the cache; labelling the trace's flat render 0.47 to
0.04 s; dE00 0.14 to about 0.04 s), and the design battery adds about 10 ms. Every gate
number is unchanged, bit for bit: run old and new on the same binary (2026-10-10, both
conditions above, cold and warm), all 246 per-icon rows, every summary and every verdict
were identical. What is left is the tracer, and the resvg renders of the trace (its flat
render at four times the input's size is about 0.4 s at 512 px, most of it the PNG
encoding `resvg_py` does on the way out, which it offers no way around).

## Human statistics: how a trace is drawn

```bash
python bench/human_stats.py --exe target/release/inkvec                     # quality-512ssop and quality-web
python bench/human_stats.py --exe target/release/inkvec --extra-args "--editability" --base-args ""
python bench/human_stats.py --exe new/inkvec --base-exe old/inkvec --conditions fast-web
python bench/human_stats.py --from-report gate/report.json --correlate      # a gate run's numbers
```

The gate's axes ask whether a trace looks like the artist's drawing and is about as long;
none asks whether it is *built* like it. `inkvec_bench/design.py` reads a file once (paths,
primitives, transforms and inherited paint, mapped to the canvas the gate renders) and
takes a battery of statistics of its construction: the segment-kind mix, lines and handles
on the axes, smooth joins and nodes at the extrema, kinked joins, corner angles, curve
sweeps, handle lengths, segment lengths, coordinates on the artist's design grid, decimals,
repeated values, mirror symmetry, primitives, strokes, colours, nested subpaths and stacked
shapes. Each is read on the trace and on the artist's own file, and the **divergence**, the
per-icon distance between the two (a share's difference, a count's log ratio, a
distribution's Wasserstein-1, a mix's total variation), is zero for a trace drawn as the
artist drew it, whatever the family's style. The module's documentation defines each one.
`halfpx`, the share of numbers on the raster's half-pixel grid, is an artefact signal (pixel
snapping), reported but left out of the composite.

The statistics are reported, not gated. The gate takes them on every icon it traces (about
10 ms each; the artist's side is cached), prints each one's family-macro mean divergence
per condition, and writes every icon's values to `--report-json` (`human`) and to
`--artifact-dir` (`human-<platform>.json`). `bench/human_stats.py` traces without
rendering (the tracer's time plus the battery) and prints, per family, the artist's mean,
the trace's mean and the divergence of each statistic, then the oddities: the icons
farthest from their artist on each statistic and overall. With `--base-exe` or
`--base-args` it traces a second arm and compares the two icon by icon: each statistic's
family-macro divergence before and after, a paired, family-stratified bootstrap interval of
the change (`gate_stats.compare`, as the gate does), how many icons moved closer and
further, and one composite, the geometric mean of the after/before ratios, to tune against
(below 1 is closer to the artist). Traces are kept under `bench/data/_cache/traces`, keyed
by the executable's bytes and the flags, so an arm already traced costs nothing.

The battery is non-redundant by construction: `--correlate` computes the Spearman
correlation of the per-icon divergences (pooled over the conditions, with the gate's own
axes when known) and drops a statistic correlated beyond |rho| 0.7 with one kept before it;
`design.KEPT` is what survived on quality-512ssop, quality-web and fast-web.

On the 246 icons of quality-512ssop and quality-web pooled, seven of the 36 candidates went:
`grid_int` (rho 0.98 with `grid_half`), `grid_level` (0.72) and `decimals` (0.74) with it,
`wide_curves` with `sweep` (0.72), `length_gini` with `segment_lengths` (0.79), `palette`
with `colours` (0.84), and `evenodd`, nonzero on 6 of 616 icon-conditions. The 29 kept
statistics stay below |rho| 0.7 with each other (largest: `segment_lengths` with
`nodes_per_subpath`, 0.68) and below 0.57 with every gate axis (largest: `colours` with
dE00, `smooth` with geom), so none repeats what the gate already reads.

## Hard cases

```bash
python bench/cases.py --exe target/release/inkvec                    # 42 isolated cases
python bench/cases.py --exe target/release/inkvec -- --mode fast     # tracer flags after --
python bench/cases.py --exe target/release/inkvec --ratchet quality  # CI: hold the passing set
```

One property per case, against geometry written by hand. Cases may fail on the day they
are written; CI holds the *set* that passes (`cases:quality`, `cases:fast` in
`bench/quality_budget.json`): a case that passed and now fails fails the build.

## Full evaluation

```bash
python bench/full_eval.py target/release/inkvec --set screen
python bench/full_eval.py target/release/inkvec --set all --sample 0.1     # a reproducible tenth
python bench/full_eval.py A.exe --compare B.exe --set held_a              # paired before/after
```

Scores a devset split (`dev`, `held_a`, `held_b`, `full`, `screen`, or `all`) in parallel
and prints a per-family table (dE00, DISTS, parameter ratio, self-residual, turning,
mirror error) plus the worst individual icons by dE00. Uses the same scoring code as the
evolve loop and the regression gate (`bench/svgeval.py`), judged at 1024px against each
icon's ground-truth SVG.
Writes `bench/data/eval_<label>.json` — per-icon numbers, not just the aggregate.

`bench/devset.json` / `bench/devset_v2.json` define the splits; `bench/gt_diff.py`,
`bench/complexity.py`, `bench/ablate.py` and `bench/sweep.py` build on the same
`svgeval` scoring to answer narrower questions (per-face diffs against ground truth,
case complexity, per-stage cost/benefit, and parameter sweeps, respectively).

## Code quality ratchet

```bash
python bench/quality.py             # check; exit 1 on regression
python bench/quality.py --report    # every metric, no gate
python bench/quality.py --update    # re-baseline, after a deliberate decision
python bench/quality.py --coverage  # ... plus per-crate line-coverage floors (CI)
python bench/quality.py --mutants   # ... plus sampled mutation floors (weekly, hours)
```

Coverage and mutation floors are committed (`coverage:*`, `mutation:*`); a floor that
was never recorded fails. Mutation floors are sampled (every 32nd mutant of the engine's
core module trees), so they are recorded at a lower confidence bound and fail only when a
run's upper bound falls below them.

Does not analyse anything itself — `cargo clippy` and `cargo fmt` do that, configured
under `[workspace.lints]` in the workspace `Cargo.toml`. This wraps `cargo clippy
--message-format=json` and holds a budget (`bench/quality_budget.json`) that can only
fall, never rise, so a warning count is paid down deliberately rather than grandfathered
forever.

## `inkvec_bench`: comparing against other tracers

The broader harness. It measures fidelity the way the numbers above do, plus two axes
no published raster-to-vector benchmark scores at all:

| Axis | Metrics | Why |
|---|---|---|
| **Fidelity** | DISTS, LPIPS, SSIM, PSNR at 1x / 4x / 16x; dE00 mean & p95 | Multi-scale is the point. An SVG is resolution-independent, so scoring only at input resolution rewards over-fitting to that resolution's anti-aliasing. Boundary errors are near-invisible at 1x and obvious at 4x. |
| **Structure** | `n_params`, `n_anchors`, `anchor_density`, segment-type mix, `param_ratio` vs. ground truth | `anchor_density` normalises by boundary length so a genuinely complex logo isn't penalised for being complex. |
| **Editability** | `primitive_fraction`, `primitive_recall`, seam area, overdraw ratio, self-intersections, layer-tree depth & naming, `edit_spread` / `edit_tear` | What separates a document a designer can work with from a bag of anchor points that happens to render correctly. |
| **Robustness** | parameter growth under noise / JPEG / boundary warp | Reproduces the protocol from the AnchorFlow paper (see History) — a tracer that blows up its parameter count on a slightly noisy input has not actually solved vectorization. |
| **Cost** | gzipped size, runtime | |

### The seam/overdraw pair

The two structural metrics worth understanding before reading any results. When each
region is stored as an independent closed path, these trade against each other and
neither can be driven to zero:

- lay regions edge-to-edge and rounding disagreement between the two copies of each
  shared boundary opens hairline **seams**;
- overlap them generously and the seams close, but geometry is drawn twice —
  **overdraw** — so every shared boundary now exists in two places and editing it means
  editing both.

A planar map with genuinely shared edges escapes the trade: seam ~= 0 at overdraw ~= 1.
That escape is one of Inkvec's central architectural bets, so the harness plots the
plane rather than either axis alone.

### Protocol: curves, not points

Every tracer has a quality knob, so a single (fidelity, complexity) pair says as much
about which setting was picked as about the engine. Each runner declares a *grid* of
settings spanning its own quality range, and results are reported as the **Pareto
frontier** per engine — at equal parameter budget, who is more accurate, across the
whole budget range.

### Corpora

**Synthetic** (default, offline, exact ground truth). Procedurally generated SVGs where
the structural intent is known, not just the pixels: `prim_*` (does the tracer return a
`<circle>`, or 40 anchors?), `mosaic_*` (adjacent regions sharing boundary vertices
exactly — the seam/overdraw probe), `symmetry_*` (exact mirror and k-fold rotational
compositions built with `<use>`), `gradient_*` (single linear/radial gradients),
`thin_features` (strokes narrower than one pixel at the small tiers), and
`rings_concentric` / `stack_overlap` / `logo_like` (occlusion, alpha, realistic
composition). Rendered at 32 / 64 / 128 / 256 / 512 px — the small tiers matter because a
32px anti-aliased icon carries its boundary signal almost entirely in partially-covered
pixels, which is exactly what a thresholding tracer discards first.

**Bring your own.** `--svg-dir` for other ground-truth SVG sets (Twemoji, Noto Emoji,
Fluent Emoji, Material, FluentUI, VectorGym); `--image-dir` for in-the-wild rasters with
no ground truth, where fidelity is only meaningful at 1x but every structural and
editability metric still applies.

### Baselines

| Runner | Status | Role |
|---|---|---|
| `vtracer` | local, MIT | The incumbent OSS colour tracer and primary baseline. The Python bindings expose the three knobs the CLI hides (`corner_threshold`, `length_threshold`, `splice_threshold`), which is what makes a proper sweep possible. |
| `potrace` | local, via pure-Python `potracer` | Bilevel control. Its optimal-polygon DP is the anchor-economy technique VTracer dropped for speed, so it marks what that cost. |
| `vectorizer_ai` | remote, **paid**, opt-in only | The commercial ceiling. |

Potrace itself is GPL and is **not** vendored; `potracer` is a clean-room Python port.

### Usage

```bash
pip install -e bench[baselines,perceptual]

inkvec-bench status                       # what's installed and usable
inkvec-bench build                        # generate corpus, render tiers
inkvec-bench run --tiers 64 128           # sweep runners x settings
inkvec-bench report                       # Pareto plots + HTML
```

Robustness protocol:

```bash
inkvec-bench run --perturb noise jpeg boundary
```

**Against the commercial ceiling — this uploads your corpus to a third party**, and
`production` mode **spends real credits**:

```bash
export VECTORIZER_AI_ID=... VECTORIZER_AI_SECRET=...
inkvec-bench run --runners vtracer vectorizer_ai --vectorizer-ai-mode production
```

`test` mode is free but watermarked, which corrupts every pixel metric; results from it
are tagged so they can never be silently mixed into a comparison.

## History

`inkvec_bench` was built before Inkvec's own tracing code existed, as the yardstick the
project set out to beat — deliberately, since a benchmark chosen after the fact tends to
flatter whatever was built. Its structural-economy and robustness protocols reproduce
those of two published works: **AnchorFlow** (arXiv 2605.19551, May 2026) and **AdaVec**
(Zhao et al., 2025) — see `docs/DESIGN.md` for the full citations. AnchorFlow's own
reported robustness bar under noise/JPEG/boundary-warp perturbation was AnchorFlow
+2.9%, AdaVec +20.7%, VTracer +106.7% parameter growth; `n_params` in the structure table
is directly comparable to AnchorFlow's reported figures (61.2 vs. VTracer's 206.4). These
are the published baselines the harness was built to reproduce, not numbers re-measured
for this release — treat them as context for why the harness is shaped the way it is,
not as a live comparison.

## Notes

- Rendering goes through **resvg**, deliberately: it is the same renderer family the
  Rust core uses, so harness measurements and engine self-checks agree. `cairosvg` is
  not used — it needs a native cairo DLL that is absent on typical Windows installs.
- Perceptual metrics (LPIPS, DISTS) need torch and are optional; everything else
  degrades gracefully without them.
- A cell that fails is recorded as a row with an `error` field rather than aborting the
  sweep. A tracer that crashes on 3% of inputs has told us something worth keeping.
- Structural and editability metrics are Python for now. They are pure geometry and
  could move into `inkvec-core` (Rust) so the engine can use them as internal invariants
  rather than only as external scoring.
