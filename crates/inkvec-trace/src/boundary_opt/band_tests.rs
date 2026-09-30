//! The band data term: exact coverage (also on gridlines, where the per-pixel term chose
//! the wrong side), an analytic gradient that matches finite differences, and an energy
//! that has no jumps.

use super::super::*;
use crate::planar::{self, PlanarMap};

/// A problem over `map` in band mode, set up as the solve sets it up.
fn band_problem<'a>(
    map: &'a PlanarMap,
    vars: &'a Vars,
    rgb: &'a [[f32; 3]],
    face: &'a [FillModel],
) -> Problem<'a> {
    let (w, h) = (map.width, map.height);
    let mut p = Problem {
        map,
        vars,
        rgb,
        face,
        w,
        h,
        pieces: Vec::new(),
        head: vec![-1; w * h],
        vhead: vec![-1; h],
        touched: Vec::new(),
        spare: Vec::new(),
        scratch: Scratch::default(),
        alpha: None,
        w_kink: 0.0,
        w_anchor: 0.0,
        band: None,
        bscratch: Default::default(),
        band_norm: (0.0, 0.0),
        active: None,
    };
    super::setup(&mut p);
    p
}

/// The planar map of a label image, its unknowns (frame pinned), and the image.
fn labelled(labels: &[u16], w: usize, h: usize, n: usize) -> (PlanarMap, Vars) {
    let map = planar::build(labels, w, h, n);
    let mut vars = build_vars(&map);
    super::pin_frame(&map, &mut vars);
    (map, vars)
}

/// Exact box coverage of the half-plane `x > x0` in pixel column `x`.
fn right_of(x: usize, x0: f64) -> f64 {
    ((x as f64 + 0.5) - x0).clamp(0.0, 1.0)
}

#[test]
fn a_straight_edge_renders_exactly_also_on_a_gridline() {
    let (w, h) = (6usize, 3usize);
    let labels: Vec<u16> = (0..w * h).map(|i| (i % w >= 3) as u16).collect();
    let face = [FillModel::Flat([0.0; 3]), FillModel::Flat([1.0; 3])];
    for delta in [0.0, -0.3, 0.2, -0.5, 0.45] {
        let (mut map, _) = labelled(&labels, w, h, 2);
        // Move the boundary at x = 2.5 (and the frame nodes it ends on) to 2.5 + delta.
        for e in map.edges.iter_mut() {
            for p in e.points.iter_mut() {
                if (p.x - 2.5).abs() < 1e-9 {
                    p.x = 2.5 + delta;
                }
            }
        }
        let mut vars = build_vars(&map);
        super::pin_frame(&map, &mut vars);
        let x0 = 2.5 + delta;
        let rgb: Vec<[f32; 3]> = (0..w * h)
            .map(|i| [right_of(i % w, x0) as f32; 3])
            .collect();
        let mut prob = band_problem(&map, &vars, &rgb, &face);
        let pos = vars.start.clone();
        let e = prob.energy(&pos, None);
        assert!(
            e < 1e-12,
            "delta {delta}: an exact render must have no residual, got {e}"
        );
        // And the wrong side is not interchangeable with the right one.
        let flipped: Vec<[f32; 3]> = rgb.iter().map(|c| [1.0 - c[0]; 3]).collect();
        let mut prob = band_problem(&map, &vars, &flipped, &face);
        assert!(prob.energy(&pos, None) > 1.0, "delta {delta}");
    }
}

/// A small deterministic generator.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Three faces: a disc of face 1 and a bar of face 2 on face 0, with a gradient fill, and
/// every interior point moved off the pixel grid.
fn three_faces(rng: &mut Lcg) -> (PlanarMap, Vars, Vec<[f32; 3]>, Vec<FillModel>) {
    let (w, h) = (10usize, 9usize);
    let labels: Vec<u16> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f64, (i / w) as f64);
            if (x - 4.0).powi(2) + (y - 4.0).powi(2) < 7.0 {
                1
            } else if x >= 7.0 {
                2
            } else {
                0
            }
        })
        .collect();
    let (mut map, _) = labelled(&labels, w, h, 3);
    for e in map.edges.iter_mut() {
        if e.left == band::OUT || e.right == band::OUT {
            continue;
        }
        for p in e.points.iter_mut() {
            p.x += (rng.next() - 0.5) * 0.6;
            p.y += (rng.next() - 0.5) * 0.6;
        }
    }
    let mut vars = build_vars(&map);
    band::pin_frame(&map, &mut vars);
    let rgb: Vec<[f32; 3]> = (0..w * h)
        .map(|_| [rng.next() as f32, rng.next() as f32, rng.next() as f32])
        .collect();
    let face = vec![
        FillModel::Flat([0.9, 0.8, 0.1]),
        FillModel::Linear {
            p0: (2.0, 2.0),
            p1: (6.0, 6.0),
            c0: [0.1, 0.2, 0.9],
            c1: [0.6, 0.1, 0.3],
            interp: crate::gradient::Interp::Srgb,
            mids: Vec::new(),
        },
        FillModel::Flat([0.2, 0.7, 0.4]),
    ];
    (map, vars, rgb, face)
}

#[test]
fn the_band_gradient_matches_finite_differences() {
    let mut rng = Lcg(3);
    for _ in 0..3 {
        let (map, vars, rgb, face) = three_faces(&mut rng);
        let mut prob = band_problem(&map, &vars, &rgb, &face);
        prob.w_kink = 0.2;
        prob.w_anchor = 0.1;
        let n = vars.start.len();
        let pos: Vec<Point> = vars
            .start
            .iter()
            .enumerate()
            .map(|(v, p)| {
                let (dx, dy) = ((rng.next() - 0.5) * 0.2, (rng.next() - 0.5) * 0.2);
                Point::new(
                    if vars.pin[v] & 1 != 0 { p.x } else { p.x + dx },
                    if vars.pin[v] & 2 != 0 { p.y } else { p.y + dy },
                )
            })
            .collect();
        let mut grad = vec![Point::new(0.0, 0.0); n];
        prob.energy(&pos, Some(&mut grad));
        let d = 1e-6;
        let mut checked = 0;
        for v in 0..n {
            for axis in 0..2 {
                if vars.pin[v] & (1 << axis) != 0 {
                    continue;
                }
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
                assert!(
                    (num - ana).abs() < 1e-4 * (1.0 + ana.abs()),
                    "var {v} axis {axis}: analytic {ana}, numeric {num}"
                );
                checked += (ana.abs() > 1e-3) as usize;
            }
        }
        assert!(checked > 20, "{checked}");
    }
}

#[test]
fn the_band_energy_has_no_jumps() {
    // Slide every interior point together, in steps far smaller than a pixel, across many
    // gridlines: the energy changes by at most its slope times the step. The per-pixel
    // term jumped by a whole pixel's residual whenever a piece changed pixel.
    let mut rng = Lcg(11);
    let (map, vars, rgb, face) = three_faces(&mut rng);
    let mut prob = band_problem(&map, &vars, &rgb, &face);
    let n = vars.start.len();
    let dir: Vec<Point> = (0..n)
        .map(|v| {
            Point::new(
                if vars.pin[v] & 1 != 0 { 0.0 } else { 1.0 },
                if vars.pin[v] & 2 != 0 { 0.0 } else { 0.37 },
            )
        })
        .collect();
    let step = 1e-4;
    let mut prev: Option<f64> = None;
    let mut worst = 0.0f64;
    for k in 0..6000 {
        let t = -0.3 + k as f64 * step;
        let pos: Vec<Point> = vars
            .start
            .iter()
            .zip(&dir)
            .map(|(p, d)| Point::new(p.x + t * d.x, p.y + t * d.y))
            .collect();
        let e = prob.energy(&pos, None);
        if let Some(p) = prev {
            worst = worst.max((e - p).abs());
        }
        prev = Some(e);
    }
    // A slope of a few units per pixel at most: well under 0.01 per step of 1e-4.
    assert!(worst < 1e-2, "largest change over one step: {worst}");
}

#[test]
fn stretches_summed_at_once_equal_the_pixel_by_pixel_sum() {
    let mut rng = Lcg(5);
    for _ in 0..4 {
        let (map, vars, rgb, face) = three_faces(&mut rng);
        let mut prob = band_problem(&map, &vars, &rgb, &face);
        let n = vars.start.len();
        let pos: Vec<Point> = vars
            .start
            .iter()
            .enumerate()
            .map(|(v, p)| {
                let (dx, dy) = ((rng.next() - 0.5) * 0.3, (rng.next() - 0.5) * 0.3);
                Point::new(
                    if vars.pin[v] & 1 != 0 { p.x } else { p.x + dx },
                    if vars.pin[v] & 2 != 0 { p.y } else { p.y + dy },
                )
            })
            .collect();
        let mut g_fast = vec![Point::new(0.0, 0.0); n];
        prob.bucket_band(&pos);
        let band = prob.band.take().unwrap();
        let mut bs = std::mem::take(&mut prob.bscratch);
        let e_fast = prob.band_data(&band, &pos, &mut bs, Some(&mut g_fast));
        let mut g_cells = vec![Point::new(0.0, 0.0); n];
        let e_cells = prob.band_data_cells(&band, &pos, Some(&mut g_cells));
        assert!(
            (e_fast - e_cells).abs() < 1e-9 * (1.0 + e_cells.abs()),
            "{e_fast} vs {e_cells}"
        );
        for (a, b) in g_fast.iter().zip(&g_cells) {
            assert!(
                (a.x - b.x).abs() < 1e-8 && (a.y - b.y).abs() < 1e-8,
                "{a:?} vs {b:?}"
            );
        }
    }
}
