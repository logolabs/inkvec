/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import InkvecTheory.Deconvolution
import InkvecTheory.Gen.Rust

/-!
# The strip reading's kernels (`crates/inkvec-trace/src/planar/strip.rs`)

* `histopolateK`: the cubic from the means of four unit cells, in the operation order of the
  code it replaces; `histopolateK_eq` says it is `histopolate`, whose constants are exact on
  every cubic (`histopolate_cubic`, `strip_reading_exact`).
* `stripResidualK`: where the normal line from a vertex meets that cubic, as the residual
  `q(s₀ + t·n_u) − t·n_v` of a shift `t`. `stripResidualK_eq` says it is that residual for the
  cubic `histopolate` returns, so (with `strip_reading_exact`) a shift whose residual
  vanishes puts the vertex on the boundary the column sums determine, and on the boundary
  itself wherever the boundary is a cubic. The engine's Newton iteration is the solver; the
  interval kernel is its checker.
-/

namespace Inkvec.Gen

open Inkvec.Gen.Expr

/-- `x[i]` as an expression. -/
abbrev v (i : ℕ) : Expr := var i
/-- A literal. -/
abbrev c (q : ℚ) : Expr := lit q

/-- The four coefficients, written as `strip.rs` writes them. -/
def histopolateOuts : List Expr :=
  [ div (sub (add (add (neg (v 0)) (mul (c 7) (v 1))) (mul (c 7) (v 2))) (v 3)) (c 12),
    div (sub (add (sub (v 0) (mul (c 15) (v 1))) (mul (c 15) (v 2))) (v 3)) (c 12),
    div (add (sub (sub (v 0) (v 1)) (v 2)) (v 3)) (c 4),
    div (add (sub (add (neg (v 0)) (mul (c 3) (v 1))) (mul (c 3) (v 2))) (v 3)) (c 6) ]

def histopolateK : Kernel where
  name := "histopolate"
  doc := "The cubic `c₀ + c₁s + c₂s² + c₃s³` whose means over the cells `[-2,-1] … [1,2]` are \
    `x[0..4]` (`Inkvec.histopolate`; exact on every cubic: `histopolate_cubic`)."
  arity := 4
  outs := histopolateOuts

/-- The kernel computes `histopolate`. -/
theorem histopolateK_eq (x : ℕ → ℝ) :
    histopolateK.outs.map (evalR x) =
      let h := histopolate (x 0) (x 1) (x 2) (x 3)
      [h.1, h.2.1, h.2.2.1, h.2.2.2] := by
  simp [histopolateK, histopolateOuts, evalR, histopolate]

/-- Horner's form of a cubic with coefficients `cs` at `s`, as `strip.rs`'s `eval`. -/
def horner (c₀ c₁ c₂ c₃ s : Expr) : Expr := add c₀ (mul s (add c₁ (mul s (add c₂ (mul s c₃)))))

/-- Inputs: `x[0..4]` the cell means, `x[4]` = `s₀`, `x[5]` = `n_u`, `x[6]` = `n_v`, `x[7]` = `t`. -/
def stripResidualOut : Expr :=
  match histopolateOuts with
  | [c₀, c₁, c₂, c₃] =>
      sub (horner c₀ c₁ c₂ c₃ (add (v 4) (mul (v 7) (v 5)))) (mul (v 7) (v 6))
  | _ => lit 0

def stripResidualK : Kernel where
  name := "strip_residual"
  doc := "`q(s₀ + t·n_u) − t·n_v` for the cubic `q` of [`histopolate_f64`] and \
    `x = [m₀, m₁, m₂, m₃, s₀, n_u, n_v, t]`: zero where the normal line meets the cubic \
    (`stripResidualK_eq`)."
  arity := 8
  outs := [stripResidualOut]

/-- The residual kernel is the cubic `histopolate` returns, at `s₀ + t·n_u`, minus `t·n_v`. -/
theorem stripResidualK_eq (x : ℕ → ℝ) :
    evalR x stripResidualOut =
      let h := histopolate (x 0) (x 1) (x 2) (x 3)
      cubic h.1 h.2.1 h.2.2.1 h.2.2.2 (x 4 + x 7 * x 5) - x 7 * x 6 := by
  simp only [stripResidualOut, histopolateOuts, horner, evalR, histopolate, cubic]
  ring

/-- **What the checker certifies.** For a cubic boundary `q`, fed its four cell means, the
residual kernel is `q(s₀ + t·n_u) − t·n_v`: zero exactly when the vertex moved by `t` along
the normal lies on `q`. -/
theorem stripResidual_on_cubic (c₀ c₁ c₂ c₃ s₀ nu nv t : ℝ) :
    let x : ℕ → ℝ := fun i => match i with
      | 0 => cellMean' (cubic c₀ c₁ c₂ c₃) (-2)
      | 1 => cellMean' (cubic c₀ c₁ c₂ c₃) (-1)
      | 2 => cellMean' (cubic c₀ c₁ c₂ c₃) 0
      | 3 => cellMean' (cubic c₀ c₁ c₂ c₃) 1
      | 4 => s₀
      | 5 => nu
      | 6 => nv
      | _ => t
    evalR x stripResidualOut = cubic c₀ c₁ c₂ c₃ (s₀ + t * nu) - t * nv := by
  intro x
  rw [stripResidualK_eq]
  simp only [x]
  rw [histopolate_cubic]

/-- The kernels of `generated/strip.rs`. -/
def stripKernels : List Kernel := [histopolateK, stripResidualK]

end Inkvec.Gen
