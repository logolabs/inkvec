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

end Inkvec
