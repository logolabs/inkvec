"""The referee for native transparency: what must not move, and what should.

    python bench/alpha_referee.py identity OLD.exe NEW.exe      # opaque input: byte-identical
    python bench/alpha_referee.py score NEW.exe [--compare OLD.exe] [--sets screen,probe,translucent]

`identity` composites every screen-set raster over white, so the input is opaque, and
requires the two builds to write the same bytes. Making the colour path four-channel must
reduce to today's arithmetic exactly when alpha is 1; any difference here is a refactor bug,
not a trade-off.

`score` traces the transparent originals and scores each against the artist's SVG the way
`alpha_eval.py` does -- over white (all the gate can see), over the design system's dark
ground (paint where the page should show), and on the alpha channel itself -- as mean
absolute error of the rendered pixels. Three sets:

  screen       the gate's 246 icons: transparent grounds, opaque artwork
  probe        the white-on-clear cases from `white_on_clear.py`
  translucent  corpus icons whose truth has real interior translucency (glows, shadows,
               washes) -- found by measuring the truth, cached in bench/data/translucent_set.json
"""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))

import svgeval  # noqa: E402
from inkvec_bench import render  # noqa: E402

SIZE = 320
DARK = np.array([0.078, 0.071, 0.063], np.float32)
WHITE = np.ones(3, np.float32)
WORK = ROOT / "out" / "alpha_referee"
TRANSLUCENT_CACHE = ROOT / "bench" / "data" / "translucent_set.json"
# A pixel is translucent in the interior when it and its four neighbours are all strictly
# between clear and opaque; an anti-aliased rim always touches one or the other.
INTERIOR_TRANSLUCENT = 0.005


def rgba(svg: str) -> np.ndarray:
    a = render.render(svg, SIZE, SIZE)
    if a.shape[-1] == 3:
        a = np.concatenate([a, np.ones(a.shape[:2] + (1,), np.float32)], axis=2)
    return a


def over(img: np.ndarray, g: np.ndarray) -> np.ndarray:
    al = img[..., 3:4]
    return np.clip(img[..., :3] * al + g * (1.0 - al), 0.0, 1.0)


def score(trace: np.ndarray, truth: np.ndarray) -> tuple[float, float, float]:
    return (float(np.abs(over(trace, WHITE) - over(truth, WHITE)).mean()),
            float(np.abs(over(trace, DARK) - over(truth, DARK)).mean()),
            float(np.abs(trace[..., 3] - truth[..., 3]).mean()))


def interior_translucency(truth: np.ndarray) -> float:
    a = truth[..., 3]
    mid = (a > 0.05) & (a < 0.95)
    m = mid.copy()
    m[1:, :] &= mid[:-1, :]
    m[:-1, :] &= mid[1:, :]
    m[:, 1:] &= mid[:, :-1]
    m[:, :-1] &= mid[:, 1:]
    return float(m.mean())


def trace(exe: str, png: Path, out: Path, extra: list[str]) -> bool:
    r = subprocess.run([exe, str(png), "-o", str(out), "-q", *extra],
                       capture_output=True, text=True, timeout=300)
    return r.returncode == 0 and out.is_file()


def screen_items() -> list[tuple[str, Path, Path]]:
    sets = svgeval.load_sets()
    out = []
    for it in sets["screen"]:
        png, gt = svgeval.item_paths(it)
        out.append((f"{it['corpus']}/{it['stem']}", png, gt))
    return out


def probe_items() -> list[tuple[str, Path, Path]]:
    import white_on_clear as woc

    d = WORK / "probe"
    d.mkdir(parents=True, exist_ok=True)
    out = []
    for name, svg in woc.CASES.items():
        png, gt = d / f"{name}.png", d / f"{name}.svg"
        if not png.exists():
            png.write_bytes(render.render_to_png(svg, 512, 512))
            gt.write_text(svg, encoding="utf-8")
        out.append((f"probe/{name}", png, gt))
    return out


def translucent_items(workers: int) -> list[tuple[str, Path, Path]]:
    if not TRANSLUCENT_CACHE.exists():
        sets = svgeval.load_sets()
        items = sets["all"]

        def measure(it):
            png, gt = svgeval.item_paths(it)
            try:
                t = interior_translucency(rgba(gt.read_text(encoding="utf-8")))
            except Exception:
                return None
            return (it["corpus"], it["stem"], t) if t > INTERIOR_TRANSLUCENT else None

        with ThreadPoolExecutor(workers) as ex:
            found = [r for r in ex.map(measure, items) if r]
        found.sort()
        TRANSLUCENT_CACHE.write_text(json.dumps(
            [{"corpus": c, "stem": s, "interior": round(t, 4)} for c, s, t in found], indent=1),
            encoding="utf-8")
        print(f"translucent set: {len(found)} of {len(items)} icons "
              f"(interior translucency > {INTERIOR_TRANSLUCENT:.1%}), cached")
    out = []
    for it in json.loads(TRANSLUCENT_CACHE.read_text(encoding="utf-8")):
        png, gt = svgeval.item_paths(it)
        out.append((f"{it['corpus']}/{it['stem']}", png, gt))
    return out


def exe_tag(exe: str) -> str:
    return hashlib.sha256(Path(exe).read_bytes()).hexdigest()[:10]


def cmd_identity(a) -> int:
    items = screen_items()
    opaque = WORK / "opaque"
    opaque.mkdir(parents=True, exist_ok=True)
    tags = {e: exe_tag(e) for e in (a.old, a.new)}
    for e in tags:
        (WORK / tags[e]).mkdir(parents=True, exist_ok=True)

    def one(item):
        name, png, _ = item
        flat = opaque / (name.replace("/", "__") + ".png")
        if not flat.exists():
            im = Image.open(png).convert("RGBA")
            bg = Image.new("RGBA", im.size, (255, 255, 255, 255))
            Image.alpha_composite(bg, im).convert("RGB").save(flat)
        outs = []
        for e in (a.old, a.new):
            o = WORK / tags[e] / (flat.stem + ".opaque.svg")
            if not o.exists() and not trace(e, flat, o, []):
                return name, None
            outs.append(o.read_bytes())
        return name, outs[0] == outs[1]

    with ThreadPoolExecutor(a.workers) as ex:
        res = list(ex.map(one, items))
    bad = [n for n, ok in res if ok is not True]
    print(f"opaque identity: {len(res) - len(bad)} / {len(res)} byte-identical")
    for n in bad[:20]:
        print(f"  DIFFERS: {n}")
    return 1 if bad else 0


def run_scores(exe: str, items, extra, workers) -> dict:
    tag = exe_tag(exe) + ("_" + "_".join(x.lstrip("-") for x in extra) if extra else "")
    d = WORK / tag
    d.mkdir(parents=True, exist_ok=True)
    cache = d / "scores.json"
    have = json.loads(cache.read_text()) if cache.exists() else {}

    def one(item):
        name, png, gt = item
        if name in have:
            return name, have[name]
        o = d / (name.replace("/", "__") + ".svg")
        if not trace(exe, png, o, extra):
            return name, None
        return name, score(rgba(o.read_text(encoding="utf-8")), rgba(gt.read_text(encoding="utf-8")))

    with ThreadPoolExecutor(workers) as ex:
        res = dict(ex.map(one, items))
    have.update({k: v for k, v in res.items() if v is not None})
    cache.write_text(json.dumps(have), encoding="utf-8")
    return res


def cmd_score(a) -> int:
    extra = a.args.split()
    sets = {}
    for s in a.sets.split(","):
        sets[s] = {"screen": screen_items, "probe": probe_items,
                   "translucent": lambda: translucent_items(a.workers)}[s]()
    for s, items in sets.items():
        new = run_scores(a.new, items, extra, a.workers)
        old_extra = a.compare_args.split() if a.compare_args else (extra if a.same_args else [])
        old = run_scores(a.compare, items, old_extra, a.workers) if a.compare else None
        keys = [k for k in new if new[k] is not None and (old is None or old.get(k) is not None)]
        if not keys:
            print(f"{s}: nothing scored")
            continue
        N = np.array([new[k] for k in keys])
        line = f"{s:12s} n={len(keys):4d}  white {N[:, 0].mean():.5f}  dark {N[:, 1].mean():.5f}  alpha {N[:, 2].mean():.5f}"
        if old:
            O = np.array([old[k] for k in keys])
            line = (f"{s:12s} n={len(keys):4d}  white {O[:, 0].mean():.5f}>{N[:, 0].mean():.5f}"
                    f"  dark {O[:, 1].mean():.5f}>{N[:, 1].mean():.5f}"
                    f"  alpha {O[:, 2].mean():.5f}>{N[:, 2].mean():.5f}")
            worse = sum(1 for k in keys if max(new[k]) > max(old[k]) + 1e-3)
            better = sum(1 for k in keys if max(new[k]) < max(old[k]) - 1e-3)
            line += f"   better/worse {better}/{worse}"
        print(line)
        if old and a.movers:
            moves = sorted(keys, key=lambda k: max(new[k]) - max(old[k]))
            for k in moves[:a.movers] + moves[-a.movers:]:
                o, n = old[k], new[k]
                print(f"    {k[:52]:52s} w {o[0]:.4f}>{n[0]:.4f} d {o[1]:.4f}>{n[1]:.4f} a {o[2]:.4f}>{n[2]:.4f}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    i = sub.add_parser("identity")
    i.add_argument("old")
    i.add_argument("new")
    i.add_argument("--workers", type=int, default=12)
    s = sub.add_parser("score")
    s.add_argument("new")
    s.add_argument("--compare", help="the build to compare against")
    s.add_argument("--args", default="", help="extra tracer flags for NEW")
    s.add_argument("--same-args", action="store_true", help="pass --args to the compare build too")
    s.add_argument("--compare-args", default="", help="extra tracer flags for the compare build")
    s.add_argument("--sets", default="screen,probe,translucent")
    s.add_argument("--movers", type=int, default=5)
    s.add_argument("--workers", type=int, default=12)
    a = ap.parse_args()
    return cmd_identity(a) if a.cmd == "identity" else cmd_score(a)


if __name__ == "__main__":
    sys.exit(main())
