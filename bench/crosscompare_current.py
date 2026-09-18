"""Reproducible current-working-tree SVG round-trip audit. No tracer changes."""
from __future__ import annotations
import argparse, base64, csv, gzip, hashlib, importlib.metadata, io, json, os, re, subprocess, sys, time
from collections import Counter
from pathlib import Path
import xml.etree.ElementTree as ET
import numpy as np
from PIL import Image
from skimage.color import rgb2lab, deltaE_ciede2000
from skimage.metrics import structural_similarity
import svgelements as se

ROOT = Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT), str(ROOT / 'bench')]
from inkvec_bench import render

OUT = ROOT / 'out/crosscompare-2026-09-15'
EXE = ROOT / 'target-crosscompare/release/inkvec.exe'
GUIDE = ROOT / 'target-crosscompare/release/examples/oracle_guidance.exe'
SEED = 'crosscompare-2026-09-15-v1'
DETAIL = dict(colormode='color', mode='spline', hierarchical='stacked', path_precision=3,
              corner_threshold=60, length_threshold=4., splice_threshold=45,
              color_precision=8, layer_difference=8, filter_speckle=2)
ENGINES = ['inkvec', 'ai', 'vtracer-default', 'vtracer-detail']
NUM = re.compile(r'[-+]?(?:\d*\.\d+|\d+\.?\d*)(?:[eE][-+]?\d+)?')

def sha(p):
    h=hashlib.sha256()
    with open(p,'rb') as f:
        for b in iter(lambda:f.read(1024*1024), b''): h.update(b)
    return h.hexdigest()

def structure(svg):
    root=ET.fromstring(svg); tags=Counter(); segs=Counter(); coords=params=path_numbers=0
    # Stored geometry, including definitions once; references and transforms separate.
    for el in root.iter():
        tag=el.tag.split('}')[-1]; tags[tag]+=1
        if tag=='path':
            d=el.get('d',''); path_numbers+=len(NUM.findall(d))
            for s in se.Path(d):
                k=type(s).__name__; segs[k]+=1
                coords += {'Move':2,'Line':2,'CubicBezier':6,'QuadraticBezier':4,'Arc':2}.get(k,0)
                params += {'Move':2,'Line':2,'CubicBezier':6,'QuadraticBezier':4,'Arc':7}.get(k,0)
        elif tag in ('polygon','polyline'):
            n=len(NUM.findall(el.get('points',''))); coords+=n; params+=n
        elif tag in ('circle','ellipse','rect','line'):
            coords += {'circle':2,'ellipse':2,'rect':2,'line':4}[tag]
            params += {'circle':3,'ellipse':4,'rect':6,'line':4}[tag]
    return dict(paths=tags['path'],subpaths=segs['Move'],coordinates=coords,geometry_params=params,
        path_numeric_args=path_numbers,primitives=sum(tags[k] for k in ('circle','ellipse','rect','line','polygon','polyline')),
        gradients=tags['linearGradient']+tags['radialGradient'],uses=tags['use'],
        transforms=sum(len(NUM.findall(e.get('transform',''))) for e in root.iter()),
        cubics=segs['CubicBezier'],lines=segs['Line'],arcs=segs['Arc'],
        bytes=len(svg.encode()),gzip_bytes=len(gzip.compress(svg.encode(),mtime=0)))

def rgb(svg,n):
    return render.composite(render.render(svg,n,n))

def png(a,p):
    Image.fromarray(np.clip(np.rint(a*255),0,255).astype('uint8')).save(p)

def select():
    sources=[]
    for family in ['lucide','material-icons','simple-icons','twemoji','noto-emoji','openmoji','fluent-emoji']:
        files=list((ROOT/'bench/data/corpus_svg'/family).glob('*.svg'))
        files.sort(key=lambda p:hashlib.sha256((SEED+family+p.name).encode()).hexdigest())
        sources.extend((family,p.stem,p) for p in files[:2])
    for name in ['prim_circle','mosaic_pie6','gradient_linear','gradient_radial']:
        sources.append(('synthetic',name,ROOT/'bench/data/corpus_svg/synthetic'/f'{name}.svg'))
    brands=Path('M:/AI STORAGE/AITrains/BrandsDataset/dataset/brands')
    if brands.exists():
        files=[p for p in brands.glob('*/logo_1.svg') if 800 < p.stat().st_size < 120000]
        files.sort(key=lambda p:hashlib.sha256((SEED+p.parent.name).encode()).hexdigest())
        sources.extend(('brands',p.parent.name,p) for p in files[:3])
    else:
        print(
            "\n" + "!"*78 + "\n"
            f"!! EXTERNAL DATASET MISSING: {brands}\n"
            "!! This brand dataset is an EXTERNAL dependency, not part of this repository.\n"
            "!! Without it this run selects 18 cases, NOT the 21 cases in the published\n"
            "!! tables (README.md, docs/results/2026-09-15.md). The 3 missing cases are real\n"
            "!! brand logos that cannot be redistributed with the repo. Re-run with the\n"
            "!! dataset in place to reproduce the published 21-case numbers.\n"
            + "!"*78 + "\n",
            file=sys.stderr, flush=True,
        )
    return sources

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--limit',type=int,default=0); args=ap.parse_args()
    OUT.mkdir(parents=True,exist_ok=True)
    import vtracer, ai_trace, torch
    ai_trace.RUNNER_EXE=GUIDE
    start=time.perf_counter(); tracer=ai_trace.AITracer(); load_seconds=time.perf_counter()-start
    # Warm model separately; timings exclude one-time model load and warmup.
    tracer.forward_image(Image.new('RGB',(256,256),'white'))
    manifest={'seed':SEED,'created':time.strftime('%Y-%m-%dT%H:%M:%S%z'),
        'git_head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
        'git_status':subprocess.check_output(['git','status','--short'],cwd=ROOT,text=True),
        'source_sha256':{str(p.relative_to(ROOT)):sha(p) for p in [ROOT/'ai_trace.py',*ROOT.glob('crates/**/*.rs'),*ROOT.glob('crates/**/Cargo.toml')]},
        'binary_sha256':{'inkvec':sha(EXE),'guidance':sha(GUIDE)},'checkpoint':str(tracer.ckpt_path),
        'checkpoint_sha256':sha(tracer.ckpt_path),'vtracer_version':importlib.metadata.version('vtracer'),
        'versions':{m:importlib.metadata.version(m) for m in ['numpy','Pillow','resvg-py','scikit-image','torch','svgelements']},
        'gpu':torch.cuda.get_device_name() if torch.cuda.is_available() else None,
        'ai_model_load_seconds':load_seconds,'vtracer_detail_config':DETAIL,'vtracer_default_config':'No keyword overrides; installed binding defaults',
        'inkvec_config':'CLI defaults, --quiet; no SR or strokes flag', 'ai_config':'AITracer.trace defaults, subpixel_deltas=True',
        'protocol':'Original SVG normalized to square viewBox with 4% margin, RGB on white, direct resvg rasterization at 256 and 512. Same PNG bytes to each engine. Score against direct original SVG render at input resolution and 1024, with no alignment adjustment. Sequential timings, one trace each, exclude scoring and model loading. Clean input only.',
        'selection':'2 hash-selected files per icon family, 3 hash-selected brand logos (800..120000 bytes), 4 named synthetic probes. Selection fixed before tracing. No training-disjoint claim.'}
    (OUT/'manifest.json').write_text(json.dumps(manifest,indent=2))
    sources=select(); sources=sources[:args.limit] if args.limit else sources
    rows=[]; errors=[]
    for i,(family,name,src) in enumerate(sources):
      key=re.sub(r'[^a-zA-Z0-9_.-]','_',family+'__'+name)
      case=OUT/'cases'/key; case.mkdir(parents=True,exist_ok=True)
      try:
        original=src.read_text(encoding='utf-8-sig'); (case/'original.svg').write_text(original,encoding='utf-8')
        truth=render.fit_viewbox(render.normalize_svg(original)[0],512,512,.04)
        ref=rgb(truth,1024); gt=structure(original); png(ref,case/'truth.png')
        if float(np.mean(1-ref))<1e-5: raise ValueError('Blank source on white')
      except Exception as e:
        errors.append(dict(family=family,name=name,error=repr(e))); continue
      for size in [256,512]:
        d=case/str(size); d.mkdir(exist_ok=True); inp=rgb(truth,size); png(inp,d/'input.png')
        base=dict(family=family,name=name,key=key,size=size,source=str(src),source_sha256=sha(src),input_sha256=sha(d/'input.png'),gt=gt)
        for engine in ENGINES:
          row={**base,'engine':engine}; out=d/f'{engine}.svg'
          try:
            t=time.perf_counter()
            if engine=='inkvec':
                proc=subprocess.run([str(EXE),str(d/'input.png'),'-o',str(out),'--quiet'],capture_output=True,timeout=180)
                if proc.returncode: raise RuntimeError(proc.stderr.decode(errors='replace'))
            elif engine=='ai': tracer.trace(d/'input.png',out)
            else: out.write_text(vtracer.convert_raw_image_to_svg((d/'input.png').read_bytes(),img_format='png',**(DETAIL if engine.endswith('detail') else {})),encoding='utf-8')
            row['seconds']=time.perf_counter()-t
            svg=out.read_text(encoding='utf-8'); row.update(structure(svg))
            pred=rgb(svg,1024); low=rgb(svg,size)
            de=deltaE_ciede2000(rgb2lab(ref),rgb2lab(pred)); foreground=np.min(ref,axis=2)<.98
            row.update(de1024=float(de.mean()),de1024_foreground=float(de[foreground].mean()),
                de1x=float(deltaE_ciede2000(rgb2lab(inp),rgb2lab(low)).mean()),
                ssim1024=float(structural_similarity(ref,pred,data_range=1.,channel_axis=2)),
                rmse1024=float(np.sqrt(np.mean((ref-pred)**2))),
                geometry_ratio=row['geometry_params']/max(gt['geometry_params'],1))
            png(pred,d/f'{engine}.png'); png(np.clip(abs(pred-ref)*6,0,1),d/f'{engine}-diff.png')
          except Exception as e: row['error']=repr(e)
          rows.append(row)
          (OUT/'results.json').write_text(json.dumps(rows,indent=2))
          print(f"{i+1}/{len(sources)} {family}/{name} {size} {engine}: "+(row.get('error') or f"dE={row['de1024']:.3f} coords={row['coordinates']} paths={row['paths']} {row['seconds']:.2f}s"),flush=True)
    (OUT/'source-errors.json').write_text(json.dumps(errors,indent=2))
    flat=[{k:v for k,v in r.items() if k!='gt'}|{'gt_'+k:v for k,v in r.get('gt',{}).items()} for r in rows]
    with (OUT/'results.csv').open('w',newline='',encoding='utf-8') as f:
        w=csv.DictWriter(f,fieldnames=sorted(set().union(*(r.keys() for r in flat))));w.writeheader();w.writerows(flat)
    print('DONE',len(rows),'rows',len(errors),'source errors',flush=True)

if __name__=='__main__': main()
