## What changed

<!-- Describe the change and why it's needed. Link any related issue. -->

## How verified

- [ ] `cargo test --release --workspace` passes
- [ ] `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets` are clean
- [ ] The regression gate (`python bench/ci_gate.py --exe target/release/inkvec
      --workers 4`) passes, or any movement is explained below
- [ ] If this change should not alter output: verified byte-identical SVG on a sample
      (describe how, e.g. which inputs)
- [ ] `docs/THIRD_PARTY.md` regenerated (`python tools/third_party.py`) if dependencies
      changed
- [ ] `CHANGELOG.md` updated under `Unreleased` for user-visible changes

<!-- If the regression gate moved, or you're intentionally loosening the quality ratchet
     (bench/quality.py --update), explain why here. -->
