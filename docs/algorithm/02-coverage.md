# Stage 02 — Coverage

> Turns an anti-aliased raster into a scalar coverage field with an explicit, per-pixel
> uncertainty, instead of a bitmap to be thresholded.

**Source:** `crates/inkvec-trace/src/coverage.rs`, with `coverage/oversample.rs` and
`coverage/resample.rs` (submodules re-exported from `coverage`, `coverage.rs:58-61`)
**Entry points:** `bilevel_coverage()` (`coverage.rs:433`), `estimate_noise()` (`coverage.rs:356`),
`intake_scale()` (`coverage.rs:832`), `ringing_score()` (`coverage.rs:627`),
`oversample_factor()` (`coverage/oversample.rs:118`), `downsample_to()` (`coverage/resample.rs:31`)
**Pipeline position:** the intake measurements every later stage is tuned by (the module's own
list, `coverage.rs:35-51`). The bilevel front end (`trace_bilevel`, `inkvec-trace/src/lib.rs:136-142`)
calls `bilevel_coverage` directly and traces its field. The Quality colour front end
(`trace_color_full_with_alpha`, `inkvec-trace/src/lib.rs:285`) does not build a `CoverageField` — it
takes the noise estimate (`estimate_noise`, `inkvec-trace/src/lib.rs:311`), the edge width
(`intake_scale`, `inkvec-trace/src/lib.rs:344`) and the ringing score (`ringing_score`,
`inkvec-trace/src/lib.rs:349`) and feeds them into palette extraction (stage 03); the native-alpha
path makes the same three measurements (`native.rs:824-829`). Later, `planar::refine_subpixel`
(`planar.rs:445`) computes each boundary point's uncertainty in `vertex_sigma`
(`planar.rs:1198-1256`), which re-derives the coverage-gradient formula this module documents
rather than sharing a `CoverageField` object. Fast mode measures none of the three: its front end
hands the shared stages `NOISE_FLOOR` as the noise (`fast/front.rs:152-163`). `inkvec-cli`'s intake
reads `intake_scale` and `oversample_factor` to price its options in the raster's own units
(`price_in_raster_units`, `inkvec-cli/src/lib.rs:328-453`) and resamples with `downsample_to`.
There is no stopwatch mark named `coverage`. In the Quality path the stopwatch starts after the
noise estimate (`inkvec-trace/src/lib.rs:333`), so that estimate falls under no mark, while the
edge-width and ringing measurements fall inside the `palette` mark (`inkvec-trace/src/lib.rs:409`).

## What problem this solves

An anti-aliased pixel at the edge of a shape is not noise, and it is not a blurry copy of a
sharp original. It is a **measurement**: the fraction of that pixel's area the foreground
colour covers. The module doc comment (`coverage.rs:1-33`) states the stakes plainly — reading
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

**Input:** `Rgba` (`coverage.rs:174-183`) — straight (unpremultiplied) RGBA floats in `[0, 1]`,
row-major, four floats per pixel.

**Output:** `CoverageField` (`coverage.rs:83-125`):

| field | type | meaning |
|---|---|---|
| `width`, `height` | `usize` | dimensions |
| `data` | `Vec<f32>` | coverage `a` at each pixel centre, `[0, 1]` |
| `sigma_alpha` | `f64` | standard deviation of the coverage estimate, from pixel noise |
| `sigma_model` | `f64` | irreducible positional error of level-set extraction itself, in pixels |
| `fg`, `bg` | `[f32; 3]` | the sRGB foreground/background colours coverage was measured against |
| `saturation` | `f64` | confidence, in `[0, 1]`, that `fg`/`bg` were actually observed |

Two derived methods matter downstream:

- `gradient_magnitude(x, y)` (`coverage.rs:142-152`) — central-difference `|grad a|` at the pixel
  nearest `(x, y)`, no interpolation; neighbours outside the field are clamped to its edge.
- `position_sigma(p)` (`coverage.rs:154-171`) — positional uncertainty of a boundary point, in
  pixels: the quantity every later stage of the bilevel path actually consumes (through
  `contour.rs:302-313`).

The module also provides four image-wide measurements that are not part of the field:
`estimate_noise` (pixel noise), `intake_scale` (edge width), `ringing_score` (compression
ringing) and `oversample_factor` (surplus pixels). Each is described below.

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
channels is deliberate, not incidental. In code this is `bilevel_coverage`'s central map
(`coverage.rs:487-493`):

```rust
let n = (p[0] - bg[0]) * d[0] + (p[1] - bg[1]) * d[1] + (p[2] - bg[2]) * d[2];
(n as f64 / dd).clamp(0.0, 1.0) as f32
```

where `d = F - B` and `dd = |F - B|^2`.

### Estimating `F` and `B`

`F` and `B` cannot simply be the darkest and lightest pixels in the image — a single JPEG
overshoot or stray speck would then define the colour axis for the whole image
(`coverage.rs:419-423`, `coverage.rs:441-442`). `bilevel_coverage` instead:

1. Composites the image over white and computes luminance
   `0.2126*R + 0.7152*G + 0.0722*B` (`coverage.rs:435-439`, Rec. 709 coefficients).
2. Sorts luminance and takes a small plateau — `0.1%` of pixels, clamped to `[1, max(n-1, 1)]`
   (`coverage.rs:460-462`) — from each extreme, rather than a flat percentile. The code comment
   notes a flat 2% percentile is "too conservative: on a shape only a few pixels wide it lands
   inside the anti-aliased ramp and reports a foreground far lighter than the truth"
   (`coverage.rs:442-444`).
3. Averages the RGB of the pixels whose luminance lies within 15% of the range `hi - lo` of each
   extreme (`coverage.rs:464-466`, `mean_rgb_where` at `coverage.rs:549-577`) to get `fg` (darker)
   and `bg` (lighter). If no pixel qualifies, the fallback is black for `fg` and white for `bg`.

Two degenerate inputs are answered rather than divided by zero: an empty image returns an
empty field (`coverage.rs:448-459`), and a uniform one (`F == B`) returns an all-zero field with
`sigma_alpha = 0` and `saturation = 1` (`coverage.rs:470-484`), which used to put NaN in every
sample. Both are pinned by tests (`coverage.rs:1120-1146`).

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

in pixels. This is the per-point `σ_k` of the fitter's objective in `inkvec-fit`,
`E = ½·χ² + λ·P` with `χ² = Σ_k (d_k / σ_k)²` (`inkvec-fit/src/lib.rs:9-20`): a point's miss is
weighed against its own measured uncertainty. Because `sigma` already contains the boundary's own
measured confidence, "the places we are allowed to simplify hard are exactly the places we
measured badly" (`coverage.rs:32-33`) — adaptive simplification needs no separate heuristic
layered on top.

The module doc comment says the same: "the per-point sigma of `inkvec-fit`'s chi-squared term,
each miss weighed by `1/sigma^2`" (`coverage.rs:30-31`). It used to word this as feeding "the
`tau * sigma` admissibility envelope in `inkvec-fit`". That per-point straightness test
(`|d_k| <= tau * sigma_k`, described at `inkvec-fit/src/lib.rs:74-80`) is no longer how the
shipping fitter works: the cone
that implemented it was removed as a correctness bug, and the test survives only as the reference
`is_admissible` (`inkvec-fit/src/lib.rs:530-549`), which "nothing in the shipping path calls"
(`inkvec-fit/src/lib.rs:92-98`). `tau` still scales tolerances elsewhere in the fitter, for
example the corner test of the decimation grid (`r_j > τ²`, `inkvec-fit/src/decimate.rs:21-28`).

### `sigma_model` and why noise alone is not enough

`CoverageField::sigma_model` (default `DEFAULT_SIGMA_MODEL = 0.05`, `coverage.rs:63-81`) is added
in quadrature to the noise-derived sigma in `position_sigma` (`coverage.rs:161-171`):

```rust
noise.hypot(self.sigma_model).clamp(1e-3, MAX_SIGMA)
```

The doc comment on the field (`coverage.rs:94-106`) explains why this exists: propagating pixel
noise alone gives "an absurdly optimistic answer on clean synthetic input" — with zero noise,
`sigma_a` and therefore `sigma_position` tend to zero, and the fit is told it knows the boundary
to a thousandth of a pixel. It does not. Extracting a boundary as the 0.5 level set of a
bilinearly-interpolated field carries its own systematic error: the true shape need not be
exactly representable by the interpolant, and the interpolant is not the true reconstruction
filter. The total is `sqrt(noise^2 + model^2)`, a resolution limit no amount of clean input
removes. The stated derivation: "Measured on analytic circles (see the `inkvec-trace` tests),
level-set extraction lands within roughly 0.05px, which is the default" (`coverage.rs:104-105`).
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

The constant's own doc comment (`coverage.rs:66-80`) records how much it decides. Combined in
quadrature in `planar.rs`, it dominates on the corpus: "79% of the gate set's 86,060 boundary
points come out between 0.050 and 0.060", so for most of a traced image the fitter's tolerance is
this constant. It is not an independent dial, though: chi² weights are `1/sigma²`, so scaling
every sigma by `k` is exactly `lambda -> k²·lambda`, and the measurement agrees — raising it to
0.10 moved the gate to dE00 +39.00% / ratio −10.28%, and `--lambda-scale 4.0`, the algebraically
equivalent change, to +44.34% / −11.68%, "the same frontier". Fidelity against compactness is
therefore tuned with `--lambda-scale`; this constant is documented as a property of the
extraction method.

When the gradient is at or below `1e-6`, `position_sigma` sets the noise term to
`MAX_SIGMA = 4.0` pixels (`coverage.rs:162-168`), and the total is clamped to `[0.001, 4]` px
(`coverage.rs:170`): a plateau where the level set is genuinely unlocalizable yields "the 4 px
ceiling rather than an infinity that would poison the fit" (`coverage.rs:158-160`).

### `saturation` and unidentifiability

`fg` and `bg` are only trustworthy if some pixel in the image is *actually* fully inside the
shape. `bilevel_coverage` measures this geometrically rather than colorimetrically, because a
colorimetric measure would be circular — normalization maps the darkest observation to 1.0 by
construction, so "how many pixels are near 1.0" always answers "plenty" (`coverage.rs:501-503`).
The non-circular question: how many covered pixels (`a >= 0.5`) are *strictly interior*, with
all four orthogonal neighbours also covered (`coverage.rs:506-525`; a neighbour outside the image
counts as uncovered)? For a resolved shape, most covered pixels are interior; for a sub-pixel
stroke, none are, because it never fills a whole pixel with margin to spare.

```text
saturation = interior / inside      (1.0 if inside == 0)
```

The `saturation` doc comment (`coverage.rs:116-124`) states plainly what low saturation means: the
colour axis is genuinely **unidentifiable**. "A 0.3px black stroke and a 1px grey stroke produce
the same pixels, and no amount of processing separates them." The honest response, and the one
implemented, is not to guess a width and assert it — it is to widen `sigma_alpha` so downstream
stages simplify rather than trust a number that cannot be known:

```rust
let confidence_penalty = (1.0 / saturation.max(0.05)).min(8.0);
sigma_alpha: sigma_pixel / contrast * confidence_penalty,
```

(`coverage.rs:536`, `coverage.rs:541`). The penalty is capped at 8x and the floor on `saturation`
in the division is `0.05`, so the penalty itself is bounded; **no stated derivation** is given for
either the `0.05` floor or the `8.0` cap specifically (see Open questions).

### `estimate_noise` — a low quantile of the Laplacian

`estimate_noise(gray, w, h)` (`coverage.rs:319-358`, body in `estimate_noise_at`,
`coverage.rs:360-397`) estimates per-channel pixel noise from a single grayscale array:

```text
sigma = max(Q_q(|L|) / z_q / sqrt(Σ k_i²), NOISE_FLOOR)
```

(`coverage.rs:330-332`), in three steps:

1. Compute the discrete Laplacian at every interior pixel: `4*c - left - right - up - down`
   (`coverage.rs:368-379`), the kernel named `LAPLACIAN_KERNEL = [4.0, -1.0, -1.0, -1.0, -1.0]`
   (`coverage.rs:300-306`). The Laplacian annihilates smooth content, so what survives in a flat
   region is noise (`coverage.rs:323`).
2. Take a **low quantile** of the absolute values, the 10th percentile (`NOISE_QUANTILE = 0.10`,
   `coverage.rs:409-410`), read by selection at index `floor(n·0.10)` (`coverage.rs:393-394`,
   `kth_smallest` at `coverage.rs:399-407`). It used to be the median (MAD), and the reason it
   changed is recorded beside the read (`coverage.rs:380-392`): the median is robust only while
   edges are rare. "On a labyrinth of three-pixel strokes the median |Laplacian| IS an edge
   response: 53 display levels of 'noise' where the trace's own flat interiors say 0.57", and
   since the palette's same-ink test and the fit tolerance divide by this, "the trace ran two
   orders of magnitude too loose and strokes merged into blobs". The tenth percentile is still
   noise while a tenth of the image is flat. Measured over 246 icons at 128, 512 and 1024 px,
   every output was byte-identical, because on clean art both readings sit on the floor; JPEG
   and added grain improve. The test `a_noiseless_edge_dense_picture_reports_no_noise`
   (`coverage.rs:1206-1227`) reproduces it with three-pixel stripes: the estimate must stay under
   2 levels, and the median reading is checked to read over 10.
3. Convert: divide by `Z10 = 0.12566` (`coverage.rs:412-415`), the 10% point of a unit
   half-normal (`Phi^-1(0.55)`), which plays the role `MAD_TO_SIGMA = 0.6745` played for the
   median (that constant is now test-only, `coverage.rs:308-310`), and by the kernel's noise
   gain, computed rather than written as a literal:
   `LAPLACIAN_KERNEL.iter().map(|c| c * c).sum::<f64>().sqrt()` (`coverage.rs:395`) — the root of
   the sum of the kernel's squared coefficients, `sqrt(4^2 + 1 + 1 + 1 + 1) = sqrt(20)`. The
   result is floored at `NOISE_FLOOR = 0.5/255` (`coverage.rs:396`, constant at
   `coverage.rs:312-317`). An image smaller than 3x3, or a buffer shorter than `w * h`, returns
   `1/255` (`coverage.rs:365-367`).

The kernel's squared-coefficient sum is `20`, not `6`, and the function's doc comment
(`coverage.rs:342-355`) records why the gain is computed: until 2026-09-08 the divisor was the
literal `sqrt(6)`, and that was a bug — `sqrt(6)` is the noise gain of the one-dimensional second
difference `[1, -2, 1]` (whose squared coefficients do sum to 6), not of the four-neighbour 2-D
Laplacian actually used here, which sums to 20. Every noise estimate before the fix was too large
by `sqrt(20/6) = 1.83x`, measured at `1.832x` against synthetic noise of known sigma
(`coverage.rs:346-348`). The error stayed invisible because `NOISE_FLOOR` binds across the whole
corpus: "clean renders and JPEG-damaged files alike measure exactly `0.50/255`"
(`coverage.rs:350-352`). Two regression tests pin it: `the_kernel_constant_matches_the_loop`
(`coverage.rs:1229-1240`) asserts the kernel is `[4.0, -1.0, -1.0, -1.0, -1.0]`, sums to zero, and
has gain `sqrt(20)`; `noise_estimate_recovers_a_known_sigma` (`coverage.rs:1176-1204`) generates
Gaussian noise of 2, 5 and 12 levels on a 400x400 image and asserts the estimate is within
`[0.9, 1.1)` of the truth, with a failure message that names the old `1.83x` ratio explicitly.

The `0.5/255` floor is stated on the constant itself (`coverage.rs:312-317`): "callers divide by
it: `color::extract_palette_mdl` tests `(nearest / sigma_noise)^2`. Half a quantisation step is
the smallest deviation an 8-bit file could even represent." The test
`noise_estimate_floors_at_half_a_level` (`coverage.rs:1242-1249`) pins it. The Quality colour path
reports a floored estimate as a saturated measurement (`inkvec-trace/src/lib.rs:313-330`), and on a
soft intake later raises the noise to the residual measured against the labels
(`regularize::residual_sigma`, `inkvec-trace/src/lib.rs:429-491`; see stage 03).

### `ringing_score` — compression ringing, read from the pixels

`ringing_score(rgb, width, height)` (`coverage.rs:579-713`) fills the gap that both of its
neighbours disclaim (`coverage.rs:581-591`): `intake_scale` reads 1.00 on a JPEG because
compression adds ringing rather than width, and `estimate_noise` cannot see it either, because an
icon is mostly empty and its low quantile of the Laplacian is zero. "Measured over 150 rendered
SVGs at four qualities, the shipped estimate is the same constant 0.00196 for a clean render, a
quality-85 JPEG and a quality-35 JPEG, while the true deviation from the clean original rises
0.0000, 0.0042, 0.0083, 0.0115."

The statistic is *where* the energy sits: anti-aliasing hugs a boundary and dies within a pixel
or two, while ringing sits in a band a few pixels out, where clean vector art is exactly flat
(`coverage.rs:593-595`). With `L` the 4-neighbour Laplacian of Rec. 709 luminance and `d` the
chamfer 3-4 distance to the nearest pixel whose gradient exceeds 24/255
(`chamfer_distance_to_edges`, `coverage.rs:715-788`):

```text
    core  = { |L_i| : d_i <= 1 px }         ring = { |L_i| : 3 px < d_i <= 7 px }
    ratio = P90(ring) / P50(core)
    hot   = ring pixels with |L| > P60(ring)
    score = ratio · (#hot right/down neighbour pairs whose L changes sign) / (#hot pairs)
```

(`coverage.rs:613-621`, constants at `coverage.rs:628-636`). The ratio is dimensionless and
stays monotone in compression where raw ring energy did not: measured 0.000 clean, 0.179 at q85,
0.215 at q60, 0.238 at q35 (`coverage.rs:600-604`). The ratio alone fires on 24% of clean brand
logos, because a gradient puts real signal in the ring, so it is multiplied by the sign
alternation rate (Gibbs ringing oscillates pixel to pixel, smooth artwork does not); "at the
shipped threshold it takes clean brand logos from 19% to 3.8% while still catching 82 to 91% of
JPEG. Measured across benchmark datasets" (`coverage.rs:606-611`). It returns 0 — the safe answer,
since the guard it feeds is only switched on by positive evidence — with no edge, under 9x9, a
short buffer, or fewer than 64 samples in either set or 64 hot pairs (`coverage.rs:623-626`). The
thresholds it is compared against (`SOFT_RINGING`, `SOFT_RINGING_LARGE`, `RINGING_MIN_DIM`) are
the palette's, `color.rs:419-452` (stage 03). Tests: `ringing_is_seen_in_the_ring_and_nowhere_else`
(`coverage.rs:956-1001`) and `ringing_score_is_safe_on_degenerate_input` (`coverage.rs:1003-1012`).

### `bilevel_coverage` and `intake_scale`

`bilevel_coverage(img)` (`coverage.rs:417-547`) is the entry point that ties the above together:
estimate `fg`/`bg`, project every pixel onto the axis to get `data`, measure `saturation`,
estimate `sigma_pixel` via `estimate_noise` on the composited-over-white luminance
(`coverage.rs:532`), and combine into `sigma_alpha` with the saturation penalty.

`intake_scale(rgb, width, height)` (`coverage.rs:790-882`) answers a different question: how many
raster pixels does one unit of genuine edge detail occupy? Its doc comment (`coverage.rs:793-799`)
gives the reason it is not the image size: a native render at 1024 traces in 1.67 s and finds 29
regions, the same content upsampled from 128 to 1024 takes 55.6 s and finds 1207. A ramp of height
`d` over `w` pixels has first difference `d/w` and second difference `~d/w^2`, so the ratio of
first to second difference recovers `w` directly, independent of contrast (`coverage.rs:807-811`).
Concretely, over every run of three pixels along a row or a column of the channel mean
`(r+g+b)/3` (`coverage.rs:813-822`):

```text
    w_obs = max(|d0|, |d1|) / |d1 − d0|     kept when max(|d0|, |d1|) > 2/255
    scale = max(1, median(clamp(w_obs, 0.25, 64)))
```

so flat interiors and single-pixel noise do not vote (`EDGE_FLOOR`, `coverage.rs:833-834`), a
straight ramp (`d1 = d0`) is skipped, fewer than 16 votes returns 1.0 (`coverage.rs:876-878`),
and the answer is at least `1.0` (`coverage.rs:881`). Measured behaviour (`coverage.rs:824-828`):
exactly 1.00 for native renders at 128, 512 and 1024; 2.39 and 4.00 for 4x and 8x Lanczos
upsamples; 1.70 and 3.00 for Gaussian blur of 1.0 and 2.0; JPEG reads 1.00. Its consumers are the
palette's soft-intake gate (`SOFT_INTAKE_EDGE = 1.75`, stage 03), `--intake-scale`
(`normalise_intake`, `inkvec-cli/src/lib.rs:123-175`) and the edge-width gate in
`price_in_raster_units` (`inkvec-cli/src/lib.rs:369-388`).

### `downsample_to` and `box_downsample_rgba8`

`downsample_to(img, nw, nh)` (`coverage/resample.rs:16-41`) area-averages an image down: an exact
box filter, each target pixel the area-weighted average of the source pixels under its footprint,
with fractional weights on the boundary pixels (`box_resample`, `coverage/resample.rs:79-183`). It
lives in the `resample` submodule with its 8-bit twin `box_downsample_rgba8`
(`coverage/resample.rs:43-77`), which reads the decoder's buffer directly for the decode-time
`--max-dim` cap (`load.rs:443`, `load.rs:500`, `load.rs:552`). Its other callers are `inkvec-cli`'s intake: the unblock pre-pass
(`inkvec-cli/src/lib.rs:277`), `--intake-scale` (`inkvec-cli/src/lib.rs:169`) and `--max-dim`
(`inkvec-cli/src/lib.rs:313`). `oversample_factor` does not use it; it box-averages its own `k x k`
blocks (`coverage/oversample.rs:129-149`).

Its doc comment (`coverage/resample.rs:23-25`) states why it averages in **premultiplied** colour
and then un-premultiplies: averaging straight colour would let a transparent pixel's stored (and
arbitrary) RGB bleed into an opaque neighbour as a dark halo, which downstream "would become a
traced contour that is not in the artwork." The test
`downsample_does_not_bleed_colour_from_transparent_pixels` (`coverage/resample.rs:200-231`) pins
this: a 4x4 image of alternating opaque-white and transparent-black pixels downsamples to 2x2 with
every colour channel above 0.99, not grey.

### `oversample_factor` — the round trip

`oversample_factor(rgb, width, height)` (`coverage/oversample.rs:81-180`) answers yet another,
distinct question from `intake_scale`: not how wide an edge is, but whether the raster's pixel
count exceeds what its content needs. It moved out of `coverage.rs` unchanged on 2026-10-02, with
its constants and tests, and is re-exported at the same public path (`coverage/oversample.rs:1-8`).
Its doc comment explains why it exists *alongside* `intake_scale` rather than replacing it:
`intake_scale` is "the wrong measure for *tolerances*, because a super-resolution model defeats
it -- it returns a sharp edge at high resolution, so the raster reads as barely oversampled when
it carries four times the pixels the drawing needs. Measured on a real brand mark upscaled 4x:
edge width 2.00, where the answer is 4" (`coverage/oversample.rs:86-90`). `oversample_factor` "asks
the question directly instead": an oversampled raster's surplus pixels can be thrown away and put
back, "indifferent to whether the surplus pixels are crisp or blurred, asking only whether they
say anything" (`coverage/oversample.rs:92-96`).

For `k` in 2, 4, 8 (`coverage/oversample.rs:98-105`): box-average `k x k` blocks (the trailing
`width mod k` columns and rows are dropped), resample back to full size bilinearly — pixel centre
`x` maps to `(x + 0.5)/k − 0.5` in the small image, clamped to its edge — and take the mean
absolute error over all pixels and channels, in 8-bit levels. A `k` passes when that error

1. stays under `OVERSAMPLE_TOL = 3.0` (`coverage/oversample.rs:10-24`, test at
   `coverage/oversample.rs:169-171`), **and**
2. is at most `OVERSAMPLE_KEEP = 0.5` of `flat_error`, the error of erasing everything but the
   per-channel median colour (`coverage/oversample.rs:26-53`, test at
   `coverage/oversample.rs:172-176`).

The answer is the largest `k` that passes with every smaller `k` also passing. The search stops
once the small image would be under 8 px on a side, and anything under 16x16 (or a buffer shorter
than `width * height`) returns 1 (`coverage/oversample.rs:119-128`).

`flat_error` (`coverage/oversample.rs:55-79`) is the yardstick of the second test:

```text
flat_error = Σ_p Σ_c |rgb_p,c − m_c| · 255 / (3 · width · height),   m_c = per-channel median
```

The median minimises the mean absolute error over constants, so this is how much an image loses
when everything but its one most typical colour is erased. It is 0 for a flat image, is computed
once (when the first `k` passes the absolute test, `coverage/oversample.rs:173`), and costs one
O(n) selection per channel.

**Why the second test exists.** `OVERSAMPLE_TOL` is a mean over every pixel, so it is diluted by
flat area (`coverage/oversample.rs:29-34`): "a 38 px² disc on a 144 px canvas, erased outright,
costs 0.47 levels against the 3.0 allowed, so it read as 8x oversampled, the speckle floor went up
64x to 128 px² and the disc was removed." A round trip that keeps the drawing loses a small share
of it; one that erases a shape loses about all of it. The margins, measured 2026-10-02 as the
round-trip error over the flat error at every factor the absolute test accepts
(`coverage/oversample.rs:35-44`):

- the corpus icons of the screen, held_a and held_b sets at 512 and 1024 px read at most 0.31 and
  0.17 (both `twemoji/1f7eb`, a square that fills the canvas), every other icon at most 0.18; at
  256 px the same square reads 0.56 at /8 and is the one corpus icon the rule moves (8x to 4x);
- 897 stress and test inputs over 128 px move only in three variants of that square;
- the 38 px² disc reads 0.36 at /2, 0.77 at /4 and 1.60 at /8, so it is 2x, not 8x.

The second test only ever lowers the answer, and on a flat image both errors are 0 and the answer
is unchanged (`coverage/oversample.rs:107-111`). The method is labelled in the code
(`coverage/oversample.rs:46-52`): **Inspired by** the relative error measures of forecast
evaluation, a method's error divided by a benchmark method's (Hyndman, R. J. & Koehler, A. B.
(2006), "Another look at measures of forecast accuracy", *International Journal of Forecasting*
22(4):679-688, doi:10.1016/j.ijforecast.2006.03.001). Here the method is the round trip and the
benchmark the best constant image under the same absolute error, the per-channel median. "The
half is our choice: a majority of the detail kept, with the margins above on both sides."

Tests: `a_lone_small_shape_is_not_read_as_eight_times_oversampled`
(`coverage/oversample.rs:200-213`) asserts the native 38 px² disc reads at most 2, the same kind of
disc upscaled 4x still reads at least 4, and a flat 64x64 image reads 8;
`flat_error_is_the_mean_deviation_from_the_median` (`coverage/oversample.rs:215-225`);
`a_native_raster_is_not_oversampled` and `an_upscaled_raster_is_oversampled`
(`coverage.rs:1313-1325`); and end to end, `a_lone_small_shape_on_a_near_empty_canvas_is_drawn`
(`inkvec-cli/tests/pipeline.rs:343-364`).

It returns 1 for most native renders at the corpus's 128 px (212 of the 246 screen icons); at
512 px nearly every native render reads 2 to 8, "which is the speckle floor scaling that caller
wants" (`coverage/oversample.rs:113-117`). Its callers (`coverage/oversample.rs:4-5`) are
`inkvec-cli`'s `price_in_raster_units` (`inkvec-cli/src/lib.rs:328-453`, the call at
`inkvec-cli/src/lib.rs:382`) and `--content-units` (`content_scale`,
`inkvec-cli/src/units.rs:33-78`, the call at `inkvec-cli/src/units.rs:76`). In
`price_in_raster_units`, with `r` the factor and `R = 128` px the reference extent, precision is
scaled by `r` only when the edge width exceeds the soft-intake threshold, min-area by `r²` and
lambda by `r` only when the longest side exceeds `R` (`inkvec-cli/src/lib.rs:328-341`); Fast mode
reads neither measurement (`inkvec-cli/src/lib.rs:360-366`).

## Constants and thresholds

| name | value | controls | stated derivation |
|---|---|---|---|
| `DEFAULT_SIGMA_MODEL` | `0.05` px | floor added in quadrature to noise-derived positional sigma | "Measured on analytic circles" (`coverage.rs:104-105`); the test lives in `inkvec-trace/tests/subpixel.rs:107-167`, not in this module — see above. Its doc (`coverage.rs:66-80`) adds that 79% of 86,060 gate boundary points sit at 0.050-0.060, and that changing it is a lambda change (0.10 ≈ `--lambda-scale 4.0`) |
| `MAX_SIGMA` (in `position_sigma`) | `4.0` px; total clamped to `[0.001, 4]` | ceiling on positional uncertainty when the gradient vanishes | no stated derivation; described only as a ceiling "rather than an infinity that would poison the fit" (`coverage.rs:158-162`) |
| plateau fraction (in `bilevel_coverage`) | `0.001` (0.1%) | how much of the luminance extreme is trusted to define `fg`/`bg` | qualitative only: chosen because a flat 2% percentile "lands inside the anti-aliased ramp" on thin shapes (`coverage.rs:441-444`, used at `coverage.rs:460`); no sweep cited for 0.1% specifically |
| extreme-band tolerance | `0.15` (15% of `hi - lo`) | which pixels near each luminance extreme are averaged into `fg`/`bg` | no stated derivation (`coverage.rs:465-466`) |
| `LAPLACIAN_KERNEL` | `[4, -1, -1, -1, -1]` | the noise estimate's 4-neighbour kernel | the loop's own arithmetic, pinned by test (`coverage.rs:300-306`, `coverage.rs:1229-1240`) |
| Laplacian noise gain | `sqrt(20)`, computed from `LAPLACIAN_KERNEL` (`coverage.rs:395`) | corrects for the 4-neighbour Laplacian's noise gain in `estimate_noise` | derived and pinned by test: root of the kernel's squared-coefficient sum; a hardcoded `sqrt(6)` (the 1-D second difference's gain) was a bug, fixed 2026-09-08 — see above |
| `NOISE_QUANTILE` | `0.10` | the quantile of the absolute Laplacian read as the noise level | measured: the median read 53 levels on a noiseless labyrinth where flat interiors say 0.57; 246 icons at 128/512/1024 px byte-identical after the change (`coverage.rs:380-392`, `coverage.rs:409-410`) |
| `Z10` | `0.12566` | divides the 10th percentile to give a Gaussian sigma | derived: `Phi^-1(0.55)`, the 10% point of the unit half-normal (`coverage.rs:412-415`) |
| `MAD_TO_SIGMA` | `0.6745`, test-only | the old median-to-sigma conversion, kept for the regression test | standard statistical constant (`coverage.rs:308-310`) |
| `NOISE_FLOOR` | `0.5/255` | minimum `estimate_noise` can return | half an 8-bit quantisation step; callers divide by it (`coverage.rs:312-317`, test at `coverage.rs:1242-1249`) |
| short-input noise | `1/255` | what `estimate_noise` returns under 3x3 or for a short buffer | no stated derivation (`coverage.rs:365-367`) |
| `confidence_penalty` floor | `saturation.max(0.05)` | prevents an unbounded penalty when `saturation` is near zero | no stated derivation (`coverage.rs:536`) |
| `confidence_penalty` cap | `8.0` | ceiling on the sigma inflation from low saturation | no stated derivation (`coverage.rs:536`) |
| `EDGE_FLOOR` (`ringing_score`) | `24/255` | gradient above which a pixel is an edge the ring is measured around | stated purpose, no numeric derivation (`coverage.rs:628-629`) |
| core / ring band (`ringing_score`) | core ≤ 1 px; ring 3–7 px | where anti-aliasing and ringing are read | the ring is fixed in pixels because ringing comes from an 8x8 DCT block and does not scale (`coverage.rs:630-634`; reason at `color.rs:436-440`) |
| `MIN_SAMPLES` (`ringing_score`) | `64` | fewest core, ring or hot-pair samples for an answer other than 0 | stated purpose: "Too few samples on either side and the percentiles mean nothing" (`coverage.rs:635-636`) |
| `EDGE_FLOOR` (`intake_scale`) | `2.0/255` | minimum first difference counted as a real edge, not noise | stated purpose, no numeric derivation (`coverage.rs:833-834`) |
| `MAX_W` (`intake_scale`) | `64.0` | clamp on a single edge-width observation | stated purpose (guards the degenerate straight-ramp case), no numeric derivation (`coverage.rs:835-837`) |
| `MIN_W` (`intake_scale`) | `0.25` | lower clamp on a single edge-width observation | no stated derivation (`coverage.rs:838`) |
| minimum edge observations | `16` | below this, `intake_scale` returns `1.0` rather than trusting a thin sample | no stated derivation (`coverage.rs:876-878`) |
| `OVERSAMPLE_TOL` | `3.0` (mean abs. error, 8-bit levels) | absolute round-trip error threshold for `oversample_factor` | measured, and the doc comment is explicit it **cannot** cleanly separate native from oversampled on its own: "the lowest native reading at /2 is 1.29 ... while the 4x upscale this exists for reads 2.12 at /4" (`coverage/oversample.rs:13-17`); it is only safe to use downstream of the `intake_scale` gate, never as a gate itself (`coverage/oversample.rs:19-23`) |
| `OVERSAMPLE_KEEP` | `0.5` (of `flat_error`) | largest share of the image's detail a round trip may lose and still count as lossless | measured margins (2026-10-02): corpus icons at 512/1024 px at most 0.31/0.17, the 38 px² disc 0.36/0.77/1.60 at /2, /4, /8; "the half is our choice" (`coverage/oversample.rs:26-53`) |
| minimum size for `oversample_factor` | `16x16`, and `sw`/`sh >= 8` at each step | avoids measuring on too little data | no stated derivation (`coverage/oversample.rs:119-128`) |

## Failure modes and edge cases

- **Sub-pixel features are unidentifiable, not merely hard.** When every feature in the image is
  narrower than a pixel, `saturation` is low and `fg`/`bg` cannot be trusted as measured — see
  the `saturation` discussion above. The module's answer is to inflate uncertainty, not to guess.
- **Ringing is invisible to two of the three detectors.** `intake_scale` measures edge *width*;
  JPEG and other lossy codecs do not widen edges, they add oscillation in flat regions beside
  them. `estimate_noise` reads a low quantile of the Laplacian, and ringing occupies a band next
  to the edge while most of the image stays flat — the quantile never leaves its floor. The test
  `ringing_does_not_widen_an_edge` (`coverage.rs:1264-1308`) pins both blind spots deliberately:
  on a synthetic ringing edge, `intake_scale` reads under `SOFT_INTAKE_EDGE_FOR_TEST = 1.75`
  (`coverage.rs:1292-1297`, constant at `coverage.rs:1310-1311`), and `estimate_noise` reads
  exactly the `0.5/255` floor on both the ringing image and a clean control
  (`coverage.rs:1304-1307`). This is documented as a **real regression**: "The regression this
  module let through on 2026-09-08 ... The palette's noise guard was gated on this function alone,
  so a compressed logo measured a native 1.15 px, the guard stayed off, and the tracer fitted the
  encoder's ringing as artwork" (`coverage.rs:1264-1269`). Two signals were added rather than
  asking these two a question they cannot answer. The first is file-level, `lossy_container`
  (`load.rs:58-96`), which reads the container format itself (JPEG always lossy; WebP's `VP8 `/`VP8L`
  RIFF tag; PNG/GIF/BMP/TIFF always lossless). Two pixel-level detectors were tried before it
  and failed for the stated reason: "the Laplacian of a lossily-coded flat region and the
  Laplacian of a cleanly-rendered 8-bit colour ramp are the same size. A clean radial gradient
  measured a *higher* 'damage' score than a q50 flat icon" (`load.rs:69-73`). The container is
  defeated by a JPEG re-saved as PNG, which is the case the second signal, `ringing_score`, exists
  for: it reads *where* the Laplacian energy sits and whether it alternates in sign, not how large
  it is (see above).
- **A single JPEG overshoot or stray speck cannot hijack the colour axis** — the plateau
  percentile in `bilevel_coverage` exists specifically to prevent this (`coverage.rs:441-444`).
- **A transparent pixel's stored colour cannot bleed into a downsample** — see the halo-bug
  discussion above and its pinned test.
- **A lone small shape on a near-empty canvas used to read as oversampled.** Before the relative
  test, the absolute round-trip error was diluted by the empty canvas: the 38 px² disc on a
  144 px canvas read 8x, the CLI raised the speckle floor 64-fold to 128 px², and the disc was
  removed. `OVERSAMPLE_KEEP` fixes that case (see above). It does not address a small dot *beside a
  large shape*: the drawing's factor still sets the floor for the dot. Measured when the fix was
  made (2026-10-02): a big disc with a 38 px² dot on a 144 px canvas reads 4x, a speckle floor of
  32 px², so the 38 px² dot survives and a 30 px² dot would not. That is the designed behaviour of
  the redundancy floor rather than this measurement's error.
- **`oversample_factor` is not a substitute for `intake_scale`, and vice versa**, and the doc
  comments on each say so explicitly: `oversample_factor` catches an SR model producing a sharp
  edge on surplus pixels; `intake_scale` catches a soft or blurred edge, which
  `oversample_factor`'s round-trip test alone would not reliably flag as oversampled versus
  genuinely band-limited native content (`coverage/oversample.rs:13-23`).

## Environment overrides

Since the settings cleanup (CHANGELOG, 0.2.0, *Changed*) the engine reads its environment through one helper (`inkvec_core::env`): a switch is off when unset, empty or `0`, and every variable is read once per process. Variables marked *removed* below are gone (their defaults are constants now); those marked *research build* are read only by a binary built with `--features research`. The full list, with what is left and why, is [`docs/internal/env-vars.md`](../internal/env-vars.md).

No `INKVEC_*` environment variable is read anywhere in the module (`coverage.rs`,
`coverage/oversample.rs`, `coverage/resample.rs`). `INKVEC_NOISE_MEDIAN` (*removed*) once switched
`estimate_noise` back to the median; its doc comment names it, and the regression test now reads
the median directly (`coverage.rs:360-363`). Callers downstream read variables that consume this
module's outputs: `INKVEC_PALDBG` prints the edge width, ringing score and the noise guard it
opened (`inkvec-trace/src/lib.rs:387-392`), and `INKVEC_NOISE_SIGMAS` (*removed*) and
`INKVEC_SAME_INK_DE00` (*removed*) used to override the palette's use of them — see
`03-palette.md`.

## Open questions

- **`DEFAULT_SIGMA_MODEL`'s analytic-circle test — resolved.** `coverage.rs`'s own
  `#[cfg(test)] mod tests` has no test of `sigma_model` against a circle, but the test does
  exist, in `inkvec-trace/tests/subpixel.rs:107-167` — see the `sigma_model` discussion above.
- **The Laplacian noise-gain factor — resolved.** It is no longer `sqrt(6)`. The code computes
  `sqrt(20)` from `LAPLACIAN_KERNEL` (`coverage.rs:395`); see the `estimate_noise` discussion for
  the fix and its regression tests.
- **`confidence_penalty`'s cap of `8.0` and floor of `0.05`** (`coverage.rs:536`) have no stated
  derivation — no sweep or measured figure is cited, unlike most other constants in this module.
- **The `0.1%` plateau, `15%` extreme-band tolerance, `EDGE_FLOOR = 2.0/255`, `MAX_W = 64.0`,
  `MIN_W = 0.25`, and the `16`-observation minimum in `intake_scale`**, and `ringing_score`'s
  `24/255` edge floor and `64`-sample minimum, are qualitatively justified (what would go wrong
  without them) but none carries a specific measured value the way `NOISE_QUANTILE`,
  `OVERSAMPLE_TOL` or `OVERSAMPLE_KEEP` do. These read as reasonable engineering guesses rather
  than swept constants.
- **The colour path never constructs a `CoverageField`, and its sigma is a separate copy.**
  It calls `estimate_noise` (`inkvec-trace/src/lib.rs:311`) and `intake_scale`
  (`inkvec-trace/src/lib.rs:344`) itself, and `vertex_sigma` (`planar.rs:1198-1256`) re-derives the
  `sigma_noise / contrast / |grad a|` formula and adds `DEFAULT_SIGMA_MODEL` in quadrature by hand
  rather than calling `CoverageField::position_sigma`. The two already differ in detail:
  `vertex_sigma` reads the coverage change across one pixel along the boundary normal on the
  unmixing axis (floored at 0.001), clamps the result to `[0.02, 2]` px, and applies the
  `--simplify-faint` inflation; `position_sigma` uses the field's central difference and clamps
  to `[0.001, 4]` px. Nothing ties a change to one to the other.
- **The module doc comment still names the `tau * sigma` envelope** (`coverage.rs:30-33`) as the
  consumer of `sigma`. The shipping fitter uses `sigma` as the χ² weight instead (see above); the
  sentence is out of date, not the arithmetic.
