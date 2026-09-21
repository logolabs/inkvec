//! The batch queue.
//!
//! Drop a folder, get a list. Several hundred rows is the normal case, so the row itself
//! is deliberately small — the interface virtualises the list, and what crosses the IPC
//! boundary is one row's worth of numbers as it changes, never the whole queue.
//!
//! One file at a time, on one worker thread. That looks conservative until you notice the
//! tracer already saturates every core on the file it is working on: running four files at
//! once would not be four times faster, it would be four times the peak memory for the
//! same throughput, and it would make cancelling mean four half-finished traces instead of
//! one. Pause and cancel are checked between rows, where stopping is clean.
//!
//! Failures stay visible after the run. A row that failed keeps its message and the
//! interface can sort failures first, because a red line that scrolled past an hour ago is
//! the same as no error at all.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::options::{Preset, Settings};
use crate::trace::{self, Outcome, Source, Tier};

/// Where a row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RowState {
    Queued,
    Running,
    Done,
    Failed,
    Skipped,
}

/// One file in the queue.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    /// Its index in the queue, which is also its identity for updates.
    pub id: usize,
    /// The file.
    pub path: PathBuf,
    /// Just the name, since that is what the column shows.
    pub file: String,
    /// The preset this row will use.
    pub preset: Preset,
    /// Where it is.
    pub state: RowState,
    /// Mean colour difference, once it has one.
    pub de00: Option<f64>,
    /// Coordinates written.
    pub coordinates: Option<usize>,
    /// Bytes written.
    pub out_bytes: Option<usize>,
    /// Where the SVG went, or would go.
    pub destination: PathBuf,
    /// Why it failed, if it did.
    pub message: Option<String>,
    /// Seconds it took.
    pub seconds: Option<f64>,
}

impl Row {
    fn new(id: usize, path: PathBuf, preset: Preset, out_dir: &Path) -> Self {
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let destination = out_dir.join(
            path.with_extension("svg")
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("out.svg")),
        );
        Self {
            id,
            path,
            file,
            preset,
            state: RowState::Queued,
            de00: None,
            coordinates: None,
            out_bytes: None,
            destination,
            message: None,
            seconds: None,
        }
    }
}

/// What the run as a whole is doing.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    /// Rows finished, whatever the outcome.
    pub finished: usize,
    /// Rows in the queue.
    pub total: usize,
    /// Rows that failed.
    pub failed: usize,
    /// Rows skipped because their output already existed.
    pub skipped: usize,
    /// Bytes written so far.
    pub bytes_written: usize,
    /// Bytes of the sources those outputs replaced, for the "smaller than source" line.
    pub source_bytes: usize,
    /// Mean of the per-row mean dE00, over rows that produced one.
    pub mean_de00: Option<f64>,
    /// Seconds elapsed.
    pub elapsed: f64,
    /// Seconds left, estimated from the rows done so far.
    pub remaining: Option<f64>,
}

/// How a run is set up.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// The files, in the order they will be traced.
    pub files: Vec<PathBuf>,
    /// The preset every row starts with.
    pub preset: Preset,
    /// Per-row overrides, by index.
    #[serde(default)]
    pub overrides: Vec<(usize, Preset)>,
    /// Where the SVGs go.
    pub output_dir: PathBuf,
    /// Leave a row alone when its output is already there.
    #[serde(default)]
    pub skip_existing: bool,
}

impl Plan {
    /// The queue this plan describes, before anything has run.
    pub fn rows(&self) -> Vec<Row> {
        self.files
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let preset = self
                    .overrides
                    .iter()
                    .find(|(idx, _)| *idx == i)
                    .map(|(_, p)| *p)
                    .unwrap_or(self.preset);
                Row::new(i, p.clone(), preset, &self.output_dir)
            })
            .collect()
    }
}

/// The controls a run answers to.
#[derive(Debug, Default)]
pub struct Controls {
    paused: AtomicBool,
    cancelled: AtomicBool,
}

impl Controls {
    /// Hold the run at the next row boundary.
    pub fn pause(&self, on: bool) {
        self.paused.store(on, Ordering::SeqCst);
    }
    /// Stop the run at the next row boundary.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
    /// Whether it has been told to stop.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
}

/// Which image files a folder holds, sorted so the queue is in a predictable order.
pub fn scan(folder: &Path) -> Result<Vec<PathBuf>, String> {
    let entries =
        std::fs::read_dir(folder).map_err(|e| format!("cannot read {}: {e}", folder.display()))?;
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_image(p))
        .collect();
    out.sort();
    Ok(out)
}

fn is_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff")
    )
}

/// Run the queue on this thread, reporting each row as it changes.
///
/// `on_row` is called when a row starts and again when it finishes; `on_totals` after
/// every row. Both should hand the value off and return.
pub fn run(
    plan: &Plan,
    base: &Settings,
    controls: &Arc<Controls>,
    on_row: impl Fn(&Row),
    on_totals: impl Fn(&Totals),
) -> Vec<Row> {
    let mut rows = plan.rows();
    let started = std::time::Instant::now();
    let mut totals = Totals {
        total: rows.len(),
        ..Default::default()
    };
    let mut de00_sum = 0.0f64;
    let mut de00_count = 0usize;

    if let Err(e) = std::fs::create_dir_all(&plan.output_dir) {
        for row in rows.iter_mut() {
            row.state = RowState::Failed;
            row.message = Some(format!("cannot create {}: {e}", plan.output_dir.display()));
            on_row(row);
        }
        return rows;
    }

    for i in 0..rows.len() {
        // Pause holds here, between rows, which is the only place stopping is clean.
        while controls.is_paused() && !controls.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(60));
        }
        if controls.is_cancelled() {
            break;
        }

        if plan.skip_existing && rows[i].destination.exists() {
            rows[i].state = RowState::Skipped;
            totals.skipped += 1;
            totals.finished += 1;
            on_row(&rows[i]);
            update_totals(&mut totals, started, de00_sum, de00_count);
            on_totals(&totals);
            continue;
        }

        rows[i].state = RowState::Running;
        on_row(&rows[i]);

        let settings = Settings {
            // The row's preset decides the drawing; the queue's own settings decide the
            // things a preset does not touch, so a batch honours both.
            ..merge(base, rows[i].preset)
        };
        let outcome = std::fs::read(&rows[i].path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| Source::open(bytes, Some(rows[i].path.clone())))
            .map(|source| {
                let source_bytes = source.bytes.len();
                (
                    trace::run(&source, &settings, Tier::Final, |_, _| {}),
                    source_bytes,
                )
            });

        match outcome {
            Err(why) => fail(&mut rows[i], why, &mut totals),
            Ok((Outcome::Traced(t), source_bytes)) => {
                match std::fs::write(&rows[i].destination, &t.svg) {
                    Ok(()) => {
                        rows[i].state = RowState::Done;
                        rows[i].de00 = t.report.mean_de00;
                        rows[i].coordinates = Some(t.report.coordinates);
                        rows[i].out_bytes = Some(t.svg.len());
                        rows[i].seconds = Some(t.report.seconds);
                        totals.bytes_written += t.svg.len();
                        totals.source_bytes += source_bytes;
                        if let Some(d) = t.report.mean_de00 {
                            de00_sum += d;
                            de00_count += 1;
                        }
                    }
                    Err(e) => {
                        let why = format!("cannot write {}: {e}", rows[i].destination.display());
                        fail(&mut rows[i], why, &mut totals);
                    }
                }
            }
            Ok((Outcome::Flat, _)) => {
                // Not a failure: there was genuinely nothing to trace.
                rows[i].state = RowState::Skipped;
                rows[i].message = Some("one flat colour — nothing to trace".into());
                totals.skipped += 1;
            }
            Ok((Outcome::Undecodable { message }, _)) => fail(&mut rows[i], message, &mut totals),
            Ok((Outcome::Failed { message }, _)) => fail(&mut rows[i], message, &mut totals),
            Ok((
                Outcome::OutOfMemory {
                    needed_gb,
                    suggest_px,
                },
                _,
            )) => fail(
                &mut rows[i],
                format!(
                    "would need about {needed_gb:.1} GB at this trace size; {suggest_px} px fits"
                ),
                &mut totals,
            ),
        }

        totals.finished += 1;
        on_row(&rows[i]);
        update_totals(&mut totals, started, de00_sum, de00_count);
        on_totals(&totals);
    }

    rows
}

fn fail(row: &mut Row, why: String, totals: &mut Totals) {
    row.state = RowState::Failed;
    row.message = Some(why);
    totals.failed += 1;
}

fn update_totals(totals: &mut Totals, started: std::time::Instant, sum: f64, count: usize) {
    totals.elapsed = started.elapsed().as_secs_f64();
    totals.mean_de00 = (count > 0).then(|| sum / count as f64);
    totals.remaining = if totals.finished > 0 && totals.finished < totals.total {
        let per = totals.elapsed / totals.finished as f64;
        Some(per * (totals.total - totals.finished) as f64)
    } else {
        None
    };
}

/// A row's preset applied on top of the queue's own settings.
///
/// A preset is a small set of deliberate differences from the defaults, so only those are
/// carried over; everything the preset does not speak to keeps whatever the queue was set
/// to. Otherwise choosing "Icon" for one row would silently reset that row's output
/// options too.
fn merge(base: &Settings, preset: Preset) -> Settings {
    let d = Settings::default();
    let p = preset.settings();
    let mut out = base.clone();
    if p.precision != d.precision {
        out.precision = p.precision;
    }
    if p.speckle_floor != d.speckle_floor {
        out.speckle_floor = p.speckle_floor;
    }
    if p.trace_size != d.trace_size {
        out.trace_size = p.trace_size;
    }
    if p.max_colours != d.max_colours {
        out.max_colours = p.max_colours;
    }
    if p.colour_merging != d.colour_merging {
        out.colour_merging = p.colour_merging;
    }
    if p.clean_up_damage != d.clean_up_damage {
        out.clean_up_damage = p.clean_up_damage;
    }
    if p.black_and_white != d.black_and_white {
        out.black_and_white = p.black_and_white;
    }
    if p.line_art != d.line_art {
        out.line_art = p.line_art;
    }
    if p.fewer_paths != d.fewer_paths {
        out.fewer_paths = p.fewer_paths;
    }
    out
}

/// The summary CSV the toolbar's "Export stats.csv" writes.
pub fn stats_csv(rows: &[Row]) -> String {
    let mut out = String::from(
        "file,preset,status,mean_de00,coordinates,output_bytes,seconds,destination,message\n",
    );
    for r in rows {
        let preset = serde_json::to_string(&r.preset).unwrap_or_default();
        out.push_str(&format!(
            "{},{},{:?},{},{},{},{},{},{}\n",
            csv(&r.file),
            csv(preset.trim_matches('"')),
            r.state,
            r.de00.map(|d| format!("{d:.4}")).unwrap_or_default(),
            r.coordinates.map(|c| c.to_string()).unwrap_or_default(),
            r.out_bytes.map(|b| b.to_string()).unwrap_or_default(),
            r.seconds.map(|s| format!("{s:.3}")).unwrap_or_default(),
            csv(&r.destination.display().to_string()),
            csv(r.message.as_deref().unwrap_or("")),
        ));
    }
    out
}

/// A CSV field that survives commas, quotes and newlines in a file name.
fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(colour: [u8; 4], size: u32) -> Vec<u8> {
        let mut img = image::RgbaImage::from_pixel(size, size, image::Rgba([250, 248, 245, 255]));
        for y in size / 4..size * 3 / 4 {
            for x in size / 4..size * 3 / 4 {
                img.put_pixel(x, y, image::Rgba(colour));
            }
        }
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn workspace(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("inkvec-batch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_folder_scan_finds_images_and_ignores_everything_else() {
        let dir = workspace("scan");
        std::fs::write(dir.join("a.png"), png([20, 69, 63, 255], 32)).unwrap();
        std::fs::write(dir.join("b.PNG"), png([20, 69, 63, 255], 32)).unwrap();
        std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
        std::fs::write(dir.join("c.svg"), b"<svg/>").unwrap();
        let found = scan(&dir).unwrap();
        assert_eq!(found.len(), 2, "{found:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_run_writes_one_svg_per_row_and_totals_them() {
        let dir = workspace("run");
        let out = dir.join("svg");
        for i in 0..3 {
            std::fs::write(
                dir.join(format!("logo-{i}.png")),
                png([20, 69, 63, 255], 48),
            )
            .unwrap();
        }
        let plan = Plan {
            files: scan(&dir).unwrap(),
            preset: Preset::Logo,
            overrides: vec![],
            output_dir: out.clone(),
            skip_existing: false,
        };
        let controls = Arc::new(Controls::default());
        let seen = std::sync::Mutex::new(Vec::new());
        let rows = run(
            &plan,
            &Settings::default(),
            &controls,
            |r| seen.lock().unwrap().push((r.id, r.state)),
            |_| {},
        );
        assert_eq!(rows.len(), 3);
        for r in &rows {
            assert_eq!(r.state, RowState::Done, "{:?}", r.message);
            assert!(r.destination.is_file());
            assert!(r.out_bytes.unwrap() > 0);
            assert!(r.de00.is_some(), "every finished row is measured");
        }
        // Each row was reported twice: starting, then finished.
        let reports = seen.lock().unwrap();
        assert_eq!(reports.len(), 6, "{reports:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn skip_existing_leaves_an_output_that_is_already_there() {
        let dir = workspace("skip");
        let out = dir.join("svg");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(dir.join("logo.png"), png([20, 69, 63, 255], 48)).unwrap();
        std::fs::write(out.join("logo.svg"), b"<svg>already here</svg>").unwrap();
        let plan = Plan {
            files: scan(&dir).unwrap(),
            preset: Preset::Logo,
            overrides: vec![],
            output_dir: out.clone(),
            skip_existing: true,
        };
        let rows = run(
            &plan,
            &Settings::default(),
            &Arc::new(Controls::default()),
            |_| {},
            |_| {},
        );
        assert_eq!(rows[0].state, RowState::Skipped);
        assert_eq!(
            std::fs::read_to_string(out.join("logo.svg")).unwrap(),
            "<svg>already here</svg>"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failure_keeps_its_message_and_does_not_stop_the_queue() {
        let dir = workspace("fail");
        std::fs::write(dir.join("a.png"), b"not really a png").unwrap();
        std::fs::write(dir.join("b.png"), png([20, 69, 63, 255], 48)).unwrap();
        let plan = Plan {
            files: scan(&dir).unwrap(),
            preset: Preset::Logo,
            overrides: vec![],
            output_dir: dir.join("svg"),
            skip_existing: false,
        };
        let rows = run(
            &plan,
            &Settings::default(),
            &Arc::new(Controls::default()),
            |_| {},
            |_| {},
        );
        assert_eq!(rows[0].state, RowState::Failed);
        assert!(rows[0].message.as_ref().unwrap().contains("PNG, JPEG"));
        assert_eq!(rows[1].state, RowState::Done, "the queue carried on");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelling_stops_the_queue_where_it_is() {
        let dir = workspace("cancel");
        for i in 0..4 {
            std::fs::write(
                dir.join(format!("logo-{i}.png")),
                png([20, 69, 63, 255], 48),
            )
            .unwrap();
        }
        let plan = Plan {
            files: scan(&dir).unwrap(),
            preset: Preset::Logo,
            overrides: vec![],
            output_dir: dir.join("svg"),
            skip_existing: false,
        };
        let controls = Arc::new(Controls::default());
        let c = Arc::clone(&controls);
        let rows = run(
            &plan,
            &Settings::default(),
            &controls,
            move |r| {
                if r.id == 0 && r.state == RowState::Done {
                    c.cancel();
                }
            },
            |_| {},
        );
        assert_eq!(rows[0].state, RowState::Done);
        assert_eq!(
            rows[1].state,
            RowState::Queued,
            "nothing after the cancel ran"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_row_preset_only_changes_what_that_preset_speaks_to() {
        let queue = Settings {
            minify: true,
            transparent_background: true,
            max_colours: 200,
            ..Settings::default()
        };
        let merged = merge(&queue, Preset::Icon);
        assert!(merged.minify, "the queue's output options survive");
        assert!(merged.transparent_background);
        assert_eq!(merged.speckle_floor, 1.0, "the preset's own values apply");
        assert_eq!(merged.max_colours, 16);
    }

    #[test]
    fn the_csv_survives_a_comma_in_a_file_name() {
        let row = Row {
            id: 0,
            path: PathBuf::from("/tmp/a,b.png"),
            file: "a,b.png".into(),
            preset: Preset::Logo,
            state: RowState::Done,
            de00: Some(0.1234),
            coordinates: Some(42),
            out_bytes: Some(99),
            destination: PathBuf::from("/tmp/out/a,b.svg"),
            message: None,
            seconds: Some(1.5),
        };
        let csv = stats_csv(&[row]);
        assert!(csv
            .lines()
            .next()
            .unwrap()
            .starts_with("file,preset,status"));
        assert!(csv.contains("\"a,b.png\""), "{csv}");
        assert!(csv.contains("0.1234"));
    }
}
