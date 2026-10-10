/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import InkvecTheory.LevelSetBias

/-!
# What a tracer that quantises coverage first can never recover

A tracer that thresholds the image (Potrace) or clusters it into flat colours before finding
boundaries (VTracer and most colour tracers) reads each pixel through a quantiser before any
geometry is decided. Those tracers cannot recover what the quantiser removed, whatever
happens afterwards; there is no tunable stage left to recover it (the `thin_features` case
of `docs/M0-BASELINE.md`). This file makes that a lower bound, against an exact upper bound
for coverage:

* `binarize_minimax`: every estimator of an edge's position that reads only the thresholded
  pixels is off by at least `½ - η` px on some edge, for every `η > 0`.
* `quantize_minimax`: every estimator reading pixels through *any* `L`-level quantiser is off
  by at least `δ/2` on some edge, for every `δ < 1/L`. For 8-bit coverage that floor is
  `1/512 px`.
* `column_sum_reading` (in `LevelSetBias.lean`) reads every such edge with error `0`.

The bounds are over the simplest family of shapes there is, a horizontal edge, so they bind
on every richer family that contains it.
-/

namespace Inkvec

open Set MeasureTheory Finset

lemma half_le_clamp01_iff (t : ℝ) : 1 / 2 ≤ clamp01 t ↔ 1 / 2 ≤ t := by
  constructor
  · intro h
    by_contra ht
    push Not at ht
    have : clamp01 t ≤ max 0 t := min_le_right _ _
    have : max 0 t < 1 / 2 := max_lt (by norm_num) ht
    linarith
  · intro h
    rcases le_total t 1 with h1 | h1
    · rw [clamp01_of_mem (by linarith) h1]; exact h
    · rw [clamp01_of_one_le h1]; norm_num

/-- The thresholded column under a horizontal edge at height `e`. -/
noncomputable def binarized (e : ℝ) : ℤ → Bool :=
  fun j => decide (1 / 2 ≤ coverage (subgraph fun _ => e) 0 j)

/-- **Binarisation costs half a pixel.** For any estimator `φ` of the edge height from the
thresholded column and any `η ∈ (0, 1)`, some edge is read at least `(1 - η)/2` px off. -/
theorem binarize_minimax (φ : (ℤ → Bool) → ℝ) (η : ℝ) (hη0 : 0 < η) (hη1 : η < 1) :
    ∃ e : ℝ, (1 - η) / 2 ≤ |φ (binarized e) - e| := by
  set e₁ : ℝ := 1 / 2
  set e₂ : ℝ := 3 / 2 - η
  have hsame : binarized e₁ = binarized e₂ := by
    funext j
    unfold binarized
    rw [coverage_flat, coverage_flat]
    apply decide_eq_decide.mpr
    rw [half_le_clamp01_iff, half_le_clamp01_iff]
    constructor
    · intro h
      have : (j : ℝ) ≤ 0 := by simp only [e₁] at h; linarith
      simp only [e₂]; linarith
    · intro h
      have hj : (j : ℝ) < 1 := by simp only [e₂] at h; linarith
      have hj' : j ≤ 0 := by
        have : j < 1 := by exact_mod_cast hj
        omega
      have : (j : ℝ) ≤ 0 := by exact_mod_cast hj'
      simp only [e₁]; linarith
  by_contra hall
  push Not at hall
  have h1 := hall e₁
  have h2 := hall e₂
  rw [hsame] at h1
  have := abs_sub_le e₂ (φ (binarized e₂)) e₁
  rw [abs_sub_comm e₂ (φ (binarized e₂))] at this
  have hd : |e₂ - e₁| = 1 - η := by
    simp only [e₁, e₂]; rw [abs_of_pos (by linarith)]; ring
  linarith

/-- The column under a horizontal edge at height `e`, read through a per-pixel quantiser. -/
noncomputable def quantized {L : ℕ} (q : ℝ → Fin L) (e : ℝ) : ℤ → Fin L :=
  fun j => q (coverage (subgraph fun _ => e) 0 j)

/-- **Any quantiser costs at least half a level.** For any per-pixel quantiser to `L` levels
(any function at all, not necessarily monotone or uniform), any estimator `φ` reading the
quantised column, and any `δ > 0` with `δ·L < 1`, some edge is read at least `δ/2` px off. -/
theorem quantize_minimax (L : ℕ) (q : ℝ → Fin L) (φ : (ℤ → Fin L) → ℝ) (δ : ℝ) (hδ : 0 < δ)
    (hδL : δ * L < 1) : ∃ e : ℝ, δ / 2 ≤ |φ (quantized q e) - e| := by
  -- `L + 1` edges `kδ` inside one pixel; two of them quantise alike.
  obtain ⟨k₁, k₂, hne, hq⟩ := Fintype.exists_ne_map_eq_of_card_lt
    (fun k : Fin (L + 1) => q ((k : ℝ) * δ)) (by simp)
  have hin : ∀ k : Fin (L + 1), 0 ≤ (k : ℝ) * δ ∧ (k : ℝ) * δ < 1 := by
    intro k
    have hk : (k : ℝ) ≤ L := by exact_mod_cast Nat.lt_succ_iff.mp k.isLt
    exact ⟨by positivity, by nlinarith⟩
  have hsame : quantized q ((k₁ : ℝ) * δ) = quantized q ((k₂ : ℝ) * δ) := by
    funext j
    unfold quantized
    rw [coverage_flat, coverage_flat]
    obtain ⟨a1, b1⟩ := hin k₁
    obtain ⟨a2, b2⟩ := hin k₂
    rcases lt_trichotomy j 0 with hj | rfl | hj
    · have : (j : ℝ) ≤ -1 := by exact_mod_cast (show j ≤ -1 by omega)
      rw [clamp01_of_one_le (by linarith), clamp01_of_one_le (by linarith)]
    · simp only [Int.cast_zero, sub_zero]
      rw [clamp01_of_mem a1 b1.le, clamp01_of_mem a2 b2.le]
      exact hq
    · have : (1 : ℝ) ≤ j := by exact_mod_cast (show 1 ≤ j by omega)
      rw [clamp01_of_nonpos (by linarith), clamp01_of_nonpos (by linarith)]
  have hgap : δ ≤ |(k₁ : ℝ) * δ - (k₂ : ℝ) * δ| := by
    rw [← sub_mul, abs_mul, abs_of_pos hδ]
    have : (1 : ℝ) ≤ |(k₁ : ℝ) - (k₂ : ℝ)| := by
      have hk : (k₁ : ℕ) ≠ (k₂ : ℕ) := fun h => hne (Fin.ext h)
      rcases Nat.lt_or_gt_of_ne hk with h | h
      · have : (k₁ : ℝ) + 1 ≤ k₂ := by exact_mod_cast h
        rw [abs_of_neg (by linarith)]; linarith
      · have : (k₂ : ℝ) + 1 ≤ k₁ := by exact_mod_cast h
        rw [abs_of_pos (by linarith)]; linarith
    nlinarith
  by_contra hall
  push Not at hall
  have h1 := hall ((k₁ : ℝ) * δ)
  have h2 := hall ((k₂ : ℝ) * δ)
  rw [hsame] at h1
  have := abs_sub_le ((k₁ : ℝ) * δ) (φ (quantized q ((k₂ : ℝ) * δ))) ((k₂ : ℝ) * δ)
  have h3 : |(k₁ : ℝ) * δ - φ (quantized q ((k₂ : ℝ) * δ))| =
      |φ (quantized q ((k₂ : ℝ) * δ)) - (k₁ : ℝ) * δ| := abs_sub_comm _ _
  linarith

/-- **Coverage reads every edge exactly.** The estimator `e ↦ j₀ + Σ coverages` has error `0`
on every edge inside rows `j₀ … j₀+m`. -/
theorem coverage_reading_exact (j₀ : ℤ) (m : ℕ) (e : ℝ) (h0 : (j₀ : ℝ) ≤ e) (h1 : e ≤ j₀ + m) :
    |((j₀ : ℝ) + ∑ r ∈ range m, coverage (subgraph fun _ => e) 0 (j₀ + r)) - e| = 0 := by
  rw [column_sum_reading e 0 j₀ m h0 h1, sub_self, abs_zero]

end Inkvec
