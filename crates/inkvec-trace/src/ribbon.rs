//! Stroke-drawn faces as centrelines plus one width (research prototype, `INKVEC_RIBBONS`).
//!
//! **The problem.** Line art (lucide entirely, openmoji's black outlines, much of every
//! icon set) is drawn as centrelines `C` with one stroke width `w` and round caps and
//! joins. Traced as filled outlines, every stroke costs both of its sides plus its caps:
//! on lucide the artist's own geometry written as outlines needs 3.87x the artist's
//! parameters, and the fitter already sits within 6% of that floor (r2-compact research,
//! 2026-10-02). Lucide is 63.5% of all parameters above 1.5x the artist on the gate's
//! screen set. No fitting change reaches it; only a change of representation does.
//!
//! **The decision this module serves.** For one face of the planar map, find the stroke
//! description -- centrelines and one width -- that best explains the face's measured
//! boundary, and report its fit (chi-squared) and its size (parameters). The caller
//! (`inkvec-cli`'s `ribbons`) compares that against the face's fitted outline by
//! description length and writes `<path fill="none" stroke=…>` where the stroke is cheaper.
//!
//! **The passes**, for one face ([`fit_face`]):
//!
//! 1. **Boundary** ([`boundary`]): the face's rings after the boundary solve, with inward
//!    normals and a segment grid ([`grid`]).
//! 2. **Width by pairing** ([`boundary`]): every boundary point walks along its inward
//!    normal to the other side; anti-parallel hits are width samples, and the median's
//!    core gives `w0` and the share of the outline that is sleeve. Faces where under
//!    [`MIN_PAIRED_SHARE`] of the outline pairs at one width, or narrower than
//!    [`MIN_WIDTH`], are declined here.
//! 3. **Topology** ([`graph`]): skeleton for topology, centre samples from the boundary
//!    for geometry, junctions and caps rebuilt from the sleeves, loops closed and
//!    sleeves continued through junctions; out come polylines of centre samples with
//!    sigmas.
//! 4. **Fit** ([`fit_chain`]): each polyline goes through the same MDL curve fitter the
//!    outlines use (`inkvec_fit::multimodel`), and a closed one may become a whole
//!    primitive (circle, ellipse, rounded rectangle) when that is cheaper, exactly as a
//!    closed outline may. Two exact prunings follow: fitted centrelines that already cost
//!    the caller's parameter budget cannot win (the next pass never removes parameters),
//!    and a residual above [`MAX_PRE_RMS`] of the width is a blob, not a stroke.
//! 5. **Stroke solve** ([`refine`]): every centreline control point and the width moved
//!    together, by Levenberg-Marquardt, to explain the measured boundary, with segments
//!    split where the boundary disagrees and the split pays for itself, and no step that
//!    gives a cubic a cusp (`INKVEC_RIBBONS_SOLVE=0` skips the pass, for A/B).
//! 6. **Score** ([`score`]): the strokes' painted outline against every measured boundary
//!    point, and the face's own interior pixels checked as painted ([`MAX_UNCOVERED`]),
//!    because a boundary residual cannot see an unpainted inside.
//!
//! Passes 3-6 run once with round joins and, when that leaves a boundary point more than
//! [`MITER_TRIGGER`] px out, once more with miter joins ([`join`]: corners rebuilt as
//! vertices, corner spurs dropped); the cheaper description by `χ²/2 + λ·k` is returned.
//!
//! **Measured** (screen set, 128 px, against v0.2.4, 2026-10-03, with the caller's
//! decision): on the 40 lucide icons dE00 0.076 -> 0.027 and the parameter ratio 4.19 ->
//! 1.26; the 35 openmoji 0.150 -> 0.128 and 1.40 -> 1.15. The tracer's report and
//! `INKVEC_RIBBONS_DEBUG` give the per-face numbers.
//!
//! **Data layout.** All coordinates are the traced raster's pixels with pixel centres at
//! integers. The face mask is a crop of the label map ([`FaceMask`]) with a one-pixel
//! empty margin, row-major.
//!
//! **Where it sits.** Quality mode only, after the boundary fits, the crossing repair and
//! the mirrors, before the emitter; off unless `INKVEC_RIBBONS=1`, so the shipped
//! pipeline never calls it.
//!
//! The literature each pass stands on is cited in that pass's module.

mod boundary;
mod bvh;
mod dist;
mod graph;
mod grid;
mod join;
mod refine;
mod score;
mod skyline;

pub use join::Join;
pub use score::Score;

use inkvec_core::{Point, Polyline};
use inkvec_fit::curves::Segment;
use inkvec_fit::primitives::{fit_primitive_or_arcs, PrimitiveFit};
use inkvec_fit::{multimodel, FitConfig, FittedPath};
use rayon::prelude::*;

/// Narrowest stroke considered, px. Below about two pixels a stroke has no interior
/// pixel of its own, the boundary solve drops its pixels as touched by both sides, and
/// its two sides are not separately measured; that case is the boundary solve's ribbon
/// parametrisation (r2-fidelity P2), not this.
pub const MIN_WIDTH: f64 = 2.0;

/// Share of a face's boundary points that must pair at one width before the face is
/// treated as stroke-drawn. Lucide outlines pair at 81.5% in the r2 research (caps and
/// junctions make up the rest); a filled blob pairs only at its own size, and a disc not
/// at all. It only saves work: everything it lets through is judged by its chi-squared.
/// At 0.5 it refused lucide's `navigation-2-off` arrow (0.45: sharp tips and a crossing
/// line), so it sits lower.
pub const MIN_PAIRED_SHARE: f64 = 0.35;

/// One face's pixels, cropped to its bounding box plus a one-pixel empty margin.
#[derive(Debug, Clone)]
pub struct FaceMask {
    /// Image column of the crop's first column.
    pub x0: isize,
    /// Image row of the crop's first row.
    pub y0: isize,
    /// Crop width, px.
    pub w: usize,
    /// Crop height, px.
    pub h: usize,
    /// Per crop pixel, row-major: in the face.
    pub on: Vec<bool>,
    /// The whole canvas's width and height, px: its frame cuts faces that run off it.
    pub canvas: (usize, usize),
}

impl FaceMask {
    /// The mask of label `face` in `labels` (row-major, `img_w` wide), cropped to the
    /// inclusive pixel box `(x0, y0, x1, y1)` plus a one-pixel margin.
    pub fn from_labels(
        labels: &[u16],
        img_w: usize,
        face: u16,
        bbox: (usize, usize, usize, usize),
    ) -> FaceMask {
        let (x0, y0, x1, y1) = bbox;
        let (w, h) = (x1 - x0 + 3, y1 - y0 + 3);
        let mut on = vec![false; w * h];
        for y in y0..=y1 {
            for x in x0..=x1 {
                if labels[y * img_w + x] == face {
                    on[(y - y0 + 1) * w + (x - x0 + 1)] = true;
                }
            }
        }
        FaceMask {
            x0: x0 as isize - 1,
            y0: y0 as isize - 1,
            w,
            h,
            on,
            canvas: (img_w, labels.len() / img_w.max(1)),
        }
    }

    /// Every label's inclusive pixel bounding box, in one pass over `labels`
    /// (`img_w` x `img_h`); `None` for a label with no pixels or outside `0..n_labels`.
    #[allow(clippy::type_complexity)]
    pub fn bounding_boxes(
        labels: &[u16],
        img_w: usize,
        img_h: usize,
        n_labels: usize,
    ) -> Vec<Option<(usize, usize, usize, usize)>> {
        let mut bb: Vec<Option<(usize, usize, usize, usize)>> = vec![None; n_labels];
        for y in 0..img_h {
            for x in 0..img_w {
                let l = labels[y * img_w + x] as usize;
                if l >= n_labels {
                    continue;
                }
                bb[l] = Some(match bb[l] {
                    None => (x, y, x, y),
                    Some((a, b, c, d)) => (a.min(x), b.min(y), c.max(x), d.max(y)),
                });
            }
        }
        bb
    }

    /// Pixels in the face.
    pub fn area(&self) -> usize {
        self.on.iter().filter(|&&v| v).count()
    }

    /// Image position of crop pixel index `p` (its centre).
    fn point_of(&self, p: usize) -> Point {
        Point::new(
            (self.x0 + (p % self.w) as isize) as f64,
            (self.y0 + (p / self.w) as isize) as f64,
        )
    }

    /// Whether crop pixel `(cx, cy)` (crop coordinates, may be out of range) is in the face.
    fn at(&self, cx: isize, cy: isize) -> bool {
        cx >= 0
            && cy >= 0
            && (cx as usize) < self.w
            && (cy as usize) < self.h
            && self.on[cy as usize * self.w + cx as usize]
    }

    /// Whether image point `q` falls in a face pixel (the pixel whose centre is nearest).
    fn contains(&self, q: Point) -> bool {
        self.at(
            q.x.round() as isize - self.x0,
            q.y.round() as isize - self.y0,
        )
    }

    /// Whether image point `q` is in a face pixel or one of its eight neighbours.
    fn near(&self, q: Point) -> bool {
        let (cx, cy) = (
            q.x.round() as isize - self.x0,
            q.y.round() as isize - self.y0,
        );
        (-1..=1).any(|dy| (-1..=1).any(|dx| self.at(cx + dx, cy + dy)))
    }
}

/// One fitted centreline: a path, or a whole primitive when that described it more
/// cheaply (the path then holds the primitive's own segments).
#[derive(Debug, Clone)]
pub struct Centreline {
    /// The centreline's geometry, px.
    pub path: FittedPath,
    /// The primitive it is, when one won.
    pub prim: Option<PrimitiveFit>,
}

impl Centreline {
    /// Parameters it costs in the fitter's own currency: the primitive's when it is one,
    /// else the path's (start point plus each segment).
    pub fn params(&self) -> f64 {
        self.prim
            .as_ref()
            .map_or_else(|| self.path.params(), |p| p.params)
    }
}

/// A face described as strokes.
#[derive(Debug, Clone)]
pub struct Ribbon {
    /// The centrelines.
    pub lines: Vec<Centreline>,
    /// The stroke width to write: twice [`Score::half`], px.
    pub width: f64,
    /// How the strokes' segments meet (`stroke-linejoin`).
    pub join: Join,
    /// The width the pairing read, px.
    pub paired_width: f64,
    /// Share of the boundary points whose pair agreed with it.
    pub paired_share: f64,
    /// How well the strokes explain the face's boundary.
    pub score: Score,
    /// Root-mean-square boundary residual before the stroke solve, px (diagnostic).
    pub pre_rms: f64,
    /// Interior pixels of the face the strokes leave unpainted ([`MAX_UNCOVERED`]).
    pub uncovered: usize,
    /// Boundary points scored.
    pub points: usize,
    /// Junctions rebuilt.
    pub junctions: usize,
    /// Round caps rebuilt.
    pub caps: usize,
    /// Skeleton branches dropped.
    pub dropped: usize,
}

impl Ribbon {
    /// Parameters of the stroke description: every centreline's, plus one for the width.
    pub fn params(&self) -> f64 {
        self.lines.iter().map(Centreline::params).sum::<f64>() + 1.0
    }

    /// Its description length in nats, `χ²/2 + λ·k`, at `lambda` nats per parameter.
    pub fn cost(&self, lambda: f64) -> f64 {
        0.5 * self.score.chi2 + lambda * self.params()
    }
}

/// Why a face was not described as strokes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decline {
    /// No ring with three distinct points.
    NoBoundary,
    /// Under [`MIN_PAIRED_SHARE`] of the boundary paired at one width.
    Unpaired {
        /// The share that did.
        share: f64,
    },
    /// The paired width is under [`MIN_WIDTH`] px.
    TooThin {
        /// The paired width, px.
        width: f64,
    },
    /// The skeleton gave no centreline with a reliable core.
    NoCentreline,
    /// The fitted centrelines already cost the caller's budget before any solve.
    NoGain {
        /// Their parameters.
        params: f64,
    },
    /// The fitted centrelines are far from the boundary before any solve.
    Misfit {
        /// Root-mean-square residual, px.
        rms: f64,
    },
    /// The strokes leave interior pixels of the face unpainted.
    Uncovered {
        /// How many.
        pixels: usize,
    },
    /// The solved strokes have a cusp, or (miter joins) a micro-segment or a hairpin a
    /// renderer would not draw as modelled.
    Undrawable,
}

/// Describe the face with boundary `rings` and pixel `mask` as strokes, fitting each
/// centreline under `cfg`. See the module documentation for the passes.
///
/// `rings` are the face's closed boundary walks (any orientation), points with per-point
/// sigma, px. `budget` is the parameter count the strokes must stay under to be worth
/// anything (the caller's: what the outline costs); a hypothesis whose fitted
/// centrelines already reach it before the solve is abandoned, since the solve never
/// removes parameters and its splits only add them. Returns the stroke description with
/// its score; whether it is *better* than the outline is the caller's decision.
pub fn fit_face(
    rings: &[Polyline],
    mask: &FaceMask,
    cfg: &FitConfig,
    budget: f64,
) -> Result<Ribbon, Decline> {
    let mut b =
        boundary::Boundary::new(rings, &|p| mask.contains(p), 2.0).ok_or(Decline::NoBoundary)?;
    b.mark_frame(mask.canvas.0, mask.canvas.1);
    let reach = mask.w.max(mask.h) as f64;
    let (w0, share) =
        boundary::stroke_width(&b.pair(reach)).ok_or(Decline::Unpaired { share: 0.0 })?;
    if share < MIN_PAIRED_SHARE {
        return Err(Decline::Unpaired { share });
    }
    if w0 < MIN_WIDTH {
        return Err(Decline::TooThin { width: w0 });
    }
    let face = Face {
        medial: graph::medial(&b, mask, w0),
        b: &b,
        mask,
        w0,
        share,
    };
    let round = hypothesis(&face, cfg, budget, Join::Round);
    // A round fit that leaves a boundary point more than a quarter pixel out is also tried
    // with miter joins: sharp corners are what round joins cannot draw.
    let try_miter = round
        .as_ref()
        .map_or(true, |r| r.score.worst > MITER_TRIGGER);
    if !try_miter || !inkvec_core::env::switch("INKVEC_RIBBONS_MITER", true) {
        return round;
    }
    let miter = hypothesis(&face, cfg, budget, Join::Miter);
    match (round, miter) {
        (Ok(r), Ok(m)) => Ok(if m.cost(cfg.lambda) < r.cost(cfg.lambda) {
            m
        } else {
            r
        }),
        (Ok(r), Err(_)) => Ok(r),
        (Err(_), m) => m,
    }
}

/// Worst boundary residual of the round fit, px, above which the miter hypothesis is
/// tried as well. Accepted lucide faces (all round) sit at 0.05-0.3 px after the solve.
const MITER_TRIGGER: f64 = 0.25;

/// Share of a face's pixels (plus two) its strokes may leave unpainted.
///
/// The boundary residual sees paint where the face is not (boundary points inside the
/// strokes) and face beyond the paint *at the boundary*, but not an unpainted interior: a
/// stroke run along the inside of a blob's outline puts every boundary point at the right
/// distance and paints a ring. So the face's own pixels are checked as well -- every pixel
/// centre at least a pixel inside the boundary (clear of the anti-aliased rim the label map
/// draws either way) must lie within the half-width plus half a pixel of a centreline.
const MAX_UNCOVERED: f64 = 0.002;

/// Interior pixels of the face (centres at least 1 px inside its measured boundary) that
/// the strokes leave unpainted: farther than `h + 0.5` px from every centreline under
/// `join`. At most every second pixel in each direction is tested on faces of over 20000
/// pixels (the count is scaled back up), which keeps a 512 px face's test to a few
/// thousand nearest-segment queries.
fn uncovered(
    b: &boundary::Boundary,
    mask: &FaceMask,
    lines: &[Centreline],
    h: f64,
    join: Join,
) -> f64 {
    let stride = if mask.area() > 20_000 { 2 } else { 1 };
    let mut pts = Vec::new();
    for y in (0..mask.h).step_by(stride) {
        for x in (0..mask.w).step_by(stride) {
            if !mask.on[y * mask.w + x] {
                continue;
            }
            let p = mask.point_of(y * mask.w + x);
            if b.grid.nearest(p, 1.0).is_none() {
                pts.push(p);
            }
        }
    }
    let d = refine::point_distances(lines, &pts, h + 0.5, join);
    d.iter().filter(|&&x| x > h + 0.5).count() as f64 * (stride * stride) as f64
}

/// Largest root-mean-square residual, as a fraction of the paired width, a hypothesis
/// may have *before* the stroke solve and still be solved. Lucide faces whose topology
/// is right start at 0.01-0.02 of the width (rms 0.1-0.2 px at 10.7 px); a blob read as
/// a stroke starts at a quarter or more.
const MAX_PRE_RMS: f64 = 0.15;

/// One face as every hypothesis and reading of it sees it.
struct Face<'a> {
    /// Its measured boundary.
    b: &'a boundary::Boundary,
    /// Its pixels.
    mask: &'a FaceMask,
    /// Its medial graph and centre samples, read once and shared by every reading.
    medial: graph::Medial,
    /// The paired stroke width, px.
    w0: f64,
    /// The share of the boundary that paired at it.
    share: f64,
}

/// One hypothesis of a face's strokes: topology read with `join`'s rules
/// ([`graph::TopoOptions`]), every chain fitted ([`fit_chain`]), the stroke solve with
/// splits ([`refine::solve_adaptive`]), and the score under that join.
fn hypothesis(face: &Face, cfg: &FitConfig, budget: f64, join: Join) -> Result<Ribbon, Decline> {
    let w0 = face.w0;
    // Topology readings tried in turn: round joins first without rebuilt corners (most
    // tight turns in round line art are arcs of about the half-width), then with them,
    // for drawings whose sharp corners leave a gap of samples long against the arms
    // (lucide `circle-arrow-right` at 512 px: w 42.7 px, a chevron of 85 px arms, read
    // tip to tip without its apex at rms 39 px).
    let readings = match join {
        Join::Round => vec![
            graph::TopoOptions::round(),
            graph::TopoOptions::round_cornered(),
        ],
        Join::Miter => vec![graph::TopoOptions::miter(w0)],
    };
    let mut last = Err(Decline::NoCentreline);
    for (i, &opts) in readings.iter().enumerate() {
        last = reading(face, cfg, budget, join, opts);
        let retry = matches!(last, Err(Decline::Misfit { .. })) && i + 1 < readings.len();
        if !retry {
            break;
        }
    }
    last
}

/// One topology reading of a face under `join` (see [`hypothesis`]): the passes 3-6 of
/// the module documentation.
fn reading(
    face: &Face,
    cfg: &FitConfig,
    budget: f64,
    join: Join,
    opts: graph::TopoOptions,
) -> Result<Ribbon, Decline> {
    let (b, mask, w0, share) = (face.b, face.mask, face.w0, face.share);
    let topo = graph::centrelines(&face.medial, b, mask, w0, opts);
    if inkvec_core::env::flag("INKVEC_RIBBONS_CHAINS") {
        for c in &topo.chains {
            let (a, z) = (c.pts[0], c.pts[c.pts.len() - 1]);
            eprintln!(
                "    {join:?} chain {} pts closed {} from ({:.1},{:.1}) to ({:.1},{:.1})",
                c.pts.len(),
                c.closed,
                a.x,
                a.y,
                z.x,
                z.y
            );
        }
    }
    // Each chain is fitted on its own, so the chains are fitted in parallel; `collect`
    // keeps their order, so the result is the sequential one.
    let lines: Vec<Centreline> = topo
        .chains
        .par_iter()
        .filter(|c| c.pts.len() >= 2)
        .map(|c| {
            fit_chain(
                &Polyline::new(c.pts.clone(), c.sigma.clone(), c.closed && c.pts.len() >= 3),
                cfg,
            )
        })
        .filter(|c| !c.path.segments.is_empty())
        .collect();
    if lines.is_empty() {
        return Err(Decline::NoCentreline);
    }
    let k = lines.iter().map(Centreline::params).sum::<f64>() + 1.0;
    if k >= budget {
        return Err(Decline::NoGain { params: k });
    }
    // A stroke reading of a blob is off by a sizeable fraction of its width almost
    // everywhere, before any solve; the solve polishes tenths of a pixel, not that.
    let pre = score::score(b, &lines, w0, None, join);
    if pre.rms > MAX_PRE_RMS * w0 {
        return Err(Decline::Misfit { rms: pre.rms });
    }
    let (lines, half) = if inkvec_core::env::switch("INKVEC_RIBBONS_SOLVE", true) {
        let (l, h) = refine::solve_adaptive(&lines, 0.5 * w0, b, cfg.lambda, join, budget);
        (l, Some(h))
    } else {
        (lines, None)
    };
    if !refine::drawable(&lines, join) {
        return Err(Decline::Undrawable);
    }
    let score = score::score(b, &lines, w0, half, join);
    let uncovered = uncovered(b, mask, &lines, score.half, join);
    if uncovered > MAX_UNCOVERED * mask.area() as f64 + 2.0 {
        return Err(Decline::Uncovered {
            pixels: uncovered as usize,
        });
    }
    Ok(Ribbon {
        lines,
        width: 2.0 * score.half,
        join,
        paired_width: w0,
        paired_share: share,
        score,
        pre_rms: pre.rms,
        uncovered: uncovered as usize,
        points: b.len(),
        junctions: topo.junctions,
        caps: topo.caps,
        dropped: topo.dropped,
    })
}

/// The chi-squared of measured boundary points against a fitted path by exact nearest
/// distance (see `score::path_chi2`): the outline's side of the comparison the caller
/// makes, measured the same way as the strokes' [`Score::chi2`].
pub fn path_chi2(points: &[Point], sigma: &[f64], path: &FittedPath) -> f64 {
    score::path_chi2(points, sigma, path)
}

/// One centre polyline fitted by the outline fitter's MDL dynamic program, or, when it is
/// closed, by a whole primitive when that costs less: `χ²/2 + λ·k` for both, the same
/// comparison the pipeline makes for a closed outline (`prefer_primitive`).
///
/// A closed chain's curve is closed exactly: the dynamic program opens a loop at a cut
/// vertex and can return its two copies of that vertex apart (1.8 px on lucide's
/// `pen-line`), which the outline writer hides behind a `Z` but a stroke would show as two
/// caps with a notch between them. The last segment's end is moved onto the start; the
/// stroke solve then refits the geometry round it.
fn fit_chain(poly: &Polyline, cfg: &FitConfig) -> Centreline {
    let mut curve = multimodel::optimal_multimodel(poly, cfg);
    if poly.closed {
        curve.closed = true;
        let start = curve.start;
        if let Some(last) = curve.segments.last_mut() {
            match last {
                Segment::Line(p) | Segment::Cubic(_, _, p) | Segment::Arc { end: p, .. } => {
                    *p = start
                }
            }
        }
        let path_cost = 0.5
            * inkvec_fit::curves::chi2(&poly.points, &poly.sigma, curve.start, &curve.segments)
            + cfg.lambda * curve.params();
        if let Some((segs, prim, cost)) =
            fit_primitive_or_arcs(&poly.points, &poly.sigma, true, cfg)
        {
            if cost < path_cost {
                return Centreline {
                    path: FittedPath {
                        start: poly.points[0],
                        segments: segs,
                        closed: true,
                    },
                    prim,
                };
            }
        }
    }
    Centreline {
        path: curve,
        prim: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rasterised round-capped straight stroke along `centre` (its first and last
    /// points), half-width `h`, on an `n` x `n` image: its label map (1 = ink, by pixel
    /// centre) and its exact outline as one dense ring.
    fn stroke_face(centre: &[Point], h: f64, n: usize) -> (Vec<u16>, Vec<Polyline>) {
        let dist = |p: Point| -> f64 {
            centre
                .windows(2)
                .map(|s| grid::point_segment(p, s[0], s[1]).0)
                .fold(f64::INFINITY, f64::min)
        };
        let labels: Vec<u16> = (0..n * n)
            .map(|i| u16::from(dist(Point::new((i % n) as f64, (i / n) as f64)) <= h))
            .collect();
        // The outline of a straight stroke from `a` to `b`: one side, the cap round `b`,
        // the other side, the cap round `a`, every ~0.25 px.
        let (a, b) = (centre[0], centre[centre.len() - 1]);
        let len = a.dist(b);
        let (ux, uy) = ((b.x - a.x) / len, (b.y - a.y) / len);
        let at = |o: Point, c: f64, s: f64| {
            Point::new(o.x + h * (c * ux - s * uy), o.y + h * (c * uy + s * ux))
        };
        let mut pts: Vec<Point> = Vec::new();
        let ns = (len / 0.25) as usize;
        let nc = 60;
        for i in 0..ns {
            let f = i as f64 / ns as f64;
            pts.push(at(
                Point::new(a.x + f * (b.x - a.x), a.y + f * (b.y - a.y)),
                0.0,
                -1.0,
            ));
        }
        for j in 0..nc {
            let t = -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * j as f64 / nc as f64;
            pts.push(at(b, t.cos(), t.sin()));
        }
        for i in 0..ns {
            let f = i as f64 / ns as f64;
            pts.push(at(
                Point::new(b.x + f * (a.x - b.x), b.y + f * (a.y - b.y)),
                0.0,
                1.0,
            ));
        }
        for j in 0..nc {
            let t = std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * j as f64 / nc as f64;
            pts.push(at(a, t.cos(), t.sin()));
        }
        let ring = Polyline::with_uniform_sigma(pts, 0.05, true);
        (labels, vec![ring])
    }

    #[test]
    fn a_straight_stroke_comes_back_as_one_line_and_its_width() {
        let centre = [Point::new(12.0, 30.0), Point::new(48.0, 30.0)];
        let (labels, rings) = stroke_face(&centre, 4.0, 64);
        let bb = FaceMask::bounding_boxes(&labels, 64, 64, 2);
        let mask = FaceMask::from_labels(&labels, 64, 1, bb[1].expect("ink"));
        let cfg = FitConfig::from_precision(64.0, 0.1, 2.0);
        let r = fit_face(&rings, &mask, &cfg, f64::INFINITY).expect("a stroke");
        assert!((r.width - 8.0).abs() < 0.05, "width {}", r.width);
        assert_eq!(r.lines.len(), 1);
        let p = &r.lines[0].path;
        let (s, e) = (p.start, p.end());
        let (l, rr) = if s.x < e.x { (s, e) } else { (e, s) };
        assert!(
            l.dist(centre[0]) < 0.3 && rr.dist(centre[1]) < 0.3,
            "{s:?} {e:?}"
        );
        assert!(r.score.worst < 0.3, "{:?}", r.score);
    }

    #[test]
    fn a_disc_is_declined() {
        let n = 40;
        let labels: Vec<u16> = (0..n * n)
            .map(|i| {
                u16::from(
                    Point::new((i % n) as f64, (i / n) as f64).dist(Point::new(20.0, 20.0)) <= 12.0,
                )
            })
            .collect();
        let ring = Polyline::with_uniform_sigma(
            (0..400)
                .map(|k| {
                    let t = std::f64::consts::TAU * k as f64 / 400.0;
                    Point::new(20.0 + 12.0 * t.cos(), 20.0 + 12.0 * t.sin())
                })
                .collect(),
            0.05,
            true,
        );
        let bb = FaceMask::bounding_boxes(&labels, n, n, 2);
        let mask = FaceMask::from_labels(&labels, n, 1, bb[1].expect("ink"));
        let cfg = FitConfig::from_precision(40.0, 0.1, 2.0);
        assert!(fit_face(&[ring], &mask, &cfg, f64::INFINITY).is_err());
    }

    #[test]
    fn mask_crops_with_a_margin() {
        let labels = vec![0u16, 0, 0, 0, 1, 1, 0, 0, 0];
        let bb = FaceMask::bounding_boxes(&labels, 3, 3, 2);
        assert_eq!(bb[1], Some((1, 1, 2, 1)));
        let m = FaceMask::from_labels(&labels, 3, 1, bb[1].expect("box"));
        assert_eq!((m.w, m.h, m.x0, m.y0), (4, 3, 0, 0));
        assert_eq!(m.area(), 2);
        assert!(m.contains(Point::new(1.2, 0.9)) && !m.contains(Point::new(0.0, 0.0)));
        assert!(m.near(Point::new(0.0, 0.0)));
    }

    #[test]
    fn a_blob_painted_as_a_ring_is_uncovered() {
        // A 30 x 30 square face, and a closed square centreline 3 px inside its edge with
        // half-width 3: the boundary is explained exactly, the middle is not painted.
        let n = 40;
        let labels: Vec<u16> = (0..n * n)
            .map(|i| u16::from((5..35).contains(&(i % n)) && (5..35).contains(&(i / n))))
            .collect();
        let bb = FaceMask::bounding_boxes(&labels, n, n, 2);
        let mask = FaceMask::from_labels(&labels, n, 1, bb[1].expect("ink"));
        let c = [(4.5, 4.5), (34.5, 4.5), (34.5, 34.5), (4.5, 34.5)];
        let mut pts = Vec::new();
        for k in 0..4 {
            let (a, b) = (c[k], c[(k + 1) & 3]);
            for i in 0..120 {
                let t = i as f64 / 120.0;
                pts.push(Point::new(a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1)));
            }
        }
        let ring = Polyline::with_uniform_sigma(pts, 0.05, true);
        let b = boundary::Boundary::new(&[ring], &|p| mask.contains(p), 2.0).expect("a ring");
        let sq = |o: f64| FittedPath {
            start: Point::new(4.5 + o, 4.5 + o),
            segments: vec![
                Segment::Line(Point::new(34.5 - o, 4.5 + o)),
                Segment::Line(Point::new(34.5 - o, 34.5 - o)),
                Segment::Line(Point::new(4.5 + o, 34.5 - o)),
                Segment::Line(Point::new(4.5 + o, 4.5 + o)),
            ],
            closed: true,
        };
        let ring_stroke = [Centreline {
            path: sq(3.0),
            prim: None,
        }];
        let gap = uncovered(&b, &mask, &ring_stroke, 3.0, Join::Miter);
        assert!(gap > 100.0, "{gap} pixels unpainted");
        // One horizontal stroke as wide as the square paints all of it.
        let fat = [Centreline {
            path: FittedPath {
                start: Point::new(5.0, 19.5),
                segments: vec![Segment::Line(Point::new(34.0, 19.5))],
                closed: false,
            },
            prim: None,
        }];
        assert_eq!(uncovered(&b, &mask, &fat, 15.5, Join::Miter), 0.0);
    }
}
