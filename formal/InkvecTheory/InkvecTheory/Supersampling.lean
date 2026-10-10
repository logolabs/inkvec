/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import InkvecTheory.Basic

/-!
# Renderers that supersample instead of integrating

The other files assume exact area coverage (`Coverage.lean`). Many renderers instead
point-sample an `n × n` grid inside each pixel and write the fraction of samples inside the
shape; inkvec's own regression corpus is rendered that way at `n = 8`
(`bench/ci_gate.py`). What does a column sum read then?

* `subCount_close`: of the `N` sub-rows of a column, the number whose centre lies below a
  height `h` is `n·h` to within one half.
* `ss_column_sum_close`: the column sum of `n × n` supersampled coverage equals the
  `n`-point midpoint rule of the boundary height across the column to within `1/(2n)` px,
  whatever the boundary.

So on an 8x8 corpus the strip reading's error is at most `1/16` px of quantisation above the
midpoint rule's, which is why it is measured at 0.008 px mean error on 8x8 renders against
0.001 px on exact ones (`bench/theory`). On a boundary aligned with the sample grid the
pixels themselves cannot do better: no sample changes until the boundary crosses a sample
row (an informal remark; `quantize_minimax` in `Quantization.lean` is the formal bound for
quantised coverage).
-/

namespace Inkvec

open Finset

/-- How many of the `N` sub-row centres `(u + ½)/n`, `u < N`, lie below height `h`. -/
noncomputable def subCount (n N : ℕ) (h : ℝ) : ℝ :=
  ∑ u ∈ range N, if ((u : ℝ) + 1 / 2) / n < h then 1 else 0

/-- **Counting sub-rows reads a height to within half a sub-row.** -/
theorem subCount_close (n N : ℕ) (hn : 0 < n) (h : ℝ) (h0 : 0 ≤ h) (h1 : h * n ≤ N) :
    |subCount n N h / n - h| ≤ 1 / (2 * n) := by
  have hnr : (0 : ℝ) < n := by exact_mod_cast hn
  set H := h * n with hH
  have hH0 : 0 ≤ H := by positivity
  -- the indicator in terms of `H`
  have hind : ∀ u : ℕ, (((u : ℝ) + 1 / 2) / n < h) ↔ ((u : ℝ) + 1 / 2 < H) := by
    intro u; rw [div_lt_iff₀ hnr]
  -- the exact count `Σ clamp01 (H - u) = H`, and the per-sub-row differences
  have hexact : ∑ u ∈ range N, clamp01 (H - u) = H := sum_clamp01_of_mem hH0 h1
  set d : ℕ → ℝ := fun u => (if ((u : ℝ) + 1 / 2) / n < h then 1 else 0) - clamp01 (H - u)
  have hsum : subCount n N h - H = ∑ u ∈ range N, d u := by
    unfold subCount; simp only [d, sum_sub_distrib, hexact]
  set a := ⌊H⌋₊
  have ha0 : (a : ℝ) ≤ H := Nat.floor_le hH0
  have ha1 : H < a + 1 := Nat.lt_floor_add_one H
  have hzero : ∀ b : ℕ, b ≠ a → d b = 0 := by
    intro b hb
    simp only [d]
    rcases Nat.lt_or_gt_of_ne hb with hlt | hgt
    · have : (b : ℝ) + 1 ≤ a := by exact_mod_cast hlt
      have hc : ((b : ℝ) + 1 / 2) / n < h := (hind b).mpr (by linarith)
      simp only [hc, ↓reduceIte]; rw [clamp01_of_one_le (by linarith)]; ring
    · have : (a : ℝ) + 1 ≤ b := by exact_mod_cast hgt
      have hc : ¬ (((b : ℝ) + 1 / 2) / n < h) := fun h' => by have := (hind b).mp h'; linarith
      simp only [hc, ↓reduceIte]; rw [clamp01_of_nonpos (by linarith)]
      ring
  have hda : |d a| ≤ 1 / 2 := by
    simp only [d]
    rw [clamp01_of_mem (by linarith) (by linarith)]
    by_cases hc : ((a : ℝ) + 1 / 2) / n < h
    · simp only [hc, ↓reduceIte]; have := (hind a).mp hc
      rw [abs_le]; constructor <;> linarith
    · simp only [hc, ↓reduceIte]; have : ¬ ((a : ℝ) + 1 / 2 < H) := fun h' => hc ((hind a).mpr h')
      rw [abs_le]; constructor <;> linarith
  have hsingle : ∑ u ∈ range N, d u = if a ∈ range N then d a else 0 := by
    split_ifs with hmem
    · exact sum_eq_single a (fun b _ hb => hzero b hb) (fun h' => absurd hmem h')
    · exact sum_eq_zero (fun b hb => hzero b (fun h' => hmem (h' ▸ hb)))
  have hbound : |subCount n N h - H| ≤ 1 / 2 := by
    rw [hsum, hsingle]; split_ifs
    · exact hda
    · simp
  have : subCount n N h / n - h = (subCount n N h - H) / n := by
    rw [hH]; field_simp
  rw [this, abs_div, abs_of_pos hnr, div_le_div_iff₀ hnr (by positivity)]
  nlinarith [hbound]

/-- The abscissa of the `s`-th sample column of pixel column `i` under `n × n` sampling. -/
noncomputable def sampleX (n : ℕ) (i : ℤ) (s : ℕ) : ℝ := i + ((s : ℝ) + 1 / 2) / n

/-- `n × n` point-supersampled coverage of the region below the graph of `f` in pixel
`(i, j)`: the fraction of the sample points `(i + (s+½)/n, j + (t+½)/n)` below the graph. -/
noncomputable def ssCoverage (n : ℕ) (f : ℝ → ℝ) (i j : ℤ) : ℝ :=
  (∑ s ∈ range n, ∑ t ∈ range n,
    if (j : ℝ) + ((t : ℝ) + 1 / 2) / n < f (sampleX n i s) then 1 else 0) / (n : ℝ) ^ 2

lemma sum_range_mul_add (G : ℕ → ℝ) (n : ℕ) :
    ∀ m : ℕ, ∑ r ∈ range m, ∑ t ∈ range n, G (r * n + t) = ∑ u ∈ range (m * n), G u
  | 0 => by simp
  | m + 1 => by
    rw [sum_range_succ, sum_range_mul_add G n m, Nat.succ_mul, sum_range_add]

/-- The column sum of supersampled coverage, reorganised as sub-row counts per sample column. -/
lemma ss_column_sum (n m : ℕ) (hn : 0 < n) (f : ℝ → ℝ) (i j₀ : ℤ) :
    ∑ r ∈ range m, ssCoverage n f i (j₀ + r) =
      (∑ s ∈ range n, subCount n (m * n) (f (sampleX n i s) - j₀)) / (n : ℝ) ^ 2 := by
  have hnr : (0 : ℝ) < n := by exact_mod_cast hn
  unfold ssCoverage subCount
  rw [← sum_div, sum_comm]
  congr 1
  refine sum_congr rfl (fun s _ => ?_)
  rw [← sum_range_mul_add
    (fun u => if ((u : ℝ) + 1 / 2) / n < f (sampleX n i s) - j₀ then (1 : ℝ) else 0) n m]
  refine sum_congr rfl (fun r _ => sum_congr rfl (fun t _ => ?_))
  have : ((j₀ + r : ℤ) : ℝ) + ((t : ℝ) + 1 / 2) / n < f (sampleX n i s) ↔
      (((r * n + t : ℕ) : ℝ) + 1 / 2) / n < f (sampleX n i s) - j₀ := by
    push_cast
    rw [show ((r : ℝ) * n + t + 1 / 2) / n = r + (t + 1 / 2) / n by field_simp; ring]
    constructor <;> intro h <;> linarith
  simp only [this]

/-- **The supersampled column sum is the midpoint rule, to within `1/(2n)`.** If at each of
the column's `n` sample abscissae the boundary lies within rows `j₀ … j₀+m`, the column sum of
`n × n` supersampled coverage is within `1/(2n)` px of the mean boundary height at those
abscissae (above `j₀`). -/
theorem ss_column_sum_close (n m : ℕ) (hn : 0 < n) (f : ℝ → ℝ) (i j₀ : ℤ)
    (hlo : ∀ s < n, (j₀ : ℝ) ≤ f (sampleX n i s))
    (hhi : ∀ s < n, f (sampleX n i s) ≤ j₀ + m) :
    |∑ r ∈ range m, ssCoverage n f i (j₀ + r) -
      (∑ s ∈ range n, (f (sampleX n i s) - j₀)) / n| ≤ 1 / (2 * n) := by
  have hnr : (0 : ℝ) < n := by exact_mod_cast hn
  rw [ss_column_sum n m hn f i j₀]
  have key : (∑ s ∈ range n, subCount n (m * n) (f (sampleX n i s) - j₀)) / (n : ℝ) ^ 2 -
      (∑ s ∈ range n, (f (sampleX n i s) - j₀)) / n =
      (∑ s ∈ range n, (subCount n (m * n) (f (sampleX n i s) - j₀) / n -
        (f (sampleX n i s) - j₀))) / n := by
    conv_rhs => rw [sum_sub_distrib, ← sum_div]
    field_simp
  rw [key, abs_div, abs_of_pos hnr, div_le_iff₀ hnr]
  calc |∑ s ∈ range n, (subCount n (m * n) (f (sampleX n i s) - j₀) / n - (f (sampleX n i s) - j₀))|
      ≤ ∑ s ∈ range n, |subCount n (m * n) (f (sampleX n i s) - j₀) / n - (f (sampleX n i s) - j₀)| :=
        abs_sum_le_sum_abs _ _
    _ ≤ ∑ _s ∈ range n, 1 / (2 * (n : ℝ)) := by
        refine sum_le_sum (fun s hs => subCount_close n (m * n) hn _ ?_ ?_)
        · have := hlo s (mem_range.mp hs); linarith
        · have := hhi s (mem_range.mp hs); push_cast
          nlinarith
    _ = 1 / (2 * n) * n := by rw [sum_const, card_range, nsmul_eq_mul]; ring

end Inkvec
