# inkvec-svgmin

Rewrite an SVG's paths as the fewest segments that draw the same picture.

```sh
inkvec-svgmin logo.svg -o logo.min.svg --stats
inkvec-svgmin *.svg --out-dir min/ --stats
```

An SVG path can be written a thousand ways: a circle as sixteen cubics or as five, a
straight edge as a run of collinear lines, one curve as four exactly-subdivided pieces.
They all render identically, and every extra segment is description length that says
nothing. This tool rewrites each `<path d>` as the cheapest description that stays within a
tolerance of the original curve, scored the way the Inkvec tracer scores everything:
`0.5·chi² + λ·params`, the minimum-description-length objective of `inkvec-fit`, over the
same alphabet of lines, cubics, circular arcs and whole primitives.

## What it promises

- **Corners survive exactly.** A raster tracer has to infer corners from pixels; here the
  source is vector, so a corner is a fact — two consecutive segments whose tangents
  disagree by more than `--corner-degrees` (30°). Those joins are hard breaks that no fit
  may smooth across; everything between two of them is refitted freely.
- **The tolerance is stated at a viewing size.** `--tolerance 0.1 --judge 1024` means "no
  point of the new outline is more than 0.1 px from the old one when the drawing is 1024 px
  wide", whatever the viewBox units are. Every rewritten run is checked against that bound
  after fitting, and a segment that strays is replaced by the exact source curve it covers.
- **Nothing but geometry changes.** Only `d` attributes are rewritten. Paint, gradients,
  ids, groups, clip paths and transforms pass through byte for byte, and a path that would
  not get *smaller* keeps its original text, so a hand-optimised file is never made larger.
- **The bytes are minimised too, not just the segments.** Each command goes down in
  whichever of its absolute and relative forms is shorter, the letter is dropped where it
  repeats, an axis-aligned line becomes `H` or `V`, a smoothly continuing cubic becomes
  `S`, leading zeros and separators go wherever a parser does not need them, and every
  number is written at the fewest decimals that still lands within a quarter of the
  tolerance. The pen is tracked as a parser would reconstruct it, so relative commands
  cannot accumulate rounding, and the result is parsed back and checked against the source
  before it is kept.

## Measured

On 40 artist SVGs from the regression corpus (lucide, material-icons, simple-icons,
noto-emoji, openmoji, twemoji), at the defaults:

| family | numbers saved | bytes saved | mean dE00 vs original | worst dE00 |
|---|---|---|---|---|
| openmoji | 27.4% | 28.8% | 0.0155 | 0.025 |
| simple-icons | 24.3% | 16.9% | 0.0152 | 0.036 |
| noto-emoji | 23.6% | 14.0% | 0.0145 | 0.029 |
| twemoji | 9.6% | 6.0% | 0.0020 | 0.005 |
| lucide, material-icons | ~0% (nothing left in the geometry) | 0.4% | 0.0004 | 0.004 |
| **all 40** | **22.1%** | **15.4%** | **0.0076** | **0.036** |

For scale, the tracer's own colour error against these files averages 0.148, so the
rewrite is twenty times below it — invisible. Individual files reach 56%; the heaviest
goes from 213 KB to 95 KB.

The two columns answer different questions, and both are worth reading. *Numbers* is the
description length — how much of the drawing was redundant. *Bytes* is the file, which
also depends on how the numbers are written. On the tracer's own output the first is 0.2%
and the second is 14.6%: there is nothing left in its geometry, which is the check this
tool exists to make, but its emitter writes absolute coordinates at a fixed precision
where relative ones at the tolerance's own precision would do.

Speed, best of three: 21–53 ms for a small icon, 0.3–0.8 s for a curve-dense 1–2 KB logo,
0.6 s for a 20 KB emoji, 6.5 s for the corpus's heaviest file (213 KB, 8,608 segments).
The fit costs the square of the samples it is shown, so the time follows curve density
rather than file size. `bench/svgmin_eval.py` reproduces the table.

## How it works

1. Parse `d` (arcs and quadratics become cubics; `svgtypes` handles the syntax).
2. Mark corners from the source tangents. A closed subpath with no corner is fitted whole,
   so a circle or rounded rectangle can describe all of it; otherwise the corners cut it
   into runs, each refitted with both ends pinned.
3. Sample each run densely, run `inkvec-fit`'s multimodel dynamic program (lines, cubics,
   arcs) plus its primitive fitter, and keep the cheaper.
4. Refit every chosen cubic by least squares to the samples nearest it (Schneider's
   method with an exact nearest-point projection), so a cubic the source really contained
   comes back exact rather than approximated.
5. Verify every run against the tolerance; emit with `S` where a join is smooth.

Library use: `inkvec_svgmin::minify(&svg, &Options::default())` returns the rewritten
document and a `Report` of what changed.
