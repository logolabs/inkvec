"""Static scientific contact sheet of measured outputs, plus fidelity/complexity plot."""
from pathlib import Path
import json
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from PIL import Image
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT/'out/crosscompare-2026-09-15'
rows=json.loads((OUT/'results.json').read_text())
chosen=[('lucide__calendar-sync','Lucide · calendar'),('noto-emoji__emoji_u1f6a3','Noto · rowing'),
        ('fluent-emoji__Musical_notes_Color_musical_notes_color','Fluent · musical notes'),
        ('synthetic__gradient_radial','Synthetic · radial gradient')]
cols=[('truth','Artist SVG'),('inkvec','Inkvec core'),('ai','Neural AITracer'),('vtracer-detail','VTracer detailed')]
fig,axes=plt.subplots(len(chosen),len(cols),figsize=(12,12.6),dpi=150)
for i,(key,name) in enumerate(chosen):
    for j,(engine,label) in enumerate(cols):
        ax=axes[i,j]; ax.set_xticks([]);ax.set_yticks([])
        for spine in ax.spines.values():spine.set_visible(False)
        path=OUT/'cases'/key/('truth.png' if engine=='truth' else '512/'+engine+'.png')
        ax.imshow(Image.open(path))
        if i==0:ax.set_title(label,fontsize=13,pad=13)
        row=next(r for r in rows if r['key']==key and r['size']==512 and r['engine']==('inkvec' if engine=='truth' else engine))
        s=row['gt'] if engine=='truth' else row
        caption=f"{s['paths']} paths · {s['coordinates']:,} coords"
        if engine!='truth':caption+=f"\nΔE00 {s['de1024']:.3f} · {s['seconds']:.2f} s"
        else:caption+=f"\n{name}"
        ax.set_xlabel(caption,fontsize=10,labelpad=7)
fig.suptitle('Same SVG → 512px raster → trace',fontsize=17,y=.989)
fig.text(.5,.009,'Scored and shown at 1024px against the original. Coords count x/y scalars; primitives and gradients also matter.\nIllustrative subset; all 21 sources and both VTracer settings are available in the complete report.',ha='center',fontsize=10)
fig.subplots_adjust(left=.025,right=.98,top=.94,bottom=.065,hspace=.34,wspace=.09)
fig.savefig(OUT/'side-by-side.png',facecolor='white');plt.close(fig)

fig,axs=plt.subplots(1,2,figsize=(11,4.6),dpi=160)
colors={'inkvec':'#146b42','ai':'#9a40a4','vtracer-default':'#2060bf','vtracer-detail':'#c26419'}
labels={'inkvec':'Inkvec core','ai':'Neural AITracer','vtracer-default':'VTracer defaults','vtracer-detail':'VTracer detailed'}
for ax,size in zip(axs,[256,512]):
 for engine,c in colors.items():
    rs=[r for r in rows if r['size']==size and r['engine']==engine and not r.get('error')]
    ax.scatter([max(r['coordinates'],1) for r in rs],[max(r['de1024'],.0001) for r in rs],s=28,alpha=.75,label=labels[engine],c=c)
 ax.set_xscale('log');ax.set_yscale('log');ax.set_xlabel('Coordinate scalars (log scale) ↓');ax.set_ylabel('Mean ΔE00 at 1024px (log scale) ↓');ax.set_title(f'{size}px input · each point is one source');ax.grid(alpha=.16)
axs[0].legend(fontsize=8);fig.suptitle('Fidelity versus geometry cost · lower and left is better',fontsize=13)
fig.tight_layout();fig.savefig(OUT/'fidelity-complexity.png',facecolor='white')
print('Created side-by-side.png and fidelity-complexity.png')
