//! The line-art pipeline (`--strokes`): a drawing made of strokes written as strokes --
//! one centreline and one width each, the way the artist drew them -- with whatever is not
//! a stroke traced and filled.
//!
//! Tried by the crate root before the ordinary pipelines, and declines (returns `None`)
//! when the drawing is not line art, so everything else is untouched. The centrelines come
//! from `inkvec_trace::centerline`, the fits from `inkvec_fit`, and the output is a complete
//! `<svg>` in the traced image's pixels (pixel centres at integers). See [`run_strokes`].

use crate::args::Args;
use crate::diag;
use crate::pathdata::{emit_decimals, fmt_fitted};
use inkvec_fit::{multimodel, FitConfig};
use inkvec_trace::{trace_bilevel, TraceOptions};

/// Emit recovered strokes as strokes: one path and one width, the way the artist
/// drew them.
///
/// 28% of the corpus is stroke art -- lucide 175/175, openmoji 143/148,
/// fluent-emoji 132/175 -- and converting it to filled outlines costs 3.29x the
/// artist's parameter count against 0.83-1.42 on every other family, because a
/// centreline restated as an outline needs both sides plus caps and joins. It
/// also destroys the one thing the artist most wants back: the line weight stops
/// being a number anyone can edit.
///
/// The pipeline: bilevel coverage of the image and its connected regions; the regions
/// that are strokes, as centrelines with widths (`inkvec_trace::centerline`), optionally
/// refined against the coverage; one shared width when they agree ([`shared_width`]);
/// whatever is not a stroke traced and filled ([`residual_fills`]); the centrelines fitted
/// and written ([`stroke_paths`]); then a per-stroke residual filter and an ink-balance
/// check. Round caps and joins, as the art this is for uses. Coordinates are px of the
/// traced raster.
///
/// Returns `None` when the drawing is not line art, so the ordinary path runs.
pub(crate) fn run_strokes(
    img: &inkvec_trace::Rgba,
    args: &Args,
    cfg: &FitConfig,
) -> Option<(String, Vec<String>)> {
    use inkvec_trace::centerline;

    inkvec_core::progress::begin("strokes");
    let cov = inkvec_trace::coverage::bilevel_coverage(img);
    let labels = centerline::bilevel_labels(&cov);
    let mut an = centerline::analyse(&cov, &labels, img.width, img.height);
    // No icon-level coverage gate: a drawing that is two fifths strokes should
    // get those two fifths as strokes and the rest as fills.
    if an.strokes.is_empty() {
        return None;
    }
    // Put the centrelines on the coverage they were measured from. Thinning and
    // the ridge fit are statements about the distance field; nothing so far has
    // compared the stroke to the picture it will render.
    if args.stroke_refine > 0 {
        centerline::refine_to_coverage(&mut an.strokes, &cov, args.stroke_refine);
    }

    let decimals = emit_decimals(args.precision);
    let (w, h) = (img.width, img.height);
    // Monochrome asks for pure black on white, and for no paper at all without a background
    // (the knock-out in `post_process` stands aside under monochrome; see there).
    let (ink, paper) = if args.monochrome {
        ("#000000".to_string(), "#ffffff".to_string())
    } else {
        (
            inkvec_trace::color::to_hex(cov.fg),
            inkvec_trace::color::to_hex(cov.bg),
        )
    };
    let paper_rect = if args.monochrome && args.no_background {
        String::new()
    } else {
        format!("<rect x=\"-0.5\" y=\"-0.5\" width=\"{w}\" height=\"{h}\" fill=\"{paper}\"/>")
    };

    let (med, shared) = shared_width(&an.strokes);
    let (fills, fill_params, residual_ink) =
        residual_fills(img, &cov, &labels, &an.strokes, args, cfg, &ink);
    let (body, mut params) = stroke_paths(&an.strokes, cfg, shared, decimals, fill_params);
    if body.is_empty() {
        return None;
    }

    // Does the drawing we are about to write contain as much ink as the one we
    // were given? `stroke_fraction` says what share of the *regions* came back as
    // strokes and it over-reports: on `bath` it reads 100% while two of the five
    // paths the artist drew -- both legs and the water line -- are missing, and
    // the rendered result carries 85% of the truth's ink for a dE00 of 3.76
    // against the filled path's 0.13. Every catastrophic icon looks like that and
    // no icon that balances does, so balance is the gate rather than coverage.
    //
    // Stroke ink is length times width, which over-counts where strokes meet and
    // under-counts the caps beyond each end; those roughly cancel and neither is
    // near the 15% that marks a miss, so the tolerance is set well outside them.
    // Does the union of the real outlines reproduce the coverage? It was meant to
    // replace the ink-balance proxy, and it does not work: measured across lucide
    // the residual sits at 0.17-0.30 for every icon, and the two ends of that
    // range are `at-sign` at 0.173, which traces badly, and `cat` at 0.300, which
    // traces well. It is dominated by the cap and join mismatch, which every icon
    // pays equally, so the thing that actually distinguishes them -- whether the
    // strokes are in the right places -- never surfaces above it. Left reachable
    // through `--stroke-residual` and defaulted past anything it would refuse.
    // Keep the strokes that fit and let the rest be filled, rather than deciding
    // for the whole drawing. A region is a stroke or it is not, independently of
    // its neighbours -- the tracer already chooses per region between a fitted
    // path and a primitive, and a stroke is a third way to say one region, not a
    // mode the whole file has to be in. Judged together, one bad stroke condemned
    // every good one with it.
    //
    // The intent is that anything refused here keeps its region out of the strokes, so it
    // survives into the residual image and is traced and filled like any other. The code
    // does not do that as it stands: the stroke paths and the residual image above were
    // both made from every stroke, before this filter, so a refused stroke is still
    // written as a stroke and its region is not filled. The filter only changes what
    // follows -- the ink balance, the kept count in the report, and declining the whole
    // drawing when nothing is kept.
    let before = an.strokes.len();
    an.strokes
        .retain(|st| centerline::stroke_residual_one(st, &cov, &labels) <= args.stroke_residual);
    let dropped = before - an.strokes.len();
    if an.strokes.is_empty() {
        return None;
    }

    // A last check that the drawing is whole: the kept strokes' ink (length times width,
    // px²) plus the residual ink pixels, against the input's total coverage. Per-region
    // selection means nothing can be dropped -- what the strokes refuse is filled -- so
    // this should never fire, and it is kept as the assertion of that rather than as a
    // policy. Outside `[stroke_balance, 2 - stroke_balance]` the drawing is declined.
    let ink_have: f64 = (0..w * h).map(|i| cov.data[i] as f64).sum();
    let ink_draw: f64 =
        an.strokes.iter().map(|s| s.length() * s.width).sum::<f64>() + residual_ink as f64;
    if ink_have > 1.0 {
        let bal = ink_draw / ink_have;
        if !(args.stroke_balance..=(2.0 - args.stroke_balance)).contains(&bal) {
            diag::stage(args.quiet, || {
                format!(
                    "  strokes       declined: would draw {:.0}% of the ink present",
                    bal * 100.0
                )
            });
            return None;
        }
    }
    if shared {
        params += 1.0;
    }

    // The shared width, cap and join go on a group, so the drawing states them
    // once. `round` is what this art uses: lucide sets both on every file.
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-0.5 -0.5 {w} {h}\" width=\"{w}\" height=\"{h}\">\
         {paper_rect}{fills}\
         <g fill=\"none\" stroke=\"{ink}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"{sw}>{body}</g></svg>",
        sw = if shared {
            format!(" stroke-width=\"{med:.*}\"", decimals)
        } else {
            String::new()
        }
    );
    Some((
        svg,
        vec![
            format!(
                "strokes       {} kept, {:.0}% of the ink, width {:.2}{}",
                an.strokes.len(),
                an.stroke_fraction * 100.0,
                med,
                if shared { " (shared)" } else { " (per stroke)" }
            ),
            format!(
                "              {dropped} refused, {} region(s) filled, {residual_ink} ink px",
                an.residual_regions.len()
            ),
            format!("params        {params:.0}"),
        ],
    ))
}

/// The width to write for every stroke at once, and whether one width will do: the
/// median of the strokes' widths (px), shared when the spread between the thinnest and
/// the thickest is within four times the largest width uncertainty (at least 0.02 px).
/// `strokes` must not be empty.
fn shared_width(strokes: &[inkvec_trace::centerline::Stroke]) -> (f64, bool) {
    // One width for the whole drawing when the strokes agree on it. The artist
    // wrote one number; recovering seven that differ in the third decimal and
    // emitting all seven would be restating measurement noise as content. They
    // agree when the spread is inside the width uncertainty the module reports.
    let mut widths: Vec<f64> = strokes.iter().map(|s| s.width).collect();
    widths.sort_by(f64::total_cmp);
    let med = widths[widths.len() / 2];
    let spread = widths[widths.len() - 1] - widths[0];
    let tol = strokes
        .iter()
        .map(|s| s.width_sigma)
        .fold(0.0f64, f64::max)
        .max(0.02)
        * 4.0;
    let shared = spread <= tol;

    (med, shared)
}

/// What is left of the drawing once the strokes are taken out, traced and filled: the
/// `<path>` elements in `ink`, their parameter count, and how many ink pixels were left.
///
/// Every pixel of a stroke's region is painted the paper colour, and the image that remains
/// is traced by the bilevel tracer and fitted with the full line/cubic/arc alphabet. Fewer
/// than eight leftover ink pixels are not traced.
fn residual_fills(
    img: &inkvec_trace::Rgba,
    cov: &inkvec_trace::CoverageField,
    labels: &[u16],
    strokes: &[inkvec_trace::centerline::Stroke],
    args: &Args,
    cfg: &FitConfig,
    ink: &str,
) -> (String, f64, usize) {
    let (w, h) = (img.width, img.height);
    let decimals = emit_decimals(args.precision);
    // Whatever was not recovered as a stroke still has to be drawn. Blanking the
    // stroke regions and tracing what is left reuses the ordinary bilevel path
    // rather than inventing a second one, and it is what makes this a
    // representation change instead of a lossy one: the first version dropped
    // three regions on `bell` and took dE00 from 0.149 to 2.224 while looking
    // like a parameter win.
    let stroke_labels: std::collections::HashSet<u16> = strokes.iter().map(|s| s.label).collect();
    let mut residual = img.clone();
    let mut residual_ink = 0usize;
    for i in 0..w * h {
        if stroke_labels.contains(&labels[i]) {
            for c in 0..3 {
                residual.data[i * 4 + c] = cov.bg[c];
            }
            residual.data[i * 4 + 3] = 1.0;
        } else if cov.data[i] >= 0.5 {
            // Ink, and not a stroke. `bilevel_labels` numbers every component
            // including the holes inside a shape, so "label is not zero" counts
            // interior background as ink and reported 4070 px on a bell whose
            // strokes already covered 91% of it.
            residual_ink += 1;
        }
    }
    let mut fills = String::new();
    let mut fill_params = 0.0f64;
    if residual_ink >= 8 {
        let (contours, _) = trace_bilevel(
            &residual,
            &TraceOptions {
                min_area: args.min_area,
            },
        );
        for c in &contours {
            // The same fitter the ordinary path uses. It used to be `optimal_polygon`,
            // which is lines only, and that is not a smaller alphabet so much as a worse
            // one: what the strokes hand back is the curved leftovers — the clapper of a
            // bell, a bracket end — and a polygon spends a vertex every time the true
            // boundary turns. On `bell` the clapper came out as 20 line segments, 40
            // numbers, where the artist wrote one arc and nine. That cost is charged to
            // the stroke representation, which is then compared against a filled outline
            // fitted with the full alphabet, and loses on a handicap it was given rather
            // than one it earned.
            let fitted = multimodel::optimal_multimodel(c, cfg);
            if fitted.segments.is_empty() {
                continue;
            }
            let mut d = String::new();
            fmt_fitted(&fitted, true, decimals, &mut d);
            if d.is_empty() {
                continue;
            }
            fill_params += fitted.params();
            fills.push_str(&format!("<path d=\"{d}\" fill=\"{ink}\"/>"));
        }
    }

    (fills, fill_params, residual_ink)
}

/// Each stroke's centreline as an unfilled `<path>`, carrying its own `stroke-width` unless
/// the width is `shared`. Returns the elements and `params` plus the parameters they spend:
/// each path's own, and one more per stroke for its width when that is not shared.
fn stroke_paths(
    strokes: &[inkvec_trace::centerline::Stroke],
    cfg: &FitConfig,
    shared: bool,
    decimals: usize,
    params: f64,
) -> (String, f64) {
    let mut body = String::new();
    let mut params = params;
    for st in strokes {
        let fitted = st.fit(cfg);
        if fitted.segments.is_empty() {
            continue;
        }
        let mut d = String::new();
        fmt_fitted(&fitted, st.closed, decimals, &mut d);
        params += fitted.params() + if shared { 0.0 } else { 1.0 };
        body.push_str("<path d=\"");
        body.push_str(&d);
        body.push_str("\" fill=\"none\"");
        if !shared {
            body.push_str(&format!(" stroke-width=\"{:.*}\"", decimals, st.width));
        }
        body.push_str("/>");
    }
    (body, params)
}
