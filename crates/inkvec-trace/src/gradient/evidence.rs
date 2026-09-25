//! Which pixels testify about a region's fill, and which are blends towards a neighbour.

/// Which pixels are evidence for their region's *fill*, as opposed to blends of that
/// region's ink with a neighbouring one.
///
/// After blend absorption every anti-aliased pixel carries the label of the ink it is
/// mostly made of, so the label map cannot say which pixels are pure. The fill fitter
/// needs to know: a wedge tip is nothing but blends, and fitted as evidence those pixels
/// buy a radial gradient on a solid black shape (luanti: black lightening to #6d6d6d
/// over the last tenth of a slab; ebox: three gradients on solid black). A pixel is a
/// blend when its colour lies on the segment from its own ink to the ink of another
/// label found within two pixels of it, within the noise. Pure pixels - within the noise
/// of their ink, or off every such segment - remain evidence, so a genuine gradient,
/// whose colours are not on a line towards a *neighbour's* ink, keeps its samples.
pub(crate) fn fill_evidence(
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    labels: &[u16],
    ink: &[[f32; 3]],
    sigma_noise: f64,
) -> Vec<bool> {
    blend_partners(rgb, w, h, labels, ink, sigma_noise, false)
        .into_iter()
        .map(|q| q[0] == PURE)
        .collect()
}

/// [`blend_partners`]' mark for a pixel that is evidence for its own fill, and for an
/// unused partner slot.
pub(crate) const PURE: u32 = u32::MAX;
/// [`blend_partners`]' mark for a pixel with more blend partners than it records.
pub(crate) const FOREIGN: u32 = u32::MAX - 1;
/// Most blend partners recorded per pixel.
pub(crate) const PARTNERS: usize = 3;

/// [`fill_evidence`], saying for each blend pixel *which* pixels' inks it is a blend
/// towards: `[PURE; _]` for evidence, otherwise the indices of the neighbouring pixels
/// (one per neighbouring label) whose label's ink the pixel lies on the segment to --
/// only the first unless `all` -- and [`FOREIGN`] when there are more than [`PARTNERS`].
///
/// A fit of several bands of one quantised ramp wants this. Every pixel of a ramp between
/// two band inks lies on the segment between them, so the plain test calls half of each
/// band a blend and leaves only the pixels next to the inks: the union then sees two
/// clusters of colour, which the ramp-or-step test reads as a step. A blend towards a
/// region that is itself part of the fit is not a blend with anything foreign, and is
/// evidence for the fit.
pub(crate) fn blend_partners(
    rgb: &[[f32; 3]],
    w: usize,
    h: usize,
    labels: &[u16],
    ink: &[[f32; 3]],
    sigma_noise: f64,
    all: bool,
) -> Vec<[u32; PARTNERS]> {
    let n = w * h;
    let tol = (3.0 * sigma_noise * 3f64.sqrt()).max(2.0 / 255.0) as f32;
    let tol2 = tol * tol;
    let mut pure = vec![[PURE; PARTNERS]; n];
    for p in 0..n {
        let l = labels[p] as usize;
        if l >= ink.len() {
            continue;
        }
        let c = rgb[p];
        let a = ink[l];
        let da = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        if da[0] * da[0] + da[1] * da[1] + da[2] * da[2] <= tol2 {
            continue; // within the noise of its own ink
        }
        let (x, y) = (p % w, p / w);
        // Two pixels: the blend fringe after absorption is up to two pixels deep on a
        // diagonal. One pixel was measured worse (twemoji flag +0.07, emoji_u1f923
        // +0.04) and did not recover the thin-hair case it was meant for.
        let (x0, x1) = (x.saturating_sub(2), (x + 2).min(w - 1));
        let (y0, y1) = (y.saturating_sub(2), (y + 2).min(h - 1));
        let mut seen: [usize; 8] = [usize::MAX; 8];
        let mut n_seen = 0usize;
        let mut blend = [PURE; PARTNERS];
        let mut k = 0usize;
        'scan: for yy in y0..=y1 {
            for xx in x0..=x1 {
                let m = labels[yy * w + xx] as usize;
                if m == l || m >= ink.len() || seen[..n_seen].contains(&m) {
                    continue;
                }
                if n_seen < seen.len() {
                    seen[n_seen] = m;
                    n_seen += 1;
                }
                let b = ink[m];
                let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
                if len2 <= 1e-9 {
                    continue;
                }
                let t = (da[0] * ab[0] + da[1] * ab[1] + da[2] * ab[2]) / len2;
                if t <= 0.0 || t >= 1.0 {
                    continue;
                }
                let perp = [da[0] - t * ab[0], da[1] - t * ab[1], da[2] - t * ab[2]];
                if perp[0] * perp[0] + perp[1] * perp[1] + perp[2] * perp[2] <= tol2 {
                    if k == PARTNERS {
                        blend = [FOREIGN; PARTNERS];
                        break 'scan;
                    }
                    blend[k] = (yy * w + xx) as u32;
                    k += 1;
                    if !all {
                        break 'scan;
                    }
                }
            }
        }
        pure[p] = blend;
    }
    if std::env::var_os("INKVEC_EVDBG").is_some() {
        let mut per: std::collections::BTreeMap<usize, (usize, usize)> = Default::default();
        for p in 0..n {
            let e = per.entry(labels[p] as usize).or_insert((0, 0));
            e.0 += 1;
            if pure[p][0] != PURE {
                e.1 += 1;
            }
        }
        for (l, (tot, bl)) in per {
            eprintln!(
                "  [ev] label {l} ink {:?} pixels {tot} blends {bl}",
                ink.get(l).map(|c| [
                    (c[0] * 255.0) as u8,
                    (c[1] * 255.0) as u8,
                    (c[2] * 255.0) as u8
                ])
            );
        }
    }
    pure
}
