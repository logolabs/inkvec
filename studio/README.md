# Inkvec Studio Lite

A desktop app for Windows, macOS and Linux that turns a raster logo into an SVG, exactly,
and minimises SVGs you already have. Built on the Inkvec engine in this repository.

**This is a work in progress, and this file is the handoff.** Status as of the first
commit on `studio-lite`. **The backend is complete and tested; the
frontend is not written yet.** This file says exactly what exists, what does not, and the
decisions already taken so the next session does not have to re-derive them.

Source of truth for the design: the Claude Design bundle (four `.dc.html` sheets and
`uploads/Untitled.md`, the UI/UX brief). Everything below follows that brief; where it and
the engine disagreed, the engine won and the note says so.

---

## What is done

### `crates/inkvec-trace` — one additive change

`with_stage_sink(sink, f)` installs a thread-local sink that receives every pipeline stage
boundary as `(name, milliseconds)` while `f` runs. `Stopwatch::mark` feeds it.

This is the only change to the engine, and it exists because constraint #2 of the brief —
*"the wait has to feel like value, not lag… design the progress display as content"* —
cannot be met otherwise. The alternatives were a spinner or a fabricated stage sequence.
It is additive, thread-local (so a batch run on rayon does not interleave), restores the
previous sink on panic, and is a no-op when nothing is installed. Six tests cover it; the
crate's other 82 tests still pass, `cargo fmt --check` and `clippy` are clean.

The internal stage names are explicitly documented as *not* a stable interface.
`studio/src-tauri/src/trace.rs::stage_label` folds all ~30 of them onto the nine the
interface names, with a wildcard arm so a new pipeline step can never put an unreadable
word in front of a user.

### `studio/src-tauri` — the whole backend, 76 tests passing

Its own cargo workspace, **excluded from the root one on purpose**: `cargo build
--workspace` runs on all three platforms in CI and must not start needing WebKitGTK.

| Module | What it does |
| --- | --- |
| `options.rs` | The 18 controls and 7 presets → `inkvec_cli::Args`. Destructured without `..`, so a new control that is not mapped is a compile error. Tooltip copy is the engine's own doc comments, unedited, per the brief. |
| `trace.rs` | Two tiers (draft/final), live stage log, and the states a trace can end in: traced, flat, undecodable, out-of-memory, failed. `Generation` retires stale traces. |
| `quality.rs` | The quality report. dE00 is **measured**: the SVG is rendered back with resvg at the traced size and compared to the raster the tracer saw, in CIEDE2000. Validated against the eight Sharma/Wu/Dalal reference pairs. |
| `lost.rs` | "What this trace could not recover" — lettering, lossy source, baked strokes, smooth ramps, dropped detail. Every row is derived from something measured; a row that cannot be established does not appear. A test asserts the copy never apologises. |
| `minify.rs` | The Minimizer tab, plus a "Removed" breakdown counted by diffing the two documents. Rows that would be zero are omitted. |
| `export.rs` | SVG, minified SVG, PNG set, ICO + favicons, `palette.json`, and the asset-pack zip with its designed plain-text README. Built in memory first so the sheet shows real byte counts. |
| `batch.rs` | The v1.1 queue. One file at a time (the tracer already saturates every core), pause/cancel at row boundaries, failures keep their message, `stats.csv`. |
| `denoiser.rs` | Download, SHA-256 verify, install, remove. Repo/URL/digest/cache path all come from `inkvec_restore`, so the app and the CLI accept the same file. |
| `settings.rs` | Preferences JSON in the platform config dir. Paths only in `recent` — no thumbnails, no contents. |
| `lib.rs` | 28 Tauri commands + events (`trace:stage`, `trace:done`, `batch:row`, `batch:totals`, `batch:finished`, `denoiser:progress`, `denoiser:done`). |

### Assets — generated, not hand-drawn

`studio/tools/make_assets.py` (`--check` mode for CI) produces from the shapes that define
them:

- App icon, **designed at 16 px first**. At that size the lowest stair-step is dropped and
  everything left grows to ≥4 device px — three 8-unit squares at 16 px read as grit.
  Visually verified on both dark and light grounds. → PNG set, ICO (16/32/48/256), ICNS.
- NSIS `sidebar.bmp` (164×314) and `header.bmp` (150×57), exact sizes.
- The four bundled first-run samples: flat logo, 64 px icon, filigree crest, B&W signature.

### Frontend shell

`package.json`, `vite.config.ts`, `tsconfig.json`, `index.html`, and
`src/styles/tokens.css` — the full dark + light token set, using the names already in
`web/index.html` plus the new ones the desktop app needs (`--accent-text`, `--checker-a/b`,
`--focus`, `--state-draft/final/stale`) and the desktop density scale. Inter and Playfair
Display latin subsets are bundled in `public/fonts/` (OFL 1.1) — the app makes no network
request it has not explained, and that includes a font.

---

## What is NOT done

1. **The entire frontend beyond `tokens.css`.** No `app.css`, no `src/main.ts`, no views,
   no components. Every screen on the four design sheets still has to be built. The backend
   API it will call is stable and documented in `lib.rs`.
2. **CI and release wiring.** No `studio.yml`; `release.yml` has no studio job. This was the
   explicit ask ("build on linux, mac, Windows and have it part of our release") and is
   still outstanding. Plan below.
3. **`inkvec` CLI as a sidecar.** Settings has an "Add inkvec to PATH" row in the design.
   The commands for it were drafted and then cut from this commit because they depend on
   the sidecar wiring in (2). Re-add `install_cli`/`remove_cli`/`cli_installed` once
   `externalBin` is set up.
4. **Share card.** Decided: render it on a `<canvas>` in the frontend rather than in Rust.
   resvg cannot shape text without a font database, and woff2 is not a format it reads;
   drawing it in the webview uses the app's own loaded fonts and makes the card identical
   to what is on screen. `save_bytes` already exists to write the PNG.
5. **Windows right-click "Vectorize with Inkvec"** — registry work, not started.
6. **Nothing has been run as an actual app.** `cargo test` passes and the crate compiles;
   `cargo tauri build` has never been executed. WebKitGTK dev packages were installed in
   this container, so a Linux build is possible here.

---

## Decisions already taken (do not re-litigate)

- **Custom title bar** (`decorations: false`). The tab switcher and the file chip belong in
  that 46 px. Cost is ours: drag regions, snap layouts and double-click-to-maximise all
  have to be wired by hand in the frontend.
- **Cancel does not interrupt a trace.** The pipeline has no cancellation point and
  inventing one would mean unwinding a numerical solve halfway. Cancel retires the
  generation: the interface returns to the last result at once, the abandoned thread
  finishes and its output is dropped. Documented in `trace.rs`; say this plainly in any
  copy, do not imply the CPU stops.
- **Out of memory is predicted, not caught.** A Rust allocation failure aborts the process.
  `memory_estimate` refuses a trace above an 8 GiB ceiling and names the size that fits.
- **Denoiser is a non-default cargo feature** (`denoiser`), to be enabled in CI for every
  target except `x86_64-apple-darwin` — mirroring the exclusion already in `release.yml`,
  where ONNX Runtime has no package. The UI reports `supported: false` honestly otherwise.
- **`panic = "abort"` is deliberately not set** in the release profile: the tracer runs
  inside `catch_unwind` so a panic becomes a "trace failed" state, not a vanished window.
- **Draft changes only trace size and time limit.** Moving precision or the palette would
  make the draft lie about what the final will look like.
- **The 18 controls** are Detail (precision, speckle floor, trace size, time limit),
  Colour (max colours, colour merging, flat fills, black & white, clean up damage),
  Shape (match repeated shapes, match threshold, fewer paths, line art, repair rings),
  Output (minify, transparent background, margin, holes as cutouts). The brief's overlay
  sheet showed 15; the remaining three come from the terminology table.

## The CI/release plan, worked out but not written

1. `.github/workflows/studio.yml` — build the app on `ubuntu-latest`, `macos-14`,
   `windows-latest` on PRs touching `studio/**`. Linux needs
   `libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev libsoup-3.0-dev libgtk-3-dev
   librsvg2-dev patchelf`. Also run `python3 studio/tools/make_assets.py --check` so the
   committed binaries cannot drift from the script.
2. `release.yml` — a `studio` job on the same five targets as the CLI matrix, producing
   `.deb`/`.rpm`/`.AppImage`, `.dmg`, and NSIS `.exe` + `.msi`; `--features denoiser`
   everywhere except `x86_64-apple-darwin`. Feed its artifacts into the existing
   `checksums` job so they land in `SHA256SUMS` and the published release unchanged.
3. Add the studio's dependencies to `docs/THIRD_PARTY.md` (`tools/third_party.py`) — the
   `licences` CI job will otherwise fail once studio is in the graph. Add the two OFL font
   notices by hand.
4. `CHANGELOG.md` entry.

## Running what exists

```sh
cd studio/src-tauri && cargo test           # 76 tests
cd .. && npm install && npm run build       # frontend shell only; there is no app yet
python3 tools/make_assets.py --check        # assets are reproducible
```
