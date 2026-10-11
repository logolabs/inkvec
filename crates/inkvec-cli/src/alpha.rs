//! Transparency, on the way in and on the way out.
//!
//! Under `--no-native-alpha` the tracer proper is opaque: palette, labels, unmix and fills
//! are all RGB and never see an alpha channel. So an RGBA input is **matted** here —
//! composited onto one chosen opaque colour, with the original alphas kept aside — and the
//! transparency is put back at the end as holes, `fill-opacity` and alpha ramps. Native
//! alpha (the default) still writes the image down over white, but hands the alphas to
//! the colour trace as well, so its palette finds inks with an opacity of their own. The
//! pipeline then replaces [`face_alpha`]'s per-face "clear" and opacity verdicts with the
//! inks' own and keeps its alpha ramps for the faces whose ink is opaque.
//!
//! Which matte is chosen matters and is not free: it must differ from the ink it meets or
//! the silhouette dissolves, and must not differ from whatever a soft edge will be
//! composited against or that edge bakes wrong. [`choose_matte`] resolves that, and
//! `docs/ALPHA.md` records what the compromise costs and what would remove it.
//!
//! Where each piece is called from:
//!
//! * intake, in `lib.rs`: [`pixel_grid`] (undoing a nearest-neighbour upscale), then, once
//!   every resampling step is done, [`alpha_source_owned`] (the matte written over the
//!   input's own buffer; the copying [`alpha_source`] is the tests' oracle for it) and
//!   [`cutout_args`]; everything after traces [`AlphaSource::flat`];
//!
//! [`pixel_grid`] (in `alpha/unblock.rs`) is the exact inverse of a nearest-neighbour
//! upscale by any factor of 2 or more; `intake_tests` holds the integer-only test it
//! replaced, as an oracle for the factors both find. The alpha scan and the flatten are
//! exact rewrites of their earlier serial versions, kept as test oracles there too: parallel
//! maps whose every output depends on one input pixel.
//! * emit time, in `pipeline.rs`: [`recover_layers`] (`--layers`) and [`face_alpha`], whose
//!   [`FaceAlpha`] tells the emitter which faces are holes, which are translucent and which
//!   fade;
//! * the emitter, `emit.rs`: [`unmatte`] and [`AlphaRamp`] when writing a translucent face.
//!
//! Conventions: colours are sRGB-encoded channel values in `[0, 1]` with straight
//! (unpremultiplied) alpha, as `inkvec_trace::Rgba` holds them, and compositing is the
//! usual "over" operator applied to those encoded values, `c' = a·c + (1 − a)·M`. Positions
//! are in px with pixel centres at integer coordinates, so the canvas spans
//! `-0.5 .. w - 0.5`. Pixel arrays are row-major, index `y · w + x`.
//!
//! # The emit side, in passes
//!
//! [`face_alpha`] is linear in the pixel count: one pass gathers every face's alpha
//! statistics, the verdicts (clear, one opacity) come from those sums, and the alpha-ramp
//! fit runs only for faces where it can succeed ([`ramp_candidate`] proves the rest `None`),
//! each on its own interior pixels gathered in one more pass ([`interior_pixels`]). Each
//! function says why its output is bit-identical to the per-face whole-image scans it
//! replaced.

use crate::args::Args;
use crate::diag;
use inkvec_core::Point;

mod layers;
pub(crate) use layers::recover_layers;

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
///
/// The emitter writes it as a `linearGradient` with `gradientUnits="userSpaceOnUse"` from
/// `p0` to `p1`, so the two points are in the trace's own pixel coordinates.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AlphaRamp {
    /// Start of the gradient axis, px: the face's most transparent end. The axis points up
    /// the fitted alpha gradient, so `a0 <= a1` always; see [`fit_alpha_ramp`].
    pub(crate) p0: Point,
    /// End of the gradient axis, px: the face's most opaque end.
    pub(crate) p1: Point,
    /// Opacity at `p0`, in `[0, 1]`: the `stop-opacity` of the gradient's first stop.
    pub(crate) a0: f32,
    /// Opacity at `p1`, in `[0, 1]`: the `stop-opacity` of the second stop.
    pub(crate) a1: f32,
    /// The one ink colour both stops share, un-matted, as sRGB in `[0, 1]`.
    pub(crate) color: [f32; 3],
}

/// A fade has to actually fade: at least this much opacity across the face, or it is a
/// wash ([`fit_alpha_ramp`]).
const RAMP_MIN_FADE: f32 = 0.15;
/// And it has to fade *linearly*: the largest RMS residual of the plane fit, in opacity
/// ([`fit_alpha_ramp`]).
const RAMP_MAX_RESIDUAL: f32 = 0.06;
/// Fewest interior pixels a face needs before a plane is fitted to its alpha
/// ([`fit_alpha_ramp`]); [`face_alpha`] skips a face below it without calling the fit.
const RAMP_MIN_INTERIOR: usize = 64;

/// Fit [`AlphaRamp`] to one face's interior pixels.
///
/// Returns `None` for a face that is flat (that is a `fill-opacity`, handled elsewhere),
/// for one with too few interior pixels to fit three coefficients against, and for one
/// whose alpha is not linear enough to be called a fade — a face that is opaque in two
/// places and clear between them is not a ramp, and inventing one would be worse than
/// baking it.
///
/// The idea: if a face fades linearly, its alpha is a tilted plane over the image, and an
/// SVG linear gradient is exactly that plane restricted to the face. So fit the plane,
/// check it really explains the alpha, read the two end opacities off it, and recover the
/// one colour under the fade.
///
/// 1. **Plane.** Over the face's interior pixels (the pixel and its four neighbours all
///    carry label `face`, so the anti-aliased rim does not vote), fit
///    `a(x, y) ≈ b0 + b1·x + b2·y` by ordinary least squares: minimise
///    `Σ_i (a_i − b0 − b1·x_i − b2·y_i)²`, where `(x_i, y_i)` is pixel `i`'s centre in px and
///    `a_i` its source alpha. The normal equations are the 3x3 system `M b = r` with
///    `M = [[n, Σx, Σy], [Σx, Σx², Σxy], [Σy, Σxy, Σy²]]` and `r = [Σa, Σa·x, Σa·y]`,
///    solved by [`solve3x3`].
/// 2. **Axis.** The plane's gradient `g = (b1, b2)` gives the fade's direction
///    `u = g / |g|` and its slope `|g|` (opacity per px). Projecting every interior pixel
///    onto `u`, `t_i = x_i·u_x + y_i·u_y`, gives the face's extent along the fade,
///    `[t_min, t_max]`. The axis ends are `p0 = t_min·u` and `p1 = t_max·u`: points on the
///    line through the origin, which is enough because a linear gradient is constant along
///    every line perpendicular to its axis. The end opacities are the plane there,
///    `a0 = b0 + |g|·t_min` and `a1 = b0 + |g|·t_max`, clamped to `[0, 1]`.
/// 3. **Tests.** The RMS residual of the plane must be at most [`RAMP_MAX_RESIDUAL`] (0.06
///    in opacity) and the fade must span at least [`RAMP_MIN_FADE`] (0.15), or the face is a
///    wash or something that is not linear.
/// 4. **Colour.** Each interior pixel with `a ≥ 0.25` is un-matted,
///    `C = (c − (1 − a)·M) / a` with `c` its matted colour and `M` the matte, and the
///    estimates are averaged with weight `a²` (see the comment at that step for why).
///
/// # Inputs
///
/// * `interior`: the face's interior pixels as `(x, y)`, **in row-major order** (increasing
///   `y · w + x`), as [`face_alpha`] gathers them. The order is part of the contract: every
///   sum below is a floating-point sum, and it is accumulated in exactly the order the
///   earlier whole-image scan visited the pixels, which is what keeps the result
///   bit-identical to that scan (see "Why this equals the scan" below);
/// * `alpha`: the source alpha per pixel, `w · h` row-major, in `[0, 1]`;
/// * `img`: the image the trace saw, matted over `matte` and opaque (every alpha 1),
///   `w` wide; its colour at a pixel is read through [`over_white`];
/// * `matte`: the colour `img` was composited onto, sRGB `[0, 1]`;
/// * `w`: the width the pixel index `i = y · w + x` into `alpha` and `img` is taken with,
///   the label map's width (the image's).
///
/// `None` when there are fewer than [`RAMP_MIN_INTERIOR`] (64) interior pixels, when the
/// normal equations are singular (all interior pixels on one line), when the plane is flat
/// (`|g| < 1e-9`), when either test fails, or when no interior pixel is opaque enough to take
/// the colour from. The alphas are assumed finite, as a decoded image's are: every test here
/// is written as "reject when above/below", and a comparison with NaN is false, so a NaN
/// alpha would slip through all of them and come out as a NaN ramp rather than `None`.
///
/// # Cost
///
/// `O(|interior|)`: two passes over the list (sums, then extent and residual) and one more
/// for the colour when the tests pass. The function used to take the whole label map and
/// find the face's pixels itself, which made [`face_alpha`] `O(faces · w · h)`: at 2048 px on
/// the three transparent test images that was 60.7 ms of an 88.5 ms stage (research
/// 2026-09-30, `research-fast-shared`).
///
/// # Why this equals the scan
///
/// The scan visited `y` in `1..h−1`, then `x` in `1..w−1`, kept the pixels whose own label
/// and four neighbours' labels were `face`, and accumulated each sum (`n`, `Σx`, ..., `Σa·y`)
/// in that visiting order, then iterated its list of kept pixels in the same order for the
/// extent, the residual and the colour. [`face_alpha`] keeps exactly the same pixels (the
/// same interior test on the same labels) in the same row-major order, and this function
/// performs the same floating-point operations on the same values in the same order, so
/// every intermediate is the same double, bit for bit. The one changed read is the colour:
/// the scan read `img.composited([1, 1, 1])[i]`, and [`over_white`] evaluates that same
/// expression for pixel `i` alone. The old function is kept as `fit_alpha_ramp_scan` in the
/// test module `alpha/ramp_tests.rs`, which asserts equality on random and degenerate maps.
pub(crate) fn fit_alpha_ramp(
    interior: &[(u32, u32)],
    alpha: &[f32],
    img: &inkvec_trace::Rgba,
    matte: [f32; 3],
    w: usize,
) -> Option<AlphaRamp> {
    let (mut n, mut sx, mut sy, mut sxx, mut sxy, mut syy) = (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut sa, mut sax, mut say) = (0.0f64, 0.0, 0.0);
    // Pass 1: the moments of the plane fit's normal equations. Each accumulator sees its
    // terms in row-major pixel order, as the scan's did.
    for &(x, y) in interior {
        let i = y as usize * w + x as usize;
        let (fx, fy) = (f64::from(x), f64::from(y));
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
    }
    if n < RAMP_MIN_INTERIOR as f64 {
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

    // Pass 2: the extremes of the face along the gradient direction, and the residual.
    let (ux, uy) = (b1 / grad, b2 / grad);
    let (mut tmin, mut tmax) = (f64::MAX, f64::MIN);
    let mut resid = 0.0f64;
    for &(x, y) in interior {
        let (x_, y_) = (f64::from(x), f64::from(y));
        let a = alpha[y as usize * w + x as usize];
        let t = x_ * ux + y_ * uy;
        tmin = tmin.min(t);
        tmax = tmax.max(t);
        let model = b0 + b1 * x_ + b2 * y_;
        resid += (a as f64 - model) * (a as f64 - model);
    }
    let rms = (resid / n).sqrt() as f32;
    if rms > RAMP_MAX_RESIDUAL {
        return None;
    }
    let (a0, a1) = (
        (b0 + grad * tmin).clamp(0.0, 1.0) as f32,
        (b0 + grad * tmax).clamp(0.0, 1.0) as f32,
    );
    if (a1 - a0).abs() < RAMP_MIN_FADE {
        return None;
    }

    // One colour for the whole face, un-matted per pixel and weighted towards the opaque
    // end: `c_obs = a C + (1-a) M`, so `C = (c_obs - (1-a) M) / a`, whose noise blows up as
    // `a` falls. Weighting by `a²` is the inverse-variance weight for exactly that.
    let (mut cw, mut acc) = (0.0f64, [0.0f64; 3]);
    for &(x, y) in interior {
        let i = y as usize * w + x as usize;
        let a = alpha[i];
        if a < 0.25 {
            continue;
        }
        let wgt = (a * a) as f64;
        let rgb = over_white(img, i);
        for k in 0..3 {
            let c = (rgb[k] - (1.0 - a) * matte[k]) / a;
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

/// Pixel `i` of `img` composited over white: `c_k · a + 1 · (1 − a)` per channel `k`, with
/// `c` the pixel's straight colour and `a` its alpha, sRGB `[0, 1]`.
///
/// This is `img.composited([1.0, 1.0, 1.0])[i]`, written out for one pixel with the same
/// operations in the same order (`bg · (1 − a)` with `bg = 1.0`, then the sum), so the f32
/// result is the same bit pattern. [`fit_alpha_ramp`] needs the colour of a few interior
/// pixels, and building the whole composited image for them cost a full pass and a
/// `12 · w · h`-byte allocation per trace with transparency, whether or not any fit ran.
/// `O(1)`; panics if `i` is outside the image, as indexing the composited image would.
fn over_white(img: &inkvec_trace::Rgba, i: usize) -> [f32; 3] {
    let p = &img.data[i * 4..i * 4 + 4];
    let a = p[3];
    let bg = [1.0f32; 3];
    [
        p[0] * a + bg[0] * (1.0 - a),
        p[1] * a + bg[1] * (1.0 - a),
        p[2] * a + bg[2] * (1.0 - a),
    ]
}

/// 3x3 solve by Cramer's rule; `None` when the system is singular.
///
/// `x_k = det(M_k) / det(M)`, where `M_k` is `m` with column `k` replaced by `r`. For a 3x3
/// system this is as cheap as elimination and needs no pivoting code. "Singular" means
/// `|det(M)| < 1e-12`, an absolute threshold: it is meant for the normal equations of
/// [`fit_alpha_ramp`], whose determinant grows with the pixel count and the spread of the
/// coordinates, so a genuine face is far above it and a degenerate one (every pixel on one
/// line) is at or near zero. A NaN determinant fails the test and gives NaN results rather
/// than `None`.
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

mod unblock;
pub(crate) use unblock::pixel_grid;
#[cfg(test)]
use unblock::PixelGrid;

/// Share of the artwork a candidate matte may hide before it is rejected.
///
/// Module-level rather than local to `choose_matte`, because `alpha_source` warns the user
/// at the same threshold and restated it as a literal `0.33` until 2026-09-08 -- two
/// numbers that had to agree, with nothing keeping them in agreement.
pub(crate) const SWALLOWED: f64 = 0.33;

/// Share of the drawn silhouette that, composited over white, the palette cannot tell from
/// white at all ([`inkvec_trace::color::SAME_INK_DE00`]). Above it the white matte has erased
/// the artwork's outline and [`alpha_source`] turns the cutout on.
///
/// Deliberately not [`SWALLOWED`], whose margin (dE00 10) is for choosing a matte colour:
/// at that margin a near-white edge counts as lost although the tracer separates it from
/// white easily, and turning the cutout on for those images made the printer and bride emoji
/// worse on every ground (`noto-emoji/emoji_u1f5a8` dE00 0.53 -> 0.76). At the same-ink
/// margin, white marks on a transparent ground read 1.00 and no icon of the 246-icon screen
/// set reads above 0.32, so a majority is far from both.
pub(crate) const LOST_TO_WHITE: f64 = 0.5;

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
///
/// The measurement, in one pass over the pixels with alpha `a ≥ 0.05` (`DRAWN_FLOOR`):
///
/// * the **drawn mass** is the pixels that vote: every pixel with `0.5 ≤ a < 0.999`, every
///   opaque pixel with a clear 4-neighbour (`a < 0.05`), and every pixel with `a < 0.5`
///   whose neighbourhood alpha is flat (range at most 0.02) or which touches a solid
///   neighbour. Opaque pixels deep inside a shape do not vote: no matte reaches them;
/// * a **glow** is a pixel with `a < 0.5`, alpha varying across its neighbourhood and no
///   solid neighbour. Glows do not vote, and if they are more than 5% (`SOFT_SHARE`) of
///   glow plus drawn mass, white is returned at once (with both shares reported as 0);
/// * a candidate matte `M` **swallows** a drawn pixel of colour `c` when its composite
///   over `M` is within CIEDE2000 distance 10 (`MARGIN`) of `M` itself:
///   `ΔE00(a·c + (1 − a)·M, M) < 10`. Its cost is the swallowed share of the drawn mass,
///   computed on a histogram with 16 levels per channel and 16 of alpha, so each
///   candidate costs a pass over at most 16⁴ buckets (far fewer in practice) rather than
///   over the image.
///
/// The first candidate, in the order white, black, magenta, green, cyan, orange, whose cost
/// is under [`SWALLOWED`] is the matte; if none is, the cheapest (the earlier on a tie).
///
/// Returns `(matte, white's cost, lost to white)`, the matte as sRGB in `[0, 1]`. The
/// second value is white's cost whichever matte won, not the winner's. The third is the
/// share of the drawn mass that, composited over white, is at least 0.9 in every channel
/// and within [`inkvec_trace::color::SAME_INK_DE00`] of white, which [`alpha_source`]
/// compares with [`LOST_TO_WHITE`]. An image with nothing drawn returns white and two
/// zeros.
pub(crate) fn choose_matte(img: &inkvec_trace::Rgba) -> ([f32; 3], f64, f64) {
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
    let mut lost = 0usize;
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
            let over_white = [p[0] * a + 1.0 - a, p[1] * a + 1.0 - a, p[2] * a + 1.0 - a];
            if over_white.iter().all(|&v| v >= 0.9)
                && inkvec_trace::color::de00(over_white, [1.0, 1.0, 1.0])
                    < inkvec_trace::color::SAME_INK_DE00
            {
                lost += 1;
            }
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
        return ([1.0, 1.0, 1.0], 0.0, 0.0);
    }
    let lost_to_white = lost as f64 / drawn as f64;
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
            return (cand, cost_of_white, lost_to_white);
        }
    }
    (
        CANDIDATES
            .into_iter()
            .min_by(|&x, &y| swallowed(x).total_cmp(&swallowed(y)))
            .unwrap_or([1.0, 1.0, 1.0]),
        cost_of_white,
        lost_to_white,
    )
}

/// The input made opaque over `matte`, and the alphas it had, kept for the emitter.
pub(crate) struct AlphaSource {
    /// The input composited over `matte` ([`flatten_over`]): same size, every alpha 1.
    /// This is the image the rest of the trace sees.
    pub(crate) flat: inkvec_trace::Rgba,
    /// The input's own alpha per pixel, clamped to `[0, 1]`, row-major, `width · height`
    /// long.
    pub(crate) alpha: Vec<f32>,
    /// The colour `flat` was composited onto, sRGB in `[0, 1]`: white unless the matte was
    /// chosen against the artwork (see [`alpha_source`]).
    pub(crate) matte: [f32; 3],
    /// Whether the transparency is carried out as `--cutout` does: asked for, or turned on
    /// here because the white matte would have swallowed the artwork. Everything downstream
    /// has to agree with the matte, so callers pass [`cutout_args`] on, not their own args.
    pub(crate) cutout: bool,
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
///
/// Except where white erases the artwork. A white mark on a transparent ground, composited
/// over white, is one flat colour: the trace came back as a single white rectangle, and with
/// `--no-background` as an empty document — the LogoLabs flask in white scored alpha error
/// 0.89 either way, and 0.0008 with the cutout. There is no trace of the artwork to keep
/// byte-for-byte there, so the cutout is turned on for that image, when more than
/// [`LOST_TO_WHITE`] of the drawn silhouette is paint the palette cannot tell from white.
/// Artwork with a soft glow reports nothing lost (white is kept for the glow's sake) and so
/// stays as it was.
///
/// With `native` (native alpha, the default) none of that applies: the image is written
/// over white, no matte is chosen, and the cutout is always on, because the alpha travels
/// with the image into the colour trace and the output carries it out.
///
/// "Any transparency" means at least one pixel with alpha under 0.999. Unless `quiet`, the
/// matte and the share of clear pixels (alpha under 0.05) go to stderr, and so does a note
/// when the cutout was turned on here.
#[cfg(test)]
pub(crate) fn alpha_source(
    img: &inkvec_trace::Rgba,
    quiet: bool,
    cutout: bool,
    native: bool,
) -> Option<AlphaSource> {
    let plan = MattePlan::of(img, cutout, native)?;
    let (flat, alpha) = flatten_over(img, plan.matte);
    Some(plan.finish(flat, alpha, quiet))
}

/// [`alpha_source`] for an image the caller is done with: the matted image is written over
/// the input's own buffer instead of a new one. `Err` gives the image back untouched when
/// it has no transparency (where `alpha_source` returns `None`).
///
/// The intake calls this once every resampling step is done, and never reads the unmatted
/// image again, so the copy [`flatten_over`] makes was pure cost: a fresh 64 MB buffer at
/// 2048 px, whose page faults and release the shared-stage research measured at 5.7 +
/// 1.7 ms. The matte is decided on the untouched image first ([`MattePlan::of`]), then
/// every pixel is flattened by the same [`flatten_pixel`] -- the same values, in the same
/// places, so the result is the image `alpha_source` returns.
///
/// Not from the literature: buffer reuse, because there is nothing to choose between.
/// See also: D. Leijen, B. Zorn, L. de Moura, "Mimalloc: Free List Sharding in Action",
/// APLAS 2019 -- the allocator-side answer to the same page-fault cost, which would also
/// help the stages this cannot reach.
pub(crate) fn alpha_source_owned(
    img: inkvec_trace::Rgba,
    quiet: bool,
    cutout: bool,
    native: bool,
) -> Result<AlphaSource, inkvec_trace::Rgba> {
    let Some(plan) = MattePlan::of(&img, cutout, native) else {
        return Err(img);
    };
    let (flat, alpha) = flatten_in_place(img, plan.matte);
    Ok(plan.finish(flat, alpha, quiet))
}

/// What [`alpha_source`] decided for an image with transparency, before any pixel is
/// flattened: the matte, whether the cutout is on, and what to tell the user.
struct MattePlan {
    /// The colour to flatten onto, sRGB 0..1.
    matte: [f32; 3],
    /// Whether the transparency is carried out as `--cutout` does.
    cutout: bool,
    /// Native alpha: nothing was chosen, the image is only written over white.
    native: bool,
    /// Share of the silhouette lost to a white matte, when that turned the cutout on here.
    swallowed: Option<f64>,
}

impl MattePlan {
    /// `None` for an image with no alpha under 0.999 ([`has_transparency`]). Natively
    /// traced: white, cutout on. Otherwise [`choose_matte`] on the untouched image, with the
    /// cutout turned on when more than [`LOST_TO_WHITE`] of the silhouette would vanish into
    /// white; the chosen matte only applies under the cutout.
    fn of(img: &inkvec_trace::Rgba, cutout: bool, native: bool) -> Option<Self> {
        if !has_transparency(img) {
            return None;
        }
        if native {
            // Nothing is chosen and nothing is lost: over white is only how the colour is
            // written down, and the alpha travels beside it into every stage that unmixes.
            // The output carries the transparency out, as the cutout does.
            return Some(MattePlan {
                matte: [1.0, 1.0, 1.0],
                cutout: true,
                native: true,
                swallowed: None,
            });
        }
        let (chosen, _, lost_to_white) = choose_matte(img);
        let swallowed = !cutout && lost_to_white > LOST_TO_WHITE;
        let cutout = cutout || swallowed;
        Some(MattePlan {
            matte: if cutout { chosen } else { [1.0, 1.0, 1.0] },
            cutout,
            native: false,
            swallowed: swallowed.then_some(lost_to_white),
        })
    }

    /// The [`AlphaSource`] for the flattened image, with the stderr notes the old
    /// `alpha_source` printed, in the same order: the matte (or "native") and the share of
    /// clear pixels (alpha under 0.05), then, when the cutout was turned on here, why.
    fn finish(self, flat: inkvec_trace::Rgba, alpha: Vec<f32>, quiet: bool) -> AlphaSource {
        diag::stage(quiet, || {
            let clear = alpha.iter().filter(|&&a| a < 0.05).count();
            let share = 100.0 * clear as f64 / alpha.len().max(1) as f64;
            if self.native {
                format!("  alpha         native, {share:.0}% of the image transparent")
            } else {
                format!(
                    "  alpha         {} matte, {share:.0}% of the image transparent",
                    inkvec_trace::color::to_hex(self.matte)
                )
            }
        });
        if let Some(lost_to_white) = self.swallowed {
            diag::stage(quiet, || {
                format!(
                    "  cutout        {:.0}% of the outline is white and would vanish into a white matte; carrying the transparency out as --cutout does",
                    100.0 * lost_to_white
                )
            });
        }
        AlphaSource {
            flat,
            alpha,
            matte: self.matte,
            cutout: self.cutout,
        }
    }
}

/// `args` as the rest of the trace must see them once [`alpha_source`] has decided: with
/// `--cutout` on when it turned the cutout on for this image. Borrowed unchanged in every
/// other case, so the common path clones nothing.
pub(crate) fn cutout_args<'a>(
    args: &'a Args,
    src: Option<&AlphaSource>,
) -> std::borrow::Cow<'a, Args> {
    match src {
        Some(s) if s.cutout && !args.cutout => std::borrow::Cow::Owned(Args {
            cutout: true,
            ..args.clone()
        }),
        _ => std::borrow::Cow::Borrowed(args),
    }
}

/// Composite `img` over an opaque `matte` and set its alpha aside.
///
/// This is the "over" operator of alpha compositing with an opaque background, per pixel
/// and channel: `c' = a·c + (1 − a)·M`, where `c` is the pixel's straight (unpremultiplied)
/// colour, `a` its alpha clamped to `[0, 1]`, and `M` the matte, all sRGB-encoded values in
/// `[0, 1]`. It blends the encoded values rather than linear light, which is what a browser
/// does when it paints a translucent SVG fill, and so what [`unmatte`] has to undo.
///
/// Returns the opaque image (every alpha 1) and the clamped alphas, row-major, one per
/// pixel. A NaN alpha is not cleaned up: `clamp` passes it through. Panics if `img.data` is
/// shorter than `4 · width · height`, as it always did.
///
/// Every pixel is computed by [`flatten_pixel`] alone, so the image is cut into chunks of
/// [`FLATTEN_CHUNK`] pixels and flattened in parallel above [`INTAKE_PARALLEL_MIN`] pixels:
/// the same expression on the same inputs, whatever thread runs it. It was a serial push loop,
/// 26.7 ms of a 2048 px transparent trace.
///
/// Method from: T. Porter, T. Duff, "Compositing Digital Images", SIGGRAPH '84,
/// pp. 253–259, DOI 10.1145/800031.808606 -- "over" with an opaque background. Adapted to
/// straight (unpremultiplied) colour, as `inkvec_trace::Rgba` stores it.
#[cfg(test)]
pub(crate) fn flatten_over(
    img: &inkvec_trace::Rgba,
    matte: [f32; 3],
) -> (inkvec_trace::Rgba, Vec<f32>) {
    use rayon::prelude::*;
    let n = img.width * img.height;
    let src = &img.data[..n * 4];
    let mut data = vec![0.0f32; n * 4];
    let mut alpha = vec![0.0f32; n];
    let run = |((out, a), src): ((&mut [f32], &mut [f32]), &[f32])| {
        for ((o, a), p) in out
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(a.iter_mut())
            .zip(src.as_chunks::<4>().0)
        {
            o.copy_from_slice(p);
            *a = flatten_pixel(o, matte);
        }
    };
    if n >= INTAKE_PARALLEL_MIN {
        data.par_chunks_mut(4 * FLATTEN_CHUNK)
            .zip(alpha.par_chunks_mut(FLATTEN_CHUNK))
            .zip(src.par_chunks(4 * FLATTEN_CHUNK))
            .for_each(run);
    } else {
        run(((&mut data, &mut alpha), src));
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

/// Flatten one pixel `p = [r, g, b, a]` over `matte` in place and return its clamped alpha:
/// with `a' = clamp(a, 0, 1)`, `p ← [r·a' + M_r·(1 − a'), g·a' + M_g·(1 − a'),
/// b·a' + M_b·(1 − a'), 1]`. The arithmetic of the old push loop, operand for operand, so
/// the floats are the same.
#[inline]
fn flatten_pixel(p: &mut [f32; 4], matte: [f32; 3]) -> f32 {
    let a = p[3].clamp(0.0, 1.0);
    for c in 0..3 {
        p[c] = p[c] * a + matte[c] * (1.0 - a);
    }
    p[3] = 1.0;
    a
}

/// [`flatten_over`] written over `img`'s own buffer: the same pixels, flattened by the same
/// [`flatten_pixel`], and the same clamped alphas, without a second image-sized buffer.
/// Parallel above [`INTAKE_PARALLEL_MIN`] pixels. Panics if `img.data` is shorter than
/// `4 · width · height`; floats past that length are dropped, as the copy never had them.
fn flatten_in_place(
    mut img: inkvec_trace::Rgba,
    matte: [f32; 3],
) -> (inkvec_trace::Rgba, Vec<f32>) {
    use rayon::prelude::*;
    let n = img.width * img.height;
    img.data.truncate(n * 4);
    assert_eq!(
        img.data.len(),
        n * 4,
        "an RGBA image holds four floats per pixel"
    );
    let mut alpha = vec![0.0f32; n];
    let run = |(px, a): (&mut [f32], &mut [f32])| {
        for (p, a) in px.as_chunks_mut::<4>().0.iter_mut().zip(a.iter_mut()) {
            *a = flatten_pixel(p, matte);
        }
    };
    if n >= INTAKE_PARALLEL_MIN {
        img.data
            .par_chunks_mut(4 * FLATTEN_CHUNK)
            .zip(alpha.par_chunks_mut(FLATTEN_CHUNK))
            .for_each(run);
    } else {
        run((&mut img.data, &mut alpha));
    }
    (img, alpha)
}

/// Below this many pixels (256 × 256) the intake's alpha scan and flatten run on the
/// calling thread: at 128 px they take microseconds.
const INTAKE_PARALLEL_MIN: usize = 1 << 16;
/// Pixels per parallel job of [`flatten_over`] and [`has_transparency`].
const FLATTEN_CHUNK: usize = 1 << 14;

/// Whether any pixel of `img` has an alpha under 0.999 ([`alpha_source`]'s "any
/// transparency").
///
/// Reads the fourth float of every whole pixel -- indices `4i + 3` below `data.len()`, the
/// same set the old `iter().skip(3).step_by(4)` visited -- and stops at the first
/// translucent one. On an opaque image that is a read of every alpha (3.3 ms serial at
/// 2048 px), now split over rayon's workers above [`INTAKE_PARALLEL_MIN`] pixels. `any` is
/// a pure predicate, so the answer does not depend on the split.
fn has_transparency(img: &inkvec_trace::Rgba) -> bool {
    use rayon::prelude::*;
    let translucent = |p: &[f32; 4]| p[3] < 0.999;
    if img.data.len() / 4 >= INTAKE_PARALLEL_MIN {
        img.data
            .as_chunks::<4>()
            .0
            .par_iter()
            .with_min_len(FLATTEN_CHUNK)
            .any(translucent)
    } else {
        img.data.as_chunks::<4>().0.iter().any(translucent)
    }
}

/// Undo [`flatten_over`] for a face the source drew translucent.
///
/// Solving `c = a·C + (1 − a)·M` for the ink `C` gives `C = (c − (1 − a)·M) / a`, with `c`
/// the matted colour the tracer measured, `a` the face's opacity and `M` the matte (sRGB
/// `[0, 1]` throughout). The division amplifies any error in `c` by `1 / a`, so a faint face
/// comes back with a noisy colour; the result is clamped to `[0, 1]` to keep it paintable,
/// and `a` is floored at 0.001 so a clear face cannot divide by zero. Written with that
/// `C` at `fill-opacity = a` over the same matte, the face reproduces `c` (unless the clamp
/// had to act).
pub(crate) fn unmatte(c: [f32; 3], a: f32, matte: [f32; 3]) -> [f32; 3] {
    let a = a.max(1e-3);
    [
        ((c[0] - (1.0 - a) * matte[0]) / a).clamp(0.0, 1.0),
        ((c[1] - (1.0 - a) * matte[1]) / a).clamp(0.0, 1.0),
        ((c[2] - (1.0 - a) * matte[2]) / a).clamp(0.0, 1.0),
    ]
}

/// The four things the emitter needs to know about transparency, per face.
///
/// The three vectors are indexed by face id (the planar map's labels) and are all as long
/// as the face count.
pub(crate) struct FaceAlpha {
    /// The source put nothing here: the face is a hole punched out of what is above it.
    pub(crate) clear: Vec<bool>,
    /// One opacity for the whole face, or 1.0 where the alpha is not flat enough to claim.
    pub(crate) opacity: Vec<f32>,
    /// The colour the image was composited onto before the tracer saw it, sRGB in
    /// `[0, 1]`; white when the input had no transparency.
    pub(crate) matte: [f32; 3],
    /// A face whose alpha fades linearly, as the gradient an editor would have drawn.
    pub(crate) alpha_ramps: Vec<Option<AlphaRamp>>,
}

/// What the source's alpha says about each face: clear, translucent at one opacity, fading
/// across it, or opaque. Upstream works on the image matted opaque, because unmixing a
/// boundary needs two opaque colours; this is where the transparency comes back.
///
/// Two measurements, and they are deliberately different. *Clear* is the mean over
/// every pixel of the face — the question is only "did the source put anything here",
/// and a face that is 3% ink at its anti-aliased rim is still nothing. *Translucent*
/// is measured on interior pixels alone, because the rim of an opaque shape is partial
/// alpha for a geometric reason, not a painterly one: taking the mean there would file
/// every small opaque mark, and every thin stroke, as half-transparent. A face with no
/// interior — a hairline, a one-pixel sliver — is therefore never thinned.
///
/// The rules, with "interior" meaning a pixel whose four neighbours carry the same label
/// (the image border excluded), and `ā` a mean alpha:
///
/// * **clear** when `ā` over all its pixels is under 0.05, or when it has at least 24
///   interior pixels and their `ā` is under 0.01 (a small hole whose rim lifts the full
///   mean; see the comment at that step);
/// * **opacity** is the interior `ā` when there are at least 24 interior pixels, `ā` lies in
///   `[0.05, 0.98]` and their standard deviation is at most 0.02; otherwise 1.0, meaning
///   "no single opacity to claim";
/// * an **alpha ramp** ([`fit_alpha_ramp`]) is tried only under the cutout, and only for a
///   face that is neither clear nor already given an opacity.
///
/// # Passes
///
/// 1. **Statistics** ([`face_stats`]), one pass over the rows' runs: per face the pixel
///    count and alpha sum over all its pixels, and the count, sum and sum of squares over
///    its interior pixels, plus whether any interior alpha differs from exactly 1. Every f64
///    sum receives the same terms in the same order as the per-pixel loop it replaced, so
///    each is the same double as before.
/// 2. **Verdicts**: clear and opacity per face, from those sums alone.
/// 3. **Ramps**, under the cutout only. A face is fitted only when [`ramp_candidate`] says
///    the fit can return anything; the others are provably `None` and skipped. The
///    candidates' interior pixels are gathered in one pass ([`interior_pixels`]) and each
///    is fitted from its own list.
///
/// # Cost
///
/// `O(w · h)` for pass 1, plus `O(w · h)` once for pass 3 when at least one face is a
/// candidate, plus each candidate's own pixel count for its fit. Until 2026-09-30 every
/// non-clear, non-wash face scanned the whole image for its fit, `O(faces · w · h)`, and
/// the image was composited over white for it whether or not any fit ran: 88.5 ms of Fast
/// mode's 333 ms at 2048 px on the three transparent test images (`bigalpha`), 60.7 ms of it
/// in those scans. The output is unchanged bit for bit; [`ramp_candidate`] and
/// [`fit_alpha_ramp`] carry the two halves of that argument.
///
/// Inputs: `img` is the matted, opaque image the trace saw ([`AlphaSource::flat`]),
/// `traced_labels` the `w · h` label map (exactly `w · h` long), and `face_color` is only
/// read for the face count.
/// Without an `alpha_src` (an opaque input) every face comes back opaque and not clear,
/// with a white matte and no ramps. `INKVEC_ALPHADBG` prints the per-face numbers.
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
    // Pass 1 (see `face_stats`); without a source alpha every sum stays zero.
    debug_assert!(traced_labels.len() == w * h, "the label map is the image's");
    let FaceStats {
        a_sum,
        a_n,
        in_sum,
        in_sq,
        in_n,
        in_faded,
    } = match alpha_src {
        Some(src) => face_stats(traced_labels, &src.alpha, w, h, n_faces),
        None => face_stats(&[], &[], 0, 0, n_faces),
    };
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
    // A face whose opacity fades across it gets a gradient instead of one number, only
    // where the cutout is carrying transparency out at all. The ramp has one colour: a
    // colour ramp and an alpha ramp in one face is a fill model this does not have, so a
    // face whose alpha is not a clean linear fade is left baked (see below for why the
    // face's fitted fill is not consulted).
    //
    // Deliberately not restricted to flat-filled faces. A fade over a white matte *looks*
    // like a colour ramp towards white — the ramp case here fits `#ca774d -> #fbf3ef` — and
    // the alpha channel is the evidence that says which of the two it is. Where the
    // source's alpha fades linearly across the face, that is the explanation, and it is the
    // one an editor can work with.
    //
    // Which faces are fitted, and how: `alpha_ramps`.
    let alpha_ramps = match alpha_src {
        Some(src) if args.cutout => {
            let facts = RampFacts {
                opacity: &opacity,
                clear: &clear,
                in_n: &in_n,
                in_faded: &in_faded,
            };
            alpha_ramps(img, src, &facts, traced_labels, w, h)
        }
        _ => vec![None; n_faces],
    };
    let dump = inkvec_core::env::flag("INKVEC_ALPHADBG");
    for f in (0..n_faces).filter(|&f| dump && a_n[f] > 0) {
        diag::debug(dump, || {
            format!(
                "  face {f}: {} px, mean alpha {:.4}, interior {} at {:.4}, clear {}, opacity {:.3}",
                a_n[f],
                a_sum[f] / a_n[f] as f64,
                in_n[f],
                if in_n[f] > 0 { in_sum[f] / in_n[f] as f64 } else { f64::NAN },
                clear[f],
                opacity[f]
            )
        });
    }
    FaceAlpha {
        clear,
        opacity,
        matte,
        alpha_ramps,
    }
}

/// Per face, what [`face_alpha`]'s first pass measures: all indexed by face id.
struct FaceStats {
    /// Sum of the source alpha over every pixel of the face.
    a_sum: Vec<f64>,
    /// Pixel count of the face.
    a_n: Vec<usize>,
    /// Sum of the alpha over the face's interior pixels.
    in_sum: Vec<f64>,
    /// Sum of the squared alpha over the interior pixels.
    in_sq: Vec<f64>,
    /// Interior pixel count.
    in_n: Vec<usize>,
    /// Whether any interior pixel's alpha differs from exactly 1: the one fact the ramp
    /// skip ([`ramp_candidate`]) needs that the sums do not carry exactly.
    in_faded: Vec<bool>,
}

/// [`face_alpha`]'s first pass: every face's alpha statistics, over all its pixels and over
/// its interior ones, in one pass over the rows' runs.
///
/// "Interior" is the test the rest of the module uses: not on the image border, and the
/// pixel's own label equal to its four neighbours'. `labels` is `w · h` row-major (the
/// caller's label map is always the image's); `alpha` is the source alpha, row-major,
/// with a pixel past its end read as 1 (opaque), as before. Labels at or past `n_faces`
/// are ignored.
///
/// # Method
///
/// Each row is cut into maximal runs of one label, and a run's pixels are added to its
/// face's sums with the sums held in local variables, stored back once per run. Inside a
/// run the left and right neighbours of every pixel but the two ends carry the run's label
/// by definition, so the interior test reduces to `x0 < x < x1 − 1` plus the pixels above
/// and below. A pixel whose alpha is exactly zero is not added at all.
///
/// # Why the sums are the same doubles as the per-pixel loop's
///
/// * **Order.** A face's pixels are visited in row-major order, as before: rows in order,
///   runs of a row left to right, pixels of a run left to right. Every accumulator belongs
///   to one face, so it receives the same terms in the same order; interleaving with other
///   faces' accumulators never mattered.
/// * **Interior.** For `x` in a maximal run `x0..x1` of label `l`, `x > 0 && row[x − 1] =
///   l` holds exactly when `x > x0` (at `x = x0` either `x0 = 0` or the pixel to the left
///   has another label), and `x + 1 < w && row[x + 1] = l` exactly when `x < x1 − 1`.
/// * **Zeros.** Adding `+0` or `−0` to a double `s` gives `s` back unless `s = −0`, and the
///   sums start at `+0` and only ever receive terms that are `≥ +0` or `±0` (alphas are
///   clamped to `[0, 1]`; `a·a ≥ +0`), so they are never `−0`. Skipping a zero alpha's `a`
///   and `a·a` therefore changes no sum. (A NaN alpha is not zero and is added, as
///   before.) Counts are integers and are added per run.
///
/// # Cost
///
/// `O(w · h)` label and alpha reads, but no per-pixel store into the per-face arrays: the
/// old loop's `sum[f] += a` made every pixel wait on the previous pixel's store to the same
/// slot (a store-to-load dependency of several cycles), and on a transparent image most
/// pixels are clear and now cost a compare. The old loop is kept as
/// `ramp_tests::face_stats_scan`, and the tests compare the two bit for bit.
///
/// Method from: He, Chao & Suzuki 2008, "A Run-Based Two-Scan Labeling Algorithm", IEEE
/// TIP 17(5) 749–756, <https://doi.org/10.1109/TIP.2008.919369>: region statistics
/// gathered per run instead of per pixel. The zero skip is not from the literature: it
/// rests on IEEE 754 signed-zero addition, and the literature on run coding does not need it.
fn face_stats(labels: &[u16], alpha: &[f32], w: usize, h: usize, n_faces: usize) -> FaceStats {
    let mut st = FaceStats {
        a_sum: vec![0.0; n_faces],
        a_n: vec![0; n_faces],
        in_sum: vec![0.0; n_faces],
        in_sq: vec![0.0; n_faces],
        in_n: vec![0; n_faces],
        in_faded: vec![false; n_faces],
    };
    let a_at = |i: usize| alpha.get(i).copied().unwrap_or(1.0);
    for y in 0..h {
        let row = &labels[y * w..(y + 1) * w];
        // The rows above and below, when both exist: only then can a pixel be interior.
        let around = (y > 0 && y + 1 < h).then(|| {
            (
                &labels[(y - 1) * w..y * w],
                &labels[(y + 1) * w..(y + 2) * w],
            )
        });
        let mut x = 0;
        while x < w {
            let l = row[x];
            let x0 = x;
            while x < w && row[x] == l {
                x += 1;
            }
            let f = l as usize;
            if f >= n_faces {
                continue;
            }
            let base = y * w;
            st.a_n[f] += x - x0;
            let mut s = st.a_sum[f];
            for i in base + x0..base + x {
                let a = a_at(i);
                if a != 0.0 {
                    s += a as f64;
                }
            }
            st.a_sum[f] = s;
            let Some((up, down)) = around else {
                continue;
            };
            // Interior candidates: strictly inside the run (see the doc comment).
            let (mut sum, mut sq, mut n, mut faded) = (st.in_sum[f], st.in_sq[f], 0, false);
            for xx in x0 + 1..x.saturating_sub(1) {
                if up[xx] != l || down[xx] != l {
                    continue;
                }
                let a32 = a_at(base + xx);
                n += 1;
                faded |= a32 != 1.0;
                if a32 != 0.0 {
                    let a = a32 as f64;
                    sum += a;
                    sq += a * a;
                }
            }
            st.in_sum[f] = sum;
            st.in_sq[f] = sq;
            st.in_n[f] += n;
            st.in_faded[f] |= faded;
        }
    }
    st
}

/// What [`face_alpha`]'s first two passes know about each face that [`alpha_ramps`] needs,
/// all indexed by face id.
struct RampFacts<'a> {
    /// The face's one opacity, 1.0 when it has none.
    opacity: &'a [f32],
    /// Whether the face is a hole.
    clear: &'a [bool],
    /// The face's interior pixel count.
    in_n: &'a [usize],
    /// Whether any interior pixel's alpha differs from exactly 1.
    in_faded: &'a [bool],
}

/// [`face_alpha`]'s third pass: an [`AlphaRamp`] for every face whose alpha fades linearly,
/// `None` for the rest; one entry per face.
///
/// A face that is clear or already a wash has its answer; the test is "opacity not below 1,
/// or NaN", the negation of the `opacity < 1.0` it used to be, so a NaN opacity still goes to
/// the fit as it always did. Of the rest, a face is fitted only when [`ramp_candidate`] says
/// the fit can succeed — on the research sets 96% of these calls were provably `None` and
/// are skipped — and the candidates' interior pixels are gathered in one pass
/// ([`interior_pixels`]), each face then fitted from its own list ([`fit_alpha_ramp`]).
///
/// `img` is the matted image, `src` the source alpha and matte, `labels` the `w · h` label
/// map. `O(w · h)` once when any face is a candidate, nothing otherwise, plus each fit's own
/// pixels. The result is bit-identical to calling the whole-image fit on every face that
/// is neither clear nor a wash; see [`ramp_candidate`] and [`fit_alpha_ramp`].
fn alpha_ramps(
    img: &inkvec_trace::Rgba,
    src: &AlphaSource,
    facts: &RampFacts,
    labels: &[u16],
    w: usize,
    h: usize,
) -> Vec<Option<AlphaRamp>> {
    let fit_face: Vec<bool> = (0..facts.opacity.len())
        .map(|f| {
            (facts.opacity[f] >= 1.0 || facts.opacity[f].is_nan())
                && !facts.clear[f]
                && ramp_candidate(facts.in_n[f], facts.in_faded[f])
        })
        .collect();
    interior_pixels(labels, w, h, &fit_face, facts.in_n)
        .iter()
        .map(|px| {
            (!px.is_empty())
                .then(|| fit_alpha_ramp(px, &src.alpha, img, src.matte, w))
                .flatten()
        })
        .collect()
}

/// Whether [`fit_alpha_ramp`] can return anything but `None` for a face with `n` interior
/// pixels, `faded` saying whether any of them has an alpha other than exactly `1.0`.
///
/// `false` means the fit is **provably** `None`, so [`face_alpha`] does not gather the
/// face's pixels or call it. Two cases, both decided from pass-1 facts alone:
///
/// 1. `n < RAMP_MIN_INTERIOR` (64): the fit's own first test.
/// 2. Every interior alpha is exactly `1.0` (`!faded`). Then the fit's plane is exactly flat
///    and it returns `None`, by this argument about the floating-point operations it
///    performs (not only about the real numbers they approximate):
///    * each `a` is `1.0` in f64, so `a · x = x` and `a · y = y` exactly, and `Σa`, `Σa·x`,
///      `Σa·y` are accumulated from the same terms in the same order as `n`, `Σx`, `Σy`:
///      they are the same doubles. The right-hand side `r` of the normal equations `M b = r`
///      is therefore *bit for bit* `M`'s first column `(n, Σx, Σy)`.
///    * [`solve3x3`] computes `b_k = det(M_k) / det(M)`, `M_k` being `M` with column `k`
///      replaced by `r`. For `k = 1` two columns are equal; the cofactor formula
///      `a00(a11 a22 − a12 a21) − a01(a10 a22 − a12 a20) + a02(a10 a21 − a11 a20)` then
///      evaluates its first two products from identical operands, so they cancel to `0`,
///      and the third bracket is `fl(Σx·Σy) − fl(Σx·Σy) = 0`: `det(M_1) = 0` exactly. For
///      `k = 2` the first and third brackets are `P` and `−P` for the same rounded `P`
///      (round-to-nearest is symmetric, `fl(u − v) = −fl(v − u)`), so the first and third
///      products are `fl(n·P)` and `−fl(n·P)`, and the middle bracket is again an exact
///      `0`: `det(M_2) = 0` exactly.
///    * So `b1 = b2 = ±0`, the gradient `|g| = 0 < 1e-9`, and the fit returns `None` (or
///      earlier, when `|det M| < 1e-12`). Nothing overflows on the way: the largest product
///      in `det(M)` is about `n³·w²·h²`, below `2^200` for any raster that fits in memory.
///      Rust never contracts `a·b + c` into a fused multiply-add on its own, so the
///      operations are the ones written.
///
/// `n` and `faded` describe the same pixels the fit would use: [`face_alpha`]'s pass 1
/// applies the fit's interior test (own label and all four neighbours' labels equal, not on
/// the image border) to the same labels and reads the same alphas.
///
/// Measured on the research sets (screen, s512, big, bigalpha; 3,125 calls): 96% of the
/// calls fell in one of the two cases, and no call ever returned a ramp.
///
/// Not from the literature: a proof that a least-squares fit is degenerate, read off
/// statistics the pass already had, because the fit is ours (a plane through the alpha
/// channel). See also: He & Chao 2015, "A Very Fast Algorithm for Simultaneously Performing
/// Connected-Component Labeling and Euler Number Computing", IEEE TIP 24(9) 2725–2735,
/// <https://doi.org/10.1109/TIP.2015.2425540>, which likewise computes a region's features
/// during the labelling scan instead of re-scanning per region.
fn ramp_candidate(n: usize, faded: bool) -> bool {
    n >= RAMP_MIN_INTERIOR && faded
}

/// The interior pixels of every face flagged in `want`, as `(x, y)` in row-major order; an
/// empty list for every other face.
///
/// A pixel is interior when it is not on the image border and its own label and its four
/// neighbours' labels are all equal: the test [`face_alpha`]'s pass 1 counts with, and the
/// one [`fit_alpha_ramp`] expects of its input. `labels` is `w · h` row-major; `in_n[f]` is
/// face `f`'s interior count from pass 1, used only to size each list exactly. Labels at or
/// past `want.len()` are ignored.
///
/// One pass over the image (`O(w · h)`), and none at all when no face is wanted, which on
/// the research sets was most traces. This replaces one whole-image scan *per fitted face*:
/// every wanted face's pixels are bucketed in a single pass, as a labelling scan gathers its
/// per-component features. Rows are read as three slices (above, this, below), so each
/// neighbour read is a slice index rather than `i ± w` arithmetic. Border rows and columns
/// hold no interior pixel, so an image under 3 px on a side gives only empty lists.
///
/// Not from the literature: a bucket pass, the standard replacement for per-key scans.
/// See also: He, Chao & Suzuki 2008, "A Run-Based Two-Scan Labeling Algorithm", IEEE TIP
/// 17(5) 749–756, <https://doi.org/10.1109/TIP.2008.919369>, for gathering per-region data
/// in one scan.
fn interior_pixels(
    labels: &[u16],
    w: usize,
    h: usize,
    want: &[bool],
    in_n: &[usize],
) -> Vec<Vec<(u32, u32)>> {
    let mut out: Vec<Vec<(u32, u32)>> = want
        .iter()
        .zip(in_n)
        .map(|(&wanted, &n)| {
            if wanted {
                Vec::with_capacity(n)
            } else {
                Vec::new()
            }
        })
        .collect();
    if !want.iter().any(|&b| b) {
        return out;
    }
    for y in 1..h.saturating_sub(1) {
        let up = &labels[(y - 1) * w..y * w];
        let row = &labels[y * w..(y + 1) * w];
        let down = &labels[(y + 1) * w..(y + 2) * w];
        for x in 1..w.saturating_sub(1) {
            let l = row[x];
            if !want.get(l as usize).copied().unwrap_or(false) {
                continue;
            }
            if row[x - 1] == l && row[x + 1] == l && up[x] == l && down[x] == l {
                out[l as usize].push((x as u32, y as u32));
            }
        }
    }
    out
}

#[cfg(test)]
mod intake_tests;
#[cfg(test)]
mod ramp_tests;
