/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import Mathlib

/-!
# The unit clamp

A pixel of height one cut by a boundary at height `t` above its bottom is covered by
`clamp01 t = min 1 (max 0 t)` of its height. Everything about box-filter coverage of a
boundary that is a graph reduces to this function, and its one structural fact is that it is
a difference of two ramps (`clamp01_eq_ramp_sub`), so stacked cells telescope
(`sum_clamp01`).
-/

namespace Inkvec

open Finset

/-- The share of a unit cell lying below height `t` measured from the cell's bottom. -/
noncomputable def clamp01 (t : ℝ) : ℝ := min 1 (max 0 t)

/-- The ramp `t ↦ max 0 t`. -/
noncomputable def ramp (t : ℝ) : ℝ := max 0 t

lemma clamp01_nonneg (t : ℝ) : 0 ≤ clamp01 t :=
  le_min zero_le_one (le_max_left _ _)

lemma clamp01_le_one (t : ℝ) : clamp01 t ≤ 1 := min_le_left _ _

lemma clamp01_of_mem {t : ℝ} (h0 : 0 ≤ t) (h1 : t ≤ 1) : clamp01 t = t := by
  unfold clamp01; rw [max_eq_right h0, min_eq_right h1]

lemma clamp01_of_nonpos {t : ℝ} (h : t ≤ 0) : clamp01 t = 0 := by
  unfold clamp01; rw [max_eq_left h, min_eq_right zero_le_one]

lemma clamp01_of_one_le {t : ℝ} (h : 1 ≤ t) : clamp01 t = 1 := by
  unfold clamp01; rw [max_eq_right (le_trans zero_le_one h), min_eq_left h]

lemma continuous_clamp01 : Continuous clamp01 := by
  unfold clamp01; fun_prop

lemma continuous_ramp : Continuous ramp := by
  unfold ramp; fun_prop

/-- **Layer-cake decomposition.** The unit clamp is a difference of two ramps one unit
apart. -/
lemma clamp01_eq_ramp_sub (t : ℝ) : clamp01 t = ramp t - ramp (t - 1) := by
  unfold clamp01 ramp
  rcases le_total t 0 with h | h
  · rw [max_eq_left h, max_eq_left (by linarith), min_eq_right zero_le_one]; ring
  · rcases le_total t 1 with h' | h'
    · rw [max_eq_right h, max_eq_left (by linarith), min_eq_right h']; ring
    · rw [max_eq_right h, max_eq_right (by linarith), min_eq_left h']; ring

/-- **Telescoping.** `n` stacked unit cells below height `t` hold `ramp t - ramp (t - n)`. -/
lemma sum_clamp01 (t : ℝ) (n : ℕ) :
    ∑ k ∈ range n, clamp01 (t - k) = ramp t - ramp (t - n) := by
  induction n with
  | zero => simp
  | succ n ih =>
    rw [sum_range_succ, ih, clamp01_eq_ramp_sub]
    have : t - (n : ℝ) - 1 = t - ((n + 1 : ℕ) : ℝ) := by push_cast; ring
    rw [this]; ring

/-- When the boundary height lies within the `n` stacked cells, the cells hold exactly `t`. -/
lemma sum_clamp01_of_mem {t : ℝ} {n : ℕ} (h0 : 0 ≤ t) (h1 : t ≤ n) :
    ∑ k ∈ range n, clamp01 (t - k) = t := by
  rw [sum_clamp01]; unfold ramp
  rw [max_eq_right h0, max_eq_left (by linarith)]; ring

/-- The cells from index `k` upwards of a stack of `n` hold `ramp (t - k)` when the boundary
lies below the top of the stack. -/
lemma sum_Ico_clamp01 {t : ℝ} {k n : ℕ} (hkn : k ≤ n) (h1 : t ≤ n) :
    ∑ m ∈ Ico k n, clamp01 (t - m) = ramp (t - k) := by
  have hsplit := sum_range_add_sum_Ico (fun m : ℕ => clamp01 (t - m)) hkn
  rw [sum_clamp01, sum_clamp01] at hsplit
  have htop : ramp (t - n) = 0 := by unfold ramp; exact max_eq_left (by linarith)
  rw [htop] at hsplit
  linarith

end Inkvec
