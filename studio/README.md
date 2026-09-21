# Inkvec Studio Lite

A desktop app for Windows, macOS and Linux that turns a raster logo into an SVG, exactly,
and minimises SVGs you already have. Tauri v2 over this repository's own engine.

It is free, and it is marketing for LogoLabs. That only works if it is genuinely good and
genuinely honest, which is why the quality report is a measurement rather than a badge and
why a whole panel is given over to saying what tracing could not recover.

**Your image never leaves this computer.** The only outbound requests the binary can make
are the update check — which sends the app version and the operating system, and can be
turned off — and the optional denoiser download. An image is never a payload.

```sh
cd studio
npm install
npm run tauri dev          # run it
npm run tauri build        # package it for this platform
```

## What it does

Three tabs.

**Vectorize** is where 90% of the time is spent. A split comparison viewer — side by side,
wipe, or A/B with hold-Space to flick — over one shared pan and zoom, with wireframe,
anchor and handle overlays and a detail mode that draws the source's pixel grid under the
vector edge. That last one is the product's whole argument: a boundary lands within about
0.05 px of where the anti-aliasing says it is, and the only way to show that is to let
somebody watch the edge cut through a partially covered pixel.

The rail, top to bottom: preset → trace/cancel → quality report → what could not be
recovered → palette → advanced → **Export, pinned**, reachable without scrolling at every
width. The preset tray holds the seven built-ins and as many as twelve saved ones; a saved
preset is a whole snapshot of the eighteen controls rather than a set of differences from
the defaults, so it cannot drift when those move.

**Minify SVG** is a different register. Tracing takes a second; this takes about thirty
milliseconds, so the tab is instant and the result *is* the screen. Its tolerance control
is a sentence — *nothing moves more than 0.1 px when the drawing is 1024 px wide* — with
the numbers inside it.

**Batch** takes a folder and a preset, writes one SVG per file, and keeps failures visible
after the run rather than letting them scroll away. Any single row can be given a preset of
its own before the run; once a row has run, its preset is frozen, because the dE00 beside
it was measured with the preset the column claims.

**Settings** can put the app into the rest of the desktop, both ways and reversibly: the
bundled `inkvec` command onto `PATH`, and a *Vectorize with Inkvec Studio* entry into the
image right-click menu. Each row shows the real path it wrote and a Remove that takes it
back out, because an app that installs something into the system and cannot say where has
asked you to trust it twice.

## How it is put together

```
studio/
  src/                 frontend: TypeScript, no framework
    lib/               DOM builder, typed IPC, store, SVG path reader
    components/        viewer, rail, overlays, export sheet + share card
    views/             workspace, minify, batch, settings + about
    styles/            tokens.css (dark + light), app.css
  src-tauri/           backend: its own cargo workspace
    src/               trace, quality, lost, minify, export, batch, denoiser,
                       settings, integration (PATH + context menu)
    binaries/          the inkvec CLI sidecar, built by scripts/sidecar.mjs
    installer/         hooks.nsh — NSIS uninstall cleanup
  scripts/             sidecar.mjs: builds and names the CLI for the target triple
  tools/               asset and notice generators
```

There is no frontend framework. The rendering budget is CSS, DOM and inline SVG; the
viewer has to survive a few thousand anchor dots, and a virtual DOM diffing thousands of
`<circle>` elements is exactly the cost that budget exists to avoid. Panels are rebuilt
whole because they are small; the viewer is driven imperatively because it is not.

`src-tauri` is **excluded from the root cargo workspace** on purpose. `cargo build
--workspace` runs on every CI platform and must not start needing WebKitGTK, a WebView2
SDK or a macOS webview. The app still depends on the engine by path, so it always builds
against the tree it sits in.

## Decisions worth knowing before you change something

**Cancelling a trace does not interrupt it.** The pipeline has no cancellation point, and
inventing one would mean unwinding a numerical solve halfway. Cancel retires the
generation instead: the interface returns to the last result at once, the abandoned thread
finishes, and its output is dropped because its generation is stale. Any copy about cancel
must say that plainly and must not imply the CPU stops.

**Out of memory is predicted, not caught.** An allocation failure in Rust aborts the
process, so a trace whose estimate is above an 8 GiB ceiling is refused up front, naming
the size that would fit.

**A draft changes only trace size and time limit.** Moving precision or the palette would
make the draft lie about what the final will look like.

**Dark is the default theme, not "system".** This is a viewer, its stage is a dark ground
so artwork reads against it, and a first run that opened light would show the app at its
least convincing. Light is fully supported, at token parity, one click away.

**The denoiser is a non-default cargo feature.** `--features denoiser` links ONNX Runtime;
the release workflow turns it on for every target except `x86_64-apple-darwin`, mirroring
the exclusion already in the engine's own release matrix. Without it the app says so
honestly rather than offering a download that would achieve nothing.

**`panic = "abort"` is deliberately not set.** The tracer runs inside `catch_unwind` so a
panic becomes a "trace failed" state rather than a window that vanishes.

**The overlay lives inside the artwork's wrapper.** Two absolutely positioned siblings
carrying the same transform drift apart the moment their containing blocks differ, and an
anchors overlay that lands beside the paths is worse than no overlay. One wrapper, one
transform.

**Overlay paint is in the stylesheet, not in presentation attributes.** `var()` is a CSS
value function and is not part of the SVG attribute grammar, so `fill="var(--anchor)"` is
silently dropped by the parser.

## The one engine change

`inkvec_trace::with_stage_sink` — a thread-local hook on `Stopwatch::mark` that reports
each pipeline stage as it is passed. Without it, the only ways to fill the second a trace
takes are a bare spinner or a fabricated sequence of stages, and the brief is explicit that
the progress display is content. It is additive, a no-op when nothing is installed, and
restores the previous sink on panic. The pipeline's internal stage names are documented as
*not* a stable interface; `trace.rs::stage_label` folds all of them onto the nine the
interface names, with a wildcard arm so a new pipeline step can never put an unreadable
word in front of a user.

## Generated assets

`tools/make_assets.py` draws the app icon, the NSIS bitmaps and the four bundled samples
from the shapes that define them, with a `--check` mode CI runs so the committed binaries
cannot drift from the script. The icon is designed at 16 px first — at that size the lowest
stair-step is dropped and everything left grows to at least four device pixels, because
three 8-unit squares at 16 px read as grit rather than as a raster.

`tools/third_party.py` writes `THIRD_PARTY.md` from the app's own dependency graph. The
app ships it next to the engine's and shows both on About: the notices a user reads have to
be the notices for the binary they are running.

## Tests

```sh
npm run sidecar                 # builds the bundled inkvec CLI first:
                                # tauri.conf.json names it in externalBin, so
                                # every cargo command wants it on disk
cd src-tauri && cargo test      # 85 tests
cd ..        && npm run build   # types + bundle
python3 tools/make_assets.py --check
python3 tools/third_party.py --check
```

The backend tests are the interesting ones. They trace real images, check the measured
dE00 against the eight Sharma/Wu/Dalal CIEDE2000 reference pairs, assert that a clean
trace reports **no** losses (a regression test for a heuristic that once claimed 469 lost
features on a 0.07 dE00 trace), and assert that the honesty panel's copy never apologises.

## Known gaps

- **Not signed.** Windows SmartScreen will warn until the certificate builds reputation.
  The release notes carry the calm, factual line about the signature and the published
  checksum. The installer's *own* copy of it does not: the design puts it on a custom NSIS
  options page, and that needs a full template override rather than the supported
  `installerHooks` extension point. Replacing Tauri's whole installer template — untested,
  on the one platform that cannot be tested here — buys a paragraph at the cost of the
  installer itself, so the hook only does uninstall cleanup. See
  `src-tauri/installer/hooks.nsh`.
- **Light theme has had less use than dark.** The tokens are complete and the switch works.
