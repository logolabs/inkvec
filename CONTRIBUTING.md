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

## The regression gate

`bench/ci_gate.py` scores the committed 246-icon screen set (`bench/data`, kept in the
repository so the gate runs from a bare checkout) and compares three numbers against
`bench/gate/baseline.json`:

- **dE00** (colour error against the artist's file) may rise at most 1%
- **turning** (anchor turning per unit length) may rise at most 1%
- **ratio** (parameter count versus the artist's) may rise at most 5%

Run it against a release build:

```
cargo build --release -p inkvec-cli
python bench/ci_gate.py --exe target/release/inkvec --workers 4
```

(needs `numpy`, `pillow`, `scikit-image`, `resvg_py`). If a metric genuinely improves, the
baseline is tightened automatically on the spot — ground gained is never given back. If a
change regresses one of the three numbers, the gate fails; it can only be bypassed with
`--exe ... --bypass-gate "<justification>"`, and only with a strong, explicit
justification the maintainers agree with. Do not pass `--bypass-gate` in a pull request
without discussing it first. If a change is a deliberate, agreed improvement, re-baseline
with `--write-baseline` and include the updated `bench/gate/baseline.json` in your diff.

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
