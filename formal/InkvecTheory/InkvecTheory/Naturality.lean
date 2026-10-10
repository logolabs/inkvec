/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import Mathlib.Analysis.InnerProductSpace.Projection.Basic
import Mathlib.Analysis.InnerProductSpace.Trace
import Mathlib.LinearAlgebra.Trace

/-!
# Naturality: the smaller true description is the more accurate one

A trace is a least-squares fit of a model to measured boundary points. Write the measurement
as `y = s + ε`: the true boundary `s` plus noise. Locally (about the fitted curve) the curves a
segmentation can draw form a linear space of the dimension of its parameter count, and the fit
is the orthogonal projection of `y` onto it. Merging two segments into one (two arcs on one
circle into one arc, two collinear lines into one) passes to a *subspace*: every curve the
merged description can draw, the split one can draw too.

The question this file answers is whether a merge that keeps the truth drawable can cost
fidelity. It cannot, for any noise at all, and what it gains is exactly the noise the extra
parameters had absorbed:

* `natural_model_closer`: if `s ∈ K ≤ L`, the fit onto `K` is at least as close to `s` as
  the fit onto `L`, for every noise `ε`: pointwise, not in expectation.
* `natural_model_gain`: the two squared errors differ by exactly `‖P_L ε - P_K ε‖²`, the
  noise lying in the directions `L` has and `K` lacks.
* `merge_chi2_increase`: the merge raises the fit's residual `‖y - P y‖²` by that same
  amount. So the description-length test the stroke solve's merges apply
  (`½·Δχ² < λ·Δk`, `crates/inkvec-trace/src/ribbon/refine/merge.rs`) reads the noise in the
  removed directions and nothing else when the merge is true.
* `noise_absorbed`: averaged over the directions of any orthonormal basis, a model of
  dimension `k` keeps `k` units of noise: `Σᵢ ‖P_K bᵢ‖² = dim K`. For isotropic noise of
  variance `σ²` per coordinate the expected squared error of the fit is therefore `σ²·dim K`,
  and every parameter a trace spends beyond the truth's costs `σ²` of squared error, while
  the merge test sees `Δχ²/σ²` with mean `Δk`, against a price of `2λ·Δk` (`λ ≈ 8` nats at
  512 px).

That is the formal content of "naturality": among the descriptions that draw the truth, the
one with the fewest parameters is the most faithful, and an MDL test with `λ > ½` accepts it
(in expectation) whenever it draws the truth. The nonlinear curve families inkvec fits are
covered to first order, about the fitted curve; the theorems themselves are exact.
-/

namespace Inkvec

open Submodule
open scoped InnerProductSpace

variable {E : Type*} [NormedAddCommGroup E] [InnerProductSpace ℝ E]

/-- The fit onto `K` minus the truth is the projected noise, when the truth lies in `K`. -/
lemma fit_error_eq {K : Submodule ℝ E} [K.HasOrthogonalProjection] {s : E} (hs : s ∈ K)
    (ε : E) : K.starProjection (s + ε) - s = K.starProjection ε := by
  rw [map_add, starProjection_eq_self_iff.mpr hs, add_sub_cancel_left]

/-- **The natural model is closer, for every noise.** Least squares onto `K ≤ L` of a
measurement `s + ε` whose truth `s` lies in `K` is at least as close to `s` as least squares
onto `L`. -/
theorem natural_model_closer {K L : Submodule ℝ E} [K.HasOrthogonalProjection]
    [L.HasOrthogonalProjection] (hKL : K ≤ L) {s : E} (hs : s ∈ K) (ε : E) :
    ‖K.starProjection (s + ε) - s‖ ≤ ‖L.starProjection (s + ε) - s‖ := by
  rw [fit_error_eq hs, fit_error_eq (hKL hs)]
  have h : K.starProjection (L.starProjection ε) = K.starProjection ε := by
    have := congrArg (fun f => f ε) (starProjection_comp_starProjection_of_le hKL)
    simpa using this
  rw [← h]
  exact norm_starProjection_apply_le _ _

/-- **The gain is the noise in the removed directions.** With `s ∈ K ≤ L`, the larger model's
squared error is the smaller one's plus `‖P_L ε - P_K ε‖²`. -/
theorem natural_model_gain {K L : Submodule ℝ E} [K.HasOrthogonalProjection]
    [L.HasOrthogonalProjection] (hKL : K ≤ L) {s : E} (hs : s ∈ K) (ε : E) :
    ‖L.starProjection (s + ε) - s‖ ^ 2 =
      ‖K.starProjection (s + ε) - s‖ ^ 2 +
        ‖L.starProjection ε - K.starProjection ε‖ ^ 2 := by
  rw [fit_error_eq hs, fit_error_eq (hKL hs)]
  set v := L.starProjection ε
  have hK : K.starProjection v = K.starProjection ε := by
    have := congrArg (fun f => f ε) (starProjection_comp_starProjection_of_le hKL)
    simpa using this
  -- `v = P_K v + (v - P_K v)` with the two parts orthogonal.
  have hperp : ⟪K.starProjection v, v - K.starProjection v⟫_ℝ = 0 := by
    rw [real_inner_comm]
    exact starProjection_inner_eq_zero v _ (starProjection_apply_mem K v)
  have := norm_add_sq_eq_norm_sq_add_norm_sq_of_inner_eq_zero
    (K.starProjection v) (v - K.starProjection v) hperp
  rw [add_sub_cancel] at this
  rw [← hK]
  simpa [sq] using this

/-- **The merge's price in residual.** With `s ∈ K ≤ L` and `y = s + ε`, the residual of the
fit onto `K` exceeds the residual onto `L` by `‖P_L ε - P_K ε‖²`: the description-length
test of a true merge reads only the noise in the directions it removes. -/
theorem merge_chi2_increase {K L : Submodule ℝ E} [K.HasOrthogonalProjection]
    [L.HasOrthogonalProjection] (hKL : K ≤ L) {s : E} (hs : s ∈ K) (ε : E) :
    ‖(s + ε) - K.starProjection (s + ε)‖ ^ 2 =
      ‖(s + ε) - L.starProjection (s + ε)‖ ^ 2 +
        ‖L.starProjection ε - K.starProjection ε‖ ^ 2 := by
  set y := s + ε
  -- `y - P_K y = (y - P_L y) + (P_L y - P_K y)`, the first part orthogonal to `L ⊇ K`.
  have hK : K.starProjection (L.starProjection y) = K.starProjection y := by
    have := congrArg (fun f => f y) (starProjection_comp_starProjection_of_le hKL)
    simpa using this
  have hdiff : L.starProjection y - K.starProjection y =
      L.starProjection ε - K.starProjection ε := by
    simp only [y, map_add, starProjection_eq_self_iff.mpr hs,
      starProjection_eq_self_iff.mpr (hKL hs)]
    abel
  have hperp : ⟪y - L.starProjection y, L.starProjection y - K.starProjection y⟫_ℝ = 0 := by
    apply starProjection_inner_eq_zero
    exact L.sub_mem (starProjection_apply_mem L y) (hKL (starProjection_apply_mem K y))
  have := norm_add_sq_eq_norm_sq_add_norm_sq_of_inner_eq_zero
    (y - L.starProjection y) (L.starProjection y - K.starProjection y) hperp
  rw [sub_add_sub_cancel] at this
  rw [← hdiff]
  simpa [sq] using this

/-- **Each dimension keeps one unit of noise.** For any orthonormal basis `b` of a
finite-dimensional space, `Σᵢ ‖P_K bᵢ‖² = dim K`. With isotropic noise `ε = Σᵢ ξᵢ bᵢ`
(uncorrelated `ξᵢ` of variance `σ²`) the fit's expected squared error is `σ²` times this. -/
theorem noise_absorbed [FiniteDimensional ℝ E] {ι : Type*} [Fintype ι] (K : Submodule ℝ E)
    (b : OrthonormalBasis ι ℝ E) :
    ∑ i, ‖K.starProjection (b i)‖ ^ 2 = (Module.finrank ℝ K : ℝ) := by
  have hproj : LinearMap.IsProj K (K.starProjection : E →ₗ[ℝ] E) :=
    ⟨fun x => starProjection_apply_mem K x,
      fun x hx => starProjection_eq_self_iff.mpr hx⟩
  rw [← hproj.trace, LinearMap.trace_eq_sum_inner _ b]
  refine Finset.sum_congr rfl (fun i _ => ?_)
  have := re_inner_starProjection_eq_normSq (K := K) (b i)
  simp only [RCLike.re_to_real] at this
  rw [real_inner_comm, ContinuousLinearMap.coe_coe, this, starProjection_apply,
    norm_coe]

end Inkvec
