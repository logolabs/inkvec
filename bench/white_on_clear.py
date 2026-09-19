"""White artwork on a transparent ground: does it survive the trace?

    python bench/white_on_clear.py [--exe target/release/inkvec.exe] [--args " --cutout"]
    python bench/white_on_clear.py --wasm web/pkg [--args " --no-background --cutout"]

A white mark composited over the white matte is one flat colour and traces to nothing, so
this is the case a matte-based intake is most likely to lose. Each truth is an SVG,
rendered to a transparent PNG, traced, and the trace scored against the SVG on three
grounds as `alpha_eval.py` does: white, the design system's dark ground, and the alpha
channel itself. The controls (a black mark, white inside a coloured disc, a coloured mark)
guard the cases the default already handles.
"""
from __future__ import annotations

import argparse
import re
import shlex
import subprocess
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))

from inkvec_bench import render  # noqa: E402

SIZE = 512
DARK = np.array([0.078, 0.071, 0.063], np.float32)

FLASK = (
    "M 881.984 591.726 C 909.088 592.132 936.829 591.866 963.981 591.887 L 964.051 615.134 "
    "C 987.819 614.909 1017.69 614.432 1041.08 615.648 C 1040.93 607.346 1040.69 600.253 "
    "1041.98 592.045 C 1069.11 592.348 1096.88 591.998 1124.06 591.953 L 1123.92 675.668 "
    "L 1101.07 675.765 C 1095.31 774.162 1119.89 868.607 1055.36 953.552 C 1004.92 1019.95 "
    "937.496 1064.82 878.928 1123.13 C 845.28 1156.38 816.712 1194.4 794.144 1235.97 "
    "C 767.017 1286.42 741.545 1353.22 780.436 1404.37 C 796.668 1425.52 824.772 1443.38 "
    "851.877 1445.43 C 877.696 1447.4 906.17 1446.57 932.261 1446.6 L 1080.96 1446.69 "
    "L 1197.09 1446.81 C 1210.8 1446.82 1244.73 1447.6 1257.07 1445.9 C 1273.88 1443.68 "
    "1289.75 1436.88 1302.95 1426.25 C 1322.04 1410.93 1332.82 1389.99 1335.14 1365.73 "
    "C 1337.48 1341.57 1330.09 1317.48 1314.6 1298.79 C 1282.06 1259.91 1239.06 1266.08 "
    "1193.98 1266.11 L 1064.24 1266.18 C 1007.04 1266.21 963.782 1274.83 923.019 1226.4 "
    "C 902.441 1198.45 902.862 1175 902.924 1142.3 C 912.343 1133.33 929.852 1117.98 "
    "939.887 1110.13 C 940.427 1142.3 933.166 1185.04 957.055 1209.47 C 975.922 1228.76 "
    "996.316 1230.57 1021.97 1229.58 C 1035.66 1229.06 1050.27 1230.01 1064.04 1229.64 "
    "C 1123.95 1229.84 1183.88 1229.63 1243.79 1229.78 C 1278.77 1229.84 1312.9 1244 "
    "1337.05 1269.48 C 1360.56 1294.24 1373.23 1327.36 1372.25 1361.49 C 1371.21 1395.65 "
    "1356.25 1427.91 1330.85 1450.78 C 1293.97 1484.58 1264.51 1482.67 1218.46 1482.55 "
    "L 1136.27 1482.52 L 960.971 1482.87 C 928.505 1482.94 895.26 1483.33 863.01 1482.84 "
    "C 824.574 1482.3 784.504 1464.07 758.904 1435.1 C 675.08 1340.22 761.832 1202.37 "
    "828.1 1124.18 C 850.986 1097.18 877.862 1073.68 903.55 1050.22 C 901.884 1021.1 "
    "903.068 980.916 903.063 950.824 L 903.075 765.518 L 903.206 711.487 C 903.313 700.62 "
    "904.014 686.497 903.106 676.093 C 895.901 675.719 889.159 675.649 881.94 675.611 "
    "C 882.424 648.129 881.982 619.316 881.984 591.726 z M 964.007 675.527 C 955.974 "
    "675.648 947.939 675.718 939.905 675.736 L 939.967 1018.59 C 957.908 1003.76 976.395 "
    "987.535 992.628 970.848 C 1042.17 921.753 1064.98 876.954 1065.28 806.604 L 1064.84 "
    "676.083 L 1040.87 676.051 C 1041.64 666.978 1041.33 661.135 1040.9 651.947 C 1019.24 "
    "651.252 986.821 650.561 965.356 651.5 C 963.031 654.329 963.936 670.679 964.007 "
    "675.527 z M 1062.08 612.678 C 1061.97 626.562 1062.1 641.194 1061.69 655 L 1095.78 "
    "654.97 C 1098.9 654.996 1101.7 655.476 1104.18 653.836 C 1105.4 645.43 1104.6 621.8 "
    "1104.64 611.911 C 1099.78 612.021 1078.39 611.957 1074.37 612.466 L 1062.08 612.678 z"
)

SQ = 'xmlns="http://www.w3.org/2000/svg" width="512" height="512"'
CASES = {
    # The LogoLabs mark in white, as a brand would ship it for dark backgrounds.
    "flask_white": f'<svg {SQ} viewBox="585 550 970 970"><path fill="#ffffff" d="{FLASK}"/></svg>',
    # A ring (with its counter, which must stay clear) crossed by a bar.
    "ring_bar_white": f'<svg {SQ} viewBox="0 0 512 512"><circle cx="256" cy="256" r="160" '
                      'fill="none" stroke="#ffffff" stroke-width="72"/>'
                      '<rect x="226" y="40" width="60" height="432" fill="#ffffff"/></svg>',
    # White with a near-white detail: two inks that both vanish into a white matte.
    "white_and_gray": f'<svg {SQ} viewBox="0 0 512 512"><circle cx="256" cy="256" r="210" '
                      'fill="#ffffff"/><rect x="176" y="176" width="160" height="160" '
                      'fill="#d9d9d9"/></svg>',
    # Controls.
    "ring_bar_black": f'<svg {SQ} viewBox="0 0 512 512"><circle cx="256" cy="256" r="160" '
                      'fill="none" stroke="#000000" stroke-width="72"/>'
                      '<rect x="226" y="40" width="60" height="432" fill="#000000"/></svg>',
    "white_in_copper": f'<svg {SQ} viewBox="0 0 512 512"><circle cx="256" cy="256" r="220" '
                       'fill="#c9754a"/><circle cx="256" cy="256" r="110" fill="none" '
                       'stroke="#ffffff" stroke-width="40"/></svg>',
    "flask_copper": f'<svg {SQ} viewBox="585 550 970 970"><path fill="#c9754a" d="{FLASK}"/></svg>',
    # No single matte serves both: white meets the ground, and so does black.
    "panda": f'<svg {SQ} viewBox="0 0 512 512"><circle cx="150" cy="130" r="70" fill="#000000"/>'
             '<circle cx="362" cy="130" r="70" fill="#000000"/><circle cx="256" cy="290" r="190" '
             'fill="#ffffff"/><ellipse cx="190" cy="270" rx="40" ry="55" fill="#000000"/>'
             '<ellipse cx="322" cy="270" rx="40" ry="55" fill="#000000"/></svg>',
    # A white mark with a soft white glow: translucency that varies, next to paint that is
    # the matte's own colour.
    "white_glow": f'<svg {SQ} viewBox="0 0 512 512"><defs><radialGradient id="g">'
                  '<stop offset="0.45" stop-color="#ffffff" stop-opacity="0.8"/>'
                  '<stop offset="1" stop-color="#ffffff" stop-opacity="0"/></radialGradient></defs>'
                  '<circle cx="256" cy="256" r="240" fill="url(#g)"/>'
                  '<rect x="186" y="186" width="140" height="140" fill="#ffffff"/></svg>',
    # A flat translucent panel over an opaque disc and over nothing.
    "translucent_panel": f'<svg {SQ} viewBox="0 0 512 512"><circle cx="220" cy="256" r="180" '
                         'fill="#c9754a"/><rect x="250" y="120" width="220" height="272" '
                         'fill="#2b6cb0" fill-opacity="0.5"/></svg>',
    # A soft shadow under a dark mark.
    "soft_shadow": f'<svg {SQ} viewBox="0 0 512 512"><defs><filter id="b" x="-50%" y="-50%" '
                   'width="200%" height="200%"><feGaussianBlur stdDeviation="14"/></filter></defs>'
                   '<rect x="150" y="170" width="220" height="220" rx="30" fill="#000000" '
                   'fill-opacity="0.45" filter="url(#b)"/><rect x="130" y="130" width="220" '
                   'height="220" rx="30" fill="#1f2937"/></svg>',
}


def over(img: np.ndarray, ground: np.ndarray) -> np.ndarray:
    al = img[..., 3:4]
    return np.clip(img[..., :3] * al + ground * (1.0 - al), 0.0, 1.0)


def rgba(svg: str) -> np.ndarray:
    a = render.render(svg, SIZE, SIZE)
    if a.shape[-1] == 3:
        a = np.concatenate([a, np.ones(a.shape[:2] + (1,), np.float32)], axis=2)
    return a


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=str(ROOT / "target/release/inkvec.exe"))
    ap.add_argument("--args", default="", help='extra CLI flags, e.g. " --cutout"')
    ap.add_argument("--work", default=str(ROOT / "out/white_on_clear"))
    ap.add_argument("--wasm", help="trace with this wasm package in node instead of the exe; "
                                   "--args may then hold only --no-background and --cutout")
    a = ap.parse_args()
    work = Path(a.work)
    work.mkdir(parents=True, exist_ok=True)
    extra = shlex.split(a.args)
    tag = ("wasm_" if a.wasm else "") + ("default" if not extra else "_".join(x.lstrip("-") for x in extra))

    trace_wasm = None
    if a.wasm:
        import wasm_parity  # noqa: E402  (bench/ is on sys.path)

        unknown = set(extra) - {"--no-background", "--cutout"}
        if unknown:
            raise SystemExit(f"--wasm takes only --no-background/--cutout, not {sorted(unknown)}")
        wasm_parity.OPTIONS.update(time_budget=0, no_background="--no-background" in extra,
                                   cutout="--cutout" in extra)
        (work / "parity_runner.mjs").write_text(wasm_parity.RUNNER_JS, encoding="utf-8")

        def trace_wasm(png: Path, out: Path) -> str:
            got, _ = wasm_parity.run_wasm("node", Path(a.wasm), png, work, out.stem)
            out.write_bytes(got.read_bytes())
            return ""

    print(f"{'wasm ' + a.wasm if a.wasm else 'exe ' + a.exe}  args [{a.args.strip()}]")
    print(f"{'case':16} {'paths':>5} {'fills':28} {'white':>7} {'dark':>7} {'alpha':>7}  verdict")
    worst = 0.0
    for name, svg in CASES.items():
        png = work / f"{name}.png"
        png.write_bytes(render.render_to_png(svg, SIZE, SIZE))
        out = work / f"{name}.{tag}.svg"
        if trace_wasm:
            try:
                trace_wasm(png, out)
            except RuntimeError as e:
                print(f"{name:16} FAILED: {str(e)[:200]}")
                worst = max(worst, 1.0)
                continue
        else:
            p = subprocess.run([a.exe, str(png), "-o", str(out), "-q", *extra],
                               capture_output=True, text=True)
            if p.returncode != 0 or not out.is_file():
                print(f"{name:16} FAILED: {p.stderr.strip()[:200]}")
                worst = max(worst, 1.0)
                continue
        traced = out.read_text(encoding="utf-8")
        truth, trace = rgba(svg), rgba(traced)
        s_white = float(np.abs(over(trace, np.ones(3, np.float32)) - over(truth, np.ones(3, np.float32))).mean())
        s_dark = float(np.abs(over(trace, DARK) - over(truth, DARK)).mean())
        s_alpha = float(np.abs(trace[..., 3] - truth[..., 3]).mean())
        fills = sorted(set(re.findall(r'fill="(#[0-9a-fA-F]{3,6})"', traced)))
        paths = traced.count("<path") + traced.count("<rect") + traced.count("<circle")
        verdict = "ok" if max(s_dark, s_alpha) < 0.02 else "LOST" if s_alpha > 0.1 else "off"
        worst = max(worst, s_dark, s_alpha)
        print(f"{name:16} {paths:5} {','.join(fills)[:28]:28} {s_white:7.4f} {s_dark:7.4f} {s_alpha:7.4f}  {verdict}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
