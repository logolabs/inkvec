# inkvec_sr — the super-resolution pre-pass

An optional stage in front of the tracer for input that has been through
something: JPEG, a screenshot, a resize, a chat app. On that input it improves
every metric at once. On clean input it makes things three times worse, which is
why it is a mode and not a default.

```sh
cd tools
python -m inkvec_sr shot.png -o shot.svg              # decide per image (default)
python -m inkvec_sr shot.png -o shot.svg --mode on    # always clean
python -m inkvec_sr shot.png -o shot.svg --mode off   # never; same as inkvec alone
python -m inkvec_sr *.png -o ./out --mode on          # a directory of them
python -m inkvec_sr shot.png -o clean.png --png-only  # just the cleaned raster
```

## What it does, measured

JPEG q50, 12 icons across all seven families, scored against the artist's own
file by the shipped CLI:

| run | dE00 | LPIPS | DINO | params |
|---|---|---|---|---|
| `--mode off` | 1.0010 | 0.0835 | 0.8181 | 19.81 |
| `--mode on` | **0.4381** | **0.0222** | **0.9254** | **6.78** |
| `--mode auto`, clean input | 0.1788 | 0.0130 | 0.9888 | 1.61 |

Colour error halves, perceptual distance improves 3.8x, and the parameter count
falls from twenty times the artist's to under seven. For scale, the benchmark
suite's Potrace row on the same degradation reads dE00 0.931, LPIPS 0.0313,
DINO 0.887, params 7.87 — this beats it on all four, which is the degraded-input
gap `bench/suite` exposed.

The third row is the reason `auto` exists: on undamaged input it declines to
clean, and the result is what tracing directly would have given.

## Why it works, given that it was not trained to

The upscaler is a classical-SR checkpoint (`upsampler: pixelshuffle`), selected
by foreground PSNR on 120 **clean** logos, with no compression, blur or noise
anywhere in training. Every signal in the package says it should not clean
anything. It does anyway: artefact gain — what survives the network relative to
what went in — is 0.36 across seven families, where a network that merely passed
damage through would score 0.52.

The mechanism appears to be a prior, not a training signal. It was fitted on
flat-colour logo art, so flat regions with hard edges are what it can express,
and ringing is not on that manifold. It cannot represent the damage, so it
projects it out.

Full derivation and details: see `docs/algorithm/01-intake.md`.

## How the three steps divide the work

**Upscale x4** is what cleans. **Box-downsample to x2** is not a denoiser however
much it looks like one — a 2x2 box has its only transfer zero at Nyquist, and the
DCT grid puts JPEG damage at f = 1/8 and below, which it passes at 0.974 of
amplitude. It is there because x2 is the useful output scale.

**Recolour** exists because the network costs ~0.6 dE00 on clean input too. Split
by region against bicubic as a control, that cost is 0.60 on flat interiors where
bicubic scores 0.03, and 1.57 on edges where bicubic scores 8.63. Each resampler
is right about the half the other is wrong about, so the pre-pass takes geometry
from the network and colour from the source: one affine map per channel, fitted
where a bicubic upsample says the image is flat, using nothing but the input.
Disable with `--no-recolour` to see the difference.

## How `auto` decides

It traces once and measures the **interior residual**: a traced model is
piecewise flat by construction, so wherever it is locally constant the input
should be too, and disagreement there is degradation rather than modelling error.

Measured over 30 icons in five conditions:

| condition | min | median | p95 | max |
|---|---|---|---|---|
| clean | 0.000 | 0.000 | 0.275 | 0.376 |
| jpeg-q80 | 0.454 | 0.907 | 1.441 | 2.708 |
| jpeg-q50 | 0.650 | 1.241 | 1.942 | 2.669 |
| blur-1.0 | 0.274 | 0.914 | 2.332 | 5.524 |
| noise-2 | 0.812 | 0.837 | 1.156 | 1.426 |

The default threshold of 0.5 cleans **0%** of clean icons and misses 4.2% of
damaged ones. That asymmetry is deliberate: cleaning a clean image is the
expensive mistake, and declining to clean a damaged one merely leaves today's
behaviour in place. Override with `--threshold`.

A cheaper detector was tried and does not work: an 8x8 block signature, which
needs no trace at all, reads 2.175 on clean against 2.133 on JPEG q50. `auto`
pays for one extra trace because nothing cheaper discriminates.

## Reproducibility

The same PNG gives the same SVG. That took work: MambaIRv2 routes each pixel to
one of 128 prompt tokens with `gumbel_softmax(hard=True)`, which samples noise on
every call including under `eval()` and `no_grad()`. Left alone, three runs of one
emoji differed by up to **12.45 levels per pixel**.

The obvious fix is the wrong one. Replacing the sample with its zero-temperature
limit -- a plain one-hot argmax -- is deterministic but measurably worse: dE00
0.5492 against 0.5364, worse on seven of eight images and outside the sampled
range on all eight. The weights were trained against the noisy routing, so the
argmax route is off the distribution they were fitted to.

So the RNG is pinned per call instead, which keeps the trained distribution and
still returns the same answer every time. The spread across draws is only
+/- 0.001 dE00, so which draw gets pinned does not matter -- only that one does.
Pass `deterministic=False` to `model.load` to sample freely.

## Cost

| | |
|---|---|
| upscale, 128 px in | ~3 s with the vendored scan; less with the CUDA kernel |
| `auto` on clean input | one extra trace, no GPU work, model never loaded |
| `auto` on damaged input | one extra trace plus the above |

This does not meet the project's five-second budget on top of a trace, which is
the main reason the mode is opt-in rather than the default for degraded input.

## Requirements

PyTorch with a CUDA device, plus `einops`, `timm`, `Pillow`, `numpy`. The weights
live in `out/sr/logo_sr_package` (unpack `logo_sr_package.zip`); point elsewhere
with `--sr-package`.

`mamba-ssm` is **not** required. It ships its scan as a compiled CUDA extension
with no Windows wheel, so `scan.py` supplies its own: the scan is a first-order
linear recurrence, hence associative, hence computable by Hillis-Steele doubling
in log2(L) steps. It reproduces the sequential definition to a relative 1e-7
(`python -m inkvec_sr.scan`). If the real `mamba-ssm` is installed it is used
instead, since its kernel is faster.

`--mode auto` additionally needs an SVG renderer (`pip install resvg-py`) to
measure the residual. Without one it says so and leaves the input alone.

## Files

| file | |
|---|---|
| `cli.py` | argument handling, mode logic, the two-pass `auto` flow |
| `clean.py` | the pre-pass: box downsample, recolour, and the detector |
| `model.py` | loading the checkpoint, tiled and feathered upscaling, alpha |
| `scan.py` | the parallel selective scan, and binding it in place of mamba-ssm |
