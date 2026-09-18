"""Three-way output parity: web/pkg (single-thread wasm), web/pkg-threads (threaded wasm)
and the native CLI must produce byte-identical SVGs for the same input and the same knobs.

This guards the two wasm arms (which may be built with different target features) and the
native build against drifting apart silently. Run it before AND after any change to the
wasm build flags or the shared pipeline: it validates the build you are about to ship
against the build you shipped last time.

    python bench/wasm_parity.py                 # 8 default images
    python bench/wasm_parity.py --images a.png b.png

The knobs mirror the Space's default "logo" controls in web/index.html:
trace(bytes, precision=0.1, min_area=2, colors=64, merge=0.035, max_dim=1024,
      time_budget=20, no_background=false, minify=false, margin=0, content_units=false)
and the CLI gets exactly those values as flags -- except the time budget, which is 0 here
unless --time-budget says otherwise. A budget is a wall-clock deadline (the merge stops at
60% of it, the boundary solve at 25%), so under it the output depends on how fast the
machine happened to be: one binary on one image gave two different SVGs in two runs on a
loaded box. Parity is only meaningful without one.

The two wasm arms must agree byte for byte; that is the gate. The native build is compared
too but only reported, because it cannot agree: wasm32 takes its transcendentals (the cbrt
in OKLab, the powf in sRGB gamma, atan2, ...) from Rust's musl-derived libm and a Windows
build takes them from the platform runtime, and a last-bit difference in a colour
conversion can flip a palette decision. --strict-native makes native a gate as well.

Exit status 0 = the gate held; 1 = a gated mismatch or any arm failed; 2 = the harness
itself could not run (node missing, glue missing).
"""

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# The Space's samples plus corpus icons chosen for coverage: fine-line material icons
# (thin strokes, small features), a lucide line icon, a gradient-shaded noto emoji
# (skin-tone composite), and one flat single-colour simple-icon.
DEFAULT_IMAGES = [
    "web/samples/adamo.png",
    "web/samples/1075thefan.png",
    "web/samples/365retailmarkets.png",
    "bench/data/corpus_raster/material-icons/512ss/rule_folder.png",
    "bench/data/corpus_raster/material-icons/512ss/report.png",
    "bench/data/corpus_raster/lucide/512ss/cat.png",
    "bench/data/corpus_raster/noto-emoji/512ss/emoji_u1f3c2_1f3fd.png",
    "bench/data/corpus_raster/simple-icons/512ss/appletv.png",
]

# The Space defaults, restated once so the wasm call and the CLI flags cannot drift.
OPTIONS = {
    "precision": 0.1,
    "min_area": 2,
    "colors": 64,
    "merge": 0.035,
    "max_dim": 1024,
    "time_budget": 0,
    "no_background": False,
    "minify": False,
    "margin": 0,
    "content_units": False,
}

# Loaded in node; `self` is pointed at the real global because the threads arm's worker
# helper addresses `self` at import time and the wasm clock reads `performance` off the
# global object. The pool is never started: rayon runs its work on the calling thread,
# which is exactly the determinism this harness is here to pin. `module_or_path` is given
# the wasm's own bytes so nothing is fetched over the network.
RUNNER_JS = r"""
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const spec = JSON.parse(process.argv[2]);

if (typeof globalThis.self === "undefined") {
  globalThis.self = globalThis;
  globalThis.addEventListener = globalThis.addEventListener || (() => {});
  globalThis.removeEventListener = globalThis.removeEventListener || (() => {});
  globalThis.postMessage = globalThis.postMessage || (() => {});
}

const gluePath = pathToFileURL(spec.glue).href;
const mod = await import(gluePath);
await mod.default({ module_or_path: new Uint8Array(readFileSync(spec.wasm)) });

const bytes = new Uint8Array(readFileSync(spec.image));
const t0 = performance.now();
const svg = mod.trace(
  bytes,
  spec.o.precision, spec.o.min_area, spec.o.colors, spec.o.merge, spec.o.max_dim,
  spec.o.time_budget, spec.o.no_background, spec.o.minify, spec.o.margin,
  spec.o.content_units
);
const ms = performance.now() - t0;
writeFileSync(spec.out, svg, "utf8");
console.log(JSON.stringify({ ok: true, out: spec.out, bytes: svg.length, ms }));
"""


def sha1_file(p: Path) -> str:
    return hashlib.sha1(p.read_bytes()).hexdigest()


def run_native(exe: Path, image: Path, out: Path) -> subprocess.CompletedProcess:
    cmd = [
        str(exe), str(image),
        "-o", str(out),
        "--precision", str(OPTIONS["precision"]),
        "--min-area", str(OPTIONS["min_area"]),
        "--colors", str(OPTIONS["colors"]),
        "--merge", str(OPTIONS["merge"]),
        "--max-dim", str(OPTIONS["max_dim"]),
        "--time-budget", str(OPTIONS["time_budget"]),
        "--margin", str(OPTIONS["margin"]),
        "-q",
    ]
    return subprocess.run(cmd, capture_output=True, text=True)


def run_wasm(node: str, pkg_dir: Path, image: Path, work: Path, tag: str) -> tuple[Path, float]:
    spec = {
        "glue": str(pkg_dir / "inkvec_wasm.js"),
        "wasm": str(pkg_dir / "inkvec_wasm_bg.wasm"),
        "image": str(image),
        "out": str(work / f"{tag}.svg"),
        "o": OPTIONS,
    }
    runner = work / "parity_runner.mjs"
    proc = subprocess.run(
        [node, str(runner), json.dumps(spec)],
        capture_output=True, text=True, cwd=str(ROOT),
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"{tag}: node runner failed on {image.name}\n{proc.stdout}\n{proc.stderr}"
        )
    res = json.loads(proc.stdout.strip().splitlines()[-1])
    out = Path(res["out"])
    if not out.is_file():
        raise RuntimeError(f"{tag}: no output written for {image.name}")
    return out, res["ms"]


def first_divergence(a: bytes, b: bytes) -> str:
    n = min(len(a), len(b))
    i = 0
    while i < n and a[i] == b[i]:
        i += 1
    lo = max(0, i - 60)
    ctx_a = a[lo:i + 60].decode("utf-8", "replace")
    ctx_b = b[lo:i + 60].decode("utf-8", "replace")
    return f"first difference at byte {i} (len {len(a)} vs {len(b)})\n    A: ...{ctx_a}...\n    B: ...{ctx_b}..."


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--exe", default=str(ROOT / "target/release/inkvec.exe"))
    ap.add_argument("--pkg", default=str(ROOT / "web/pkg"))
    ap.add_argument("--threads", default=str(ROOT / "web/pkg-threads"))
    ap.add_argument("--node", default="node")
    ap.add_argument("--work", default=str(ROOT / "out/parity-work"))
    ap.add_argument("--images", nargs="*", default=DEFAULT_IMAGES)
    ap.add_argument("--time-budget", type=float, default=0.0,
                    help="seconds; nonzero makes the output depend on machine speed")
    ap.add_argument("--strict-native", action="store_true",
                    help="also fail when native differs from the wasm arms")
    args = ap.parse_args()
    OPTIONS["time_budget"] = args.time_budget

    exe = Path(args.exe)
    pkg = Path(args.pkg)
    thr = Path(args.threads)
    for need in (exe, pkg / "inkvec_wasm.js", pkg / "inkvec_wasm_bg.wasm",
                 thr / "inkvec_wasm.js", thr / "inkvec_wasm_bg.wasm"):
        if not need.is_file():
            print(f"harness cannot run: missing {need}", file=sys.stderr)
            return 2
    node = shutil.which(args.node) or args.node

    work = Path(args.work)
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)
    (work / "parity_runner.mjs").write_text(RUNNER_JS, encoding="utf-8")

    print(f"native : {exe}")
    print(f"pkg    : {pkg}")
    print(f"threads: {thr}")
    print(f"options: {json.dumps(OPTIONS)}")
    print()

    fails = 0
    native_diffs = 0
    rows = []
    for image in args.images:
        image = Path(image)
        if not image.is_absolute():
            image = ROOT / image
        if not image.is_file():
            print(f"SKIP (missing): {image}")
            fails += 1
            continue
        stem = image.stem

        native_out = work / f"{stem}.native.svg"
        proc = run_native(exe, image, native_out)
        if proc.returncode != 0 or not native_out.is_file():
            print(f"FAIL {stem}: native exited {proc.returncode}\n{proc.stdout}\n{proc.stderr}")
            fails += 1
            continue

        try:
            pkg_out, pkg_ms = run_wasm(node, pkg, image, work, f"{stem}.pkg")
            thr_out, thr_ms = run_wasm(node, thr, image, work, f"{stem}.mt")
        except RuntimeError as e:
            print(f"FAIL {stem}: {e}")
            fails += 1
            continue

        h_nat, h_pkg, h_mt = (sha1_file(p) for p in (native_out, pkg_out, thr_out))
        wasm_ok = h_pkg == h_mt
        native_ok = h_nat == h_pkg == h_mt
        ok = native_ok if args.strict_native else wasm_ok
        rows.append((stem, h_nat, h_pkg, h_mt, ok))
        status = "PASS" if ok else "FAIL"
        native_note = "" if native_ok else "  (native differs)"
        sizes = (native_out.stat().st_size, pkg_out.stat().st_size, thr_out.stat().st_size)
        print(f"{status} {stem}: native={sizes[0]}B pkg={sizes[1]}B mt={sizes[2]}B"
              f"  one-core wasm: pkg {pkg_ms:.0f} ms, mt {thr_ms:.0f} ms{native_note}")
        if not native_ok:
            native_diffs += 1
        if not ok:
            fails += 1
            if not wasm_ok:
                print("  pkg vs threads: " + first_divergence(
                    pkg_out.read_bytes(), thr_out.read_bytes()))
            if args.strict_native and h_nat != h_pkg:
                print("  native vs pkg: " + first_divergence(
                    native_out.read_bytes(), pkg_out.read_bytes()))

    print()
    gate = "native / pkg / pkg-threads" if args.strict_native else "pkg / pkg-threads"
    if native_diffs and not args.strict_native:
        print(f"note: native differs from wasm on {native_diffs} of {len(rows)} image(s) "
              "(platform libm; not gated)")
    if fails:
        print(f"{fails} image(s) FAILED parity across {gate}")
        return 1
    print(f"all {len(rows)} image(s) byte-identical across {gate}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
