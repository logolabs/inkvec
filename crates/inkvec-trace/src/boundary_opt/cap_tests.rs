//! The iteration cap of [`optimise_alpha_capped`]: `None` is [`optimise_alpha`] bit for bit,
//! a cap only ever stops the descent earlier, and a cap at or above the iterations the solve
//! takes changes nothing.
//!
//! The fixture is a straight vertical boundary measured off its true position in an image
//! rendered from exact area coverage (as in `solve_tests`), so the solve has work to do and
//! takes several iterations.

use super::*;
use crate::planar::Edge;

/// A `w × h` image with a vertical boundary at `x = x0`: black left of it, white right of
/// it, each pixel its exact white coverage.
fn image(w: usize, h: usize, x0: f64) -> Vec<[f32; 3]> {
    (0..w * h)
        .map(|p| [((p % w) as f64 + 0.5 - x0).clamp(0.0, 1.0) as f32; 3])
        .collect()
}

/// The boundary measured at `x`: one point per row, off the gridlines, between two nodes on
/// the frame.
fn map_at(x: f64, w: usize, h: usize) -> PlanarMap {
    let mut points = vec![Point::new(x, -0.5)];
    points.extend((0..h).map(|y| Point::new(x, y as f64 + 0.13)));
    points.push(Point::new(x, h as f64 - 0.5));
    let n = points.len();
    PlanarMap {
        edges: vec![Edge {
            points,
            sigma: vec![0.5; n],
            left: 0,
            right: 1,
            start_node: 0,
            end_node: 1,
            closed: false,
            lambda_scale: 1.0,
        }],
        width: w,
        height: h,
        n_labels: 2,
    }
}

const FACES: [FillModel; 2] = [FillModel::Flat([1.0; 3]), FillModel::Flat([0.0; 3])];

/// Everything a solve writes, as text, so two runs compare bit for bit.
fn outcome(map: &PlanarMap, rep: &Option<Report>) -> String {
    format!(
        "{:?} {:?}",
        map.edges[0].points,
        rep.as_ref()
            .map(|r| (r.before, r.after, r.iters, r.moved, r.scale))
    )
}

#[test]
fn no_cap_is_the_uncapped_solve_bit_for_bit() {
    let (w, h) = (8, 10);
    for truth in [3.3, 2.72, 3.85] {
        let rgb = image(w, h, truth);
        let mut a = map_at(3.0, w, h);
        let mut b = a.clone();
        let ra = optimise_alpha(&mut a, &rgb, &FACES, None, None);
        let rb = optimise_alpha_capped(&mut b, &rgb, &FACES, None, None, None);
        assert_eq!(outcome(&a, &ra), outcome(&b, &rb));
        assert!(ra.is_some(), "the fixture must give the solve work to do");
    }
}

#[test]
fn a_cap_stops_the_descent_early_and_a_loose_cap_changes_nothing() {
    let (w, h) = (8, 10);
    let rgb = image(w, h, 3.3);
    let mut full = map_at(3.0, w, h);
    let rf = optimise_alpha(&mut full, &rgb, &FACES, None, None).expect("the solve improves");
    assert!(
        rf.iters > 1,
        "the fixture must take more than one iteration"
    );
    // Capped at one iteration: at most one taken, still an improvement, and no lower an
    // energy than the full solve reached.
    let mut one = map_at(3.0, w, h);
    let r1 = optimise_alpha_capped(&mut one, &rgb, &FACES, None, None, Some(1))
        .expect("one iteration already improves");
    assert_eq!(r1.iters, 1);
    assert!(r1.after < r1.before);
    assert!(r1.after >= rf.after);
    // A cap of zero iterations does nothing at all, as when the solve gains nothing.
    let mut zero = map_at(3.0, w, h);
    let start = zero.edges[0].points.clone();
    assert!(optimise_alpha_capped(&mut zero, &rgb, &FACES, None, None, Some(0)).is_none());
    assert_eq!(zero.edges[0].points, start);
    // A cap at the iterations the solve takes, or far above the solver's own ceiling, is the
    // uncapped solve.
    for cap in [rf.iters, rf.iters + 1, usize::MAX] {
        let mut m = map_at(3.0, w, h);
        let r = optimise_alpha_capped(&mut m, &rgb, &FACES, None, None, Some(cap));
        assert_eq!(
            outcome(&m, &r),
            outcome(&full, &Some(rf.clone_report())),
            "cap {cap}"
        );
    }
}

impl Report {
    /// A copy, for comparing reports in these tests (the shipped type is not `Clone`).
    fn clone_report(&self) -> Report {
        Report {
            before: self.before,
            after: self.after,
            iters: self.iters,
            moved: self.moved,
            scale: self.scale,
        }
    }
}
