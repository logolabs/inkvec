"""Trace a raster, optionally cleaning it with the x4 upscaler first.

    python -m inkvec_sr photo.png -o photo.svg              # decide per image
    python -m inkvec_sr shot.png -o shot.svg --mode on      # always clean
    python -m inkvec_sr *.png -o ./out --mode on
    python -m inkvec_sr shot.png -o clean.png --png-only    # just the raster

Why there is a mode at all, rather than always doing it: on **clean** input the
pre-pass is three times worse than tracing directly (dE00 0.195 against 0.605),
because the network's own edge profile costs more than the damage it removes when
there is no damage. On JPEG q50 it improves every column at once. So the
interesting default is neither on nor off but `auto`, which traces once, asks the
trace how well it fits the input, and only reaches for the GPU when the answer is
"badly".

The detector is the interior residual: a traced model is piecewise flat by
construction, so wherever it is locally constant the input should be too, and the
disagreement there is degradation rather than modelling error. It reads 0.000 on
an undamaged raster and 1.509 on JPEG q50. The cheaper candidate -- an 8x8 block
signature needing no trace -- was measured and does not discriminate (2.175 clean
against 2.133 JPEG), so `auto` pays for one extra trace instead.

Measured during development.
"""
from __future__ import annotations

import argparse
import subprocess
import sys
import time
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_EXE = ROOT / "target/release/inkvec.exe"


def _load_rgba(path: Path) -> np.ndarray:
    from PIL import Image

    return np.asarray(Image.open(path).convert("RGBA"), np.float64) / 255.0


def _on_white(rgba: np.ndarray) -> np.ndarray:
    a = rgba[..., 3:4]
    return rgba[..., :3] * a + (1.0 - a)


def _save(rgba: np.ndarray, path: Path) -> None:
    from PIL import Image

    arr = (rgba.clip(0, 1) * 255).round().astype(np.uint8)
    mode = "RGBA" if arr.shape[2] == 4 else "RGB"
    Image.fromarray(arr, mode).save(path, optimize=True)


def _trace(exe: Path, src: Path, dst: Path, extra: list[str]) -> float:
    t = time.perf_counter()
    r = subprocess.run([str(exe), str(src), "-o", str(dst), "--quiet", *extra],
                       capture_output=True)
    if r.returncode != 0 or not dst.exists():
        msg = (r.stderr or b"").decode(errors="replace").strip()
        raise SystemExit(f"inkvec failed on {src.name}: {msg[:400]}")
    return time.perf_counter() - t


def _render_like(svg_text: str, w: int, h: int) -> np.ndarray | None:
    """Render an SVG at `w` x `h` on white, or None if no renderer is installed.

    The width and height are both passed because the caller compares the result against
    the input pixel for pixel. This took a square `n` x `n` until 2026-09-08, which meant
    `--mode auto` raised a broadcast error on any image that was not square -- a 400x100
    wordmark rendered the model at 100x100 and numpy refused to subtract it. Since the only
    caller renders in order to difference against the source, matching the source's shape
    is not a nicety here; it is the whole contract.
    """
    sys.path.insert(0, str(ROOT / "bench"))
    try:
        from inkvec_bench import render  # noqa: PLC0415
    except Exception:  # noqa: BLE001
        return None
    try:
        r = render.render(svg_text, w, h)
    except Exception:  # noqa: BLE001
        return None
    a = r[..., 3:4]
    return r[..., :3] * a + (1.0 - a)


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="inkvec_sr",
        description="Trace a raster, optionally cleaning it with the x4 "
                    "upscaler first.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="Modes:\n"
               "  auto  trace, measure how well the trace fits, clean and "
               "retrace only if it fits badly (default)\n"
               "  on    always clean first\n"
               "  off   never clean; identical to calling inkvec directly\n")
    p.add_argument("inputs", nargs="+", type=Path)
    p.add_argument("-o", "--out", type=Path, required=True,
                   help="output file, or a directory when there are several inputs")
    p.add_argument("--mode", choices=["auto", "on", "off"], default="auto")
    p.add_argument("--threshold", type=float, default=None,
                   help="interior residual above which auto cleans "
                        "(default 0.5; clean reads 0.000, JPEG q50 reads 1.509)")
    p.add_argument("--scale", type=int, default=2,
                   help="output raster scale before tracing (default 2)")
    p.add_argument("--no-recolour", action="store_true",
                   help="skip putting flat colours back on the source's values")
    p.add_argument("--png", action="store_true",
                   help="also keep the cleaned raster beside the SVG")
    p.add_argument("--png-only", action="store_true",
                   help="write the cleaned raster and stop; do not trace")
    p.add_argument("--exe", type=Path, default=DEFAULT_EXE)
    p.add_argument("--sr-package", type=Path, default=None,
                   help="directory holding weights/ and mambairv2_arch.py")
    p.add_argument("--device", default=None)
    p.add_argument("--tile", type=int, default=256)
    p.add_argument("--overlap", type=int, default=32)
    p.add_argument("--fp32", action="store_true", help="disable bf16 autocast")
    p.add_argument("--args", default="",
                   help="extra arguments passed through to inkvec")
    p.add_argument("-q", "--quiet", action="store_true")
    return p


def main(argv: list[str] | None = None) -> int:
    from . import clean as C

    a = build_parser().parse_args(argv)
    thr = C.DEGRADED_RESIDUAL if a.threshold is None else a.threshold
    extra = a.args.split() if a.args else []
    say = (lambda *x: None) if a.quiet else (lambda *x: print(*x, flush=True))

    if not a.png_only and not a.exe.exists():
        raise SystemExit(f"inkvec not found at {a.exe}; build it or pass --exe")

    many = len(a.inputs) > 1
    if many:
        a.out.mkdir(parents=True, exist_ok=True)
    elif a.out.parent != Path(""):
        a.out.parent.mkdir(parents=True, exist_ok=True)

    up = None  # loaded lazily: `auto` on clean input never needs the GPU

    def upscaler():
        nonlocal up
        if up is None:
            from . import model

            up = model.load(a.sr_package, a.device)
            say(f"  upscaler: {up.label}")
        return up

    rc = 0
    for src in a.inputs:
        if not src.exists():
            print(f"  {src}: no such file", file=sys.stderr)
            rc = 1
            continue
        stem = src.stem
        if a.png_only:
            dst = (a.out / f"{stem}.clean.png") if many else a.out
        else:
            dst = (a.out / f"{stem}.svg") if many else a.out

        rgba = _load_rgba(src)
        do_clean = a.mode == "on"
        note = ""
        probe_sec = 0.0

        if a.mode == "auto" and not a.png_only:
            first = dst.with_suffix(".pass1.svg")
            sec = probe_sec = _trace(a.exe, src, first, extra)
            model_rgb = _render_like(first.read_text(encoding="utf-8"),
                                     rgba.shape[1], rgba.shape[0])
            if model_rgb is None:
                note = ("auto needs an SVG renderer (pip install resvg-py); "
                        "leaving the input alone")
                resid = float("nan")
            else:
                resid = C.interior_residual(_on_white(rgba), model_rgb)
            do_clean = np.isfinite(resid) and resid > thr
            if not do_clean:
                first.replace(dst)
                say(f"  {src.name} -> {dst.name}  residual {resid:.3f} "
                    f"<= {thr}, traced directly  {sec:.2f}s"
                    + (f"  [{note}]" if note else ""))
                continue
            first.unlink(missing_ok=True)
            note = f"residual {resid:.3f} > {thr}"
        elif a.mode == "auto" and a.png_only:
            do_clean = True  # nothing to measure without a trace

        if not do_clean:
            sec = _trace(a.exe, src, dst, extra)
            say(f"  {src.name} -> {dst.name}  no pre-pass  {sec:.2f}s")
            continue

        up_ = upscaler()          # model load is one-off, not per image
        t0 = time.perf_counter()
        cleaned = C.prepass(up_, rgba, out_scale=a.scale,
                            recolour=not a.no_recolour, tile=a.tile,
                            overlap=a.overlap, bf16=not a.fp32)
        sr_sec = time.perf_counter() - t0

        if a.png_only:
            _save(cleaned, dst)
            say(f"  {src.name} -> {dst.name}  "
                f"{cleaned.shape[1]}x{cleaned.shape[0]}  {sr_sec:.2f}s")
            continue

        png = dst.with_suffix(".clean.png")
        _save(cleaned, png)
        try:
            tr_sec = _trace(a.exe, png, dst, extra)
        finally:
            if not a.png:
                png.unlink(missing_ok=True)
        total = probe_sec + sr_sec + tr_sec
        say(f"  {src.name} -> {dst.name}  cleaned x{a.scale}"
            + (f" ({note})" if note else "")
            + (f"  probe {probe_sec:.2f}s" if probe_sec else "")
            + f"  sr {sr_sec:.2f}s  trace {tr_sec:.2f}s  total {total:.2f}s")

    return rc
