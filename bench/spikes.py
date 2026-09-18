"""Count spikes in an emitted SVG: anchors where the path doubles back on itself.

    python bench/spikes.py out/*.svg

Measured on 2026-09-07, `365retailmarkets` at 768 px: 12 spikes in 205 anchors with the
boundary solve on, 6 with `INKVEC_BOPT=0` — the solve doubles them and sharpens the worst
from 170 to 176 degrees. On the same logo pixel-upscaled 8x it is 74 spikes in 2595 anchors,
one of them a 180-degree needle with 650-px arms; undoing the upscale takes it to 7 in 132.


A spike is a vertex whose two incident tangents turn by more than SHARP degrees away from
straight — i.e. the outline comes in and goes back out almost the way it came. Corners are
not spikes: a right angle turns 90 degrees. Anything past ~150 is a needle.
"""
from __future__ import annotations

import math
import re
import sys
from pathlib import Path

NUM = r"[-+]?(?:\d+\.\d*|\.\d+|\d+)(?:[eE][-+]?\d+)?"
SHARP = 150.0   # degrees of turn
MIN_ARM = 0.35  # px; ignore needles shorter than this on both sides


def points(d: str):
    toks = re.findall(r"[MLCZmlcz]|" + NUM, d)
    i, cmd, cur, start = 0, None, (0.0, 0.0), None
    rings, ring = [], []
    while i < len(toks):
        t = toks[i]
        if t in "MLCZmlcz":
            cmd = t.upper()
            i += 1
            if cmd == "Z":
                if ring:
                    rings.append(ring)
                    ring = []
                cur = start or cur
                continue
        try:
            if cmd == "M":
                if ring:
                    rings.append(ring)
                cur = (float(toks[i]), float(toks[i + 1])); i += 2
                start = cur; ring = [cur]; cmd = "L"
            elif cmd == "L":
                cur = (float(toks[i]), float(toks[i + 1])); i += 2
                ring.append(cur)
            elif cmd == "C":
                vals = [float(toks[i + j]) for j in range(6)]; i += 6
                cur = (vals[4], vals[5]); ring.append(cur)
            else:
                i += 1
        except (IndexError, ValueError):
            break
    if ring:
        rings.append(ring)
    return rings


def spikes(svg: str):
    out = []
    for d in re.findall(r' d="([^"]+)"', svg):
        for ring in points(d):
            n = len(ring)
            if n < 4:
                continue
            for k in range(n):
                a, b, c = ring[(k - 1) % n], ring[k], ring[(k + 1) % n]
                u = (b[0] - a[0], b[1] - a[1])
                v = (c[0] - b[0], c[1] - b[1])
                lu, lv = math.hypot(*u), math.hypot(*v)
                if lu < 1e-6 or lv < 1e-6:
                    continue
                cos = max(-1.0, min(1.0, (u[0] * v[0] + u[1] * v[1]) / (lu * lv)))
                turn = math.degrees(math.acos(cos))
                if turn > SHARP and max(lu, lv) > MIN_ARM:
                    out.append((b, round(turn, 1), round(lu, 2), round(lv, 2)))
    return out


if __name__ == "__main__":
    for p in sys.argv[1:]:
        svg = Path(p).read_text(encoding="utf-8")
        s = spikes(svg)
        anchors = len(re.findall(r"[LC]", " ".join(re.findall(r' d="([^"]+)"', svg))))
        print(f"{Path(p).name:28s} anchors {anchors:5d}  spikes {len(s):4d}"
              + ("   worst " + str(sorted(s, key=lambda t: -t[1])[:3]) if s else ""))
