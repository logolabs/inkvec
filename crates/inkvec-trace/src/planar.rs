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
//!
//! # Where this sits
//!
//! In the colour pipeline (the crate root's trace), after the palette and region stages
//! have produced a label map with one `u16` face id per pixel:
//!
//! 1. [`build`] turns the label map into a [`PlanarMap`] whose vertices sit on pixel
//!    corners, at half-integer coordinates in the pixel-centre convention;
//! 2. [`refine_subpixel_alpha`] slides each vertex along its boundary normal to where the
//!    anti-aliased pixels say the boundary is, and records how well that is known;
//! 3. [`refine_junctions`] (in `junctions`) places the junction nodes themselves;
//! 4. `boundary_opt` then solves all positions jointly against the image (Quality only),
//!    and `symmetry` re-imposes detected mirror symmetry.
//!
//! [`face_edge_order`] is how later stages turn the map back into closed rings per face.
//! The fast pipeline, `occlusion`, `symmetry`, `taper` and the research decoders read the
//! map too.
//!
//! Coordinates are in pixels with pixel centres at integer coordinates, so pixel `(x, y)`
//! covers `[x−0.5, x+0.5] x [y−0.5, y+0.5]` and grid node `(i, j)` is at `(i−0.5, j−0.5)`.

use std::collections::HashMap;

use inkvec_core::{Point, Polyline, Vec2};

use crate::gradient::FillModel;

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
pub(crate) mod runs;
pub use junctions::{node_position, refine_junctions};

/// Grid node index. Nodes sit at pixel corners: node `(i, j)` is at image coordinate
/// `(i - 0.5, j - 0.5)`, with `i` in `0..=w` and `j` in `0..=h`.
#[inline]
fn node_id(i: usize, j: usize, w: usize) -> u32 {
    u32::try_from(j * (w + 1) + i)
        .expect("planar node id exceeds u32; the raster is too large to subdivide")
}

/// Image position of grid node `(i, j)`: the top-left corner of pixel `(i, j)`.
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
///
/// It runs from node `a` to node `b`, one pixel side long; `left` and `right` are the face
/// labels on either side when walking from `a` to `b` (`u16::MAX` outside the image).
#[derive(Clone, Copy)]
struct Seg {
    a: u32,
    b: u32,
    left: u16,
    right: u16,
}

/// The segments incident to each node, in segment order: every node that has any, in
/// increasing id, with its segments in one flat list. A map from node to segment list, in
/// two sorted arrays rather than a hash map of vectors -- the same lists, without an
/// allocation and a hash per boundary node.
struct Incidence {
    /// Nodes with at least one segment, increasing.
    nodes: Vec<u32>,
    /// Where each node's segments start in `segs`; one more entry than `nodes`.
    start: Vec<usize>,
    /// Segment indices, grouped by node and increasing within a node.
    segs: Vec<usize>,
}

impl Incidence {
    /// Index `segs` by endpoint: each segment is listed under both of its nodes. Built by
    /// sorting `(node, segment)` pairs, so the order within a node is segment order.
    fn new(segs: &[Seg]) -> Self {
        let mut pairs: Vec<(u32, usize)> = Vec::with_capacity(2 * segs.len());
        for (k, s) in segs.iter().enumerate() {
            pairs.push((s.a, k));
            pairs.push((s.b, k));
        }
        pairs.sort_unstable();
        let mut nodes = Vec::new();
        let mut start = Vec::new();
        for (i, &(n, _)) in pairs.iter().enumerate() {
            if nodes.last() != Some(&n) {
                nodes.push(n);
                start.push(i);
            }
        }
        start.push(pairs.len());
        Self {
            nodes,
            start,
            segs: pairs.into_iter().map(|(_, k)| k).collect(),
        }
    }

    /// The segments at the `i`-th node of `nodes`.
    fn at(&self, i: usize) -> &[usize] {
        &self.segs[self.start[i]..self.start[i + 1]]
    }

    /// The segments at node `n`, if it has any.
    fn get(&self, n: u32) -> Option<&[usize]> {
        self.nodes.binary_search(&n).ok().map(|i| self.at(i))
    }
}

/// Build the planar map from an integer label image.
///
/// `labels` is row-major, `w x h`, one face id per pixel, ids below `n_labels`. Every
/// pixel side between two different labels (including the image border, against a
/// virtual outside label `u16::MAX`) becomes a unit [`Seg`] between two grid nodes. A node
/// touched by exactly two segments is a pass-through; any other node is a junction.
/// Segments are then chained, junction to junction, into [`Edge`]s, and whatever remains
/// forms closed loops with no junction on them. The result is exact topology: vertices on
/// pixel corners, uncertainty `0.5` px everywhere until [`refine_subpixel`] measures it.
///
/// The output order is deterministic: open chains in increasing start-node id, then
/// closed loops in segment order. Later stages index edges by position, so that matters.
pub fn build(labels: &[u16], w: usize, h: usize, n_labels: usize) -> PlanarMap {
    let mut segs = dual_segments(labels, w, h);
    split_saddle_corners(&mut segs, labels, w, h);

    // Adjacency, and node degree.
    let inc = Incidence::new(&segs);
    let mut used = vec![false; segs.len()];
    let mut edges: Vec<Edge> = Vec::new();
    walk_open_chains(&segs, &inc, &mut used, &mut edges, w, h);
    walk_closed_loops(&segs, &inc, &mut used, &mut edges, w, h);

    PlanarMap {
        edges,
        width: w,
        height: h,
        n_labels,
    }
}

/// Every pixel side separating two labels, as a unit segment on the dual grid.
///
/// Vertical sides first (row by row), then horizontal ones. Each is oriented so that the
/// label it records as `left` is on the left when walking from `a` to `b` in image
/// coordinates (y down).
fn dual_segments(labels: &[u16], w: usize, h: usize) -> Vec<Seg> {
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
    segs
}

/// Where four pixels meet at a corner and one diagonal is a single face, give the other
/// two their own copy of that corner.
///
/// Such a node is degree 4, so the walk below stops there — once for each shape that
/// touches it. All four then hold the one point, and the two on the cut diagonal are
/// welded at it: each draws a boundary that runs to a spike and turns back, so the
/// fill pinches to nothing and reopens. That is the bowtie, and no later stage can
/// undo it, because one node has one refined position however many curves end there.
///
/// Which segment belongs to which shape is not a guess. Each dual edge separates a
/// known pair of the four pixels: `above` runs between NW and NE, `left` between NW
/// and SW, `right` between NE and SE, `below` between SW and SE. When NE and SW are
/// one face, {above, left} is the whole of NW's boundary here and {right, below} the
/// whole of SE's, so handing SE's pair a copy of the corner leaves each passing
/// through a point of its own. Both start at the same place, because the pixels do
/// meet there; what changes is that they are two points, each free to move to where
/// the image puts it, so the following stages open the gap the coverage says is there
/// instead of holding it shut at zero.
///
/// Note what makes the merge legal: NE and SW being *one face* means the two segments
/// joined into a chain separate the same pair of faces. Chaining segments that do not
/// would hand the edge one face's identity while half of it borders another, and the
/// other face would lose that edge from its ring entirely. Which diagonal is one face
/// is settled upstream, in `merge_saddle_faces`, where the image can be consulted.
/// Here it is read off the labels, and a node whose diagonals are both one face, or
/// neither, is left alone: still a junction, exactly as before.
///
/// The copy is a second node id, `real + (w+1)(h+1)`: one whole grid further on, which
/// [`node_position`] folds back to the same place.
fn split_saddle_corners(segs: &mut [Seg], labels: &[u16], w: usize, h: usize) {
    // Real nodes are `0..(w+1)*(h+1)`; a node split below gets a second id one whole
    // grid further on, which `node_position` folds back to the same place.
    let plane = u32::try_from((w + 1) * (h + 1))
        .expect("planar node count exceeds u32; the raster is too large to subdivide");

    // Every node with segments, in increasing id -- which is row by row, left to right,
    // the order the grid is scanned in -- and the lists are read before any segment
    // below is given its copy of a corner.
    let by_node = Incidence::new(segs);
    for ni in 0..by_node.nodes.len() {
        let real = by_node.nodes[ni];
        let here = by_node.at(ni);
        let (i, j) = ((real as usize) % (w + 1), (real as usize) / (w + 1));
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

/// A node is a junction when it is not a simple pass-through.
///
/// Degree 4 (four pixels meeting at a corner) is treated as a junction rather than guessed
/// at: splitting there is always topologically safe, whereas picking a diagonal pairing can
/// weld two regions that should be separate. What [`split_saddle_corners`] resolved no
/// longer arrives here as degree 4; everything else still does. A node with no segment at
/// all (`None`) is not on any chain, and counts as a junction so a walk stops there.
fn is_junction(segs_here: Option<&[usize]>) -> bool {
    segs_here.map(<[usize]>::len) != Some(2)
}

/// Walk every chain that starts at a junction, in increasing node id, appending one open
/// [`Edge`] per chain to `edges` and marking its segments in `used`.
///
/// From each junction, each unused incident segment starts a chain, which follows
/// pass-through nodes (degree 2) until it reaches another junction. The edge's `left` and
/// `right` are the first segment's, flipped if that segment points into the junction.
fn walk_open_chains(
    segs: &[Seg],
    inc: &Incidence,
    used: &mut [bool],
    edges: &mut Vec<Edge>,
    w: usize,
    h: usize,
) {
    let junctions: Vec<usize> = (0..inc.nodes.len())
        .filter(|&i| is_junction(Some(inc.at(i))))
        .collect();

    for ji in junctions {
        let start = inc.nodes[ji];
        let list = inc.at(ji);
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
                let cands = inc.get(next_node);
                if is_junction(cands) {
                    cur_node = next_node;
                    break;
                }
                let Some(cands) = cands else {
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
}

/// Anything [`walk_open_chains`] left is a closed loop with no junction at all: follow
/// each from its first unused segment back to its start and append it as a closed
/// [`Edge`] whose start and end node are the same. A loop of fewer than three nodes
/// cannot enclose anything and is dropped.
fn walk_closed_loops(
    segs: &[Seg],
    inc: &Incidence,
    used: &mut [bool],
    edges: &mut Vec<Edge>,
    w: usize,
    h: usize,
) {
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
            let Some(cands) = inc.get(next_node) else {
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
}

/// Smallest colour separation (sRGB, Euclidean) a vertex is unmixed across.
const MIN_UNMIX_CONTRAST: f64 = 0.02;

/// Points each side of a vertex its tangent is taken over; see the comment where it is
/// used, in `probe_chord`, for the measurement that left it at one (it was
/// `INKVEC_SUBPX_WIN`, 1..=8).
const SUBPX_WIN: usize = 1;

/// Cosine of the turning angle between a point's two chords above which the point is
/// treated as a corner by `refine_subpixel` (60 degrees). A staircase at any slope turns
/// by at most 45 degrees between chords two points long, so slanted edges stay smooth.
const CORNER_COS: f64 = 0.5;

/// Move every boundary vertex of `map` to its sub-pixel position, and record how well
/// localized it is.
///
/// Topology is already fixed and stays fixed: this only moves points along the local
/// boundary normal. That ordering is what lets exactness and sub-pixel accuracy coexist —
/// a vertex can be repositioned freely without any risk of changing which regions are
/// adjacent, because adjacency was decided on the integer label map.
///
/// For a vertex between regions `A` and `B`, unmixing the surrounding pixels against
/// those two colours gives a local coverage field, and the boundary is its `0.5` level.
/// Searching along the normal for that crossing is a one-dimensional root find; where
/// the profile is a clean step it is instead read off directly by inverting the exact
/// box-filter coverage of a straight edge (see `invert_step`).
///
/// `rgb` is the source image, row-major sRGB `0..1`, the same size as the map.
/// `face_fill[f]` is the fill model of face `f`; the colours unmixed against are chosen
/// per vertex by [`crate::gradient::unmix_pair`]. `sigma_noise` is the pixel noise from
/// [`crate::coverage::estimate_noise`], in sRGB units. `simplify_faint` inflates the
/// uncertainty of low-contrast boundaries (`--simplify-faint`; see `vertex_sigma`).
pub fn refine_subpixel(
    map: &mut PlanarMap,
    rgb: &[[f32; 3]],
    face_fill: &[FillModel],
    sigma_noise: f64,
    simplify_faint: bool,
) {
    refine_subpixel_alpha(map, rgb, face_fill, sigma_noise, simplify_faint, None)
}

/// [`refine_subpixel`] with alpha as a fourth channel: `alpha` is the source's alpha per
/// pixel and each face's opacity. A pixel on the edge between white paint and the clear
/// ground is then unmixed along the alpha axis, where over white it had no contrast at all.
/// With `None` this is exactly [`refine_subpixel`].
///
/// Edges with the outside of the image on one side are left untouched, as are edges whose
/// faces have no fill model. Each moved point is at most 1 px from where it started, and
/// its sigma is in `[0.02, 2]` px before the curvature correction of
/// [`crate::contour::inflate_for_curvature`]. `INKVEC_SUBPXDBG=1` prints every vertex's
/// probes to stderr; `INKVEC_DUMP_CONTOUR=<file>` appends every refined edge to a file.
///
/// Two phases, [`measure_subpixel`] (reads the map, in parallel) and [`Refined::apply`]
/// (writes it); see [`measure_subpixel`] for the parallel schedule and why it is exact.
pub fn refine_subpixel_alpha(
    map: &mut PlanarMap,
    rgb: &[[f32; 3]],
    face_fill: &[FillModel],
    sigma_noise: f64,
    simplify_faint: bool,
    src_alpha: Option<(&[f32], &[f32])>,
) {
    measure_subpixel(map, rgb, face_fill, sigma_noise, simplify_faint, src_alpha).apply(map);
}

/// The refined geometry of every edge of a map, measured but not yet written back: what
/// [`measure_subpixel`] returns and [`Refined::apply`] consumes.
pub(crate) struct Refined {
    /// Per edge, in edge order: the moved points and their sigmas, or `None` for an edge
    /// the refinement leaves as it is (the image border, or a face without a fill model).
    edges: Vec<Option<(Vec<Point>, Vec<f64>)>>,
}

impl Refined {
    /// Write the measured geometry into `map`, edge by edge; an edge measured as `None` is
    /// left untouched. `map` must be the map that was measured (same edges, same order).
    /// `O(edges)`: each edge's two vectors are moved, not copied.
    pub(crate) fn apply(self, map: &mut PlanarMap) {
        debug_assert_eq!(self.edges.len(), map.edges.len());
        for (e, r) in map.edges.iter_mut().zip(self.edges) {
            if let Some((points, sigma)) = r {
                e.points = points;
                e.sigma = sigma;
            }
        }
    }
}

/// Fewest vertices an edge must have before its vertices are refined in parallel; shorter
/// edges run as one task each. Also the smallest chunk of a long edge's vertices one
/// thread takes, so a task is at least ~30 µs of work (0.44 µs per vertex measured at
/// 2048 px) against rayon's few-µs cost per split.
const PAR_VERTICES: usize = 64;

/// Measure the sub-pixel position and sigma of every vertex of `map` (the arguments are
/// [`refine_subpixel_alpha`]'s), without changing `map`.
///
/// # Schedule
///
/// Refining a vertex reads only the image, the two faces' fills and its own edge's
/// *original* points (its neighbours on the lattice, for the normal), and writes only its
/// own output: it is a pure map over vertices. The curvature correction of a vertex's sigma
/// then reads the edge's *moved* points, all of which are known by then: a pure map again.
/// So both are run as parallel maps, edges in parallel and, inside an edge of at least
/// [`PAR_VERTICES`] points, vertices in parallel too — at 2048 px one edge can hold most
/// of the image's vertices (longest edge 8,192 points, median edge 450, on the opaque
/// `big` set), so edge-level parallelism alone would leave one thread doing most of the
/// work. Each result is written to its own slot of an indexed output (rayon's `collect`
/// and `unzip` over an indexed iterator keep positions), and the values are those the
/// serial loop computed.
///
/// # Why the output is identical to the serial loop
///
/// The serial loop computed each vertex from the same inputs with the same operations;
/// scheduling only changes *when* each value is computed, not how. No value is combined
/// across vertices or threads — there is no parallel reduction, so no floating-point sum
/// whose rounding depends on how the work was split. The measuring phase does not write
/// the map, so no vertex can see another's moved position (the serial loop wrote an edge's
/// points only after the whole edge was measured, so it could not either).
///
/// The two diagnostics keep the serial order: with `INKVEC_SUBPXDBG` (a line per vertex)
/// or `INKVEC_DUMP_CONTOUR` (a block per edge, appended to a file) set, everything runs on
/// the calling thread in edge and vertex order, as before. The dump path is read once.
///
/// Method from: Blelloch, Fineman, Gibbons & Shun 2012, "Internally deterministic parallel
/// algorithms can be fast", PPoPP 2012, 181–192, <https://doi.org/10.1145/2145816.2145840>:
/// a parallel loop whose iterations are independent and write disjoint, indexed outputs
/// computes the same result as the serial loop on every schedule ("internal determinism").
/// Adapted: two nested levels (edges, then vertices of long edges) with a minimum task size.
/// Related work that shaped the "no reductions" rule: Demmel & Nguyen 2013, "Fast
/// Reproducible Floating-Point Summation", ARITH 2013, 163–172,
/// <https://doi.org/10.1109/ARITH.2013.9>, on how a parallel sum's result depends on the
/// split; nothing here sums across vertices, so no reproducible summation is needed.
pub(crate) fn measure_subpixel(
    map: &PlanarMap,
    rgb: &[[f32; 3]],
    face_fill: &[FillModel],
    sigma_noise: f64,
    simplify_faint: bool,
    src_alpha: Option<(&[f32], &[f32])>,
) -> Refined {
    use rayon::prelude::*;
    let ctx = RefineCtx {
        src: Source {
            rgb,
            alpha: src_alpha.map(|(img_a, _)| img_a),
            w: map.width,
            h: map.height,
        },
        face_alpha: src_alpha.map(|(_, face_a)| face_a),
        sigma_noise,
        min_contrast: (3.0 * sigma_noise).max(MIN_UNMIX_CONTRAST),
        simplify_faint,
        debug: inkvec_core::env::flag("INKVEC_SUBPXDBG"),
        dump: inkvec_core::env::path("INKVEC_DUMP_CONTOUR"),
    };
    let edges = if ctx.debug || ctx.dump.is_some() {
        map.edges
            .iter()
            .map(|e| refine_edge(&ctx, face_fill, e, false))
            .collect()
    } else {
        map.edges
            .par_iter()
            .map(|e| refine_edge(&ctx, face_fill, e, true))
            .collect()
    };
    Refined { edges }
}

/// The source image, sampled bilinearly at pixel-centre coordinates.
struct Source<'a> {
    /// Row-major sRGB `0..1`, `w x h`.
    rgb: &'a [[f32; 3]],
    /// The source's own alpha per pixel, when it has one.
    alpha: Option<&'a [f32]>,
    w: usize,
    h: usize,
}

impl Source<'_> {
    /// Bilinear sample of the source image at pixel-centre coordinates.
    ///
    /// Taps outside the image are dropped and the remaining weights renormalised, so the
    /// border is effectively clamped. `None` when no tap lands inside the image.
    fn rgb(&self, x: f64, y: f64) -> Option<[f32; 3]> {
        let (w, h) = (self.w, self.h);
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
            let c = self.rgb[sy as usize * w + sx as usize];
            acc[0] += c[0] * wt;
            acc[1] += c[1] * wt;
            acc[2] += c[2] * wt;
            wsum += wt;
        }
        if wsum <= 1e-6 {
            return None;
        }
        Some([acc[0] / wsum, acc[1] / wsum, acc[2] / wsum])
    }

    /// Bilinear sample of the source alpha, like [`Self::rgb`]; 1 (opaque) when the source
    /// has no alpha or no tap lands inside the image.
    fn alpha(&self, x: f64, y: f64) -> f32 {
        let Some(img_a) = self.alpha else {
            return 1.0;
        };
        let (w, h) = (self.w, self.h);
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
    }
}

/// What [`refine_vertex`] reads that is the same for the whole map.
struct RefineCtx<'a> {
    src: Source<'a>,
    /// Each face's opacity, when the source has an alpha channel.
    face_alpha: Option<&'a [f32]>,
    /// Pixel noise, sRGB units.
    sigma_noise: f64,
    /// Below this colour separation a vertex is not unmixed: `max(3·sigma_noise, 0.02)`.
    min_contrast: f64,
    simplify_faint: bool,
    /// `INKVEC_SUBPXDBG`: print every vertex's probes to stderr.
    debug: bool,
    /// `INKVEC_DUMP_CONTOUR`: the file every refined edge is appended to.
    dump: Option<std::path::PathBuf>,
}

/// Refine every vertex of one edge (see [`refine_subpixel_alpha`]), then apply the
/// curvature correction to its sigmas and optionally dump it. Returns the moved points and
/// their sigmas, or `None` when the edge is left as it is: one side is outside the image
/// (`u16::MAX`), or a face has no fill model.
///
/// With `par`, an edge of at least [`PAR_VERTICES`] points is refined as a parallel map
/// over its vertices, in chunks of at least that many; otherwise serially. Either way
/// vertex `k`'s result is `refine_vertex(.., &e.points, k)` on the original points, and its
/// sigma is `inflate_for_curvature(&moved, k, ..)` on the moved ones, so the output does not
/// depend on `par` (see [`measure_subpixel`]). `O(points)` work.
fn refine_edge(
    ctx: &RefineCtx,
    face_fill: &[FillModel],
    e: &Edge,
    par: bool,
) -> Option<(Vec<Point>, Vec<f64>)> {
    use rayon::prelude::*;
    if e.left == u16::MAX || e.right == u16::MAX {
        // One side is outside the image; there is nothing to unmix against.
        return None;
    }
    // `left`/`right` are face ids. Unmixing needs each face's own colour, which is
    // not the palette indexed by face id — several faces share one ink, and a face
    // fitted as a gradient has no single palette entry at all.
    let (Some(fa), Some(fb)) = (
        face_fill.get(e.left as usize),
        face_fill.get(e.right as usize),
    ) else {
        return None;
    };

    let n = e.points.len();
    let par = par && n >= PAR_VERTICES;
    let vertex = |k: usize| refine_vertex(ctx, fa, fb, e.left, e.right, &e.points, k);
    let (moved, sigmas): (Vec<Point>, Vec<f64>) = if par {
        (0..n)
            .into_par_iter()
            .with_min_len(PAR_VERTICES)
            .map(vertex)
            .unzip()
    } else {
        (0..n).map(vertex).unzip()
    };

    // The planar path extracts boundaries as level sets on a pixel grid exactly as
    // the bilevel path does, so it inherits the same curvature-dependent systematic
    // error and needs the same correction.
    let closed = e.closed;
    let inflate = |k: usize| crate::contour::inflate_for_curvature(&moved, k, sigmas[k], closed);
    let sigmas: Vec<f64> = if par {
        (0..n)
            .into_par_iter()
            .with_min_len(PAR_VERTICES)
            .map(inflate)
            .collect()
    } else {
        (0..n).map(inflate).collect()
    };
    if let Some(path) = ctx.dump.as_deref() {
        dump_contour(path, &moved, &sigmas, closed);
    }
    Some((moved, sigmas))
}

/// Dump the measured boundary for offline study of its error structure
/// (`INKVEC_DUMP_CONTOUR=<file>`, appended, one `x y sigma` line per point after a
/// `# edge <n> closed <bool>` header). The whole faceting question turns on how the
/// extraction error is correlated along a boundary, and that is a property of these
/// numbers, not of an argument about them. Write errors are ignored: this is a debugging
/// aid and must never fail a trace. `path` is the variable's value, read once per map by
/// [`measure_subpixel`], which runs serially whenever it is set so the file keeps edge
/// order.
fn dump_contour(path: &std::path::Path, points: &[Point], sigmas: &[f64], closed: bool) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "# edge {} closed {}", points.len(), closed);
        for (q, sg) in points.iter().zip(sigmas.iter()) {
            let _ = writeln!(f, "{:.6} {:.6} {:.6}", q.x, q.y, sg);
        }
    }
}

/// The sub-pixel position of vertex `k` of an edge between faces `left` (fill `fa`) and
/// `right` (fill `fb`), and its positional sigma before the curvature correction.
///
/// Steps: take the local normal ([`probe_chord`]); build the unmixing axis between the two
/// faces' colours at this point ([`unmix_axis`]); read the coverage at the pixel centres
/// the normal passes through ([`probe_pixel_centres`]); if the profile is a clean step
/// between two flat fills, invert it directly ([`invert_step`]), otherwise root-find the
/// 0.5 level of the interpolated coverage ([`root_find_half`]). The shift along the normal
/// is clamped to ±1 px. A vertex with no usable normal or no contrast stays where it is,
/// at the grid uncertainty 0.5 px.
fn refine_vertex(
    ctx: &RefineCtx,
    fa: &FillModel,
    fb: &FillModel,
    left: u16,
    right: u16,
    points: &[Point],
    k: usize,
) -> (Point, f64) {
    let p = points[k];
    let (t, corner) = probe_chord(points, k);
    let tl = t.norm();
    let axis = unmix_axis(ctx, fa, fb, left, right, p);
    // Nothing to unmix across: the vertex stays on the grid at grid uncertainty.
    if tl < 1e-9 || axis.contrast < ctx.min_contrast {
        return (p, 0.5);
    }
    let nx = -t.y / tl;
    let ny = t.x / tl;

    let probes = probe_pixel_centres(ctx, &axis, p, nx, ny);
    // Which way alpha grows along the normal.
    let dir = match (probes.first(), probes.last()) {
        (Some(f), Some(l)) if l.0 > f.0 && (l.1 - f.1).abs() > 0.05 => (l.1 - f.1).signum(),
        _ => 0.0,
    };
    let mut hit = invert_step(&probes, dir, nx, ny);
    // The inversion also needs the vertex to lie between two flat fills. Against a
    // gradient face the unmixing colours are the model's local prediction, and a small error in them moves
    // alpha enough to misplace an inverted edge, where the root-find only needs
    // the crossing; measured: colour families +0.015 dE00 with the inversion
    // applied at gradient boundaries, no loss without.
    let both_flat = !fa.is_gradient() && !fb.is_gradient();
    if !is_step_like(&probes, corner, both_flat, axis.contrast, ctx.min_contrast) {
        hit = root_find_half(ctx, &axis, p, nx, ny);
    }
    if ctx.debug {
        eprintln!(
            "  [subpx] p=({:.3},{:.3}) n=({:.2},{:.2}) probes={:?} dir={} hit={:?} ca={:?} cb={:?}",
            p.x, p.y, nx, ny, probes, dir, hit, axis.ca, axis.cb
        );
    }

    let shift = hit.unwrap_or(0.0).clamp(-1.0, 1.0);
    let moved = Point::new(p.x + nx * shift, p.y + ny * shift);
    (moved, vertex_sigma(ctx, &axis, p, nx, ny))
}

/// The chord whose perpendicular is taken as vertex `k`'s normal, and whether `k` is a
/// corner.
///
/// The chord runs from the point `SUBPX_WIN` before `k` to the one `SUBPX_WIN` after
/// (clamped to the ends of the edge). With a wider window, a vertex whose two chords turn
/// by more than 60 degrees (`CORNER_COS`) is a corner and falls back to its immediate
/// neighbours. At the shipped window of 1 the corner test never fires.
fn probe_chord(points: &[Point], k: usize) -> (Vec2, bool) {
    let n = points.len();
    let p = points[k];
    // Local tangent from neighbours, normal perpendicular to it. The window is
    // `SUBPX_WIN` points each side.
    //
    // A wider `SUBPX_WIN` averages the tangent over more points. The
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
    let win = SUBPX_WIN;
    let (pa, pb) = (points[k.saturating_sub(win)], points[(k + win).min(n - 1)]);
    let corner = win > 1 && {
        let (c0, c1) = (p - pa, pb - p);
        let (l0, l1) = (c0.norm(), c1.norm());
        l0 > 1e-9 && l1 > 1e-9 && (c0.x * c1.x + c0.y * c1.y) / (l0 * l1) < CORNER_COS
    };
    let (pa, pb) = if corner {
        (points[k.saturating_sub(1)], points[(k + 1).min(n - 1)])
    } else {
        (pa, pb)
    };
    (pb - pa, corner)
}

/// The axis a vertex's coverage is measured along: from the right face's colour
/// (coverage 0) to the left face's (coverage 1), optionally with opacity as a fourth
/// channel.
struct UnmixAxis {
    /// Left face's colour here, sRGB. Kept for the debug print.
    ca: [f32; 3],
    /// Right face's colour here, sRGB.
    cb: [f32; 3],
    /// `ca − cb`.
    d: [f32; 3],
    /// `|d|²`, plus `da²` when opacity is used.
    dd: f64,
    /// Right face's opacity (1 when opacity is not used).
    ab: f32,
    /// Left opacity minus right opacity (0 when not used).
    da: f32,
    /// Whether opacity is a fourth channel of the axis.
    with_alpha: bool,
    /// Length of the axis: the colour separation `|ca − cb|` as `unmix_pair` measures
    /// it, combined in quadrature with `da` when opacity is used.
    contrast: f64,
}

/// Build the [`UnmixAxis`] between faces `left` and `right` at point `p`.
///
/// The opacity axis is used only when the source had one and the colours over white
/// cannot tell the faces apart (`contrast < min_contrast`), the one case they do not
/// already carry the alpha (see `boundary_opt`): white paint on the clear ground, one
/// fade's bands. A face with no recorded opacity counts as opaque.
fn unmix_axis(
    ctx: &RefineCtx,
    fa: &FillModel,
    fb: &FillModel,
    left: u16,
    right: u16,
    p: Point,
) -> UnmixAxis {
    let (ca, cb, contrast) = crate::gradient::unmix_pair(fa, fb, p.x, p.y);
    let d = [ca[0] - cb[0], ca[1] - cb[1], ca[2] - cb[2]];
    let dd = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) as f64;
    let face_alpha = ctx.face_alpha.filter(|_| contrast < ctx.min_contrast);
    let (ab, da) = match face_alpha {
        Some(fa_) => {
            let (a_a, a_b) = (
                fa_.get(left as usize).copied().unwrap_or(1.0),
                fa_.get(right as usize).copied().unwrap_or(1.0),
            );
            (a_b, a_a - a_b)
        }
        None => (1.0, 0.0),
    };
    let (dd, contrast) = if face_alpha.is_some() {
        (
            dd + (da * da) as f64,
            (contrast * contrast + (da * da) as f64).sqrt(),
        )
    } else {
        (dd, contrast)
    };
    UnmixAxis {
        ca,
        cb,
        d,
        dd,
        ab,
        da,
        with_alpha: face_alpha.is_some(),
        contrast,
    }
}

impl UnmixAxis {
    /// Coverage of the left face at `(x, y)`: the least-squares projection of the
    /// bilinearly sampled pixel onto the axis,
    ///
    /// ```text
    ///     a = ((P − cb)·d + (A − ab)·da) / dd      clamped to [0, 1]
    /// ```
    ///
    /// with `P` the sampled colour and `A` the sampled source alpha (the second term only
    /// when opacity is used). `None` outside the image.
    fn coverage(&self, src: &Source, x: f64, y: f64) -> Option<f64> {
        let (cb, d) = (self.cb, self.d);
        let p = src.rgb(x, y)?;
        let mut n = (p[0] - cb[0]) * d[0] + (p[1] - cb[1]) * d[1] + (p[2] - cb[2]) * d[2];
        if self.with_alpha {
            n += (src.alpha(x, y) - self.ab) * self.da;
        }
        Some((n as f64 / self.dd).clamp(0.0, 1.0))
    }
}

/// Coverage at the centres of the pixels the normal through `p` passes through.
///
/// Probes at `u = −1, −0.5, 0, 0.5, 1` px along the unit normal `(nx, ny)`, each rounded
/// to the pixel it lands in; returns `(s, a)` per distinct pixel, where `s` is that
/// pixel centre's signed distance from `p` projected on the normal and `a` its coverage,
/// sorted by `s`. Pixels outside the image are skipped.
fn probe_pixel_centres(
    ctx: &RefineCtx,
    axis: &UnmixAxis,
    p: Point,
    nx: f64,
    ny: f64,
) -> Vec<(f64, f64)> {
    let (w, h) = (ctx.src.w, ctx.src.h);
    let centre_alpha = |u: f64| -> Option<(f64, f64)> {
        // Pixel containing p + n*u; alpha at its centre, and the centre's
        // signed position along the normal.
        let (x, y) = (p.x + nx * u, p.y + ny * u);
        let (cx, cy) = (x.round(), y.round());
        if cx < 0.0 || cy < 0.0 || cx >= w as f64 || cy >= h as f64 {
            return None;
        }
        let a = axis.coverage(&ctx.src, cx, cy)?;
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
    probes
}

/// Distance from the centre of a pixel with coverage `a` to a straight edge crossing it,
/// signed towards the side where coverage falls, for an edge whose unit normal has
/// components `na >= nb >= 0` (the larger and smaller of `|nx|`, `|ny|`).
///
/// This inverts the exact area of a unit square cut by a half-plane (box-filter
/// coverage). Moving the edge a distance `t` from the centre, the covered area is linear,
/// `a = 0.5 + t/na`, while the edge still crosses the two sides it enters and leaves by
/// (`|t| <= (na − nb)/2`), and quadratic once it clips a corner:
/// `1 − a = (d2 − t)² / (2·na·nb)` with `d2 = (na + nb)/2`. Solving each for `t` gives the
/// two branches below; `hi = max(a, 1 − a)` and the sign restores which side was fuller.
/// On an axis (`nb = 0`) only the linear branch applies; at 45 degrees
/// (`na = nb`) only the quadratic one does.
fn edge_offset(a: f64, na: f64, nb: f64) -> f64 {
    let (hi, s) = if a >= 0.5 { (a, 1.0) } else { (1.0 - a, -1.0) };
    let d1 = 0.5 * (na - nb);
    let d2 = 0.5 * (na + nb);
    let d = if hi - 0.5 <= d1 / na {
        (hi - 0.5) * na
    } else {
        d2 - (2.0 * na * nb * (1.0 - hi)).max(0.0).sqrt()
    };
    s * d
}

/// Read the edge's offset along the normal directly from partially covered pixels,
/// assuming the profile is a step (see [`is_step_like`] for when that holds).
///
/// `probes` are the sorted `(s, a)` pairs from [`probe_pixel_centres`] and `dir` the sign
/// of the coverage slope along the normal (0 when there is none). Returns the offset in
/// px, or `None` when no probe is informative.
fn invert_step(probes: &[(f64, f64)], dir: f64, nx: f64, ny: f64) -> Option<f64> {
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
            .find(|w| (w[0].1 - 0.5) * (w[1].1 - 0.5) <= 0.0 && (w[1].1 - w[0].1).abs() > 1e-9)
            .map(|w| (w[0], w[1]))
    } else {
        None
    };
    if let Some(((u0, a0), (u1, a1))) = bracket {
        if partial(a0) && partial(a1) {
            est = u0 + (u1 - u0) * (0.5 - a0) / (a1 - a0);
        } else if partial(a0) {
            est = u0 - dir * edge_offset(a0, na, nb);
        } else if partial(a1) {
            est = u1 - dir * edge_offset(a1, na, nb);
        } else {
            // Both saturated: the edge falls between the two centres.
            est = 0.5 * (u0 + u1);
        }
        wsum = 1.0;
    } else if dir != 0.0 {
        for &(uc, a) in probes {
            if a > 0.03 && a < 0.97 {
                // The nearer alpha is to 0.5 the more the pixel straddles the
                // edge and the better its estimate; saturated pixels say nothing.
                let wgt = 1.0 - (a - 0.5).abs() * 2.0;
                est += wgt * (uc - dir * edge_offset(a, na, nb));
                wsum += wgt;
            }
        }
    }
    if wsum > 1e-9 {
        Some(est / wsum)
    } else {
        None
    }
}

/// Whether the coverage profile across a vertex is a clean step, so that
/// [`invert_step`]'s reading can be trusted; otherwise [`root_find_half`] decides.
///
/// Requires: not a corner, at least three probes, flat fills on both sides, a monotone
/// profile (to within 0.05), saturation at both ends (min below 0.12, max above 0.88),
/// and a contrast of at least twice the unmixing threshold.
fn is_step_like(
    probes: &[(f64, f64)],
    corner: bool,
    both_flat: bool,
    contrast: f64,
    min_contrast: f64,
) -> bool {
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
    // A two-pixel saturation requirement on each side was tried against thin
    // strokes and gaps and measured worse on the full set (objective 0.6703 vs
    // 0.6816 but dE00 +0.004 and parameters +9%): it starved the very edges the
    // inversion is for. The monotone test alone carries the thin-feature case.
    // Forcing the root-find everywhere (once INKVEC_SUBPX_MODE=root) was the
    // other arm of the A/B and is no longer switchable.
    !corner
        && probes.len() >= 3
        && both_flat
        && monotone
        && probes.iter().map(|q| q.1).fold(1.0f64, f64::min) < 0.12
        && probes.iter().map(|q| q.1).fold(0.0f64, f64::max) > 0.88
        && contrast >= 2.0 * min_contrast
}

/// The first crossing of coverage 0.5 along the normal, by linear interpolation between
/// ten bilinear samples evenly spaced over `u` in `[−1, 1]` px. `None` when coverage
/// never crosses 0.5 there.
fn root_find_half(ctx: &RefineCtx, axis: &UnmixAxis, p: Point, nx: f64, ny: f64) -> Option<f64> {
    const STEPS: usize = 9;
    let mut prev: Option<(f64, f64)> = None;
    for s in 0..=STEPS {
        let u = -1.0 + 2.0 * s as f64 / STEPS as f64;
        let Some(a) = axis.coverage(&ctx.src, p.x + nx * u, p.y + ny * u) else {
            continue;
        };
        if let Some((pu, pa_)) = prev {
            if (pa_ - 0.5) * (a - 0.5) <= 0.0 && (a - pa_).abs() > 1e-9 {
                return Some(pu + (u - pu) * (0.5 - pa_) / (a - pa_));
            }
        }
        prev = Some((u, a));
    }
    None
}

/// Positional uncertainty of a refined vertex, in px.
///
/// Noise divided by contrast gives coverage uncertainty; dividing again by the coverage
/// gradient converts it to pixels. Combined in quadrature with the resolution limit of
/// the method:
///
/// ```text
///     g     = max(|a(p + n/2) − a(p − n/2)|, 0.001)       (coverage change per px)
///     sigma = clamp(max(hypot(sigma_noise / contrast / g, sigma_model) · v, SIGMA_FLOOR),
///                   0.02, 2)
/// ```
///
/// with `sigma_model` = [`crate::coverage::DEFAULT_SIGMA_MODEL`] and `v` the faint-edge
/// inflation below (1 unless `simplify_faint`). A sample outside the image reads as
/// coverage 0 on the near side and 1 on the far side.
fn vertex_sigma(ctx: &RefineCtx, axis: &UnmixAxis, p: Point, nx: f64, ny: f64) -> f64 {
    let g = {
        let a0 = axis
            .coverage(&ctx.src, p.x - nx * 0.5, p.y - ny * 0.5)
            .unwrap_or(0.0);
        let a1 = axis
            .coverage(&ctx.src, p.x + nx * 0.5, p.y + ny * 0.5)
            .unwrap_or(1.0);
        (a1 - a0).abs().max(1e-3)
    };
    let contrast = axis.contrast;
    let s = (ctx.sigma_noise / contrast) / g;
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
    let visibility = if ctx.simplify_faint {
        (CONTRAST_REF / contrast.max(1e-6)).clamp(1.0, MAX_INFLATION)
    } else {
        1.0
    };
    (s.hypot(crate::coverage::DEFAULT_SIGMA_MODEL) * visibility)
        .max(crate::contour::SIGMA_FLOOR)
        .clamp(0.02, 2.0)
}

/// For each face, the ordered list of rings, each ring a sequence of
/// `(edge index, reversed)`.
///
/// Deliberately returns *references into the map* rather than geometry. The caller
/// assembles the actual curves, so the same traversal works whatever alphabet the fitter
/// used — polylines, cubics, or later arcs and primitives — and, more importantly, every
/// face that touches a boundary names the same edge index. Nothing is copied, so the two
/// sides of a boundary cannot drift apart.
///
/// A face sees an edge forwards when it is the edge's `left` face and reversed when it is
/// its `right`, so every ring keeps its face on the left. Rings are assembled greedily by
/// matching each edge's end node to an unused edge starting there; a closed edge is a ring
/// by itself. The outside label (`u16::MAX`) gets no rings. A guard bounds each walk by
/// the face's edge count, so malformed input yields a short ring rather than a hang.
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

#[cfg(test)]
mod refine_tests;
