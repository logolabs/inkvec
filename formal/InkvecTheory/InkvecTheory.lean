/-
Copyright (c) 2026 LogoLabs. Released under the Apache 2.0 licence (see LICENSE).
-/
import InkvecTheory.Basic
import InkvecTheory.Coverage
import InkvecTheory.NullSpace
import InkvecTheory.Identifiability
import InkvecTheory.Deconvolution
import InkvecTheory.LevelSetBias
import InkvecTheory.Quantization
import InkvecTheory.ThinStroke
import InkvecTheory.Rank
import InkvecTheory.Noise
import InkvecTheory.Supersampling
import InkvecTheory.Naturality
import InkvecTheory.Gen.Expr
import InkvecTheory.Gen.Rust
import InkvecTheory.Gen.Strip
import InkvecTheory.Windows
import InkvecTheory.Gen.Evidence

/-!
# Inkvec theory: inverting box-filter rasterisation

Machine-checked theorems behind inkvec's coverage-first design. `README.md` maps each
theorem to the code it bears on; `Audit.lean` prints the axioms each depends on.
-/
