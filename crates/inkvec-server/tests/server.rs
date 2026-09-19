//! Integration tests for the HTTP surface itself: all three ways of sending an image, all
//! four ways of sending options (and that a later source overrides an earlier one), error
//! mapping, the white-on-transparent default, and the concurrency limit.
//!
//! The shared contract over HTTP is `tests/contract.rs`; this file is everything the
//! contract does not cover because it is specific to this binding (multipart, headers, query
//! parameters, load shedding).

mod common;

use axum::http::StatusCode;
use common::{app, get, json_body, post_multipart, post_raw, post_raw_with_header};
use inkvec_server::AppState;
use std::fs;

fn tiny_png() -> Vec<u8> {
    fs::read(common::contract_dir().join("tiny.png")).expect("tiny.png")
}

fn white_on_clear_png() -> Vec<u8> {
    fs::read(common::contract_dir().join("white_on_clear.png")).expect("white_on_clear.png")
}

fn encode(s: &str) -> String {
    form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

// ---------------------------------------------------------------------------------------
// All three body kinds trace the same image.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn raw_body_traces() {
    let app = app();
    let (status, headers, body) = post_raw(&app, "/trace", Some("image/png"), tiny_png()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("content-type").unwrap(), "image/svg+xml");
    assert!(String::from_utf8_lossy(&body).starts_with("<svg"));
}

#[tokio::test]
async fn octet_stream_content_type_also_traces() {
    let app = app();
    let (status, _headers, body) =
        post_raw(&app, "/trace", Some("application/octet-stream"), tiny_png()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).starts_with("<svg"));
}

#[tokio::test]
async fn multipart_body_traces() {
    let app = app();
    let (status, headers, body) = post_multipart(&app, "/trace", &tiny_png(), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("content-type").unwrap(), "image/svg+xml");
    assert!(String::from_utf8_lossy(&body).starts_with("<svg"));
}

#[tokio::test]
async fn multipart_without_an_image_part_is_invalid_image() {
    let app = app();
    let (status, _headers, body) = post_multipart(&app, "/trace", &[], None).await;
    // An empty `image` part is itself indistinguishable from a missing one to multer; both
    // are the same user error and this service maps them the same way.
    assert!(status == StatusCode::BAD_REQUEST, "status was {status}");
    assert_eq!(json_body(&body)["error"]["code"], "invalid_image");
}

// ---------------------------------------------------------------------------------------
// Options: query, ?options=, header, multipart part, and precedence between them.
// ---------------------------------------------------------------------------------------

const NONDEFAULT_OPTIONS: &str =
    r#"{"colors":8,"minify":true,"no_background":true,"harmonize":false}"#;

#[tokio::test]
async fn all_four_option_sources_agree() {
    let app = app();
    let bytes = tiny_png();

    let (s1, _, via_generic_query) = post_raw(
        &app,
        "/trace?colors=8&minify=true&no_background=true&harmonize=false",
        Some("image/png"),
        bytes.clone(),
    )
    .await;
    let (s2, _, via_options_query) = post_raw(
        &app,
        &format!("/trace?options={}", encode(NONDEFAULT_OPTIONS)),
        Some("image/png"),
        bytes.clone(),
    )
    .await;
    let (s3, _, via_header) = post_raw_with_header(
        &app,
        "/trace",
        "X-Inkvec-Options",
        NONDEFAULT_OPTIONS,
        bytes.clone(),
    )
    .await;
    let (s4, _, via_multipart) =
        post_multipart(&app, "/trace", &bytes, Some(NONDEFAULT_OPTIONS)).await;

    for s in [s1, s2, s3, s4] {
        assert_eq!(s, StatusCode::OK);
    }
    assert_eq!(
        via_generic_query, via_options_query,
        "query-object vs generic query"
    );
    assert_eq!(via_generic_query, via_header, "header vs generic query");
    assert_eq!(
        via_generic_query, via_multipart,
        "multipart part vs generic query"
    );

    // Ties this test to the facade directly too, not just to itself.
    let truth = inkvec::trace(
        &bytes,
        &inkvec::Options::from_json(NONDEFAULT_OPTIONS).unwrap(),
    )
    .unwrap();
    assert_eq!(String::from_utf8(via_generic_query).unwrap(), truth.svg);
}

#[tokio::test]
async fn a_later_option_source_overrides_an_earlier_one() {
    let app = app();
    let bytes = tiny_png();

    // Query says colors=2; the header says colors=8 and must win.
    let (status, _headers, overridden) = post_raw_with_header(
        &app,
        "/trace?colors=2",
        "X-Inkvec-Options",
        r#"{"colors":8}"#,
        bytes.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status2, _headers2, direct) =
        post_raw(&app, "/trace?colors=8", Some("image/png"), bytes).await;
    assert_eq!(status2, StatusCode::OK);

    assert_eq!(
        overridden, direct,
        "the header's colors=8 should have won over the query's colors=2"
    );
}

#[tokio::test]
async fn a_boolean_query_param_accepts_true_and_1() {
    let app = app();
    let bytes = tiny_png();
    let (_, _, a) = post_raw(
        &app,
        "/trace?no_background=true",
        Some("image/png"),
        bytes.clone(),
    )
    .await;
    let (_, _, b) = post_raw(&app, "/trace?no_background=1", Some("image/png"), bytes).await;
    assert_eq!(a, b);
}

// ---------------------------------------------------------------------------------------
// Error mapping.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn unknown_option_is_422_invalid_options_from_the_facade() {
    let app = app();
    let (status, _headers, body) = post_raw_with_header(
        &app,
        "/trace",
        "X-Inkvec-Options",
        r#"{"colours":8}"#,
        tiny_png(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let parsed = json_body(&body);
    assert_eq!(parsed["error"]["code"], "invalid_options");
    assert!(
        parsed["error"]["message"]
            .as_str()
            .unwrap()
            .contains("colours"),
        "message should name the field: {parsed}"
    );
}

#[tokio::test]
async fn out_of_range_option_is_422() {
    let app = app();
    let (status, _headers, body) =
        post_raw(&app, "/trace?colors=0", Some("image/png"), tiny_png()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_body(&body)["error"]["code"], "invalid_options");
}

#[tokio::test]
async fn garbage_bytes_are_400_invalid_image() {
    let app = app();
    let (status, _headers, body) = post_raw(&app, "/trace", Some("image/png"), vec![0u8; 64]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json_body(&body)["error"]["code"], "invalid_image");
}

#[tokio::test]
async fn empty_body_is_400_invalid_image() {
    let app = app();
    let (status, _headers, body) = post_raw(&app, "/trace", Some("image/png"), vec![]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json_body(&body)["error"]["code"], "invalid_image");
}

#[tokio::test]
async fn oversize_body_is_413_too_large() {
    // A deliberately tiny limit, well under tiny.png's size.
    let state = AppState::new(4, 16, None, None);
    let app = inkvec_server::app(state);
    let (status, _headers, body) = post_raw(&app, "/trace", Some("image/png"), tiny_png()).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json_body(&body)["error"]["code"], "too_large");
}

#[tokio::test]
async fn oversize_multipart_body_is_413_too_large() {
    let state = AppState::new(4, 16, None, None);
    let app = inkvec_server::app(state);
    let (status, _headers, body) = post_multipart(&app, "/trace", &tiny_png(), None).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(json_body(&body)["error"]["code"], "too_large");
}

// ---------------------------------------------------------------------------------------
// White artwork on a transparent ground: defaults must not paint a background.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn white_on_transparent_defaults_to_shapes_with_no_background_rect() {
    let app = app();
    let bytes = white_on_clear_png();
    let truth = inkvec::trace(&bytes, &inkvec::Options::default()).expect("facade trace");

    let (status, headers, body) = post_raw(&app, "/trace", Some("image/png"), bytes).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("content-type").unwrap(), "image/svg+xml");

    let svg = String::from_utf8(body).expect("utf8 svg");
    assert_eq!(
        svg, truth.svg,
        "HTTP output should equal the facade's own trace"
    );
    assert!(
        svg.contains("#ffffff"),
        "the white shape should be present: {svg}"
    );
    assert!(
        !svg.to_ascii_lowercase().contains("id=\"bg\""),
        "no full-canvas background face should be drawn: {svg}"
    );
}

// ---------------------------------------------------------------------------------------
// Concurrency: a request past the limit sheds load with 503, deterministically (no timing
// race: the test holds the one permit itself before making the request).
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn over_the_concurrency_limit_is_503_busy() {
    let state = AppState::new(1, 20 * 1024 * 1024, None, None);
    let semaphore = state.concurrency_semaphore();
    let app = inkvec_server::app(state);

    let permit = semaphore
        .clone()
        .try_acquire_owned()
        .expect("the only permit");
    let (status, _headers, body) = post_raw(&app, "/trace", Some("image/png"), tiny_png()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(&body)["error"]["code"], "busy");

    drop(permit);
    let (status2, _headers2, _body2) =
        post_raw(&app, "/trace", Some("image/png"), tiny_png()).await;
    assert_eq!(
        status2,
        StatusCode::OK,
        "a permit freed up should let the next request through"
    );
}

// ---------------------------------------------------------------------------------------
// Misc endpoints.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn healthz_is_ok() {
    let app = app();
    let (status, _headers, body) = get(&app, "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_body(&body)["status"], "ok");
}

#[tokio::test]
async fn openapi_json_embeds_the_generated_document() {
    let app = app();
    let (status, headers, body) = get(&app, "/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("content-type").unwrap(), "application/json");
    let parsed = json_body(&body);
    assert_eq!(parsed["openapi"], "3.1.0");
    assert!(parsed["paths"]["/trace"].is_object());
    assert_eq!(
        parsed["components"]["schemas"]["Options"]["title"],
        "InkvecOptions"
    );
}
