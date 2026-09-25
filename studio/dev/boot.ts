await import("./mock");
const app = await import("../src/main");

// The store, for `tools/mock_checks.py` to read: importing main.ts again from the page would
// start a second copy of the app whenever vite has reloaded it under a new URL.
(window as unknown as { __store: unknown }).__store = app.store;

export {};
