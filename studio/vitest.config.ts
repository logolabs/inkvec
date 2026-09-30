import { defineConfig } from "vitest/config";

// The unit tests (`npm test`): the interface's pure logic, run in Node with happy-dom
// standing in for the few DOM calls (a paste event, a timer, `document.querySelectorAll`).
//
// Kept apart from `vite.config.ts` on purpose: that file builds two apps and loads the
// browser build's loading screen from disk, none of which a test needs. The build-time
// constants are defined here as the desktop build sets them, so a module that branches on
// `__INKVEC_WEB__` takes its desktop path, and none reaches for the network or a worker.
export default defineConfig({
  define: {
    __APP_VERSION__: JSON.stringify("0.0.0-test"),
    __INKVEC_WEB__: "false",
    __INKVEC_WASM_TOKEN__: JSON.stringify(""),
    __INKVEC_WASM_BYTES__: "{}",
  },
  test: {
    environment: "happy-dom",
    include: ["src/**/*.test.ts"],
    // Spies and stubbed globals are put back after every test, so none leaks into the next.
    restoreMocks: true,
    unstubGlobals: true,
  },
});
