"""The renderer floor estimated per image, checked against the artists' own geometry.

`docs/theory/chain-boundary.md` (Phase 2, milestone 2): the engine's forward model is the
exact geometry, and what the renderer added beyond 8-bit rounding is a floor the evidence
estimates from the image itself, so that `chi2/M` at the truth stays near 1 whatever made
the input. This script makes the inputs, runs the engine's evidence on them
(`crates/inkvec-trace/examples/evidence_calibrate.rs`), and scores every run window against
the truth.

Inputs, per icon and size, all drawn from the artist's file:

* `exact`     per-element compositing with exact coverage at 8x, box-filtered, 8-bit;
* `ss8`       per-element compositing with 8 x 8 point samples per pixel, 8-bit;
* `lat32`     tiny-skia's sample lattice (4 x 4 per 8x pixel: 32 per pixel), exact
              flattening, 8-bit;
* `fresh`     resvg 0.48.1 now (`build_corpus_v2.render_supersampled`, the harness's code);
* `committed` the gate's committed intake (`bench/data/corpus_raster/<family>/<size>ss`);
* `web`       the gate's noisy tier (size 400 only): the committed 512 px intake flattened
              onto white, bicubic-resized to 400 px and saved as a quality-80 JPEG with 4:2:0
              chroma (`bench/build_web_tier.py`).

The truth for all of them is the first before rounding, with the true geometry (arcs as
arcs): what the engine's exact forward model should read. Inputs, truths and the harness's
rows are cached under `bench/data/theory_floor_cache/` (ignored by git, regenerable).

Reported per input: `χ²/M` of the truth under rounding alone and under the image's
self-calibrated window variances, by class (straight, curved) and with each run's offset
profiled out; the engine's own score of the truth (`χ²_floor` and Huber's cost per
independent measurement, replicas counted once, by family); and each image's window scale
and tail.

    cargo build --release -p inkvec-trace --example evidence_calibrate
    python3 bench/theory/floor_selfcal.py [--per-family 6] [--sizes 128,512]
"""

from __future__ import annotations

import argparse
import io
import math
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

import numpy as np
from PIL import Image

import renderer_floor as rf

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "bench"))
CACHE = ROOT / "bench/data/theory_floor_cache"
CONDITIONS = ("exact", "ss8", "lat32", "fresh", "committed")
ALL_CONDITIONS = CONDITIONS + ("web",)
EXE = ROOT / "target/release/examples/evidence_calibrate"
W4 = np.array([1.0, -4.0, 6.0, -4.0, 1.0])
COLS = ["name", "edge", "index", "axis", "line", "lo", "hi", "s", "sum", "qvar", "truth",
        "left_low", "pos", "closed", "lattice", "z", "var"]
FLOOR_COLS = ["lattice", "window_var", "edge_var", "sigma", "ramp_width", "soft", "ringing",
              "extra_straight", "extra_curved", "n_straight", "n_curved", "nu", "window_scale",
              "sigma_edge0"]


# ---------------------------------------------------------------------------------- inputs

def render_points(layers, size, sub):
    """Per-element compositing at 1x with `sub x sub` point samples per pixel."""
    acc = np.zeros((size, size, 4))
    for reg, col, _, _ in layers:
        c = rf.lattice_coverage(reg, size, ss=1, sub=sub)
        acc = c[..., None] * np.array([*col, 1.0]) + (1 - c[..., None]) * acc
    return acc


def encode_png(pm, path):
    """Premultiplied RGBA -> straight 8-bit PNG, rounded as the corpus builder rounds."""
    a = pm[..., 3:4]
    rgb = np.where(a > 1e-6, pm[..., :3] / np.maximum(a, 1e-6), 0.0)
    out = np.concatenate([rgb, a], -1)
    Image.fromarray((np.clip(out, 0, 1) * 255 + 0.5).astype(np.uint8), "RGBA").save(path)


def inputs_for(item, size, conditions):
    """{condition: PNG path} and the truth file (sRGB over white, f32), cached; None when the
    file uses what `renderer_floor.layers_of` does not draw."""
    import build_corpus_v2 as B
    d = CACHE / f"{size}" / item["corpus"]
    d.mkdir(parents=True, exist_ok=True)
    stem = item["stem"]
    truth_path = d / f"{stem}.truth.rgbf32"
    svg = (ROOT / "bench/data/corpus_svg" / item["corpus"] / f"{stem}.svg").read_text()
    layers, truth = None, None
    if not truth_path.exists():
        layers = rf.layers_of(svg, size, "true")
        if not layers:
            return None, None
        truth = rf.render_layered(layers, size, rf.SS)
        over_white = truth[..., :3] + (1.0 - truth[..., 3:4])
        over_white.astype("<f4").tofile(truth_path)
        np.save(d / f"{stem}.truth.npy", truth.astype(np.float32))
    out = {}
    for cond in conditions:
        if cond in ("committed", "web"):
            tier = (f"{size}ss", "png") if cond == "committed" else ("web", "jpg")
            p = ROOT / "bench/data/corpus_raster" / item["corpus"] / tier[0] / f"{stem}.{tier[1]}"
            if p.exists():
                out[cond] = p
            continue
        p = d / f"{stem}.{cond}.png"
        if not p.exists():
            if cond == "fresh":
                p.write_bytes(B.render_supersampled(svg, size, rf.SS))
            else:
                if layers is None:
                    layers = rf.layers_of(svg, size, "true")
                if truth is None:
                    truth = np.load(d / f"{stem}.truth.npy").astype(np.float64)
                if cond == "exact":
                    encode_png(truth, p)
                elif cond == "ss8":
                    encode_png(render_points(layers, size, 8), p)
                elif cond == "lat32":
                    encode_png(rf.render_lattice(layers, size), p)
        out[cond] = p
    return out, truth_path


def pick_items(per_family, seed=1):
    import svgeval
    items = svgeval.load_sets()["screen"]
    rng = np.random.default_rng(seed)
    by_fam = defaultdict(list)
    for it in items:
        by_fam[it["corpus"]].append(it)
    pick = []
    for _, its in sorted(by_fam.items()):
        its = [it for it in its if rf.layers_of_ok(it)]
        rng.shuffle(its)
        pick += its[:per_family]
    return pick


# ------------------------------------------------------------------------------ the rows

def rows_for(items, size, cond, exe, refresh=False):
    """The harness's rows for every item at `size` under `cond` (cached TSV), and the floor
    it estimated per image."""
    out = CACHE / f"{size}" / f"rows.{cond}.tsv"
    if refresh or not out.exists():
        manifest = CACHE / f"{size}" / f"manifest.{cond}.tsv"
        lines = []
        for it in items:
            try:
                paths, truth = inputs_for(it, size, [cond])
            except Exception as ex:  # a file this cannot draw is skipped, and said so
                print(f"  skip {it['corpus']}/{it['stem']}: {type(ex).__name__}: {ex}", file=sys.stderr)
                continue
            if paths and cond in paths:
                lines.append(f"{it['corpus']}/{it['stem']}\t{paths[cond]}\t{truth}")
        manifest.write_text("\n".join(lines) + "\n")
        with open(out, "w") as f:
            subprocess.run([str(exe), str(manifest)], stdout=f, check=True)
    rows, floors, edges = [], {}, []
    for line in out.read_text().splitlines():
        f = line.split("\t")
        if f[0] == "#floor":
            floors[f[1]] = dict(zip(FLOOR_COLS, (float(v) for v in f[2:])))
            continue
        if f[0] == "#edge":
            edges.append((f[1], int(f[2]), int(f[3]), float(f[4]), float(f[5]), float(f[6]), float(f[7])))
            continue
        rows.append(f)
    if not rows:
        floors["__edges__"] = edges
        return None, floors
    cols = list(zip(*rows))
    t = {}
    for k, c in zip(COLS, cols):
        if k in ("name", "axis"):
            t[k] = np.array(c)
        elif k in ("edge", "index", "line", "lo", "hi", "left_low", "closed", "lattice"):
            t[k] = np.array(c, dtype=int)
        else:
            t[k] = np.array(c, dtype=float)
    t["family"] = np.array([n.split("/")[0] for n in t["name"]])
    floors["__edges__"] = edges
    return t, floors


# ------------------------------------------------------------------------------ analysis

def runs(t):
    """Index arrays of each edge's windows, in order (rows arrive grouped by image and edge)."""
    key = np.char.add(t["name"], np.char.add("#", t["edge"].astype(str)))
    out, start = [], 0
    for i in range(1, len(key) + 1):
        if i == len(key) or key[i] != key[start]:
            out.append(np.arange(start, i))
            start = i
    return out


def curvature(t, rr):
    """|second difference| of the measured positions at each window of a run (consecutive
    windows of one axis), NaN where undefined."""
    k = np.full(len(t["pos"]), np.nan)
    for r in rr:
        for j in range(1, len(r) - 1):
            a, b, c = r[j - 1], r[j], r[j + 1]
            if t["axis"][a] == t["axis"][b] == t["axis"][c] and \
                    abs(t["line"][b] - t["line"][a]) == 1 and abs(t["line"][c] - t["line"][b]) == 1:
                k[b] = abs(t["pos"][a] - 2 * t["pos"][b] + t["pos"][c])
    return k


def huber(z, k):
    a = np.abs(z)
    return np.where(a <= k, z * z, 2 * k * a - k * k)


def report(size, cond, t, floors):
    """chi2/M of the truth under rounding alone and under the image's own calibrated noise
    (plain and with Huber's cost at the image's kappa), over all windows and by class; and
    with each run's offset profiled out (what a fitted candidate sees)."""
    r = t["sum"] - t["truth"]
    rr = runs(t)
    kap = curvature(t, rr)
    straight = kap < 0.01
    curved = kap >= 0.01
    kappa = np.array([min(3.0, math.sqrt(floors[n]["nu"])) if math.isfinite(floors[n]["nu"]) else 3.0
                      for n in t["name"]])
    # Each run's offset, profiled out under the calibrated variances.
    prof = np.zeros(len(r))
    for run in rr:
        w = 1 / t["var"][run]
        prof[run] = r[run] - (r[run] * w).sum() / w.sum()
    m_prof = len(r) - len(rr)
    print(f"-- {size} px {cond}: {len(r)} windows in {len(rr)} runs, {len(floors) - 1} images")
    for name, m in (("all", np.ones(len(r), bool)), ("straight", straight), ("curved", curved)):
        zq = r[m] / np.sqrt(t["qvar"][m])
        zc = r[m] / np.sqrt(t["var"][m])
        hub = huber(zc, kappa[m])
        print(f"   {name:9s} n {m.sum():6d}  rounding {np.mean(zq ** 2):9.2f}  calibrated {np.mean(zc ** 2):7.2f}"
              f"  median {np.median(zc ** 2) / 0.4549:5.2f}  huber {np.mean(hub):6.2f}"
              f"  |z|>3 {np.mean(np.abs(zc) > 3):.3f}")
    zp = prof / np.sqrt(t["var"])
    print(f"   profiled  chi2/M {np.sum(zp ** 2) / m_prof:7.2f}  huber {np.sum(huber(zp, kappa)) / m_prof:6.2f}")
    ed = floors.get("__edges__", [])
    if ed:
        a = np.array([e[2:] for e in ed])  # m, dof, chi2, chi2_floor, cost
        fam = np.array([e[0].split("/")[0] for e in ed])
        print(f"   engine score of the truth: chi2_floor/dof {a[:, 3].sum() / a[:, 1].sum():6.2f}  "
              f"huber/dof {a[:, 4].sum() / a[:, 1].sum():6.2f}  chi2/dof {a[:, 2].sum() / a[:, 1].sum():7.2f}  "
              f"dof/m {a[:, 1].sum() / a[:, 0].sum():.2f}")
        print("   by family (chi2_floor/dof, huber/dof): " + "  ".join(
            f"{f} {a[fam == f, 3].sum() / a[fam == f, 1].sum():.2f}/{a[fam == f, 4].sum() / a[fam == f, 1].sum():.2f}"
            for f in sorted(set(fam))))
    imgs = {k: v for k, v in floors.items() if k != "__edges__"}
    ws = [f["window_scale"] for f in imgs.values()]
    nus = [f["nu"] for f in imgs.values()]
    print(f"   window_scale median {np.median(ws):.2f} [{min(ws):.2f}, {max(ws):.2f}]  nu median {np.median(nus):.1f}"
          f"  finite {sum(math.isfinite(v) for v in nus)}/{len(nus)}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--per-family", type=int, default=3)
    ap.add_argument("--sizes", default="128")
    ap.add_argument("--conditions", default=",".join(CONDITIONS))
    ap.add_argument("--exe", default=str(EXE))
    ap.add_argument("--refresh", action="store_true", help="rerun the harness")
    a = ap.parse_args()
    items = pick_items(a.per_family)
    for size in [int(s) for s in a.sizes.split(",")]:
        for cond in a.conditions.split(","):
            t, floors = rows_for(items, size, cond, a.exe, a.refresh)
            if t is not None:
                report(size, cond, t, floors)


if __name__ == "__main__":
    main()
