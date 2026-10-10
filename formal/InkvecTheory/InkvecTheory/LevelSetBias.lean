/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import InkvecTheory.Coverage

/-!
# The bias of reading an edge as the ½-crossing of interpolated coverage

Most sub-pixel tracers (and inkvec's own bilevel contour, `contour.rs`, and its
`root_find_half` fallback in `planar.rs`) put the boundary where the *linear interpolation*
of pixel-centre coverages crosses ½. Even on the easiest input there is, a straight
axis-aligned step edge with no noise, that reading is biased:

* `levelSet_reading`: an edge `δ` above the centre of its pixel is read at
  `δ/(½+δ)` instead of `δ`;
* `levelSetBias_le`, `levelSetBias_max`: the error is at most `3/2 - √2 ≈ 0.0858 px`, and
  that bound is attained (at `δ = (√2-1)/2`).

`column_sum_reading` is the alternative this theory recommends: the column sum reads the
same edge exactly, for every `δ`.

`docs/algorithm/02-coverage.md` measures "roughly 0.05 px" for level-set extraction on
analytic circles and makes it `DEFAULT_SIGMA_MODEL`; this file shows that floor is a
property of the reading, not of the data.
-/

namespace Inkvec

open Set MeasureTheory Finset Real

/-- The coverage of a horizontal edge at height `e` in pixel `(i, j)` is `clamp01 (e - j)`. -/
lemma coverage_flat (e : ℝ) (i j : ℤ) :
    coverage (subgraph fun _ => e) i j = clamp01 (e - j) := by
  rw [coverage_subgraph _ continuous_const, intervalIntegral.integral_const, smul_eq_mul]
  ring

/-- The error of the ½-crossing reading of an edge `δ` above its pixel's centre. -/
noncomputable def levelSetBias (δ : ℝ) : ℝ := δ / (1 / 2 + δ) - δ

/-- **What the ½-crossing reads.** Let the edge sit at `e = j + ½ + δ`, `0 ≤ δ ≤ ½`, inside
pixel row `j` (centre `j + ½`). Pixel `j` reads `½ + δ` and pixel `j+1` reads `0`; the linear
interpolation between their centres crosses ½ at `e + levelSetBias δ`. -/
theorem levelSet_reading (e : ℝ) (i j : ℤ) (h0 : (j : ℝ) + 1 / 2 ≤ e) (h1 : e ≤ j + 1) :
    let a₀ := coverage (subgraph fun _ => e) i j
    let a₁ := coverage (subgraph fun _ => e) i (j + 1)
    ((j : ℝ) + 1 / 2) + (a₀ - 1 / 2) / (a₀ - a₁) = e + levelSetBias (e - (j + 1 / 2)) := by
  intro a₀ a₁
  have ha₀ : a₀ = e - j := by
    show coverage _ i j = _
    rw [coverage_flat, clamp01_of_mem (by linarith) (by linarith)]
  have ha₁ : a₁ = 0 := by
    show coverage _ i (j + 1) = _
    rw [coverage_flat, clamp01_of_nonpos (by push_cast; linarith)]
  rw [ha₀, ha₁]
  unfold levelSetBias
  have hne : e - (j : ℝ) ≠ 0 := by linarith
  rw [show (1 : ℝ) / 2 + (e - (j + 1 / 2)) = e - j by ring, sub_zero]
  field_simp
  ring

/-- The ½-crossing reading is never below the edge in this configuration, and never more than
`3/2 - √2` px above it. -/
theorem levelSetBias_le (δ : ℝ) (h0 : 0 ≤ δ) (h1 : δ ≤ 1 / 2) :
    0 ≤ levelSetBias δ ∧ levelSetBias δ ≤ 3 / 2 - √2 := by
  unfold levelSetBias
  have hu : (0 : ℝ) < 1 / 2 + δ := by linarith
  have hs : √2 ^ 2 = 2 := sq_sqrt (by norm_num)
  have hs0 : 0 ≤ √2 := sqrt_nonneg 2
  constructor
  · rw [sub_nonneg, le_div_iff₀ hu]; nlinarith
  · rw [sub_le_iff_le_add, div_le_iff₀ hu]
    nlinarith [sq_nonneg (1 / 2 + δ - √2 / 2)]

/-- **The bound is sharp.** At `δ = (√2 - 1)/2` the ½-crossing is `3/2 - √2 ≈ 0.0858` px off. -/
theorem levelSetBias_max : levelSetBias ((√2 - 1) / 2) = 3 / 2 - √2 := by
  unfold levelSetBias
  have hs : √2 ^ 2 = 2 := sq_sqrt (by norm_num)
  have hpos : (0 : ℝ) < √2 := by positivity
  rw [show (1 : ℝ) / 2 + (√2 - 1) / 2 = √2 / 2 by ring]
  field_simp
  nlinarith

/-- `3/2 - √2` lies between `0.0857` and `0.0858`. -/
lemma levelSetBias_max_approx : 0.0857 < 3 / 2 - √2 ∧ 3 / 2 - √2 < 0.0858 := by
  have hs : √2 ^ 2 = 2 := sq_sqrt (by norm_num)
  have h0 : 0 ≤ √2 := sqrt_nonneg 2
  constructor <;> nlinarith

/-- **The column sum reads the same edge exactly.** For an edge anywhere inside rows
`j₀ … j₀+m`, `j₀ + Σ coverages = e`. -/
theorem column_sum_reading (e : ℝ) (i j₀ : ℤ) (m : ℕ) (h0 : (j₀ : ℝ) ≤ e) (h1 : e ≤ j₀ + m) :
    (j₀ : ℝ) + ∑ r ∈ range m, coverage (subgraph fun _ => e) i (j₀ + r) = e := by
  rw [column_sum_eq_average _ continuous_const i j₀ m (fun _ _ => h0) (fun _ _ => h1),
    intervalIntegral.integral_const, smul_eq_mul]
  ring

end Inkvec
