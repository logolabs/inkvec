"""Resampled-input evaluation: the screen set upscaled 2x, plus the Studio crest sample.

    python bench/resampled_eval.py OLD_EXE [NEW_EXE] [--kinds ring2x,soft2x,crest]
        [--mode quality|fast] [--set screen] [--n N] [--workers 2] [--json OUT.json]
        [--keep DIR]

A local benchmark, not a gate condition. The CI gate (`bench/ci_gate.py`) scores rasters
rendered from the artist's SVG with a box filter (the 128ss / 512ss tiers), and a box filter
has no negative lobe, so those rasters never carry ringing. Real input often does: it was
resized by an image editor or a web pipeline first. The Studio's own crest sample is such a
file, and the palette regression it shows (one gold traced as three inks, the dots notched;
bisected to 772771b, v0.1.4, by the r2-palette research of 2026-10-02) stayed invisible to
every gate run for seven releases. This script makes that class of input measurable.

# The inputs

For every icon of the chosen set (default `screen`, 246 icons), its committed 128ss raster
is upscaled 2x with Pillow and written once to `bench/data/_cache/raster/<kind>/`:

* **ring2x**: `Image.resize(..., LANCZOS)`. Pillow resamples an RGBA image premultiplied
  (it converts to `RGBa`, resizes and converts back), and the Lanczos kernel's negative lobes
  overshoot at every edge. With alpha clipped at 1, the overshoot of an opaque edge becomes
  an opaque pixel `k * s` with `k > 1`: a rim brighter (or darker) than the ink it borders.
  That is the crest disease, reproduced on the whole screen set.
* **soft2x**: `Image.resize(..., BILINEAR)`. No overshoot; edges two pixels wide, so every
  anti-aliasing ramp carries intermediate colours on several pixels instead of one.
* **crest**: `studio/src-tauri/samples/crest-filigree.png`, 512 px, as shipped. It has no
  ground truth, so it is reported by its palette and structure only.

The artist's SVGs stay the ground truth for ring2x and soft2x: an upscaled raster of the
same drawing should trace to the same drawing.

# What is measured, per icon

* `de00`: the gate's colour error, computed as `svgeval.score_one` does: our SVG rendered
  at `svgeval.JUDGE_SIZE` (1024) and composited over white, against the cached 8-bit
  render of the artist's file, CIEDE2000 mean (`inkvec_bench.metrics.color.delta_e00`).
* `ratio`: emitted parameters over the artist's (`svgmodel.parse(svg).n_params / gt_params`).
* `fills`: distinct flat paints in our file (hex and opacity).
* `invented`: our flat paints farther than `MATCH` (2.5) CIEDE2000 from every colour the
  artist wrote: flat fills, every gradient stop, and every translucent fill composited over
  white and over each opaque artist fill.
* `dupes`: pairs of our flat paints at the same opacity less than `DUPE` (1.0) CIEDE2000
  apart, i.e. one ink written as two hexes.
* `paths`: painted shapes in our file; `bytes`: its size.

The palette counts are vocabulary-level (colours compared as written, not where they are
painted), which is the cheap form of the r2-palette scorer
(`tmp/r2-palette/palscore.py`, which renders a coverage map per paint). The coverage form
is the one to use for a research claim; this one is for a quick A/B and it agrees with the
coverage form on the direction of every change the research measured.

With two executables the rows are paired by icon and the summary gives, per kind: the mean
of each measure for both, the per-icon dE00 change (better / worse counts beyond 1e-4,
worst five regressions) and a paired bootstrap 95 % interval of the relative change of the
mean dE00 (2000 resamples of icons, seed 0, percentile interval). Method from: Koehn 2004,
"Statistical Significance Tests for Machine Translation Evaluation", EMNLP 2004,
https://aclanthology.org/W04-3250 (paired bootstrap resampling of a fixed test set), as in
`bench/gate_stats.py`; here unstratified, because this is a reading aid and not a gate.

Per-icon results are cached by executable hash, kind, mode and set
(`bench/data/_cache/resampled/`), so the baseline side of an A/B is traced once.

Traces run at below-normal priority with one rayon thread per worker (`--workers`, default
2), as every scoring run on this machine must.

Sources for the input construction: Pillow `Image.resize`, src/PIL/Image.py (RGBA and LA
are resized premultiplied for every filter except NEAREST); "Lanczos resampling",
Wikipedia, section Limitations (ringing: "light and dark halos along any strong edges").
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
import xml.etree.ElementTree as ET
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "bench"))
import svgeval  # noqa: E402

KINDS = ("ring2x", "soft2x", "crest")
CREST = ROOT / "studio" / "src-tauri" / "samples" / "crest-filigree.png"
MATCH = 2.5       # CIEDE2000: our paint is one of the artist's colours
DUPE = 1.0        # CIEDE2000: two of our paints are one ink written twice
BELOW_NORMAL = 0x00004000
SVG_NS = "http://www.w3.org/2000/svg"
XLINK = "http://www.w3.org/1999/xlink"
SHAPES = {"path", "circle", "ellipse", "rect", "polygon", "polyline", "line"}
SKIP = {"defs", "clipPath", "mask", "title", "desc", "style", "metadata",
        "linearGradient", "radialGradient", "pattern", "filter", "symbol"}
NAMED = {"black": "#000000", "white": "#ffffff", "red": "#ff0000", "green": "#008000",
         "blue": "#0000ff", "yellow": "#ffff00", "gray": "#808080", "grey": "#808080",
         "orange": "#ffa500", "none": "none", "transparent": "none"}


# ------------------------------------------------------------------ inputs
def input_path(kind: str, it: dict) -> Path:
    """The resampled raster of icon `it` for `kind` (built on first use), or the crest."""
    if kind == "crest":
        return CREST
    dst = svgeval.CACHE / "raster" / kind / it["corpus"] / f"{it['stem']}.png"
    if dst.exists():
        return dst
    from PIL import Image
    src = ROOT / "bench" / "data" / "corpus_raster" / it["corpus"] / "128ss" / f"{it['stem']}.png"
    im = Image.open(src).convert("RGBA")
    flt = Image.LANCZOS if kind == "ring2x" else Image.BILINEAR
    out = im.resize((im.width * 2, im.height * 2), flt)
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp = dst.with_name(f"{dst.stem}.{os.getpid()}.tmp.png")
    out.save(tmp)
    try:
        os.replace(tmp, dst)
    except OSError:
        tmp.unlink(missing_ok=True)
    return dst


# ------------------------------------------------------------------ SVG vocabulary
def parse_color(v: str | None, current: str = "#000000"):
    """A paint value as `#rrggbb`, `("url", id)`, `"none"` or None (unreadable)."""
    if v is None:
        return None
    s = v.strip()
    m = re.fullmatch(r"url\(\s*['\"]?#([^)'\"]+)['\"]?\s*\)(\s+\S+)?", s)
    if m:
        return ("url", m.group(1))
    s = s.lower()
    if s == "currentcolor":
        return current
    s = NAMED.get(s, s)
    if s == "none":
        return "none"
    if re.fullmatch(r"#[0-9a-f]{3}", s):
        s = "#" + "".join(c * 2 for c in s[1:])
    if re.fullmatch(r"#[0-9a-f]{6}", s):
        return s
    m = re.fullmatch(r"rgb\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\)", s)
    if m:
        return "#%02x%02x%02x" % tuple(min(255, int(x)) for x in m.groups())
    return None


def _num(v, default: float = 1.0) -> float:
    try:
        v = str(v).strip()
        return float(v[:-1]) / 100 if v.endswith("%") else float(v)
    except (TypeError, ValueError):
        return default


def _tag(e) -> str:
    return e.tag.split("}")[-1] if isinstance(e.tag, str) else ""


def _style(el) -> dict:
    st = {k: el.get(k) for k in ("fill", "stroke", "stop-color", "stop-opacity", "opacity",
                                 "fill-opacity", "stroke-opacity", "color") if el.get(k) is not None}
    for part in (el.get("style") or "").split(";"):
        if ":" in part:
            k, v = part.split(":", 1)
            st[k.strip()] = v.strip()
    return st


def vocabulary(text: str) -> dict:
    """The paints a document uses: `flat` {(hex, opacity rounded to 0.01)}, `stops` (every
    gradient stop colour), `shapes` (painted shapes). Opacity is the product of `opacity`
    down the tree and the paint's own `*-opacity`; `<use>` is followed."""
    root = ET.fromstring(text)
    byid = {e.get("id"): e for e in root.iter() if e.get("id")}
    out = {"flat": set(), "stops": set(), "shapes": 0}

    def stops_of(gid: str, seen: tuple = ()):
        g = byid.get(gid)
        if g is None:
            return
        st = [s for s in g if _tag(s) == "stop"]
        if not st:
            href = g.get("{%s}href" % XLINK) or g.get("href")
            if href and href.startswith("#") and href[1:] not in seen:
                stops_of(href[1:], seen + (href[1:],))
            return
        for s in st:
            c = parse_color(_style(s).get("stop-color", "#000000"))
            if isinstance(c, str) and c != "none":
                out["stops"].add(c)

    def walk(el, op: float, inh: dict):
        tg = _tag(el)
        if tg in SKIP:
            return
        sty = _style(el)
        st = dict(inh)
        st.update({k: v for k, v in sty.items()
                   if k in ("fill", "stroke", "fill-opacity", "stroke-opacity", "color")})
        op = op * _num(sty.get("opacity", 1))
        if tg in SHAPES:
            cur = parse_color(st.get("color", "#000000")) or "#000000"
            cur = cur if isinstance(cur, str) else "#000000"
            for k, default in (("fill", "#000000"), ("stroke", "none")):
                if k == "fill" and tg in ("line", "polyline") and "fill" not in st:
                    continue
                a = op * _num(st.get(f"{k}-opacity", 1))
                c = parse_color(st.get(k, default), cur)
                if isinstance(c, tuple):
                    stops_of(c[1])
                    out["shapes"] += 1
                elif c and c != "none" and a > 0.02:
                    out["flat"].add((c, round(min(1.0, a), 2)))
                    out["shapes"] += 1
        if tg == "use":
            href = el.get("{%s}href" % XLINK) or el.get("href")
            if href and href.startswith("#") and href[1:] in byid:
                ref = byid[href[1:]]
                for ch in (list(ref) if _tag(ref) == "symbol" else [ref]):
                    walk(ch, op, st)
        for ch in el:
            walk(ch, op, st)

    walk(root, 1.0, {})
    return out


def _rgb(h: str) -> np.ndarray:
    return np.array([int(h[i:i + 2], 16) for i in (1, 3, 5)], dtype=np.float64) / 255.0


def _lab(rgbs) -> np.ndarray:
    from skimage.color import rgb2lab
    return rgb2lab(np.clip(np.asarray(rgbs, dtype=np.float64), 0, 1).reshape(-1, 1, 3)).reshape(-1, 3)


def _de(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    """CIEDE2000 between every row of `a` and every row of `b`, shape (len(a), len(b))."""
    from skimage.color import deltaE_ciede2000
    if len(a) == 0 or len(b) == 0:
        return np.full((len(a), len(b)), np.inf)
    aa = np.repeat(a, len(b), axis=0)
    bb = np.tile(b, (len(a), 1))
    return deltaE_ciede2000(aa.reshape(-1, 1, 3), bb.reshape(-1, 1, 3)).reshape(len(a), len(b))


def palette_counts(ours: dict, artist: dict | None) -> dict:
    """`fills`, `dupes` and (with an artist vocabulary) `invented`; see the module docs."""
    flat = sorted(ours["flat"])
    rows = {"fills": len(flat), "shapes": ours["shapes"]}
    lab = _lab([_rgb(h) for h, _ in flat]) if flat else np.zeros((0, 3))
    d = _de(lab, lab)
    rows["dupes"] = int(sum(1 for i in range(len(flat)) for j in range(i + 1, len(flat))
                            if flat[i][1] == flat[j][1] and d[i, j] < DUPE))
    if artist is None:
        return rows
    pool = [_rgb(h) for h, _ in artist["flat"]] + [_rgb(h) for h in artist["stops"]]
    opaque = [_rgb(h) for h, a in artist["flat"] if a >= 0.999]
    for h, a in artist["flat"]:
        if a < 0.999:
            for base in [np.ones(3)] + opaque:
                pool.append(a * _rgb(h) + (1 - a) * base)
    # Our translucent paints are compared as they look over white, as the artist's are.
    ours_vis = [a * _rgb(h) + (1 - a) * np.ones(3) for h, a in flat]
    dm = _de(_lab(ours_vis) if ours_vis else np.zeros((0, 3)),
             _lab(pool) if pool else np.zeros((0, 3)))
    rows["invented"] = int((dm.min(axis=1) > MATCH).sum()) if len(pool) else len(flat)
    return rows


# ------------------------------------------------------------------ one icon
def score_one(job: tuple) -> dict:
    """Trace one input with one executable and measure it (see the module docs)."""
    exe, kind, it, mode, keep = job
    from inkvec_bench import render, svgmodel
    from inkvec_bench.metrics import color as mcolor
    png = input_path(kind, it)
    key = f"{it['corpus']}/{it['stem']}"
    out_dir = Path(keep) if keep else svgeval.CACHE / "resampled" / "svg"
    out_dir = out_dir / Path(exe).stem / kind / mode
    out_dir.mkdir(parents=True, exist_ok=True)
    out = out_dir / f"{it['corpus']}__{it['stem']}.svg"
    args = [str(exe), str(png), "-o", str(out), "--quiet"] + (["--mode", "fast"] if mode == "fast" else [])
    env = {**os.environ, "RAYON_NUM_THREADS": "1"}
    t0 = time.time()
    try:
        r = subprocess.run(args, capture_output=True, timeout=600, env=env,
                           creationflags=BELOW_NORMAL if os.name == "nt" else 0)
    except subprocess.TimeoutExpired:
        return {"key": key, "fail": "timeout"}
    secs = time.time() - t0
    if r.returncode != 0 or not out.exists():
        return {"key": key, "fail": f"exit {r.returncode}"}
    raw = out.read_bytes()
    svg = raw.decode("utf-8")
    row = {"key": key, "corpus": it["corpus"], "seconds": secs, "bytes": len(raw),
           "sha256": hashlib.sha256(raw).hexdigest(),
           "circles": svg.count("<circle") + svg.count("<ellipse")}
    if kind == "crest":
        from PIL import Image
        src = np.asarray(Image.open(png).convert("RGBA"), dtype=np.float32) / 255.0
        target = src[..., :3] * src[..., 3:4] + (1.0 - src[..., 3:4])
        mine = render.composite(render.render(svg, src.shape[1], src.shape[0]))
        row["de00"] = float(mcolor.delta_e00(target, mine)["de00_mean"])
        row.update(palette_counts(vocabulary(svg), None))
        row["inks"] = sorted(f"{h}@{a:g}" for h, a in vocabulary(svg)["flat"])
    else:
        _, gt = svgeval.item_paths(it)
        b = render.composite(render.render(svg, svgeval.JUDGE_SIZE, svgeval.JUDGE_SIZE))
        ref = svgeval.gt_render(gt, it["corpus"], it["stem"])
        row["de00"] = float(mcolor.delta_e00(ref, b)["de00_mean"])
        row["ratio"] = svgmodel.parse(svg).n_params / max(1, it.get("gt_params") or 1)
        row.update(palette_counts(vocabulary(svg), vocabulary(gt.read_text(encoding="utf-8"))))
    if not keep:
        out.unlink(missing_ok=True)
    return row


def _init():
    for k in svgeval.PIN_VARS:
        os.environ[k] = "1"
    if os.name == "nt":
        import ctypes
        ctypes.windll.kernel32.SetPriorityClass(ctypes.windll.kernel32.GetCurrentProcess(), BELOW_NORMAL)


def cache_file(exe: Path, kind: str, mode: str, set_name: str) -> Path:
    """Where one build's rows for one kind, mode and set are cached. `INKVEC_CACHE_SALT`
    joins the key, as in `svgeval`, so two environment-switched runs of one binary do not
    read each other's rows."""
    salt = os.environ.get("INKVEC_CACHE_SALT", "")
    salt = f"-{hashlib.sha1(salt.encode()).hexdigest()[:8]}" if salt else ""
    return svgeval.CACHE / "resampled" / f"{svgeval.exe_key(exe)}-{kind}-{mode}-{set_name}{salt}-v1.json"


def run(exe: Path, kind: str, mode: str, set_name: str, items: list, workers: int,
        keep: str | None) -> dict:
    """Per-icon rows for one executable and kind, from the cache where possible."""
    cf = cache_file(exe, kind, mode, set_name)
    have = json.loads(cf.read_text(encoding="utf-8")) if cf.exists() and not keep else {}
    todo = [it for it in items if f"{it['corpus']}/{it['stem']}" not in have]
    if todo:
        jobs = [(str(exe), kind, it, mode, keep) for it in todo]
        if workers <= 1 or len(jobs) == 1:
            rows = list(map(score_one, jobs))
        else:
            with ProcessPoolExecutor(min(workers, len(jobs)), initializer=_init) as ex:
                rows = list(ex.map(score_one, jobs, chunksize=1))
        for r in rows:
            have[r["key"]] = r
        if not keep:
            cf.parent.mkdir(parents=True, exist_ok=True)
            cf.write_text(json.dumps(have), encoding="utf-8")
    return {k: have[k] for k in (f"{it['corpus']}/{it['stem']}" for it in items) if k in have}


# ------------------------------------------------------------------ summary
MEASURES = ("de00", "ratio", "fills", "invented", "dupes", "shapes", "circles", "bytes")


def _mean(rows: dict, m: str) -> float:
    v = [r[m] for r in rows.values() if "fail" not in r and m in r]
    return float(np.mean(v)) if v else float("nan")


def summary(kind: str, a: dict, b: dict | None) -> tuple[list[str], dict]:
    lines = [f"== {kind}: {len(a)} inputs"]
    doc: dict = {"kind": kind, "n": len(a)}
    present = [m for m in MEASURES if any(m in r for r in a.values())]
    head = f"  {'measure':<10}{'A':>12}" + (f"{'B':>12}{'B/A-1':>10}" if b else "")
    lines.append(head)
    for m in present:
        ma = _mean(a, m)
        doc[f"A_{m}"] = ma
        if b:
            mb = _mean(b, m)
            doc[f"B_{m}"] = mb
            rel = (mb / ma - 1) * 100 if ma else float("nan")
            lines.append(f"  {m:<10}{ma:>12.4f}{mb:>12.4f}{rel:>9.1f}%")
        else:
            lines.append(f"  {m:<10}{ma:>12.4f}")
    fa = sum(1 for r in a.values() if "fail" in r)
    if fa:
        lines.append(f"  failures A: {fa}")
    if kind == "crest":
        for tag, rows in (("A", a), ("B", b)):
            for r in (rows or {}).values():
                if "inks" in r:
                    lines.append(f"  {tag} inks: {' '.join(r['inks'])}")
    if not b:
        return lines, doc
    fb = sum(1 for r in b.values() if "fail" in r)
    if fb:
        lines.append(f"  failures B: {fb}")
    keys = [k for k in a if k in b and "fail" not in a[k] and "fail" not in b[k]]
    same = sum(1 for k in keys if a[k]["sha256"] == b[k]["sha256"])
    d = np.array([b[k]["de00"] - a[k]["de00"] for k in keys])
    better, worse = int((d < -1e-4).sum()), int((d > 1e-4).sum())
    lines.append(f"  identical SVGs {same}/{len(keys)}; dE00 better {better}, worse {worse}")
    doc.update(identical=same, better=better, worse=worse)
    if len(keys) > 2:
        av = np.array([a[k]["de00"] for k in keys])
        rng = np.random.default_rng(0)
        idx = rng.integers(0, len(keys), size=(2000, len(keys)))
        rel = (av[idx] + d[idx]).mean(axis=1) / av[idx].mean(axis=1) - 1
        lo, hi = np.percentile(rel, [2.5, 97.5]) * 100
        lines.append(f"  dE00 change {d.mean() / av.mean() * 100:+.2f}%  (95% CI {lo:+.2f}% .. {hi:+.2f}%)")
        doc.update(de00_rel=float(d.mean() / av.mean()), ci=[float(lo), float(hi)])
        # The decile of icons that were worst under A, the gate's "worst tenth".
        q = np.argsort(av)[-max(1, len(keys) // 10):]
        lines.append(f"  worst tenth (by A): {av[q].mean():.4f} -> {(av[q] + d[q]).mean():.4f}")
    order = np.argsort(d)[::-1][:5]
    worst = [(keys[i], float(d[i])) for i in order if d[i] > 1e-4]
    if worst:
        lines.append("  worst regressions: " + ", ".join(f"{k} {v:+.4f}" for k, v in worst))
    doc["worst"] = worst
    return lines, doc


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("exe", type=Path)
    ap.add_argument("exe_b", type=Path, nargs="?")
    ap.add_argument("--kinds", default=",".join(KINDS))
    ap.add_argument("--mode", default="quality", choices=("quality", "fast"))
    ap.add_argument("--set", default="screen")
    ap.add_argument("--n", type=int, default=None, help="first N icons only (a quick look)")
    ap.add_argument("--workers", type=int, default=2)
    ap.add_argument("--json", type=Path, default=None, help="write the summary and rows here")
    ap.add_argument("--keep", default=None, help="keep the SVGs under this folder (no cache)")
    a = ap.parse_args()
    _init()
    items = svgeval.load_sets()[a.set]
    if a.n:
        items = items[: a.n]
    exes = [a.exe.resolve()] + ([a.exe_b.resolve()] if a.exe_b else [])
    report: dict = {"exes": [str(e) for e in exes], "mode": a.mode, "set": a.set, "kinds": {}}
    for kind in a.kinds.split(","):
        its = [{"corpus": "studio", "stem": "crest-filigree", "gt_params": None}] if kind == "crest" else items
        rows = [run(e, kind, a.mode, a.set, its, a.workers, a.keep) for e in exes]
        lines, doc = summary(kind, rows[0], rows[1] if len(rows) > 1 else None)
        print("\n".join(lines), flush=True)
        report["kinds"][kind] = {"summary": doc, "rows": rows}
    if a.json:
        a.json.parent.mkdir(parents=True, exist_ok=True)
        a.json.write_text(json.dumps(report, indent=1), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
