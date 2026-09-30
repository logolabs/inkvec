/**
 * The dev mock's entry point (`dev/index.html`, served by `npm run mock`): installs the mock
 * backend (`mock.ts`) before the app's own modules load, then starts the real app on it.
 */

await import("./mock");
const app = await import("../src/main");

// The store, for `tools/mock_checks.py` to read: importing main.ts again from the page would
// start a second copy of the app whenever vite has reloaded it under a new URL.
(window as unknown as { __store: unknown }).__store = app.store;

export {};
