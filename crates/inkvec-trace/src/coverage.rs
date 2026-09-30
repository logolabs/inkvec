//! Coverage recovery (DESIGN.md S2).
//!
//! An anti-aliased pixel is not a blurry approximation of the shape — it is a
//! *measurement* of how much of that pixel the shape covers. Reading it as a measurement
//! rather than as noise to be thresholded away is the single largest lever in this
//! project, and it is the failure `docs/M0-BASELINE.md` §3 quantifies: on `thin_features`
//! VTracer recovers 2 elements of 11, and no parameter setting recovers more, because the
//! information is destroyed before any tunable stage runs.
//!
//! For a boundary between a foreground colour `F` and a background colour `B`, an
//! observed pixel is `P = a*F + (1-a)*B`. Projecting onto the `F-B` axis inverts that:
//!
//! ```text
//!     a = dot(P - B, F - B) / |F - B|^2
//! ```
//!
//! which is a least-squares estimate over the colour channels, so it uses all three
//! rather than a single luminance projection.
//!
//! **Uncertainty comes out of the same equation, and this is the part that matters
//! downstream.** Two error-propagation steps:
//!
//! 1. Pixel noise of standard deviation `sigma_pixel` gives a coverage uncertainty
//!    `sigma_a = sigma_pixel / |F - B|`. A faint boundary — small `|F - B|` — is
//!    measured badly, and says so.
//! 2. The boundary is the level set `a = 0.5`. For an implicit curve, positional
//!    uncertainty is `sigma_a / |grad a|`. A crisp edge has a steep coverage gradient and
//!    localizes to a fraction of a pixel; a soft or blurred edge does not.
//!
//! So `sigma = sigma_pixel / (|F - B| * |grad a|)`, in pixels. That single expression is
//! what feeds the `tau * sigma` admissibility envelope in `inkvec-fit`, and it is why
//! adaptive simplification needs no separate heuristic: the places we are allowed to
//! simplify hard are exactly the places we measured badly.
//!
//! # Where this sits
//!
//! This is the intake stage. It holds the raster type the whole crate works on ([`Rgba`],
//! straight sRGB `0..1`), the two-colour coverage field ([`bilevel_coverage`], the input to
//! the monochrome trace in the crate root and `inkvec-cli`'s pipeline, and to
//! `centerline`), and the image-wide measurements
//! every later stage is tuned by:
//!
//! * [`estimate_noise`], the pixel noise `sigma_pixel` in sRGB units, which the palette's
//!   same-ink test and the fitter's tolerance divide by;
//! * [`intake_scale`], how many pixels an edge takes to cross;
//! * [`ringing_score`], whether compression ringing surrounds the edges;
//! * [`oversample_factor`], how many times more pixels the raster has than its drawing needs.
//!
//! Resampling ([`downsample_to`], [`box_downsample_rgba8`]) lives in the `resample`
//! submodule and is re-exported here. Callers are the crate root (decode and intake),
//! `color`, `contour`, `centerline`, `regularize`, and `inkvec-cli`'s intake.
//!
//! Unless a function says otherwise, pixel `(x, y)` is sampled at its centre, which sits at
//! integer coordinates `(x, y)`.

use inkvec_core::Point;

mod resample;
pub use resample::{box_downsample_rgba8, downsample_to};

/// Systematic positional error of level-set extraction, in pixels. See
/// [`CoverageField::sigma_model`].
///
/// Worth knowing how much of the pipeline's behaviour this one number decides. It is
/// combined in quadrature with the statistical term in `planar.rs`, and on this corpus
/// it dominates: 79% of the gate set's 86,060 boundary points come out between 0.050
/// and 0.060, so for most of a traced image the fitter's tolerance *is* this constant
/// rather than anything measured from the image (see the flat-sigma measurement in `contour.rs`,
/// which prices the remaining variation at about one percent).
///
/// That makes it a large dial, but not an independent one. Because chi2 weights are
/// `1/sigma²`, scaling every sigma by `k` is exactly `lambda -> k²·lambda`, and the
/// measurement agrees: raising this to 0.10 moves the gate to dE00 +39.00% / ratio
/// -10.28%, and `--lambda-scale 4.0` — the algebraically equivalent change — to +44.34%
/// / -11.68%, the same frontier. So tune fidelity against compactness with
/// `--lambda-scale`, which is documented as a tuning knob; this is documented as a
/// property of the extraction method, and changing it silently retunes the whole
/// pipeline while claiming to state a physical fact about it.
pub const DEFAULT_SIGMA_MODEL: f64 = 0.05;

/// A scalar coverage field sampled at pixel centres.
#[derive(Debug, Clone)]
pub struct CoverageField {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Row-major coverage samples, one per pixel, in `[0, 1]`.
    pub data: Vec<f32>,
    /// Standard deviation of the coverage estimate, propagated from pixel noise.
    pub sigma_alpha: f64,
    /// Irreducible positional error of level-set extraction itself, in pixels.
    ///
    /// Propagating *pixel noise* alone gives an absurdly optimistic answer on clean
    /// synthetic input: with no noise, sigma tends to zero and the fit is told it knows
    /// the boundary to a thousandth of a pixel. It does not. Locating a boundary as the
    /// 0.5 level set of a bilinearly interpolated coverage field carries a systematic
    /// error of its own — the shape need not be exactly representable, and the
    /// interpolant is not the true reconstruction filter.
    ///
    /// So the total is `sqrt(noise^2 + model^2)`: a resolution limit that no amount of
    /// clean input removes. Measured on analytic circles (see the `inkvec-trace` tests),
    /// level-set extraction lands within roughly 0.05px, which is the default.
    pub sigma_model: f64,
    /// The ink and paper colours the coverage was measured against, in sRGB.
    ///
    /// `bilevel_coverage` estimates both to build its colour axis and then threw them
    /// away, which left every caller that wanted to *paint* the result re-deriving them
    /// by a different route. A stroke needs its own colour, and it should be the one the
    /// coverage was defined by or the two disagree at every anti-aliased pixel.
    pub fg: [f32; 3],
    /// The background colour the coverage was measured against, in sRGB. See [`fg`](Self::fg).
    pub bg: [f32; 3],
    /// How confidently the foreground/background colours were identified, in `[0, 1]`.
    ///
    /// Low when the image contains no fully-covered pixels — that is, when every feature
    /// is narrower than a pixel. In that regime the colour axis is genuinely
    /// **unidentifiable**: a 0.3px black stroke and a 1px grey stroke produce the same
    /// pixels, and no amount of processing separates them. The honest response is to
    /// widen `sigma_alpha` so downstream stages simplify rather than to guess a width and
    /// state it confidently.
    pub saturation: f64,
}

impl CoverageField {
    /// Coverage at pixel `(x, y)`.
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }

    /// Coverage at `(x, y)`, with out-of-range coordinates clamped to the field's edge.
    #[inline]
    pub fn get_clamped(&self, x: isize, y: isize) -> f32 {
        let xc = x.clamp(0, self.width as isize - 1) as usize;
        let yc = y.clamp(0, self.height as isize - 1) as usize;
        self.get(xc, yc)
    }

    /// Central-difference gradient magnitude, in coverage units per pixel.
    ///
    /// `|grad a| = sqrt(gx² + gy²)` with `gx = (a[x+1, y] − a[x−1, y]) / 2` and likewise for
    /// `gy`, evaluated at the pixel nearest `(x, y)` (no interpolation). Neighbours outside
    /// the field are clamped to the edge, so the gradient there is one-sided and halved.
    pub fn gradient_magnitude(&self, x: f64, y: f64) -> f64 {
        let (xi, yi) = (x.round() as isize, y.round() as isize);
        let gx = 0.5 * (self.get_clamped(xi + 1, yi) - self.get_clamped(xi - 1, yi)) as f64;
        let gy = 0.5 * (self.get_clamped(xi, yi + 1) - self.get_clamped(xi, yi - 1)) as f64;
        gx.hypot(gy)
    }

    /// Positional uncertainty, in pixels, of a boundary point at `p`.
    ///
    /// `sigma = sqrt((sigma_alpha / |grad a|)² + sigma_model²)`, clamped to `[0.001, 4]` px.
    /// The first term is noise propagated through the level set (see the module docs), the
    /// second the method's own resolution limit. A vanishing gradient — a plateau where the
    /// level set is genuinely unlocalizable — yields the 4 px ceiling rather than an
    /// infinity that would poison the fit.
    pub fn position_sigma(&self, p: Point) -> f64 {
        const MAX_SIGMA: f64 = 4.0;
        let g = self.gradient_magnitude(p.x, p.y);
        let noise = if g <= 1e-6 {
            MAX_SIGMA
        } else {
            self.sigma_alpha / g
        };
        // Noise and the method's own resolution limit add in quadrature.
        noise.hypot(self.sigma_model).clamp(1e-3, MAX_SIGMA)
    }
}

/// RGBA image with straight (unpremultiplied) channels in `[0, 1]`.
#[derive(Debug, Clone)]
pub struct Rgba {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Row-major, 4 floats per pixel.
    pub data: Vec<f32>,
}

impl Rgba {
    /// Straight RGBA colour at pixel `(x, y)`.
    #[inline]
    pub fn pixel(&self, x: usize, y: usize) -> [f32; 4] {
        let i = (y * self.width + x) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    /// Composite over a solid background, dropping alpha.
    pub fn composited(&self, bg: [f32; 3]) -> Vec<[f32; 3]> {
        (0..self.width * self.height)
            .map(|i| {
                let p = &self.data[i * 4..i * 4 + 4];
                let a = p[3];
                [
                    p[0] * a + bg[0] * (1.0 - a),
                    p[1] * a + bg[1] * (1.0 - a),
                    p[2] * a + bg[2] * (1.0 - a),
                ]
            })
            .collect()
    }
}

/// The kernel [`estimate_noise`] convolves with: the centre pixel against its four
/// 4-connected neighbours.
///
/// It is written down as coefficients, rather than only as arithmetic in the loop, so that
/// the noise gain below can be *computed* from it instead of transcribed beside it. See
/// [`estimate_noise`] for why that distinction earned itself a constant.
const LAPLACIAN_KERNEL: [f64; 5] = [4.0, -1.0, -1.0, -1.0, -1.0];

/// A median absolute deviation to a Gaussian sigma (`MAD = 0.6745 * sigma`); test-only now.
#[cfg(test)]
const MAD_TO_SIGMA: f64 = 0.6745;

/// The smallest noise this will report, in sRGB units.
///
/// A clean render has no noise to find and the estimate would be zero, but callers divide
/// by it: `color::extract_palette_mdl` tests `(nearest / sigma_noise)^2`. Half a quantisation
/// step is the smallest deviation an 8-bit file could even represent.
pub const NOISE_FLOOR: f64 = 0.5 / 255.0;

/// Estimate the pixel noise of one grey channel (row-major, `w x h`, sRGB `0..1`) from a
/// low quantile of the absolute Laplacian. Returns a Gaussian sigma in sRGB units, never
/// below [`NOISE_FLOOR`].
///
/// The Laplacian annihilates smooth content, so what survives in a flat region is noise.
/// A robust order statistic (rather than the mean) keeps genuine edges from inflating the
/// estimate — an edge is a large deviation, but a rare one. This used to be the median
/// absolute deviation (MAD); it now reads the 10th percentile, for the reason given inside
/// `estimate_noise_at`. With `L(x, y) = 4·g(x,y) − g(x−1,y) − g(x+1,y) − g(x,y−1) − g(x,y+1)`
/// over interior pixels:
///
/// ```text
///     sigma = max(Q_q(|L|) / z_q / sqrt(Σ k_i²), NOISE_FLOOR)
/// ```
///
/// `Q_q` is the `q = 0.10` quantile, `z_q` the same quantile of a unit half-normal
/// (`Z10`; `MAD_TO_SIGMA` plays this role for the median), and `k_i` the kernel
/// coefficients. The last divisor undoes the gain the kernel applies to independent
/// noise: for a linear filter that gain is the root of the sum of its squared
/// coefficients, so it is computed from `LAPLACIAN_KERNEL` rather than written down.
///
/// An image smaller than 3x3, or a buffer shorter than `w * h`, returns `1/255`.
///
/// **That computation is the fix for a real bug, and the reason it is not a literal.**
/// Until 2026-09-08 this divided by `sqrt(6)`, described in the comment as the 4-neighbour
/// Laplacian's gain. It is not: `sqrt(6)` is the gain of the *one-dimensional* second
/// difference `[1, -2, 1]`, whose squared coefficients sum to 6. The kernel actually
/// applied here sums to `4^2 + 1 + 1 + 1 + 1 = 20`, so every live estimate was too large by
/// `sqrt(20/6) = 1.83x` — measured at 1.832x against synthetic noise of known sigma, which
/// is what `noise_estimate_recovers_a_known_sigma` now checks on every run.
///
/// The error was invisible for as long as it was because [`NOISE_FLOOR`] binds across the
/// whole corpus: clean renders and JPEG-damaged files alike measure exactly `0.50/255`.
/// Only densely noisy input — sensor noise, grain, dithering — ever put the arithmetic in
/// play. A constant that is wrong, that the corpus cannot see, and that other tuned
/// constants have quietly absorbed is the worst kind to leave as a literal; deriving it
/// from the kernel means changing the kernel can no longer leave a stale gain behind.
pub fn estimate_noise(gray: &[f32], w: usize, h: usize) -> f64 {
    estimate_noise_at(gray, w, h, NOISE_QUANTILE, Z10)
}

/// [`estimate_noise`] at the `at` quantile (a fraction in `0..1`), `z` being that quantile
/// of the half-normal. The test reads the median it replaced here (it was
/// `INKVEC_NOISE_MEDIAN=1`). The quantile is read by index `floor(n·at)` into the sorted
/// `|L|` values, with no interpolation.
fn estimate_noise_at(gray: &[f32], w: usize, h: usize, at: f64, z: f64) -> f64 {
    if w < 3 || h < 3 || gray.len() < w * h {
        return 1.0 / 255.0;
    }
    let mut lap = Vec::with_capacity((w - 2) * (h - 2));
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let c = gray[y * w + x];
            let v = 4.0 * c
                - gray[y * w + x - 1]
                - gray[y * w + x + 1]
                - gray[(y - 1) * w + x]
                - gray[(y + 1) * w + x];
            lap.push(v.abs());
        }
    }
    // Read a LOW quantile, not the median, with the Gaussian factor for that quantile.
    //
    // The median is robust only while edges are rare: half the pixels are then flat, so the
    // middle of the distribution is noise and edges sit in a tail it ignores. That is a
    // property of the picture, not of the estimator. On a labyrinth of three-pixel strokes
    // the median |Laplacian| IS an edge response: 53 display levels of "noise" where the
    // trace's own flat interiors say 0.57. The palette's is-this-one-ink test and the fit
    // tolerance both divide by this, so the trace ran two orders of magnitude too loose and
    // strokes merged into blobs. The tenth percentile is still noise while a tenth of the
    // image is flat, against the median's half, and stays unbiased on Gaussian noise because
    // `Z10` is that quantile of the half-normal; the cost is variance, which the floor
    // absorbs. Measured over 246 icons at 128, 512 and 1024 px: every output byte-identical,
    // because on clean art both readings sit on the floor. JPEG and added grain improve.
    let k = ((lap.len() as f64) * at) as usize;
    let mad = kth_smallest(&mut lap, k) as f64;
    let gain = LAPLACIAN_KERNEL.iter().map(|c| c * c).sum::<f64>().sqrt();
    (mad / z / gain).max(NOISE_FLOOR)
}

/// The value at index `k` of `v` sorted ascending by `f32::total_cmp`, found by selection
/// (`select_nth_unstable_by`: the element a sort would put at `k`, in linear expected time)
/// rather than a full sort; `v` is reordered. Under a total order equal elements have equal
/// bits, so the value is exactly the sorted one. Not from the literature: the noise and
/// ringing measures read one or two order statistics, and sorting a whole image's worth of
/// Laplacians for them cost 60-75 ms on a 2048 px render.
pub(crate) fn kth_smallest(v: &mut [f32], k: usize) -> f32 {
    *v.select_nth_unstable_by(k, f32::total_cmp).1
}

/// The quantile of `|Laplacian|` read as the noise level.
const NOISE_QUANTILE: f64 = 0.10;

/// The tenth percentile of the half-normal: `Phi^-1(0.55)`. Dividing the tenth percentile of
/// `|x|` by this recovers sigma, as dividing the median by `MAD_TO_SIGMA` does. (For a unit
/// normal `P(|Z| <= z) = 2·Phi(z) − 1`, so the 10% point solves `Phi(z) = 0.55`.)
const Z10: f64 = 0.12566;

/// Recover a coverage field for a two-colour (bilevel) image.
///
/// The image is first composited over white, so transparency reads as paper. `F` and `B`
/// are estimated as robust extremes of the luminance distribution (Rec. 709 weights on the
/// sRGB values) rather than as the min and max, so a stray speck or a JPEG overshoot cannot
/// define the colour axis for the whole image: each is the mean colour of the pixels whose
/// luminance lies within 15% of the range from its extreme. Each pixel's coverage is then
/// the least-squares projection from the module docs, `a = (P−B)·(F−B) / |F−B|²`, clamped
/// to `[0, 1]`. The darker extreme is always taken as the foreground.
///
/// `sigma_alpha = sigma_pixel / |F−B| · min(1 / max(saturation, 0.05), 8)`: noise
/// propagated through the projection, inflated when the foreground was never observed
/// (see `saturation` below and on [`CoverageField`]).
///
/// Edge cases: an empty image returns an empty field; a uniform one (`F == B`) returns an
/// all-zero field with `sigma_alpha = 0`, since there is no boundary to find.
pub fn bilevel_coverage(img: &Rgba) -> CoverageField {
    let (w, h) = (img.width, img.height);
    let rgb = img.composited([1.0, 1.0, 1.0]);
    let lum: Vec<f32> = rgb
        .iter()
        .map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
        .collect();

    // Take the extremes, but require a small plateau so a single JPEG overshoot or a
    // stray speck cannot define the colour axis for the whole image. A flat percentile
    // (2%) is too conservative: on a shape only a few pixels wide it lands inside the
    // anti-aliased ramp and reports a foreground far lighter than the truth.
    let mut sorted = lum.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let n = sorted.len();
    if n == 0 {
        return CoverageField {
            width: w,
            height: h,
            data: Vec::new(),
            sigma_alpha: 0.0,
            sigma_model: DEFAULT_SIGMA_MODEL,
            fg: [0.0, 0.0, 0.0],
            bg: [1.0, 1.0, 1.0],
            saturation: 1.0,
        };
    }
    let plateau = ((n as f64 * 0.001) as usize).clamp(1, n.saturating_sub(1).max(1));
    let lo = sorted[plateau.min(n - 1)];
    let hi = sorted[n.saturating_sub(1 + plateau)];

    // Foreground is the darker extreme; background the lighter.
    let fg = mean_rgb_where(&rgb, &lum, |v| v <= lo + (hi - lo) * 0.15, [0.0, 0.0, 0.0]);
    let bg = mean_rgb_where(&rgb, &lum, |v| v >= hi - (hi - lo) * 0.15, [1.0, 1.0, 1.0]);

    let d = [fg[0] - bg[0], fg[1] - bg[1], fg[2] - bg[2]];
    let dd = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) as f64;
    // A uniform image has fg == bg, so the colour axis is degenerate and dividing by `dd`
    // below would produce NaN (0/0) that `f64::clamp` passes straight through. There is no
    // boundary to extract; return an empty, fully-saturated field instead.
    if dd <= f64::EPSILON {
        return CoverageField {
            width: w,
            height: h,
            data: vec![0.0; w * h],
            sigma_alpha: 0.0,
            sigma_model: DEFAULT_SIGMA_MODEL,
            fg,
            bg,
            saturation: 1.0,
        };
    }
    let contrast = dd.sqrt().max(1e-6);

    let data: Vec<f32> = rgb
        .iter()
        .map(|p| {
            let n = (p[0] - bg[0]) * d[0] + (p[1] - bg[1]) * d[1] + (p[2] - bg[2]) * d[2];
            (n as f64 / dd).clamp(0.0, 1.0) as f32
        })
        .collect();

    // Is the foreground colour ever actually *observed*?
    //
    // It is observed only where some pixel lies entirely inside the shape. A feature
    // narrower than a pixel has no such pixel, so the darkest thing in the image is a
    // blend, and estimating `F` from it reports a foreground far lighter than the truth.
    //
    // Measuring this from the coverage field would be circular — normalization maps the
    // darkest observation to 1.0 by construction, so "how many pixels are near 1.0"
    // always answers "plenty". The non-circular question is geometric: how many covered
    // pixels are *strictly interior*, with every neighbour also covered? For a resolved
    // shape, most are. For a sub-pixel stroke, none are.
    let mut inside = 0usize;
    let mut interior = 0usize;
    for y in 0..h {
        for x in 0..w {
            if data[y * w + x] < 0.5 {
                continue;
            }
            inside += 1;
            let nb_inside = |dx: isize, dy: isize| -> bool {
                let (nx, ny) = (x as isize + dx, y as isize + dy);
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    return false;
                }
                data[ny as usize * w + nx as usize] >= 0.5
            };
            if nb_inside(-1, 0) && nb_inside(1, 0) && nb_inside(0, -1) && nb_inside(0, 1) {
                interior += 1;
            }
        }
    }
    let saturation = if inside == 0 {
        1.0
    } else {
        interior as f64 / inside as f64
    };

    let sigma_pixel = estimate_noise(&lum, w, h);
    // Under-saturated images have an under-determined colour axis, so the coverage
    // estimate is less trustworthy than pixel noise alone would suggest. Inflating sigma
    // propagates that into the fit tolerance instead of hiding it.
    let confidence_penalty = (1.0 / saturation.max(0.05)).min(8.0);
    CoverageField {
        width: w,
        height: h,
        data,
        sigma_alpha: sigma_pixel / contrast * confidence_penalty,
        sigma_model: DEFAULT_SIGMA_MODEL,
        fg,
        bg,
        saturation,
    }
}

/// Mean colour of the pixels whose luminance satisfies `pred`, or `fallback` if none does.
///
/// `rgb` and `lum` are parallel per-pixel arrays (sRGB `0..1` and its luminance). The sum
/// is accumulated in f64 so a large image does not lose precision.
fn mean_rgb_where(
    rgb: &[[f32; 3]],
    lum: &[f32],
    pred: impl Fn(f32) -> bool,
    fallback: [f32; 3],
) -> [f32; 3] {
    let mut acc = [0.0f64; 3];
    let mut n = 0usize;
    for (c, &l) in rgb.iter().zip(lum) {
        if pred(l) {
            acc[0] += c[0] as f64;
            acc[1] += c[1] as f64;
            acc[2] += c[2] as f64;
            n += 1;
        }
    }
    if n == 0 {
        return fallback;
    }
    [
        (acc[0] / n as f64) as f32,
        (acc[1] / n as f64) as f32,
        (acc[2] / n as f64) as f32,
    ]
}

/// How much of the image looks like compression ringing rather than artwork.
///
/// **This fills a gap both of its neighbours explicitly disclaim.** [`intake_scale`] reads
/// 1.00 on a JPEG and says so in its own doc: compression adds ringing rather than width,
/// and that is the noise estimate's business, not this one's. But [`estimate_noise`] cannot
/// see it either, and says so too: an icon is mostly empty, so more than half its pixels are
/// exactly flat, the median (and so the lower quantile it now reads) of the Laplacian is
/// zero, and the estimate sits on [`NOISE_FLOOR`]
/// however damaged the file is. Measured over 150 rendered SVGs at four qualities, the
/// shipped estimate is the same constant 0.00196 for a clean render, a quality-85 JPEG and a
/// quality-35 JPEG, while the true deviation from the clean original rises 0.0000, 0.0042,
/// 0.0083, 0.0115. Until now the only thing that could tell the difference was the file
/// extension, through `lossy_container`, which a re-encode through PNG silently defeats.
///
/// The statistic is *where* the energy sits, not how much of it there is. Anti-aliasing hugs
/// a boundary and dies within a pixel or two; ringing sits in a band a few pixels out, where
/// clean vector art is exactly flat. So take
///
/// * `core`, pixels within 1 px of a strong edge, whose Laplacian is the edge itself, and
/// * `ring`, pixels 3 to 7 px out, which clean art leaves empty,
///
/// and divide the ring's 90th percentile by the core's median. The ratio is dimensionless, so
/// it does not care how much contrast or how many edges an image has, which is what made the
/// raw ring energy useless: heavy compression blurs edges, shrinking numerator and denominator
/// together, and only the ratio stays monotone (measured 0.000 clean, 0.179 at q85, 0.215 at
/// q60, 0.238 at q35).
///
/// The ratio alone fires on 24% of clean brand logos, because a gradient puts real signal in
/// the ring. So it is multiplied by the *sign alternation rate* of the Laplacian there: Gibbs
/// ringing oscillates pixel to pixel, smooth artwork does not. That does not separate them
/// perfectly, since an 8-bit gradient has banding steps that oscillate too, but at the shipped
/// threshold it takes clean brand logos from 19% to 3.8% while still catching 82 to 91% of
/// JPEG. Measured across benchmark datasets.
///
/// In symbols, with `L` the 4-neighbour Laplacian of Rec. 709 luminance and `d` the chamfer
/// distance to the nearest pixel whose gradient exceeds 24/255:
///
/// ```text
///     core  = { |L_i| : d_i <= 1 px }         ring = { |L_i| : 3 px < d_i <= 7 px }
///     ratio = P90(ring) / P50(core)
///     hot   = ring pixels with |L| > P60(ring)
///     score = ratio · (#hot right/down neighbour pairs whose L changes sign) / (#hot pairs)
/// ```
///
/// Returns 0.0 when there is no edge to measure around, which is the safe answer: the guard
/// this feeds is only ever switched on by positive evidence. The same holds for images
/// under 9x9, a buffer shorter than `width * height`, or fewer than 64 samples in either
/// set or 64 hot pairs.
pub fn ringing_score(rgb: &[[f32; 3]], width: usize, height: usize) -> f64 {
    /// A gradient magnitude below this is not an edge worth measuring around.
    const EDGE_FLOOR: f32 = 24.0 / 255.0;
    /// Chamfer 3-4 distance transform: three units to a pixel.
    const UNIT: u16 = 3;
    const CORE_D: u16 = UNIT;
    const RING_IN: u16 = 3 * UNIT;
    const RING_OUT: u16 = 7 * UNIT;
    /// Too few samples on either side and the percentiles mean nothing.
    const MIN_SAMPLES: usize = 64;

    if width < 9 || height < 9 || rgb.len() < width * height {
        return 0.0;
    }
    let lum: Vec<f32> = rgb
        .iter()
        .map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2])
        .collect();

    let Some(dist) = chamfer_distance_to_edges(&lum, width, height, EDGE_FLOOR) else {
        return 0.0;
    };

    let lap = |i: usize| -> f32 {
        4.0 * lum[i] - lum[i - 1] - lum[i + 1] - lum[i - width] - lum[i + width]
    };
    let mut core: Vec<f32> = Vec::new();
    let mut ring: Vec<f32> = Vec::new();
    let mut ring_idx: Vec<usize> = Vec::new();
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let i = y * width + x;
            let d = dist[i];
            if d <= CORE_D {
                core.push(lap(i).abs());
            } else if d > RING_IN && d <= RING_OUT {
                ring.push(lap(i).abs());
                ring_idx.push(i);
            }
        }
    }
    if core.len() < MIN_SAMPLES || ring.len() < MIN_SAMPLES {
        return 0.0;
    }
    // Percentile `q` is the element at `round((len - 1) q)` of the sorted values. The values
    // are magnitudes (no NaN, no negative zero), where `total_cmp` orders as `<` does, so a
    // selection returns exactly what sorting did. p60 is selected inside the part p90's
    // selection left below it.
    let at = |len: usize, q: f64| ((len - 1) as f64 * q).round() as usize;
    let mut core = core;
    let k50 = at(core.len(), 0.50);
    let core_med = (kth_smallest(&mut core, k50) as f64).max(1e-9);
    let mut ring_sel = ring.clone();
    let (k90, k60) = (at(ring_sel.len(), 0.90), at(ring_sel.len(), 0.60));
    let ring_p90 = kth_smallest(&mut ring_sel, k90) as f64;
    let ring_p60 = (kth_smallest(&mut ring_sel[..=k90], k60) as f64).max(1e-9);
    let ratio = ring_p90 / core_med;
    if ratio <= 0.0 {
        return 0.0;
    }

    // Sign alternation among the ring pixels that actually carry energy. A pixel is only
    // paired with a neighbour when both are hot, so flat zeros cannot drift the rate.
    let mut hot = vec![false; width * height];
    for (&i, &a) in ring_idx.iter().zip(ring.iter()) {
        if a as f64 > ring_p60 {
            hot[i] = true;
        }
    }
    let (mut flips, mut pairs) = (0usize, 0usize);
    for &i in &ring_idx {
        if !hot[i] {
            continue;
        }
        for j in [i + 1, i + width] {
            if j < hot.len() && hot[j] {
                pairs += 1;
                if lap(i) * lap(j) < 0.0 {
                    flips += 1;
                }
            }
        }
    }
    if pairs < MIN_SAMPLES {
        return 0.0;
    }
    ratio * (flips as f64 / pairs as f64)
}

/// Chamfer 3-4 distance from every pixel to the nearest strong edge of `lum`.
///
/// A pixel is an edge (distance 0) when its central-difference gradient
/// `sqrt((l[x+1]−l[x−1])² + (l[y+1]−l[y−1])²)` exceeds `edge_floor`; border pixels are never
/// edges. Distances are then propagated by the classic two-pass chamfer transform: a
/// forward raster pass taking the minimum over the already-visited N, NW, NE and W
/// neighbours, then a backward pass over S, SW, SE and E, with axial steps costing 3 and
/// diagonal steps 4. The result approximates Euclidean distance times 3 (3 units to a
/// pixel), in integers. Pixels with no edge anywhere keep `u16::MAX / 2`, so the additions
/// cannot overflow.
///
/// Returns `None` when the image has no edge at all. Needs `width, height >= 3`.
fn chamfer_distance_to_edges(
    lum: &[f32],
    width: usize,
    height: usize,
    edge_floor: f32,
) -> Option<Vec<u16>> {
    let mut dist = vec![u16::MAX / 2; width * height];
    let mut any_edge = false;
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let i = y * width + x;
            let gx = lum[i + 1] - lum[i - 1];
            let gy = lum[i + width] - lum[i - width];
            if (gx * gx + gy * gy).sqrt() > edge_floor {
                dist[i] = 0;
                any_edge = true;
            }
        }
    }
    if !any_edge {
        return None;
    }
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            let mut best = dist[i];
            if y > 0 {
                best = best.min(dist[i - width] + 3);
                if x > 0 {
                    best = best.min(dist[i - width - 1] + 4);
                }
                if x + 1 < width {
                    best = best.min(dist[i - width + 1] + 4);
                }
            }
            if x > 0 {
                best = best.min(dist[i - 1] + 3);
            }
            dist[i] = best;
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let i = y * width + x;
            let mut best = dist[i];
            if y + 1 < height {
                best = best.min(dist[i + width] + 3);
                if x > 0 {
                    best = best.min(dist[i + width - 1] + 4);
                }
                if x + 1 < width {
                    best = best.min(dist[i + width + 1] + 4);
                }
            }
            if x + 1 < width {
                best = best.min(dist[i + 1] + 3);
            }
            dist[i] = best;
        }
    }
    Some(dist)
}

/// How many raster pixels one unit of genuine detail occupies — the intake's
/// point-spread width, in pixels.
///
/// Every pixel-denominated threshold in this tracer was tuned on a 128 px corpus,
/// and the tempting way to carry them to other sizes is to scale them with the
/// image dimensions. That is measurably wrong. A *native* render at 1024 traces in
/// 1.67 s and finds 29 regions; the same content upsampled from 128 to 1024 takes
/// 55.6 s and finds 1207. Native input reports 30 regions at 512 and 29 at 1024 --
/// already content-determined, already right. Scaling thresholds by image size
/// would coarsen detail that is genuinely resolved.
///
/// What varies is not how many pixels there are but how many an edge takes to
/// cross. A native antialiased render puts an edge in about one pixel at any
/// resolution; an 8x upsample smears it over several, and so does a blur, and so
/// does a photograph of a screen. That width is the unit the thresholds actually
/// meant.
///
/// Measured from the ratio of first to second differences. A ramp of height `d`
/// spread over `w` pixels has first difference `d/w` and second difference about
/// `d/w^2`, so their ratio is `w` and the contrast cancels: this reads a width
/// without needing to know how strong the edge is. Taken as a median over pixels
/// that sit on a real edge, so flat interiors and single-pixel noise do not vote.
///
/// Concretely, for every run of three pixels `a, b, c` along a row or a column of the
/// channel mean `(r+g+b)/3`, with `d0 = b − a` and `d1 = c − b`:
///
/// ```text
///     w_obs = max(|d0|, |d1|) / |d1 − d0|     kept when max(|d0|, |d1|) > 2/255
///     scale = max(1, median(clamp(w_obs, 0.25, 64)))
/// ```
///
/// A straight ramp has `d1 = d0` and says nothing (skipped, as is any non-finite ratio);
/// fewer than 16 votes returns 1.0.
///
/// Measured behaviour: exactly
/// 1.00 for native renders at 128, 512 and 1024; 2.39 and 4.00 for 4x and 8x
/// LANCZOS upsamples; 1.70 and 3.00 for Gaussian blur of 1.0 and 2.0. JPEG reads
/// 1.00, correctly -- compression adds ringing rather than width, and that is the
/// noise estimate's business, not this one's.
///
/// Returns at least 1.0. On any input the corpus contains this is exactly 1.0, so
/// every constant keeps the meaning it was tuned with.
pub fn intake_scale(rgb: &[[f32; 3]], width: usize, height: usize) -> f64 {
    /// Below this a first difference is noise rather than an edge.
    const EDGE_FLOOR: f32 = 2.0 / 255.0;
    /// A vanishing second difference means a straight ramp, where the ratio blows
    /// up and says nothing; and nothing here is wider than this many pixels.
    const MAX_W: f64 = 64.0;
    const MIN_W: f64 = 0.25;

    if width < 3 || height < 3 || rgb.len() < width * height {
        return 1.0;
    }
    use rayon::prelude::*;
    let lum: Vec<f32> = rgb[..width * height]
        .par_iter()
        .map(|c| (c[0] + c[1] + c[2]) / 3.0)
        .collect();
    // One run of three pixels `a, b, c`: |first difference| / |second difference|.
    let vote = |a: f32, b: f32, c: f32| -> Option<f64> {
        let (d0, d1) = (b - a, c - b);
        let first = d0.abs().max(d1.abs());
        let second = (d1 - d0).abs();
        if first > EDGE_FLOOR && second > 1e-6 {
            let r = (first / second) as f64;
            if r.is_finite() {
                return Some(r.clamp(MIN_W, MAX_W));
            }
        }
        None
    };
    // Every run of three along a row and along a column votes once. The median below does
    // not depend on the order the votes arrive in, so the columns are walked row by row too
    // (a column-major walk jumps a whole row per step), and the rows in parallel.
    let rows = lum
        .par_chunks(width)
        .flat_map_iter(|row| row.windows(3).filter_map(move |t| vote(t[0], t[1], t[2])));
    let cols = (0..height - 2).into_par_iter().flat_map_iter(|y| {
        let (r0, r1, r2) = (
            &lum[y * width..(y + 1) * width],
            &lum[(y + 1) * width..(y + 2) * width],
            &lum[(y + 2) * width..(y + 3) * width],
        );
        (0..width).filter_map(move |x| vote(r0[x], r1[x], r2[x]))
    });
    let mut w_obs: Vec<f64> = rows.chain(cols).collect();
    if w_obs.len() < 16 {
        return 1.0;
    }
    let mid = w_obs.len() / 2;
    w_obs.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
    w_obs[mid].max(1.0)
}

/// Mean absolute round-trip error, in 8-bit levels, above which a downsample has lost
/// something.
///
/// This cannot separate native from oversampled on its own, and it was measured trying:
/// across seventy corpus rasters the lowest native reading at /2 is 1.29 (a synthetic
/// gradient, which really is band-limited and really does survive halving), while the 4x
/// upscale this exists for reads 2.12 at /4. The distributions overlap, so a threshold
/// permissive enough to catch the upscale would also rewrite smooth native artwork.
///
/// So this is not a gate and must not be used as one. The caller decides whether a raster
/// is native -- `intake_scale` against `color::SOFT_INTAKE_EDGE` does that, and its
/// margin is real (native maximum 1.50 against a 1.75 threshold) -- and only then asks
/// this by how much. Inside that gate the value can be generous, because nothing native
/// reaches it.
const OVERSAMPLE_TOL: f64 = 3.0;

/// By what factor this raster carries the same drawing on more pixels than it needs.
///
/// [`intake_scale`] answers a related question by measuring how wide an edge transition
/// is, and it is the right measure for the palette's noise guard: a soft edge really does
/// put intermediate colours on the ramp. It is the wrong measure for *tolerances*,
/// because a super-resolution model defeats it -- it returns a sharp edge at high
/// resolution, so the raster reads as barely oversampled when it carries four times the
/// pixels the drawing needs. Measured on a real brand mark upscaled 4x: edge width 2.00,
/// where the answer is 4.
///
/// This asks the question directly instead. An oversampled raster has a property that
/// sharpening cannot fake: its pixels can be thrown away and put back. Halve it, restore
/// it, and compare -- if nothing was lost, the halved version already carried the whole
/// drawing. Repeated, that gives the factor, and it is indifferent to whether the surplus
/// pixels are crisp or blurred, asking only whether they say anything.
///
/// For `k` in 2, 4, 8: box-average `k x k` blocks (the trailing `width mod k` columns and
/// rows are dropped), resample back to full size bilinearly — pixel centre `x` maps to
/// `(x + 0.5)/k − 0.5` in the small image, clamped to its edge — and take the mean absolute
/// error over all pixels and channels, in 8-bit levels. The largest `k` whose error stays
/// under `OVERSAMPLE_TOL`, with every smaller `k` also passing, is the answer. The search
/// stops once the small image would be under 8 px on a side.
///
/// Returns 1 for a native render, which is every raster in the corpus, so a caller that
/// scales by this leaves native intake exactly as it found it; also for anything under
/// 16x16 or a buffer shorter than `width * height`.
pub fn oversample_factor(rgb: &[[f32; 3]], width: usize, height: usize) -> usize {
    if width < 16 || height < 16 || rgb.len() < width * height {
        return 1;
    }
    let mut best = 1usize;
    for k in [2usize, 4, 8] {
        let (sw, sh) = (width / k, height / k);
        if sw < 8 || sh < 8 {
            break;
        }
        // Box down, bilinear back, and compare against what we started with.
        let mut small = vec![[0.0f32; 3]; sw * sh];
        for y in 0..sh {
            for x in 0..sw {
                let mut acc = [0.0f64; 3];
                for dy in 0..k {
                    for dx in 0..k {
                        let p = rgb[(y * k + dy) * width + (x * k + dx)];
                        for c in 0..3 {
                            acc[c] += p[c] as f64;
                        }
                    }
                }
                let n = (k * k) as f64;
                small[y * sw + x] = [
                    (acc[0] / n) as f32,
                    (acc[1] / n) as f32,
                    (acc[2] / n) as f32,
                ];
            }
        }
        let mut err = 0.0f64;
        for y in 0..height {
            for x in 0..width {
                // Bilinear sample of `small` at this pixel's centre.
                let fx = ((x as f64 + 0.5) / k as f64 - 0.5).clamp(0.0, sw as f64 - 1.0);
                let fy = ((y as f64 + 0.5) / k as f64 - 0.5).clamp(0.0, sh as f64 - 1.0);
                let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
                let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
                let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
                for c in 0..3 {
                    let a = small[y0 * sw + x0][c] as f64 * (1.0 - tx)
                        + small[y0 * sw + x1][c] as f64 * tx;
                    let b = small[y1 * sw + x0][c] as f64 * (1.0 - tx)
                        + small[y1 * sw + x1][c] as f64 * tx;
                    err += (a * (1.0 - ty) + b * ty - rgb[y * width + x][c] as f64).abs();
                }
            }
        }
        err = err * 255.0 / (width * height * 3) as f64;
        if err < OVERSAMPLE_TOL {
            best = k;
        } else {
            break;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hard step at `edge`, then a linear ramp of `width` pixels, then flat again.
    /// `width = 1` is a native render's one blend pixel.
    fn ramp(w: usize, h: usize, edge: f64, width: f64) -> Vec<[f32; 3]> {
        let mut out = vec![[0.0f32; 3]; w * h];
        for y in 0..h {
            for x in 0..w {
                let t = ((x as f64 - edge) / width).clamp(0.0, 1.0) as f32;
                out[y * w + x] = [t, t, t];
            }
        }
        out
    }

    fn gray(rgb: &[[f32; 3]]) -> Vec<f32> {
        rgb.iter().map(|c| c[0]).collect()
    }

    #[test]
    fn a_native_edge_measures_one_pixel() {
        // The documented anchor: every raster in the corpus reads exactly 1.0, which is
        // what lets every pixel-denominated constant keep the meaning it was tuned with.
        let img = ramp(64, 64, 20.0, 1.0);
        let s = intake_scale(&img, 64, 64);
        assert!((s - 1.0).abs() < 0.15, "native edge read {s}, want 1.0");
    }

    /// A step blurred by a Gaussian of the given sigma -- what a resample or an upscale
    /// actually leaves behind. A piecewise-linear ramp will not do here: its second
    /// difference vanishes in the interior, which is the degenerate case `intake_scale`
    /// documents as saying nothing.
    fn blurred_step(w: usize, h: usize, edge: f64, sigma: f64) -> Vec<[f32; 3]> {
        let mut out = vec![[0.0f32; 3]; w * h];
        for y in 0..h {
            for x in 0..w {
                // 0.5 * (1 + erf(d / (sigma * sqrt 2))), with a tanh standing in for erf.
                let d = (x as f64 - edge) / sigma;
                let t = (0.5 * (1.0 + (0.8 * d).tanh())) as f32;
                out[y * w + x] = [t, t, t];
            }
        }
        out
    }

    #[test]
    fn a_blurred_edge_measures_wider() {
        // Monotone in the true width, which is the whole basis for using it as a gate.
        let mut last = 0.0;
        for sigma in [1.0, 2.0, 4.0] {
            let img = blurred_step(128, 128, 60.0, sigma);
            let s = intake_scale(&img, 128, 128);
            assert!(
                s > last,
                "sigma {sigma} read {s}, not above the previous {last}"
            );
            last = s;
        }
        // ...and a genuinely soft intake clears the threshold the palette gates on.
        assert!(last > SOFT_INTAKE_EDGE_FOR_TEST);
    }

    #[test]
    fn intake_scale_never_reports_under_one() {
        // A flat image has no edge to measure; the floor keeps callers from scaling by
        // something smaller than a pixel.
        let flat = vec![[0.5f32; 3]; 32 * 32];
        assert!(intake_scale(&flat, 32, 32) >= 1.0);
    }

    /// A clean edge scores zero and a ringing one does not, with the shipped gate between.
    ///
    /// The two images differ only in what sits 3-7 px out from the boundary: nothing, or a
    /// decaying oscillation of the kind a DCT reconstruction leaves. Both have the same edge,
    /// the same contrast and the same size, so anything the statistic reports comes from the
    /// ring alone. This is the property the whole detector rests on, and it is the one that
    /// four other candidate statistics failed: they measured edge sharpness instead, and so
    /// FELL as compression got worse.
    #[test]
    fn ringing_is_seen_in_the_ring_and_nowhere_else() {
        let (w, h) = (256usize, 256usize);
        let build = |ring: bool| -> Vec<[f32; 3]> {
            let mut px = vec![[1.0f32; 3]; w * h];
            for y in 0..h {
                for x in 0..w {
                    // A vertical boundary down the middle: dark left, light right.
                    let mut v = if x < w / 2 { 0.1f32 } else { 0.9 };
                    if ring {
                        // Gibbs-like overshoot, decaying with distance and alternating sign,
                        // confined to the band the statistic reads.
                        let d = (x as i32 - (w / 2) as i32).unsigned_abs() as usize;
                        if (3..=7).contains(&d) {
                            let sign = if d.is_multiple_of(2) { 1.0 } else { -1.0 };
                            v += sign * 0.10 * (1.0 - (d as f32 - 3.0) / 5.0);
                        }
                    }
                    px[y * w + x] = [v, v, v];
                }
            }
            px
        };
        let clean = ringing_score(&build(false), w, h);
        let ringy = ringing_score(&build(true), w, h);
        assert!(
            clean < crate::color::SOFT_RINGING_LARGE,
            "a clean edge must not trip the guard, got {clean}"
        );
        assert!(
            ringy > crate::color::SOFT_RINGING_LARGE,
            "ringing in the band must trip the guard, got {ringy}"
        );
        assert!(
            ringy > clean * 2.0 + 0.01,
            "ringing {ringy} must be clearly above clean {clean}"
        );
    }

    /// The statistic answers rather than panicking on inputs with no edge or no room.
    #[test]
    fn ringing_score_is_safe_on_degenerate_input() {
        assert_eq!(ringing_score(&[], 0, 0), 0.0);
        assert_eq!(ringing_score(&[[0.5; 3]; 16], 4, 4), 0.0);
        // A perfectly flat image has no boundary to measure around.
        assert_eq!(ringing_score(&vec![[0.5; 3]; 64 * 64], 64, 64), 0.0);
        // Mismatched dimensions must not index out of bounds.
        assert_eq!(ringing_score(&[[0.5; 3]; 16], 100, 100), 0.0);
    }

    /// `intake_scale` as it shipped: rows, then columns walked column by column.
    fn intake_scale_reference(rgb: &[[f32; 3]], width: usize, height: usize) -> f64 {
        if width < 3 || height < 3 || rgb.len() < width * height {
            return 1.0;
        }
        let lum = |i: usize| -> f32 { (rgb[i][0] + rgb[i][1] + rgb[i][2]) / 3.0 };
        let mut w_obs: Vec<f64> = Vec::new();
        for axis in 0..2 {
            let (n_outer, n_inner, step) = if axis == 0 {
                (height, width, 1usize)
            } else {
                (width, height, width)
            };
            for o in 0..n_outer {
                for k in 0..n_inner.saturating_sub(2) {
                    let base = if axis == 0 {
                        o * width + k
                    } else {
                        k * width + o
                    };
                    let (a, b, c) = (lum(base), lum(base + step), lum(base + 2 * step));
                    let (d0, d1) = (b - a, c - b);
                    let first = d0.abs().max(d1.abs());
                    let second = (d1 - d0).abs();
                    if first > 2.0 / 255.0 && second > 1e-6 {
                        let r = (first / second) as f64;
                        if r.is_finite() {
                            w_obs.push(r.clamp(0.25, 64.0));
                        }
                    }
                }
            }
        }
        if w_obs.len() < 16 {
            return 1.0;
        }
        let mid = w_obs.len() / 2;
        w_obs.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
        w_obs[mid].max(1.0)
    }

    #[test]
    fn intake_scale_walking_rows_only_equals_the_column_walk() {
        let mut s = 0xfeed_beefu64;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 40) as f32 / (1u64 << 24) as f32
        };
        for case in 0..60 {
            let (w, h) = (1 + (next() * 70.0) as usize, 1 + (next() * 70.0) as usize);
            let width = 1.0 + next() * 6.0;
            let img: Vec<[f32; 3]> = (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as f32, (i / w) as f32);
                    let t = (((x + 0.7 * y) % 17.0) / width).min(1.0);
                    let n = if case % 3 == 0 { next() * 0.02 } else { 0.0 };
                    [t + n, 0.5 * t, 1.0 - t]
                })
                .collect();
            let (a, b) = (intake_scale(&img, w, h), intake_scale_reference(&img, w, h));
            assert_eq!(a.to_bits(), b.to_bits(), "case {case} {w}x{h}");
        }
    }

    #[test]
    fn selection_returns_what_sorting_did_including_nested_percentiles() {
        let mut s = 0x1234_5678_9abc_def1u64;
        for len in 1..200usize {
            let v: Vec<f32> = (0..len)
                .map(|_| {
                    s ^= s << 13;
                    s ^= s >> 7;
                    s ^= s << 17;
                    // Few distinct values, so ties are common.
                    ((s >> 40) % 23) as f32 * 0.125
                })
                .collect();
            let mut sorted = v.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let at = |q: f64| ((len - 1) as f64 * q).round() as usize;
            for q in [0.0, 0.1, 0.5, 0.6, 0.9, 1.0] {
                let mut w = v.clone();
                assert_eq!(kth_smallest(&mut w, at(q)), sorted[at(q)]);
            }
            let mut w = v.clone();
            let (k90, k60) = (at(0.9), at(0.6));
            assert_eq!(kth_smallest(&mut w, k90), sorted[k90]);
            assert_eq!(kth_smallest(&mut w[..=k90], k60), sorted[k60]);
        }
    }

    #[test]
    fn degenerate_sizes_are_answered_not_panicked() {
        // Every one of these is reachable from a real file: a 1-px favicon, a truncated
        // decode, a caller that passed the wrong dimensions.
        assert!(intake_scale(&[], 0, 0) >= 1.0);
        assert!(intake_scale(&[[0.0; 3]; 4], 2, 2) >= 1.0);
        assert!(intake_scale(&[[0.0; 3]; 4], 100, 100) >= 1.0);
        assert_eq!(oversample_factor(&[], 0, 0), 1);
        assert_eq!(oversample_factor(&[[0.0; 3]; 4], 2, 2), 1);
        assert!(estimate_noise(&[], 0, 0) > 0.0);
        assert!(estimate_noise(&[0.5; 4], 2, 2) > 0.0);
    }

    #[test]
    fn bilevel_coverage_is_defined_on_a_uniform_image() {
        // A solid-colour image has fg == bg, which used to divide by zero and put NaN in
        // every coverage sample; `f64::clamp` passes NaN through, so it survived to the
        // contour stage. There must be no NaN and saturation must read fully covered.
        let img = Rgba {
            width: 16,
            height: 16,
            data: [0.5f32, 0.5, 0.5, 1.0].repeat(16 * 16),
        };
        let field = bilevel_coverage(&img);
        assert_eq!(field.data.len(), 16 * 16);
        assert!(field.data.iter().all(|v| v.is_finite()), "no NaN");
        assert_eq!(field.saturation, 1.0);
    }

    #[test]
    fn bilevel_coverage_is_defined_on_a_zero_pixel_image() {
        let img = Rgba {
            width: 0,
            height: 0,
            data: Vec::new(),
        };
        let field = bilevel_coverage(&img);
        assert!(field.data.is_empty());
        assert_eq!(field.saturation, 1.0);
    }

    #[test]
    fn estimate_noise_guards_a_short_buffer() {
        // A truncated buffer with claimed dimensions must not index out of bounds; the
        // sibling estimators all carry this guard, estimate_noise was the one that did not.
        assert!(estimate_noise(&[], 100, 100) > 0.0);
        assert!(estimate_noise(&[0.5f32; 4], 100, 100) > 0.0);
    }

    /// Deterministic standard normal samples, by Box-Muller on a linear congruential
    /// generator. Written out rather than pulled from a crate so the test has no
    /// dependency and cannot flake between runs.
    fn normals(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        let mut next = || {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            ((s >> 8) as f64 + 0.5) / 16777216.0
        };
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            let (u1, u2): (f64, f64) = (next(), next());
            let r = (-2.0 * u1.ln()).sqrt();
            out.push((r * (std::f64::consts::TAU * u2).cos()) as f32);
            out.push((r * (std::f64::consts::TAU * u2).sin()) as f32);
        }
        out.truncate(n);
        out
    }

    /// The estimator must recover a sigma it was never told, from data whose true sigma is
    /// known by construction.
    ///
    /// This is the test that catches a wrong noise-gain divisor, and it is the reason the
    /// gain is computed from `LAPLACIAN_KERNEL` rather than written beside it. The previous
    /// divisor of `sqrt(6)` — the *one-dimensional* second difference's gain, applied to a
    /// two-dimensional kernel — made every live estimate 1.83x too large, and no test in
    /// the suite could see it because every corpus image sits on the noise floor. A test
    /// that asserts a stated constant only re-states it; a test that regenerates the truth
    /// and demands the code find it is the one that fails when the derivation is wrong.
    #[test]
    fn noise_estimate_recovers_a_known_sigma() {
        let (w, h) = (400, 400);
        for &sigma_levels in &[2.0f64, 5.0, 12.0] {
            let sigma = sigma_levels / 255.0;
            let g: Vec<f32> = normals(w * h, 4242)
                .iter()
                .map(|z| 0.5 + z * sigma as f32)
                .collect();
            let est = estimate_noise(&g, w, h);
            let ratio = est / sigma;
            assert!(
                (0.9..1.1).contains(&ratio),
                "true sigma {sigma_levels}/255, estimated {:.3}/255, ratio {ratio:.3} \
                 (a ratio near 1.83 means the noise gain is the 1-D sqrt(6) again)",
                est * 255.0
            );
        }
    }

    /// A noiseless picture must not report noise, however many edges it has.
    ///
    /// The regression the quantile exists for: where most of the image is edge, the median
    /// |Laplacian| reads an edge and calls it noise — 53 display levels against a true 0.57
    /// on a real labyrinth, which loosens every tolerance downstream. Three-pixel stripes
    /// reproduce it: a third of the pixels sit mid-run with a zero Laplacian, so the tenth
    /// percentile is noise and the median is not. The median reading is checked to fail,
    /// which is the proof the test bites.
    #[test]
    fn a_noiseless_edge_dense_picture_reports_no_noise() {
        let (w, h) = (300, 300);
        let g: Vec<f32> = (0..w * h)
            .map(|i| if (i % w) % 6 < 3 { 0.0 } else { 1.0 })
            .collect();
        let est = estimate_noise(&g, w, h) * 255.0;
        assert!(
            est < 2.0,
            "a noiseless stripe pattern reported {est:.1} display levels of noise;              reading the median instead of the {NOISE_QUANTILE} quantile reports tens"
        );
        let median = estimate_noise_at(&g, w, h, 0.5, MAD_TO_SIGMA) * 255.0;
        assert!(median > 10.0, "the median reads {median:.1}");
    }

    /// The gain is only correct if the coefficients describe the loop's arithmetic.
    #[test]
    fn the_kernel_constant_matches_the_loop() {
        // The loop computes `4*c - n - s - e - w`.
        assert_eq!(LAPLACIAN_KERNEL, [4.0, -1.0, -1.0, -1.0, -1.0]);
        // A Laplacian must annihilate a constant field, or it is not measuring noise.
        let sum: f64 = LAPLACIAN_KERNEL.iter().sum();
        assert!(sum.abs() < 1e-12, "kernel does not sum to zero: {sum}");
        // And the gain that divides the MAD is this kernel's, not another's.
        let gain = LAPLACIAN_KERNEL.iter().map(|c| c * c).sum::<f64>().sqrt();
        assert!((gain - 20.0f64.sqrt()).abs() < 1e-12, "gain {gain}");
    }

    #[test]
    fn noise_estimate_floors_at_half_a_level() {
        // A clean render has no noise to find, and the estimate must not return zero:
        // the palette divides by it.
        let flat = vec![0.5f32; 64 * 64];
        let s = estimate_noise(&flat, 64, 64);
        assert!((s - 0.5 / 255.0).abs() < 1e-9, "clean image read {s}");
    }

    #[test]
    fn noise_estimate_rises_with_real_noise() {
        // Deterministic pseudo-noise, so this cannot flake.
        let (w, h) = (64, 64);
        let mut g = vec![0.5f32; w * h];
        let mut seed = 12345u32;
        for v in g.iter_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *v += ((seed >> 16) as f32 / 65535.0 - 0.5) * 0.1;
        }
        assert!(estimate_noise(&g, w, h) > 2.0 * (0.5 / 255.0));
    }

    /// The regression this module let through on 2026-09-08.
    ///
    /// `intake_scale` measures edge *width*, and JPEG does not widen edges -- it rings
    /// flat regions. The palette's noise guard was gated on this function alone, so a
    /// compressed logo measured a native 1.15 px, the guard stayed off, and the tracer
    /// fitted the encoder's ringing as artwork. The fix was to stop asking this function
    /// a question it cannot answer (see `crate::lossy_container`), and this test pins the
    /// limitation in place so nobody gates on it again by mistake.
    #[test]
    fn ringing_does_not_widen_an_edge() {
        let (w, h) = (96, 96);
        let mut img = ramp(w, h, 30.0, 1.0);
        // Ringing: a decaying oscillation in the flat regions either side of the edge,
        // leaving the transition itself one pixel wide.
        let mut seed = 999u32;
        for y in 0..h {
            for x in 0..w {
                let d = (x as i64 - 30).abs();
                if d < 2 {
                    continue;
                }
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let amp = 0.06 * (-(d as f32) / 8.0).exp() * ((seed >> 16) as f32 / 65535.0 - 0.5);
                for c in 0..3 {
                    img[y * w + x][c] = (img[y * w + x][c] + amp).clamp(0.0, 1.0);
                }
            }
        }
        let s = intake_scale(&img, w, h);
        assert!(
            s < SOFT_INTAKE_EDGE_FOR_TEST,
            "ringing read {s}; if this now exceeds {SOFT_INTAKE_EDGE_FOR_TEST} the \
                 gate story in `crate::lossy_container` needs revisiting"
        );
        // And neither does the noise estimate, which is the part that makes this a real
        // hole rather than a merely awkward one. `estimate_noise` reads a low quantile of
        // the Laplacian: ringing lives in a band beside the edge while most of the image
        // stays flat, so that quantile never leaves its floor. Measured on the JPEG that prompted
        // the fix it read 0.50/255 -- the floor -- exactly as a clean render does. With
        // both pixel detectors blind, the container is the only honest witness left.
        let clean = ramp(w, h, 30.0, 1.0);
        let floor = 0.5 / 255.0;
        assert!((estimate_noise(&gray(&img), w, h) - floor).abs() < 1e-9);
        assert!((estimate_noise(&gray(&clean), w, h) - floor).abs() < 1e-9);
    }

    /// Kept local so this test does not depend on `color`'s constant staying public.
    const SOFT_INTAKE_EDGE_FOR_TEST: f64 = 1.75;

    #[test]
    fn a_native_raster_is_not_oversampled() {
        // The contract callers rely on: scaling by this leaves native intake untouched.
        let img = ramp(64, 64, 20.0, 1.0);
        assert_eq!(oversample_factor(&img, 64, 64), 1);
    }

    #[test]
    fn an_upscaled_raster_is_oversampled() {
        // A blurred/upscaled step has a wider transition that survives halving.
        let img = blurred_step(64, 64, 30.0, 2.0);
        assert!(oversample_factor(&img, 64, 64) >= 2);
    }

    #[test]
    fn intake_scale_with_degenerate_or_nan_data_never_panics() {
        // Small dimensions
        assert_eq!(intake_scale(&[], 0, 0), 1.0);
        assert_eq!(intake_scale(&[[0.5, 0.5, 0.5]], 1, 1), 1.0);
        assert_eq!(intake_scale(&[[0.5, 0.5, 0.5]; 4], 2, 2), 1.0);

        // Degenerate data with NaNs
        let mut nan_rgb = vec![[0.5f32, 0.5, 0.5]; 32 * 32];
        nan_rgb[10] = [f32::NAN, 0.5, 0.5];
        nan_rgb[25] = [0.5, f32::INFINITY, 0.5];
        let scale = intake_scale(&nan_rgb, 32, 32);
        assert!(scale.is_finite() && scale >= 1.0);
    }
}
