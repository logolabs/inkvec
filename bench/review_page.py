"""Build a before/after review page (HTML artifact) from two full_eval JSONs.

    python bench/review_page.py --a bench/data/eval_day2_A.json --b bench/data/eval_day2_B.json \
        --exe-a <before.exe> --exe-b <after.exe> --out review.html [--n 48]

Picks the icons that moved most (both directions) plus the worst remaining, renders
source / GT / before / after at 320 px as data URIs, and writes a single self-contained
page with a per-family table, filters and a changed-pixels heat map.
"""
from __future__ import annotations

import argparse
import base64
import io
import json
import re
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "bench"))
from inkvec_bench import render  # noqa: E402
import svgeval  # noqa: E402

SIZE = 320


def data_uri(img: Image.Image) -> str:
    buf = io.BytesIO()
    img.save(buf, format="PNG", optimize=True)
    return "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode()


def render_svg(svg: str) -> Image.Image:
    arr = render.composite(render.render(svg, SIZE, SIZE))
    return Image.fromarray((np.clip(arr, 0, 1) * 255 + 0.5).astype(np.uint8))


def trace(exe: Path, png: Path) -> str:
    out = svgeval.WORK / f"_rev_{exe.stem}.svg"
    subprocess.run([str(exe), str(png), "-o", str(out), "--quiet"], check=True, capture_output=True)
    return out.read_text(encoding="utf-8")


def stats(svg: str) -> dict:
    return {"paths": len(re.findall(r"<path\b", svg)),
            "gradients": len(re.findall(r"<(?:linear|radial)Gradient\b", svg)),
            "bytes": len(svg.encode())}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--a", required=True)
    ap.add_argument("--b", required=True)
    ap.add_argument("--exe-a", required=True)
    ap.add_argument("--exe-b", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--n", type=int, default=48)
    ap.add_argument("--title", default="inkvec Day-2 Review")
    a = ap.parse_args()
    A = json.loads(Path(a.a).read_text(encoding="utf-8"))
    B = json.loads(Path(a.b).read_text(encoding="utf-8"))
    ia, ib = A["images"], B["images"]
    stems = [s for s in ia if s in ib]
    delta = {s: ib[s]["de00"] - ia[s]["de00"] for s in stems}
    improved = sorted(stems, key=lambda s: delta[s])[: a.n // 3]
    regressed = sorted(stems, key=lambda s: -delta[s])[: a.n // 3]
    worst = [s for s in sorted(stems, key=lambda s: -ib[s]["de00"]) if s not in improved and s not in regressed][: a.n - len(improved) - len(regressed)]
    chosen = [(s, "improved") for s in improved] + [(s, "regressed") for s in regressed] + [(s, "worst") for s in worst]

    exe_a, exe_b = Path(a.exe_a).resolve(), Path(a.exe_b).resolve()
    cards = []
    for stem, group in chosen:
        it = {"corpus": ia[stem]["corpus"], "stem": ia[stem].get("stem", stem)}
        png, gt = svgeval.item_paths(it)
        src = Image.open(png).convert("RGBA")
        bg = Image.new("RGBA", src.size, (255, 255, 255, 255))
        src = Image.alpha_composite(bg, src).convert("RGB").resize((SIZE, SIZE), Image.NEAREST)
        sa, sb = trace(exe_a, png), trace(exe_b, png)
        cards.append({
            "stem": it["stem"], "corpus": it["corpus"], "group": group,
            "before": {**{k: ia[stem][k] for k in ("de00", "dists", "ratio")}, **stats(sa)},
            "after": {**{k: ib[stem][k] for k in ("de00", "dists", "ratio")}, **stats(sb)},
            "img": {"src": data_uri(src), "gt": data_uri(render_svg(gt.read_text(encoding="utf-8"))),
                    "before": data_uri(render_svg(sa)), "after": data_uri(render_svg(sb))},
        })
        print(f"{group:9s} {stem:40s} {ia[stem]['de00']:.3f} -> {ib[stem]['de00']:.3f}", flush=True)

    fams = sorted({ia[s]["corpus"] for s in stems})
    famrows = []
    for f in fams + ["ALL"]:
        ss = [s for s in stems if f == "ALL" or ia[s]["corpus"] == f]
        da = np.mean([ia[s]["de00"] for s in ss]); db = np.mean([ib[s]["de00"] for s in ss])
        ta = np.mean([ia[s]["dists"] for s in ss]); tb = np.mean([ib[s]["dists"] for s in ss])
        ra = np.mean([ia[s]["ratio"] for s in ss]); rb = np.mean([ib[s]["ratio"] for s in ss])
        dd = np.array([delta[s] for s in ss])
        famrows.append({"family": f, "n": len(ss), "de_a": da, "de_b": db, "dists_a": ta, "dists_b": tb,
                        "ratio_a": ra, "ratio_b": rb, "better": int((dd < -1e-4).sum()), "worse": int((dd > 1e-4).sum())})
    data = {"title": a.title, "families": famrows, "cards": cards,
            "objective": {"a": A["objective"], "b": B["objective"]}}
    tpl = (ROOT / "bench" / "review_tpl.html").read_text(encoding="utf-8")
    html = tpl.replace("/*DATA*/[]/*END*/", json.dumps(data).replace("</", "<\\/"))
    Path(a.out).write_text(html, encoding="utf-8")
    print("wrote", a.out, len(html) // 1024, "KB")


if __name__ == "__main__":
    main()
