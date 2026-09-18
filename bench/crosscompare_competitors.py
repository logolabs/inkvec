"""Inkvec vs. named competitors: VTracer 1.0 pre-release (highest priority), plus
RasterTrace/Trazor research.

Reuses the shared case set and metric/scoring code from crosscompare_current.py
(imports select()/structure()/rgb()/png()/sha()) exactly as crosscompare_4way.py
does, so the case set and metric definitions cannot drift between runs.

Engines:
  inkvec              target/release/inkvec.exe, CLI defaults, --quiet. Same
                      binary crosscompare_4way.py uses; not rebuilt here.
  vtracer-default      Released vtracer Python binding (installed: 0.6.15,
                      "vtracer-default" in crosscompare_4way.py), defaults.
  vtracer-1.0-default  visioncortex/vtracer tag 1.0.0-alpha.4 (newest 1.0
                      pre-release; latest.json app build 163 / cee22021,
                      published 2026-09-12), official Windows CLI zip from
                      GitHub Releases, run with no extra flags (its own
                      defaults: clustering=color-cluster, hierarchical=stacked,
                      mode unset -> spline-equivalent internal default).
  vtracer-1.0-simplify Same 1.0.0-alpha.4 binary, with the new features the
                      user asked about turned on explicitly (neither is
                      default): --simplify 1.5 (paper.js-style curve
                      simplification, tolerance in the tool's own suggested
                      1-2.5 px range) and --hierarchical cutout (the new
                      seam-free mosaic tessellation with shared boundaries,
                      replacing the old lossy "cutout").
  trazor-auto          github.com/PhenX/Trazor (MIT, clean-room, no GPL/WASM
                      wrapping -- own from-scratch Potrace-class tracer),
                      cloned locally and run via its own @trazor/engine
                      through a small Node/tsx CLI wrapper written for this
                      benchmark (tools/vendor/trazor/trace-cli.ts). "auto"
                      mode reproduces what the trazor.studio browser app
                      applies on image load: analyzeImage() + recommendSettings()
                      + profile patch, over DEFAULT_SETTINGS -- not a forced
                      profile. Traced locally, no upload, no browser.

RasterTrace (github.com/bradsec/rastertrace) is NOT benchmarked as a separate
engine: it is a browser-only WASM app (rastertrace.com) whose own page states
"Vector tracing is powered by the open-source vtracer 1.0 alpha and
visioncortex libraries" and "Converted to SVG on your machine; no upload" --
i.e. it *is* vtracer 1.0-alpha compiled to WASM with a UI around it, not an
independent engine. The vtracer-1.0-* rows above measure that engine directly
(minus RasterTrace's own pre/post-processing, if any -- we did not inspect its
JS glue, only its own stated attribution). We do not upload images to
rastertrace.com or automate its browser UI, per the task rules.

    python bench/crosscompare_competitors.py [--limit N]
"""
from __future__ import annotations
import hashlib, json, os, subprocess, sys, time
from pathlib import Path
import numpy as np
from PIL import Image
from skimage.color import rgb2lab, deltaE_ciede2000
from skimage.metrics import structural_similarity

ROOT = Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT), str(ROOT / 'bench')]
from inkvec_bench import render
from inkvec_bench.metrics import raster as mraster, dino as mdino
import crosscompare_current as cc

OUT = ROOT / 'out/crosscompare-competitors-2026-09-15'
EXE = ROOT / 'target/release/inkvec.exe'
VTRACER10 = ROOT / 'tools/vendor/vtracer-1.0/extracted/vtracer.exe'
TRAZOR_DIR = ROOT / 'tools/vendor/trazor'
TRAZOR_CLI = TRAZOR_DIR / 'trace-cli.ts'
NPX = 'npx.cmd' if os.name == 'nt' else 'npx'
ENGINES = ['inkvec', 'vtracer-default', 'vtracer-1.0-default', 'vtracer-1.0-simplify', 'trazor-auto']


def trace_one(engine, png_path, out_svg):
    if engine == 'inkvec':
        r = subprocess.run([str(EXE), str(png_path), '-o', str(out_svg), '--quiet'], capture_output=True, timeout=180)
        if r.returncode:
            raise RuntimeError(r.stderr.decode(errors='replace'))
    elif engine == 'vtracer-default':
        import vtracer
        out_svg.write_text(vtracer.convert_raw_image_to_svg(png_path.read_bytes(), img_format='png'), encoding='utf-8')
    elif engine == 'vtracer-1.0-default':
        r = subprocess.run([str(VTRACER10), '--input', str(png_path), '--output', str(out_svg)],
                           capture_output=True, timeout=180, text=True)
        if r.returncode or not out_svg.exists():
            raise RuntimeError(f'rc={r.returncode} {r.stderr[:2000]}')
    elif engine == 'vtracer-1.0-simplify':
        r = subprocess.run([str(VTRACER10), '--input', str(png_path), '--output', str(out_svg),
                            '--simplify', '1.5', '--hierarchical', 'cutout'],
                           capture_output=True, timeout=180, text=True)
        if r.returncode or not out_svg.exists():
            raise RuntimeError(f'rc={r.returncode} {r.stderr[:2000]}')
    elif engine == 'trazor-auto':
        r = subprocess.run([NPX, 'tsx', str(TRAZOR_CLI), str(png_path), str(out_svg), '--mode', 'auto'],
                           capture_output=True, timeout=180, text=True, cwd=str(TRAZOR_DIR), shell=(os.name == 'nt'))
        if r.returncode or not out_svg.exists():
            raise RuntimeError(f'rc={r.returncode} {r.stderr[-2000:]}')
    else:
        raise ValueError(engine)


def vtracer10_version():
    r = subprocess.run([str(VTRACER10), '--version'], capture_output=True, text=True)
    return r.stdout.strip() or r.stderr.strip()


def trazor_commit():
    return subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=str(TRAZOR_DIR), capture_output=True, text=True).stdout.strip()


def main():
    import argparse, importlib.metadata
    ap = argparse.ArgumentParser(); ap.add_argument('--limit', type=int, default=0); args = ap.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    backbone = 'dinov3' if mdino.available('dinov3') else 'dinov2'
    manifest = {
        'seed': cc.SEED, 'created': time.strftime('%Y-%m-%dT%H:%M:%S%z'),
        'git_head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        'binary_sha256': {'inkvec': cc.sha(EXE), 'vtracer-1.0': cc.sha(VTRACER10)},
        'vtracer_default_version': importlib.metadata.version('vtracer'),
        'vtracer_1_0_version_string': vtracer10_version(),
        'vtracer_1_0_source': 'https://github.com/visioncortex/vtracer/releases/tag/1.0.0-alpha.4 -- '
                              'official Windows CLI zip (vtracer-x86_64-pc-windows-msvc.zip), no published '
                              'checksum file for this specific asset (only the desktop-app installer/AppImage '
                              'assets ship .sha256/.sig); sha256 of the binary we ran is in binary_sha256 above. '
                              'GitHub API reports this release prerelease=false despite the 1.0.0-alpha.4 semver tag.',
        'vtracer_1_0_new_flags': '--simplify 1.5 (curve simplification, not default) and '
                                 '--hierarchical cutout (seam-free mosaic tessellation, default is stacked)',
        'trazor_source': 'https://github.com/PhenX/Trazor (MIT), commit ' + trazor_commit() +
                         ', run locally via tools/vendor/trazor/trace-cli.ts (Node/tsx, our wrapper) '
                         'through @trazor/engine vectorize(), auto-recommended settings.',
        'rastertrace': 'github.com/bradsec/rastertrace + rastertrace.com; NOT benchmarked separately '
                       '-- browser-only WASM app, its own site states it wraps "vtracer 1.0 alpha"; '
                       'see vtracer-1.0-* rows for that engine measured directly. No image uploaded, no '
                       'browser automation used, per task rules.',
        'dino_backbone': backbone, 'engines': ENGINES,
        'protocol': 'Same 21 cases and protocol as crosscompare_current.py / crosscompare_4way.py (same '
                    'select()/structure()): original SVG normalized to square viewBox with 4% margin, RGB on '
                    'white, resvg raster at 512, scored at 1024 against the original render. Clean input only. '
                    'Timings measured on a machine with other CPU-bound work running concurrently '
                    '(a Rust build and CPU inference); treat all seconds as approximate, not definitive.',
    }
    (OUT / 'manifest.json').write_text(json.dumps(manifest, indent=2))
    sources = cc.select(); sources = sources[:args.limit] if args.limit else sources
    rows, errors = [], []
    for i, (family, name, src) in enumerate(sources):
        import re
        key = re.sub(r'[^a-zA-Z0-9_.-]', '_', family + '__' + name)
        case = OUT / 'cases' / key; case.mkdir(parents=True, exist_ok=True)
        try:
            original = src.read_text(encoding='utf-8-sig'); (case / 'original.svg').write_text(original, encoding='utf-8')
            truth = render.fit_viewbox(render.normalize_svg(original)[0], 512, 512, .04)
            ref = cc.rgb(truth, 1024); gt = cc.structure(original); cc.png(ref, case / 'truth.png')
            if float(np.mean(1 - ref)) < 1e-5:
                raise ValueError('Blank source on white')
        except Exception as e:
            errors.append(dict(family=family, name=name, error=repr(e))); continue
        d = case / '512'; d.mkdir(exist_ok=True)
        inp = cc.rgb(truth, 512); cc.png(inp, d / 'input.png')
        base = dict(family=family, name=name, key=key, source=str(src), source_sha256=cc.sha(src), gt=gt)
        for engine in ENGINES:
            row = {**base, 'engine': engine}; out = d / f'{engine}.svg'
            try:
                t = time.perf_counter()
                trace_one(engine, d / 'input.png', out)
                row['seconds'] = time.perf_counter() - t
                svg = out.read_text(encoding='utf-8'); row.update(cc.structure(svg))
                pred = cc.rgb(svg, 1024)
                de = deltaE_ciede2000(rgb2lab(ref), rgb2lab(pred))
                row.update(de1024=float(de.mean()),
                           ssim1024=float(structural_similarity(ref, pred, data_range=1., channel_axis=2)),
                           dists=float(mraster.dists_distance(ref, pred)),
                           dino=float(mdino.dino_score(ref, pred, backbone=backbone)),
                           geometry_ratio=row['geometry_params'] / max(gt['geometry_params'], 1))
                cc.png(pred, d / f'{engine}.png')
            except Exception as e:
                row['error'] = repr(e)
            rows.append(row)
            (OUT / 'results.json').write_text(json.dumps(rows, indent=2))
            print(f"{i+1}/{len(sources)} {family}/{name} {engine}: " +
                  (row.get('error') or f"dE={row['de1024']:.3f} coords={row['coordinates']} paths={row['paths']} {row['seconds']:.2f}s"),
                  flush=True)
    (OUT / 'source-errors.json').write_text(json.dumps(errors, indent=2))
    print('DONE', len(rows), 'rows', len(errors), 'source errors', flush=True)


if __name__ == '__main__':
    main()
