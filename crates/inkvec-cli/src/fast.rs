//! Fast and balanced mode, as the command line drives them.
//!
//! The front end is the quality one with the fast flag set (see
//! `inkvec_trace::ColorOptions::fast`): the same palette, labels, planar map and sub-pixel
//! refinement, without the global boundary solve and without gradient recovery. The fit is
//! `inkvec_trace::fast`, a Potrace-class pipeline, in place of the multi-model dynamic
//! program, and the ring repair and shape harmonization after it do not run. Everything
//! from the fills onward -- alpha, seams, the emitter, minify -- is shared with quality
//! mode, so the two write the same kind of document.
//!
//! **Balanced** (`--mode balanced`, opt-in) is Fast with two of Quality's ideas added back
//! at a fraction of their price, on rasters up to [`BALANCED_MAX_SIDE`] on their longer
//! side:
//!
//! 1. the global boundary solve (`inkvec_trace::boundary_opt`), stopped after
//!    [`BALANCED_SOLVE_ITERS`] iterations ([`solve_iters`]); and
//! 2. the Fast fitter with tolerances a quarter finer
//!    (`inkvec_trace::fast::FastFit::balanced`, [`fit_config`]).
//!
//! The r2-fastq study (2026-10-02) priced every Quality stage by dE00 gained per
//! millisecond added to Fast. At 128 px the gap to Quality is geometry, not colour: 94 % of
//! Fast's error lies within one source pixel of a contour, and its Shapley split is the
//! curve fit 49 %, the boundary solve 42 %, the front end 9 %. The curve dynamic program is
//! 17-30x Fast's work; the solve is cheap, and an anytime one: 4 of its 32 iterations buy
//! 81 % of its gain. The fitter's tolerances cost no time at all. So balanced takes the
//! solve's first iterations and the finer tolerances, and leaves the dynamic program,
//! gradient recovery, ring repair and harmonization to Quality.
//!
//! The size gate: on 2048 px renders of the 21 cross-compare cases the same arm bought
//! -5 % dE00 on the 19 flat ones for 2.3x the work, raised parameters by a third and
//! regressed a gradient case, because judged at 1024 px a 2048 px raster's geometry is
//! already finer than the judge can see; what is left there is the fill model. Above the
//! threshold balanced is Fast, byte for byte, and its report line says so.
//!
//! Inspired by: S. Zilberstein (1996), "Using anytime algorithms in intelligent systems",
//! AI Magazine 17(3):73, <https://ojs.aaai.org/aimagazine/index.php/aimagazine/article/view/1232>:
//! composing anytime modules under a budget from their measured performance profiles. Here
//! the budget is fixed per mode from the profiles above rather than allocated at run time,
//! and counted in iterations, so the output is the same on every machine. See also:
//! M. Yang, H. Chao, C. Zhang, J. Guo, L. Yuan, J. Sun (2016), "Effective clipart image
//! vectorization through direct optimization of bezigons", IEEE TVCG 22(2),
//! <https://arxiv.org/abs/1602.01913>, a crude partition refined by optimisation against
//! the image, which is this mode's shape.
//!
//! Where it plugs in: `pipeline.rs` asks [`on`] twice, once to set the front end's fast
//! flag in the colour options and once to pick [`fit`] instead of the per-edge dynamic
//! program, and [`solve_iters`] once for the solve's cap; after emitting, it puts
//! [`report`] at the top of the stats. [`fast_ignored`] is re-exported from `lib.rs` so
//! Inkvec Studio (`studio/core/src/options.rs`) can check its own list of Quality-only
//! settings against it. In: the settled [`Args`], and for the fit the planar map and each
//! face's fill. Out: one fitted path per map edge, and a line of text for the report.

use crate::args::{Args, TraceMode};
use inkvec_fit::{primitives::PrimitiveFit, FittedPath};
use inkvec_trace::planar::PlanarMap;

/// Whether `args` ask for the Fast engine: `--mode fast` or `--mode balanced`.
pub(crate) fn on(args: &Args) -> bool {
    args.mode.fast_engine()
}

/// Longest side, in px of the traced raster, up to which `balanced` adds its stages to
/// Fast; above it, balanced is Fast. See the module docs for why.
pub(crate) const BALANCED_MAX_SIDE: usize = 1024;

/// The boundary solve's iteration cap in `balanced` mode, per independent part of the
/// boundary. r2-fastq's profile on the 128 px screen set: 2 iterations buy 50 % of the
/// 32-iteration solve's dE00 gain, 4 buy 81 %, 8 buy 89 %, 16 buy 93 %, while the stage's
/// cost is mostly fixed setup (2 iterations already cost 1.2-1.4x Fast at 128 px, 4 cost
/// 1.3-1.4x). Four is the knee.
pub(crate) const BALANCED_SOLVE_ITERS: usize = 4;

/// Whether `args` ask for balanced mode and a `width` x `height` raster is small enough
/// for it ([`BALANCED_MAX_SIDE`]): the one test behind both of balanced's additions, so
/// above the threshold the trace is Fast's in every respect.
pub(crate) fn balanced(args: &Args, width: usize, height: usize) -> bool {
    args.mode == TraceMode::Balanced && width.max(height) <= BALANCED_MAX_SIDE
}

/// The boundary solve's iteration cap for a `width` x `height` raster
/// (`inkvec_trace::ColorOptions::boundary_iters`): [`BALANCED_SOLVE_ITERS`] in balanced
/// mode up to its size threshold, `None` otherwise, which leaves Quality's solve uncapped
/// and Fast without one.
pub(crate) fn solve_iters(args: &Args, width: usize, height: usize) -> Option<usize> {
    balanced(args, width, height).then_some(BALANCED_SOLVE_ITERS)
}

/// The fast fitter's tolerances for a `width` x `height` raster: balanced's finer ones
/// where [`balanced`] holds, Fast's own otherwise.
fn fit_config(args: &Args, width: usize, height: usize) -> inkvec_trace::fast::FastFit {
    if balanced(args, width, height) {
        inkvec_trace::fast::FastFit::balanced()
    } else {
        inkvec_trace::fast::FastFit::default()
    }
}

/// Every boundary of the map, fitted by the fast fitter: a closed boundary that is a circle
/// or an ellipse as that primitive, everything else as lines and cubics. The tolerances are
/// [`fit_config`]'s for the map's size: balanced's finer ones where balanced applies.
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
    args: &Args,
    map: &PlanarMap,
    fills: &[inkvec_trace::gradient::FillFit],
) -> Vec<(FittedPath, Option<PrimitiveFit>)> {
    let cfg = fit_config(args, map.width, map.height);
    inkvec_trace::fast::fit_edges(&map.edges, fills, &cfg, map.width, map.height)
}

/// Every option in `args`, moved off its default, that fast mode's colour trace does not read:
/// each one only steers a stage fast mode skips. Balanced reads the same options as Fast:
/// its boundary solve is capped by iterations, never by `--time-budget`, so its output does
/// not depend on the machine either.
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

/// The report line fast or balanced mode writes for a `width` x `height` raster: what it
/// ran, and every option it was given that only steers a stage it skips. The ignored list
/// is [`fast_ignored`]'s, appended only when it is not empty, so a run on default settings
/// prints the fixed first half alone. Balanced above its size threshold says it ran Fast.
pub(crate) fn report(args: &Args, width: usize, height: usize) -> String {
    let ignored = fast_ignored(args);
    let mut line = if balanced(args, width, height) {
        format!(
            "balanced mode Potrace-class fit at 0.75x tolerances, flat fills, boundary solve \
             capped at {BALANCED_SOLVE_ITERS} iterations; not run: curve DP, gradients, ring \
             repair, harmonization"
        )
    } else if args.mode == TraceMode::Balanced {
        format!(
            "balanced mode over {BALANCED_MAX_SIDE} px, so fast mode's path: Potrace-class \
             fit, flat fills; not run: boundary solve, curve DP, gradients, ring repair, \
             harmonization"
        )
    } else {
        "fast mode     Potrace-class fit, flat fills; not run: boundary solve, curve DP, \
         gradients, ring repair, harmonization"
            .to_string()
    };
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
        let plain = report(&Args::default(), 128, 128);
        assert!(!plain.contains("ignored"), "{plain}");
        let tuned = report(
            &Args {
                tau: 3.0,
                bezier_cost: Some(4.0),
                ..Args::default()
            },
            128,
            128,
        );
        assert!(tuned.ends_with("ignored: --tau --bezier-cost"), "{tuned}");
    }

    /// Balanced adds its stages up to its size threshold and is Fast above it, and the
    /// report line says which.
    #[test]
    fn balanced_is_fast_above_its_size_threshold() {
        let b = Args {
            mode: TraceMode::Balanced,
            ..Args::default()
        };
        let f = Args {
            mode: TraceMode::Fast,
            ..Args::default()
        };
        let (at, over) = (BALANCED_MAX_SIDE, BALANCED_MAX_SIDE + 1);
        assert_eq!(solve_iters(&b, at, 16), Some(BALANCED_SOLVE_ITERS));
        assert_eq!(solve_iters(&b, 16, over), None);
        assert_eq!(solve_iters(&f, 16, 16), None);
        assert_eq!(solve_iters(&Args::default(), 16, 16), None);
        let fine = inkvec_trace::fast::FastFit::balanced();
        let plain = inkvec_trace::fast::FastFit::default();
        assert_eq!(format!("{:?}", fit_config(&b, at, at)), format!("{fine:?}"));
        assert_eq!(
            format!("{:?}", fit_config(&b, over, 8)),
            format!("{plain:?}")
        );
        assert_eq!(format!("{:?}", fit_config(&f, 8, 8)), format!("{plain:?}"));
        assert!(report(&b, at, at).starts_with("balanced mode Potrace-class"));
        assert!(report(&b, over, 8).contains("so fast mode's path"));
        assert!(report(&f, 8, 8).starts_with("fast mode "));
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
        let fits = fit(&Args::default(), &map, &fills);
        assert_eq!(fits.len(), map.edges.len());
        let square = fits
            .iter()
            .find(|(f, _)| f.closed && f.segments.len() == 4)
            .expect("the square's boundary is four lines");
        assert!(square.1.is_none());
    }
}
