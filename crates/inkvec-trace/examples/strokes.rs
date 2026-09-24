//! Does `centerline::analyse` recover the strokes an artist drew?
//!
//! 28% of the corpus (451 of 1583 icons) is stroke art: lucide is 175 of 175,
//! openmoji 143 of 148, fluent-emoji 132 of 175. The tracer converts all of it to
//! filled outlines, which is why lucide costs 3.29x the artist's parameter count
//! while every other family sits between 0.83 and 1.42. `centerline.rs` exists to
//! fix exactly that and has never been called from anything.
//!
//! This runs it and reports, per icon, what it found against what the artist
//! wrote. It is a measurement, not a feature: nothing here changes the output.
//!
//!   cargo run --release -p inkvec-trace --example strokes -- <png>...

use std::path::Path;

use inkvec_fit::FitConfig;
use inkvec_trace::{centerline, coverage, load_image};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: strokes <png>...");
        std::process::exit(2);
    }

    println!(
        "  {:<34} {:>7} {:>8} {:>7} {:>8} {:>7} {:>9}",
        "icon", "strokes", "cover", "width", "w sigma", "closed", "params"
    );
    println!("  {}", "-".repeat(86));

    let (mut tot_strokes, mut tot_cover, mut n) = (0usize, 0.0f64, 0usize);
    let mut tot_params = 0.0f64;
    for a in &args {
        let p = Path::new(a);
        let img = match load_image(p) {
            Ok(i) => i,
            Err(e) => {
                println!("  {:<34} load failed: {e}", short(p));
                continue;
            }
        };
        let t = std::time::Instant::now();
        let cov = coverage::bilevel_coverage(&img);
        let labels = centerline::bilevel_labels(&cov);
        let criteria = if std::env::var_os("STROKES_GRAPH").is_some() {
            centerline::GRAPH_CRITERIA
        } else {
            centerline::Criteria::default()
        };
        let an = centerline::analyse_with(&cov, &labels, img.width, img.height, criteria);
        let ms = t.elapsed().as_secs_f64() * 1e3;

        // The artist's line weight is one number, so the spread of recovered
        // widths says whether this really is one stroked drawing.
        let widths: Vec<f64> = an.strokes.iter().map(|s| s.width).collect();
        let (wmed, wsig) = if widths.is_empty() {
            (f64::NAN, f64::NAN)
        } else {
            let mut v = widths.clone();
            v.sort_by(f64::total_cmp);
            let med = v[v.len() / 2];
            let sig = an
                .strokes
                .iter()
                .map(|s| s.width_sigma)
                .fold(0.0f64, f64::max);
            (med, sig)
        };
        let closed = an.strokes.iter().filter(|s| s.closed).count();
        // What the strokes would cost to state: the fitted centreline plus one
        // number for the width. This is the whole point -- the artist wrote one
        // path and one scalar, and the filled outline restates it as two sides.
        let extent = img.width.max(img.height) as f64;
        let cfg = FitConfig::from_precision(extent, 0.1, 2.0);
        let sparams: f64 = an.strokes.iter().map(|s| s.params(&cfg)).sum();
        println!(
            "  {:<34} {:>7} {:>7.1}% {:>7.2} {:>8.3} {:>7} {:>9} {:>7.0}ms",
            short(p),
            an.strokes.len(),
            an.stroke_fraction * 100.0,
            wmed,
            wsig,
            closed,
            format!("{sparams:.0}"),
            ms
        );
        tot_params += sparams;
        tot_strokes += an.strokes.len();
        tot_cover += an.stroke_fraction;
        n += 1;
    }

    if n > 0 {
        println!("  {}", "-".repeat(86));
        println!(
            "  {} icons, {} strokes total, mean coverage {:.1}%, {:.0} stroke params",
            n,
            tot_strokes,
            100.0 * tot_cover / n as f64,
            tot_params
        );
        println!(
            "\n  `cover` is the share of inked area the recovered strokes explain. Near\n  \
             100% on line art means the drawing really is strokes and the module found\n  \
             them; a low number means it did not, and the outline path is still right."
        );
    }
}

fn short(p: &Path) -> String {
    let f = p.file_stem().map(|s| s.to_string_lossy().to_string());
    f.unwrap_or_else(|| p.display().to_string())
        .chars()
        .take(34)
        .collect()
}
