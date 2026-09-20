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

fn run() -> Result<(), String> {
    let mut inputs: Vec<PathBuf> = Vec::new();
    let mut output: Option<PathBuf> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut opts = Options::default();
    let mut stats = false;
    let mut it = std::env::args().skip(1);
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
        let (out, rep) = minify(&svg, &opts).map_err(|e| format!("{}: {e}", input.display()))?;
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

fn parse<T: std::str::FromStr>(s: &str) -> Result<T, String> {
    s.parse().map_err(|_| format!("not a number: {s}"))
}

fn name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

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
