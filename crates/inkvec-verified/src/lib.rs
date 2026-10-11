//! Checkers generated from the Lean definitions Inkvec's theorems are about.
//!
//! The engine's solvers are free: floating point, heuristics, any algorithm. A result a
//! theorem speaks about is used only if a checker in this crate accepts it, on a certificate
//! the solver emits (`docs/theory/verification.md`). The checkers are not written here: they
//! are generated (`src/generated/`) from expressions in `formal/InkvecTheory/InkvecTheory/Gen/`,
//! the same expressions the theorems are stated about, and CI regenerates them and fails on
//! any difference, so they cannot drift from the proofs.
//!
//! Every kernel comes in three forms:
//!
//! * `<name>_f64`: the formula in `f64`, in the expression's own order of operations, for the
//!   solvers;
//! * `<name>_iv`: an enclosure through [`iv`]'s outward-rounded intervals, which contains the
//!   exact real value whenever it returns one (`Inkvec.Gen.Expr.evalI_sound`);
//! * `<name>_q`: the exact rational value through [`q`]'s checked `i128` arithmetic
//!   (`Inkvec.Gen.Expr.evalQ_sound`).
//!
//! The trusted base is small and stated: [`iv`] and [`q`] implement the operations the Lean
//! development models (`Inkvec.Gen.Enclosing`, the field operations on ℚ); IEEE 754 makes
//! `+ − × ÷ √` correctly rounded, so a result widened by one unit in the last place on each
//! side contains the exact one; and the printer in `Gen/Rust.lean` maps each constructor to
//! one runtime call. A refusal (`None`) is always safe: the caller keeps its fallback.

#![forbid(unsafe_code)]

pub mod generated;
pub mod iv;
pub mod q;
