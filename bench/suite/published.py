"""Numbers quoted from papers, kept apart from numbers we measured.

Nothing here was run on this machine. These are figures printed by other people about
systems whose weights or binaries are not available to us, and they are in a separate file
so that they can never be mistaken for a row of our own table. Each carries its source.

Comparing against them is legitimate but only under the conditions each entry records: the
benchmark, the tier, and the metric implementation have to match, and where our harness
differs -- it renders with resvg because libcairo does not load here, while SVGenius uses
cairosvg -- that is noted on the entry rather than buried.
"""
from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Reference:
    system: str
    benchmark: str
    lpips: float | None
    dino: float | None
    source: str
    note: str = ""


REFERENCES: list[Reference] = [
    Reference(
        system="StarVector-8B",
        benchmark="svgenius-hard",
        lpips=0.258,
        dino=0.893,
        source="VectorArk, arXiv 2605.24398, reported as the strongest baseline",
    ),
    Reference(
        system="VectorArk (InternVL2-1B)",
        benchmark="svgenius-hard",
        lpips=0.120,
        dino=0.958,
        source="VectorArk, arXiv 2605.24398, CVPR 2026",
        note="runs a classical vectoriser at inference to build its input outline; "
             "colour is recovered afterwards as a per-path median; "
             "33-44 s per image on an A100",
    ),
    Reference(
        system="StarVector-8B",
        benchmark="sarena-hard",
        lpips=0.252,
        dino=0.902,
        source="VectorArk, arXiv 2605.24398",
    ),
    Reference(
        system="VectorArk (InternVL2-1B)",
        benchmark="sarena-hard",
        lpips=0.093,
        dino=0.975,
        source="VectorArk, arXiv 2605.24398",
    ),
]


def print_reference(benchmark: str) -> None:
    rows = [r for r in REFERENCES if r.benchmark == benchmark]
    if not rows:
        return
    print(f"\npublished figures for {benchmark} (quoted, not reproduced here):")
    for r in rows:
        lp = f"{r.lpips:.3f}" if r.lpips is not None else "  -  "
        dn = f"{r.dino:.3f}" if r.dino is not None else "  -  "
        print(f"  {r.system:26s} LPIPS {lp}   DINO {dn}   [{r.source}]")
        if r.note:
            print(f"  {'':26s} {r.note}")
    print("  Our rows render through resvg; SVGenius's own harness uses cairosvg, which")
    print("  can move absolute values. Treat gaps of a few per cent as unresolved.")
