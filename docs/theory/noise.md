# Noisy inputs

> The theory so far reads exact renders: box-filtered, rounded to 8 bits, nothing else. The
> files people send have been resampled and compressed on the way. This page says what that
> adds, measured; what of the theory survives it; what has to change; and how the gate
> holds the engine to it.

## What arrives

A logo usually reaches a tracer as a screenshot or a download: drawn at one size, resized to
another with a bicubic or Lanczos kernel, and saved as a JPEG, often with the colour stored at
half resolution (4:2:0). Each step leaves its mark at the edges: blur and overshoot from the
resampling kernel, 8×8 block quantisation and ringing from the codec, colour bleeding a pixel
or two across every edge from the subsampled chroma. Zhang, Liang, Van Gool and Timofte
(2021), *Designing a practical degradation model for deep blind image super-resolution*, ICCV,
arXiv 2103.14006, model real degradations as exactly this chain.

The gate's `web` tier fixes it at one realistic setting: each of the 246 screen icons at
512 px, flattened onto white, resized to 400 px (bicubic) and saved at JPEG quality 80 with
4:2:0 chroma (`bench/build_web_tier.py`). `quality-web` and `fast-web` are gated like the
other six conditions.

## Where the noise sits

`bench/theory/noise_profile.py`, 41 icons, the `web` raster against the exact box-filtered
raster at 400 px, RMS in 8-bit levels by distance to the nearest exact edge:

| distance to an edge | share of pixels | resampling (luma) | JPEG (luma) | JPEG (chroma) |
|---|---|---|---|---|
| under 1 px | 4.7 % | 7.9 | 4.5 | 6.5 |
| 1-2 px | 3.7 % | 0.3 | 3.1 | 2.7 |
| 2-3 px | 3.6 % | 0.05 | 2.3 | 1.6 |
| 3-5 px | 5.5 % | 0.05 | 1.7 | 1.3 |
| 5-8 px | 8.1 % | 0.05 | 0.95 | 1.05 |
| 8 px and more | 74 % | 0.03 | 0.20 | 0.35 |

Two facts follow. The resampling's error is deterministic and confined to the edge pixel: a
blur, which a forward model reproduces, not noise. The codec's error is noise, and it is not
stationary: twenty times larger at an edge than in the flat interior three quarters of the
image is made of. One noise level per image is wrong wherever it is read. The engine's
estimate (`coverage::estimate_noise`, a low quantile of the Laplacian, so flat regions)
returns its floor of 0.5 levels on every one of these images; its measured replacement on a
lossy intake (`regularize::residual_sigma`, the larger of an interior and an edge median)
returns the edge's 4-5 levels and applies them to the interiors too. The first lets ringing
halos through as inks; the second lets two flat regions a few levels apart pass for one
gradient.

## What survives: area windows are unbiased

A column window's sum is the area of a face in that column (the window identity,
`window_sum_eq_area`). Under a blur kernel `k` whose weights over the outputs sum to one for
every input position, Fubini gives, for a window that covers the blurred transition,

    Σ_{y ∈ W} (f ∗ k)(x, y) = (A ∗ k_x)(x) + Δ · m_y

where `A` is the column-area profile, `k_x` the kernel's horizontal marginal, `Δ` the step
between the two plateaus and `m_y` the kernel's vertical first moment. A centred kernel has
`m_y = 0`, so vertical blur, overshoot and ringing vanish from a column window entirely, and
horizontal blur is a one-dimensional convolution of the area profile, the identity on a
straight edge and `½ μ₂ A''` on a curved one. The measured means agree: over 38 826 windows
the sums stay within 0.0004 px of area of the exact ones on the 8-bit, the resampled and the
`web` rasters alike, and growing the windows only adds noise.

What changes is the spread: 0.005 px of area on an exact render, 0.039 once resampled
(Pillow normalises its weights per output pixel, so at a non-integer factor an edge's sum
ripples with its phase against the output grid; the same 0.041 from an exact 512 px render),
0.066 on `web`. The tails grow with it: the 95th percentile is 1.2 times the RMS on the exact
render and 1.9 times on `web`.

This is why reading areas is the right primitive for noisy input. A tracer that thresholds,
or reads the half-coverage crossing, has an error that grows with the blur; the window's sum
does not move, it only becomes less precise, and its precision can be measured from the
image itself.

## What has to change

1. **A noise model per image and per place, not one number.** Interior and edge noise
   measured separately, the edge's as a function of distance; a window's variance from the
   windows themselves (the fourth difference of consecutive windows reads it without knowing
   the geometry). Each decision takes the noise of the pixels it reads: an ink or a gradient
   from interiors, a boundary from its edge band.
2. **Heavy tails.** A Student-t or Huber likelihood, which is χ² for small residuals (exact
   renders keep their behaviour) and does not let one block step or ringing lobe overrule a
   constraint.
3. **A forward model with the blur in it.** Score a candidate's area profile convolved with
   `k_x` rather than the profile itself; `k`'s width from the edge profiles
   (`softness::ramp_evidence` already measures it). On an exact render the kernel is the
   identity and nothing changes.
4. **Colour from luma.** With 4:2:0 the chroma is at half resolution and bleeds across edges;
   the coverage between two inks is read from luma wherever their lumas differ, and inks are
   robust (median) estimates of interiors.
5. **Halos are not faces.** A band narrower than the kernel along an edge, whose colour lies
   on the line through the two inks it separates but beyond them (overshoot), or off it by
   the chroma's bleed, is explained by the edge; a face must beat that explanation by its own
   description length.

## Noise makes the prior matter more

For nested descriptions `M₀ ⊂ M₁`, `M₁` adding `k` parameters (or dropping a tie), the
evidence for `M₀` grows with the noise when the fit `M₁` buys is within it: the likelihood
flattens and the prior's preference for the shorter description carries more of the
posterior (`chain-representation.md`). On noisy input the honest result snaps more to the
grid, ties more coordinates and writes fewer segments, not more; detail finer than the blur
was never in the pixels, and the shortest description that fits drops it. A trace whose
parameter count rises on noisy input is fitting the noise. The gate's `ratio` axis on `web`
reads it.

## Where the engine stands (2026-10-10)

Against the opaque 512 px tier, on the gate's 246 icons:

| | Quality `web` | Quality `512ssop` | Fast `web` | Fast `512ssop` |
|---|---|---|---|---|
| dE00 | 0.202 | 0.040 | 0.213 | 0.080 |
| geom (px) | 0.347 | 0.097 | 0.354 | 0.282 |
| geom_far (px) | 0.145 | 0.019 | 0.136 | 0.142 |
| parameters / artist's | 2.16 | 1.41 | 7.22 | 3.51 |

Fast takes ringing halos for inks (a two-colour flag comes back with four and 215 faces:
the halo colours lie 0.08-0.09 sRGB from the line between the two inks, twice the blend
tolerance, because 4:2:0 moves chroma independently of luma) and its parameters explode on
the colour families (openmoji 13×, twemoji 8.9×). Quality loses inks instead: on a soft
intake the palette treats colours within 5 dE00 as one ink, to keep ramp colours out, and
that also folds real neighbouring inks together (the mosaic's 37 fills come back as 26
inks and 25 faces, five of them gradients, where the clean raster gives 34 and 59 flat
ones); the interior residual then reads palette bias (4.5 levels on the mosaic, where the
codec's interior noise is 0.2), so splitting the noise level by population alone does not
help. Gradients are not the culprit on balance: tracing `web` without them raises dE00 by
62 % and `geom` by 139 %. What separates a ramp colour from an ink is where it sits (within
the blur's reach of an edge, or with an interior of its own), not how far it is from its
neighbour. The boundary chain owns items 1-4
above (`chain-boundary.md`), the representation chain item 5 and the prior's side
(`chain-representation.md`).
