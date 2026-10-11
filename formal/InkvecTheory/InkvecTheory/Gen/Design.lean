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
* `spanLinkK`: for `x = [p₁, q₀, s]`, the output `p₁ − q₀ + 2s`: non-negative exactly when
  a span ending at `p₁` and the next starting at `q₀`, each widened by `s`, touch. Two
  regions computed from different polygons can leave a crack of a thousandth of a pixel
  between spans that meet; a chain of linked spans covers what lies between its ends
  (`chain_cover`).
* `line_cover`: the fold the checker performs over one line's spans. If every span of a
  list is certified within a chain of linked spans of another (`span_within` against the
  chain's first start and last end, `span_link` between neighbours), the union of the first
  lies within the union of the second, each widened by `s`.
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

/-- `p₁ − q₀ + 2s` over `x = [p₁, q₀, s]`. -/
def spanLinkOuts : List Expr :=
  [add (sub (var 0) (var 1)) (add (var 2) (var 2))]

def spanLinkK : Kernel where
  name := "span_link"
  doc := "For `x = [p₁, q₀, s]`: `p₁ − q₀ + 2s`, non-negative exactly when a span ending at \
    `p₁` and the next one starting at `q₀`, each widened by `s`, touch (`chain_cover`)."
  arity := 3
  outs := spanLinkOuts

/-- The kernel's output. -/
theorem spanLinkK_eq (x : ℕ → ℝ) :
    spanLinkK.outs.map (evalR x) = [x 0 - x 1 + (x 2 + x 2)] := by
  simp [spanLinkK, spanLinkOuts, evalR]

/-- Consecutive spans of a chain, each widened by `s`, touch. -/
def Linked (s : ℝ) : List (ℝ × ℝ) → Prop
  | b :: c :: cs => 0 ≤ b.2 - c.1 + (s + s) ∧ Linked s (c :: cs)
  | _ => True

/-- The end of a chain's last span. -/
def lastHi : ℝ × ℝ → List (ℝ × ℝ) → ℝ
  | b, [] => b.2
  | _, c :: cs => lastHi c cs

/-- **A chain of linked spans covers what lies between its ends.** -/
theorem chain_cover (s y : ℝ) : ∀ (b : ℝ × ℝ) (bs : List (ℝ × ℝ)),
    Linked s (b :: bs) → b.1 - s ≤ y → y ≤ lastHi b bs + s →
    ∃ c ∈ b :: bs, c.1 - s ≤ y ∧ y ≤ c.2 + s
  | b, [], _, h₀, h₁ => ⟨b, List.mem_singleton_self _, h₀, h₁⟩
  | b, c :: cs, hl, h₀, h₁ => by
      by_cases hy : y ≤ b.2 + s
      · exact ⟨b, List.mem_cons_self .., h₀, hy⟩
      · obtain ⟨hbc, hcs⟩ := hl
        obtain ⟨d, hd, hd'⟩ := chain_cover s y c cs hcs (by linarith) h₁
        exact ⟨d, List.mem_cons_of_mem _ hd, hd'⟩

/-- **One scan line.** If each span of `A` is certified within a chain of linked spans of
`B` (`span_within` against the chain's first start and last end, `span_link` between
neighbours), the union of `A`'s spans lies in the union of `B`'s, each widened by `s`. -/
theorem line_cover (A B : List (ℝ × ℝ)) (s : ℝ)
    (h : ∀ a ∈ A, ∃ b bs, (∀ c ∈ b :: bs, c ∈ B) ∧ Linked s (b :: bs) ∧
      0 ≤ a.1 - b.1 + s ∧ 0 ≤ lastHi b bs - a.2 + s) :
    (⋃ a ∈ A, Set.Icc a.1 a.2) ⊆ ⋃ b ∈ B, Set.Icc (b.1 - s) (b.2 + s) := by
  intro y hy
  simp only [Set.mem_iUnion] at hy ⊢
  obtain ⟨a, ha, hya⟩ := hy
  obtain ⟨b, bs, hmem, hl, h₀, h₁⟩ := h a ha
  obtain ⟨c, hc, hc'⟩ :=
    chain_cover s y b bs hl (by linarith [hya.1]) (by linarith [hya.2])
  exact ⟨c, hmem c hc, hc'⟩

/-- The kernels of `generated/design.rs`. -/
def designKernels : List Kernel := [spanWithinK, spanLinkK]

end Inkvec.Gen
