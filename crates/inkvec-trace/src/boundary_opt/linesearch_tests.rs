//! The Moré–Thuente search against one-dimensional functions whose minimisers are known,
//! including the first test function of Moré & Thuente (1994), §5.

use super::*;

/// What a driven search ended with: the outcome, the step it stands on, and the trials.
#[derive(Debug)]
struct Run {
    outcome: Next,
    step: f64,
    evals: usize,
}

/// Drive a search on `phi` (returning `φ(a)` and `φ'(a)`) from `stp`, as the solver does:
/// on `Converged` the last trial, on `Stop` the best sufficient-decrease trial.
fn drive(phi: impl Fn(f64) -> (f64, f64), stp: f64, stpmax: f64) -> Option<Run> {
    let (f0, g0) = phi(0.0);
    let mut ls = MoreThuente::new(f0, g0, stp, stpmax);
    let mut evals = 0;
    loop {
        let a = ls.stp();
        assert!(a > 0.0 && a <= stpmax, "trial {a} outside (0, {stpmax}]");
        let (f, g) = phi(a);
        evals += 1;
        assert!(evals <= MAX_EVALS, "more than MAX_EVALS trials");
        match ls.update(f, g) {
            Next::Eval(_) => continue,
            Next::Converged => {
                return Some(Run {
                    outcome: Next::Converged,
                    step: a,
                    evals,
                })
            }
            Next::Stop => {
                return ls.best().map(|(b, _)| Run {
                    outcome: Next::Stop,
                    step: b,
                    evals,
                })
            }
        }
    }
}

/// Whether `a` satisfies the strong Wolfe conditions for `phi`.
fn strong_wolfe(phi: &impl Fn(f64) -> (f64, f64), a: f64) -> bool {
    let (f0, g0) = phi(0.0);
    let (f, g) = phi(a);
    f <= f0 + FTOL * a * g0 && g.abs() <= GTOL * g0.abs()
}

/// Moré & Thuente's function 1, `φ(a) = −a/(a² + β)` with `β = 2`: minimiser `√2`.
fn mt1(a: f64) -> (f64, f64) {
    let b = 2.0;
    (-a / (a * a + b), (a * a - b) / (a * a + b).powi(2))
}

#[test]
fn a_quadratic_converges_to_a_wolfe_step_from_any_start() {
    let phi = |a: f64| ((a - 2.0).powi(2), 2.0 * (a - 2.0));
    for stp in [1e-3, 0.1, 1.0, 2.0, 3.5, 10.0] {
        let r = drive(phi, stp, 100.0).expect("a step");
        assert_eq!(r.outcome, Next::Converged, "start {stp}: {r:?}");
        assert!(strong_wolfe(&phi, r.step), "start {stp}: {r:?}");
    }
}

#[test]
fn more_thuente_function_one_reaches_a_wolfe_step() {
    // The paper's starting points 1e-3, 1e-1, 1e1, 1e3. The tolerances differ from the
    // paper's (μ = 1e-3, η = 0.1); the guarantee tested is theirs: a strong Wolfe step
    // within the trial cap.
    for stp in [1e-3, 1e-1, 1e1, 1e3] {
        let r = drive(mt1, stp, 1e4).expect("a step");
        assert_eq!(r.outcome, Next::Converged, "start {stp}: {r:?}");
        assert!(strong_wolfe(&mt1, r.step), "start {stp}: {r:?}");
    }
}

#[test]
fn a_short_first_step_is_extended_not_accepted() {
    // The Armijo search accepted any short step; this one must extrapolate to where the
    // slope has flattened.
    let phi = |a: f64| ((a - 1.0).powi(2), 2.0 * (a - 1.0));
    let r = drive(phi, 0.01, 10.0).expect("a step");
    assert!(r.step > 0.1, "{r:?}");
    assert!(strong_wolfe(&phi, r.step));
}

#[test]
fn a_minimiser_beyond_the_largest_step_stops_at_the_largest_step() {
    // The slope stays steeper than GTOL of the start's all the way to `stpmax`, so no step
    // within it meets the curvature condition: the search ends on the bound.
    let phi = |a: f64| (-a + 0.01 * a * a, -1.0 + 0.02 * a);
    let r = drive(phi, 0.5, 1.0).expect("a step");
    assert_eq!(r.outcome, Next::Stop);
    assert_eq!(r.step, 1.0);
}

#[test]
fn a_kinked_function_ends_on_a_sufficient_decrease_step() {
    // |a − 1| has no step satisfying the strong curvature condition except exactly at the
    // kink; the search must still end, within the cap, on a step of sufficient decrease.
    let phi = |a: f64| {
        (
            (a - 1.0).abs() - 1.0 + 0.0 * a,
            if a < 1.0 { -1.0 } else { 1.0 },
        )
    };
    let r = drive(phi, 0.3, 10.0).expect("a step");
    let (f0, g0) = phi(0.0);
    let (f, _) = phi(r.step);
    assert!(f <= f0 + FTOL * r.step * g0, "{r:?}");
    assert!(r.evals <= MAX_EVALS);
}

#[test]
fn an_ascent_everywhere_has_no_step() {
    // A slope that claims descent but a function that only rises: no sufficient decrease
    // exists, and the search must say so rather than return a step.
    let phi = |a: f64| {
        (
            if a == 0.0 { 0.0 } else { 1.0 + a },
            if a == 0.0 { -1.0 } else { 1.0 },
        )
    };
    assert!(drive(phi, 1.0, 10.0).is_none());
}

#[test]
fn the_step_never_leaves_its_bounds_and_nan_does_not_hang() {
    let phi = |a: f64| {
        if a > 0.5 {
            (f64::NAN, f64::NAN)
        } else {
            ((a - 0.4).powi(2), 2.0 * (a - 0.4))
        }
    };
    let r = drive(phi, 1.0, 2.0).expect("a step");
    assert!(r.step <= 0.5, "{r:?}");
}
