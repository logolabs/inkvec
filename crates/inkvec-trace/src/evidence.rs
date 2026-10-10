//! The evidence a raster gives about the boundaries of a planar map, as window areas, and
//! the evaluator the representation chain scores candidate descriptions with
//! (`docs/theory/chain-boundary.md`, "Interface proposal"; the shared vocabulary is
//! [`inkvec_core::likelihood`]).
//!
//! # What it holds
//!
//! For every edge between two flat fills, its **run windows**: maximal runs of the edge's
//! partial pixels along a column (where the edge runs within 45° of horizontal) or a row,
//! each with the sum of the edge's left face's unmixed weights (which is that face's exact
//! area in the window, whatever the curve: the window identity, B2.1) and the variance of
//! that sum (8-bit quantisation of the partial pixels, B1.2, plus the renderer floor's
//! per-window part, B2.3). Every partial pixel of an edge is in exactly one window. Around
//! every **junction** (a node where three or more edges end) the pixels within a small
//! radius are kept per pixel, in colour, for the local term (B3.2), with the junction's arms
//! in cyclic order and the pairs that may continue one another (A4). Along runs, **corner
//! proposals** where the fourth difference of consecutive windows fires (B3.3). And the
//! **renderer floor** (A3): per window, and the offset shared by the windows of an edge that
//! runs along an axis or a diagonal.
//!
//! # Where it sits
//!
//! [`build`] reads the planar map after stage 07 (its topology, and its geometry only to say
//! which edge owns a pixel and which way it runs), the composited image and each face's fill.
//! It changes nothing in the trace; the fitter consumes it through
//! [`BoundaryLikelihood`]. Edges with a gradient on either side, or the image's frame on one
//! side, have no run windows yet (their windows would need the moment term of B1.4).
//!
//! Method from: the column-sum (partial-area) identity of volume-of-fluid reconstruction and
//! sub-pixel edge detection (E. G. Puckett (2010), CAMCoS 5(1); A. Trujillo-Pino et al.
//! (2013), Image and Vision Computing 31(1), <https://doi.org/10.1016/j.imavis.2012.10.005>).
//! Not from the literature: the window partition, its use as the likelihood of arbitrary
//! descriptions, and the renderer-floor model (measured in `bench/theory/renderer_floor.py`).

use inkvec_core::likelihood::{
    BoundaryLikelihood, Chi2, CornerProposal, Floor, JunctionReport, Owner, Piece, RenderModel,
    RunObs, StrokeBand,
};
use inkvec_core::noise::NoiseModel;

use crate::gradient::FillModel;
use crate::planar::{Edge, PlanarMap};

pub mod checks;
mod local;
pub mod noise;
mod reports;
mod windows;

use inkvec_core::Point;
use local::{CornerPixel, Local, LocalPixel, StripPixel};
use windows::{Axis2, RunPixel};

/// Record a corner term (the pixels of windows the starting geometry does not cross) and its
/// proposal, placed before run window `index` of edge `edge`.
fn push_corner(
    corners: &mut Vec<CornerProposal>,
    terms: &mut Vec<Vec<CornerPixel>>,
    edge: usize,
    index: usize,
    pixels: Vec<CornerPixel>,
) {
    let n = pixels.len().max(1) as f64;
    let at = pixels.iter().fold(Point::new(0.0, 0.0), |a, p| {
        Point::new(a.x + p.x as f64 / n, a.y + p.y as f64 / n)
    });
    corners.push(CornerProposal {
        edge: edge as u32,
        index,
        z: f64::INFINITY,
        at,
    });
    terms.push(pixels);
}

/// How the evidence is built.
#[derive(Debug, Clone, Copy)]
pub struct EvidenceOptions {
    /// The renderer floor; `None` for the corpus intake's (resvg at 8× with 4 × 4 samples per
    /// device pixel: a 32-sample lattice, [`Floor::lattice`]).
    pub floor: Option<Floor>,
    /// How candidates are drawn (usvg's arc tolerance in px).
    pub render: RenderModel,
    /// A pixel within this distance (px) of an edge's starting geometry is that edge's.
    pub reach: f64,
    /// Pixels within this distance (px) of a junction belong to its local term.
    pub local_radius: f64,
    /// Corner proposals: smallest `|D|` in standard deviations.
    pub corner_z: f64,
    /// Junction continuations: largest angle difference in standard deviations.
    pub continuation_z: f64,
    /// The intake was lossy with subsampled chroma (a JPEG): weights are read from luma, the
    /// channel kept at full resolution, wherever the two inks' lumas differ enough.
    pub lossy: bool,
    /// The intake's box-equivalent edge width (`softness::ramp_evidence`), px: 1 on a native
    /// render. A blurred edge's ramp spans about this many pixels, and the reach widens to
    /// cover it, so that each window holds the observed ramp whole.
    pub ramp_width: f64,
    /// Estimate each window's variance beyond rounding from the image's own fourth
    /// differences ([`noise`]); off, the floor's `window_var` alone is added.
    pub self_calibrate: bool,
}

impl Default for EvidenceOptions {
    fn default() -> Self {
        Self {
            floor: None,
            render: RenderModel::default(),
            reach: 1.6,
            local_radius: 2.5,
            corner_z: 3.0,
            continuation_z: 3.0,
            lossy: false,
            ramp_width: 1.0,
            self_calibrate: true,
        }
    }
}

/// Pixels two edges both reach away from a junction (thin features), marked taken: their
/// positions, and the same in colour with their pair of edges (variance `var` per channel).
fn strip_terms(
    claims: &[windows::Claim],
    rgb: &[[f32; 3]],
    w: usize,
    reach: f64,
    var: f64,
    taken: &mut [bool],
) -> (Vec<(i32, i32)>, Vec<StripPixel>) {
    let (mut at, mut strips) = (Vec::new(), Vec::new());
    for (i, c) in claims.iter().enumerate() {
        if c.edge != u32::MAX && c.dist <= reach && c.dist2 <= reach && !taken[i] {
            taken[i] = true;
            let (x, y) = ((i % w) as i32, (i / w) as i32);
            at.push((x, y));
            let p = rgb[i];
            strips.push(StripPixel {
                x,
                y,
                rgb: [p[0] as f64, p[1] as f64, p[2] as f64],
                var,
                pair: (c.edge.min(c.edge2), c.edge.max(c.edge2)),
            });
        }
    }
    (at, strips)
}

/// The two-ink axis an edge between inks `left` and `right` is read on, or `None` when they
/// are too close to unmix. Self-calibrating, a window's variance starts from rounding alone
/// and the image's own windows supply the rest ([`noise`]); the pixel noise still sets the
/// third-ink test. On a lossy intake, luma where the inks' lumas differ enough.
fn edge_axis(
    left: [f32; 3],
    right: [f32; 3],
    sigma_noise: f64,
    opts: &EvidenceOptions,
) -> Option<Axis2> {
    let sigma_var = if opts.self_calibrate {
        0.0
    } else {
        sigma_noise
    };
    let axis = Axis2 {
        third_tol2: (4.0 * sigma_noise.max(1.0 / 255.0)).powi(2),
        ..Axis2::new(left, right, sigma_var)
    };
    if axis.dd.sqrt() < (3.0 * sigma_noise).max(0.02) {
        return None;
    }
    Some(if opts.lossy {
        axis.luma_of(LUMA_MIN_CONTRAST, LOSSY_CHROMA_TOL)
            .unwrap_or(Axis2 {
                third_tol2: LOSSY_CHROMA_TOL * LOSSY_CHROMA_TOL,
                ..axis
            })
    } else {
        axis
    })
}

/// `|d|²` of an axis in the units its weights are read in (luma's contrast, squared, on a
/// luma axis).
fn axis_dd(a: Axis2) -> f64 {
    match a.luma {
        Some((_, dy)) => dy * dy,
        None => a.dd,
    }
}

/// Box-equivalent edge width above which an intake is soft (resampled or blurred): its
/// windows then take every pixel of the edge's band on their line, not only those that read
/// as mixtures, since noise splits a blurred ramp's run of partial pixels.
const SOFT_RAMP_WIDTH: f64 = 1.2;
/// The shape of the per-pixel noise by distance from the nearest edge on a resized JPEG,
/// relative to the first band (luma, web tier: 9.4, 3.1, 2.3 and 1.7 levels within 1, 2, 3
/// and 5 px; `bench/theory/noise_profile.py`). Scaled per image by the windows' own noise.
const EDGE_NOISE_SHAPE: [f64; 4] = [1.0, 0.33, 0.25, 0.18];

/// Least luma difference (sRGB units) between two inks for their windows to be read from
/// luma on a lossy intake: three times the noise measured within a pixel of a web-tier edge
/// (about nine levels).
const LUMA_MIN_CONTRAST: f64 = 0.1;
/// How far off the colour line a lossy intake's pixel may sit before it holds a third ink:
/// 4:2:0 chroma bleeds across every edge, at about 6.5 levels rms within a pixel of one on the
/// web tier, so four of those (in sRGB units).
const LOSSY_CHROMA_TOL: f64 = 26.0 / 255.0;
/// Plateau pixels for an edge's inks are taken this far beyond its reach, px.
const PLATEAU_BAND: f64 = 2.5;
/// Fewest plateau pixels on a side for their median to stand for that side's ink.
const PLATEAU_MIN: usize = 5;

/// Per edge, the per-channel median colour of the pixels just beyond its reach on each side
/// (left, right): the inks a window's weights are read against, robust to noise, ringing and
/// a JPEG's block errors (a mean is not). Pixels that another edge also nearly reaches are
/// left out. `None` where a side has fewer than [`PLATEAU_MIN`] such pixels (a thin face).
fn plateau_inks(
    claims: &[windows::Claim],
    rgb: &[[f32; 3]],
    reach: f64,
    n_edges: usize,
) -> Vec<[Option<[f32; 3]>; 2]> {
    let mut side: Vec<[Vec<[f32; 3]>; 2]> =
        (0..n_edges).map(|_| [Vec::new(), Vec::new()]).collect();
    for (i, c) in claims.iter().enumerate() {
        if c.edge == u32::MAX || c.dist <= reach || c.dist2 < c.dist + 1.0 {
            continue;
        }
        side[c.edge as usize][usize::from(!c.left)].push(rgb[i]);
    }
    side.into_iter()
        .map(|mut lr| {
            let med = |v: &mut Vec<[f32; 3]>| -> Option<[f32; 3]> {
                if v.len() < PLATEAU_MIN {
                    return None;
                }
                let mut out = [0f32; 3];
                for (ch, o) in out.iter_mut().enumerate() {
                    let mut c: Vec<f32> = v.iter().map(|p| p[ch]).collect();
                    let k = c.len() / 2;
                    *o = *c.select_nth_unstable_by(k, f32::total_cmp).1;
                }
                Some(out)
            };
            [med(&mut lr[0]), med(&mut lr[1])]
        })
        .collect()
}

/// The evidence of one image: see the module docs.
#[derive(Debug, Clone)]
pub struct Evidence {
    width: usize,
    height: usize,
    runs: Vec<Vec<RunObs>>,
    run_pixels: Vec<Vec<Vec<(i32, i32)>>>,
    lattice_edge: Vec<bool>,
    lengths: Vec<Vec<f64>>,
    locals: Vec<Local>,
    junctions: Vec<JunctionReport>,
    corners: Vec<CornerProposal>,
    /// Per corner proposal, its per-pixel term (empty for a proposal inside a run).
    corner_terms: Vec<Vec<CornerPixel>>,
    /// Pixels shared by two edges away from a junction (thin features): scored only against a
    /// stroke band ([`BoundaryLikelihood::chi2_band`]).
    strip_pixels: Vec<(i32, i32)>,
    /// The same pixels, in colour, with the pair of edges that reach them.
    strips: Vec<StripPixel>,
    /// Pixels of an edge's own that hold a third ink: not scored.
    third_pixels: Vec<(i32, i32)>,
    floor: Floor,
    render: RenderModel,
    faces: Vec<FillModel>,
    edge_faces: Vec<(u16, u16)>,
    /// Per edge, the two-ink axis its pixels were unmixed on (`None`: no runs).
    axes: Vec<Option<Axis2>>,
    /// Per edge and run window, the variance of its sum from quantisation (and measured
    /// noise) alone, before the renderer floor.
    quant_var: Vec<Vec<f64>>,
    /// What the windows' fourth differences said about their error beyond rounding.
    calibration: noise::Calibration,
    /// The intake's noise, summarised for the representation chain.
    noise: NoiseModel,
    /// Per edge, the variance of the offset its windows share (A3).
    offset_var: Vec<f64>,
    /// Per edge and run window, the fractional index of its centre along the edge's points.
    point_at: Vec<Vec<f64>>,
}

/// The variance of one 8-bit level's rounding, per channel.
const Q2_12: f64 = 1.0 / (255.0 * 255.0 * 12.0);

/// The local term of every junction: the pixels within `r` px of it that no earlier term
/// holds, each with its colour and variance `var` (marked taken).
fn junction_locals(
    junctions: &[JunctionReport],
    rgb: &[[f32; 3]],
    (w, h): (usize, usize),
    r: f64,
    var: f64,
    taken: &mut [bool],
) -> Vec<Local> {
    let mut locals = Vec::with_capacity(junctions.len());
    for j in junctions {
        let mut pixels = Vec::new();
        let (x0, x1) = (
            (j.at.x - r).floor().max(0.0) as usize,
            ((j.at.x + r).ceil() as usize).min(w - 1),
        );
        let (y0, y1) = (
            (j.at.y - r).floor().max(0.0) as usize,
            ((j.at.y + r).ceil() as usize).min(h - 1),
        );
        for y in y0..=y1 {
            for x in x0..=x1 {
                if (x as f64 - j.at.x).hypot(y as f64 - j.at.y) > r || taken[y * w + x] {
                    continue;
                }
                taken[y * w + x] = true;
                let c = rgb[y * w + x];
                pixels.push(LocalPixel {
                    x: x as i32,
                    y: y as i32,
                    rgb: [c[0] as f64, c[1] as f64, c[2] as f64],
                    var,
                });
            }
        }
        locals.push(Local {
            node: j.node,
            at: j.at,
            arms: j
                .arms
                .iter()
                .map(|a| (a.edge as usize, a.at_start))
                .collect(),
            pixels,
        });
    }
    locals
}

/// One edge's share of the evidence: its run windows (with their pixels), the corner terms
/// between them (each with the index of the run window it precedes), and its third-ink pixels.
#[derive(Default)]
struct EdgeEvidence {
    runs: Vec<RunObs>,
    run_pixels: Vec<Vec<(i32, i32)>>,
    corner_terms: Vec<(usize, Vec<CornerPixel>)>,
    third: Vec<(i32, i32)>,
}

/// What [`edge_evidence`] reads besides the edge.
struct Intake<'a> {
    rgb: &'a [[f32; 3]],
    alpha: Option<&'a [f32]>,
    width: usize,
    claims: &'a [windows::Claim],
    /// Take every non-third pixel of the edge's band, not only those reading as mixtures.
    band: bool,
}

/// An edge's polyline indexed by arclength, so that a window's crossing test reads only the
/// stretch near it (`O(log n)` to find, instead of the whole edge per window).
struct NearPolyline<'a> {
    points: &'a [Point],
    closed: bool,
    /// Cumulative arclength at each point (and, closed, back at the first).
    s: Vec<f64>,
}

impl<'a> NearPolyline<'a> {
    fn new(points: &'a [Point], closed: bool) -> Self {
        let n = points.len();
        let m = if closed && n > 0 { n + 1 } else { n };
        let mut s = Vec::with_capacity(m);
        let mut acc = 0.0;
        for k in 0..m {
            if k > 0 {
                acc += points[k % n].dist(points[(k - 1) % n]);
            }
            s.push(acc);
        }
        NearPolyline { points, closed, s }
    }

    /// The points within `half` px of arclength of `at` (wrapping round a closed edge), with
    /// one more point either side so the stretch reaches past them.
    fn around(&self, at: f64, half: f64) -> Vec<Point> {
        let n = self.points.len();
        if n == 0 {
            return Vec::new();
        }
        let total = *self.s.last().unwrap_or(&0.0);
        if !self.closed {
            let lo = self.s.partition_point(|&v| v < at - half).saturating_sub(1);
            let hi = (self.s.partition_point(|&v| v <= at + half) + 1).min(n);
            return self.points[lo..hi].to_vec();
        }
        if total <= 2.0 * half + 2.0 {
            let mut all = self.points.to_vec();
            all.push(self.points[0]);
            return all;
        }
        // Closed: from the point before `at − half` round to the one after `at + half`.
        let s0 = (at - half).rem_euclid(total);
        let k0 = self.s.partition_point(|&v| v <= s0).saturating_sub(1) % n;
        let mut out = vec![self.points[k0]];
        let mut walked = self.s[k0] - s0;
        let mut k = k0;
        while walked <= 2.0 * half && out.len() <= n {
            let next = (k + 1) % n;
            walked += self.points[next].dist(self.points[k]);
            out.push(self.points[next]);
            k = next;
        }
        // One more point past the end.
        let next = (k + 1) % n;
        out.push(self.points[next]);
        out
    }
}

/// The run windows and corner terms of edge `e` between flat inks (`axis`), from its pixels
/// `mine` (indices into the image), marking the pixels it uses taken.
fn edge_evidence(
    e: &Edge,
    axis: &Axis2,
    mine: &[usize],
    at: &Intake,
    taken: &mut [bool],
) -> EdgeEvidence {
    let w = at.width;
    let mut out = EdgeEvidence::default();
    let mut px = Vec::with_capacity(mine.len());
    let profile = at.band.then(|| windows::class_profile(&e.points, e.closed));
    for &i in mine {
        let mut m = axis.unmix(at.rgb[i], at.alpha.map(|a| a[i]));
        if m.third {
            out.third.push(((i % w) as i32, (i / w) as i32));
            taken[i] = true;
            continue;
        }
        if at.band && !m.partial {
            m.partial = true;
            m.var = axis.var_partial;
        }
        let c = at.claims[i];
        let column = match &profile {
            Some(p) => windows::class_at(p, c.s),
            None => c.t.0.abs() >= c.t.1.abs(),
        };
        px.push(RunPixel {
            x: (i % w) as i32,
            y: (i / w) as i32,
            m,
            s: c.s,
            t: c.t,
            column,
        });
    }
    for p in &px {
        if p.m.partial {
            taken[(p.y as usize) * w + p.x as usize] = true;
        }
    }
    // No pure flanks: a pixel that reads as one face's pure colour may hold up to half a
    // level of the other's area and reads none of it, an error of one sign the rounding model
    // does not describe (measured: chi2/M 1.85 with flanks, on 8-bit squares). The windows'
    // areas need no flank to be exact (the clamp), so a window is its partial pixels only.
    let mut flank = |_: i32, _: i32| None;
    let grouped = windows::group(&px, &mut flank);
    // A window the starting geometry does not cross once (a corner turns inside it, or the
    // edge ends there) cannot be read as a run: its pixels go to a corner term, scored per
    // pixel, and the place is proposed as a corner.
    let near = NearPolyline::new(&e.points, e.closed);
    let mut pending: Option<Vec<CornerPixel>> = None;
    for (o, p) in grouped {
        let piece = near.around(o.s, o.window.len() as f64 + 4.0);
        if checks::polyline_left_area(&piece, &o.window).is_some() {
            if let Some(pix) = pending.take() {
                out.corner_terms.push((out.runs.len(), pix));
            }
            out.runs.push(o);
            out.run_pixels.push(p);
        } else {
            let term = pending.get_or_insert_with(Vec::new);
            for (x, y) in p {
                let i = y as usize * w + x as usize;
                let m = axis.unmix(at.rgb[i], at.alpha.map(|a| a[i]));
                term.push(CornerPixel {
                    x,
                    y,
                    a: m.a,
                    var: m.var,
                });
            }
        }
    }
    if let Some(pix) = pending.take() {
        out.corner_terms.push((out.runs.len(), pix));
    }
    out
}

/// Build the evidence of `rgb` (row-major sRGB, the map's size) for `map`, whose faces have
/// fills `faces`; `sigma_noise` is [`crate::coverage::estimate_noise`]'s. `O(boundary pixels)`.
pub fn build(
    map: &PlanarMap,
    rgb: &[[f32; 3]],
    faces: &[FillModel],
    sigma_noise: f64,
    opts: &EvidenceOptions,
) -> Evidence {
    build_with_alpha(map, rgb, None, faces, sigma_noise, opts)
}

/// [`build`], told the source's alpha (`rgb` is then the image composited onto white). Where
/// the source drew an ink partly over the clear ground the intake rounded the alpha, not the
/// colour, and the weight's variance is one level's whatever the ink's contrast with white.
pub fn build_with_alpha(
    map: &PlanarMap,
    rgb: &[[f32; 3]],
    alpha: Option<&[f32]>,
    faces: &[FillModel],
    sigma_noise: f64,
    opts: &EvidenceOptions,
) -> Evidence {
    let alpha = alpha.filter(|a| a.len() == rgb.len());
    let (w, h) = (map.width, map.height);
    let floor = opts.floor.unwrap_or_else(|| Floor::lattice(32));
    let reach = opts.reach.max(0.5 * opts.ramp_width + 1.1);
    let claims = windows::claims(map, reach + PLATEAU_BAND);
    let mut taken = vec![false; w * h];

    // Junctions and their local pixels.
    let junctions = reports::junctions(map, 6.0, 0.02, opts.continuation_z);
    let extra = (sigma_noise * sigma_noise - 0.25 / (255.0 * 255.0)).max(0.0);
    let locals = junction_locals(
        &junctions,
        rgb,
        (w, h),
        opts.local_radius,
        Q2_12 + extra,
        &mut taken,
    );

    // Pixels two edges reach away from a junction: thin features, kept out of runs.
    let (strip_pixels, mut strips) = strip_terms(&claims, rgb, w, reach, Q2_12 + extra, &mut taken);

    let intake = Intake {
        rgb,
        alpha,
        width: w,
        claims: &claims,
        band: opts.lossy || opts.ramp_width > SOFT_RAMP_WIDTH,
    };
    let AllEdges {
        mut runs,
        run_pixels,
        third_pixels,
        mut corners,
        mut corner_terms,
        axes,
    } = all_edges(map, faces, reach, &intake, sigma_noise, opts, &mut taken);
    let edge_faces: Vec<(u16, u16)> = map.edges.iter().map(|e| (e.left, e.right)).collect();

    let quant_var: Vec<Vec<f64>> = runs
        .iter()
        .map(|r| r.iter().map(|o| o.var).collect())
        .collect();
    let (calibration, noise, offset_var) = calibrate_runs(
        &mut runs,
        &quant_var,
        &run_pixels,
        &axes,
        floor,
        sigma_noise,
        opts,
    );
    let mut locals = locals;
    if !noise.is_clean() {
        let s2 = noise.sigma_edge_at(0.0).powi(2);
        raise_local_noise(
            s2,
            &mut locals,
            &mut strips,
            &corners,
            &mut corner_terms,
            &axes,
        );
    }
    let RunReports {
        point_at,
        lattice_edge,
        lengths,
        corners: run_corners,
    } = run_reports(map, &runs, floor, opts);
    for c in run_corners {
        corners.push(c);
        corner_terms.push(Vec::new());
    }
    Evidence {
        width: w,
        height: h,
        runs,
        run_pixels,
        lattice_edge,
        lengths,
        locals,
        junctions,
        corners,
        corner_terms,
        strip_pixels,
        strips,
        third_pixels,
        floor,
        render: RenderModel {
            psf_mu2: noise.psf_mu2,
            ..opts.render
        },
        faces: faces.to_vec(),
        edge_faces,
        axes,
        quant_var,
        calibration,
        noise,
        offset_var,
        point_at,
    }
}

/// What each edge's runs report once their variances are known: where each window sits along
/// the edge's points, whether the edge runs along the floor's lattice, each window's
/// boundary length, and the corner proposals of the fourth difference.
struct RunReports {
    point_at: Vec<Vec<f64>>,
    lattice_edge: Vec<bool>,
    lengths: Vec<Vec<f64>>,
    corners: Vec<CornerProposal>,
}

fn run_reports(
    map: &PlanarMap,
    runs: &[Vec<RunObs>],
    floor: Floor,
    opts: &EvidenceOptions,
) -> RunReports {
    let n_edges = runs.len();
    let point_at: Vec<Vec<f64>> = runs
        .iter()
        .zip(&map.edges)
        .map(|(r, e)| {
            r.iter()
                .map(|o| point_index(&e.points, e.closed, o.s))
                .collect()
        })
        .collect();
    let mut out = RunReports {
        point_at,
        lattice_edge: vec![false; n_edges],
        lengths: vec![Vec::new(); n_edges],
        corners: Vec::new(),
    };
    for k in 0..n_edges {
        out.corners.extend(reports::corners(
            k as u32,
            &runs[k],
            map.edges[k].closed,
            opts.corner_z,
            opts.lossy,
        ));
        out.lattice_edge[k] = reports::on_lattice(&runs[k], floor.lattice);
        out.lengths[k] = reports::window_lengths(&runs[k]);
    }
    out
}

/// Per-pixel terms take the noise within a pixel of an edge (`s2`, per channel): junctions,
/// thin strips, and corner terms (in their axis's weight units).
fn raise_local_noise(
    s2: f64,
    locals: &mut [Local],
    strips: &mut [StripPixel],
    corners: &[CornerProposal],
    corner_terms: &mut [Vec<CornerPixel>],
    axes: &[Option<Axis2>],
) {
    for l in locals.iter_mut() {
        for p in &mut l.pixels {
            p.var = p.var.max(s2);
        }
    }
    for p in strips.iter_mut() {
        p.var = p.var.max(s2);
    }
    for (c, term) in corners.iter().zip(corner_terms.iter_mut()) {
        let dd = axes[c.edge as usize].map_or(1.0, axis_dd);
        for p in term.iter_mut() {
            p.var = p.var.max(s2 / dd);
        }
    }
}

/// Every edge's runs, corner terms and third-ink pixels, with the axis each was read on.
struct AllEdges {
    runs: Vec<Vec<RunObs>>,
    run_pixels: Vec<Vec<Vec<(i32, i32)>>>,
    third_pixels: Vec<(i32, i32)>,
    corners: Vec<CornerProposal>,
    corner_terms: Vec<Vec<CornerPixel>>,
    axes: Vec<Option<Axis2>>,
}

/// Read every edge between two flat fills: its pixels within `reach` that nothing has taken,
/// unmixed against its plateau inks ([`plateau_inks`], else the faces' fills).
fn all_edges(
    map: &PlanarMap,
    faces: &[FillModel],
    reach: f64,
    intake: &Intake,
    sigma_noise: f64,
    opts: &EvidenceOptions,
    taken: &mut [bool],
) -> AllEdges {
    let n_edges = map.edges.len();
    let mut out = AllEdges {
        runs: vec![Vec::new(); n_edges],
        run_pixels: vec![Vec::new(); n_edges],
        third_pixels: Vec::new(),
        corners: Vec::new(),
        corner_terms: Vec::new(),
        axes: vec![None; n_edges],
    };
    let mut by_edge: Vec<Vec<usize>> = vec![Vec::new(); n_edges];
    for (i, c) in intake.claims.iter().enumerate() {
        if c.edge != u32::MAX && c.dist <= reach && !taken[i] {
            by_edge[c.edge as usize].push(i);
        }
    }
    let inks = plateau_inks(intake.claims, intake.rgb, reach, n_edges);
    for (k, e) in map.edges.iter().enumerate() {
        let (Some(FillModel::Flat(cl)), Some(FillModel::Flat(cr))) =
            (faces.get(e.left as usize), faces.get(e.right as usize))
        else {
            continue;
        };
        let [il, ir] = inks[k];
        let Some(axis) = edge_axis(il.unwrap_or(*cl), ir.unwrap_or(*cr), sigma_noise, opts) else {
            continue;
        };
        let ev = edge_evidence(e, &axis, &by_edge[k], intake, taken);
        out.axes[k] = Some(axis);
        out.third_pixels.extend(ev.third);
        for (index, pix) in ev.corner_terms {
            push_corner(&mut out.corners, &mut out.corner_terms, k, index, pix);
        }
        out.runs[k] = ev.runs;
        out.run_pixels[k] = ev.run_pixels;
    }
    out
}

/// The fractional point index along `points` (closed: wrapping) at arclength `s`.
fn point_index(points: &[Point], closed: bool, s: f64) -> f64 {
    let n = points.len();
    if n < 2 {
        return 0.0;
    }
    let segs = if closed { n } else { n - 1 };
    let mut acc = 0.0;
    for j in 0..segs {
        let l = points[(j + 1) % n].dist(points[j]);
        if s < acc + l || j + 1 == segs {
            let f = if l > 0.0 {
                ((s - acc) / l).clamp(0.0, 1.0)
            } else {
                0.0
            };
            return j as f64 + f;
        }
        acc += l;
    }
    (segs - 1) as f64
}

/// Add each window's variance beyond rounding (the image's own estimate for the window's
/// class, at least the floor's `window_var`), deflate replica stretches to one measurement
/// each, and set each edge's offset variance: one window's variance under self-calibration
/// (the phase-locking argument of [`noise`]), else the floor's `edge_var` on edges along
/// the lattice. Returns the calibration, the noise model it summarises and the offsets.
#[allow(clippy::too_many_arguments)]
fn calibrate_runs(
    runs: &mut [Vec<RunObs>],
    quant_var: &[Vec<f64>],
    run_pixels: &[Vec<Vec<(i32, i32)>>],
    axes: &[Option<Axis2>],
    floor: Floor,
    sigma_noise: f64,
    opts: &EvidenceOptions,
) -> (noise::Calibration, NoiseModel, Vec<f64>) {
    let classes: Vec<Vec<(noise::Class, Option<f64>)>> =
        runs.iter().map(|r| noise::classify(r)).collect();
    let cal = if opts.self_calibrate {
        let ds: Vec<noise::Stencil> = runs
            .iter()
            .zip(quant_var)
            .zip(&classes)
            .flat_map(|((r, q), c)| noise::fourth_differences(r, q, c))
            .collect();
        noise::calibrate(&ds)
    } else {
        noise::Calibration::NONE
    };
    let reps_all: Vec<Vec<usize>> = runs
        .iter()
        .map(|r| {
            if opts.self_calibrate {
                noise::replica_stretches(r)
            } else {
                vec![1; r.len()]
            }
        })
        .collect();
    let lattice = if opts.self_calibrate {
        lattice_of(runs, &reps_all, run_pixels, quant_var)
    } else {
        None
    };
    let sawtooth = lattice.map_or(0.0, |n| 1.0 / (12.0 * (n as f64).powi(2)));
    let (mut v_q, mut v_all) = (0.0, 0.0);
    let mut per_pixel: Vec<f64> = Vec::new();
    let mut offset_var = vec![0.0; runs.len()];
    for (k, r) in runs.iter_mut().enumerate() {
        let dd = axes[k].map_or(1.0, axis_dd);
        let reps = &reps_all[k];
        let mut single: Vec<f64> = Vec::with_capacity(r.len());
        for (i, o) in r.iter_mut().enumerate() {
            let mut extra = cal.extra[classes[k][i].0 as usize].max(floor.window_var);
            if reps[i] >= 2 {
                extra += sawtooth;
            }
            let one = quant_var[k][i] + extra;
            single.push(one);
            o.var = one * reps[i] as f64;
            o.share = 1.0 / reps[i] as f64;
            // The noise summary reads the part independent between windows only: a
            // renderer's lattice offset is systematic, not noise.
            let rough = cal.extra[classes[k][i].0 as usize];
            v_q += quant_var[k][i];
            v_all += quant_var[k][i] + rough;
            let n = run_pixels[k][i].len().max(1) as f64;
            per_pixel.push(rough * dd / n);
        }
        offset_var[k] = if opts.self_calibrate && !single.is_empty() {
            let m = single.len() / 2;
            *single.select_nth_unstable_by(m, f64::total_cmp).1
        } else {
            0.0
        };
    }
    let ramp = opts.ramp_width.max(1.0);
    let noisy = v_all > v_q * 1.0001 || opts.lossy || ramp > SOFT_RAMP_WIDTH;
    let noise = if !opts.self_calibrate || !noisy {
        NoiseModel::clean()
    } else {
        let s_pix = if per_pixel.is_empty() {
            0.0
        } else {
            let k = per_pixel.len() / 2;
            per_pixel
                .select_nth_unstable_by(k, f64::total_cmp)
                .1
                .max(0.0)
                .sqrt()
        };
        noise_model(&cal, s_pix, sigma_noise, ramp, opts.lossy, (v_q, v_all))
    };
    (cal, noise, offset_var)
}

/// A point-sampling renderer's lattice, from the single partial pixels of axis-aligned replica
/// stretches (one value per stretch of at least three): their windows then carry its
/// sawtooth.
fn lattice_of(
    runs: &[Vec<RunObs>],
    reps: &[Vec<usize>],
    run_pixels: &[Vec<Vec<(i32, i32)>>],
    quant_var: &[Vec<f64>],
) -> Option<u32> {
    let mut values: Vec<(f64, f64)> = Vec::new();
    for (k, r) in runs.iter().enumerate() {
        let mut i = 0;
        while i < r.len() {
            let n = reps[k][i];
            if n >= 3 && run_pixels[k][i].len() == 1 {
                values.push((r[i].sum, (3.0 * quant_var[k][i]).sqrt()));
            }
            i += n.max(1);
        }
    }
    noise::detect_lattice(&values)
}

/// The noise model a calibration summarises: per-pixel edge noise `s_pix` (colour units, the
/// windows' error spread over their pixels) shaped by distance, the blur from the ramp width,
/// and the window scale from the rounding and total variances `(v_q, v_all)`.
fn noise_model(
    cal: &noise::Calibration,
    s_pix: f64,
    sigma_noise: f64,
    ramp: f64,
    lossy: bool,
    (v_q, v_all): (f64, f64),
) -> NoiseModel {
    let flat = sigma_noise.max(0.5 / 255.0);
    NoiseModel {
        sigma_flat: flat,
        sigma_edge: EDGE_NOISE_SHAPE
            .iter()
            .map(|t| (flat * flat + (s_pix * t).powi(2)).sqrt())
            .collect(),
        psf_radius: 0.5 * (ramp - 1.0),
        psf_mu2: 0.25 * (ramp * ramp - 1.0),
        nu: cal.nu,
        lossy,
        window_scale: if v_q > 0.0 { (v_all / v_q).sqrt() } else { 1.0 },
    }
}

/// Edge noise (per channel, within a pixel of an edge) above which an intake's measured noise
/// is folded into the planar map's per-point uncertainties: three 8-bit levels. Clean renders
/// read at most about 1.3 (bench: `floor_selfcal.py`, exact, lattice and resvg renders at
/// 128 px), a resized quality-80 JPEG 4 to 27.
pub const FOLD_LEVEL: f64 = 3.0 / 255.0;

/// Measure an intake's noise from its own edges, for an intake the colour path found soft or
/// lossy (its edges wider than a native render's, ringing, or a lossy container). Where the
/// edges' noise is under [`FOLD_LEVEL`] the model is [`NoiseModel::clean`], so that a clean
/// intake is traced exactly as before.
///
/// With `INKVEC_NOISE_FOLD=1` it also folds the noise into `map`'s per-point uncertainties:
/// each point's `σ` grows by the windows' measured error beyond rounding, in quadrature, which
/// is what the fitter divides by. Off by default until the representation chain's fitter reads
/// it with its robust loss: with today's fitter it buys simpler fits at a cost in fidelity
/// (`quality-web`, 62 icons: turning −5.0 %, parameters −1.4 %, dE00 +2.8 %, geom +1.7 %).
pub fn measure_noise(
    map: &mut PlanarMap,
    rgb: &[[f32; 3]],
    faces: &[FillModel],
    sigma_noise: f64,
    lossy: bool,
) -> NoiseModel {
    let fold = inkvec_core::env::flag("INKVEC_NOISE_FOLD");
    measure_noise_with(map, rgb, faces, sigma_noise, lossy, fold)
}

/// The colour path's last step: a soft or lossy intake's noise sits at its edges, where
/// flat-region estimates cannot see it, so it is measured there ([`measure_noise`]) into
/// [`ColorTrace::noise`](crate::ColorTrace::noise). A clean intake (`soft` false) is
/// returned as it came, without measuring anything.
pub(crate) fn with_noise(
    mut ct: crate::ColorTrace,
    rgb: &[[f32; 3]],
    soft: bool,
    lossy: bool,
    sw: &mut crate::Stopwatch,
) -> crate::ColorTrace {
    if soft {
        let faces: Vec<FillModel> = ct.face_fill.iter().map(|f| f.model.clone()).collect();
        ct.noise = measure_noise(&mut ct.map, rgb, &faces, ct.sigma_noise, lossy);
        sw.mark("noise");
    }
    ct
}

/// [`measure_noise`], with the fold into the per-point uncertainties chosen by the caller.
pub fn measure_noise_with(
    map: &mut PlanarMap,
    rgb: &[[f32; 3]],
    faces: &[FillModel],
    sigma_noise: f64,
    lossy: bool,
    fold: bool,
) -> NoiseModel {
    let ramps = crate::softness::ramp_evidence(rgb, map.width, map.height);
    let opts = EvidenceOptions {
        lossy,
        ramp_width: ramps.width.max(1.0),
        ..EvidenceOptions::default()
    };
    let ev = build(map, rgb, faces, sigma_noise, &opts);
    let noise = ev.noise().clone();
    if noise.is_clean() || noise.sigma_edge_at(0.0) <= FOLD_LEVEL {
        return NoiseModel::clean();
    }
    if fold {
        let extra = ev.calibration.extra[0].max(ev.calibration.extra[1]);
        for e in &mut map.edges {
            for s in &mut e.sigma {
                *s = (*s * *s + extra).sqrt();
            }
        }
    }
    noise
}

impl Evidence {
    /// Image size, px.
    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// The pixels of edge `e`'s run window `i`, in order along its line.
    pub fn window_pixels(&self, e: usize, i: usize) -> &[(i32, i32)] {
        &self.run_pixels[e][i]
    }

    /// The pixels of a junction's local term, by node id.
    pub fn local_pixels(&self, node: u32) -> Vec<(i32, i32)> {
        self.locals
            .iter()
            .find(|l| l.node == node)
            .map(|l| l.pixels.iter().map(|p| (p.x, p.y)).collect())
            .unwrap_or_default()
    }

    /// The pixels of corner proposal `i`'s per-pixel term (empty for a proposal inside a run).
    pub fn corner_pixels(&self, i: usize) -> Vec<(i32, i32)> {
        self.corner_terms[i].iter().map(|p| (p.x, p.y)).collect()
    }

    /// Pixels two edges share away from a junction (thin features; not scored yet).
    pub fn strip_pixels(&self) -> &[(i32, i32)] {
        &self.strip_pixels
    }

    /// Pixels on an edge holding a third ink (not scored).
    pub fn third_pixels(&self) -> &[(i32, i32)] {
        &self.third_pixels
    }

    /// Per run window of edge `e`, the variance of its sum from quantisation (and measured
    /// noise) alone, before the renderer floor.
    pub fn quantisation_var(&self, e: usize) -> &[f64] {
        &self.quant_var[e]
    }

    /// What the windows' fourth differences said about their error beyond rounding.
    pub fn calibration(&self) -> &noise::Calibration {
        &self.calibration
    }

    /// The intake's noise as the windows measured it ([`NoiseModel::clean`] when nothing
    /// beyond rounding was found on a native, lossless intake).
    pub fn noise(&self) -> &NoiseModel {
        &self.noise
    }

    /// The sums edge `e`'s run windows read on another image of the same size (row-major,
    /// composited onto white): the same pixels unmixed on the same two inks. Scoring a known
    /// truth this way gives each window's error, `sum − truth`.
    pub fn window_sums_of(&self, e: usize, rgb: &[[f32; 3]]) -> Vec<f64> {
        let Some(axis) = self.axes[e] else {
            return Vec::new();
        };
        self.run_pixels[e]
            .iter()
            .map(|pix| {
                pix.iter()
                    .map(|&(x, y)| {
                        axis.unmix(rgb[y as usize * self.width + x as usize], None)
                            .a
                    })
                    .sum()
            })
            .collect()
    }
}

impl BoundaryLikelihood for Evidence {
    fn edge_count(&self) -> usize {
        self.runs.len()
    }

    fn runs(&self, e: usize) -> &[RunObs] {
        &self.runs[e]
    }

    fn render_model(&self) -> RenderModel {
        self.render
    }

    fn floor(&self) -> Floor {
        self.floor
    }

    fn edge_on_lattice(&self, e: usize) -> bool {
        self.lattice_edge[e]
    }

    fn edge_offset_var(&self, e: usize) -> f64 {
        let lattice = if self.lattice_edge[e] {
            self.floor.edge_var
        } else {
            0.0
        };
        self.offset_var[e].max(lattice)
    }

    fn tail_nu(&self) -> f64 {
        self.noise.nu
    }

    fn window_point_index(&self, e: usize) -> Vec<f64> {
        self.point_at[e].clone()
    }

    fn chi2_band(&self, pair: (usize, usize), band: &StrokeBand, inks: [[f32; 3]; 3]) -> Chi2 {
        let (a, b) = (pair.0.min(pair.1) as u32, pair.0.max(pair.1) as u32);
        local::chi2_band(&self.strips, (a, b), band, inks)
    }

    fn chi2_local_layers(
        &self,
        node: u32,
        layers: &[(&[Piece], [f32; 3])],
        ground: [f32; 3],
    ) -> Chi2 {
        match self.locals.iter().find(|l| l.node == node) {
            Some(term) => local::chi2_layers(term, &self.render, layers, ground),
            None => Chi2::default(),
        }
    }

    fn chi2_local(&self, owner: Owner, curves: &[(usize, &[Piece])]) -> Chi2 {
        match owner {
            Owner::Junction(node) => {
                let Some(term) = self.locals.iter().find(|l| l.node == node) else {
                    return Chi2::default();
                };
                // Order the candidates as the term's arms.
                let mut ordered: Vec<&[Piece]> = Vec::with_capacity(term.arms.len());
                for &(e, _) in &term.arms {
                    match curves.iter().find(|(ce, _)| *ce == e) {
                        Some((_, c)) => ordered.push(c),
                        None => return Chi2::default(),
                    }
                }
                let colour = |e: usize, left: bool, x: i32, y: i32| -> [f64; 3] {
                    let (l, r) = self.edge_faces[e];
                    let f = if left { l } else { r };
                    let c = self
                        .faces
                        .get(f as usize)
                        .map_or([0.0; 3], |m| m.color_at(x as f64, y as f64));
                    [c[0] as f64, c[1] as f64, c[2] as f64]
                };
                local::chi2(term, &self.render, &ordered, &colour).unwrap_or_default()
            }
            Owner::Corner(i) => {
                // Only the corner's own pixels: the run windows round it are the runs'.
                let Some(c) = self.corners.get(i as usize) else {
                    return Chi2::default();
                };
                match curves.iter().find(|(ce, _)| *ce == c.edge as usize) {
                    Some((_, cv)) => {
                        local::chi2_corner(&self.corner_terms[i as usize], &self.render, cv)
                    }
                    None => Chi2::default(),
                }
            }
        }
    }

    fn junctions(&self) -> &[JunctionReport] {
        &self.junctions
    }

    fn corners(&self) -> &[CornerProposal] {
        &self.corners
    }

    fn density(&self, e: usize) -> Vec<(f64, f64)> {
        self.runs[e]
            .iter()
            .zip(&self.lengths[e])
            .map(|(o, l)| (o.s, l / o.var.max(1e-300)))
            .collect()
    }
}

#[cfg(test)]
#[path = "evidence_tests.rs"]
mod tests;
