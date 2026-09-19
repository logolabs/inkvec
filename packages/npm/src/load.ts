// Loads the single-threaded build.
//
// The default source is the `.wasm` beside the bindings, as `new URL(..., import.meta.url)`
// -- the pattern bundlers rewrite to an emitted asset. In a browser that URL is fetched; in
// Node.js it is a `file:` URL and is read from disk.

import * as glue from "../wasm/st/inkvec_wasm.js";
import type { Glue, WasmSource } from "./common.js";
import { resolveSource } from "./runtime.js";

export async function load(source?: WasmSource): Promise<Glue> {
  const wasm = await resolveSource(
    source ?? new URL("../wasm/st/inkvec_wasm_bg.wasm", import.meta.url),
  );
  await glue.default({ module_or_path: wasm as never });
  return glue as unknown as Glue;
}
