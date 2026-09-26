// The single-threaded entry, `@logolabs/inkvec`, through its public name (the package
// resolves itself, so this exercises the `exports` map as a user would).

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

import * as inkvec from "@logolabs/inkvec";
import { InkvecError, defaults, optionsSchema, trace, traceRGBA, version } from "@logolabs/inkvec";
import {
  ROOT,
  cargoVersion,
  fills,
  rawGlue,
  sample,
  samples,
  translucentDisk,
  whiteDisk,
} from "./helpers.mjs";
import { decodePng, encodePng } from "./png.mjs";

const rejectsWith = (p, code, pattern) =>
  assert.rejects(p, (e) => {
    assert.ok(e instanceof InkvecError, `expected InkvecError, got ${e?.constructor?.name}: ${e}`);
    assert.equal(e.code, code, e.message);
    if (pattern) assert.match(e.message, pattern);
    return true;
  });

test("the public surface", () => {
  assert.deepEqual(Object.keys(inkvec).sort(), [
    "InkvecError",
    "defaults",
    "init",
    "optionsSchema",
    "trace",
    "traceRGBA",
    "version",
  ]);
});

test("version() is the workspace version, and the WebAssembly's", async () => {
  assert.equal(version(), cargoVersion());
  await inkvec.init();
  assert.equal((await rawGlue("st")).version(), version());
});

test("optionsSchema() is bindings/options.schema.json, read from Rust", async () => {
  const committed = JSON.parse(readFileSync(join(ROOT, "bindings/options.schema.json"), "utf8"));
  assert.deepEqual(await optionsSchema(), committed);
});

test("defaults() gives every option, each at the schema's default", async () => {
  const schema = await optionsSchema();
  const d = await defaults();
  assert.deepEqual(Object.keys(d).sort(), Object.keys(schema.properties).sort());
  for (const [k, prop] of Object.entries(schema.properties)) {
    assert.deepEqual(d[k], prop.default, k);
  }
});

test("the option types are generated from the current schema", (t) => {
  const gen = join(ROOT, "bindings/codegen/generate.py");
  const python = [process.env.INKVEC_PYTHON, "python3", "python"].find(
    (p) => p && spawnSync(p, ["--version"]).status === 0,
  );
  if (!python) return t.skip("no Python 3 to run bindings/codegen");
  const r = spawnSync(python, [gen, "--check"], { encoding: "utf8" });
  assert.equal(r.status, 0, r.stderr || r.stdout);
});

test("every sample traces, and equals the raw WebAssembly exports byte for byte", async (t) => {
  const glue = await rawGlue("st");
  const d = await defaults();
  for (const { name, bytes } of samples()) {
    await t.test(name, async () => {
      const svg = await trace(bytes);
      assert.match(svg, /^<svg[\s>]/);
      assert.match(svg, /<\/svg>\s*$/);
      assert.equal(glue.trace_json(bytes, "{}"), svg, "trace_json");
      // The export the Space calls (`prepare(...).traceOnce()` inside), with no options
      // and with every option spelled out at its default.
      assert.equal(glue.trace(bytes, "{}"), svg, "trace, no options");
      assert.equal(glue.trace(bytes, JSON.stringify(d)), svg, "trace, explicit defaults");
    });
  }
});

test("the same input and options give the same bytes", async () => {
  for (const name of ["tiny.png", "1075thefan.png"]) {
    const bytes = sample(name);
    const a = await trace(bytes, { colors: 16 });
    const b = await trace(bytes, { colors: 16 });
    assert.equal(a, b, name);
  }
});

test("options reach the tracer", async () => {
  const bytes = sample("tiny.png");
  const plain = await trace(bytes);
  assert.notEqual(await trace(bytes, { colors: 2 }), plain, "colors");
  assert.notEqual(await trace(bytes, { margin: 0.1 }), plain, "margin");
  const minified = await trace(bytes, { minify: true });
  assert.ok(minified.length < plain.length, "minify");
  assert.equal(await trace(bytes, {}), plain, "{} is the defaults");
  assert.equal(await trace(bytes, await defaults()), plain, "defaults() is the defaults");
});

test("every byte container is accepted and traces the same", async () => {
  const buf = sample("tiny.png");
  const expected = await trace(buf);
  const u8 = new Uint8Array(buf);
  const ab = u8.buffer.slice(u8.byteOffset, u8.byteOffset + u8.byteLength);
  const padded = new Uint8Array(ab.byteLength + 16);
  padded.set(u8, 8);
  for (const [what, input] of [
    ["Uint8Array", u8],
    ["ArrayBuffer", ab],
    ["DataView over an offset", new DataView(padded.buffer, 8, u8.byteLength)],
    ["Blob", new Blob([u8], { type: "image/png" })],
  ]) {
    assert.equal(await trace(input), expected, what);
  }
});

test("traceRGBA on a PNG's pixels equals trace on the PNG", async () => {
  for (const name of ["tiny.png", "1075thefan.png"]) {
    const png = sample(name);
    const img = decodePng(png);
    const expected = await trace(png);
    assert.equal(await traceRGBA(img), expected, `${name} as ImageData`);
    assert.equal(
      await traceRGBA(new Uint8Array(img.data.buffer), img.width, img.height),
      expected,
      `${name} as bytes`,
    );
  }
  const png = sample("tiny.png");
  const img = decodePng(png);
  assert.equal(
    await traceRGBA(img, { max_dim: 48 }),
    await trace(png, { max_dim: 48 }),
    "capped by max_dim",
  );
});

for (const [what, options] of [
  ["natively (the default)", {}],
  ["composited, with cutout", { native_alpha: false, cutout: true }],
]) {
  test(`white artwork on a transparent ground keeps its transparency ${what}`, async () => {
    const img = whiteDisk(64);
    const svg = await traceRGBA(img, options);
    assert.equal(await trace(encodePng(img), options), svg, "PNG and pixels agree");
    const painted = fills(svg).filter((f) => f !== "none");
    assert.ok(painted.length > 0, `the disk is drawn: ${svg}`);
    for (const f of painted) {
      assert.match(f, /^(#fff|#ffffff|white)$/, `only white is painted, found ${f}: ${svg}`);
    }
    assert.doesNotMatch(svg, /<rect\b/, `no canvas rectangle: ${svg}`);
  });
}

test("native_alpha: a translucent disk keeps its opacity; off, it is composited", async () => {
  const img = translucentDisk(64);
  assert.equal((await defaults()).native_alpha, true);
  const native = await traceRGBA(img);
  assert.match(native, /fill-opacity=/, `own colour at its own opacity: ${native}`);
  assert.doesNotMatch(native, /<rect\b/, `no canvas rectangle: ${native}`);
  const composited = await traceRGBA(img, { native_alpha: false });
  assert.doesNotMatch(composited, /fill-opacity=/, `opaque, composited onto white: ${composited}`);
});

test("options are checked by the tracer", async () => {
  const bytes = sample("tiny.png");
  await rejectsWith(trace(bytes, { colours: 8 }), "invalid_options", /colours/);
  await rejectsWith(trace(bytes, { colors: 0 }), "invalid_options", /colors/);
  await rejectsWith(trace(bytes, { colors: "16" }), "invalid_options");
  await rejectsWith(trace(bytes, { precision: 0 }), "invalid_options", /precision/);
  await rejectsWith(trace(bytes, { precision: Number.NaN }), "invalid_options");
  await rejectsWith(trace(bytes, { harmonize_threshold: 2 }), "invalid_options", /harmonize_threshold/);
  await rejectsWith(trace(bytes, [1, 2]), "invalid_options", /object/);
  await rejectsWith(trace(bytes, 16), "invalid_options", /object/);
});

test("input is checked", async () => {
  await rejectsWith(trace("logo.png"), "invalid_image", /bytes, not a path/);
  await rejectsWith(trace(42), "invalid_image");
  await rejectsWith(trace(new Uint8Array([1, 2, 3, 4])), "invalid_image");
  await rejectsWith(trace(new Uint8Array(0)), "invalid_image");
  const { data } = whiteDisk(8);
  await rejectsWith(traceRGBA(data, 8, 7), "invalid_image", /8x7/);
  await rejectsWith(traceRGBA(data, 0, 8), "invalid_image", /width/);
  await rejectsWith(traceRGBA(data, 8.5, 8), "invalid_image", /width/);
  await rejectsWith(traceRGBA(data, -8, 8), "invalid_image", /width/);
  await rejectsWith(traceRGBA([0, 0, 0, 0], 1, 1), "invalid_image", /Uint8ClampedArray/);
  await rejectsWith(traceRGBA({ data, width: 8 }), "invalid_image");
});

test("a failed call leaves the instance usable", async () => {
  const bytes = sample("tiny.png");
  const before = await trace(bytes);
  await rejectsWith(trace(new Uint8Array([0x89, 0x50, 0x4e, 0x47])), "invalid_image");
  assert.equal(await trace(bytes), before);
});
