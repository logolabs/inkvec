// The cross-language contract, bindings/contract/cases.json, checked the way
// crates/inkvec/tests/contract.rs checks it:
//
//   - `expect`: the kind of error, or the reported size. The JavaScript API returns the SVG
//     alone, so the size is read from its root `width`/`height`, which carry it unless a
//     `margin` grew them (those cases are not size-checked here);
//   - `svg[target]`: the SVG's length and SHA-256 on this build target, `wasm32-unknown`
//     for both WebAssembly builds. A target without hashes is reported, not failed, unless
//     INKVEC_CONTRACT_REQUIRE_HASH=1; INKVEC_BLESS=add records them;
//   - `same_svg_as`: identical SVG to the named case.
//
// Both entries run every case, so the threaded build is held to the same bytes.

import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

import * as single from "@logolabs/inkvec";
import * as threaded from "@logolabs/inkvec/threads";
import { CONTRACT, rawGlue, sha256 } from "./helpers.mjs";

const FILE = join(CONTRACT, "cases.json");
const doc = JSON.parse(readFileSync(FILE, "utf8"));
const bless = process.env.INKVEC_BLESS === "add";
const requireHash = process.env.INKVEC_CONTRACT_REQUIRE_HASH === "1";

async function runCase(api, c) {
  const bytes = new Uint8Array(readFileSync(join(CONTRACT, c.input)));
  try {
    const svg =
      c.form === "rgba"
        ? await api.traceRGBA(bytes, c.width, c.height, c.options)
        : await api.trace(bytes, c.options);
    return { svg };
  } catch (e) {
    return { error: e.code, message: e.message };
  }
}

const rootSize = (svg) => {
  const root = /^<svg\b[^>]*>/.exec(svg)?.[0] ?? "";
  const attr = (k) => Number(new RegExp(`\\s${k}="([0-9.]+)"`).exec(root)?.[1]);
  return { width: attr("width"), height: attr("height") };
};

for (const [label, api, arm] of [
  ["@logolabs/inkvec", single, "st"],
  ["@logolabs/inkvec/threads", threaded, "threads"],
]) {
  test(`contract: ${label}`, async (t) => {
    if (api === threaded) await threaded.init(undefined, 2);
    else await single.init();
    const target = (await rawGlue(arm)).build_target();
    assert.equal(target, "wasm32-unknown");

    const svgs = new Map();
    const unhashed = [];
    for (const c of doc.cases) {
      await t.test(c.name, async () => {
        const got = await runCase(api, c);
        if (c.expect.error) {
          assert.equal(got.error, c.expect.error, got.message ?? "traced without an error");
          return;
        }
        assert.equal(got.error, undefined, got.message);
        svgs.set(c.name, got.svg);
        if (!c.options.margin) {
          assert.deepEqual(rootSize(got.svg), { width: c.expect.width, height: c.expect.height });
        }
        const digest = { bytes: Buffer.byteLength(got.svg, "utf8"), sha256: sha256(got.svg) };
        const want = c.svg?.[target];
        if (bless && arm === "st") {
          c.svg = { ...(c.svg ?? {}), [target]: digest };
        } else if (want) {
          assert.deepEqual(digest, want, `SVG on ${target}`);
        } else {
          unhashed.push(`${c.name}: ${JSON.stringify(digest)}`);
        }
      });
    }

    await t.test("same_svg_as", () => {
      for (const c of doc.cases.filter((c) => c.same_svg_as)) {
        assert.ok(svgs.has(c.same_svg_as), `${c.same_svg_as} produced no SVG`);
        assert.equal(svgs.get(c.name), svgs.get(c.same_svg_as), `${c.name} vs ${c.same_svg_as}`);
      }
    });

    if (bless && arm === "st") {
      writeFileSync(FILE, JSON.stringify(doc, null, 2) + "\n");
      t.diagnostic(`recorded ${target} hashes in bindings/contract/cases.json`);
    } else if (unhashed.length) {
      const msg = `no SVG hashes recorded for ${target}; checked everything else:\n  ${unhashed.join("\n  ")}`;
      if (requireHash) assert.fail(msg);
      t.diagnostic(msg);
    }
  });
}
