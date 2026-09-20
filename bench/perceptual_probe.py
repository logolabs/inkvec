"""Which cheap metric sees the damage dE00 misses?

The gate holds colour error, parameter count and two structure signals. None of them moves
much when a boundary slides half a pixel or a corner softens -- the shape of every edit a
beautifier, a harmoniser or a learned prior makes. This measures candidates against edits
whose severity is known:

  translate N   the whole drawing moved by N source pixels. 0.05 px is invisible and must
                not alarm a metric; 1 px is plainly wrong and must.
  decimals 1    coordinates rounded to a tenth of a pixel, the rounding that cost 7.2% of
                the corpus's fidelity before the emitter wrote two decimals.
  unguarded     shape harmonization with its evidence guard lifted: the regression that
                doubled the gate's dE00 in 0.1.1.

Each variant is rendered and scored against the artist's file with dE00, GMSD, HaarPSI and
(where torch is installed) DISTS. DISTS is the reference: it is the perceptual metric this
repository already trusts, and too heavy to run in CI. The winner is whichever cheap metric
tracks DISTS best and separates the invisible edits from the visible ones.

    python bench/perceptual_probe.py --exe target/release/inkvec.exe [--n 20]
"""
from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))

import svgeval  # noqa: E402
from inkvec_bench.metrics import color as mcolor  # noqa: E402
from inkvec_bench.metrics import raster as mraster  # noqa: E402
from inkvec_bench import render  # noqa: E402
from perceptual import gmsd, haarpsi  # noqa: E402

JUDGE = svgeval.JUDGE_SIZE


def translated(svg: str, d: float) -> str:
    """The same drawing, moved `d` source pixels diagonally."""
    m = re.search(r"<svg\b[^>]*>", svg)
    if not m:
        raise ValueError("no <svg> element")
    head, rest = svg[: m.end()], svg[m.end():]
    close = rest.rfind("</svg>")
    body, tail = rest[:close], rest[close:]
    return f'{head}<g transform="translate({d},{d})">{body}</g>{tail}'


def one_shape_moved(svg: str, d: float) -> str | None:
    """One shape moved `d` source pixels: the local edit a beautifier really makes.

    The shape with the longest geometry is taken as the drawing's subject, skipping a
    first element that paints the whole canvas.
    """
    els = [m for m in re.finditer(r"<(path|circle|rect|ellipse)\b[^>]*/>", svg)]
    if len(els) < 2:
        return None
    subject = max(els[1:], key=lambda m: len(m.group(0)))
    s, e = subject.span()
    return (svg[:s] + f'<g transform="translate({d},{d})">' + svg[s:e] + "</g>" + svg[e:])


def sawtooth(svg: str, amp: float) -> str:
    """Every other coordinate nudged in and out: a boundary that wobbles without moving.

    The ink stays where it was on average, so colour error should barely notice; a human
    sees a jagged edge at once. This is the blind spot a perceptual axis is meant to cover.
    """
    num = re.compile(r"-?\d+\.?\d*(?:e-?\d+)?")

    def one_path(m: re.Match) -> str:
        d, out, i, cmd, k = m.group(1), [], 0, "", 0
        while i < len(d):
            c = d[i]
            if c.isalpha():
                cmd, k = c, 0
                out.append(c)
                i += 1
                continue
            hit = num.match(d, i)
            if not hit:
                out.append(c)
                i += 1
                continue
            # An arc's radii, rotation and flags are not coordinates, and a flag must stay
            # a bare 0 or 1: leave the first five alone, wobble only the endpoint.
            if cmd.upper() == "A" and k < 5:
                out.append(hit.group(0))
            else:
                v = float(hit.group(0)) + (amp if (k // 2) % 2 == 0 else -amp)
                out.append(f"{v:.3f}")
            k += 1
            i = hit.end()
        return 'd="' + "".join(out) + '"'

    # `(?<![a-z])` so this does not match the `d="` inside `id="..."`.
    return re.sub(r'(?<![a-zA-Z])d="([^"]+)"', one_path, svg)


def flatten_gradients(svg: str) -> str:
    """Each gradient replaced by its first stop: the ramp-to-flat disease."""
    stops = {}
    for g in re.finditer(r'<(?:linear|radial)Gradient id="([^"]+)"[^>]*>(.*?)</(?:linear|radial)Gradient>',
                         svg, re.S):
        c = re.search(r'stop-color="([^"]+)"', g.group(2))
        if c:
            stops[g.group(1)] = c.group(1)
    if not stops:
        return None
    out = svg
    for gid, col in stops.items():
        out = out.replace(f'fill="url(#{gid})"', f'fill="{col}"')
    return out


def trace(exe: str, png: Path, out: Path, env_extra: dict) -> str:
    subprocess.run([exe, str(png), "-o", str(out), "-q"], check=True,
                   env=dict(os.environ, **env_extra))
    return out.read_text(encoding="utf-8")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=str(ROOT / "target/release/inkvec.exe"))
    ap.add_argument("--n", type=int, default=20)
    args = ap.parse_args()

    items = svgeval.load_sets()["screen"]
    step = max(1, len(items) // args.n)
    items = items[::step][: args.n]
    tmp = Path(tempfile.mkdtemp())
    have_dists = mraster.perceptual_available().get("dists", False)
    print(f"{len(items)} icons, judge {JUDGE} px, DISTS {'on' if have_dists else 'MISSING'}")

    variants = ["base", "translate 0.05", "translate 0.10", "translate 0.25",
                "translate 0.50", "translate 1.00", "one shape 0.25", "one shape 0.50",
                "one shape 1.00", "sawtooth 0.15", "sawtooth 0.30", "flat gradients",
                "decimals 1", "unguarded harmonize"]
    rows: dict[str, list[dict]] = {v: [] for v in variants}

    for n, it in enumerate(items, 1):
        png, gt = svgeval.item_paths(it)
        ref = svgeval.gt_render(gt, it["corpus"], it["stem"])
        base_svg = trace(args.exe, png, tmp / "b.svg", {})
        made = {"base": base_svg,
                "decimals 1": trace(args.exe, png, tmp / "d.svg", {"INKVEC_EMIT_DECIMALS": "1"}),
                "unguarded harmonize": trace(args.exe, png, tmp / "h.svg",
                                             {"INKVEC_HARMONIZE_TOL": "1000"})}
        for d in (0.05, 0.10, 0.25, 0.50, 1.00):
            made[f"translate {d:.2f}"] = translated(base_svg, d)
        for d in (0.25, 0.50, 1.00):
            one = one_shape_moved(base_svg, d)
            if one is not None:
                made[f"one shape {d:.2f}"] = one
        for a in (0.15, 0.30):
            made[f"sawtooth {a:.2f}"] = sawtooth(base_svg, a)
        flat = flatten_gradients(base_svg)
        if flat is not None:
            made["flat gradients"] = flat

        for name, svg in made.items():
            img = render.render(svg, JUDGE, JUDGE)
            if img.shape[-1] == 4:
                img = img[..., :3] * img[..., 3:] + (1.0 - img[..., 3:])
            row = {"de00": float(mcolor.delta_e00(ref, img)["de00_mean"]),
                   "gmsd": gmsd(ref, img), "haarpsi": haarpsi(ref, img)}
            if have_dists:
                row["dists"] = mraster.dists_distance(ref, img)
            rows[name].append(row)
        print(f"  [{n}/{len(items)}] {it['corpus']}/{it['stem']}", flush=True)

    metrics = ["de00", "gmsd", "haarpsi"] + (["dists"] if have_dists else [])
    print(f"\n{'variant':22}" + "".join(f"{m:>26}" for m in metrics))
    print(f"{'':22}" + "".join(f"{'value  (x base)':>26}" for _ in metrics))
    base = {m: np.array([r[m] for r in rows["base"]]) for m in metrics}
    for v in variants:
        if not rows[v]:
            continue
        cells = ""
        for m in metrics:
            cur = np.array([r[m] for r in rows[v]])
            ratio = float(np.mean(cur) / np.mean(base[m])) if np.mean(base[m]) > 0 else float("nan")
            cells += f"{np.mean(cur):>18.5f} ({ratio:4.2f}x)"
        print(f"{v:22}{cells}")

    if have_dists:
        print("\nAgreement with DISTS over every (icon, variant) pair:")
        d = np.concatenate([[r["dists"] for r in rows[v]] for v in variants])
        for m in ("de00", "gmsd", "haarpsi"):
            x = np.concatenate([[r[m] for r in rows[v]] for v in variants])
            rank = lambda z: np.argsort(np.argsort(z))  # noqa: E731
            sp = float(np.corrcoef(rank(x), rank(d))[0, 1])
            print(f"  {m:8} Spearman {sp:+.3f}")

    print("\nSeparation, invisible (0.05 px) vs visible (0.50 px), in base-image units:")
    for m in metrics:
        inv = np.array([r[m] for r in rows["translate 0.05"]])
        vis = np.array([r[m] for r in rows["translate 0.50"]])
        b = np.array([r[m] for r in rows["base"]])
        print(f"  {m:8} 0.05px {np.mean(inv / b):5.2f}x   0.50px {np.mean(vis / b):5.2f}x")
    return 0


if __name__ == "__main__":
    sys.exit(main())
