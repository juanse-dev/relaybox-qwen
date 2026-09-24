#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use http_body_util::BodyExt;

use serde_json::Value;
use tower::ServiceExt;

use relaybox::api::router;
use relaybox::application::DeliveryService;
use relaybox::infrastructure::SqliteDeliveryRepository;

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct TestDb {
    path: PathBuf,
}

impl TestDb {
    pub fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Self {
            path: PathBuf::from(format!("relaybox-test-{}-{n}.db", std::process::id())),
        }
    }

    pub fn url(&self) -> String {
        format!("sqlite://{}", self.path.to_string_lossy())
    }

    pub async fn router(&self) -> axum::Router {
        let repository = SqliteDeliveryRepository::connect(&self.url())
            .await
            .expect("failed to connect test database");
        router(DeliveryService::new(repository))
    }

    pub async fn count_by_key(&self, key: &str) -> i64 {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&self.url())
            .await
            .expect("failed to open counting database");
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM deliveries WHERE idempotency_key = ?")
                .bind(key)
                .fetch_one(&pool)
                .await
                .expect("count query failed");
        let _ = pool.close().await;
        count
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let base = self.path.to_string_lossy();
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{base}{suffix}"));
        }
    }
}

pub async fn post_body(
    router: &axum::Router,
    key: Option<&str>,
    body: Vec<u8>,
) -> (http::StatusCode, Value) {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri("/v1/deliveries");
    if let Some(key) = key {
        builder = builder.header("Idempotency-Key", key);
    }
    send(
        router,
        builder.body(Body::from(body)).expect("valid request"),
    )
    .await
}

pub async fn post_json(
    router: &axum::Router,
    key: Option<&str>,
    body: &Value,
) -> (http::StatusCode, Value) {
    let bytes = serde_json::to_vec(body).expect("serialize request body");
    post_body(router, key, bytes).await
}

pub async fn post_body_with_key_bytes(
    router: &axum::Router,
    key: Option<&[u8]>,
    body: Vec<u8>,
) -> (http::StatusCode, Value) {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri("/v1/deliveries");
    if let Some(key) = key {
        let value = http::HeaderValue::from_bytes(key).expect("header bytes are valid");
        builder = builder.header("Idempotency-Key", value);
    }
    send(
        router,
        builder.body(Body::from(body)).expect("valid request"),
    )
    .await
}

pub async fn post_body_raw(
    router: &axum::Router,
    key: Option<&str>,
    body: Vec<u8>,
) -> (http::StatusCode, Vec<u8>) {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri("/v1/deliveries");
    if let Some(key) = key {
        builder = builder.header("Idempotency-Key", key);
    }
    send_bytes(
        router,
        builder.body(Body::from(body)).expect("valid request"),
    )
    .await
}

pub async fn raw_request(
    router: &axum::Router,
    method: &str,
    uri: &str,
) -> (http::StatusCode, Value) {
    send(
        router,
        http::Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .expect("valid request"),
    )
    .await
}

pub async fn get_delivery(router: &axum::Router, id: &str) -> (http::StatusCode, Value) {
    send(
        router,
        http::Request::builder()
            .uri(format!("/v1/deliveries/{id}"))
            .body(Body::empty())
            .expect("valid request"),
    )
    .await
}

pub async fn get_delivery_raw(router: &axum::Router, id: &str) -> (http::StatusCode, Vec<u8>) {
    send_bytes(
        router,
        http::Request::builder()
            .uri(format!("/v1/deliveries/{id}"))
            .body(Body::empty())
            .expect("valid request"),
    )
    .await
}

pub async fn get_health(router: &axum::Router) -> (http::StatusCode, Value) {
    send(
        router,
        http::Request::builder()
            .uri("/health")
            .body(Body::empty())
            .expect("valid request"),
    )
    .await
}

async fn send_bytes(
    router: &axum::Router,
    request: http::Request<Body>,
) -> (http::StatusCode, Vec<u8>) {
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("request failed");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, bytes.to_vec())
}

async fn send(router: &axum::Router, request: http::Request<Body>) -> (http::StatusCode, Value) {
    let (status, bytes) = send_bytes(router, request).await;
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        parse_response(&bytes)
    };
    (status, value)
}

fn parse_response(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("response is JSON")
}

pub fn assert_error(body: &Value, code: &str) {
    let error = body.get("error").expect("response has an error object");
    assert_eq!(
        error.get("code").and_then(Value::as_str),
        Some(code),
        "unexpected error body: {body}"
    );
    assert!(
        error.get("message").and_then(Value::as_str).is_some(),
        "error message missing: {body}"
    );
}

pub fn assert_created_at_shape(value: &str) {
    let bytes = value.as_bytes();
    assert_eq!(
        bytes.len(),
        20,
        "created_at should be second-precision RFC3339 UTC: {value}"
    );
    for (i, b) in bytes.iter().enumerate() {
        match i {
            4 | 7 => assert_eq!(*b, b'-', "unexpected created_at: {value}"),
            10 => assert_eq!(*b, b'T', "unexpected created_at: {value}"),
            13 | 16 => assert_eq!(*b, b':', "unexpected created_at: {value}"),
            19 => assert_eq!(*b, b'Z', "unexpected created_at: {value}"),
            _ => assert!(b.is_ascii_digit(), "unexpected created_at: {value}"),
        }
    }
}
