//! What the vectorization wizard needs from the backend, and nothing it does not.
//!
//! The wizard is an interface over the ordinary settings: everything it changes is a
//! control the Tune tab already has, and the automatic trace it starts from is the one
//! opening an image has always run. Three things are new here.
//!
//! **Previews.** Choosing what an image is ("line art", "an icon") is easier with the
//! answer in front of you, so the first step shows a small trace of *this* image under each
//! choice. A preview is a draft with a generation of its own: starting one never retires
//! the trace the interface is waiting for, it never lands in the drawing cache (which holds
//! only the drawing on screen), it never becomes the remembered settings, and it waits for
//! the one trace slot behind any interactive trace, the way a batch row does. See
//! [`run_preview`] and `lib.rs`'s `start_preview`.
//!
//! **Facts about the source.** Whether the image has any transparency at all, and how
//! noisy its pixels are, decide which steps the wizard shows and what "Auto chose" offers.
//! Neither is part of a trace's report, and computing them there would put their cost on
//! every trace; they are asked for once, after the first trace of an image has finished.
//!
//! **The file the app was launched with.** The Windows context-menu entry runs the app with
//! the image's path as its argument, which nothing used to read. [`path_from_args`] picks
//! it out of a command line — the first one, and a second launch's, which the single-instance
//! plugin forwards to the window that is already open.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::options::Settings;
use crate::trace::{self, MeasureLevel, Outcome, Source, Tier};

/// The extensions the app opens, as the file picker offers them.
pub const OPENABLE: [&str; 9] = [
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "svg",
];

/// The image a command line asks the app to open, if it names one.
///
/// The first argument is the program itself and is skipped; so is anything that looks like
/// a flag. The first remaining argument that is an existing file with an extension the app
/// opens wins. A relative path is read against `cwd`, the directory the launch came from
/// (a second instance's own, which is not this process's). `is_file` is the existence test,
/// passed in so the rule can be tested without touching the disk.
pub fn path_from_args<I, S>(
    args: I,
    cwd: Option<&Path>,
    is_file: impl Fn(&Path) -> bool,
) -> Option<PathBuf>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().skip(1).find_map(|arg| {
        let arg = arg.as_ref().trim().trim_matches('"');
        if arg.is_empty() || arg.starts_with('-') {
            return None;
        }
        let raw = PathBuf::from(arg);
        let path = match cwd {
            Some(dir) if raw.is_relative() => dir.join(raw),
            _ => raw,
        };
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        (OPENABLE.contains(&ext.as_str()) && is_file(&path)).then_some(path)
    })
}

/// What the wizard wants to know about an image that a trace's report does not say.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Facts {
    /// The pixel noise the engine's own estimator measures, in 8-bit levels. A clean
    /// render sits at the estimator's floor, half a level; JPEG damage and camera noise
    /// read several.
    pub noise_levels: f64,
    /// Whether any of the image is see-through, so "Trace transparency" means something.
    pub has_alpha: bool,
    /// The share of the canvas that is mostly clear (alpha under a half).
    pub clear_share: f64,
}

/// How transparent a pixel must be before the image counts as having transparency: well
/// clear of the rounding an opaque file's alpha channel can carry.
const ALPHA_OPAQUE: f32 = 0.99;

/// Measure `img`.
///
/// The noise is `inkvec_trace::coverage::estimate_noise` on the luminance the palette sees,
/// with transparent pixels composited on white first: a clear pixel's colour channels are
/// arbitrary, and the edge between two arbitrary colours is not noise in the picture.
pub fn facts_of(img: &inkvec_trace::Rgba) -> Facts {
    let n = img.width * img.height;
    let mut lum = Vec::with_capacity(n);
    let mut translucent = 0usize;
    let mut clear = 0usize;
    for p in img.data.as_chunks::<4>().0.iter().take(n) {
        let a = p[3].clamp(0.0, 1.0);
        if a < ALPHA_OPAQUE {
            translucent += 1;
        }
        if a < 0.5 {
            clear += 1;
        }
        let y = 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
        lum.push(y * a + (1.0 - a));
    }
    let sigma = inkvec_trace::coverage::estimate_noise(&lum, img.width, img.height);
    Facts {
        noise_levels: sigma * 255.0,
        // One stray pixel of alpha is a file-format artefact, not transparency anybody drew.
        has_alpha: translucent * 1000 > n.max(1),
        clear_share: clear as f64 / n.max(1) as f64,
    }
}

/// Trace a preview: the draft of `settings`, measured only as far as a thumbnail needs.
///
/// The same plan the interactive draft follows — a small image's draft is its final — and
/// the same pipeline, so a preview shows what choosing those settings will draw. It is
/// never given the drawing cache: that holds the one drawing on screen, and a preview that
/// replaced it would make the next output-option change trace again.
pub fn run_preview(
    source: &Arc<Source>,
    settings: &Settings,
    draft_px: u32,
    draft_seconds: f64,
) -> Outcome {
    let (settings, tier) = trace::plan(
        settings,
        Tier::Draft,
        draft_px,
        draft_seconds,
        (source.width, source.height),
    );
    trace::run_at(
        source,
        &settings,
        tier,
        None,
        MeasureLevel::Summary,
        |_, _| {},
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(w: usize, h: usize, px: impl Fn(usize, usize) -> [f32; 4]) -> inkvec_trace::Rgba {
        let mut data = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&px(x, y));
            }
        }
        inkvec_trace::Rgba {
            width: w,
            height: h,
            data,
        }
    }

    #[test]
    fn the_launch_path_is_the_first_openable_file_after_the_program() {
        let exists = |p: &Path| p.to_string_lossy().contains("logo");
        let args = [
            "C:/Apps/inkvec-studio.exe",
            "--flag",
            "C:/Brand/logo.png",
            "C:/Brand/logo2.png",
        ];
        assert_eq!(
            path_from_args(args, None, exists),
            Some(PathBuf::from("C:/Brand/logo.png"))
        );
        // The program itself is never the image, even when it would qualify.
        assert_eq!(path_from_args(["C:/logo.png"], None, exists), None);
    }

    #[test]
    fn a_launch_path_must_be_an_existing_image() {
        let exists = |p: &Path| !p.to_string_lossy().contains("missing");
        assert_eq!(path_from_args(["app", "notes.txt"], None, exists), None);
        assert_eq!(
            path_from_args(["app", "C:/missing.png"], None, exists),
            None
        );
        assert_eq!(path_from_args(["app", ""], None, exists), None);
        // The case of the extension is the file system's business, not ours.
        assert_eq!(
            path_from_args(["app", "C:/Scan.JPG"], None, exists),
            Some(PathBuf::from("C:/Scan.JPG"))
        );
        // The shell's quotes, if they survive, are not part of the name.
        assert_eq!(
            path_from_args(["app", "\"C:/a b/c.webp\""], None, exists),
            Some(PathBuf::from("C:/a b/c.webp"))
        );
    }

    #[test]
    fn a_relative_launch_path_is_read_against_the_launch_directory() {
        let dir = std::env::temp_dir();
        let seen = std::cell::RefCell::new(Vec::new());
        let got = path_from_args(["app", "logo.svg"], Some(&dir), |p| {
            seen.borrow_mut().push(p.to_path_buf());
            true
        });
        assert_eq!(got, Some(dir.join("logo.svg")));
        assert_eq!(seen.borrow().as_slice(), [dir.join("logo.svg")]);
    }

    #[test]
    fn a_real_file_on_disk_is_found() {
        let file = std::env::temp_dir().join(format!("inkvec-launch-{}.png", std::process::id()));
        std::fs::write(&file, b"not really a png").unwrap();
        let got = path_from_args(
            ["app".to_string(), file.display().to_string()],
            None,
            Path::is_file,
        );
        let _ = std::fs::remove_file(&file);
        assert_eq!(got, Some(file));
    }

    #[test]
    fn a_clean_opaque_image_is_quiet_and_has_no_alpha() {
        let img = rgba(64, 64, |x, _| {
            if x < 32 {
                [0.1, 0.2, 0.6, 1.0]
            } else {
                [0.95, 0.9, 0.8, 1.0]
            }
        });
        let f = facts_of(&img);
        assert!(!f.has_alpha);
        assert_eq!(f.clear_share, 0.0);
        assert!(
            f.noise_levels < 1.0,
            "a clean render reads at the floor: {}",
            f.noise_levels
        );
    }

    #[test]
    fn noise_reads_as_noise() {
        // A fixed pseudo-random pattern, about four levels either way.
        let mut state = 0x2545_f491_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f32 / u32::MAX as f32 - 0.5) * (8.0 / 255.0)
        };
        let noise: Vec<f32> = (0..64 * 64).map(|_| next()).collect();
        let img = rgba(64, 64, |x, y| {
            let v = 0.5 + noise[y * 64 + x];
            [v, v, v, 1.0]
        });
        let f = facts_of(&img);
        assert!(
            f.noise_levels > 1.5,
            "noise should read above the floor: {}",
            f.noise_levels
        );
    }

    #[test]
    fn a_cutout_has_alpha_and_its_clear_ground_is_not_noise() {
        // Clear pixels with arbitrary colour channels, as exporters leave them.
        let img = rgba(64, 64, |x, y| {
            if (16..48).contains(&x) && (16..48).contains(&y) {
                [0.8, 0.1, 0.1, 1.0]
            } else if (x + y) % 2 == 0 {
                [0.0, 0.0, 0.0, 0.0]
            } else {
                [1.0, 1.0, 1.0, 0.0]
            }
        });
        let f = facts_of(&img);
        assert!(f.has_alpha);
        assert!((f.clear_share - 0.75).abs() < 1e-9, "{}", f.clear_share);
        assert!(
            f.noise_levels < 1.0,
            "the clear ground is flat once composited: {}",
            f.noise_levels
        );
    }

    #[test]
    fn a_stray_translucent_pixel_is_not_transparency() {
        let img = rgba(64, 64, |x, y| {
            if x == 0 && y == 0 {
                [1.0, 1.0, 1.0, 0.5]
            } else {
                [1.0, 1.0, 1.0, 1.0]
            }
        });
        assert!(!facts_of(&img).has_alpha);
    }

    fn sample_source() -> Arc<Source> {
        let mut img = image::RgbaImage::from_pixel(80, 80, image::Rgba([245, 242, 234, 255]));
        for y in 20..60u32 {
            for x in 20..60u32 {
                img.put_pixel(x, y, image::Rgba([20, 69, 63, 255]));
            }
        }
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        Arc::new(Source::open(out.into_inner(), None).unwrap())
    }

    #[test]
    fn a_preview_is_a_trace_of_the_settings_it_was_given() {
        let source = sample_source();
        let settings = Settings::default();
        let Outcome::Traced(t) = run_preview(&source, &settings, 512, 2.0) else {
            panic!("the preview should trace");
        };
        assert!(t.svg.contains("#14453f"), "the square is drawn: {}", t.svg);
        // A preview measures what a thumbnail shows and no more: the colour difference and
        // the counts, not the palette or the confidence bands.
        assert!(t.report.mean_de00.is_some());
        assert!(t.bands.is_none());
        assert!(t.palette.is_empty());
    }

    #[test]
    fn a_preview_never_takes_the_place_of_the_drawing_in_hand() {
        let source = sample_source();
        let cache = trace::Cache::default();
        let settings = Settings::default();
        let Outcome::Traced(_) =
            trace::run(&source, &settings, Tier::Final, Some(&cache), |_, _| {})
        else {
            panic!("the trace should trace");
        };
        let other = Settings {
            black_and_white: true,
            ..Settings::default()
        };
        let Outcome::Traced(_) = run_preview(&source, &other, 512, 2.0) else {
            panic!("the preview should trace");
        };
        assert!(
            cache.reuse(&source, &settings, Tier::Final).is_some(),
            "the drawing on screen is still the one kept"
        );
        assert!(cache.reuse(&source, &other, Tier::Final).is_none());
    }
}
