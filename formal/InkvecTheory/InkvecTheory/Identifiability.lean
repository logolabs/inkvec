/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import InkvecTheory.Coverage

/-!
# What box-filter coverage does determine

`NullSpace.lean` shows a free-form boundary is not determined by its pixels. This file shows
the other half of the dichotomy: a boundary drawn from a *finite-dimensional model* is
determined, exactly, by as many pixel columns as the model has coefficients.

* `exists_zero_of_integral_eq_zero`, `zeros_of_zero_averages`: a continuous function whose
  mean vanishes on `n` consecutive unit cells has `n` distinct zeros, one inside each cell.
* `poly_eq_of_averages`: two polynomials of degree `< n` with the same means on `n`
  consecutive cells are equal (histopolation is unisolvent).
* `polynomial_boundary_determined`: two polynomial boundaries of degree `< n` that render to
  the same pixels on `n` consecutive columns are the same boundary. Combined with
  `column_sum_eq_average`, the model order a curve can be identified at is bounded by the
  number of pixel columns its span covers: a line needs 2, a cubic graph 4.
* `line_from_two_columns`: the closed form for a straight edge.

This is the theorem behind fitting with a *model* (inkvec's MDL dynamic program over lines,
arcs and cubics) rather than optimising free points: within a model class of dimension at
most the number of columns covered, the data term has a unique zero.
-/

namespace Inkvec

open Set MeasureTheory Finset Polynomial

/-- A continuous function with zero mean on `[a, b]` vanishes somewhere strictly inside. -/
theorem exists_zero_of_integral_eq_zero {f : ℝ → ℝ} (hf : Continuous f) {a b : ℝ} (hab : a < b)
    (h0 : ∫ x in a..b, f x = 0) : ∃ r ∈ Ioo a b, f r = 0 := by
  by_contra hne
  push Not at hne
  set m := (a + b) / 2 with hm_def
  have hm : m ∈ Ioo a b := ⟨by linarith, by linarith⟩
  -- `f` keeps the sign it has at the midpoint throughout `(a, b)`.
  have keep : ∀ g : ℝ → ℝ, Continuous g → (∀ r ∈ Ioo a b, g r ≠ 0) → 0 < g m →
      ∀ x ∈ Ioo a b, 0 < g x := by
    intro g hg hgne hgm x hx
    by_contra hx'
    push Not at hx'
    have hlt : g x < 0 := lt_of_le_of_ne hx' (hgne x hx)
    obtain ⟨r, hr, hr0⟩ := isPreconnected_Ioo.intermediate_value hx hm hg.continuousOn
      (show (0 : ℝ) ∈ Icc (g x) (g m) from ⟨hlt.le, hgm.le⟩)
    exact hgne r hr hr0
  rcases lt_or_gt_of_ne (hne m hm) with hneg | hpos
  · have hpos' := keep (fun x => -f x) hf.neg (fun r hr h => hne r hr (by linarith))
      (by linarith)
    have := intervalIntegral.intervalIntegral_pos_of_pos_on (f := fun x => -f x)
      (hf.neg.intervalIntegrable a b) hpos' hab
    rw [intervalIntegral.integral_neg, h0, neg_zero] at this
    exact lt_irrefl _ this
  · have := intervalIntegral.intervalIntegral_pos_of_pos_on (hf.intervalIntegrable a b)
      (keep f hf hne hpos) hab
    rw [h0] at this
    exact lt_irrefl _ this

/-- **Zero averages force zeros.** If a continuous `f` has zero mean on each of the `n`
consecutive unit cells starting at `a`, it has a zero inside each, and these are `n`
distinct points. -/
theorem zeros_of_zero_averages {f : ℝ → ℝ} (hf : Continuous f) (a : ℝ) (n : ℕ)
    (h0 : ∀ k < n, ∫ x in (a + k)..(a + k + 1), f x = 0) :
    ∃ r : Fin n → ℝ, StrictMono r ∧ ∀ k, f (r k) = 0 := by
  have hex : ∀ k : Fin n, ∃ r ∈ Ioo (a + k) (a + k + 1), f r = 0 := fun k =>
    exists_zero_of_integral_eq_zero hf (by linarith) (h0 k k.isLt)
  choose r hr hr0 using hex
  refine ⟨r, fun k l hkl => ?_, hr0⟩
  have hkl' : (k : ℝ) + 1 ≤ l := by exact_mod_cast (Fin.lt_def.mp hkl)
  linarith [(hr k).2, (hr l).1]

/-- **Histopolation is unisolvent.** A polynomial of degree `< n` whose means vanish on `n`
consecutive unit cells is zero. -/
theorem poly_eq_zero_of_zero_averages (p : ℝ[X]) (a : ℝ) (n : ℕ) (hdeg : p.natDegree < n)
    (h0 : ∀ k < n, ∫ x in (a + k)..(a + k + 1), p.eval x = 0) : p = 0 := by
  obtain ⟨r, hr, hr0⟩ := zeros_of_zero_averages p.continuous a n h0
  exact p.eq_zero_of_natDegree_lt_card_of_eval_eq_zero hr.injective hr0 (by simpa using hdeg)

/-- Two polynomials of degree `< n` with equal means on `n` consecutive cells are equal. -/
theorem poly_eq_of_averages (p q : ℝ[X]) (a : ℝ) (n : ℕ) (hp : p.natDegree < n)
    (hq : q.natDegree < n)
    (h : ∀ k < n, ∫ x in (a + k)..(a + k + 1), p.eval x = ∫ x in (a + k)..(a + k + 1), q.eval x) :
    p = q := by
  have hd : (p - q).natDegree < n := lt_of_le_of_lt (natDegree_sub_le p q) (max_lt hp hq)
  have := poly_eq_zero_of_zero_averages (p - q) a n hd (fun k hk => by
    simp only [eval_sub]
    rw [intervalIntegral.integral_sub (p.continuous.intervalIntegrable _ _)
      (q.continuous.intervalIntegrable _ _), h k hk, sub_self])
  exact sub_eq_zero.mp this

/-- **Polynomial boundaries are determined by their pixels.** Let two polynomial boundaries
of degree `< n` stay within rows `j₀ … j₀+m` over the `n` columns starting at column `a`. If
the two shapes below them have the same box-filter coverage at every one of those
`n × m` pixels, the boundaries are the same polynomial. -/
theorem polynomial_boundary_determined (p q : ℝ[X]) (a j₀ : ℤ) (n m : ℕ) (hp : p.natDegree < n)
    (hq : q.natDegree < n)
    (hpb : ∀ k < n, ∀ x ∈ Icc ((a + k : ℤ) : ℝ) ((a + k : ℤ) + 1),
      (j₀ : ℝ) ≤ p.eval x ∧ p.eval x ≤ j₀ + m)
    (hqb : ∀ k < n, ∀ x ∈ Icc ((a + k : ℤ) : ℝ) ((a + k : ℤ) + 1),
      (j₀ : ℝ) ≤ q.eval x ∧ q.eval x ≤ j₀ + m)
    (hcov : ∀ k < n, ∀ r < m,
      coverage (subgraph fun x => p.eval x) (a + k) (j₀ + r) =
        coverage (subgraph fun x => q.eval x) (a + k) (j₀ + r)) :
    p = q := by
  refine poly_eq_of_averages p q a n hp hq (fun k hk => ?_)
  have ep := column_sum_eq_average (fun x => p.eval x) p.continuous (a + k) j₀ m
    (fun x hx => (hpb k hk x hx).1) (fun x hx => (hpb k hk x hx).2)
  have eq := column_sum_eq_average (fun x => q.eval x) q.continuous (a + k) j₀ m
    (fun x hx => (hqb k hk x hx).1) (fun x hx => (hqb k hk x hx).2)
  have hsum : ∑ r ∈ range m, coverage (subgraph fun x => p.eval x) (a + k) (j₀ + r) =
      ∑ r ∈ range m, coverage (subgraph fun x => q.eval x) (a + k) (j₀ + r) :=
    sum_congr rfl (fun r hr => hcov k hk r (mem_range.mp hr))
  rw [ep, eq] at hsum
  have hcast : ((a + k : ℤ) : ℝ) = (a : ℝ) + k := by push_cast; ring
  rw [hcast] at hsum
  linarith

/-- The column mean of a straight boundary is its height at the column centre. -/
lemma integral_affine_column (s b a : ℝ) :
    ∫ x in a..(a + 1), (s * x + b) = s * (a + 1 / 2) + b := by
  rw [intervalIntegral.integral_add (f := fun x => s * x) (g := fun _ => b)
    ((continuous_const.mul continuous_id).intervalIntegrable _ _) intervalIntegrable_const,
    intervalIntegral.integral_const_mul, integral_id, intervalIntegral.integral_const,
    smul_eq_mul]
  ring

/-- **A straight edge from two column sums, in closed form.** If the boundary `y = s·x + b`
stays within rows `j₀ … j₀+m` over columns `i` and `i+1`, with column sums `S₀`, `S₁`, then
`s = S₁ - S₀` and `b = S₀ + j₀ - s·(i + ½)`. No normal, no threshold, no iteration. -/
theorem line_from_two_columns (s b : ℝ) (i j₀ : ℤ) (m : ℕ)
    (hlo : ∀ x ∈ Icc (i : ℝ) (i + 2), (j₀ : ℝ) ≤ s * x + b)
    (hhi : ∀ x ∈ Icc (i : ℝ) (i + 2), s * x + b ≤ j₀ + m) :
    let S₀ := ∑ r ∈ range m, coverage (subgraph fun x => s * x + b) i (j₀ + r)
    let S₁ := ∑ r ∈ range m, coverage (subgraph fun x => s * x + b) (i + 1) (j₀ + r)
    s = S₁ - S₀ ∧ b = S₀ + j₀ - s * (i + 1 / 2) := by
  intro S₀ S₁
  have hc : Continuous fun x : ℝ => s * x + b := by fun_prop
  have e0 := column_sum_eq_average _ hc i j₀ m
    (fun x hx => hlo x ⟨hx.1, by linarith [hx.2]⟩) (fun x hx => hhi x ⟨hx.1, by linarith [hx.2]⟩)
  have e1 := column_sum_eq_average _ hc (i + 1) j₀ m
    (fun x hx => hlo x ⟨by push_cast at hx; linarith [hx.1], by push_cast at hx; linarith [hx.2]⟩)
    (fun x hx => hhi x ⟨by push_cast at hx; linarith [hx.1], by push_cast at hx; linarith [hx.2]⟩)
  rw [integral_affine_column] at e0
  push_cast at e1
  rw [integral_affine_column] at e1
  have hS0 : S₀ = s * (i + 1 / 2) + b - j₀ := e0
  have hS1 : S₁ = s * ((i : ℝ) + 1 + 1 / 2) + b - j₀ := by
    have : S₁ = ∑ r ∈ range m, coverage (subgraph fun x => s * x + b) (i + 1) (j₀ + r) := rfl
    rw [this, e1]
  constructor
  · rw [hS0, hS1]; ring
  · rw [hS0]; ring

end Inkvec
