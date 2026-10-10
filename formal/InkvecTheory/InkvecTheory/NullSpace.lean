/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import InkvecTheory.Coverage

/-!
# What box-filter coverage cannot see

`docs/algorithm/08-boundary-solve.md` ("The sawtooth") records that the boundary solve can
"wander into that null space and return a row of triangular teeth that render almost as
well as a straight edge". This file proves that the null space is real, characterises it,
and shows the teeth are not "almost" invisible but *exactly* invisible:

* `invisible_perturbation`: any perturbation with zero mean over a pixel column, that keeps
  the boundary inside one row of pixels there, leaves every pixel of that column unchanged.
* `rasterization_not_injective`: two different shapes with identical coverage at every
  pixel of the plane (a sine ripple of any frequency `k ≥ 1`).
* `polyline_zigzag_invisible` and `polyline_kernel`: for a polyline with one vertex per
  pixel border (the parametrisation `planar::build` produces and `boundary_opt` moves),
  the alternating zigzag `δ·(-1)^k` is invisible, and it is the *only* invisible
  deformation of the vertex heights: the null space is exactly one-dimensional.
* `coverage_layer_cake`, `column_fiber`: in general a column's coverages determine, and are
  determined by, the integrated distribution function `t ↦ ∫ (f - t)₊` at integer `t`.
  Rearranging the boundary inside a column without changing that function is invisible.
-/

namespace Inkvec

open Set MeasureTheory Finset Real

/-- **Invisible perturbations.** If `f` and `f + h` both stay within row `j` over column `i`
and `h` has zero mean over the column, every pixel of column `i` has the same coverage. -/
theorem invisible_perturbation (f h : ℝ → ℝ) (hf : Continuous f) (hh : Continuous h) (i j : ℤ)
    (hband : ∀ x ∈ Icc (i : ℝ) (i + 1),
      (j : ℝ) ≤ f x ∧ f x ≤ j + 1 ∧ (j : ℝ) ≤ f x + h x ∧ f x + h x ≤ j + 1)
    (hzero : ∫ x in (i : ℝ)..(i + 1), h x = 0) (j' : ℤ) :
    coverage (subgraph (f + h)) i j' = coverage (subgraph f) i j' := by
  have hle : (i : ℝ) ≤ i + 1 := by linarith
  rw [coverage_subgraph _ (hf.add hh), coverage_subgraph _ hf]
  rcases lt_trichotomy j' j with hlt | heq | hgt
  · -- rows below the boundary's row are full on both sides
    have hj : (j' : ℝ) + 1 ≤ j := by exact_mod_cast hlt
    refine intervalIntegral.integral_congr (fun x hx => ?_)
    rw [uIcc_of_le hle] at hx
    obtain ⟨h1, -, h3, -⟩ := hband x hx
    simp only [Pi.add_apply]
    rw [clamp01_of_one_le (by linarith), clamp01_of_one_le (by linarith)]
  · subst heq
    have e1 : ∫ x in (i : ℝ)..(i + 1), clamp01 ((f + h) x - j') =
        ∫ x in (i : ℝ)..(i + 1), ((f x - j') + h x) := by
      refine intervalIntegral.integral_congr (fun x hx => ?_)
      rw [uIcc_of_le hle] at hx
      obtain ⟨-, -, h3, h4⟩ := hband x hx
      simp only [Pi.add_apply]
      rw [clamp01_of_mem (by linarith) (by linarith)]; ring
    have e2 : ∫ x in (i : ℝ)..(i + 1), clamp01 (f x - j') =
        ∫ x in (i : ℝ)..(i + 1), (f x - j') := by
      refine intervalIntegral.integral_congr (fun x hx => ?_)
      rw [uIcc_of_le hle] at hx
      obtain ⟨h1, h2, -, -⟩ := hband x hx
      rw [clamp01_of_mem (by linarith) (by linarith)]
    rw [e1, e2, intervalIntegral.integral_add (f := fun x => f x - (j' : ℝ)) (g := h)
      ((hf.sub continuous_const).intervalIntegrable _ _) (hh.intervalIntegrable _ _), hzero,
      add_zero]
  · -- rows above are empty on both sides
    have hj : (j : ℝ) + 1 ≤ j' := by exact_mod_cast hgt
    refine intervalIntegral.integral_congr (fun x hx => ?_)
    rw [uIcc_of_le hle] at hx
    obtain ⟨-, h2, -, h4⟩ := hband x hx
    simp only [Pi.add_apply]
    rw [clamp01_of_nonpos (by linarith), clamp01_of_nonpos (by linarith)]

/-- Different boundaries bound different shapes: a statement about sets, not functions. -/
lemma subgraph_ne_of_ne {f g : ℝ → ℝ} (hfg : f ≠ g) : subgraph f ≠ subgraph g := by
  intro hs
  obtain ⟨x, hx⟩ := Function.ne_iff.mp hfg
  rcases lt_or_gt_of_ne hx with h | h
  · have : ((x, f x) : ℝ × ℝ) ∈ subgraph g := h
    rw [← hs] at this; exact absurd (show f x < f x from this) (lt_irrefl _)
  · have : ((x, g x) : ℝ × ℝ) ∈ subgraph f := h
    rw [hs] at this; exact absurd (show g x < g x from this) (lt_irrefl _)

/-- A whole-period ripple has zero mean over every pixel column. -/
lemma integral_sin_ripple (k : ℕ) (hk : 0 < k) (i : ℤ) :
    ∫ x in (i : ℝ)..(i + 1), sin (2 * π * k * x) = 0 := by
  have hc : (2 * π * k : ℝ) ≠ 0 := by positivity
  rw [intervalIntegral.integral_comp_mul_left (fun x => sin x) hc, integral_sin]
  have : 2 * π * (k : ℝ) * ((i : ℝ) + 1) = 2 * π * k * i + (k : ℕ) * (2 * π) := by ring
  rw [this, cos_add_nat_mul_two_pi]; simp

/-- **The box filter is not injective, at every frequency.** A horizontal edge inside row
`j` and the same edge carrying a ripple `ε sin(2πkx)` of any whole frequency `k ≥ 1` and any
amplitude that keeps it inside the row have identical coverage at every pixel of the plane,
yet bound different shapes. The invisible ripples span an infinite-dimensional space, so no
amount of data-term optimisation can choose among them: only a prior can. -/
theorem sine_ripple_invisible (c ε : ℝ) (j : ℤ) (k : ℕ) (hk : 0 < k)
    (hlo : (j : ℝ) + |ε| ≤ c) (hhi : c + |ε| ≤ j + 1) (i j' : ℤ) :
    coverage (subgraph (fun x => c + ε * sin (2 * π * k * x))) i j' =
      coverage (subgraph (fun _ => c)) i j' := by
  have hrw : (fun x => c + ε * sin (2 * π * k * x)) =
      (fun _ : ℝ => c) + (fun x => ε * sin (2 * π * k * x)) := rfl
  rw [hrw]
  refine invisible_perturbation _ _ continuous_const (by fun_prop) i j (fun x _ => ?_) ?_ j'
  · have hs := abs_le.mp (show |ε * sin (2 * π * k * x)| ≤ |ε| by
      rw [abs_mul]; exact mul_le_of_le_one_right (abs_nonneg _) (abs_sin_le_one _))
    have he := abs_nonneg ε
    refine ⟨by linarith, by linarith, by linarith [hs.1], by linarith [hs.2]⟩
  · rw [intervalIntegral.integral_const_mul, integral_sin_ripple k hk, mul_zero]

theorem rasterization_not_injective :
    ∃ Ω₁ Ω₂ : Set (ℝ × ℝ), Ω₁ ≠ Ω₂ ∧ ∀ i j, coverage Ω₁ i j = coverage Ω₂ i j := by
  refine ⟨subgraph (fun x => 1 / 2 + 1 / 4 * sin (2 * π * (1 : ℕ) * x)),
    subgraph (fun _ => 1 / 2), subgraph_ne_of_ne ?_, fun i j' =>
      sine_ripple_invisible (1 / 2) (1 / 4) 0 1 one_pos (by norm_num [abs_of_pos])
        (by norm_num [abs_of_pos]) i j'⟩
  intro h
  have := congrFun h (1 / 4)
  have hs : sin (2 * π * ((1 : ℕ) : ℝ) * (1 / 4)) = 1 := by
    rw [show 2 * π * ((1 : ℕ) : ℝ) * (1 / 4) = π / 2 by push_cast; ring, sin_pi_div_two]
  rw [hs] at this; norm_num at this

/-! ### Polylines with a vertex on every pixel border -/

/-- The unit hat function, the piecewise-linear interpolation basis. -/
noncomputable def hat (t : ℝ) : ℝ := max 0 (1 - |t|)

lemma continuous_hat : Continuous hat := by unfold hat; fun_prop

/-- The polyline through `(k, y k)` for `k = 0 … n`. -/
noncomputable def polyline (y : ℕ → ℝ) (n : ℕ) (x : ℝ) : ℝ :=
  ∑ k ∈ range (n + 1), y k * hat (x - k)

lemma continuous_polyline (y : ℕ → ℝ) (n : ℕ) : Continuous (polyline y n) := by
  unfold polyline
  exact continuous_finsetSum _ (fun k _ =>
    continuous_const.mul (continuous_hat.comp (continuous_id.sub continuous_const)))

lemma polyline_add (y z : ℕ → ℝ) (n : ℕ) :
    polyline (y + z) n = polyline y n + polyline z n := by
  funext x; simp only [polyline, Pi.add_apply, add_mul, sum_add_distrib]

/-- On column `i` the polyline is the chord between its two vertices. -/
lemma polyline_on_column (y : ℕ → ℝ) (n i : ℕ) (hi : i < n) {x : ℝ}
    (hx : x ∈ Icc (i : ℝ) (i + 1)) :
    polyline y n x = y i + (y (i + 1) - y i) * (x - i) := by
  obtain ⟨hx0, hx1⟩ := hx
  unfold polyline
  have hsub : ({i, i + 1} : Finset ℕ) ⊆ range (n + 1) := by
    intro k hk; simp only [Finset.mem_insert, Finset.mem_singleton] at hk
    rcases hk with rfl | rfl <;> simp <;> omega
  rw [← sum_subset hsub (fun k _ hk => ?_)]
  · rw [sum_pair (by omega)]
    unfold hat
    have e1 : |x - (i : ℕ)| = x - i := abs_of_nonneg (by simp; linarith)
    have e2 : |x - ((i + 1 : ℕ) : ℝ)| = i + 1 - x := by
      rw [abs_of_nonpos (by push_cast; linarith)]; push_cast; ring
    rw [e1, e2, max_eq_right (by linarith), max_eq_right (by linarith)]; ring
  · simp only [Finset.mem_insert, Finset.mem_singleton, not_or] at hk
    unfold hat
    rcases Nat.lt_or_gt_of_ne hk.1 with h | h
    · have hk' : (k : ℝ) + 1 ≤ i := by exact_mod_cast h
      rw [max_eq_left (by rw [abs_of_nonneg (by linarith)]; linarith), mul_zero]
    · have hk' : (i : ℝ) + 2 ≤ k := by
        have : i + 2 ≤ k := by omega
        exact_mod_cast this
      rw [max_eq_left (by rw [abs_of_nonpos (by linarith)]; linarith), mul_zero]

/-- The column mean of a polyline is the mean of its two vertex heights (the trapezoid). -/
lemma integral_polyline_column (y : ℕ → ℝ) (n i : ℕ) (hi : i < n) :
    ∫ x in (i : ℝ)..(i + 1), polyline y n x = (y i + y (i + 1)) / 2 := by
  have hle : (i : ℝ) ≤ i + 1 := by linarith
  rw [intervalIntegral.integral_congr (g := fun x => y i + (y (i + 1) - y i) * (x - i))
    (fun x hx => polyline_on_column y n i hi (by rwa [uIcc_of_le hle] at hx))]
  have hs := intervalIntegral.integral_comp_sub_right
    (fun x => y i + (y (i + 1) - y i) * x) (a := (i : ℝ)) (b := (i : ℝ) + 1) (i : ℝ)
  rw [hs, sub_self, add_sub_cancel_left,
    intervalIntegral.integral_add (f := fun _ => y i) (g := fun x => (y (i + 1) - y i) * x)
      intervalIntegrable_const ((continuous_const.mul continuous_id).intervalIntegrable _ _),
    intervalIntegral.integral_const_mul, integral_id]
  simp; ring

/-- The alternating zigzag of amplitude `δ` on the vertices. -/
noncomputable def zigzag (δ : ℝ) (k : ℕ) : ℝ := δ * (-1) ^ k

/-- **The sawtooth is exactly invisible.** Adding the vertex zigzag `δ·(-1)^k` to a polyline
with a vertex on every pixel border changes no pixel of any column where both polylines stay
inside one row. This is the "row of triangular teeth" of `08-boundary-solve.md`. -/
theorem polyline_zigzag_invisible (y : ℕ → ℝ) (δ : ℝ) (n i : ℕ) (hi : i < n) (j : ℤ)
    (hband : ∀ x ∈ Icc (i : ℝ) (i + 1),
      (j : ℝ) ≤ polyline y n x ∧ polyline y n x ≤ j + 1 ∧
      (j : ℝ) ≤ polyline (y + zigzag δ) n x ∧ polyline (y + zigzag δ) n x ≤ j + 1) (j' : ℤ) :
    coverage (subgraph (polyline (y + zigzag δ) n)) (i : ℤ) j' =
      coverage (subgraph (polyline y n)) (i : ℤ) j' := by
  rw [polyline_add]
  refine invisible_perturbation _ _ (continuous_polyline y n) (continuous_polyline _ n)
    (i : ℤ) j (fun x hx => ?_) ?_ j'
  · have := hband x (by simpa using hx)
    rw [polyline_add] at this; simpa using this
  · have := integral_polyline_column (zigzag δ) n i hi
    simp only [Int.cast_natCast]
    rw [this]; unfold zigzag; rw [pow_succ]; ring

/-- **The zigzag is the whole null space.** If every column mean of a polyline vanishes
(the linearised coverage change of a vertex perturbation inside one row of pixels), its
vertex heights are an alternating zigzag. With one vertex per pixel border and motion only
across the boundary, the data term therefore has exactly one flat direction per boundary
run, and a prior has to fix exactly that one. -/
theorem polyline_kernel (z : ℕ → ℝ) (n : ℕ)
    (hz : ∀ i < n, ∫ x in (i : ℝ)..(i + 1), polyline z n x = 0) :
    ∀ k ≤ n, z k = zigzag (z 0) k := by
  intro k hk
  induction k with
  | zero => simp [zigzag]
  | succ k ih =>
    have h := hz k (by omega)
    rw [integral_polyline_column z n k (by omega)] at h
    rw [show z (k + 1) = -z k by linarith, ih (by omega)]
    unfold zigzag; rw [pow_succ]; ring

/-! ### The layer cake: exactly what a column of coverages determines -/

/-- The integrated distribution function of the boundary heights over column `i`. -/
noncomputable def layer (f : ℝ → ℝ) (i : ℤ) (t : ℝ) : ℝ := ∫ x in (i : ℝ)..(i + 1), ramp (f x - t)

theorem coverage_layer_cake (f : ℝ → ℝ) (hf : Continuous f) (i j : ℤ) :
    coverage (subgraph f) i j = layer f i j - layer f i (j + 1) := by
  unfold layer
  have h1 : IntervalIntegrable (fun x => ramp (f x - j)) volume (i : ℝ) (i + 1) :=
    (continuous_ramp.comp (hf.sub continuous_const)).intervalIntegrable _ _
  have h2 : IntervalIntegrable (fun x => ramp (f x - (j + 1))) volume (i : ℝ) (i + 1) :=
    (continuous_ramp.comp (hf.sub continuous_const)).intervalIntegrable _ _
  rw [coverage_subgraph f hf, ← intervalIntegral.integral_sub h1 h2]
  refine intervalIntegral.integral_congr (fun x _ => ?_)
  rw [clamp01_eq_ramp_sub, show f x - j - 1 = f x - (j + 1) by ring]

/-- Above the boundary the layer function vanishes. -/
lemma layer_eq_sum (f : ℝ → ℝ) (hf : Continuous f) (i j₀ : ℤ) (n k : ℕ) (hkn : k ≤ n)
    (hhi : ∀ x ∈ Icc (i : ℝ) (i + 1), f x ≤ j₀ + n) :
    layer f i (j₀ + k) = ∑ m ∈ Finset.Ico k n, coverage (subgraph f) i (j₀ + m) := by
  have hle : (i : ℝ) ≤ i + 1 := by linarith
  have hc : ∀ m ∈ Finset.Ico k n, IntervalIntegrable (fun x => clamp01 (f x - ((j₀ + m : ℤ) : ℝ)))
      volume (i : ℝ) (i + 1) := fun m _ =>
    (continuous_clamp01.comp (hf.sub continuous_const)).intervalIntegrable _ _
  simp_rw [coverage_subgraph f hf]
  rw [← intervalIntegral.integral_finsetSum hc]
  unfold layer
  refine intervalIntegral.integral_congr (fun x hx => ?_)
  rw [uIcc_of_le hle] at hx
  have h := sum_Ico_clamp01 (t := f x - j₀) hkn (by linarith [hhi x hx])
  simp only [Int.cast_add, Int.cast_natCast]
  rw [show f x - ((j₀ : ℝ) + k) = f x - j₀ - k by ring, ← h]
  refine sum_congr rfl (fun m _ => ?_)
  congr 1; ring

/-- **Fibre of the box filter on one column.** Two boundaries below the top of rows
`j₀ … j₀+n` give the same coverages in those rows exactly when their layer functions agree at
every integer level from `j₀` to `j₀+n`. The pixels see a boundary only through how much of
it lies above each integer height, never through where along the column it does so. -/
theorem column_fiber (f g : ℝ → ℝ) (hf : Continuous f) (hg : Continuous g) (i j₀ : ℤ) (n : ℕ)
    (hfh : ∀ x ∈ Icc (i : ℝ) (i + 1), f x ≤ j₀ + n)
    (hgh : ∀ x ∈ Icc (i : ℝ) (i + 1), g x ≤ j₀ + n) :
    (∀ k < n, coverage (subgraph f) i (j₀ + k) = coverage (subgraph g) i (j₀ + k)) ↔
      (∀ k ≤ n, layer f i (j₀ + k) = layer g i (j₀ + k)) := by
  constructor
  · intro hcov k hk
    rw [layer_eq_sum f hf i j₀ n k hk hfh, layer_eq_sum g hg i j₀ n k hk hgh]
    exact sum_congr rfl (fun m hm => hcov m (Finset.mem_Ico.mp hm).2)
  · intro hlay k hk
    rw [coverage_layer_cake f hf, coverage_layer_cake g hg]
    have h1 := hlay k hk.le
    have h2 := hlay (k + 1) hk
    have e : ((j₀ + (k : ℤ) : ℤ) : ℝ) + 1 = (j₀ : ℝ) + ((k + 1 : ℕ) : ℝ) := by push_cast; ring
    have e' : ((j₀ + (k : ℤ) : ℤ) : ℝ) = (j₀ : ℝ) + (k : ℝ) := by push_cast; ring
    rw [e, e', h1, h2]

end Inkvec
