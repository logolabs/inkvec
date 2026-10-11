/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import InkvecTheory.Basic
import Mathlib.Probability.Moments.Variance

/-!
# The strip reading under pixel noise

A column sum is linear in the pixels, so pixel noise passes through it without bias and with
a variance that adds up; the cell-mean-to-point and face-value corrections are linear too. A
½-crossing reading is a ratio of noisy pixels and has neither property. This file proves the
linear half:

* `strip_unbiased`: zero-mean noise on each pixel leaves the column sum's mean exact.
* `strip_variance`: `k` pixels with independent noise of variance `σ²` give the sum variance
  `k·σ²`.
* `weighted_variance`, `face_value_noise`: a fixed linear combination of independent column
  sums has variance `Σ wᵢ²·V`; for the face value of `strip.rs` that is `25/36·V`, less than
  one column's own variance, and for the cell-mean-to-point correction `113/96·V`.

The noise model is explicit and minimal: integrable, zero-mean, pairwise independent,
square-integrable where variances are claimed. Nothing about Gaussianity is assumed.
-/

namespace Inkvec

open MeasureTheory ProbabilityTheory Finset

variable {Ω : Type*} [MeasurableSpace Ω] {μ : Measure Ω} [IsProbabilityMeasure μ]

/-- **The column sum is unbiased.** If pixel `i` of a window reads its true coverage `a i` plus
zero-mean noise `ε i`, the expected column sum is the true column sum, hence (by
`column_sum_eq_average`) the boundary's column mean. -/
theorem strip_unbiased (k : ℕ) (a : Fin k → ℝ) (ε : Fin k → Ω → ℝ)
    (hint : ∀ i, Integrable (ε i) μ) (h0 : ∀ i, ∫ ω, ε i ω ∂μ = 0) :
    ∫ ω, ∑ i, (a i + ε i ω) ∂μ = ∑ i, a i := by
  have h := integral_finsetSum (μ := μ) univ (f := fun i ω => a i + ε i ω)
    (fun i _ => (integrable_const _).add (hint i))
  rw [h]
  refine sum_congr rfl (fun i _ => ?_)
  rw [integral_add (integrable_const _) (hint i), integral_const, h0 i]
  simp

omit [IsProbabilityMeasure μ] in
/-- **Variance of a weighted sum of independent readings.** -/
theorem weighted_variance {ι : Type*} (s : Finset ι) (w : ι → ℝ) (X : ι → Ω → ℝ)
    (hL2 : ∀ i ∈ s, MemLp (X i) 2 μ) (hind : Set.Pairwise ↑s fun i j => X i ⟂ᵢ[μ] X j) :
    variance (fun ω => ∑ i ∈ s, w i * X i ω) μ = ∑ i ∈ s, w i ^ 2 * variance (X i) μ := by
  have hsum : (fun ω => ∑ i ∈ s, w i * X i ω) = ∑ i ∈ s, (fun ω => w i * X i ω) := by
    funext ω; simp [Finset.sum_apply]
  rw [hsum, IndepFun.variance_sum (fun i hi => (hL2 i hi).const_mul (w i))
    (fun i hi j hj hij => (hind hi hj hij).comp (measurable_const_mul (w i))
      (measurable_const_mul (w j)))]
  exact sum_congr rfl (fun i _ => variance_const_mul (w i) (X i) μ)

/-- **Variance of the column sum.** `k` pixels with pairwise independent noise of variance
`σ²` each give the column sum the variance `k·σ²`. -/
theorem strip_variance (k : ℕ) (a : Fin k → ℝ) (ε : Fin k → Ω → ℝ) (σ : ℝ)
    (hL2 : ∀ i, MemLp (ε i) 2 μ) (hind : Pairwise fun i j => ε i ⟂ᵢ[μ] ε j)
    (hvar : ∀ i, variance (ε i) μ = σ ^ 2) :
    variance (fun ω => ∑ i, (a i + ε i ω)) μ = k * σ ^ 2 := by
  have hshift : (fun ω => ∑ i, (a i + ε i ω)) = fun ω => (∑ i, 1 * ε i ω) + ∑ i, a i := by
    funext ω; rw [sum_add_distrib]; simp [add_comm]
  have hmeas : AEStronglyMeasurable (fun ω => ∑ i, 1 * ε i ω) μ :=
    Finset.aestronglyMeasurable_fun_sum _ (fun i _ => ((hL2 i).const_mul 1).aestronglyMeasurable)
  rw [hshift, variance_add_const hmeas,
    weighted_variance univ (fun _ => 1) ε (fun i _ => hL2 i)
      (fun i _ j _ hij => hind hij)]
  simp [hvar]

omit [IsProbabilityMeasure μ] in
/-- **The face value averages noise down.** With the four column sums of `strip.rs`'s
`histopolate` carrying pairwise independent errors of equal variance `V`, the boundary height
read at the pixel border, `(7(S₁ + S₂) - (S₀ + S₃))/12`, has error variance `25/36·V`. -/
theorem face_value_noise (X : Fin 4 → Ω → ℝ) (V : ℝ) (hL2 : ∀ i, MemLp (X i) 2 μ)
    (hind : Pairwise fun i j => X i ⟂ᵢ[μ] X j) (hvar : ∀ i, variance (X i) μ = V) :
    variance (fun ω => ∑ i, ![-1 / 12, 7 / 12, 7 / 12, -1 / 12] i * X i ω) μ = 25 / 36 * V := by
  rw [weighted_variance univ _ X (fun i _ => hL2 i) (fun i _ j _ hij => hind hij)]
  simp [Fin.sum_univ_four, hvar]
  ring

omit [IsProbabilityMeasure μ] in
/-- The cell-mean-to-point correction `A - (A₊ - 2A + A₋)/24` of `Deconvolution.lean`, with
weights `-1/24, 13/12, -1/24` on three pairwise independent means of variance `V`, has error
variance `((1/24)² + (13/12)² + (1/24)²)·V = 113/96·V`. -/
theorem point_from_means_noise (X : Fin 3 → Ω → ℝ) (V : ℝ) (hL2 : ∀ i, MemLp (X i) 2 μ)
    (hind : Pairwise fun i j => X i ⟂ᵢ[μ] X j) (hvar : ∀ i, variance (X i) μ = V) :
    variance (fun ω => ∑ i, ![-1 / 24, 13 / 12, -1 / 24] i * X i ω) μ = 113 / 96 * V := by
  rw [weighted_variance univ _ X (fun i _ => hL2 i) (fun i _ j _ hij => hind hij)]
  simp [Fin.sum_univ_three, hvar]
  ring

end Inkvec
