//! Client-level tests: auth header, query/body encoding, `data` unwrapping,
//! and error-envelope decoding — driven against a `wiremock` server. The
//! blocking client is exercised on a blocking thread so it doesn't clash with
//! the test's tokio runtime.

use dpl_dply::client::unwrap_data;
use dpl_dply::DplyClient;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Run a blocking closure off the async test runtime.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

#[tokio::test]
async fn get_sends_bearer_and_unwraps_data() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/edge/sites"))
        .and(header("authorization", "Bearer test-token"))
        .and(header("accept", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "id": "s1", "name": "alpha" }]
        })))
        .mount(&server)
        .await;

    let uri = server.uri();
    let value = blocking(move || {
        let c = DplyClient::new(uri, Some("test-token".into())).unwrap();
        c.get("/api/v1/edge/sites", &[]).unwrap()
    })
    .await;

    let rows = unwrap_data(value);
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["name"], "alpha");
}

#[tokio::test]
async fn get_forwards_query_params() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/edge/sites"))
        .and(query_param("status", "building"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
        .mount(&server)
        .await;

    let uri = server.uri();
    let ok = blocking(move || {
        let c = DplyClient::new(uri, Some("t".into())).unwrap();
        c.get("/api/v1/edge/sites", &[("status", "building".into())])
            .is_ok()
    })
    .await;
    assert!(ok);
}

#[tokio::test]
async fn post_sends_json_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/edge/sites/site-1/deployments"))
        .and(wiremock::matchers::body_json(json!({ "git_branch": "main" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "d1", "status": "queued" })))
        .mount(&server)
        .await;

    let uri = server.uri();
    let value: Value = blocking(move || {
        let c = DplyClient::new(uri, Some("t".into())).unwrap();
        c.post(
            "/api/v1/edge/sites/site-1/deployments",
            json!({ "git_branch": "main" }),
        )
        .unwrap()
    })
    .await;
    assert_eq!(value["status"], "queued");
}

#[tokio::test]
async fn non_2xx_decodes_error_envelope() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/edge/sites/nope"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "message": "Validation failed",
            "errors": { "site": ["The site does not exist."] }
        })))
        .mount(&server)
        .await;

    let uri = server.uri();
    let err = blocking(move || {
        let c = DplyClient::new(uri, Some("t".into())).unwrap();
        c.get("/api/v1/edge/sites/nope", &[]).unwrap_err()
    })
    .await;

    match err {
        dpl_dply::DplyApiError::Api { status, message, errors, .. } => {
            assert_eq!(status, 422);
            assert_eq!(message, "Validation failed");
            assert_eq!(errors["site"], vec!["The site does not exist."]);
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[tokio::test]
async fn missing_token_is_auth_error_without_network() {
    // No token → require_token() fails before any request is made.
    let err = blocking(|| {
        let c = DplyClient::new("https://dply.example", None).unwrap();
        c.get("/api/v1/edge/sites", &[]).unwrap_err()
    })
    .await;
    assert!(err.is_auth());
}
