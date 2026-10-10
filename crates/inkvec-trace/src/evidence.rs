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
    RunObs,
};

use crate::gradient::FillModel;
use crate::planar::{Edge, PlanarMap};

pub mod checks;
mod local;
mod reports;
mod windows;

use inkvec_core::Point;
use local::{CornerPixel, Local, LocalPixel};
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
        }
    }
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
    /// Pixels shared by two edges away from a junction (thin features): not yet scored.
    strip_pixels: Vec<(i32, i32)>,
    /// Pixels of an edge's own that hold a third ink: not scored.
    third_pixels: Vec<(i32, i32)>,
    floor: Floor,
    render: RenderModel,
    faces: Vec<FillModel>,
    edge_faces: Vec<(u16, u16)>,
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
    width: usize,
    claims: &'a [windows::Claim],
    window_var: f64,
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
    for &i in mine {
        let m = axis.unmix(at.rgb[i]);
        if m.third {
            out.third.push(((i % w) as i32, (i / w) as i32));
            taken[i] = true;
            continue;
        }
        let c = at.claims[i];
        px.push(RunPixel {
            x: (i % w) as i32,
            y: (i / w) as i32,
            m,
            s: c.s,
            t: c.t,
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
    let grouped = windows::group(&px, &mut flank, at.window_var);
    // A window the starting geometry does not cross once (a corner turns inside it, or the
    // edge ends there) cannot be read as a run: its pixels go to a corner term, scored per
    // pixel, and the place is proposed as a corner.
    let mut poly = e.points.clone();
    if e.closed && !poly.is_empty() {
        poly.push(poly[0]);
    }
    let mut pending: Option<Vec<CornerPixel>> = None;
    for (o, p) in grouped {
        if checks::polyline_left_area(&poly, &o.window).is_some() {
            if let Some(pix) = pending.take() {
                out.corner_terms.push((out.runs.len(), pix));
            }
            out.runs.push(o);
            out.run_pixels.push(p);
        } else {
            let term = pending.get_or_insert_with(Vec::new);
            for (x, y) in p {
                let m = axis.unmix(at.rgb[y as usize * w + x as usize]);
                term.push(CornerPixel {
                    x,
                    y,
                    a: m.a,
                    var: axis.var_partial,
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
    let (w, h) = (map.width, map.height);
    let floor = opts.floor.unwrap_or_else(|| Floor::lattice(32));
    let claims = windows::claims(map, opts.reach);
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
    let mut strip_pixels = Vec::new();
    for (i, c) in claims.iter().enumerate() {
        if c.edge != u32::MAX && c.edge2 != u32::MAX && !taken[i] {
            taken[i] = true;
            strip_pixels.push(((i % w) as i32, (i / w) as i32));
        }
    }

    let n_edges = map.edges.len();
    let mut runs = vec![Vec::new(); n_edges];
    let mut run_pixels = vec![Vec::new(); n_edges];
    let mut third_pixels = Vec::new();
    let mut by_edge: Vec<Vec<usize>> = vec![Vec::new(); n_edges];
    for (i, c) in claims.iter().enumerate() {
        if c.edge != u32::MAX && !taken[i] {
            by_edge[c.edge as usize].push(i);
        }
    }
    let edge_faces: Vec<(u16, u16)> = map.edges.iter().map(|e| (e.left, e.right)).collect();
    let mut corners: Vec<CornerProposal> = Vec::new();
    let mut corner_terms: Vec<Vec<CornerPixel>> = Vec::new();
    let intake = Intake {
        rgb,
        width: w,
        claims: &claims,
        window_var: floor.window_var,
    };
    for (k, e) in map.edges.iter().enumerate() {
        let (Some(FillModel::Flat(cl)), Some(FillModel::Flat(cr))) =
            (faces.get(e.left as usize), faces.get(e.right as usize))
        else {
            continue;
        };
        let axis = Axis2::new(*cl, *cr, sigma_noise);
        if axis.dd.sqrt() < (3.0 * sigma_noise).max(0.02) {
            continue;
        }
        let ev = edge_evidence(e, &axis, &by_edge[k], &intake, &mut taken);
        third_pixels.extend(ev.third);
        for (index, pix) in ev.corner_terms {
            push_corner(&mut corners, &mut corner_terms, k, index, pix);
        }
        runs[k] = ev.runs;
        run_pixels[k] = ev.run_pixels;
    }

    let mut lattice_edge = vec![false; n_edges];
    let mut lengths = vec![Vec::new(); n_edges];
    for k in 0..n_edges {
        for c in reports::corners(k as u32, &runs[k], map.edges[k].closed, opts.corner_z) {
            corners.push(c);
            corner_terms.push(Vec::new());
        }
        lattice_edge[k] = reports::on_lattice(&runs[k], floor.lattice);
        lengths[k] = reports::window_lengths(&runs[k]);
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
        third_pixels,
        floor,
        render: opts.render,
        faces: faces.to_vec(),
        edge_faces,
    }
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
