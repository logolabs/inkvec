//! The per-trace cost model. Its own test binary, because entering a scope moves the prices for
//! every thread in the process, and the unit tests share one.

use inkvec_core::{Point, Polyline};
use inkvec_fit::cost::{
    arc_params, cubic_max_turn_radians, cubic_params, g1_break_radians, with_cost_model, CostModel,
};
use inkvec_fit::curves::{Segment, PARAMS_ARC, PARAMS_ARC_WRITTEN};
use inkvec_fit::FitConfig;
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
    assert_eq!(
        cubic_params(),
        std.cubic_params,
        "the prices must not leak out of a panic"
    );
}

#[test]
fn a_nested_scope_inherits_rather_than_deadlocking() {
    let _s = serial();
    let outer = CostModel::with_overrides(Some(4.0), None);
    let inner = CostModel::with_overrides(Some(9.0), None);
    with_cost_model(outer, || {
        with_cost_model(inner, || {
            assert_eq!(
                cubic_params(),
                4.0,
                "the inner call runs under the outer scope"
            );
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
fn written_arcs_charge_an_arc_seven_and_limit_one_cubic_to_ninety_degrees() {
    let _s = serial();
    let arc = Segment::circular_arc(5.0, false, true, Point::new(1.0, 0.0));
    assert_eq!(arc_params(), PARAMS_ARC);
    assert_eq!(arc.params(), PARAMS_ARC);
    assert!(
        cubic_max_turn_radians().is_infinite(),
        "no limit by default"
    );
    let written = CostModel::standard().with_written_arcs();
    assert_eq!(written.cubic_params, CostModel::standard().cubic_params);
    with_cost_model(written, || {
        assert_eq!(arc_params(), PARAMS_ARC_WRITTEN);
        assert_eq!(arc.params(), 7.0, "what the SVG writes");
        assert!((cubic_max_turn_radians() - 90f64.to_radians()).abs() < 1e-12);
    });
    assert_eq!(arc_params(), PARAMS_ARC);
    assert!(cubic_max_turn_radians().is_infinite());
}

/// The turn of a fitted cubic, degrees: the angle between its end directions.
fn cubic_turn(start: Point, c1: Point, c2: Point, end: Point) -> f64 {
    let (ax, ay) = (c1.x - start.x, c1.y - start.y);
    let (bx, by) = (end.x - c2.x, end.y - c2.y);
    let c = (ax * bx + ay * by) / ((ax * ax + ay * ay).sqrt() * (bx * bx + by * by).sqrt());
    c.clamp(-1.0, 1.0).acos().to_degrees()
}

/// A round cap: a half circle of radius `r` px sampled about a pixel apart, with sigma `cap`
/// on its points, between two straight sides of 20 points at sigma 0.05.
fn round_cap(r: f64, cap: f64) -> Polyline {
    let (mut pts, mut sigma) = (Vec::new(), Vec::new());
    for k in 0..20 {
        pts.push(Point::new(-r, 20.0 - k as f64));
        sigma.push(0.05);
    }
    let m = (std::f64::consts::PI * r).round() as usize;
    for k in 0..=m {
        let a = std::f64::consts::PI * (1.0 - k as f64 / m as f64);
        pts.push(Point::new(r * a.cos(), -r * a.sin()));
        sigma.push(cap);
    }
    for k in 1..=20 {
        pts.push(Point::new(r, k as f64));
        sigma.push(0.05);
    }
    Polyline::new(pts, sigma, false)
}

/// The case the 90-degree limit exists for, at 128 px: the round cap of a lucide stroke
/// (radius 5.3 px) at the plain sigma, and a wider one (8 px) whose points carry an inflated
/// sigma. With the arc at seven and no limit each comes back as one cubic over the whole
/// half circle (the wider one swallowing a straight side too): two arcs cost `8λ`, about
/// 57 nats, more than one cubic, against about 15 nats of misfit. With the limit no cubic of
/// either fit stands in for more than a quarter of the circle (10 degrees allowed for the
/// joins `refine` smooths afterwards).
#[test]
fn a_round_cap_is_not_one_cubic_under_written_arcs() {
    let _s = serial();
    let cfg = FitConfig::from_precision(128.0, 0.1, 2.0);
    let widest = |poly: &Polyline, model: CostModel| -> f64 {
        let path = with_cost_model(model, || {
            inkvec_fit::multimodel::optimal_multimodel(poly, &cfg)
        });
        let (mut at, mut most) = (path.start, 0.0f64);
        for s in &path.segments {
            if let Segment::Cubic(c1, c2, end) = *s {
                most = most.max(cubic_turn(at, c1, c2, end));
            }
            at = s.end();
        }
        most
    };
    let unlimited = CostModel {
        arc_params: PARAMS_ARC_WRITTEN,
        ..CostModel::standard()
    };
    for (r, cap) in [(5.3, 0.05), (8.0, 0.35)] {
        let poly = round_cap(r, cap);
        let written = widest(&poly, CostModel::standard().with_written_arcs());
        assert!(
            written <= 100.0,
            "r {r}: a cubic turns {written:.1} degrees"
        );
        // Its end directions turn 180 degrees on the plain cap; on the wider one, where the
        // cubic also covers a straight side it cannot follow, about 124.
        let one = widest(&poly, unlimited);
        assert!(
            one > 110.0,
            "r {r}: without the limit the widest cubic turns {one:.1}"
        );
    }
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
