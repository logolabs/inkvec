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
- **And everything that is not geometry.** Colours written in the fewest characters,
  numeric attributes without their leading and trailing zeros, presentation attributes
  that restate what the element already inherits, `style="fill:…"` written as an
  attribute, an attribute every child of a group shares moved onto the group, ids nothing
  references, empty groups, comments, `<metadata>`, and the whitespace between and inside
  tags. `<title>` is never touched and `<desc>` only when it is empty or is a drawing
  program signing its work — both are read aloud by a screen reader. These rules are
  re-implemented from [SVGO](https://github.com/svg/svgo) (MIT); see `src/document.rs` and
  the NOTICE file. `--no-document` turns them off.

## Measured

On 40 artist SVGs from the regression corpus (lucide, material-icons, simple-icons,
noto-emoji, openmoji, twemoji), at the defaults:

| family | numbers saved | bytes saved | mean dE00 vs original | worst dE00 |
|---|---|---|---|---|
| openmoji | 28.8% | 40.9% | 0.0155 | 0.025 |
| simple-icons | 26.2% | 18.8% | 0.0152 | 0.036 |
| noto-emoji | 25.5% | 22.4% | 0.0150 | 0.029 |
| twemoji | 10.5% | 6.5% | 0.0020 | 0.005 |
| lucide, material-icons | ~0–6% (little left in the geometry) | 1–9% | 0.0006 | 0.005 |
| **all 40** | **23.9%** | **23.5%** | **0.0077** | **0.036** |

For scale, the tracer's own colour error against these files averages 0.148, so the
rewrite is twenty times below it — invisible. Individual files reach 56%; the heaviest
goes from 213 KB to 95 KB.

The two columns answer different questions, and both are worth reading. *Numbers* is the
description length — how much of the drawing was redundant. *Bytes* is the file, which
also depends on how the numbers are written and on everything around them. On the tracer's
own output the first is 5.8% and the second is 22.1%: there is almost nothing left in its
geometry, which is the check this tool exists to make, but its emitter writes absolute
coordinates at a fixed precision where relative ones at the tolerance's own precision
would do.

### Against SVGO

On a nine-file spread (two real-world illustrations and seven corpus icons, 61 KB in
total), against [SVGO](https://github.com/svg/svgo) 4 at its defaults:

| | bytes saved |
|---|---|
| SVGO alone | 29.7% |
| this tool alone | **33.5%** |
| this tool, then SVGO | 43.1% |
| SVGO, then this tool | **44.1%** |

They are not the same tool. SVGO reads markup and cannot know that sixteen cubics are a
circle; this refits the geometry, which is where its lead comes from and why it wins most
on hand-drawn files. Running both still beats either, because SVGO minifies things this
deliberately leaves alone — stylesheets, transforms, `<defs>`, and merging paths.

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
