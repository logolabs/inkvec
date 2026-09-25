# inkvec-server

Inkvec's HTTP service: `POST` an image, get back SVG. It is the `inkvec` crate (see
`../inkvec/README.md`) behind a small axum server, packaged as a Docker image
(`services/docker/Dockerfile`) so the tracer can run as a sidecar or a standalone endpoint
without embedding Rust, a C ABI or a Python wheel.

Like every other binding, this service does not know what an option is: it accepts options as
JSON (or query parameters typed against the same schema) and hands them to
`inkvec::Options::from_json`, returning the facade's own validation errors. Nothing about an
option is written in this crate; see `docs/BINDINGS.md`.

## Run it

With Docker:

```sh
docker build -f services/docker/Dockerfile -t inkvec-server .
docker run --rm -p 8080:8080 inkvec-server
```

Or natively, from the workspace:

```sh
cargo run --release -p inkvec-server
```

Then:

```sh
curl -sf --data-binary @logo.png -H 'Content-Type: image/png' \
  'http://localhost:8080/trace?colors=8&no_background=true' -o logo.svg

curl -sf -X POST -F image=@logo.png -F 'options={"colors":8}' \
  http://localhost:8080/trace -o logo.svg

curl -s http://localhost:8080/options/schema | jq .
curl -s http://localhost:8080/healthz
curl -s http://localhost:8080/version
```

## Endpoints

| Method | Path | Returns |
|---|---|---|
| `POST` | `/trace` | `image/svg+xml`, with `X-Inkvec-Width` / `X-Inkvec-Height` headers |
| `GET` | `/options/schema` | The options JSON Schema (`bindings/options.schema.json`, byte for byte) |
| `GET` | `/options/defaults` | Every option at its default, as JSON |
| `GET` | `/healthz` | `{"status": "ok"}` |
| `GET` | `/version` | `{"version": ..., "build_target": ...}` |
| `GET` | `/openapi.json` | The generated OpenAPI 3.1 document |

### `POST /trace`

The body is the image: raw bytes with `Content-Type: image/*` or `application/octet-stream`
(any of the encoded forms `inkvec::trace` reads -- PNG, JPEG, WebP, GIF, BMP, TIFF), or
`multipart/form-data` with an `image` part and an optional `options` part (a JSON object).

Options come from up to four places, a later one overriding a key an earlier one set:

1. generic query parameters matching a name in the options schema (`?colors=8`,
   `?no_background=true`), converted to that option's JSON type;
2. the `options` query parameter: the full options object as URL-encoded JSON
   (`?options=%7B%22colors%22%3A8%7D`);
3. the `X-Inkvec-Options` header: the full options object as JSON;
4. for a multipart request, the `options` part: the full options object as JSON.

The merged object is validated exactly once, by `inkvec::Options::from_json` -- an unknown
key, wrong type or out-of-range value is `422 invalid_options`, naming the option, same as
every other binding.

Responses:

| Status | Body | When |
|---|---|---|
| `200` | `image/svg+xml` | traced |
| `400` | `{"error": {"code": "invalid_image", "message": ...}}` | not a decodable image |
| `413` | `{"error": {"code": "too_large", "message": ...}}` | body over the size limit |
| `422` | `{"error": {"code": "invalid_options", "message": ...}}` | bad option |
| `500` | `{"error": {"code": "internal", "message": ...}}` | the pipeline panicked, or the request's server-side time cap elapsed |
| `503` | `{"error": {"code": "busy", "message": ...}}` | at `INKVEC_MAX_CONCURRENCY` traces in flight |

## Options

<!-- inkvec:options:begin -->
| Option | Type | Default | Range | Meaning |
|---|---|---|---|---|
| `precision` | number | `0.1` | > 0 | Sets the description-length cost of a coordinate, in pixels: lambda = ln(extent / precision). Smaller values buy more detail with more coordinates. It does not set the digits written; coordinates are always written at 2 decimals. |
| `min_area` | number | `2.0` | > 0 | Discard features smaller than this area, in square pixels. |
| `colors` | integer | `64` | >= 1 and <= 4096 | Maximum palette size. |
| `merge` | number | `0.035` | >= 0 | OKLab distance below which two colours are treated as one ink. |
| `max_dim` | integer | `2048` | - | Inputs larger than this on their longer side, in pixels, are traced at this size and the SVG is written at the original size. Trace time grows with the pixel count. 0 means no cap. |
| `time_budget` | number | `0.0` | >= 0 | Advisory wall-clock budget, in seconds; 0 means none. Gradient-band merging stops at 60% of it and the boundary solve gets 25%; the output is still a correct trace, with more fills or a less polished outline. A nonzero budget makes the output depend on machine speed and load, so it is no longer reproducible. |
| `margin` | number | `0.0` | >= 0 | Transparent margin around the output, as a fraction of the larger side. The viewBox grows; the geometry does not move. |
| `no_background` | bool | `false` | - | Knock the background out: the face that covers the whole canvas is not painted, so the artwork sits on transparency. |
| `minify` | bool | `false` | - | No ids or groups, no trailing zeros. Same geometry, typically about a tenth smaller. |
| `editability` | bool | `false` | - | Spend parameters on structure an artist can edit: joins between curves made G1-smooth, handles snapped to the axes and to 45 degrees, handles of one curve made equal in length, nodes that nearly share a coordinate made to share it, and rings that are their own mirror image locked into exact mirrors. Every change is guarded to the fit's own tolerance -- 3 sigma of the source point plus half a pixel, or 1.5 px for a mirror lock -- so the picture stays within a fraction of a pixel of the default trace; the price measured on 25 icons is about 0.04 dE00. Off by default. |
| `native_alpha` | bool | `true` | - | Trace transparency natively: each ink is a colour and an opacity, and the transparent ground is an ink of its own, instead of the image being composited onto a matte first. Holes stay holes, white artwork on a transparent ground traces, glows and shadows stay translucent, and a fade is one gradient of colour and opacity. An opaque input traces the same either way. On by default, as on the command line (where the environment variable INKVEC_NATIVE_ALPHA=0 turns the default off); false composites onto a matte first, as releases up to 0.1.3 did. |
| `cutout` | bool | `false` | - | With native_alpha off, carry the input's transparency into the SVG: a face the source drew transparent becomes a hole, one drawn at a single opacity keeps it as fill-opacity, and white artwork on a transparent ground survives. Changes nothing for an opaque input, and nothing with native_alpha on (the default), which already carries the transparency out. |
| `content_units` | bool | `false` | - | Scale the fit tolerances with the raster, so a large, simple drawing gets the parameter count of a small one. Trades fidelity for parsimony: small squares can come back as circles and thin rings broken. |
| `harmonize` | bool | `true` | - | Shape harmonization (on by default): marks that repeat across the drawing are redrawn from one consensus geometry per cluster, which saves parameters. A mark takes the consensus only where that stays within 0.1 px of the boundary traced for it and costs fewer parameters; a face another face is drawn against, and a fitted circle or rounded rectangle, is never moved. Set it to false to skip the pass. |
| `harmonize_threshold` | number | `0.92` | >= 0 and <= 1 | Shape-equivalence threshold for harmonization: the outline similarity (IoU after affine normalisation) above which two marks count as the same shape. |
| `merge_colors` | string | `""` | - | Colour groups: fills to draw as one, so the shapes between them join rather than being recoloured. Empty (the default) changes nothing. Groups are separated by ';' and members by ','; a member is a colour '#rrggbb' as it appears in a trace of the same image, or a gradient written as its stop colours joined by '>'. An optional '=' says what the group becomes: '=#rrggbb' a flat colour, '=@n' its n-th member (1-based; a gradient there is refitted over the whole group); without it, the member covering the most of the image. Example: '#c0392b,#e74c3c;#f00>#00f,#0a0=@1'. A group costs one extra trace. |
<!-- inkvec:options:end -->

The same table, as JSON Schema, is `GET /options/schema` -- generated from `inkvec::Options`
by `cargo test -p inkvec`, committed as `bindings/options.schema.json`. `GET /openapi.json`
embeds it verbatim as `components.schemas.Options`, generated from it in turn by
`bindings/codegen/openapi.py`.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `INKVEC_PORT` | `8080` | TCP port to bind (`0.0.0.0`) |
| `INKVEC_MAX_CONCURRENCY` | number of CPU cores | Traces allowed to run at once. A request past the limit gets `503 busy` immediately rather than queuing -- a CPU-bound endpoint that queued would just move the wait from the network into memory. |
| `INKVEC_MAX_BODY_BYTES` | `20971520` (20 MB) | Request body cap, enforced while reading (not just `Content-Length`), for both the raw-bytes and multipart forms. Over it is `413 too_large`. |
| `INKVEC_REQUEST_TIMEOUT_SECS` | `60` | Server-side wall-clock cap on one trace; `0` disables it. See "time_budget is advisory" below. |
| `INKVEC_DEFAULT_TIME_BUDGET` | unset | If set, and a request does not itself set `time_budget`, this value (seconds) is used instead of the facade's default (no budget). This is the *other* way to bound trace time -- see below. |
| `RUST_LOG` | `info` | `tracing`'s `EnvFilter` syntax, e.g. `inkvec_server=debug,tower_http=debug` |

## `time_budget` is advisory (known limitation)

The CHANGELOG's release notes for the CLI's `--time-budget` say it plainly: it "does not
bound how long tracing takes on noise-like input, only on well-behaved artwork." This service
adds a server-side backstop (`INKVEC_REQUEST_TIMEOUT_SECS`) on top of it, but the backstop is
coarse: Rust cannot interrupt a running OS thread, so when the cap elapses the request that
triggered it gets its `500` response and stops waiting, while the `spawn_blocking` worker
thread actually running the trace keeps running until the pipeline finishes or panics on its
own. Under sustained abuse (very large, adversarial or noise-like input sent faster than
traces finish) this can accumulate blocked worker threads faster than `INKVEC_MAX_CONCURRENCY`
alone would suggest; size the concurrency limit and the timeout together, and prefer capping
input at the reverse proxy (`INKVEC_MAX_DIM`-equivalent is `max_dim` in the options, not an
env var, since it is a normal Inkvec option) over relying on the timeout as a hard bound.

## Security notes

* **No authentication.** This service traces whatever bytes it is given and returns SVG; it
  has no notion of a caller. Put it behind your own auth (a reverse proxy, an API gateway) and
  rate limiting before exposing it beyond a trusted network.
* **CPU-bound.** A trace is real work (the pipeline runs on rayon's global thread pool inside
  each `spawn_blocking` task); `INKVEC_MAX_CONCURRENCY` and `INKVEC_MAX_BODY_BYTES` are the
  only built-in defenses against resource exhaustion. Set both deliberately for the machine
  this runs on, and consider a request-rate limit in front of it.
* **No secrets, no persistence.** The service reads no files besides its own binary, writes
  nothing to disk, and holds a trace's image and options only in memory for the request's
  lifetime -- the same "no I/O" guarantee as the `inkvec` crate itself.
* **Non-root container.** The image runs as a dedicated non-root user (see the Dockerfile);
  do not override that in deployment.

## Building and testing locally

```sh
cargo test --release -p inkvec-server
cargo clippy -p inkvec-server --all-targets
```

The integration tests (`tests/`) build the same router `main.rs` serves, run the shared
contract (`bindings/contract/cases.json`) over it through every input form (raw bytes,
multipart, options via query, header and JSON body), and check error mapping, the
white-on-transparent default, and the concurrency limit.

## Docker

See `services/docker/Dockerfile` (multi-stage: `rust:1.98-bookworm` builder,
`gcr.io/distroless/cc-debian12:nonroot` runtime) and `services/docker/compose.yaml` for a
runnable example. `.dockerignore` at the repository root keeps the build context to the
crates the image actually needs.
