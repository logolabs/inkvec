package inkvec

import _ "embed"

// wasmModule is the tracer: Inkvec's C ABI (crates/inkvec-ffi) compiled for wasm32-wasip1.
//
// In the monorepo it is built by tools/build_go_wasm.sh and not committed; until that has
// run, this package does not compile ("pattern inkvec.wasm: no matching files found"). The
// published module, github.com/logolabs/inkvec-go, carries it.
//
//go:embed inkvec.wasm
var wasmModule []byte
