"""What each stage is worth, against what it costs.

    python bench/ablate.py --exe target/release/inkvec.exe --set screen

Turns one stage off at a time and measures what the output loses. Read next to the size of
the code that stage owns, it says which parts of the pipeline are earning their complexity
and which are candidates for a simpler algorithm — the point being that a stage worth
0.001 in colour error and 900 lines is a stage to replace, not to tune.

The comparison is paired per icon against the same build with nothing turned off, so the
number is the stage's own contribution and not a difference of averages. Results are cached
per executable, and every arm gets its own cache salt because an environment switch does
not otherwise change the key.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent

# Each arm: a name, the environment that disables the stage, extra tracer arguments, and
# the source files the stage owns. Line counts come from those files, so a stage that
# spreads across modules is charged for all of it.
ARMS = [
    ("boundary solve", {"INKVEC_BOPT": "0"}, "", ["inkvec-trace/src/boundary_opt.rs"]),
    ("gradients", {}, "--no-gradients", ["inkvec-trace/src/gradient.rs"]),
    ("repair", {}, "--no-repair", []),
    ("polish", {}, "--no-polish", ["inkvec-fit/src/polish.rs"]),
    ("residual carving", {"INKVEC_NO_CARVE": "1"}, "", []),
    ("blend absorption", {"INKVEC_NO_ABSORB": "1"}, "", []),
    ("taper at junctions", {"INKVEC_NO_TAPER": "1"}, "", []),
    ("image adjudication", {"INKVEC_IMGADJ": "0"}, "", ["inkvec-fit/src/smooth.rs"]),
]


def code_lines(paths: list[str]) -> int:
    n = 0
    for rel in paths:
        f = ROOT / "crates" / rel
        if f.exists():
            n += sum(1 for l in f.read_text(encoding="utf-8", errors="replace").splitlines()
                     if l.strip() and not l.strip().startswith("//"))
    return n


def run(exe: str, label: str, set_name: str, workers: int, env: dict, args: str) -> dict:
    e = dict(os.environ)
    e.update(env)
    # An environment switch is invisible to the score cache's key, so give each arm its own.
    e["INKVEC_CACHE_SALT"] = label + "|" + args
    cmd = [sys.executable, str(ROOT / "bench" / "full_eval.py"), exe,
           "--set", set_name, "--workers", str(workers), "--label", label]
    if args:
        cmd.append(f"--args={args}")
    subprocess.run(cmd, check=True, capture_output=True, env=e, cwd=ROOT)
    p = ROOT / "bench" / "data" / f"eval_{label}_A.json"
    return json.loads(p.read_text(encoding="utf-8"))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default="target/release/inkvec.exe")
    ap.add_argument("--set", default="screen")
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--out", default="out/ablation.json")
    a = ap.parse_args()

    base = run(a.exe, "abl_base", a.set, a.workers, {}, "")
    bi = base["images"]
    print(f"baseline: {len(bi)} icons, objective {base['objective']:.4f}\n", flush=True)
    print(f"{'stage off':20s} {'dE00':>9s} {'DISTS':>9s} {'objective':>10s} "
          f"{'worse/better':>13s} {'code':>6s} {'per 100 lines':>14s}")

    rows = []
    for name, env, args, files in ARMS:
        label = "abl_" + re.sub(r"[^a-z]+", "", name.lower())[:12]
        try:
            got = run(a.exe, label, a.set, a.workers, env, args)
        except subprocess.CalledProcessError as exc:
            print(f"{name:20s}  failed: {exc}")
            continue
        gi = got["images"]
        keys = [k for k in bi if k in gi]
        d = np.array([gi[k]["de00"] - bi[k]["de00"] for k in keys])
        di = np.array([gi[k]["dists"] - bi[k]["dists"] for k in keys])
        dobj = got["objective"] - base["objective"]
        lines = code_lines(files)
        per = f"{dobj / lines * 100:.4f}" if lines else "-"
        print(f"{name:20s} {d.mean():+9.4f} {di.mean():+9.5f} {dobj:+10.4f} "
              f"{int((d > 1e-4).sum()):6d}/{int((d < -1e-4).sum()):<6d} "
              f"{lines if lines else '-':>6} {per:>14s}", flush=True)
        rows.append({"stage": name, "d_de00": float(d.mean()), "d_dists": float(di.mean()),
                     "d_objective": float(dobj), "lines": lines})

    Path(ROOT / a.out).parent.mkdir(parents=True, exist_ok=True)
    (ROOT / a.out).write_text(json.dumps(rows, indent=1), encoding="utf-8")
    print(f"\nA positive number is what the stage is worth: how much worse the output gets "
          f"without it.\nwrote {a.out}")


if __name__ == "__main__":
    main()
