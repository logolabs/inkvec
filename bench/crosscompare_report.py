"""Build a portable, offline review page from the measured round-trip audit."""
from pathlib import Path
import json, statistics, html
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT/'out/crosscompare-2026-09-15'
ENGINES=['inkvec','ai','vtracer-default','vtracer-detail']
LABELS={'inkvec':'Inkvec core','ai':'Neural AITracer','vtracer-default':'VTracer defaults','vtracer-detail':'VTracer detailed'}

def summarize(rows):
    out=[]
    for size in sorted({r['size'] for r in rows}):
      for family in ['All families']+sorted({r['family'] for r in rows}):
       for engine in ENGINES:
        rs=[r for r in rows if r['size']==size and r['engine']==engine and (family=='All families' or r['family']==family)]
        ok=[r for r in rs if not r.get('error')]
        d={'size':size,'family':family,'engine':engine,'n':len(rs),'failed':len(rs)-len(ok)}
        for k in ['de1024','de1x','de1024_foreground','ssim1024','paths','subpaths','coordinates','geometry_params','geometry_ratio','bytes','seconds']:
            d[k]=statistics.mean(r[k] for r in ok) if ok else None
        d['median_seconds']=statistics.median(r['seconds'] for r in ok) if ok else None
        out.append(d)
    return out

def main():
    rows=json.loads((OUT/'results.json').read_text()); summary=summarize(rows)
    import sys
    sys.path.insert(0,str(ROOT/'bench'))
    from inkvec_bench import render
    for case in (OUT/'cases').iterdir():
        original=case/'original.svg'
        if original.exists():
            normalized=render.fit_viewbox(render.normalize_svg(original.read_text(encoding='utf-8'))[0],512,512,.04)
            (case/'truth.svg').write_text(normalized,encoding='utf-8')
    (OUT/'summary.json').write_text(json.dumps(summary,indent=2))
    m=json.loads((OUT/'manifest.json').read_text())
    data=json.dumps({'rows':rows,'summary':summary,'labels':LABELS}).replace('</','<\\/')
    page=TEMPLATE.replace('__DATA__',data).replace('__VERSION__',m['vtracer_version'])
    (OUT/'index.html').write_text(page,encoding='utf-8')
    text=['# Current Inkvec / AITracer / VTracer round-trip comparison','',m['protocol'],'',
          'VTracer version: '+m['vtracer_version']+'. Two fixed settings, not a full Pareto sweep. AI model loading: '+f"{m['ai_model_load_seconds']:.2f} s"+'.',
          '', 'Selection: '+m['selection'],'',
          '| Input px | Engine | n / failures | Mean ΔE00 at 1024 ↓ | Mean coords ↓ | Mean paths | Mean KB | Median seconds ↓ |',
          '|---|---|---|---|---|---|---|---|']
    for s in summary:
        if s['family']=='All families':text.append(f"| {s['size']} | {LABELS[s['engine']]} | {s['n']} / {s['failed']} | {s['de1024']:.4f} | {s['coordinates']:.1f} | {s['paths']:.1f} | {s['bytes']/1024:.2f} | {s['median_seconds']:.3f} |")
    paired=[]
    for size in [256,512]:
        groups={k:{r['engine']:r for r in rows if r['key']==k and r['size']==size and not r.get('error')} for k in {r['key'] for r in rows}}
        groups=[v for v in groups.values() if all(e in v for e in ['inkvec','vtracer-default','vtracer-detail'])]
        ai_groups=[v for v in groups if 'ai' in v]
        paired.append({'size':size,'n':len(groups),'ai_matched_n':len(ai_groups),
            'core_fidelity_wins_vs_both_vtracer':sum(v['inkvec']['de1024']<min(v[e]['de1024'] for e in ENGINES[2:]) for v in groups),
            'core_coordinate_wins_vs_both_vtracer':sum(v['inkvec']['coordinates']<min(v[e]['coordinates'] for e in ENGINES[2:]) for v in groups),
            'ai_fidelity_wins_vs_core':sum(v['ai']['de1024']<v['inkvec']['de1024'] for v in ai_groups),
            'ai_fidelity_wins_vs_both_vtracer':sum(v['ai']['de1024']<min(v[e]['de1024'] for e in ENGINES[2:]) for v in ai_groups)})
    (OUT/'paired.json').write_text(json.dumps(paired,indent=2))
    text+=['','## Paired counts','']
    for p in paired:
        text.append(f"At {p['size']}px ({p['n']} matched core/VTracer cases), core has lower ΔE than both VTracer settings in {p['core_fidelity_wins_vs_both_vtracer']} cases and fewer coordinate scalars than both in {p['core_coordinate_wins_vs_both_vtracer']}. Among {p['ai_matched_n']} successful AI comparisons, AI has lower ΔE than core in {p['ai_fidelity_wins_vs_core']} cases, and lower ΔE than both VTracer settings in {p['ai_fidelity_wins_vs_both_vtracer']} cases. Strict comparisons; no significance claim.")
    text+=['','## Dataset breakdown at 512px','', '| Dataset | Inkvec ΔE / coords | AITracer ΔE / coords | VTracer default ΔE / coords | VTracer detailed ΔE / coords |','|---|---|---|---|---|']
    for family in sorted({r['family'] for r in rows}):
        ss=[next(s for s in summary if s['size']==512 and s['family']==family and s['engine']==e) for e in ENGINES]
        text.append('| '+family+' | '+' | '.join(f"{s['de1024']:.3f} / {s['coordinates']:.0f}" if s['de1024'] is not None else 'unavailable' for s in ss)+' |')
    text+=['','## Interpretation limits','',
      'This is a small descriptive audit of locally available data, not a statistically representative or training-disjoint benchmark. Synthetic probes deliberately test primitives and gradients. Results are separated by family and resolution in the review page. No claims of research novelty follow from a comparison with one baseline.',
      'These are clean raster round trips on white. They do not establish performance on JPEG, blur, photographs, transparency retention, or arbitrary SVG effects. Lower complexity is only a benefit when visual fidelity is retained. The neural pipeline is measured as currently implemented, with its native preprocessing and decoded guidance; no oracle SVG geometry enters either tracer.',
      'Timings are one sequential run per case; neural weights are loaded once and a blank 256px forward pass is warmed up. They include tracing and file I/O, exclude scoring, and are indicative rather than controlled microbenchmarks.',
      'The AITracer 256px synthetic circle run failed with exit 101. A separate diagnostic retry reproduced a Rust panic in crates/inkvec-trace/src/symmetry.rs:117: an index of 2 into a vector of length 2. The original failed row is retained, and stderr is preserved in failure-diagnostic.txt. No tracing code was changed during this audit.',
      '', '## Metric definitions','',
      'Paths count stored <path> elements; subpaths count parsed moveto segments, including implicit path syntax. Coordinates count x/y scalars after expanding path commands: Move/Line/Arc endpoints = 2, quadratic = 4, cubic = 6. They exclude arc radii/angles/flags. Circle/ellipse centers = 2, rectangle origins = 2, lines = 4; polygon/polyline point scalars are counted. A coordinate pair contributes two scalars. Close contributes zero.',
      'Geometry parameters add shape sizes and arc radii/angle/flags: circle = 3, ellipse = 4, rectangle = 6, arc = 7. These are canonical geometry counts, not exact serialized numeric-token counts. Geometry stored in definitions is counted once; use references and transforms are listed separately. Fill/gradient parameters, stroke widths, transforms, and viewBox are excluded. The separate path numeric-argument count records literal numbers in d attributes. These conventions are not equivalent to every published parameter metric.',
      'A single compound path can contain many disconnected subpaths; fewer paths alone is not proof of easier editing. Artist SVGs may use strokes and reusable primitives whereas traces may use filled outlines. The comparison retains those representational differences.',
      'ΔE00 = mean CIEDE2000 against direct rendering of the original; lower is better. SSIM uses scikit-image defaults with data_range=1 and channel_axis=2; higher is better. Foreground ΔE excludes reference pixels whose RGB channels are all >=0.98, reducing white-background dilution. Difference views show absolute RGB error amplified 6× on black, not ΔE. All artwork panels are rendered at 1024px except the input raster.',
      '', '## Reproduction','',
      'Build: cargo build --release -p inkvec-cli --bin inkvec --example oracle_guidance --features research-guidance --target-dir target-crosscompare',
      'Run: python bench/crosscompare_current.py', 'Report: python bench/crosscompare_report.py',
      'Exact source/checkpoint/binary hashes and settings are in manifest.json; per-output data are in results.json and results.csv. Each case includes the source SVG, actual input PNGs, output SVGs, 1024px renders and amplified error images.',
      'VTracer upstream documentation: https://github.com/visioncortex/vtracer . This audit uses the installed version named above, not an unversioned upstream development build.']
    (OUT/'REPORT.md').write_text('\n'.join(text),encoding='utf-8')
    print(json.dumps([s for s in summary if s['family']=='All families'],indent=2))

TEMPLATE=r'''<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Inkvec · Round-trip audit</title>
<style>
:root{color-scheme:light dark;--bg:#f7f8fa;--fg:#18202b;--muted:#526174;--line:#cbd3de;--surface:#fff;--accent:#2457d6} @media(prefers-color-scheme:dark){:root{--bg:#11151b;--fg:#e5eaf0;--muted:#a7b4c6;--line:#39434f;--surface:#1a2029;--accent:#99b7ff}}
*{box-sizing:border-box}body{margin:0;padding:28px;font:15px/1.5 system-ui,sans-serif;background:var(--bg);color:var(--fg)}main{max-width:1720px;margin:auto}h1{font-size:30px;font-weight:600;margin:0 0 6px}h2{font-size:21px;font-weight:600;margin:26px 0 10px}p{max-width:1050px;color:var(--muted)}a{color:var(--accent)}.controls{display:flex;gap:20px;align-items:end;flex-wrap:wrap;margin:20px 0}label{display:grid;gap:5px}select,button,input{font:inherit;color:var(--fg);background:var(--surface);border:1px solid var(--line);border-radius:4px;padding:7px}input[type=range]{padding:0;width:150px}.panes{display:grid;grid-template-columns:repeat(5,minmax(0,1fr));gap:14px}figure{margin:0;min-width:0}figcaption{margin:7px 0;font-weight:600}.view{background:white;aspect-ratio:1;overflow:hidden;border:1px solid var(--line);touch-action:none;cursor:grab}.view img{width:100%;height:100%;object-fit:contain;transform-origin:center;user-select:none;pointer-events:none}.meta{font-size:13px;font-variant-numeric:tabular-nums;margin-top:8px}.meta b{font-weight:600}.scroll{overflow:auto}table{border-collapse:collapse;width:100%;font-size:13px;font-variant-numeric:tabular-nums}th,td{padding:9px 12px;border-bottom:1px solid var(--line);text-align:right;white-space:nowrap}th:first-child,td:first-child{text-align:left}th{font-weight:600}.good{font-weight:700;color:var(--accent)}.small{font-size:13px;color:var(--muted)}details{margin:25px 0}summary{cursor:pointer}.err{color:#ce5656;white-space:normal;overflow-wrap:anywhere}.chart{width:100%;height:290px}.chart text{fill:var(--fg);font-size:12px}.chart line{stroke:var(--line)}@media(max-width:1000px){.panes{grid-template-columns:repeat(3,minmax(0,1fr))}}@media(max-width:650px){body{padding:16px}.panes{grid-template-columns:repeat(2,minmax(0,1fr))}}
label{max-width:100%;min-width:0}select{max-width:100%}#case{max-width:min(420px,88vw)}
</style></head><body><main>
<h1>What did the tracer contribute?</h1><p>Original SVG → identical raster → fresh Inkvec core, neural AITracer and VTracer __VERSION__. Inspect all selected cases, including failures. Measurements compare each reconstruction to the artist’s original at 1024 px.</p>
<div class="controls"><label>Dataset<select id="family"></select></label><label>Case<select id="case"></select></label><label>Raster size<select id="size"><option>256</option><option>512</option></select></label><label>VTracer setting<select id="vt"><option value="vtracer-default">Installed defaults</option><option value="vtracer-detail">Detailed baseline</option></select></label><button id="prev">Previous</button><button id="next">Next</button></div>
<div class="controls"><label>View<select id="view"><option value="fill">Measured 1024px renders</option><option value="vector">Vector SVGs (browser render)</option><option value="diff">Absolute error ×6</option></select></label><label>Shared zoom <input id="zoom" type="range" min="1" max="8" step="0.1" value="1"></label><button id="reset">Reset view</button><span class="small">Drag any image to pan all five. Use Vector SVGs for resolution-independent zoom.</span></div>
<div class="panes" id="panes"></div>
<h2 id="case-title">Per-case geometry and fidelity</h2><div class="scroll"><table id="detail"></table></div>
<p class="small">Coords = canonical x/y scalars, including path starts and control points; a pair counts as two. Paths may contain many subpaths. Primitives and gradients are listed separately. Lower counts are useful only when fidelity survives.</p>
<h2>Dataset comparison</h2><p class="small">Arithmetic means for the selected dataset and raster size; runtime is the median. “All families” weights each selected case equally. Two samples per icon family, three brands and four synthetic probes: these are descriptive results, not a population estimate.</p><div class="scroll"><table id="aggregate"></table></div>
<h2>Across all datasets</h2><p class="small">Each cell: mean ΔE00 at 1024px / mean coordinate scalars. Uses the selected raster size.</p><div class="scroll"><table id="families"></table></div>
<details><summary>Protocol, limitations and data</summary><p>Clean RGB rasters on white, 4% viewBox margin, direct resvg rasterization. The original is rendered independently at 1024px: 4× input size for 256px and 2× for 512px. No output alignment correction. Same input PNG SHA256 for all engines. Neural settings and core defaults are unmodified; no artist geometry is provided to them.</p><p>VTracer defaults and the project's detailed configuration are both measured. This is not a full settings sweep. The neural model may have seen these datasets during training. This run does not test JPEG, blur, retained alpha, or establish algorithmic novelty. Timings are sequential single measurements with model loading excluded; full setup time is recorded in the manifest.</p><p>Foreground ΔE excludes near-white reference pixels. SSIM and whole-image ΔE include the background. A simpler result with missing objects is a failure of fidelity, not a complexity win. Stored definitions count once and use references/transform values are separate. Geometry parameters exclude colour, gradient, stroke and transform parameters.</p><p><a href="REPORT.md">Full report and counting definitions</a> · <a href="results.csv">CSV</a> · <a href="results.json">Raw results</a> · <a href="manifest.json">Versions, source hashes and configuration</a> · <a href="source-errors.json">Source errors</a></p></details>
<p id="status" class="small" aria-live="polite"></p>
</main><script>
const DATA=__DATA__;
const $=id=>document.getElementById(id), rows=DATA.rows, labels=DATA.labels;
const esc=s=>String(s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const fmt=(x,n=0)=>x==null?'—':Number(x).toLocaleString('en',{maximumFractionDigits:n,minimumFractionDigits:n});
let pan={x:0,y:0},drag=null;
const families=['All families',...new Set(rows.map(r=>r.family))];
$('family').innerHTML=families.map(f=>`<option>${esc(f)}</option>`).join('');
function cases(){const f=$('family').value;return [...new Map(rows.filter(r=>f==='All families'||r.family===f).map(r=>[r.key,r])).values()]}
function choose(){const cs=cases();$('case').innerHTML=cs.map(r=>`<option value="${r.key}">${esc(r.family+' / '+r.name)}</option>`).join('');draw()}
function transform(){document.querySelectorAll('.view img').forEach(im=>im.style.transform=`translate(${pan.x*100}%,${pan.y*100}%) scale(${$('zoom').value})`)}
function draw(){const key=$('case').value,size=+$('size').value,rs=rows.filter(r=>r.key===key&&r.size===size);if(!rs.length)return;const r=rs[0],base=`cases/${key}/${size}/`,g=r.gt,mode=$('view').value;
const panes=[['Artist SVG',`cases/${key}/truth.${mode==='vector'?'svg':'png'}`,g,`cases/${key}/original.svg`],['Input raster',base+'input.png',null,base+'input.png'],...['inkvec','ai',$('vt').value].map(e=>{const a=rs.find(x=>x.engine===e);return[labels[e],base+e+(mode==='diff'?'-diff':'')+(mode==='vector'?'.svg':'.png'),a,base+e+'.svg']})];
$('panes').innerHTML=panes.map(([name,img,s,link])=>`<figure><figcaption>${name}</figcaption><div class="view">${s?.error?'<p class="err">Trace failed</p>':`<img src="${img}" alt="${esc(name+' '+r.name)}" draggable="false">`}</div><div class="meta">${s&&!s.error?`<b>${fmt(s.paths)}</b> paths · <b>${fmt(s.coordinates)}</b> coords<br>${fmt(s.primitives)} primitives · ${fmt(s.gradients)} gradients${s.de1024!==undefined?`<br>ΔE <b>${fmt(s.de1024,3)}</b> · ${fmt(s.seconds,3)} s`:''}`:(s?.error?'Trace failed':`${size} × ${size} pixels`)}<br><a href="${s?.error?'failure-diagnostic.txt':link}" target="_blank">${s?.error?'Failure details':name==='Input raster'?'Open PNG':'Open SVG'}</a></div></figure>`).join('');
const headers=['Engine','Paths','Subpaths','Coords','Geom. params','Primitives','Gradients','Cubic / line / arc','ΔE 1× ↓','ΔE 1024 ↓','Foreground ΔE ↓','SSIM ↑','KiB','Seconds ↓'];
const line=(name,s)=>`<tr><td>${esc(name)}</td>${s.error?`<td colspan="13" class="err">${esc(s.error)}</td>`:[s.paths,s.subpaths,s.coordinates,s.geometry_params,s.primitives,s.gradients,`${s.cubics} / ${s.lines} / ${s.arcs}`,fmt(s.de1x,3),fmt(s.de1024,3),fmt(s.de1024_foreground,3),fmt(s.ssim1024,4),fmt(s.bytes/1024,2),fmt(s.seconds,3)].map(v=>`<td>${v}</td>`).join('')}</tr>`;
$('detail').innerHTML='<thead><tr>'+headers.map(x=>`<th>${x}</th>`).join('')+'</tr></thead><tbody>'+line('Artist source',g)+rs.map(s=>line(labels[s.engine],s)).join('')+'</tbody>';
$('case-title').textContent=r.family+' / '+r.name+' · '+size+'px input';
const ag=DATA.summary.filter(s=>s.size===size&&s.family===$('family').value);
$('aggregate').innerHTML='<thead><tr>'+['Engine','Cases / failures','ΔE 1024 ↓','Foreground ΔE ↓','SSIM ↑','Paths','Coords','Geom. / artist','KiB','Median s ↓'].map(x=>`<th>${x}</th>`).join('')+'</tr></thead><tbody>'+ag.map(s=>`<tr><td>${labels[s.engine]}</td><td>${s.n} / ${s.failed}</td><td>${fmt(s.de1024,3)}</td><td>${fmt(s.de1024_foreground,3)}</td><td>${fmt(s.ssim1024,4)}</td><td>${fmt(s.paths,1)}</td><td>${fmt(s.coordinates,1)}</td><td>${fmt(s.geometry_ratio,2)}×</td><td>${fmt(s.bytes/1024,2)}</td><td>${fmt(s.median_seconds,3)}</td></tr>`).join('')+'</tbody>';
$('status').textContent=`${new Set(rows.map(r=>r.key)).size} source SVGs · ${rows.length} traced outputs · ${rows.filter(r=>r.error).length} trace errors · ${cases().findIndex(r=>r.key===key)+1} / ${cases().length} selected cases`;transform();
$('families').innerHTML='<thead><tr><th>Dataset</th>'+Object.values(labels).map(x=>`<th>${x}</th>`).join('')+'</tr></thead><tbody>'+families.slice(1).map(f=>'<tr><td>'+esc(f)+'</td>'+Object.keys(labels).map(e=>{const s=DATA.summary.find(a=>a.family===f&&a.size===size&&a.engine===e);return `<td>${fmt(s?.de1024,3)} / ${fmt(s?.coordinates)}</td>`}).join('')+'</tr>').join('')+'</tbody>';
document.querySelectorAll('.view').forEach(v=>{v.onpointerdown=e=>{drag={x:e.clientX,y:e.clientY,px:pan.x,py:pan.y,w:v.clientWidth};v.setPointerCapture(e.pointerId)};v.onpointermove=e=>{if(drag){pan={x:drag.px+(e.clientX-drag.x)/drag.w,y:drag.py+(e.clientY-drag.y)/drag.w};transform()}};v.onpointerup=()=>drag=null;v.onpointercancel=()=>drag=null});}
$('family').onchange=choose;['case','size','vt','view'].forEach(id=>$(id).onchange=draw);$('zoom').oninput=transform;
$('reset').onclick=()=>{pan={x:0,y:0};$('zoom').value=1;transform()};
function step(d){const s=$('case');s.selectedIndex=(s.selectedIndex+d+s.options.length)%s.options.length;draw()}
$('next').onclick=()=>step(1);$('prev').onclick=()=>step(-1);choose();
</script></body></html>'''

if __name__=='__main__':main()
