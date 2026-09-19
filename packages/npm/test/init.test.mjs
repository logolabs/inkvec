// `init` with an explicit source, in a process of its own so the first load is this one.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { InkvecError, init, trace } from "@logolabs/inkvec";
import { sample } from "./helpers.mjs";

const wasmUrl = new URL("../wasm/st/inkvec_wasm_bg.wasm", import.meta.url);

test("a load that fails is reported and forgotten", async () => {
  await assert.rejects(init(new Uint8Array([0, 97, 115, 109, 9, 9, 9, 9])), (e) => {
    assert.ok(e instanceof InkvecError);
    assert.equal(e.code, "load_failed");
    return true;
  });
  await assert.rejects(init("/no/such/dir/inkvec.wasm"), (e) => e.code === "load_failed");
});

test("init takes the module's bytes, a compiled module, a path or a file: URL", async () => {
  const bytes = readFileSync(wasmUrl);
  // The first source that loads wins; the rest resolve without loading again.
  await init(bytes);
  await init(await WebAssembly.compile(bytes));
  await init(fileURLToPath(wasmUrl));
  await init(wasmUrl);
  assert.match(await trace(sample("tiny.png")), /^<svg/);
});
