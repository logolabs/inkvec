// The first thing each thread of the Node.js pool runs (see `load-threads.ts`): the
// globals a Web Worker has and the rayon helper expects, over `worker_threads`, and then
// the helper itself. This thread belongs to the pool alone, so defining globals here
// touches nothing of the caller's.

import { builtin } from "./runtime.js";

type Listener = (event: { data: unknown }) => void;
type Port = {
  postMessage(data: unknown): void;
  on(type: string, fn: (data: unknown) => void): void;
  off(type: string, fn: (data: unknown) => void): void;
};

const threads = builtin<{ parentPort: Port | null; workerData: { url: string } }>("worker_threads");
if (!threads?.parentPort) throw new Error("node-worker.js runs only as an inkvec pool thread");
const port = threads.parentPort;
const wrapped = new Map<Listener, (data: unknown) => void>();
const scope = globalThis as Record<string, unknown>;

scope.self = globalThis;
scope.postMessage = (data: unknown) => port.postMessage(data);
scope.addEventListener = (type: string, fn: Listener) => {
  const h = (data: unknown) => fn({ data });
  wrapped.set(fn, h);
  port.on(type, h);
};
scope.removeEventListener = (type: string, fn: Listener) => {
  const h = wrapped.get(fn);
  if (h) port.off(type, h);
  wrapped.delete(fn);
};

await import(threads.workerData.url);
