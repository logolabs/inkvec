//! Evidence of damage of the input's own, which `--restore auto` and `--sr auto` need before
//! they act, besides the probe trace's residual.
//!
//! # Why the residual alone is not enough
//!
//! Both pre-passes decide `auto` on one signal: how far the input sits from its own probe
//! trace where the trace claims to be flat ([`inkvec_sr::detect::interior_residual`], above
//! `--restore-threshold` / `--sr-threshold`). Smooth shading in clean, gradient-heavy art
//! reads as that residual too. Measured with `--restore auto` on the gate's opaque 512 px tier
//! (`quality-512ssop`, PNG renders, 2026-10-10): the restorer ran on 7 of 246 clean icons, all
//! Noto emoji with shading, and the condition read dE00 +4.49 % and geom +13.9 %
//! (`emoji_u1f5a8` geom 0.19 -> 3.05, `emoji_u1f469_1f3fb_200d_1f52c` dE00 0.22 -> 0.53).
//!
//! So `auto` acts only when the pixels or the file say the input was damaged, independently
//! of the residual:
//!
//! * the restorer, which undoes compression ([`compressed`]): a lossy container (`--lossy`,
//!   resolved from the file's first bytes), or JPEG ringing around the edges
//!   ([`inkvec_trace::coverage::ringing_score`] above the gate the palette's noise guard
//!   uses), which is how a JPEG re-saved as PNG is told;
//! * the upscaler, which undoes resampling as well ([`soft`]): either of those, or edges
//!   wider than a native render's (`coverage::intake_scale` above
//!   [`inkvec_trace::color::SOFT_INTAKE_EDGE`]).
//!
//! These are the three signals that open the palette's soft intake, and a clean render
//! passes none of them (`color::SOFT_RINGING`'s and `SOFT_INTAKE_EDGE`'s measurements over
//! the corpus).
//!
//! # Order
//!
//! The evidence is asked first and the residual only when there is some. The residual is
//! the costly half: it renders the probe trace at full size. Measured on the gate's
//! transparent 512 px tier in Fast mode (246 icons, 2026-10-10), `auto` asking the residual
//! first cost 16.5 ms of CPU per icon on a 31 ms trace (the render and the residual), and the
//! ringing score 5.5 ms more where the residual fired. Both conditions are needed, so the
//! order changes no decision, only what a clean input pays: a lossy container is a flag, and
//! a lossless one costs the ringing score alone.

use inkvec_trace::{color, coverage, Rgba};

/// Why the input looks compressed, or `None` when nothing says so: `lossy` is the resolved
/// `--lossy` (`On` for a lossy container; `Auto` when the caller never resolved it, which
/// reads as "not known"), and the ringing is measured on the image over white.
pub(crate) fn compressed(img: &Rgba, lossy: inkvec_sr::Mode) -> Option<&'static str> {
    if lossy == inkvec_sr::Mode::On {
        return Some("lossy container");
    }
    let rgb = img.composited([1.0, 1.0, 1.0]);
    let gate = if img.width.min(img.height) >= color::RINGING_MIN_DIM {
        color::SOFT_RINGING_LARGE
    } else {
        color::SOFT_RINGING
    };
    (coverage::ringing_score(&rgb, img.width, img.height) > gate).then_some("ringing")
}

/// Why the input looks compressed or resampled, or `None`: [`compressed`], or edges wider
/// than a native render's.
pub(crate) fn soft(img: &Rgba, lossy: inkvec_sr::Mode) -> Option<&'static str> {
    compressed(img, lossy).or_else(|| {
        let rgb = img.composited([1.0, 1.0, 1.0]);
        (coverage::intake_scale(&rgb, img.width, img.height) > color::SOFT_INTAKE_EDGE)
            .then_some("wide edges")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat two-colour image: no ringing, one-pixel edges.
    fn clean(w: usize, h: usize) -> Rgba {
        let mut data = Vec::with_capacity(w * h * 4);
        for p in 0..w * h {
            let v = if (p % w) < w / 2 { 0.1 } else { 0.9 };
            data.extend_from_slice(&[v, v, v, 1.0]);
        }
        Rgba {
            width: w,
            height: h,
            data,
        }
    }

    #[test]
    fn a_clean_render_shows_no_damage_and_a_lossy_container_does() {
        let img = clean(64, 64);
        assert_eq!(compressed(&img, inkvec_sr::Mode::Off), None);
        assert_eq!(compressed(&img, inkvec_sr::Mode::Auto), None);
        assert_eq!(soft(&img, inkvec_sr::Mode::Off), None);
        assert_eq!(
            compressed(&img, inkvec_sr::Mode::On),
            Some("lossy container")
        );
        assert_eq!(soft(&img, inkvec_sr::Mode::On), Some("lossy container"));
    }
}
