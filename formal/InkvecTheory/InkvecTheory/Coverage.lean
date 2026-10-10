/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import InkvecTheory.Basic

/-!
# Box-filter coverage, from Lebesgue measure

The image formation model inkvec inverts (`crates/inkvec-trace/src/coverage.rs`): a pixel's
value is the *area* of the shape inside the pixel's unit square. Here that area is the
Lebesgue measure of `Ω ∩ pixel i j` in `ℝ²`; nothing about it is assumed.

For a shape whose boundary is the graph of a continuous `f` (the shape lies below it),
`coverage_subgraph` turns the 2-D area into a 1-D integral of the unit clamp, and
`column_sum` stacks a column of pixels:

  Σ_{rows} coverage = ∫_{column} (f x - j₀) dx        (when the boundary stays in the rows)

so the sum of a column's coverages is **exactly** the column average of the boundary
height, whatever the boundary's slope or curvature. The identity is the one volume-of-fluid
interface reconstruction rests on (E. G. Puckett (2010), CAMCoS 5(1), eq. (11)) and the
"partial area effect" edge detector of A. Trujillo-Pino et al. (2013), Image and Vision
Computing 31(1); here it is derived from Lebesgue measure for every continuous graph and any
number of rows, and machine-checked.

Conventions: pixel `(i, j)` is the open square `(i, i+1) × (j, j+1)`; its centre is
`(i + ½, j + ½)`. (inkvec's code puts pixel centres on integers; shift by ½.)
-/

namespace Inkvec

open Set MeasureTheory Finset

/-- The open unit pixel whose lower-left corner is `(i, j)`. -/
def pixel (i j : ℤ) : Set (ℝ × ℝ) := Ioo (i : ℝ) (i + 1) ×ˢ Ioo (j : ℝ) (j + 1)

/-- **Box-filter coverage**: the Lebesgue area of `Ω` inside pixel `(i, j)`. An anti-aliased
renderer that computes exact area coverage (libart, FreeType's smooth rasteriser, font-rs,
inkvec's own `boundary_opt/band.rs`) writes this number, times the ink contrast, into the
pixel. -/
noncomputable def coverage (Ω : Set (ℝ × ℝ)) (i j : ℤ) : ℝ := (volume (Ω ∩ pixel i j)).toReal

/-- The region strictly below the graph of `f`. -/
def subgraph (f : ℝ → ℝ) : Set (ℝ × ℝ) := {p | p.2 < f p.1}

lemma subgraph_inter_pixel (f : ℝ → ℝ) (i j : ℤ) :
    subgraph f ∩ pixel i j =
      regionBetween (fun _ => (j : ℝ)) (fun x => j + clamp01 (f x - j)) (Ioo (i : ℝ) (i + 1)) := by
  ext ⟨x, y⟩
  simp only [subgraph, pixel, regionBetween, Set.mem_inter_iff, Set.mem_ofPred_eq, Set.mem_prod, Set.mem_Ioo,
    clamp01]
  constructor
  · rintro ⟨hy, ⟨hx0, hx1⟩, hy0, hy1⟩
    refine ⟨⟨hx0, hx1⟩, hy0, ?_⟩
    have : y - j < min 1 (max 0 (f x - j)) :=
      lt_min (by linarith) (lt_max_of_lt_right (by linarith))
    linarith
  · rintro ⟨⟨hx0, hx1⟩, hy0, hy⟩
    have h' : y - j < min 1 (max 0 (f x - j)) := by linarith
    rcases lt_min_iff.mp h' with ⟨h1, h2⟩
    rcases lt_max_iff.mp h2 with h3 | h3
    · exact absurd h3 (by linarith)
    · exact ⟨by linarith, ⟨hx0, hx1⟩, hy0, by linarith⟩

/-- **Coverage of a graph boundary.** The area of the subgraph of a continuous `f` inside
pixel `(i, j)` is the integral over the pixel's column of the unit clamp of the boundary's
height above the pixel's bottom. -/
theorem coverage_subgraph (f : ℝ → ℝ) (hf : Continuous f) (i j : ℤ) :
    coverage (subgraph f) i j = ∫ x in (i : ℝ)..(i + 1), clamp01 (f x - j) := by
  have hg : Continuous fun x => clamp01 (f x - j) :=
    continuous_clamp01.comp (hf.sub continuous_const)
  have hle : (i : ℝ) ≤ i + 1 := by linarith
  have hint : IntegrableOn (fun x => (j : ℝ) + clamp01 (f x - j)) (Ioo (i : ℝ) (i + 1)) :=
    ((continuous_const.add hg).integrableOn_Icc).mono_set Ioo_subset_Icc_self
  unfold coverage
  rw [subgraph_inter_pixel, Measure.volume_eq_prod,
    volume_regionBetween_eq_integral (integrableOn_const (by simp) (by simp)) hint measurableSet_Ioo
      (fun x _ => le_add_of_nonneg_right (clamp01_nonneg _))]
  have hfun : ((fun x => (j : ℝ) + clamp01 (f x - j)) - fun _ => (j : ℝ)) =
      fun x => clamp01 (f x - j) := by
    funext x; simp
  rw [hfun, ENNReal.toReal_ofReal (setIntegral_nonneg measurableSet_Ioo
      (fun x _ => clamp01_nonneg _)),
    intervalIntegral.integral_of_le hle, integral_Ioc_eq_integral_Ioo]

/-- Each coverage is a genuine fraction. -/
lemma coverage_subgraph_mem (f : ℝ → ℝ) (hf : Continuous f) (i j : ℤ) :
    0 ≤ coverage (subgraph f) i j ∧ coverage (subgraph f) i j ≤ 1 := by
  have hg : Continuous fun x => clamp01 (f x - j) :=
    continuous_clamp01.comp (hf.sub continuous_const)
  have hle : (i : ℝ) ≤ i + 1 := by linarith
  rw [coverage_subgraph f hf]
  constructor
  · exact intervalIntegral.integral_nonneg hle (fun x _ => clamp01_nonneg _)
  · calc ∫ x in (i : ℝ)..(i + 1), clamp01 (f x - j)
        ≤ ∫ _ in (i : ℝ)..(i + 1), (1 : ℝ) :=
          intervalIntegral.integral_mono_on hle (hg.intervalIntegrable _ _)
            intervalIntegrable_const (fun x _ => clamp01_le_one _)
      _ = 1 := by simp

/-- A column of `n` pixels stacked from row `j₀` holds the integral of the boundary's height
above `j₀`, clamped to the column's height `n`. -/
theorem column_sum (f : ℝ → ℝ) (hf : Continuous f) (i j₀ : ℤ) (n : ℕ) :
    ∑ k ∈ range n, coverage (subgraph f) i (j₀ + k) =
      ∫ x in (i : ℝ)..(i + 1), (ramp (f x - j₀) - ramp (f x - j₀ - n)) := by
  have hc : ∀ k ∈ range n, IntervalIntegrable (fun x => clamp01 (f x - ((j₀ + k : ℤ) : ℝ)))
      volume (i : ℝ) (i + 1) := fun k _ =>
    (continuous_clamp01.comp (hf.sub continuous_const)).intervalIntegrable _ _
  simp_rw [coverage_subgraph f hf]
  rw [← intervalIntegral.integral_finsetSum hc]
  congr 1; funext x
  have := sum_clamp01 (f x - j₀) n
  simp only [Int.cast_add, Int.cast_natCast]
  rw [← this]
  refine sum_congr rfl (fun k _ => ?_)
  congr 1; ring

/-- **Column-sum theorem (exact strip average).** If the boundary stays within rows
`j₀ … j₀ + n` over column `i`, the column's coverages sum to the column average of the
boundary height above `j₀`. No slope, curvature or normal direction enters. -/
theorem column_sum_eq_average (f : ℝ → ℝ) (hf : Continuous f) (i j₀ : ℤ) (n : ℕ)
    (hlo : ∀ x ∈ Icc (i : ℝ) (i + 1), (j₀ : ℝ) ≤ f x)
    (hhi : ∀ x ∈ Icc (i : ℝ) (i + 1), f x ≤ j₀ + n) :
    ∑ k ∈ range n, coverage (subgraph f) i (j₀ + k) = (∫ x in (i : ℝ)..(i + 1), f x) - j₀ := by
  have hle : (i : ℝ) ≤ i + 1 := by linarith
  rw [column_sum f hf]
  have hcongr : ∫ x in (i : ℝ)..(i + 1), (ramp (f x - j₀) - ramp (f x - j₀ - n)) =
      ∫ x in (i : ℝ)..(i + 1), (f x - j₀) := by
    refine intervalIntegral.integral_congr (fun x hx => ?_)
    rw [uIcc_of_le hle] at hx
    have h1 := hlo x hx; have h2 := hhi x hx
    unfold ramp; rw [max_eq_right (by linarith), max_eq_left (by linarith)]; ring
  rw [hcongr, intervalIntegral.integral_sub (hf.intervalIntegrable _ _) intervalIntegrable_const]
  simp

end Inkvec
