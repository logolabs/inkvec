//! Test-only replay of dumped fast-fitter inputs: every map edge of a trace, with its closed
//! flag and its contrast, written by the phase-1 research build (`INKVEC_FFDUMP`) and read
//! back here without the engine.
//!
//! The tests below are `#[ignore]`d and read the dumps from the directory named by
//! `INKVEC_FFD_DIR` (searched recursively for `*.ffd`); run them with
//! `INKVEC_FFD_DIR=<dir> cargo test --release -p inkvec-trace replay -- --ignored --nocapture`.
//! They check the exact rewrites of the fitter against their kept references, bit for bit,
//! on every dumped edge, and time the stages single-threaded.
//!
//! Format ("FFD1", little endian): u32 edge count, then per edge u8 closed, f64 contrast,
//! u32 n, n × (f64 x, f64 y).

use super::{polygon, smooth, FastFit};
use inkvec_core::Point;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// One dumped edge.
pub(crate) struct DumpEdge {
    pub closed: bool,
    pub contrast: f64,
    pub pts: Vec<Point>,
}

/// Read one dump.
pub(crate) fn load(path: &Path) -> Vec<DumpEdge> {
    let b = std::fs::read(path).expect("read a dump");
    assert_eq!(&b[0..4], b"FFD1", "{}", path.display());
    let mut at = 4;
    let u32_at = |at: &mut usize| {
        let v = u32::from_le_bytes(b[*at..*at + 4].try_into().expect("4 bytes"));
        *at += 4;
        v
    };
    let count = u32_at(&mut at) as usize;
    let f64_at = |at: &mut usize| {
        let v = f64::from_le_bytes(b[*at..*at + 8].try_into().expect("8 bytes"));
        *at += 8;
        v
    };
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let closed = b[at] != 0;
        at += 1;
        let contrast = f64_at(&mut at);
        let n = u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes")) as usize;
        at += 4;
        let pts = (0..n)
            .map(|_| {
                let x = f64_at(&mut at);
                let y = f64_at(&mut at);
                Point::new(x, y)
            })
            .collect();
        out.push(DumpEdge {
            closed,
            contrast,
            pts,
        });
    }
    out
}

/// Every `*.ffd` under `INKVEC_FFD_DIR`, sorted, or `None` when the variable is not set.
pub(crate) fn dump_files() -> Option<Vec<PathBuf>> {
    let root = inkvec_core::env::path("INKVEC_FFD_DIR")?;
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)
            .expect("read a dump directory")
            .flatten()
        {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "ffd") {
                out.push(p);
            }
        }
    }
    out.sort();
    Some(out)
}

/// The points `fit_edge` hands the polygon for this edge: the ring without its lattice
/// start node, denoised. `None` for an edge too short for a polygon.
pub(crate) fn polygon_input(e: &DumpEdge) -> Option<Vec<Point>> {
    let pts = if e.closed && e.pts.len() >= 8 {
        &e.pts[1..]
    } else {
        &e.pts[..]
    };
    let n = pts.len();
    if n < 3 || (e.closed && n < 4) {
        return None;
    }
    Some(smooth::denoise(pts, e.closed))
}

/// The tolerances the replay runs every edge at: its own (from its contrast), and the
/// loosest the fitter ever uses.
fn tolerances(e: &DumpEdge) -> [f64; 2] {
    let base = FastFit::default();
    [
        base.for_contrast(e.contrast).poly_tol,
        base.for_contrast(0.0).poly_tol,
    ]
}

/// Every dumped edge's polygon, at its own tolerance and at the loosest, against the
/// kept reference (`polygon::tests::open_ref` / `closed_ref`), vertex list for vertex list.
#[test]
#[ignore = "needs INKVEC_FFD_DIR"]
fn replay_polygon_matches_the_reference_on_every_dumped_edge() {
    let Some(files) = dump_files() else {
        eprintln!("INKVEC_FFD_DIR not set; nothing replayed");
        return;
    };
    let (mut edges, mut runs) = (0usize, 0usize);
    for f in &files {
        for e in load(f) {
            edges += 1;
            let Some(pts) = polygon_input(&e) else {
                continue;
            };
            for tol in tolerances(&e) {
                runs += 1;
                let (new, old) = if e.closed {
                    (
                        polygon::closed(&pts, tol),
                        polygon::tests::closed_ref(&pts, tol),
                    )
                } else {
                    (
                        polygon::open(&pts, tol),
                        polygon::tests::open_ref(&pts, tol),
                    )
                };
                assert_eq!(
                    new,
                    old,
                    "{}: edge of {} points, tol {tol}",
                    f.display(),
                    pts.len()
                );
            }
        }
    }
    eprintln!(
        "{} dumps, {edges} edges, {runs} polygon runs: all identical",
        files.len()
    );
}

/// Smallest of `reps` timings of `f`, in ms.
fn min_ms(reps: usize, mut f: impl FnMut()) -> f64 {
    (0..reps)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1e3
        })
        .fold(f64::INFINITY, f64::min)
}

/// `INKVEC_FFD_REPS`, the number of timed runs a replay keeps the smallest of, or
/// `default` when unset.
fn reps(default: usize) -> usize {
    inkvec_core::env::count("INKVEC_FFD_REPS").unwrap_or(default)
}

/// Run the polygon of one prepared input, [`open`](polygon::open) or
/// [`closed`](polygon::closed), or with `reference` their kept references.
fn run_polygon(pts: &[Point], closed: bool, tol: f64, reference: bool) {
    use std::hint::black_box;
    let v = match (closed, reference) {
        (true, false) => polygon::closed(black_box(pts), tol),
        (false, false) => polygon::open(black_box(pts), tol),
        (true, true) => polygon::tests::closed_ref(black_box(pts), tol),
        (false, true) => polygon::tests::open_ref(black_box(pts), tol),
    };
    black_box(v);
}

/// The polygon of every dumped edge of at least `INKVEC_FFD_MIN` points (default 2048),
/// each timed alone, smallest of `INKVEC_FFD_REPS` (default 30) runs, against the kept
/// reference timed the same way: on a shared machine the minimum over many runs is what
/// catches the cores idle, which a parallel scan needs. Prints the sums, in ms.
#[test]
#[ignore = "needs INKVEC_FFD_DIR"]
fn replay_long_edges() {
    let Some(files) = dump_files() else {
        eprintln!("INKVEC_FFD_DIR not set; nothing replayed");
        return;
    };
    let min_n = inkvec_core::env::count("INKVEC_FFD_MIN").unwrap_or(2048);
    let reps = reps(30);
    let (mut edges, mut new, mut old) = (0usize, 0.0, 0.0);
    for f in &files {
        for e in load(f) {
            let Some(pts) = polygon_input(&e) else {
                continue;
            };
            if pts.len() < min_n {
                continue;
            }
            let tol = tolerances(&e)[0];
            edges += 1;
            new += min_ms(reps, || run_polygon(&pts, e.closed, tol, false));
            old += min_ms(reps, || run_polygon(&pts, e.closed, tol, true));
        }
    }
    eprintln!("{edges} edges of {min_n}+ points: polygon {new:.2} ms, reference {old:.2} ms");
}

/// True for the image-frame ring of a dump: a closed edge whose bounding box is the
/// bounding box of every edge of the map, with its corner at the image's (-0.5, -0.5).
fn frame_flags(edges: &[DumpEdge]) -> Vec<bool> {
    let bbox = |pts: &mut dyn Iterator<Item = Point>| {
        pts.fold(
            (f64::MAX, f64::MAX, f64::MIN, f64::MIN),
            |(x0, y0, x1, y1), p| (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)),
        )
    };
    let all = bbox(&mut edges.iter().flat_map(|e| e.pts.iter().copied()));
    edges
        .iter()
        .map(|e| {
            e.closed
                && !e.pts.is_empty()
                && all.0 == -0.5
                && all.1 == -0.5
                && bbox(&mut e.pts.iter().copied()) == all
        })
        .collect()
}

/// The columns of [`replay_timing`], in order.
const TIMING_COLUMNS: [&str; 6] = [
    "fit_edge",
    "prims",
    "poly frame",
    "poly other",
    "poly ref",
    "sum slowest",
];

/// One dump's row of [`replay_timing`], in ms, smallest of `reps` runs each: the whole
/// `fit_edge` over every edge; the primitive test on the rings; the polygon stage on the
/// image frame, on the other edges, and on all of them with the kept reference; and the
/// slowest single edge's `fit_edge`.
fn time_dump(edges: &[DumpEdge], reps: usize) -> [f64; 6] {
    use std::hint::black_box;
    let base = FastFit::default();
    let frame = frame_flags(edges);
    let cfgs: Vec<FastFit> = edges
        .iter()
        .map(|e| base.for_contrast(e.contrast))
        .collect();
    let inputs: Vec<(Vec<Point>, bool, f64, bool)> = edges
        .iter()
        .zip(&cfgs)
        .zip(&frame)
        .filter_map(|((e, c), &fr)| polygon_input(e).map(|p| (p, e.closed, c.poly_tol, fr)))
        .collect();
    let rings: Vec<Vec<Point>> = edges
        .iter()
        .filter(|e| e.closed)
        .map(|e| {
            let pts = if e.pts.len() >= 8 {
                &e.pts[1..]
            } else {
                &e.pts[..]
            };
            smooth::denoise(pts, true)
        })
        .collect();
    // The polygon of the frame's inputs (`Some(true)`), the others' (`Some(false)`), or
    // all of them through the reference (`None`).
    let poly = |which: Option<bool>| {
        min_ms(reps, || {
            for (p, closed, tol, fr) in &inputs {
                if which.is_some_and(|w| w != *fr) {
                    continue;
                }
                run_polygon(p, *closed, *tol, which.is_none());
            }
        })
    };
    let whole = |e: &DumpEdge, c: &FastFit| {
        black_box(super::fit_edge(black_box(&e.pts), e.closed, c));
    };
    [
        min_ms(reps, || {
            for (e, c) in edges.iter().zip(&cfgs) {
                whole(e, c);
            }
        }),
        min_ms(reps, || {
            for r in &rings {
                black_box(super::prims::primitive(black_box(r)));
            }
        }),
        poly(Some(true)),
        poly(Some(false)),
        poly(None),
        edges
            .iter()
            .zip(&cfgs)
            .map(|(e, c)| min_ms(reps, || whole(e, c)))
            .fold(0.0f64, f64::max),
    ]
}

/// Timings per dump set (the dump directory's first component below `INKVEC_FFD_DIR`),
/// summed over its dumps, smallest of `INKVEC_FFD_REPS` (default 5) runs each; see
/// [`time_dump`] for the columns. The edges are fitted one after another, so everything
/// but a boundary long enough for the polygon's parallel scan runs on one thread; the
/// last column, the sum over images of the slowest edge, is what bounds the parallel
/// fit's wall time.
#[test]
#[ignore = "needs INKVEC_FFD_DIR"]
fn replay_timing() {
    let (Some(files), Some(root)) = (dump_files(), inkvec_core::env::path("INKVEC_FFD_DIR")) else {
        eprintln!("INKVEC_FFD_DIR not set; nothing replayed");
        return;
    };
    let reps = reps(5);
    let mut sets: std::collections::BTreeMap<String, [f64; 6]> = Default::default();
    for f in &files {
        let set = f
            .strip_prefix(&root)
            .ok()
            .and_then(|r| r.components().next())
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        let row = time_dump(&load(f), reps);
        for (a, b) in sets.entry(set).or_default().iter_mut().zip(row) {
            *a += b;
        }
    }
    eprint!("{:<10}", "set");
    for c in TIMING_COLUMNS {
        eprint!("{c:>13}");
    }
    eprintln!();
    for (k, s) in &sets {
        eprint!("{k:<10}");
        for v in s {
            eprint!("{v:>13.2}");
        }
        eprintln!();
    }
}
