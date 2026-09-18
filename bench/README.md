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
python bench/ci_gate.py --exe target/release/inkvec --write-baseline   # re-baseline after a deliberate change
python bench/ci_gate.py --exe target/release/inkvec --bypass-gate "reason"  # explicit, justified override
```

Scores the committed 246-icon screen set (1.6 MB under `bench/data`, so this runs from a
bare checkout) and compares three numbers against `bench/gate/baseline.json`:

| metric | may rise by at most |
|---|---|
| dE00 (colour error vs. the artist's file) | 1% |
| turning (anchor turning per unit length) | 1% |
| parameter ratio vs. the artist's file | 5% |

A regression on any of the three fails the gate. An improvement updates the baseline
downward automatically — ground gained is never given back. This is the CI-facing check;
run it before opening a PR that touches tracing behaviour.

## Full evaluation

```bash
python bench/full_eval.py target/release/inkvec --set screen
python bench/full_eval.py target/release/inkvec --set all --sample 0.1     # a reproducible tenth
python bench/full_eval.py A.exe --compare B.exe --set held_a              # paired before/after
```

Scores a devset split (`dev`, `held_a`, `held_b`, `full`, `screen`, or `all`) in parallel
and prints a per-family table (dE00, DISTS, parameter ratio, self-residual, turning,
mirror error) plus the worst individual icons by dE00. Uses the same scoring code as the
evolve loop (`evolve/svgeval.py`), judged at 1024px against each icon's ground-truth SVG.
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
```

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
