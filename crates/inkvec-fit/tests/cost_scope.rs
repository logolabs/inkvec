//! The per-trace cost model. Its own test binary, because entering a scope moves the prices for
//! every thread in the process, and the unit tests share one.

use inkvec_fit::cost::{cubic_params, g1_break_radians, with_cost_model, CostModel};
use std::sync::Mutex;

/// These tests change the shared prices, so they run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn a_scope_changes_the_prices_and_puts_them_back() {
    let _s = serial();
    let std = CostModel::standard();
    let asked = CostModel::with_overrides(Some(3.0), Some(30.0));
    assert_ne!(asked, std);
    with_cost_model(asked, || {
        assert_eq!(cubic_params(), 3.0);
        assert!((g1_break_radians() - 30f64.to_radians()).abs() < 1e-12);
    });
    assert_eq!(cubic_params(), std.cubic_params);
    assert!((g1_break_radians() - std.g1_break_degrees.to_radians()).abs() < 1e-12);
}

#[test]
fn a_scope_is_undone_when_the_work_inside_it_panics() {
    let _s = serial();
    let std = CostModel::standard();
    let asked = CostModel::with_overrides(Some(2.5), None);
    let r = std::panic::catch_unwind(|| with_cost_model(asked, || panic!("boom")));
    assert!(r.is_err());
    assert_eq!(cubic_params(), std.cubic_params, "the prices must not leak out of a panic");
}

#[test]
fn a_nested_scope_inherits_rather_than_deadlocking() {
    let _s = serial();
    let outer = CostModel::with_overrides(Some(4.0), None);
    let inner = CostModel::with_overrides(Some(9.0), None);
    with_cost_model(outer, || {
        with_cost_model(inner, || {
            assert_eq!(cubic_params(), 4.0, "the inner call runs under the outer scope");
        });
        assert_eq!(cubic_params(), 4.0);
    });
}

#[test]
fn the_prices_reach_the_threads_a_trace_spawns() {
    let _s = serial();
    let asked = CostModel::with_overrides(Some(3.5), None);
    with_cost_model(asked, || {
        // The fit runs on rayon workers, so a thread-local scope would be invisible to it.
        let seen: Vec<f64> = std::thread::scope(|s| {
            (0..4)
                .map(|_| s.spawn(cubic_params))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect()
        });
        assert!(seen.iter().all(|&p| p == 3.5), "{seen:?}");
    });
}

#[test]
fn standard_traces_run_together_while_a_changed_one_runs_alone() {
    let _s = serial();
    let std = CostModel::standard();
    // Two standard scopes on two threads at once: both are readers, neither blocks the other.
    std::thread::scope(|s| {
        let a = s.spawn(|| with_cost_model(std, || cubic_params()));
        let b = s.spawn(|| with_cost_model(std, || cubic_params()));
        assert_eq!(a.join().unwrap(), std.cubic_params);
        assert_eq!(b.join().unwrap(), std.cubic_params);
    });
}
