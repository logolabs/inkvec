"""Score another engine's SVGs with the harness inkvec is judged by.

    python bench/score_external.py --dir out/compare_60/vectormagic --label "Vector Magic"
    python bench/score_external.py --dir out/compare_60/vectormagic --label VM --exe target/release/inkvec.exe

Reads `out/compare_60/manifest.json` (or `--manifest`), finds one SVG per icon in `--dir`
by matching the manifest's `file` stem, and scores it exactly as `full_eval` scores ours:
rendered at the judging resolution against the artist's SVG, mean CIEDE2000, DISTS, and
editable parameters as a ratio of the artist's. Pass `--exe` to score a inkvec build over
the same icons in the same run, so the two columns are strictly comparable.

Matching is by name: an engine that writes `noto-emoji__emoji_u1f600.svg`, or
`noto-emoji__emoji_u1f600.png.svg`, or puts them in per-family subdirectories, all work.
Icons with no SVG are reported as missing rather than skipped silently — an engine that
fails on an input has not scored zero there, and saying so is the point.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "bench"))
import svgeval  # noqa: E402
from inkvec_bench import render, svgmodel  # noqa: E402
from inkvec_bench.metrics import color as mcolor  # noqa: E402


def find_svg(root: Path, stem: str) -> Path | None:
    for cand in (
        root / f"{stem}.svg",
        root / f"{stem}.png.svg",
        root / f"{stem}.png.svgz",
    ):
        if cand.exists():
            return cand
    hits = sorted(root.rglob(f"{stem}.svg")) + sorted(root.rglob(f"{stem}.png.svg"))
    return hits[0] if hits else None


def score(svg: str, ref: np.ndarray, gt_params: int) -> dict:
    img = render.composite(render.render(svg, svgeval.JUDGE_SIZE, svgeval.JUDGE_SIZE))
    from inkvec_bench.metrics import raster
    return {
        "de00": float(mcolor.delta_e00(ref, img)["de00_mean"]),
        "dists": float(raster.dists_distance(ref, img)),
        "params": svgmodel.parse(svg).n_params,
        "ratio": svgmodel.parse(svg).n_params / max(1, gt_params),
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", required=True, help="directory of the other engine's SVGs")
    ap.add_argument("--label", default="other")
    ap.add_argument("--manifest", default="out/compare_60/manifest.json")
    ap.add_argument("--exe", help="also score this inkvec build over the same icons")
    ap.add_argument("--args", default="", help="extra tracer arguments for --exe")
    ap.add_argument("--out", help="write the per-icon table here as JSON")
    a = ap.parse_args()

    rows = json.loads(Path(a.manifest).read_text(encoding="utf-8"))
    other_dir = Path(a.dir)
    work = svgeval.WORK / "_external"
    work.mkdir(parents=True, exist_ok=True)

    results, missing = [], []
    for it in rows:
        stem_key = Path(it["file"]).stem
        gt = svgeval.item_paths(it)[1]
        png = svgeval.item_paths(it)[0]
        ref = svgeval.gt_render(gt, it["corpus"], it["stem"])
        found = find_svg(other_dir, stem_key) or find_svg(other_dir, it["stem"])
        row = {"corpus": it["corpus"], "stem": it["stem"]}
        if found is None:
            missing.append(stem_key)
        else:
            row[a.label] = score(found.read_text(encoding="utf-8"), ref, it.get("gt_params", 0))
        if a.exe:
            out = work / f"{stem_key}.svg"
            r = subprocess.run(
                [str(Path(a.exe).resolve()), str(png), "-o", str(out), "--quiet",
                 *(a.args.split() if a.args else [])],
                capture_output=True,
            )
            if r.returncode == 0:
                row["inkvec"] = score(out.read_text(encoding="utf-8"), ref,
                                      it.get("gt_params", 0))
        results.append(row)
        print(".", end="", flush=True)
    print()

    cols = [c for c in (a.label, "inkvec") if any(c in r for r in results)]
    fams = sorted({r["corpus"] for r in results})
    print(f"\n{'family':16s} {'n':>3s}" + "".join(f"{c[:12]:>26s}" for c in cols))
    print(f"{'':16s} {'':>3s}" + "".join(f"{'dE00':>9s}{'DISTS':>9s}{'ratio':>8s}" for _ in cols))
    for fam in fams + ["ALL"]:
        sub = [r for r in results if fam == "ALL" or r["corpus"] == fam]
        line = f"{fam:16s} {len(sub):3d}"
        for c in cols:
            v = [r[c] for r in sub if c in r]
            if not v:
                line += f"{'-':>26s}"
                continue
            line += (f"{np.mean([x['de00'] for x in v]):9.4f}"
                     f"{np.mean([x['dists'] for x in v]):9.4f}"
                     f"{np.mean([x['ratio'] for x in v]):8.2f}")
        print(line)

    if len(cols) == 2:
        both = [r for r in results if all(c in r for c in cols)]
        d = np.array([r["inkvec"]["de00"] - r[a.label]["de00"] for r in both])
        di = np.array([r["inkvec"]["dists"] - r[a.label]["dists"] for r in both])
        print(f"\npaired on {len(both)} icons, inkvec minus {a.label}:")
        print(f"  dE00  mean {d.mean():+.4f}   inkvec better on {int((d < 0).sum())}, "
              f"worse on {int((d > 0).sum())}")
        print(f"  DISTS mean {di.mean():+.5f}   inkvec better on {int((di < 0).sum())}, "
              f"worse on {int((di > 0).sum())}")
        worst = sorted(both, key=lambda r: r[a.label]["de00"] - r["inkvec"]["de00"])[:5]
        print("  inkvec loses most on: "
              + ", ".join(f"{r['stem']} {r['inkvec']['de00']:.2f} vs {r[a.label]['de00']:.2f}"
                          for r in worst))
    if missing:
        print(f"\n{len(missing)} icon(s) with no {a.label} SVG: {missing[:8]}")
    if a.out:
        Path(a.out).write_text(json.dumps(results, indent=1), encoding="utf-8")
        print("wrote", a.out)


if __name__ == "__main__":
    main()
