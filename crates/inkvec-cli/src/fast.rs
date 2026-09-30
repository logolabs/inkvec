//! Fast mode, as the command line drives it.
//!
//! The front end is the quality one with the fast flag set (see
//! `inkvec_trace::ColorOptions::fast`): the same palette, labels, planar map and sub-pixel
//! refinement, without the global boundary solve and without gradient recovery. The fit is
//! `inkvec_trace::fast`, a Potrace-class pipeline, in place of the multi-model dynamic
//! program, and the ring repair and shape harmonization after it do not run. Everything
//! from the fills onward -- alpha, seams, the emitter, minify -- is shared with quality
//! mode, so the two write the same kind of document.
//!
//! Where it plugs in: `pipeline.rs` asks [`on`] twice, once to set the front end's fast
//! flag in the colour options and once to pick [`fit`] instead of the per-edge dynamic
//! program; after emitting, it puts [`report`] at the top of the stats. [`fast_ignored`]
//! is re-exported from `lib.rs` so Inkvec Studio (`studio/core/src/options.rs`) can check
//! its own list of Quality-only settings against it. In: the settled [`Args`], and for the
//! fit the planar map and each face's fill. Out: one fitted path per map edge, and a line
//! of text for the report.

use crate::args::{Args, TraceMode};
use inkvec_fit::{primitives::PrimitiveFit, FittedPath};
use inkvec_trace::planar::PlanarMap;

/// Whether `args` ask for fast mode.
pub(crate) fn on(args: &Args) -> bool {
    args.mode == TraceMode::Fast
}

/// Every boundary of the map, fitted by the fast fitter: a closed boundary that is a circle
/// or an ellipse as that primitive, everything else as lines and cubics.
///
/// `fills` is indexed by face id, so the fitter can loosen its tolerance on an edge between
/// two faces of similar colour or along a gradient (see `inkvec_trace::fast::fit_edges`).
/// The result is parallel to `map.edges`: entry `i` is edge `i`'s path, in the map's own
/// pixel coordinates (pixel centres at integers, so the canvas spans `-0.5 .. w - 0.5`),
/// with `Some` primitive when the edge was recognised as a circle or an ellipse. Each shared
/// edge is fitted once, so both faces that meet along it draw the same curve. The map's
/// size goes along so that the image frame, when one face runs round the whole border, is
/// written as the image rectangle rather than fitted.
pub(crate) fn fit(
    map: &PlanarMap,
    fills: &[inkvec_trace::gradient::FillFit],
) -> Vec<(FittedPath, Option<PrimitiveFit>)> {
    let cfg = inkvec_trace::fast::FastFit::default();
    inkvec_trace::fast::fit_edges(&map.edges, fills, &cfg, map.width, map.height)
}

/// Every option in `args`, moved off its default, that fast mode's colour trace does not read:
/// each one only steers a stage fast mode skips.
///
/// Public so a front end can check its own list of Quality-only settings against this one
/// (Inkvec Studio's Tune tab hides them in Fast). Black & white and line art take their own
/// routes, which do not depend on the mode; this list is about the colour trace.
///
/// A setting counts as "moved" when it differs from [`Args::default`], so the answer does
/// not depend on whether fast mode is actually on: callers ask it about a mode they may be
/// about to switch to. The flags come back in a fixed order, spelled as on the command line.
pub fn fast_ignored(args: &Args) -> Vec<&'static str> {
    let d = Args::default();
    let mut ignored: Vec<&'static str> = Vec::new();
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
    // Two of the stages the report line names as not run. Not `--no-gradients`: fast mode's
    // front end still merges smooth ramps into one gradient fill on an opaque image, and the
    // flag turns that off.
    check(args.no_repair, "--no-repair");
    check(
        args.harmonize != d.harmonize,
        if args.harmonize {
            "--harmonize"
        } else {
            "--no-harmonize"
        },
    );
    ignored
}

/// The report line fast mode writes: what it ran, and every option it was given that only
/// steers a stage it skips. The ignored list is [`fast_ignored`]'s, appended only when it is
/// not empty, so a run on default settings prints the fixed first half alone.
pub(crate) fn report(args: &Args) -> String {
    let ignored = fast_ignored(args);
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
    fn the_stages_fast_mode_skips_are_named_when_their_options_are_set() {
        let d = Args::default();
        let tuned = fast_ignored(&Args {
            no_repair: true,
            harmonize: !d.harmonize,
            ..Args::default()
        });
        assert_eq!(tuned, ["--no-repair", "--no-harmonize"]);
        assert!(fast_ignored(&d).is_empty());
        // Fast still merges smooth ramps, so turning gradients off is not ignored.
        assert!(fast_ignored(&Args {
            no_gradients: true,
            ..Args::default()
        })
        .is_empty());
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
