//! Tests of the band merge: the common-pixel score, the strided union, the work cap and
//! the merge itself on small synthetic label maps.

use super::*;

#[test]
fn common_score_ignores_incomparable_cached_residuals() {
    let (w, h) = (12, 8);
    let rgb = vec![[0.4; 3]; w * h];
    let group: Vec<u32> = (0..w * h).map(|p| u32::from(p % w >= 6)).collect();
    let pixels: Vec<_> = (0..w * h).collect();
    let pure = vec![true; w * h];
    let left = flat_only([0.4; 3], 2.0);
    let mut right = left.clone();
    right.cost = 1e12; // Measured on another population; must not enter comparison.
    right.chi2 = 1e12;
    let gain = common_pixel_gain(
        &rgb,
        w,
        h,
        &pixels,
        &group,
        &|p: usize| pure[p],
        0,
        1,
        &left,
        &right,
        &left,
        1.0 / 255.0,
        2.0,
    )
    .unwrap();
    assert!((gain - 2.0 * PARAMS_FLAT).abs() < 1e-6);
}

#[test]
fn common_score_rejects_erasing_a_real_color_step() {
    let (w, h) = (12, 8);
    let group: Vec<u32> = (0..w * h).map(|p| u32::from(p % w >= 6)).collect();
    let rgb: Vec<_> = group
        .iter()
        .map(|&g| if g == 0 { [0.2; 3] } else { [0.8; 3] })
        .collect();
    let pixels: Vec<_> = (0..w * h).collect();
    let pure = vec![true; w * h];
    let gain = common_pixel_gain(
        &rgb,
        w,
        h,
        &pixels,
        &group,
        &|p: usize| pure[p],
        0,
        1,
        &flat_only([0.2; 3], 2.0),
        &flat_only([0.8; 3], 2.0),
        &flat_only([0.5; 3], 2.0),
        1.0 / 255.0,
        2.0,
    )
    .unwrap();
    assert!(gain < -1000.0);
}

#[test]
fn common_score_declines_without_evidence() {
    let model = flat_only([0.0; 3], 1.0);
    assert!(common_pixel_gain(
        &[[0.0; 3]; 16],
        4,
        4,
        &(0..16).collect::<Vec<_>>(),
        &[0; 16],
        &|_: usize| false,
        0,
        1,
        &model,
        &model,
        &model,
        0.01,
        1.0
    )
    .is_none());
}

#[test]
fn strided_union_takes_the_concatenations_stride_without_building_it() {
    for &(la, lb) in &[
        (0usize, 0usize),
        (5, 0),
        (0, 7),
        (3, 4),
        (FIT_PIXELS_CAP, 0),
        (FIT_PIXELS_CAP, 1),
        (1, FIT_PIXELS_CAP),
        (40_000, 40_000),
        (1_260_000, 9),
        (7, 200_001),
    ] {
        let a: Vec<usize> = (0..la).map(|i| 3 * i + 1).collect();
        let b: Vec<usize> = (0..lb).map(|i| 5 * i + 2).collect();
        let concat: Vec<usize> = a.iter().chain(&b).copied().collect();
        let want: Vec<usize> = if concat.len() > FIT_PIXELS_CAP {
            concat
                .iter()
                .copied()
                .step_by(concat.len() / FIT_PIXELS_CAP)
                .collect()
        } else {
            concat.clone()
        };
        assert_eq!(&strided_union([&a, &b])[..], &want[..], "{la} + {lb}");
    }
}

/// `union_work`'s gathered count is the length of what `strided_union` gathers, for
/// every size class (under, at and over the cap, and far over it).
#[test]
fn union_work_charges_what_a_fit_gathers() {
    for n in [
        0usize,
        1,
        4095,
        4096,
        4097,
        FIT_PIXELS_CAP - 1,
        FIT_PIXELS_CAP,
        FIT_PIXELS_CAP + 1,
        2 * FIT_PIXELS_CAP - 1,
        2 * FIT_PIXELS_CAP,
        1_260_000,
        4_194_304,
    ] {
        let a: Vec<usize> = (0..n).collect();
        let gathered = strided_union([&a, &[]]).len();
        let want = gathered as u64 + MODEL_WORK_PER_SAMPLE * gathered.min(MAX_FIT_SAMPLES) as u64;
        assert_eq!(union_work(n), want, "{n}");
    }
}

/// The cap is a strict upper bound, a refused charge spends nothing, and the clock is
/// not read without a deadline.
#[test]
fn the_merge_budget_charges_up_to_its_cap() {
    let mut b = MergeBudget::new(10, 10, None);
    b.cap = 100;
    assert!(b.charge(60));
    assert!(b.charge(40), "exactly the cap is allowed");
    assert!(!b.charge(1), "one over is refused");
    assert_eq!((b.spent, b.stop), (100, Some(MergeStop::Work)));
    assert!(!b.out_of_time(), "no deadline, no clock");
    let mut late = MergeBudget::new(10, 10, Some(inkvec_core::clock::Instant::now()));
    assert!(late.out_of_time());
    assert_eq!(late.stop, Some(MergeStop::Clock));
    // Saturating, so an enormous image cannot wrap the cap round to a small number.
    assert_eq!(MergeBudget::new(usize::MAX, usize::MAX, None).cap, u64::MAX);
    // Work a self-bounded step already did is added even past the cap, and the next
    // charge, of any size, is refused and records the stop.
    let mut over = MergeBudget::new(10, 10, None);
    over.cap = 100;
    over.charge_spent(150);
    assert_eq!((over.spent, over.stop), (150, None));
    assert!(!over.charge(0));
    assert_eq!(over.stop, Some(MergeStop::Work));
}

/// A ramp in thin flat bands, which region recovery joins into one gradient: the
/// default cap does not bind (the result is the unbounded result), a cap of zero stops
/// the loop before its first wave (every band stays a separate flat fill, a correct
/// trace), and so does a deadline that has already passed.
#[test]
fn the_merge_stops_at_the_cap_and_otherwise_is_unchanged() {
    let (w, h, band) = (40usize, 20usize, 2usize);
    let q8 = |v: f32| (v * 255.0).round() / 255.0;
    let ramp = |x: usize| 0.25 + 0.5 * x as f32 / (w - 1) as f32;
    let rgb: Vec<[f32; 3]> = (0..w * h).map(|p| [q8(ramp(p % w)); 3]).collect();
    let inks: Vec<[f32; 3]> = (0..w / band)
        .map(|b| [q8(ramp(b * band) * 0.5 + ramp(b * band + 1) * 0.5); 3])
        .collect();
    let pal = Palette {
        colors: inks
            .iter()
            .map(|&c| crate::color::rgb_to_oklab(c))
            .collect(),
        weight: vec![1.0; inks.len()],
        alpha: vec![1.0; inks.len()],
        rgb: inks,
    };
    let base: Vec<u16> = (0..w * h).map(|p| ((p % w) / band) as u16).collect();
    let (sigma, lambda) = (0.5 / 255.0, bic_lambda(w * h));
    let spend = |budget: MergeBudget| {
        let mut labels = base.clone();
        let (fills, ink, left) = merge_bands_budgeted(
            &mut labels,
            &rgb,
            w,
            h,
            &pal,
            sigma,
            lambda,
            None,
            true,
            budget,
        );
        ((labels, format!("{fills:?}"), ink), left)
    };
    let run = |budget: MergeBudget| spend(budget).0;
    let capped = |cap: u64| MergeBudget {
        cap,
        spent: 0,
        deadline: None,
        stop: None,
    };
    let (unbounded, all) = spend(capped(u64::MAX));
    assert_eq!(all.stop, None, "the unbounded loop ends by itself");
    assert_eq!(run(MergeBudget::new(w, h, None)), unbounded);
    // The cap is strict and every unit charged is spent: a cap of exactly what the whole
    // loop spent changes nothing, one unit less stops it early, with a different result.
    assert_eq!(run(capped(all.spent)), unbounded);
    let (short, left) = spend(capped(all.spent - 1));
    assert_eq!(left.stop, Some(MergeStop::Work));
    assert_ne!(short, unbounded);
    let first = unbounded.0[0];
    assert!(unbounded.0.iter().all(|&l| l == first), "one region");
    let mut none = MergeBudget::new(w, h, None);
    none.cap = 0;
    let stopped = run(none);
    let past = run(MergeBudget::new(
        w,
        h,
        Some(inkvec_core::clock::Instant::now()),
    ));
    assert_eq!(stopped, past, "both stop before the first wave");
    let mut kinds: Vec<u16> = stopped.0.clone();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(kinds.len(), w / band, "every band stays its own region");
}

#[test]
fn test_merge_gradient_bands_flat_regions() {
    let w = 4;
    let h = 4;
    let rgb = vec![[0.0f32, 0.0, 0.0]; w * h];
    let mut labels = vec![0u16; w * h];
    let pal = Palette {
        colors: vec![crate::color::rgb_to_oklab([0.0, 0.0, 0.0])],
        rgb: vec![[0.0, 0.0, 0.0]],
        weight: vec![1.0],
        alpha: vec![1.0],
    };
    let fits = merge_gradient_bands(&mut labels, &rgb, w, h, &pal, 1.0 / 255.0, 1.0);
    assert!(!fits.is_empty());
}
