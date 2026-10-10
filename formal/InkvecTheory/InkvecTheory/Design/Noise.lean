/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import Mathlib.Analysis.SpecialFunctions.Log.Basic
import Mathlib.Tactic

/-!
# Noise and constraints: a constraint accepted on clean input stays accepted on noisier input

`docs/theory/chain-representation.md`, R0.4 (noise). A description `M₀` (a tie, a snap to the
design grid, a dropped coordinate) is nested in `M₁`, which has `k` more free coordinates.
With the free coordinates integrated out (Laplace), the log posterior odds of `M₀` over `M₁`
at noise scale `σ` are

  `logOdds k Δ a c σ = c − Δ / (2σ²) + k · log (a / σ)`,

where `Δ ≥ 0` is the misfit the constraint costs (in px², the drop in the residual sum of
squares `M₁` buys), `k · log (a / σ)` the Occam factor of the `k` coordinates (`a / σ` is the
ratio of a coordinate's prior width to its posterior width; `a = W / (√(2π) g)` for a range
`W` and a geometry factor `g`), and `c` the log prior odds of the constraint (the design prior).

* `accept_monotone`: for a fixed misfit `Δ`, if the constraint is accepted at `σ₁` it is
  accepted at every `σ₂ ≥ σ₁`, as long as at `σ₂` the coordinates still cost at least half a
  nat each beyond the prior odds (`k / 2 ≤ c + k · log (a / σ₂)`, the regime in which the
  Laplace approximation is meaningful). On noisy input a tie or a snap costs less fit, and
  the description falls back on the design prior.
* `huber_eq_sq`: the Huber loss with threshold `κ` is the Gaussian `u² / 2` on residuals
  within `κ` standard units, so on such residuals `accept_monotone` holds for it verbatim.
-/

namespace Inkvec.Design

open Real

/-- Log posterior odds of the constrained description over the free one (see the module
docs). -/
noncomputable def logOdds (k Δ a c σ : ℝ) : ℝ := c - Δ / (2 * σ ^ 2) + k * Real.log (a / σ)

/-- **Accepted once, accepted at more noise.** -/
theorem accept_monotone {k Δ a c σ₁ σ₂ : ℝ} (hk : 0 ≤ k) (ha : 0 < a)
    (h₁ : 0 < σ₁) (h₁₂ : σ₁ ≤ σ₂)
    (hacc : 0 ≤ logOdds k Δ a c σ₁) (hreg : k / 2 ≤ c + k * Real.log (a / σ₂)) :
    0 ≤ logOdds k Δ a c σ₂ := by
  have h₂ : 0 < σ₂ := lt_of_lt_of_le h₁ h₁₂
  unfold logOdds at *
  by_cases hcase : k * σ₂ ^ 2 ≤ Δ
  · -- The misfit is still significant at `σ₂`: the odds have only risen.
    have ht : 0 < σ₂ / σ₁ := div_pos h₂ h₁
    have hlog : Real.log (a / σ₁) - Real.log (a / σ₂) ≤ ((σ₂ / σ₁) ^ 2 - 1) / 2 := by
      have e : Real.log (a / σ₁) - Real.log (a / σ₂) = Real.log (σ₂ / σ₁) := by
        rw [← Real.log_div (div_pos ha h₁).ne' (div_pos ha h₂).ne']
        congr 1
        field_simp
      rw [e]
      have hl := Real.log_le_sub_one_of_pos (pow_pos ht 2)
      rw [Real.log_pow] at hl
      push_cast at hl
      linarith
    have hsq : 0 ≤ σ₂ ^ 2 - σ₁ ^ 2 := by nlinarith
    have key : k * (((σ₂ / σ₁) ^ 2 - 1) / 2) ≤ Δ / (2 * σ₁ ^ 2) - Δ / (2 * σ₂ ^ 2) := by
      have e1 : Δ / (2 * σ₁ ^ 2) - Δ / (2 * σ₂ ^ 2) =
          Δ * (σ₂ ^ 2 - σ₁ ^ 2) / (2 * σ₁ ^ 2 * σ₂ ^ 2) := by
        field_simp
      have e2 : k * (((σ₂ / σ₁) ^ 2 - 1) / 2) =
          k * σ₂ ^ 2 * (σ₂ ^ 2 - σ₁ ^ 2) / (2 * σ₁ ^ 2 * σ₂ ^ 2) := by
        field_simp
      rw [e1, e2]
      apply div_le_div_of_nonneg_right _ (by positivity)
      exact mul_le_mul_of_nonneg_right hcase hsq
    nlinarith [mul_le_mul_of_nonneg_left hlog hk]
  · -- The misfit is within noise at `σ₂`: the prices alone carry the constraint.
    push Not at hcase
    have : Δ / (2 * σ₂ ^ 2) < k / 2 := by
      rw [div_lt_iff₀ (by positivity)]
      nlinarith
    linarith

/-- The Huber loss of a residual `u` in standard units, threshold `κ`. -/
noncomputable def huber (κ u : ℝ) : ℝ := if |u| ≤ κ then u ^ 2 / 2 else κ * |u| - κ ^ 2 / 2

/-- **Huber is Gaussian on inliers**: within `κ` standard units the robust cost is the
squared one, so clean inputs (every residual within `κ`) keep their behaviour and
`accept_monotone` applies to them unchanged. -/
theorem huber_eq_sq {κ u : ℝ} (h : |u| ≤ κ) : huber κ u = u ^ 2 / 2 := by
  simp [huber, h]

/-- An inlier at `σ₁` stays an inlier at any larger noise scale. -/
theorem inlier_of_le {κ r σ₁ σ₂ : ℝ} (h₁ : 0 < σ₁) (h₁₂ : σ₁ ≤ σ₂)
    (h : |r / σ₁| ≤ κ) : |r / σ₂| ≤ κ := by
  have h₂ : 0 < σ₂ := lt_of_lt_of_le h₁ h₁₂
  rw [abs_div, abs_of_pos h₂]
  rw [abs_div, abs_of_pos h₁, div_le_iff₀ h₁] at h
  rw [div_le_iff₀ h₂]
  have hκ : 0 ≤ κ := by nlinarith [abs_nonneg r]
  nlinarith [mul_le_mul_of_nonneg_left h₁₂ hκ]

/-- The Huber loss never exceeds the Gaussian one: an outlier costs less evidence. -/
theorem huber_le_sq (κ u : ℝ) : huber κ u ≤ u ^ 2 / 2 := by
  unfold huber
  split_ifs with h
  · exact le_rfl
  · push Not at h
    nlinarith [abs_nonneg u, sq_abs u]

end Inkvec.Design
