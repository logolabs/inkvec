//! Transparency, on the way in and on the way out.
//!
//! The tracer proper is opaque: palette, labels, unmix and fills are all RGB and never see
//! an alpha channel. So an RGBA input is **matted** here — composited onto one chosen
//! opaque colour, with the original alphas kept aside — and the transparency is put back
//! at the end as holes, `fill-opacity` and alpha ramps.
//!
//! Which matte is chosen matters and is not free: it must differ from the ink it meets or
//! the silhouette dissolves, and must not differ from whatever a soft edge will be
//! composited against or that edge bakes wrong. [`choose_matte`] resolves that, and
//! `docs/ALPHA.md` records what the compromise costs and what would remove it.

use inkvec_core::Point;
use inkvec_trace::{gradient, planar};

use crate::Args;

/// Resample an oversampled intake down to one pixel per unit of real detail.
///
/// This is the whole answer to "make the thresholds work at any resolution", and
/// it is one change rather than a scale factor threaded through every constant.
/// The thresholds are in pixels and were tuned where one pixel was one unit of
/// detail; rather than restate each of them in some other unit, put the input back
/// into the units they were written in.
///
/// It has to be the *point spread* that decides, not the image size. A native
/// render at 1024 resolves genuine detail — it reads a scale of exactly 1.00 and
/// is not touched, and it already traces in 1.7 s. An upsample, a blur or a
/// photograph of a screen carries fewer units of detail than it has pixels, and
/// those extra pixels are not information: they are what shatters the palette into
/// a thousand regions and what the tracer then spends a minute describing.
///
/// Nothing is lost in the output. The SVG keeps its `width` and `height` in the
/// original units and only its `viewBox` shrinks, so it renders at exactly the
/// size it always did — and being a vector, at any other size too.
/// Uncertainty of a face's mean colour, in sRGB units, for the layer hypothesis.
///
/// The module's default is 1.5/255, the noise of its own tests. Our face colours are
/// medians over evidence pixels of a matted image and carry more than that: on a synthetic
/// stack of three translucent discs, 1.5 finds nothing and 3 finds the layer with a
/// residual of 0.0007. The false-alarm rate grows with the square of this, so it is the
/// smallest value that finds a layer we know is there.
const LAYER_SIGMA_SRGB: f64 = 3.0 / 255.0;

/// A face whose opacity fades across it: the axis, and the opacity at each end.
///
/// One `fill-opacity` describes a wash. It cannot describe a glow, a feathered edge or a
/// fade-out, and the tracer's answer until now was to bake those against the matte — which
/// is right on the matte's own colour and wrong on every other ground. A linear gradient
/// whose two stops share a colour and differ in `stop-opacity` describes them exactly, in
/// the same element an SVG editor would use.
///
/// The colour is one value: a fade is the same ink at varying coverage, so recovering it
/// once from the whole face — un-matted per pixel, weighted towards the opaque end where
/// the division by alpha is well conditioned — is both simpler and better posed than
/// letting it vary.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AlphaRamp {
    pub(crate) p0: Point,
    pub(crate) p1: Point,
    pub(crate) a0: f32,
    pub(crate) a1: f32,
    pub(crate) color: [f32; 3],
}

/// Fit [`AlphaRamp`] to one face's interior pixels.
///
/// Returns `None` for a face that is flat (that is a `fill-opacity`, handled elsewhere),
/// for one with too few interior pixels to fit three coefficients against, and for one
/// whose alpha is not linear enough to be called a fade — a face that is opaque in two
/// places and clear between them is not a ramp, and inventing one would be worse than
/// baking it.
pub(crate) fn fit_alpha_ramp(
    face: usize,
    labels: &[u16],
    alpha: &[f32],
    rgb: &[[f32; 3]],
    matte: [f32; 3],
    w: usize,
    h: usize,
) -> Option<AlphaRamp> {
    /// A fade has to actually fade: this much opacity across the face, or it is a wash.
    const MIN_FADE: f32 = 0.15;
    /// And it has to fade *linearly*: residual of the plane fit, in opacity.
    const MAX_RESIDUAL: f32 = 0.06;
    const MIN_INTERIOR: usize = 64;

    let (mut n, mut sx, mut sy, mut sxx, mut sxy, mut syy) = (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut sa, mut sax, mut say) = (0.0f64, 0.0, 0.0);
    let mut interior: Vec<(f64, f64, f32)> = Vec::new();
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let i = y * w + x;
            if labels[i] as usize != face {
                continue;
            }
            let same = labels[i - 1] as usize == face
                && labels[i + 1] as usize == face
                && labels[i - w] as usize == face
                && labels[i + w] as usize == face;
            if !same {
                continue;
            }
            let (fx, fy) = (x as f64, y as f64);
            let a = alpha[i] as f64;
            n += 1.0;
            sx += fx;
            sy += fy;
            sxx += fx * fx;
            sxy += fx * fy;
            syy += fy * fy;
            sa += a;
            sax += a * fx;
            say += a * fy;
            interior.push((fx, fy, alpha[i]));
        }
    }
    if n < MIN_INTERIOR as f64 {
        return None;
    }

    // Least squares plane a = b0 + b1 x + b2 y, by the normal equations.
    let m = [[n, sx, sy], [sx, sxx, sxy], [sy, sxy, syy]];
    let r = [sa, sax, say];
    let b = solve3x3(m, r)?;
    let (b0, b1, b2) = (b[0], b[1], b[2]);
    let grad = (b1 * b1 + b2 * b2).sqrt();
    if grad < 1e-9 {
        return None;
    }

    // The extremes of the face along the gradient direction.
    let (ux, uy) = (b1 / grad, b2 / grad);
    let (mut tmin, mut tmax) = (f64::MAX, f64::MIN);
    let mut resid = 0.0f64;
    for &(x, y, a) in &interior {
        let t = x * ux + y * uy;
        tmin = tmin.min(t);
        tmax = tmax.max(t);
        let model = b0 + b1 * x + b2 * y;
        resid += (a as f64 - model) * (a as f64 - model);
    }
    let rms = (resid / n).sqrt() as f32;
    if rms > MAX_RESIDUAL {
        return None;
    }
    let (a0, a1) = (
        (b0 + grad * tmin).clamp(0.0, 1.0) as f32,
        (b0 + grad * tmax).clamp(0.0, 1.0) as f32,
    );
    if (a1 - a0).abs() < MIN_FADE {
        return None;
    }

    // One colour for the whole face, un-matted per pixel and weighted towards the opaque
    // end: `c_obs = a C + (1-a) M`, so `C = (c_obs - (1-a) M) / a`, whose noise blows up as
    // `a` falls. Weighting by `a²` is the inverse-variance weight for exactly that.
    let (mut cw, mut acc) = (0.0f64, [0.0f64; 3]);
    for &(x, y, a) in &interior {
        if a < 0.25 {
            continue;
        }
        let i = y as usize * w + x as usize;
        let wgt = (a * a) as f64;
        for k in 0..3 {
            let c = (rgb[i][k] - (1.0 - a) * matte[k]) / a;
            acc[k] += wgt * c as f64;
        }
        cw += wgt;
    }
    if cw <= 0.0 {
        return None;
    }
    let color = [
        (acc[0] / cw).clamp(0.0, 1.0) as f32,
        (acc[1] / cw).clamp(0.0, 1.0) as f32,
        (acc[2] / cw).clamp(0.0, 1.0) as f32,
    ];
    Some(AlphaRamp {
        p0: Point::new(ux * tmin, uy * tmin),
        p1: Point::new(ux * tmax, uy * tmax),
        a0,
        a1,
        color,
    })
}

/// 3x3 solve by Cramer's rule; `None` when the system is singular.
fn solve3x3(m: [[f64; 3]; 3], r: [f64; 3]) -> Option<[f64; 3]> {
    let det = |a: [[f64; 3]; 3]| {
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    };
    let d = det(m);
    if d.abs() < 1e-12 {
        return None;
    }
    let mut out = [0.0; 3];
    for k in 0..3 {
        let mut mk = m;
        for row in 0..3 {
            mk[row][k] = r[row];
        }
        out[k] = det(mk) / d;
    }
    Some(out)
}

/// The factor a nearest-neighbour upscale multiplied this image by, if it is one.
///
/// Someone who has a 96-px logo and wants a big SVG resizes the PNG first. Every viewer
/// does that with nearest neighbour on request, and plenty do it without being asked, so
/// what arrives is a grid of k x k constant blocks. The tracer then describes exactly what
/// it is given: on a 96-px logo blown up to 768 the palette shatters from 3 inks to 13, and
/// the boundary comes back as 1568 straight lines walking round pixel corners — the
/// staircase is not an artefact of the fit, it is in the file.
///
/// Replication is exactly invertible: average each block and the original pixels come back
/// bit for bit. So the test is strict — every block constant, no tolerance for "nearly" —
/// because a 1-px tolerance would also catch a genuine drawing of large flat squares, and
/// averaging that away would be a real loss. `k` is the largest factor that passes, tried
/// downwards so an 8x upscale is undone as 8x and not as 2x.
///
/// Anti-aliased and resampled upscales are a different problem and not this one: their
/// blocks are not constant, they fail here, and `--sr` is what addresses them.
pub(crate) fn pixel_grid(img: &inkvec_trace::Rgba) -> Option<usize> {
    const MAX_FACTOR: usize = 32;
    let (w, h) = (img.width, img.height);
    let px = |x: usize, y: usize| -> &[f32] { &img.data[(y * w + x) * 4..(y * w + x) * 4 + 4] };
    // Below this there is nothing to gain and something to lose: a 2x undo of a small icon
    // leaves too few pixels for the boundary solve to work with.
    let smallest = 64;
    let mut k = MAX_FACTOR.min(w / smallest.min(w)).min(h / smallest.min(h));
    while k >= 2 {
        if w % k == 0 && h % k == 0 {
            let constant = (0..h / k).all(|by| {
                (0..w / k).all(|bx| {
                    let first = px(bx * k, by * k);
                    (0..k).all(|dy| {
                        (0..k).all(|dx| {
                            let p = px(bx * k + dx, by * k + dy);
                            (0..4).all(|c| (p[c] - first[c]).abs() < 1.0 / 512.0)
                        })
                    })
                })
            });
            if constant {
                return Some(k);
            }
        }
        k -= 1;
    }
    None
}

/// What the transparent parts of the input are put against before tracing.
///
/// Everything downstream works on an opaque image: unmixing an anti-aliased pixel needs
/// two opaque colours, and the palette has no fourth dimension. White was the fixed
/// choice, and white destroys the input people bring most often — a white mark on a
/// transparent ground composites to one flat white and traces to nothing at all, and a
/// pale translucent panel merges into the background it is drawn over.
///
/// So the matte is judged by what it would *swallow*: of the content that meets
/// transparency — the silhouette, and any half-solid pixel along it — how much would come
/// out indistinguishable from the matte itself. White is tried first and kept unless it
/// swallows a third of that, which is the difference between a white sock on an emoji and
/// a white logo.
///
/// Two cheaper rules were tried and measured on the 246-icon screen set first: the
/// furthest candidate from every colour in the image cost 0.4123 -> 0.4735, because a
/// white highlight buried inside an emoji swung it onto a saturated matte; the same test
/// on silhouette colours alone still swung emoji for a sock. Both are why the bar is a
/// share of the drawn mass rather than a distance to any one ink.
///
/// The corpus cannot referee this. It is scored over white, where content lost against a
/// white matte is invisible; the loss is real only on the page the SVG is used on.
///
/// White is first in the ladder for a reason beyond the corpus: the blend line between an
/// ink and its matte is what the palette has to spend inks on, and a saturated matte
/// lengthens that line at every edge in the image.
///
/// The matte never reaches the output — a face whose source pixels are transparent is
/// dropped (and punched out as a hole under `--cutout`), and a face the source drew
/// translucent is un-composited against this same colour before it is written.
/// Share of the artwork a candidate matte may hide before it is rejected.
///
/// Module-level rather than local to `choose_matte`, because `alpha_source` warns the user
/// at the same threshold and restated it as a literal `0.33` until 2026-09-08 -- two
/// numbers that had to agree, with nothing keeping them in agreement.
pub(crate) const SWALLOWED: f64 = 0.33;

pub(crate) fn choose_matte(img: &inkvec_trace::Rgba) -> ([f32; 3], f64) {
    // In order of preference. The two neutrals first, then colours artwork rarely uses.
    const CANDIDATES: [[f32; 3]; 6] = [
        [1.0, 1.0, 1.0],
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 1.0],
        [0.0, 1.0, 0.0],
        [0.0, 1.0, 1.0],
        [1.0, 0.5, 0.0],
    ];
    // How far a drawn pixel has to land from the matte to survive it, and how much of the
    // drawn mass may be swallowed before the candidate is rejected. DRAWN is high on
    // purpose: faint content is baked against the matte whatever it is — only a face with
    // a *flat* alpha interior is un-composited — and letting it vote flipped two emoji with
    // soft glows onto a black matte, which bakes them dark and cost dE00 0.22 -> 5.31 on
    // `noto-emoji/emoji_u1f56f`. What the matte must not swallow is content that is
    // actually there.
    const MARGIN: f32 = 10.0;
    const DRAWN: f32 = 0.5;
    const DRAWN_FLOOR: f32 = 0.05;

    if let Some(v) = std::env::var_os("INKVEC_MATTE") {
        match v.to_string_lossy().to_lowercase().as_str() {
            "white" => return ([1.0, 1.0, 1.0], 0.0),
            "black" => return ([0.0, 0.0, 0.0], 0.0),
            "magenta" => return ([1.0, 0.0, 1.0], 0.0),
            _ => {}
        }
    }

    // What the matte has to keep: every pixel that is drawn but not fully opaque, plus the
    // opaque pixels along the silhouette, in coarse (colour, alpha) buckets.
    let (w, h) = (img.width, img.height);
    let alpha = |x: usize, y: usize| img.data[(y * w + x) * 4 + 3];
    // One pass over the alpha. Three kinds of pixel matter, and they are told apart by
    // their own alpha and their neighbours':
    //
    //   * the silhouette — solid pixels that meet transparency, and the rim between,
    //   * flat translucency — a panel drawn at one opacity, which the emitter can carry
    //     out as `fill-opacity` once the palette separates it from the ground,
    //   * a glow — translucency on a ramp, which no single opacity describes.
    //
    // The first two are what the matte must not swallow. The third gets no vote and, when
    // there is enough of it, keeps white outright: a glow is baked against the matte
    // whatever it is, there is no right answer (the file's eventual background is unknown),
    // and white is the conventional one. On `noto-emoji/emoji_u1f56f`, a white candle with
    // a soft flame, letting the flame vote chose a black matte that read the silhouette
    // correctly and baked the flame dark — dE00 0.22 -> 5.31.
    const SOFT_SHARE: f64 = 0.05;
    const FLAT_ALPHA: f32 = 0.02;
    let mut hist: std::collections::HashMap<[u8; 4], usize> = std::collections::HashMap::new();
    let mut drawn = 0usize;
    let mut soft = 0usize;
    for y in 0..h {
        for x in 0..w {
            let a = alpha(x, y);
            if a < DRAWN_FLOOR {
                continue;
            }
            let (mut lo, mut hi, mut meets_clear, mut meets_solid) = (a, a, false, false);
            for (nx, ny) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if nx >= w || ny >= h {
                    continue;
                }
                let na = alpha(nx, ny);
                meets_clear |= na < DRAWN_FLOOR;
                meets_solid |= na >= DRAWN;
                lo = lo.min(na);
                hi = hi.max(na);
            }
            let keep = if a >= DRAWN {
                a < 0.999 || meets_clear // the silhouette and its rim
            } else if hi - lo > FLAT_ALPHA && !meets_solid {
                soft += 1; // a glow: counted, but it does not vote
                false
            } else {
                true // translucency at one opacity
            };
            if !keep {
                continue;
            }
            drawn += 1;
            let p = &img.data[(y * w + x) * 4..(y * w + x) * 4 + 3];
            let q = |v: f32| ((v.clamp(0.0, 1.0) * 15.0).round() as u8).min(15);
            *hist
                .entry([
                    q(p[0]),
                    q(p[1]),
                    q(p[2]),
                    (a.clamp(0.0, 1.0) * 15.0).round() as u8,
                ])
                .or_default() += 1;
        }
    }
    if drawn == 0 || soft as f64 > SOFT_SHARE * (drawn + soft) as f64 {
        return ([1.0, 1.0, 1.0], 0.0);
    }
    let buckets: Vec<([f32; 3], f32, usize)> = hist
        .into_iter()
        .map(|(b, n)| {
            (
                [b[0] as f32 / 15.0, b[1] as f32 / 15.0, b[2] as f32 / 15.0],
                b[3] as f32 / 15.0,
                n,
            )
        })
        .collect();
    // Composited over the candidate, how much of that mass lands on the candidate itself.
    let swallowed = |cand: [f32; 3]| -> f64 {
        let lost: usize = buckets
            .iter()
            .filter(|&&(c, a, _)| {
                let over = [
                    c[0] * a + cand[0] * (1.0 - a),
                    c[1] * a + cand[1] * (1.0 - a),
                    c[2] * a + cand[2] * (1.0 - a),
                ];
                inkvec_trace::color::de00(over, cand) < MARGIN
            })
            .map(|&(_, _, n)| n)
            .sum();
        lost as f64 / drawn as f64
    };
    let cost_of_white = swallowed(CANDIDATES[0]);
    for cand in CANDIDATES {
        if swallowed(cand) < SWALLOWED {
            return (cand, cost_of_white);
        }
    }
    (
        CANDIDATES
            .into_iter()
            .min_by(|&x, &y| swallowed(x).total_cmp(&swallowed(y)))
            .unwrap_or([1.0, 1.0, 1.0]),
        cost_of_white,
    )
}

/// The input made opaque over `matte`, and the alphas it had, kept for the emitter.
pub(crate) struct AlphaSource {
    pub(crate) flat: inkvec_trace::Rgba,
    pub(crate) alpha: Vec<f32>,
    pub(crate) matte: [f32; 3],
}

/// The matted image, when the input has any transparency at all. `None` — and so not one
/// changed number anywhere downstream — for an image whose alpha channel is solid.
///
/// The matte is chosen against the artwork only under `--cutout`. That flag is what makes
/// the choice safe to act on: without it the transparency cannot reach the output anyway
/// (a transparent face is painted, not punched, and translucency is baked), so a matte
/// that suits the artwork would change every edge for no gain the file can carry. With
/// white fixed, the tracer is byte-for-byte what it was — which is what the committed CI
/// gate measures, and what it rejected the auto matte for: dE00 0.15404 -> 0.15654 against
/// a limit of 0.15558, on a corpus that is scored over white and cannot see the gain.
pub(crate) fn alpha_source(
    img: &inkvec_trace::Rgba,
    quiet: bool,
    cutout: bool,
) -> Option<AlphaSource> {
    if !img.data.iter().skip(3).step_by(4).any(|&a| a < 0.999) {
        return None;
    }
    let (chosen, cost_of_white) = choose_matte(img);
    let matte = if cutout { chosen } else { [1.0, 1.0, 1.0] };
    let (flat, alpha) = flatten_over(img, matte);
    if !quiet {
        let clear = alpha.iter().filter(|&&a| a < 0.05).count();
        eprintln!(
            "  alpha         {} matte, {:.0}% of the image transparent",
            inkvec_trace::color::to_hex(matte),
            100.0 * clear as f64 / alpha.len().max(1) as f64
        );
        if !cutout && cost_of_white > SWALLOWED {
            eprintln!(
                "  warning       {:.0}% of the artwork meets transparency in white and will \
be lost against the white matte; --cutout keeps it",
                100.0 * cost_of_white
            );
        }
    }
    Some(AlphaSource { flat, alpha, matte })
}

pub(crate) fn flatten_over(
    img: &inkvec_trace::Rgba,
    matte: [f32; 3],
) -> (inkvec_trace::Rgba, Vec<f32>) {
    let n = img.width * img.height;
    let mut data = Vec::with_capacity(n * 4);
    let mut alpha = Vec::with_capacity(n);
    for i in 0..n {
        let p = &img.data[i * 4..i * 4 + 4];
        let a = p[3].clamp(0.0, 1.0);
        for c in 0..3 {
            data.push(p[c] * a + matte[c] * (1.0 - a));
        }
        data.push(1.0);
        alpha.push(a);
    }
    (
        inkvec_trace::Rgba {
            width: img.width,
            height: img.height,
            data,
        },
        alpha,
    )
}

/// Undo [`flatten_over`] for a face the source drew translucent.
pub(crate) fn unmatte(c: [f32; 3], a: f32, matte: [f32; 3]) -> [f32; 3] {
    let a = a.max(1e-3);
    [
        ((c[0] - (1.0 - a) * matte[0]) / a).clamp(0.0, 1.0),
        ((c[1] - (1.0 - a) * matte[1]) / a).clamp(0.0, 1.0),
        ((c[2] - (1.0 - a) * matte[2]) / a).clamp(0.0, 1.0),
    ]
}

/// The four things the emitter needs to know about transparency, per face.
pub(crate) struct FaceAlpha {
    /// The source put nothing here: the face is a hole punched out of what is above it.
    pub(crate) clear: Vec<bool>,
    /// One opacity for the whole face, or 1.0 where the alpha is not flat enough to claim.
    pub(crate) opacity: Vec<f32>,
    /// The colour the image was composited onto before the tracer saw it.
    pub(crate) matte: [f32; 3],
    /// A face whose alpha fades linearly, as the gradient an editor would have drawn.
    pub(crate) alpha_ramps: Vec<Option<AlphaRamp>>,
}

/// Translucent layers: one shape at one opacity, seen against several grounds.
///
/// `alpha::decompose` recovers these from the face partition alone — no alpha channel
/// needed, because the evidence is that the differences between a layer's faces are
/// parallel to the differences between the grounds beneath them, scaled by `1 - a`. It
/// is what turns three overlapping circles at 85% into three circles instead of five
/// flat patches.
///
/// Two things make it safe to act on. The hypothesis is only entertained where the
/// cutout is already carrying transparency out, and it is only accepted when it
/// explains the faces to well inside the uncertainty of a face's own colour: a missed
/// layer costs parameters, an invented one is a visible error, and the module's own
/// documentation is emphatic about which way to lean.
///
/// Off by default, and the reason is compactness rather than correctness. The layer
/// reproduces the image exactly — the faces beneath it are repainted with the ground
/// and the layer is composited over them — but it *adds* a path rather than removing
/// any, because the ground pieces it should reunite are still separate faces at
/// different levels of the paint order. Reuniting them means relabelling and rebuilding
/// the map, which is the work this waits on. On real art it is rare besides: two of
/// forty icons in the census.
pub(crate) fn recover_layers(
    args: &Args,
    map: &planar::PlanarMap,
    face_color: &[usize],
    fills: &[gradient::FillFit],
    pal: &inkvec_trace::color::Palette,
    traced_labels: &[u16],
) -> Option<inkvec_trace::alpha::AlphaAnalysis> {
    if args.layers || std::env::var_os("INKVEC_LAYERS").is_some() {
        let n_faces = face_color.len();
        let mut area = vec![0usize; n_faces];
        for &l in traced_labels.iter() {
            if (l as usize) < n_faces {
                area[l as usize] += 1;
            }
        }
        let rgb_of: Vec<[f32; 3]> = (0..n_faces)
            .map(|f| match fills.get(f).map(|x| &x.model) {
                Some(gradient::FillModel::Flat(c)) => *c,
                _ => face_color
                    .get(f)
                    .and_then(|&ci| pal.rgb.get(ci))
                    .copied()
                    .unwrap_or([0.0, 0.0, 0.0]),
            })
            .collect();
        let mut adjacency: Vec<(usize, usize)> = map
            .edges
            .iter()
            .filter(|e| e.left != e.right)
            .map(|e| (e.left as usize, e.right as usize))
            .filter(|&(a, b)| a < n_faces && b < n_faces)
            .collect();
        adjacency.sort_unstable();
        adjacency.dedup();
        // The module's own advice: the compositing space is a property of the file, not a
        // constant, so try both and keep the fit that explains the faces better.
        let sigma_srgb = std::env::var("INKVEC_LAYER_SIGMA")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| v / 255.0)
            .unwrap_or(LAYER_SIGMA_SRGB);
        let best = [
            inkvec_trace::alpha::Space::Srgb,
            inkvec_trace::alpha::Space::Linear,
        ]
        .into_iter()
        .map(|space| {
            let opt = inkvec_trace::alpha::AlphaOptions {
                space,
                sigma_srgb,
                ..Default::default()
            };
            (
                space,
                inkvec_trace::alpha::decompose_with(&rgb_of, &area, &adjacency, &opt),
            )
        })
        .max_by_key(|(_, an)| an.layers.len());
        if let Some((space, an)) = &best {
            if !args.quiet && !an.layers.is_empty() {
                eprintln!(
                    "  layers        {} translucent layer(s) over a continuous ground ({:?})",
                    an.layers.len(),
                    space
                );
                for l in &an.layers {
                    eprintln!(
                        "                {} at {:.3} across {} faces, residual {:.5}",
                        inkvec_trace::color::to_hex(l.color),
                        l.alpha,
                        l.faces.len(),
                        l.residual
                    );
                }
            }
        }
        best.map(|(_, an)| an).filter(|an| !an.layers.is_empty())
    } else {
        None
    }
}

/// What the source's alpha says about each face.
/// or opaque. Upstream works on the image matted opaque, because unmixing a boundary
/// needs two opaque colours; this is where the transparency comes back.
///
/// Two measurements, and they are deliberately different. *Clear* is the mean over
/// every pixel of the face — the question is only "did the source put anything here",
/// and a face that is 3% ink at its anti-aliased rim is still nothing. *Translucent*
/// is measured on interior pixels alone, because the rim of an opaque shape is partial
/// alpha for a geometric reason, not a painterly one: taking the mean there would file
/// every small opaque mark, and every thin stroke, as half-transparent. A face with no
/// interior — a hairline, a one-pixel sliver — is therefore never thinned.
#[allow(clippy::too_many_arguments)]
pub(crate) fn face_alpha(
    img: &inkvec_trace::Rgba,
    args: &Args,
    alpha_src: Option<&AlphaSource>,
    face_color: &[usize],
    traced_labels: &[u16],
    w: usize,
    h: usize,
) -> FaceAlpha {
    const CLEAR_ALPHA: f32 = 0.05;
    const OPAQUE_ALPHA: f32 = 0.98;
    const MIN_INTERIOR: usize = 24;
    // One number can only describe one opacity. A face whose alpha varies across its
    // interior — a soft shadow, a fading glow — is not a translucent fill, and writing its
    // mean as `fill-opacity` is worse than leaving it baked over the matte: measured on the
    // screen set, thinning everything cost 0.4123 -> 0.4148, and thinning only the faces
    // that hold one opacity costs nothing.
    const FLAT_ALPHA_SD: f64 = 0.02;
    let n_faces = face_color.len();
    let (mut a_sum, mut a_n) = (vec![0.0f64; n_faces], vec![0usize; n_faces]);
    let (mut in_sum, mut in_n) = (vec![0.0f64; n_faces], vec![0usize; n_faces]);
    let mut in_sq = vec![0.0f64; n_faces];
    if let Some(src) = alpha_src {
        for (i, &l) in traced_labels.iter().enumerate() {
            let f = l as usize;
            if f >= n_faces {
                continue;
            }
            let a = src.alpha.get(i).copied().unwrap_or(1.0) as f64;
            a_sum[f] += a;
            a_n[f] += 1;
            let (x, y) = (i % w, i / w);
            let interior = x > 0
                && y > 0
                && x + 1 < w
                && y + 1 < h
                && traced_labels[i - 1] == l
                && traced_labels[i + 1] == l
                && traced_labels[i - w] == l
                && traced_labels[i + w] == l;
            if interior {
                in_sum[f] += a;
                in_sq[f] += a * a;
                in_n[f] += 1;
            }
        }
    }
    // A small hole is mostly rim. The rim is anti-aliased against the opaque shape around
    // it, so its partial alpha lifts the face's mean over CLEAR_ALPHA even when nothing was
    // drawn inside: a 19 px transparent square in a cap measured mean 0.065 over 396 pixels
    // with all 323 interior pixels at exactly 0, and came out painted in the matte colour.
    // Where there is enough interior to judge by, an interior that is fully transparent makes
    // the face a hole whatever its rim says.
    const CLEAR_INTERIOR: f64 = 0.01;
    let clear: Vec<bool> = (0..n_faces)
        .map(|f| {
            a_n[f] > 0
                && ((a_sum[f] / a_n[f] as f64) < CLEAR_ALPHA as f64
                    || (in_n[f] >= MIN_INTERIOR && in_sum[f] / (in_n[f] as f64) < CLEAR_INTERIOR))
        })
        .collect();
    let opacity: Vec<f32> = (0..n_faces)
        .map(|f| {
            if in_n[f] < MIN_INTERIOR {
                return 1.0;
            }
            let n = in_n[f] as f64;
            let mean = in_sum[f] / n;
            let sd = (in_sq[f] / n - mean * mean).max(0.0).sqrt();
            let a = mean as f32;
            if a > OPAQUE_ALPHA || a < CLEAR_ALPHA || sd > FLAT_ALPHA_SD {
                1.0
            } else {
                a
            }
        })
        .collect();
    let matte = alpha_src.map(|s| s.matte).unwrap_or([1.0, 1.0, 1.0]);
    // A face whose opacity fades across it gets a gradient instead of one number. Only
    // where the cutout is carrying transparency out at all, and only for flat-coloured
    // faces: a colour ramp and an alpha ramp in one face is a fill model this does not
    // have, and guessing at it would be worse than baking.
    let matted_rgb = alpha_src.map(|_| img.composited([1.0, 1.0, 1.0]));
    let alpha_ramps: Vec<Option<AlphaRamp>> = (0..n_faces)
        .map(|f| {
            let src = alpha_src?;
            if !args.cutout {
                return None;
            }
            // Deliberately not restricted to flat-filled faces. A fade over a white matte
            // *looks* like a colour ramp towards white — the ramp case here fits
            // `#ca774d -> #fbf3ef` — and the alpha channel is the evidence that says which
            // of the two it is. Where the source's alpha fades linearly across the face,
            // that is the explanation, and it is the one an editor can work with.
            if opacity[f] < 1.0 || clear[f] {
                return None; // a wash, or nothing at all
            }
            fit_alpha_ramp(
                f,
                traced_labels,
                &src.alpha,
                matted_rgb.as_ref()?,
                matte,
                w,
                h,
            )
        })
        .collect();
    if std::env::var_os("INKVEC_ALPHADBG").is_some() {
        for f in 0..n_faces {
            if a_n[f] > 0 {
                eprintln!(
                    "  face {f}: {} px, mean alpha {:.4}, interior {} at {:.4}, clear {}, opacity {:.3}",
                    a_n[f],
                    a_sum[f] / a_n[f] as f64,
                    in_n[f],
                    if in_n[f] > 0 { in_sum[f] / in_n[f] as f64 } else { f64::NAN },
                    clear[f],
                    opacity[f]
                );
            }
        }
    }
    FaceAlpha {
        clear,
        opacity,
        matte,
        alpha_ramps,
    }
}
