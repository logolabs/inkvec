/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import InkvecTheory.LevelSetBias

/-!
# Strokes thinner than a pixel

A stroke is the region between two boundaries `lo ≤ hi`. When it is thinner than a pixel,
its pixels never read full coverage, and `planar::ridge_offset` reads it from its coverage
centroid and integrated width (`bench/cases` `ribbon_w1`, `docs/algorithm/07-subpixel.md`).
This file settles exactly what such a stroke's pixels determine:

* `coverage_band`: a stroke's coverage is the difference of its two boundaries' coverages
  (Lebesgue measure, no assumption).
* `straddle_inversion`: when the stroke crosses a pixel border along a whole column, the two
  pixels it touches give its mean centre line and mean width **exactly**:
  `centre = j + 1 + (aHi - aLo)/2`, `width = aLo + aHi`.
* `hidden_stroke_invisible`: when it lies inside one pixel row, every position inside that
  row gives the same pixels. The position is undetermined within `1 - w` px, and only a prior
  can place it (the minimax choice is the pixel centre).
* `centroid_bias`: in the straddling case the centroid reading `ridge_offset` uses is off by
  `(1 - w)(aHi/w - ½)`, which reaches `(1 - w)/2`: as bad as not knowing the position at all,
  in exactly the case where it is known.
* `width_contrast_ambiguity`: a hidden stroke's pixels fix only the product of its width
  and its ink contrast (`coverage.rs`: "A 0.3px black stroke and a 1px grey stroke produce
  the same pixels").
-/

namespace Inkvec

open Set MeasureTheory Finset

/-- The region between two boundaries. -/
def band (lo hi : ℝ → ℝ) : Set (ℝ × ℝ) := subgraph hi \ subgraph lo

lemma measurableSet_subgraph {f : ℝ → ℝ} (hf : Continuous f) : MeasurableSet (subgraph f) :=
  measurableSet_lt measurable_snd (hf.measurable.comp measurable_fst)

lemma volume_pixel_lt_top (i j : ℤ) : volume (pixel i j) < ⊤ := by
  unfold pixel
  rw [Measure.volume_eq_prod, Measure.prod_prod]
  simp [Real.volume_Ioo]

/-- **A stroke's coverage is a difference of two edges' coverages.** -/
theorem coverage_band (lo hi : ℝ → ℝ) (hlo : Continuous lo) (hle : ∀ x, lo x ≤ hi x)
    (i j : ℤ) : coverage (band lo hi) i j = coverage (subgraph hi) i j - coverage (subgraph lo) i j := by
  unfold coverage band
  have hsub : subgraph lo ∩ pixel i j ⊆ subgraph hi ∩ pixel i j :=
    fun p hp => ⟨lt_of_lt_of_le hp.1 (hle p.1), hp.2⟩
  have hmeas : MeasurableSet (subgraph lo ∩ pixel i j) :=
    (measurableSet_subgraph hlo).inter (measurableSet_Ioo.prod measurableSet_Ioo)
  have hfin : volume (subgraph hi ∩ pixel i j) ≠ ⊤ :=
    ((measure_mono inter_subset_right).trans_lt (volume_pixel_lt_top i j)).ne
  have hset : (subgraph hi \ subgraph lo) ∩ pixel i j =
      (subgraph hi ∩ pixel i j) \ (subgraph lo ∩ pixel i j) := by
    ext p
    constructor
    · rintro ⟨⟨h1, h2⟩, h3⟩; exact ⟨⟨h1, h3⟩, fun h => h2 h.1⟩
    · rintro ⟨⟨h1, h3⟩, h2⟩; exact ⟨⟨h1, fun h => h2 ⟨h, h3⟩⟩, h3⟩
  rw [hset, measure_sdiff hsub hmeas.nullMeasurableSet
    (ne_top_of_le_ne_top hfin (measure_mono hsub)), ENNReal.toReal_sub_of_le (measure_mono hsub) hfin]

/-- **Exact inversion of a straddling stroke.** If over column `i` the stroke's lower edge
stays in row `j` and its upper edge in row `j+1`, the two pixels read
`aLo = j + 1 - mean lo` and `aHi = mean hi - (j + 1)`, so the column mean of the centre line is
`j + 1 + (aHi - aLo)/2` and the column mean of the width is `aLo + aHi`, exactly. -/
theorem straddle_inversion (lo hi : ℝ → ℝ) (hlo : Continuous lo) (hhi : Continuous hi)
    (hle : ∀ x, lo x ≤ hi x) (i j : ℤ)
    (hlo_row : ∀ x ∈ Icc (i : ℝ) (i + 1), (j : ℝ) ≤ lo x ∧ lo x ≤ j + 1)
    (hhi_row : ∀ x ∈ Icc (i : ℝ) (i + 1), (j : ℝ) + 1 ≤ hi x ∧ hi x ≤ j + 2) :
    let aLo := coverage (band lo hi) i j
    let aHi := coverage (band lo hi) i (j + 1)
    let mlo := ∫ x in (i : ℝ)..(i + 1), lo x
    let mhi := ∫ x in (i : ℝ)..(i + 1), hi x
    aLo = j + 1 - mlo ∧ aHi = mhi - (j + 1) ∧
      (mlo + mhi) / 2 = j + 1 + (aHi - aLo) / 2 ∧ mhi - mlo = aLo + aHi := by
  intro aLo aHi mlo mhi
  have hI : (i : ℝ) ≤ i + 1 := by linarith
  have congr_on : ∀ {F G : ℝ → ℝ}, (∀ x ∈ Icc (i : ℝ) (i + 1), F x = G x) →
      ∫ x in (i : ℝ)..(i + 1), F x = ∫ x in (i : ℝ)..(i + 1), G x := fun h =>
    intervalIntegral.integral_congr (fun x hx => h x (by rwa [uIcc_of_le hI] at hx))
  have hm : aLo = j + 1 - mlo := by
    show coverage _ i j = _
    rw [coverage_band lo hi hlo hle, coverage_subgraph hi hhi, coverage_subgraph lo hlo,
      congr_on (G := fun _ => (1 : ℝ)) (fun x hx => clamp01_of_one_le (by linarith [hhi_row x hx])),
      congr_on (G := fun x => lo x - j) (fun x hx => clamp01_of_mem
        (by linarith [hlo_row x hx]) (by linarith [hlo_row x hx])),
      intervalIntegral.integral_sub (hlo.intervalIntegrable _ _) intervalIntegrable_const]
    simp [mlo]; ring
  have hp : aHi = mhi - (j + 1) := by
    show coverage _ i (j + 1) = _
    rw [coverage_band lo hi hlo hle, coverage_subgraph hi hhi, coverage_subgraph lo hlo,
      congr_on (G := fun x => hi x - ((j + 1 : ℤ) : ℝ)) (fun x hx => clamp01_of_mem
        (by push_cast; linarith [hhi_row x hx]) (by push_cast; linarith [hhi_row x hx])),
      congr_on (G := fun _ => (0 : ℝ)) (fun x hx => clamp01_of_nonpos
        (by push_cast; linarith [hlo_row x hx])),
      intervalIntegral.integral_sub (hhi.intervalIntegrable _ _) intervalIntegrable_const]
    simp [mhi]
  refine ⟨hm, hp, ?_, ?_⟩ <;> rw [hm, hp] <;> ring

/-- **A stroke inside one pixel row is invisible to position.** Two strokes of the same
width `w`, both lying inside row `j`, have the same coverage at every pixel. -/
theorem hidden_stroke_invisible (c c' w : ℝ) (j : ℤ) (hw : 0 ≤ w)
    (hc : (j : ℝ) ≤ c - w / 2 ∧ c + w / 2 ≤ j + 1)
    (hc' : (j : ℝ) ≤ c' - w / 2 ∧ c' + w / 2 ≤ j + 1) (i j' : ℤ) :
    coverage (band (fun _ => c - w / 2) (fun _ => c + w / 2)) i j' =
      coverage (band (fun _ => c' - w / 2) (fun _ => c' + w / 2)) i j' := by
  rw [coverage_band _ _ continuous_const (fun _ => by linarith),
    coverage_band _ _ continuous_const (fun _ => by linarith),
    coverage_flat, coverage_flat, coverage_flat, coverage_flat]
  rcases lt_trichotomy j' j with h | rfl | h
  · have : (j' : ℝ) + 1 ≤ j := by exact_mod_cast h
    rw [clamp01_of_one_le (by linarith), clamp01_of_one_le (by linarith),
      clamp01_of_one_le (by linarith), clamp01_of_one_le (by linarith)]
  · rw [clamp01_of_mem (by linarith) (by linarith), clamp01_of_mem (by linarith) (by linarith),
      clamp01_of_mem (by linarith) (by linarith), clamp01_of_mem (by linarith) (by linarith)]
    ring
  · have : (j : ℝ) + 1 ≤ j' := by exact_mod_cast h
    rw [clamp01_of_nonpos (by linarith), clamp01_of_nonpos (by linarith),
      clamp01_of_nonpos (by linarith), clamp01_of_nonpos (by linarith)]

/-- **The centroid's bias on a straddling stroke.** With the two pixels' coverages
`aLo = j + 1 - lo` and `aHi = hi - (j + 1)` of a stroke `[lo, hi]` of width `w = hi - lo > 0`,
the coverage centroid of the two pixel centres is off the stroke's centre by
`(1 - w)·(aHi/w - ½)`. -/
theorem centroid_bias (lo hi : ℝ) (j : ℤ) (hw : 0 < hi - lo) :
    let aLo := (j : ℝ) + 1 - lo
    let aHi := hi - (j + 1)
    let w := hi - lo
    (aLo * (j + 1 / 2) + aHi * (j + 3 / 2)) / (aLo + aHi) - (lo + hi) / 2 =
      (1 - w) * (aHi / w - 1 / 2) := by
  intro aLo aHi w
  have hs : aLo + aHi = w := by simp only [aLo, aHi, w]; ring
  rw [hs]
  have hw' : w ≠ 0 := ne_of_gt hw
  field_simp
  simp only [aLo, aHi, w]
  ring

/-- The centroid's bias reaches `(1 - w)/2` while the stroke still straddles: put a stroke of
width `w < 1` with its upper edge exactly on the border (`aHi = 0`). -/
theorem centroid_bias_worst (w : ℝ) (j : ℤ) (hw0 : 0 < w) :
    let lo := (j : ℝ) + 1 - w
    let hi := (j : ℝ) + 1
    ((j + 1 - lo) * (j + 1 / 2) + (hi - (j + 1)) * (j + 3 / 2)) / ((j + 1 - lo) + (hi - (j + 1)))
      - (lo + hi) / 2 = -(1 - w) / 2 := by
  intro lo hi
  have hw' : w ≠ 0 := ne_of_gt hw0
  have hden : ((j : ℝ) + 1 - lo) + (hi - ((j : ℝ) + 1)) = w := by simp only [lo, hi]; ring
  rw [hden]
  simp only [lo, hi]
  field_simp
  ring

/-- **Width and contrast trade off.** A hidden stroke of width `w` in ink `F` on ground `B`
writes `B + w·(F - B)` into its pixel; so does width `w'` in ink `B + (w/w')·(F - B)`. -/
theorem width_contrast_ambiguity (B F w w' : ℝ) (hw' : w' ≠ 0) :
    B + w * (F - B) = B + w' * ((B + (w / w') * (F - B)) - B) := by
  field_simp
  ring

end Inkvec
