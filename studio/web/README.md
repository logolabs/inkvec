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

This Space has two pages. The front page presents Inkvec: before-and-after on real logos
with the geometry showing, the latest comparison with other tracers, what is new, and how it
works. **Inkvec Studio Lite**, under [`studio/`](studio/), is the editor itself.

## Inkvec Studio Lite

The Inkvec Studio editor, in your browser. Drop a logo, an icon or flat artwork and get an
exact SVG back: boundaries where the anti-aliasing says they are, circles as circles, one path
per colour, and the number of coordinates chosen by minimum description length rather than a
tolerance slider. Nothing is uploaded — the tracer runs in this tab, as WebAssembly, on your
own processor.

It is the desktop app's interface and the desktop app's code, not a demo of them:

- **Vectorize** with a live draft while you move a control and the full trace once you stop,
  ten presets, the Custom wizard, the measured colour difference, the palette with colour
  groups, snapping to a brand palette, and export (SVG, minified SVG, PNG sizes, a favicon set,
  an asset pack) as a download.
- **Minify SVG**: rewrite an SVG you already have in fewer bytes, with the difference measured.
- **Fabricate**: vinyl sheets, stickers, stencils and laser cut lines, with a preflight.
- **Clean up damage**: the trained denoiser for JPEG and screenshot damage, downloaded once
  into your browser, in the background as the Studio opens (not when your browser asks to save
  data), and run on your GPU where there is one (ONNX Runtime Web).

Hugging Face shows this Space inside a frame on its own page, and a browser will not give a
framed page the threads the engine needs (cross-origin isolation needs every page above it to
ask for it). So the front page opens the Studio in **a tab of its own**, where it runs on every
core and can use the denoiser; a logo dropped on the front page goes with it, handed over
inside your browser. Its own address:
<https://logolabs-inkvec.static.hf.space/studio/index.html>. It is built for a laptop or
desktop screen; **Full screen** is in its top bar.

The desktop app, **Inkvec Studio** (Windows, macOS, Linux), is the same interface plus what a
browser cannot do: folders of images in one batch, the `inkvec` command line, the right-click
menu, and traces larger than 2048 px. Releases: <https://github.com/logolabs/inkvec/releases>.

## How it runs

The Studio's backend is a shared Rust core (`studio/core`) that the desktop app wraps in Tauri
and this page wraps in a Web Worker (`studio/wasm`). Two builds of that worker sit side by side:
one with a rayon thread pool, which needs the cross-origin isolation the headers above ask for,
and a single-threaded one for anywhere that isolation is not granted. Same code, same output.

Source, benchmarks and licences: <https://github.com/logolabs/inkvec>. Apache-2.0. Made by
LogoLabs. Trademarks and brand logos depicted in benchmark comparisons are the property of
their respective owners and used solely for nominative benchmarking demonstration.
