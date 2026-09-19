# Changelog

All notable changes to `@logolabs/inkvec` are documented here. The package's version is the
Inkvec workspace version, so the tracer's own changes are in the repository's
[CHANGELOG](https://github.com/logolabs/inkvec/blob/main/CHANGELOG.md); this file records
what changed in the JavaScript package itself. Before 1.0 the API may change between minor
versions.

## [Unreleased]

### Added

- First release: `trace`, `traceRGBA`, `init`, `defaults`, `optionsSchema`, `version` and
  `InkvecError`, over the single-threaded WebAssembly build, for browsers, Node.js, Deno and
  Bun from one ES module entry.
- `@logolabs/inkvec/threads`: the threaded build, for cross-origin isolated pages (inside a
  Web Worker) and Node.js (`worker_threads`).
- TypeScript types for the options, generated from the Rust options schema.
