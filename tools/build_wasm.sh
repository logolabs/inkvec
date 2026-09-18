#!/usr/bin/env bash
# Build both WebAssembly packages the Space serves.
#
#   web/pkg/          stable toolchain, one core, runs anywhere
#   web/pkg-threads/  nightly toolchain, a rayon pool, needs a cross-origin isolated page
#
# The tracer, its f64 arithmetic and its output are identical between the two. Only the
# number of cores differs, and `web/worker.js` picks whichever the browser can run.
#
# Prerequisites:
#   rustup toolchain install nightly --component rust-src
#   cargo install wasm-pack
#
# After building, bump `V` in web/worker.js so returning visitors do not keep the old wasm.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT=$(pwd)
CRATE="$ROOT/crates/inkvec-wasm"

# The two arms need separate target directories. They compile the same crate for the same
# triple, so they write the same `target/wasm32-unknown-unknown/release/inkvec_wasm.wasm`;
# whichever ran second would be judged up to date and hand wasm-pack the other one's
# binary. That failure is silent right until wasm-bindgen reports `__wasm_init_tls`
# missing, which is the non-atomics build wearing the threaded build's name.

echo "==> single-threaded (stable) -> web/pkg"
CARGO_TARGET_DIR="$ROOT/target/wasm-st" \
  wasm-pack build "$CRATE" --target web --release --out-dir "$ROOT/web/pkg"

echo "==> threaded (nightly, build-std) -> web/pkg-threads"
# Three things are needed beyond `--features threads`, and each fails differently:
#
#   -Z build-std      the shipped `std` has no atomics; without rebuilding it the module
#                     links but carries no thread-local storage
#   +atomics          makes the code use atomic instructions -- but on its own leaves the
#                     memory unshared, and rayon hands that memory to its workers with
#                     `postMessage`, which fails with "#<Memory> could not be cloned"
#   --import-memory   wasm-bindgen's threading support requires the JS side to own the
#                     memory, since it is the JS side that passes it to each worker
#
# A shared memory must also declare a maximum, hence --max-memory.
CARGO_TARGET_DIR="$ROOT/target/wasm-mt" \
RUSTUP_TOOLCHAIN=nightly \
RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals -C link-arg=--shared-memory -C link-arg=--import-memory -C link-arg=--max-memory=4294967296 -C link-arg=--export=__wasm_init_tls -C link-arg=--export=__tls_size -C link-arg=--export=__tls_align -C link-arg=--export=__tls_base" \
  wasm-pack build "$CRATE" --target web --release --out-dir "$ROOT/web/pkg-threads" \
  -- -Z build-std=panic_abort,std --features threads

# wasm-pack writes a .gitignore into every out-dir, which would keep the package out of the
# Space entirely. The built package IS the deliverable here.
rm -f "$ROOT/web/pkg/.gitignore" "$ROOT/web/pkg-threads/.gitignore"

# wasm-bindgen-rayon's worker helper reaches its own package with `import('../../..')` --
# a directory, which only resolves under a bundler. A browser asks the server for the
# directory, gets something that is not JavaScript, and the spawned worker never reports
# ready: `initThreadPool` then waits for it forever, with no error anywhere. Name the file.
#
# The cache token is copied from worker.js so the helper and its parent always agree about
# which build they are running; a stale copy of the bindings against a fresh wasm would be
# a much harder failure to read than this one.
V=$(grep -oE 'v=[0-9]+' "$ROOT/web/worker.js" | head -1)
for h in "$ROOT"/web/pkg-threads/snippets/*/src/workerHelpers.js; do
  sed -i "s|await import('../../..')|await import('../../../inkvec_wasm.js?$V')|" "$h"
  grep -q "inkvec_wasm.js?$V" "$h" || { echo "helper patch failed: $h" >&2; exit 1; }
done
echo "patched rayon worker helper -> ../../../inkvec_wasm.js?$V"

echo
ls -l "$ROOT/web/pkg/inkvec_wasm_bg.wasm" "$ROOT/web/pkg-threads/inkvec_wasm_bg.wasm"
