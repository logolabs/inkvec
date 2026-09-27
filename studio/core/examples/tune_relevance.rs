//! Which Tune controls change the drawing in which engine: measured, not read off the code.
//!
//! Traces each input through the Studio's own path (`Settings` -> `to_args` -> the pipeline
//! -> the output options), once per engine at the defaults and once per non-default value
//! of every control, and prints whether the SVG bytes moved. A control "applies" to an
//! engine if any value moves any output. Some controls only matter beside another one, so
//! each variant is also tried in a few contexts (black & white on, line art on, shape
//! matching off, transparency composited).
//!
//! "Clean up damage" needs a denoiser. A stub one (a 3x3 box blur) is installed through the
//! seam the browser build uses, which is enough to see whether the setting reaches the
//! drawing; what a real network does to an image is a different question.
//!
//! ```text
//! cargo run --release -p inkvec-studio-core --example tune_relevance -- out.csv a.png b.png
//! ```

use inkvec_studio_core::options::{Cleanup, Settings, TraceMode};
use inkvec_studio_core::trace::{run_at, MeasureLevel, Outcome, Source, Tier};
use std::io::Write;
use std::sync::Arc;

/// A 3x3 box blur over each plane of the network's planar input: a stand-in denoiser.
fn blur(chw: &[f32], w: usize, h: usize) -> Result<Vec<f32>, String> {
    let mut out = chw.to_vec();
    for c in 0..3 {
        let plane = &chw[c * w * h..(c + 1) * w * h];
        for y in 0..h {
            for x in 0..w {
                let (mut s, mut n) = (0.0f32, 0.0f32);
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                        if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
                            s += plane[yy as usize * w + xx as usize];
                            n += 1.0;
                        }
                    }
                }
                out[c * w * h + y * w + x] = s / n;
            }
        }
    }
    Ok(out)
}

type Variant = (&'static str, &'static str, fn(&mut Settings));

fn variants() -> Vec<Variant> {
    vec![
        ("precision", "0.05", |s| s.precision = 0.05),
        ("precision", "0.3", |s| s.precision = 0.3),
        ("speckleFloor", "0", |s| s.speckle_floor = 0.0),
        ("speckleFloor", "16", |s| s.speckle_floor = 16.0),
        ("traceSize", "256", |s| s.trace_size = 256),
        ("timeLimit", "0.3", |s| s.time_limit = 0.3),
        ("timeLimit", "30", |s| s.time_limit = 30.0),
        ("maxColours", "4", |s| s.max_colours = 4),
        ("maxColours", "2048", |s| s.max_colours = 2048),
        ("colourMerging", "0", |s| s.colour_merging = 0.0),
        ("colourMerging", "0.12", |s| s.colour_merging = 0.12),
        ("flatFills", "toggle", |s| s.flat_fills = !s.flat_fills),
        ("blackAndWhite", "toggle", |s| {
            s.black_and_white = !s.black_and_white
        }),
        ("traceTransparency", "toggle", |s| {
            s.trace_transparency = !s.trace_transparency
        }),
        ("cleanUpDamage", "auto", |s| {
            s.clean_up_damage = Cleanup::Auto
        }),
        ("cleanUpDamage", "on", |s| s.clean_up_damage = Cleanup::On),
        ("matchRepeatedShapes", "toggle", |s| {
            s.match_repeated_shapes = !s.match_repeated_shapes
        }),
        ("matchThreshold", "0.5", |s| s.match_threshold = 0.5),
        ("matchThreshold", "1.0", |s| s.match_threshold = 1.0),
        ("fewerPaths", "toggle", |s| s.fewer_paths = !s.fewer_paths),
        ("lineArt", "toggle", |s| s.line_art = !s.line_art),
        ("repairRings", "toggle", |s| {
            s.repair_rings = !s.repair_rings
        }),
        ("editability", "toggle", |s| s.editability = !s.editability),
        ("bezierCost", "3", |s| s.bezier_cost = 3.0),
        ("bezierCost", "10", |s| s.bezier_cost = 10.0),
        ("cornerAngle", "2", |s| s.corner_angle = 2.0),
        ("cornerAngle", "40", |s| s.corner_angle = 40.0),
        ("minify", "toggle", |s| s.minify = !s.minify),
        ("transparentBackground", "toggle", |s| {
            s.transparent_background = !s.transparent_background
        }),
        ("margin", "0.1", |s| s.margin = 0.1),
        ("holesAsCutouts", "toggle", |s| {
            s.holes_as_cutouts = !s.holes_as_cutouts
        }),
    ]
}

/// A context: settings a variant is tried on top of, and which variants it is for (empty:
/// all of them). The control that sets the context is not varied in it.
type Context = (Variant, &'static [&'static str]);

fn contexts() -> Vec<Context> {
    vec![
        (("default", "", |_| {}), &[]),
        // Black & white and line art take their own routes through the pipeline, which may
        // read controls the colour route does not.
        (("blackAndWhite", "on", |s| s.black_and_white = true), &[]),
        (("lineArt", "on", |s| s.line_art = true), &[]),
        // The threshold is only read while matching is on.
        (
            ("matchRepeatedShapes", "flipped", |s| {
                s.match_repeated_shapes = !s.match_repeated_shapes
            }),
            &["matchThreshold"],
        ),
        // The output's transparency options, with the image composited on a matte.
        (
            ("traceTransparency", "flipped", |s| {
                s.trace_transparency = !s.trace_transparency
            }),
            &[
                "holesAsCutouts",
                "transparentBackground",
                "maxColours",
                "flatFills",
            ],
        ),
    ]
}

fn svg(source: &Arc<Source>, s: &Settings) -> String {
    match run_at(
        source,
        s,
        Tier::Final,
        None,
        MeasureLevel::Summary,
        |_, _| {},
    ) {
        Outcome::Traced(t) => t.svg,
        other => format!("<!-- {other:?} -->"),
    }
}

fn main() {
    inkvec_studio_core::trace::set_external_denoiser(blur);
    let mut argv = std::env::args().skip(1);
    let out_path = argv.next().expect("usage: tune_relevance OUT.csv INPUT...");
    let inputs: Vec<String> = argv.collect();
    let mut out = std::fs::File::create(&out_path).expect("output file");
    writeln!(out, "input,mode,context,control,value,changed,bytes").unwrap();
    for input in &inputs {
        let bytes = std::fs::read(input).expect("input");
        let source = Arc::new(Source::open(bytes, None).expect("decodes"));
        let name = std::path::Path::new(input)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for mode in [TraceMode::Quality, TraceMode::Fast] {
            let plain = svg(
                &source,
                &Settings {
                    mode,
                    ..Settings::default()
                },
            );
            for ((ck, cv, ctx), only) in contexts() {
                let mut base = Settings {
                    mode,
                    ..Settings::default()
                };
                ctx(&mut base);
                let reference = svg(&source, &base);
                // A context that does not engage on this image (line art that declines,
                // a matte on an opaque image) would only repeat the default context.
                if ck != "default" && reference == plain {
                    writeln!(out, "{name},{mode:?},{ck}={cv},(inactive),,false,0").unwrap();
                    continue;
                }
                // The pipeline has no clock without a time limit, so a second trace of the
                // same settings is byte-identical; checked, because everything below rests on it.
                let again = svg(&source, &base);
                let context = if cv.is_empty() {
                    ck.to_string()
                } else {
                    format!("{ck}={cv}")
                };
                writeln!(
                    out,
                    "{name},{mode:?},{context},(repeat),,{},{}",
                    reference != again,
                    reference.len()
                )
                .unwrap();
                for (key, value, apply) in variants() {
                    if key == ck || !(only.is_empty() || only.contains(&key)) {
                        continue;
                    }
                    // TUNE_ONLY=a,b narrows a follow-up run to the controls it is about.
                    if let Ok(keys) = std::env::var("TUNE_ONLY") {
                        if !keys.split(',').any(|k| k == key) {
                            continue;
                        }
                    }
                    let mut s = base.clone();
                    apply(&mut s);
                    let drawn = svg(&source, &s);
                    // TUNE_DUMP=dir keeps every drawing, for a look at what moved.
                    if let Ok(dir) = std::env::var("TUNE_DUMP") {
                        let file = format!("{name}.{mode:?}.{context}.{key}={value}.svg");
                        let _ = std::fs::write(std::path::Path::new(&dir).join(&file), &drawn);
                        let file = format!("{name}.{mode:?}.{context}.reference.svg");
                        let _ = std::fs::write(std::path::Path::new(&dir).join(&file), &reference);
                    }
                    writeln!(
                        out,
                        "{name},{mode:?},{context},{key},{value},{},{}",
                        drawn != reference,
                        drawn.len()
                    )
                    .unwrap();
                }
                out.flush().unwrap();
                eprintln!("{name} {mode:?} {context} done");
            }
        }
    }
}
