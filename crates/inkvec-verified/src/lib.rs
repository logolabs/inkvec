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

#[cfg(test)]
mod tests {
    use crate::generated::design::{span_link_f64, span_within_f64};
    use crate::generated::evidence::{
        corner_excess_f64, fourth_difference_f64, trapezoid_f64, window_term_f64,
    };
    use crate::generated::strip::{histopolate_f64, strip_residual_f64};

    /// Every float kernel once, from its own crate: coverage counts an `#[inline]` function's
    /// runs only where it is instrumented, and its calls from the integration tests and from
    /// the tracer may read as none. The values are exact in `f64`; the kernels' properties are
    /// tested in `tests/`.
    #[test]
    fn every_float_kernel_on_an_exact_case() {
        // [a0, a1] = [1, 2] inside [0, 3] widened by 1/2; spans ending at 2 and starting at
        // 3, each widened by 1/2, just touch.
        assert_eq!(span_within_f64(&[1.0, 2.0, 0.0, 3.0, 0.5]), [1.5, 1.5]);
        assert_eq!(span_link_f64(&[2.0, 3.0, 0.5]), [0.0]);
        // From (0, 1/2) to (1, 3/4) above 0: 5/8. S − A = 1/2 over V = 1/16: χ² 4.
        assert_eq!(trapezoid_f64(&[0.0, 0.5, 1.0, 0.75, 0.0]), [0.625]);
        assert_eq!(window_term_f64(&[1.75, 1.25, 0.0625]), [4.0, 0.5]);
        // h = (0, 0, 0, 1, 2): D = −2; unit variances weigh 1 + 16 + 36 + 16 + 1 = 70.
        let h = [0.0, 0.0, 0.0, 1.0, 2.0];
        assert_eq!(fourth_difference_f64(&h), [-2.0]);
        let mut x = [1.0; 11];
        x[..5].copy_from_slice(&h);
        x[10] = 1.0 / 16.0;
        assert_eq!(corner_excess_f64(&x), [4.0 - 70.0 / 16.0]);
        // Equal means are the constant cubic, which the line t = 1 meets wherever n_v = 1.
        assert_eq!(histopolate_f64(&[1.0; 4]), [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(
            strip_residual_f64(&[1.0, 1.0, 1.0, 1.0, 0.3, 0.0, 1.0, 1.0]),
            [0.0]
        );
    }
}
