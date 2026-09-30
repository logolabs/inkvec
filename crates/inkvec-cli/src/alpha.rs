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
//!   every resampling step is done, [`alpha_source`] and [`cutout_args`]; everything after
//!   traces [`AlphaSource::flat`];
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

use inkvec_core::Point;
use inkvec_trace::{gradient, planar};

use crate::args::Args;
use crate::diag;

/// Uncertainty of a face's mean colour, in sRGB units, for the layer hypothesis.
///
/// The module (`inkvec_trace::alpha`) defaults to 1.5/255, the noise of its own tests. Our
/// face colours are medians over evidence pixels of a matted image and carry more than
/// that: on a synthetic stack of three translucent discs, 1.5 finds nothing and 3 finds
/// the layer with a
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
/// 3. **Tests.** The RMS residual of the plane must be at most `MAX_RESIDUAL` (0.06 in
///    opacity) and the fade must span at least `MIN_FADE` (0.15), or the face is a wash or
///    something that is not linear.
/// 4. **Colour.** Each interior pixel with `a ≥ 0.25` is un-matted,
///    `C = (c − (1 − a)·M) / a` with `c` its matted colour and `M` the matte, and the
///    estimates are averaged with weight `a²` (see the comment at that step for why).
///
/// Inputs: `labels`, `alpha` and `rgb` are `w · h` row-major arrays, the label map, the
/// source alpha in `[0, 1]`, and the image matted over `matte` (sRGB `[0, 1]`). `None` when
/// there are fewer than `MIN_INTERIOR` (64) interior pixels, when the normal equations are
/// singular (all interior pixels on one line), when the plane is flat (`|g| < 1e-9`), when
/// either test fails, or when no interior pixel is opaque enough to take the colour from.
/// The alphas are assumed finite, as a decoded image's are: every test here is written as
/// "reject when above/below", and a comparison with NaN is false, so a NaN alpha would slip
/// through all of them and come out as a NaN ramp rather than `None`.
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
/// "Constant" means every channel of every pixel in a block, alpha included, within
/// `1/512` of the block's top-left pixel: under half an 8-bit level, so for 8-bit input it
/// is exact equality and only float round-off is forgiven. A factor must divide both sides
/// exactly, and it is capped at 32 and so that at least 64 px remain on each side; a raster
/// under 128 px on either side is never unblocked.
///
/// Anti-aliased and resampled upscales are a different problem and not this one: their
/// blocks are not constant, they fail here, and `--sr` is what addresses them.
///
/// Called by the intake in `lib.rs` (unless `--no-unblock`), before any other resampling;
/// it only measures, and the caller downsamples by `k`.
///
/// # How: the gcd of the change positions, then the block test
///
/// The factors worth testing are found first, in one early-exiting scan
/// ([`change_gcd`]): `g = gcd(w, h, every x where a pixel differs sharply from its left
/// neighbour, every y where a row differs sharply from the row above)`. Then, from the
/// largest factor down, only the `k` that divide `g` get the block test
/// ([`blocks_constant`], the test this function always ran).
///
/// *Why the answer is the same.* The old loop returned the largest `k ≤ k_max` dividing
/// `w` and `h` whose blocks pass the test. Take any such `k` that passes. Two horizontally
/// adjacent pixels at `x − 1` and `x` with `k ∤ x` lie in one block, so each is within
/// `1/512` of the block's first pixel (the test's comparison is on a rounded f32
/// difference, but `2⁻⁹` is a float and rounding is monotone, so the exact difference is
/// below `2⁻⁹` too) and they differ by less than `2 · 2⁻⁹ = 1/256`. So every position where
/// neighbours differ by more than `1/256` is a multiple of `k`, and so are `w` and `h`:
/// `k` divides `g`. Skipping the `k ∤ g` therefore never skips a passing factor, and the
/// others get the old test itself, so the result is the old result for any input.
///
/// *Why it is fast.* On anything that is not an upscale, two edges at coprime positions
/// appear within the first rows of content and `g` falls to 1: the scan stops there and no
/// block test runs. The old loop ran the block test for every divisor of `w` and `h` from
/// 32 down, each scanning until its first non-constant block -- 7.1 ms at 2048 px, most of it
/// spent on the blank rows above the artwork, once per divisor. For 8-bit input the two
/// views coincide: distinct levels are at least `1/255 > 1/256` apart, so a "sharp change" is
/// any change, the divisors of `g` are exactly the factors whose blocks are constant, and
/// the block test only confirms.
///
/// Not from the literature: the gcd of change positions as a candidate filter for exact
/// block replication, because the published resampling detectors are statistical (they
/// estimate a periodic correlation of an interpolated signal) and would not reproduce this
/// function's exact answer. See also: A. C. Popescu, H. Farid, "Exposing Digital Forgeries by
/// Detecting Traces of Resampling", IEEE Trans. Signal Processing 53(2):758–767, 2005, DOI
/// 10.1109/TSP.2004.839932.
pub(crate) fn pixel_grid(img: &inkvec_trace::Rgba) -> Option<usize> {
    const MAX_FACTOR: usize = 32;
    let (w, h) = (img.width, img.height);
    // Below this there is nothing to gain and something to lose: a 2x undo of a small icon
    // leaves too few pixels for the boundary solve to work with.
    let smallest = 64;
    let k_max = MAX_FACTOR.min(w / smallest.min(w)).min(h / smallest.min(h));
    if k_max < 2 {
        return None;
    }
    let g = change_gcd(img);
    (2..=k_max)
        .rev()
        .find(|&k| divides(k, g) && blocks_constant(img, k))
}

/// Whether `k` divides `n` (`k ≥ 1`), without the remainder operator: wazero's arm64
/// compiler miscompiled `i32.rem_u` in a hot loop last round (the Go binding runs this
/// crate as WebAssembly), so new code here tests divisibility by multiplying back.
fn divides(k: usize, n: usize) -> bool {
    (n / k) * k == n
}

/// The greatest common divisor, by Stein's binary algorithm (shifts and subtraction only,
/// for the same reason as [`divides`]). `gcd(0, n) = n`.
fn gcd(mut a: usize, mut b: usize) -> usize {
    if a == 0 || b == 0 {
        return a | b;
    }
    let shift = (a | b).trailing_zeros();
    a >>= a.trailing_zeros();
    loop {
        b >>= b.trailing_zeros();
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        b -= a;
        if b == 0 {
            return a << shift;
        }
    }
}

/// `gcd(w, h, X, Y)` for the `w × h` image, where `X` is every column `x ≥ 1` at which some
/// row's pixel differs from its left neighbour by more than `1/256` in some channel, and
/// `Y` every row `y ≥ 1` in which some pixel differs that much from the one above. The
/// scan runs row by row and stops as soon as the gcd reaches 1. NaN differences count as
/// no change (a NaN pixel fails the block test anyway). O(pixels read); on typical art the
/// read stops a few rows into the content.
///
/// Blank rows are the common case before the content starts (a logo on a white page), so a
/// row is first compared with the row above, and with itself shifted by one pixel, bit for
/// bit ([`same_bits`], which vectorises); only a row that differs is examined pixel by
/// pixel. Identical bits mean every difference is 0 (or NaN), which is never sharp, so the
/// shortcut cannot hide a change.
fn change_gcd(img: &inkvec_trace::Rgba) -> usize {
    /// Neighbours in one block differ by less than this (see [`pixel_grid`]).
    const SHARP: f32 = 1.0 / 256.0;
    let (w, h) = (img.width, img.height);
    let sharp = |a: &[f32], b: &[f32]| a.iter().zip(b).any(|(p, q)| (p - q).abs() > SHARP);
    let mut g = gcd(w, h);
    for y in 0..h {
        if g < 2 {
            break;
        }
        let row = &img.data[y * w * 4..(y + 1) * w * 4];
        if y > 0 {
            let above = &img.data[(y - 1) * w * 4..y * w * 4];
            if !same_bits(above, row) && sharp(above, row) {
                g = gcd(g, y);
            }
        }
        // Each pixel against its left neighbour: the row against itself one pixel over.
        if same_bits(&row[4..], &row[..row.len() - 4]) {
            continue;
        }
        for x in 1..w {
            if g < 2 {
                break;
            }
            if sharp(&row[(x - 1) * 4..x * 4], &row[x * 4..x * 4 + 4]) {
                g = gcd(g, x);
            }
        }
    }
    g
}

/// Whether two equally long float slices hold the same bits. Compared 64 floats at a time
/// with an OR of XORs, a loop without an early exit that the compiler vectorises; the early
/// exit is per block.
fn same_bits(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len()
        && a.chunks(64).zip(b.chunks(64)).all(|(x, y)| {
            x.iter()
                .zip(y)
                .fold(0u32, |acc, (p, q)| acc | (p.to_bits() ^ q.to_bits()))
                == 0
        })
}

/// The block test: every channel of every pixel in every `k × k` block within `1/512` of
/// the block's top-left pixel (so for 8-bit input, exactly equal). `k` must divide both
/// sides. Stops at the first block that fails; a full scan when every block passes.
fn blocks_constant(img: &inkvec_trace::Rgba, k: usize) -> bool {
    let (w, h) = (img.width, img.height);
    let px = |x: usize, y: usize| -> &[f32] { &img.data[(y * w + x) * 4..(y * w + x) * 4 + 4] };
    (0..h / k).all(|by| {
        (0..w / k).all(|bx| {
            let first = px(bx * k, by * k);
            (0..k).all(|dy| {
                (0..k).all(|dx| {
                    let p = px(bx * k + dx, by * k + dy);
                    (0..4).all(|c| (p[c] - first[c]).abs() < 1.0 / 512.0)
                })
            })
        })
    })
}

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
pub(crate) fn alpha_source(
    img: &inkvec_trace::Rgba,
    quiet: bool,
    cutout: bool,
    native: bool,
) -> Option<AlphaSource> {
    if !has_transparency(img) {
        return None;
    }
    if native {
        // Nothing is chosen and nothing is lost: over white is only how the colour is
        // written down, and the alpha travels beside it into every stage that unmixes.
        // The output carries the transparency out, as the cutout does.
        let (flat, alpha) = flatten_over(img, [1.0, 1.0, 1.0]);
        diag::stage(quiet, || {
            let clear = alpha.iter().filter(|&&a| a < 0.05).count();
            format!(
                "  alpha         native, {:.0}% of the image transparent",
                100.0 * clear as f64 / alpha.len().max(1) as f64
            )
        });
        return Some(AlphaSource {
            flat,
            alpha,
            matte: [1.0, 1.0, 1.0],
            cutout: true,
        });
    }
    let (chosen, _, lost_to_white) = choose_matte(img);
    let swallowed = !cutout && lost_to_white > LOST_TO_WHITE;
    let cutout = cutout || swallowed;
    let matte = if cutout { chosen } else { [1.0, 1.0, 1.0] };
    let (flat, alpha) = flatten_over(img, matte);
    diag::stage(quiet, || {
        let clear = alpha.iter().filter(|&&a| a < 0.05).count();
        format!(
            "  alpha         {} matte, {:.0}% of the image transparent",
            inkvec_trace::color::to_hex(matte),
            100.0 * clear as f64 / alpha.len().max(1) as f64
        )
    });
    diag::stage(quiet || !swallowed, || {
        format!(
            "  cutout        {:.0}% of the outline is white and would vanish into a white \
matte; carrying the transparency out as --cutout does",
            100.0 * lost_to_white
        )
    });
    Some(AlphaSource {
        flat,
        alpha,
        matte,
        cutout,
    })
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
            .chunks_exact_mut(4)
            .zip(a.iter_mut())
            .zip(src.chunks_exact(4))
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
fn flatten_pixel(p: &mut [f32], matte: [f32; 3]) -> f32 {
    let a = p[3].clamp(0.0, 1.0);
    for c in 0..3 {
        p[c] = p[c] * a + matte[c] * (1.0 - a);
    }
    p[3] = 1.0;
    a
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
    let translucent = |p: &[f32]| p[3] < 0.999;
    if img.data.len() / 4 >= INTAKE_PARALLEL_MIN {
        img.data
            .par_chunks_exact(4)
            .with_min_len(FLATTEN_CHUNK)
            .any(translucent)
    } else {
        img.data.chunks_exact(4).any(translucent)
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

/// Translucent layers: one shape at one opacity, seen against several grounds.
///
/// `inkvec_trace::alpha::decompose_with` recovers these from the face partition alone — no
/// alpha channel needed, because the evidence is that the differences between a layer's
/// faces are parallel to the differences between the grounds beneath them, scaled by
/// `1 - a`. A face under a layer of colour `L` and opacity `a` reads
/// `c_f = a·L + (1 − a)·G_f`, with `G_f` the ground it covers, so two covered faces differ
/// by `c_f − c_g = (1 − a)·(G_f − G_g)`. It is what turns three overlapping circles at 85%
/// into three circles instead of five flat patches.
///
/// It is only accepted when it explains the faces to well inside the uncertainty of a
/// face's own colour ([`LAYER_SIGMA_SRGB`]): a missed layer costs parameters, an invented
/// one is a visible error, and the module's own documentation is emphatic about which way
/// to lean.
///
/// Off by default (`--layers`), and the reason is compactness rather than correctness. The
/// layer reproduces the image exactly — the faces beneath it are repainted with the ground
/// and the layer is composited over them — but it only pays when the ground pieces it
/// reunites merge back into fewer shapes. The pipeline does that merge and then keeps the
/// layered document only when it has fewer shapes and no more bytes than the flat one. On
/// real art it is rare besides: two of forty icons in the census.
///
/// Inputs, all indexed by face id: `face_color` (palette index per face), `fills` (a flat
/// fill's colour is used as the face colour, anything else falls back to the palette ink),
/// and `traced_labels` (the label map, for each face's pixel area). Adjacency comes from
/// the map's edges. Both compositing spaces, sRGB-encoded and linear light, are tried and
/// the one that finds more layers is kept (linear on a tie, as `max_by_key` keeps the last
/// maximum). Returns `None` when `--layers` is off or no layer was found; unless `quiet`,
/// each found layer is described on stderr.
pub(crate) fn recover_layers(
    args: &Args,
    map: &planar::PlanarMap,
    face_color: &[usize],
    fills: &[gradient::FillFit],
    pal: &inkvec_trace::color::Palette,
    traced_labels: &[u16],
) -> Option<inkvec_trace::alpha::AlphaAnalysis> {
    if args.layers {
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
        // constant, so try both. "Better" is judged by how many layers each finds; on a tie
        // `max_by_key` keeps the last, the linear-light fit.
        let sigma_srgb = LAYER_SIGMA_SRGB;
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
            let quiet = args.quiet || an.layers.is_empty();
            diag::stage(quiet, || {
                format!(
                    "  layers        {} translucent layer(s) over a continuous ground ({:?})",
                    an.layers.len(),
                    space
                )
            });
            for l in &an.layers {
                diag::stage(quiet, || {
                    format!(
                        "                {} at {:.3} across {} faces, residual {:.5}",
                        inkvec_trace::color::to_hex(l.color),
                        l.alpha,
                        l.faces.len(),
                        l.residual
                    )
                });
            }
        }
        best.map(|(_, an)| an).filter(|an| !an.layers.is_empty())
    } else {
        None
    }
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
/// Inputs: `img` is the matted, opaque image the trace saw ([`AlphaSource::flat`]),
/// `traced_labels` the `w · h` label map, and `face_color` is only read for the face count.
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
    // A face whose opacity fades across it gets a gradient instead of one number, only
    // where the cutout is carrying transparency out at all. The ramp has one colour: a
    // colour ramp and an alpha ramp in one face is a fill model this does not have, so a
    // face whose alpha is not a clean linear fade is left baked (see below for why the
    // face's fitted fill is not consulted).
    //
    // `img` is already opaque (matted over `matte`), so compositing it over white changes
    // no value and only turns it into RGB triples; the ramp fit un-mattes against `matte`.
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

#[cfg(test)]
mod intake_tests {
    use super::*;
    use inkvec_trace::Rgba;

    /// Deterministic pseudo-random numbers (a 64-bit LCG).
    fn lcg(s: &mut u64) -> u64 {
        *s = s
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *s >> 33
    }

    /// `pixel_grid` as it was: every divisor of both sides from the largest down, each with
    /// the block test.
    fn old_pixel_grid(img: &Rgba) -> Option<usize> {
        const MAX_FACTOR: usize = 32;
        let (w, h) = (img.width, img.height);
        let smallest = 64;
        let mut k = MAX_FACTOR.min(w / smallest.min(w)).min(h / smallest.min(h));
        while k >= 2 {
            if w % k == 0 && h % k == 0 && blocks_constant(img, k) {
                return Some(k);
            }
            k -= 1;
        }
        None
    }

    /// A `w × h` 8-bit image of flat rectangles on a ground (edges at arbitrary positions),
    /// optionally with a few noisy pixels.
    fn art(w: usize, h: usize, seed: u64, noise: usize) -> Rgba {
        let mut s = seed;
        let mut data = vec![1.0f32; w * h * 4];
        for _ in 0..6 {
            let (x0, y0) = ((lcg(&mut s) as usize) % w, (lcg(&mut s) as usize) % h);
            let (x1, y1) = (
                (x0 + 1 + (lcg(&mut s) as usize) % w).min(w),
                (y0 + 1 + (lcg(&mut s) as usize) % h).min(h),
            );
            let c = [0, 1, 2, 3].map(|_| (lcg(&mut s) % 256) as f32 / 255.0);
            for y in y0..y1 {
                for x in x0..x1 {
                    data[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&c);
                }
            }
        }
        for _ in 0..noise {
            let p = (lcg(&mut s) as usize) % (w * h);
            data[p * 4] = (lcg(&mut s) % 256) as f32 / 255.0;
        }
        Rgba {
            width: w,
            height: h,
            data,
        }
    }

    /// Nearest-neighbour upscale by `k`.
    fn upscale(img: &Rgba, k: usize) -> Rgba {
        let (w, h) = (img.width * k, img.height * k);
        let mut data = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let p = (y / k) * img.width + x / k;
                data.extend_from_slice(&img.data[p * 4..p * 4 + 4]);
            }
        }
        Rgba {
            width: w,
            height: h,
            data,
        }
    }

    /// `flatten_over` as it was: a serial push loop.
    fn old_flatten_over(img: &Rgba, matte: [f32; 3]) -> (Rgba, Vec<f32>) {
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
            Rgba {
                width: img.width,
                height: img.height,
                data,
            },
            alpha,
        )
    }

    /// Random straight RGBA with odd values mixed in: NaN, −0, alpha outside [0, 1].
    fn random_rgba(w: usize, h: usize, seed: u64, extra: usize) -> Rgba {
        let mut s = seed;
        let mut data: Vec<f32> = (0..w * h * 4 + extra)
            .map(|i| match lcg(&mut s) % 6 {
                0 if i % 4 == 3 => 1.0,
                1 => 0.0,
                2 => (lcg(&mut s) % 256) as f32 / 255.0,
                _ => (lcg(&mut s) % 100_000) as f32 * 1.2e-5 - 0.05,
            })
            .collect();
        for (i, v) in [f32::NAN, -0.0, 1.5, -0.25].into_iter().enumerate() {
            if let Some(d) = data.get_mut(i * 13 + 3) {
                *d = v;
            }
        }
        Rgba {
            width: w,
            height: h,
            data,
        }
    }

    fn bits(v: &[f32]) -> Vec<u32> {
        v.iter().map(|f| f.to_bits()).collect()
    }

    /// Serial and parallel sizes, every matte the ladder uses: the same image and alphas,
    /// bit for bit, as the serial push loop.
    #[test]
    fn the_parallel_flatten_is_the_serial_one() {
        for (w, h) in [(0, 0), (1, 1), (7, 1), (1, 7), (50, 40), (300, 260)] {
            let img = random_rgba(w, h, (w * 31 + h) as u64, 0);
            for matte in [[1.0, 1.0, 1.0], [0.0, 0.0, 0.0], [1.0, 0.5, 0.0]] {
                let (a, aa) = flatten_over(&img, matte);
                let (b, ba) = old_flatten_over(&img, matte);
                assert_eq!(bits(&a.data), bits(&b.data), "{w}x{h}");
                assert_eq!(bits(&aa), bits(&ba), "{w}x{h}");
            }
        }
    }

    /// The parallel scan against the old strided scan, on opaque images with one translucent
    /// pixel anywhere (first, last, middle, in a trailing partial pixel), on NaN alpha, and
    /// on buffers whose length is not a multiple of four.
    #[test]
    fn the_transparency_scan_is_the_old_one() {
        let old = |img: &Rgba| img.data.iter().skip(3).step_by(4).any(|&a| a < 0.999);
        for (w, h) in [(0usize, 0usize), (1, 1), (9, 3), (300, 260)] {
            for extra in [0usize, 1, 3] {
                let n = w * h;
                let mut img = Rgba {
                    width: w,
                    height: h,
                    data: vec![1.0; n * 4 + extra],
                };
                assert_eq!(has_transparency(&img), old(&img));
                for at in [0, n / 2, n.saturating_sub(1), n] {
                    for v in [0.5, 0.9989, f32::NAN] {
                        let mut t = img.clone();
                        if let Some(d) = t.data.get_mut(at * 4 + 3) {
                            *d = v;
                        }
                        assert_eq!(has_transparency(&t), old(&t), "{w}x{h}+{extra} at {at}");
                    }
                }
                img.data.iter_mut().for_each(|v| *v = 0.2);
                assert_eq!(has_transparency(&img), old(&img));
            }
        }
    }

    #[test]
    fn gcd_is_euclid() {
        let euclid = |mut a: usize, mut b: usize| {
            while b != 0 {
                (a, b) = (b, a % b);
            }
            a
        };
        for a in 0..200 {
            for b in 0..200 {
                assert_eq!(gcd(a, b), euclid(a, b), "{a} {b}");
            }
        }
        assert!(divides(4, 2048) && !divides(3, 2048) && divides(7, 0));
    }

    /// The gcd filter against the old full search: plain art, upscales by 2 to 8 (the
    /// factor must come back the same), upscales with one pixel broken early or late,
    /// odd sizes, a flat image, blocks that vary inside the 1/512 tolerance (not 8-bit, so
    /// the block test has to decide), and a NaN.
    #[test]
    fn the_gcd_filter_finds_what_the_full_search_found() {
        let mut cases: Vec<(String, Rgba)> = Vec::new();
        for (seed, (w, h)) in [(1u64, (160usize, 128usize)), (2, (96, 80)), (3, (64, 64))] {
            let a = art(w, h, seed, 0);
            cases.push((format!("art {w}x{h}"), a.clone()));
            for k in [2, 3, 4, 5, 8] {
                let up = upscale(&a, k);
                cases.push((format!("art {w}x{h} x{k}"), up.clone()));
                for at in [7usize, up.width * up.height - 3] {
                    let mut b = up.clone();
                    b.data[at * 4 + 1] = if b.data[at * 4 + 1] > 0.5 { 0.0 } else { 1.0 };
                    cases.push((format!("art {w}x{h} x{k} broken at {at}"), b));
                }
            }
            cases.push((format!("noisy {w}x{h}"), upscale(&art(w, h, seed, 40), 2)));
        }
        let flat = Rgba {
            width: 256,
            height: 192,
            data: vec![0.25; 256 * 192 * 4],
        };
        cases.push(("flat".into(), flat));
        let mut odd = upscale(&art(97, 64, 9, 0), 2);
        odd.width -= 1;
        odd.data.truncate(odd.width * odd.height * 4);
        cases.push(("odd width".into(), odd));
        // Within-block wobble below the tolerance: the old test passes, and the new one must
        // not be fooled by neighbours that differ by up to 2/512.
        let mut wobble = upscale(&art(80, 64, 4, 0), 4);
        let mut s = 99u64;
        for v in wobble.data.iter_mut() {
            *v += ((lcg(&mut s) % 7) as f32 - 3.0) * (0.45 / 512.0) / 3.0;
        }
        assert_eq!(
            old_pixel_grid(&wobble),
            Some(4),
            "the old test forgives the wobble"
        );
        cases.push(("wobble".into(), wobble.clone()));
        let mut drift = wobble;
        for (i, v) in drift.data.iter_mut().enumerate() {
            if (i / 4) % drift.width % 4 == 3 {
                *v += 1.5 / 512.0;
            }
        }
        cases.push(("drift".into(), drift));
        let mut nan = upscale(&art(64, 64, 5, 0), 2);
        nan.data[1000] = f32::NAN;
        cases.push(("nan".into(), nan));
        for (name, img) in &cases {
            assert_eq!(pixel_grid(img), old_pixel_grid(img), "{name}");
        }
        // And the factors really are recovered.
        assert_eq!(pixel_grid(&upscale(&art(160, 128, 1, 0), 4)), Some(4));
    }
}
