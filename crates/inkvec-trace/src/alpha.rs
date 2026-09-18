//! Translucent layer recovery from a flat face partition (DESIGN.md §4, S1).
//!
//! 27% of 200 artist-authored SVGs (Noto Emoji + Twemoji) use `opacity`,
//! `fill-opacity` or `stop-opacity`. A tracer that models none of it turns every
//! semi-transparent shape into a mosaic of flat opaque patches, one per distinct
//! background it happens to cross. The benchmark case `stack_overlap` — three
//! overlapping circles, two of them at `fill-opacity="0.85"` — costs 13.2x the
//! ground-truth parameter count for exactly this reason, and it is our worst input by
//! that measure. Three circles are not five flat regions; they are three circles.
//!
//! # The algebra
//!
//! One translucent layer of colour `C` at opacity `a`, sitting over two *different*
//! backgrounds `G1` and `G2`, produces two observed faces:
//!
//! ```text
//!   c_F1 = a·C + (1-a)·c_G1
//!   c_F2 = a·C + (1-a)·c_G2
//!   -----------------------------------------------
//!   c_F1 - c_F2 = (1-a)·(c_G1 - c_G2)
//! ```
//!
//! The unknown layer colour cancels. What remains says the two *observed* differences
//! are parallel, with `(1-a)` the scale between them — a one-parameter least-squares
//! problem whose **parallelism residual is the acceptance test**. Recovering the colour
//! is then substitution: `C = (c_F - (1-a)·c_G) / a`.
//!
//! A single `(F, G)` pair cannot do this: three equations, four unknowns. Two pairs over
//! two genuinely different backgrounds give six equations for four unknowns, and the two
//! degrees of over-determination are the whole basis for believing the answer. So a
//! layer seen against only one background is rejected, always — not because we cannot
//! guess, but because a guess would be indistinguishable from a flat region.
//!
//! # Working space
//!
//! Compositing is linear in **linear light**, so the algebra is done there, using the
//! transfer functions in [`crate::color`]. That is where alpha blending is physically
//! linear, and the parallelism test is a straightness test: a straight blend line in
//! linear light is a curve in sRGB, so getting the space wrong is a systematic error
//! that both rejects real layers and admits coincidences.
//!
//! **But it is the wrong default for SVG, and that is not a theoretical worry.** The
//! spec's compositing space is sRGB, and the benchmark's own `stack_overlap.png` is
//! composited there: blue at 0.85 over white comes out `#5095d5`, where linear light
//! predicts `#749ed6` — 36/255 apart on the red channel. Run the traced faces of that
//! image through [`decompose`] and it correctly reports nothing, because the model does
//! not fit; run them through [`decompose_with`] under [`Space::Srgb`] and both layers
//! come back, `#39a169` at `a = 0.854` and `#3283ce` at `a = 0.855` against a ground
//! truth of `#38a169` and `#3182ce` at 0.85.
//!
//! So [`Space`] is a parameter, and the caller is expected to *estimate* it per image
//! rather than assume it, exactly as DESIGN.md S0 requires. [`decompose`] keeps linear
//! light because that is the physics; a pipeline reading SVG-rendered rasters should
//! pass [`Space::Srgb`], and one that does not know should try both and keep the fit
//! with the smaller residual — they differ by far more than noise, so the choice is
//! decidable from the data.
//!
//! # Peeling
//!
//! Stacked layers only resolve in order. Green over blue over white has no pair anywhere
//! that solves for blue while green still covers half of it — the blue-under-green faces
//! are not blue-over-anything, they are green-over-blue. So: find the best-supported
//! layer, record it, replace each of its faces' colours with the **recovered background**
//! `(c_F - a·C)/(1-a)`, and repeat. In `stack_overlap` green comes off first (four
//! faces), which turns the two green-over-blue faces into blue faces, and blue then has
//! four faces of its own instead of two.
//!
//! # Conservatism
//!
//! A false layer is a visible error — it unions faces that are not one shape and paints
//! them a colour that appears nowhere in the image. Every gate here is set to fail
//! closed:
//!
//! * at least two independent `(face, background)` hypotheses, over backgrounds that are
//!   perceptually far apart (an ill-conditioned `c_G1 - c_G2` makes `(1-a)` meaningless);
//! * the four faces of a hypothesis must form a **quad**: `F1-F2`, `G1-G2`, `F1-G1` and
//!   `F2-G2` adjacent, and the diagonals `F1-G2`, `F2-G1` not *both* so. That is the
//!   geometry of a translucent edge crossing a background edge — four regions meeting at
//!   one point, in that cyclic order — and it is by far the cheapest way to throw out
//!   coincidences between faces that have nothing to do with each other;
//! * the recovered layer's faces must form **one connected region**, since the emitter
//!   unions them into one shape;
//! * the parallelism residual is judged against a propagated pixel-noise model, per
//!   channel, not against a bare distance — and that model is carried through peeling,
//!   because undoing a composite divides the noise by `1-a`;
//! * the standard error on `a` must itself be small. A residual test alone is not enough
//!   once the noise model has inflated: everything fits when nothing is measured;
//! * `a` outside `(0.05, 0.98)` is rejected, as is any `C` outside gamut;
//! * a face may not be both a layer face and a background of the same layer.
//!
//! The gates are **graded by how over-determined the fit is**, because two faces is the
//! algebraic minimum and nothing can make it as safe as three. `n` hypotheses give
//! `3n - 4` degrees of freedom: 2 for `n = 2`, 5 for `n = 3`, 8 for `n = 4`. At `n = 2`
//! the whole case rests on a two-dimensional residual, and a residual band wide enough
//! to absorb 2/255 of colour noise is also wide enough that some unrelated colour
//! quadruples fall inside it. So `n = 2` is held to a much tighter residual and a much
//! larger required background separation than `n >= 3`, where a chance three-way
//! agreement is not a practical concern.
//!
//! Measured on random flat mosaics — uniformly random colours on a 4-connected grid,
//! a harsher palette than real art, where inks are few and repeat:
//!
//! ```text
//!   faces   any spurious layer   spurious layer with >=3 faces
//!      9          0.10%                    0.00%
//!     25          0.42%                    0.03%
//!    100          2.15%                    0.00%
//! ```
//!
//! and the price, on inputs that *do* contain a layer, is recall rather than accuracy:
//!
//! ```text
//!   noise     minimal (2 faces)    stack_overlap (4 faces/layer, 2 layers)
//!   0/255          100%                     100%
//!   1/255         98.8%                     100%
//!   2/255         69.2%                     100%
//!   3/255         43.0%                    88.4%
//! ```
//!
//! A missed layer costs parameters. An invented one is a visible error. The asymmetry in
//! those two tables is the whole design.

use std::collections::HashMap;

use crate::color::{linear_to_srgb, rgb_to_oklab, srgb_to_linear, Oklab};

/// One recovered translucent layer.
#[derive(Debug, Clone)]
pub struct Layer {
    /// Layer colour in **sRGB** `[0, 1]` — what the emitter writes as `fill`.
    pub color: [f32; 3],
    /// Opacity in `(0, 1)` — what the emitter writes as `fill-opacity`.
    pub alpha: f32,
    /// Faces this layer covers, ascending. Their union is the layer's shape.
    pub faces: Vec<usize>,
    /// RMS of `c_F - (a·C + (1-a)·c_G)` over every face and channel, in the working
    /// space. Directly comparable to the per-face colour noise (`sigma_srgb` scaled into
    /// the working space); values far below it mean the layer hypothesis explains the
    /// observation to within measurement error.
    pub residual: f64,
}

/// What [`decompose`] recovered.
#[derive(Debug, Clone, Default)]
pub struct AlphaAnalysis {
    /// Layers in peel order: `layers[0]` is frontmost.
    pub layers: Vec<Layer>,
    /// Faces claimed by no layer, ascending.
    pub opaque_faces: Vec<usize>,
    /// Every face's colour after all layers have been peeled off it — the colour of
    /// whatever lies at the bottom of the stack there. Faces sharing a value here are
    /// the pieces of one opaque base shape, which is what the emitter unions.
    pub base_rgb: Vec<[f32; 3]>,
}

/// The space the compositing algebra is done in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    /// Physically correct: `alpha` blends linear-light radiance.
    Linear,
    /// What a renderer that composites on gamma-encoded values does. SVG's default.
    Srgb,
}

/// Tunables. The defaults are the conservative setting described in the module docs.
#[derive(Debug, Clone, Copy)]
pub struct AlphaOptions {
    /// Uncertainty of a **face's mean colour**, in sRGB units. Not per-pixel noise: a
    /// face colour is already an average over its pixels. Drives every residual test.
    ///
    /// The default, `1.5/255`, is the standard deviation of a uniform `+/- 2/255`
    /// perturbation — the noise level the tests exercise. Raising it accepts more layers
    /// *and* more false ones: the false-alarm rate scales with `sigma^2`.
    pub sigma_srgb: f64,
    /// The [`Space`] the compositing algebra is evaluated in.
    pub space: Space,
    /// Hard cap on peel depth. Three stacked translucent layers is already unusual art.
    pub max_layers: usize,
    /// Faces smaller than this take no part: sub-pixel slivers carry colours that are
    /// themselves blends, and would manufacture layer evidence out of anti-aliasing.
    pub min_face_area: usize,
    /// Cap on neighbours considered per face, largest by area first. Bounds the
    /// hypothesis enumeration on pathological maps; 24 is far above a real face's degree.
    pub max_neighbors: usize,
}

impl Default for AlphaOptions {
    fn default() -> Self {
        Self {
            sigma_srgb: 1.5 / 255.0,
            space: Space::Linear,
            max_layers: 4,
            min_face_area: 4,
            max_neighbors: 24,
        }
    }
}

/// Opacity below this is indistinguishable from no layer at all; above it, from an
/// opaque fill. Either way the recovered colour is not worth trusting.
pub const A_MIN: f64 = 0.05;
/// See [`A_MIN`].
pub const A_MAX: f64 = 0.98;

/// Minimum OKLab separation between the two backgrounds, with three or more
/// hypotheses. Below this, `c_G1 - c_G2` is noise and the least-squares scale it
/// determines is meaningless.
const MIN_BG_SEP: f32 = 0.10;

/// Minimum background separation in the minimal, two-hypothesis case. Much larger,
/// because the acceptance cone around `c_G1 - c_G2` has angular width proportional to
/// `sigma / |c_G1 - c_G2|`: a well-separated pair of backgrounds is what makes the
/// parallelism test sharp, and with only two degrees of over-determination sharpness is
/// the only thing standing between us and a coincidence.
const MIN_BG_SEP_STRICT: f32 = 0.32;

/// A layer must actually change what is beneath it by at least this much in OKLab,
/// otherwise "layer over background" and "one region" are the same claim.
const MIN_LAYER_EFFECT: f32 = 0.03;

/// Chi-square gate on one solved pair, two degrees of freedom. Loose: this only decides
/// what is worth clustering, and the cluster gates below decide what is believed.
const CHI2_PAIR_MAX: f64 = 9.0;

/// Chi-square per degree of freedom for the joint fit, with three or more hypotheses.
const CHI2_CLUSTER_PER_DOF: f64 = 4.0;

/// Total chi-square for the joint fit in the minimal, two-hypothesis case (2 dof).
const CHI2_MINIMAL_MAX: f64 = 4.0;

/// Chi-square gate on one `(face, background)` hypothesis under the fitted layer, three
/// degrees of freedom.
const CHI2_INSTANCE_MAX: f64 = 12.0;

/// How far outside `[0, 1]` a recovered layer colour may stray before it is not a colour.
/// Non-zero because `C = (c_F - (1-a)c_G)/a` amplifies face-colour noise by `1/a`.
const GAMUT_SLACK: f64 = 0.04;

/// Clustering tolerance on `a`.
const TOL_A: f64 = 0.05;
/// Clustering tolerance on `C`, in OKLab, before the `1/a` noise amplification is
/// applied. Loose on purpose: the joint refit and its gates decide, not the clustering.
const TOL_C: f32 = 0.06;

/// Floor on `d(linear)/d(sRGB)`. Without it the noise model claims impossible precision
/// in the shadows, where quantisation and dither dominate.
const MIN_SLOPE: f64 = 0.20;

/// Safety valve on hypothesis enumeration.
const MAX_SOLUTIONS: usize = 400_000;

/// Ceiling on the noise inflation a peel may accumulate. Past this the face carries no
/// usable colour information and should not be evidence for anything.
const MAX_SIGMA_SCALE: f64 = 24.0;

/// Largest standard error on the recovered `a` we will report as a measurement.
/// Approximate — the weights behind it ignore the per-channel transfer slope — but the
/// quantity it bounds is the right one: how well the data actually pin the opacity.
const MAX_ALPHA_SD: f64 = 0.04;

type V3 = [f64; 3];

#[inline]
fn sub3(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn dot3(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Composite `color` at `alpha` over `under`, in linear light. The forward model this
/// module inverts; exposed so callers can round-trip a recovered layer.
pub fn composite(color: [f32; 3], alpha: f32, under: [f32; 3]) -> [f32; 3] {
    composite_in(color, alpha, under, Space::Linear)
}

/// [`composite`] in an explicit space.
pub fn composite_in(color: [f32; 3], alpha: f32, under: [f32; 3], space: Space) -> [f32; 3] {
    let c = to_work(color, space);
    let u = to_work(under, space);
    let a = alpha as f64;
    from_work(
        [
            a * c[0] + (1.0 - a) * u[0],
            a * c[1] + (1.0 - a) * u[1],
            a * c[2] + (1.0 - a) * u[2],
        ],
        space,
    )
}

fn to_work(c: [f32; 3], space: Space) -> V3 {
    match space {
        Space::Linear => [
            srgb_to_linear(c[0]) as f64,
            srgb_to_linear(c[1]) as f64,
            srgb_to_linear(c[2]) as f64,
        ],
        Space::Srgb => [c[0] as f64, c[1] as f64, c[2] as f64],
    }
}

fn from_work(c: V3, space: Space) -> [f32; 3] {
    let cl = |v: f64| v.clamp(0.0, 1.0) as f32;
    match space {
        Space::Linear => [
            linear_to_srgb(cl(c[0])),
            linear_to_srgb(cl(c[1])),
            linear_to_srgb(cl(c[2])),
        ],
        Space::Srgb => [cl(c[0]), cl(c[1]), cl(c[2])],
    }
}

/// `d(working value) / d(sRGB value)` at a given working value — how much a unit of
/// sRGB-domain measurement error is worth here. Near white in linear light this is 2.27,
/// so a 2/255 error on a white background is 0.018 of linear range; ignoring that is how
/// a noise model ends up rejecting every layer over a light background.
fn slope(work_val: f64, space: Space) -> f64 {
    match space {
        Space::Srgb => 1.0,
        Space::Linear => {
            let s = linear_to_srgb(work_val.clamp(0.0, 1.0) as f32) as f64;
            let d = if s <= 0.04045 {
                1.0 / 12.92
            } else {
                (2.4 / 1.055) * ((s + 0.055) / 1.055).powf(1.4)
            };
            d.max(MIN_SLOPE)
        }
    }
}

/// Recover translucent layers from a face partition and its colours.
///
/// * `face_rgb` — one sRGB colour per face, in `[0, 1]`.
/// * `face_area` — pixel count per face, same length.
/// * `adjacency` — unordered face-id pairs that share a boundary. Duplicates, reversals
///   and self-pairs are tolerated; out-of-range ids are ignored.
///
/// Returns layers in peel order (frontmost first), the faces no layer claimed, and every
/// face's fully-peeled base colour. Conservative by construction: on input with no
/// translucency it returns no layers.
pub fn decompose(
    face_rgb: &[[f32; 3]],
    face_area: &[usize],
    adjacency: &[(usize, usize)],
) -> AlphaAnalysis {
    decompose_with(face_rgb, face_area, adjacency, &AlphaOptions::default())
}

/// [`decompose`] with explicit options — notably the compositing space, which DESIGN.md
/// S0 says to estimate per image rather than assume.
pub fn decompose_with(
    face_rgb: &[[f32; 3]],
    face_area: &[usize],
    adjacency: &[(usize, usize)],
    opt: &AlphaOptions,
) -> AlphaAnalysis {
    let n = face_rgb.len();
    if n == 0 || face_area.len() != n {
        return AlphaAnalysis {
            base_rgb: face_rgb.to_vec(),
            ..Default::default()
        };
    }

    // Neighbours: deduplicated, self-pairs dropped, largest-area first then capped.
    let mut nb: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut adj_set: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    for &(u, v) in adjacency {
        if u >= n || v >= n || u == v {
            continue;
        }
        nb[u].push(v);
        nb[v].push(u);
        adj_set.insert((u.min(v), u.max(v)));
    }
    for list in nb.iter_mut() {
        list.sort_unstable();
        list.dedup();
        list.sort_by_key(|&f| std::cmp::Reverse(face_area[f]));
        list.truncate(opt.max_neighbors);
    }

    let mut ctx = Ctx {
        work: face_rgb.iter().map(|&c| to_work(c, opt.space)).collect(),
        lab: face_rgb.iter().map(|&c| rgb_to_oklab(c)).collect(),
        slope: face_rgb
            .iter()
            .map(|&c| {
                let w = to_work(c, opt.space);
                [
                    slope(w[0], opt.space),
                    slope(w[1], opt.space),
                    slope(w[2], opt.space),
                ]
            })
            .collect(),
        scale: vec![1.0; n],
        area: face_area,
        nb,
        adj_set,
        opt: *opt,
        sig2: opt.sigma_srgb * opt.sigma_srgb,
    };

    let mut layers: Vec<Layer> = Vec::new();
    let mut claimed = vec![false; n];

    for _ in 0..opt.max_layers {
        let Some(found) = ctx.peel_once() else { break };
        for &(f, _) in &found.inst {
            claimed[f] = true;
        }
        // Substitute the recovered background so a layer beneath this one becomes
        // visible to the next round. This is the recovered value, not the neighbour's
        // colour: it is what the algebra says is under *this* face.
        //
        // And carry the uncertainty with it. Undoing a composite divides by `1-a`, so a
        // 0.85-opacity layer multiplies the noise on every face it covered by 6.7 — the
        // background is only faintly visible through the layer, and no amount of algebra
        // can recover what the layer hid. Leaving `sigma` at its original value here was
        // a real bug: the *second* layer in `stack_overlap` lost the two faces that had
        // been under the first one, because their peeled colours were judged against a
        // precision they no longer had. The layer colour's own uncertainty, roughly
        // `sigma/sqrt(n)`, enters the same way.
        let a = found.a;
        let n_inst = found.inst.len() as f64;
        for &(f, _) in &found.inst {
            let c = ctx.work[f];
            let bg = [
                ((c[0] - a * found.c[0]) / (1.0 - a)).clamp(0.0, 1.0),
                ((c[1] - a * found.c[1]) / (1.0 - a)).clamp(0.0, 1.0),
                ((c[2] - a * found.c[2]) / (1.0 - a)).clamp(0.0, 1.0),
            ];
            let prev = ctx.scale[f];
            ctx.scale[f] = ((prev * prev + 1.0 / n_inst).sqrt() / (1.0 - a)).min(MAX_SIGMA_SCALE);
            ctx.set_face(f, bg);
        }
        let mut faces: Vec<usize> = found.inst.iter().map(|&(f, _)| f).collect();
        faces.sort_unstable();
        layers.push(Layer {
            color: from_work(found.c, opt.space),
            alpha: a as f32,
            faces,
            residual: found.residual,
        });
    }

    AlphaAnalysis {
        layers,
        opaque_faces: (0..n).filter(|&f| !claimed[f]).collect(),
        base_rgb: ctx.work.iter().map(|&c| from_work(c, opt.space)).collect(),
    }
}

// --- internals ---------------------------------------------------------------------

struct Ctx<'a> {
    /// Current face colours in the working space; rewritten as layers are peeled.
    work: Vec<V3>,
    lab: Vec<Oklab>,
    slope: Vec<[f64; 3]>,
    /// Per-face multiplier on `sigma_srgb`, growing by `1/(1-a)` each time a layer is
    /// peeled off that face.
    scale: Vec<f64>,
    area: &'a [usize],
    nb: Vec<Vec<usize>>,
    adj_set: std::collections::HashSet<(usize, usize)>,
    opt: AlphaOptions,
    sig2: f64,
}

/// One solved `(F1 over G1, F2 over G2)` hypothesis.
struct Sol {
    a: f64,
    c: V3,
    chi2: f64,
    f1: usize,
    g1: usize,
    f2: usize,
    g2: usize,
}

/// A cluster of solutions agreeing on `(a, C)`, after the joint refit.
struct Fitted {
    a: f64,
    c: V3,
    residual: f64,
    inst: Vec<(usize, usize)>,
}

impl Ctx<'_> {
    fn set_face(&mut self, f: usize, w: V3) {
        self.work[f] = w;
        let rgb = from_work(w, self.opt.space);
        self.lab[f] = rgb_to_oklab(rgb);
        self.slope[f] = [
            slope(w[0], self.opt.space),
            slope(w[1], self.opt.space),
            slope(w[2], self.opt.space),
        ];
    }

    #[inline]
    fn usable(&self, f: usize) -> bool {
        self.area[f] >= self.opt.min_face_area
    }

    #[inline]
    fn adjacent(&self, u: usize, v: usize) -> bool {
        self.adj_set.contains(&(u.min(v), u.max(v)))
    }

    /// Enumerate hypotheses, cluster them, refit, and return the best-supported layer.
    fn peel_once(&self) -> Option<Fitted> {
        let sols = self.enumerate();
        if sols.is_empty() {
            return None;
        }

        // Greedy clustering in (a, C), best-fitting solutions first so a cluster's
        // centroid is seeded by its most reliable evidence.
        let mut order: Vec<usize> = (0..sols.len()).collect();
        order.sort_by(|&i, &j| {
            sols[i]
                .chi2
                .partial_cmp(&sols[j].chi2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(i.cmp(&j))
        });

        struct Clu {
            a_sum: f64,
            c_sum: V3,
            n: f64,
            inst: Vec<(usize, usize, f64)>,
        }
        let mut clusters: Vec<Clu> = Vec::new();

        for &si in &order {
            let s = &sols[si];
            let lab_c = rgb_to_oklab(from_work(s.c, self.opt.space));
            let tol_c = TOL_C / (s.a as f32).clamp(0.3, 1.0);
            let mut hit = None;
            for (ci, cl) in clusters.iter().enumerate() {
                let ca = cl.a_sum / cl.n;
                if (ca - s.a).abs() > TOL_A {
                    continue;
                }
                let cc = [cl.c_sum[0] / cl.n, cl.c_sum[1] / cl.n, cl.c_sum[2] / cl.n];
                if rgb_to_oklab(from_work(cc, self.opt.space)).dist(lab_c) <= tol_c {
                    hit = Some(ci);
                    break;
                }
            }
            let ci = match hit {
                Some(ci) => ci,
                None => {
                    if clusters.len() >= 64 {
                        continue;
                    }
                    clusters.push(Clu {
                        a_sum: 0.0,
                        c_sum: [0.0; 3],
                        n: 0.0,
                        inst: Vec::new(),
                    });
                    clusters.len() - 1
                }
            };
            let cl = &mut clusters[ci];
            cl.a_sum += s.a;
            cl.c_sum = [
                cl.c_sum[0] + s.c[0],
                cl.c_sum[1] + s.c[1],
                cl.c_sum[2] + s.c[2],
            ];
            cl.n += 1.0;
            cl.inst.push((s.f1, s.g1, s.chi2));
            cl.inst.push((s.f2, s.g2, s.chi2));
        }

        // Refit each cluster jointly and keep the ones that survive every gate.
        let mut best: Option<(usize, usize, f64, Fitted)> = None;
        for cl in &clusters {
            // One background per layer face: keep the best-fitting hypothesis.
            let mut per_face: HashMap<usize, (usize, f64)> = HashMap::new();
            for &(f, g, chi2) in &cl.inst {
                let e = per_face.entry(f).or_insert((g, f64::MAX));
                if chi2 < e.1 {
                    *e = (g, chi2);
                }
            }
            let mut inst: Vec<(usize, usize)> =
                per_face.iter().map(|(&f, &(g, _))| (f, g)).collect();
            inst.sort_unstable();
            // A face cannot be both over and under the same layer.
            let bgs: std::collections::HashSet<usize> = inst.iter().map(|&(_, g)| g).collect();
            inst.retain(|&(f, _)| !bgs.contains(&f));

            let Some(fit) = self.fit_cluster(inst) else {
                continue;
            };
            let faces = fit.inst.len();
            let area: usize = fit.inst.iter().map(|&(f, _)| self.area[f]).sum();
            let better = match &best {
                None => true,
                Some((bf, ba, br, _)) => {
                    (faces, area) > (*bf, *ba)
                        || ((faces, area) == (*bf, *ba) && fit.residual < *br)
                }
            };
            if better {
                best = Some((faces, area, fit.residual, fit));
            }
        }
        best.map(|(_, _, _, f)| f)
    }

    /// Every `((F1, G1), (F2, G2))` hypothesis that survives the pair-level gates.
    ///
    /// Enumeration is driven by the adjacency `F1 - F2` rather than by all face pairs:
    /// two pieces of one translucent layer meet exactly where the backgrounds beneath
    /// them meet, so a layer that is not connected across the background boundary is not
    /// evidence we are willing to use. That also turns an `O(F^2 · deg^2)` search into
    /// `O(E · deg^2)`.
    fn enumerate(&self) -> Vec<Sol> {
        let mut out = Vec::new();
        for f1 in 0..self.work.len() {
            if !self.usable(f1) {
                continue;
            }
            for &f2 in &self.nb[f1] {
                if f2 <= f1 || !self.usable(f2) {
                    continue;
                }
                for &g1 in &self.nb[f1] {
                    if g1 == f2 || !self.usable(g1) {
                        continue;
                    }
                    if self.lab[f1].dist(self.lab[g1]) < MIN_LAYER_EFFECT {
                        continue;
                    }
                    for &g2 in &self.nb[f2] {
                        if g2 == f1 || g2 == g1 || !self.usable(g2) {
                            continue;
                        }
                        // The quad: F1-F2 and G1-G2 are the same boundary, once above
                        // the layer and once below it. If the two candidate backgrounds
                        // do not meet, the two candidate layer faces are not two views
                        // of one layer crossing one edge.
                        if !self.adjacent(g1, g2) {
                            continue;
                        }
                        // ...and the quad's *diagonals* must not both be edges. Where a
                        // translucent silhouette crosses a background boundary, four
                        // regions meet at a point in the cyclic order F1, F2, G2, G1, so
                        // F1 touches G2 only at that point, never along a curve. Faces
                        // joined across both diagonals are not arranged the way the
                        // hypothesis claims, whatever their colours happen to satisfy.
                        //
                        // *Both*, not either, and that weakening is a concession to
                        // rasterisation. Traced from the real 128px `stack_overlap.png`,
                        // the map carries five adjacencies beyond the twelve the three
                        // circles induce — every one a lens tip two or three pixel
                        // corners long, where an arc crossing should have been a single
                        // point. One of them, white touching blue-over-red, is a diagonal
                        // of exactly the quad that recovers the blue layer. Rejecting on
                        // either diagonal loses that layer on the real image and gains
                        // nothing on a planar map, where diagonals do not arise at all;
                        // it pays only on adjacency graphs too dense to be planar, which
                        // a face map is not. A caller that drops shared boundaries under
                        // a pixel or so in length can afford the stricter rule, and
                        // should.
                        if self.adjacent(f1, g2) && self.adjacent(f2, g1) {
                            continue;
                        }
                        if self.lab[f2].dist(self.lab[g2]) < MIN_LAYER_EFFECT {
                            continue;
                        }
                        if self.lab[g1].dist(self.lab[g2]) < MIN_BG_SEP {
                            continue;
                        }
                        if let Some(s) = self.solve_pair(f1, g1, f2, g2) {
                            out.push(s);
                            if out.len() >= MAX_SOLUTIONS {
                                return out;
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// The two-pair solve of the module header: least-squares scale between `c_F1 - c_F2`
    /// and `c_G1 - c_G2`, accepted on its parallelism residual.
    fn solve_pair(&self, f1: usize, g1: usize, f2: usize, g2: usize) -> Option<Sol> {
        let (cf1, cf2) = (self.work[f1], self.work[f2]);
        let (cg1, cg2) = (self.work[g1], self.work[g2]);
        let df = sub3(cf1, cf2);
        let dg = sub3(cg1, cg2);
        let dgn2 = dot3(dg, dg);
        if dgn2 < 1e-6 {
            return None;
        }
        let s = dot3(df, dg) / dgn2;
        let a = 1.0 - s;
        if !(A_MIN..=A_MAX).contains(&a) {
            return None;
        }

        // Parallelism residual, weighted by propagated per-face colour noise. Four face
        // colours contribute; the two backgrounds enter scaled by s.
        let mut chi2 = 0.0;
        for k in 0..3 {
            let r = df[k] - s * dg[k];
            let s1 = self.slope[f1][k] * self.scale[f1];
            let s2 = self.slope[f2][k] * self.scale[f2];
            let t1 = self.slope[g1][k] * self.scale[g1];
            let t2 = self.slope[g2][k] * self.scale[g2];
            let var = self.sig2 * (s1 * s1 + s2 * s2 + s * s * (t1 * t1 + t2 * t2));
            if var <= 0.0 {
                return None;
            }
            chi2 += r * r / var;
        }
        if !(chi2.is_finite() && chi2 <= CHI2_PAIR_MAX) {
            return None;
        }

        // C = (mean(c_F) - s·mean(c_G)) / a, the least-squares layer colour for this pair.
        let mut c = [0.0f64; 3];
        for k in 0..3 {
            let u = 0.5 * ((cf1[k] + cf2[k]) - s * (cg1[k] + cg2[k]));
            c[k] = u / a;
            if !(-GAMUT_SLACK..=1.0 + GAMUT_SLACK).contains(&c[k]) {
                return None;
            }
        }

        Some(Sol {
            a,
            c,
            chi2,
            f1,
            g1,
            f2,
            g2,
        })
    }

    /// Joint least squares over a set of `(face, background)` hypotheses.
    ///
    /// With `s = 1-a` and `u = a·C` the model `c_F = u + s·c_G` is *linear* in `(u, s)`,
    /// so the fit is a centred covariance ratio — no iteration, no initialisation, and
    /// the same estimator as the two-pair case generalised to n pairs.
    fn solve_ls(&self, inst: &[(usize, usize)]) -> Option<(f64, V3)> {
        if inst.len() < 2 {
            return None;
        }
        // Inverse-variance weights. After a peel the faces that were under the previous
        // layer carry several times the noise of the ones that were not, and averaging
        // them as equals throws away the good measurements.
        let w: Vec<f64> = inst
            .iter()
            .map(|&(f, g)| 1.0 / (self.scale[f].powi(2) + self.scale[g].powi(2)))
            .collect();
        let wsum: f64 = w.iter().sum();
        if wsum <= 0.0 || !wsum.is_finite() {
            return None;
        }
        let mut mf = [0.0f64; 3];
        let mut mg = [0.0f64; 3];
        for (i, &(f, g)) in inst.iter().enumerate() {
            for k in 0..3 {
                mf[k] += w[i] * self.work[f][k];
                mg[k] += w[i] * self.work[g][k];
            }
        }
        for k in 0..3 {
            mf[k] /= wsum;
            mg[k] /= wsum;
        }

        let (mut num, mut den) = (0.0f64, 0.0f64);
        for (i, &(f, g)) in inst.iter().enumerate() {
            for k in 0..3 {
                let dg = self.work[g][k] - mg[k];
                num += w[i] * dg * (self.work[f][k] - mf[k]);
                den += w[i] * dg * dg;
            }
        }
        // Conditioning: the backgrounds must genuinely spread. Two backgrounds a
        // just-noticeable difference apart determine nothing. Normalised back to
        // unit weights so the threshold means the same thing however noisy the faces.
        if den / wsum * (inst.len() as f64) < 2e-3 {
            return None;
        }

        // The opacity must be *measured*, not merely consistent. `Var(s) ~ sigma^2/den`,
        // so a fit over faces whose colours the peeling has already made uncertain can
        // satisfy every residual test while pinning nothing: the residual is small
        // because the error bars are enormous. That is how a third, imaginary layer
        // appeared on the twice-peeled faces of `stack_overlap` under noise. A standard
        // error on `a` is the honest thing to threshold, and it is the one gate that
        // tightens rather than loosens as evidence degrades.
        if (self.sig2 / den).sqrt() > MAX_ALPHA_SD {
            return None;
        }
        let a = 1.0 - num / den;
        if !(A_MIN..=A_MAX).contains(&a) {
            return None;
        }
        let s = 1.0 - a;

        let mut c = [0.0f64; 3];
        for k in 0..3 {
            c[k] = (mf[k] - s * mg[k]) / a;
            if !(-GAMUT_SLACK..=1.0 + GAMUT_SLACK).contains(&c[k]) {
                return None;
            }
        }
        Some((a, c))
    }

    /// Goodness of fit of one `(face, background)` hypothesis against a stated layer.
    fn instance_chi2(&self, f: usize, g: usize, a: f64, c: V3) -> f64 {
        let s = 1.0 - a;
        let mut chi2 = 0.0;
        for (k, &ck) in c.iter().enumerate() {
            let r = self.work[f][k] - (a * ck + s * self.work[g][k]);
            let sf = self.slope[f][k] * self.scale[f];
            let sg = self.slope[g][k] * self.scale[g];
            let var = self.sig2 * (sf * sf + s * s * sg * sg);
            if var <= 0.0 {
                return f64::INFINITY;
            }
            chi2 += r * r / var;
        }
        chi2
    }

    /// Every face the stated layer explains, with the neighbour it is lying over.
    ///
    /// The pair enumeration finds a layer from *one* crossing of *one* background edge.
    /// A layer that crosses three backgrounds has more faces than any single pair knows
    /// about, and under measurement noise the pair that seeded the cluster is not
    /// necessarily the one that assigns each face its best background. So once `(a, C)`
    /// is on the table, ask the whole map: which faces does this layer explain, and over
    /// what? That is what makes the recovered face set complete — and it cuts both ways,
    /// because a layer invented from a coincidence gains no new faces from it: every
    /// addition is another three-dimensional colour equation that has to come out right.
    fn consensus(&self, a: f64, c: V3) -> Vec<(usize, usize)> {
        let mut out: Vec<(usize, usize)> = Vec::new();
        for f in 0..self.work.len() {
            if !self.usable(f) {
                continue;
            }
            let mut best: Option<(usize, f64)> = None;
            for &g in &self.nb[f] {
                if !self.usable(g) || self.lab[f].dist(self.lab[g]) < MIN_LAYER_EFFECT {
                    continue;
                }
                let chi2 = self.instance_chi2(f, g, a, c);
                if chi2 > CHI2_INSTANCE_MAX {
                    continue;
                }
                match best {
                    Some((_, b)) if b <= chi2 => {}
                    _ => best = Some((g, chi2)),
                }
            }
            if let Some((g, _)) = best {
                out.push((f, g));
            }
        }
        // A face cannot be both over and under the same layer.
        let bgs: std::collections::HashSet<usize> = out.iter().map(|&(_, g)| g).collect();
        out.retain(|&(f, _)| !bgs.contains(&f));
        out
    }

    /// Do the layer's faces form one connected region of the map?
    fn connected(&self, inst: &[(usize, usize)]) -> bool {
        let Some(&(start, _)) = inst.first() else {
            return false;
        };
        let members: std::collections::HashSet<usize> = inst.iter().map(|&(f, _)| f).collect();
        let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut stack = vec![start];
        seen.insert(start);
        while let Some(f) = stack.pop() {
            for &g in &self.nb[f] {
                if members.contains(&g) && seen.insert(g) {
                    stack.push(g);
                }
            }
        }
        seen.len() == members.len()
    }

    /// Fit a cluster's seed hypotheses, grow the fit to consensus, and apply the gates.
    fn fit_cluster(&self, seed: Vec<(usize, usize)>) -> Option<Fitted> {
        let mut inst = seed;
        let (mut a, mut c) = self.solve_ls(&inst)?;
        for _ in 0..4 {
            let next = self.consensus(a, c);
            if next.len() < 2 {
                return None;
            }
            if next == inst {
                break;
            }
            inst = next;
            let solved = self.solve_ls(&inst)?;
            a = solved.0;
            c = solved.1;
        }

        // The layer's faces must be one connected region of the map. This is the
        // emitter's contract: the faces get unioned into a single filled shape, so a
        // recovered "layer" whose pieces are scattered across the image is not a layer
        // even if every one of its pieces fits the colour model. It is also the gate
        // that keeps the consensus step honest — growing a coincidence by one more face
        // now requires that face to be adjacent to the ones already claimed.
        if !self.connected(&inst) {
            return None;
        }

        // Two hypotheses is the algebraic minimum and gets the strict tier; three or
        // more is over-determined enough that a coincidence is not a practical concern.
        let minimal = inst.len() == 2;

        // Backgrounds must be far apart perceptually, not merely numerically.
        let mut sep = 0.0f32;
        for i in 0..inst.len() {
            for j in i + 1..inst.len() {
                sep = sep.max(self.lab[inst[i].1].dist(self.lab[inst[j].1]));
            }
        }
        if sep
            < if minimal {
                MIN_BG_SEP_STRICT
            } else {
                MIN_BG_SEP
            }
        {
            return None;
        }

        let n = inst.len() as f64;
        let s = 1.0 - a;
        let mut chi2_total = 0.0f64;
        let mut sq = 0.0f64;
        for &(f, g) in &inst {
            let chi2_i = self.instance_chi2(f, g, a, c);
            if !(chi2_i.is_finite() && chi2_i <= CHI2_INSTANCE_MAX) {
                return None;
            }
            chi2_total += chi2_i;
            for (k, &ck) in c.iter().enumerate() {
                let r = self.work[f][k] - (a * ck + s * self.work[g][k]);
                sq += r * r;
            }
        }
        let budget = if minimal {
            CHI2_MINIMAL_MAX
        } else {
            CHI2_CLUSTER_PER_DOF * (3.0 * n - 4.0)
        };
        if !(chi2_total.is_finite() && chi2_total <= budget) {
            return None;
        }

        inst.sort_unstable();
        Some(Fitted {
            a,
            c,
            residual: (sq / (3.0 * n)).sqrt(),
            inst,
        })
    }
}
