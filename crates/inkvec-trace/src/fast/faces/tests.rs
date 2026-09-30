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
    let mut a = labels.clone();
    let fa = RunLabels::new(&labels, w, h).write_faces(&mut a);
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
    let labels = vec![0u16, 0, 0, 0, 1, 0, 0, 0, 2];
    let mut runs = RunLabels::new(&labels, 3, 3);
    runs.despeckle(2);
    assert_eq!(runs.to_labels(), vec![0, 0, 0, 0, 0, 0, 0, 0, 0]);
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
    let mut runs = RunLabels::new(&labels, w, h);
    runs.absorb_slivers(px, &inks);
    let labels = runs.to_labels();
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
    let mut runs = RunLabels::new(&labels, w, h);
    runs.absorb_rims(px, &inks);
    let labels = runs.to_labels();
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
    let mut runs = RunLabels::new(&hairline, w, h);
    runs.absorb_rims(px, &inks);
    assert_eq!(runs.to_labels(), hairline);
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
    let labels: Vec<u16> = (0..w * h)
        .map(|p| if p / w >= 7 { 2 } else { u16::from(p % w >= 5) })
        .collect();
    let mut runs = RunLabels::new(&labels, w, h);
    runs.merge_same_inks(&inks);
    let labels = runs.to_labels();
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
    let mut runs = RunLabels::new(&labels, 4, 3);
    runs.components();
    assert_eq!(runs.size, vec![9, 3]);
    // The runs of pixel 0 (row 0, x 0..2) and pixel 3 (row 0, x 3..4).
    assert_eq!(runs.run_comp[0], runs.run_comp[2]);
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
    // One run image through every pass, as in `fast::front`.
    let mut runs = RunLabels::new(&c.labels, w, h);
    runs.assert_canonical();
    assert_eq!(runs.to_labels(), want, "{what}: runs");
    let rc = reference::components(&want, w, h);
    runs.components();
    assert_eq!(runs.size, rc.size, "{what}: component sizes");
    assert_eq!(runs.label, rc.label, "{what}: component labels");
    assert_eq!(
        runs.interiors(),
        reference::interiors(&rc, w, h),
        "{what}: interiors"
    );
    reference::absorb_slivers(&mut want, &full, &c.inks, w, h);
    runs.absorb_slivers(px, &c.inks);
    runs.assert_canonical();
    assert_eq!(runs.to_labels(), want, "{what}: slivers");
    reference::absorb_rims(&mut want, &full, &c.inks, w, h);
    runs.absorb_rims(px, &c.inks);
    runs.assert_canonical();
    assert_eq!(runs.to_labels(), want, "{what}: rims");
    reference::merge_same_inks(&mut want, &c.inks, w, h);
    runs.merge_same_inks(&c.inks);
    runs.assert_canonical();
    assert_eq!(runs.to_labels(), want, "{what}: same-ink merge");
    reference::despeckle(&mut want, w, h, min_size);
    runs.despeckle(min_size);
    runs.assert_canonical();
    assert_eq!(runs.to_labels(), want, "{what}: despeckle to {min_size}");
    let (want_ids, want_inks) = reference::faces(&want, w, h);
    // The face ids overwrite whatever the buffer held, as over the palette's labels.
    let mut got = c.labels.clone();
    let got_inks = runs.write_faces(&mut got);
    assert_eq!(got_inks, want_inks, "{what}: face inks");
    assert_eq!(got, want_ids, "{what}: face ids");
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
        (67, 5),
        (130, 3),
    ];
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..400 {
        let (w, h) = sizes[round % sizes.len()];
        let c = random_case(&mut r, w, h);
        let min_size = [0, 1, 2, 3, 4, 16][r.below(6)];
        check_case(&c, min_size, &format!("round {round}, {w}x{h}"));
    }
}

#[test]
fn empty_images_have_no_runs_no_components_and_no_faces() {
    for (w, h) in [(0, 0), (0, 3), (3, 0)] {
        let inks = [[0.0f32, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]];
        let px = Pixels {
            rgb: &[],
            alpha: None,
        };
        let mut runs = RunLabels::new(&[], w, h);
        runs.assert_canonical();
        runs.absorb_slivers(px, &inks);
        runs.absorb_rims(px, &inks);
        runs.merge_same_inks(&inks);
        runs.despeckle(4);
        assert!(runs.write_faces(&mut []).is_empty(), "{w}x{h}");
    }
}

#[test]
fn long_runs_are_found_to_the_pixel_across_vector_blocks() {
    // Runs whose ends fall at every offset of the 8-label blocks the scan compares.
    let w = 70;
    for cut in 0..w {
        for tail in [0u16, 1] {
            let row: Vec<u16> = (0..w).map(|x| if x < cut { 0 } else { 1 + tail }).collect();
            let runs = RunLabels::new(&row, w, 1);
            runs.assert_canonical();
            assert_eq!(runs.to_labels(), row, "cut at {cut}");
            assert_eq!(runs.runs.len(), 1 + usize::from(cut > 0 && cut < w));
        }
    }
}

// ---------------------------------------------------------------- the research dumps

/// What the shipped stage made of a dump: each face's ink, and a face id per pixel.
type ShippedFaces = (Vec<usize>, Vec<u16>);

/// A label image the fast front end dumped after its palette (`INKVEC_FACESDUMP`, research
/// build): w, h, ink count (u32 LE), inks (`[f32; 4]`), labels (u16), composited sRGB
/// (`[f32; 3]`), a native-alpha flag byte and, when set, the opacity (f32). Its `.out`
/// holds what the shipped stage made of it: face count (u32), each face's ink (u32) and a
/// face id per pixel (u16).
fn load_dump(path: &std::path::Path) -> (Case, Option<ShippedFaces>) {
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

/// Per-pass wall time of the run-based clean-up against the shipped per-pixel code on
/// every research dump of at least `INKVEC_FACES_BENCH_MIN` pixels (default 1: all), best
/// of 5 runs each, printed as one line per dump. A measurement, not a check.
#[test]
#[ignore = "timing; needs INKVEC_FACES_DUMPS"]
fn time_the_passes_on_the_research_dumps() {
    use std::time::Instant;
    let Some(dir) = inkvec_core::env::path("INKVEC_FACES_DUMPS") else {
        return;
    };
    let min_px = inkvec_core::env::count("INKVEC_FACES_BENCH_MIN").unwrap_or(1);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("dump directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "bin"))
        .collect();
    files.sort();
    let best = |f: &mut dyn FnMut() -> f64| (0..5).map(|_| f()).fold(f64::MAX, f64::min);
    eprintln!("dump w h runs | new slivers rims merge despeckle faces total | reference");
    for f in &files {
        let (c, _) = load_dump(f);
        if c.w * c.h < min_px {
            continue;
        }
        let floor = (4.0 * (c.w * c.h) as f64 / (512.0 * 512.0))
            .min(16.0)
            .round()
            .max(2.0) as usize;
        let px = Pixels {
            rgb: &c.rgb,
            alpha: c.alpha.as_deref(),
        };
        let mut t = [0.0f64; 6];
        let mut n_runs = 0;
        for _ in 0..5 {
            let mut lap = [0.0f64; 6];
            let mut out = c.labels.clone();
            let s = Instant::now();
            let mut runs = RunLabels::new(&c.labels, c.w, c.h);
            lap[0] = s.elapsed().as_secs_f64();
            n_runs = runs.runs.len();
            runs.absorb_slivers(px, &c.inks);
            lap[1] = s.elapsed().as_secs_f64();
            runs.absorb_rims(px, &c.inks);
            lap[2] = s.elapsed().as_secs_f64();
            runs.merge_same_inks(&c.inks);
            lap[3] = s.elapsed().as_secs_f64();
            runs.despeckle(floor);
            lap[4] = s.elapsed().as_secs_f64();
            std::hint::black_box(runs.write_faces(&mut out));
            lap[5] = s.elapsed().as_secs_f64();
            if t[5] == 0.0 || lap[5] < t[5] {
                t = lap;
            }
        }
        let full: Vec<[f32; 4]> = (0..c.w * c.h).map(|p| px.get(p)).collect();
        let reference_s = best(&mut || {
            let mut l = c.labels.clone();
            let s = Instant::now();
            std::hint::black_box(reference::stage(&mut l, &full, &c.inks, c.w, c.h, floor));
            s.elapsed().as_secs_f64()
        });
        let ms = |a: f64| a * 1e3;
        eprintln!(
            "{} {} {} {} | {:.3} {:.3} {:.3} {:.3} {:.3} {:.3} {:.3} | {:.3}",
            f.file_stem().unwrap().to_string_lossy(),
            c.w,
            c.h,
            n_runs,
            ms(t[0]),
            ms(t[1] - t[0]),
            ms(t[2] - t[1]),
            ms(t[3] - t[2]),
            ms(t[4] - t[3]),
            ms(t[5] - t[4]),
            ms(t[5]),
            ms(reference_s)
        );
    }
}
