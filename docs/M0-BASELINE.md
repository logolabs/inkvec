# M0 — Measured baseline

**Run:** 720 cells, 0 failures. 20 synthetic ground-truth inputs at the 128px tier,
2 engines, 18 settings each, clean + boundary-perturbed variants.
Reproduce with `inkvec-bench --tag m0 run --tiers 128 --perturb boundary`.

This is the number M1 has to beat. Everything here is measured, not asserted, and every
figure below is regenerable from the harness.

---

## 1. Parameter economy: median 5.9× worse than ground truth

VTracer at its **best DISTS@4x setting per input**, against the SVG the raster was
rendered from:

| input | GT params | VTracer params | blow-up | DISTS@4x | overdraw |
|---|---:|---:|---:|---:|---:|
| gradient_linear | 12 | 534 | **44.5×** | 0.120 | 2.33 |
| prim_roundrect | 12 | 300 | 25.0× | 0.027 | 1.45 |
| stack_overlap | 15 | 366 | 24.4× | 0.071 | 1.87 |
| rings_concentric | 36 | 864 | 24.0× | 0.033 | **6.02** |
| logo_like | 24 | 432 | 18.0× | 0.044 | 2.88 |
| gradient_radial | 9 | 120 | 13.3× | 0.097 | 2.02 |
| mosaic_pie12 | 114 | 888 | 7.8× | 0.062 | 3.61 |
| prim_circle | 9 | 66 | 7.3× | 0.008 | 1.41 |
| mosaic_pie6 | 60 | 390 | 6.5× | 0.043 | 3.33 |
| symmetry_rot8 | 66 | 348 | 5.3× | 0.109 | 1.33 |
| … | | | | | |
| thin_features | 66 | 48 | **0.7×** | **0.290** | 1.02 |

**Median blow-up: 5.9×.** The worst cases are exactly the four deficits from
`DESIGN.md` §3, and they are separable:

- **gradients** (44.5×, 13.3×) — banded into discrete layers rather than fitted.
- **primitives** (25.0× roundrect, 7.3× circle) — no primitive support.
- **stacking** (6.02× overdraw on concentric rings) — geometry drawn six times over.
- **sub-pixel** (thin_features) — see §3; the one case where *fewer* parameters is
  the failure, not the win.

## 2. Primitive recall: 0.00 on 20 of 20 inputs

VTracer did not recover a single primitive anywhere in the corpus. Not one circle,
ellipse, rectangle, rounded rectangle, star or regular polygon — including on inputs
that are *nothing but* one primitive on a flat background.

This is the cleanest possible confirmation that the gap is structural rather than a
matter of tuning. `prim_circle` renders at DISTS@4x = 0.008 — essentially perfect — while
costing 66 parameters instead of 3. **Pixel metrics cannot see this at all**, which is
precisely why the harness had to exist before the engine.

## 3. Sub-pixel features are destroyed, and no setting recovers them

`thin_features` contains 10 vertical strokes from 0.6 to 5.6 user units wide — at the
128px tier, 0.3px to 2.8px. Ground truth: 11 elements.

**VTracer output: 2 elements.** Eight of ten strokes are gone.

| metric | value | reading |
|---|---:|---|
| DISTS@4x | 0.290 | worst in the corpus by 1.7× |
| ΔE00 mean@4x | 9.43 | ~5× the just-noticeable difference |
| SSIM@4x | 0.829 | |
| params | 48 vs GT 66 | *fewer* — because content is missing |

Every one of the 10 settings produced byte-identical output (48 params). The parameter
sweep cannot recover the strokes because the information was destroyed at thresholding,
before any tunable stage ran. This is deficit #1 measured directly, and it is the single
strongest empirical argument for the S2 sub-pixel front end.

## 4. The seam/overdraw trade is real and behaves as predicted

Ground truth for the `mosaic_*` inputs sits at **overdraw 1.00, seam 0.00** — regions
share boundary vertices exactly.

| VTracer mode | overdraw | seam@1x | seam@4x | params | DISTS@4x |
|---|---:|---:|---:|---:|---:|
| `cutout` | 1.73 | 0.079% | 0.216% | 327 | 0.082 |
| `spline` (stacked) | 1.43 | 0.000% | 0.000% | 312 | 0.067 |
| `polygon` (stacked) | 1.43 | 0.000% | 0.000% | 64 | 0.132 |

Two things worth noting:

1. **Seams appear only in cutout mode**, and stacked mode buys their absence with
   overdraw. Neither reaches the corner. On the mosaic inputs specifically, overdraw is
   **1.64** where ground truth is 1.00 — roughly 64% of the geometry is redundant.
2. **Seams grow 2.4× from 1x to 4x** (3.0× on the mosaic inputs). A defect that is
   nearly invisible at source resolution and clearly visible zoomed is exactly what a
   single-scale benchmark would miss, and it validates the multi-scale protocol.

## 5. Robustness: Potrace's dropped dynamic program is worth ~18 points

Parameter growth under smooth boundary perturbation (AnchorFlow's protocol):

| engine | median growth | mean growth | n |
|---|---:|---:|---:|
| **potrace** | **0.0%** | +12.9% | 160 |
| **vtracer** | **+18.0%** | +77.8% | 200 |

Published reference points: AnchorFlow +2.9%, AdaVec +20.7%, VTracer +106.7%. Our
VTracer mean (+77.8%) is the same regime as the published +106.7%; the difference is
attributable to perturbation magnitude and to our corpus being synthetic flat art.

> **Correction (M1 increment 1).** The causal attribution below — that the optimal-polygon
> dynamic program is what buys the robustness — **did not survive a controlled test**. Holding
> the admissibility rule, cost function and inputs fixed and varying only locality, a greedy
> segmenter is just as stable as the DP (0.0% median for both). The likely cause is the
> *tolerance envelope*, not non-locality. See [`M1-PROGRESS.md`](M1-PROGRESS.md) §4. The
> measurements below stand; the explanation does not.

**The interesting result is Potrace's zero median.** Potrace's optimal-polygon step —
the lexicographic (segment-count, penalty) shortest-cycle DP, the one non-local stage in
its pipeline — absorbs smooth boundary noise without spending anchors on it. VTracer
explicitly dropped that step for speed, and it inflates.

This is direct empirical support for design decision **S4.1** (adapt Potrace's
straightness criterion and optimal-polygon DP to floating-point input), and it raises
that item's priority. It also sharpens the associated risk: the DP is defined on the
integer lattice, so the port is genuine research, not a translation. Now we know what it
buys.

---

## What M1 must beat

| axis | VTracer (measured) | M1 target | why reachable |
|---|---|---|---|
| param blow-up vs GT | 5.9× median | **< 2×** | primitives + arcs + node budget |
| primitive recall | **0.00** | > 0.80 | S4.2 detection/snapping |
| overdraw (mosaics) | 1.64 | **~1.00** | planar map with shared edges |
| seam@4x (cutout) | 0.216% | **0.00%** | shared edges make seams impossible |
| thin_features elements | 2 of 11 | **11 of 11** | S2 coverage inversion |
| param growth (boundary) | +18.0% median | **< 5%** | S4.1 optimal-polygon DP |

Note that M1 does **not** need to win on DISTS to be a success. VTracer's fidelity is
already good on most of this corpus (median DISTS@4x ≈ 0.05); it buys that fidelity with
6× the parameters, zero primitives, and 64% redundant geometry. The claim to prove is
*equal or better fidelity at a fraction of the structural cost* — which is why the
protocol reports Pareto frontiers rather than single points.

## Caveats

- One tier (128px) and one corpus (synthetic, 20 inputs). Broaden to the emoji corpora
  and the 32/64px tiers before quoting these as general figures.
- The synthetic corpus is favourable to primitive recall by construction — it is built
  from primitives. That is the intent (it isolates the capability) but it is not a
  claim about real-world logo composition.
- Vectorizer.AI, the actual ceiling, has not been run: it needs API credentials.
  Until it is in the table, "beats VTracer" is the only defensible claim.
- Potrace is bilevel, so its fidelity numbers are not comparable with VTracer's; it is a
  control for anchor economy and robustness only.
