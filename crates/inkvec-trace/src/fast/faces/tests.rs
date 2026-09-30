use super::reference_tests as reference;
use super::*;

/// The shape tests' pixels: each pixel exactly its ink's colour, opaque.
fn ink_rgb(labels: &[u16], inks: &[[f32; 4]]) -> Vec<[f32; 3]> {
    labels
        .iter()
        .map(|&l| {
            let c = inks[l as usize];
            [c[0], c[1], c[2]]
        })
        .collect()
}

#[test]
fn faces_match_the_flood_fill() {
    let (w, h) = (7, 5);
    let labels: Vec<u16> = (0..w * h)
        .map(|p| {
            let (x, y) = (p % w, p / w);
            u16::from((x + y) % 3 == 0 || (x == 3 && y > 0))
        })
        .collect();
    let (a, fa) = faces(&labels, w, h);
    let (b, fb) = crate::regions::split_components(&labels, w, h);
    assert_eq!(fa.len(), fb.len());
    // The same partition: two pixels share a face in one exactly when in the other.
    for p in 0..w * h {
        for q in 0..w * h {
            assert_eq!(a[p] == a[q], b[p] == b[q]);
        }
    }
}

#[test]
fn a_lone_pixel_takes_its_surroundings() {
    let mut labels = vec![0u16, 0, 0, 0, 1, 0, 0, 0, 2];
    despeckle(&mut labels, 3, 3, 2);
    assert_eq!(labels, vec![0, 0, 0, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn a_rim_of_a_blend_ink_goes_to_the_inks_either_side_and_a_hairline_stays() {
    // White, a one-pixel strip of light grey (ink 2), black, white, a black hairline
    // (ink 1), white.
    let (w, h) = (8, 18);
    let inks = [
        [1.0f32, 1.0, 1.0, 1.0],
        [0.0, 0.0, 0.0, 1.0],
        [0.6, 0.6, 0.6, 1.0],
    ];
    let mut labels = vec![0u16; w * h];
    for x in 0..w {
        labels[4 * w + x] = 2;
        for y in 5..9 {
            labels[y * w + x] = 1;
        }
        labels[13 * w + x] = 1;
    }
    let rgb = ink_rgb(&labels, &inks);
    let px = Pixels {
        rgb: &rgb,
        alpha: None,
    };
    absorb_slivers(&mut labels, px, &inks, w, h);
    assert!(
        (0..w).all(|x| labels[4 * w + x] == 0),
        "the grey strip is a blend"
    );
    assert!(
        (0..w).all(|x| labels[13 * w + x] == 1),
        "the hairline stays"
    );
}

#[test]
fn the_grey_rim_of_a_thin_stroke_goes_to_the_stroke_and_the_paper() {
    // White paper, a light grey rim row, a two-pixel black stroke (no interior, so
    // `absorb_slivers` cannot use it), another rim row, paper. Rim pixels a little
    // darker than the rim ink go to the stroke side, the rest to the paper.
    let (w, h) = (10, 8);
    let inks = [
        [1.0f32, 1.0, 1.0, 1.0],
        [0.0, 0.0, 0.0, 1.0],
        [0.93, 0.93, 0.93, 1.0],
    ];
    let mut labels = vec![0u16; w * h];
    let mut rgb = vec![[1.0f32; 3]; w * h];
    for x in 0..w {
        labels[2 * w + x] = 2;
        labels[5 * w + x] = 2;
        rgb[2 * w + x] = [0.93; 3];
        rgb[5 * w + x] = if x < 5 { [0.3; 3] } else { [0.93; 3] };
        for y in 3..5 {
            labels[y * w + x] = 1;
            rgb[y * w + x] = [0.0; 3];
        }
    }
    let px = Pixels {
        rgb: &rgb,
        alpha: None,
    };
    let mut hairline = labels.clone();
    absorb_rims(&mut labels, px, &inks, w, h);
    assert!(
        (0..w).all(|x| labels[2 * w + x] == 0),
        "the rim above is paper"
    );
    assert!(
        (0..5).all(|x| labels[5 * w + x] == 1),
        "darker rim pixels join the stroke"
    );
    assert!((5..w).all(|x| labels[5 * w + x] == 0));
    assert!(
        (0..w).all(|x| (3..5).all(|y| labels[y * w + x] == 1)),
        "the stroke stays"
    );
    // Without the rims, the black stroke on white is no blend of anything and stays.
    for x in 0..w {
        hairline[2 * w + x] = 0;
        hairline[5 * w + x] = 0;
    }
    let before = hairline.clone();
    absorb_rims(&mut hairline, px, &inks, w, h);
    assert_eq!(hairline, before);
}

#[test]
fn two_inks_one_eye_cannot_tell_apart_become_one_and_a_step_stays() {
    // Left half ink 0, right half ink 1 (1 dE00 away), and a bar of ink 2 (a clearly
    // different grey) across the bottom.
    let (w, h) = (12, 9);
    let inks = [
        [0.02f32, 0.02, 0.02, 1.0],
        [0.035, 0.035, 0.03, 1.0],
        [0.5, 0.5, 0.5, 1.0],
    ];
    assert!(
        crate::color::de00([0.02, 0.02, 0.02], [0.035, 0.035, 0.03]) < crate::color::SAME_INK_DE00
    );
    let mut labels: Vec<u16> = (0..w * h)
        .map(|p| if p / w >= 7 { 2 } else { u16::from(p % w >= 5) })
        .collect();
    merge_same_inks(&mut labels, &inks, w, h);
    // The larger of the two near-blacks (ink 1, 7 columns) takes the other.
    assert!(labels[..7 * w].iter().all(|&l| l == 1));
    assert!(labels[7 * w..].iter().all(|&l| l == 2));
}

#[test]
fn a_u_shape_is_one_component() {
    // 1 1 . 1
    // 1 . . 1
    // 1 1 1 1
    let labels = vec![1u16, 1, 0, 1, 1, 0, 0, 1, 1, 1, 1, 1];
    let c = components(&labels, 4, 3);
    assert_eq!(c.size.len(), 2);
    assert_eq!(c.comp[0], c.comp[3]);
}

// ---------------------------------------------------------------- the oracle

/// xorshift64*: a small deterministic generator for the random cases.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// One random clean-up input: labels, pixel colours, optional opacity, inks.
struct Case {
    w: usize,
    h: usize,
    labels: Vec<u16>,
    rgb: Vec<[f32; 3]>,
    alpha: Option<Vec<f32>>,
    inks: Vec<[f32; 4]>,
}

/// Inks that exercise every pass: a few free colours, a near-twin of the first (the
/// same-ink merge), and a midpoint of two others (the rims). Opaque unless `alpha`.
fn random_inks(r: &mut Rng, alpha: bool) -> Vec<[f32; 4]> {
    let k = 2 + r.below(4);
    let mut inks: Vec<[f32; 4]> = (0..k)
        .map(|_| {
            let a = if alpha && r.below(3) == 0 {
                r.unit()
            } else {
                1.0
            };
            [r.unit(), r.unit(), r.unit(), a]
        })
        .collect();
    let a = inks[0];
    inks.push([a[0] + 0.004, a[1] + 0.003, a[2], a[3]]);
    let (b, c) = (inks[1], inks[r.below(k)]);
    inks.push(std::array::from_fn(|i| 0.5 * (b[i] + c[i])));
    inks
}

/// A label image in one of several shapes: blocks with one- and two-pixel strips drawn
/// across them, pure noise, a checkerboard, stripes, or one ink.
fn random_labels(r: &mut Rng, w: usize, h: usize, n_inks: usize) -> Vec<u16> {
    let n = w * h;
    let pick = |r: &mut Rng| r.below(n_inks) as u16;
    match r.below(6) {
        0 => (0..n).map(|_| pick(r)).collect(),
        1 => (0..n).map(|p| ((p % w + p / w) % 2) as u16).collect(),
        2 => {
            let s = 1 + r.below(3);
            (0..n).map(|p| ((p / w / s) % n_inks) as u16).collect()
        }
        3 => vec![pick(r); n],
        _ => {
            let mut l = vec![pick(r); n];
            for _ in 0..1 + r.below(6) {
                let (x0, y0) = (r.below(w), r.below(h));
                let (x1, y1) = (x0 + 1 + r.below(w), y0 + 1 + r.below(h));
                let v = pick(r);
                for y in y0..y1.min(h) {
                    for x in x0..x1.min(w) {
                        l[y * w + x] = v;
                    }
                }
            }
            for _ in 0..r.below(4) {
                let v = pick(r);
                let thick = 1 + r.below(2);
                if r.below(2) == 0 {
                    let y0 = r.below(h);
                    for y in y0..(y0 + thick).min(h) {
                        l[y * w..(y + 1) * w].fill(v);
                    }
                } else {
                    let x0 = r.below(w);
                    for y in 0..h {
                        for x in x0..(x0 + thick).min(w) {
                            l[y * w + x] = v;
                        }
                    }
                }
            }
            for _ in 0..r.below(1 + n / 16) {
                l[r.below(n)] = pick(r);
            }
            l
        }
    }
}

fn random_case(r: &mut Rng, w: usize, h: usize) -> Case {
    let alpha = r.below(3) == 0;
    let inks = random_inks(r, alpha);
    let labels = random_labels(r, w, h, inks.len());
    // Each pixel its ink, a blend of its ink and another, or a colour of its own.
    let cols: Vec<[f32; 4]> = labels
        .iter()
        .map(|&l| {
            let a = inks[l as usize];
            match r.below(4) {
                0 => a,
                1 => {
                    let b = inks[r.below(inks.len())];
                    let t = r.unit();
                    let e = 0.03 * (r.unit() - 0.5);
                    std::array::from_fn(|k| a[k] + t * (b[k] - a[k]) + e)
                }
                2 => [r.unit(), r.unit(), r.unit(), a[3]],
                _ => std::array::from_fn(|k| a[k] + 0.02 * (r.unit() - 0.5)),
            }
        })
        .collect();
    Case {
        w,
        h,
        labels,
        rgb: cols.iter().map(|c| [c[0], c[1], c[2]]).collect(),
        alpha: alpha.then(|| cols.iter().map(|c| c[3]).collect()),
        inks,
    }
}

/// Every pass of the rewrite against the shipped code, one pass at a time, then the faces.
fn check_case(c: &Case, min_size: usize, what: &str) {
    let (w, h) = (c.w, c.h);
    let px = Pixels {
        rgb: &c.rgb,
        alpha: c.alpha.as_deref(),
    };
    let full: Vec<[f32; 4]> = (0..w * h).map(|p| px.get(p)).collect();
    let mut want = c.labels.clone();
    let mut got = c.labels.clone();
    reference::absorb_slivers(&mut want, &full, &c.inks, w, h);
    absorb_slivers(&mut got, px, &c.inks, w, h);
    assert_eq!(got, want, "{what}: slivers");
    reference::absorb_rims(&mut want, &full, &c.inks, w, h);
    absorb_rims(&mut got, px, &c.inks, w, h);
    assert_eq!(got, want, "{what}: rims");
    reference::merge_same_inks(&mut want, &c.inks, w, h);
    merge_same_inks(&mut got, &c.inks, w, h);
    assert_eq!(got, want, "{what}: same-ink merge");
    reference::despeckle(&mut want, w, h, min_size);
    despeckle(&mut got, w, h, min_size);
    assert_eq!(got, want, "{what}: despeckle to {min_size}");
    assert_eq!(
        faces(&got, w, h),
        reference::faces(&want, w, h),
        "{what}: faces"
    );
}

#[test]
fn the_clean_up_equals_the_shipped_passes_on_random_and_degenerate_images() {
    let sizes = [
        (1, 1),
        (1, 9),
        (9, 1),
        (2, 2),
        (3, 3),
        (5, 4),
        (4, 7),
        (16, 9),
        (33, 17),
        (40, 40),
    ];
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..400 {
        let (w, h) = sizes[round % sizes.len()];
        let c = random_case(&mut r, w, h);
        let min_size = [0, 1, 2, 3, 4, 16][r.below(6)];
        check_case(&c, min_size, &format!("round {round}, {w}x{h}"));
    }
}

// ---------------------------------------------------------------- the research dumps

/// A label image the fast front end dumped after its palette (`INKVEC_FACESDUMP`, research
/// build): w, h, ink count (u32 LE), inks (`[f32; 4]`), labels (u16), composited sRGB
/// (`[f32; 3]`), a native-alpha flag byte and, when set, the opacity (f32). Its `.out`
/// holds what the shipped stage made of it: face count (u32), each face's ink (u32) and a
/// face id per pixel (u16).
fn load_dump(path: &std::path::Path) -> (Case, Option<(Vec<usize>, Vec<u16>)>) {
    let b = std::fs::read(path).expect("dump");
    let u32at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let f32at = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let (w, h, k) = (u32at(&b, 0), u32at(&b, 4), u32at(&b, 8));
    let n = w * h;
    let inks: Vec<[f32; 4]> = (0..k)
        .map(|i| std::array::from_fn(|c| f32at(12 + 16 * i + 4 * c)))
        .collect();
    let mut o = 12 + 16 * k;
    let labels: Vec<u16> = (0..n)
        .map(|i| u16::from_le_bytes([b[o + 2 * i], b[o + 2 * i + 1]]))
        .collect();
    o += 2 * n;
    let rgb: Vec<[f32; 3]> = (0..n)
        .map(|i| std::array::from_fn(|c| f32at(o + 12 * i + 4 * c)))
        .collect();
    o += 12 * n;
    let alpha = (b[o] != 0).then(|| (0..n).map(|i| f32at(o + 1 + 4 * i)).collect());
    let out = std::fs::read(path.with_extension("bin.out")).ok().map(|b| {
        let nf = u32at(&b, 0);
        let fc = (0..nf).map(|i| u32at(&b, 4 + 4 * i)).collect();
        let o = 4 + 4 * nf;
        let ids = (0..n)
            .map(|i| u16::from_le_bytes([b[o + 2 * i], b[o + 2 * i + 1]]))
            .collect();
        (fc, ids)
    });
    let case = Case {
        w,
        h,
        labels,
        rgb,
        alpha,
        inks,
    };
    (case, out)
}

/// The differential test over real label images: every `*.bin` in the directory named by
/// `INKVEC_FACES_DUMPS`, pass by pass against the shipped code, at the speckle floor the
/// front end would use for `--min-area 2` and at 1, 4 and 16. The reference's own faces
/// are also checked against the dump's `.out` where the floor reproduces it.
#[test]
#[ignore = "needs INKVEC_FACES_DUMPS, a directory of research dumps"]
fn the_clean_up_equals_the_shipped_passes_on_the_research_dumps() {
    let Some(dir) = inkvec_core::env::path("INKVEC_FACES_DUMPS") else {
        return;
    };
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("dump directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "bin"))
        .collect();
    files.sort();
    let (mut n, mut out_same) = (0, 0);
    for f in &files {
        let (c, out) = load_dump(f);
        let floor = (4.0 * (c.w * c.h) as f64 / (512.0 * 512.0))
            .min(16.0)
            .round()
            .max(2.0) as usize;
        let what = f.display().to_string();
        for m in [floor, 1, 4, 16] {
            check_case(&c, m, &what);
        }
        if let Some((fc, ids)) = out {
            let px = Pixels {
                rgb: &c.rgb,
                alpha: c.alpha.as_deref(),
            };
            let full: Vec<[f32; 4]> = (0..c.w * c.h).map(|p| px.get(p)).collect();
            let mut l = c.labels.clone();
            let (rid, rfc) = reference::stage(&mut l, &full, &c.inks, c.w, c.h, floor);
            out_same += usize::from(rid == ids && rfc == fc);
        }
        n += 1;
    }
    eprintln!("{n} dumps equal pass by pass; the reference reproduces {out_same} .out files");
    assert!(n > 0, "no dumps in {}", dir.display());
}
