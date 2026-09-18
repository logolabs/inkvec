"""4-way raster round-trip audit: Inkvec, VTracer, Covecto, potrace-color (color_trace).

Same 21 hash-selected cases, same protocol and scoring as crosscompare_current.py (imports its
select()/structure()/rgb()/png()/sha() so the case set and metric definitions cannot drift), plus
DISTS and DINOv3 (not in the original 2-way audit). Two engines added:

  covecto       codecoradev/covecto v0.1.1, MIT, engine=auto (its own auto-selection between a
                pixel-exact and a spline mode). Official release zip, sha256-verified against
                the release's own checksums file. tools/vendor/covecto/covecto.exe
  color-trace   migvel/color_trace ("potrace-color", GPL-2.0-or-later): quantizes with pngquant
                then traces each colour layer with potrace and stacks them. -c 16 colours, -s
                (stack) as the README recommends for accuracy. Patched locally for this machine
                (see NOTE comments in tools/vendor/color_trace/color_trace_multi.py): Windows
                multiprocessing spawn cannot pickle the raw Manager the upstream script passed to
                worker processes (fixed by passing an explicit settings dict instead), upstream
                built but never raised the subprocess-failure exception (silent corruption on any
                tool failure -- fixed), and pngquant 3.x rejects the single-dash -nofs/-force
                spelling this script used (fixed to --nofs/--force). Behaviour on a passing
                pngquant call is otherwise unchanged from upstream. potrace 1.16 official Windows
                binary, ImageMagick 7 (installed via choco) and pngquant 3.0.3 (choco) underneath.

    python bench/crosscompare_4way.py [--limit N]
"""
from __future__ import annotations
import base64, hashlib, importlib.metadata, io, json, os, subprocess, sys, time
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

OUT = ROOT / 'out/crosscompare-4way-2026-09-15'
EXE = ROOT / 'target/release/inkvec.exe'
COVECTO = ROOT / 'tools/vendor/covecto/covecto.exe'
COLOR_TRACE = ROOT / 'tools/vendor/color_trace/color_trace_multi.py'
ENGINES = ['inkvec', 'vtracer-default', 'covecto-auto', 'covecto-spline', 'color-trace']


def trace_one(engine, png_path, out_svg):
    if engine == 'inkvec':
        r = subprocess.run([str(EXE), str(png_path), '-o', str(out_svg), '--quiet'], capture_output=True, timeout=180)
        if r.returncode:
            raise RuntimeError(r.stderr.decode(errors='replace'))
    elif engine == 'vtracer-default':
        import vtracer
        out_svg.write_text(vtracer.convert_raw_image_to_svg(png_path.read_bytes(), img_format='png'), encoding='utf-8')
    elif engine in ('covecto-auto', 'covecto-spline'):
        eng = 'auto' if engine == 'covecto-auto' else 'spline'
        r = subprocess.run([str(COVECTO), 'vectorize', str(png_path), '-o', str(out_svg), '--engine', eng, '--no-progress'],
                           capture_output=True, timeout=180, text=True)
        if r.returncode or not out_svg.exists():
            raise RuntimeError(f'rc={r.returncode} {r.stderr[:2000]}')
    elif engine == 'color-trace':
        r = subprocess.run([sys.executable, str(COLOR_TRACE), '-i', str(png_path), '-o', str(out_svg), '-c', '16', '-s'],
                           capture_output=True, timeout=180, text=True)
        if r.returncode or not out_svg.exists():
            raise RuntimeError(f'rc={r.returncode} {r.stderr[-2000:]}')
    else:
        raise ValueError(engine)


def main():
    import argparse
    ap = argparse.ArgumentParser(); ap.add_argument('--limit', type=int, default=0); args = ap.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    backbone = 'dinov3' if mdino.available('dinov3') else 'dinov2'
    manifest = {
        'seed': cc.SEED, 'created': time.strftime('%Y-%m-%dT%H:%M:%S%z'),
        'git_head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        'binary_sha256': {'inkvec': cc.sha(EXE), 'covecto': cc.sha(COVECTO)},
        'vtracer_version': importlib.metadata.version('vtracer'),
        'covecto_version': subprocess.run([str(COVECTO), '--version'], capture_output=True, text=True).stdout.strip(),
        'color_trace_source': 'https://github.com/migvel/color_trace (GPL-2.0-or-later), patched for '
                              'Windows multiprocessing + pngquant 3.x flags -- see NOTE comments in the vendored file',
        'potrace_version': subprocess.run([str(ROOT / 'tools/vendor/potrace/potrace.exe'), '--version'],
                                          capture_output=True, text=True).stdout.splitlines()[0],
        'pngquant_version': subprocess.run(['pngquant', '--version'], capture_output=True, text=True).stdout.strip(),
        'dino_backbone': backbone, 'engines': ENGINES,
        'protocol': 'Same 21 cases and protocol as crosscompare_current.py (same select()/structure()): '
                    'original SVG normalized to square viewBox with 4% margin, RGB on white, resvg raster at 512, '
                    'scored at 1024 against the original render. Covecto: engine=auto, defaults otherwise. '
                    'color-trace ("potrace-color"): -c 16 -s (stack), defaults otherwise. Clean input only.',
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
