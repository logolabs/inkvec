//! Tracing pipelines for strokes, bilevel, and full-colour inputs.

use super::*;

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
/// Returns `None` when the drawing is not line art, so the ordinary path runs.
pub(crate) fn run_strokes(
    img: &inkvec_trace::Rgba,
    args: &Args,
    cfg: &FitConfig,
) -> Option<(String, Vec<String>)> {
    use inkvec_trace::centerline;

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
    let ink = inkvec_trace::color::to_hex(cov.fg);
    let paper = inkvec_trace::color::to_hex(cov.bg);

    // One width for the whole drawing when the strokes agree on it. The artist
    // wrote one number; recovering seven that differ in the third decimal and
    // emitting all seven would be restating measurement noise as content. They
    // agree when the spread is inside the width uncertainty the module reports.
    let mut widths: Vec<f64> = an.strokes.iter().map(|s| s.width).collect();
    widths.sort_by(f64::total_cmp);
    let med = widths[widths.len() / 2];
    let spread = widths[widths.len() - 1] - widths[0];
    let tol = an
        .strokes
        .iter()
        .map(|s| s.width_sigma)
        .fold(0.0f64, f64::max)
        .max(0.02)
        * 4.0;
    let shared = spread <= tol;

    // Whatever was not recovered as a stroke still has to be drawn. Blanking the
    // stroke regions and tracing what is left reuses the ordinary bilevel path
    // rather than inventing a second one, and it is what makes this a
    // representation change instead of a lossy one: the first version dropped
    // three regions on `bell` and took dE00 from 0.149 to 2.224 while looking
    // like a parameter win.
    let stroke_labels: std::collections::HashSet<u16> =
        an.strokes.iter().map(|s| s.label).collect();
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

    let mut body = String::new();
    let mut params = fill_params;
    for st in &an.strokes {
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
    // Anything refused here keeps its label out of `stroke_labels` below, so its
    // region survives into the residual image and is traced and filled like any
    // other. Nothing can go missing either way.
    let before = an.strokes.len();
    an.strokes
        .retain(|st| centerline::stroke_residual_one(st, &cov, &labels) <= args.stroke_residual);
    let dropped = before - an.strokes.len();
    if an.strokes.is_empty() {
        return None;
    }

    // A last check that the drawing is whole. Per-region selection means nothing
    // can be dropped -- what the strokes refuse is filled -- so this should never
    // fire, and it is kept as the assertion of that rather than as a policy.
    let ink_have: f64 = (0..w * h).map(|i| cov.data[i] as f64).sum();
    let ink_draw: f64 =
        an.strokes.iter().map(|s| s.length() * s.width).sum::<f64>() + residual_ink as f64;
    if ink_have > 1.0 {
        let bal = ink_draw / ink_have;
        if !(args.stroke_balance..=(2.0 - args.stroke_balance)).contains(&bal) {
            if !args.quiet {
                eprintln!(
                    "  strokes       declined: would draw {:.0}% of the ink present",
                    bal * 100.0
                );
            }
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
         <rect x=\"-0.5\" y=\"-0.5\" width=\"{w}\" height=\"{h}\" fill=\"{paper}\"/>{fills}\
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

pub(crate) fn run_bilevel(
    img: &inkvec_trace::Rgba,
    args: &Args,
    cfg: &FitConfig,
) -> (String, Vec<String>) {
    let (w, h) = (img.width, img.height);
    let (contours, field) = trace_bilevel(
        img,
        &TraceOptions {
            // Deliberately NOT in content units. `min_area` is a speckle floor: it removes
            // what noise makes, and noise does not scale with the raster. Scaled by s² it
            // was 32 px² at 512 and swallowed the 1-2 px gaps between a scribble's strokes
            // (abra_agency: tile error 602 -> 371 with the floor back in pixels; 60 real
            // brands at 512: dE00 worst 1.33 -> 0.96, p99 0.73 -> 0.58, params 0.69x -> 0.92x
            // of the artist's, 2026-09-05). The fit tolerances stay in content units.
            min_area: args.min_area,
        },
    );
    let measured: usize = contours.iter().map(|c| c.len()).sum();
    let s_content = content_scale(img, args);
    let contours: Vec<inkvec_core::Polyline> = contours
        .iter()
        .map(|c| in_content_units(c, s_content))
        .collect();
    let segs: Vec<Segmentation> = contours.iter().map(|c| optimal_polygon(c, cfg)).collect();
    let anchors: usize = segs.iter().map(|s| s.segment_count()).sum();

    let paths: Vec<Vec<Point>> = contours
        .iter()
        .zip(&segs)
        .map(|(c, s)| {
            let max_shift = 4.0 * c.sigma.iter().copied().fold(0.0, f64::max).max(0.25);
            adjust_vertices(c, s, max_shift)
        })
        .collect();

    let svg = emit_bilevel(&paths, w, h, args.precision);
    (
        svg,
        vec![
            format!(
                "coverage      sigma_alpha {:.5}   resolvedness {:.2}",
                field.sigma_alpha, field.saturation
            ),
            format!("contours      {}", contours.len()),
            format!(
                "anchors       {anchors}  (from {measured} measured points, {:.1}x reduction)",
                measured as f64 / anchors.max(1) as f64
            ),
        ],
    )
}

pub(crate) fn run_color(
    img: &inkvec_trace::Rgba,
    args: &Args,
    cfg: &FitConfig,
    alpha_src: Option<&AlphaSource>,
) -> Result<(String, Vec<String>), Stop> {
    run_color_impl(img, args, cfg, alpha_src, |_| {})
}

/// Research-only access to shared geometry between raster tracing and fitting.
/// The caller must preserve edge incidence, junction endpoints, and uncertainty.
#[cfg(feature = "research-guidance")]
pub fn trace_color_guided(
    img: &inkvec_trace::Rgba,
    args: &Args,
    guide: impl FnOnce(&mut inkvec_trace::ColorTrace),
) -> Result<(String, Vec<String>), Stop> {
    run_color_impl(img, args, &fit_config(img, args), None, guide)
}

/// The tracer options the colour path runs with, as the command line asks for them.
///
/// Extracted so that every entry into the colour tracer -- the shipped one and the
/// research ones below -- hands it the same options, rather than a hand-copied
/// approximation that drifts the first time a flag is added.
pub(crate) fn color_options(args: &Args) -> ColorOptions {
    let (deadline, boundary_ms) = if args.time_budget > 0.0 {
        (
            Some(
                inkvec_core::clock::Instant::now()
                    + std::time::Duration::from_secs_f64(args.time_budget * 0.6),
            ),
            Some((args.time_budget * 0.25 * 1000.0).max(50.0) as u64),
        )
    } else {
        (None, None)
    };
    ColorOptions {
        merge_distance: args.merge_distance,
        max_colors: args.max_colors,
        // `Auto` should already have been resolved by `resolve_lossy`; if a caller skipped
        // that, treating it as "not known to be lossy" keeps a clean intake untouched.
        lossy_intake: args.lossy == inkvec_sr::Mode::On,
        alpha_inks: args.cutout,
        native_alpha: args.native_alpha,
        simplify_faint: args.simplify_faint,
        deadline,
        boundary_ms,
        // A 2px floor is appropriate for the continuous bilevel path, but in colour
        // mode it lets anti-aliased edge samples become their own tiny faces. Those faces
        // split a smooth shared boundary into a literal pixel staircase before fitting.
        // Nine pixels removes that confetti on the grid canaries while long thin artwork
        // remains intact; an explicit --min-area always wins for deliberately tiny art.
        // A 9-pixel colour-mode floor was tried (2026-09-03) to absorb anti-aliased
        // stair-step faces and measured worse on the full 980-icon set: objective
        // 0.7885 vs 0.7673 with the 2 px floor, 508 icons worse / 171 better in dE00 -
        // it erased real dots and thin details more often than it removed confetti.
        // Blend absorption and residual carving handle the stair-steps at the label
        // level instead. The default therefore stays at the --min-area value (2 px).
        // Area, so the square of the content scale.
        min_region: args.min_area.max(1.0) as usize,
        gradients: !args.no_gradients,
    }
}

/// Research-only: trace from a label map the caller produced, e.g. a network's.
///
/// The palette and labelling stages are replaced wholesale by `labels`; everything from
/// the planar map onwards, and the whole of the fit/repair/emit path below, is the
/// shipped one. See [`inkvec_trace::trace_color_from_labels`].
#[cfg(feature = "research-guidance")]
pub fn trace_color_from_labels_guided(
    img: &inkvec_trace::Rgba,
    args: &Args,
    labels: &[u16],
    n_labels: usize,
    guide: impl FnOnce(&mut inkvec_trace::ColorTrace),
) -> Result<(String, Vec<String>), Stop> {
    let opts = color_options(args);
    let mut sw = inkvec_trace::Stopwatch::start();
    let mut traced = inkvec_trace::trace_color_from_labels(img, &opts, labels, n_labels);
    guide(&mut traced);
    sw.mark("trace_total");
    finish_color(img, args, &fit_config(img, args), None, traced)
}

pub(crate) fn run_color_impl(
    img: &inkvec_trace::Rgba,
    args: &Args,
    cfg: &FitConfig,
    alpha_src: Option<&AlphaSource>,
    guide: impl FnOnce(&mut inkvec_trace::ColorTrace),
) -> Result<(String, Vec<String>), Stop> {
    let opts = color_options(args);
    let mut sw = inkvec_trace::Stopwatch::start();
    // Colour groups recolour the image, and everything after -- the trace and the fit that
    // checks itself against the pixels -- sees the recoloured one. See `regroup`.
    let regrouped;
    let (img, merge_report) = if args.merge_colors.is_empty() {
        (img, Vec::new())
    } else {
        // The groups name fills the caller saw in a trace, so they are found in one.
        let first = inkvec_trace::trace_color_full_with_alpha(
            img,
            &opts,
            alpha_src.map(|a| a.alpha.as_slice()),
        );
        let (out, outcomes) = regroup::apply(img, &first, &args.merge_colors);
        sw.mark("merge_colors");
        regrouped = out;
        (&regrouped, merge_lines(&outcomes))
    };
    let mut traced = inkvec_trace::trace_color_full_with_alpha(
        img,
        &opts,
        alpha_src.map(|a| a.alpha.as_slice()),
    );
    guide(&mut traced);
    sw.mark("trace_total");
    let (svg, mut report) = finish_color(img, args, cfg, alpha_src, traced)?;
    report.splice(0..0, merge_report);
    Ok((svg, report))
}

/// One report line per colour group: what merged into what, and what matched nothing.
fn merge_lines(outcomes: &[regroup::GroupOutcome]) -> Vec<String> {
    let hex = |c: [f32; 3]| {
        let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
    };
    outcomes
        .iter()
        .map(|o| {
            let mut line = match (o.target, o.gradient) {
                (Some(t), _) => {
                    let fills: Vec<String> = o.merged.iter().map(|&c| hex(c)).collect();
                    format!("merge colors  {} -> {}", fills.join(" + "), hex(t))
                }
                (None, true) => {
                    let fills: Vec<String> = o.merged.iter().map(|&c| hex(c)).collect();
                    format!("merge colors  {} -> one gradient", fills.join(" + "))
                }
                (None, false) => {
                    "merge colors  group left alone (fewer than two fills found)".to_string()
                }
            };
            if !o.unmatched.is_empty() {
                let missing: Vec<String> = o
                    .unmatched
                    .iter()
                    .map(|m| match m {
                        regroup::Member::Flat(c) => hex(*c),
                        regroup::Member::Gradient(s) => {
                            s.iter().map(|&c| hex(c)).collect::<Vec<_>>().join(">")
                        }
                    })
                    .collect();
                line.push_str(&format!("; not found in the trace: {}", missing.join(", ")));
            }
            line
        })
        .collect()
}

/// Why the colour pipeline stopped before producing an SVG.
///
/// Returned instead of exiting the process, so a library caller (the WASM build, the desk
/// app, a research harness) keeps control. Only the command line turns it into an exit code.
#[derive(Debug)]
pub enum Stop {
    /// The image is a single flat colour and `Args::strict` asked for that to be an error.
    FlatInput,
    /// `INKVEC_DUMP_MAP` asked for the planar map to be written instead of a trace, and it
    /// was written to this path.
    MapDumped(std::path::PathBuf),
    /// `INKVEC_DUMP_MAP` asked for the planar map, and writing it failed.
    MapDumpFailed(String),
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stop::FlatInput => {
                write!(
                    f,
                    "nothing to trace: the image is a single flat colour (--strict)"
                )
            }
            Stop::MapDumped(p) => write!(f, "planar map written to {}", p.display()),
            Stop::MapDumpFailed(e) => write!(f, "could not write the planar map dump: {e}"),
        }
    }
}

impl std::error::Error for Stop {}

/// Everything after the trace: fit each boundary, repair crossings, choose fills, and
/// emit. It reads nothing but the `ColorTrace` and the raster, which is what lets a
/// research entry swap the tracer out and keep the rest of the pipeline intact.
fn finish_color(
    img: &inkvec_trace::Rgba,
    args: &Args,
    cfg: &FitConfig,
    alpha_src: Option<&AlphaSource>,
    traced: inkvec_trace::ColorTrace,
) -> Result<(String, Vec<String>), Stop> {
    let (w, h) = (img.width, img.height);
    // Label generation needs the planar map and nothing after it: `INKVEC_DUMP_MAP=<path>`
    // writes it in a compact binary form and stops before the curve fit, which is half the
    // run. Format (little endian): u32 w, h, n_labels, n_edges; then per edge u32 left,
    // right, u8 closed, u32 n, then n x (f32 x, f32 y, f32 sigma). Same intake as a trace.
    if let Some(path) = inkvec_core::env::path("INKVEC_DUMP_MAP") {
        let mut buf: Vec<u8> = Vec::new();
        let m = &traced.map;
        for v in [w as u32, h as u32, m.n_labels as u32, m.edges.len() as u32] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for e in &m.edges {
            buf.extend_from_slice(&(e.left as u32).to_le_bytes());
            buf.extend_from_slice(&(e.right as u32).to_le_bytes());
            buf.push(u8::from(e.closed));
            buf.extend_from_slice(&(e.points.len() as u32).to_le_bytes());
            for (k, p) in e.points.iter().enumerate() {
                let s = e.sigma.get(k).copied().unwrap_or(0.1);
                buf.extend_from_slice(&(p.x as f32).to_le_bytes());
                buf.extend_from_slice(&(p.y as f32).to_le_bytes());
                buf.extend_from_slice(&(s as f32).to_le_bytes());
            }
        }
        return match std::fs::write(&path, buf) {
            Ok(()) => Err(Stop::MapDumped(std::path::PathBuf::from(path))),
            Err(e) => Err(Stop::MapDumpFailed(e.to_string())),
        };
    }
    if traced.palette.len() <= 1 {
        eprintln!("warning: nothing to trace -- the image is a single flat colour");
        if args.strict {
            return Err(Stop::FlatInput);
        }
    }
    let mut sw = inkvec_trace::Stopwatch::start();
    let traced_labels = traced.labels.clone();
    let boundary_report = traced.boundary_opt;
    let symmetry = traced.symmetry;
    let symmetrised = traced.symmetrised;
    let face_fade = traced.face_fade;
    let (map, pal, face_color, face_fill) = (
        traced.map,
        traced.palette,
        traced.face_color,
        traced.face_fill,
    );

    // Each boundary is fitted exactly once, with a mixed line/cubic alphabet chosen by
    // MDL. Both faces that touch it then reference the same fitted curve — that is what
    // makes seams unrepresentable rather than merely rare.
    let measured: usize = map.edges.iter().map(|e| e.points.len()).sum();
    // One global dynamic program per boundary, with lines and cubics in the same
    // alphabet. This replaces the earlier two-pass fitter (line DP for corners, then a
    // swept kurbo fit per run), which was both slower — a 96-point smoothing/tolerance
    // sweep per run, enough to time out on complex inputs — and, on smooth lobed shapes,
    // catastrophically wrong: it reached 20px of deviation on a star where this reaches
    // 0.09px.
    let s_content = content_scale(img, args);
    let polys: Vec<inkvec_core::Polyline> = map
        .edges
        .iter()
        .map(|e| in_content_units(&e.as_polyline(), s_content))
        .collect();
    // What a parameter costs on *this* boundary. `lambda` is one exchange rate for the
    // whole drawing, which prices a boundary the artist lavished detail on exactly like a
    // plain straight run; the per-edge scale is where a predictor of local parameter
    // density is allowed to disagree with that average, and the global one is the leeway
    // over the drawing as a whole. Both are 1.0 unless something set them, and at 1.0
    // `cfg_k` is `cfg`, so the fit is unchanged.
    let lambda_scales: Vec<f64> = map
        .edges
        .iter()
        .map(|e| e.lambda_scale * args.lambda_scale)
        .collect();
    let cfg_repair = FitConfig {
        lambda: cfg.lambda * args.lambda_scale,
        ..*cfg
    };
    // Every boundary is fitted independently, so fit them on every core. The work per
    // edge varies by orders of magnitude (a two-point sliver against a thousand-point
    // outline), which is exactly the shape of problem rayon's work stealing handles.
    use rayon::prelude::*;
    let ring_timing = inkvec_core::env::flag("INKVEC_TIMING");
    let ring_times: std::sync::Mutex<Vec<(f64, usize)>> = std::sync::Mutex::new(Vec::new());
    let results: Vec<(FittedPath, Option<PrimitiveFit>)> = polys
        .par_iter()
        .zip(lambda_scales.par_iter())
        .map(|(poly, &scale)| {
            let poly = poly.clone();
            // This boundary's own exchange rate. `cfg_k == *cfg` when the scale is 1.0,
            // which is every path but the guided one.
            let cfg_k = FitConfig {
                lambda: cfg.lambda * scale,
                ..*cfg
            };
            let t_ring = inkvec_core::clock::Instant::now();
            let curve = multimodel::optimal_multimodel(&poly, &cfg_k);
            if ring_timing {
                ring_times
                    .lock()
                    .unwrap()
                    .push((t_ring.elapsed().as_secs_f64() * 1e3, poly.points.len()));
            }

            // A boundary that *is* a circle should be described as one. Both candidates
            // are scored by the same MDL cost, so the three numbers of a circle beat the
            // twenty-four of four cubics whenever the evidence actually supports a
            // circle, and lose when it does not.
            // It is worth a great deal: measured by taking this whole-boundary primitive
            // path out (the `INKVEC_NO_PRIMITIVE` ablation, since removed) over the
            // 246-icon gate set, removing it costs 30.52% of the parameter ratio
            // (1.4818 -> 1.9341) and 9.01% of dE00, far more than any other lever
            // measured on this tree.
            let attempt = fit_primitive_or_arcs(&poly.points, &poly.sigma, poly.closed, &cfg_k);
            match attempt {
                Some((segs, prim, cost)) if cost < path_cost(&poly, &curve, &cfg_k) => (
                    FittedPath {
                        start: poly.points[0],
                        segments: segs,
                        closed: poly.closed,
                    },
                    prim,
                ),
                _ => (curve, None),
            }
        })
        .collect();
    let mut prims: Vec<Option<PrimitiveFit>> = Vec::with_capacity(results.len());
    let mut fitted: Vec<FittedPath> = Vec::with_capacity(results.len());
    for (f, pr) in results {
        fitted.push(f);
        prims.push(pr);
    }
    // A local merge can trigger a more expensive global crossing repair. Keep
    // an independent baseline through primitive selection and repair so the
    // trial is judged on the geometry that those stages actually return.
    // The structural simplifier's transactional baseline: research builds only.
    let mut structural_baseline =
        if cfg!(feature = "research") && inkvec_core::env::flag("INKVEC_STRUCTURAL") {
            let results: Vec<_> = polys
                .par_iter()
                .zip(lambda_scales.par_iter())
                .map(|(poly, &scale)| {
                    let cfg_k = FitConfig {
                        lambda: cfg.lambda * scale,
                        ..*cfg
                    };
                    let curve = multimodel::optimal_multimodel_without_structural(poly, &cfg_k);
                    match fit_primitive_or_arcs(&poly.points, &poly.sigma, poly.closed, &cfg_k) {
                        Some((segs, prim, cost)) if cost < path_cost(poly, &curve, &cfg_k) => (
                            FittedPath {
                                start: poly.points[0],
                                segments: segs,
                                closed: poly.closed,
                            },
                            prim,
                        ),
                        _ => (curve, None),
                    }
                })
                .collect();
            let (paths, primitives): (Vec<_>, Vec<_>) = results.into_iter().unzip();
            Some((paths, primitives))
        } else {
            None
        };
    sw.mark("fit_dp");
    if ring_timing {
        let mut rt = ring_times.into_inner().unwrap();
        rt.sort_by(|a, b| b.0.total_cmp(&a.0));
        let total: f64 = rt.iter().map(|r| r.0).sum();
        let top: Vec<String> = rt
            .iter()
            .take(5)
            .map(|(ms, n)| format!("{ms:.0} ms/{n} pts"))
            .collect();
        eprintln!(
            "  [t] fit_dp rings: {} rings, {:.0} ms of ring work, top {}",
            rt.len(),
            total,
            top.join(", ")
        );
    }

    // Rings before polish, so a boundary refitted by the repair below is polished
    // afterwards rather than losing it. The traversal depends only on the map.
    let order = planar::face_edge_order(&map);

    /// The faces a layer covers, merged into the ground beneath them.
    ///
    /// A layer cuts every shape it crosses into pieces, and painting those pieces back is
    /// what makes the layer form *bigger* than the flat one: the cut is drawn twice, once
    /// on each side, and neither drawing means anything once the layer is composited over
    /// them. Relabelling the pieces to a single face makes the cut interior, and an
    /// interior edge belongs to no ring — so it stops being written at all, and the
    /// geometry is untouched otherwise. The boundary points, their fits and their sigmas
    /// are per *edge* and are not disturbed by any of this.
    fn merge_map(map: &planar::PlanarMap, remap: &[u16]) -> planar::PlanarMap {
        let mut out = map.clone();
        for e in out.edges.iter_mut() {
            let l = remap.get(e.left as usize).copied().unwrap_or(e.left);
            let r = remap.get(e.right as usize).copied().unwrap_or(e.right);
            if l == r {
                // Interior now: `face_edge_order` skips a label out of range, which is how
                // an edge leaves every ring without disturbing the indices the fits use.
                e.left = u16::MAX;
                e.right = u16::MAX;
            } else {
                e.left = l;
                e.right = r;
            }
        }
        out
    }

    // Removed: two stages that refined curves against the image after the fit, and a
    // third switch that turned one of them off.
    //
    // `polish` nudged each cubic's control points against the coverage field; the
    // adjudication stage re-scored chord runs against the raster and upgraded the ones
    // the image said were bending. Both were measured wins when they were written, and
    // both are now harmful, because `boundary_opt::optimise` solves the whole boundary
    // against that same image before the fit ever runs. Refining locally afterwards
    // partly undoes a better global answer.
    //
    // Ablated on 246 icons, then confirmed on all 980 (paired, 2026-09-03): removing them
    // takes the objective from 0.5477 to 0.4960 and dE00 from 0.2503 to 0.2146, with 799
    // icons better and 86 worse, and every family improving. Polish alone made 197 of 246
    // better by not running. They cost 580 lines and eleven per cent of the objective.
    //
    // What went with them: `inkvec_fit::polish`, `inkvec_fit::smooth`, `--no-polish`, and
    // the INKVEC_IMGADJ / ADJ_SIGMA / ADJ_MARGIN / ADJ_DEVCAP knobs. The lesson worth
    // keeping is the one that justified the adjudication in the first place: the dynamic
    // program cannot tell a gently curving boundary from a chain of chords using the
    // extracted points alone, because the extraction error is *correlated along* the
    // boundary. The answer to that is to put the question to the image - which is now
    // what the boundary solve does, for every point at once rather than per run.

    // Removed: a joint image-evidence solve for shared junctions, off by default and
    // deleted with its two modules (`inkvec_fit::junctions`, `inkvec_trace::render`).
    // A junction is one topological point shared by all its incident boundaries, and
    // solving it from all of them at once instead of letting one edge's local fit decide
    // is the right idea — but `boundary_opt::optimise` above now does that job for the
    // whole boundary, not just its nodes, and does it by default. The node-only solve
    // measured 0.0002 on the objective over 980 icons (0.7255 against 0.7253), which is
    // not a difference. Two lessons from it are worth keeping. Arcs had to be frozen
    // during the solve because their radius is a coupled variable that a two-coordinate
    // node solve cannot see. And a move proposed from local normal evidence had to be
    // judged by a full-scene render before it was accepted: at dense junctions a move
    // can lower every incident one-dimensional residual and still make a neighbouring
    // face's raster coverage worse.

    // A self-crossing boundary is invisible to the objective — both curves pass through
    // their measured points and the render barely changes — but the ring it produces is
    // invalid and unpleasant to edit. Measured over 180 real emoji, 53 (29%) emitted at
    // least one, against VTracer's 10 (5.6%), and classifying 2126 rings showed *none*
    // were a single cubic looping: every one is two different edges of a face crossing
    // after each was fitted within its own tolerance.
    //
    // This runs *after* polish, not before. Polish moves control points by up to a pixel,
    // and a thin neck is exactly where that reopens a crossing the repair had closed —
    // measured over 40 emoji, repairing first left 12 invalid rings where repairing last
    // leaves 9. The repair has to see the geometry that is actually emitted.
    let mut repaired = if args.no_repair {
        0
    } else {
        repair_ring_crossings(&order, &mut fitted, &polys, &cfg_repair)
    };
    if let Some((mut baseline, baseline_prims)) = structural_baseline.take() {
        let baseline_repaired = if args.no_repair {
            0
        } else {
            repair_ring_crossings(&order, &mut baseline, &polys, &cfg_repair)
        };
        if !structural_trial_improves(&polys, &fitted, &baseline, &lambda_scales, cfg) {
            fitted = baseline;
            prims = baseline_prims;
            repaired = baseline_repaired;
        }
    }
    sw.mark("repair");

    let n_cubic: usize = fitted
        .iter()
        .flat_map(|f| f.segments.iter())
        .filter(|s| matches!(s, Segment::Cubic(..)))
        .count();
    let n_line: usize = fitted
        .iter()
        .flat_map(|f| f.segments.iter())
        .filter(|s| matches!(s, Segment::Line(..)))
        .count();

    // Per-face fill model. A region shaded by a gradient is one region, not a stack of
    // flat bands: fitting it as a gradient replaces a pile of near-duplicate faces with
    // four numbers and two stops, and the same MDL objective decides whether the evidence
    // supports that.
    // Every stage above is free to break a mirror by breaking a tie, and the fitter breaks
    // them most of all: a dynamic program walks one boundary forwards and its reflection
    // backwards, and can segment them differently. So the fits are made to agree here,
    // after everyone else has finished: one boundary of each mirrored pair keeps its own
    // fit, and its partner takes that fit reflected. Exact by construction, and no search.
    let mut mirrored_fits = 0usize;
    if !symmetry.is_empty() {
        for k in 0..fitted.len() {
            // A shape that is its own reflection keeps its fit; where that fit is a
            // primitive, centring it on the axis makes it symmetric exactly.
            for m in symmetry.self_mirrors(k) {
                if let Some(pf) = prims.get(k).and_then(|p| p.as_ref()) {
                    let centred = inkvec_trace::symmetry::centre_primitive(pf, m);
                    prims[k] = Some(centred);
                    mirrored_fits += 1;
                }
            }
            let Some((m, pr)) = symmetry.mirror_of(k) else {
                continue;
            };
            if pr.edge >= fitted.len() {
                continue;
            }
            let mut path = inkvec_trace::symmetry::reflect_path(&fitted[pr.edge], m);
            if pr.rev {
                path = path.reversed();
            }
            fitted[k] = path;
            prims[k] = prims[pr.edge]
                .as_ref()
                .map(|pf| inkvec_trace::symmetry::reflect_primitive(pf, m));
            mirrored_fits += 1;
        }
    }

    let fills: Vec<gradient::FillFit> = face_fill
        .into_iter()
        .enumerate()
        .map(|(fi, f)| {
            if args.no_gradients {
                let c = face_color
                    .get(fi)
                    .and_then(|&ci| pal.rgb.get(ci))
                    .copied()
                    .unwrap_or([0.0, 0.0, 0.0]);
                gradient::FillFit {
                    model: gradient::FillModel::Flat(c),
                    ..f
                }
            } else {
                demote_imperceptible_gradient(f)
            }
        })
        .collect();
    let n_grad = fills
        .iter()
        .filter(|f| !matches!(f.model, gradient::FillModel::Flat(_)))
        .count();
    sw.mark("fills");

    let layers = alpha::recover_layers(args, &map, &face_color, &fills, &pal, &traced_labels);

    let FaceAlpha {
        mut clear,
        mut opacity,
        matte,
        alpha_ramps,
    } = alpha::face_alpha(img, args, alpha_src, &face_color, &traced_labels, w, h);
    // Traced natively, a face's opacity is its ink's: the palette found the ink *as* a
    // colour at an opacity, so a band of a fade is one opacity by construction, where the
    // flatness test above would call it varying and bake it opaque. The ramp fit stays for
    // the faces the palette did find opaque.
    if args.native_alpha && alpha_src.is_some() {
        for (f, &ink) in face_color.iter().enumerate() {
            let a = pal.alpha.get(ink).copied().unwrap_or(1.0);
            if let Some(c) = clear.get_mut(f) {
                *c = a < 0.05;
            }
            if let Some(o) = opacity.get_mut(f) {
                *o = if a < 0.05 || a > 0.98 { 1.0 } else { a };
            }
        }
        // A fade is translucent throughout, so it is punched out of whatever paints under
        // it exactly as a wash is; its opacity is written by its gradient's stops, and the
        // number here only has to say "below one".
        for (f, fade) in face_fade.iter().enumerate() {
            if fade.is_some() {
                if let Some(c) = clear.get_mut(f) {
                    *c = false;
                }
                if let Some(o) = opacity.get_mut(f) {
                    *o = 0.5;
                }
            }
        }
    }
    let fades: Vec<Option<(gradient::FillModel, gradient::FillModel)>> = (0..face_color.len())
        .map(|f| {
            face_fade
                .get(f)
                .cloned()
                .flatten()
                .map(|fd| (fd.alpha, fd.color))
        })
        .collect();

    // Both forms of the document, costed against each other.
    //
    // A layer is only worth having if it says the same thing in fewer marks. It repaints
    // the faces it covers with the ground and composites itself over them, so the image is
    // identical either way and the whole decision is a parameter count — the same test
    // every fill model and every arc has to pass, with the residual term equal on both
    // sides. Where the layer does not pay, the flat form is what is written.
    // Editability mode: post-fit structure passes, every one guarded to the ring's
    // own tolerance. Runs after repair and harmonization so nothing downstream
    // re-breaks what was locked.
    if args.editability {
        let stats = editable::edit_all(&polys, &mut fitted, &prims);
        if !args.quiet {
            eprintln!("{}", stats.summary());
        }
    }
    let emit = |order: &[FaceRings], an: Option<Layers>| {
        emit_color(
            order,
            &fitted,
            &prims,
            &fills,
            &pal,
            &face_color,
            &clear,
            args.cutout,
            args.native_alpha,
            args.no_background,
            &opacity,
            &alpha_ramps,
            &fades,
            an,
            matte,
            w,
            h,
            args.precision,
            args.harmonize,
            args.harmonize_threshold,
            args.use_symbols,
        )
    };
    let flat = emit(&order, None);
    let svg = match layers.as_ref().filter(|an| !an.layers.is_empty()) {
        None => flat,
        Some(an) => {
            // Every face a layer covers is relabelled onto the ground it belongs with, so
            // the cuts the layer made stop being drawn at all.
            let mut remap: Vec<u16> = (0..map.n_labels as u16).collect();
            for l in &an.layers {
                for &f in &l.faces {
                    if f >= map.n_labels {
                        continue;
                    }
                    let base = an.base_rgb.get(f).copied().unwrap_or([0.0, 0.0, 0.0]);
                    // The ground this piece belongs with: a face not under the layer whose
                    // own colour is the colour underneath this one.
                    let host = (0..map.n_labels).find(|&g| {
                        g != f
                            && !l.faces.contains(&g)
                            && an
                                .base_rgb
                                .get(g)
                                .is_some_and(|c| inkvec_trace::color::de00(*c, base) < 1.0)
                    });
                    if let Some(g) = host {
                        remap[f] = g as u16;
                    }
                }
            }
            let merged = merge_map(&map, &remap);
            let order_m = planar::face_edge_order(&merged);
            // The layer's own outline, by the same trick: its pieces become one face, so
            // the cuts *inside* the layer stop being drawn as well. Without this the layer
            // path repeats every edge the ground merge just removed, and the document comes
            // out with fewer shapes and more coordinates, which is not simpler.
            let layer_shapes: Vec<FaceRings> = an
                .layers
                .iter()
                .map(|l| {
                    let Some(&first) = l.faces.first() else {
                        return Vec::new();
                    };
                    let mut remap_l: Vec<u16> = (0..map.n_labels as u16).collect();
                    for &f in &l.faces {
                        if f < map.n_labels {
                            remap_l[f] = first as u16;
                        }
                    }
                    planar::face_edge_order(&merge_map(&map, &remap_l))
                        .get(first)
                        .cloned()
                        .unwrap_or_default()
                })
                .collect();
            let layered = emit(&order_m, Some((an, &layer_shapes)));
            let cost = |doc: &str| -> (usize, usize) {
                let shapes = ["<path", "<rect", "<circle", "<ellipse"]
                    .iter()
                    .map(|t| doc.matches(t).count())
                    .sum::<usize>();
                (shapes, doc.len())
            };
            let (pf, bf) = cost(&flat);
            let (pl, bl) = cost(&layered);
            // Fewer shapes *and* not more coordinates. A document with fewer paths and
            // more numbers in them is not the simpler one, whatever the path count says.
            let pays = pl < pf && bl <= bf;
            if !args.quiet {
                eprintln!(
                    "                as layers {pl} shapes / {bl} bytes, flat {pf} / {bf}: {}",
                    if pays { "kept" } else { "dropped" }
                );
            }
            if pays {
                layered
            } else {
                flat
            }
        }
    };
    sw.mark("emit");
    if let Some(path) = &args.uncertainty {
        let bands =
            uncertainty::bands_svg(&svg, &map.edges, map.width, map.height, args.uncertainty_k);
        if let Err(e) = std::fs::write(path, bands) {
            eprintln!(
                "could not write the confidence bands to {}: {e}",
                path.display()
            );
        }
    }
    let anchors = n_cubic + n_line;
    Ok((
        svg,
        vec![
            format!(
                "palette       {} colours, {} faces ({n_grad} gradient)",
                pal.len(),
                face_color.len()
            ),
            format!(
                "planar map    {} shared edges ({} primitive)",
                map.edges.len(),
                prims.iter().filter(|p| p.is_some()).count()
            ),
            boundary_report.as_ref().map_or_else(
                || "boundary solve  no gain".to_string(),
                |r| format!(
                    "boundary solve  E {:.1} -> {:.1} in {} iteration(s), {} point(s) moved{}",
                    r.before,
                    r.after,
                    r.iters,
                    r.moved,
                    if r.scale < 1.0 {
                        format!(", scaled to {:.0}% to stay simple", r.scale * 100.0)
                    } else {
                        String::new()
                    }
                ),
            ),
            if symmetry.is_empty() {
                "symmetry      none in the label map".to_string()
            } else {
                format!(
                    "symmetry      {} mirror(s), {symmetrised} point(s) averaged, {mirrored_fits} fit(s) reflected",
                    symmetry.mirrors.len()
                )
            },
            format!("repair        {repaired} boundary refit(s) to stop rings crossing"),
            format!(
                "segments      {anchors}  ({n_line} line, {n_cubic} cubic) from {measured} measured points, {:.1}x reduction",
                measured as f64 / anchors.max(1) as f64
            ),
        ],
    ))
}

/// MDL cost of a fitted path against the measurements it came from, in the same units
/// the fitters minimise, so a primitive and a curve description are directly comparable.
fn structural_trial_improves(
    polys: &[inkvec_core::Polyline],
    trial: &[FittedPath],
    baseline: &[FittedPath],
    scales: &[f64],
    cfg: &FitConfig,
) -> bool {
    if polys.len() != trial.len() || polys.len() != baseline.len() || polys.len() != scales.len() {
        return false;
    }
    let mut strict = false;
    for (((poly, candidate), original), &scale) in polys.iter().zip(trial).zip(baseline).zip(scales)
    {
        let rate = |p: &FittedPath| p.segments.iter().map(|s| s.params()).sum::<f64>();
        // An SVG arc writes seven numbers even when its geometric model has
        // only five degrees of freedom. Protect both notions of complexity.
        let written = |p: &FittedPath| {
            p.segments
                .iter()
                .map(|s| match s {
                    Segment::Line(..) => 2usize,
                    Segment::Cubic(..) => 6,
                    Segment::Arc { .. } => 7,
                })
                .sum::<usize>()
        };
        let (new_rate, old_rate) = (rate(candidate), rate(original));
        if new_rate > old_rate || written(candidate) > written(original) {
            return false;
        }
        let local = FitConfig {
            lambda: cfg.lambda * scale,
            ..*cfg
        };
        let (new_cost, old_cost) = (
            path_cost(poly, candidate, &local),
            path_cost(poly, original, &local),
        );
        if !new_cost.is_finite() || !old_cost.is_finite() || new_cost > old_cost + 1e-9 {
            return false;
        }
        strict |= new_rate < old_rate;
    }
    strict
}

fn path_cost(poly: &inkvec_core::Polyline, path: &FittedPath, cfg: &FitConfig) -> f64 {
    let chi2 = inkvec_fit::curves::chi2(&poly.points, &poly.sigma, path.start, &path.segments);
    0.5 * chi2 + cfg.lambda * path.params()
}

/// Reject a gradient whose two stops a viewer could not tell apart.
///
/// MDL already charges a gradient for its extra parameters, but a gradient has more
/// freedom than a flat fill and will always explain sensor noise a little better. When
/// the noise estimate is even slightly low, that freedom wins on cost while describing
/// something that is not there — flat concentric rings came back as four radial
/// gradients, costing fidelity rather than buying it.
///
/// The guard is perceptual rather than statistical: if the fitted endpoints are within a
/// just-noticeable difference in OKLab, there is no gradient to see, whatever the
/// residual says.
fn demote_imperceptible_gradient(f: gradient::FillFit) -> gradient::FillFit {
    use inkvec_trace::color::rgb_to_oklab;
    /// A conservative multiple of a just-noticeable difference in OKLab.
    const JND: f32 = 0.02;

    let (c0, c1) = match f.model {
        gradient::FillModel::Flat(_) => return f,
        gradient::FillModel::Linear { c0, c1, .. } => (c0, c1),
        gradient::FillModel::Radial { c0, c1, .. } => (c0, c1),
    };
    if rgb_to_oklab(c0).dist(rgb_to_oklab(c1)) >= JND {
        return f;
    }
    let mid = [
        0.5 * (c0[0] + c1[0]),
        0.5 * (c0[1] + c1[1]),
        0.5 * (c0[2] + c1[2]),
    ];
    gradient::FillFit {
        model: gradient::FillModel::Flat(mid),
        ..f
    }
}

/// A basic colour name for an OKLab colour, for use as an element id.
///
/// Not a semantic label — it does not know that a shape is a wing or a wheel — but it is
/// the part of naming that can be done from the geometry alone, and it is the difference
/// between an editor showing `path4728` and showing `blue-3`. Semantic naming needs a
/// model that has seen the picture (DESIGN.md S6); this needs nothing.
pub(crate) fn colour_name(rgb: [f32; 3]) -> &'static str {
    let c = inkvec_trace::color::rgb_to_oklab(rgb);
    let chroma = (c.a * c.a + c.b * c.b).sqrt();
    if chroma < 0.03 {
        return match c.l {
            l if l > 0.93 => "white",
            l if l > 0.72 => "light-grey",
            l if l > 0.42 => "grey",
            l if l > 0.18 => "dark-grey",
            _ => "black",
        };
    }
    // Hue in degrees, measured the usual way round the OKLab a/b plane.
    let hue = c.b.atan2(c.a).to_degrees().rem_euclid(360.0);
    let base = match hue {
        h if h < 20.0 => "red",
        h if h < 45.0 => "orange",
        h if h < 70.0 => "yellow",
        h if h < 100.0 => "olive",
        h if h < 165.0 => "green",
        h if h < 200.0 => "teal",
        h if h < 240.0 => "cyan",
        h if h < 285.0 => "blue",
        h if h < 320.0 => "purple",
        h if h < 345.0 => "magenta",
        _ => "red",
    };
    // Brown is dark orange, and calling it orange reads wrong in a layer list.
    if (base == "orange" || base == "red") && c.l < 0.5 {
        return "brown";
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_transaction_rejects_repair_inflation_and_accepts_exact_merges() {
        use inkvec_core::{Point, Polyline};
        use inkvec_fit::curves::Segment;
        let a = Point::new(0.0, 0.0);
        let b = Point::new(10.0, 0.0);
        let c = Point::new(20.0, 0.0);
        let poly = Polyline::new(vec![a, b, c], vec![0.1; 3], false);
        let original = FittedPath {
            start: a,
            segments: vec![Segment::Line(b), Segment::Line(c)],
            closed: false,
        };
        let compact = FittedPath {
            start: a,
            segments: vec![Segment::Line(c)],
            closed: false,
        };
        let inflated = FittedPath {
            start: a,
            segments: vec![Segment::Cubic(b, b, c)],
            closed: false,
        };
        let cfg = FitConfig::default();
        assert!(structural_trial_improves(
            &[poly.clone()],
            &[compact],
            &[original.clone()],
            &[1.0],
            &cfg
        ));
        assert!(!structural_trial_improves(
            &[poly.clone()],
            &[inflated],
            &[original.clone()],
            &[1.0],
            &cfg
        ));
        assert!(!structural_trial_improves(
            &[poly],
            &[],
            &[original],
            &[1.0],
            &cfg
        ));
    }

    #[test]
    fn test_colour_name() {
        assert_eq!(colour_name([1.0, 1.0, 1.0]), "white");
        assert_eq!(colour_name([0.0, 0.0, 0.0]), "black");
        assert_eq!(colour_name([1.0, 0.0, 0.0]), "orange");
        assert_eq!(colour_name([0.0, 0.0, 1.0]), "blue");
    }

    #[test]
    fn test_path_cost_computation() {
        use inkvec_core::{Point, Polyline};
        use inkvec_fit::curves::Segment;
        let p = FittedPath {
            start: Point::new(0.0, 0.0),
            segments: vec![
                Segment::Line(Point::new(10.0, 10.0)),
                Segment::Line(Point::new(20.0, 10.0)),
            ],
            closed: false,
        };
        let poly = Polyline::new(
            vec![
                Point::new(0.0, 0.0),
                Point::new(10.0, 10.0),
                Point::new(20.0, 10.0),
            ],
            vec![0.5, 0.5, 0.5],
            false,
        );
        let cfg = FitConfig::from_precision(256.0, 0.1, 2.0);
        let cost = path_cost(&poly, &p, &cfg);
        assert!(cost > 0.0);
    }
}
