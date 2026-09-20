---
title: Inkvec
colorFrom: gray
colorTo: yellow
sdk: static
pinned: true
license: apache-2.0
short_description: Exact SVG from logos and icons, entirely in your browser
custom_headers:
  cross-origin-embedder-policy: require-corp
  cross-origin-opener-policy: same-origin
  cross-origin-resource-policy: cross-origin
---

# Inkvec

Exact vectors from logos, icons and flat artwork, entirely in the browser. Inkvec is
LogoLabs' tracer compiled to WebAssembly and served as a static page: drop an image, get an
SVG, nothing leaves your machine.

Boundaries land where the anti-aliasing says they are, circles come back as circles, the
pieces of one colour are one path, and the number of coordinates is chosen by minimum
description length rather than a tolerance slider. Across 21 cases — 14 hash-selected real
icons across seven families, 4 hand-picked synthetic probes, and 3 real brand logos from an
external dataset (selection fixed from the seed `crosscompare-2026-09-15-v1`) — scored
against the source render, Inkvec's median colour error (dE00) is 11x lower than VTracer's
default output at roughly a quarter of the coordinates; the 3 brand logos are not part of the
repo, so a clean clone reproduces the other 18. Full numbers, sourced, are in the
[project README](https://github.com/logolabs/inkvec#results).

Damaged input — a JPEG, a WebP, something an image model decoded — can be cleaned first by
the trained denoiser, which runs here on WebGPU. See [The denoiser](#the-denoiser).

## Build

```
rustup target add wasm32-unknown-unknown
rustup toolchain install nightly --component rust-src
tools/build_wasm.sh
```

That writes two packages, and `web/worker.js` picks between them at load:

| | `web/pkg/` | `web/pkg-threads/` |
|---|---|---|
| toolchain | stable | nightly, with `std` rebuilt for atomics |
| cores | one | one rayon worker per core |
| needs | nothing | a cross-origin isolated page |

The tracer, its arithmetic and its output are the same in both — measured, by hashing the
SVGs from each: on a sixteen-core machine the threaded build is 3.7x to 4.3x faster on real
logos and returns the identical bytes.

Isolation is what the `custom_headers` block in this README asks the Space for. Where it is
not granted the page loads the single-threaded package instead and everything still works.

The page is `web/index.html`; serve it over HTTP (ES modules do not load from `file://`).

## The denoiser

The `--restore` pre-pass runs in the browser as well, off by default, with the same three
modes the command line has:

| | |
|---|---|
| **No denoise** | what the page has always done |
| **Auto** | trace once, measure how far the raster disagrees with its own trace where the trace claims a flat interior, denoise and retrace only above the threshold |
| **On** | always denoise first |

It is the same network, not a second one: `web/denoise.js` loads `restorer.onnx` from
[`Logolabs/inkvec-denoiser-001`](https://huggingface.co/Logolabs/inkvec-denoiser-001) — the
file the command line pulls, checked against the same SHA-256 the WebAssembly build reports —
into [ONNX Runtime Web](https://onnxruntime.ai/docs/tutorials/web/), pinned to 1.30.0. WebGPU
runs it where the browser has it, and ONNX Runtime's WebAssembly kernels where it does not;
which one ran is on the result line.

Everything around the network is the tracer's own code, reached through WebAssembly rather
than rewritten in JavaScript: the compositing onto white, the pad to a multiple of 16, the
crop, the quantisation to 256 levels, the extreme-snapping (`inkvec_restore::network_input`
and `network_output`), the `auto` decision (`inkvec_restore::decide`) and the forced soft
intake for the trace that follows. `denoise.js` turns one Float32Array into another and does
nothing else.

Measured against native ONNX Runtime on a 512-px JPEG, the WebAssembly kernels agree to
4.2e-7; after the 8-bit quantisation the tracer reads, one channel of one pixel in 786,432
differs by one level.

Turning it on downloads ONNX Runtime Web (~28 MB, from jsDelivr) and the weights (~80 MB,
from Hugging Face) once; the browser caches both. The image is still never uploaded.

## Deploy

This folder is the Space. Create a Space under the LogoLabs org with the **static** SDK and
push these files (README, `index.html`, `worker.js`, `denoise.js`, `samples/`, the built
`pkg/` and `pkg-threads/`). Build the packages with `tools/build_wasm.sh`, then upload this
folder to the Space with `huggingface-cli` once you are logged in. ONNX Runtime and the
denoiser weights are not part of the upload: the page fetches them at run time.

## Notes

- Single-threaded in the browser (rayon runs its work on the calling thread when it cannot
  spawn). A 768-px logo takes about eight seconds; the native binary uses every core and is
  several times faster. Browser threads need cross-origin isolation headers the static SDK
  cannot set; a `coi-serviceworker` shim is the usual workaround if that ever matters.
- Inputs above the "max dimension" setting are traced at that size and written at the
  original size, so a 4000-px screenshot still finishes.
- The denoiser runs at the size the tracer will see — after "max dimension", after the
  unblock — because that is where `--restore` sits in the pipeline, and it costs what a
  20-million-parameter network costs: seconds on a GPU, longer on the WebAssembly kernels.
  A browser whose only WebGPU adapter is a software one (SwiftShader, lavapipe) is taken as
  no GPU at all and the WebAssembly kernels run instead: measured in a headless Chromium, a
  512-px pass those kernels finished in about eight seconds had still not returned ten
  minutes into the software adapter.
