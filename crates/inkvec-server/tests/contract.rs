//! The shared cross-language contract (`bindings/contract/cases.json`), run over HTTP.
//!
//! Every other binding runs these cases in-process (`inkvec::trace` / `trace_rgba` behind a
//! C ABI or a Python/JS wrapper); this service has no raw-pixel endpoint, so only the
//! `encoded` -- the pieces of the contract that are meaningful over HTTP: options via
//! `?options=<url-encoded JSON>`, and the reported size or `expect.error` kind, mapped to the
//! HTTP status this service returns for it. Where the case records this host's build target
//! (`inkvec::build_target()`, e.g. `x86_64-windows-msvc` here, `x86_64-linux-gnu` inside the
//! Linux container), the response body is checked byte for byte against it, the same as the
//! Rust facade's own contract test.

mod common;

use axum::http::StatusCode;

use common::{app, get, json_body, post_raw, sha256_hex};
use serde_json::Value;
use std::fs;

fn cases() -> Value {
    let path = common::contract_dir().join("cases.json");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("cases.json")
}

fn error_status(kind: &str) -> StatusCode {
    match kind {
        "invalid_image" => StatusCode::BAD_REQUEST,
        "invalid_options" => StatusCode::UNPROCESSABLE_ENTITY,
        "internal" => StatusCode::INTERNAL_SERVER_ERROR,
        other => panic!("unknown contract error kind {other}"),
    }
}

#[tokio::test]
async fn every_encoded_contract_case_over_http() {
    let app = app();
    let dir = common::contract_dir();
    let root = cases();
    let target = inkvec::build_target();
    let mut checked = 0usize;
    let mut skipped_rgba = 0usize;

    for case in root["cases"].as_array().expect("cases array") {
        let name = case["name"].as_str().expect("case.name");
        if case["form"].as_str() != Some("encoded") {
            // No raw-pixel endpoint on this service; the facade's own contract test
            // (crates/inkvec/tests/contract.rs) covers the `rgba` form.
            skipped_rgba += 1;
            continue;
        }
        let input = case["input"].as_str().expect("case.input");
        let bytes = fs::read(dir.join(input)).unwrap_or_else(|e| panic!("{name}: {input}: {e}"));
        let options = case["options"].to_string();
        let uri = format!(
            "/trace?options={}",
            form_urlencoded::byte_serialize(options.as_bytes()).collect::<String>()
        );

        let (status, headers, body) =
            post_raw(&app, &uri, Some("application/octet-stream"), bytes).await;

        if let Some(err) = case["expect"]["error"].as_str() {
            assert_eq!(status, error_status(err), "{name}: status for {err}");
            let parsed = json_body(&body);
            assert_eq!(parsed["error"]["code"], err, "{name}: error code");
            checked += 1;
            continue;
        }

        assert_eq!(
            status,
            StatusCode::OK,
            "{name}: status ({body:?})",
            body = String::from_utf8_lossy(&body)
        );
        let w: u64 = case["expect"]["width"].as_u64().expect("expect.width");
        let h: u64 = case["expect"]["height"].as_u64().expect("expect.height");
        assert_eq!(
            headers.get("x-inkvec-width").and_then(|v| v.to_str().ok()),
            Some(w.to_string().as_str()),
            "{name}: X-Inkvec-Width"
        );
        assert_eq!(
            headers.get("x-inkvec-height").and_then(|v| v.to_str().ok()),
            Some(h.to_string().as_str()),
            "{name}: X-Inkvec-Height"
        );
        assert_eq!(
            headers.get("content-type").and_then(|v| v.to_str().ok()),
            Some("image/svg+xml"),
            "{name}: Content-Type"
        );

        if let Some(expected) = case["svg"].get(target) {
            let expected_bytes = expected["bytes"].as_u64().expect("svg.bytes") as usize;
            let expected_sha = expected["sha256"].as_str().expect("svg.sha256");
            assert_eq!(
                body.len(),
                expected_bytes,
                "{name}: SVG byte length on {target}"
            );
            assert_eq!(
                sha256_hex(&body),
                expected_sha,
                "{name}: SVG SHA-256 on {target}"
            );
        }
        checked += 1;
    }

    assert!(checked > 0, "no contract cases were run");
    eprintln!("ran {checked} encoded contract case(s) over HTTP, skipped {skipped_rgba} rgba-form case(s)");
}

/// `GET /options/schema` is `inkvec::options_schema_json()`, byte for byte.
#[tokio::test]
async fn options_schema_matches_the_facade() {
    let app = app();
    let (status, _headers, body) = get(&app, "/options/schema").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        String::from_utf8(body).unwrap().trim_end(),
        inkvec::options_schema_json().trim_end()
    );
}

/// `GET /options/defaults` is `inkvec::Options::default()`, serialised the same way.
#[tokio::test]
async fn options_defaults_matches_the_facade() {
    let app = app();
    let (status, _headers, body) = get(&app, "/options/defaults").await;
    assert_eq!(status, StatusCode::OK);
    let got: Value = serde_json::from_slice(&body).unwrap();
    let want = serde_json::to_value(inkvec::Options::default()).unwrap();
    assert_eq!(got, want);
}

/// `GET /version` names this build's crate version and target, both from the facade.
#[tokio::test]
async fn version_matches_the_facade() {
    let app = app();
    let (status, _headers, body) = get(&app, "/version").await;
    assert_eq!(status, StatusCode::OK);
    let got = json_body(&body);
    assert_eq!(got["version"], inkvec::version());
    assert_eq!(got["build_target"], inkvec::build_target());
}
