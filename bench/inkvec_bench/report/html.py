"""HTML report: leaderboard, Pareto plots, and visual side-by-sides.

The visual grid matters as much as the tables. Every quantitative claim this harness
makes should be checkable by eye at 4x zoom, because that is where tracing failures
actually live and where a reviewer will look first.
"""
from __future__ import annotations

import base64
import html as _html
from pathlib import Path

import numpy as np

CSS = """
:root { color-scheme: light dark; --fg:#16181d; --bg:#fbfbfa; --mut:#6b7280;
        --line:#e3e3e0; --card:#fff; --accent:#2b6cb0; }
@media (prefers-color-scheme: dark) { :root {
  --fg:#e8e8e6; --bg:#16181d; --mut:#9aa0aa; --line:#2c3038; --card:#1d2027; --accent:#7aa7d8; } }
* { box-sizing:border-box; }
body { margin:0; padding:2rem 1.5rem 5rem; background:var(--bg); color:var(--fg);
       font:14px/1.55 ui-sans-serif,system-ui,-apple-system,Segoe UI,Roboto,sans-serif; }
.wrap { max-width:1180px; margin:0 auto; }
h1 { font-size:1.6rem; margin:0 0 .25rem; letter-spacing:-.01em; }
h2 { font-size:1.1rem; margin:2.5rem 0 .75rem; padding-bottom:.4rem;
     border-bottom:1px solid var(--line); letter-spacing:-.005em; }
.sub { color:var(--mut); margin:0 0 1.5rem; }
table { border-collapse:collapse; width:100%; font-size:12.5px; font-variant-numeric:tabular-nums; }
th,td { text-align:right; padding:.4rem .55rem; border-bottom:1px solid var(--line); }
th:first-child,td:first-child { text-align:left; }
th { color:var(--mut); font-weight:600; white-space:nowrap; }
tbody tr:hover { background:color-mix(in srgb, var(--accent) 7%, transparent); }
.scroll { overflow-x:auto; -webkit-overflow-scrolling:touch; }
img.plot { width:100%; max-width:760px; border:1px solid var(--line); border-radius:8px; background:var(--card); }
.grid { display:grid; grid-template-columns:repeat(auto-fill,minmax(150px,1fr)); gap:.85rem; }
.cell { background:var(--card); border:1px solid var(--line); border-radius:8px; padding:.5rem; }
.cell img { width:100%; image-rendering:pixelated; border-radius:4px;
            background:repeating-conic-gradient(#0002 0 25%, transparent 0 50%) 50%/12px 12px; }
.cell .cap { font-size:11px; color:var(--mut); margin-top:.35rem; line-height:1.35;
             overflow-wrap:anywhere; }
.note { background:var(--card); border:1px solid var(--line); border-left:3px solid var(--accent);
        border-radius:6px; padding:.75rem .9rem; margin:1rem 0; color:var(--mut); font-size:13px; }
code { font:12px ui-monospace,SFMono-Regular,Menlo,monospace;
       background:color-mix(in srgb, var(--fg) 8%, transparent); padding:.1em .35em; border-radius:3px; }
"""


def _img_b64(path: Path) -> str:
    return base64.b64encode(Path(path).read_bytes()).decode("ascii")


def _table(df, max_rows: int = 40) -> str:
    if df is None or len(df) == 0:
        return "<p class='sub'>No data.</p>"
    d = df.head(max_rows)
    head = "".join(f"<th>{_html.escape(str(c))}</th>" for c in d.columns)
    body = []
    for _, r in d.iterrows():
        cells = []
        for v in r:
            if isinstance(v, float):
                cells.append("—" if not np.isfinite(v) else (f"{v:.4g}" if abs(v) < 1e4 else f"{v:.0f}"))
            else:
                cells.append(_html.escape(str(v)))
        body.append("<tr>" + "".join(f"<td>{c}</td>" for c in cells) + "</tr>")
    return f"<div class='scroll'><table><thead><tr>{head}</tr></thead><tbody>{''.join(body)}</tbody></table></div>"


def build_report(
    df,
    out_dir: Path,
    plots: list[tuple[str, Path]],
    visuals: list[tuple[str, list[tuple[str, Path]]]] | None = None,
    robustness=None,
    title: str = "Inkvec benchmark",
) -> Path:
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    ok = df[df["error"].fillna("") == ""] if "error" in df else df
    n_fail = len(df) - len(ok)

    parts: list[str] = [
        f"<h1>{_html.escape(title)}</h1>",
        f"<p class='sub'>{len(ok)} scored cells &middot; {n_fail} failures &middot; "
        f"{ok['runner'].nunique() if len(ok) else 0} engines &middot; "
        f"{ok['uid'].nunique() if len(ok) else 0} inputs</p>",
        "<div class='note'>Fidelity is reported at multiple zoom levels because an SVG is "
        "resolution-independent: a tracer that snaps boundaries to the input pixel grid "
        "looks fine at <code>1x</code> and wrong at <code>4x</code>. Complexity is reported as "
        "<code>n_params</code> and <code>anchor_density</code> rather than raw anchor count, so a "
        "genuinely complex logo is not penalised for being complex.</div>",
    ]

    # --- leaderboard ---
    if len(ok):
        agg_cols = [c for c in (
            "n_params", "n_anchors", "anchor_density", "primitive_fraction",
            "overdraw_ratio", "svg_gzip_bytes", "runtime_s",
            "dists@1x", "dists@4x", "lpips@4x", "dino@1x", "ssim@4x", "de00_mean@4x",
            "seam_fraction@4x", "param_ratio", "primitive_recall",
        ) if c in ok.columns]
        if agg_cols:
            lb = ok.groupby("runner")[agg_cols].median().reset_index()
            parts += ["<h2>Leaderboard (median over all cells)</h2>", _table(lb)]

    # --- plots ---
    if plots:
        parts.append("<h2>Pareto frontiers</h2>")
        for caption, p in plots:
            if p and Path(p).exists():
                parts.append(
                    f"<p class='sub'>{_html.escape(caption)}</p>"
                    f"<img class='plot' src='data:image/png;base64,{_img_b64(p)}' alt=''>"
                )

    # --- robustness ---
    if robustness is not None and len(robustness):
        parts += [
            "<h2>Robustness to input perturbation</h2>",
            "<div class='note'>Parameter growth when the input is degraded. Boundary-following "
            "tracers inherit contour noise as redundant anchors. Published reference points: "
            "AnchorFlow +2.9%, AdaVec +20.7%, VTracer +106.7%.</div>",
            _table(robustness),
        ]

    # --- visuals ---
    if visuals:
        parts.append("<h2>Visual comparison</h2>")
        for name, cells in visuals:
            parts.append(f"<p class='sub'><strong>{_html.escape(name)}</strong></p><div class='grid'>")
            for cap, p in cells:
                if p and Path(p).exists():
                    parts.append(
                        f"<div class='cell'><img src='data:image/png;base64,{_img_b64(p)}' alt=''>"
                        f"<div class='cap'>{_html.escape(cap)}</div></div>"
                    )
            parts.append("</div>")

    doc = (
        f"<!doctype html><html><head><meta charset='utf-8'>"
        f"<meta name='viewport' content='width=device-width,initial-scale=1'>"
        f"<title>{_html.escape(title)}</title><style>{CSS}</style></head>"
        f"<body><div class='wrap'>{''.join(parts)}</div></body></html>"
    )
    dest = out_dir / "index.html"
    dest.write_text(doc, encoding="utf-8")
    return dest
