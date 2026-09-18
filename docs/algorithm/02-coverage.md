# Stage 02 — Coverage

> Turns an anti-aliased raster into a scalar coverage field with an explicit, per-pixel
> uncertainty, instead of a bitmap to be thresholded.

**Source:** `crates/inkvec-trace/src/coverage.rs`
**Entry points:** `bilevel_coverage()` (`coverage.rs:191`), `estimate_noise()` (`coverage.rs:165`),
`intake_scale()` (`coverage.rs:337`), `oversample_factor()` (`coverage.rs:467`),
`downsample_to()` (`coverage.rs:388`)
**Pipeline position:** first stage of the bilevel front end (`trace_bilevel`, `lib.rs:156`), which
calls `bilevel_coverage` directly. The colour front end (`trace_color_full_with_alpha`,
`lib.rs:256`) does not build a `CoverageField` — it takes the same noise estimate
(`estimate_noise`, `lib.rs:269`) and the same edge-width measurement (`intake_scale`, `lib.rs:282`)
and feeds them into palette extraction (stage 03) and, later, into `planar::refine_subpixel`
(`planar.rs:660-699`), which re-derives the coverage-gradient formula this module documents
rather than sharing a `CoverageField` object. There is no stopwatch mark named `coverage`; its
cost is folded into whichever caller runs first.

## What problem this solves

An anti-aliased pixel at the edge of a shape is not noise, and it is not a blurry copy of a
sharp original. It is a **measurement**: the fraction of that pixel's area the foreground
colour covers. `coverage.rs:1-33` (the module doc comment) states the stakes plainly — reading
that pixel as a measurement rather than thresholding it away is, in the authors' words, "the
single largest lever in this project", and cites the failure this fixes: on the `thin_features`
benchmark, VTracer recovers 2 of 11 elements and no parameter setting recovers more, because
"the information is destroyed before any tunable stage runs" (`coverage.rs:6-8`, referencing
`docs/M0-BASELINE.md` §3).

Recovering the fraction is only half the job. The other half — and the reason this module is
worth a stage of its own rather than a one-line helper — is recovering **how much that fraction
should be trusted**, per pixel, and handing that number downstream so every later stage can
decide how hard to try without being told separately.

## Inputs and outputs

**Input:** `Rgba` (`coverage.rs:120-127`) — straight (unpremultiplied) RGBA floats in `[0, 1]`,
row-major, four floats per pixel.

**Output:** `CoverageField` (`coverage.rs:41-79`):

| field | type | meaning |
|---|---|---|
| `width`, `height` | `usize` | dimensions |
| `data` | `Vec<f32>` | coverage `a` at each pixel centre, `[0, 1]` |
| `sigma_alpha` | `f64` | standard deviation of the coverage estimate, from pixel noise |
| `sigma_model` | `f64` | irreducible positional error of level-set extraction itself, in pixels |
| `fg`, `bg` | `[f32; 3]` | the sRGB foreground/background colours coverage was measured against |
| `saturation` | `f64` | confidence, in `[0, 1]`, that `fg`/`bg` were actually observed |

Two derived methods matter downstream:

- `gradient_magnitude(x, y)` (`coverage.rs:95-100`) — central-difference `|grad a|` at a point.
- `position_sigma(p)` (`coverage.rs:107-117`) — positional uncertainty of a boundary point, in
  pixels: the quantity every later stage actually consumes.

## How it works

### Inverting the measurement

For a boundary between foreground `F` and background `B`, an observed pixel is the coverage
mixture

```text
P = a*F + (1-a)*B
```

Projecting `P` onto the `F - B` axis inverts this:

```text
a = dot(P - B, F - B) / |F - B|^2
```

This is a least-squares estimate across all three colour channels at once (`coverage.rs:10-18`),
not a single luminance threshold — the module doc comment is explicit that using all three
channels is deliberate, not incidental. In code this is `bilevel_coverage`'s central loop
(`coverage.rs:218-224`):

```rust
let n = (p[0] - bg[0]) * d[0] + (p[1] - bg[1]) * d[1] + (p[2] - bg[2]) * d[2];
(n as f64 / dd).clamp(0.0, 1.0) as f32
```

where `d = F - B` and `dd = |F - B|^2`.

### Estimating `F` and `B`

`F` and `B` cannot simply be the darkest and lightest pixels in the image — a single JPEG
overshoot or stray speck would then define the colour axis for the whole image
(`coverage.rs:188-190, 199-202`). `bilevel_coverage` instead:

1. Composites the image over white and computes luminance
   `0.2126*R + 0.7152*G + 0.0722*B` (`coverage.rs:196`, Rec. 709 coefficients).
2. Sorts luminance and takes a small plateau — `0.1%` of pixels, clamped to `[1, n-1]`
   (`coverage.rs:206`) — from each extreme, rather than a flat percentile. The doc comment notes
   a flat 2% percentile is "too conservative: on a shape only a few pixels wide it lands inside
   the anti-aliased ramp and reports a foreground far lighter than the truth"
   (`coverage.rs:200-202`).
3. Averages the RGB of pixels within 15% of that plateau band at each extreme
   (`coverage.rs:211-212`) to get `fg` (darker) and `bg` (lighter).

### Two error-propagation steps

This is the part the module doc comment (`coverage.rs:20-33`) calls out as mattering
downstream, and it is worth reproducing exactly because the rest of the tracer's adaptive
behaviour is a consequence of it, not a separate design choice:

**Step 1 — pixel noise to coverage noise.** Pixel noise of standard deviation `sigma_pixel`
propagates through the projection linearly:

```text
sigma_a = sigma_pixel / |F - B|
```

A faint boundary — small `|F - B|` — is measured badly, and the arithmetic says so directly: the
same noise divided by a smaller contrast gives a larger `sigma_a`.

**Step 2 — coverage noise to positional noise.** The boundary is extracted as the level set
`a = 0.5`. For an implicit curve, positional uncertainty along the gradient direction is

```text
sigma_position = sigma_a / |grad a|
```

A crisp edge has a steep coverage gradient and localizes to a fraction of a pixel; a soft or
blurred edge has a shallow gradient and does not.

Composing the two:

```text
sigma = sigma_pixel / (|F - B| * |grad a|)
```

in pixels. This expression is what feeds the `tau * sigma` admissibility envelope in
`inkvec-fit` (`crates/inkvec-fit/src/lib.rs:16, 49-51, 398-405`): a candidate chord is admissible
only if every measured point lies within `tau * sigma_k` of it. Because `sigma` already contains
the boundary's own measured confidence, "the places we are allowed to simplify hard are exactly
the places we measured badly" (`coverage.rs:32-33`) — adaptive simplification needs no separate
heuristic layered on top.

### `sigma_model` and why noise alone is not enough

`CoverageField::sigma_model` (default `DEFAULT_SIGMA_MODEL = 0.05`, `coverage.rs:39`) is added
in quadrature to the noise-derived sigma in `position_sigma` (`coverage.rs:107-117`):

```rust
noise.hypot(self.sigma_model).clamp(1e-3, MAX_SIGMA)
```

The doc comment on the field (`coverage.rs:50-60`) explains why this exists: propagating pixel
noise alone gives "an absurdly optimistic answer on clean synthetic input" — with zero noise,
`sigma_a` and therefore `sigma_position` tend to zero, and the fit is told it knows the boundary
to a thousandth of a pixel. It does not. Extracting a boundary as the 0.5 level set of a
bilinearly-interpolated field carries its own systematic error: the true shape need not be
exactly representable by the interpolant, and the interpolant is not the true reconstruction
filter. The total is `sqrt(noise^2 + model^2)`, a resolution limit no amount of clean input
removes. The stated derivation: "Measured on analytic circles (see the `inkvec-trace` tests),
level-set extraction lands within roughly 0.05px, which is the default" (`coverage.rs:59-60`).
That test is not in this module's own `#[cfg(test)]` block; it lives in
`crates/inkvec-trace/tests/subpixel.rs`. `recovers_circle_radius_to_sub_pixel_accuracy`
(`subpixel.rs:107-127`) renders an analytic circle, runs it through the full `trace_bilevel`
pipeline (which performs this module's level-set extraction), fits a circle to the recovered
contour, and asserts radius and centre error under `0.1` px;
`accuracy_holds_as_the_shape_shrinks_toward_the_pixel_grid` (`subpixel.rs:154-167`) repeats
this down to a 6px radius with a `0.15` px bound. Both measure the full contour-then-fit
pipeline rather than isolating `sigma_model` alone, and their asserted bounds (`0.1`–`0.15` px)
are a looser envelope than the doc comment's "roughly 0.05px" — consistent with it, not an
independent re-derivation of the exact figure.

`position_sigma` also floors the pure-noise term at `MAX_SIGMA = 4.0` pixels when the gradient
is at or below `1e-6` (`coverage.rs:108-114`) — a plateau where the level set is genuinely
unlocalizable yields a large but finite value, "rather than an infinity that would poison the
fit" (`coverage.rs:106`).

### `saturation` and unidentifiability

`fg` and `bg` are only trustworthy if some pixel in the image is *actually* fully inside the
shape. `bilevel_coverage` measures this geometrically rather than colorimetrically, because a
colorimetric measure would be circular — normalization maps the darkest observation to 1.0 by
construction, so "how many pixels are near 1.0" always answers "plenty" (`coverage.rs:232-234`).
The non-circular question: how many covered pixels (`a >= 0.5`) are *strictly interior*, with
all four orthogonal neighbours also covered (`coverage.rs:237-256`)? For a resolved shape, most
covered pixels are interior; for a sub-pixel stroke, none are, because it never fills a whole
pixel with margin to spare.

```text
saturation = interior / inside      (1.0 if inside == 0)
```

The `saturation` doc comment (`coverage.rs:70-78`) states plainly what low saturation means: the
colour axis is genuinely **unidentifiable**. "A 0.3px black stroke and a 1px grey stroke produce
the same pixels, and no amount of processing separates them." The honest response, and the one
implemented, is not to guess a width and assert it — it is to widen `sigma_alpha` so downstream
stages simplify rather than trust a number that cannot be known:

```rust
let confidence_penalty = (1.0 / saturation.max(0.05)).min(8.0);
sigma_alpha: sigma_pixel / contrast * confidence_penalty,
```

(`coverage.rs:267, 272`). The penalty is capped at 8x and the floor on `saturation` in the
division is `0.05`, so the penalty itself is bounded; **no stated derivation** is given for
either the `0.05` floor or the `8.0` cap specifically (see Open questions).

### `estimate_noise` — MAD of a Laplacian

`estimate_noise(gray, w, h)` (`coverage.rs:211-231`) estimates per-channel pixel noise from a
single grayscale array:

1. Compute the discrete Laplacian at every interior pixel: `4*c - left - right - up - down`
   (`coverage.rs:219-223`), using the kernel named `LAPLACIAN_KERNEL = [4.0, -1.0, -1.0, -1.0,
   -1.0]` (`coverage.rs:172`). The Laplacian annihilates smooth content, so what survives in a
   flat region is noise (`coverage.rs:188`).
2. Take the **median** absolute value, not the mean — "an edge is a large deviation, but a rare
   one", and the median keeps genuine edges from inflating the estimate (`coverage.rs:189-190`).
3. Convert: `mad / MAD_TO_SIGMA / gain`, where `MAD_TO_SIGMA = 0.6745` (`coverage.rs:177`) is the
   standard MAD-to-Gaussian-sigma conversion factor and `gain` is computed, not a literal:
   `LAPLACIAN_KERNEL.iter().map(|c| c * c).sum::<f64>().sqrt()` (`coverage.rs:229`) — the root of
   the sum of the kernel's squared coefficients, `sqrt(4^2 + 1 + 1 + 1 + 1) = sqrt(20)`. The
   result is floored at `NOISE_FLOOR = 0.5/255` (`coverage.rs:184`, `230`).

The kernel's squared-coefficient sum is `20`, not `6`. This matches the doc's own earlier
arithmetic check and is now confirmed by the source itself: the function's doc comment
(`coverage.rs:192-210`) records that until 2026-09-08 the divisor was the literal `sqrt(6)`,
and that this was a bug — `sqrt(6)` is the noise gain of the one-dimensional second difference
`[1, -2, 1]` (whose squared coefficients do sum to 6), not of the four-neighbour 2-D Laplacian
actually used here, which sums to 20. Every noise estimate before the fix was too large by
`sqrt(20/6) = 1.83x`, measured at `1.832x` against synthetic noise of known sigma
(`coverage.rs:202`). Two regression tests now pin this: `the_kernel_constant_matches_the_loop`
(`coverage.rs:992-1002`) asserts the kernel is `[4.0, -1.0, -1.0, -1.0, -1.0]` and that its
computed gain equals `sqrt(20)`; `noise_estimate_recovers_a_known_sigma`
(`coverage.rs:971-989`) regenerates synthetic noise of known sigma and asserts the estimate
recovers it, with a failure message that names the old `1.83x` ratio explicitly as the
regression to watch for. The `0.5/255` floor is
explained by its consumer, not by this function: the code comment beside the floor notes "the
palette divides by it" (`coverage.rs:1007`), pinned by the test
`noise_estimate_floors_at_half_a_level` (`coverage.rs:1004-1011`), so a literal zero would be a
division-by-zero risk two stages downstream.

### `bilevel_coverage`, `intake_scale`, `downsample_to`

`bilevel_coverage(img)` (`coverage.rs:191-278`) is the entry point that ties the above together:
estimate `fg`/`bg`, project every pixel onto the axis to get `data`, measure `saturation`,
estimate `sigma_pixel` via `estimate_noise` on the composited-over-white luminance, and combine
into `sigma_alpha` with the saturation penalty.

`intake_scale(rgb, width, height)` (`coverage.rs:306-381`) answers a different question: how many
raster pixels does one unit of genuine edge detail occupy? Its full derivation and measured
figures are documented in its own doc comment (`coverage.rs:306-336`) and are load-bearing for
stage 03, so they are repeated in `03-palette.md` rather than here. The short version: a ramp of
height `d` over `w` pixels has first difference `d/w` and second difference `~d/w^2`, so the
ratio of first to second difference recovers `w` directly, independent of contrast. The function
takes the **median** of this ratio over pixels that sit on a real edge (`first > EDGE_FLOOR`,
`coverage.rs:339, 369`), so flat interiors and single-pixel noise do not vote, and returns at
least `1.0` (`coverage.rs:335-336, 380`).

`downsample_to(img, nw, nh)` (`coverage.rs:388-431`) area-averages an image down. It exists partly
to serve `oversample_factor`'s round-trip test and partly as a general utility. Its doc comment
(`coverage.rs:384-387`) states the reason it averages in **premultiplied** colour and then
un-premultiplies: averaging straight colour would let a transparent pixel's stored (and
arbitrary) RGB bleed into an opaque neighbour as a dark halo, which downstream "would become a
traced contour that is not in the artwork." The test
`downsample_does_not_bleed_colour_from_transparent_pixels` (`coverage.rs:701-731`) pins this: a
checkerboard of opaque-white and transparent-black pixels downsamples to pure white, not grey.

`oversample_factor(rgb, width, height)` (`coverage.rs:449-524`) answers yet another, distinct
question from `intake_scale`: not how wide an edge is, but whether the raster's pixel count
exceeds what its content needs. It halves the image (box filter), restores it (bilinear), and
compares against the original; if the round-trip error stays under `OVERSAMPLE_TOL = 3.0`
(mean absolute error in 8-bit levels, `coverage.rs:433-447`), nothing was lost and the factor is
credited. Repeated at 2x, 4x, 8x. Its own doc comment explains why it exists *alongside*
`intake_scale` rather than replacing it: `intake_scale` is "the wrong measure for tolerances,
because a super-resolution model defeats it — it returns a sharp edge at high resolution, so the
raster reads as barely oversampled when it carries four times the pixels the drawing needs.
Measured on the incorpo mark upscaled 4x: edge width 2.00, where the answer is 4"
(`coverage.rs:454-457`). `oversample_factor` "asks the question directly instead": an oversampled
raster's surplus pixels can be thrown away and put back losslessly, "indifferent to whether the
surplus pixels are crisp or blurred, asking only whether they say anything" (`coverage.rs:459-463`).

## Constants and thresholds

| name | value | controls | stated derivation |
|---|---|---|---|
| `DEFAULT_SIGMA_MODEL` | `0.05` px | floor added in quadrature to noise-derived positional sigma | "Measured on analytic circles" (`coverage.rs:59-60`); the test lives in `inkvec-trace/tests/subpixel.rs:107-167`, not in this module — see above |
| `MAX_SIGMA` (in `position_sigma`) | `4.0` px | ceiling on positional uncertainty when the gradient vanishes | no stated derivation; described only as "a large but finite value" to avoid poisoning the fit (`coverage.rs:106`) |
| plateau fraction (in `bilevel_coverage`) | `0.001` (0.1%) | how much of the luminance extreme is trusted to define `fg`/`bg` | qualitative only: chosen because a flat 2% percentile "lands inside the anti-aliased ramp" on thin shapes (`coverage.rs:200-202`); no sweep cited for 0.1% specifically |
| extreme-band tolerance | `0.15` (15% of `hi - lo`) | which pixels near each luminance extreme are averaged into `fg`/`bg` | no stated derivation |
| `MAD_TO_SIGMA` | `0.6745`, fixed | MAD-to-Gaussian-sigma conversion in `estimate_noise` | standard statistical constant, stated not derived (`coverage.rs:174-177`) |
| Laplacian noise gain | `sqrt(20)`, computed from `LAPLACIAN_KERNEL` (`coverage.rs:172, 229`) | corrects for the 4-neighbour Laplacian's noise gain in `estimate_noise` | derived and pinned by test: root of the kernel's squared-coefficient sum; a hardcoded `sqrt(6)` (the 1-D second difference's gain) was a bug, fixed 2026-09-08 — see above |
| noise floor | `0.5/255` | minimum `estimate_noise` can return | derivation is downstream: the palette divides by this value (`coverage.rs:1007`, test at `coverage.rs:1004-1011`) |
| `confidence_penalty` floor | `saturation.max(0.05)` | prevents an unbounded penalty when `saturation` is near zero | no stated derivation |
| `confidence_penalty` cap | `8.0` | ceiling on the sigma inflation from low saturation | no stated derivation |
| `EDGE_FLOOR` (`intake_scale`) | `2.0/255` | minimum first difference counted as a real edge, not noise | stated purpose, no numeric derivation (`coverage.rs:339`) |
| `MAX_W` (`intake_scale`) | `64.0` | clamp on a single edge-width observation | stated purpose (guards the degenerate straight-ramp case), no numeric derivation (`coverage.rs:341-342`) |
| `MIN_W` (`intake_scale`) | `0.25` | lower clamp on a single edge-width observation | no stated derivation |
| minimum edge observations | `16` | below this, `intake_scale` returns `1.0` rather than trusting a thin sample | no stated derivation (`coverage.rs:375-377`) |
| `OVERSAMPLE_TOL` | `3.0` (mean abs. error, 8-bit levels) | round-trip error threshold for `oversample_factor` | measured, and the doc comment is explicit it **cannot** cleanly separate native from oversampled on its own: "the lowest native reading at /2 is 1.29 ... while the 4x upscale this exists for reads 2.12 at /4" (`coverage.rs:436-440`); the doc comment states this value is only safe to use downstream of the `intake_scale` gate, never as a gate itself |
| minimum size for `oversample_factor` | `16x16`, and `sw`/`sh >= 8` at each step | avoids measuring on too little data | no stated derivation |

## Failure modes and edge cases

- **Sub-pixel features are unidentifiable, not merely hard.** When every feature in the image is
  narrower than a pixel, `saturation` is low and `fg`/`bg` cannot be trusted as measured — see
  the `saturation` discussion above. The module's answer is to inflate uncertainty, not to guess.
- **Ringing is invisible to both detectors in this module.** `intake_scale` measures edge
  *width*; JPEG and other lossy codecs do not widen edges, they add oscillation in flat regions
  beside them. `estimate_noise` is a **median** Laplacian, and ringing occupies a band next to
  the edge while most of the image stays flat — the median never leaves its floor. The test
  `ringing_does_not_widen_an_edge` (`coverage.rs:641-677`) pins both blind spots deliberately: on
  a synthetic ringing edge, `intake_scale` reads under `SOFT_INTAKE_EDGE_FOR_TEST = 1.75`
  (`coverage.rs:661-666`), and `estimate_noise` reads exactly the `0.5/255` floor on both the
  ringing image and a clean control (`coverage.rs:673-677`). This is documented as a **real
  regression**: "The regression this module let through on 2026-09-08... The palette's noise
  guard was gated on this function alone, so a compressed logo measured a native 1.15 px, the
  guard stayed off, and the tracer fitted the encoder's ringing as artwork" (`coverage.rs:633-640`).
  The fix was to stop asking `intake_scale` a question it cannot answer and add a second,
  file-level signal — `crate::lossy_container` (`lib.rs:94-113`), which reads the container
  format itself (JPEG always lossy; WebP's `VP8 `/`VP8L` RIFF tag; PNG/GIF/BMP/TIFF always
  lossless) rather than trying to detect damage from pixels. Two pixel-level detectors were tried
  first and both failed for the stated reason: "the Laplacian of a lossily-coded flat region and
  the Laplacian of a cleanly-rendered 8-bit colour ramp are the same size... A clean radial
  gradient measured a *higher* 'damage' score than a q50 flat icon" (`lib.rs:87-89`).
- **A single JPEG overshoot or stray speck cannot hijack the colour axis** — the plateau
  percentile in `bilevel_coverage` exists specifically to prevent this (`coverage.rs:199-202`).
- **A transparent pixel's stored colour cannot bleed into a downsample** — see the halo-bug
  discussion above and its pinned test.
- **`oversample_factor` is not a substitute for `intake_scale`, and vice versa**, and the doc
  comments on each say so explicitly: `oversample_factor` catches an SR model producing a sharp
  edge on surplus pixels; `intake_scale` catches a soft or blurred edge, which
  `oversample_factor`'s round-trip test alone would not reliably flag as oversampled versus
  genuinely band-limited native content (`coverage.rs:436-440`).

## Environment overrides

No `INKVEC_*` environment variable is read directly inside `coverage.rs` (confirmed: no
`env::var` call anywhere in the file). Callers downstream (`color.rs`, `lib.rs`) read several
(`INKVEC_NOISE_SIGMAS`, `INKVEC_SAME_INK_DE00`, `INKVEC_PALDBG`) that consume this module's
outputs — see `03-palette.md`.

## Open questions

- **`DEFAULT_SIGMA_MODEL`'s analytic-circle test — resolved.** `coverage.rs`'s own
  `#[cfg(test)] mod tests` has no test of `sigma_model` against a circle, but the test does
  exist, in `inkvec-trace/tests/subpixel.rs:107-167` — see the `sigma_model` discussion above.
- **The Laplacian noise-gain factor — resolved.** It is no longer `sqrt(6)`. The 4-neighbour
  Laplacian kernel used here, `LAPLACIAN_KERNEL = [4.0, -1.0, -1.0, -1.0, -1.0]`
  (`coverage.rs:172`), has squared-coefficient sum `20`, and the code now computes its gain as
  `sqrt(20)` directly from the kernel (`coverage.rs:229`) rather than hardcoding a constant. A
  hardcoded `sqrt(6)` — the gain of the unrelated 1-D second difference `[1, -2, 1]` — was
  shipped until 2026-09-08 and inflated every noise estimate by `1.83x`; see the
  `estimate_noise` discussion above for the fix and its regression tests.
- **`confidence_penalty`'s cap of `8.0` and floor of `0.05`** (`coverage.rs:267`) have no stated
  derivation — no sweep or measured figure is cited, unlike almost every other constant in this
  module.
- **The `0.1%` plateau, `15%` extreme-band tolerance, `EDGE_FLOOR = 2.0/255`, `MAX_W = 64.0`,
  `MIN_W = 0.25`, and the `16`-observation minimum in `intake_scale`** are all qualitatively
  justified (what would go wrong without them) but none carries a specific measured value the
  way `SOFT_INTAKE_EDGE` or `OVERSAMPLE_TOL` do. These read as reasonable engineering guesses
  rather than swept constants.
- **The colour path (`trace_color_full_with_alpha`) never constructs a `CoverageField`.** It
  independently calls `estimate_noise` (`lib.rs:269`) and `intake_scale` (`lib.rs:282`), and
  `planar::refine_subpixel` (`planar.rs:660-699`) re-derives the `sigma_pixel / contrast / |grad
  a|` formula and adds `DEFAULT_SIGMA_MODEL` in quadrature by hand, rather than sharing this
  module's `CoverageField::position_sigma` method. The two implementations are consistent today,
  but nothing enforces that they stay so — a change to `position_sigma` would not automatically
  propagate to the colour path's copy in `planar.rs`.
