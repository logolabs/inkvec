"""Sweep one engine constant at a time, measuring on both corpora.

Every constant in the engine was chosen against the synthetic corpus, whose shapes have
4 to 36 parameters. Real emoji have 400 to 2600, and the two corpora disagree about what
several of these constants are for — the curvature-uncertainty gain was three times too
strong for real content and nobody could have known from synthetic alone.

So each constant gets swept against both, and a value is only adopted if it does not
lose on either. Run:

    python sweep.py NONLINEARITY_GAIN 0.2 0.35 0.5
    python sweep.py --all
"""
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
TARGET = "target2"
EXE = ROOT / TARGET / "release" / "inkvec.exe"

# Constant name -> (file, values to try).
#
# Every entry is proved live by `assert_live` before it is swept. Two were removed on
# 2026-09-08 for failing that test: `BLEND_IMMUNE_WEIGHT`, which nothing read at all,
# and `CORNER_DEGREES`, which is read only inside `fit_path` and so is reachable from
# examples and tests but not from the shipped binary. Both had been swept before, and
# both had been returning the same score for every arm.
CONSTANTS = {
    "DEFAULT_MERGE_DISTANCE": ("crates/inkvec-trace/src/color.rs", [0.035, 0.055, 0.080]),
    "DEFAULT_SIGMA_MODEL": ("crates/inkvec-trace/src/coverage.rs", [0.03, 0.05, 0.08]),
    "LINEARITY_WINDOW": ("crates/inkvec-trace/src/contour.rs", [2, 3, 5]),
    "MAX_CURVATURE_SIGMA": ("crates/inkvec-trace/src/contour.rs", [0.2, 0.354, 0.6]),
    "JND_FLOOR": ("crates/inkvec-trace/src/color.rs", [0.006, 0.012, 0.025]),
}

N_REAL = 16
N_SYN = 8


def set_constant(path: Path, name: str, value) -> str:
    src = path.read_text(encoding="utf-8")
    pat = re.compile(rf"(const {name}: (\w+) = )([0-9.]+)(;)")
    m = pat.search(src)
    if not m:
        raise SystemExit(f"{name} not found in {path}")
    ty, old = m.group(2), m.group(3)
    # Write a literal the declared type accepts: an integer type must not get "45.0",
    # and a float type must not get "45".
    lit = str(int(float(value))) if ty.startswith(("u", "i")) else f"{float(value)}"
    path.write_text(pat.sub(rf"\g<1>{lit}\g<4>", src, count=1), encoding="utf-8")
    return old


def build() -> bool:
    r = subprocess.run(
        ["cargo", "build", "--release", "--quiet"],
        cwd=ROOT,
        capture_output=True,
        env={**__import__("os").environ, "CARGO_TARGET_DIR": TARGET},
    )
    if r.returncode != 0:
        print("   build failed:", r.stderr.decode()[:300])
    return r.returncode == 0


def measure(files, gt_dir: Path):
    sys.path.insert(0, str(ROOT / "bench"))
    from inkvec_bench import render, svgmodel
    from inkvec_bench.metrics import color as mcolor
    from inkvec_bench.metrics import raster

    D, P, E = [], [], []
    tmp = ROOT / "bench" / "data" / "_sweep.svg"
    for f in files:
        gt = gt_dir / (f.stem + ".svg")
        if not gt.exists():
            continue
        r = subprocess.run([str(EXE), str(f), "-o", str(tmp), "--quiet"], capture_output=True)
        if r.returncode != 0:
            continue
        svg = tmp.read_text(encoding="utf-8")
        doc = svgmodel.parse(svg)
        gdoc = svgmodel.parse(gt.read_text(encoding="utf-8"))
        inp = render.load_rgba(f)
        c = render.render(svg, 128, 128)
        a, b = render.composite(inp), render.composite(c)
        D.append(raster.compare(a, b)["dists"])
        E.append(mcolor.delta_e00(a, b)["de00_mean"])
        P.append(doc.n_params / max(1, gdoc.n_params))
    if not D:
        return None
    return float(np.median(D)), float(np.median(P)), float(np.median(E))


def corpora():
    d = ROOT / "bench" / "data" / "corpus_raster"
    real = sorted((d / "noto-emoji" / "128").glob("*.png"))[:N_REAL]
    syn_names = [
        "prim_circle", "prim_hex", "logo_like", "gradient_linear",
        "mosaic_pie6", "rings_concentric", "thin_features", "symmetry_rot5",
    ][:N_SYN]
    syn = [d / "synthetic" / "128" / f"{n}.png" for n in syn_names]
    return real, syn


def assert_live(name: str, path: Path) -> bool:
    """Prove that this constant still changes the output, before spending a sweep on it.

    A sweep over a constant nothing reads does not fail. It succeeds, quietly, returning the
    same score for every arm -- which reads as "the engine is insensitive to this". That is
    a conclusion, and a wrong one. It is worse than no measurement, because it looks like
    evidence, and evidence gets built on. Two of the six constants originally listed in
    `CONSTANTS` were in exactly that state on 2026-09-08:

    * `BLEND_IMMUNE_WEIGHT` was declared and documented but read nowhere, left behind when
      commit 185ca7e replaced abundance-based blend detection with a shape-based test.
    * `CORNER_DEGREES` is read -- inside `fit_path`, which the shipped binary no longer
      calls; it survives only in examples and tests.

    A static "is this name mentioned anywhere else" check catches the first and misses the
    second. The only check that catches both is the empirical one: set the value to
    something the engine could not plausibly ignore, trace an image, and require the bytes
    to differ. Doing the experiment costs two builds and settles a question that reading the
    code only argues about.
    """
    probe = next(
        iter(sorted((ROOT / "bench" / "data" / "corpus_raster" / "noto-emoji" / "128ss").glob("*.png"))),
        None,
    )
    if probe is None:
        print(f"   ! {name}: no probe image available, liveness unchecked")
        return True

    tmp = ROOT / "bench" / "data" / "_live.svg"

    def trace():
        r = subprocess.run(
            [str(EXE), str(probe), "-o", str(tmp), "--quiet"], capture_output=True
        )
        return tmp.read_text(encoding="utf-8") if r.returncode == 0 else None

    if not build():
        return True                       # a broken build is a different problem
    base = trace()
    if base is None:
        print(f"   ! {name}: probe trace failed, liveness unchecked")
        return True

    m = re.search(rf"const {name}: (\w+) = ([0-9.]+);", path.read_text(encoding="utf-8"))
    if not m:
        print(f"   ! {name}: not found in {path.name}")
        return False
    cur = float(m.group(2))

    original = None
    try:
        # Both directions, so a constant that happens to saturate one way is still moved
        # the other.
        for value in (cur * 8.0 + 1.0, cur / 8.0):
            old = set_constant(path, name, value)
            if original is None:
                original = old
            if build() and trace() != base:
                return True
        print(f"   ! {name} is NOT LIVE: the trace is byte-identical at {cur}, "
              f"{cur * 8.0 + 1.0} and {cur / 8.0}.")
        print("     A sweep would report insensitivity that is really deadness.")
        print("     Find where it ought to be read, or delete it.")
        return False
    finally:
        if original is not None:
            set_constant(path, name, float(original))
            build()


def sweep(name: str, values) -> None:
    rel, _ = CONSTANTS[name]
    path = ROOT / rel
    real, syn = corpora()
    gt_real = ROOT / "bench" / "data" / "corpus_svg" / "noto-emoji"
    gt_syn = ROOT / "bench" / "data" / "corpus_svg" / "synthetic"

    print(f"\n=== {name} ({rel}) ===")
    if not assert_live(name, path):
        print("  skipped: not live, so a sweep here would manufacture a false result")
        return
    print(f"  {'value':>10} {'REAL dists':>11}{'ratio':>7}{'dE00':>7}   {'SYN dists':>10}{'ratio':>7}{'dE00':>7}")
    original = None
    rows = []
    for v in values:
        old = set_constant(path, name, v)
        if original is None:
            original = old
        if not build():
            continue
        r = measure(real, gt_real)
        s = measure(syn, gt_syn)
        if r and s:
            rows.append((v, r, s))
            print(f"  {v:>10} {r[0]:11.4f}{r[1]:7.2f}{r[2]:7.2f}   {s[0]:10.4f}{s[1]:7.2f}{s[2]:7.2f}")
    # Restore.
    if original is not None:
        set_constant(path, name, float(original))
        build()
    if rows:
        best_r = min(rows, key=lambda x: x[1][0])
        best_s = min(rows, key=lambda x: x[2][0])
        print(f"  best on real: {best_r[0]}   best on synthetic: {best_s[0]}"
              f"   {'AGREE' if best_r[0] == best_s[0] else 'DISAGREE'}")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--all":
        for name, (_, vals) in CONSTANTS.items():
            sweep(name, vals)
    elif len(sys.argv) > 2:
        name = sys.argv[1]
        vals = [float(v) if "." in v else int(v) for v in sys.argv[2:]]
        sweep(name, vals)
    else:
        print(__doc__)
        print("constants:", ", ".join(CONSTANTS))
