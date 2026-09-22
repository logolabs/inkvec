//! Measure how editable a set of SVGs is: `cargo run --release -p inkvec-svgmin --example
//! structure -- a.svg b.svg ...`, or a list of paths on stdin; with `--json` first, each file's counts as JSON.
//!
//! Prints the pooled ratios over every file, and the median of the per-file ratios, which is
//! what the Studio's structure card quotes for hand-drawn files.

use inkvec_svgmin::{structure, Structure};

fn ratio(n: usize, d: usize) -> Option<f64> {
    (d > 0).then(|| n as f64 / d as f64)
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    match v.len() {
        0 => f64::NAN,
        n if n % 2 == 1 => v[n / 2],
        n => (v[n / 2 - 1] + v[n / 2]) / 2.0,
    }
}

fn main() {
    let mut total = Structure::default();
    let (mut axis, mut smooth, mut aligned) = (Vec::new(), Vec::new(), Vec::new());
    let mut files = 0usize;
    // Paths as arguments, or one per line on stdin for sets too long for a command line.
    let mut paths: Vec<String> = std::env::args().skip(1).collect();
    // `--json` prints one file's counts as the JSON the apps read, and stops.
    if paths.first().map(String::as_str) == Some("--json") {
        for path in &paths[1..] {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            println!("{}", structure(&text).to_json());
        }
        return;
    }
    if paths.is_empty() {
        paths = std::io::stdin().lines().map_while(Result::ok).collect();
    }
    for path in paths {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let s = structure(&text);
        if s.nodes == 0 {
            continue;
        }
        files += 1;
        total.nodes += s.nodes;
        total.cubics += s.cubics;
        total.handles += s.handles;
        total.axis_handles += s.axis_handles;
        total.joins += s.joins;
        total.smooth_joins += s.smooth_joins;
        total.aligned_nodes += s.aligned_nodes;
        axis.extend(ratio(s.axis_handles, s.handles));
        smooth.extend(ratio(s.smooth_joins, s.joins));
        aligned.extend(ratio(s.aligned_nodes, s.nodes));
    }
    println!("files            {files}");
    println!("nodes            {}", total.nodes);
    println!("cubics           {}", total.cubics);
    println!(
        "axis handles     pooled {:.3}   median per file {:.3}   ({} files with handles)",
        ratio(total.axis_handles, total.handles).unwrap_or(f64::NAN),
        median(axis.clone()),
        axis.len()
    );
    println!(
        "smooth joins     pooled {:.3}   median per file {:.3}   ({} files with joins)",
        ratio(total.smooth_joins, total.joins).unwrap_or(f64::NAN),
        median(smooth.clone()),
        smooth.len()
    );
    println!(
        "aligned nodes    pooled {:.3}   median per file {:.3}",
        ratio(total.aligned_nodes, total.nodes).unwrap_or(f64::NAN),
        median(aligned)
    );
}
