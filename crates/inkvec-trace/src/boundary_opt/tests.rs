//! Unit tests of the boundary solve's gridline walk (the coverage, its gradient and the
//! solve itself are tested in `band_tests` and `solve_tests`).

use super::*;

/// The gridline walk used to step `m += 1.0` until it passed the far end, which never
/// happens when that end is infinite or so large that adding one changes nothing. Run on
/// a thread with a deadline, so a regression fails instead of hanging the suite.
#[test]
fn crossings_end_on_non_finite_and_huge_coordinates() {
    let (done, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let p = Point::new;
        let mut out = Vec::new();
        // An ordinary segment still crosses every gridline it spans: x = 0.5 .. 3.5.
        crossings(p(0.2, 0.2), p(3.7, 0.2), 0, 1, &mut out);
        let lines: Vec<f64> = out.iter().map(|&(t, _)| 0.2 + 3.5 * t).collect();
        assert_eq!(out.len(), 4, "{lines:?}");
        for (got, want) in lines.iter().zip([0.5, 1.5, 2.5, 3.5]) {
            assert!((got - want).abs() < 1e-9, "{lines:?}");
        }
        for (a, b) in [
            (p(0.0, 0.0), p(f64::INFINITY, 1.0)),
            (p(f64::NEG_INFINITY, 0.0), p(0.0, 0.0)),
            (p(0.0, f64::NAN), p(2.0, f64::INFINITY)),
            (p(1e20, 0.0), p(1e20 + 1e5, 0.0)),
            (p(-1e300, 3.0), p(1e300, 3.0)),
            (p(0.0, 0.0), p(1e12, 1e12)),
        ] {
            crossings(a, b, 0, 1, &mut out);
            assert!(out.iter().all(|&(t, _)| t.is_finite()));
        }
        done.send(()).unwrap();
    });
    wait.recv_timeout(std::time::Duration::from_secs(20))
        .expect("crossings() did not return on a non-finite or huge coordinate");
}
