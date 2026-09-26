//! Fast mode, as the command line drives it.
//!
//! The front end is the quality one with the fast flag set (see
//! `inkvec_trace::ColorOptions::fast`): the same palette, labels, planar map and sub-pixel
//! refinement, without the global boundary solve and without gradient recovery. The fit is
//! `inkvec_trace::fast`, a Potrace-class pipeline, in place of the multi-model dynamic
//! program, and the ring repair and shape harmonization after it do not run. Everything
//! from the fills onward -- alpha, seams, the emitter, minify -- is shared with quality
//! mode, so the two write the same kind of document.

use crate::{Args, TraceMode};
use inkvec_fit::{primitives::PrimitiveFit, FittedPath};
use inkvec_trace::planar::PlanarMap;

/// Whether `args` ask for fast mode.
pub(crate) fn on(args: &Args) -> bool {
    args.mode == TraceMode::Fast
}

/// Every boundary of the map, fitted by the fast fitter: a closed boundary that is a circle
/// or an ellipse as that primitive, everything else as lines and cubics.
pub(crate) fn fit(
    map: &PlanarMap,
    fills: &[inkvec_trace::gradient::FillFit],
) -> Vec<(FittedPath, Option<PrimitiveFit>)> {
    let cfg = inkvec_trace::fast::FastFit::default();
    let face_rgb: Vec<[f32; 3]> = fills.iter().map(|f| f.model.representative()).collect();
    inkvec_trace::fast::fit_edges(&map.edges, &face_rgb, &cfg)
}

/// The report line fast mode writes: what it ran, and every option it was given that only
/// steers a stage it skips.
pub(crate) fn report(args: &Args) -> String {
    let d = Args::default();
    let mut ignored: Vec<&str> = Vec::new();
    // Not `--precision` or `--lambda-scale`: the intake rescales both on an oversampled
    // raster, so a change here is not evidence the caller set them.
    let mut check = |set: bool, flag: &'static str| {
        if set {
            ignored.push(flag);
        }
    };
    check(args.tau != d.tau, "--tau");
    check(args.content_units, "--content-units");
    check(args.bezier_cost.is_some(), "--bezier-cost");
    check(args.corner_angle.is_some(), "--corner-angle");
    check(args.time_budget > 0.0, "--time-budget");
    check(args.simplify_faint, "--simplify-faint");
    check(
        args.harmonize_threshold != d.harmonize_threshold,
        "--harmonize-threshold",
    );
    check(args.use_symbols, "--use-symbols");
    let mut line = "fast mode     Potrace-class fit, flat fills; not run: boundary solve, \
                    curve DP, gradients, ring repair, harmonization"
        .to_string();
    if !ignored.is_empty() {
        line.push_str(&format!("; ignored: {}", ignored.join(" ")));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_lists_only_options_that_were_changed() {
        let plain = report(&Args::default());
        assert!(!plain.contains("ignored"), "{plain}");
        let tuned = report(&Args {
            tau: 3.0,
            bezier_cost: Some(4.0),
            ..Args::default()
        });
        assert!(tuned.ends_with("ignored: --tau --bezier-cost"), "{tuned}");
    }

    #[test]
    fn a_square_map_fits_to_lines() {
        let mut labels = vec![0u16; 16 * 16];
        for y in 4..12 {
            for x in 4..12 {
                labels[y * 16 + x] = 1;
            }
        }
        let map = inkvec_trace::planar::build(&labels, 16, 16, 2);
        let fills: Vec<_> = [[1.0f32; 3], [0.0; 3]]
            .iter()
            .map(|&c| inkvec_trace::gradient::FillFit {
                model: inkvec_trace::gradient::FillModel::Flat(c),
                chi2: 0.0,
                params: 3.0,
                cost: 0.0,
            })
            .collect();
        let fits = fit(&map, &fills);
        assert_eq!(fits.len(), map.edges.len());
        let square = fits
            .iter()
            .find(|(f, _)| f.closed && f.segments.len() == 4)
            .expect("the square's boundary is four lines");
        assert!(square.1.is_none());
    }
}
