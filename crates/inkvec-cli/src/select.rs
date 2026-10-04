//! Structural hypotheses, chosen by description length (`--hypotheses`, off by default).
//!
//! # The problem
//!
//! Three structural decisions of the colour tracer are right for most images and wrong,
//! by a lot, on a minority:
//!
//! * **blend absorption** reads a pixel between two inks as an anti-aliased blend of them
//!   and gives it to one of them; on a sub-pixel gap between two black shapes the grey pixel
//!   is the gap, not a blend, and absorbing it closes the gap (`simple-icons/ubiquiti` dE00
//!   0.342 with absorption, 0.070 without, at 128 px);
//! * **native alpha** traces transparency as inks with an opacity; on some translucent
//!   overlays compositing onto a matte first traces closer (`noto` `1f9da` 0.803 → 0.650);
//! * **the merge distance** (0.035 in OKLab) merges two close inks the artist kept apart
//!   (`noto` `1f478` 0.552 → 0.423 at 0.020).
//!
//! Measured by the r2-fidelity research (2026-10-02, section 3.6) on the 128 px screen set
//! and `held_a`: absorption is the wrong call on 30 % and 24 % of the icons. Used always,
//! each alternative is worse on average; chosen per icon, by which trace explains the input
//! raster best, they were worth −8.7 % dE00 on the screen set and −9.9 % on `held_a`, within
//! 0.2 % of choosing with the artist's file in hand.
//!
//! # The method
//!
//! Trace the image as the settings ask (the default hypothesis), then once per applicable
//! alternative ([`hypotheses`]), and keep the trace with the shortest two-part description
//! length (Rissanen's minimum description length principle, the rule every stage of the
//! tracer already applies inside itself):
//!
//! ```text
//! DL_i = RSS_i / (2·σ̂²) + PRICE · k_i · ½·ln N
//! ```
//!
//! * `RSS_i`: the squared error, summed over pixels and the three sRGB channels in `0..1`,
//!   between trace `i` rendered at the input's resolution with an exact box filter
//!   ([`render_residual`]) and the input, both composited onto white;
//! * `k_i`: the trace's geometry parameters, counted as the benchmark counts them (each
//!   number of the path data, and the numbers that place a primitive; [`count_params`]);
//! * `N = w·h / e²`: the observations the raster really holds, `e` the measured edge width
//!   (`inkvec_trace::coverage::intake_scale`, 1 for a crisp raster), since a blurred edge
//!   spreads one observation over several pixels;
//! * `σ̂² = min_i RSS_i / N`: the noise level, estimated as the best hypothesis's residual
//!   per observation.
//!
//! Method from: J. Rissanen (1978), *Modeling by shortest data description*, Automatica
//! 14(5):465–471, <https://doi.org/10.1016/0005-1098(78)90005-5>, the two-part code
//! `−log P(data | model) + (k/2)·log N`; and S. C. Zhu, A. Yuille (1996), *Region
//! competition: unifying snakes, region growing, and Bayes/MDL for multiband image
//! segmentation*, IEEE TPAMI 18(9):884–900, <https://doi.org/10.1109/34.537343>, which
//! chooses between whole segmentations of an image by that sum. Adapted: the noise level
//! is unknown and estimated from the best candidate, the observation count is discounted
//! for blur, and the price per number is `PRICE = 0.75` of Rissanen's `½·ln N`: the
//! in-house measurement (2026-09-16, 6888 traces of the screen set, crisp and 2x-blurred,
//! fourteen settings each) that chose this local form found it within 0.0009 dE00 of the
//! best global formula, and the multiplier between 0.6 and 0.9 to matter little.
//!
//! The alternatives mostly barely change the parameter count, so in practice the data term
//! decides; the price is there so that no hypothesis that spends more numbers can win on
//! fit alone (a lower lambda always fits the input better).
//!
//! # Measured, and cost
//!
//! Against the same build without it (2026-10-04, the 246-icon screen set and `held_a`,
//! judged at 1024 px against the artist's file, family-macro dE00): −6.0 % at 128 px on the
//! screen set and −7.6 % on `held_a` (the worst tenth −5.4 % and −8.4 %; 48 and 33 icons
//! better by more than 0.01, 9 and 1 worse); −0.9 % at 512 px (the worst tenth −8.9 %, 14
//! better and 15 worse). The 512 px losses are mostly lucide outlines, where the matte
//! hypothesis paints a stroked ring as a filled shape with a hole: the same geometry, but
//! the renderer anti-aliases a fill's edge and a stroke's edge differently, and the
//! artist's file is a stroke (`lucide/square-minus` 0.004 → 0.110). Parameters +6.9 % at
//! 128 px on the screen set, nearly all from three icons where blend absorption off keeps
//! anti-aliased slivers as faces (`synthetic/mosaic_grid6` 2.8 → 6.3 times the artist's),
//! and −0.2 % at 512 px. Up to three more traces and four renders at about 1024 px: 3.6
//! times the trace time at 512 px (20 icons, interleaved), which is why it is not on by
//! default.

use crate::args::Args;
use inkvec_trace::Rgba;

/// The price of a number, as a fraction of Rissanen's `½·ln N` (see the module docs).
const PRICE: f64 = 0.75;

/// The merge distance of the third hypothesis, in OKLab: the value the r2-fidelity research
/// measured (0.035 is the default).
const MERGE_ALT: f32 = 0.020;

/// The renders are made at `SS` times the input's size, at most `RENDER_MAX` px on the
/// longer side, and box-averaged back: exact area coverage to within 1/`SS`² of a pixel.
const RENDER_MAX: usize = 1024;
const SS_MAX: usize = 8;

/// One finished trace: the document, its report lines and the fit's lambda.
pub(crate) type Trace = (String, Vec<String>, f64);

/// The alternatives worth tracing for these settings and this raster, each with a short
/// name for the report: blend absorption off; a matte instead of native alpha, when the
/// raster has transparency and native alpha is on; the merge distance `MERGE_ALT`, when the
/// settings merge further than that.
pub(crate) fn hypotheses(args: &Args, transparent: bool) -> Vec<(&'static str, Args)> {
    let mut out = Vec::new();
    if args.absorb_blends {
        out.push((
            "no blend absorption",
            Args {
                absorb_blends: false,
                ..args.clone()
            },
        ));
    }
    if transparent && args.native_alpha {
        out.push((
            "matte instead of native alpha",
            Args {
                native_alpha: false,
                ..args.clone()
            },
        ));
    }
    if args.merge_distance > MERGE_ALT {
        out.push((
            "merge 0.020",
            Args {
                merge_distance: MERGE_ALT,
                ..args.clone()
            },
        ));
    }
    out
}

/// Trace every hypothesis with `trace` and return the one with the shortest description
/// length, `base` (the trace of the settings as given) included, with a report line saying
/// which was kept. `img` is the raster the traces are of (straight RGBA, before any matte),
/// `w × h` the size their documents are drawn at.
///
/// A hypothesis whose trace fails, or whose document cannot be rendered, is skipped; ties
/// keep the earlier one, so the default wins any tie.
pub(crate) fn choose(
    img: &Rgba,
    args: &Args,
    base: Trace,
    trace: impl Fn(&Args) -> Result<Trace, Box<dyn std::error::Error>>,
) -> Result<Trace, Box<dyn std::error::Error>> {
    let (w, h) = (img.width, img.height);
    let target = inkvec_sr::clean::on_white(img);
    let edge = inkvec_trace::coverage::intake_scale(&target, w, h).max(1.0);
    let n = ((w * h) as f64 / (edge * edge)).max(2.0);
    let transparent = img.data.as_chunks::<4>().0.iter().any(|p| p[3] < 1.0);
    let mut cands: Vec<(&'static str, Trace, f64, usize)> = Vec::new();
    let Some(rss) = render_residual(&base.0, &target, w, h) else {
        return Ok(base);
    };
    let k = count_params(&base.0);
    cands.push(("the default", base, rss, k));
    for (name, a) in hypotheses(args, transparent) {
        let Ok(t) = trace(&a) else { continue };
        let Some(rss) = render_residual(&t.0, &target, w, h) else {
            continue;
        };
        let k = count_params(&t.0);
        cands.push((name, t, rss, k));
    }
    let best_rss = cands.iter().map(|c| c.2).fold(f64::INFINITY, f64::min);
    let lnn = n.ln();
    // σ̂² from the best fit; a perfect fit (no residual at all) leaves only the rate term.
    let dl = |rss: f64, k: usize| {
        let data = if best_rss > 0.0 {
            rss * n / (2.0 * best_rss)
        } else if rss > 0.0 {
            f64::INFINITY
        } else {
            0.0
        };
        data + PRICE * k as f64 * 0.5 * lnn
    };
    let lengths: Vec<f64> = cands.iter().map(|c| dl(c.2, c.3)).collect();
    let mut pick = 0usize;
    for (i, &l) in lengths.iter().enumerate() {
        if l < lengths[pick] {
            pick = i;
        }
    }
    let line = format!(
        "hypotheses    kept {} of {}: description length {:.1} nats against {:.1} for the default",
        cands[pick].0,
        cands.len(),
        lengths[pick],
        lengths[0]
    );
    let (_, (svg, mut stats, lambda), _, _) = cands.swap_remove(pick);
    stats.push(line);
    Ok((svg, stats, lambda))
}

/// The squared error between `svg` drawn at the input's resolution and the input,
/// `Σ_pixels Σ_{R,G,B} (model − input)²`, both composited onto white, in sRGB `0..1`.
///
/// The document is rendered at `s` times the size (`s` = `RENDER_MAX / max(w, h)`, between
/// 1 and `SS_MAX`), each rendered pixel composited onto white, and the `s × s` blocks
/// averaged: compositing onto an opaque ground is linear in the premultiplied colour, so the
/// block average of the composited pixels is the composite of the area-averaged coverage,
/// the input raster's own box filter. `target` is the input composited onto white
/// (`inkvec_sr::clean::on_white`). `None` when the document does not render.
pub(crate) fn render_residual(svg: &str, target: &[[f32; 3]], w: usize, h: usize) -> Option<f64> {
    let s = (RENDER_MAX / w.max(h).max(1)).clamp(1, SS_MAX);
    let big = inkvec_sr::detect::render_svg(svg, w * s, h * s).ok()?;
    let white = inkvec_sr::clean::on_white(&big);
    let inv = 1.0 / (s * s) as f64;
    let mut rss = 0.0f64;
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f64; 3];
            for dy in 0..s {
                let row = (y * s + dy) * w * s + x * s;
                for p in &white[row..row + s] {
                    for c in 0..3 {
                        acc[c] += p[c] as f64;
                    }
                }
            }
            let t = target[y * w + x];
            for c in 0..3 {
                let d = acc[c] * inv - t[c] as f64;
                rss += d * d;
            }
        }
    }
    Some(rss)
}

/// The geometry parameters of a document, as the benchmark's `svgmodel` counts them: every
/// number of every `d` attribute (a line costs 2, a cubic 6, an arc 7), and the numbers that
/// place a primitive (`<rect>` x, y, width, height and a corner radius, `<circle>` cx, cy, r,
/// `<ellipse>` cx, cy, rx, ry and its rotation). Colours, stops and opacities are not
/// geometry and are not counted.
pub(crate) fn count_params(svg: &str) -> usize {
    let mut k = 0usize;
    let mut rest = svg;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        rest = &rest[end..];
        let name = tag[1..]
            .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .next()
            .unwrap_or("");
        let present = |a: &str| tag.contains(&format!(" {a}=\""));
        k += match name {
            "path" => tag
                .split_once(" d=\"")
                .and_then(|(_, d)| d.split_once('"'))
                .map_or(0, |(d, _)| count_numbers(d)),
            "rect" => 4 + usize::from(present("rx")),
            "circle" => 3,
            "ellipse" => 4 + usize::from(present("transform")),
            _ => 0,
        };
    }
    k
}

/// The numbers in path data (a run of digits with an optional sign, point and exponent).
fn count_numbers(d: &str) -> usize {
    let b = d.as_bytes();
    let (mut i, mut n) = (0usize, 0usize);
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() || c == b'.' {
            n += 1;
            let mut dot = false;
            while i < b.len() && (b[i].is_ascii_digit() || (b[i] == b'.' && !dot)) {
                dot |= b[i] == b'.';
                i += 1;
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                i += 1;
                if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
                    i += 1;
                }
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
        } else {
            i += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_are_counted_as_the_benchmark_counts_them() {
        let svg = "<svg viewBox=\"-0.5 -0.5 8 8\"><path d=\"M1.00,2.00L3.00,4.00C1,2 3,4 5,6A1,1 0 0,1 2,2Z\"/>\
<rect x=\"1\" y=\"1\" width=\"2\" height=\"2\" rx=\"0.5\"/><circle cx=\"4\" cy=\"4\" r=\"1\"/>\
<ellipse cx=\"4\" cy=\"4\" rx=\"2\" ry=\"1\" transform=\"rotate(10 4 4)\"/>\
<linearGradient x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"/></svg>";
        // path 2 + 2 + 6 + 7, rect 5, circle 3, ellipse 5; the gradient is not geometry.
        assert_eq!(count_params(svg), 17 + 5 + 3 + 5);
        assert_eq!(count_numbers("M-1.5e-3,2L.5.5"), 4);
    }

    #[test]
    fn the_hypotheses_follow_the_settings() {
        let a = Args::default();
        fn names(v: Vec<(&'static str, Args)>) -> Vec<&'static str> {
            v.into_iter().map(|(n, _)| n).collect()
        }
        assert_eq!(
            names(hypotheses(&a, true)),
            [
                "no blend absorption",
                "matte instead of native alpha",
                "merge 0.020"
            ]
        );
        // Opaque: native alpha changes nothing, so the matte is not a hypothesis.
        assert_eq!(
            names(hypotheses(&a, false)),
            ["no blend absorption", "merge 0.020"]
        );
        let tight = Args {
            merge_distance: 0.01,
            absorb_blends: false,
            native_alpha: false,
            ..Args::default()
        };
        assert!(hypotheses(&tight, true).is_empty());
    }

    #[test]
    fn a_document_matching_its_raster_has_no_residual_and_a_wrong_one_has() {
        // A 4 x 4 raster, the left half black, the right half white.
        let (w, h) = (4usize, 4usize);
        let target: Vec<[f32; 3]> = (0..w * h)
            .map(|i| if i % w < 2 { [0.0; 3] } else { [1.0; 3] })
            .collect();
        let head = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-0.5 -0.5 4 4\" width=\"4\" height=\"4\">";
        let right = format!(
            "{head}<rect x=\"-0.5\" y=\"-0.5\" width=\"2\" height=\"4\" fill=\"#000\"/></svg>"
        );
        let wrong = format!(
            "{head}<rect x=\"-0.5\" y=\"-0.5\" width=\"3\" height=\"4\" fill=\"#000\"/></svg>"
        );
        let r0 = render_residual(&right, &target, w, h).unwrap();
        let r1 = render_residual(&wrong, &target, w, h).unwrap();
        assert!(r0 < 1e-6, "{r0}");
        // One column of four pixels painted black instead of white, three channels each.
        assert!((r1 - 12.0).abs() < 0.1, "{r1}");
    }
}
