# Showcase at 1024 px: handoff

## Done (code, committed on feature/studio-web)
- `tools/showcase_data.py --size N` (default 1024). Every set, including the 21 benchmark cases, is rendered at N px (square, white, 4% margin) and traced by every engine (Inkvec, VTracer 1.0 best flags, Trazor, VTracer 0.6), then scored at 1024. Pictures and tables come from the same traces.
- The competitor cache is keyed by size: `out/showcase/<N>px/<set>/<key>/`. Old 512 entries are never reused.
- `showcase.json` records `size`. The case raster is lossless WebP; thumbnails are 80 px lossy WebP.
- The front page and Studio Showcase state the input size and label the benchmark table "re-run at N px". Both name the 2026-09-25 512 px run as the benchmark of record.
- `studio/web/landing_check.py` screenshots 115animal, abrinor and SCM at 1x/4x/12x.
- `studio/.shots-web/compare_512_1024.py` (untracked) renders a 512-vs-1024 sheet from the git HEAD files and the new ones.

## Not done
- The data is NOT regenerated. A trial run at 1024 used a tree without the render fix. It was discarded; `web/showcase*` is still the 512 data from 23bbf69.
- **Merge first:** feature/color-groups (7e9020a+) still needs merging. It has two relevant commits:
  - 1f48a43 / 1cc469b: the `bench/inkvec_bench/render.py` `fit_viewbox` fix. Non-square logos were squeezed; SCM's dots came out as 8x14 px ellipses.
  - 17f3723: trademark lines in `showcase_data.py`, `web/index.html` and `showcase.ts`. Keep both their lines and the --size work.
- **Unexplained skips:** the trial run skipped 4 brands at 1024 with `RuntimeError('')`: yachtsandmore_nl, fitbudd_com, legia_net and marcialpons_es. An engine failed with an empty stderr; check which one, likely a timeout or crash at 1024. Skipped cases drop out of the set.
- **Wrong corpus count:** the end-of-run summary printed "corpus: 0 cases" although the log shows 21 corpus lines. Check that `out["corpus"]` is filled.
- Not re-verified after these edits: `landing_check.py` and `smoke.py`.

## One step to regenerate and verify (after the merge; build the merged tree)
```
cargo build --release -p inkvec-cli   # own CARGO_TARGET_DIR
python tools/showcase_data.py --exe <abs path>/inkvec.exe --size 1024 --run "M:/AI STORAGE/SVGIfication/out/crosscompare-competitors-2026-09-25" --vendor "M:/AI STORAGE/SVGIfication/tools/vendor"
cd studio && node scripts/build-web.mjs --skip-wasm && (python web/serve.py &) && python web/landing_check.py && python web/smoke.py
```
- **SCM check:** open `out/showcase/1024px/bench/brands__sangchaimeter_com/input.png`. The dots must be round.
- **Size:** the Space size is printed by `build-web.mjs` and `scripts/deploy-space.py` (dry run).
- **Run time:** the first run at 1024 takes about 40 minutes, because every competitor traces every case once. Later runs only re-trace Inkvec.
