# Inkvec Engineering Roadmap & TODO

This document tracks upcoming architectural improvements and next-generation milestones following the `v0.1.1` release.

---

## 1. Neural Geometric Post-Processing Refinement Pass

While the current Inkvec core uses dynamic programming over minimum description length (MDL) curves and planar face mapping, raster tracing inherently leaves subtle quantization artifacts, blunted sharp corners, and micro-wiggles on long straight edges.

We have trained a 20M vision-conditioned Graph Neural Network (`TraceRefineGNN20M`) specifically trained on raster/vector pairs to predict continuous anchor nudges, edge straightness, and CAD geometric regularities.

### Implementation Tasks:
- [ ] **Model Export & Sidecar Runtime:**
  - Export `checkpoints_gnn_large/gnn_best_val.pt` to optimized ONNX Runtime and TorchScript formats.
  - Implement a zero-overhead C++/Rust or sidecar runtime interface (`inkvec --refine <auto|on|off>`).
- [ ] **End-to-End Geometric Solver Integration:**
  - Connect the equality-constrained least-squares KKT solver (`cad_solver.py`) directly into the Rust pipeline to enforce exact mathematical horizontal, vertical, parallel, collinear, and orthogonal geometric snapping ($< 10^{-6}\text{px}$ error).
  - Integrate topological simplification (`topology_simplifier.py`) to collapse collinear over-segmented edges and restore sharp $C^0$ apex corners from blunted tracer contours.
- [ ] **Dual-Head Optimization:**
  - Evaluate joint inference on both node position nudges ($\Delta \mathbf{x}$) and edge straightness logits to optimize curve vs. line decision boundaries globally.

---

## 2. High-Volume Benchmarking & Extended Corpus Evaluation

The current continuous integration gate tests against a grandfathered 246-icon screen set (`bench/gate/baseline.json`). To further harden quality across diverse visual styles, higher-volume, large-scale benchmarks are required.

### Implementation Tasks:
- [ ] **Large-Scale Multi-Thousand Icon Benchmark Suite:**
  - Scale benchmark harness from 246 icons to 2,500+ diverse vector graphics sourced from Lucide, Material Design, Simple Icons, Twemoji, OpenMoji, and real-world brand marks.
  - Stratify corpus by complexity tiers (simple geometric logos, complex filigree, gradient artwork, and scanned vintage emblems).
- [ ] **Automated Multi-Engine Pareto Frontier:**
  - Benchmark Inkvec against Potrace, VTracer, and commercial engines (e.g. Adobe Illustrator Image Trace) across high volumes.
  - Compute automated Pareto frontiers for:
    - Colour fidelity ($\Delta E_{00}$) vs. parameter count / SVG file size.
    - Turning metric (anchor curvature smoothness) vs. corner fidelity.
    - Sub-pixel self-resolution reconstruction error.
- [ ] **Distributed Benchmark Runner:**
  - Implement batch execution with persistent caching, multi-core worker pools, and streaming regression metrics to avoid CI timeout while running deep benchmark sweeps.

---

## 3. Output-Preserving Speedups (deferred: diminishing returns)

Fifteen browser threads buy only about 4.3× over one core, because two stages do not scale. Measured on `adamo` at 768 px, native, 1 vs 16 threads (2026-09-19):

| stage | 1 thread | 16 threads | limit |
|---|---|---|---|
| `fit_dp` | 4185 ms | 768 ms | the largest single ring (681 ms); each ring's DP is sequential |
| `palette` | 662 ms | 399 ms | mostly serial |
| `carve` | 104 ms | 102 ms | serial |

Both can be made faster without changing a byte of output: compute the same numbers in parallel, and keep every decision in the same arithmetic and the same order. The comparisons in `solve_open` are `base + cost_a < base + cost_b`, which is not the same as `cost_a < cost_b` in floating point, so they must stay as written; only the fits feeding them may move. The estimated ceiling is 1.3–1.5× end to end, which is why this waits.

### Implementation Tasks:
- [ ] **Critical-path ring speculation in `fit_dp`:**
  - For the one or two rings longer than the average per-thread load, fit their spans in parallel ahead of the sequential sweep, which then reads those results; spans past the prune are discarded unread.
  - A blanket block-parallel version (2026-09-06) produced identical output but was slower at 2048 px, where every core was already busy with other rings. Apply it to the critical path only.
- [ ] **Largest-first ring scheduling:** start the biggest rings first so none begins late. Exact by construction; small gain.
- [ ] **Palette serial remainder:** profile which loop keeps `palette` at 1.66× on 16 threads; parallelize it with results combined in the existing order.
- [ ] **Acceptance:** byte-identical SVGs before and after on the 246-icon screen set, the Space samples and a few 2048-px logos, with `--time-budget 0` (a wall-clock budget makes output depend on speed); then `python bench/wasm_parity.py` on rebuilt wasm.
- [ ] **Optional, separate: native = browser output.** Today they differ on 6 of 8 parity images in the last digit, occasionally in path structure, because wasm32 takes transcendentals (OKLab `cbrt`, sRGB `powf`, `atan2`) from Rust's `libm` and Windows from its runtime. Routing them through the `libm` crate on every target would make the CLI and the Space agree, at the cost of changing native output once and re-baselining the gate.
