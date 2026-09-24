//! `cargo run -p inkvec-fab --example fab -- IN.svg OUT_DIR MODE WIDTH_MM`
//!
//! Prepare one SVG and write the preview and every sheet, with the checks and timings.
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let svg = std::fs::read_to_string(&a[1]).expect("read");
    let out = std::path::Path::new(&a[2]);
    std::fs::create_dir_all(out).expect("mkdir");
    let mode = match a.get(3).map(String::as_str) {
        Some("layered") => inkvec_fab::Mode::Layered,
        Some("inlay") => inkvec_fab::Mode::Inlay,
        Some("sticker") => inkvec_fab::Mode::Sticker,
        Some("stencil") => inkvec_fab::Mode::Stencil,
        Some("lines") => inkvec_fab::Mode::Lines,
        _ => inkvec_fab::Mode::SingleColour,
    };
    let width_mm = a.get(4).and_then(|w| w.parse().ok()).unwrap_or(100.0);
    let t = Instant::now();
    let an = inkvec_fab::analyze(&svg).expect("analyze");
    println!(
        "analyze {:.0} ms: {} items, {} nodes",
        t.elapsed().as_secs_f64() * 1e3,
        an.items,
        an.nodes
    );
    for c in &an.colours {
        println!(
            "  {} {:5.1}%{}{}",
            c.hex,
            100.0 * c.coverage,
            if c.background { " background" } else { "" },
            if c.gradient { " gradient" } else { "" }
        );
    }
    let o = inkvec_fab::Options {
        mode,
        width_mm,
        max_line_mm: std::env::var("FAB_MAX_LINE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0),
        dogbone_mm: std::env::var("FAB_DOGBONE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0),
        size_check_mm: std::env::var("FAB_SIZE_CHECK")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0),
        ..Default::default()
    };
    let t = Instant::now();
    let p = inkvec_fab::prepare(&svg, &o).expect("prepare");
    println!(
        "prepare {:.0} ms, {:.1} x {:.1} mm",
        t.elapsed().as_secs_f64() * 1e3,
        p.size_mm[0],
        p.size_mm[1]
    );
    std::fs::write(out.join("preview.svg"), &p.preview_svg).expect("write");
    std::fs::write(out.join("plan.dxf"), &p.dxf).expect("write");
    for (i, l) in p.layers.iter().enumerate() {
        println!(
            "  layer {i} {} {}: {} nodes, {} parts, {:.0} mm2",
            l.name, l.hex, l.nodes, l.parts, l.area_mm2
        );
        std::fs::write(out.join(format!("layer{i}.svg")), &l.svg).expect("write");
    }
    for c in &p.checks {
        println!("  [{:?}] {}", c.level, c.message);
    }
    // `FAB_JSON=1`: also write the analysis and plan as JSON, the shapes the Studio sees.
    if std::env::var_os("FAB_JSON").is_some() {
        std::fs::write(
            out.join("analysis.json"),
            serde_json::to_string(&an).expect("json"),
        )
        .expect("write");
        std::fs::write(
            out.join("plan.json"),
            serde_json::to_string(&p).expect("json"),
        )
        .expect("write");
    }
}
