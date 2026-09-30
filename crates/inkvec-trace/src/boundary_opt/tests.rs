//! Unit tests of the boundary solve's building blocks: the gridline walk, the coverage of
//! a cut pixel, junction wedges, and the analytic gradient against finite differences.

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

/// Coverage of a pixel split by a straight vertical boundary, read off the geometry the
/// optimiser builds, against the area computed by hand.
#[test]
fn coverage_of_a_split_pixel() {
    // Two faces, boundary at x = 0.3 through a 3x1 strip of pixels.
    let mut map = PlanarMap {
        edges: vec![crate::planar::Edge {
            points: vec![Point::new(0.3, -0.5), Point::new(0.3, 2.5)],
            sigma: vec![0.5, 0.5],
            left: 0,
            right: 1,
            start_node: 0,
            end_node: 1,
            closed: false,
            lambda_scale: 1.0,
        }],
        width: 3,
        height: 3,
        n_labels: 2,
    };
    let vars = build_vars(&map);
    let face = vec![
        FillModel::Flat([1.0, 1.0, 1.0]),
        FillModel::Flat([0.0, 0.0, 0.0]),
    ];
    // Pixel (0, y) spans x in -0.5..0.5, so the boundary at x = 0.3 leaves a fifth of
    // it on the +x side, and for a chain walking +y that side is `left` (the planar
    // map's convention). The rendered value is therefore 0.2 of the left face's white.
    let rgb = vec![[0.2, 0.2, 0.2]; 9];
    let mut prob = Problem {
        map: &map,
        vars: &vars,
        rgb: &rgb,
        face: &face,
        w: 3,
        h: 3,
        pieces: Vec::new(),
        head: vec![-1; 9],
        scratch: Scratch::default(),
        alpha: None,
        w_kink: 0.0,
        w_anchor: 0.0,
        junctions: true,
        touched: Vec::new(),
        cells_dbg: false,
        chunks: 1,
    };
    let pos = vars.start.clone();
    prob.bucket(&pos);
    // Pixels (0,0), (0,1) and (0,2) are each cut in two, and each must read exactly
    // the mixture the geometry implies.
    let e = prob.data(&pos, None);
    assert!(e < 1e-9, "expected an exact fit, got {e}");
    assert_eq!(prob.pieces.len(), 3, "one piece per pixel crossed");
    // And the sign matters: a target of 0.8 would be the other side, and wrong.
    let rgb2 = vec![[0.8, 0.8, 0.8]; 9];
    prob.rgb = &rgb2;
    assert!(
        prob.data(&pos, None) > 1.0,
        "left/right must not be interchangeable"
    );
    let _ = &mut map;
}

/// A three-way junction: the wedges must partition the pixel, and the node must feel
/// the image through them.
#[test]
fn junction_wedges_partition_the_pixel() {
    // One node at the centre of pixel (1, 1), three boundaries leaving it. Directions
    // are deliberately off the diagonals so that no chain exits through a corner.
    let edge = |to: Point, left: u16, right: u16, end: u32| crate::planar::Edge {
        points: vec![Point::new(1.0, 1.0), to],
        sigma: vec![0.5, 0.5],
        left,
        right,
        start_node: 0,
        end_node: end,
        closed: false,
        lambda_scale: 1.0,
    };
    let map = PlanarMap {
        edges: vec![
            edge(Point::new(1.0, -0.5), 2, 0, 1),
            edge(Point::new(-0.5, 2.0), 1, 2, 2),
            edge(Point::new(2.5, 2.2), 0, 1, 3),
        ],
        width: 3,
        height: 3,
        n_labels: 3,
    };
    let vars = build_vars(&map);
    // The three edges share their first point, so it is one unknown.
    assert_eq!(vars.var[0][0], vars.var[1][0]);
    assert_eq!(vars.var[0][0], vars.var[2][0]);
    assert!(vars.junction[vars.var[0][0] as usize]);
    let face = vec![
        FillModel::Flat([1.0, 0.0, 0.0]),
        FillModel::Flat([0.0, 1.0, 0.0]),
        FillModel::Flat([0.0, 0.0, 1.0]),
    ];
    let rgb = vec![[0.4, 0.4, 0.2]; 9];
    let mut prob = Problem {
        map: &map,
        vars: &vars,
        rgb: &rgb,
        face: &face,
        w: 3,
        h: 3,
        pieces: Vec::new(),
        head: vec![-1; 9],
        scratch: Scratch::default(),
        alpha: None,
        w_kink: 0.0,
        w_anchor: 0.0,
        junctions: false,
        touched: Vec::new(),
        cells_dbg: false,
        chunks: 1,
    };
    let pos = vars.start.clone();
    prob.bucket(&pos);
    let without = prob.data(&pos, None);
    prob.junctions = true;
    prob.bucket(&pos);
    let with = prob.data(&pos, None);
    // The junction pixel was accepted, so it added a residual of its own. Had the
    // wedges failed to partition the square, the term would have been skipped.
    assert!(
        with > without + 1e-9,
        "junction pixel contributed nothing: {without} vs {with}"
    );

    // And the node moves under the image: analytic gradient against a central
    // difference, at the junction unknown.
    let n = vars.start.len();
    let mut grad = vec![Point::new(0.0, 0.0); n];
    prob.w_kink = 0.3;
    prob.w_anchor = 0.2;
    let pos: Vec<Point> = vars
        .start
        .iter()
        .enumerate()
        .map(|(i, p)| Point::new(p.x + 0.013 * i as f64, p.y - 0.011 * i as f64))
        .collect();
    prob.energy(&pos, Some(&mut grad));
    let d = 1e-6;
    let v = vars.var[0][0] as usize;
    for axis in 0..2 {
        let mut plus = pos.clone();
        let mut minus = pos.clone();
        if axis == 0 {
            plus[v].x += d;
            minus[v].x -= d;
        } else {
            plus[v].y += d;
            minus[v].y -= d;
        }
        let num = (prob.energy(&plus, None) - prob.energy(&minus, None)) / (2.0 * d);
        let ana = if axis == 0 { grad[v].x } else { grad[v].y };
        assert!(
            (num - ana).abs() < 1e-3 * (1.0 + ana.abs()),
            "junction axis {axis}: analytic {ana}, numeric {num}"
        );
    }
}

/// The analytic gradient against a central difference of the energy.
#[test]
fn gradient_matches_finite_differences() {
    let map = PlanarMap {
        edges: vec![crate::planar::Edge {
            // Deliberately off the gridlines: a point sitting exactly on one is a
            // configuration where the objective is not differentiable, because moving
            // it changes which pixel its piece belongs to.
            points: vec![
                Point::new(0.13, -0.42),
                Point::new(0.35, 0.61),
                Point::new(0.19, 1.43),
                Point::new(0.31, 2.38),
            ],
            sigma: vec![0.5; 4],
            left: 0,
            right: 1,
            start_node: 0,
            end_node: 1,
            closed: false,
            lambda_scale: 1.0,
        }],
        width: 3,
        height: 3,
        n_labels: 2,
    };
    let vars = build_vars(&map);
    let face = vec![
        FillModel::Flat([1.0, 0.9, 0.8]),
        FillModel::Flat([0.1, 0.2, 0.3]),
    ];
    let rgb = vec![[0.55, 0.5, 0.45]; 9];
    let mut prob = Problem {
        map: &map,
        vars: &vars,
        rgb: &rgb,
        face: &face,
        w: 3,
        h: 3,
        pieces: Vec::new(),
        head: vec![-1; 9],
        scratch: Scratch::default(),
        alpha: None,
        w_kink: 0.7,
        w_anchor: 0.3,
        junctions: true,
        touched: Vec::new(),
        cells_dbg: false,
        chunks: 1,
    };
    let n = vars.start.len();
    // Away from the start, so the anchor term is not sitting at its minimum.
    let pos: Vec<Point> = vars
        .start
        .iter()
        .enumerate()
        .map(|(i, p)| Point::new(p.x + 0.01 * i as f64, p.y - 0.007 * i as f64))
        .collect();
    for (wk, wa) in [(0.0, 0.0), (0.7, 0.0), (0.0, 0.3), (0.7, 0.3)] {
        prob.w_kink = wk;
        prob.w_anchor = wa;
        let mut grad = vec![Point::new(0.0, 0.0); n];
        prob.energy(&pos, Some(&mut grad));
        let d = 1e-6;
        for v in 0..n {
            for axis in 0..2 {
                let mut plus = pos.clone();
                let mut minus = pos.clone();
                if axis == 0 {
                    plus[v].x += d;
                    minus[v].x -= d;
                } else {
                    plus[v].y += d;
                    minus[v].y -= d;
                }
                let ep = prob.energy(&plus, None);
                let em = prob.energy(&minus, None);
                let num = (ep - em) / (2.0 * d);
                let ana = if axis == 0 { grad[v].x } else { grad[v].y };
                assert!(
                    (num - ana).abs() < 1e-3 * (1.0 + ana.abs()),
                    "kink {wk} anchor {wa} var {v} axis {axis}: analytic {ana}, numeric {num}"
                );
            }
        }
    }
}
