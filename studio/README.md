<img src="src/assets/mark.svg" alt="" height="72" align="right">

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

The denoiser is compiled into every build by default (`--no-default-features` leaves it
out, and the release does that for exactly one target, Intel macOS, where ONNX Runtime has
no package). The first build downloads a prebuilt ONNX Runtime and links it statically.

### Building on an older MSVC

On Windows that static link needs MSVC 17.10 or newer; with 17.9 it ends in an unresolved
`__std_find_last_of_trivial_pos_1`. Either update Visual Studio, or link ONNX Runtime
dynamically from Microsoft's own release and ship its DLLs beside the app. Put
`onnxruntime-win-x64-1.28.0` under `tools/vendor/onnxruntime-1.28.0/` and build with

```powershell
$env:ORT_LIB_PATH = "<repo>\tools\vendor\onnxruntime-1.28.0\onnxruntime-win-x64-1.28.0\lib"
$env:ORT_PREFER_DYNAMIC_LINK = "1"
.\node_modules\.bin\tauri.cmd build --bundles nsis --config src-tauri\tauri.ort-dynamic.conf.json
```

`tauri.ort-dynamic.conf.json` adds `onnxruntime.dll` and `onnxruntime_providers_shared.dll`
to the bundle, staging them first with `scripts/stage-ort.mjs` (it needs `ORT_LIB_PATH`). Without
them the app will not start, because it imports the DLL at load.

## What it does

Four tabs.

**Vectorize** is where 90% of the time is spent. A split comparison viewer — side by side,
wipe, or A/B with hold-Space to flick — over one shared pan and zoom, with wireframe,
anchor and handle overlays and a detail mode that draws the source's pixel grid under the
vector edge. That last one is the product's whole argument: a boundary lands within about
0.05 px of where the anti-aliasing says it is, and the only way to show that is to let
somebody watch the edge cut through a partially covered pixel.

Above both halves sit the two settings people most often come for, drawn big and always
in view: the **denoiser** (Off, Auto, On; with a *Download it* prompt if it is chosen before
it is installed) and **Editable** (a switch). They are the same settings the groups below
hold, drawn once rather than twice.

The rail has two halves, chosen by a switch at its top. **Result** is what came out: the
quality report, how editable the drawing is, what could not be recovered, the palette.
**Tune** is what makes it: the presets and the twenty-one controls, in four groups that fold
and say how many of their controls have moved off the preset. They are separate because they
are used at different moments, and because one long column put the controls a thousand
pixels from the number they change. What both share is pinned at the foot: a live readout
of the colour difference, the coordinates and the file size, each with what the last
control moved it by, and **Export**, reachable without scrolling at every width. Nothing
covers the stage while you tune; there is no sheet that slides over the drawing.

The preset tray holds the eight built-ins and as many as twelve saved ones; a saved preset
is a whole snapshot of the twenty-one controls rather than a set of differences from the
defaults, so it cannot drift when those move.

**Editable structure** is the newest control, and the Result tab has a card for it. A trace
is fitted for pixels alone, so what an artist meets when they open it in a vector editor
— handles at arbitrary angles, joins with a kink, nodes that line up with nothing — is
noise. The card counts three habits of hand-drawn files on this drawing (handles on an
axis, smooth joins, nodes sharing a coordinate), each beside where hand-drawn files sit
(the median of 1,544 artist SVGs, measured with the same function,
`inkvec_svgmin::structure`), so what the switch does is shown and its price is the
readout's colour difference beside it. A drawing of lines and arcs says so instead of
showing empty bars.

**Minify SVG** is a different register. Tracing takes a second; this takes about thirty
milliseconds, so the tab is instant and the result *is* the screen. Its tolerance control
is a sentence — *nothing moves more than 0.1 px when the drawing is 1024 px wide* — with
the numbers inside it.

**Fabricate** prepares a drawing for a cutter, in millimetres, through `inkvec-fab`: one
colour, layered vinyl (each colour running under the ones above by the bleed, with
registration marks), inlay, print-then-cut sticker, stencil, or Lines for a pen, a scoring
blade or a laser line. The stage shows the sheets with what preflight found drawn over them
(parts and gaps narrower than the material's minimum feature, specks, translucency); the
cutting card holds kerf, mirror, weed border, a router bit's diameter for dogbones, and the
size-check square; Save writes an SVG per sheet and a combined file, and optionally DXF and
G-code.

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

**The denoiser is a default cargo feature.** It links ONNX Runtime, and "Clean up damage"
and the Photo-or-scan preset do nothing without it, so every build has it. The release
workflow passes `--no-default-features` for exactly one target, `x86_64-apple-darwin`,
mirroring the exclusion already in the engine's own release matrix. A build without it says
so honestly rather than offering a download that would achieve nothing.

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

## The mark

`src/assets/mark.svg` is not drawn here. `tools/make_assets.py` takes it from
`web/logo.svg` — the one the site serves — and makes it mono, which is already this
project's treatment for the mark: the documentation site recolours it to the copper
accent.

One file covers both readings of "mono". Its ink is `currentColor` and the root carries
`color="#c9754a"`, so on its own — the image above, a file preview — it is the copper
mark the site shows, while `appMark` removes that `color` and the mark takes whatever
the text around it is using: 18 px in the chrome, 72 px on About, and the light theme
needs no second file.

`make_assets.py --check` fails if it ever drifts from `web/logo.svg`, which is the
point — the mark changed twice in one day while this app was being built.

The platform icon files (`.ico`, `.icns`, the PNGs) are this same mark, in copper on a
transparent ground: `make_assets.py` rasterises the mark's own path, so there is no second
drawing to keep in step. It has no tile behind it, so it reads on a light taskbar and a
dark one alike.

## Generated assets

`tools/make_assets.py` draws the app icon, the NSIS bitmaps and the four bundled samples
from the shapes that define them, with a `--check` mode CI runs so the committed binaries
cannot drift from the script.

`tools/third_party.py` writes `STUDIO_THIRD_PARTY.md` from the app's own dependency graph. The
app ships it next to the engine's and shows both on About: the notices a user reads have to
be the notices for the binary they are running.

## Looking at the interface without building the app

```sh
npm run mock        # http://127.0.0.1:1420/dev/index.html
```

`dev/` serves the real interface in an ordinary browser over a stand-in for the Rust
backend (`dev/mock.ts`, on Tauri's own `mockIPC`). The control table is the real one,
extracted from `options.rs` by `dev/gen_controls.py`; the traces are SVGs the real CLI wrote
(`dev/traced/`) and the report's structure counts are measured from them, but the colour
difference and the timings are made up, and it says so at the top of the file. It exists for
looking at layout and behaviour — it is how the two-halves rail was designed — and never
ships: the build's inputs are `index.html` and `splash.html` only.

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
None of them reads or writes the real preferences or the installed denoiser: the functions
that touch a file take its path (`save_at`, `load_at`, `reset_at`, `status_at`), and the
tests pass temporary ones.

To run a built app without touching your own setup, point it at scratch directories:

```sh
INKVEC_STUDIO_CONFIG_DIR=/tmp/studio-prefs INKVEC_STUDIO_MODEL_DIR=/tmp/studio-model  ./inkvec-studio
```

`preferences.json` then lives in the first, and `restorer.onnx` is looked for (and
downloaded to) the second only; with the second set and empty, the denoiser reads as not
installed rather than falling back to the model in your cache. Settings' *Add to PATH*
and the right-click entry still write where they say they do, so leave those alone in a
smoke test.

## Known gaps

- **Not signed**, on any platform: no certificate is configured for Windows, and the
  macOS bundles are neither signed nor notarised, so SmartScreen warns and Gatekeeper
  needs a Control-click **Open** the first time. The release notes say exactly that, next
  to the published checksums and the steps for each platform. The installer's *own* copy
  of it does not: the design puts it on a custom NSIS
  options page, and that needs a full template override rather than the supported
  `installerHooks` extension point. Replacing Tauri's whole installer template — untested,
  on the one platform that cannot be tested here — buys a paragraph at the cost of the
  installer itself, so the hook only does uninstall cleanup. See
  `src-tauri/installer/hooks.nsh`.
- **Light theme has had less use than dark.** The tokens are complete and the switch works.
