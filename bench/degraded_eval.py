"""Degraded-input evaluation: real-world damage the CI gate cannot see, scored against the
artist's file.

    python bench/degraded_eval.py OLD_EXE [NEW_EXE] [--groups up4bic,down2up,...] [--all]
        [--mode quality|fast] [--workers 2] [--data DIR] [--json OUT.json] [--keep DIR]
        [-- extra tracer flags]

A local benchmark, not a gate condition. The CI gate (`bench/ci_gate.py`) scores rasters
rendered from the artist's SVG with a box filter at 128 and 512 px: clean, native input by
construction. Every failure the r2-inputs research found on real input (2026-10-02) was
invisible to it: a nearest-neighbour 3.5x zoom traced at 13 inks where the art has 3, a
4x bicubic upscale at 8x the artist's parameters, and the first soft-intake candidate's
glow regression (a native 512 px render with a soft glow reduced to 64 px, dE00 +47 %),
which its own degraded benchmark had no glow case to catch.

# The inputs

The r2-inputs stress set, built by that research's `stress.py` under `--data` (default
`out/r2-inputs`): `DATA/inputs/<group>/<id>.<ext>` is the degraded raster and
`DATA/gt/<group>/<id>.png` the artist's file rendered at 1024 px on the long side over white,
transformed the way the group transforms its input (a glow or a shadow applied to the
artist's render as well). The degradations follow the practical degradation models of
BSRGAN and Real-ESRGAN -- random blur, down- and up-sampling with nearest, bilinear and
bicubic kernels, noise and JPEG, applied to clean renders. Method from: K. Zhang, J. Liang,
L. Van Gool, R. Timofte, "Designing a Practical Degradation Model for Deep Blind Image
Super-Resolution", ICCV 2021, arXiv 2103.14006; X. Wang, L. Xie, C. Dong, Y. Shan,
"Real-ESRGAN: Training Real-World Blind Super-Resolution with Pure Synthetic Data", ICCVW
2021, arXiv 2107.10833 (as surveyed in the r2-inputs report, section 4.1).

Default groups (`DEFAULT_GROUPS`), the eight the report's 5.8 asks for:

* **up4bic**: a 128 px render, bicubic 4x to 512 (RGBA);
* **down2up**: a 256 px render, bicubic 2x to 512;
* **nn35**: a 128 px render, nearest-neighbour 3.5x (cells of 3 and 4 pixels);
* **nn4jpeg**: a 128 px render over white, nearest 4x, then JPEG q85;
* **jpeg50png**: a 512 px render, JPEG q50, decoded and saved as PNG (the container lies);
* **glow**: a 512 px render with a sigma-12 px blue outer glow at 75 % under the art;
* **cleartype**: a screenshot -- icons at 96 px, wordmarks 384 wide -- with ClearType's
  3x horizontal sub-pixel coverage and 5-tap [1,2,3,2,1]/9 filter;
* **scan**: a 512 px render over white, blur 1.0, noise 4 levels, JPEG q85.

`--groups` takes any group present under `DATA/inputs` (the stress set has 38: `shadow`,
`up3lanw`, `nn4`, `clean512` and the controls among them). By default each group is traced on
`CORE`, two sources per family where the set has them (15 of its 28); `--all` uses all 28.

# What is measured, per input

* `de00`: our SVG rendered at the reference's size and composited over white, against the
  reference, CIEDE2000 mean (`inkvec_bench.metrics.color.delta_e00`), as `stress.py` scored.
* `ratio`: emitted parameters over the artist's (`svgmodel.parse(svg).n_params`).
* `seconds` (wall, at below-normal priority with one rayon thread, so robust to nothing but
  comparable within a run), `bytes`, and `intake`: what the intake did, read from the
  tracer's own stage lines (`unblock ... upscale of WxH`, `soft intake ... tracing at WxH`).

With two executables the rows are paired by input, and per group the summary gives the
means of both, identical SVGs, better/worse counts beyond 1e-4 dE00, a paired bootstrap 95 %
interval of the relative change of the mean dE00 (2000 resamples, seed 0, percentile
interval), the per-family means and the five worst regressions. Method from: P. Koehn,
"Statistical Significance Tests for Machine Translation Evaluation", EMNLP 2004,
https://aclanthology.org/W04-3250 (paired bootstrap resampling of a fixed test set), as in
`bench/gate_stats.py`; unstratified here, because this is a reading aid and not a gate.

Rows are cached per executable hash, group, mode and flags under `bench/data/_cache/degraded/`
(`INKVEC_CACHE_SALT` joins the key, as in `svgeval`), so the baseline side of an A/B is
traced once. Traces run at below-normal priority with one rayon thread per worker.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import time
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "bench"))
import svgeval  # noqa: E402

BELOW_NORMAL = 0x00004000


def default_data() -> Path:
    """`out/r2-inputs` of this checkout, or of the main checkout when this is a git worktree
    (`out/` is not versioned, so a worktree has none of its own)."""
    here = ROOT / "out" / "r2-inputs"
    if here.is_dir():
        return here
    try:
        common = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "--git-common-dir"],
                                capture_output=True, text=True, timeout=30).stdout.strip()
        main = (ROOT / common).resolve().parent / "out" / "r2-inputs"
        if main.is_dir():
            return main
    except (OSError, subprocess.SubprocessError):
        pass
    return here
DEFAULT_GROUPS = ("up4bic", "down2up", "nn35", "nn4jpeg", "jpeg50png", "glow", "cleartype", "scan")

#: Every source of the stress set: id -> (family, the artist's parameter count).
SOURCES = {
    "brands__bbcollects_com": ("brands", 118),
    "brands__betterimpact_com": ("brands", 1677),
    "brands__dribbble_com": ("brands", 882),
    "brands__jurlique_com_au": ("brands", 718),
    "brands__kurkul_com": ("brands", 2180),
    "brands__rent_com_au": ("brands", 510),
    "brands__sangchaimeter_com": ("brands", 2447),
    "brands__shinhancard_com": ("brands", 1510),
    "lucide__app-window-mac": ("lucide", 12),
    "lucide__diff": ("lucide", 6),
    "lucide__omega": ("lucide", 43),
    "material-icons__12mp": ("material-icons", 160),
    "material-icons__flight": ("material-icons", 44),
    "material-icons__phone_iphone": ("material-icons", 68),
    "noto-emoji__emoji_u0030": ("noto-emoji", 108),
    "noto-emoji__emoji_u1f46c_1f3fd": ("noto-emoji", 2590),
    "noto-emoji__emoji_u1f93c_1f3fe_200d_2640": ("noto-emoji", 2182),
    "openmoji__1F195": ("openmoji", 34),
    "openmoji__1F468-200D-1F9B1": ("openmoji", 436),
    "openmoji__1F9B3": ("openmoji", 66),
    "simple-icons__alchemy": ("simple-icons", 265),
    "simple-icons__fluxer": ("simple-icons", 228),
    "simple-icons__osano": ("simple-icons", 28),
    "synthetic__gradient_linear": ("synthetic", 12),
    "synthetic__prim_ellipse": ("synthetic", 10),
    "twemoji__1f17e": ("twemoji", 80),
    "twemoji__1f469-1f3ff-200d-1f91d-200d-1f468-1f3fc": ("twemoji", 928),
    "twemoji__1f7eb": ("twemoji", 32),
}

#: Two sources per family (one for the synthetic pair): a line icon and a filled one, an
#: emoji with flat fills and one with shading, two wordmarks.
CORE = (
    "lucide__app-window-mac", "lucide__omega",
    "material-icons__12mp", "material-icons__flight",
    "noto-emoji__emoji_u0030", "noto-emoji__emoji_u1f93c_1f3fe_200d_2640",
    "openmoji__1F195", "openmoji__1F468-200D-1F9B1",
    "simple-icons__alchemy", "simple-icons__fluxer",
    "twemoji__1f17e", "twemoji__1f469-1f3ff-200d-1f91d-200d-1f468-1f3fc",
    "synthetic__prim_ellipse",
    "brands__jurlique_com_au", "brands__betterimpact_com",
)

INTAKE_RE = (
    (re.compile(r"unblock\s+\d+x\d+ is a (\S+)x pixel upscale of (\d+x\d+)"), "unblock {0} to {1}"),
    (re.compile(r"soft intake .*tracing at (\d+x\d+) \(/(\d+)\)"), "soft /{1} to {0}"),
    (re.compile(r"soft intake .*kept as is: (.+)$"), "soft kept: {0}"),
    (re.compile(r"restore\s+residual ([\d.]+) > [\d.]+.*restored"), "restored (residual {0})"),
    (re.compile(r"restore\s+residual ([\d.]+) <= [\d.]+, traced directly"), "restore kept (residual {0})"),
    (re.compile(r"restore\s+residual ([\d.]+) > [\d.]+, but no restorer"), "restore wanted, none available (residual {0})"),
)


def input_path(data: Path, group: str, sid: str) -> Path | None:
    """The degraded raster of source `sid` in `group`, whatever its extension."""
    hits = sorted((data / "inputs" / group).glob(f"{sid}.*"))
    return hits[0] if hits else None


def intake_note(stderr: str) -> str:
    """What the intake did, from the tracer's stage lines; empty when it did nothing."""
    notes = []
    for line in stderr.splitlines():
        for rx, fmt in INTAKE_RE:
            m = rx.search(line)
            if m:
                notes.append(fmt.format(*m.groups()))
    return "; ".join(notes)


def score_one(job: tuple) -> dict:
    """Trace one input with one executable and measure it (see the module docs)."""
    exe, data, group, sid, mode, flags, keep = job
    from PIL import Image
    from inkvec_bench import render, svgmodel
    from inkvec_bench.metrics import color as mcolor
    data = Path(data)
    png = input_path(data, group, sid)
    key = f"{group}/{sid}"
    if png is None:
        return {"key": key, "fail": "no input"}
    out_dir = Path(keep) if keep else svgeval.CACHE / "degraded" / "svg"
    out_dir = out_dir / Path(exe).stem / mode / group
    out_dir.mkdir(parents=True, exist_ok=True)
    out = out_dir / f"{sid}.svg"
    args = [str(exe), str(png), "-o", str(out)] + (["--mode", "fast"] if mode == "fast" else []) + list(flags)
    env = {**os.environ, "RAYON_NUM_THREADS": "1"}
    env.pop("INKVEC_DIAG", None)
    # One retry: on a machine short of memory a trace can fail to allocate and exit 1, and a
    # transient failure must not be cached as the build's result.
    for attempt in range(2):
        t0 = time.time()
        try:
            r = subprocess.run(args, capture_output=True, timeout=900, env=env,
                               creationflags=BELOW_NORMAL if os.name == "nt" else 0)
        except subprocess.TimeoutExpired:
            return {"key": key, "fail": "timeout"}
        secs = time.time() - t0
        if r.returncode == 0 and out.exists():
            break
    if r.returncode != 0 or not out.exists():
        return {"key": key, "fail": f"exit {r.returncode}"}
    raw = out.read_bytes()
    svg = raw.decode("utf-8")
    ref = np.asarray(Image.open(data / "gt" / group / f"{sid}.png").convert("RGB"),
                     dtype=np.float32) / 255.0
    ours = render.composite(render.render(svg, ref.shape[1], ref.shape[0]))
    fam, gt_params = SOURCES.get(sid, (sid.split("__")[0], 1))
    row = {"key": key, "group": group, "id": sid, "family": fam, "seconds": secs,
           "bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest(),
           "de00": float(mcolor.delta_e00(ref, ours)["de00_mean"]),
           "params": svgmodel.parse(svg).n_params,
           "intake": intake_note(r.stderr.decode("utf-8", "replace"))}
    row["ratio"] = row["params"] / max(1, gt_params)
    if not keep:
        out.unlink(missing_ok=True)
    return row


def _init():
    for k in svgeval.PIN_VARS:
        os.environ[k] = "1"
    if os.name == "nt":
        import ctypes
        ctypes.windll.kernel32.SetPriorityClass(ctypes.windll.kernel32.GetCurrentProcess(), BELOW_NORMAL)


def cache_file(exe: Path, group: str, mode: str, flags: list[str]) -> Path:
    """Where one build's rows for one group, mode and flag set are cached."""
    salt = os.environ.get("INKVEC_CACHE_SALT", "") + "|" + " ".join(flags)
    salt = hashlib.sha1(salt.encode()).hexdigest()[:8]
    return svgeval.CACHE / "degraded" / f"{svgeval.exe_key(exe)}-{group}-{mode}-{salt}-v1.json"


def run(exe: Path, data: Path, group: str, mode: str, flags: list[str], ids: list[str],
        workers: int, keep: str | None) -> dict:
    """Per-input rows for one executable and group, from the cache where possible."""
    cf = cache_file(exe, group, mode, flags)
    have = json.loads(cf.read_text(encoding="utf-8")) if cf.exists() and not keep else {}
    todo = [s for s in ids if f"{group}/{s}" not in have]
    if todo:
        jobs = [(str(exe), str(data), group, s, mode, flags, keep) for s in todo]
        if workers <= 1 or len(jobs) == 1:
            rows = list(map(score_one, jobs))
        else:
            with ProcessPoolExecutor(min(workers, len(jobs)), initializer=_init) as ex:
                rows = list(ex.map(score_one, jobs, chunksize=1))
        # Failures are reported but not cached, so the next run tries them again.
        failed = {r["key"]: r for r in rows if "fail" in r}
        have.update({r["key"]: r for r in rows if "fail" not in r})
        if not keep:
            cf.parent.mkdir(parents=True, exist_ok=True)
            cf.write_text(json.dumps(have), encoding="utf-8")
        have = {**have, **failed}
    return {k: have[k] for k in (f"{group}/{s}" for s in ids) if k in have}


def _mean(rows: dict, m: str) -> float:
    v = [r[m] for r in rows.values() if "fail" not in r and m in r]
    return float(np.mean(v)) if v else float("nan")


def summary(group: str, a: dict, b: dict | None) -> tuple[list[str], dict]:
    """The group's lines and its JSON record (see the module docs)."""
    lines = [f"== {group}: {len(a)} inputs"]
    doc: dict = {"group": group, "n": len(a)}
    for m in ("de00", "ratio", "seconds"):
        ma = _mean(a, m)
        doc[f"A_{m}"] = ma
        if b:
            mb = _mean(b, m)
            doc[f"B_{m}"] = mb
            lines.append(f"  {m:<8}{ma:>10.4f}{mb:>10.4f}{(mb / ma - 1) * 100 if ma else float('nan'):>+9.1f}%")
        else:
            lines.append(f"  {m:<8}{ma:>10.4f}")
    ra = [r["ratio"] for r in a.values() if "fail" not in r]
    doc["A_ratio_median"] = float(np.median(ra)) if ra else float("nan")
    for tag, rows in (("A", a), ("B", b)):
        fails = [k for k, r in (rows or {}).items() if "fail" in r]
        if fails:
            lines.append(f"  failures {tag}: {len(fails)} ({', '.join(fails[:4])})")
    if not b:
        acted = sum(1 for r in a.values() if r.get("intake"))
        lines.append(f"  intake acted on {acted}/{len(a)}")
        return lines, doc
    rb = [r["ratio"] for r in b.values() if "fail" not in r]
    doc["B_ratio_median"] = float(np.median(rb)) if rb else float("nan")
    keys = [k for k in a if k in b and "fail" not in a[k] and "fail" not in b[k]]
    same = sum(1 for k in keys if a[k]["sha256"] == b[k]["sha256"])
    d = np.array([b[k]["de00"] - a[k]["de00"] for k in keys])
    av = np.array([a[k]["de00"] for k in keys])
    better, worse = int((d < -1e-4).sum()), int((d > 1e-4).sum())
    lines.append(f"  identical SVGs {same}/{len(keys)}; dE00 better {better}, worse {worse}; "
                 f"ratio median {doc['A_ratio_median']:.2f} -> {doc['B_ratio_median']:.2f}")
    doc.update(identical=same, better=better, worse=worse)
    if len(keys) > 2 and av.mean() > 0:
        rng = np.random.default_rng(0)
        idx = rng.integers(0, len(keys), size=(2000, len(keys)))
        rel = (av[idx] + d[idx]).mean(axis=1) / av[idx].mean(axis=1) - 1
        lo, hi = np.percentile(rel, [2.5, 97.5]) * 100
        lines.append(f"  dE00 change {d.mean() / av.mean() * 100:+.2f}%  (95% CI {lo:+.2f}% .. {hi:+.2f}%)")
        doc.update(de00_rel=float(d.mean() / av.mean()), ci=[float(lo), float(hi)])
    fams = sorted({a[k]["family"] for k in keys})
    doc["families"] = {}
    for f in fams:
        ks = [k for k in keys if a[k]["family"] == f]
        fa, fb = np.mean([a[k]["de00"] for k in ks]), np.mean([b[k]["de00"] for k in ks])
        doc["families"][f] = [float(fa), float(fb)]
        lines.append(f"    {f:<15} n={len(ks):<2} dE00 {fa:.4f} -> {fb:.4f}")
    order = np.argsort(d)[::-1][:5]
    worst = [(keys[i], float(d[i])) for i in order if d[i] > 1e-4]
    doc["worst"] = worst
    for k, dv in worst:
        lines.append(f"  worse: {k}  {a[k]['de00']:.4f} -> {b[k]['de00']:.4f} ({dv:+.4f})"
                     f"  [{b[k].get('intake') or 'intake did nothing'}]")
    acted = [(k, b[k]["intake"]) for k in keys if b[k].get("intake") != a[k].get("intake")]
    for k, note in acted[:40]:
        lines.append(f"  intake: {k}: {a[k].get('intake') or '-'} -> {note or '-'}")
    return lines, doc


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("exes", nargs="+", type=Path, help="one executable, or OLD NEW")
    ap.add_argument("--groups", default=",".join(DEFAULT_GROUPS))
    ap.add_argument("--all", action="store_true", help="every source, not CORE")
    ap.add_argument("--mode", default="quality", choices=("quality", "fast"))
    ap.add_argument("--workers", type=int, default=2)
    ap.add_argument("--data", type=Path, default=None,
                    help="the stress set (default: out/r2-inputs, the main checkout's in a worktree)")
    ap.add_argument("--json", type=Path)
    ap.add_argument("--keep", help="write the SVGs here instead of a temporary folder")
    ap.add_argument("--new-flags", default="",
                    help="tracer flags for the second executable only, space-separated "
                         "(e.g. '--restore auto' against the default)")
    ap.add_argument("flags", nargs="*", help="extra tracer flags for both, after --")
    a = ap.parse_args()
    if len(a.exes) > 2:
        ap.error("one executable, or two to compare")
    a.data = a.data or default_data()
    if not (a.data / "inputs").is_dir():
        ap.error(f"no stress set under {a.data} (build it with the r2-inputs stress.py)")
    exes = [e.resolve() for e in a.exes]
    ids = sorted(SOURCES) if a.all else list(CORE)
    report = []
    for group in [g for g in a.groups.split(",") if g]:
        present = [s for s in ids if input_path(a.data, group, s) is not None]
        if not present:
            print(f"== {group}: no inputs under {a.data / 'inputs' / group}")
            continue
        flags = [a.flags, a.flags + a.new_flags.split()]
        rows = [run(e, a.data, group, a.mode, f, present, a.workers, a.keep)
                for e, f in zip(exes, flags)]
        lines, doc = summary(group, rows[0], rows[1] if len(rows) > 1 else None)
        print("\n".join(lines), flush=True)
        report.append(doc)
    if a.json:
        a.json.parent.mkdir(parents=True, exist_ok=True)
        a.json.write_text(json.dumps({"exes": [str(e) for e in exes], "mode": a.mode,
                                      "flags": a.flags, "new_flags": a.new_flags,
                                      "groups": report}, indent=1),
                          encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
