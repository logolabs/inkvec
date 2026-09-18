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
