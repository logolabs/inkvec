//! `inkvec-svgmin`: rewrite an SVG's paths as the fewest segments that draw the same picture.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use inkvec_svgmin::{minify, Options, Report};

const USAGE: &str = "\
usage: inkvec-svgmin <input.svg> [-o <output.svg>] [options]
       inkvec-svgmin <inputs...> --out-dir <dir> [options]

Rewrites every <path d> as the cheapest description that stays within a tolerance of the
original curve, by minimum description length. Corners in the source survive exactly;
paint, ids, groups and transforms pass through untouched; a path that would not get
cheaper is left as it was.

options:
  -o, --output <file>       write here (default: stdout)
      --out-dir <dir>       one output per input, same file names
      --tolerance <px>      largest deviation, in pixels at --judge size   [0.1]
      --judge <px>          the viewing size the tolerance is stated at   [1024]
      --corner-degrees <d>  turn above which a join is a hard corner      [30]
      --decimals <n>        coordinate decimals (default: from the tolerance)
      --no-document         leave everything that is not path data alone: colours,
                            attributes, comments, metadata, whitespace
      --bytes-only          rewrite the bytes, not the drawing: no segment is removed,
                            moved or re-chosen, only spelled more cheaply
      --stats               print what changed, to stderr
  -h, --help
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("inkvec-svgmin: {e}");
            ExitCode::from(1)
        }
    }
}

/// Parse the arguments, rewrite each input, and write each result to `-o`, into `--out-dir`,
/// or to stdout. The first error stops the run.
fn run() -> Result<(), String> {
    run_iter(std::env::args().skip(1))
}

fn run_iter<I: IntoIterator<Item = String>>(args: I) -> Result<(), String> {
    let mut inputs: Vec<PathBuf> = Vec::new();
    let mut output: Option<PathBuf> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut opts = Options::default();
    let mut stats = false;
    let mut bytes_only = false;
    let mut it = args.into_iter();
    let value = |it: &mut dyn Iterator<Item = String>, flag: &str| -> Result<String, String> {
        it.next().ok_or_else(|| format!("{flag} needs a value"))
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "-o" | "--output" => output = Some(value(&mut it, "-o")?.into()),
            "--out-dir" => out_dir = Some(value(&mut it, "--out-dir")?.into()),
            "--tolerance" => opts.tolerance_px = parse(&value(&mut it, "--tolerance")?)?,
            "--judge" => opts.judge = parse(&value(&mut it, "--judge")?)?,
            "--corner-degrees" => {
                opts.corner_degrees = parse(&value(&mut it, "--corner-degrees")?)?
            }
            "--decimals" => opts.decimals = Some(parse::<usize>(&value(&mut it, "--decimals")?)?),
            "--no-document" => opts.document = false,
            "--bytes-only" => bytes_only = true,
            "--stats" => stats = true,
            s if s.starts_with('-') => return Err(format!("unknown option {s}\n{USAGE}")),
            _ => inputs.push(a.into()),
        }
    }
    if inputs.is_empty() {
        return Err(format!("no input\n{USAGE}"));
    }
    if inputs.len() > 1 && out_dir.is_none() {
        return Err("several inputs need --out-dir".into());
    }
    if let Some(d) = &out_dir {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }

    let mut total = Report::default();
    let t0 = std::time::Instant::now();
    for input in &inputs {
        let svg =
            std::fs::read_to_string(input).map_err(|e| format!("{}: {e}", input.display()))?;
        let rewrite = if bytes_only {
            inkvec_svgmin::compact
        } else {
            minify
        };
        let (out, rep) = rewrite(&svg, &opts).map_err(|e| format!("{}: {e}", input.display()))?;
        let dest: Option<PathBuf> = match (&out_dir, &output) {
            (Some(d), _) => Some(d.join(input.file_name().unwrap_or(input.as_os_str()))),
            (None, Some(o)) => Some(o.clone()),
            (None, None) => None,
        };
        match dest {
            Some(p) => std::fs::write(&p, out).map_err(|e| format!("{}: {e}", p.display()))?,
            None => print!("{out}"),
        }
        if stats {
            eprintln!("{}: {}", name(input), line(&rep));
        }
        add(&mut total, &rep);
    }
    if stats && inputs.len() > 1 {
        eprintln!(
            "total ({} files, {:.0} ms): {}",
            inputs.len(),
            t0.elapsed().as_secs_f64() * 1e3,
            line(&total)
        );
    }
    Ok(())
}

/// A flag's value as a number.
fn parse<T: std::str::FromStr>(s: &str) -> Result<T, String> {
    s.parse().map_err(|_| format!("not a number: {s}"))
}

/// The file name of `p`, for the `--stats` lines.
fn name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// One `--stats` line: what the rewrite did, and the parameter saving as a percentage.
fn line(r: &Report) -> String {
    let pct = if r.params_before > 0.0 {
        100.0 * (1.0 - r.params_after / r.params_before)
    } else {
        0.0
    };
    format!(
        "{} paths ({} rewritten, {} as primitives), segments {} -> {}, params {:.0} -> {:.0} \
         ({pct:.1}% smaller), {} runs guarded, tolerance {:.4} units",
        r.paths,
        r.rewritten,
        r.primitives,
        r.segments_before,
        r.segments_after,
        r.params_before,
        r.params_after,
        r.guarded,
        r.tolerance_units
    )
}

/// Add one file's report to the running total. The tolerance is per file; the total keeps
/// the last one.
fn add(t: &mut Report, r: &Report) {
    t.paths += r.paths;
    t.rewritten += r.rewritten;
    t.primitives += r.primitives;
    t.subpaths += r.subpaths;
    t.segments_before += r.segments_before;
    t.segments_after += r.segments_after;
    t.params_before += r.params_before;
    t.params_after += r.params_after;
    t.guarded += r.guarded;
    t.tolerance_units = r.tolerance_units;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_flags_and_errors() {
        assert!(run_iter(vec!["--help".to_string()]).is_ok());
        assert!(run_iter(vec!["-h".to_string()]).is_ok());
        assert!(run_iter(Vec::<String>::new()).is_err());
        assert!(run_iter(vec!["--unknown".to_string()]).is_err());
        assert!(run_iter(vec!["-o".to_string()]).is_err());
        assert!(run_iter(vec!["--tolerance".to_string(), "abc".to_string()]).is_err());
        assert!(run_iter(vec!["file1.svg".to_string(), "file2.svg".to_string()]).is_err());
    }

    #[test]
    fn test_cli_run_files_and_modes() {
        let dir = std::env::temp_dir().join(format!("svgmin_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let in_file = dir.join("input.svg");
        let out_file = dir.join("output.svg");
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><path d="M 0 0 L 100 0 L 100 100 Z"/></svg>"#;
        std::fs::write(&in_file, svg).unwrap();

        let res = run_iter(vec![
            in_file.to_str().unwrap().to_string(),
            "-o".to_string(),
            out_file.to_str().unwrap().to_string(),
            "--tolerance".to_string(),
            "0.2".to_string(),
            "--judge".to_string(),
            "512".to_string(),
            "--corner-degrees".to_string(),
            "45".to_string(),
            "--decimals".to_string(),
            "2".to_string(),
            "--no-document".to_string(),
            "--stats".to_string(),
        ]);
        assert!(res.is_ok());
        assert!(out_file.exists());

        // Multi-file with --out-dir and --bytes-only
        let out_dir = dir.join("out_dir");
        let in_file2 = dir.join("input2.svg");
        std::fs::write(&in_file2, svg).unwrap();
        let res2 = run_iter(vec![
            in_file.to_str().unwrap().to_string(),
            in_file2.to_str().unwrap().to_string(),
            "--out-dir".to_string(),
            out_dir.to_str().unwrap().to_string(),
            "--bytes-only".to_string(),
            "--stats".to_string(),
        ]);
        assert!(res2.is_ok());

        // Also test stdout mode
        let res3 = run_iter(vec![in_file.to_str().unwrap().to_string()]);
        assert!(res3.is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_helpers() {
        let mut t = Report::default();
        let r = Report {
            paths: 1,
            rewritten: 1,
            primitives: 0,
            subpaths: 1,
            segments_before: 4,
            segments_after: 3,
            params_before: 8.0,
            params_after: 6.0,
            guarded: 0,
            tolerance_units: 0.1,
        };
        add(&mut t, &r);
        assert_eq!(t.paths, 1);
        let s = line(&r);
        assert!(s.contains("25.0% smaller"));
        let p = Path::new("test.svg");
        assert_eq!(name(p), "test.svg");
    }
}
