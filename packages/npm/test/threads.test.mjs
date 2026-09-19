// The threaded entry in Node.js: the pool starts on worker_threads, leaves no globals
// behind, and traces byte-identically to the single-threaded build.

import assert from "node:assert/strict";
import { test } from "node:test";

import * as single from "@logolabs/inkvec";
import * as threaded from "@logolabs/inkvec/threads";
import { rawGlue, sample, samples } from "./helpers.mjs";
import { decodePng } from "./png.mjs";

test("the public surface", () => {
  assert.deepEqual(Object.keys(threaded).sort(), [
    "InkvecError",
    "defaults",
    "init",
    "optionsSchema",
    "threadCount",
    "trace",
    "traceRGBA",
    "version",
  ]);
});

test("init rejects a bad pool size", async () => {
  await assert.rejects(threaded.init(undefined, 0), RangeError);
  await assert.rejects(threaded.init(undefined, 1.5), RangeError);
  assert.equal(threaded.threadCount(), 0);
});

test("the pool starts, and leaves no browser globals behind", async () => {
  await threaded.init(undefined, 4);
  assert.equal(threaded.threadCount(), 4);
  assert.equal((await rawGlue("threads")).threads_available(), true);
  assert.equal(typeof globalThis.Worker, "undefined");
  assert.equal(typeof globalThis.self, "undefined");
  // A second init is a no-op, whatever it asks for.
  await threaded.init(undefined, 8);
  assert.equal(threaded.threadCount(), 4);
});

test("same output as the single-threaded build", async (t) => {
  for (const { name, bytes } of samples()) {
    await t.test(name, async () => {
      const [a, b] = [await single.trace(bytes), await threaded.trace(bytes)];
      assert.equal(b, a);
    });
  }
  const img = decodePng(sample("tiny.png"));
  assert.equal(
    await threaded.traceRGBA(img, { cutout: true }),
    await single.traceRGBA(img, { cutout: true }),
    "traceRGBA",
  );
});

test("defaults and schema match the single-threaded build", async () => {
  assert.deepEqual(await threaded.defaults(), await single.defaults());
  assert.deepEqual(await threaded.optionsSchema(), await single.optionsSchema());
  assert.equal(threaded.version(), single.version());
});
