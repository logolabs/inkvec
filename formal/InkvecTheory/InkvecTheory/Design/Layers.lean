/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import Mathlib.Data.Set.Lattice.Disjoint
import Mathlib.Data.Set.Lattice.Indexed
import Mathlib.Order.Interval.Finset.Nat
import Mathlib.Tactic

/-!
# Layers: the painter's interval

`docs/theory/chain-representation.md`, R4.1 and R4.1b; the code is
`crates/inkvec-cli/src/layers.rs`.

Elements `E 0, …, E (m-1)` (regions of the plane) are painted in that order, each over the
ones before it. Element `i` *shows* what it covers and nothing painted later covers:
`shows E m i = E i \ above E m i`. A planar map gives each face a visible region `V i`,
pairwise disjoint. Which shapes `E` paint exactly those regions?

* `painter_interval_iff`: element `i` shows `V i` for every `i` exactly when
  `V i ⊆ E i ⊆ V i ∪ above V m i` for every `i`. The condition on `E i` names no other
  element's shape, only the data `V`: given the paint order, each face is completed on its
  own, and any shape in its interval paints the same picture.
* `above_eq_of_interval`: under those conditions, what the elements above `i` cover is
  what the faces above `i` show.

The compositing identities of R4.1b, for one pixel with exact area coverages:

* `seam_of_exact_outline`: a lower face drawn exactly to its visible region, under an upper
  face of coverage `a`, lets the ground show with weight `a (1 − a)`: the seam.
* `no_seam_of_completion`: completed across the pixel, the lower face shows exactly its
  share; and `junction_residue`: where the pixel also holds ground area `b` (a junction of
  three paints) the residue is `a·b·(c_i − c_bg)`.
-/

namespace Inkvec.Design

open Set

variable {α : Type*}

/-- What the elements painted after `i` (and before `m`) cover. -/
def above (E : ℕ → Set α) (m i : ℕ) : Set α := ⋃ j ∈ Finset.Ioo i m, E j

/-- What element `i` shows when `E 0, …, E (m - 1)` are painted in that order. -/
def shows (E : ℕ → Set α) (m i : ℕ) : Set α := E i \ above E m i

theorem mem_above {E : ℕ → Set α} {m i : ℕ} {x : α} :
    x ∈ above E m i ↔ ∃ j, i < j ∧ j < m ∧ x ∈ E j := by
  simp [above, Finset.mem_Ioo, and_assoc]

/-- Under the interval conditions, the elements above `i` cover what the faces above `i`
show. -/
theorem above_eq_of_interval {V E : ℕ → Set α} {m : ℕ}
    (h : ∀ j < m, V j ⊆ E j ∧ E j ⊆ V j ∪ above V m j) (i : ℕ) :
    above E m i = above V m i := by
  ext x
  simp only [mem_above]
  constructor
  · rintro ⟨j, hij, hjm, hx⟩
    rcases (h j hjm).2 hx with hv | hu
    · exact ⟨j, hij, hjm, hv⟩
    · obtain ⟨k, hjk, hkm, hk⟩ := mem_above.mp hu
      exact ⟨k, hij.trans hjk, hkm, hk⟩
  · rintro ⟨j, hij, hjm, hx⟩
    exact ⟨j, hij, hjm, (h j hjm).1 hx⟩

/-- **The painter's interval.** With pairwise disjoint visible regions `V`, every element
shows exactly its face's region if and only if each lies between that region and that
region together with what the faces above it show. -/
theorem painter_interval_iff {V E : ℕ → Set α} {m : ℕ}
    (hV : ∀ i j, i ≠ j → Disjoint (V i) (V j)) :
    (∀ i < m, shows E m i = V i) ↔ (∀ i < m, V i ⊆ E i ∧ E i ⊆ V i ∪ above V m i) := by
  constructor
  · intro hs i him
    refine ⟨fun x hx => ?_, fun x hx => ?_⟩
    · rw [← hs i him] at hx
      exact hx.1
    · classical
      -- The last element at or above `i` that covers `x` shows it.
      let S := (Finset.Ico i m).filter (fun j => x ∈ E j)
      have hS : S.Nonempty := ⟨i, Finset.mem_filter.mpr ⟨Finset.mem_Ico.mpr ⟨le_rfl, him⟩, hx⟩⟩
      obtain ⟨j, hjS, hjmax⟩ : ∃ j ∈ S, ∀ k ∈ S, k ≤ j :=
        ⟨S.max' hS, S.max'_mem hS, fun k hk => S.le_max' k hk⟩
      obtain ⟨hjr, hxj⟩ := Finset.mem_filter.mp hjS
      obtain ⟨hij, hjm⟩ := Finset.mem_Ico.mp hjr
      have hshow : x ∈ shows E m j := by
        refine ⟨hxj, fun ha => ?_⟩
        obtain ⟨k, hjk, hkm, hk⟩ := mem_above.mp ha
        have hkS : k ∈ S :=
          Finset.mem_filter.mpr ⟨Finset.mem_Ico.mpr ⟨hij.trans hjk.le, hkm⟩, hk⟩
        exact absurd (hjmax k hkS) (not_le.mpr hjk)
      rw [hs j hjm] at hshow
      rcases eq_or_lt_of_le hij with heq | hlt
      · rw [← heq] at hshow
        exact Or.inl hshow
      · exact Or.inr (mem_above.mpr ⟨j, hlt, hjm, hshow⟩)
  · intro h i him
    rw [shows, above_eq_of_interval h i]
    ext x
    constructor
    · rintro ⟨hx, hna⟩
      rcases (h i him).2 hx with hv | hu
      · exact hv
      · exact absurd hu hna
    · intro hx
      refine ⟨(h i him).1 hx, fun ha => ?_⟩
      obtain ⟨j, hij, _, hj⟩ := mem_above.mp ha
      exact Set.disjoint_left.mp (hV i j hij.ne) hx hj

/-- **Each face on its own.** Given the order, whether element `i` is acceptable depends on
`E i` and the data `V` alone: replacing `E i` by any `E'` in the same interval keeps every
element showing its region. -/
theorem replace_in_interval {V E : ℕ → Set α} {m : ℕ}
    (hV : ∀ i j, i ≠ j → Disjoint (V i) (V j))
    (h : ∀ i < m, shows E m i = V i) (k : ℕ) (E' : Set α)
    (hlo : V k ⊆ E') (hhi : E' ⊆ V k ∪ above V m k) :
    ∀ i < m, shows (Function.update E k E') m i = V i := by
  rw [painter_interval_iff hV] at h ⊢
  intro i him
  by_cases hik : i = k
  · subst hik
    simpa using And.intro hlo hhi
  · simpa [Function.update_of_ne hik] using h i him

/-! ### One pixel: the compositing of R4.1b

`a` is the upper face's area coverage of the pixel, `c_j` its colour; `c_i` the lower
face's, `c_bg` the ground's. "Over" compositing paints the lower face with its own coverage
first, then the upper one. -/

/-- The lower face drawn to its visible region exactly (coverage `1 − a`) leaves the ground
showing with weight `a (1 − a)`. -/
theorem seam_of_exact_outline (a ci cj cbg : ℝ) :
    (a * cj + (1 - a) * ((1 - a) * ci + a * cbg)) - (a * cj + (1 - a) * ci) =
      a * (1 - a) * (cbg - ci) := by
  ring

/-- Completed across the pixel (coverage 1) the lower face shows exactly its share. -/
theorem no_seam_of_completion (a ci cj cbg : ℝ) :
    (a * cj + (1 - a) * (1 * ci + 0 * cbg)) - (a * cj + (1 - a) * ci) = 0 := by
  ring

/-- With ground area `b` in the pixel too (a junction of three paints), the completed face
covers `1 − b` and the residue is `a · b · (c_i − c_bg)`. -/
theorem junction_residue (a b ci cj cbg : ℝ) :
    (a * cj + (1 - a) * ((1 - b) * ci + b * cbg)) - (a * cj + (1 - a - b) * ci + b * cbg) =
      a * b * (ci - cbg) := by
  ring

end Inkvec.Design
