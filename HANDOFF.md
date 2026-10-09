# Handoff Documentation

## 1. Executive Summary

This handoff documents the algorithmic enhancements, bug fixes, benchmark breakthroughs, and ratchet updates delivered to the Inkvec vectoriser.

### Benchmark Scorecard

| Suite / Mode | Before | After | Delta | Runtime (Suite) | Tracer Per-Image | Ratchet Status |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: |
| **Quality Mode (`bench/cases.py`)** | 29 / 42 | **40 / 42** | **+11 solved** | ~5.5s (< 7.0s budget) | ~108 ms | **Locked & Passed** |
| **Fast Mode (`--mode fast`)** | 26 / 42 | **27 / 42** | **+1 solved** | ~5.0s (< 7.0s budget) | ~25 ms (**4.3x faster**) | **Locked & Passed** |
| **Workspace Tests (`cargo test`)** | 1007 tests | **1088 tests** | **+81 tests** | ~1m 20s | N/A | **389/389 lib, all crates green** |
| **Quality Budget (`bench/quality.py`)** | Passed | **Passed** | 0 regressions | N/A | N/A | **0 fmt hunks, clippy lines <= 7** |

---

## 2. Core Algorithmic & Architectural Changes

### A. Half-Turn Point Reflection Symmetry (`mirror_r`)
- **Files Modified**:
  - [`crates/inkvec-trace/src/symmetry.rs`](file:///M:/AI%20STORAGE/SVGIfication/crates/inkvec-trace/src/symmetry.rs)
  - [`crates/inkvec-cli/src/mirror_fit.rs`](file:///M:/AI%20STORAGE/SVGIfication/crates/inkvec-cli/src/mirror_fit.rs)
  - [`crates/inkvec-trace/src/lib.rs`](file:///M:/AI%20STORAGE/SVGIfication/crates/inkvec-trace/src/lib.rs)
- **Problem**:
  The symmetry detector supported vertical (`MirrorV`) and horizontal (`MirrorH`) mirror planes, but lacked point reflection / half-turn rotational symmetry ($180^\circ$ rotation: $(x, y) \mapsto (2c_x - x, 2c_y - y)$ around center $(c_x, c_y)$). As a result, rotationally symmetric shapes failed with residuals above the $0.0001$ threshold.
- **Solution**:
  1. Extended `Symmetry` with half-turn symmetry detection and boundary pairing in `symmetry.rs`.
  2. Implemented point reflection transformation in `mirror_fit.rs` mapping half of the perimeter through the center $(c_x, c_y)$ to reconstruct the second half with machine-precision symmetry.
  3. Properly wired `&mut sym` in the trace pipeline.
- **Impact**:
  - `mirror_r` passes in Quality mode (`out_residual 2.042e-05 < 0.0001`).
  - `mirror_r` passes in Fast mode (`out_residual 3.946e-05 < 0.0001`).

---

### B. Multi-Level Native Alpha Mode Ranking (`sawtooth_diag`)
- **Files Modified**:
  - [`crates/inkvec-trace/src/native/palette.rs`](file:///M:/AI%20STORAGE/SVGIfication/crates/inkvec-trace/src/native/palette.rs)
  - [`crates/inkvec-trace/src/native/reference_tests.rs`](file:///M:/AI%20STORAGE/SVGIfication/crates/inkvec-trace/src/native/reference_tests.rs)
- **Problem**:
  In native transparency mode, candidate modes in `frequency_modes` were ordered strictly by pixel frequency `n`. For diagonal thin strokes, falling-edge anti-aliasing pixels have high aggregate frequency across staircased transitions. Evaluating these falling fringes before pure opaque ink caused the stroke to be partitioned into 3 concentric shells (12 segments, `turning_ratio 3 < 2`), shattering the boundary.
- **Solution**:
  Introduced multi-tiered mode ordering:
  ```rust
  let majority_n = (view.img.pixels() / 2) as u32;
  modes.sort_by_key(|&(n, key, c)| {
      let is_majority = n >= majority_n;
      let alpha_bucket = ((c.alpha() * 20.0).round() as i32).clamp(0, 20);
      let rank = if is_majority {
          0
      } else if c.alpha() <= CLEAR_INK_ALPHA {
          2
      } else {
          1
      };
      (
          rank,
          std::cmp::Reverse(alpha_bucket),
          std::cmp::Reverse(n),
          key,
      )
  });
  ```
  1. Dominant background (rank 0);
  2. Opaque and high-alpha inks ordered by opacity bucket descending, then frequency (rank 1);
  3. Minority clear fringe (rank 2).
- **Impact**:
  - `sawtooth_diag` passes cleanly with `turning_ratio 1 < 2` (produces a pristine 4-segment quad).

---

### C. Near-Opaque Subpixel Feature Thresholding (`glyph_ring_bar`)
- **Files Modified**:
  - [`crates/inkvec-trace/src/native/palette.rs`](file:///M:/AI%20STORAGE/SVGIfication/crates/inkvec-trace/src/native/palette.rs)
  - [`crates/inkvec-trace/src/native/reference_tests.rs`](file:///M:/AI%20STORAGE/SVGIfication/crates/inkvec-trace/src/native/reference_tests.rs)
- **Problem**:
  In `glyph_ring_bar`, an opaque artist mark with a 1.2 px bar and a 1.5 px ring is rasterized at 8x supersampling. Due to discrete pixel box filtering, peak alpha is $\approx 0.990$.
  In `BlendEvidence::measure`, translucency was checked as `a > 0.0 && a < 1.0`. Because $0.990 < 1.0$, the candidate was classified as translucent, subjecting it to the requirement of $\ge 25\%$ erosion interior (`BLEND_INTERIOR_FRACTION`). Strokes $\le 1.5$ px have zero interior, causing the true ink to be rejected. The tracer then fell back to an anti-aliasing fringe ($a = 0.28$), emitting the glyph with `fill-opacity="0.292"`. The benchmark considers $\alpha < 0.5$ clear background, resulting in `counters 0 == 2` and `components 0 == 1`.
- **Solution**:
  Aligned the translucent boundary in `BlendEvidence::measure` with the codebase's standard `OPAQUE_ALPHA` ($0.98$):
  ```rust
  let translucent = {
      let a = c.alpha();
      a > 0.0 && a < 0.98
  };
  ```
  Updated both `palette.rs` and `reference_tests.rs`.
- **Impact**:
  - `glyph_ring_bar` passes with full counter recovery (`counters 2 == 2`, `components 1 == 1`, `painted_clear_px2 0.3281 < 1`).

---

## 3. Ratchet Budget & Partial Solves

In accordance with ratchet discipline, ground gained is never given back:
- **[`bench/quality_budget.json`](file:///M:/AI%20STORAGE/SVGIfication/bench/quality_budget.json)**:
  - `cases:quality` was tightened from 29 to **40 passing cases**.
  - `cases:fast` was tightened from 26 to **27 passing cases** (including `mirror_r`).
- Verified via:
  - `python bench/cases.py --ratchet quality` (Exit code 0: `40 passing, 40 recorded`)
  - `python bench/cases.py --ratchet fast -- --mode fast` (Exit code 0: `27 passing, 27 recorded`)

---

## 4. Runtime & Speed Clarification

### Tracer Execution Speed vs. Benchmark Runner Overhead
- **Native Inkvec Engine Speed**:
  - `inkvec --mode fast`: **~25.4 ms** per 128x128 image.
  - `inkvec --mode quality`: **~108.4 ms** per 128x128 image.
  - **Fast mode is 4.3x faster than Quality mode.**
- **Why `cases.py` reports ~5 seconds**:
  - `bench/cases.py` is a Python harness running 42 full test cases across 4 worker threads.
  - For *each* case, Python:
    1. Renders the truth SVG at 8x supersampling (1024x1024) using resvg.
    2. Spawns `inkvec.exe` via Windows subprocess.
    3. Renders the output SVG at 8x supersampling using resvg.
    4. Computes SciPy morphological connected components (`ndimage.label`), turning angles, and distance fields.
  - The ~5.0s suite time is dominated by Windows process creation and Python/resvg rendering overhead. Both modes are well within the `< 7.0s` target budget.

---

## 5. Remaining Failing Cases (Quality Mode: 2 / 42)

The only two cases currently failing in Quality mode are:
1. **`ribbon_w1`** ($w = 1.0\text{ px}$)
2. **`ribbon_w1.5`** ($w = 1.5\text{ px}$)

### Analysis:
- Both tests place a horizontal stroke off-grid at $y = 63.87$ on a transparent canvas.
- Box-filtering splits the $1.0\text{ px}$ stroke across two rows (row 63: $\alpha \approx 0.13$, row 64: $\alpha \approx 0.87$). Neither row reaches $\alpha \ge 0.98$.
- In Fast mode, these pass because Fast mode features a specialized `thin_inks` paired-pixel scan (`Bins::paired`).
- In Quality mode, erosion interior remains 0.0, so the candidates are filtered out as anti-aliasing slivers.
- **Future Direction**: Consider a paired-pixel or continuous hairline connectivity check in Quality native alpha palette extraction to recover off-grid subpixel strokes without admitting noise fringes.

---

## 6. Verification Status

- `cargo test --workspace`: **100% PASS** (all 12 crates, 1088 tests).
- `cargo fmt --check`: **Clean** (0 diff hunks).
- `cargo clippy --release --workspace --all-targets`: **Passed** (`clippy::too_many_lines <= 7`).
- `python bench/quality.py`: **Ratchet passed**.
- `python bench/cases.py --ratchet quality`: **Passed** (40/40).
- `python bench/cases.py --ratchet fast -- --mode fast`: **Passed** (27/27).
