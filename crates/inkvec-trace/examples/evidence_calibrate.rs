//! Every run window of an image against a known truth: the measurement harness behind the
//! renderer floor's per-image calibration (`bench/theory/floor_selfcal.py`,
//! `docs/theory/chain-boundary.md`, Phase 2).
//!
//! Reads a manifest of `name<TAB>png<TAB>truth` lines, where `truth` is the image the PNG
//! should have been with no renderer floor and no rounding: `height × width × 3` little-endian
//! `f32`, sRGB composited onto white. Each PNG is traced by the Quality front end (composited
//! onto white, as the CLI does) and its evidence built with the PNG's own alpha. One row per
//! run window goes to stdout:
//!
//! `name edge index axis line lo hi s sum quant_var truth left_low pos closed lattice z var`
//!
//! `pos` is the window's measured mean position, `truth` the sum the truth image reads on the
//! same pixels with the same two inks, `z` the fourth difference centred there in standard
//! deviations of quantisation alone (NaN where five consecutive windows are not available).
//! `var` is the window's variance with the image's self-calibrated noise. An `#edge` line per
//! edge scores the truth with the evidence's own `score`: windows, independent measurements,
//! `χ²`, `χ²` with the edge's offset, the Huber cost and the offset's variance. A `#floor` line per
//! image gives the floor, the trace's noise, edge width, soft share and ringing, the
//! calibrated extra variance (straight, curved) with its counts, the tail's `ν`, the window
//! scale and the edge noise of the first band.
//!
//! Run: `cargo run --release -p inkvec-trace --example evidence_calibrate -- manifest.tsv`

use std::io::{BufRead, Write};
use std::path::Path;

use inkvec_core::likelihood::{score, Axis, BoundaryLikelihood, Floor, RunObs};
use inkvec_core::noise::huber_kappa;
use inkvec_trace::evidence::{self, EvidenceOptions};
use inkvec_trace::{load_image, trace_color_full, ColorOptions};

fn read_truth(path: &Path, n: usize) -> Option<Vec<[f32; 3]>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() != n * 12 {
        return None;
    }
    Some(
        bytes
            .as_chunks::<12>()
            .0
            .iter()
            .map(|c| {
                let f = |k: usize| f32::from_le_bytes([c[k], c[k + 1], c[k + 2], c[k + 3]]);
                [f(0), f(4), f(8)]
            })
            .collect(),
    )
}

fn main() {
    let manifest = std::env::args()
        .nth(1)
        .expect("usage: evidence_calibrate manifest.tsv");
    let file = std::fs::File::open(&manifest).expect("manifest");
    let out = std::io::stdout();
    let mut out = out.lock();
    for line in std::io::BufReader::new(file).lines() {
        let line = line.expect("line");
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 3 {
            continue;
        }
        let (name, png, truth) = (f[0], Path::new(f[1]), Path::new(f[2]));
        let Ok(img) = load_image(png) else {
            eprintln!("skip {name}: cannot read {}", png.display());
            continue;
        };
        let n = img.width * img.height;
        let Some(truth) = read_truth(truth, n) else {
            eprintln!("skip {name}: truth has the wrong size");
            continue;
        };
        let rgb = img.composited([1.0, 1.0, 1.0]);
        let alpha: Vec<f32> = img.data.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
        let ct = trace_color_full(&img, &ColorOptions::default());
        let faces: Vec<_> = ct.face_fill.iter().map(|f| f.model.clone()).collect();
        let lossy = matches!(
            png.extension().and_then(|e| e.to_str()),
            Some("jpg" | "jpeg" | "JPG" | "JPEG")
        );
        let ramps = inkvec_trace::softness::ramp_evidence(&rgb, img.width, img.height);
        let opts = EvidenceOptions {
            floor: Some(Floor {
                lattice: 1_000_000,
                window_var: 0.0,
                edge_var: 0.0,
            }),
            lossy,
            ramp_width: ramps.width.max(1.0),
            ..EvidenceOptions::default()
        };
        let t0 = std::time::Instant::now();
        let ev =
            evidence::build_with_alpha(&ct.map, &rgb, Some(&alpha), &faces, ct.sigma_noise, &opts);
        let t_ev = t0.elapsed().as_secs_f64() * 1e3;
        let t1 = std::time::Instant::now();
        let _ = inkvec_trace::softness::ramp_evidence(&rgb, img.width, img.height);
        let t_ramp = t1.elapsed().as_secs_f64() * 1e3;
        eprintln!(
            "{name}\tevidence {t_ev:.2} ms\tramp {t_ramp:.2} ms\tedges {}",
            ct.map.edges.len()
        );
        let fl = ev.floor();
        let ringing = inkvec_trace::coverage::ringing_score(&rgb, img.width, img.height);
        let cal = ev.calibration();
        let nm = ev.noise();
        writeln!(
            out,
            "#floor\t{name}\t{}\t{:.6e}\t{:.6e}\t{:.6e}\t{:.4}\t{:.4}\t{:.4}\t{:.6e}\t{:.6e}\t{}\t{}\t{:.3}\t{:.3}\t{:.5}",
            fl.lattice,
            fl.window_var,
            fl.edge_var,
            ct.sigma_noise,
            ramps.width,
            ramps.soft_fraction,
            ringing,
            cal.extra[0],
            cal.extra[1],
            cal.count[0],
            cal.count[1],
            cal.nu,
            nm.window_scale,
            nm.sigma_edge[0]
        )
        .ok();
        for e in 0..ev.edge_count() {
            write_edge(&mut out, name, &ev, e, &truth, ct.map.edges[e].closed);
        }
    }
}

/// The fourth difference of five consecutive windows of one axis centred at `i`, in standard
/// deviations of rounding alone; NaN where they are not available.
fn fourth_z(obs: &[RunObs], qv: &[f64], i: usize) -> f64 {
    if i < 2 || i + 2 >= obs.len() {
        return f64::NAN;
    }
    let five = &obs[i - 2..=i + 2];
    let ok = five.windows(2).all(|p| {
        p[0].window.axis == p[1].window.axis && (p[1].window.line - p[0].window.line).abs() == 1
    });
    if !ok {
        return f64::NAN;
    }
    let w = [1.0, -4.0, 6.0, -4.0, 1.0];
    let d: f64 = five.iter().zip(w).map(|(o, w)| w * o.mean_position()).sum();
    let v: f64 = (0..5).map(|j| w[j] * w[j] * qv[i - 2 + j]).sum();
    d / v.sqrt()
}

/// One edge's `#edge` line (the truth scored as a candidate would be, by the trait's own
/// `score`) and its window rows.
fn write_edge(
    out: &mut impl Write,
    name: &str,
    ev: &evidence::Evidence,
    e: usize,
    truth: &[[f32; 3]],
    closed: bool,
) {
    let obs = ev.runs(e);
    if obs.is_empty() {
        return;
    }
    let tsum = ev.window_sums_of(e, truth);
    let qv = ev.quantisation_var(e);
    let r: Vec<f64> = obs.iter().zip(&tsum).map(|(o, t)| o.sum - t).collect();
    let v: Vec<f64> = obs.iter().map(|o| o.var).collect();
    let sh: Vec<f64> = obs.iter().map(|o| o.share).collect();
    let sc = score(
        &r,
        &v,
        &sh,
        ev.edge_offset_var(e),
        huber_kappa(ev.tail_nu()),
    );
    writeln!(
        out,
        "#edge\t{name}\t{e}\t{}\t{:.4}\t{:.6}\t{:.6}\t{:.6}\t{:.4e}",
        sc.m,
        sc.dof,
        sc.chi2,
        sc.chi2_floor,
        sc.cost,
        ev.edge_offset_var(e)
    )
    .ok();
    for (i, o) in obs.iter().enumerate() {
        writeln!(
            out,
            "{name}\t{e}\t{i}\t{}\t{}\t{}\t{}\t{:.4}\t{:.7}\t{:.4e}\t{:.7}\t{}\t{:.6}\t{}\t{}\t{:.4}\t{:.4e}",
            if o.window.axis == Axis::Column { "c" } else { "r" },
            o.window.line,
            o.window.lo,
            o.window.hi,
            o.s,
            o.sum,
            qv[i],
            tsum[i],
            o.left_low as u8,
            o.mean_position(),
            closed as u8,
            ev.edge_on_lattice(e) as u8,
            fourth_z(obs, qv, i),
            o.var
        )
        .ok();
    }
}
