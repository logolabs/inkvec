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

/// Suite B1: Exact half-integer pixel boundary crossings and walkable limit
#[test]
fn test_walkable_limits_and_half_integer_crossings() {
    // walkable limits
    assert!(!walkable(GRID_LIMIT - 100.0, GRID_LIMIT));
    assert!(walkable(GRID_LIMIT - 100.0, GRID_LIMIT - 1.0));
    assert!(!walkable(-GRID_LIMIT, -GRID_LIMIT + 100.0));
    assert!(walkable(-GRID_LIMIT + 1.0, -GRID_LIMIT + 100.0));
    assert!(!walkable(0.0, GRID_MAX_SPAN));
    assert!(walkable(0.0, GRID_MAX_SPAN - 1.0));
    assert!(!walkable(0.0, f64::NAN));
    assert!(!walkable(f64::NAN, 0.0));
    assert!(!walkable(f64::NEG_INFINITY, 0.0));
    assert!(!walkable(0.0, f64::INFINITY));

    // crossings: endpoint falling exactly on a half-integer boundary 5.5
    // Horizontal step: from (1.2, 3.5) to (5.5, 3.5)
    let mut out = Vec::new();
    crossings(Point::new(1.2, 3.5), Point::new(5.5, 3.5), 0, 1, &mut out);
    assert_eq!(out.len(), 4);
    let v_lines: Vec<f64> = out
        .iter()
        .filter_map(|(_, prov)| match prov {
            Prov::CrossV { line, .. } => Some(*line),
            _ => None,
        })
        .collect();
    assert_eq!(v_lines, vec![1.5, 2.5, 3.5, 4.5]);
    // The endpoint 5.5 must NOT be crossed
    assert!(!v_lines.iter().any(|&l| (l - 5.5).abs() < 1e-9));

    // Vertical step: from (2.5, 1.2) to (2.5, 5.5)
    crossings(Point::new(2.5, 1.2), Point::new(2.5, 5.5), 0, 1, &mut out);
    assert_eq!(out.len(), 4);
    let h_lines: Vec<f64> = out
        .iter()
        .filter_map(|(_, prov)| match prov {
            Prov::CrossH { line, .. } => Some(*line),
            _ => None,
        })
        .collect();
    assert_eq!(h_lines, vec![1.5, 2.5, 3.5, 4.5]);
    assert!(!h_lines.iter().any(|&l| (l - 5.5).abs() < 1e-9));
}

/// Suite B2: Collinear touch and overlap in segments_cross
#[test]
fn test_segments_cross_collinear_overlap_and_touches() {
    let p = Point::new;

    // Collinear overlapping segments: must return true
    assert!(segments_cross(
        p(0.0, 0.0),
        p(10.0, 0.0),
        p(5.0, 0.0),
        p(15.0, 0.0)
    ));
    assert!(segments_cross(
        p(0.0, 0.0),
        p(10.0, 0.0),
        p(-5.0, 0.0),
        p(5.0, 0.0)
    ));
    assert!(segments_cross(
        p(0.0, 0.0),
        p(10.0, 0.0),
        p(2.0, 0.0),
        p(8.0, 0.0)
    ));
    // Vertical collinear overlap
    assert!(segments_cross(
        p(3.0, 0.0),
        p(3.0, 10.0),
        p(3.0, 4.0),
        p(3.0, 12.0)
    ));

    // Collinear disjoint segments: must return false
    assert!(!segments_cross(
        p(0.0, 0.0),
        p(10.0, 0.0),
        p(12.0, 0.0),
        p(20.0, 0.0)
    ));
    assert!(!segments_cross(
        p(0.0, 0.0),
        p(10.0, 0.0),
        p(-10.0, 0.0),
        p(-2.0, 0.0)
    ));
    assert!(!segments_cross(
        p(3.0, 0.0),
        p(3.0, 5.0),
        p(3.0, 7.0),
        p(3.0, 12.0)
    ));

    // Collinear endpoint touch: returns true
    assert!(segments_cross(
        p(0.0, 0.0),
        p(10.0, 0.0),
        p(10.0, 0.0),
        p(20.0, 0.0)
    ));

    // Proper crossing
    assert!(segments_cross(
        p(0.0, 5.0),
        p(10.0, 5.0),
        p(5.0, 0.0),
        p(5.0, 10.0)
    ));

    // Parallel disjoint
    assert!(!segments_cross(
        p(0.0, 0.0),
        p(10.0, 0.0),
        p(0.0, 2.0),
        p(10.0, 2.0)
    ));

    // T-junction touch
    assert!(segments_cross(
        p(0.0, 0.0),
        p(10.0, 0.0),
        p(5.0, 0.0),
        p(5.0, 5.0)
    ));
}

/// Suite B3: Analytical chain rule gradients in scatter() vs numerical central finite differences
fn quadratic_test_loss(q: Point) -> (f64, f64, f64) {
    let val = 1.3 * (q.x - 2.5).powi(2) + 2.7 * (q.y + 1.8).powi(2);
    let gx = 2.6 * (q.x - 2.5);
    let gy = 5.4 * (q.y + 1.8);
    (val, gx, gy)
}

/// Suite B3: Analytical chain rule gradients in scatter() for CrossV
#[test]
fn test_scatter_cross_v_analytical_vs_numerical() {
    let eps = 1e-6;
    let v_cases = [
        (2.5, Point::new(1.0, 3.0), Point::new(4.0, 7.0)),
        (0.5, Point::new(0.1, -2.0), Point::new(0.9, 5.0)),
        (10.5, Point::new(12.0, 4.0), Point::new(9.0, -1.0)),
        (5.5, Point::new(5.1, 10.0), Point::new(5.9, -8.0)),
        (-1.5, Point::new(-3.0, 2.0), Point::new(-0.5, 0.5)),
    ];

    for &(line, a, b) in &v_cases {
        let q_at = |pa: Point, pb: Point| -> Point {
            let t = (line - pa.x) / (pb.x - pa.x);
            Point::new(line, pa.y + t * (pb.y - pa.y))
        };

        let q0 = q_at(a, b);
        let (_, gx, gy) = quadratic_test_loss(q0);

        let mut grad = vec![Point::new(0.0, 0.0), Point::new(0.0, 0.0)];
        scatter(
            Prov::CrossV { line, a: 0, b: 1 },
            gx,
            gy,
            &[a, b],
            &mut grad,
        );

        let num_ax = (quadratic_test_loss(q_at(Point::new(a.x + eps, a.y), b)).0
            - quadratic_test_loss(q_at(Point::new(a.x - eps, a.y), b)).0)
            / (2.0 * eps);
        let num_ay = (quadratic_test_loss(q_at(Point::new(a.x, a.y + eps), b)).0
            - quadratic_test_loss(q_at(Point::new(a.x, a.y - eps), b)).0)
            / (2.0 * eps);
        let num_bx = (quadratic_test_loss(q_at(a, Point::new(b.x + eps, b.y))).0
            - quadratic_test_loss(q_at(a, Point::new(b.x - eps, b.y))).0)
            / (2.0 * eps);
        let num_by = (quadratic_test_loss(q_at(a, Point::new(b.x, b.y + eps))).0
            - quadratic_test_loss(q_at(a, Point::new(b.x, b.y - eps))).0)
            / (2.0 * eps);

        assert!(
            (grad[0].x - num_ax).abs() < 1e-5,
            "CrossV a.x: ana {} vs num {}",
            grad[0].x,
            num_ax
        );
        assert!(
            (grad[0].y - num_ay).abs() < 1e-5,
            "CrossV a.y: ana {} vs num {}",
            grad[0].y,
            num_ay
        );
        assert!(
            (grad[1].x - num_bx).abs() < 1e-5,
            "CrossV b.x: ana {} vs num {}",
            grad[1].x,
            num_bx
        );
        assert!(
            (grad[1].y - num_by).abs() < 1e-5,
            "CrossV b.y: ana {} vs num {}",
            grad[1].y,
            num_by
        );
    }
}

/// Suite B4: Analytical chain rule gradients in scatter() for CrossH and Vertex
#[test]
fn test_scatter_cross_h_and_vertex_analytical_vs_numerical() {
    let eps = 1e-6;
    let h_cases = [
        // (line, a, b)
        (3.5, Point::new(2.0, 1.0), Point::new(6.0, 5.0)),
        (0.5, Point::new(-3.0, 0.1), Point::new(4.0, 0.9)),
        (8.5, Point::new(5.0, 10.0), Point::new(1.0, 7.0)),
        (1.5, Point::new(10.0, 1.2), Point::new(-2.0, 1.8)),
        (-2.5, Point::new(0.0, -4.0), Point::new(3.0, -1.0)),
    ];

    for &(line, a, b) in &h_cases {
        let q_at = |pa: Point, pb: Point| -> Point {
            let t = (line - pa.y) / (pb.y - pa.y);
            Point::new(pa.x + t * (pb.x - pa.x), line)
        };

        let q0 = q_at(a, b);
        let (_, gx, gy) = quadratic_test_loss(q0);

        let mut grad = vec![Point::new(0.0, 0.0), Point::new(0.0, 0.0)];
        scatter(
            Prov::CrossH { line, a: 0, b: 1 },
            gx,
            gy,
            &[a, b],
            &mut grad,
        );

        let num_ax = (quadratic_test_loss(q_at(Point::new(a.x + eps, a.y), b)).0
            - quadratic_test_loss(q_at(Point::new(a.x - eps, a.y), b)).0)
            / (2.0 * eps);
        let num_ay = (quadratic_test_loss(q_at(Point::new(a.x, a.y + eps), b)).0
            - quadratic_test_loss(q_at(Point::new(a.x, a.y - eps), b)).0)
            / (2.0 * eps);
        let num_bx = (quadratic_test_loss(q_at(a, Point::new(b.x + eps, b.y))).0
            - quadratic_test_loss(q_at(a, Point::new(b.x - eps, b.y))).0)
            / (2.0 * eps);
        let num_by = (quadratic_test_loss(q_at(a, Point::new(b.x, b.y + eps))).0
            - quadratic_test_loss(q_at(a, Point::new(b.x, b.y - eps))).0)
            / (2.0 * eps);

        assert!(
            (grad[0].x - num_ax).abs() < 1e-5,
            "CrossH a.x: ana {} vs num {}",
            grad[0].x,
            num_ax
        );
        assert!(
            (grad[0].y - num_ay).abs() < 1e-5,
            "CrossH a.y: ana {} vs num {}",
            grad[0].y,
            num_ay
        );
        assert!(
            (grad[1].x - num_bx).abs() < 1e-5,
            "CrossH b.x: ana {} vs num {}",
            grad[1].x,
            num_bx
        );
        assert!(
            (grad[1].y - num_by).abs() < 1e-5,
            "CrossH b.y: ana {} vs num {}",
            grad[1].y,
            num_by
        );
    }
}

/// Suite B5: scatter() on Vertex and near-parallel segments
#[test]
fn test_scatter_vertex_and_parallel() {
    // 1. Prov::Vertex test
    let mut grad = vec![Point::new(0.0, 0.0)];
    scatter(
        Prov::Vertex(0),
        4.2,
        -1.8,
        &[Point::new(0.0, 0.0)],
        &mut grad,
    );
    assert_eq!(grad[0].x, 4.2);
    assert_eq!(grad[0].y, -1.8);

    // 2. Parallel segments (|dx| < 1e-9 for CrossV, |dy| < 1e-9 for CrossH)
    let mut grad = vec![Point::new(0.0, 0.0), Point::new(0.0, 0.0)];
    let p_vert = [Point::new(1.0, 2.0), Point::new(1.0, 5.0)];
    scatter(
        Prov::CrossV {
            line: 1.0,
            a: 0,
            b: 1,
        },
        1.0,
        1.0,
        &p_vert,
        &mut grad,
    );
    assert_eq!(grad[0].x, 0.0);
    assert_eq!(grad[0].y, 0.0);
    assert_eq!(grad[1].x, 0.0);
    assert_eq!(grad[1].y, 0.0);

    let p_horiz = [Point::new(2.0, 3.0), Point::new(5.0, 3.0)];
    scatter(
        Prov::CrossH {
            line: 3.0,
            a: 0,
            b: 1,
        },
        1.0,
        1.0,
        &p_horiz,
        &mut grad,
    );
    assert_eq!(grad[0].x, 0.0);
    assert_eq!(grad[0].y, 0.0);
    assert_eq!(grad[1].x, 0.0);
    assert_eq!(grad[1].y, 0.0);
}
