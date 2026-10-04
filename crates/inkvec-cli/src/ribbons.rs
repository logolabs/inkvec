//! Research prototype (`INKVEC_RIBBONS=1`): faces that were drawn as strokes, written as
//! strokes -- centrelines with one `stroke-width`, round caps and joins -- instead of as
//! filled outlines. Off by default; with the variable unset nothing here runs and the
//! output is byte-identical to the shipped pipeline.
//!
//! **Where it runs.** In the colour pipeline after every boundary is fitted, repaired and
//! mirrored and every face's fill and transparency are settled, before the document is
//! written ([`crate::pipeline`]). It hands the emitter a map from face to finished stroke
//! element(s); the emitter stops painting those faces and paints the strokes on top of
//! everything, where the existing underlap ([`crate::seams`]) lets every neighbour reach
//! under them so no seam shows ([`crate::emit`]).
//!
//! **Which faces.** Opaque, flat-filled faces that are not the canvas: their geometry goes
//! to [`inkvec_trace::ribbon::fit_face`], which pairs the two sides of the outline,
//! rebuilds the centrelines and fits them (see that module for the passes and their
//! literature).
//!
//! **The decision** ([`decide`]) is by description length, in the fitter's own units
//! (nats), on the same evidence for both descriptions -- the face's measured boundary
//! points with their sigmas:
//!
//! `ΔL = (χ²_stroke - χ²_outline)/2 + λ·(k_stroke - k_vanish)`
//!
//! * `χ²_outline` is the fitted edges' chi-squared against their own measured points, the
//!   quantity each edge's dynamic program minimised; `χ²_stroke` is the strokes' painted
//!   outline against the same points ([`inkvec_trace::ribbon::Score`]).
//! * `k_stroke` is the strokes' parameter count (centrelines plus one width).
//! * `k_vanish` is what *leaves the document* when the face becomes strokes: the edges
//!   only this face writes. In the stacked emitter a face writes its outline rings and,
//!   under native alpha, the transparent holes punched out of it; an outline edge that a
//!   painted neighbour also writes (two siblings side by side) stays, and a hole whose
//!   inside is another painted face is that face's to write and stays too. A ring wholly
//!   written by this face also loses its move-to (2).
//!
//! The face becomes strokes when `ΔL < 0` and `k_stroke < k_vanish`: cheaper overall and
//! strictly fewer parameters, because a representation change that spends more
//! parameters to buy fidelity is not what this stage is for.
//!
//! Method from: Rissanen (1978), Modeling by shortest data description, Automatica 14(5),
//! doi:10.1016/0005-1098(78)90005-5 -- the two-part code the whole fitter uses, here
//! choosing between two models of one face. Inspired by: Favreau, Lafarge, Bousseau
//! (2016), Fidelity vs. simplicity: a global approach to line drawing vectorization, ACM
//! TOG 35(4), doi:10.1145/2897824.2925946, whose energy trades a line drawing's fitting
//! error against its number of curves; ours uses that trade to choose between strokes and
//! fills per face, with the error measured on the painted outline.

use std::collections::BTreeMap;

use inkvec_core::Polyline;
use inkvec_fit::primitives::PrimitiveFit;
use inkvec_fit::{FitConfig, FittedPath};
use inkvec_trace::ribbon::{self, FaceMask, Ribbon};
use inkvec_trace::{gradient, planar};
use rayon::prelude::*;

use crate::faces::FaceRings;
use crate::pathdata::fmt_fitted;
use crate::primitive::stroke_element;
use crate::rings;

/// Whether the stage is switched on (`INKVEC_RIBBONS=1`).
pub(crate) fn on() -> bool {
    inkvec_core::env::flag("INKVEC_RIBBONS")
}

/// The stage as the colour pipeline calls it, on the colour document `doc` before its
/// strokes are added (its rings, fits, primitives, fills and transparency), the planar map
/// and the label map. Nothing (an empty map) unless the switch is on, the trace is Quality
/// and the output is in colour (fast mode and `--monochrome` never run it). `cfg`'s lambda
/// is scaled by `--lambda-scale` here, as the pipeline scales it for the repair. The report
/// line goes to stderr (unless `--quiet`).
pub(crate) fn stage(
    args: &crate::args::Args,
    cfg: &FitConfig,
    fast: bool,
    doc: &crate::emit::ColorDoc,
    map: &planar::PlanarMap,
    labels: &[u16],
) -> Ribbons {
    if !on() || fast || args.monochrome {
        return Ribbons::default();
    }
    let cfg_r = FitConfig {
        lambda: cfg.lambda * args.lambda_scale,
        ..*cfg
    };
    let out = choose(&Inputs {
        map,
        order: doc.order,
        fitted: doc.fitted,
        prims: doc.prims,
        fills: doc.fill_fits,
        clear: doc.clear,
        opacity: doc.opacity,
        labels,
        w: map.width,
        h: map.height,
        cfg: &cfg_r,
        decimals: crate::pathdata::emit_decimals(args.precision),
    });
    if let Some(line) = &out.line {
        crate::diag::stage(args.quiet, || line.clone());
    }
    out
}

/// Whether to print one line per candidate face to stderr (`INKVEC_RIBBONS_DEBUG=1`).
fn debug() -> bool {
    inkvec_core::env::flag("INKVEC_RIBBONS_DEBUG")
}

/// Everything the stage reads: the settled map, fits, fills and transparency.
pub(crate) struct Inputs<'a> {
    /// The planar map; its edges hold the solved boundary points and sigmas.
    pub(crate) map: &'a planar::PlanarMap,
    /// Each face's rings over the map's edges.
    pub(crate) order: &'a [FaceRings],
    /// Each edge's fitted curve.
    pub(crate) fitted: &'a [FittedPath],
    /// Each edge's whole-edge primitive, where one won.
    pub(crate) prims: &'a [Option<PrimitiveFit>],
    /// Each face's fill model.
    pub(crate) fills: &'a [gradient::FillFit],
    /// Each face is transparent in the source.
    pub(crate) clear: &'a [bool],
    /// Each face's opacity.
    pub(crate) opacity: &'a [f32],
    /// The traced label map (face id per pixel), row-major.
    pub(crate) labels: &'a [u16],
    /// Raster width, px.
    pub(crate) w: usize,
    /// Raster height, px.
    pub(crate) h: usize,
    /// The fit configuration, lambda already scaled by `--lambda-scale`.
    pub(crate) cfg: &'a FitConfig,
    /// Decimals per written coordinate.
    pub(crate) decimals: usize,
}

/// What the stage decided: per face written as strokes, its element text, and a report
/// line.
#[derive(Default)]
pub(crate) struct Ribbons {
    /// Face -> the stroke element(s) that replace it, in face order.
    pub(crate) elements: BTreeMap<usize, String>,
    /// One report line (`None` when the stage did not run).
    pub(crate) line: Option<String>,
}

/// One candidate's outcome, for the decision and the debug line.
struct Outcome {
    /// The face.
    face: usize,
    /// The stroke description, or why there is none.
    fit: Result<Ribbon, ribbon::Decline>,
    /// The outline's chi-squared on the same points.
    chi2_outline: f64,
    /// Parameters that leave the document with the face.
    k_vanish: f64,
}

/// Run the stage over every candidate face and decide each by [`decide`].
pub(crate) fn choose(inp: &Inputs) -> Ribbons {
    let t0 = inkvec_core::clock::Instant::now();
    let nest = rings::nesting(inp.order, inp.fitted);
    let n_faces = inp.order.len();
    let bboxes = FaceMask::bounding_boxes(inp.labels, inp.w, inp.h, n_faces);
    let image_area = (inp.w * inp.h) as f64;
    let candidates: Vec<usize> = (0..n_faces)
        .filter(|&f| {
            !inp.order[f].is_empty()
                && matches!(
                    inp.fills.get(f).map(|x| &x.model),
                    Some(gradient::FillModel::Flat(_))
                )
                && !inp.clear.get(f).copied().unwrap_or(false)
                && inp.opacity.get(f).copied().unwrap_or(1.0) >= 1.0
                && bboxes[f].is_some()
        })
        .collect();
    let drawers = edge_drawers(inp, &nest);
    let outcomes: Vec<Outcome> = candidates
        .par_iter()
        .filter_map(|&f| {
            let mask = FaceMask::from_labels(inp.labels, inp.w, f as u16, bboxes[f]?);
            let area = mask.area() as f64;
            if area < 16.0 || area > 0.5 * image_area {
                return None;
            }
            let k_vanish = vanishing_params(inp, &nest, &drawers, f);
            // Nothing to save: no stroke description can beat writing nothing more.
            if k_vanish < 4.0 {
                return None;
            }
            let fit = ribbon::fit_face(&face_rings(inp, f), &mask, inp.cfg, k_vanish);
            Some(Outcome {
                face: f,
                fit,
                chi2_outline: outline_chi2(inp, f),
                k_vanish,
            })
        })
        .collect();
    let mut out = Ribbons::default();
    let (mut saved, mut n_tried) = (0.0, 0usize);
    for o in &outcomes {
        n_tried += 1;
        let verdict = decide(o, inp.cfg.lambda);
        if debug() {
            debug_line(o, verdict);
        }
        if let (Some(_), Ok(r)) = (verdict, &o.fit) {
            let Some(gradient::FillModel::Flat(c)) = inp.fills.get(o.face).map(|x| &x.model) else {
                continue;
            };
            let hex = inkvec_trace::color::to_hex(*c);
            out.elements.insert(
                o.face,
                element(r, &hex, inp.decimals, &format!("stroke-{}", o.face)),
            );
            saved += o.k_vanish - r.params();
        }
    }
    out.line = Some(format!(
        "ribbons       {} of {n_tried} face(s) written as strokes, {saved:.0} parameter(s) saved",
        out.elements.len()
    ));
    if inkvec_core::env::flag("INKVEC_TIMING") {
        eprintln!(
            "  [t] ribbons            {:.1} ms",
            t0.elapsed().as_secs_f64() * 1e3
        );
    }
    out
}

/// The verdict for one outcome: `Some(ΔL)` when the face should become strokes (see the
/// module documentation for `ΔL`), `None` otherwise.
///
/// `INKVEC_RIBBONS_FORCE=1` (inspection only) accepts every fitted face that saves
/// parameters, whatever its fit, so the failures can be looked at.
fn decide(o: &Outcome, lambda: f64) -> Option<f64> {
    let r = o.fit.as_ref().ok()?;
    let k = r.params();
    let dl = 0.5 * (r.score.chi2 - o.chi2_outline) + lambda * (k - o.k_vanish);
    let force = inkvec_core::env::flag("INKVEC_RIBBONS_FORCE");
    ((dl < 0.0 || force) && k < o.k_vanish && dl.is_finite()).then_some(dl)
}

/// `INKVEC_RIBBONS_DEBUG`: one line per candidate on stderr.
fn debug_line(o: &Outcome, verdict: Option<f64>) {
    let msg = match &o.fit {
        Err(e) => format!("  ribbon face {:>4}: declined {e:?}", o.face),
        Ok(r) => format!(
            "  ribbon face {:>4}: {:?} w {:.3} (paired {:.3}, share {:.2}) lines {} junctions {} caps {} | k {:.0} vs vanish {:.0} | chi2 {:.0} vs outline {:.0} | rms {:.3} (pre {:.3}) worst {:.2} out {} uncov {} | {}",
            o.face,
            r.join,
            r.width,
            r.paired_width,
            r.paired_share,
            r.lines.len(),
            r.junctions,
            r.caps,
            r.params(),
            o.k_vanish,
            r.score.chi2,
            o.chi2_outline,
            r.score.rms,
            r.pre_rms,
            r.score.worst,
            r.score.outliers,
            r.uncovered,
            verdict.map_or_else(|| "kept as fill".to_string(), |dl| format!("STROKES (dL {dl:.0})"))
        ),
    };
    eprintln!("{msg}");
}

/// Face `f`'s rings as closed polylines of the map's solved boundary points and sigmas,
/// each edge walked in the ring's direction.
fn face_rings(inp: &Inputs, f: usize) -> Vec<Polyline> {
    inp.order[f]
        .iter()
        .map(|ring| {
            let (mut pts, mut sig) = (Vec::new(), Vec::new());
            for &(k, rev) in ring {
                let e = &inp.map.edges[k];
                let n = e.points.len();
                for i in 0..n {
                    let j = if rev { n - 1 - i } else { i };
                    pts.push(e.points[j]);
                    sig.push(e.sigma.get(j).copied().unwrap_or(0.1));
                }
            }
            Polyline::new(pts, sig, true)
        })
        .collect()
}

/// The fitted outline's chi-squared on face `f`'s boundary points: each distinct edge of
/// its rings once, against its own fitted curve, by exact nearest distance
/// ([`ribbon::path_chi2`], the same measure as the strokes' score).
fn outline_chi2(inp: &Inputs, f: usize) -> f64 {
    let mut seen = std::collections::BTreeSet::new();
    let mut chi2 = 0.0;
    for ring in &inp.order[f] {
        for &(k, _) in ring {
            if !seen.insert(k) {
                continue;
            }
            let e = &inp.map.edges[k];
            let p = &inp.fitted[k];
            chi2 += ribbon::path_chi2(&e.points, &e.sigma, p);
        }
    }
    chi2
}

/// Per edge, the painted faces that write it as part of their outline in the stacked
/// document: every face that is not transparent, for the edges of its outline rings.
fn edge_drawers(inp: &Inputs, nest: &rings::Nesting) -> Vec<Vec<usize>> {
    let mut drawers: Vec<Vec<usize>> = vec![Vec::new(); inp.map.edges.len()];
    for (g, rings) in inp.order.iter().enumerate() {
        if inp.clear.get(g).copied().unwrap_or(false) {
            continue;
        }
        for &r in &nest.outer[g] {
            for &(k, _) in &rings[r] {
                drawers[k].push(g);
            }
        }
    }
    drawers
}

/// The parameters face `f`'s element writes that no other face writes: its outline
/// edges no painted neighbour also outlines, and the edges of its holes whose other side
/// is transparent (punched out of it). Edges are priced as the fitter prices them --
/// segments only, plus the ring's move-to (2) when the whole ring goes, and a ring that is
/// one primitive at the primitive's own count.
fn vanishing_params(inp: &Inputs, nest: &rings::Nesting, drawers: &[Vec<usize>], f: usize) -> f64 {
    let seg_params = |k: usize| -> f64 { inp.fitted[k].segments.iter().map(|s| s.params()).sum() };
    let mut total = 0.0;
    for (r, ring) in inp.order[f].iter().enumerate() {
        let outline = nest.outer[f].contains(&r);
        let goes = |&(k, _): &(usize, bool)| -> bool {
            if outline {
                drawers[k].iter().all(|&g| g == f)
            } else {
                let e = &inp.map.edges[k];
                let other = if e.left as usize == f {
                    e.right
                } else {
                    e.left
                } as usize;
                inp.clear.get(other).copied().unwrap_or(false)
            }
        };
        if let [(k, _)] = ring.as_slice() {
            if let Some(p) = inp.prims.get(*k).and_then(|p| p.as_ref()) {
                if goes(&ring[0]) {
                    total += p.params;
                }
                continue;
            }
        }
        let gone: Vec<&(usize, bool)> = ring.iter().filter(|e| goes(e)).collect();
        total += gone.iter().map(|&&(k, _)| seg_params(k)).sum::<f64>();
        if gone.len() == ring.len() {
            total += 2.0;
        }
    }
    total
}

/// The SVG for a face written as strokes: one `<path>` with every non-primitive
/// centreline as a subpath, stroked `hex` at the ribbon's width with round caps and the
/// ribbon's joins (round or miter, SVG's default miter limit), then one stroked primitive
/// element per centreline that is a whole primitive.
/// Ids are `id` and `id-k`.
fn element(r: &Ribbon, hex: &str, decimals: usize, id: &str) -> String {
    let mut d = String::new();
    let mut prims = String::new();
    for (k, line) in r.lines.iter().enumerate() {
        match line
            .prim
            .as_ref()
            .and_then(|p| stroke_element(&p.kind, r.width, hex, decimals))
        {
            Some(mut el) => {
                if let Some(sp) = el.find(' ') {
                    el.insert_str(sp, &format!(" id=\"{id}-{k}\""));
                }
                prims.push_str(&el);
            }
            None => fmt_fitted(&line.path, line.path.closed, decimals, &mut d),
        }
    }
    let mut out = String::new();
    if !d.is_empty() {
        out.push_str(&format!(
            "<path id=\"{id}\" d=\"{d}\" fill=\"none\" stroke=\"{hex}\" stroke-width=\"{w:.decimals$}\" stroke-linecap=\"{c}\" stroke-linejoin=\"{j}\"/>",
            w = r.width,
            c = r.cap.svg(),
            j = r.join.svg()
        ));
    }
    out.push_str(&prims);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use inkvec_core::Point;
    use inkvec_fit::curves::Segment;
    use inkvec_trace::ribbon::{Cap, Centreline, Join, Score};

    /// A one-line ribbon of width 4 with the given chi-squared.
    fn ribbon(chi2: f64, join: Join) -> Ribbon {
        Ribbon {
            lines: vec![Centreline {
                path: FittedPath {
                    start: Point::new(1.0, 2.0),
                    segments: vec![Segment::Line(Point::new(11.0, 2.0))],
                    closed: false,
                },
                prim: None,
            }],
            width: 4.0,
            join,
            cap: Cap::Round,
            paired_width: 4.0,
            paired_share: 0.9,
            score: Score {
                chi2,
                half: 2.0,
                rms: 0.01,
                worst: 0.02,
                outliers: 0,
            },
            pre_rms: 0.01,
            uncovered: 0,
            points: 100,
            junctions: 0,
            caps: 2,
            dropped: 0,
        }
    }

    fn outcome(fit: Result<Ribbon, ribbon::Decline>, chi2_outline: f64, k_vanish: f64) -> Outcome {
        Outcome {
            face: 1,
            fit,
            chi2_outline,
            k_vanish,
        }
    }

    #[test]
    fn decide_weighs_fit_against_parameters() {
        // k = 2 + 2 + 1 = 5 against 20 that vanish: 15 parameters at lambda 7 is 105 nats.
        let r = ribbon(150.0, Join::Round);
        assert_eq!(r.params(), 5.0);
        // chi2 worse by 100, i.e. 50 nats: still a win.
        assert!(decide(&outcome(Ok(r.clone()), 50.0, 20.0), 7.0).is_some());
        // chi2 worse by 300, i.e. 150 nats: a loss.
        assert!(decide(&outcome(Ok(r.clone()), -150.0, 20.0), 7.0).is_none());
        // Not fewer parameters: never, however good the fit.
        assert!(decide(&outcome(Ok(r.clone()), 1e6, 5.0), 7.0).is_none());
        // An outline that could not be measured (infinite) keeps the outline.
        assert!(decide(&outcome(Ok(r), f64::INFINITY, 20.0), 7.0).is_none());
        // A declined face is never written as strokes.
        assert!(decide(&outcome(Err(ribbon::Decline::NoCentreline), 0.0, 20.0), 7.0).is_none());
    }

    #[test]
    fn element_writes_one_stroked_path_with_its_join() {
        let el = element(&ribbon(1.0, Join::Miter), "#000000", 2, "stroke-3");
        assert_eq!(
            el,
            "<path id=\"stroke-3\" d=\"M1.00,2.00L11.00,2.00\" fill=\"none\" stroke=\"#000000\" stroke-width=\"4.00\" stroke-linecap=\"round\" stroke-linejoin=\"miter\"/>"
        );
        assert!(element(&ribbon(1.0, Join::Round), "#123456", 2, "s")
            .contains("stroke-linejoin=\"round\""));
    }
}
