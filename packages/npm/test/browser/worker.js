// The threaded build in a Web Worker on a cross-origin isolated page: the configuration the
// README asks for.
import { init, threadCount, trace } from "/dist/threads.js";

const sha = async (s) =>
  [...new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s)))]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");

try {
  await init(undefined, 4);
  const png = new Uint8Array(await (await fetch("/samples/tiny.png")).arrayBuffer());
  const t = performance.now();
  const svg = await trace(png);
  postMessage({ sha: await sha(svg), bytes: svg.length, ms: performance.now() - t, threads: threadCount() });
} catch (e) {
  postMessage({ error: String((e && e.stack) || e) });
}
