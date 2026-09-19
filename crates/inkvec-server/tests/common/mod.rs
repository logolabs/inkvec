//! Shared helpers for the integration tests. `tests/common/mod.rs` (not `tests/common.rs`) so
//! cargo does not treat it as its own test binary.
//!
//! Compiled once per test binary (`contract.rs`, `server.rs`), each of which uses a different
//! subset -- hence `allow(dead_code)` rather than trimming helpers a sibling binary needs.

#![allow(dead_code)]

use axum::body::{to_bytes, Body};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode};
use axum::Router;
use inkvec_server::AppState;
use std::path::PathBuf;
use std::time::Duration;
use tower::ServiceExt;

/// `bindings/contract`, the fixtures and cases every binding is tested against.
pub(crate) fn contract_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bindings/contract")
}

/// A state with generous limits and no server-side timeout, so ordinary tests are not
/// sensitive to machine speed. Tests that exercise a specific limit build their own
/// `AppState` instead.
pub(crate) fn test_state() -> AppState {
    AppState::new(8, 20 * 1024 * 1024, None, None)
}

pub(crate) fn app() -> Router {
    inkvec_server::app(test_state())
}

/// The response, decoded into (status, headers, body bytes), from one request against a
/// (cheaply cloneable) router.
pub(crate) async fn send(app: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.expect("request");
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = to_bytes(resp.into_body(), 64 * 1024 * 1024)
        .await
        .expect("response body");
    (status, headers, body.to_vec())
}

/// `POST {uri}` with a raw body and an optional `Content-Type`.
pub(crate) async fn post_raw(
    app: &Router,
    uri: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method(Method::POST).uri(uri);
    if let Some(ct) = content_type {
        builder = builder.header("content-type", ct);
    }
    let req = builder.body(Body::from(body)).expect("request");
    send(app, req).await
}

/// `POST {uri}` with an extra header and a raw body.
pub(crate) async fn post_raw_with_header(
    app: &Router,
    uri: &str,
    header_name: &str,
    header_value: &str,
    body: Vec<u8>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("content-type", "application/octet-stream")
        .header(
            HeaderName::from_bytes(header_name.as_bytes()).expect("header name"),
            HeaderValue::from_str(header_value).expect("header value"),
        )
        .body(Body::from(body))
        .expect("request");
    send(app, req).await
}

/// `POST {uri}` as `multipart/form-data`, with an `image` part and an optional `options` part.
pub(crate) async fn post_multipart(
    app: &Router,
    uri: &str,
    image: &[u8],
    options: Option<&str>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let boundary = "InkvecTestBoundary123456";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"image\"; filename=\"input.png\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
    body.extend_from_slice(image);
    body.extend_from_slice(b"\r\n");
    if let Some(options) = options {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"options\"\r\n\r\n");
        body.extend_from_slice(options.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .expect("request");
    send(app, req).await
}

pub(crate) async fn get(app: &Router, uri: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .expect("request");
    send(app, req).await
}

pub(crate) fn json_body(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body)
        .unwrap_or_else(|e| panic!("not JSON: {e}: {}", String::from_utf8_lossy(body)))
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A generous per-request timeout so a genuinely hung test fails fast rather than hanging CI.
pub(crate) const TEST_TIMEOUT: Duration = Duration::from_secs(30);
