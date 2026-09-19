//! The public API's promises: valid SVG at the input's size, determinism across runs and
//! thread counts, the two input forms agreeing, and errors of the right kind.

use inkvec::{Error, Options};
use std::path::PathBuf;

fn sample(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../bindings/contract")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Parse the SVG as XML and return the root's `width` and `height` attributes.
fn svg_size(svg: &str) -> (String, String) {
    let doc = roxmltree::Document::parse(svg).expect("well-formed XML");
    let root = doc.root_element();
    assert_eq!(root.tag_name().name(), "svg");
    assert_eq!(
        root.tag_name().namespace(),
        Some("http://www.w3.org/2000/svg")
    );
    (
        root.attribute("width").expect("width").to_string(),
        root.attribute("height").expect("height").to_string(),
    )
}

/// A dark disc on a white canvas, anti-aliased by 4x4 supersampling.
fn disc_rgba(size: u32) -> Vec<u8> {
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    let (c, r) = (size as f64 / 2.0, size as f64 * 0.3);
    for y in 0..size {
        for x in 0..size {
            let mut inside = 0;
            for sy in 0..4 {
                for sx in 0..4 {
                    let (fx, fy) = (
                        x as f64 + (sx as f64 + 0.5) / 4.0,
                        y as f64 + (sy as f64 + 0.5) / 4.0,
                    );
                    if (fx - c).hypot(fy - c) < r {
                        inside += 1;
                    }
                }
            }
            let v = 255 - (inside * (255 - 30) / 16) as u8;
            px.extend_from_slice(&[v, v, (v as u16 * 3 / 4 + 60) as u8, 255]);
        }
    }
    px
}

#[test]
fn a_png_traces_to_valid_svg_at_its_own_size() {
    let t = inkvec::trace(&sample("tiny.png"), &Options::default()).unwrap();
    assert_eq!((t.width, t.height), (96, 96));
    assert_eq!(svg_size(&t.svg), ("96".into(), "96".into()));
    assert!(t.svg.contains("<path") || t.svg.contains("<circle") || t.svg.contains("<rect"));
}

#[test]
fn raw_pixels_trace_to_valid_svg() {
    let px = disc_rgba(40);
    let t = inkvec::trace_rgba(&px, 40, 40, &Options::default()).unwrap();
    assert_eq!((t.width, t.height), (40, 40));
    assert_eq!(svg_size(&t.svg), ("40".into(), "40".into()));
    // A disc is fitted as a circle primitive, not a polygon.
    assert!(
        t.svg.contains("<circle") || t.svg.contains("<ellipse"),
        "{}",
        t.svg
    );
}

#[test]
fn raw_pixels_match_the_decoded_png() {
    let png = sample("tiny.png");
    let img = image::load_from_memory(&png).unwrap().to_rgba8();
    let opts = Options::default();
    let from_png = inkvec::trace(&png, &opts).unwrap();
    let from_px = inkvec::trace_rgba(img.as_raw(), img.width(), img.height(), &opts).unwrap();
    assert_eq!(from_png, from_px);
}

#[test]
fn output_is_deterministic_across_runs_and_thread_counts() {
    let png = sample("tiny.png");
    let opts = Options::default();
    let first = inkvec::trace(&png, &opts).unwrap();
    assert_eq!(
        inkvec::trace(&png, &opts).unwrap(),
        first,
        "second run differs"
    );
    for threads in [1, 3] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let t = pool.install(|| inkvec::trace(&png, &opts)).unwrap();
        assert_eq!(t, first, "differs on {threads} rayon thread(s)");
    }
}

#[test]
fn concurrent_calls_are_independent() {
    let png = sample("tiny.png");
    let px = disc_rgba(32);
    let a = inkvec::trace(&png, &Options::default()).unwrap();
    let b = inkvec::trace_rgba(&px, 32, 32, &Options::default()).unwrap();
    std::thread::scope(|s| {
        let hs: Vec<_> = (0..4)
            .map(|i| {
                let (png, px) = (&png, &px);
                s.spawn(move || {
                    if i % 2 == 0 {
                        inkvec::trace(png, &Options::default()).unwrap()
                    } else {
                        inkvec::trace_rgba(px, 32, 32, &Options::default()).unwrap()
                    }
                })
            })
            .collect();
        for (i, h) in hs.into_iter().enumerate() {
            let t = h.join().unwrap();
            assert_eq!(t, if i % 2 == 0 { a.clone() } else { b.clone() });
        }
    });
}

#[test]
fn options_change_the_output() {
    let png = sample("tiny.png");
    let base = inkvec::trace(&png, &Options::default()).unwrap();
    let mut o = Options::default();
    o.minify = true;
    let small = inkvec::trace(&png, &o).unwrap();
    assert!(small.svg.len() < base.svg.len());
    let mut o = Options::default();
    o.margin = 0.25;
    let (w, _) = svg_size(&inkvec::trace(&png, &o).unwrap().svg);
    assert_eq!(w.parse::<f64>().unwrap(), 96.0 * 1.5);
}

#[test]
fn invalid_options_are_refused_before_any_work() {
    let png = sample("tiny.png");
    let mut o = Options::default();
    o.colors = 0;
    assert!(
        matches!(inkvec::trace(&png, &o), Err(Error::InvalidOptions(m)) if m.contains("colors"))
    );
    let mut o = Options::default();
    o.precision = f64::NAN;
    let px = disc_rgba(8);
    assert!(matches!(
        inkvec::trace_rgba(&px, 8, 8, &o),
        Err(Error::InvalidOptions(_))
    ));
}

#[test]
fn bad_images_are_invalid_image_errors() {
    let e = inkvec::trace(b"definitely not a png", &Options::default()).unwrap_err();
    assert_eq!(e.code(), "invalid_image");
    assert!(e.to_string().starts_with("invalid image: "));
    let px = disc_rgba(8);
    let e = inkvec::trace_rgba(&px[..px.len() - 1], 8, 8, &Options::default()).unwrap_err();
    assert!(
        matches!(e, Error::InvalidImage(ref m) if m.contains("256 bytes")),
        "{e}"
    );
    let e = inkvec::trace_rgba(&[], 0, 8, &Options::default()).unwrap_err();
    assert_eq!(e.code(), "invalid_image");
    let e = inkvec::trace_rgba(&[], u32::MAX, u32::MAX, &Options::default()).unwrap_err();
    assert_eq!(e.code(), "invalid_image");
}

#[test]
fn the_version_is_the_crate_version() {
    assert_eq!(inkvec::version(), env!("CARGO_PKG_VERSION"));
}
