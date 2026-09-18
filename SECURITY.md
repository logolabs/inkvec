# Security Policy

## Supported versions

Inkvec is pre-1.0. Only the latest published `0.x` release is supported with security
fixes; please upgrade before reporting an issue against an older version.

## Reporting a vulnerability

Please **do not** open a public issue for a security vulnerability. Instead, use GitHub's
private vulnerability reporting:

1. Go to the [Security tab](https://github.com/logolabs/inkvec/security) of this
   repository.
2. Click "Report a vulnerability" to open a private advisory.

If you cannot use GitHub's reporting flow for some reason, contact the maintainers at
**security@\<domain\>** (placeholder — the maintainer will fill in a real address before
launch).

Please include:

- the Inkvec version (`inkvec --version`) and platform,
- the command line or API call used,
- the input file (or a minimized reproduction) if you can share it, and
- what you expected versus what happened.

## Scope

In scope:

- **Malformed or adversarial image input** — crashes, panics, excessive memory/CPU use,
  or memory-safety issues while decoding or tracing a PNG, JPEG, WebP, GIF, BMP or TIFF.
- **Command-line and library argument handling** — flag parsing, path handling, and any
  external command invoked via flags such as `--sr-command`.
- **The browser build** (`crates/inkvec-wasm`, `web/`) — anything that would let a hosted
  page misbehave beyond what the user's own input authorizes, given tracing runs
  entirely client-side and nothing is uploaded.

Out of scope:

- The optional trained restorer network itself (`restorer.onnx`) — it is not part of this
  repository and is distributed separately.
- Issues that require an attacker to already control the machine running `inkvec` or to
  modify the binary or its dependencies.
- The benchmark harness (`bench/`) and research tooling, which are developer-only and not
  part of any shipped artifact.

## Response expectations

This is a small open-source project without a dedicated security team. We aim to
acknowledge new reports within a week and to keep you updated as we investigate, but we
do not commit to a fixed resolution SLA before 1.0. Coordinated disclosure is
appreciated; we will credit reporters (with permission) in the release notes.
