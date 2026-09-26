//! Fast mode's front end: palette and labels in two passes ([`super::palette`]), speckle
//! removal and faces ([`super::faces`]), the ramp pass ([`super::bands`]), then the shared
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
fn trace(
    img: &Rgba,
    opts: &ColorOptions,
    native: Option<&[f32]>,
    cutout: Option<&[f32]>,
) -> ColorTrace {
    let (w, h) = (img.width, img.height);
    let rgb = img.composited([1.0, 1.0, 1.0]);
    let mut sw = Stopwatch::start();
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
    {
        let px: Vec<[f32; 4]> = (0..w * h)
            .map(|p| {
                let c = rgb[p];
                [c[0], c[1], c[2], native.map_or(1.0, |a| a[p])]
            })
            .collect();
        let inks: Vec<[f32; 4]> = (0..pal.len())
            .map(|i| {
                let c = pal.rgb[i];
                [c[0], c[1], c[2], pal.alpha.get(i).copied().unwrap_or(1.0)]
            })
            .collect();
        super::faces::absorb_slivers(&mut labels, &px, &inks, w, h);
    }
    sw.mark("slivers");
    super::faces::despeckle(&mut labels, w, h, opts.min_region);
    sw.mark("despeckle");
    let (mut labels, mut face_color) = super::faces::faces(&labels, w, h);
    sw.mark("split");
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
    // refinement only needs a floor under its contrast test.
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
        &mut sw,
        native,
        face_alpha,
    )
}
