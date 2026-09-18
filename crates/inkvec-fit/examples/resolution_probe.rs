//! Diagnose sampling-phase sensitivity on an exact polygon (no raster noise).
use inkvec_core::{Point, Polyline};
use inkvec_fit::{
    multimodel::{optimal_multimodel, path_max_deviation},
    FitConfig,
};

fn main() {
    for side in [180usize, 193, 201, 257, 401, 513] {
        let mut pts = Vec::new();
        let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        for edge in 0..4 {
            let (a, b) = (corners[edge], corners[(edge + 1) % 4]);
            for i in 0..side {
                let t = i as f64 / side as f64;
                let x = (a.0 + (b.0 - a.0) * t) * side as f64;
                let y = (a.1 + (b.1 - a.1) * t) * side as f64;
                pts.push(Point::new(x * 0.8 - y * 0.6, x * 0.6 + y * 0.8));
            }
        }
        for phase in [0, 1, 2] {
            let mut p = pts.clone();
            p.rotate_left(phase);
            let poly = Polyline::new(p, vec![0.05; pts.len()], true);
            let cfg = FitConfig::from_precision(side as f64, 0.1, 2.0);
            let fit = optimal_multimodel(&poly, &cfg);
            println!(
                "side {side} phase {phase}: {} segments, max deviation {:.5}",
                fit.segments.len(),
                path_max_deviation(&poly, &fit)
            );
        }
    }
}
