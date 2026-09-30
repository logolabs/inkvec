//! Tests of the alpha-ramp fit's list form against the whole-image scan it replaced
//! (2026-09-30), and of the skip rule `ramp_candidate`: bit-for-bit equality on random and
//! degenerate label maps, and "skipped means `None`" checked against the scan itself; and
//! of `face_stats`, the run-based first pass, against the per-pixel loop it replaced.

use super::*;

/// The whole-image-scan form of [`fit_alpha_ramp`], as it was before 2026-09-30: it finds
/// the face's interior pixels itself by visiting every pixel. Kept as the reference the
/// tests hold the list form to; see [`fit_alpha_ramp`] for the method. `labels`, `alpha`
/// and `rgb` are `w · h` row-major; `rgb` is the matted image composited over white.
#[allow(clippy::too_many_arguments)]
pub(super) fn fit_alpha_ramp_scan(
    face: usize,
    labels: &[u16],
    alpha: &[f32],
    rgb: &[[f32; 3]],
    matte: [f32; 3],
    w: usize,
    h: usize,
) -> Option<AlphaRamp> {
    const MIN_FADE: f32 = RAMP_MIN_FADE;
    const MAX_RESIDUAL: f32 = RAMP_MAX_RESIDUAL;
    const MIN_INTERIOR: usize = RAMP_MIN_INTERIOR;

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

/// A tiny deterministic generator (xorshift64*), so the tests need no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// An opaque image of `w x h` random 8-bit colours.
fn image(rng: &mut Rng, w: usize, h: usize) -> inkvec_trace::Rgba {
    let mut data = Vec::with_capacity(w * h * 4);
    for _ in 0..w * h {
        for _ in 0..3 {
            data.push(rng.below(256) as f32 / 255.0);
        }
        data.push(1.0);
    }
    inkvec_trace::Rgba {
        width: w,
        height: h,
        data,
    }
}

/// Two fits are the same bit for bit.
fn same(a: Option<AlphaRamp>, b: Option<AlphaRamp>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.p0.x.to_bits() == b.p0.x.to_bits()
                && a.p0.y.to_bits() == b.p0.y.to_bits()
                && a.p1.x.to_bits() == b.p1.x.to_bits()
                && a.p1.y.to_bits() == b.p1.y.to_bits()
                && a.a0.to_bits() == b.a0.to_bits()
                && a.a1.to_bits() == b.a1.to_bits()
                && a.color
                    .iter()
                    .zip(b.color)
                    .all(|(x, y)| x.to_bits() == y.to_bits())
        }
        _ => false,
    }
}

/// For every face of `labels`: the list form equals the scan, and every face the skip
/// rule rejects really is `None` under the scan. Returns how many faces gave a ramp.
fn check(labels: &[u16], alpha: &[f32], img: &inkvec_trace::Rgba, n_faces: usize) -> usize {
    let (w, h) = (img.width, img.height);
    let matte = [1.0, 1.0, 1.0];
    let rgb = img.composited([1.0, 1.0, 1.0]);
    let want = vec![true; n_faces];
    // Pass-1 facts, computed independently of `face_alpha`.
    let (mut in_n, mut faded) = (vec![0usize; n_faces], vec![false; n_faces]);
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let i = y * w + x;
            let l = labels[i];
            if [i - 1, i + 1, i - w, i + w].iter().all(|&j| labels[j] == l) {
                in_n[l as usize] += 1;
                faded[l as usize] |= alpha[i] != 1.0;
            }
        }
    }
    let lists = interior_pixels(labels, w, h, &want, &in_n);
    let mut found = 0;
    for f in 0..n_faces {
        assert_eq!(lists[f].len(), in_n[f], "face {f}: interior count");
        let old = fit_alpha_ramp_scan(f, labels, alpha, &rgb, matte, w, h);
        let new = fit_alpha_ramp(&lists[f], alpha, img, matte, w);
        assert!(same(old, new), "face {f}: {old:?} vs {new:?}");
        if !ramp_candidate(in_n[f], faded[f]) {
            assert!(old.is_none(), "face {f} skipped but the scan fits {old:?}");
        }
        found += usize::from(new.is_some());
    }
    found
}

#[test]
fn the_list_fit_equals_the_scan_on_random_maps() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut ramps = 0;
    for _ in 0..80 {
        let (w, h) = (1 + rng.below(48) as usize, 1 + rng.below(48) as usize);
        let n_faces = 1 + rng.below(4) as usize;
        // Blocky labels, so faces have interiors: a random face per 6x6 block.
        let bw = w.div_ceil(6);
        let block: Vec<u16> = (0..bw * h.div_ceil(6))
            .map(|_| rng.below(n_faces as u64) as u16)
            .collect();
        let labels: Vec<u16> = (0..w * h)
            .map(|p| block[(p / w / 6) * bw + (p % w) / 6])
            .collect();
        // Per face: opaque, a linear fade along a random axis, or noise.
        let kind: Vec<u64> = (0..n_faces).map(|_| rng.below(3)).collect();
        let (gx, gy) = (rng.unit() * 0.05, rng.unit() * 0.05);
        let alpha: Vec<f32> = (0..w * h)
            .map(|p| {
                let (x, y) = ((p % w) as f32, (p / w) as f32);
                match kind[labels[p] as usize] {
                    0 => 1.0,
                    1 => (0.1 + gx * x + gy * y).min(1.0),
                    _ => rng.unit(),
                }
            })
            .collect();
        let img = image(&mut rng, w, h);
        ramps += check(&labels, &alpha, &img, n_faces);
    }
    assert!(ramps > 0, "no case exercised an accepted ramp");
}

#[test]
fn the_list_fit_equals_the_scan_on_degenerate_maps() {
    let mut rng = Rng(7);
    // One pixel, one row, one column, and a square; on each, one face everywhere
    // (opaque and fading), a checkerboard (no interior at all), and a face that is the
    // three-pixel ring along the image border.
    let shapes: [(usize, usize); 4] = [(1, 1), (37, 1), (1, 37), (30, 30)];
    for (w, h) in shapes {
        let img = image(&mut rng, w, h);
        let one = vec![0u16; w * h];
        check(&one, &vec![1.0; w * h], &img, 1);
        let fade: Vec<f32> = (0..w * h).map(|p| 0.1 + 0.02 * (p % w) as f32).collect();
        check(&one, &fade, &img, 1);
        let checker: Vec<u16> = (0..w * h).map(|p| (((p % w) + p / w) % 2) as u16).collect();
        check(&checker, &fade, &img, 2);
        let ring: Vec<u16> = (0..w * h)
            .map(|p| {
                let (x, y) = (p % w, p / w);
                u16::from(x < 3 || y < 3 || x + 3 >= w || y + 3 >= h)
            })
            .collect();
        check(&ring, &fade, &img, 2);
    }
    // A clean fade over the whole face is found by both.
    let (w, h) = (30, 30);
    let img = image(&mut rng, w, h);
    let fade: Vec<f32> = (0..w * h).map(|p| 0.1 + 0.03 * (p % w) as f32).collect();
    assert_eq!(check(&vec![0u16; w * h], &fade, &img, 1), 1);
}

/// `face_alpha`'s first pass as a per-pixel loop, the form [`face_stats`] replaced: the
/// reference its tests hold it to.
fn face_stats_scan(labels: &[u16], alpha: &[f32], w: usize, h: usize, n_faces: usize) -> FaceStats {
    let mut st = FaceStats {
        a_sum: vec![0.0; n_faces],
        a_n: vec![0; n_faces],
        in_sum: vec![0.0; n_faces],
        in_sq: vec![0.0; n_faces],
        in_n: vec![0; n_faces],
        in_faded: vec![false; n_faces],
    };
    for (i, &l) in labels.iter().enumerate() {
        let f = l as usize;
        if f >= n_faces {
            continue;
        }
        let a32 = alpha.get(i).copied().unwrap_or(1.0);
        let a = a32 as f64;
        st.a_sum[f] += a;
        st.a_n[f] += 1;
        let (x, y) = (i % w, i / w);
        let interior = x > 0
            && y > 0
            && x + 1 < w
            && y + 1 < h
            && labels[i - 1] == l
            && labels[i + 1] == l
            && labels[i - w] == l
            && labels[i + w] == l;
        if interior {
            st.in_sum[f] += a;
            st.in_sq[f] += a * a;
            st.in_n[f] += 1;
            st.in_faded[f] |= a32 != 1.0;
        }
    }
    st
}

/// Two sets of statistics are the same bit for bit.
fn same_stats(a: &FaceStats, b: &FaceStats) -> bool {
    let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    bits(&a.a_sum) == bits(&b.a_sum)
        && bits(&a.in_sum) == bits(&b.in_sum)
        && bits(&a.in_sq) == bits(&b.in_sq)
        && a.a_n == b.a_n
        && a.in_n == b.in_n
        && a.in_faded == b.in_faded
}

#[test]
fn face_statistics_from_runs_equal_the_pixel_loop() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let shapes = [
        (1usize, 1usize),
        (1, 9),
        (9, 1),
        (2, 2),
        (3, 3),
        (17, 11),
        (64, 48),
    ];
    for (w, h) in shapes {
        for case in 0..6 {
            let n_faces = 1 + rng.below(4) as usize;
            let bw = w.div_ceil(4);
            let block: Vec<u16> = (0..bw * h.div_ceil(3))
                .map(|_| rng.below(n_faces as u64 + 1) as u16)
                .collect();
            // Blocks, noise, one label, a checkerboard; label `n_faces` is out of range.
            let labels: Vec<u16> = (0..w * h)
                .map(|p| match case {
                    0 | 1 => block[(p / w / 3) * bw + (p % w) / 4],
                    2 => rng.below(n_faces as u64 + 1) as u16,
                    3 => 0,
                    _ => ((p % w + p / w) % 2) as u16,
                })
                .collect();
            // Clear, opaque, 8-bit levels, and signed zeros.
            let alpha: Vec<f32> = (0..w * h)
                .map(|_| match rng.below(5) {
                    0 => 0.0,
                    1 => -0.0,
                    2 => 1.0,
                    _ => rng.below(256) as f32 / 255.0,
                })
                .collect();
            let old = face_stats_scan(&labels, &alpha, w, h, n_faces);
            let new = face_stats(&labels, &alpha, w, h, n_faces);
            assert!(same_stats(&old, &new), "{w}x{h} case {case}");
            // A short alpha channel reads as opaque past its end, in both.
            let short = &alpha[..alpha.len() / 2];
            let old = face_stats_scan(&labels, short, w, h, n_faces);
            let new = face_stats(&labels, short, w, h, n_faces);
            assert!(same_stats(&old, &new), "{w}x{h} case {case}, short alpha");
        }
    }
}
