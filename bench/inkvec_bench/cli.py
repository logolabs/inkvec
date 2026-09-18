"""Command line entry point.

    inkvec-bench status                      what is installed and usable
    inkvec-bench build                       generate the corpus and render raster tiers
    inkvec-bench run                         sweep runners x settings, score every cell
    inkvec-bench report                      Pareto plots + HTML
    inkvec-bench all                         build, run, report
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

from . import config
from .corpora import build as corpus_build
from .corpora import perturb as corpus_perturb
from .corpora import synthetic


def _manifest_path() -> Path:
    return config.CORPUS_RASTER_DIR / "manifest.csv"


# --- commands ----------------------------------------------------------------------


def cmd_status(args) -> int:
    from .metrics.raster import perceptual_available
    from .runners import registry

    print("renderer")
    try:
        from .render import render

        render('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><rect width="8" height="8"/></svg>', 8, 8)
        print("  resvg                OK")
    except Exception as e:
        print(f"  resvg                FAILED: {e}")

    print("runners")
    for name, ok, why in registry.status():
        print(f"  {name:20s} {'OK' if ok else 'unavailable':12s} {why}")

    print("metrics")
    print("  psnr/ssim            OK  (scikit-image)")
    print("  deltaE00             OK  (scikit-image)")
    _perceptual_hints = {
        "lpips": "  pip install lpips",
        "dists": "  pip install DISTS-pytorch",
        # DINOv3 is a gated repo: the usual failure is a missing licence acceptance
        # rather than a missing package, so say so instead of suggesting an install.
        "dino": "  pip install transformers; accept the licence on the model page "
                "and huggingface-cli login",
    }
    for k, ok in perceptual_available().items():
        hint = "" if ok else _perceptual_hints.get(k, "")
        label = k
        if k == "dino":
            from .metrics.dino import DEFAULT_BACKBONE

            label = f"dino ({DEFAULT_BACKBONE})"
        print(f"  {label:20s} {'OK' if ok else 'unavailable':12s}{hint}")
    try:
        import shapely  # noqa: F401

        print("  overdraw/validity    OK  (shapely)")
    except ImportError:
        print("  overdraw/validity    unavailable  pip install shapely")

    print(f"\ndata root: {config.DATA_ROOT}")
    mp = _manifest_path()
    print(f"corpus manifest: {'present' if mp.exists() else 'not built yet'} ({mp})")
    return 0


def cmd_build(args) -> int:
    config.ensure_data_dirs()
    tiers = tuple(args.tiers) if args.tiers else config.RASTER_TIERS

    items = []
    if not args.no_synthetic:
        d = config.CORPUS_SVG_DIR / "synthetic"
        specs = synthetic.build(d, seed=args.seed)
        print(f"synthetic: {len(specs)} source SVGs -> {d}")
        items += corpus_build.build_rasters(d, config.CORPUS_RASTER_DIR, "synthetic", tiers)

    for extra in args.svg_dir or []:
        p = Path(extra)
        name = p.name
        print(f"importing SVG corpus {name} from {p}")
        items += corpus_build.build_rasters(p, config.CORPUS_RASTER_DIR, name, tiers)

    for extra in args.image_dir or []:
        p = Path(extra)
        print(f"importing raster corpus from {p}")
        items += corpus_build.import_wild(p, config.CORPUS_RASTER_DIR, p.name)

    corpus_build.write_manifest(items, _manifest_path())
    print(f"\n{len(items)} raster inputs across tiers {tiers}")
    print(f"manifest: {_manifest_path()}")
    return 0


def cmd_run(args) -> int:
    import pandas as pd

    from . import evaluate
    from .runners import registry

    mp = _manifest_path()
    if not mp.exists():
        print("No corpus. Run: inkvec-bench build", file=sys.stderr)
        return 1

    items = corpus_build.read_manifest(mp)
    if args.tiers:
        items = [i for i in items if i.tier in set(args.tiers)]
    if args.corpus:
        items = [i for i in items if i.corpus in set(args.corpus)]
    if args.limit:
        items = items[: args.limit]
    if not items:
        print("No corpus items match the filters.", file=sys.stderr)
        return 1

    if args.vectorizer_ai_mode == "production":
        n = len(items) * 4
        print(
            f"\n  Vectorizer.AI is set to PRODUCTION mode.\n"
            f"  This will consume paid API credits for roughly {n} images.\n"
        )
        if not args.yes and input("  Type 'yes' to continue: ").strip().lower() != "yes":
            print("Aborted.")
            return 1

    runners = registry.build(args.runners, args.vectorizer_ai_mode)
    usable = []
    for r in runners:
        ok, why = r.available()
        if ok:
            usable.append(r)
        else:
            print(f"skipping {r.name}: {why}")
    if not usable:
        print("No usable runners.", file=sys.stderr)
        return 1

    paths = config.Paths(args.tag).ensure()
    print(f"{len(items)} inputs x {len(usable)} runners -> {paths.runs}")

    scales = config.EVAL_SCALES_DEEP if getattr(args, "deep", False) else config.EVAL_SCALES
    print(f"evaluating fidelity at scales {scales}")
    rows = evaluate.sweep(items, usable, paths.runs, "clean",
                          edit_locality=args.edit_locality, scales=scales)

    if args.perturb:
        png_by_uid = {i.uid: i.png_path for i in items}
        for kind in args.perturb:
            print(f"\nperturbation: {kind}")
            pdir = config.CORPUS_RASTER_DIR / "_perturbed" / kind
            made = corpus_perturb.build_perturbed(
                list(png_by_uid.values()), config.CORPUS_RASTER_DIR / "_perturbed", (kind,), args.seed
            )[kind]
            by_name = {p.name: p for p in made}
            pmap = {
                uid: by_name[Path(src).name]
                for uid, src in png_by_uid.items()
                if Path(src).name in by_name
            }
            rows += evaluate.sweep(items, usable, paths.runs, kind, pmap, False, scales)

    df = evaluate.rows_to_frame(rows)
    out = paths.results / "results.csv"
    df.to_csv(out, index=False)
    n_fail = int((df["error"].fillna("") != "").sum())
    print(f"\n{len(df)} cells ({n_fail} failed) -> {out}")
    return 0


def cmd_report(args) -> int:
    import pandas as pd

    from .render import render, render_to_png
    from .report import html, pareto

    paths = config.Paths(args.tag).ensure()
    csv_path = paths.results / "results.csv"
    if not csv_path.exists():
        print(f"No results at {csv_path}. Run: inkvec-bench run", file=sys.stderr)
        return 1
    df = pd.read_csv(csv_path)
    if "error" not in df:
        df["error"] = ""
    clean = df[df["variant"] == "clean"] if "variant" in df else df

    plots = []
    quality_candidates = [
        ("dists@4x", False), ("lpips@4x", False), ("dists@1x", False),
        ("lpips@1x", False), ("ssim@4x", True), ("ssim@1x", True),
        ("dino@1x", True), ("dino@4x", True),
    ]
    for qcol, higher in quality_candidates:
        if qcol in clean.columns and clean[qcol].notna().any():
            for cost in ("n_params", "anchor_density"):
                if cost not in clean.columns:
                    continue
                p = pareto.plot(
                    clean, paths.report / f"pareto_{cost}_{qcol.replace('@','_')}.png",
                    cost_col=cost, quality_col=qcol, higher_is_better=higher,
                    title=f"{qcol} vs {cost} — Pareto frontier per engine",
                )
                if p:
                    plots.append((f"Fidelity ({qcol}) against complexity ({cost}).", p))
            break

    p = pareto.plot_seam_overdraw(clean, paths.report / "seam_overdraw.png")
    if p:
        plots.append((
            "Seam vs overdraw. A path-list representation must trade one against the "
            "other; only genuinely shared edges reach the corner.", p))

    robustness = pareto.robustness_table(df)

    # Visual side-by-sides at 4x for a few representative inputs.
    visuals = []
    if not args.no_visuals:
        names = args.visual or ["prim_circle", "mosaic_pie6", "logo_like", "symmetry_rot5"]
        vdir = paths.report / "visuals"
        vdir.mkdir(parents=True, exist_ok=True)
        for name in names:
            sub = clean[clean["name"] == name]
            if sub.empty:
                continue
            tier = int(sub["tier"].min())
            cells = []
            gt = config.CORPUS_SVG_DIR / "synthetic" / f"{name}.svg"
            if gt.exists():
                out = vdir / f"{name}_gt.png"
                out.write_bytes(render_to_png(gt.read_text(encoding="utf-8"), tier * 4, tier * 4))
                cells.append((f"ground truth @{tier*4}px", out))
            src = config.CORPUS_RASTER_DIR / "synthetic" / str(tier) / f"{name}.png"
            if src.exists():
                cells.append((f"input @{tier}px", src))
            for runner, part in sub.groupby("runner"):
                qcol = next((c for c, _ in quality_candidates if c in part.columns and part[c].notna().any()), None)
                # Show each engine at its *best* setting, not an arbitrary one.
                best = part.sort_values(qcol).iloc[0] if qcol else part.iloc[0]
                key = str(best.get("setting_key", "") or "")
                cand = paths.runs / str(runner) / "clean" / str(tier) / key / f"{name}.svg"
                if not cand.exists():
                    found = list((paths.runs / str(runner)).rglob(f"{tier}/*/{name}.svg"))
                    if not found:
                        continue
                    cand = found[0]
                out = vdir / f"{name}_{runner}.png"
                try:
                    out.write_bytes(render_to_png(cand.read_text(encoding="utf-8"), tier * 4, tier * 4))
                except Exception:
                    continue
                cells.append((f"{runner} · {best.get('setting','')} · {int(best.get('n_params', 0))} params", out))
            if cells:
                visuals.append((f"{name} (input {tier}px, shown at 4x)", cells))

    dest = html.build_report(df, paths.report, plots, visuals, robustness)
    print(f"report: {dest}")
    if len(robustness):
        print("\nRobustness (parameter growth under perturbation):")
        print(robustness.to_string(index=False))
    return 0


def cmd_all(args) -> int:
    for fn in (cmd_build, cmd_run, cmd_report):
        rc = fn(args)
        if rc:
            return rc
    return 0


# --- argument parsing --------------------------------------------------------------


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog="inkvec-bench", description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--tag", default="default", help="name this run; separates outputs")
    sub = p.add_subparsers(dest="cmd", required=True)

    sp = sub.add_parser("status", help="show what is installed and usable")
    sp.set_defaults(func=cmd_status)

    sp = sub.add_parser("build", help="generate corpus and render raster tiers")
    sp.add_argument("--tiers", type=int, nargs="*", help=f"default {config.RASTER_TIERS}")
    sp.add_argument("--no-synthetic", action="store_true")
    sp.add_argument("--svg-dir", nargs="*", help="extra ground-truth SVG corpora")
    sp.add_argument("--image-dir", nargs="*", help="in-the-wild raster corpora (no ground truth)")
    sp.add_argument("--seed", type=int, default=0)
    sp.set_defaults(func=cmd_build)

    sp = sub.add_parser("run", help="sweep runners and score every cell")
    sp.add_argument("--runners", nargs="*", help="default: all local runners")
    sp.add_argument("--tiers", type=int, nargs="*")
    sp.add_argument("--corpus", nargs="*")
    sp.add_argument("--limit", type=int)
    sp.add_argument("--perturb", nargs="*", choices=sorted(corpus_perturb.PERTURBATIONS),
                    help="also run the robustness protocol")
    sp.add_argument("--edit-locality", action="store_true", help="expensive; re-renders per probe")
    sp.add_argument("--deep", action="store_true", help="also evaluate fidelity at 16x (~4x slower)")
    sp.add_argument("--vectorizer-ai-mode", choices=["test", "production"],
                    help="enable the paid API. 'test' is free but watermarked, so it "
                         "corrupts pixel metrics; 'production' consumes credits.")
    sp.add_argument("--yes", action="store_true", help="skip the paid-API confirmation")
    sp.add_argument("--seed", type=int, default=0)
    sp.set_defaults(func=cmd_run)

    sp = sub.add_parser("report", help="Pareto plots and HTML report")
    sp.add_argument("--visual", nargs="*", help="input names to show side by side")
    sp.add_argument("--no-visuals", action="store_true")
    sp.set_defaults(func=cmd_report)

    sp = sub.add_parser("all", help="build, run, report")
    for a in ("--tiers",):
        sp.add_argument(a, type=int, nargs="*")
    sp.add_argument("--runners", nargs="*")
    sp.add_argument("--corpus", nargs="*")
    sp.add_argument("--limit", type=int)
    sp.add_argument("--perturb", nargs="*", choices=sorted(corpus_perturb.PERTURBATIONS))
    sp.add_argument("--edit-locality", action="store_true")
    sp.add_argument("--deep", action="store_true")
    sp.add_argument("--vectorizer-ai-mode", choices=["test", "production"])
    sp.add_argument("--yes", action="store_true")
    sp.add_argument("--no-synthetic", action="store_true")
    sp.add_argument("--svg-dir", nargs="*")
    sp.add_argument("--image-dir", nargs="*")
    sp.add_argument("--visual", nargs="*")
    sp.add_argument("--no-visuals", action="store_true")
    sp.add_argument("--seed", type=int, default=0)
    sp.set_defaults(func=cmd_all)

    args = p.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
