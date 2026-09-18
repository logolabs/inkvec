//! Deciding whether an input is damaged enough to be worth cleaning.
//!
//! Getting this wrong is expensive in one direction and cheap in the other. A
//! clean image that gets the pre-pass traces three times worse than if it had
//! been left alone (dE00 0.605 against 0.195). A damaged image that misses the
//! pre-pass merely traces the way it does today. So the threshold sits above the
//! worst clean icon rather than between the two means.
//!
//! The signal is the **interior residual**: a traced model is piecewise flat by
//! construction, so wherever it is locally constant the input should be too, and
//! disagreement there is degradation rather than modelling error.
//!
//! Measured over 30 icons in five conditions (`h26_detector_threshold.py`):
//!
//! | condition | min | median | p95 | max |
//! |---|---|---|---|---|
//! | clean | 0.000 | 0.000 | 0.275 | 0.376 |
//! | jpeg-q80 | 0.454 | 0.907 | 1.441 | 2.708 |
//! | jpeg-q50 | 0.650 | 1.241 | 1.942 | 2.669 |
//! | blur-1.0 | 0.274 | 0.914 | 2.332 | 5.524 |
//! | noise-2 | 0.812 | 0.837 | 1.156 | 1.426 |
//!
//! At 0.5 no clean icon is cleaned and 4.2% of damaged ones are missed. Clean
//! and blur-1.0 do overlap, so no threshold separates every condition; JPEG, the
//! case this was built for, is cleanly separated.
//!
//! A cheaper detector was tried and does not work: an 8x8 block signature, which
//! needs no trace at all, reads 2.175 on clean against 2.133 on JPEG q50. `Auto`
//! pays for one extra trace because nothing cheaper discriminates.

use inkvec_trace::Rgba;

use crate::clean;

/// Interior residual above which an input is treated as degraded.
pub const DEGRADED_RESIDUAL: f64 = 0.5;

/// What went wrong while rasterising a traced SVG for comparison against the input.
#[derive(Debug)]
pub enum RenderError {
    /// The SVG text could not be parsed. Carries the parser's error message.
    Parse(String),
    /// The render target pixmap could not be allocated.
    Alloc,
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::Parse(e) => write!(f, "could not parse the traced SVG: {e}"),
            RenderError::Alloc => write!(f, "could not allocate the render target"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Rasterise an SVG to `w` x `h`, premultiplied alpha undone, in [0, 1].
///
/// resvg deliberately, and not by accident of what was available: it is the same
/// renderer the benchmark harness scores with, so a residual measured here and a
/// score measured there cannot disagree about what the SVG looks like.
pub fn render_svg(svg: &str, w: usize, h: usize) -> Result<Rgba, RenderError> {
    let opt = resvg::usvg::Options::default();
    let tree =
        resvg::usvg::Tree::from_str(svg, &opt).map_err(|e| RenderError::Parse(e.to_string()))?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w as u32, h as u32).ok_or(RenderError::Alloc)?;

    let size = tree.size();
    let transform =
        resvg::tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // tiny-skia hands back premultiplied bytes; the rest of this crate works in
    // straight alpha, and undoing it here keeps that invariant in one place.
    let mut data = vec![0.0f32; w * h * 4];
    for (i, px) in pixmap.pixels().iter().enumerate() {
        let a = px.alpha() as f32 / 255.0;
        let inv = if a > 0.0 { 1.0 / (a * 255.0) } else { 0.0 };
        data[i * 4] = px.red() as f32 * inv;
        data[i * 4 + 1] = px.green() as f32 * inv;
        data[i * 4 + 2] = px.blue() as f32 * inv;
        data[i * 4 + 3] = a;
    }
    Ok(Rgba {
        width: w,
        height: h,
        data,
    })
}

/// Disagreement with the input, on an 8-bit scale, where the trace is flat.
///
/// Both images are composited onto white first, so a transparent input and an
/// SVG that draws nothing there agree instead of differing by whatever the PNG
/// happened to store under alpha 0.
///
/// **The normalisation is `sum / 9n`, not the `sum / 3n` an RMS over three
/// channels would give**, so this reads 1/sqrt(3) of the true RMS. That is not a
/// bug to fix: it is the formula every threshold in this module was calibrated
/// against (`h22`, `h26`), and the reference implementation in
/// `tools/inkvec_sr/clean.py` divides by three twice for the same reason.
/// Correcting it without recalibrating [`DEGRADED_RESIDUAL`] would move every
/// clean icon across the threshold -- which is exactly what it did when this was
/// first written the "right" way.
///
/// Returns `None` when the trace has too little flat area to judge on -- the
/// caller should treat that as "cannot tell" rather than as "clean".
pub fn interior_residual(input: &Rgba, model: &Rgba) -> Option<f64> {
    if input.width != model.width || input.height != model.height {
        return None;
    }
    let (a, b) = (clean::on_white(input), clean::on_white(model));

    // Mask the *composited* model, not the raw one. In straight alpha the colour
    // channels are constant across an anti-aliased edge while only alpha moves,
    // so a mask built on them calls every boundary pixel flat -- and boundary
    // pixels are exactly where a trace and its input disagree. Compositing first
    // makes the edge visible to the mask, which is what the reference does.
    let flat_src = Rgba {
        width: model.width,
        height: model.height,
        data: b.iter().flat_map(|p| [p[0], p[1], p[2], 1.0]).collect(),
    };
    let mask = clean::flat_mask(&flat_src, 1.5 / 255.0);
    let n = mask.iter().filter(|&&m| m).count();
    if n < 100 {
        return None;
    }
    let mut acc = 0.0f64;
    for (i, &m) in mask.iter().enumerate() {
        if !m {
            continue;
        }
        for c in 0..3 {
            let d = (b[i][c] - a[i][c]) as f64 * 255.0;
            acc += d * d;
        }
    }
    Some((acc / (n as f64 * 9.0)).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, v: f32) -> Rgba {
        Rgba {
            width: w,
            height: h,
            data: (0..w * h).flat_map(|_| [v, v, v, 1.0]).collect(),
        }
    }

    #[test]
    fn identical_images_have_no_residual() {
        let a = solid(32, 32, 0.5);
        assert!(interior_residual(&a, &a).unwrap() < 1e-9);
    }

    #[test]
    fn a_known_offset_matches_the_reference_scale() {
        // A uniform 4-level offset. The true RMS would be 4.0; this metric
        // carries the reference implementation's extra factor of three, so it
        // must read 4/sqrt(3). Pinning the constant here is what stops someone
        // "fixing" the normalisation and silently decalibrating the threshold.
        let a = solid(32, 32, 0.5);
        let b = solid(32, 32, 0.5 + 4.0 / 255.0);
        let r = interior_residual(&a, &b).unwrap();
        let want = 4.0 / 3.0f64.sqrt();
        assert!((r - want).abs() < 0.05, "expected ~{want:.3}, got {r}");
    }

    #[test]
    fn renders_a_trivial_svg() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" width="10" height="10"><rect width="10" height="10" fill="#000000"/></svg>"##;
        let img = render_svg(svg, 16, 16).expect("should render");
        assert_eq!((img.width, img.height), (16, 16));
        let p = img.pixel(8, 8);
        assert!(
            p[3] > 0.99 && p[0] < 0.01,
            "expected opaque black, got {p:?}"
        );
    }
}
