# Contributing to Inkvec

Thanks for taking an interest in Inkvec. This document covers how to build it, what CI
checks before a merge, and what a pull request should look like.

## Building and testing

Rust 1.88 or newer (the workspace `rust-version`). From a checkout:

```
cargo build --release --workspace
cargo test --release --workspace
```

For the browser build (`crates/inkvec-wasm`), a plain `cargo check -p inkvec-wasm --target
wasm32-unknown-unknown` on stable is enough to catch compile errors; producing the actual
`web/pkg` / `web/pkg-threads` packages needs `tools/build_wasm.sh` (nightly + `rust-src` +
`wasm-pack`), which is only run when you are changing the WASM build itself.

Before sending a pull request, run what CI runs:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D clippy::correctness -D clippy::suspicious
cargo test --release --workspace
```

## Before you push

Most red CI runs are not failed tests but things that need no build to find: code left
unformatted in one of the four Cargo workspaces (the root, `studio/core`, `studio/src-tauri`,
`studio/wasm`), a copy of the version number that was not bumped, or a generated file (the
bindings, the Studio's copy of the guide, the Studio's licence notices) left stale.
`tools/prepush.py` runs exactly those checks, in the order CI does, and stops at the first
failure with a one-line hint saying how to fix it:

```
python tools/prepush.py            # about a minute; most of it is the licence notices
python tools/prepush.py --full     # also the quality ratchet (bench/quality.py), minutes
python tools/prepush.py --list     # what it runs
python tools/prepush.py --skip tsc # leave one check out
```

The TypeScript check uses the Studio's own compiler, so run `npm ci` in `studio/` once
first (or skip it with `--skip tsc` if you did not touch the Studio). Every check runs at
below-normal priority, so it does not starve a build running beside it.

To have git run it on every push:

```
python tools/install_hooks.py          # add --full to include the quality ratchet
python tools/install_hooks.py --uninstall
```

The hook lives where git looks for hooks, so every worktree of the checkout shares it, and
the installer refuses to replace a pre-push hook it did not write (unless `--force`).
`git push --no-verify` skips it for one push.

## The regression gate

`bench/ci_gate.py` scores the committed 246-icon screen set (`bench/data`, kept in the
repository so the gate runs from a bare checkout) under eight conditions: Quality and Fast
mode, each at 128 px, 512 px with transparency, 512 px flattened onto white (an opaque logo)
and `web` (the same logo resized to 400 px and saved as a quality-80 JPEG: the noise real
inputs carry; `bench/build_web_tier.py`).
Each condition is compared icon by icon with the per-platform baseline
`bench/gate/baselines/<os>-<arch>.json`, on four axes:

- **dE00** (colour error against the artist's file), margin 1%
- **turning_gap** (distance from the artist's control-polygon turning, per canvas side),
  margin 2%
- **ratio_gap** (distance from the artist's parameter count, `|ln(ratio)|`), margin 3%

Turning and parameters are judged as distances from the artist's file, not as raw values:
a trace that turns more, or writes more numbers, toward what the artist drew is not a
regression. The raw values are still reported.
- **geom** (geometric match to the artist's file: the mean distance between the trace's
  edges and the artist's, in input pixels, read as the area between them), margin 2%

The verdict is statistical: a paired bootstrap interval for the change of each axis, and a
non-inferiority test that passes when the one-sided 95% upper bound stays below the margin.
Where a change is too broad for the 246 icons to resolve that margin (typically at 512 px),
the margin is floored at the smallest effect the set can detect, and such a pass is
reported as "within-noise"; there is no "inconclusive" verdict. Details and the
literature are in `bench/README.md`.

Run it against a release build (about 40 min for all eight conditions on a desktop; pass
`--conditions` for a subset while iterating):

```
cargo build --release -p inkvec-cli
python bench/ci_gate.py --exe target/release/inkvec --workers 4
```

(needs `numpy`, `pillow`, `scikit-image`, `resvg_py`). `python tools/prepush.py --gate`
runs it with the hard cases. Only a demonstrable gain moves a baseline. If a change
regresses an axis, the gate fails; it can only be bypassed with
`--exe ... --bypass-gate "<justification>"`, and only with a strong, explicit
justification the maintainers agree with. Do not pass `--bypass-gate` in a pull request
without discussing it first. If a change is a deliberate, agreed trade, re-baseline with
`--write-baseline` and include the updated `bench/gate/baselines/` files in your diff.

## The quality ratchet

`bench/quality.py` does not analyse anything itself — `cargo clippy`, `rustc`'s own lints
and `cargo fmt` do that, configured under `[workspace.lints]` in the workspace
`Cargo.toml`. What it adds is a budget recorded in `bench/quality_budget.json` that
grandfathers today's existing debt (for example, long functions clippy's
`too_many_lines` already flags) so the count can fall but never rise: a warning count
going up on a lint that already has debt fails, but existing debt does not block
unrelated work.

```
python bench/quality.py             # check; exit 1 on regression
python bench/quality.py --report    # every metric, no gate
```

`--update` re-baselines after a deliberate decision (for example, intentionally adding a
long function) and prints what it loosened so the change is visible in review — use it
sparingly and explain why in the pull request.

## Third-party licence notices

`docs/THIRD_PARTY.md` is generated from the resolved dependency graph. If your change
adds, removes or updates a Cargo dependency, regenerate it:

```
python tools/third_party.py
```

CI checks it is current with `python tools/third_party.py --check`. If your change
introduces a dependency that is not under a permissive licence, expect follow-up
questions — see the licence list embedded in `tools/third_party.py`.

## Commit and pull request expectations

- Keep commits focused; prefer several small commits over one large one when the changes
  are logically separate.
- Describe **why**, not just what, in commit messages and PR descriptions.
- If a change is meant to be a no-op on output, say so and how you checked (byte-identical
  SVG on a sample, or the regression gate showing no movement).
- Match the existing code style; `cargo fmt` and the workspace lints are the source of
  truth, not personal preference.
- New public items should be documented (`missing_docs` is a workspace lint).
- Update `CHANGELOG.md` under `Unreleased` for user-visible changes.

## Licensing

Inkvec is licensed under Apache-2.0 (see [`LICENSE`](LICENSE)). By submitting a
contribution, you agree it is provided under the same licence (inbound = outbound). We do
not require a separate contributor licence agreement (CLA).
