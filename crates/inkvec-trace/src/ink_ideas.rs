//! Five experimental rules for deciding which colours are inks. Selected by `INKVEC_INK_IDEA`.
//! Nothing here runs unless that variable is set; the shipped trace is untouched.
//!
//! **Why this module exists.** The palette's acceptance test is mispriced on both sides of one
//! inequality (`color::extract_palette_mdl`, the `worth_it` escape):
//!
//! * the COST side charges a new ink `lambda * PARAMS_PER_INK`, three parameters, which is the
//!   price of stating a colour. What a spurious ink actually costs is the geometry it creates:
//!   measured at 0.106 emitted parameters per pixel of planar-map boundary (56 traces, 512 px),
//!   a region with an 8-span boundary costs about 51 parameters, and the 45 extra regions a
//!   restored icon produced cost about 2,300. That is 18.7 nats charged against ~14,000 spent.
//! * the EVIDENCE side counts every pixel as an independent observation. A smooth tint left by a
//!   restorer, or a JPEG ringing band, is one spatially correlated field, not ten thousand
//!   independent measurements, so a 0.02 offset over 10,000 pixels clears the bar ~28,000 times.
//!
//! Every rule below attacks one of those errors, in a different way:
//!
//! 1. `idea1_ink_mdl`: merge whole inks, with evidence discounted by spatial correlation and
//!    cost charged for the boundaries and regions the ink creates.
//! 2. `idea2_region_mdl`: the same objective at REGION granularity, greedy by priority queue,
//!    so one spurious component can merge while the rest of its ink stays.
//! 3. `idea3_contrast_potts`: the existing Potts smoother with a contrast-sensitive boundary
//!    price. Cutting along an image edge is nearly free, cutting through a smooth field is
//!    expensive. `3b` uses the measured noise as sigma.
//! 4. `idea4_edge_witness`: an ink survives only if the boundary around it carries more image
//!    contrast than its own interior does.
//! 5. `idea5_inks_from_edges`: inks are read off the plateaus either side of strong edges; a
//!    palette ink no edge ever witnesses is dissolved into its neighbours.
//!
//! All five relabel pixels onto existing palette entries; none adds an ink. The stages after
//! labelling (despeckle, gradient band merging, carving, fill fitting) run exactly as shipped,
//! so a rule's effect is measured through the whole trace rather than on the labels alone.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use crate::color::{de00, rgb_to_oklab, Palette};

/// Emitted parameters per pixel of planar-map boundary, measured over 56 traces at 512 px
/// (median 0.106; clean 0.087, JPEG q40 0.119). Label contacts are counted on 4-neighbour
/// pairs, which overstate a diagonal boundary by up to sqrt 2, so the default is a little lower.
const KAPPA_DEFAULT: f64 = 0.10;

/// Below this many interior pixels, a region's mean and variance are not trusted.
const MIN_INTERIOR: f64 = 16.0;

/// Below this many interior neighbour pairs, spatial correlation is not estimated (rho = 0,
/// i.e. pixels are treated as independent). This is what keeps a thin stroke, which has almost
/// no interior, from having its evidence discounted to nothing.
const MIN_PAIRS: f64 = 64.0;

/// What the rules need to know about the trace they are running inside.
pub struct Ctx {
    /// Image width in pixels.
    pub w: usize,
    /// Image height in pixels.
    pub h: usize,
    /// The noise level the rest of the trace will use, after any measured escalation.
    pub sigma: f64,
    /// `regularize::residual_sigma`, measured whether or not the gate opened.
    pub measured_sigma: f64,
    /// `coverage::intake_scale`: 1.0 on a native render, wider on blurred or upscaled input.
    pub edge_width: f64,
}

/// Run the rule named by `idea` and return how many pixels changed label.
pub fn apply(idea: &str, rgb: &[[f32; 3]], labels: &mut [u16], pal: &Palette, ctx: &Ctx) -> usize {
    if rgb.len() < ctx.w * ctx.h || labels.len() < ctx.w * ctx.h || ctx.w < 3 || ctx.h < 3 {
        return 0;
    }
    let before = labels.to_vec();
    match idea.trim() {
        "1" => idea1_ink_mdl(rgb, labels, pal, ctx),
        "2" => idea2_region_mdl(rgb, labels, pal, ctx),
        "3" => idea3_contrast_potts(rgb, labels, pal, ctx, ctx.sigma),
        "3b" => idea3_contrast_potts(rgb, labels, pal, ctx, ctx.sigma.max(ctx.measured_sigma)),
        "4" => idea4_edge_witness(rgb, labels, pal, ctx),
        "5" => idea5_inks_from_edges(rgb, labels, pal, ctx),
        _ => {}
    }
    before
        .iter()
        .zip(labels.iter())
        .filter(|(a, b)| a != b)
        .count()
}

fn lambda_of(ctx: &Ctx) -> f64 {
    0.5 * ((ctx.w * ctx.h).max(3) as f64).ln()
}

fn lum(c: [f64; 3]) -> f64 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn px(rgb: &[[f32; 3]], i: usize) -> [f64; 3] {
    [rgb[i][0] as f64, rgb[i][1] as f64, rgb[i][2] as f64]
}

fn d2(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|c| (a[c] - b[c]).powi(2)).sum()
}

/// All four (existing) neighbours carry the same label.
fn interior(labels: &[u16], w: usize, h: usize, i: usize) -> bool {
    let (x, y) = (i % w, i / w);
    let l = labels[i];
    (x == 0 || labels[i - 1] == l)
        && (x + 1 >= w || labels[i + 1] == l)
        && (y == 0 || labels[i - w] == l)
        && (y + 1 >= h || labels[i + w] == l)
}

/// Measure residuals against a fitted PLANE (the fill model's linear gradient) rather than the
/// flat mean. Default on; `INKVEC_INK_PLANE=0` restores the flat version.
///
/// Found by smoke test before the full run: against a flat mean, a genuine gradient-filled box
/// has a smoothly correlated residual, so its evidence was discounted to almost nothing and rule 1
/// collapsed a CLEAN diagram from 8 inks to 2. The correlation discount is meant for what the
/// fill model cannot explain -- a tint, a ringing band -- not for shading it fits anyway.
fn plane_residuals() -> bool {
    inkvec_core::env::switch("INKVEC_INK_PLANE", true)
}

/// Sufficient statistics for one group of pixels (an ink, or a region). Every field is additive,
/// so two groups merge by summing.
#[derive(Clone, Default)]
struct Acc {
    n: f64,
    n_int: f64,
    s1: [f64; 3],
    s2: [f64; 3],
    // interior coordinate moments, for the plane fit
    sx: f64,
    sy: f64,
    sxx: f64,
    syy: f64,
    sxy: f64,
    svx: [f64; 3],
    svy: [f64; 3],
    num: f64,
    den: f64,
    pairs: f64,
    fallback: [f64; 3],
}

impl Acc {
    fn mean(&self) -> [f64; 3] {
        if self.n_int >= MIN_INTERIOR {
            [
                self.s1[0] / self.n_int,
                self.s1[1] / self.n_int,
                self.s1[2] / self.n_int,
            ]
        } else {
            self.fallback
        }
    }
    /// Per-channel least-squares plane `v = a + b x + c y` over interior pixels, or the flat
    /// mean when the plane is disabled or the geometry is degenerate (a line of pixels).
    fn plane(&self) -> [[f64; 3]; 3] {
        let m = self.mean();
        let flat = [[m[0], 0.0, 0.0], [m[1], 0.0, 0.0], [m[2], 0.0, 0.0]];
        if !plane_residuals() || self.n_int < MIN_INTERIOR {
            return flat;
        }
        let a = [
            [self.n_int, self.sx, self.sy],
            [self.sx, self.sxx, self.sxy],
            [self.sy, self.sxy, self.syy],
        ];
        let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
        let scale = (self.sxx + self.syy).max(1.0) * self.n_int;
        if det.abs() < 1e-9 * scale * scale.max(1.0) {
            return flat;
        }
        let mut out = flat;
        for c in 0..3 {
            let b = [self.s1[c], self.svx[c], self.svy[c]];
            let col = |k: usize| -> f64 {
                let mut m2 = a;
                for r in 0..3 {
                    m2[r][k] = b[r];
                }
                m2[0][0] * (m2[1][1] * m2[2][2] - m2[1][2] * m2[2][1])
                    - m2[0][1] * (m2[1][0] * m2[2][2] - m2[1][2] * m2[2][0])
                    + m2[0][2] * (m2[1][0] * m2[2][1] - m2[1][1] * m2[2][0])
            };
            out[c] = [col(0) / det, col(1) / det, col(2) / det];
        }
        out
    }
    /// Residual variance per channel, against the plane (or the flat mean).
    fn var(&self) -> f64 {
        if self.n_int < MIN_INTERIOR {
            return 0.0;
        }
        let p = self.plane();
        (0..3)
            .map(|c| {
                let sse = self.s2[c]
                    - (p[c][0] * self.s1[c] + p[c][1] * self.svx[c] + p[c][2] * self.svy[c]);
                (sse / self.n_int).max(0.0)
            })
            .sum::<f64>()
            / 3.0
    }
    /// Lag-1 spatial correlation of the interior residual, clamped to [0, 0.98].
    ///
    /// The discount this feeds exists for one case: a smooth residual of NOISE-LIKE amplitude,
    /// which is a tint or a ringing band, one correlated field rather than n independent
    /// observations. It must not fire on a smooth residual of LARGE amplitude, which is shading
    /// the fill fitter will explain as a gradient and whose ink is real.
    ///
    /// The first version did not make that distinction and the screen set caught it: noto-emoji,
    /// radial gradients at 128 px, went from dE00 0.389 to 3.553 as real inks were merged into
    /// their neighbours. A 512 px sample had been neutral, because its emoji were flat.
    fn rho(&self) -> f64 {
        let v = self.var();
        let quantisation_only = v < (0.25f64 / 255.0).powi(2);
        let structure_not_tint =
            v > (inkvec_core::env::number("INKVEC_INK_TINT_MAX").unwrap_or(3.0) / 255.0).powi(2);
        if self.pairs < MIN_PAIRS || self.den <= 1e-14 || quantisation_only || structure_not_tint {
            0.0
        } else {
            (self.num / self.den).clamp(0.0, 0.98)
        }
    }
    /// Effective number of independent observations: n((1 - rho)/(1 + rho))^2, the 2-D form
    /// of the AR(1) variance inflation factor.
    fn n_eff(&self) -> f64 {
        let r = self.rho();
        self.n * ((1.0 - r) / (1.0 + r)).powi(2)
    }
    fn absorb(&mut self, o: &Acc) {
        self.n += o.n;
        self.n_int += o.n_int;
        for c in 0..3 {
            self.s1[c] += o.s1[c];
            self.s2[c] += o.s2[c];
            self.svx[c] += o.svx[c];
            self.svy[c] += o.svy[c];
        }
        self.sx += o.sx;
        self.sy += o.sy;
        self.sxx += o.sxx;
        self.syy += o.syy;
        self.sxy += o.sxy;
        self.num += o.num;
        self.den += o.den;
        self.pairs += o.pairs;
        if o.n > self.n - o.n {
            self.fallback = o.fallback;
        }
    }
}

/// Evidence, in nats, that `small` is NOT the same ink as `big`: the offset between them,
/// measured against the larger of the pixel noise and `small`'s own residual spread, over
/// `small`'s EFFECTIVE sample size.
fn evidence(small: &Acc, big: &Acc, sigma: f64) -> f64 {
    // The scale is the MEASUREMENT noise, never the region's own spread: a region's spread is
    // either a tint (handled by n_eff) or shading (real, and must keep its evidence). Using the
    // spread here was the second half of the emoji regression: a radial gradient's variance
    // divided its own evidence down to nothing.
    let se2 = (sigma * sigma).max(1e-12);
    0.5 * small.n_eff() * d2(small.mean(), big.mean()) / se2
}

/// Accumulate per-group statistics for any grouping of pixels (`group[i]` indexes `accs`).
fn accumulate(
    rgb: &[[f32; 3]],
    labels: &[u16],
    group: &[usize],
    accs: &mut [Acc],
    w: usize,
    h: usize,
) {
    let n = w * h;
    let int: Vec<bool> = (0..n).map(|i| interior(labels, w, h, i)).collect();
    for i in 0..n {
        let a = &mut accs[group[i]];
        a.n += 1.0;
        if int[i] {
            let v = px(rgb, i);
            let (x, y) = ((i % w) as f64, (i / w) as f64);
            a.n_int += 1.0;
            a.sx += x;
            a.sy += y;
            a.sxx += x * x;
            a.syy += y * y;
            a.sxy += x * y;
            for c in 0..3 {
                a.s1[c] += v[c];
                a.s2[c] += v[c] * v[c];
                a.svx[c] += v[c] * x;
                a.svy[c] += v[c] * y;
            }
        }
    }
    let planes: Vec<[[f64; 3]; 3]> = accs.iter().map(|a| a.plane()).collect();
    let fitted_lum = |g: usize, i: usize| -> f64 {
        let (x, y) = ((i % w) as f64, (i / w) as f64);
        let p = planes[g];
        lum([
            p[0][0] + p[0][1] * x + p[0][2] * y,
            p[1][0] + p[1][1] * x + p[1][2] * y,
            p[2][0] + p[2][1] * x + p[2][2] * y,
        ])
    };
    for i in 0..n {
        if !int[i] {
            continue;
        }
        let g = group[i];
        let ri = lum(px(rgb, i)) - fitted_lum(g, i);
        let (x, y) = (i % w, i / w);
        for j in [
            if x + 1 < w { Some(i + 1) } else { None },
            if y + 1 < h { Some(i + w) } else { None },
        ]
        .into_iter()
        .flatten()
        {
            if int[j] && group[j] == g {
                let rj = lum(px(rgb, j)) - fitted_lum(g, j);
                let a = &mut accs[g];
                a.num += ri * rj;
                a.den += 0.5 * (ri * ri + rj * rj);
                a.pairs += 1.0;
            }
        }
    }
}

/// Connected components of equal label, 4-connected, with u32 ids.
fn components(labels: &[u16], w: usize, h: usize) -> (Vec<u32>, Vec<u16>) {
    let n = w * h;
    let mut comp = vec![u32::MAX; n];
    let mut ink = Vec::new();
    let mut stack = Vec::new();
    for seed in 0..n {
        if comp[seed] != u32::MAX {
            continue;
        }
        let id = ink.len() as u32;
        let l = labels[seed];
        ink.push(l);
        comp[seed] = id;
        stack.push(seed);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            for j in [
                if x > 0 { Some(i - 1) } else { None },
                if x + 1 < w { Some(i + 1) } else { None },
                if y > 0 { Some(i - w) } else { None },
                if y + 1 < h { Some(i + w) } else { None },
            ]
            .into_iter()
            .flatten()
            {
                if labels[j] == l && comp[j] == u32::MAX {
                    comp[j] = id;
                    stack.push(j);
                }
            }
        }
    }
    (comp, ink)
}

/// Apply a set of whole-ink merges found in one pass, never letting an ink be both a source and
/// a target in the same pass (so merges cannot chain through a colour that is itself moving).
fn batch_merge(labels: &mut [u16], k: usize, mut cands: Vec<(f64, usize, usize)>) -> bool {
    if cands.is_empty() {
        return false;
    }
    cands.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut src = vec![false; k];
    let mut dst = vec![false; k];
    let mut remap: Vec<u16> = (0..k as u16).collect();
    let mut any = false;
    for (_, s, t) in cands {
        if src[t] || dst[s] || src[s] {
            continue;
        }
        src[s] = true;
        dst[t] = true;
        remap[s] = t as u16;
        any = true;
    }
    for l in labels.iter_mut() {
        *l = remap[*l as usize];
    }
    any
}

// ---------------------------------------------------------------------------------------------
// 1. Whole-ink description length with boundary cost and correlated evidence.
// ---------------------------------------------------------------------------------------------

fn idea1_ink_mdl(rgb: &[[f32; 3]], labels: &mut [u16], pal: &Palette, ctx: &Ctx) {
    let (w, h, k) = (ctx.w, ctx.h, pal.rgb.len());
    let lambda = lambda_of(ctx);
    let kappa = inkvec_core::env::number("INKVEC_INK_KAPPA").unwrap_or(KAPPA_DEFAULT);
    for _ in 0..48 {
        let group: Vec<usize> = labels.iter().map(|&l| l as usize).collect();
        let mut accs = vec![Acc::default(); k];
        for (l, a) in accs.iter_mut().enumerate() {
            a.fallback = [
                pal.rgb[l][0] as f64,
                pal.rgb[l][1] as f64,
                pal.rgb[l][2] as f64,
            ];
        }
        accumulate(rgb, labels, &group, &mut accs, w, h);
        let mut contact = vec![0f64; k * k];
        for i in 0..w * h {
            let (x, y) = (i % w, i / w);
            for j in [
                if x + 1 < w { Some(i + 1) } else { None },
                if y + 1 < h { Some(i + w) } else { None },
            ]
            .into_iter()
            .flatten()
            {
                let (a, b) = (labels[i] as usize, labels[j] as usize);
                if a != b {
                    contact[a * k + b] += 1.0;
                    contact[b * k + a] += 1.0;
                }
            }
        }
        let (_, comp_ink) = components(labels, w, h);
        let mut comps = vec![0f64; k];
        for &l in &comp_ink {
            comps[l as usize] += 1.0;
        }
        let largest = (0..k)
            .max_by(|&a, &b| accs[a].n.total_cmp(&accs[b].n))
            .unwrap_or(0);
        let mut cands = Vec::new();
        for s in 0..k {
            if s == largest || accs[s].n == 0.0 {
                continue;
            }
            let mut best = (f64::INFINITY, usize::MAX);
            for t in 0..k {
                let c = contact[s * k + t];
                if t == s || c <= 0.0 || accs[t].n == 0.0 {
                    continue;
                }
                let dj =
                    evidence(&accs[s], &accs[t], ctx.sigma) - lambda * (3.0 * comps[s] + kappa * c);
                if dj < best.0 {
                    best = (dj, t);
                }
            }
            if best.0 < 0.0 {
                cands.push((best.0, s, best.1));
            }
        }
        if !batch_merge(labels, k, cands) {
            break;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 2. Region-adjacency description length, greedy by priority queue.
// ---------------------------------------------------------------------------------------------

#[derive(PartialEq)]
struct Entry {
    gain: f64,
    a: u32,
    b: u32,
    va: u32,
    vb: u32,
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Entry {
    fn cmp(&self, o: &Self) -> Ordering {
        self.gain.total_cmp(&o.gain)
    }
}

fn idea2_region_mdl(rgb: &[[f32; 3]], labels: &mut [u16], pal: &Palette, ctx: &Ctx) {
    let (w, h) = (ctx.w, ctx.h);
    let n = w * h;
    let lambda = lambda_of(ctx);
    let kappa = inkvec_core::env::number("INKVEC_INK_KAPPA").unwrap_or(KAPPA_DEFAULT);
    let (comp, ink) = components(labels, w, h);
    let r = ink.len();
    let group: Vec<usize> = comp.iter().map(|&c| c as usize).collect();
    let mut accs = vec![Acc::default(); r];
    for (g, a) in accs.iter_mut().enumerate() {
        let c = pal.rgb[ink[g] as usize];
        a.fallback = [c[0] as f64, c[1] as f64, c[2] as f64];
    }
    accumulate(rgb, labels, &group, &mut accs, w, h);
    let mut nb: Vec<HashMap<u32, f64>> = vec![HashMap::new(); r];
    for i in 0..n {
        let (x, y) = (i % w, i / w);
        for j in [
            if x + 1 < w { Some(i + 1) } else { None },
            if y + 1 < h { Some(i + w) } else { None },
        ]
        .into_iter()
        .flatten()
        {
            let (a, b) = (comp[i], comp[j]);
            if a != b {
                *nb[a as usize].entry(b).or_default() += 1.0;
                *nb[b as usize].entry(a).or_default() += 1.0;
            }
        }
    }
    let mut ink_of: Vec<u16> = ink.clone();
    let mut parent: Vec<u32> = (0..r as u32).collect();
    let mut version = vec![0u32; r];
    let gain_of = |accs: &[Acc], a: usize, b: usize, cont: f64| -> f64 {
        let (big, small) = if accs[a].n >= accs[b].n {
            (a, b)
        } else {
            (b, a)
        };
        // positive gain = merging lowers the description length
        lambda * (3.0 + kappa * cont) - evidence(&accs[small], &accs[big], ctx.sigma)
    };
    let mut heap = BinaryHeap::new();
    for a in 0..r {
        for (&b, &cont) in &nb[a] {
            if (a as u32) < b {
                heap.push(Entry {
                    gain: gain_of(&accs, a, b as usize, cont),
                    a: a as u32,
                    b,
                    va: 0,
                    vb: 0,
                });
            }
        }
    }
    let cap = inkvec_core::env::number("INKVEC_INK_MAX_MERGES").unwrap_or(400_000.0) as usize;
    let mut merges = 0;
    while let Some(e) = heap.pop() {
        if e.gain <= 0.0 || merges >= cap {
            break;
        }
        let (a, b) = (e.a as usize, e.b as usize);
        if parent[a] != e.a || parent[b] != e.b || version[a] != e.va || version[b] != e.vb {
            continue;
        }
        let cont = match nb[a].get(&e.b) {
            Some(&c) => c,
            None => continue,
        };
        // recheck with current stats: the entry may have been pushed before either side grew
        if gain_of(&accs, a, b, cont) <= 0.0 {
            continue;
        }
        let (big, small) = if accs[a].n >= accs[b].n {
            (a, b)
        } else {
            (b, a)
        };
        let moved = std::mem::take(&mut accs[small]);
        accs[big].absorb(&moved);
        // the survivor keeps its own ink; its mean stays its own unless the absorbed side was larger
        ink_of[small] = ink_of[big];
        parent[small] = big as u32;
        let small_nb = std::mem::take(&mut nb[small]);
        for (c, cc) in small_nb {
            let cu = c as usize;
            nb[cu].remove(&(small as u32));
            if cu == big {
                continue;
            }
            *nb[cu].entry(big as u32).or_default() += cc;
            *nb[big].entry(c).or_default() += cc;
        }
        nb[big].remove(&(small as u32));
        version[big] += 1;
        version[small] += 1;
        merges += 1;
        let vb = version[big];
        let entries: Vec<(u32, f64)> = nb[big].iter().map(|(&c, &cc)| (c, cc)).collect();
        for (c, cc) in entries {
            let cu = c as usize;
            let (lo, hi) = if big < cu { (big, cu) } else { (cu, big) };
            heap.push(Entry {
                gain: gain_of(&accs, big, cu, cc),
                a: lo as u32,
                b: hi as u32,
                va: if lo == big { vb } else { version[cu] },
                vb: if hi == big { vb } else { version[cu] },
            });
        }
    }
    let find = |mut x: usize, parent: &[u32]| -> usize {
        while parent[x] as usize != x {
            x = parent[x] as usize;
        }
        x
    };
    for i in 0..n {
        labels[i] = ink_of[find(comp[i] as usize, &parent)];
    }
}

// ---------------------------------------------------------------------------------------------
// 3. Contrast-sensitive Potts.
// ---------------------------------------------------------------------------------------------

fn idea3_contrast_potts(
    rgb: &[[f32; 3]],
    labels: &mut [u16],
    pal: &Palette,
    ctx: &Ctx,
    sigma: f64,
) {
    let (w, h) = (ctx.w, ctx.h);
    let n = w * h;
    let inv2s2 = 1.0 / (2.0 * sigma.max(1e-6).powi(2));
    // The existing smoother's boundary price, in the same nats-per-neighbour-pair units.
    let price =
        ((n.max(3)) as f64).ln() * inkvec_core::env::number("INKVEC_POTTS_SCALE").unwrap_or(1.0);
    // Boykov-Jolly contrast weights: beta from the mean squared neighbour difference.
    let (mut sum, mut cnt) = (0f64, 0f64);
    for i in 0..n {
        let (x, y) = (i % w, i / w);
        if x + 1 < w {
            sum += d2(px(rgb, i), px(rgb, i + 1));
            cnt += 1.0;
        }
        if y + 1 < h {
            sum += d2(px(rgb, i), px(rgb, i + w));
            cnt += 1.0;
        }
    }
    let mean = (sum / cnt.max(1.0)).max(1e-12);
    let wr: Vec<f64> = (0..n)
        .map(|i| {
            if i % w + 1 < w {
                (-d2(px(rgb, i), px(rgb, i + 1)) / (2.0 * mean)).exp()
            } else {
                0.0
            }
        })
        .collect();
    let wd: Vec<f64> = (0..n)
        .map(|i| {
            if i / w + 1 < h {
                (-d2(px(rgb, i), px(rgb, i + w)) / (2.0 * mean)).exp()
            } else {
                0.0
            }
        })
        .collect();
    let weight = |i: usize, j: usize| -> f64 {
        if j == i + 1 {
            wr[i]
        } else if i == j + 1 {
            wr[j]
        } else if j == i + w {
            wd[i]
        } else {
            wd[j]
        }
    };
    let data = |i: usize, l: u16| -> f64 {
        let c = pal.rgb[l as usize];
        (0..3)
            .map(|k| (rgb[i][k] as f64 - c[k] as f64).powi(2))
            .sum::<f64>()
            * inv2s2
    };
    let nbrs = |i: usize| -> ([usize; 4], usize) {
        let (x, y) = (i % w, i / w);
        let mut out = [0usize; 4];
        let mut m = 0;
        for j in [
            if x > 0 { Some(i - 1) } else { None },
            if x + 1 < w { Some(i + 1) } else { None },
            if y > 0 { Some(i - w) } else { None },
            if y + 1 < h { Some(i + w) } else { None },
        ]
        .into_iter()
        .flatten()
        {
            out[m] = j;
            m += 1;
        }
        (out, m)
    };
    for _ in 0..12 {
        let mut moved = 0;
        for parity in 0..2 {
            for i in 0..n {
                if (i % w + i / w) % 2 != parity {
                    continue;
                }
                let (nn, m) = nbrs(i);
                let energy = |l: u16| -> f64 {
                    data(i, l)
                        + price
                            * nn[..m]
                                .iter()
                                .filter(|&&j| labels[j] != l)
                                .map(|&j| weight(i, j))
                                .sum::<f64>()
                };
                let mut best = labels[i];
                let mut cost = energy(best);
                for &j in &nn[..m] {
                    let l = labels[j];
                    let e = energy(l);
                    if e + 1e-9 < cost {
                        cost = e;
                        best = l;
                    }
                }
                if best != labels[i] {
                    labels[i] = best;
                    moved += 1;
                }
            }
        }
        if moved == 0 {
            break;
        }
    }
    // Joint component moves, as in `regularize::labels`, with the shared boundary priced by
    // contrast weight rather than by count.
    let region_cost = 9.0 * price;
    for _ in 0..4 {
        let (comp, ink) = components(labels, w, h);
        let r = ink.len();
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); r];
        for i in 0..n {
            members[comp[i] as usize].push(i);
        }
        let mut merged = 0;
        for g in 0..r {
            let cur = ink[g];
            let mut shared: HashMap<u16, f64> = HashMap::new();
            for &i in &members[g] {
                let (nn, m) = nbrs(i);
                for &j in &nn[..m] {
                    if labels[j] != cur && comp[j] as usize != g {
                        *shared.entry(labels[j]).or_default() += weight(i, j);
                    }
                }
            }
            let mut best = cur;
            let mut gain = 0.0;
            for (&t, &sw) in &shared {
                let delta: f64 = members[g]
                    .iter()
                    .map(|&i| data(i, t) - data(i, cur))
                    .sum::<f64>()
                    - price * sw
                    - region_cost;
                if delta < gain {
                    gain = delta;
                    best = t;
                }
            }
            if best != cur {
                for &i in &members[g] {
                    labels[i] = best;
                }
                merged += 1;
            }
        }
        if merged == 0 {
            break;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 4. Edge-witnessed inks.
// ---------------------------------------------------------------------------------------------

fn idea4_edge_witness(rgb: &[[f32; 3]], labels: &mut [u16], pal: &Palette, ctx: &Ctx) {
    let (w, h, k) = (ctx.w, ctx.h, pal.rgb.len());
    let tau =
        inkvec_core::env::number("INKVEC_WITNESS_TAU").unwrap_or(0.2) / ctx.edge_width.max(1.0);
    for _ in 0..48 {
        let group: Vec<usize> = labels.iter().map(|&l| l as usize).collect();
        let mut accs = vec![Acc::default(); k];
        for (l, a) in accs.iter_mut().enumerate() {
            a.fallback = [
                pal.rgb[l][0] as f64,
                pal.rgb[l][1] as f64,
                pal.rgb[l][2] as f64,
            ];
        }
        accumulate(rgb, labels, &group, &mut accs, w, h);
        let mut int_sum = vec![0f64; k];
        let mut int_cnt = vec![0f64; k];
        let mut con_sum = vec![0f64; k * k];
        let mut con_cnt = vec![0f64; k * k];
        for i in 0..w * h {
            let (x, y) = (i % w, i / w);
            for j in [
                if x + 1 < w { Some(i + 1) } else { None },
                if y + 1 < h { Some(i + w) } else { None },
            ]
            .into_iter()
            .flatten()
            {
                let d = d2(px(rgb, i), px(rgb, j)).sqrt();
                let (a, b) = (labels[i] as usize, labels[j] as usize);
                if a == b {
                    int_sum[a] += d;
                    int_cnt[a] += 1.0;
                } else {
                    con_sum[a * k + b] += d;
                    con_cnt[a * k + b] += 1.0;
                    con_sum[b * k + a] += d;
                    con_cnt[b * k + a] += 1.0;
                }
            }
        }
        let largest = (0..k)
            .max_by(|&a, &b| accs[a].n.total_cmp(&accs[b].n))
            .unwrap_or(0);
        let mut cands = Vec::new();
        for s in 0..k {
            if s == largest || accs[s].n == 0.0 {
                continue;
            }
            let t = (0..k)
                .filter(|&t| t != s && accs[t].n > 0.0 && con_cnt[s * k + t] > 0.0)
                .max_by(|&a, &b| con_cnt[s * k + a].total_cmp(&con_cnt[s * k + b]));
            let Some(t) = t else { continue };
            let offset = d2(accs[s].mean(), accs[t].mean()).sqrt().max(1e-6);
            let across = con_sum[s * k + t] / con_cnt[s * k + t];
            let within = if int_cnt[s] > 0.0 {
                int_sum[s] / int_cnt[s]
            } else {
                0.0
            };
            let witness = (across - within) / offset;
            if witness < tau {
                cands.push((witness, s, t));
            }
        }
        if !batch_merge(labels, k, cands) {
            break;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 5. Inks read off the plateaus either side of strong edges.
// ---------------------------------------------------------------------------------------------

fn idea5_inks_from_edges(rgb: &[[f32; 3]], labels: &mut [u16], pal: &Palette, ctx: &Ctx) {
    let (w, h, k) = (ctx.w, ctx.h, pal.rgb.len());
    let n = w * h;
    let t_edge = (12.0f64 / 255.0).max(8.0 * ctx.sigma);
    let t_flat = t_edge / 3.0;
    let tol = inkvec_core::env::number("INKVEC_EDGE_INK_DE00").unwrap_or(4.0) as f32;
    let mut samples: Vec<[f32; 3]> = Vec::new();
    let step = |i: usize, dx: isize, dy: isize, s: isize| -> Option<usize> {
        let (x, y) = ((i % w) as isize + dx * s, (i / w) as isize + dy * s);
        if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
            None
        } else {
            Some(y as usize * w + x as usize)
        }
    };
    for i in 0..n {
        for (dx, dy) in [(1isize, 0isize), (0, 1)] {
            let Some(j) = step(i, dx, dy, 1) else {
                continue;
            };
            if d2(px(rgb, i), px(rgb, j)).sqrt() <= t_edge {
                continue;
            }
            // plateau on the i side at -2/-3, on the j side at +2/+3 from j
            let (Some(a2), Some(a3), Some(b2), Some(b3)) = (
                step(i, dx, dy, -2),
                step(i, dx, dy, -3),
                step(j, dx, dy, 2),
                step(j, dx, dy, 3),
            ) else {
                continue;
            };
            if d2(px(rgb, a2), px(rgb, a3)).sqrt() < t_flat {
                samples.push(rgb[a2]);
            }
            if d2(px(rgb, b2), px(rgb, b3)).sqrt() < t_flat {
                samples.push(rgb[b2]);
            }
        }
    }
    let stride = (samples.len() / 40_000).max(1);
    let mut count = vec![0usize; k];
    let mut used = 0usize;
    for s in samples.iter().step_by(stride) {
        let (idx, _) = pal.nearest(rgb_to_oklab(*s));
        if de00(*s, pal.rgb[idx]) < tol {
            count[idx] += 1;
        }
        used += 1;
    }
    let need = ((used as f64) * 0.002).max(8.0) as usize;
    let mut area = vec![0usize; k];
    for &l in labels.iter() {
        area[l as usize] += 1;
    }
    let largest = (0..k).max_by_key(|&l| area[l]).unwrap_or(0);
    let witnessed: Vec<bool> = (0..k).map(|l| l == largest || count[l] >= need).collect();
    if witnessed.iter().all(|&x| x) {
        return;
    }
    let keep: Vec<usize> = (0..k).filter(|&l| witnessed[l] && area[l] > 0).collect();
    if keep.is_empty() {
        return;
    }
    for i in 0..n {
        if witnessed[labels[i] as usize] {
            continue;
        }
        let c = rgb_to_oklab(rgb[i]);
        let best = keep
            .iter()
            .copied()
            .min_by(|&a, &b| c.dist(pal.colors[a]).total_cmp(&c.dist(pal.colors[b])))
            .unwrap_or(largest);
        labels[i] = best as u16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 24x24 white canvas with a black 12x12 square: two inks, hard noise-free edges.
    fn two_inks() -> (Vec<[f32; 3]>, Vec<u16>, Palette, Ctx) {
        let (w, h) = (24usize, 24usize);
        let mut rgb = vec![[1.0f32; 3]; w * h];
        let mut labels = vec![0u16; w * h];
        for y in 6..18 {
            for x in 6..18 {
                rgb[y * w + x] = [0.0; 3];
                labels[y * w + x] = 1;
            }
        }
        let inks = [[1.0f32; 3], [0.0f32; 3]];
        let dark = 144.0 / (w * h) as f32;
        let pal = Palette {
            colors: inks.iter().map(|&c| rgb_to_oklab(c)).collect(),
            rgb: inks.to_vec(),
            weight: vec![1.0 - dark, dark],
            alpha: vec![1.0, 1.0],
        };
        let ctx = Ctx {
            w,
            h,
            sigma: 0.002,
            measured_sigma: 0.002,
            edge_width: 1.0,
        };
        (rgb, labels, pal, ctx)
    }

    #[test]
    fn an_unknown_rule_changes_nothing() {
        let (rgb, mut labels, pal, ctx) = two_inks();
        let before = labels.clone();
        assert_eq!(apply("not-a-rule", &rgb, &mut labels, &pal, &ctx), 0);
        assert_eq!(labels, before);
    }

    #[test]
    fn inputs_too_small_to_have_an_interior_are_left_alone() {
        let (rgb, mut labels, pal, mut ctx) = two_inks();
        ctx.w = 2; // `apply` needs at least 3x3
        let before = labels.clone();
        assert_eq!(apply("1", &rgb, &mut labels, &pal, &ctx), 0);
        assert_eq!(labels, before);
    }

    #[test]
    fn every_rule_keeps_both_inks_of_a_clean_two_ink_image() {
        for idea in ["1", "2", "3", "3b", "4", "5"] {
            let (rgb, mut labels, pal, ctx) = two_inks();
            let before = labels.clone();
            let changed = apply(idea, &rgb, &mut labels, &pal, &ctx);
            assert_eq!(
                (changed, &labels),
                (0, &before),
                "rule {idea} relabelled a clean two-ink image"
            );
        }
    }
}
