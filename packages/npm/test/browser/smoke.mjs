#!/usr/bin/env node
// Browser smoke test: serve the built package over HTTP with the cross-origin isolation
// headers, open test/browser/page.html in headless Chrome, and check what it reports
// against Node.js's own trace of the same file.
//
//   node test/browser/smoke.mjs            finds Chrome, or set CHROME=/path/to/chrome
//
// Exit 0 = passed, 1 = failed, 2 = no Chrome found (skipped).

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { extname, join, normalize } from "node:path";

import { trace, version } from "@logolabs/inkvec";
import { PKG, SAMPLES, sha256 } from "../helpers.mjs";

function findChrome() {
  const candidates = [
    process.env.CHROME,
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  ];
  return candidates.find((p) => p && existsSync(p));
}

const TYPES = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".wasm": "application/wasm",
  ".png": "image/png",
};

const chrome = findChrome();
if (!chrome) {
  console.log("no Chrome found; set CHROME to run the browser smoke test");
  process.exit(2);
}

let resolveResult;
const result = new Promise((r) => (resolveResult = r));

const server = createServer((req, res) => {
  const url = new URL(req.url, "http://localhost");
  if (req.method === "POST" && url.pathname === "/result") {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.end("ok");
      resolveResult(JSON.parse(body));
    });
    return;
  }
  const rel = normalize(decodeURIComponent(url.pathname)).replace(/^[\\/]+/, "");
  const file = rel.startsWith("samples") ? join(SAMPLES, rel.slice("samples".length)) : join(PKG, rel);
  if (!file.startsWith(PKG) && !file.startsWith(SAMPLES)) {
    res.writeHead(403).end();
    return;
  }
  if (!existsSync(file)) {
    res.writeHead(404).end();
    return;
  }
  res.writeHead(200, {
    "Content-Type": TYPES[extname(file)] ?? "application/octet-stream",
    "Cross-Origin-Opener-Policy": "same-origin",
    "Cross-Origin-Embedder-Policy": "require-corp",
    "Cache-Control": "no-store",
  });
  res.end(readFileSync(file));
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const { port } = server.address();

const profile = mkdtempSync(join(tmpdir(), "inkvec-chrome-"));
const browser = spawn(
  chrome,
  [
    "--headless=new",
    "--disable-gpu",
    // CI runners do not offer the user namespaces Chrome's sandbox needs.
    ...(process.env.CI ? ["--no-sandbox"] : []),
    "--no-first-run",
    "--no-default-browser-check",
    `--user-data-dir=${profile}`,
    `http://127.0.0.1:${port}/test/browser/page.html`,
  ],
  { stdio: "ignore" },
);

const timeout = new Promise((_, reject) =>
  setTimeout(() => reject(new Error("no result from the page in 180 s")), 180_000),
);

let failed = 0;
const check = (ok, what, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}${detail ? `  (${detail})` : ""}`);
  if (!ok) failed++;
};

try {
  const r = await Promise.race([result, timeout]);
  if (r.error) throw new Error(r.error);
  const expected = sha256(await trace(readFileSync(join(SAMPLES, "tiny.png"))));
  check(r.isolated === true, "page is cross-origin isolated");
  check(r.version === version(), "version", r.version);
  check(r.st?.sha === expected, "single-threaded trace equals Node.js", `${r.st?.bytes} B, ${r.st?.ms?.toFixed(0)} ms`);
  check(r.rgba?.sha === expected, "traceRGBA on canvas ImageData equals the PNG's trace");
  check(
    /load_failed: .*Web Worker/.test(r.mainThreadThreads ?? ""),
    "threads refuse the main thread",
    r.mainThreadThreads,
  );
  check(
    r.mt?.sha === expected && r.mt?.threads === 4,
    "threaded trace in a worker equals Node.js",
    r.mt?.error ?? `${r.mt?.threads} threads, ${r.mt?.ms?.toFixed(0)} ms`,
  );
} catch (e) {
  console.log(`FAIL ${e.message}`);
  failed++;
} finally {
  browser.kill();
  server.close();
  await new Promise((r) => setTimeout(r, 500));
  try {
    rmSync(profile, { recursive: true, force: true });
  } catch {}
}
console.log(failed ? `${failed} check(s) failed` : "browser smoke test passed");
process.exit(failed ? 1 : 0);
