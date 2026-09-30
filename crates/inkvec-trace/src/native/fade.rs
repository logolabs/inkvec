//! Fades: translucent regions whose opacity varies across them, written as one gradient
//! with `stop-color` and `stop-opacity`.
//!
//! The **fades** stage of the native-alpha path ([`super::trace_color`]), after the
//! gradient-band merge and the carve and before faces are split. It has no counterpart on
//! the opaque path, which cannot see opacity. In: the label map, the per-label fills and
//! inks from the band merge, the image over white (sRGB `[0, 1]`) and its alpha. Out: the
//! label map with each accepted fade relabelled as one new label, that label's fill
//! rewritten as the fade seen over white, and a [`Fade`] per label for the emitter.
//!
//! Colour conventions: `s` is a straight (un-premultiplied) colour, `a` an opacity,
//! `P = s·a = W − (1 − a)` the premultiplied colour recovered from the pixel over white
//! `W`. An opacity profile is stored as an ordinary grey [`gradient::FillModel`] whose
//! every channel holds the opacity.

use crate::color::Palette;
use crate::gradient;

/// A face whose opacity varies across it: a glow, a soft shadow, a vignette, a flame's
/// halo. Written as one gradient carrying `stop-color` and `stop-opacity` at each stop.
#[derive(Debug, Clone, PartialEq)]
pub struct Fade {
    /// The colour profile, straight (not composited over anything), on the geometry and
    /// at the stop offsets of `alpha`. Most fades in the corpus change colour as they fade
    /// (175 of the 308 files that use `stop-opacity`), so this is a profile, not a colour.
    pub color: gradient::FillModel,
    /// The opacity profile, as a fill model whose every stop is a grey equal to the
    /// opacity there: the geometry and the stops of an ordinary gradient.
    pub alpha: gradient::FillModel,
}

/// A model's stops in order, `(offset, colour)`: one stop for a flat fill.
pub(crate) fn model_stops(m: &gradient::FillModel) -> Vec<(f64, [f32; 3])> {
    match m {
        gradient::FillModel::Flat(c) => vec![(0.0, *c)],
        gradient::FillModel::Linear { c0, c1, mids, .. }
        | gradient::FillModel::Radial { c0, c1, mids, .. } => {
            let mut v = vec![(0.0, *c0)];
            v.extend(mids.iter().copied());
            v.push((1.0, *c1));
            v
        }
    }
}

/// `m` with its stop colours replaced, in order; offsets and geometry kept.
///
/// `cols` must hold one colour per stop of `m` ([`model_stops`]); a shorter slice panics.
fn restop(m: &gradient::FillModel, cols: &[[f32; 3]]) -> gradient::FillModel {
    match m {
        gradient::FillModel::Flat(_) => gradient::FillModel::Flat(cols[0]),
        gradient::FillModel::Linear { mids, .. } | gradient::FillModel::Radial { mids, .. } => {
            let k = mids.len();
            m.with_stops(
                cols[0],
                mids.iter()
                    .zip(&cols[1..=k])
                    .map(|(&(o, _), &c)| (o, c))
                    .collect(),
                cols[k + 1],
            )
        }
    }
}

impl Fade {
    /// The lowest opacity the profile reaches: at a fade's rim, where it meets the ground.
    ///
    /// The minimum over the opacity model's stops (channel 0 of each grey stop), capped at
    /// 1. Between stops the profile is linear, so the minimum is always at a stop.
    pub fn rim_alpha(&self) -> f32 {
        let stops = |m: &gradient::FillModel| -> Vec<f32> {
            match m {
                gradient::FillModel::Flat(c) => vec![c[0]],
                gradient::FillModel::Linear { c0, c1, mids, .. }
                | gradient::FillModel::Radial { c0, c1, mids, .. } => {
                    let mut v = vec![c0[0], c1[0]];
                    v.extend(mids.iter().map(|m| m.1[0]));
                    v
                }
            }
        };
        stops(&self.alpha).into_iter().fold(1.0f32, f32::min)
    }

    /// The same fade as a fill over white, which is how every other stage sees a face:
    /// `W = s·a + (1 - a)`, linear in the gradient coordinate exactly as `a` is.
    ///
    /// Computed per stop on the opacity model's geometry and offsets (the colour model has
    /// the same stops). It is exact at the stops; between two stops whose colours differ,
    /// the true `s(t)·a(t) + 1 − a(t)` is quadratic in `t` and the linear stop-to-stop
    /// ramp is its chord.
    pub fn over_white(&self) -> gradient::FillModel {
        let a_stops = model_stops(&self.alpha);
        let c_stops = model_stops(&self.color);
        let cols: Vec<[f32; 3]> = a_stops
            .iter()
            .enumerate()
            .map(|(i, &(_, ag))| {
                let a = ag[0].clamp(0.0, 1.0);
                let s = c_stops.get(i).or(c_stops.last()).map_or([1.0; 3], |c| c.1);
                [s[0] * a + 1.0 - a, s[1] * a + 1.0 - a, s[2] * a + 1.0 - a]
            })
            .collect();
        restop(&self.alpha, &cols)
    }
}

/// Solve the small symmetric system `a·x = b` by Gaussian elimination with partial
/// pivoting; `None` when it is singular.
///
/// `a` is `n × n`, `b` holds three right-hand sides (one per colour channel) per row, and
/// so does the result. "Singular" means a pivot below `1e-12` in absolute value. `O(n³)`,
/// meant for the handful of stops in a fade.
pub(super) fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<[f64; 3]>) -> Option<Vec<[f64; 3]>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..n {
            let f = a[row][col] / a[col][col];
            for k in col..n {
                a[row][k] -= f * a[col][k];
            }
            for c in 0..3 {
                b[row][c] -= f * b[col][c];
            }
        }
    }
    let mut x = vec![[0.0f64; 3]; n];
    for row in (0..n).rev() {
        for c in 0..3 {
            let mut s = b[row][c];
            for k in row + 1..n {
                s -= a[row][k] * x[k][c];
            }
            x[row][c] = s / a[row][row];
        }
    }
    Some(x)
}

/// The colour profile of a fade, on the geometry and stops of its opacity profile.
///
/// With the geometry fixed every pixel has its gradient coordinate `t`, and the colour is
/// piecewise linear in `t` between the stops, so the stop colours are a small linear least
/// squares. Weighted by `a²`, which is the residual in premultiplied colour -- what the
/// pixel actually shows -- so a pixel too faint to see cannot steer the colour. A stop no
/// pixel testifies about (a halo's inner stop under an opaque flame) is held to the fade's
/// mean colour by a light ridge.
///
/// # The formula
///
/// With `S_0 … S_{m−1}` the unknown stop colours at the opacity model's offsets `o_j`, a
/// pixel at gradient coordinate `t` in span `[o_j, o_{j+1}]`, `u = (t − o_j)/(o_{j+1} − o_j)`,
/// is modelled as `s(t) = (1 − u) S_j + u S_{j+1}`, and the fit minimises
///
/// `E(S) = Σ_p (a_p s(t_p) − P_p)² + ρ Σ_j |S_j − s̄|²`
///
/// over pixels with `a_p ≥ 1e-3`, `P_p = W_p − (1 − a_p)` per channel. The normal
/// equations are tridiagonal (`m × m`, three right-hand sides) and solved with [`solve`].
/// `s̄ = Σ P_p / Σ a_p` is the fade's mean straight colour (white when no pixel counts);
/// the ridge is `ρ = 1e-6 + 1e-3 · mean(diag)`. A singular system falls back to `s̄` at
/// every stop. Stop colours are clamped to `[0, 1]` and returned on the opacity model's
/// geometry and offsets.
pub(super) fn fit_colour_stops(
    alpha_model: &gradient::FillModel,
    px: &[usize],
    rgb: &[[f32; 3]],
    alpha: &[f32],
    w: usize,
) -> gradient::FillModel {
    let offs: Vec<f64> = model_stops(alpha_model).iter().map(|s| s.0).collect();
    let m = offs.len();
    let mut a = vec![vec![0.0f64; m]; m];
    let mut b = vec![[0.0f64; 3]; m];
    let (mut mean, mut msum) = ([0.0f64; 3], 0.0f64);
    for &p in px {
        let ap = alpha[p] as f64;
        if ap < 1e-3 {
            continue;
        }
        let pm = [
            (rgb[p][0] as f64 - (1.0 - ap)),
            (rgb[p][1] as f64 - (1.0 - ap)),
            (rgb[p][2] as f64 - (1.0 - ap)),
        ];
        for k in 0..3 {
            mean[k] += pm[k];
        }
        msum += ap;
        // s(t) = (1-u)·S_j + u·S_{j+1}; residual a·s(t) - pm, so the design row is a·basis.
        let t = alpha_model.t_at((p % w) as f64, (p / w) as f64);
        let j = (0..m - 1).rfind(|&j| t >= offs[j]).unwrap_or(0);
        let span = (offs[j + 1] - offs[j]).max(1e-9);
        let u = ((t - offs[j]) / span).clamp(0.0, 1.0);
        let (r0, r1) = (ap * (1.0 - u), ap * u);
        a[j][j] += r0 * r0;
        a[j][j + 1] += r0 * r1;
        a[j + 1][j] += r0 * r1;
        a[j + 1][j + 1] += r1 * r1;
        for k in 0..3 {
            b[j][k] += r0 * pm[k];
            b[j + 1][k] += r1 * pm[k];
        }
    }
    let mean = if msum > 1e-9 {
        [mean[0] / msum, mean[1] / msum, mean[2] / msum]
    } else {
        [1.0; 3]
    };
    let ridge = 1e-6 + 1e-3 * (0..m).map(|i| a[i][i]).sum::<f64>() / m as f64;
    for i in 0..m {
        a[i][i] += ridge;
        for k in 0..3 {
            b[i][k] += ridge * mean[k];
        }
    }
    let x = solve(a, b).unwrap_or_else(|| vec![mean; m]);
    let cols: Vec<[f32; 3]> = x
        .iter()
        .map(|c| {
            [
                c[0].clamp(0.0, 1.0) as f32,
                c[1].clamp(0.0, 1.0) as f32,
                c[2].clamp(0.0, 1.0) as f32,
            ]
        })
        .collect();
    restop(alpha_model, &cols)
}

/// Chi-square of a model of a region's pixels -- opacity and premultiplied colour, each
/// beyond the half-level quantisation dead zone -- where `model(p)` is `(colour, alpha)`.
///
/// `χ² = Σ_p [ ρ(a_p − â_p) + Σ_c ρ(P_{p,c} − ŝ_c(p) â_p) ]`, with `(ŝ, â) = model(p)`,
/// `P = W − (1 − a)` the observed premultiplied colour and
/// `ρ(e) = (max(|e| − 0.5/255, 0) / σ)²`. The dead zone means a model within half an 8-bit
/// level of a pixel pays nothing for it, since the pixel cannot say more than that.
/// `σ` is the per-channel noise in `[0, 1]` units.
pub(super) fn fade_chi2(
    px: &[usize],
    rgb: &[[f32; 3]],
    alpha: &[f32],
    sigma: f64,
    model: impl Fn(usize) -> ([f32; 3], f32),
) -> f64 {
    const DEAD: f64 = 0.5 / 255.0;
    let r = |e: f64| {
        let e = (e.abs() - DEAD).max(0.0) / sigma;
        e * e
    };
    px.iter()
        .map(|&p| {
            let (s, am) = model(p);
            let (ap, am) = (alpha[p] as f64, am as f64);
            let mut c2 = r(ap - am);
            for k in 0..3 {
                let pm = rgb[p][k] as f64 - (1.0 - ap);
                c2 += r(pm - s[k] as f64 * am);
            }
            c2
        })
        .sum()
}

/// Editable numbers in an opacity profile: the geometry, and one number per stop where a
/// colour stop has three.
///
/// Flat: 1. Linear: 4 (two end points) + 2 (end opacities) + 2 per interior stop (offset
/// and opacity). Radial: 3 for a circle or 5 for an ellipse (`aspect != 1`), plus the same
/// stop count.
pub(super) fn alpha_params(m: &gradient::FillModel) -> f64 {
    match m {
        gradient::FillModel::Flat(_) => 1.0,
        gradient::FillModel::Linear { mids, .. } => 4.0 + 2.0 + 2.0 * mids.len() as f64,
        gradient::FillModel::Radial { aspect, mids, .. } => {
            let geom = if *aspect == 1.0 { 3.0 } else { 5.0 };
            geom + 2.0 + 2.0 * mids.len() as f64
        }
    }
}

/// An opacity profile fitted by the ordinary fill fitter, run on the alpha as a grey image.
/// Only sRGB-space candidates: `stop-opacity` interpolates linearly in opacity, and a
/// linear-light fit of a grey would be a different curve. The grey repeats the alpha in
/// three channels, so its chi-square counts the evidence three times; the cost here counts
/// it once, and prices each stop at one number.
///
/// Returns the cheapest `(model, cost)` with `cost = ½ χ²/3 + λ · alpha_params(model)`
/// over the fitter's candidates for `pixels` (those passing `member`), restricted to flat
/// fills when `flat_only`. `grey` is the alpha replicated into three channels, `σ` its
/// noise and `λ` nats per parameter. With no candidate at all it returns a flat opacity of
/// 1 at infinite cost.
pub(super) fn fit_opacity(
    grey: &[[f32; 3]],
    w: usize,
    h: usize,
    pixels: &[usize],
    member: impl Fn(usize) -> bool + Sync,
    sigma: f64,
    lambda: f64,
    flat_only: bool,
) -> (gradient::FillModel, f64) {
    gradient::fit_pixels(grey, w, h, pixels, member, |_| true, sigma, lambda)
        .into_iter()
        .filter(|f| match &f.model {
            gradient::FillModel::Flat(_) => true,
            gradient::FillModel::Linear { interp, .. }
            | gradient::FillModel::Radial { interp, .. } => {
                !flat_only && *interp == gradient::Interp::Srgb
            }
        })
        .map(|f| {
            let cost = 0.5 * f.chi2 / 3.0 + lambda * alpha_params(&f.model);
            (f.model, cost)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((gradient::FillModel::Flat([1.0; 3]), f64::INFINITY))
}

/// Turn each connected run of translucent regions into one fade, where one opacity profile
/// and one colour profile on it cost less than the separate washes.
///
/// The palette finds a fade as bands, one ink per opacity level, and the colour merge
/// cannot join them: over white a white glow is white everywhere, and the difference is all
/// in the alpha. Here translucent regions are gathered into connected clusters; the ordinary
/// fill fitter is asked for the geometry from the alpha alone -- linear, radial or
/// elliptical, with interior stops -- then the colour stops are fitted on that geometry,
/// and the whole is priced against the separate washes by the same description length as
/// every other merge, opacity and premultiplied colour residuals both. A single region
/// whose opacity ramps is a fade on its own.
///
/// Returns, per label id (new ids included), the fade that label is, if any.
///
/// # The steps
///
/// 1. For every label, its straight colour `s = Σ(W − (1 − a)) / Σ a` over its pixels.
/// 2. *Washes*: labels whose ink has an opacity in `[0.05, 0.98)` and a defined colour.
/// 3. *Clusters*: 4-connected components of wash pixels, regardless of colour.
/// 4. For each cluster of at least 16 pixels (and while `u16` ids remain): fit the opacity
///    geometry on the alpha alone ([`fit_opacity`]); a flat fit ends it. Otherwise fit the
///    colour stops on that geometry ([`fit_colour_stops`]) and price
///    `union = ½ χ²_fade + λ (alpha_params + 3 · stops)` against
///    `separate = Σ_bands (½ χ²_band + 4 λ)`, each band one colour (its step-1 estimate) at
///    one opacity (its median alpha). A cheaper union becomes a fresh label covering the
///    whole cluster, with fill [`Fade::over_white`], the first band's ink, and its [`Fade`].
/// 5. Washes that stayed washes get their fill rewritten as `s·a + (1 − a)` from step 1,
///    so that the emitter's un-matting by `a` recovers `s` exactly.
///
/// `fills_by_label` and `label_ink` are padded as new ids are created, with flat palette
/// colours (white past the palette) and identity inks. `INKVEC_FADEDBG` prints each
/// cluster's verdict.
#[allow(clippy::too_many_arguments)]
pub(super) fn merge_fades(
    labels: &mut [u16],
    fills_by_label: &mut Vec<gradient::FillFit>,
    label_ink: &mut Vec<usize>,
    rgb: &[[f32; 3]],
    alpha: &[f32],
    w: usize,
    h: usize,
    pal: &Palette,
    sigma: f64,
    lambda: f64,
) -> Vec<Option<Fade>> {
    /// An opacity at or above this is paint, not a fade.
    const OPAQUE_BAND: f32 = 0.98;
    let n = w * h;
    let n_labels = labels
        .iter()
        .map(|&l| l as usize + 1)
        .max()
        .unwrap_or(0)
        .max(fills_by_label.len())
        .max(label_ink.len())
        .max(pal.len());
    let ink_of = |l: usize, label_ink: &[usize]| label_ink.get(l).copied().unwrap_or(l);

    // Each translucent label's colour, from its own pixels: every pixel of a colour `s` at
    // opacity `a` is `W - (1 - a) = s·a` exactly, so `s = Σ(W - (1 - a)) / Σa` whatever the
    // opacities are. Un-matting the band's one colour by its one opacity instead divides a
    // colour error by `a`: a black shadow's faint bands came out #353535 and #2a2a2a and were
    // never recognised as one colour.
    let mut pm = vec![([0.0f64; 3], 0.0f64); n_labels];
    for p in 0..n {
        let (c, a) = (rgb[p], alpha[p]);
        let e = &mut pm[labels[p] as usize];
        for k in 0..3 {
            e.0[k] += (c[k] - (1.0 - a)) as f64;
        }
        e.1 += a as f64;
    }
    let colour_of = |acc: &([f64; 3], f64)| -> Option<[f32; 3]> {
        (acc.1 > 1e-6).then(|| {
            [
                (acc.0[0] / acc.1).clamp(0.0, 1.0) as f32,
                (acc.0[1] / acc.1).clamp(0.0, 1.0) as f32,
                (acc.0[2] / acc.1).clamp(0.0, 1.0) as f32,
            ]
        })
    };

    // The washes: every translucent label, whatever fill the colour merge gave it. A band
    // of a fade carries a stretch of the ramp inside it, so the merge over white often fits
    // it a colour gradient -- five of the candle halo's did -- and a colour gradient over
    // white is not something a translucent face can be written as: its stops already hold
    // the white, and `fill-opacity` would apply it twice. The fit below starts from the
    // pixels, opacity and colour both, so the fill it replaces does not matter.
    let wash: Vec<bool> = (0..n_labels)
        .map(|l| {
            let a = pal.alpha.get(ink_of(l, label_ink)).copied().unwrap_or(1.0);
            (0.05..OPAQUE_BAND).contains(&a) && colour_of(&pm[l]).is_some()
        })
        .collect();
    if !wash.iter().any(|&b| b) {
        return vec![None; n_labels];
    }

    // Connected clusters of wash pixels. Colour does not split them: two washes of
    // different colours join only if one colour profile explains both, and the cost below
    // says whether it does.
    let mut cluster = vec![u32::MAX; n];
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for start in 0..n {
        if !wash[labels[start] as usize] || cluster[start] != u32::MAX {
            continue;
        }
        let id = clusters.len() as u32;
        let mut stack = vec![start];
        let mut px = Vec::new();
        cluster[start] = id;
        while let Some(p) = stack.pop() {
            px.push(p);
            let (x, y) = (p % w, p / w);
            for q in [
                (x > 0).then(|| p - 1),
                (x + 1 < w).then(|| p + 1),
                (y > 0).then(|| p - w),
                (y + 1 < h).then(|| p + w),
            ]
            .into_iter()
            .flatten()
            {
                if cluster[q] == u32::MAX && wash[labels[q] as usize] {
                    cluster[q] = id;
                    stack.push(q);
                }
            }
        }
        px.sort_unstable();
        clusters.push(px);
    }

    let grey: Vec<[f32; 3]> = alpha.iter().map(|&a| [a, a, a]).collect();
    let mut fade_of: Vec<Option<Fade>> = vec![None; n_labels];
    let mut next = n_labels;
    let dbg = inkvec_core::env::flag("INKVEC_FADEDBG");
    if dbg {
        let translucent_gradient = (0..n_labels)
            .filter(|&l| {
                let a = pal.alpha.get(ink_of(l, label_ink)).copied().unwrap_or(1.0);
                (0.05..OPAQUE_BAND).contains(&a)
                    && fills_by_label.get(l).is_some_and(|f| f.model.is_gradient())
            })
            .count();
        eprintln!(
            "  fades: {} washes, {} translucent labels with a colour gradient, {} clusters",
            wash.iter().filter(|&&b| b).count(),
            translucent_gradient,
            clusters.len()
        );
    }
    for (cid, px) in clusters.iter().enumerate() {
        let mut bands: Vec<u16> = px.iter().map(|&p| labels[p]).collect();
        bands.sort_unstable();
        bands.dedup();
        // One band is enough: a single region whose opacity ramps is a fade on its own.
        if px.len() < 16 || next >= u16::MAX as usize {
            if dbg {
                eprintln!(
                    "  fade cluster {cid}: {} band(s), {} px -- too small",
                    bands.len(),
                    px.len()
                );
            }
            continue;
        }
        let cid = cid as u32;
        // The geometry comes from the opacity, which is what a fade is.
        let (alpha_model, _) =
            fit_opacity(&grey, w, h, px, |p| cluster[p] == cid, sigma, lambda, false);
        if !alpha_model.is_gradient() {
            if dbg {
                eprintln!(
                    "  fade cluster {cid}: {} bands, {} px -- opacity fits flat",
                    bands.len(),
                    px.len()
                );
            }
            continue;
        }
        let color_model = fit_colour_stops(&alpha_model, px, rgb, alpha, w);
        let n_stops = model_stops(&alpha_model).len() as f64;
        let union_chi2 = fade_chi2(px, rgb, alpha, sigma, |p| {
            let (x, y) = ((p % w) as f64, (p / w) as f64);
            (color_model.color_at(x, y), alpha_model.color_at(x, y)[0])
        });
        let union = 0.5 * union_chi2 + lambda * (alpha_params(&alpha_model) + 3.0 * n_stops);
        // The separate washes: each band one opacity and one colour of its own.
        let mut separate = 0.0;
        for &b in &bands {
            let bp: Vec<usize> = px.iter().copied().filter(|&p| labels[p] == b).collect();
            let Some(s) = colour_of(&pm[b as usize]) else {
                continue;
            };
            let mut av: Vec<f32> = bp.iter().map(|&p| alpha[p]).collect();
            let mid = av.len() / 2;
            let a_b = *av.select_nth_unstable_by(mid, |x, y| x.total_cmp(y)).1;
            separate += 0.5 * fade_chi2(&bp, rgb, alpha, sigma, |_| (s, a_b)) + lambda * 4.0;
        }
        if dbg {
            eprintln!(
                "  fade cluster {cid}: {} bands, {} px, {} union {union:.1} vs separate {separate:.1}",
                bands.len(),
                px.len(),
                alpha_model.kind()
            );
        }
        if union >= separate {
            continue;
        }
        let fade = Fade {
            color: color_model,
            alpha: alpha_model,
        };
        let id = next;
        next += 1;
        let first_ink = ink_of(bands[0] as usize, label_ink);
        for &p in px {
            labels[p] = id as u16;
        }
        // Any label without a fill of its own falls back to its palette colour downstream;
        // padding must say the same, not invent one.
        while fills_by_label.len() <= id {
            let l = fills_by_label.len();
            fills_by_label.push(gradient::FillFit {
                model: gradient::FillModel::Flat(pal.rgb.get(l).copied().unwrap_or([1.0; 3])),
                chi2: 0.0,
                params: gradient::PARAMS_FLAT,
                cost: 0.0,
            });
        }
        fills_by_label[id] = gradient::FillFit {
            model: fade.over_white(),
            chi2: union_chi2,
            params: alpha_params(&fade.alpha) + 3.0 * n_stops,
            cost: union,
        };
        if label_ink.len() <= id {
            let len = label_ink.len();
            label_ink.extend(len..=id);
        }
        label_ink[id] = first_ink;
        if fade_of.len() <= id {
            fade_of.resize(id + 1, None);
        }
        fade_of[id] = Some(fade);
    }

    // The washes that stay washes get the same colour estimate. The emitter recovers a
    // wash's colour by un-matting its fill at its opacity, so the fill is written as that
    // colour over white at that opacity, and the division by `a` recovers it exactly.
    let still_present: std::collections::HashSet<u16> = labels.iter().copied().collect();
    for l in 0..n_labels {
        if !wash[l] || !still_present.contains(&(l as u16)) {
            continue;
        }
        let (Some(s), Some(&a)) = (colour_of(&pm[l]), pal.alpha.get(ink_of(l, label_ink))) else {
            continue;
        };
        while fills_by_label.len() <= l {
            let k = fills_by_label.len();
            fills_by_label.push(gradient::FillFit {
                model: gradient::FillModel::Flat(pal.rgb.get(k).copied().unwrap_or([1.0; 3])),
                chi2: 0.0,
                params: gradient::PARAMS_FLAT,
                cost: 0.0,
            });
        }
        fills_by_label[l].model =
            gradient::FillModel::Flat([s[0] * a + 1.0 - a, s[1] * a + 1.0 - a, s[2] * a + 1.0 - a]);
    }
    fade_of
}
