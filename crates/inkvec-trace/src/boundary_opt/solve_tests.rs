//! The boundary solve against boundaries whose true position is known.
//!
//! Every image here is rendered from exact area coverage, so the geometry that produced it
//! is the zero-residual answer: the solve must move a mis-measured edge onto it, and the
//! energy terms must read the values the geometry implies.

use super::*;
use crate::planar::Edge;

fn edge(points: Vec<Point>, left: u16, right: u16, nodes: (u32, u32), closed: bool) -> Edge {
    let n = points.len();
    Edge {
        points,
        sigma: vec![0.5; n],
        left,
        right,
        start_node: nodes.0,
        end_node: nodes.1,
        closed,
        lambda_scale: 1.0,
    }
}

fn problem<'a>(
    map: &'a PlanarMap,
    vars: &'a Vars,
    rgb: &'a [[f32; 3]],
    face: &'a [FillModel],
) -> Problem<'a> {
    let (w, h) = (map.width, map.height);
    Problem {
        map,
        vars,
        rgb,
        face,
        w,
        h,
        pieces: Vec::new(),
        head: vec![-1; w * h],
        scratch: Scratch::default(),
        alpha: None,
        w_kink: 0.0,
        w_anchor: 0.0,
        junctions: false,
        touched: Vec::new(),
        cells_dbg: false,
        chunks: 1,
    }
}

/// A vertical boundary at `x = x0` down a `w x h` image: white (the chain's left, +x
/// side when walking +y) to the right, black to the left. Pixel values are the exact
/// white coverage of each pixel.
fn vertical_edge_image(w: usize, h: usize, x0: f64) -> Vec<[f32; 3]> {
    (0..w * h)
        .map(|p| {
            let x = (p % w) as f64;
            let cov = ((x + 0.5) - x0).clamp(0.0, 1.0) as f32;
            [cov; 3]
        })
        .collect()
}

/// The measured boundary: straight down at `x`, one point per row, off the gridlines.
fn vertical_chain(x: f64, h: usize) -> Vec<Point> {
    let mut pts = vec![Point::new(x, -0.5)];
    pts.extend((0..h).map(|y| Point::new(x, y as f64 + 0.13)));
    pts.push(Point::new(x, h as f64 - 0.5));
    pts
}

const WHITE_BLACK: [FillModel; 2] = [FillModel::Flat([1.0; 3]), FillModel::Flat([0.0; 3])];

#[test]
fn a_mismeasured_edge_moves_onto_the_true_subpixel_position() {
    let (w, h) = (8usize, 10usize);
    for truth in [3.3, 2.72, 3.85] {
        let rgb = vertical_edge_image(w, h, truth);
        let start = 3.0;
        let mut map = PlanarMap {
            edges: vec![edge(vertical_chain(start, h), 0, 1, (0, 1), false)],
            width: w,
            height: h,
            n_labels: 2,
        };
        let n = map.edges[0].points.len();
        let before = map.edges[0].points.clone();
        let rep = optimise(&mut map, &rgb, &WHITE_BLACK, None).expect("the solve improves");
        assert!(rep.after < rep.before, "{} -> {}", rep.before, rep.after);
        assert_eq!(rep.moved, n, "every point moves");
        assert_eq!(rep.scale, 1.0, "a straight edge cannot fold");
        assert!(rep.iters >= 1);
        // The middle of the chain lands within 0.025 px of the truth, from 0.28-0.85 px
        // away, and every interior point at least two thirds of the way. What is left is
        // by design: the kink prior keeps the chain straight and its two ends are nodes,
        // anchored four times harder (they still move a sixth of the way or more, and never
        // past the truth). Points slide along the edge only slightly.
        let xs: Vec<f64> = map.edges[0].points.iter().map(|p| p.x).collect();
        assert_converged(&xs, start, truth);
        for (p, q) in map.edges[0].points.iter().zip(&before) {
            assert!(
                (p.y - q.y).abs() < 0.5,
                "slid along the edge: {q:?} -> {p:?}"
            );
        }
    }
}

/// The shape a straight edge's solve converges to; see the test above.
fn assert_converged(xs: &[f64], start: f64, truth: f64) {
    let n = xs.len();
    let err0 = (truth - start).abs();
    for (i, &x) in xs.iter().enumerate() {
        let err = (x - truth).abs();
        let what = format!("truth {truth}: points at {xs:.4?} (started at {start})");
        if i == 0 || i + 1 == n {
            let frac = (x - start) / (truth - start);
            assert!((0.05..=1.0).contains(&frac), "end {i}: {what}");
        } else if (n / 4..3 * n / 4).contains(&i) {
            let limit = if err0 > 0.5 { err0 * 0.5 } else { 0.025 };
            assert!(err < limit, "middle point {i}: {what}");
        } else {
            assert!(err < err0 * 0.75, "point {i}: {what}");
        }
    }
}

#[test]
fn white_paint_on_the_clear_ground_is_placed_by_alpha_alone() {
    // Both faces are white over white; only their opacities differ (0.9 paint, 0.1 wash),
    // so the colour channels carry no contrast and alpha has to place the edge.
    let (w, h) = (8usize, 10usize);
    let truth = 3.35;
    let rgb = vec![[1.0f32; 3]; w * h];
    let cov = vertical_edge_image(w, h, truth);
    let (al, ar) = (0.9f32, 0.1f32);
    let alpha: Vec<f32> = cov.iter().map(|c| c[0] * al + (1.0 - c[0]) * ar).collect();
    let face = [FillModel::Flat([1.0; 3]), FillModel::Flat([1.0; 3])];
    let opacity = [al, ar];
    let mut map = PlanarMap {
        edges: vec![edge(vertical_chain(3.0, h), 0, 1, (0, 1), false)],
        width: w,
        height: h,
        n_labels: 2,
    };
    // Without alpha there is nothing to see.
    assert!(optimise(&mut map.clone(), &rgb, &face, None).is_none());
    let rep = optimise_alpha(&mut map, &rgb, &face, None, Some((&alpha, &opacity)))
        .expect("alpha drives the solve");
    assert!(rep.after < rep.before);
    let n = map.edges[0].points.len();
    let xs: Vec<f64> = map.edges[0].points.iter().map(|p| p.x).collect();
    assert_eq!(xs.len(), n);
    assert_converged(&xs, 3.0, truth);
}

#[test]
fn an_edge_already_in_place_is_left_alone() {
    let (w, h) = (6usize, 6usize);
    let rgb = vertical_edge_image(w, h, 2.8);
    let mut map = PlanarMap {
        edges: vec![edge(vertical_chain(2.8, h), 0, 1, (0, 1), false)],
        width: w,
        height: h,
        n_labels: 2,
    };
    // Zero residual to start from: nothing to gain.
    assert!(optimise(&mut map, &rgb, &WHITE_BLACK, None).is_none());
    // Degenerate inputs.
    let mut empty = PlanarMap {
        edges: vec![],
        width: w,
        height: h,
        n_labels: 2,
    };
    assert!(optimise(&mut empty, &rgb, &WHITE_BLACK, None).is_none());
    let mut two = PlanarMap {
        edges: vec![edge(
            vec![Point::new(1.0, -0.5), Point::new(1.0, 5.5)],
            0,
            1,
            (0, 1),
            false,
        )],
        width: w,
        height: h,
        n_labels: 2,
    };
    assert!(
        optimise(&mut two, &rgb, &WHITE_BLACK, None).is_none(),
        "fewer than 3 unknowns"
    );
}

#[test]
fn the_alpha_channel_enters_the_data_term_where_colour_is_silent() {
    // Boundary at x = 0.3 through column 0 of a 3x3 image: left (white paint at 0.8) covers
    // 0.2 of each column-0 pixel, right (white at 0.1) the rest.
    let map = PlanarMap {
        edges: vec![edge(
            vec![Point::new(0.3, -0.5), Point::new(0.3, 2.5)],
            0,
            1,
            (0, 1),
            false,
        )],
        width: 3,
        height: 3,
        n_labels: 2,
    };
    let vars = build_vars(&map);
    let face = [FillModel::Flat([1.0; 3]), FillModel::Flat([1.0; 3])];
    let rgb = vec![[1.0f32; 3]; 9];
    let opacity = [0.8f32, 0.1];
    let exact = 0.2 * 0.8 + 0.8 * 0.1;
    let img_a = vec![exact; 9];
    let mut prob = problem(&map, &vars, &rgb, &face);
    prob.alpha = Some((&img_a, &opacity));
    let pos = vars.start.clone();
    prob.bucket(&pos);
    assert!(prob.data(&pos, None) < 1e-12);
    // Off by 0.1 in alpha in each of the three pixels.
    let off = vec![exact + 0.1; 9];
    prob.alpha = Some((&off, &opacity));
    let d = prob.data(&pos, None);
    assert!((d - 3.0 * 0.01).abs() < 1e-9, "{d}");
    // Without the alpha channel the pixels have no contrast at all.
    prob.alpha = None;
    assert_eq!(prob.data(&pos, None), 0.0);
}

#[test]
fn the_alpha_gradient_matches_finite_differences() {
    let map = PlanarMap {
        edges: vec![edge(
            vec![
                Point::new(0.13, -0.42),
                Point::new(0.35, 0.61),
                Point::new(0.19, 1.43),
                Point::new(0.31, 2.38),
            ],
            0,
            1,
            (0, 1),
            false,
        )],
        width: 3,
        height: 3,
        n_labels: 2,
    };
    let vars = build_vars(&map);
    let face = [FillModel::Flat([0.5; 3]), FillModel::Flat([0.5; 3])];
    let rgb = vec![[0.5f32; 3]; 9];
    let opacity = [0.9f32, 0.3];
    let img_a = vec![0.55f32; 9];
    let mut prob = problem(&map, &vars, &rgb, &face);
    prob.alpha = Some((&img_a, &opacity));
    let n = vars.start.len();
    let pos: Vec<Point> = vars
        .start
        .iter()
        .enumerate()
        .map(|(i, p)| Point::new(p.x + 0.01 * i as f64, p.y - 0.007 * i as f64))
        .collect();
    let mut grad = vec![Point::new(0.0, 0.0); n];
    let e = prob.energy(&pos, Some(&mut grad));
    assert!(e > 1e-4, "alpha residual expected, got {e}");
    let d = 1e-6;
    let mut nonzero = 0;
    for v in 0..n {
        for axis in 0..2 {
            let (mut plus, mut minus) = (pos.clone(), pos.clone());
            if axis == 0 {
                plus[v].x += d;
                minus[v].x -= d;
            } else {
                plus[v].y += d;
                minus[v].y -= d;
            }
            let num = (prob.energy(&plus, None) - prob.energy(&minus, None)) / (2.0 * d);
            let ana = if axis == 0 { grad[v].x } else { grad[v].y };
            nonzero += (ana.abs() > 1e-3) as usize;
            assert!(
                (num - ana).abs() < 1e-3 * (1.0 + ana.abs()),
                "var {v} axis {axis}: analytic {ana}, numeric {num}"
            );
        }
    }
    assert!(nonzero >= 2, "alpha must drive the points");
}

/// The energy's analytic gradient against central differences, at every unknown.
fn assert_gradient_matches(prob: &mut Problem<'_>, pos: &[Point]) {
    let n = pos.len();
    let mut grad = vec![Point::new(0.0, 0.0); n];
    prob.energy(pos, Some(&mut grad));
    let d = 1e-6;
    for v in 0..n {
        for axis in 0..2 {
            let (mut plus, mut minus) = (pos.to_vec(), pos.to_vec());
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
                "var {v} axis {axis}: analytic {ana}, numeric {num}"
            );
        }
    }
}

#[test]
fn the_gradient_through_vertical_gridline_crossings_is_exact() {
    // A diagonal chain crosses vertical and horizontal gridlines alike, so both kinds of
    // crossing vertex carry gradient (the steep chains elsewhere cross horizontal ones only).
    let map = PlanarMap {
        edges: vec![edge(
            vec![
                Point::new(-0.41, 0.21),
                Point::new(0.73, 0.84),
                Point::new(1.62, 1.37),
                Point::new(2.41, 2.13),
            ],
            0,
            1,
            (0, 1),
            false,
        )],
        width: 3,
        height: 3,
        n_labels: 2,
    };
    let vars = build_vars(&map);
    let face = [
        FillModel::Flat([0.9, 0.8, 0.1]),
        FillModel::Flat([0.1, 0.3, 0.7]),
    ];
    let rgb: Vec<[f32; 3]> = (0..9).map(|i| [0.2 + 0.07 * i as f32, 0.5, 0.4]).collect();
    let mut prob = problem(&map, &vars, &rgb, &face);
    prob.w_kink = 0.4;
    prob.w_anchor = 0.25;
    let pos: Vec<Point> = vars
        .start
        .iter()
        .enumerate()
        .map(|(i, p)| Point::new(p.x - 0.013 * i as f64, p.y + 0.017 * i as f64))
        .collect();
    prob.bucket(&pos);
    let e = prob.data(&pos, None);
    assert!(e > 1e-3, "{e}");
    assert_gradient_matches(&mut prob, &pos);
}

#[test]
fn a_three_way_junction_pixel_reads_its_wedge_areas() {
    // Node at the centre of pixel (1, 1); chains leave it up, right and down. They cut the
    // pixel into the top-right quarter (face 0, red), the bottom-right quarter (face 1,
    // green) and the left half (face 2, blue), so it renders (0.25, 0.25, 0.5). The up
    // chain has a collinear point inside the pixel, so it is two pieces there.
    let map = PlanarMap {
        edges: vec![
            edge(
                vec![
                    Point::new(1.0, 1.0),
                    Point::new(1.0, 0.8),
                    Point::new(1.0, -0.5),
                ],
                2,
                0,
                (0, 1),
                false,
            ),
            edge(
                vec![Point::new(1.0, 1.0), Point::new(2.5, 1.0)],
                0,
                1,
                (0, 2),
                false,
            ),
            edge(
                vec![Point::new(1.0, 1.0), Point::new(1.0, 2.5)],
                1,
                2,
                (0, 3),
                false,
            ),
        ],
        width: 3,
        height: 3,
        n_labels: 3,
    };
    let vars = build_vars(&map);
    let face = [
        FillModel::Flat([1.0, 0.0, 0.0]),
        FillModel::Flat([0.0, 1.0, 0.0]),
        FillModel::Flat([0.0, 0.0, 1.0]),
    ];
    // Every pixel a chain crosses, rendered exactly: (1,0) is split blue | red, (2,1) red
    // over green, (1,2) blue | green; (1,1) is the junction.
    let mut rgb = vec![[0.0f32; 3]; 9];
    rgb[1] = [0.5, 0.0, 0.5];
    rgb[5] = [0.5, 0.5, 0.0];
    rgb[7] = [0.0, 0.5, 0.5];
    rgb[4] = [0.25, 0.25, 0.5];
    let pos = vars.start.clone();
    let mut prob = problem(&map, &vars, &rgb, &face);
    prob.junctions = true;
    prob.bucket(&pos);
    assert!(
        prob.data(&pos, None) < 1e-12,
        "exact render must fit exactly"
    );
    // A black target at the junction: the residual is the wedge mixture itself,
    // 0.25^2 + 0.25^2 + 0.5^2.
    let mut dark = rgb.clone();
    dark[4] = [0.0; 3];
    prob.rgb = &dark;
    let d = prob.data(&pos, None);
    assert!((d - 0.375).abs() < 1e-9, "{d}");
    // Excluded when junctions are off.
    prob.junctions = false;
    assert!(prob.data(&pos, None) < 1e-12);
    // With the node and the chains moved off their exact places, the junction's wedges
    // still carry an exact gradient to every unknown.
    prob.junctions = true;
    prob.w_kink = 0.2;
    prob.w_anchor = 0.1;
    let moved: Vec<Point> = pos
        .iter()
        .enumerate()
        .map(|(i, p)| {
            Point::new(
                p.x + 0.031 * (i % 3) as f64 - 0.02,
                p.y - 0.023 * (i % 2) as f64 + 0.01,
            )
        })
        .collect();
    assert_gradient_matches(&mut prob, &moved);
}

#[test]
fn the_kink_prior_sums_absolute_second_differences_around_a_ring() {
    // A closed 2x2 square: at every corner a - 2b + c has length 2*sqrt(2).
    let map = PlanarMap {
        edges: vec![edge(
            vec![
                Point::new(1.0, 1.0),
                Point::new(3.0, 1.0),
                Point::new(3.0, 3.0),
                Point::new(1.0, 3.0),
            ],
            0,
            1,
            (0, 0),
            true,
        )],
        width: 5,
        height: 5,
        n_labels: 2,
    };
    let vars = build_vars(&map);
    assert!(
        vars.junction.iter().all(|&j| !j),
        "a closed ring has no junction"
    );
    let rgb = vec![[0.0f32; 3]; 25];
    let mut prob = problem(&map, &vars, &rgb, &WHITE_BLACK);
    prob.w_kink = 0.5;
    let pos = vars.start.clone();
    let want = 4.0 * 0.5 * (8.0f64 + 1e-4).sqrt();
    assert!((prob.priors(&pos, None) - want).abs() < 1e-12);
    // The anchor: every point 0.1 right of where it was measured.
    prob.w_kink = 0.0;
    prob.w_anchor = 2.0;
    let moved: Vec<Point> = pos.iter().map(|p| Point::new(p.x + 0.1, p.y)).collect();
    assert!((prob.priors(&moved, None) - 4.0 * 2.0 * 0.01).abs() < 1e-12);
}

#[test]
fn junction_points_are_anchored_four_times_harder() {
    // An open chain: its two end points are nodes (junctions), the middle one is not.
    let map = PlanarMap {
        edges: vec![edge(
            vec![
                Point::new(0.5, 0.5),
                Point::new(1.5, 0.7),
                Point::new(2.5, 0.5),
            ],
            0,
            1,
            (7, 8),
            false,
        )],
        width: 4,
        height: 2,
        n_labels: 2,
    };
    let vars = build_vars(&map);
    assert_eq!(vars.junction, vec![true, false, true]);
    let rgb = vec![[0.0f32; 3]; 8];
    let mut prob = problem(&map, &vars, &rgb, &WHITE_BLACK);
    prob.w_anchor = 1.0;
    let shift = |v: usize| {
        let mut p = vars.start.clone();
        p[v].y += 0.1;
        prob.priors(&p, None)
    };
    assert!((shift(1) - 0.01).abs() < 1e-12);
    assert!((shift(0) - 4.0 * 0.01).abs() < 1e-12);
    assert!((shift(2) - 4.0 * 0.01).abs() < 1e-12);
}

#[test]
fn the_chunked_data_term_sums_to_the_sequential_one() {
    // 64 x 64 cells, so the chunked path is taken.
    let (w, h) = (64usize, 64usize);
    let rgb = vertical_edge_image(w, h, 30.3);
    let map = PlanarMap {
        edges: vec![edge(vertical_chain(30.1, h), 0, 1, (0, 1), false)],
        width: w,
        height: h,
        n_labels: 2,
    };
    let vars = build_vars(&map);
    let pos = vars.start.clone();
    let n = pos.len();
    let mut prob = problem(&map, &vars, &rgb, &WHITE_BLACK);
    prob.bucket(&pos);
    let mut g1 = vec![Point::new(0.0, 0.0); n];
    let seq = prob.data(&pos, Some(&mut g1));
    // Each of the 64 rows is off by 0.2 in coverage in three channels.
    assert!((seq - 64.0 * 3.0 * 0.04).abs() < 1e-5, "{seq}");
    prob.chunks = 16;
    let mut g16 = vec![Point::new(0.0, 0.0); n];
    let par = prob.data(&pos, Some(&mut g16));
    assert!((par - seq).abs() < 1e-9, "{par} vs {seq}");
    assert!((prob.data(&pos, None) - seq).abs() < 1e-9);
    for (a, b) in g1.iter().zip(&g16) {
        assert!((a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9);
    }
}

// ------------------------------------------------------------------ crossings

fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

#[test]
fn segment_crossing_truth_table() {
    // A proper X, in all four orientations.
    let (a, b, c, d) = (p(0.0, 0.0), p(2.0, 2.0), p(0.0, 2.0), p(2.0, 0.0));
    for (s, t) in [
        ((a, b), (c, d)),
        ((b, a), (c, d)),
        ((a, b), (d, c)),
        ((c, d), (a, b)),
    ] {
        assert!(segments_cross(s.0, s.1, t.0, t.1), "{s:?} x {t:?}");
    }
    // Disjoint, parallel, and one ending short of the other.
    assert!(!segments_cross(a, b, p(3.0, 0.0), p(5.0, 2.0)));
    assert!(!segments_cross(a, p(2.0, 0.0), p(0.0, 1.0), p(2.0, 1.0)));
    assert!(!segments_cross(a, b, p(2.0, 0.0), p(1.2, 0.8)));
    // Ending on the other's line, but beyond its end: no touch.
    assert!(!segments_cross(
        p(0.0, 0.0),
        p(1.0, 0.0),
        p(3.0, 1.0),
        p(2.0, 0.0)
    ));
    // Collinear: overlapping folds back on itself and counts; apart does not.
    assert!(segments_cross(
        p(0.0, 0.0),
        p(2.0, 0.0),
        p(1.0, 0.0),
        p(3.0, 0.0)
    ));
    assert!(!segments_cross(
        p(0.0, 0.0),
        p(1.0, 0.0),
        p(2.0, 0.0),
        p(3.0, 0.0)
    ));
    // A touch counts: an endpoint on the other's interior (a T), and two segments whose
    // endpoints coincide. Callers skip pairs sharing an unknown, so this is only ever a
    // touch between distinct points.
    assert!(segments_cross(
        p(0.0, 0.0),
        p(2.0, 0.0),
        p(1.0, 0.0),
        p(1.0, 1.0)
    ));
    assert!(segments_cross(
        p(0.0, 0.0),
        p(2.0, 0.0),
        p(0.0, 0.0),
        p(0.0, 1.0)
    ));
    assert!(segments_cross(
        p(1.0, 0.0),
        p(2.0, 0.0),
        p(0.0, 0.0),
        p(1.0, 0.0)
    ));
}

#[test]
fn gridline_crossings_in_parameter_order() {
    let mut out = Vec::new();
    crossings(p(0.2, 0.3), p(2.8, 1.9), 0, 1, &mut out);
    // Vertical gridlines x = 0.5, 1.5, 2.5 at t = 0.3/2.6, 1.3/2.6, 2.3/2.6; horizontal
    // y = 0.5, 1.5 at t = 0.2/1.6, 1.2/1.6.
    let want: [(f64, char, f64); 5] = [
        (0.3 / 2.6, 'v', 0.5),
        (0.2 / 1.6, 'h', 0.5),
        (1.3 / 2.6, 'v', 1.5),
        (1.2 / 1.6, 'h', 1.5),
        (2.3 / 2.6, 'v', 2.5),
    ];
    let mut want = want.to_vec();
    want.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(out.len(), want.len());
    for ((t, prov), (wt, kind, line)) in out.iter().zip(&want) {
        assert!((t - wt).abs() < 1e-12, "{t} vs {wt}");
        match (prov, kind) {
            (
                Prov::CrossV {
                    line: l,
                    a: 0,
                    b: 1,
                },
                'v',
            )
            | (
                Prov::CrossH {
                    line: l,
                    a: 0,
                    b: 1,
                },
                'h',
            ) => {
                assert_eq!(l, line)
            }
            _ => panic!("wrong crossing kind at t = {t}"),
        }
    }
    // Walking the other way reverses the order; a segment inside one pixel crosses none,
    // and an end exactly on a gridline is not a crossing.
    crossings(p(2.8, 1.9), p(0.2, 0.3), 0, 1, &mut out);
    assert!((out[0].0 - (1.0 - 2.3 / 2.6)).abs() < 1e-12);
    crossings(p(0.1, 0.1), p(0.4, 0.3), 0, 1, &mut out);
    assert!(out.is_empty());
    crossings(p(0.5, 0.1), p(1.2, 0.1), 0, 1, &mut out);
    assert!(out.is_empty());
}

fn count(edges: Vec<Edge>, w: usize, h: usize) -> usize {
    let map = PlanarMap {
        edges,
        width: w,
        height: h,
        n_labels: 2,
    };
    let vars = build_vars(&map);
    crossings_count(&map, &vars, &vars.start, w, h)
}

#[test]
fn crossing_count_finds_folds_between_and_within_boundaries() {
    let square = || vec![p(1.0, 1.0), p(3.0, 1.0), p(3.0, 3.0), p(1.0, 3.0)];
    assert_eq!(count(vec![edge(square(), 0, 1, (0, 0), true)], 6, 6), 0);
    // A bow tie: the closing segment crosses the second.
    let bow = vec![p(1.0, 1.0), p(3.0, 1.0), p(1.0, 3.0), p(3.0, 3.0)];
    assert_eq!(count(vec![edge(bow.clone(), 0, 1, (0, 0), true)], 6, 6), 1);
    // The same, far from the origin (the pixel-box test must use the box's size).
    let far: Vec<Point> = bow.iter().map(|q| p(q.x + 40.0, q.y + 40.0)).collect();
    assert_eq!(count(vec![edge(far, 0, 1, (0, 0), true)], 50, 50), 1);
    // Open, it is a Z and does not cross itself.
    assert_eq!(count(vec![edge(bow, 0, 1, (0, 1), false)], 6, 6), 0);
    // Two boundaries crossing once; two sharing only their end node do not cross.
    let e1 = edge(
        vec![p(0.2, 1.1), p(1.4, 1.2), p(2.6, 1.3)],
        0,
        1,
        (0, 1),
        false,
    );
    let e2 = edge(vec![p(1.3, 0.1), p(1.35, 2.4)], 0, 1, (2, 3), false);
    assert_eq!(count(vec![e1, e2], 4, 4), 1);
    let f1 = edge(vec![p(0.2, 0.2), p(1.4, 1.2)], 0, 1, (0, 1), false);
    let f2 = edge(vec![p(2.6, 0.3), p(1.4, 1.2)], 0, 1, (2, 1), false);
    assert_eq!(count(vec![f1, f2], 4, 4), 0);
}
