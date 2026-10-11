/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import InkvecTheory.ThinStroke

/-!
# Windows: any set of whole pixels measures an area

The boundary chain scores a candidate description against the pixels by windows
(`docs/theory/chain-boundary.md`, B2.1; `crates/inkvec-trace/src/evidence.rs`):

* `window_sum_eq_area`: summed over any finite set of pixels, a region's coverages are its
  area inside their union, for every measurable region. No graph, slope or threshold enters;
  `column_sum_eq_average` is the case of a graph and one column.
* `trapezoid_integral`: the integral of the straight piece from `(ua, va)` to `(ub, vb)`,
  measured from a level `vlo`, is the trapezoid `(ub − ua)((va − vlo) + (vb − vlo))/2`: the
  per-piece area the window checker sums (`Gen/Evidence.lean`, `trapezoidK`).
-/

namespace Inkvec

open Set MeasureTheory Finset

lemma measurableSet_pixel (i j : ℤ) : MeasurableSet (pixel i j) :=
  measurableSet_Ioo.prod measurableSet_Ioo

lemma disjoint_Ioo_int {i i' : ℤ} (h : i ≠ i') :
    Disjoint (Ioo (i : ℝ) (i + 1)) (Ioo (i' : ℝ) (i' + 1)) := by
  rw [Set.disjoint_left]
  intro x hx hx'
  rcases lt_or_gt_of_ne h with hl | hl
  · have : (i : ℝ) + 1 ≤ i' := by exact_mod_cast hl
    linarith [hx.2, hx'.1]
  · have : (i' : ℝ) + 1 ≤ i := by exact_mod_cast hl
    linarith [hx.1, hx'.2]

/-- Distinct pixels are disjoint. -/
lemma disjoint_pixel {p q : ℤ × ℤ} (h : p ≠ q) : Disjoint (pixel p.1 p.2) (pixel q.1 q.2) := by
  unfold pixel
  by_cases h1 : p.1 = q.1
  · have h2 : p.2 ≠ q.2 := fun h2 => h (Prod.ext h1 h2)
    exact Set.disjoint_prod.mpr (Or.inr (disjoint_Ioo_int h2))
  · exact Set.disjoint_prod.mpr (Or.inl (disjoint_Ioo_int h1))

/-- **The window identity.** Summed over any finite set `W` of pixels, the coverages of a
measurable region are its area inside the pixels' union. -/
theorem window_sum_eq_area (Ω : Set (ℝ × ℝ)) (hΩ : MeasurableSet Ω) (W : Finset (ℤ × ℤ)) :
    ∑ p ∈ W, coverage Ω p.1 p.2 = (volume (Ω ∩ ⋃ p ∈ W, pixel p.1 p.2)).toReal := by
  have hfin : ∀ p ∈ W, volume (Ω ∩ pixel p.1 p.2) ≠ ⊤ := fun p _ =>
    ((measure_mono inter_subset_right).trans_lt (volume_pixel_lt_top p.1 p.2)).ne
  unfold coverage
  rw [← ENNReal.toReal_sum hfin, Set.inter_iUnion₂, measure_biUnion_finset]
  · intro p _ q _ hpq
    exact (disjoint_pixel hpq).mono inter_subset_right inter_subset_right
  · intro p _
    exact hΩ.inter (measurableSet_pixel _ _)

/-- **A straight piece's area above a level.** The integral of the affine interpolant from
`(ua, va)` to `(ub, vb)`, less `vlo`, is the trapezoid. -/
theorem trapezoid_integral (ua va ub vb vlo : ℝ) (h : ua ≠ ub) :
    ∫ u in ua..ub, (va + (u - ua) / (ub - ua) * (vb - va) - vlo) =
      (ub - ua) * ((va - vlo) + (vb - vlo)) / 2 := by
  have hd : ub - ua ≠ 0 := sub_ne_zero.mpr (Ne.symm h)
  set k := (vb - va) / (ub - ua) with hk
  have hf : ∀ u : ℝ, va + (u - ua) / (ub - ua) * (vb - va) - vlo = (va - vlo - ua * k) + u * k := by
    intro u; rw [hk]; field_simp; ring
  simp_rw [hf]
  have h1 : IntervalIntegrable (fun _ : ℝ => va - vlo - ua * k) volume ua ub :=
    intervalIntegrable_const
  have h2 : IntervalIntegrable (fun u : ℝ => u * k) volume ua ub :=
    (continuous_id.mul continuous_const).intervalIntegrable _ _
  rw [intervalIntegral.integral_add h1 h2, intervalIntegral.integral_const,
    intervalIntegral.integral_mul_const, integral_id, smul_eq_mul, hk]
  field_simp
  ring

/-! ## Window sums under a blur or a resampling

A resized or blurred intake's pixel `y` reads `Σ_s w(y, s) c(s)` of the exact image's pixels
`c` (`docs/theory/noise.md`, `chain-boundary.md` "Blur"). Summed over a window, the window
identity survives whenever every input pixel the window draws on gives the window its whole
weight (`window_sum_resampled`): an integer-ratio box filter does, a general resampler only
on average over phase, and what it leaves is its phase ripple. Along the strip a centred
kernel leaves a straight edge's area profile alone (`blur_affine_profile`) and moves a
curved one by exactly half its second moment times the profile's second derivative
(`blur_quadratic_profile`): the forward model's correction, not noise. -/

/-- **Mass preservation over a window.** If every input pixel `s ∈ S` gives the window `Y`
its whole weight (`Σ_{y∈Y} w y s = 1`), the window's resampled sum is the inputs' sum. -/
theorem window_sum_resampled (Y S : Finset ℤ) (w : ℤ → ℤ → ℝ) (c : ℤ → ℝ)
    (hw : ∀ s ∈ S, ∑ y ∈ Y, w y s = 1) :
    ∑ y ∈ Y, ∑ s ∈ S, w y s * c s = ∑ s ∈ S, c s := by
  rw [Finset.sum_comm]
  refine Finset.sum_congr rfl fun s hs => ?_
  rw [← Finset.sum_mul, hw s hs, one_mul]

/-- **A centred kernel leaves an affine profile alone.** For weights `k` on a finite set of
offsets with `Σ k = 1` and `Σ k i · i = 0`, the blurred profile of `A x = α + β x` is `A`. -/
theorem blur_affine_profile (I : Finset ℤ) (k : ℤ → ℝ) (hk : ∑ i ∈ I, k i = 1)
    (hm : ∑ i ∈ I, k i * i = 0) (α β x : ℝ) :
    ∑ i ∈ I, k i * (α + β * (x - i)) = α + β * x := by
  have e : ∀ i ∈ I, k i * (α + β * (x - i)) = (α + β * x) * k i - β * (k i * i) := by
    intro i _; ring
  rw [Finset.sum_congr rfl e, Finset.sum_sub_distrib, ← Finset.mul_sum, ← Finset.mul_sum, hk,
    hm]
  ring

/-- **A centred kernel moves a curved profile by `½ μ₂ A''`.** With `μ₂ = Σ k i · i²`, the
blurred profile of `A x = α + β x + γ x²` is `A x + γ μ₂`, and `A'' = 2γ`. -/
theorem blur_quadratic_profile (I : Finset ℤ) (k : ℤ → ℝ) (hk : ∑ i ∈ I, k i = 1)
    (hm : ∑ i ∈ I, k i * i = 0) (α β γ x : ℝ) :
    ∑ i ∈ I, k i * (α + β * (x - i) + γ * (x - i) ^ 2) =
      α + β * x + γ * x ^ 2 + (1 / 2) * (∑ i ∈ I, k i * (i : ℝ) ^ 2) * (2 * γ) := by
  have e : ∀ i ∈ I, k i * (α + β * (x - i) + γ * (x - i) ^ 2) =
      (α + β * x + γ * x ^ 2) * k i - (β + 2 * γ * x) * (k i * i) + γ * (k i * (i : ℝ) ^ 2) := by
    intro i _; ring
  rw [Finset.sum_congr rfl e, Finset.sum_add_distrib, Finset.sum_sub_distrib, ← Finset.mul_sum,
    ← Finset.mul_sum, ← Finset.mul_sum, hk, hm]
  ring

end Inkvec
