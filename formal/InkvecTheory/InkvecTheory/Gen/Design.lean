/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import InkvecTheory.Gen.Rust
import InkvecTheory.Design.Layers

/-!
# The representation chain's kernels (`crates/inkvec-cli/src/layers/check.rs`)

The painter's interval (`InkvecTheory.Design.Layers.painter_interval_iff`) is checked on scan
lines: on each line, every span of the lower region must lie in a span of the completed
shape, and every span of the shape in a span of the upper region, within a slack `s`. The
solver (the layers stage) computes the spans and proposes, for each span, the span that
covers it: the certificate. The checker decides each pair with `spanWithinK`.

* `spanWithinK`: for `x = [a₀, a₁, b₀, b₁, s]`, the outputs `a₀ − b₀ + s` and
  `b₁ − a₁ + s`. `spanWithinK_eq` says what they are; `spanWithin_meaning`: both
  non-negative means `[a₀, a₁] ⊆ [b₀ − s, b₁ + s]`.
* `line_cover`: the fold the checker performs over one line's spans. If every span of a
  list is certified within some span of another, the union of the first lies within the
  union of the second, each widened by `s`.
-/

namespace Inkvec.Gen

open Inkvec.Gen.Expr

/-- `[a₀ − b₀ + s, b₁ − a₁ + s]` over `x = [a₀, a₁, b₀, b₁, s]`. -/
def spanWithinOuts : List Expr :=
  [add (sub (var 0) (var 2)) (var 4), add (sub (var 3) (var 1)) (var 4)]

def spanWithinK : Kernel where
  name := "span_within"
  doc := "For `x = [a₀, a₁, b₀, b₁, s]`: `[a₀ − b₀ + s, b₁ − a₁ + s]`, both non-negative \
    exactly when the span `[a₀, a₁]` lies in `[b₀ − s, b₁ + s]` (`spanWithin_meaning`); \
    the per-span check of the painter's interval (`painter_interval_iff`)."
  arity := 5
  outs := spanWithinOuts

/-- The kernel's outputs. -/
theorem spanWithinK_eq (x : ℕ → ℝ) :
    spanWithinK.outs.map (evalR x) = [x 0 - x 2 + x 4, x 3 - x 1 + x 4] := by
  simp [spanWithinK, spanWithinOuts, evalR]

/-- **What the checker certifies for one span.** -/
theorem spanWithin_meaning (a₀ a₁ b₀ b₁ s : ℝ) (h₀ : 0 ≤ a₀ - b₀ + s) (h₁ : 0 ≤ b₁ - a₁ + s) :
    Set.Icc a₀ a₁ ⊆ Set.Icc (b₀ - s) (b₁ + s) := by
  intro y hy
  exact ⟨by linarith [hy.1], by linarith [hy.2]⟩

/-- **One scan line.** If each span of `A` is certified within some span of `B`, the union of
`A`'s spans lies in the union of `B`'s, each widened by `s`. -/
theorem line_cover (A B : List (ℝ × ℝ)) (s : ℝ)
    (h : ∀ a ∈ A, ∃ b ∈ B, 0 ≤ a.1 - b.1 + s ∧ 0 ≤ b.2 - a.2 + s) :
    (⋃ a ∈ A, Set.Icc a.1 a.2) ⊆ ⋃ b ∈ B, Set.Icc (b.1 - s) (b.2 + s) := by
  intro y hy
  simp only [Set.mem_iUnion] at hy ⊢
  obtain ⟨a, ha, hya⟩ := hy
  obtain ⟨b, hb, h₀, h₁⟩ := h a ha
  exact ⟨b, hb, spanWithin_meaning a.1 a.2 b.1 b.2 s h₀ h₁ hya⟩

/-- The kernels of `generated/design.rs`. -/
def designKernels : List Kernel := [spanWithinK]

end Inkvec.Gen
