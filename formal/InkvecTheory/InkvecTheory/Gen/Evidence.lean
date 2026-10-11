/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import InkvecTheory.Windows
import InkvecTheory.Deconvolution
import InkvecTheory.Gen.Rust

/-!
# The boundary evidence's kernels (`crates/inkvec-trace/src/evidence.rs`)

A candidate description is scored against window areas (`Windows.lean`). Its checker takes
a certificate from the solver: the candidate cut at the window's strip and ends into straight
pieces, each labelled inside the window, above it or below it. The kernels:

* `trapezoidK`: the area a straight piece inside the window contributes,
  `(ub − ua)((va − vlo) + (vb − vlo))/2`; `trapezoidK_integral` says it is the integral of the
  piece's height above `vlo` (`trapezoid_integral`).
* `windowTermK`: one window's `χ²` term `(S − A)²/V` and its residual.
* `fourthDiffK`: the fourth difference `h₀ − 4h₁ + 6h₂ − 4h₃ + h₄` of five consecutive window
  means. `fourthDiff_cubic`: it vanishes on the cell means of every cubic, so on a smooth run
  it reads only noise and the quartic term. `fourthDiff_side`: it is twelve times the
  disagreement `strip.rs`'s corner test measures (the one-sided cubic at the pixel border
  against the central one).
* `cornerExcessK`: `D² − z²·Σ wᵢ² Vᵢ`; certified non-negative (`nonneg_of_evalI`), it says the
  fourth difference is at least `z` of its standard deviations: a corner proposal.
-/

namespace Inkvec.Gen

open Inkvec.Gen.Expr

/-- `x[i]` as an expression. -/
abbrev ev (i : ℕ) : Expr := var i
/-- A literal. -/
abbrev el (q : ℚ) : Expr := lit q

/-- Inputs `x = [ua, va, ub, vb, vlo]`. -/
def trapezoidOut : Expr :=
  div (mul (sub (ev 2) (ev 0)) (add (sub (ev 1) (ev 4)) (sub (ev 3) (ev 4)))) (el 2)

def trapezoidK : Kernel where
  name := "trapezoid"
  doc := "The area `(ub − ua)((va − vlo) + (vb − vlo))/2` a straight piece from `(ua, va)` to \
    `(ub, vb)` inside a window adds above its end `vlo`, `x = [ua, va, ub, vb, vlo]` \
    (`trapezoidK_integral`)."
  arity := 5
  outs := [trapezoidOut]

theorem trapezoidK_eq (x : ℕ → ℝ) :
    evalR x trapezoidOut = (x 2 - x 0) * ((x 1 - x 4) + (x 3 - x 4)) / 2 := by
  simp [trapezoidOut, evalR]

/-- **What the trapezoid certifies.** It is the integral of the piece's height above `vlo`
over the piece's extent across the strip. -/
theorem trapezoidK_integral (x : ℕ → ℝ) (h : x 0 ≠ x 2) :
    evalR x trapezoidOut =
      ∫ u in (x 0)..(x 2), (x 1 + (u - x 0) / (x 2 - x 0) * (x 3 - x 1) - x 4) := by
  rw [trapezoidK_eq, trapezoid_integral _ _ _ _ _ h]

/-- Inputs `x = [S, A, V]`: the measured window sum, the candidate's area, the variance. -/
def windowTermOuts : List Expr :=
  [ div (mul (sub (ev 0) (ev 1)) (sub (ev 0) (ev 1))) (ev 2), sub (ev 0) (ev 1) ]

def windowTermK : Kernel where
  name := "window_term"
  doc := "One window's `χ²` term `(S − A)²/V` and its residual `S − A`, `x = [S, A, V]`."
  arity := 3
  outs := windowTermOuts

theorem windowTermK_eq (x : ℕ → ℝ) :
    windowTermK.outs.map (evalR x) = [(x 0 - x 1) ^ 2 / x 2, x 0 - x 1] := by
  simp [windowTermK, windowTermOuts, evalR, sq]

/-- `h₀ − 4h₁ + 6h₂ − 4h₃ + h₄` from `x[0..5]`. -/
def fourthDiffOut : Expr :=
  add (sub (add (sub (ev 0) (mul (el 4) (ev 1))) (mul (el 6) (ev 2))) (mul (el 4) (ev 3))) (ev 4)

def fourthDiffK : Kernel where
  name := "fourth_difference"
  doc := "The fourth difference `h₀ − 4h₁ + 6h₂ − 4h₃ + h₄` of five consecutive window means \
    (`fourthDiff_cubic`: zero on every cubic; `fourthDiff_side`: twelve times `strip.rs`'s \
    one-sided disagreement)."
  arity := 5
  outs := [fourthDiffOut]

theorem fourthDiffK_eq (x : ℕ → ℝ) :
    evalR x fourthDiffOut = x 0 - 4 * x 1 + 6 * x 2 - 4 * x 3 + x 4 := by
  simp [fourthDiffOut, evalR]

/-- **The fourth difference vanishes on every cubic.** Fed the means of a cubic over five
consecutive unit cells, it is zero: on a smooth run it reads only noise and the quartic term
(`quartic_defect`). -/
theorem fourthDiff_cubic (c₀ c₁ c₂ c₃ k : ℝ) :
    let f := cubic c₀ c₁ c₂ c₃
    cellMean' f (k - 2) - 4 * cellMean' f (k - 1) + 6 * cellMean' f k - 4 * cellMean' f (k + 1) +
      cellMean' f (k + 2) = 0 := by
  intro f
  simp only [f, cellMean', integral_cubic]
  unfold cubicPrim
  ring

/-- **`strip.rs`'s corner test is the fourth difference.** The cubic through the four left
means, read at the pixel border one cell on, less the face value of the four central means, is
the fourth difference of the five over twelve. -/
theorem fourthDiff_side (m₀ m₁ m₂ m₃ m₄ : ℝ) :
    let l := histopolate m₀ m₁ m₂ m₃
    let c := histopolate m₁ m₂ m₃ m₄
    cubic l.1 l.2.1 l.2.2.1 l.2.2.2 1 - c.1 = (m₀ - 4 * m₁ + 6 * m₂ - 4 * m₃ + m₄) / 12 := by
  simp only [histopolate, cubic]
  ring

/-- `D² − z²·(V₀ + 16V₁ + 36V₂ + 16V₃ + V₄)` from `x = [h₀..h₄, V₀..V₄, z²]`. -/
def cornerExcessOut : Expr :=
  let d := fourthDiffOut
  let varD := add (add (add (add (ev 5) (mul (el 16) (ev 6))) (mul (el 36) (ev 7)))
    (mul (el 16) (ev 8))) (ev 9)
  sub (mul d d) (mul (ev 10) varD)

def cornerExcessK : Kernel where
  name := "corner_excess"
  doc := "`D² − z²·Var D` for the fourth difference `D` of five window means with variances \
    `V₀..V₄`, `x = [h₀..h₄, V₀..V₄, z²]`: non-negative exactly when `|D|` is at least `z` of its \
    standard deviations (a corner proposal)."
  arity := 11
  outs := [cornerExcessOut]

theorem cornerExcessK_eq (x : ℕ → ℝ) :
    evalR x cornerExcessOut =
      (x 0 - 4 * x 1 + 6 * x 2 - 4 * x 3 + x 4) ^ 2 -
        x 10 * (x 5 + 16 * x 6 + 36 * x 7 + 16 * x 8 + x 9) := by
  simp [cornerExcessOut, fourthDiffOut, evalR, sq]

/-- The kernels of `generated/evidence.rs`. -/
def evidenceKernels : List Kernel := [trapezoidK, windowTermK, fourthDiffK, cornerExcessK]

end Inkvec.Gen
