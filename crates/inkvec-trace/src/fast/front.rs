//! Fast mode's front end: palette and labels in two passes ([`super::palette`]), rims and
//! near-identical inks put back into the faces they belong to, speckle removal and faces
//! ([`super::faces`]), the ramp pass ([`super::bands`]), then the shared
//! planar-map stages -- saddles, the map, symmetry, and the sub-pixel refinement of every
//! boundary point.
//!
//! What it leaves out, next to [`crate::trace_color_full_with_alpha`]: the MDL palette and
//! its noise and ringing measurements, blend absorption (the labelling already sends blends
//! to a neighbour's ink), gradient band merging, residual carving and fade recovery, and, in
//! `finish_color_trace_alpha`, the global boundary solve and order-first decoding.

use crate::{coverage, gradient, ColorOptions, ColorTrace, Palette, Rgba, Stopwatch};

/// Trace an image with transparency natively, the fast way: inks carry an opacity and the
/// clear ground is an ink, as in [`crate::native`], from the same histogram palette as an
/// opaque image. No ramps: a gradient across opacity is a fade, which is quality mode's.
pub(crate) fn trace_color_native(img: &Rgba, opts: &ColorOptions, alpha: &[f32]) -> ColorTrace {
    trace(img, opts, Some(alpha), None)
}

/// Trace an opaque (or matted) image the fast way. `source_alpha` is only read to split
/// inks by opacity for `--cutout` (see `color::split_alpha_inks`).
pub(crate) fn trace_color(
    img: &Rgba,
    opts: &ColorOptions,
    source_alpha: Option<&[f32]>,
) -> ColorTrace {
    trace(img, opts, None, source_alpha)
}

/// Speckle floor per 512 x 512 pixels of image, in pixels.
const SPECKLE_PER_512: f64 = 4.0;
/// Largest speckle floor, in pixels: VTracer's default (`filter_speckle` 4, a 4 x 4 patch).
const SPECKLE_MAX: f64 = 16.0;

/// The smallest face fast mode keeps, in pixels: `min_region`, or more on a large image.
///
/// Noise does not shrink as the raster grows, and a fixed floor of two pixels kept every
/// grain of a textured 1672 px masthead as a face with an outline. The floor grows with the
/// image's area, four pixels at 512 x 512, up to VTracer's sixteen from 1024 x 1024 on; a
/// 128 px icon keeps `min_region`. On 20 brand logos rendered at 2048 px this removed 14% of
/// the coordinates and lowered dE00 (0.0729 -> 0.0725); on the masthead 19% at +0.01.
fn speckle_floor(min_region: usize, w: usize, h: usize) -> usize {
    let scaled = (SPECKLE_PER_512 * (w * h) as f64 / (512.0 * 512.0)).min(SPECKLE_MAX);
    min_region.max(scaled.round() as usize)
}

/// A face's fill before the ramp pass: ink `ink`'s sRGB colour as a flat fill, with no
/// fit cost (fast mode never compares fill models by cost). An index outside the palette
/// gives black rather than panicking.
fn flat_fill(pal: &Palette, ink: usize) -> gradient::FillFit {
    gradient::FillFit {
        model: gradient::FillModel::Flat(pal.rgb.get(ink).copied().unwrap_or([0.0; 3])),
        chi2: 0.0,
        params: gradient::PARAMS_FLAT,
        cost: 0.0,
    }
}

/// The front end. `native` is the source alpha when transparency is traced natively;
/// `cutout` the source alpha when the image was matted and `--cutout` splits inks by it.
///
/// Every stage reads the image composited over white (sRGB 0..1); in native mode the
/// source opacity rides along as a fourth channel. The stages, each timed by the
/// stopwatch under its progress name: `palette` ([`super::palette::palette_and_labels`],
/// then `color::split_alpha_inks` for `--cutout`); `slivers` (the labels read into row runs,
/// [`super::faces::RunLabels::new`], then [`super::faces::RunLabels::absorb_slivers`],
/// [`super::faces::RunLabels::absorb_rims`], on a lossy intake
/// [`super::faces::RunLabels::absorb_halos`], and [`super::faces::RunLabels::merge_same_inks`]);
/// `despeckle` ([`super::faces::RunLabels::despeckle`]); `split`
/// ([`super::faces::RunLabels::write_faces`], the face ids written over the label buffer,
/// the only per-pixel write of the clean-up); and `ramps` ([`super::bands::merge_ramps`],
/// opaque images with gradients on only). The faces, their fills and inks then go to
/// [`crate::finish_color_trace_alpha`], which builds the planar map and refines it; the
/// caller (`inkvec-cli`'s `fast::fit`) then fits the map's edges with [`super::fit_edges`].
fn trace(
    img: &Rgba,
    opts: &ColorOptions,
    native: Option<&[f32]>,
    cutout: Option<&[f32]>,
) -> ColorTrace {
    let (w, h) = (img.width, img.height);
    let rgb = img.composited([1.0, 1.0, 1.0]);
    let mut sw = Stopwatch::start();
    inkvec_core::progress::begin("palette");
    let (mut pal, mut labels) = super::palette::palette_and_labels(
        &rgb,
        native,
        w,
        h,
        opts.merge_distance,
        opts.max_colors,
    );
    if let Some(a) = cutout.filter(|a| opts.alpha_inks && a.len() == labels.len()) {
        crate::color::split_alpha_inks(&mut labels, &mut pal, a);
    }
    sw.mark("palette");
    inkvec_core::progress::begin("slivers");
    // The clean-up works on the label image's row runs (`faces::RunLabels`): the labels
    // are read here once, and the pixels are written once, with the face ids, at the end.
    let mut runs = super::faces::RunLabels::new(&labels, w, h);
    {
        // Each pixel's colour and opacity, read in place where a pass needs it.
        let px = super::faces::Pixels {
            rgb: &rgb,
            alpha: native,
        };
        let inks: Vec<[f32; 4]> = (0..pal.len())
            .map(|i| {
                let c = pal.rgb[i];
                [c[0], c[1], c[2], pal.alpha.get(i).copied().unwrap_or(1.0)]
            })
            .collect();
        runs.absorb_slivers(px, &inks);
        // Every face costs an outline, and every place a face touches a boundary is a
        // junction the fitter has to stop at. The rims of small text and the second black
        // of large type were most of both on a textured masthead: 4,068 faces and 47,548
        // coordinates, where these two passes leave 8,798 at a lower dE00.
        runs.absorb_rims(px, &inks);
        // A lossy intake's edges carry the codec's halos, and its inks come back split
        // into variants a few dE00 apart; both are handled only on a lossy intake, and only
        // on an opaque one (the halo test reads JPEG's colour space, which has no
        // opacity). A clean intake takes neither, and its output is unchanged.
        let lossy = opts.lossy_intake && native.is_none();
        if lossy {
            let (halos, specks) = runs.absorb_halos(px, &inks);
            crate::diag!(
                "slivers",
                "lossy intake: {halos} halo components and {specks} halo specks given back"
            );
        }
        runs.merge_same_inks(&inks, lossy.then_some(crate::color::SOFT_SAME_INK_DE00));
    }
    sw.mark("slivers");
    inkvec_core::progress::begin("despeckle");
    runs.despeckle(speckle_floor(opts.min_region, w, h));
    sw.mark("despeckle");
    inkvec_core::progress::begin("split");
    // The face ids overwrite the palette's labels in place: one write per pixel.
    let mut face_color = runs.write_faces(&mut labels);
    drop(runs);
    sw.mark("split");
    inkvec_core::progress::begin("ramps");
    let mut face_fill: Vec<gradient::FillFit> =
        face_color.iter().map(|&l| flat_fill(&pal, l)).collect();
    if opts.gradients && native.is_none() {
        super::bands::merge_ramps(
            &rgb,
            w,
            h,
            &pal,
            &mut labels,
            &mut face_fill,
            &mut face_color,
        );
    }
    sw.mark("ramps");
    let n_faces = face_color.len();
    let face_alpha: Option<Vec<f32>> = native.map(|_| {
        face_color
            .iter()
            .map(|&c| pal.alpha.get(c).copied().unwrap_or(1.0))
            .collect()
    });
    // No noise measurement: the fast fitter reads no per-point uncertainty, and the
    // refinement only needs a floor under its contrast test. For the same reason the strip
    // reading's side test is not scaled with noise here (`soft_intake` false): with only the
    // floor to scale by, it would move the threshold on faint edges by a constant rather than
    // by anything measured (on `fast-web` it changed 12 icons, dE00 and geom no better).
    crate::finish_color_trace_alpha(
        img,
        opts,
        &rgb,
        pal,
        labels,
        face_fill,
        face_color,
        n_faces,
        coverage::NOISE_FLOOR,
        false,
        &mut sw,
        native,
        face_alpha,
    )
}
