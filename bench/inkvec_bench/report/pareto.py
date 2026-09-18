"""Pareto analysis.

The protocol rule for this benchmark: **report curves, not points.**

Every tracer has a quality knob, so a single (fidelity, complexity) pair says as much
about which setting the author picked as about the engine. Two engines can trade places
depending entirely on where each was sampled. The honest comparison is the frontier
each engine can reach across its own settings — "at equal anchor budget, who is more
accurate", answered across the whole budget range.
"""
from __future__ import annotations

from pathlib import Path

import numpy as np


def pareto_mask(cost: np.ndarray, quality: np.ndarray, higher_is_better: bool = True) -> np.ndarray:
    """Boolean mask of non-dominated points (minimise cost, optimise quality)."""
    q = quality if higher_is_better else -quality
    ok = np.isfinite(cost) & np.isfinite(q)
    idx = np.argsort(cost, kind="stable")
    best = -np.inf
    keep = np.zeros(len(cost), dtype=bool)
    for i in idx:
        if not ok[i]:
            continue
        if q[i] > best:
            keep[i] = True
            best = q[i]
    return keep


def frontier(df, cost_col: str, quality_col: str, higher_is_better: bool = True):
    """Return the subset of ``df`` on the Pareto frontier, sorted by cost."""
    sub = df[np.isfinite(df[cost_col]) & np.isfinite(df[quality_col])]
    if sub.empty:
        return sub
    m = pareto_mask(sub[cost_col].to_numpy(), sub[quality_col].to_numpy(), higher_is_better)
    return sub[m].sort_values(cost_col)


def plot(
    df,
    out_path: Path,
    cost_col: str = "n_params",
    quality_col: str = "dists@4x",
    higher_is_better: bool = False,
    title: str | None = None,
    group_col: str = "runner",
):
    """Scatter every setting, overlay each engine's frontier."""
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    sub = df[df["error"].fillna("") == ""] if "error" in df else df
    sub = sub[np.isfinite(sub.get(cost_col, np.nan)) & np.isfinite(sub.get(quality_col, np.nan))]
    if sub.empty:
        return None

    fig, ax = plt.subplots(figsize=(7.5, 5.0), dpi=140)
    colors = plt.cm.tab10.colors
    for i, (g, part) in enumerate(sub.groupby(group_col)):
        c = colors[i % len(colors)]
        ax.scatter(part[cost_col], part[quality_col], s=16, alpha=0.35, color=c, label=None)
        f = frontier(part, cost_col, quality_col, higher_is_better)
        if not f.empty:
            ax.plot(f[cost_col], f[quality_col], "-o", color=c, lw=1.8, ms=4.5, label=str(g))

    ax.set_xscale("log")
    ax.set_xlabel(f"{cost_col}  (lower is better)")
    direction = "higher is better" if higher_is_better else "lower is better"
    ax.set_ylabel(f"{quality_col}  ({direction})")
    ax.set_title(title or f"{quality_col} vs {cost_col}")
    ax.grid(alpha=0.25, which="both", lw=0.5)
    ax.legend(frameon=False, fontsize=8)
    fig.tight_layout()

    out_path = Path(out_path)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out_path)
    plt.close(fig)
    return out_path


def plot_seam_overdraw(df, out_path: Path, title: str | None = None):
    """The seam/overdraw plane — the trade a path-list representation cannot escape.

    Ground truth sits at (1.0, 0.0): every region drawn exactly once, no gaps. A
    stacked representation drifts right (geometry drawn repeatedly); a cutout mosaic
    drifts up (hairline gaps). A planar map with genuinely shared edges is the only
    thing that can sit in the corner.
    """
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    seam_col = next((c for c in df.columns if c.startswith("seam_fraction@")), None)
    if seam_col is None or "overdraw_ratio" not in df.columns:
        return None
    sub = df[np.isfinite(df["overdraw_ratio"]) & np.isfinite(df[seam_col])]
    if sub.empty:
        return None

    fig, ax = plt.subplots(figsize=(6.4, 5.0), dpi=140)
    colors = plt.cm.tab10.colors
    for i, (g, part) in enumerate(sub.groupby("runner")):
        ax.scatter(part["overdraw_ratio"], part[seam_col] * 100,
                   s=22, alpha=0.6, color=colors[i % len(colors)], label=str(g))
    ax.scatter([1.0], [0.0], marker="*", s=260, color="black", zorder=5,
               label="ideal (shared edges)")
    ax.set_xlabel("overdraw ratio   (1.0 = each region drawn once)")
    ax.set_ylabel("seam area  (% of opaque area)")
    ax.set_title(title or "Seam / overdraw trade-off")
    ax.grid(alpha=0.25, lw=0.5)
    ax.legend(frameon=False, fontsize=8)
    fig.tight_layout()

    out_path = Path(out_path)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(out_path)
    plt.close(fig)
    return out_path


def robustness_table(df):
    """Anchor/parameter growth under each perturbation, relative to the clean run.

    Reproduces AnchorFlow's protocol. Their reported bar: AnchorFlow +2.9%,
    AdaVec +20.7%, VTracer +106.7%.
    """
    import pandas as pd

    if "variant" not in df or df["variant"].nunique() < 2:
        return pd.DataFrame()
    ok = df[df["error"].fillna("") == ""]
    keys = ["runner", "setting", "uid"]
    clean = ok[ok["variant"] == "clean"].set_index(keys)["n_params"]
    rows = []
    for variant, part in ok[ok["variant"] != "clean"].groupby("variant"):
        p = part.set_index(keys)["n_params"]
        common = p.index.intersection(clean.index)
        if len(common) == 0:
            continue
        growth = (p.loc[common] / clean.loc[common] - 1.0) * 100
        g = growth.reset_index().rename(columns={"n_params": "growth_pct"})
        for runner, rp in g.groupby("runner"):
            rows.append({
                "runner": runner,
                "perturbation": variant,
                "param_growth_pct_median": float(rp["growth_pct"].median()),
                "param_growth_pct_mean": float(rp["growth_pct"].mean()),
                "n": int(len(rp)),
            })
    return pd.DataFrame(rows).sort_values(["perturbation", "param_growth_pct_median"])
