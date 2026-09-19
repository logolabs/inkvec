#!/usr/bin/env bash
# Build the WebAssembly module the Go package (packages/go) embeds.
#
#   tools/build_go_wasm.sh        -> packages/go/inkvec.wasm
#
# It is the C ABI crate (crates/inkvec-ffi) compiled for wasm32-wasip1 as a cdylib: the same
# `inkvec_*` functions as the native library, plus `inkvec_alloc` / `inkvec_dealloc` so the
# host can place bytes in the module's memory. The Go package runs it with wazero, so Go
# users need no cgo and no C toolchain. Single-threaded: WASI preview 1 has no threads, and
# rayon runs its work on the calling thread.
#
# `go get` cannot run a build step, so the Go mirror repository (github.com/logolabs/inkvec-go)
# commits the module this script writes; in this repository it is ignored and rebuilt
# (.github/workflows/go.yml).
#
# Three steps:
#
#   cargo      the module, without SIMD (see step 3)
#   wasm-opt   -O3, optional: INKVEC_WASM_OPT, else `wasm-opt` on PATH, else the copy
#              wasm-pack keeps in its cache; without one the module stays unoptimised
#   python     tools/wasm_float_select.py rewrites every floating-point `select` as an
#              integer one, around a wazero compiler bug on amd64 that makes a float
#              `select` return the wrong operand (details in that file). It decodes every
#              instruction and does not handle SIMD, which measured neither faster nor
#              slower under wazero, so the module is built without it.
#
# Prerequisites: rustup target add wasm32-wasip1; Python 3 (PYTHON, else python3 / python).
#
#   INKVEC_GO_WASM_OUT   where the module goes (default packages/go/inkvec.wasm)
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT=$(pwd)
OUT="${INKVEC_GO_WASM_OUT:-$ROOT/packages/go/inkvec.wasm}"
TARGET=wasm32-wasip1

# Paths as rustc sees them (Windows paths under Git Bash), to strip them from the module:
# panic locations would otherwise carry the builder's checkout and cargo home. Written as
# CARGO_ENCODED_RUSTFLAGS (0x1f-separated) because either path can hold spaces.
native() { cygpath -w "$1" 2>/dev/null || printf '%s' "$1"; }
CARGO_HOME_DIR="${CARGO_HOME:-$HOME/.cargo}"
flags=(
  "--remap-path-prefix=$(native "$ROOT")=inkvec"
  "--remap-path-prefix=$(native "$CARGO_HOME_DIR")=cargo"
)
CARGO_ENCODED_RUSTFLAGS=$(IFS=$'\x1f'; printf '%s' "${flags[*]}")
export CARGO_ENCODED_RUSTFLAGS

echo "==> cargo: inkvec-ffi for $TARGET"
cargo rustc --release --locked -p inkvec-ffi --target "$TARGET" --crate-type cdylib
BUILT="$ROOT/target/$TARGET/release/inkvec_ffi.wasm"

find_wasm_opt() {
  if [ -n "${INKVEC_WASM_OPT:-}" ]; then printf '%s' "$INKVEC_WASM_OPT"; return; fi
  if command -v wasm-opt >/dev/null 2>&1; then command -v wasm-opt; return; fi
  local cache
  for cache in "${LOCALAPPDATA:-}/.wasm-pack" "$HOME/.cache/.wasm-pack" "$HOME/Library/Caches/.wasm-pack"; do
    for w in "$cache"/wasm-opt-*/bin/wasm-opt "$cache"/wasm-opt-*/bin/wasm-opt.exe; do
      if [ -x "$w" ]; then printf '%s' "$w"; return; fi
    done
  done
}

mkdir -p "$(dirname "$OUT")"
OPTIMISED="$ROOT/target/$TARGET/release/inkvec_ffi.opt.wasm"
WASM_OPT=$(find_wasm_opt || true)
if [ -n "$WASM_OPT" ]; then
  echo "==> wasm-opt: $WASM_OPT"
  # The WebAssembly 2.0 features wazero runs by default (api.CoreFeaturesV2), which are the
  # ones rustc uses here. rustc also marks extended-const as allowed; a module like this one
  # (not position-independent) has no use for it, and wazero would refuse it without an
  # experimental flag, so wasm-opt must not introduce it either. The output has no custom
  # sections: wasm-opt drops the name section (0.6 MB of function names, which only a trap's
  # stack trace would show) unless given -g, and the two --strip flags drop the rest.
  "$WASM_OPT" -O3 --strip-producers --strip-target-features \
    --enable-bulk-memory --enable-mutable-globals \
    --enable-nontrapping-float-to-int --enable-sign-ext --enable-multivalue \
    --enable-reference-types --disable-extended-const \
    "$BUILT" -o "$OPTIMISED"
else
  echo "==> no wasm-opt found; the module stays unoptimised"
  cp "$BUILT" "$OPTIMISED"
fi

PY="${PYTHON:-$(command -v python3 || command -v python)}"
echo "==> $PY tools/wasm_float_select.py"
"$PY" "$ROOT/tools/wasm_float_select.py" "$OPTIMISED" "$OUT"

ls -l "$BUILT" "$OUT"
