//! Controlled image-formation experiment, not an end-to-end tracing benchmark.
//! Run: cargo run -p inkvec-fit --release --example pareto_probe
//! The reference is a disk with known Gaussian blur and correlated edge noise.
//! Candidates are disks and sinusoidally perturbed disks; rates are declared
//! parameter counts, not SVG byte counts. Budgets use the known injected noise.
use inkvec_fit::pareto::{frontier, select, Budget, Score};
use std::f64::consts::TAU;

const N: usize = 64;

fn render(radius: f64, amplitude: f64, frequency: usize) -> Vec<f64> {
    (0..N * N)
        .map(|i| {
            let mut coverage = 0.0;
            for sy in 0..4 {
                for sx in 0..4 {
                    let x = (i % N) as f64 + (sx as f64 + 0.5) / 4.0 - 32.0;
                    let y = (i / N) as f64 + (sy as f64 + 0.5) / 4.0 - 32.0;
                    let boundary = radius + amplitude * (frequency as f64 * y.atan2(x)).sin();
                    coverage += f64::from(x.hypot(y) <= boundary) / 16.0;
                }
            }
            coverage
        })
        .collect()
}

fn blur(image: &[f64]) -> Vec<f64> {
    let mut weights: Vec<_> = (-4..=4)
        .map(|k| (-0.5 * (k as f64).powi(2)).exp())
        .collect();
    let sum: f64 = weights.iter().sum();
    weights.iter_mut().for_each(|w| *w /= sum);
    let mut tmp = vec![0.0; N * N];
    let mut out = vec![0.0; N * N];
    convolve(image, &mut tmp, &weights, false);
    convolve(&tmp, &mut out, &weights, true);
    out
}

fn convolve(src: &[f64], dst: &mut [f64], weights: &[f64], vertical: bool) {
    for y in 0..N {
        for x in 0..N {
            dst[y * N + x] = weights
                .iter()
                .enumerate()
                .map(|(j, w)| {
                    let k = j as isize - 4;
                    let xx = (x as isize + if vertical { 0 } else { k }).clamp(0, N as isize - 1)
                        as usize;
                    let yy = (y as isize + if vertical { k } else { 0 }).clamp(0, N as isize - 1)
                        as usize;
                    w * src[yy * N + xx]
                })
                .sum();
        }
    }
}

fn mse(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f64>() / a.len() as f64
}

// Sherman-Morrison inverse of covariance σ0² I + σn² vvᵀ, rescaled to
// MSE units so the demonstration's same coordinate price can be used.
// v is a KNOWN synthetic error mode, not learned from the winning candidate.
fn correlated_mse(a: &[f64], b: &[f64], mode: &[f64]) -> f64 {
    let rss: f64 = a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum();
    let projection: f64 = a
        .iter()
        .zip(b)
        .zip(mode)
        .map(|((x, y), v)| (x - y) * v)
        .sum();
    let mode_norm: f64 = mode.iter().map(|v| v * v).sum();
    let variance_ratio = (1.0_f64 / 255.0 / 0.025).powi(2);
    ((rss - projection.powi(2) / (mode_norm + variance_ratio)) / a.len() as f64).max(0.0)
}

// Integral of squared curvature derivative, with evidence length fixed at 1 px.
// Evaluated on the analytic smooth polar curve, independently of raster samples.
fn wobble(radius: f64, amplitude: f64, frequency: usize) -> f64 {
    let n = 2048;
    let dt = TAU / n as f64;
    let samples: Vec<_> = (0..n)
        .map(|i| {
            let phase = frequency as f64 * i as f64 * dt;
            let r = radius + amplitude * phase.sin();
            let dr = amplitude * frequency as f64 * phase.cos();
            let ddr = -amplitude * (frequency as f64).powi(2) * phase.sin();
            let speed = r.hypot(dr);
            ((r * r + 2.0 * dr * dr - r * ddr) / speed.powi(3), speed)
        })
        .collect();
    (0..n)
        .map(|i| {
            let (k, speed) = samples[i];
            let (next_k, next_speed) = samples[(i + 1) % n];
            (next_k - k).powi(2) / (dt * (speed + next_speed) * 0.5)
        })
        .sum()
}

fn run_case(name: &str, true_amplitude: f64, true_frequency: usize) {
    let clean = render(18.0, true_amplitude, true_frequency);
    let blurred = blur(&clean);
    let mode: Vec<_> = (0..N * N)
        .map(|i| {
            let x = (i % N) as f64 + 0.5 - 32.0;
            let y = (i / N) as f64 + 0.5 - 32.0;
            (16.0 * y.atan2(x)).sin() * (-0.5 * (x.hypot(y) - 18.0).powi(2)).exp()
        })
        .collect();
    let observed: Vec<_> = blurred
        .iter()
        .enumerate()
        .map(|(i, value)| (value + 0.025 * mode[i]).clamp(0.0, 1.0))
        .collect();
    let noise_mse = mse(&observed, &blurred);
    let mut scores = Vec::new();
    let mut raw = Vec::new();
    let mut truth = Vec::new();
    let mut correlated = Vec::new();
    let mut labels = Vec::new();
    for radius_step in -4..=4 {
        let radius = 18.0 + radius_step as f64 * 0.05;
        for frequency in [0, 8, 16, 24] {
            for amplitude_step in 0..=8 {
                if (frequency == 0) != (amplitude_step == 0) {
                    continue;
                }
                let amplitude = amplitude_step as f64 * 0.025;
                let image = render(radius, amplitude, frequency);
                let forward = blur(&image);
                let score = Score {
                    distortion: mse(&forward, &observed),
                    rate: if frequency == 0 {
                        3
                    } else {
                        6 * frequency as u64
                    },
                    wobble: wobble(radius, amplitude, frequency),
                };
                scores.push(score);
                correlated.push(correlated_mse(&forward, &observed, &mode));
                raw.push(mse(&image, &observed));
                truth.push(mse(&image, &clean));
                labels.push(format!("r={radius:.2};a={amplitude:.3};f={frequency}"));
            }
        }
    }
    // Fixed demonstration exchange rate; not calibrated on a corpus.
    let lambda = 1e-8;
    let raw_pick = (0..scores.len())
        .min_by(|&i, &j| {
            (raw[i] + lambda * scores[i].rate as f64)
                .total_cmp(&(raw[j] + lambda * scores[j].rate as f64))
        })
        .unwrap();
    let forward_pick = (0..scores.len())
        .min_by(|&i, &j| {
            (scores[i].distortion + lambda * scores[i].rate as f64)
                .total_cmp(&(scores[j].distortion + lambda * scores[j].rate as f64))
        })
        .unwrap();
    let budget = Budget {
        distortion: 1.1 * noise_mse,
        wobble: 0.001,
    };
    let correlated_pick = (0..scores.len())
        .min_by(|&i, &j| {
            (correlated[i] + lambda * scores[i].rate as f64)
                .total_cmp(&(correlated[j] + lambda * scores[j].rate as f64))
        })
        .unwrap();
    let constrained = select(&scores, budget);
    println!("case={name}");
    println!("candidates={}, frontier={}, known_noise_mse={noise_mse:.9}, distortion_budget={:.9}, wobble_budget={}", scores.len(), frontier(&scores).len(), budget.distortion, budget.wobble);
    println!("method,candidate,parameters,raw_mse,forward_mse,wobble,truth_mse,correlated_mse");
    for (name, i) in [
        ("raw_mse_price", Some(raw_pick)),
        ("forward_mse_price", Some(forward_pick)),
        ("correlated_forward_price", Some(correlated_pick)),
        ("constrained_pareto", constrained),
    ] {
        if let Some(i) = i {
            println!(
                "{name},{},{},{:.9},{:.9},{:.9},{:.9},{:.9}",
                labels[i],
                scores[i].rate,
                raw[i],
                scores[i].distortion,
                scores[i].wobble,
                truth[i],
                correlated[i]
            );
        } else {
            println!("{name},INFEASIBLE");
        }
    }
    if let Some(i) = constrained {
        assert!(frontier(&scores).contains(&i));
    }
}

fn main() {
    run_case("smooth_disk", 0.0, 0);
    // A deliberately adverse control: real undulations must not be erased just
    // because a universal smoothness budget worked on the disk.
    run_case("genuine_undulations", 0.2, 8);
}
