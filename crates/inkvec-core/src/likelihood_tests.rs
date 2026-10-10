use super::*;

fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

fn col(line: i32, lo: i32, hi: i32) -> Window {
    Window {
        axis: Axis::Column,
        line,
        lo,
        hi,
    }
}

fn row(line: i32, lo: i32, hi: i32) -> Window {
    Window {
        axis: Axis::Row,
        line,
        lo,
        hi,
    }
}

/// A horizontal line walked right has the face above on its left; walked left, below.
#[test]
fn horizontal_line_both_ways() {
    let w = col(3, 1, 4); // strip x in [2.5, 3.5], y in [0.5, 4.5]
    let right = [Piece::Line([p(0.0, 2.3), p(10.0, 2.3)])];
    let left = [Piece::Line([p(10.0, 2.3), p(0.0, 2.3)])];
    assert!((left_area(&right, &w) - 1.8).abs() < 1e-12);
    assert!((left_area(&left, &w) - 2.2).abs() < 1e-12);
}

/// A vertical line walked down has the face to the east on its left; walked up, west.
#[test]
fn vertical_line_both_ways() {
    let w = row(5, 0, 3); // strip y in [4.5, 5.5], x in [-0.5, 3.5]
    let down = [Piece::Line([p(1.2, 0.0), p(1.2, 10.0)])];
    let up = [Piece::Line([p(1.2, 10.0), p(1.2, 0.0)])];
    assert!(
        (left_area(&down, &w) - 2.3).abs() < 1e-12,
        "{}",
        left_area(&down, &w)
    );
    assert!((left_area(&up, &w) - 1.7).abs() < 1e-12);
}

/// A slanted line leaving the window through its bottom is clamped there.
#[test]
fn clamped_at_the_window_end() {
    // y = 2 + 3(x - 2.5): enters the strip at y = 2, leaves at y = 5 > 4.5.
    let w = col(3, 1, 4);
    let l = [Piece::Line([p(2.0, 0.5), p(4.0, 6.5)])];
    // area above the line within y in [0.5, 4.5]: ∫ clamp(2 + 3s, ., 4.5) - 0.5 ds, s in [0,1]
    // = ∫_0^{5/6} (1.5 + 3s) ds + (1/6)·4 = 1.25 + 1.0417 + 0.6667
    let want = 1.5 * (5.0 / 6.0) + 1.5 * (5.0f64 / 6.0).powi(2) + 4.0 / 6.0;
    assert!(
        (left_area(&l, &w) - want).abs() < 1e-12,
        "{} vs {want}",
        left_area(&l, &w)
    );
}

/// A cubic graph's left area against direct numerical integration of its clamped height.
#[test]
fn cubic_graph_matches_integration() {
    // x linear in t (control x equally spaced), so y(x) is the cubic itself.
    let c = [p(1.0, 3.0), p(3.0, 1.2), p(5.0, 4.9), p(7.0, 2.2)];
    let piece = [Piece::Cubic(c)];
    let y_of_x = |x: f64| {
        let t = (x - 1.0) / 6.0;
        let m = 1.0 - t;
        m * m * m * 3.0 + 3.0 * m * m * t * 1.2 + 3.0 * m * t * t * 4.9 + t * t * t * 2.2
    };
    for line in 2..=6 {
        for (lo, hi) in [(1, 4), (2, 3), (3, 5)] {
            let w = col(line, lo, hi);
            let (ylo, yhi) = (lo as f64 - 0.5, hi as f64 + 0.5);
            let n = 200_000;
            let x0 = line as f64 - 0.5;
            let mut acc = 0.0;
            for k in 0..n {
                let x = x0 + (k as f64 + 0.5) / n as f64;
                acc += y_of_x(x).clamp(ylo, yhi) - ylo;
            }
            acc /= n as f64;
            let got = left_area(&piece, &w);
            assert!(
                (got - acc).abs() < 1e-8,
                "line {line} [{lo},{hi}]: {got} vs {acc}"
            );
        }
    }
}

/// A curve that folds inside the strip (an S that doubles back in x) still gives the area of
/// its left face: the two passes over the doubled part cancel.
#[test]
fn a_fold_inside_the_strip() {
    // Polyline: (2.5→3.3) at y=2, back (3.3→2.9) at y=2.4, on (2.9→3.5) at y=2.8.
    let pl = [
        Piece::Line([p(2.0, 2.0), p(3.3, 2.0)]),
        Piece::Line([p(3.3, 2.0), p(2.9, 2.4)]),
        Piece::Line([p(2.9, 2.4), p(4.0, 2.8)]),
    ];
    let w = col(3, 1, 4);
    // Cross-section of the region above the curve, x in [2.5, 3.5]:
    // x in [2.5, 2.9): 2.0 - 0.5; x in [2.9, 3.3): above the third leg... the region left of
    // the walk is the set with odd crossing count above; evaluate by brute force.
    let n = 400_000;
    let mut acc = 0.0;
    for k in 0..n {
        let x = 2.5 + (k as f64 + 0.5) / n as f64;
        let mut ys: Vec<f64> = Vec::new();
        for pc in &pl {
            if let Piece::Line([a, b]) = pc {
                if (a.x - x) * (b.x - x) < 0.0 {
                    let t = (x - a.x) / (b.x - a.x);
                    ys.push(a.y + t * (b.y - a.y));
                }
            }
        }
        ys.sort_by(f64::total_cmp);
        // Above the topmost crossing is the left face; then alternating.
        let mut inside = true;
        let mut prev = 0.5;
        let mut len = 0.0;
        for y in ys.iter().chain(std::iter::once(&4.5)) {
            let y = y.clamp(0.5, 4.5);
            if inside {
                len += y - prev;
            }
            prev = y;
            inside = !inside;
        }
        acc += len;
    }
    acc /= n as f64;
    let got = left_area(&pl, &w);
    assert!((got - acc).abs() < 1e-5, "{got} vs {acc}");
}

/// A U crosses the same column twice: each window is scored by its own pass, and a window
/// between the passes belongs wholly to the face the passes leave it to.
#[test]
fn a_u_crosses_one_column_twice() {
    let u = [
        Piece::Line([p(0.0, 2.0), p(5.0, 2.0)]),
        Piece::Line([p(5.0, 2.0), p(5.0, 8.0)]),
        Piece::Line([p(5.0, 8.0), p(0.0, 8.0)]),
    ];
    // Walking +x at y = 2 the left face is above; walking -x at y = 8 it is below.
    assert!(
        (left_area(&u, &col(3, 1, 3)) - 1.5).abs() < 1e-12,
        "{}",
        left_area(&u, &col(3, 1, 3))
    );
    assert!(
        (left_area(&u, &col(3, 7, 9)) - 1.5).abs() < 1e-12,
        "{}",
        left_area(&u, &col(3, 7, 9))
    );
    // Between the passes, inside the U: the right face.
    assert_eq!(left_area(&u, &col(3, 4, 5)), 0.0);
    // Above both passes: the left face of the upper pass.
    assert_eq!(left_area(&u, &col(3, -4, -3)), 2.0);
}

/// usvg's quarter circle is one cubic with arm (4/3)·tan(π/8)·r.
#[test]
fn quarter_circle_as_usvg_draws_it() {
    let a = EllipticalArc {
        centre: p(0.0, 0.0),
        radii: (1.0, 1.0),
        x_rotation: 0.0,
        start_angle: 0.0,
        sweep_angle: PI / 2.0,
    };
    let c = a.to_cubics(0.1);
    assert_eq!(c.len(), 1);
    let k = (4.0 / 3.0) * (PI / 8.0).tan();
    let Piece::Cubic([p0, p1, p2, p3]) = c[0] else {
        panic!()
    };
    for (q, w) in [
        (p0, (1.0, 0.0)),
        (p1, (1.0, k)),
        (p2, (k, 1.0)),
        (p3, (0.0, 1.0)),
    ] {
        assert!(
            (q.x - w.0).abs() < 1e-12 && (q.y - w.1).abs() < 1e-12,
            "{q:?} vs {w:?}"
        );
    }
    // A half turn is two, a full turn four, below the 3600-tolerance radius.
    let half = EllipticalArc {
        sweep_angle: PI,
        ..a
    };
    assert_eq!(half.to_cubics(0.1).len(), 2);
    // A large radius against the tolerance needs more.
    // (1.1163 · 50000)^(1/6) = 6.18 pieces per turn.
    let big = EllipticalArc {
        radii: (5000.0, 5000.0),
        sweep_angle: 2.0 * PI,
        ..a
    };
    assert_eq!(big.to_cubics(0.1).len(), 7);
}

/// A disc drawn as usvg's four cubics loses about 2.7e-4 of its radius at the 45° points,
/// and the window areas see it: the as-rendered and the true areas of a window there differ.
#[test]
fn rendered_circle_differs_from_the_true_one() {
    let r = 100.0;
    let a = EllipticalArc {
        centre: p(0.0, 0.0),
        radii: (r, r),
        x_rotation: 0.0,
        // From (-r, 0) over the top (negative y) to (r, 0).
        start_angle: PI,
        sweep_angle: PI,
    };
    let truth = [Piece::Arc(a)];
    let drawn = RenderModel::usvg().as_rendered(&truth);
    // The default model draws arcs exactly.
    assert_eq!(RenderModel::default().as_rendered(&truth), truth.to_vec());
    // The cubic is exact at the ends and the middle of each quarter, and furthest out (by
    // 2.7e-4 r) about 19° from either end: the column at x = -33 meets the circle at
    // y = -94.4, 19.3° from the top.
    let w = col(-33, -98, -91);
    let d = left_area(&drawn, &w) - left_area(&truth, &w);
    assert!(d.abs() > 0.005 && d.abs() < 0.05, "{d}");
}

/// Prefix moments recover a quadratic graph from its exact column means.
#[test]
fn run_moments_fit_a_quadratic() {
    let (a, b, c) = (3.2, 0.31, -0.017);
    let f_mean = |x0: f64| {
        // mean over [x0, x0+1] of a + b x + c x²
        let pr = |x: f64| a * x + b * x * x / 2.0 + c * x * x * x / 3.0;
        pr(x0 + 1.0) - pr(x0)
    };
    let obs: Vec<RunObs> = (0..12)
        .map(|i| {
            let x0 = i as f64 - 0.5;
            let m = f_mean(x0);
            let lo = (m.floor() as i32) - 1;
            RunObs {
                window: col(i, lo, lo + 3),
                s: i as f64,
                sum: m - (lo as f64 - 0.5),
                var: 1e-6,
                left_low: true,
                share: 1.0,
            }
        })
        .collect();
    let mo = RunMoments::new(&obs, 2);
    let (theta, chi) = mo.fit(0..12).expect("fit");
    assert!(
        (theta[0] - a).abs() < 1e-9 && (theta[1] - b).abs() < 1e-9 && (theta[2] - c).abs() < 1e-9,
        "{theta:?}"
    );
    assert!(chi.chi2 < 1e-6, "{chi:?}");
    assert!(mo.fit(0..2).is_none(), "fewer windows than coefficients");
    // A line through the same means misfits by many standard deviations.
    let line = RunMoments::new(&obs, 1).fit(0..12).expect("line").1;
    assert!(line.chi2 > 100.0, "{line:?}");
}

/// A degenerate arc is its chord; a window lists its pixels along its line.
#[test]
fn degenerate_arcs_and_window_pixels() {
    let a = EllipticalArc {
        centre: p(1.0, 2.0),
        radii: (0.0, 3.0),
        x_rotation: 0.0,
        start_angle: 0.0,
        sweep_angle: PI / 2.0,
    };
    let c = a.to_cubics(0.1);
    let [Piece::Line([s, e])] = c[..] else {
        panic!("{c:?}")
    };
    assert!(
        s.dist(p(1.0, 2.0)) < 1e-12 && e.dist(p(1.0, 5.0)) < 1e-12,
        "{s:?} {e:?}"
    );
    let w = col(3, 5, 7);
    assert_eq!((w.len(), w.is_empty()), (3, false));
    assert_eq!(w.pixels().collect::<Vec<_>>(), vec![(3, 5), (3, 6), (3, 7)]);
    assert_eq!(
        row(2, 0, 1).pixels().collect::<Vec<_>>(),
        vec![(0, 2), (1, 2)]
    );
    let empty = col(0, 4, 3);
    assert_eq!((empty.len(), empty.is_empty()), (0, true));
}

/// A quadratic and the same curve raised to a cubic give every window the same area.
#[test]
fn a_quadratic_is_its_raised_cubic() {
    let (a, b, c) = (p(0.5, 3.0), p(3.0, -1.0), p(6.5, 4.0));
    let towards =
        |q: Point, r: Point| p(q.x + 2.0 / 3.0 * (r.x - q.x), q.y + 2.0 / 3.0 * (r.y - q.y));
    let quad = [Piece::Quad([a, b, c])];
    let cubic = [Piece::Cubic([a, towards(a, b), towards(c, b), c])];
    for line in 1..=6 {
        for w in [col(line, 0, 4), col(line, 1, 2), col(line, -1, 5)] {
            let (q, k) = (left_area(&quad, &w), left_area(&cubic, &w));
            assert!((q - k).abs() < 1e-12, "{w:?}: {q} vs {k}");
        }
    }
}

/// Arcs are integrated through fine cubics; a curve that never enters a window's strip gives
/// no area (NaN); a window no pass comes near is wholly one face's, on rows as on columns.
#[test]
fn arcs_rows_and_curves_that_miss() {
    let arc = EllipticalArc {
        centre: p(0.0, 0.0),
        radii: (10.0, 10.0),
        x_rotation: 0.0,
        start_angle: PI,
        sweep_angle: PI,
    };
    assert!(power(&Piece::Arc(arc)).is_none());
    assert_eq!(
        u_range(&Piece::Arc(arc), Axis::Column),
        (f64::NEG_INFINITY, f64::INFINITY)
    );
    // The upper half circle walked from (-10, 0) to (10, 0): at x = 0 it is at y = -10, and
    // the left face (above, walking +x) holds y in [-10.5, -10] of the window [-10.5, -9.5],
    // plus the circle's mean drop across the column, 1/(24 r) to the next order.
    let a = left_area(&[Piece::Arc(arc)], &col(0, -10, -10));
    assert!((a - (0.5 + 1.0 / 240.0)).abs() < 1e-5, "{a}");
    assert!(left_area(&[], &col(0, 0, 1)).is_nan());
    // A cubic whose control polygon reaches x = 4 but whose curve turns back at x = 3.
    let bulge = [Piece::Cubic([
        p(0.0, 0.0),
        p(4.0, 1.0),
        p(4.0, 2.0),
        p(0.0, 3.0),
    ])];
    assert!(left_area(&bulge, &col(4, 0, 3)).is_nan());
    // Walking +y the left face is east: a row window east of the line is wholly left.
    let down = [Piece::Line([p(1.2, 0.0), p(1.2, 10.0)])];
    assert_eq!(left_area(&down, &row(5, 5, 7)), 3.0);
    assert_eq!(left_area(&down, &row(5, -6, -4)), 0.0);
}

/// The lattice floor's two variances.
#[test]
fn lattice_floor() {
    let f = Floor::lattice(32);
    assert_eq!(f.lattice, 32);
    assert!((f.window_var - 1.0 / (12.0 * 32768.0)).abs() < 1e-18);
    assert!((f.edge_var - 1.0 / (12.0 * 1024.0)).abs() < 1e-15);
    assert_eq!(Floor::lattice(0).lattice, 1);
}

/// A fixed set of run windows behind the trait, to test its provided methods.
struct Fixed {
    runs: Vec<RunObs>,
    floor: Floor,
    lattice: bool,
}

/// The same, saying where each window sits along the edge's points.
struct Placed {
    inner: Fixed,
    at: Vec<f64>,
}

impl BoundaryLikelihood for Placed {
    fn edge_count(&self) -> usize {
        1
    }
    fn runs(&self, e: usize) -> &[RunObs] {
        self.inner.runs(e)
    }
    fn render_model(&self) -> RenderModel {
        RenderModel::default()
    }
    fn floor(&self) -> Floor {
        self.inner.floor
    }
    fn edge_on_lattice(&self, _: usize) -> bool {
        false
    }
    fn chi2_local(&self, _: Owner, _: &[(usize, &[Piece])]) -> Chi2 {
        Chi2::default()
    }
    fn junctions(&self) -> &[JunctionReport] {
        &[]
    }
    fn corners(&self) -> &[CornerProposal] {
        &[]
    }
    fn density(&self, _: usize) -> Vec<(f64, f64)> {
        Vec::new()
    }
    fn window_point_index(&self, _: usize) -> Vec<f64> {
        self.at.clone()
    }
}

/// The scorer partitions the windows among any segmentation of the points (each window in
/// exactly one span, wrapping on a closed edge), and its O(1) moments and fits are the direct
/// sums.
#[test]
fn the_scorer_partitions_and_sums() {
    let runs: Vec<RunObs> = (0..12)
        .map(|i| RunObs {
            window: col(i, 1, 4),
            s: i as f64,
            sum: 1.8 + 0.01 * ((i * 7) % 5) as f64,
            var: 1e-4 * (1 + i % 3) as f64,
            left_low: true,
            share: 1.0,
        })
        .collect();
    let at: Vec<f64> = (0..12).map(|i| 0.5 + 1.7 * i as f64).collect(); // in [0, 20)
    let lik = Placed {
        inner: Fixed {
            runs: runs.clone(),
            floor: Floor::lattice(32),
            lattice: false,
        },
        at,
    };
    let sc = EdgeScorer::new(&lik, 0, 20);
    for cuts in [vec![0, 5, 11, 20], vec![0, 1, 2, 3, 19, 20], vec![0, 20]] {
        let mut seen = vec![0; 12];
        for w in cuts.windows(2) {
            let (a, b) = sc.windows_between_points(w[0], w[1]);
            for k in a.chain(b) {
                seen[k] += 1;
            }
        }
        assert!(seen.iter().all(|&c| c == 1), "{cuts:?}: {seen:?}");
    }
    // A closed edge cut at points 7 and 15: the second span runs through the seam.
    let (a1, b1) = sc.windows_between_points(7, 15);
    let (a2, b2) = sc.windows_between_points(15, 7);
    let mut seen = vec![0; 12];
    for k in a1.chain(b1).chain(a2).chain(b2) {
        seen[k] += 1;
    }
    assert!(seen.iter().all(|&c| c == 1), "{seen:?}");
    // Moments against the direct sums.
    let m = sc.weight_moments(3..9);
    for (p, mp) in m.iter().enumerate() {
        let want: f64 = runs[3..9]
            .iter()
            .map(|o| (o.window.line as f64).powi(p as i32) / o.var)
            .sum();
        assert!(
            (mp - want).abs() < 1e-9 * want.abs().max(1.0),
            "p {p}: {mp} vs {want}"
        );
    }
    let (theta, chi) = sc.best_graph(0..12, 1).expect("line");
    let (t2, c2) = RunMoments::new(&runs, 1).fit(0..12).expect("line");
    assert_eq!(theta, t2);
    assert_eq!(chi, c2);
    assert!(sc.best_graph(0..12, 3).is_some());
    let line = [Piece::Line([p(-1.0, 2.3), p(13.0, 2.3)])];
    assert_eq!(sc.chi2(2..6, &line).m, 4);
}

/// SVG's endpoint arcs become centre arcs through the same endpoints, with the flags' sense.
#[test]
fn svg_arcs_in_centre_form() {
    let (a, b) = (p(10.0, 0.0), p(0.0, 10.0));
    for (large, sweep) in [(false, false), (false, true), (true, false), (true, true)] {
        let Piece::Arc(arc) = Piece::from_svg_arc(a, 10.0, 10.0, 0.0, large, sweep, b) else {
            panic!("an arc")
        };
        let (s, e) = (
            arc.at(arc.start_angle),
            arc.at(arc.start_angle + arc.sweep_angle),
        );
        assert!(
            s.dist(a) < 1e-9 && e.dist(b) < 1e-9,
            "{large} {sweep}: {s:?} {e:?}"
        );
        assert_eq!(arc.sweep_angle > 0.0, sweep, "{large} {sweep}");
        let quarter = (arc.sweep_angle.abs() - PI / 2.0).abs() < 1e-9;
        assert_eq!(quarter, !large, "{large} {sweep}: {}", arc.sweep_angle);
    }
    // Radii too small for the chord are scaled up to a half circle on it.
    let Piece::Arc(half) =
        Piece::from_svg_arc(p(0.0, 0.0), 1.0, 1.0, 0.0, false, true, p(4.0, 0.0))
    else {
        panic!("an arc")
    };
    assert!((half.radii.0 - 2.0).abs() < 1e-9 && (half.sweep_angle.abs() - PI).abs() < 1e-9);
    // Coincident ends or a zero radius: a line.
    assert!(matches!(
        Piece::from_svg_arc(a, 0.0, 3.0, 0.0, false, true, b),
        Piece::Line(_)
    ));
}

impl BoundaryLikelihood for Fixed {
    fn edge_count(&self) -> usize {
        1
    }
    fn runs(&self, _: usize) -> &[RunObs] {
        &self.runs
    }
    fn render_model(&self) -> RenderModel {
        RenderModel::default()
    }
    fn floor(&self) -> Floor {
        self.floor
    }
    fn edge_on_lattice(&self, _: usize) -> bool {
        self.lattice
    }
    fn chi2_local(&self, _: Owner, _: &[(usize, &[Piece])]) -> Chi2 {
        Chi2::default()
    }
    fn junctions(&self) -> &[JunctionReport] {
        &[]
    }
    fn corners(&self) -> &[CornerProposal] {
        &[]
    }
    fn density(&self, _: usize) -> Vec<(f64, f64)> {
        Vec::new()
    }
}

/// An offset every window of an edge shares costs `χ²` per window, but next to nothing once
/// the per-edge floor term absorbs it (A3), and only on an edge that shares it.
#[test]
fn a_shared_offset_is_the_floors() {
    // A horizontal edge at y = 2.3 walked +x, read 0.01 px low in every window.
    let runs: Vec<RunObs> = (0..10)
        .map(|i| RunObs {
            window: col(i, 1, 4),
            s: i as f64,
            sum: 1.8 + 0.01,
            var: 1e-4,
            left_low: true,
            share: 1.0,
        })
        .collect();
    let line = [Piece::Line([p(-1.0, 2.3), p(11.0, 2.3)])];
    let floor = Floor {
        lattice: 32,
        window_var: 0.0,
        edge_var: 1e-2,
    };
    let on = Fixed {
        runs: runs.clone(),
        floor,
        lattice: true,
    };
    let c = on.chi2_run(0, 0..10, &line);
    assert_eq!(c.m, 10);
    assert!((c.chi2 - 10.0).abs() < 1e-6, "{c:?}");
    // χ² − b²(Σr/V)²/(1 + b²Σ1/V) = 10 − 1e4/1001.
    assert!((c.chi2_floor - (10.0 - 1e4 / 1001.0)).abs() < 1e-6, "{c:?}");
    let r = on.residuals_run(0, 0..10, &line);
    assert!(r.iter().all(|v| (v - 1.0).abs() < 1e-6), "{r:?}");
    let (theta, _) = on.run_moments(0, 1).fit(0..10).expect("fit");
    assert!(
        (theta[0] - 2.31).abs() < 1e-9 && theta[1].abs() < 1e-9,
        "{theta:?}"
    );
    let off = Fixed {
        runs,
        floor,
        lattice: false,
    };
    let c = off.chi2_run(0, 0..10, &line);
    assert_eq!(c.chi2_floor, c.chi2);
    assert_eq!((c + c).m, 20);
}

/// The Kalman filter's χ² and log det are those of the dense covariance
/// `diag(v) + τ² ρ^|i−j|`, and reduce to the independent and the shared-offset cases.
#[test]
fn correlated_chi2_is_the_dense_one() {
    let r = [0.013, -0.004, 0.021, 0.009, -0.011, 0.017];
    let v = [1e-4, 2e-4, 1.5e-4, 1e-4, 3e-4, 1e-4];
    for (tau2, rho) in [
        (0.0f64, 0.5f64),
        (4e-4, 0.0),
        (4e-4, 0.6),
        (4e-4, 1.0),
        (1e-2, 1.0),
    ] {
        let n = r.len();
        let sigma: Vec<Vec<f64>> = (0..n)
            .map(|i| {
                (0..n)
                    .map(|j| {
                        let d = (i as i32 - j as i32).unsigned_abs() as i32;
                        tau2 * rho.powi(d) + if i == j { v[i] } else { 0.0 }
                    })
                    .collect()
            })
            .collect();
        let x = solve_spd(&sigma, &r).expect("positive definite");
        let want: f64 = x.iter().zip(&r).map(|(a, b)| a * b).sum();
        // log det from the Cholesky factor's diagonal, by the same elimination.
        let mut l = vec![vec![0.0; n]; n];
        let mut logdet = 0.0;
        for i in 0..n {
            for j in 0..=i {
                let s: f64 = (0..j).map(|k| l[i][k] * l[j][k]).sum();
                if i == j {
                    l[i][i] = (sigma[i][i] - s).sqrt();
                    logdet += 2.0 * l[i][i].ln();
                } else {
                    l[i][j] = (sigma[i][j] - s) / l[j][j];
                }
            }
        }
        let c = vec![Correlated { tau2, rho }; n];
        let (chi2, ld) = chi2_correlated(&r, &v, &c);
        assert!(
            (chi2 - want).abs() < 1e-9 * want.max(1.0),
            "tau2 {tau2} rho {rho}: {chi2} vs {want}"
        );
        assert!((ld - logdet).abs() < 1e-9, "{ld} vs {logdet}");
    }
    // No correlated part: the independent sum.
    let plain: f64 = r.iter().zip(&v).map(|(a, b)| a * a / b).sum();
    let (chi2, _) = chi2_correlated(&r, &v, &[]);
    assert!((chi2 - plain).abs() < 1e-12 * plain);
}

/// A singular system (the same column three times cannot fix a slope) is refused.
#[test]
fn a_singular_fit_is_refused() {
    let o = RunObs {
        window: col(3, 1, 4),
        s: 0.0,
        sum: 1.8,
        var: 1e-4,
        left_low: true,
        share: 1.0,
    };
    assert!(RunMoments::new(&[o, o, o], 1).fit(0..3).is_none());
    assert!(RunMoments::new(&[o, o, o], 0).fit(0..3).is_some());
}
