# InkvecTheory — machine-checked theory of inverting box-filter rasterisation

A Lean 4 / Mathlib library proving what an anti-aliased raster does and does not determine
about the shapes drawn into it, and what that implies for a tracer. Every result is proved
from Lebesgue measure on `ℝ²` or from first principles. **The library has no `sorry`, and no
axiom of its own.** `Audit.lean` prints the axioms each headline theorem uses; every line
reads `[propext, Classical.choice, Quot.sound]`, Lean's three standard axioms.

The prose companion, with the literature survey, the comparison against prior work and the
measured effect on inkvec, is [`docs/theory/README.md`](../../docs/theory/README.md).

## Build

```bash
curl -sSfL https://raw.githubusercontent.com/leanprover/elan/master/elan-init.sh | sh -s -- -y --default-toolchain none
cd formal/InkvecTheory
lake exe cache get          # prebuilt Mathlib (about 5 GB); skips hours of compiling
lake build                  # the library: about a minute and a half on four cores
lake build InkvecTheory.Audit   # prints the axiom audit
```

Toolchain and Mathlib are pinned in `lean-toolchain` and `lake-manifest.json`
(Lean 4.34.1, Mathlib `v4.34.1`).

## The model, stated once

* **Pixel** `(i, j)` is the open square `(i, i+1) × (j, j+1)`; inkvec's code puts pixel
  centres on integers, so its coordinates are these minus ½.
* **Box-filter coverage** `coverage Ω i j` is the Lebesgue area of `Ω ∩ pixel i j`
  (`Coverage.lean`). This is what exact-area renderers write (libart, FreeType's smooth
  rasteriser, font-rs, inkvec's own `boundary_opt/band.rs`). Point-supersampling renderers
  are treated separately (`Supersampling.lean`).
* A **boundary** is, locally, the graph of a continuous function over one axis: the shape is
  `subgraph f = {(x, y) | y < f x}`. Every orientation reduces to this one by symmetry, and
  the hypotheses say exactly where the graph must lie.
* Two-colour **unmixing** (`coverage.rs`: `a = (P − B)·(F − B)/|F − B|²`) is taken as given:
  the theorems are about the coverage `a`.

## Theorems, and the code each bears on

| file | theorem | statement | inkvec code |
|---|---|---|---|
| `Coverage` | `coverage_subgraph` | the area of a graph's subgraph in a pixel is `∫ clamp01 (f x − j)` over the pixel's column | the model every stage assumes (`coverage.rs`) |
| `Coverage` | `column_sum_eq_average` | **a column's coverages sum to the column mean of the boundary height**, for every continuous graph, whatever its slope or curvature | `planar/strip.rs` `line_mean` |
| `NullSpace` | `invisible_perturbation` | a zero-mean perturbation that keeps the boundary in one pixel row changes no pixel | the sawtooth of `boundary_opt` (`08-boundary-solve.md`) |
| `NullSpace` | `sine_ripple_invisible`, `rasterization_not_injective` | two different shapes with identical coverage at every pixel of the plane, at every frequency | why the boundary solve needs a prior |
| `NullSpace` | `polyline_zigzag_invisible`, `polyline_kernel` | with one vertex per pixel border, the zigzag `δ(−1)ᵏ` is invisible and is the **only** invisible deformation: the null space is exactly one-dimensional | `boundary_opt.rs` priors (`K_KINK`) |
| `NullSpace` | `coverage_layer_cake`, `column_fiber` | a column's coverages determine, and are determined by, `t ↦ ∫ (f − t)₊` at integer `t`: rearranging the boundary inside a column is invisible | — |
| `Identifiability` | `zeros_of_zero_averages` | zero means on `n` cells force `n` distinct zeros | — |
| `Identifiability` | `poly_eq_of_averages` | a polynomial of degree `< n` is determined by its means on `n` cells (histopolation is unisolvent) | `strip.rs` `histopolate` |
| `Identifiability` | `polynomial_boundary_determined` | **polynomial boundaries of degree `< n` are determined by their pixels on `n` columns** | `inkvec-fit` model selection |
| `Identifiability` | `line_from_two_columns` | closed form: `s = S₁ − S₀`, `b = S₀ + j₀ − s(i + ½)` | — |
| `Deconvolution` | `cubic_point_from_means` | `A − (A₊ − 2A + A₋)/24` is the centre value of **every cubic** | — |
| `Deconvolution` | `quartic_defect` | the first missed term is `−(3/640)·f''''` | — |
| `Deconvolution` | `boundary_point_from_pixels` | from raw pixel areas of three columns, the exact boundary point of a cubic boundary | `strip.rs` |
| `Deconvolution` | `strip_reading_exact` | **end to end: from the raw pixel areas of four columns, `strip.rs`'s `histopolate` returns the boundary cubic's own coefficients** | `strip.rs` `strip_along` |
| `Deconvolution` | `histopolate_cubic`, `face_value_cubic`, `side_stencil_cubic` | **the rational constants in `strip.rs` are exact**: its `histopolate` returns any cubic's coefficients from its four cell means; the face value `(7(m₁+m₂) − (m₀+m₃))/12`; the one-sided stencils of its corner test agree on every cubic | `strip.rs` `histopolate`, `SIDE_TOL` test |
| `LevelSetBias` | `levelSet_reading`, `levelSetBias_le`, `levelSetBias_max` | reading an edge at the ½-crossing of interpolated coverage is off by up to **`3/2 − √2 ≈ 0.0858 px` on a straight noiseless step, and the bound is attained** | `contour.rs`, `planar::root_find_half`, `DEFAULT_SIGMA_MODEL` |
| `LevelSetBias` | `column_sum_reading` | the column sum reads the same edges exactly | `strip.rs` |
| `Quantization` | `binarize_minimax` | **any** estimator reading thresholded pixels is off by ≥ ½ − η px on some edge | Potrace-, VTracer-class tracers |
| `Quantization` | `quantize_minimax` | any `L`-level quantiser (any function at all) costs ≥ `δ/2` for every `δ < 1/L` | — |
| `ThinStroke` | `coverage_band` | a stroke's coverage is the difference of its edges' coverages | — |
| `ThinStroke` | `straddle_inversion` | a sub-pixel stroke straddling a pixel border is read **exactly**: centre `j + 1 + (a₊ − a₋)/2`, width `a₋ + a₊` | `planar::ridge_offset` |
| `ThinStroke` | `hidden_stroke_invisible` | inside one pixel its position is undetermined within `1 − w` | — |
| `ThinStroke` | `centroid_bias`, `centroid_bias_worst` | the coverage centroid is off by `(1 − w)(a₊/w − ½)`, up to `(1 − w)/2` | `planar::ridge_offset` |
| `ThinStroke` | `width_contrast_ambiguity` | a hidden stroke fixes only width × contrast | `coverage.rs` `saturation` |
| `Rank` | `flat_directions` | `N` free points against `M` pixels leave ≥ `2N − M` flat directions | `boundary_opt.rs` unknowns |
| `Noise` | `strip_unbiased`, `strip_variance` | zero-mean pixel noise leaves the column sum unbiased, variance `k·σ²` | `strip.rs` |
| `Noise` | `face_value_noise`, `point_from_means_noise` | the face value has noise gain `25/36`, the point correction `113/96` | `strip.rs` |
| `Supersampling` | `subCount_close`, `ss_column_sum_close` | under `n × n` point supersampling the column sum is the `n`-point midpoint rule of the boundary to within `1/(2n)` px | the 8x8 corpus of `bench/ci_gate.py` |
| `Gen.Expr` | `evalQ_sound`, `evalI_sound`, `nonneg_of_evalI` | a generated checker's exact rational value is the real value; its outward-rounded interval contains the real value; an interval with a non-negative lower end certifies a condition `e ≥ 0` | `crates/inkvec-verified` (`q.rs`, `iv.rs`) |
| `Gen.Strip` | `histopolateK_eq`, `stripResidualK_eq`, `stripResidual_on_cubic` | the generated stencil is `histopolate`; the generated residual vanishes exactly where the vertex, moved along its normal, lies on the cubic the column sums determine | `strip.rs` `histopolate`, `certified` |
| `Naturality` | `natural_model_closer`, `natural_model_gain` | **a merge that keeps the truth drawable never costs fidelity**: least squares onto `K ≤ L` with the truth in `K` is at least as close to it for every noise, by exactly the noise in the removed directions | `ribbon/refine/merge.rs` |
| `Naturality` | `merge_chi2_increase`, `noise_absorbed` | the merge raises the residual by that noise and nothing else; each model dimension keeps one unit of noise variance (`Σᵢ ‖P bᵢ‖² = dim K`), so every surplus parameter costs `σ²` of squared error | the description-length price `λ` per parameter |

## What is new, and what is not

The identity behind `column_sum_eq_average` is classical: volume-of-fluid interface
reconstruction rests on it (Puckett 2010, eq. 11), and Trujillo-Pino et al. (2013) built a
sub-pixel edge detector on it. Histopolation unisolvence and the `1/24` cell-average
correction are textbook finite-volume facts (Shu 1997; McCorquodale & Colella 2011). What
this library adds, as far as the survey in `docs/theory/README.md` could establish:

* **the first machine-checked proofs** of any of it: no formalisation of rasterisation
  coverage, anti-aliasing or sub-pixel recovery was found in Lean, Coq, Isabelle or HOL Light;
* the **exact null space**: the layer-cake fibre of a column (`column_fiber`) and the
  one-dimensional zigzag kernel of the border-vertex polyline (`polyline_kernel`), where the
  literature has the binary analogues (Havelock's locales, Dorst–Smeulders domains);
* the **sharp bias constant** `3/2 − √2` of the ½-crossing reading;
* the end-to-end chain from Lebesgue pixel areas to boundary points and to the code's own
  constants (`strip_reading_exact`, `histopolate_cubic`); the cubic exactness of the
  correction itself is classical (fourth-order finite-volume reconstruction);
* the **supersampled** version of the column-sum theorem with its `1/(2n)` floor.

It does **not** prove that inkvec is the best tracer, and no theorem could: "best" is an
empirical claim about real inputs and a chosen metric. What it does establish is narrower
and checkable: within the box-filter model, which readings are exact, which are biased and
by how much, what no estimator can recover, and that the parts of inkvec built on these
theorems compute what the theorems say.

## Generated checkers

The engine's solvers are free; a result a theorem speaks about is used only if a checker
generated from the definition the theorem is about accepts it
([`docs/theory/verification.md`](../../docs/theory/verification.md)). `InkvecTheory/Gen/` holds
the bridge: `Expr`, a straight-line expression language with a real semantics (what theorems
are about), an exact rational one and an interval one, with their soundness theorems; a
printer to Rust; and the kernels (`Gen/Strip.lean`). To regenerate:

```bash
lake build InkvecTheory && lake env lean InkvecTheory/Gen/Emit.lean
```

which writes `crates/inkvec-verified/src/generated/`. CI runs it and fails on any difference
from the committed files, so the Rust cannot drift from the definitions. The trusted base is
stated in `Gen/Expr.lean`: the Rust runtime (`iv.rs`, `q.rs`) implements the operations
`Enclosing` and ℚ model (IEEE 754's correctly rounded `+ − × ÷ √`, widened one float outward),
and the printer maps each constructor to one runtime call.
