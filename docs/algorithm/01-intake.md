# Stage 01 — Intake

> Turns an arbitrary source file into the exact pixels the rest of the pipeline was tuned
> against: decoded, checked for lossy damage, undone if it is a disguised upscale, cleaned
> if it is genuinely degraded, priced in units of content rather than pixels, and matted to
> two opaque colours wherever it carries transparency.

**Source:** `crates/inkvec-trace/src/lib.rs` (decode and lossy detection),
`crates/inkvec-cli/src/lib.rs` (the driver: `trace_image`, resolution invariance),
`crates/inkvec-cli/src/alpha.rs` (unblock detection, matting),
`crates/inkvec-sr` (the super-resolution pre-pass, as a crate of its own)
**Entry points:** `run()` (`crates/inkvec-cli/src/lib.rs:402`), which calls
`load_image()` (`inkvec-trace/src/lib.rs:115`) then `resolve_lossy()`
(`inkvec-cli/src/lib.rs:424`), then `trace_image()` (`inkvec-cli/src/lib.rs:196`) — the
function that does essentially everything this stage covers.
**Pipeline position:** before the trace crate is invoked at all. `trace_image` ends by
calling one of `run_strokes` / `run_bilevel` / `run_color`, and it is `run_color` that
calls `inkvec_trace::trace_color_full_with_alpha` — stage 03 onward. Intake carries no
stopwatch mark of its own; its cost is folded into whatever the caller measures around
`trace_image`, except for the SR pre-pass, which times itself explicitly (see below).

## What problem this solves

A raster file is not a canonical representation of a drawing — it is one snapshot at one
size, possibly re-encoded, possibly resized by something that was not the artist, possibly
carrying transparency the palette has no dimension for. Every stage downstream of intake
was tuned against a specific kind of input (a native anti-aliased render, roughly 128 px on
its long side, opaque), and every one of those assumptions can be wrong on a file a user
actually hands the tool. Intake is not one problem; it is five, bundled because they all
have to be resolved before a single pixel reaches the palette:

1. **Decode.** Turn bytes of an unknown container format into straight RGBA floats.
2. **Know whether the file lies about its own cleanliness.** A JPEG or lossy WebP looks,
   pixel by pixel, like a slightly noisy PNG — except the noise clusters at edges in a way
   that fools every pixel-level detector tried (see below), so the container format itself
   has to be asked.
3. **Undo what a viewer already did to the file.** Someone with a 96-px logo who wants a
   large SVG frequently resizes the PNG first, with nearest-neighbour resampling — many
   viewers do this without being asked. What arrives is not more detail, it is the same
   detail redrawn as a staircase of large flat blocks, and the tracer would faithfully
   vectorise the staircase.
4. **Price every pixel-denominated constant in the tracer per unit of content, not per
   pixel**, because the same logo exported at 128 px and at 512 px is the same drawing and
   should cost the same number of SVG parameters — see Resolution invariance, below.
5. **Reduce transparency to two opaque colours**, because the projection at the heart of
   stage 02 (`coverage::bilevel_coverage`) and the palette's colour-mixture algebra both
   need a foreground and a background colour to unmix a boundary against; a fourth,
   alpha, dimension has nowhere to go in either of those.

## Inputs and outputs

**Input:** a file path (`load_image`, `crates/inkvec-trace/src/lib.rs:115`) or an
in-memory byte slice (`decode_image`, `lib.rs:120`), in any format the `image` crate
recognises; in practice PNG, JPEG, WebP, GIF, BMP or TIFF (`crates/inkvec-cli/src/args.rs:103`
lists the accepted extensions). Both funnel through `from_dynamic` (`lib.rs:125-134`),
which calls `.to_rgba8()` and divides every byte by 255, producing straight
(unpremultiplied) floats — the same `Rgba` type stage 02 consumes.

**Output:** an `Rgba` the size the tracer will actually trace at (which may differ from the
file's own dimensions — see unblock, SR and `--max-dim` below), an `Args` whose `lossy` and
`precision`/`min_area` fields have been resolved from `auto` to concrete values, and — when
any resampling happened — a `(display_w, display_h)` pair the emitted SVG is retargeted to
at the end, so the file the user gets back always claims the size the input file claimed,
whatever size it was actually traced at.

## How it works

### Decode and the lossy-container check

`load_image` / `decode_image` do nothing exotic — decode, then `from_dynamic`. The
interesting function next to them is `lossy_container` (`lib.rs:94-113`):

```rust
pub fn lossy_container(bytes: &[u8]) -> Option<bool> {
    match image::guess_format(bytes).ok()? {
        image::ImageFormat::Jpeg => Some(true),
        image::ImageFormat::WebP => match bytes.get(12..16)? {
            b"VP8 " => Some(true),
            b"VP8L" => Some(false),
            _ => None,
        },
        image::ImageFormat::Png
        | image::ImageFormat::Gif
        | image::ImageFormat::Bmp
        | image::ImageFormat::Tiff => Some(false),
        _ => None,
    }
}
```

This is a fact about the container, not a statistic about the pixels, and the doc comment
(`lib.rs:75-93`) is explicit about why that distinction matters: the palette has a guard for
exactly this kind of damage (`color::SOFT_NOISE_SIGMAS`, documented in `03-palette.md`), but
until 2026-09-08 it was switched only by `coverage::intake_scale` — the measured edge
*width* — which is blind to compression, because JPEG rings flat regions without widening
an edge. WebP is one container holding two different codecs, so the answer is read from the
RIFF sub-chunk tag at byte 12: `VP8 ` (with the trailing space) is the lossy codec, `VP8L`
is lossless, and `VP8X`, the extended container, is left `None` rather than guessed — its
sub-chunks would need walking to know for certain. Every other accepted format stores exact
samples and reads `Some(false)`. An unrecognised format reads `None`.

Four tests pin this down (`lossy_container_tests`, `lib.rs:1278-1321`): a real JPEG and PNG
encode read `Some(true)` / `Some(false)` (`jpeg_is_lossy_and_png_is_not`); only the first 32
bytes of the JPEG are needed (`a_header_is_enough`) — which matters, because the caller only
reads a small head of the file, not the whole thing; the three WebP RIFF tags resolve as
described (`webp_is_read_from_the_riff_chunk`); and nonsense bytes resolve to `None`, not to
a guess (`nonsense_is_unknown_not_clean`).

### `resolve_lossy` and `--lossy auto|on|off`

`resolve_lossy` (`crates/inkvec-cli/src/lib.rs:424-437`) turns the CLI's three-way `--lossy`
flag (itself `inkvec_sr::Mode`, reused rather than a separate enum) into a definite yes or
no, once, before the trace crate ever sees a pixel:

```rust
pub fn resolve_lossy(args: &Args, head: impl FnOnce() -> Option<Vec<u8>>) -> Args {
    let mut out = args.clone();
    if out.lossy == inkvec_sr::Mode::Auto {
        let lossy = head()
            .and_then(|b| inkvec_trace::lossy_container(&b))
            .unwrap_or(false);
        out.lossy = if lossy { inkvec_sr::Mode::On } else { inkvec_sr::Mode::Off };
    }
    out
}
```

`head` is injected rather than read directly so the function is testable without touching
disk; `run()` (`lib.rs:402-416`) supplies it by reading the first 32 bytes of the input
file. An unreadable or unrecognised container resolves to `Off` — "not known to be lossy" —
because, as the doc comment states, the guard this feeds costs **10.9%** on the 246-icon
screen set when it runs on a clean intake (0.4005 → 0.4442, measured 2026-09-08) and must
not fire on a guess (`lib.rs:420-423`). `--lossy` defaults to `Auto`
(`crates/inkvec-cli/src/args.rs:82`), so container-format detection runs on every trace
unless a user overrides it; `On` is there for a file that no longer admits what was done to
it — a screenshot of a JPEG re-saved as PNG (`args.rs:38-42`).

The resolved `args.lossy == Mode::On` becomes `ColorOptions::lossy_intake`
(`crates/inkvec-cli/src/lib.rs:800`), which the colour front end reads alongside
`coverage::intake_scale` to decide whether the palette's soft-intake constants
(`SOFT_NOISE_SIGMAS`, `SOFT_SAME_INK_DE00`) switch on — documented fully in `03-palette.md`.

### The unblock pre-pass — undoing an exact upscale

`pixel_grid` (`crates/inkvec-cli/src/alpha.rs:227-255`) answers a narrow, exact question:
is this raster a nearest-neighbour replication of a smaller one? A `k`× replication is
exactly invertible — average each `k`×`k` block and the original pixels return bit for
bit — so the test is deliberately strict, not a tolerance-based heuristic:

```rust
let constant = (0..h / k).all(|by| {
    (0..w / k).all(|bx| {
        let first = px(bx * k, by * k);
        (0..k).all(|dy| (0..k).all(|dx| {
            let p = px(bx * k + dx, by * k + dy);
            (0..4).all(|c| (p[c] - first[c]).abs() < 1.0 / 512.0)
        }))
    })
});
```

Every pixel in every block must match its block's first pixel to within `1.0/512.0`
per channel — no tolerance for "nearly", because a looser test would also catch a genuine
drawing of large flat squares and averaging that away would be a real loss
(`alpha.rs:219-223`). `k` is tried downwards from `MAX_FACTOR = 32` to `2` so an 8× upscale
is reported as 8×, not folded into a coarser factor it also happens to divide; below
`smallest = 64` pixels per side the search stops even trying, because an unblocked image
that small would leave too few pixels for the later boundary solve to work with
(`alpha.rs:231-234`).

`trace_image` runs this first, before anything else looks at the image
(`crates/inkvec-cli/src/lib.rs:210-226`), and the doc comment above the call states the
measured cost of a naive tracer that skips it: a 96-px logo blown up to 768 traces its
pixel boundaries directly, coming back as 13 inks instead of 3 and 1568 straight lines
walking round pixel corners instead of the original curves. `--no-unblock` disables it.
Detection is exact-match only; a resampled or anti-aliased upscale — where block boundaries
are not perfectly constant — fails this test by design and is `--sr`'s problem instead
(`alpha.rs:225-226`).

### Why the SR pre-pass runs after unblock, not before

`inkvec-sr` (its own crate) exists for damage `pixel_grid` cannot undo exactly: JPEG
ringing, blur, a genuine photograph of a logo. It is documented in full below; the ordering
question is intake-specific and the comment at the call site
(`crates/inkvec-cli/src/lib.rs:201-209`) states the reason plainly:

> Before the pre-pass, not after, and that is the whole point of the order: run the
> upscaler on the blocky version and it treats the block edges as the artwork — measured on
> a 96-px logo blown up to 768, `--sr on` came back with 14 inks and 1846 segments, worse
> than doing nothing. On the recovered original it has something real to put detail back
> into.

In other words: unblock is exact and free of side effects when it applies, so it always
runs first; whatever the SR model sees next is the smallest raster that is honestly
representative of the drawing, and it has real edges to sharpen rather than staircase
artefacts to hallucinate detail onto.

### The SR pre-pass itself: `Mode::Auto | On | Off`

`--sr` (default `Off`, `crates/inkvec-cli/src/args.rs:83`) controls a network-based
upscale-then-halve cleanup, implemented in the `inkvec-sr` crate.
`inkvec_sr::Mode` (`crates/inkvec-sr/src/lib.rs:32-40`):

```rust
pub enum Mode {
    #[default]
    Auto,   // trace, measure the fit, clean and retrace only if the fit is bad
    On,     // always clean first
    Off,    // never clean
}
```

`Mode::On` calls `inkvec_sr::prepass` (`inkvec-sr/src/lib.rs:85-122`) unconditionally: it
zeroes the colour channels of fully-transparent pixels (so they cannot drag their upscaled
neighbours toward an arbitrary stored colour), upscales with whatever `Upscaler`
implementation was built (the in-process model under the `model` feature, or a packaged
Python fallback via `external::External`), halves back down with a box filter
(`clean::box_downsample`) to the requested `--sr-scale` (default 2), and — unless
`--sr-no-recolour` — repaints flat regions with the source's own colours
(`clean::match_flats`) rather than trusting the network's colour reconstruction.

`Mode::Auto` (`crates/inkvec-cli/src/lib.rs:231-250`) is the more interesting path: it
traces the image once as it arrived (`trace_once`, `lib.rs:471-482`, itself a stripped
version of the ordinary pipeline), renders that trace back to a raster with `resvg`
(`inkvec_sr::detect::render_svg`, the same renderer the benchmark harness scores with — so
a residual measured here cannot disagree with a score measured there), and asks
`inkvec_sr::decide` (`inkvec-sr/src/lib.rs:139-147`) whether the trace explains the input
where the trace itself claims to be flat:

```rust
pub fn decide(img: &Rgba, svg: &str, threshold: f64) -> Decision {
    let Ok(model) = detect::render_svg(svg, img.width, img.height) else {
        return Decision::Keep { residual: None };
    };
    match detect::interior_residual(img, &model) {
        Some(r) if r > threshold => Decision::Clean { residual: Some(r) },
        r => Decision::Keep { residual: r },
    }
}
```

`interior_residual` (`inkvec-sr/src/detect.rs:108-140`) is the signal: both images are
composited onto white, a mask marks pixels where the *traced model* is flat (using the
composited colour, not the straight-alpha one, because straight-alpha channels are constant
across an anti-aliased edge while only alpha moves — a mask built on them would call every
boundary pixel flat, exactly where a trace and its input are expected to disagree), and the
RMS colour difference over that masked, genuinely-flat area is the residual. A face-model
disagreement in flat territory is not modelling error — a piecewise-flat model is flat by
construction — so it is degradation.

**The normalisation is deliberately `sum / 9n`, not the `sum / 3n` an RMS over three colour
channels would give** (`detect.rs:97-104`), so this residual reads `1/sqrt(3)` of the true
RMS. The doc comment calls this out explicitly as *not* a bug to fix: it is the formula
every threshold in the module was calibrated against, mirrored deliberately in the reference
Python implementation (`tools/inkvec_sr/clean.py`, "divides by three twice for the same
reason"), and "correcting it without recalibrating `DEGRADED_RESIDUAL` would move every
clean icon across the threshold — which is exactly what it did when this was first written
the 'right' way." `interior_residual` returns `None` when fewer than 100 masked pixels
exist to judge on, and the caller treats "cannot tell" as "keep" rather than "clean it",
because cleaning a clean image is the expensive mistake — three times worse than doing
nothing, dE00 0.605 against 0.195, per the module doc (`lib.rs:9-12`).

`DEGRADED_RESIDUAL = 0.5` (`detect.rs:36`, also the default for `--sr-threshold`,
`args.rs:84`) is calibrated over 30 icons in five conditions (`detect.rs:13-25`):

| condition | min | median | p95 | max |
|---|---|---|---|---|
| clean | 0.000 | 0.000 | 0.275 | 0.376 |
| jpeg-q80 | 0.454 | 0.907 | 1.441 | 2.708 |
| jpeg-q50 | 0.650 | 1.241 | 1.942 | 2.669 |
| blur-1.0 | 0.274 | 0.914 | 2.332 | 5.524 |
| noise-2 | 0.812 | 0.837 | 1.156 | 1.426 |

At `0.5`, no clean icon in this set is cleaned and 4.2% of damaged ones are missed; clean
and `blur-1.0` overlap, so no threshold cleanly separates every condition, but the case the
detector was built for — JPEG — is cleanly separated. A cheaper, pixel-only detector (an 8×8
block signature, needing no trace at all) was tried and refuted: it reads 2.175 on clean
against 2.133 on JPEG q50, no better than noise. `Auto` therefore pays for one full extra
trace, because nothing cheaper discriminates.

### `--intake-scale` (opt-in): resampling an oversampled input

`normalise_intake` (`crates/inkvec-cli/src/lib.rs:157-181`) is a separate, opt-in mechanism
from the unblock and SR pre-passes above: rather than undoing an exact replication or
cleaning genuine damage, it resamples an input that is native but carries more pixels per
unit of edge detail than the tracer needs — a photograph or a smoothly resized image, where
`coverage::intake_scale` (stage 02) reads a wide edge. It only runs when `replicated` is
false (unblock did not already fire) and `args.sr == Mode::Off` (the SR pre-pass, when it
runs, is expected to have already normalised scale):

```rust
fn normalise_intake(img: inkvec_trace::Rgba, quiet: bool) -> (inkvec_trace::Rgba, bool) {
    let scale = inkvec_trace::coverage::intake_scale(&rgb, img.width, img.height);
    if scale < INTAKE_SCALE_FLOOR { return (img, false); }
    let s = scale.min(INTAKE_SCALE_CAP);
    ...
    let out = inkvec_trace::coverage::downsample_to(&img, nw, nh);
    (out, true)
}
```

`INTAKE_SCALE_FLOOR = 1.5` (`lib.rs:152`) — below this the intake is left alone; "every
image in the corpus reads exactly 1.00" per the constant's own comment.
`INTAKE_SCALE_CAP = 8.0` (`lib.rs:155`) bounds how much a single, possibly wrong, estimate
is allowed to throw away at once. `--intake-scale` defaults off (`args.rs:88, 248`); the
usage text states the trade explicitly (`args.rs:202-215`): on 8×-upsampled input at 1024 px
it is 24× faster with a twelfth of the parameters and half the colour error, but **DINO**
— the corpus's structural-similarity measure — falls from 0.954 to 0.916, so it is offered
for input too slow to trace at all, not to make an already-tractable trace faster.

### `--max-dim` and `--time-budget`

`--max-dim` (default `2048`, `args.rs:67, 227`) is a hard ceiling applied after every other
resampling decision (`crates/inkvec-cli/src/lib.rs:295-314`): if the longest side still
exceeds it, the image is box-filtered down to it — the same exact area average
`normalise_intake` uses, "so edges stay edges" — and the SVG is written at the original
size regardless. The stated reason is trace time, which grows with pixel count; 2048 is
chosen to keep "a typical logo under a few seconds" (`args.rs:113-116`), a qualitative
target rather than a swept figure.

`--time-budget` (default `0.0`, meaning unlimited, `args.rs:68, 228`) is advisory rather
than a hard deadline. It is spent in `run_color` (`crates/inkvec-cli/src/lib.rs:787-794`):
60% of it becomes the deadline passed to `ColorOptions::deadline`, which stops gradient-band
merging early (the one stage whose cost grows with the square of the region count — an
unmerged band is still a correct fill, just an unmerged one); 25% becomes the millisecond
budget for the boundary solve. The output is stated to remain "a correct trace, with more
fills or a less polished outline" at any budget (`args.rs:117-120`) — the two fractions
(`0.6`, `0.25`) are not derived from a sweep in anything read for this stage; see Open
questions.

### Resolution invariance: `content_scale`, `REF_EXTENT`, `fit_config`, `in_content_units`

This is the subtlest mechanism in the stage, and it exists to fix a real, measured disease.
Every pixel-denominated constant in the tracer — the positional uncertainty
`DEFAULT_SIGMA_MODEL` (≈0.05 px, stage 02), the output coordinate `--precision` (0.1 px),
the speckle floor `--min-area` (2 px²) — was set against a corpus whose images are roughly
128 px on the long side. Handed a 512-px raster of the *same* logo, nothing about those
constants changes, but the geometry the fitter sees does: four times the boundary points,
each carrying four times the pixel residual for the same relative fit, while `lambda`
(`= ln(extent/precision)`, `crates/inkvec-fit/src/lib.rs:64-80`) has grown by only
`ln(4) ≈ 1.386` — a fraction of a nat, nowhere near enough to offset a chi-squared term
that scales with the square of the residual times four times as many points. The fitter
therefore keeps buying segments it does not need, because each additional segment looks
cheap next to how expensive staying wrong has become. Measured on real brand logos
(`crates/inkvec-cli/src/lib.rs:90-99`): **3.54× the artist's own parameter count at 128 px,
6.02× at 256 px, 11.62× at 512 px, for content that had not changed at all.** A brand mark
has one complexity, and the doc comment's framing is exact: a vectoriser should return it
"whatever resolution the export happened to be."

The fix restates the pixel-denominated quantities per unit of content. Let
`REF_EXTENT = 128.0` (`lib.rs:103`) be the size the corpus was tuned at, and

```rust
fn content_scale(img: &inkvec_trace::Rgba, args: &Args) -> f64 {
    if !on { return 1.0; }
    (img.width.max(img.height) as f64 / REF_EXTENT).max(1.0)
}
```

(`lib.rs:107-120`) — never below `1.0`, so a raster *smaller* than the reference does not
get to claim more precision than the reference had. `s = extent / REF_EXTENT` is exactly
`1.0` at 128 px, so nothing moves for the corpus the tracer was tuned on.

Two things then use `s`, and it is important to be precise about which one the code
actually applies where:

**`in_content_units`** (`lib.rs:139-148`) multiplies a fitted polyline's *positional sigma*
— the per-point uncertainty from stage 02 — by `s`. This is called on every boundary before
fitting, in both `run_bilevel` (`lib.rs:748-750`) and `run_color` (`lib.rs:851-855`),
whenever `--content-units` is on, regardless of which other path produced the image. Because
the admissibility test in `inkvec-fit` accepts a candidate chord only while every point
stays within `tau * sigma` of it (stage 02's central idea), inflating `sigma` by `s` lets
the same *relative* deviation — now `s` times larger in raw pixels, because the boundary
itself is `s` times bigger — pass the same test it would have passed at 128 px.

**`fit_config`** (`lib.rs:122-136`) goes further: it also multiplies `--precision` by `s`
before deriving `lambda`, and then multiplies the resulting `lambda` by `s` again:

```rust
fn fit_config(img: &inkvec_trace::Rgba, args: &Args) -> FitConfig {
    let extent = img.width.max(img.height) as f64;
    let s = content_scale(img, args);
    let mut cfg = FitConfig::from_precision(extent, args.precision * s, args.tau);
    cfg.lambda *= s;
    cfg
}
```

Algebraically, because `extent = REF_EXTENT * s` whenever `s > 1`, the first factor of `s`
cancels the `s` in `extent`, leaving `lambda` fixed at `ln(REF_EXTENT / precision)` before
the second multiplication restores it to `s * ln(REF_EXTENT / precision)` — the doc comment
states this outcome directly (`lib.rs:122-129`): the further factor of `s` prices the point
count, because "the data term is a sum over boundary samples, there are `s` times as many
of them per unit of content, and pricing a parameter in the same currency means scaling its
cost by the same `s`." The comment's own conclusion is that **both** mechanisms are needed
together: "Together with sigma scaled by `s` at the fit, the optimum is the one a 128 px
raster of the same shape would reach — from better points."

**What actually reaches the emitted SVG is only the first mechanism.** `trace_image`
constructs the `cfg` that is passed to `run_strokes`, `run_bilevel` and `run_color` — the
functions that produce the SVG the user gets — directly, at `lib.rs:374`:

```rust
let cfg = FitConfig::from_precision(extent, args.precision, args.tau);
```

with the raw pixel `extent` and the unscaled `args.precision`; `content_scale` plays no
part in this call. `fit_config`, the function that scales `lambda` as well as `sigma`, is
called in exactly one place in the whole crate: `trace_once` (`lib.rs:471-482`), the probe
trace used only when `--sr auto` decides whether the input needs cleaning — and `--sr`
defaults to `Off` (`args.rs:83`), so on the overwhelmingly common configuration `fit_config`
is never called at all. In every actual, emitted trace, `--content-units` widens each
boundary's tolerance (`in_content_units`) but leaves `lambda` at its raw-pixel value,
growing only logarithmically with resolution — the same quantity the module's own framing
names as insufficient on its own. See Open questions: whether this is a deliberate
simplification (sigma-widening alone may be "enough" in practice, since it is what the
9.6.2026 commit measured its real-brand-logo numbers against) or an unfinished wiring-up of
`fit_config` into the production path could not be settled by reading the source alone.

`content_units` defaults to `false` (`args.rs:66, 226`); `INKVEC_CONTENT_SCALE=1` (*removed*) is the
environment equivalent (`lib.rs:114-115`). It is off by default because it is a genuine
trade, not a free correctness fix: the usage text states 5-px rotated squares get accepted
as circles and thin rings come out broken under it, trading 1.16× the artist's parameters
for 0.82× (`args.rs:158-161`).

### The oversample block: pricing `--precision` and `--min-area` in the raster's own units

A separate mechanism, downstream of everything above, addresses a related but distinct
disease: a raster that is native (not resized, not compressed) but was simply exported far
larger than its content needs — a super-resolution model's own output, for instance, which
returns genuinely sharp edges at high resolution and so is invisible to `intake_scale`
(edge width) even though it is carrying far more pixels than the drawing needs. The comment
at the call site (`crates/inkvec-cli/src/lib.rs:325-341`) gives the measured cost of leaving
this alone: on the incorpo mark upscaled 4×, 301 paths and 11,960 coordinates against 16
paths and 456 coordinates for the same drawing at 1×; hand-scaling `--precision` and
`--min-area` for the known factor brought it back to 21 paths and 441 coordinates.

The fix is `coverage::oversample_factor` (stage 02), gated behind `intake_scale` because the
two measure different things and only one of them can make the first call safely:

```rust
let oversample = if inkvec_trace::coverage::intake_scale(&rgb, w, h)
    > inkvec_trace::color::SOFT_INTAKE_EDGE
{
    inkvec_trace::coverage::oversample_factor(&rgb, w, h) as f64
} else {
    1.0
};
```

(`lib.rs:342-358`). `intake_scale` decides *whether* this is native content — every corpus
raster reads at most 1.50 against the `SOFT_INTAKE_EDGE = 1.75` threshold (documented fully
in `03-palette.md`) — a job `oversample_factor`'s round-trip test cannot safely do on its
own, because smooth native artwork survives halving too and would be rewritten for no
reason. `oversample_factor` then measures *by how much*, which `intake_scale` cannot: an SR
model's sharp-but-oversampled output reads only 2.00 by edge width where the true factor is
4. When `oversample > 1.0`, `--precision` is scaled by `oversample` and `--min-area` by
`oversample²` (it is an area) before `FitConfig` is built (`lib.rs:359-372`) — this happens
independently of, and after, the `--content-units` machinery above; both can be active on
the same trace.

### Alpha handling and matting

Everything from `coverage::bilevel_coverage` (stage 02) onward assumes an opaque image: the
coverage projection needs a definite foreground and background colour to unmix a pixel
against, and the palette has no fourth, alpha, dimension to cluster on. So intake's last
step, after every resampling decision above, is to matte transparency away
(`crates/inkvec-cli/src/alpha.rs:319-320`).

`choose_matte` (`alpha.rs:287-424`) picks the flattening colour by what it would *swallow*
rather than by a fixed choice. White was the historical default, and white is exactly wrong
for the input people bring most often: a white mark on a transparent ground composites to
one flat white and traces to nothing at all; a pale translucent panel disappears into a
white background it is drawn over. The function walks the image once, classifying drawn
pixels into three kinds — the silhouette and its anti-aliased rim, flat translucency (one
opacity, which the emitter can later carry out as `fill-opacity`), and a "glow" (opacity
that varies across the shape, which no single flattening colour can serve honestly and so
gets no vote) — then tries six candidate colours in order (white, black, magenta, green,
cyan, orange) and keeps the first one that would not composite more than `SWALLOWED = 0.33`
of the drawn-and-translucent mass into itself, measured by CIEDE2000 within `MARGIN = 10.0`
(`alpha.rs:288-305, 411-423`). Two cheaper rules were tried and measured worse on the
246-icon screen set first — "furthest colour from everything in the image" swung a white
highlight buried in an emoji onto a saturated matte (0.4123 → 0.4735); voting on silhouette
colours alone did the same for a white sock — which is why the bar is a *share of drawn
mass*, not a distance to any single ink (`alpha.rs:271-275`).

This choice only reaches the traced faces under `--cutout` (default off): without it, the
image is always matted to plain white, because transparency cannot reach the output anyway
(a transparent face is painted solid, not punched, and translucency is baked flat), so
choosing a content-aware matte would move every edge in the file for no gain it could carry.
The doc comment on `alpha_source` records exactly why this gate is not optional: the
committed CI screen-set gate rejected an always-on content-aware matte outright — dE00
0.15404 → 0.15654 against a limit of 0.15558 — because the corpus is scored over white and
cannot see any of the gain a better matte buys on other backgrounds (`alpha.rs:436-442`).
`INKVEC_MATTE=white|black|magenta` (*removed*) forces the answer from the environment
(`alpha.rs:309-316`), bypassing `choose_matte` entirely.

`crates/inkvec-trace/src/alpha.rs` is a related but distinct mechanism, run *after* the
trace rather than during intake: `decompose` (`alpha.rs:345`) recovers a translucent layer
— one shape at one opacity, seen through several different backgrounds — from the flat face
partition the trace already produced. The module doc comment (`alpha.rs:1-90`) states the
algebra plainly: a layer of colour `C` at opacity `a` over two *different* backgrounds `G1`
and `G2` produces two observed faces whose difference `c_F1 − c_F2 = (1−a)·(c_G1 − c_G2)` is
independent of the unknown `C` — a face seen against only one background can never be
distinguished from a flat region, so a layer with only one hypothesis is rejected, always.
Recovering the layer needs at least two hypotheses over well-separated backgrounds, a
quad-adjacency test that matches the actual geometry of a translucent edge crossing a
background edge, and a battery of conservative gates (opacity in `(0.05, 0.98)`, standard
error on the recovered opacity bounded, and more — `alpha.rs:68-90`), because a false layer
is a visible error: it unions faces that are not one shape and paints them a colour that
appears nowhere in the source. This is exposed as `--layers` (default off, opt-in via
`INKVEC_LAYERS` (*removed*) too), and its own doc comment records it firing on "about one real icon in
twenty" of the census used to tune it — two of forty (`args.rs:126-132`,
`crates/inkvec-cli/src/alpha.rs:528-529`).

## Constants and thresholds

| name | value | controls | stated derivation |
|---|---|---|---|
| `REF_EXTENT` | `128.0` px | the intake size every pixel-denominated tracer constant was tuned at; divisor for `content_scale` | measured: 3.54×/6.02×/11.62× the artist's parameter count at 128/256/512 px for unchanged content (`crates/inkvec-cli/src/lib.rs:90-99`) |
| `INTAKE_SCALE_FLOOR` | `1.5` | below this, `--intake-scale` leaves the input alone | "Every image in the corpus reads exactly 1.00" (`lib.rs:150-151`) |
| `INTAKE_SCALE_CAP` | `8.0` | ceiling on how much `--intake-scale` will discard from one estimate | "A wrong estimate should cost detail slowly, not all at once" (`lib.rs:153-154`) |
| `--max-dim` default | `2048` px | ceiling on traced (not emitted) size | qualitative: "keeps a typical logo under a few seconds" (`crates/inkvec-cli/src/args.rs:113-116`); no sweep cited |
| `--time-budget` split | `0.6` merge / `0.25` boundary-solve | how an advisory wall-clock budget is allotted between the two most expensive stages | stated as a fixed split, no numeric derivation given (`args.rs:117-120`, `lib.rs:787-794`) |
| `MAX_FACTOR` (`pixel_grid`) | `32` | largest replication factor the unblock pre-pass will try | tried downwards so an 8× upscale is reported as 8×, not folded into a smaller divisor; no numeric derivation for the cap itself (`crates/inkvec-cli/src/alpha.rs:222-223, 228`) |
| `smallest` (`pixel_grid`) | `64` px | floor below which unblocking is not attempted | "leaves too few pixels for the boundary solve to work with" (`alpha.rs:231-234`); no swept value |
| block-constant tolerance (`pixel_grid`) | `1.0/512.0` per channel | how exactly a block must match to be called a replication | "strict — no tolerance for 'nearly'"; qualitative only, no numeric derivation for `1/512` specifically (`alpha.rs:219-223`) |
| `MARGIN` (`choose_matte`) | `10.0` (CIEDE2000) | how close a composited colour must land to a matte candidate to count as "swallowed" | no stated numeric derivation |
| `SWALLOWED` | `0.33` | share of drawn-and-translucent mass a matte candidate may swallow before rejection | motivated by the white-highlight and white-sock cases; the specific `0.33` itself is not swept (`alpha.rs:271-306`) |
| `DRAWN` | `0.5` | alpha above which a pixel counts as part of the silhouette rather than a glow/translucency vote | "faint content is baked against the matte whatever it is... letting it vote flipped two emoji onto a black matte" — qualitative (`alpha.rs:297-307`) |
| `DRAWN_FLOOR` | `0.05` | alpha below which a pixel is ignored entirely | no stated numeric derivation |
| `SOFT_SHARE` | `0.05` | share of drawn+soft pixels that must be "glow" before white is kept outright without running the candidate ladder | no stated numeric derivation |
| `FLAT_ALPHA` | `0.02` | spread in a pixel's neighbour alphas below which its translucency counts as "flat" rather than a glow | no stated numeric derivation |
| CI-gate literal `0.33` (`alpha_source`) | `0.33` | warns the user when white would swallow more than this share, absent `--cutout` | equal to `SWALLOWED` in value but written as a separate literal, not a reference to the constant (`alpha.rs:457`) — see Open questions |
| `DEGRADED_RESIDUAL` / `--sr-threshold` default | `0.5` | interior-residual threshold above which `--sr auto` cleans | measured over 30 icons, five conditions: sits above the worst clean reading (`0.376`) and below the weakest damaged one (`0.454`, jpeg-q80); "4.2% of damaged ones are missed" at this value (`crates/inkvec-sr/src/detect.rs:1-36`) |
| `--sr-scale` default | `2` | output scale of the pre-pass relative to the input | not derived in what was read; stated as the default only |
| interior-residual normalisation | `sum / 9n`, not `sum / 3n` | scales every residual reading (and therefore `DEGRADED_RESIDUAL`) to `1/sqrt(3)` of a true RMS | deliberate, matched to the reference Python implementation; explicitly "not a bug to fix" (`inkvec-sr/src/detect.rs:97-104`) |
| `SOFT_INTAKE_EDGE` | `1.75` px | gates whether `oversample_factor` runs at all | fully documented in `03-palette.md`; reused here unmodified |
| `OVERSAMPLE_TOL` | `3.0` | round-trip error tolerance inside `oversample_factor` | fully documented in `02-coverage.md`; reused here unmodified |

## Failure modes and edge cases

- **Unblocking after the SR pre-pass instead of before it destroys the SR model's input.**
  Measured directly: a 96-px logo blown up to 768 and cleaned before unblocking came back
  with 14 inks and 1846 segments, worse than tracing the blocky image untouched
  (`crates/inkvec-cli/src/lib.rs:201-209`).
- **A compressed logo that passes both pixel-level checks is traced as if it were clean.**
  This was a real, dated regression, fixed 2026-09-08 (`96c3b79`): a JPEG logo measured
  `intake_scale` at 1.15 px, under the 1.75 px `SOFT_INTAKE_EDGE` threshold, so the
  palette's soft-intake guard stayed off and the tracer fitted the encoder's ringing as
  artwork — **243 paths over 1301 faces, 89% of them covering 5% of the drawing.** Two
  pixel-level detectors were tried and refuted first: `sigma_noise` (a median Laplacian)
  reads its `0.5/255` floor on the damaged file, identical to a clean render, because
  ringing occupies a thin band beside the edge while most of the image stays flat and never
  moves the median; a flat-region Laplacian statistic separated clean (`0.0000`) from JPEG
  (`1.1`–`2.7`) cleanly — until it met a genuinely clean radial gradient, which scored
  *higher* "damage" than the compressed flat icon, because 8-bit ramp quantisation produces
  the same Laplacian spikes a lossy codec does. The fix — reading `lossy_container` instead
  of inferring damage from pixels — reduced the same file to **80 paths over 654 faces**,
  with the face, wrench, hand and lettering intact: it removed noise, not artwork
  (commit `96c3b79`, `crates/inkvec-trace/src/coverage.rs:633-640`).
- **Resolution dependence, before the fix, was severe and monotonic**: 3.54×/6.02×/11.62×
  the artist's parameter count at 128/256/512 px for a logo that had not changed — the
  disease `content_scale`, `REF_EXTENT` and `in_content_units` exist to treat (see How it
  works, above, and the Open question about `fit_config` not being wired into the emitted
  trace).
- **`--content-units` is a genuine trade, not a free fix**: 5-px rotated squares get fitted
  as circles and thin rings come out broken under it, in exchange for 0.82× the artist's
  parameters against 1.16× without it (`crates/inkvec-cli/src/args.rs:158-161`).
- **`--intake-scale` costs structural accuracy for speed**, and says so in its own help
  text: 24× faster with a twelfth of the parameters and half the colour error on 8×-upsampled
  input at 1024 px, but DINO — the corpus's structural measure — falls from 0.954 to 0.916
  (`args.rs:202-213`).
- **The SR pre-pass is three times worse than doing nothing on clean input** (dE00 0.605
  against 0.195, `inkvec-sr/src/lib.rs:9-12`), which is why `Mode::Auto` pays for a full
  probe trace before deciding, rather than guessing from pixel statistics.
- **A soft glow is deliberately excluded from voting on the matte**, and the cost of getting
  that wrong is recorded directly: letting a candle's flame vote on `noto-emoji/emoji_u1f56f`
  chose a black matte that baked the flame dark, dE00 0.22 → 5.31
  (`crates/inkvec-cli/src/alpha.rs:330-335`).
- **`pixel_grid`'s exact-match requirement is a deliberate blind spot, not an oversight.**
  A resampled or anti-aliased upscale — bilinear, Lanczos, or anything that blends across
  block edges — fails the constant-block test by design; that class of damage is `--sr`'s
  job (`alpha.rs:225-226`).

## Environment overrides

Since the settings cleanup (CHANGELOG, *Unreleased*) the engine reads its environment through one helper (`inkvec_core::env`): a switch is off when unset, empty or `0`, and every variable is read once per process. Variables marked *removed* below are gone (their defaults are constants now); those marked *research build* are read only by a binary built with `--features research`. The full list, with what is left and why, is [`docs/internal/env-vars.md`](../internal/env-vars.md).

| variable | effect | default | source |
|---|---|---|---|
| `INKVEC_CONTENT_SCALE=1` (*removed*) | equivalent to `--content-units` | unset (off) | `crates/inkvec-cli/src/lib.rs:114-115` |
| `INKVEC_MATTE=white\|black\|magenta` (*removed*) | forces `choose_matte`'s answer, bypassing the swallowed-mass search | unset (`choose_matte` decides) | `crates/inkvec-cli/src/alpha.rs:309-316` |
| `INKVEC_LAYERS` (*removed*) | forces `--layers` on | unset (off, same as `--layers` unset) | `crates/inkvec-cli/src/alpha.rs:538` |
| `INKVEC_LAYER_SIGMA` (*removed*) | overrides the sRGB noise sigma used when fitting a translucent layer | `LAYER_SIGMA_SRGB` | `crates/inkvec-cli/src/alpha.rs:567-571` |
| `INKVEC_ALPHADBG` | prints alpha/layer diagnostics to stderr | unset (silent) | `crates/inkvec-cli/src/alpha.rs:708`, `crates/inkvec-cli/src/emit.rs:569` |

No `INKVEC_*` variable is read inside `inkvec-trace/src/lib.rs`'s `load_image`
(`lib.rs:127-130`), `decode_image` (`lib.rs:133-136`), `from_dynamic` (`lib.rs:138-147`) or
`lossy_container` (`lib.rs:105-124`), nor anywhere in the `inkvec-sr` crate — every knob in
the SR pre-pass and the lossy-container check is a CLI flag, not an environment variable.
Other functions later in that same file (`lib.rs`) do read many `INKVEC_*` variables — for
palette, boundary-solve, decode and other downstream stages — but none of those reads
happen inside the four intake functions named above.

## Open questions

- **`fit_config`'s lambda-scaling is written, documented as necessary, and not wired into
  the trace that is actually emitted.** Its own doc comment (`crates/inkvec-cli/src/lib.rs:122-129`)
  states plainly that both halves — sigma scaled at the fit, and `lambda` scaled with it —
  are needed together for the fitter to reach "the optimum a 128 px raster of the same
  shape would reach." But `trace_image`, which produces the SVG returned to the user, builds
  its `FitConfig` directly at `lib.rs:374` (`FitConfig::from_precision(extent, args.precision,
  args.tau)`), never calling `fit_config`. `fit_config` is called in exactly one place in
  the crate — `trace_once` (`lib.rs:471-482`), the probe trace `--sr auto` uses to decide
  whether to clean — and `--sr` defaults to `Off`, so on the ordinary code path `fit_config`
  is never invoked at all. In every actual, emitted trace under `--content-units`, only
  `in_content_units`' sigma-widening runs; `lambda` stays at its raw-pixel value, growing
  only logarithmically with resolution, which is the exact insufficiency the module's own
  framing (`REF_EXTENT`'s doc comment) names as the problem. Whether this is deliberate
  (sigma-widening alone may already suffice for the cases measured — the real-brand-logo
  numbers cited throughout this file were presumably measured against the code as it stands)
  or an incomplete wiring-up left over from `fit_config`'s introduction could not be
  determined from the source or its history alone, and the tracer was not run to check.
- **The CI-gate warning threshold in `alpha_source` (`crates/inkvec-cli/src/alpha.rs:457`,
  literal `0.33`) duplicates `SWALLOWED` (`alpha.rs:305`) by value but not by reference.**
  A future change to `SWALLOWED` would silently desynchronise the warning from the matte
  selection it is meant to describe.
- **`choose_matte`'s `MARGIN`, `DRAWN_FLOOR`, `SOFT_SHARE` and `FLAT_ALPHA`** are each given
  a qualitative role in their surrounding comments but no measured sweep, unlike `SWALLOWED`
  and `DRAWN`, which are tied to specific before/after cases (the white-highlight and
  candle-flame regressions).
- **`pixel_grid`'s `MAX_FACTOR = 32`, `smallest = 64`, and the `1/512` per-channel
  tolerance** are each qualitatively justified but not swept to a specific measured value.
- **`--time-budget`'s `0.6` / `0.25` split** between gradient-band merging and the boundary
  solve is stated as a fixed allocation with no derivation shown; it is plausible this was
  chosen by observing which of the two stages tends to dominate runtime, but that reasoning
  is not recorded where it was read.
- **`DEGRADED_RESIDUAL`'s calibration set is small** — 30 icons in five synthetic conditions
  — and the module's own table shows `clean` and `blur-1.0` overlapping in range even at the
  chosen threshold. The doc comment is honest about this ("no threshold separates every
  condition"), but the generalisation of `0.5` beyond this specific 30-icon set was not
  independently verified here.
