//! Planar map with genuinely shared edges (DESIGN.md S3, §4).
//!
//! This is the structural claim of the whole project. A tracer that stores every region
//! as an independent closed path has to choose between two failures, and cannot avoid
//! both: lay regions edge-to-edge and rounding disagreement between the two copies of
//! each shared boundary opens hairline **seams**; overlap them and geometry is drawn
//! twice — **overdraw** — so every shared boundary exists in two places and editing it
//! means editing both. `M0-BASELINE.md` §4 measures VTracer sitting on that trade at
//! overdraw 1.64 where ground truth is 1.00, with seams that grow 2.4x from 1x to 4x zoom.
//!
//! Here a boundary between two regions is stored **once**. Both faces reference the same
//! edge, so seams are not merely rare, they are unrepresentable, and overdraw is 1.0 by
//! construction rather than by tuning.
//!
//! Topology is taken from the integer label map and is therefore exact — junctions are
//! grid nodes, and two faces meeting at one refer to the same integer id. Geometry is
//! then refined to sub-pixel positions *afterwards*, which is what lets accuracy and
//! exactness coexist: moving a vertex cannot change who is adjacent to whom.

use std::collections::HashMap;

use inkvec_core::{Point, Polyline};

/// One boundary curve of the map, running between two junction nodes.
///
/// Exactly two faces touch it. Traversed in stored order, `left` lies to the left and
/// `right` to the right.
#[derive(Debug, Clone)]
pub struct Edge {
    /// Sub-pixel points along the curve.
    pub points: Vec<Point>,
    /// Positional uncertainty at each point, in pixels, parallel to `points`.
    pub sigma: Vec<f64>,
    /// Face id lying to the left, traversed in stored order.
    pub left: u16,
    /// Face id lying to the right, traversed in stored order.
    pub right: u16,
    /// Node the curve starts at.
    pub start_node: u32,
    /// Node the curve ends at.
    pub end_node: u32,
    /// True when the curve closes on itself with no junction (an isolated region
    /// boundary, such as a disc sitting alone on a background).
    pub closed: bool,
    /// Multiplier on the MDL cost of a parameter for *this* boundary, relative to the
    /// drawing's global `lambda`.
    ///
    /// One global exchange rate between fidelity and description length prices a
    /// hand-drawn flourish and a plain straight run identically. Below 1.0 a parameter is
    /// cheap here and the fitter spends detail; above 1.0 it is dear and the boundary is
    /// described more plainly. `1.0` is the neutral value and leaves the fit exactly as it
    /// was, which is what every stage of the shipped pipeline sets; only the research
    /// guidance path moves it.
    pub lambda_scale: f64,
}

impl Edge {
    /// This edge's geometry as a measured boundary the fitter can consume.
    pub fn as_polyline(&self) -> Polyline {
        Polyline::new(self.points.clone(), self.sigma.clone(), self.closed)
    }
}

/// A planar subdivision of the image into faces, with each shared boundary stored once
/// as an [`Edge`] referenced by both adjacent faces. See the module docs.
#[derive(Debug, Default, Clone)]
pub struct PlanarMap {
    /// The map's boundary curves.
    pub edges: Vec<Edge>,
    /// Image width in pixels.
    pub width: usize,
    /// Image height in pixels.
    pub height: usize,
    /// Number of distinct face labels.
    pub n_labels: usize,
}

pub(crate) mod junctions;
pub use junctions::{node_position, refine_junctions};

/// Grid node index. Nodes sit at pixel corners: node `(i, j)` is at image coordinate
/// `(i - 0.5, j - 0.5)`, with `i` in `0..=w` and `j` in `0..=h`.
#[inline]
fn node_id(i: usize, j: usize, w: usize) -> u32 {
    u32::try_from(j * (w + 1) + i)
        .expect("planar node id exceeds u32; the raster is too large to subdivide")
}

#[inline]
pub(crate) fn node_point(i: usize, j: usize) -> Point {
    Point::new(i as f64 - 0.5, j as f64 - 0.5)
}

/// Label of pixel `(x, y)`, with out-of-bounds treated as a distinct virtual background
/// so that shapes touching the image border still close.
#[inline]
fn label_at(labels: &[u16], w: usize, h: usize, x: isize, y: isize) -> u16 {
    if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
        u16::MAX
    } else {
        labels[y as usize * w + x as usize]
    }
}

/// A boundary segment on the dual grid, separating two pixels of differing label.
#[derive(Clone, Copy)]
struct Seg {
    a: u32,
    b: u32,
    left: u16,
    right: u16,
}

/// Build the planar map from an integer label image.
pub fn build(labels: &[u16], w: usize, h: usize, n_labels: usize) -> PlanarMap {
    let mut segs: Vec<Seg> = Vec::new();

    // Vertical dual edges: node (i, j) -> (i, j+1) separates pixel (i-1, j) from (i, j).
    for j in 0..h {
        for i in 0..=w {
            let l = label_at(labels, w, h, i as isize - 1, j as isize);
            let r = label_at(labels, w, h, i as isize, j as isize);
            if l != r {
                // Walking downward (+y), the pixel on the left in screen terms is (i, j).
                segs.push(Seg {
                    a: node_id(i, j, w),
                    b: node_id(i, j + 1, w),
                    left: r,
                    right: l,
                });
            }
        }
    }
    // Horizontal dual edges: node (i, j) -> (i+1, j) separates pixel (i, j-1) from (i, j).
    for j in 0..=h {
        for i in 0..w {
            let u = label_at(labels, w, h, i as isize, j as isize - 1);
            let d = label_at(labels, w, h, i as isize, j as isize);
            if u != d {
                // Walking rightward (+x), the pixel above is on the left.
                segs.push(Seg {
                    a: node_id(i, j, w),
                    b: node_id(i + 1, j, w),
                    left: u,
                    right: d,
                });
            }
        }
    }

    // Real nodes are `0..(w+1)*(h+1)`; a node split below gets a second id one whole
    // grid further on, which `node_position` folds back to the same place.
    let plane = u32::try_from((w + 1) * (h + 1))
        .expect("planar node count exceeds u32; the raster is too large to subdivide");

    // Where four pixels meet at a corner and one diagonal is a single face, give the
    // other two their own copy of that corner.
    //
    // Such a node is degree 4, so the walk below stops there — once for each shape that
    // touches it. All four then hold the one point, and the two on the cut diagonal are
    // welded at it: each draws a boundary that runs to a spike and turns back, so the
    // fill pinches to nothing and reopens. That is the bowtie, and no later stage can
    // undo it, because one node has one refined position however many curves end there.
    //
    // Which segment belongs to which shape is not a guess. Each dual edge separates a
    // known pair of the four pixels: `above` runs between NW and NE, `left` between NW
    // and SW, `right` between NE and SE, `below` between SW and SE. When NE and SW are
    // one face, {above, left} is the whole of NW's boundary here and {right, below} the
    // whole of SE's, so handing SE's pair a copy of the corner leaves each passing
    // through a point of its own. Both start at the same place, because the pixels do
    // meet there; what changes is that they are two points, each free to move to where
    // the image puts it, so the following stages open the gap the coverage says is there
    // instead of holding it shut at zero.
    //
    // Note what makes the merge legal: NE and SW being *one face* means the two segments
    // joined into a chain separate the same pair of faces. Chaining segments that do not
    // would hand the edge one face's identity while half of it borders another, and the
    // other face would lose that edge from its ring entirely. Which diagonal is one face
    // is settled upstream, in `merge_saddle_faces`, where the image can be consulted.
    // Here it is read off the labels, and a node whose diagonals are both one face, or
    // neither, is left alone: still a junction, exactly as before.
    {
        let mut by_node: HashMap<u32, Vec<usize>> = HashMap::new();
        for (k, s) in segs.iter().enumerate() {
            by_node.entry(s.a).or_default().push(k);
            by_node.entry(s.b).or_default().push(k);
        }
        for j in 0..=h {
            for i in 0..=w {
                let real = node_id(i, j, w);
                let Some(here) = by_node.get(&real) else {
                    continue;
                };
                if here.len() != 4 {
                    continue;
                }
                let nw = label_at(labels, w, h, i as isize - 1, j as isize - 1);
                let ne = label_at(labels, w, h, i as isize, j as isize - 1);
                let sw = label_at(labels, w, h, i as isize - 1, j as isize);
                let se = label_at(labels, w, h, i as isize, j as isize);
                if (nw == se) == (ne == sw) {
                    continue; // both diagonals one face, or neither: nothing to read.
                }
                // Name the four incident segments by where their far end lies.
                let (mut left, mut right, mut below) = (None, None, None);
                for &k in here {
                    let s = segs[k];
                    let other = if s.a == real { s.b } else { s.a };
                    let (oi, oj) = ((other as usize) % (w + 1), (other as usize) / (w + 1));
                    if oj == j && oi > i {
                        right = Some(k);
                    } else if oj == j && oi < i {
                        left = Some(k);
                    } else if oi == i && oj > j {
                        below = Some(k);
                    }
                }
                // The pair that takes the copy: SE's when NE and SW are the one face,
                // SW's when it is NW and SE.
                let moved = if ne == sw {
                    (right, below)
                } else {
                    (left, below)
                };
                let (Some(p0), Some(p1)) = moved else {
                    continue;
                };
                for k in [p0, p1] {
                    if segs[k].a == real {
                        segs[k].a += plane;
                    } else {
                        segs[k].b += plane;
                    }
                }
            }
        }
    }

    // Adjacency, and node degree.
    let mut inc: HashMap<u32, Vec<usize>> = HashMap::new();
    for (k, s) in segs.iter().enumerate() {
        inc.entry(s.a).or_default().push(k);
        inc.entry(s.b).or_default().push(k);
    }

    // A node is a junction when it is not a simple pass-through. Degree 4 (four pixels
    // meeting at a corner) is treated as a junction rather than guessed at: splitting
    // there is always topologically safe, whereas picking a diagonal pairing can weld two
    // regions that should be separate. What the split above resolved no longer arrives
    // here as degree 4; everything else still does.
    let is_junction = |n: u32| -> bool { inc.get(&n).map(|v| v.len()) != Some(2) };

    let mut used = vec![false; segs.len()];
    let mut edges: Vec<Edge> = Vec::new();

    // Walk chains that start at junctions.
    let mut junctions: Vec<u32> = inc.keys().copied().filter(|&n| is_junction(n)).collect();
    junctions.sort_unstable();

    for start in junctions {
        let Some(list) = inc.get(&start) else {
            continue;
        };
        for &first in list {
            if used[first] {
                continue;
            }
            let mut chain_nodes = vec![start];
            let (mut cur_seg, mut cur_node) = (first, start);
            let (left, right) = {
                let s = segs[first];
                if s.a == start {
                    (s.left, s.right)
                } else {
                    (s.right, s.left)
                }
            };

            loop {
                used[cur_seg] = true;
                let s = segs[cur_seg];
                let next_node = if s.a == cur_node { s.b } else { s.a };
                chain_nodes.push(next_node);
                if is_junction(next_node) {
                    cur_node = next_node;
                    break;
                }
                let Some(cands) = inc.get(&next_node) else {
                    break;
                };
                let Some(&nxt) = cands.iter().find(|&&k| k != cur_seg && !used[k]) else {
                    cur_node = next_node;
                    break;
                };
                cur_seg = nxt;
                cur_node = next_node;
            }

            let points: Vec<Point> = chain_nodes
                .iter()
                .map(|&n| node_position(n, w, h))
                .collect();
            let sigma = vec![0.5; points.len()];
            edges.push(Edge {
                points,
                sigma,
                left,
                right,
                start_node: start,
                end_node: cur_node,
                closed: false,
                lambda_scale: 1.0,
            });
        }
    }

    // Anything left is a closed loop with no junction at all.
    for k in 0..segs.len() {
        if used[k] {
            continue;
        }
        let s0 = segs[k];
        let (left, right) = (s0.left, s0.right);
        let mut chain_nodes = vec![s0.a];
        let (mut cur_seg, mut cur_node) = (k, s0.a);
        loop {
            used[cur_seg] = true;
            let s = segs[cur_seg];
            let next_node = if s.a == cur_node { s.b } else { s.a };
            if next_node == s0.a {
                break;
            }
            chain_nodes.push(next_node);
            let Some(cands) = inc.get(&next_node) else {
                break;
            };
            let Some(&nxt) = cands.iter().find(|&&x| x != cur_seg && !used[x]) else {
                break;
            };
            cur_seg = nxt;
            cur_node = next_node;
        }
        if chain_nodes.len() >= 3 {
            let points: Vec<Point> = chain_nodes
                .iter()
                .map(|&n| node_position(n, w, h))
                .collect();
            let sigma = vec![0.5; points.len()];
            edges.push(Edge {
                points,
                sigma,
                left,
                right,
                start_node: s0.a,
                end_node: s0.a,
                closed: true,
                lambda_scale: 1.0,
            });
        }
    }

    PlanarMap {
        edges,
        width: w,
        height: h,
        n_labels,
    }
}

/// Smallest colour separation (sRGB, Euclidean) a vertex is unmixed across.
const MIN_UNMIX_CONTRAST: f64 = 0.02;

/// Move every boundary vertex to its sub-pixel position, and record how well localized it
/// is.
///
/// Topology is already fixed and stays fixed: this only moves points along the local
/// boundary normal. That ordering is what lets exactness and sub-pixel accuracy coexist —
/// a vertex can be repositioned freely without any risk of changing which regions are
/// adjacent, because adjacency was decided on the integer label map.
///
/// For a vertex between regions `A` and `B`, unmixing the surrounding pixels against
/// those two colours gives a local coverage field, and the boundary is its `0.5` level.
/// Searching along the normal for that crossing is a one-dimensional root find.
///
/// `face_fill[f]` is the fill model of face `f`; the colours unmixed against are chosen
/// per vertex by [`crate::gradient::unmix_pair`].
fn subpx_window() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("INKVEC_SUBPX_WIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&w| (1..=8).contains(&w))
            .unwrap_or(1)
    })
}

/// Cosine of the turning angle between a point's two chords above which the point is
/// treated as a corner by `refine_subpixel` (60 degrees). A staircase at any slope turns
/// by at most 45 degrees between chords two points long, so slanted edges stay smooth.
const CORNER_COS: f64 = 0.5;

/// Move every boundary vertex of `map` to its sub-pixel position, and record how well
/// localized it is. See the module docs above for how.
pub fn refine_subpixel(
    map: &mut PlanarMap,
    rgb: &[[f32; 3]],
    face_fill: &[crate::gradient::FillModel],
    sigma_noise: f64,
    simplify_faint: bool,
) {
    refine_subpixel_alpha(map, rgb, face_fill, sigma_noise, simplify_faint, None)
}

/// [`refine_subpixel`] with alpha as a fourth channel: `alpha` is the source's alpha per
/// pixel and each face's opacity. A pixel on the edge between white paint and the clear
/// ground is then unmixed along the alpha axis, where over white it had no contrast at all.
/// With `None` this is exactly [`refine_subpixel`].
pub fn refine_subpixel_alpha(
    map: &mut PlanarMap,
    rgb: &[[f32; 3]],
    face_fill: &[crate::gradient::FillModel],
    sigma_noise: f64,
    simplify_faint: bool,
    src_alpha: Option<(&[f32], &[f32])>,
) {
    let (w, h) = (map.width, map.height);
    let min_contrast = (3.0 * sigma_noise).max(MIN_UNMIX_CONTRAST);
    let sample_alpha = |x: f64, y: f64| -> f32 {
        let Some((img_a, _)) = src_alpha else {
            return 1.0;
        };
        let (xf, yf) = (x.floor(), y.floor());
        let (x0, y0) = (xf as isize, yf as isize);
        let (tx, ty) = ((x - xf) as f32, (y - yf) as f32);
        let (mut acc, mut wsum) = (0.0f32, 0.0f32);
        for (dx, dy, wt) in [
            (0, 0, (1.0 - tx) * (1.0 - ty)),
            (1, 0, tx * (1.0 - ty)),
            (0, 1, (1.0 - tx) * ty),
            (1, 1, tx * ty),
        ] {
            let (sx, sy) = (x0 + dx, y0 + dy);
            if sx < 0 || sy < 0 || sx >= w as isize || sy >= h as isize {
                continue;
            }
            acc += img_a[sy as usize * w + sx as usize] * wt;
            wsum += wt;
        }
        if wsum <= 1e-6 {
            1.0
        } else {
            acc / wsum
        }
    };

    let sample = |x: f64, y: f64| -> Option<[f32; 3]> {
        // Bilinear sample of the source image at pixel-centre coordinates.
        let (xf, yf) = (x.floor(), y.floor());
        let (x0, y0) = (xf as isize, yf as isize);
        let (tx, ty) = ((x - xf) as f32, (y - yf) as f32);
        let mut acc = [0.0f32; 3];
        let mut wsum = 0.0f32;
        for (dx, dy, wt) in [
            (0, 0, (1.0 - tx) * (1.0 - ty)),
            (1, 0, tx * (1.0 - ty)),
            (0, 1, (1.0 - tx) * ty),
            (1, 1, tx * ty),
        ] {
            let (sx, sy) = (x0 + dx, y0 + dy);
            if sx < 0 || sy < 0 || sx >= w as isize || sy >= h as isize {
                continue;
            }
            let c = rgb[sy as usize * w + sx as usize];
            acc[0] += c[0] * wt;
            acc[1] += c[1] * wt;
            acc[2] += c[2] * wt;
            wsum += wt;
        }
        if wsum <= 1e-6 {
            return None;
        }
        Some([acc[0] / wsum, acc[1] / wsum, acc[2] / wsum])
    };

    for e in &mut map.edges {
        if e.left == u16::MAX || e.right == u16::MAX {
            // One side is outside the image; there is nothing to unmix against.
            continue;
        }
        // `left`/`right` are face ids. Unmixing needs each face's own colour, which is
        // not the palette indexed by face id — several faces share one ink, and a face
        // fitted as a gradient has no single palette entry at all.
        let (Some(fa), Some(fb)) = (
            face_fill.get(e.left as usize),
            face_fill.get(e.right as usize),
        ) else {
            continue;
        };

        let n = e.points.len();
        let mut moved = Vec::with_capacity(n);
        let mut sigmas = Vec::with_capacity(n);

        for k in 0..n {
            let p = e.points[k];
            // Local tangent from neighbours, normal perpendicular to it. The window is
            // INKVEC_SUBPX_WIN points each side (default 1).
            //
            // `INKVEC_SUBPX_WIN` widens the window the tangent is taken over. The
            // marching-squares polyline is a staircase, so a tangent from the immediate
            // neighbours is quantised to a few directions and on a slanted edge the probe
            // line is off-axis. Two points each side averages that out and improves the
            // colour error (620-icon subset: dE00 0.2663 -> 0.2534 against the root-find,
            // where one point each side gives 0.2591) - but it costs DISTS (0.0418 ->
            // 0.0435), because a wide tangent at a corner moves the vertex sideways, and
            // it has a failure mode: on one openmoji juggler the body's boundary moved
            // enough to change which face the emitter paints on top (dE00 0.23 -> 3.20).
            // Corners keep the narrow tangent already, so the remaining harm is elsewhere.
            // Default is one point each side until that is understood (LOG-43).
            let win = subpx_window();
            let (pa, pb) = (
                e.points[k.saturating_sub(win)],
                e.points[(k + win).min(n - 1)],
            );
            let corner = win > 1 && {
                let (c0, c1) = (p - pa, pb - p);
                let (l0, l1) = (c0.norm(), c1.norm());
                l0 > 1e-9 && l1 > 1e-9 && (c0.x * c1.x + c0.y * c1.y) / (l0 * l1) < CORNER_COS
            };
            let (pa, pb) = if corner {
                (e.points[k.saturating_sub(1)], e.points[(k + 1).min(n - 1)])
            } else {
                (pa, pb)
            };
            let t = pb - pa;
            let tl = t.norm();
            let (ca, cb, contrast) = crate::gradient::unmix_pair(fa, fb, p.x, p.y);
            let d = [ca[0] - cb[0], ca[1] - cb[1], ca[2] - cb[2]];
            let dd = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) as f64;
            // The opacity axis, when the source had one -- used only where the colours over
            // white cannot tell the faces apart, the one case they do not already carry the
            // alpha (see `boundary_opt`): white paint on the clear ground, one fade's bands.
            let src_alpha = src_alpha.filter(|_| contrast < min_contrast);
            let (ab, da) = match src_alpha {
                Some((_, fa_)) => {
                    let (a_a, a_b) = (
                        fa_.get(e.left as usize).copied().unwrap_or(1.0),
                        fa_.get(e.right as usize).copied().unwrap_or(1.0),
                    );
                    (a_b, a_a - a_b)
                }
                None => (1.0, 0.0),
            };
            let (dd, contrast) = if src_alpha.is_some() {
                (
                    dd + (da * da) as f64,
                    (contrast * contrast + (da * da) as f64).sqrt(),
                )
            } else {
                (dd, contrast)
            };
            // Nothing to unmix across: the vertex stays on the grid at grid uncertainty.
            if tl < 1e-9 || contrast < min_contrast {
                moved.push(p);
                sigmas.push(0.5);
                continue;
            }
            let alpha = |x: f64, y: f64| -> Option<f64> {
                let p = sample(x, y)?;
                let mut n = (p[0] - cb[0]) * d[0] + (p[1] - cb[1]) * d[1] + (p[2] - cb[2]) * d[2];
                if src_alpha.is_some() {
                    n += (sample_alpha(x, y) - ab) * da;
                }
                Some((n as f64 / dd).clamp(0.0, 1.0))
            };
            let nx = -t.y / tl;
            let ny = t.x / tl;

            // Where the edge is, from the pixels it crosses.
            //
            // A pixel's value is the box-filtered coverage of the boundary: for a
            // straight edge the coverage along the normal is a ramp exactly one pixel
            // wide (times the chord the normal cuts through a pixel), and the 0.5 level
            // of that ramp is the edge. The value at a pixel centre is a sample of the
            // ramp, so a pixel that is partly covered says directly how far the edge is
            // from its centre: `(0.5 - alpha) / slope`. The earlier root-find on a
            // bilinear interpolation between centres was biased: interpolating from a
            // pure neighbour (alpha 0) to the partial pixel (alpha 0.67) puts the 0.5
            // crossing at 0.25 of the way, not 0.33, so every edge was pulled towards
            // the .0/.5 grid by up to 0.17 px - which on a Material icon (24-unit grid,
            // edges at multiples of 5.33 px) is nearly every edge, measured 0.19 px off
            // on straight runs and 2.5% of ink missing.
            // Coverage changes by the chord length the normal cuts through a pixel per
            // unit of edge movement: 1 on an axis, sqrt(2) on a diagonal. So the edge is
            // (alpha - 0.5) / chord from the centre, and chord = 1 / max(|nx|, |ny|).
            // (Dividing by max(|nx|,|ny|) instead pushed every diagonal and curved edge
            // out by up to 2x: rounded outlines came back uniformly fat.)
            // That linear rule is exact only for an axis-aligned edge. A pixel cut by a
            // slanted edge has a coverage curve with quadratic tails - at 45 degrees it
            // is quadratic throughout - and reading a 90%-covered pixel with the chord
            // put the edge 0.57 px from its centre where the truth is 0.39. Slanted
            // strokes came out fat by 0.1-0.2 px, which on one-colour logos and outlined
            // emoji cost more than the axis-aligned gain (0.009 dE00 on openmoji and
            // simple-icons against the root-find). `edge_offset` inverts the exact
            // half-plane coverage of a unit square.
            let (na, nb) = {
                let (a, b) = (nx.abs(), ny.abs());
                if a >= b {
                    (a, b)
                } else {
                    (b, a)
                }
            };
            let edge_offset = |a: f64| -> f64 {
                // Distance from the centre of a pixel with coverage `a` to the edge,
                // signed towards the side where coverage falls.
                let (hi, s) = if a >= 0.5 { (a, 1.0) } else { (1.0 - a, -1.0) };
                let d1 = 0.5 * (na - nb);
                let d2 = 0.5 * (na + nb);
                let d = if hi - 0.5 <= d1 / na {
                    (hi - 0.5) * na
                } else {
                    d2 - (2.0 * na * nb * (1.0 - hi)).max(0.0).sqrt()
                };
                s * d
            };
            let centre_alpha = |u: f64| -> Option<(f64, f64)> {
                // Pixel containing p + n*u; alpha at its centre, and the centre's
                // signed position along the normal.
                let (x, y) = (p.x + nx * u, p.y + ny * u);
                let (cx, cy) = (x.round(), y.round());
                if cx < 0.0 || cy < 0.0 || cx >= w as f64 || cy >= h as f64 {
                    return None;
                }
                let a = alpha(cx, cy)?;
                Some(((cx - p.x) * nx + (cy - p.y) * ny, a))
            };
            let mut probes: Vec<(f64, f64)> = Vec::with_capacity(5);
            for u in [-1.0, -0.5, 0.0, 0.5, 1.0] {
                if let Some(c) = centre_alpha(u) {
                    if !probes.iter().any(|q: &(f64, f64)| (q.0 - c.0).abs() < 1e-9) {
                        probes.push(c);
                    }
                }
            }
            probes.sort_by(|a, b| a.0.total_cmp(&b.0));
            // Which way alpha grows along the normal.
            let dir = match (probes.first(), probes.last()) {
                (Some(f), Some(l)) if l.0 > f.0 && (l.1 - f.1).abs() > 0.05 => (l.1 - f.1).signum(),
                _ => 0.0,
            };
            let mut est = 0.0;
            let mut wsum = 0.0;
            // Two partial pixels either side of the crossing: the ramp is sampled on
            // both sides of 0.5, and the crossing interpolated between the two centres is
            // unbiased however wide the ramp is - a slanted edge whose local normal is
            // quantised to the grid by the marching-squares polyline spreads its ramp
            // over two or three pixels, and inverting a single pixel with the chord of
            // the wrong normal put such points 0.3-0.5 px off (script wordmarks). The
            // single-pixel inversion is only needed, and only exact, when the partial
            // pixel sits between saturated neighbours. Inverting a single pixel
            // everywhere instead (once INKVEC_SUBPX_MODE=inv) is the measured-worse arm
            // of that comparison and is no longer switchable.
            //
            // And only the two probes that bracket the crossing take part. A stroke
            // under two pixels wide straddling two pixel centres reads 0.9 / 0.9: the
            // far pixel is partial because of the stroke's *other* edge, and averaging
            // it in as evidence of this edge pushed the point half a pixel into the
            // stroke (script wordmarks and one-colour logos +0.1 dE00 each).
            let partial = |a: f64| a > 0.03 && a < 0.97;
            let bracket = if dir != 0.0 {
                probes
                    .windows(2)
                    .find(|w| {
                        (w[0].1 - 0.5) * (w[1].1 - 0.5) <= 0.0 && (w[1].1 - w[0].1).abs() > 1e-9
                    })
                    .map(|w| (w[0], w[1]))
            } else {
                None
            };
            if let Some(((u0, a0), (u1, a1))) = bracket {
                if partial(a0) && partial(a1) {
                    est = u0 + (u1 - u0) * (0.5 - a0) / (a1 - a0);
                } else if partial(a0) {
                    est = u0 - dir * edge_offset(a0);
                } else if partial(a1) {
                    est = u1 - dir * edge_offset(a1);
                } else {
                    // Both saturated: the edge falls between the two centres.
                    est = 0.5 * (u0 + u1);
                }
                wsum = 1.0;
            } else if dir != 0.0 {
                for &(uc, a) in &probes {
                    if a > 0.03 && a < 0.97 {
                        // The nearer alpha is to 0.5 the more the pixel straddles the
                        // edge and the better its estimate; saturated pixels say nothing.
                        let wgt = 1.0 - (a - 0.5).abs() * 2.0;
                        est += wgt * (uc - dir * edge_offset(a));
                        wsum += wgt;
                    }
                }
            }
            let mut hit = if wsum > 1e-9 { Some(est / wsum) } else { None };
            // The inversion assumes a step: coverage goes from one ink to the other
            // within about a pixel, so a partial pixel's value is the edge's offset. A
            // soft boundary - two bands of one gradient, a shadow's edge - is a ramp,
            // and read as a step a single sample throws the point up to a pixel off
            // (synthetic gradients 0.15 -> 0.35 dE00, noto +0.08 when this was applied
            // everywhere). Require the profile along the normal to saturate on both
            // sides within the probe span; otherwise root-find the 0.5 level as before.
            // ... and be monotone: a one-pixel stroke has background on both sides of
            // the probe span, saturating at both ends like a step but as a ridge, and
            // read as a step it threw the point a pixel out (synthetic thin_features
            // 0.91 -> 3.00 dE00, script wordmarks +0.25).
            let monotone = {
                let a: Vec<f64> = probes.iter().map(|q| q.1).collect();
                let inc = a.windows(2).all(|w| w[1] >= w[0] - 0.05);
                let dec = a.windows(2).all(|w| w[1] <= w[0] + 0.05);
                inc || dec
            };
            // ... and lie between two flat fills. Against a gradient face the unmixing
            // colours are the model's local prediction, and a small error in them moves
            // alpha enough to misplace an inverted edge, where the root-find only needs
            // the crossing; measured: colour families +0.015 dE00 with the inversion
            // applied at gradient boundaries, no loss without.
            let both_flat = !fa.is_gradient() && !fb.is_gradient();
            // A two-pixel saturation requirement on each side was tried against thin
            // strokes and gaps and measured worse on the full set (objective 0.6703 vs
            // 0.6816 but dE00 +0.004 and parameters +9%): it starved the very edges the
            // inversion is for. The monotone test alone carries the thin-feature case.
            // Forcing the root-find everywhere (once INKVEC_SUBPX_MODE=root) was the
            // other arm of the A/B and is no longer switchable.
            let step_like = !corner
                && probes.len() >= 3
                && both_flat
                && monotone
                && probes.iter().map(|q| q.1).fold(1.0f64, f64::min) < 0.12
                && probes.iter().map(|q| q.1).fold(0.0f64, f64::max) > 0.88
                && contrast >= 2.0 * min_contrast;
            if !step_like {
                hit = None;
                const STEPS: usize = 9;
                let mut prev: Option<(f64, f64)> = None;
                for s in 0..=STEPS {
                    let u = -1.0 + 2.0 * s as f64 / STEPS as f64;
                    let Some(a) = alpha(p.x + nx * u, p.y + ny * u) else {
                        continue;
                    };
                    if let Some((pu, pa_)) = prev {
                        if (pa_ - 0.5) * (a - 0.5) <= 0.0 && (a - pa_).abs() > 1e-9 {
                            hit = Some(pu + (u - pu) * (0.5 - pa_) / (a - pa_));
                            break;
                        }
                    }
                    prev = Some((u, a));
                }
            }
            if std::env::var_os("INKVEC_SUBPXDBG").is_some() {
                eprintln!(
                    "  [subpx] p=({:.3},{:.3}) n=({:.2},{:.2}) probes={:?} dir={} hit={:?} ca={:?} cb={:?}",
                    p.x, p.y, nx, ny, probes, dir, hit, ca, cb
                );
            }

            let shift = hit.unwrap_or(0.0).clamp(-1.0, 1.0);
            moved.push(Point::new(p.x + nx * shift, p.y + ny * shift));

            // Positional uncertainty: noise divided by contrast gives coverage
            // uncertainty; dividing again by the coverage gradient converts it to
            // pixels. Combined in quadrature with the resolution limit of the method.
            let g = {
                let a0 = alpha(p.x - nx * 0.5, p.y - ny * 0.5).unwrap_or(0.0);
                let a1 = alpha(p.x + nx * 0.5, p.y + ny * 0.5).unwrap_or(1.0);
                (a1 - a0).abs().max(1e-3)
            };
            let s = (sigma_noise / contrast) / g;
            // Adaptive simplification: a boundary nobody can see does not deserve
            // coordinates.
            //
            // The line above is the *statistical* uncertainty, and on clean art it is
            // swamped by the method's own resolution limit — measured on two identical
            // wavy edges, one at 90% contrast and one at 7%, the tracer spent 100 and 103
            // segments. That is the right answer to "where is the boundary" and the wrong
            // answer to "how much description is this boundary worth": the error a viewer
            // sees is the position error times the contrast across it, so at a tenth of the
            // contrast a coordinate buys a tenth of the visible accuracy.
            //
            // So the uncertainty handed to the fitter is inflated below a reference
            // contrast, which spends the description length where it shows. Above the
            // reference nothing changes, which is most of a logo.
            //
            // Off by default, because the corpus disagrees with the argument: on the
            // 246-icon screen set it saves 0.3% of the parameters and costs 0.2% of the
            // colour error, objective 0.3992 -> 0.4023. Those icons are high-contrast art
            // where the faint case barely arises, and the metric sees the loss and not the
            // gain — the same blindness that keeps `--cutout` off. Ask for it with
            // `--simplify-faint` on material that has genuinely indistinct boundaries.
            const CONTRAST_REF: f64 = 0.25;
            const MAX_INFLATION: f64 = 4.0;
            let visibility = if simplify_faint {
                (CONTRAST_REF / contrast.max(1e-6)).clamp(1.0, MAX_INFLATION)
            } else {
                1.0
            };
            sigmas.push(match crate::contour::sigma_flat() {
                Some(flat) => flat,
                None => (s.hypot(crate::coverage::DEFAULT_SIGMA_MODEL) * visibility)
                    .max(crate::contour::sigma_floor())
                    .clamp(0.02, 2.0),
            });
        }

        // The planar path extracts boundaries as level sets on a pixel grid exactly as
        // the bilevel path does, so it inherits the same curvature-dependent systematic
        // error and needs the same correction.
        let closed = e.closed;
        let sigmas: Vec<f64> = (0..moved.len())
            .map(|k| crate::contour::inflate_for_curvature(&moved, k, sigmas[k], closed))
            .collect();
        // Dump the measured boundary for offline study of its error structure. The whole
        // faceting question turns on how the extraction error is correlated along a
        // boundary, and that is a property of these numbers, not of an argument about them.
        if let Some(path) = std::env::var_os("INKVEC_DUMP_CONTOUR") {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(f, "# edge {} closed {}", moved.len(), e.closed);
                for (q, sg) in moved.iter().zip(sigmas.iter()) {
                    let _ = writeln!(f, "{:.6} {:.6} {:.6}", q.x, q.y, sg);
                }
            }
        }
        e.points = moved;
        e.sigma = sigmas;
    }
}

/// For each face, the ordered list of rings, each ring a sequence of
/// `(edge index, reversed)`.
///
/// Deliberately returns *references into the map* rather than geometry. The caller
/// assembles the actual curves, so the same traversal works whatever alphabet the fitter
/// used — polylines, cubics, or later arcs and primitives — and, more importantly, every
/// face that touches a boundary names the same edge index. Nothing is copied, so the two
/// sides of a boundary cannot drift apart.
pub fn face_edge_order(map: &PlanarMap) -> Vec<Vec<Vec<(usize, bool)>>> {
    let mut per_label: Vec<Vec<(usize, bool)>> = vec![Vec::new(); map.n_labels];
    for (k, e) in map.edges.iter().enumerate() {
        if (e.left as usize) < map.n_labels {
            per_label[e.left as usize].push((k, false));
        }
        if (e.right as usize) < map.n_labels {
            per_label[e.right as usize].push((k, true));
        }
    }

    let mut out: Vec<Vec<Vec<(usize, bool)>>> = Vec::with_capacity(map.n_labels);
    for refs in per_label.iter() {
        let mut rings: Vec<Vec<(usize, bool)>> = Vec::new();

        let mut by_start: HashMap<u32, Vec<usize>> = HashMap::new();
        for (slot, &(k, rev)) in refs.iter().enumerate() {
            let e = &map.edges[k];
            let s = if rev { e.end_node } else { e.start_node };
            by_start.entry(s).or_default().push(slot);
        }
        let mut consumed = vec![false; refs.len()];

        for slot0 in 0..refs.len() {
            if consumed[slot0] {
                continue;
            }
            let (k0, rev0) = refs[slot0];
            if map.edges[k0].closed {
                consumed[slot0] = true;
                rings.push(vec![(k0, rev0)]);
                continue;
            }

            let mut ring: Vec<(usize, bool)> = Vec::new();
            let mut slot = slot0;
            let start_node = {
                let e = &map.edges[k0];
                if rev0 {
                    e.end_node
                } else {
                    e.start_node
                }
            };
            let mut guard = 0usize;

            loop {
                if consumed[slot] || guard > refs.len() + 4 {
                    break;
                }
                consumed[slot] = true;
                guard += 1;

                let (k, rev) = refs[slot];
                ring.push((k, rev));

                let e = &map.edges[k];
                let end = if rev { e.start_node } else { e.end_node };
                if end == start_node {
                    break;
                }
                let Some(cands) = by_start.get(&end) else {
                    break;
                };
                let Some(&nxt) = cands.iter().find(|&&s| !consumed[s]) else {
                    break;
                };
                slot = nxt;
            }

            if !ring.is_empty() {
                rings.push(ring);
            }
        }
        out.push(rings);
    }
    out
}

#[cfg(test)]
mod saddle_tests {
    use super::*;

    /// Four pixels meeting at a corner, with the two on one diagonal a single face:
    ///
    /// ```text
    ///   1 0        the 0s are one face, meeting at the centre corner;
    ///   0 2        1 and 2 are two shapes that touch there and nowhere else.
    /// ```
    ///
    /// Left alone the centre is a four-way junction, and 1 and 2 are welded at it: both
    /// draw a boundary that ends at that one point. Split, each gets a point of its own,
    /// so the two can be placed independently and the fill between them can open.
    fn touching_corner() -> (Vec<u16>, usize, usize) {
        (vec![1, 0, 0, 2], 2, 2)
    }

    /// Count the curves that stop at `p`, and the curves that carry it at all.
    fn at_point(map: &PlanarMap, p: Point) -> (usize, usize) {
        let same = |q: &Point| (q.x - p.x).abs() < 1e-9 && (q.y - p.y).abs() < 1e-9;
        let ending = map
            .edges
            .iter()
            .filter(|e| {
                !e.closed
                    && (e.points.first().is_some_and(same) || e.points.last().is_some_and(same))
            })
            .count();
        let carrying = map
            .edges
            .iter()
            .filter(|e| e.points.iter().any(same))
            .count();
        (ending, carrying)
    }

    #[test]
    fn four_inks_meeting_at_a_corner_stay_a_junction() {
        // No diagonal is one face, so nothing can be read off the labels and the corner
        // keeps the four-way junction it has always had.
        let map = build(&[1, 0, 3, 2], 2, 2, 4);
        let (ending, _) = at_point(&map, node_point(1, 1));
        assert_eq!(ending, 4, "every curve should stop at an unreadable corner");
    }

    #[test]
    fn a_corner_two_shapes_share_becomes_two_points() {
        let (labels, w, h) = touching_corner();
        let map = build(&labels, w, h, 3);
        let (ending, carrying) = at_point(&map, node_point(1, 1));
        // Neither shape stops there any more: each passes through a copy of its own.
        assert_eq!(ending, 0, "the corner should no longer be a junction");
        assert_eq!(
            carrying, 2,
            "exactly the two shapes that touch should carry the corner"
        );
    }

    #[test]
    fn splitting_gives_each_shape_its_own_copy_of_the_corner() {
        let (labels, w, h) = touching_corner();
        let map = build(&labels, w, h, 3);
        // The two ids for the corner are one grid apart, and both decode to it.
        let plane = ((w + 1) * (h + 1)) as u32;
        let real = node_id(1, 1, w);
        assert_eq!(node_position(real, w, h), node_position(real + plane, w, h));

        // Faces 1 and 2 each still own a closed boundary, and every edge still separates
        // exactly the two faces it did before: the split must not merge segments whose
        // sides differ.
        for e in &map.edges {
            assert_ne!(e.left, e.right, "an edge with the same face on both sides");
        }
        let rings = face_edge_order(&map);
        for face in [1usize, 2] {
            assert!(!rings[face].is_empty(), "face {face} lost its ring");
        }
    }
}
