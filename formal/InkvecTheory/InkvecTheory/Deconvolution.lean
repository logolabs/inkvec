/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import InkvecTheory.Coverage

/-!
# From column sums to boundary points, exactly

`column_sum_eq_average` gives each pixel column's *mean* boundary height. A mean is not a
point: for a curved boundary it is off the height at the column centre by `f''/24` plus
higher terms. The finite-volume correction

  f(c) = A(c) - (A(c+1) - 2·A(c) + A(c-1)) / 24

(cell averages to point values, as in high-order finite-volume schemes) removes that, and
`cubic_point_from_means` proves it is **exact for every cubic**, not merely fourth-order
accurate. `quartic_defect` pins the first term it misses: `-3/640 · f''''`.

`boundary_point_from_pixels` chains it with the column-sum theorem: from the raw pixel areas
of three adjacent columns, the boundary height at the middle column's centre, exact for any
cubic boundary and with no normal direction, threshold or iteration involved. The correction
is the fourth-order finite-volume one (McCorquodale & Colella 2011) and Trujillo-Pino et al.'s
(2013) centre coefficient; what this file adds is the machine-checked chain from pixel areas
to the point, and to the constants of `strip.rs`.
-/

namespace Inkvec

open Set MeasureTheory Finset

/-- A cubic, written out. -/
def cubic (c₀ c₁ c₂ c₃ : ℝ) (x : ℝ) : ℝ := c₀ + c₁ * x + c₂ * x ^ 2 + c₃ * x ^ 3

/-- Its antiderivative. -/
noncomputable def cubicPrim (c₀ c₁ c₂ c₃ : ℝ) (x : ℝ) : ℝ :=
  c₀ * x + c₁ * x ^ 2 / 2 + c₂ * x ^ 3 / 3 + c₃ * x ^ 4 / 4

lemma continuous_cubic (c₀ c₁ c₂ c₃ : ℝ) : Continuous (cubic c₀ c₁ c₂ c₃) := by
  unfold cubic; fun_prop

lemma hasDerivAt_cubicPrim (c₀ c₁ c₂ c₃ x : ℝ) :
    HasDerivAt (cubicPrim c₀ c₁ c₂ c₃) (cubic c₀ c₁ c₂ c₃ x) x := by
  have h := ((((hasDerivAt_id x).const_mul c₀).add
    (((hasDerivAt_pow 2 x).const_mul c₁).div_const 2)).add
    (((hasDerivAt_pow 3 x).const_mul c₂).div_const 3)).add
    (((hasDerivAt_pow 4 x).const_mul c₃).div_const 4)
  unfold cubicPrim cubic
  convert h using 1
  · funext y; simp
  · simp; ring

lemma integral_cubic (c₀ c₁ c₂ c₃ a b : ℝ) :
    ∫ x in a..b, cubic c₀ c₁ c₂ c₃ x = cubicPrim c₀ c₁ c₂ c₃ b - cubicPrim c₀ c₁ c₂ c₃ a :=
  intervalIntegral.integral_eq_sub_of_hasDerivAt (fun x _ => hasDerivAt_cubicPrim c₀ c₁ c₂ c₃ x)
    ((continuous_cubic c₀ c₁ c₂ c₃).intervalIntegrable _ _)

/-- The mean of `f` over the unit cell centred at `c`. -/
noncomputable def cellMean (f : ℝ → ℝ) (c : ℝ) : ℝ := ∫ x in (c - 1 / 2)..(c + 1 / 2), f x

/-- The cell-mean-to-point correction. -/
noncomputable def pointFromMeans (Aₗ A Aᵣ : ℝ) : ℝ := A - (Aᵣ - 2 * A + Aₗ) / 24

/-- **Exact for cubics.** The cell-mean-to-point correction returns the value at the centre of
any cubic, at any centre. -/
theorem cubic_point_from_means (c₀ c₁ c₂ c₃ c : ℝ) :
    pointFromMeans (cellMean (cubic c₀ c₁ c₂ c₃) (c - 1)) (cellMean (cubic c₀ c₁ c₂ c₃) c)
      (cellMean (cubic c₀ c₁ c₂ c₃) (c + 1)) = cubic c₀ c₁ c₂ c₃ c := by
  unfold pointFromMeans cellMean
  rw [integral_cubic, integral_cubic, integral_cubic]
  unfold cubicPrim cubic
  ring

/-- The plain cell mean is off by the curvature: `f''(c)/24` for a cubic. -/
theorem cubic_mean_bias (c₀ c₁ c₂ c₃ c : ℝ) :
    cellMean (cubic c₀ c₁ c₂ c₃) c - cubic c₀ c₁ c₂ c₃ c = (2 * c₂ + 6 * c₃ * c) / 24 := by
  unfold cellMean
  rw [integral_cubic]
  unfold cubicPrim cubic
  ring

/-- **The first term the correction misses.** For `x⁴` (fourth derivative 24) at centre 0
the correction is off by `-9/80 = -(3/640)·24`: the method's error constant is `3/640`
times the fourth derivative, about `0.0047 · f''''`. -/
theorem quartic_defect :
    pointFromMeans (cellMean (fun x => x ^ 4) (-1)) (cellMean (fun x => x ^ 4) 0)
      (cellMean (fun x => x ^ 4) 1) - (0 : ℝ) ^ 4 = -(3 / 640) * 24 := by
  unfold pointFromMeans cellMean
  simp only [integral_pow]
  norm_num

/-- **Boundary point from raw pixel areas.** Let a cubic boundary stay within rows
`j₀ … j₀+m` over columns `i-1, i, i+1`, and let `Sₖ` be the sum of the box-filter coverages
of column `k` over those rows. Then the boundary passes through
`(i + ½, j₀ + Sᵢ - (Sᵢ₊₁ - 2·Sᵢ + Sᵢ₋₁)/24)`, exactly. -/
theorem boundary_point_from_pixels (c₀ c₁ c₂ c₃ : ℝ) (i j₀ : ℤ) (m : ℕ)
    (hlo : ∀ x ∈ Icc ((i : ℝ) - 1) (i + 2), (j₀ : ℝ) ≤ cubic c₀ c₁ c₂ c₃ x)
    (hhi : ∀ x ∈ Icc ((i : ℝ) - 1) (i + 2), cubic c₀ c₁ c₂ c₃ x ≤ j₀ + m) :
    let S := fun k : ℤ => ∑ r ∈ range m, coverage (subgraph (cubic c₀ c₁ c₂ c₃)) k (j₀ + r)
    cubic c₀ c₁ c₂ c₃ (i + 1 / 2) = j₀ + pointFromMeans (S (i - 1)) (S i) (S (i + 1)) := by
  intro S
  have hc := continuous_cubic c₀ c₁ c₂ c₃
  have hS : ∀ k : ℤ, (i : ℝ) - 1 ≤ k → (k : ℝ) + 1 ≤ i + 2 →
      S k = cellMean (cubic c₀ c₁ c₂ c₃) (k + 1 / 2) - j₀ := by
    intro k hk1 hk2
    have := column_sum_eq_average _ hc k j₀ m
      (fun x hx => hlo x ⟨by linarith [hx.1], by linarith [hx.2]⟩)
      (fun x hx => hhi x ⟨by linarith [hx.1], by linarith [hx.2]⟩)
    unfold cellMean
    rw [show (k : ℝ) + 1 / 2 - 1 / 2 = k by ring, show (k : ℝ) + 1 / 2 + 1 / 2 = k + 1 by ring]
    exact this
  rw [hS (i - 1) (by push_cast; linarith) (by push_cast; linarith),
    hS i (by linarith) (by linarith), hS (i + 1) (by push_cast; linarith) (by push_cast; linarith)]
  have key := cubic_point_from_means c₀ c₁ c₂ c₃ ((i : ℝ) + 1 / 2)
  push_cast
  rw [show (i : ℝ) - 1 + 1 / 2 = (i : ℝ) + 1 / 2 - 1 by ring,
    show (i : ℝ) + 1 + 1 / 2 = (i : ℝ) + 1 / 2 + 1 by ring]
  unfold pointFromMeans at key ⊢
  linarith

end Inkvec

/-! ### The constants of `crates/inkvec-trace/src/planar/strip.rs` -/

namespace Inkvec

/-- `strip.rs`'s `histopolate`: the cubic coefficients from the means over the cells
`[-2,-1]`, `[-1,0]`, `[0,1]`, `[1,2]`, with the code's rational constants. -/
noncomputable def histopolate (m₀ m₁ m₂ m₃ : ℝ) : ℝ × ℝ × ℝ × ℝ :=
  ((-m₀ + 7 * m₁ + 7 * m₂ - m₃) / 12,
   (m₀ - 15 * m₁ + 15 * m₂ - m₃) / 12,
   (m₀ - m₁ - m₂ + m₃) / 4,
   (-m₀ + 3 * m₁ - 3 * m₂ + m₃) / 6)

/-- The mean of `f` over the unit cell `[k, k+1]`. -/
noncomputable def cellMean' (f : ℝ → ℝ) (k : ℝ) : ℝ := ∫ x in k..(k + 1), f x

/-- **`strip.rs`'s constants are exact.** Fed the four cell means of any cubic, `histopolate`
returns that cubic's coefficients. With `poly_eq_of_averages` this is the whole correctness
argument of the strip reading on a cubic boundary: the column sums are the cell means
(`column_sum_eq_average`), and the cubic rebuilt from them is the boundary. -/
theorem histopolate_cubic (c₀ c₁ c₂ c₃ : ℝ) :
    histopolate (cellMean' (cubic c₀ c₁ c₂ c₃) (-2)) (cellMean' (cubic c₀ c₁ c₂ c₃) (-1))
      (cellMean' (cubic c₀ c₁ c₂ c₃) 0) (cellMean' (cubic c₀ c₁ c₂ c₃) 1) = (c₀, c₁, c₂, c₃) := by
  unfold histopolate cellMean'
  simp only [integral_cubic]
  unfold cubicPrim
  refine Prod.ext ?_ (Prod.ext ?_ (Prod.ext ?_ ?_)) <;> simp <;> ring

/-- The constant term alone is the classical fourth-order face value
`(7(m₁ + m₂) - (m₀ + m₃))/12`: the boundary's height at the pixel border between the two
middle cells. -/
theorem face_value_cubic (c₀ c₁ c₂ c₃ : ℝ) :
    (7 * (cellMean' (cubic c₀ c₁ c₂ c₃) (-1) + cellMean' (cubic c₀ c₁ c₂ c₃) 0) -
      (cellMean' (cubic c₀ c₁ c₂ c₃) (-2) + cellMean' (cubic c₀ c₁ c₂ c₃) 1)) / 12 =
      cubic c₀ c₁ c₂ c₃ 0 := by
  unfold cellMean'
  simp only [integral_cubic]
  unfold cubicPrim cubic
  ring

/-- The one-sided stencils of `strip.rs`'s corner test agree with the central one on every
cubic: the cubic histopolated from cells `[-3,-2] … [0,1]`, read one cell to the right of its
own origin, is the same cubic's value at `0`. (The right-hand stencil is the mirror image.) -/
theorem side_stencil_cubic (c₀ c₁ c₂ c₃ : ℝ) :
    let h := histopolate (cellMean' (cubic c₀ c₁ c₂ c₃) (-3))
      (cellMean' (cubic c₀ c₁ c₂ c₃) (-2)) (cellMean' (cubic c₀ c₁ c₂ c₃) (-1))
      (cellMean' (cubic c₀ c₁ c₂ c₃) 0)
    cubic h.1 h.2.1 h.2.2.1 h.2.2.2 1 = cubic c₀ c₁ c₂ c₃ 0 := by
  intro h
  simp only [h, histopolate, cellMean', integral_cubic]
  unfold cubicPrim cubic
  ring

end Inkvec

namespace Inkvec

open Set MeasureTheory Finset

/-- **The strip reading is exact, end to end.** Let a cubic boundary, written in coordinates
local to the pixel border `x = X₀` as `y = cubic c₀ c₁ c₂ c₃ (x - X₀)`, stay within rows
`j₀ … j₀+m` over the four columns `X₀-2 … X₀+1`. Sum each column's box-filter coverages over
those rows, add back `j₀`, and feed the four results to `strip.rs`'s `histopolate`: out come
the boundary's own coefficients. The vertex `strip.rs` then slides onto this cubic is on the
boundary. -/
theorem strip_reading_exact (c₀ c₁ c₂ c₃ : ℝ) (X₀ j₀ : ℤ) (m : ℕ)
    (hlo : ∀ x ∈ Icc ((X₀ : ℝ) - 2) (X₀ + 2), (j₀ : ℝ) ≤ cubic c₀ c₁ c₂ c₃ (x - X₀))
    (hhi : ∀ x ∈ Icc ((X₀ : ℝ) - 2) (X₀ + 2), cubic c₀ c₁ c₂ c₃ (x - X₀) ≤ j₀ + m) :
    let S := fun k : ℤ =>
      (j₀ : ℝ) + ∑ r ∈ range m, coverage (subgraph fun x => cubic c₀ c₁ c₂ c₃ (x - X₀)) (X₀ + k) (j₀ + r)
    histopolate (S (-2)) (S (-1)) (S 0) (S 1) = (c₀, c₁, c₂, c₃) := by
  intro S
  have hc : Continuous fun x => cubic c₀ c₁ c₂ c₃ (x - X₀) :=
    (continuous_cubic c₀ c₁ c₂ c₃).comp (continuous_id.sub continuous_const)
  have hS : ∀ k : ℤ, -2 ≤ k → k ≤ 1 → S k = cellMean' (cubic c₀ c₁ c₂ c₃) k := by
    intro k hk1 hk2
    have hk1' : (-2 : ℝ) ≤ k := by exact_mod_cast hk1
    have hk2' : (k : ℝ) ≤ 1 := by exact_mod_cast hk2
    have h := column_sum_eq_average _ hc (X₀ + k) j₀ m
      (fun x hx => hlo x ⟨by push_cast at hx; linarith [hx.1], by push_cast at hx; linarith [hx.2]⟩)
      (fun x hx => hhi x ⟨by push_cast at hx; linarith [hx.1], by push_cast at hx; linarith [hx.2]⟩)
    show (j₀ : ℝ) + _ = _
    rw [h]
    unfold cellMean'
    have hsub := intervalIntegral.integral_comp_sub_right (cubic c₀ c₁ c₂ c₃)
      (a := ((X₀ + k : ℤ) : ℝ)) (b := ((X₀ + k : ℤ) : ℝ) + 1) (X₀ : ℝ)
    rw [hsub]
    push_cast
    rw [show (X₀ : ℝ) + k - X₀ = k by ring, show (X₀ : ℝ) + k + 1 - X₀ = k + 1 by ring]
    ring
  rw [hS (-2) (by norm_num) (by norm_num), hS (-1) (by norm_num) (by norm_num),
    hS 0 (by norm_num) (by norm_num), hS 1 (by norm_num) (by norm_num)]
  have := histopolate_cubic c₀ c₁ c₂ c₃
  push_cast at this ⊢
  exact this

end Inkvec
