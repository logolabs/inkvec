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
description length rather than a tolerance slider. Across 21 real logos, icons and emoji,
scored against the source render, Inkvec's median colour error (dE00) is 12.5x lower than
VTracer's default output at roughly a third of the coordinates. Full numbers, sourced, are
in the [project README](https://github.com/logolabs/inkvec#results).

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
logos and returns the identical bytes. `web/threadtest.html` is the harness that says so;
open it with and without isolation.

Isolation is what the `custom_headers` block in this README asks the Space for. Where it is
not granted the page loads the single-threaded package instead and everything still works.

The page is `web/index.html`; serve it over HTTP (ES modules do not load from `file://`).

## Deploy

This folder is the Space. Create a Space under the LogoLabs org with the **static** SDK and
push these files (README, `index.html`, `worker.js`, `samples/`, the built `pkg/`).
`tools/deploy_space.sh logolabs/inkvec` builds the package and uploads with
`huggingface-cli` once you are logged in.

## Notes

- Single-threaded in the browser (rayon runs its work on the calling thread when it cannot
  spawn). A 768-px logo takes about eight seconds; the native binary uses every core and is
  several times faster. Browser threads need cross-origin isolation headers the static SDK
  cannot set; a `coi-serviceworker` shim is the usual workaround if that ever matters.
- Inputs above the "max dimension" setting are traced at that size and written at the
  original size, so a 4000-px screenshot still finishes.
