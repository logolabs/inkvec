# AI Usage & Provenance Disclosure

**Project:** Inkvec / LogoLabs (`logolabs/inkvec`)  
**Publication Date:** September 2026  
**License:** Apache-2.0  

---

## 1. Executive Summary

This document provides a comprehensive, transparent disclosure regarding the role of Artificial Intelligence (AI) across the **Inkvec** codebase, its software engineering lifecycle, its training pipelines, and its runtime vectorization architecture.

AI usage in this project falls into three distinct, decoupled categories:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                             INKVEC AI TAXONOMY                              │
├──────────────────────────────┬──────────────────────────────┬───────────────┤
│  Class A: Development Tools  │  Class B: Runtime AI Models  │  Class C:     │
│  (Agentic LLMs / Assistants) │  (Restoration & Refinement)  │  Core Tracing │
├──────────────────────────────┼──────────────────────────────┼───────────────┤
│ • Code synthesis & refactor  │ • MambaIR / MambaIRv2 (SR)   │ • 100% NON-AI │
│ • CI / Release stabilization │ • Denoiser (restorer.onnx)   │ • Exact Math  │
│ • Static quality ratchets    │ • TraceRefineGNN20M (NextGen)│ • Planar DCEL │
│ • Benchmark harness authoring│ • EuroHPC compute trained    │ • MDL DP Fits │
└──────────────────────────────┴──────────────────────────────┴───────────────┘
```

1. **Class A — AI in Development:** Large Language Models (specifically Google DeepMind Antigravity / Gemini models) were utilized as agentic pair-programming assistants under strict human oversight for coding, test writing, CI/CD triage, benchmark scripting, and documentation.
2. **Class B — AI in Runtime Components:** Selective, modular deep-learning networks are provided as optional pre-processing and post-processing stages for raster damage removal (denoising, super-resolution) and graph refinement.
3. **Class C — Deterministic Core Engine:** The fundamental vectorization core (palette quantization, planar DCEL topology, sub-pixel boundary optimization, dynamic programming curve fitting) is **entirely non-neural and deterministic**. It is based on computational geometry, physical optics, and information theory — **no generative models, no diffusion, and no LLM hallucinations are present in the core tracing path**.

---

## 2. Class A: AI in the Software Engineering Lifecycle

### 2.1 Agentic Pair-Programming Roles
During the development of Inkvec, agentic AI assistants were deployed for specific engineering workflows:

- **CI/CD Stabilization & Cross-Platform Triaging:**
  - Diagnosed platform-specific target graph resolution quirks in `cargo tree` and `cargo-about`.
  - Identified that ANSI terminal color codes emitted when `CARGO_TERM_COLOR: always` was set corrupted regular expression matching during automated license verification on Linux runners.
  - Provisoned missing headless test dependencies (`svgelements`, `resvg_py`, `scikit-image`) in GitHub Actions environments.
  - Upgraded release automation workflows to target modern GitHub runners (`macos-15-intel`, `ubuntu-24.04-arm`).
- **Quality Ratchet & Static Analysis Maintenance:**
  - Automated tracking of code-complexity baselines in `bench/quality_budget.json`, enforcing that legacy long functions remain grandfathered while no newly added functions exceed line budgets.
  - Maintained zero warnings under `cargo fmt --check` and `cargo clippy -D clippy::correctness -D clippy::suspicious`.
- **Benchmark Suite & Provenance Scripting:**
  - Authored automated scoring scripts (`bench/ci_gate.py`, `bench/svgeval.py`) evaluating perceptual color error ($\Delta E_{00}$ in OKLab space), parameter economy ratios, and angular turning metrics against a 246-icon screen set.
  - Implemented cryptographic provenance recording (executable SHA256, compiler snapshot, git commit hash) embedded directly into `bench/gate/baseline.json`.
- **Documentation & Visual Asset Generation:**
  - Generated technical breakdowns and algorithmic walkthroughs in `PIPELINE_EXPLANATION.md` and `docs/algorithm/`.
  - Created standalone SVG architectural diagrams (`docs/assets/pipeline-step-by-step.svg`, `docs/assets/planar-map-topology.svg`, etc.) to illustrate mathematical operations.

### 2.2 Human Oversight, Review & Safety Guardrails
To prevent code rot, security vulnerabilities, or algorithmic hallucinations:
- **No Downgrading Quality Thresholds:** A strict policy is enforced preventing agents from artificially loosening regression limits. The gates require that $dE_{00}$ error and anchor turning metric rise by at most $1.0\%$, and parameter ratio rises by at most $5.0\%$.
- **Compilation & Verification Gates:** All generated Rust code must compile under the workspace Minimum Supported Rust Version (MSRV 1.88) without warnings, pass all native unit tests, pass cross-platform builds across Linux, Windows, and macOS, and pass WebAssembly compilation (`wasm32-unknown-unknown`).
- **Citation Auditing:** All academic literature cited in technical documentation is verified against primary indexers (arXiv, IEEE, ACM, CVPR, ECCV) to eliminate hallucinated titles or non-existent paper references.

---

## 3. Class B: AI in Runtime Machine Learning Models

Inkvec provides optional, modular neural components designed to handle extreme image degradations before and after the deterministic geometric engine runs. These models are completely decoupled from the core tracer.

### 3.1 Raster Denoiser & Restorer (`inkvec-restore`)
- **Purpose:** Scans lossy raster inputs (low-quality JPEG and WebP images) and eliminates discrete cosine transform (DCT) ringing, high-frequency blocking artifacts, and compression haze before edge detection.
- **Model Name / Weights:** `restorer.onnx` hosted publicly on Hugging Face at [`Logolabs/inkvec-denoiser-001`](https://huggingface.co/Logolabs/inkvec-denoiser-001) under the Apache-2.0 license.
- **Runtime Execution:**
  - `restore-model` (default): Accelerated CPU execution via ONNX Runtime.
  - `restore-burn`: Pure Rust inference via the Burn deep-learning framework (portable, zero native dependencies).
  - `restore-wgpu`: GPU-accelerated inference via Burn's WebGPU backend (Vulkan, DirectX 12, Metal).
- **Training Compute Acknowledgement:**
  - Trained using supercomputer time awarded by the **EuroHPC Joint Undertaking** on the **Arrhenius GPU supercomputer at NAISS (Sweden)** under Project ID **EHPC-AIF-2026PG01-907**.

### 3.2 Super-Resolution Pre-Pass (`inkvec-sr`)
- **Purpose:** Restores sub-pixel structural details for tiny inputs (e.g. 16px to 64px favicons) prior to vector contouring.
- **Architecture:** Based on MambaIR / MambaIRv2 state-space models for image restoration.
- **Provenance:** Third-party model architecture (Apache-2.0, Guo et al., ECCV 2024); weights are fetched on demand by Python tooling and are never bundled in the binary.

### 3.3 Geometric Refinement GNN (`TraceRefineGNN20M` — Research / Next-Gen)
- **Purpose:** Post-processes raw traced vector graphs by predicting continuous anchor point nudges ($\Delta \mathbf{x}$), edge straightness classifications, and CAD regularity constraints (tangency, perpendicularity, parallelism).
- **Architecture:** 20M parameter vision-conditioned Graph Neural Network utilizing EdgeConv and spatial cross-attention over the raster input.
- **Training Environment:** Trained locally on NVIDIA GeForce RTX 4060 GPU across 45,000 optimization steps (15 epochs) on synthetic and curated vector datasets, with live metrics logged to Weights & Biases under project `svg-refine-gnn`.
- **Integration:** Operates in conjunction with an equality-constrained least-squares KKT solver (`cad_solver.py`) to enforce mathematical CAD snapping ($< 10^{-6}\text{px}$ tolerance).

---

## 4. Class C: Deterministic Non-Neural Guarantees

For applications demanding reproducible, certifiable, and zero-hallucination vector output, the core vectorization engine in Inkvec is **100% deterministic mathematical code**:

```
[Raster Input]
      │
      ▼
┌────────────────────────────────────────────────────────────────────────┐
│               DETERMINISTIC VECTOR CORE (NO NEURAL NETWORKS)           │
├────────────────────────────────────────────────────────────────────────┤
│ 1. Sub-Pixel Linear Unmixing:  Inverts optical anti-aliasing in 3D RGB │
│ 2. MDL Palette Clustering:     Minimum Description Length in OKLab     │
│ 3. Planar DCEL Map:            Shared half-edge topological boundaries │
│ 4. Boundary Optimization:      Levenberg-Marquardt + Shoelace Jacobian │
│ 5. Curve Fitting:              Dynamic Programming over Bézier / Arcs  │
│ 6. Topological Repair:         Winding-number resolution & G1 fairing  │
└────────────────────────────────────────────────────────────────────────┘
      │
      ▼
[Scalable Vector Graphics (SVG)]
```

- **Zero Generative Hallucination:** The core tracer never "invents" features, characters, or shapes that do not exist in the source image. Every point's placement is determined by minimizing $\chi^2$ continuous coverage residuals.
- **Exact Topology:** The Doubly Connected Edge List (DCEL) guarantees that boundaries between shapes are mathematically identical. Seams, pixel bleed, and overdraw are structurally impossible.
- **Robust Arithmetic:** All geometric decisions use adaptive-precision floating-point arithmetic and exact geometric orientation predicates based on Jonathan Richard Shewchuk's formulations.

---

## 5. Licensing, Ethics & Training Data Compliance

- **Permissive Licensing:** All source code in this repository is licensed under **Apache-2.0**.
- **Third-Party Crate Audit:** Every Rust crate compiled into Inkvec is audited for permissive licensing (MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib). The full manifest is automatically tracked and updated in [`docs/THIRD_PARTY.md`](THIRD_PARTY.md).
- **Clean-Room Baseline Separation:** Potrace is licensed under GPL-2.0. Inkvec **never links, incorporates, or redistributes Potrace source or object code**. Potrace is invoked solely as an external command-line executable in optional benchmark comparison scripts.
- **Test Corpus Data Disclaimer:** The rasters committed under `bench/data` (Lucide, Material Design, Twemoji, OpenMoji, Simple Icons) are strictly used as test benchmarks to evaluate vectorization fidelity against ground truth. All marks and logos remain the trademarks of their respective owners.

---

## 6. Feedback and Inquiries

For questions regarding AI disclosures, algorithmic provenance, or academic citations, please open an issue on GitHub at [`github.com/logolabs/inkvec`](https://github.com/logolabs/inkvec) or contact the maintainers at LogoLabs.
