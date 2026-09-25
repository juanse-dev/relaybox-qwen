mod common;

use http::StatusCode;
use serde_json::{json, Value};

#[tokio::test]
async fn health_returns_ok() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, body) = common::get_health(&router).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "status": "ok" }));
}

#[tokio::test]
async fn post_creates_pending_delivery() {
    let db = common::TestDb::new();
    let router = db.router().await;
    let payload = json!({ "event": "invoice.created", "invoice_id": "inv_123" });

    let (status, body) = common::post_json(
        &router,
        Some("key-1"),
        &json!({
            "target_url": "https://example.test/webhooks",
            "payload": payload,
        }),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    let id = body["id"].as_str().expect("response has an id");
    uuid::Uuid::parse_str(id).expect("id is a UUID");
    assert_eq!(body["status"], json!("pending"));
    assert_eq!(body["attempts"], json!(0));
    assert_eq!(body["target_url"], json!("https://example.test/webhooks"));
    assert_eq!(body["payload"], payload);
    common::assert_created_at_shape(body["created_at"].as_str().expect("created_at"));

    let (status, fetched) = common::get_delivery(&router, id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched, body);
}

#[tokio::test]
async fn replay_same_key_and_content_returns_existing_delivery() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (first_status, first) = common::post_json(
        &router,
        Some("key-1"),
        &json!({
            "target_url": "https://example.test/webhooks",
            "payload": { "a": 1, "b": 2 },
        }),
    )
    .await;
    assert_eq!(first_status, StatusCode::CREATED);

    let (second_status, second) = common::post_json(
        &router,
        Some("key-1"),
        &json!({
            "target_url": "https://example.test/webhooks",
            "payload": { "b": 2, "a": 1 },
        }),
    )
    .await;

    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(second["id"], first["id"]);
    assert_eq!(second["created_at"], first["created_at"]);
    assert_eq!(db.count_by_key("key-1").await, 1);
}

#[tokio::test]
async fn same_key_different_payload_returns_conflict_and_keeps_original() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, first) = common::post_json(
        &router,
        Some("key-1"),
        &json!({
            "target_url": "https://example.test/webhooks",
            "payload": { "version": 1 },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, body) = common::post_json(
        &router,
        Some("key-1"),
        &json!({
            "target_url": "https://example.test/webhooks",
            "payload": { "version": 2 },
        }),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
    common::assert_error(&body, "idempotency_conflict");

    let (status, fetched) = common::get_delivery(&router, first["id"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["payload"], json!({ "version": 1 }));
}

#[tokio::test]
async fn same_key_different_target_url_returns_conflict() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, _) = common::post_json(
        &router,
        Some("key-1"),
        &json!({
            "target_url": "https://example.test/webhooks",
            "payload": { "version": 1 },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, body) = common::post_json(
        &router,
        Some("key-1"),
        &json!({
            "target_url": "https://example.test/other",
            "payload": { "version": 1 },
        }),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
    common::assert_error(&body, "idempotency_conflict");
}

#[tokio::test]
async fn post_missing_idempotency_key_returns_400() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, body) = common::post_json(
        &router,
        None,
        &json!({ "target_url": "https://example.test/webhooks", "payload": null }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    common::assert_error(&body, "missing_idempotency_key");
}

#[tokio::test]
async fn post_blank_idempotency_key_returns_400() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, body) = common::post_json(
        &router,
        Some("   \t"),
        &json!({ "target_url": "https://example.test/webhooks", "payload": null }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    common::assert_error(&body, "invalid_idempotency_key");
}

#[tokio::test]
async fn post_oversized_idempotency_key_returns_400() {
    let db = common::TestDb::new();
    let router = db.router().await;
    let key = format!("  {}  ", "a".repeat(129));

    let (status, body) = common::post_json(
        &router,
        Some(&key),
        &json!({ "target_url": "https://example.test/webhooks", "payload": null }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    common::assert_error(&body, "invalid_idempotency_key");
}

#[tokio::test]
async fn post_accepts_128_byte_idempotency_key() {
    let db = common::TestDb::new();
    let router = db.router().await;
    let key = "a".repeat(128);

    let (status, _) = common::post_json(
        &router,
        Some(&key),
        &json!({ "target_url": "https://example.test/webhooks", "payload": null }),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn post_malformed_json_returns_400() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, body) = common::post_body(&router, Some("key-1"), b"{not json".to_vec()).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    common::assert_error(&body, "invalid_json");
}

#[tokio::test]
async fn post_non_object_body_returns_400() {
    let db = common::TestDb::new();
    let router = db.router().await;

    for body in [json!([1, 2]), json!("just a string"), json!(42)] {
        let (status, response) = common::post_json(&router, Some("key-1"), &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        common::assert_error(&response, "invalid_request");
    }
}

#[tokio::test]
async fn post_missing_target_url_returns_400() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, response) =
        common::post_json(&router, Some("key-1"), &json!({ "payload": null })).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    common::assert_error(&response, "invalid_request");
}

#[tokio::test]
async fn post_non_string_target_url_returns_422() {
    let db = common::TestDb::new();
    let router = db.router().await;

    for target_url in [json!(42), json!([1]), json!(true), json!(null)] {
        let (status, response) = common::post_json(
            &router,
            Some("key-1"),
            &json!({ "target_url": target_url, "payload": null }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        common::assert_error(&response, "invalid_target_url");
    }
}

#[tokio::test]
async fn post_missing_payload_returns_400() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, body) = common::post_json(
        &router,
        Some("key-1"),
        &json!({ "target_url": "https://example.test/webhooks" }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    common::assert_error(&body, "invalid_request");
}

#[tokio::test]
async fn post_invalid_target_url_returns_422() {
    let db = common::TestDb::new();
    let router = db.router().await;

    for target_url in [
        "/hooks",
        "ftp://example.test/hook",
        "https:///path",
        "http:////example.com/x",
        "https://\n/path",
        "https://\t/path",
        "https://\r/path",
    ] {
        let (status, body) = common::post_json(
            &router,
            Some("key-1"),
            &json!({ "target_url": target_url, "payload": null }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        common::assert_error(&body, "invalid_target_url");
    }
}

#[tokio::test]
async fn get_unknown_delivery_returns_404() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, body) =
        common::get_delivery(&router, "3f2c1a5e-9b8d-4e6f-a1c2-d3e4f5a6b7c8").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    common::assert_error(&body, "delivery_not_found");
}

#[tokio::test]
async fn get_malformed_uuid_returns_404() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, body) = common::get_delivery(&router, "not-a-uuid").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    common::assert_error(&body, "delivery_not_found");
}

#[tokio::test]
async fn payload_round_trips_any_json_value() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let payloads: Vec<Value> = vec![
        Value::Null,
        json!([1, "two", true, null]),
        json!("plain text"),
        json!(3.5),
        json!(false),
    ];

    for (i, payload) in payloads.iter().enumerate() {
        let key = format!("round-{i}");
        let (status, created) = common::post_json(
            &router,
            Some(&key),
            &json!({ "target_url": "https://example.test/webhooks", "payload": payload }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        let (status, fetched) =
            common::get_delivery(&router, created["id"].as_str().unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(fetched["payload"], *payload);
    }
}

#[tokio::test]
async fn concurrent_same_key_creates_single_delivery() {
    let db = common::TestDb::new();
    let router = db.router().await;
    let body = json!({
        "target_url": "https://example.test/hook",
        "payload": { "n": 1 },
    });

    let mut handles = Vec::new();
    for _ in 0..8 {
        let router = router.clone();
        let body = body.clone();
        handles.push(tokio::spawn(async move {
            common::post_json(&router, Some("race-key"), &body).await
        }));
    }

    let mut created_count = 0;
    let mut first_id: Option<String> = None;
    for handle in handles {
        let (status, response) = handle.await.expect("task completed");
        assert!(
            matches!(status, StatusCode::CREATED | StatusCode::OK),
            "unexpected status: {status}"
        );
        if status == StatusCode::CREATED {
            created_count += 1;
        }
        let id = response["id"]
            .as_str()
            .expect("response has an id")
            .to_owned();
        match &first_id {
            Some(first) => assert_eq!(&id, first),
            None => first_id = Some(id),
        }
    }

    assert_eq!(created_count, 1);
    assert_eq!(db.count_by_key("race-key").await, 1);
}

#[tokio::test]
async fn post_accepts_bodies_larger_than_default_axum_limit() {
    let db = common::TestDb::new();
    let router = db.router().await;
    let big = "x".repeat(3 * 1024 * 1024);

    let (status, created) = common::post_json(
        &router,
        Some("key-big"),
        &json!({
            "target_url": "https://example.test/webhooks",
            "payload": { "data": big },
        }),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);

    let (status, fetched) = common::get_delivery(&router, created["id"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["payload"], json!({ "data": big }));
}

#[tokio::test]
async fn post_preserves_numbers_beyond_f64_precision() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, created) = common::post_body(
        &router,
        Some("big-int"),
        br#"{"target_url":"https://example.test/webhooks","payload":{"n":18446744073709551617}}"#
            .to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let expected = serde_json::from_str::<Value>("18446744073709551617").expect("valid number");
    assert_eq!(created["payload"]["n"], expected);

    let (status, fetched) = common::get_delivery(&router, created["id"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["payload"]["n"], expected);

    let (status, body) = common::post_body(
        &router,
        Some("big-int"),
        br#"{"target_url":"https://example.test/webhooks","payload":{"n":18446744073709551618}}"#
            .to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    common::assert_error(&body, "idempotency_conflict");
}

#[tokio::test]
async fn post_accepts_out_of_range_float_literals() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, created) = common::post_body(
        &router,
        Some("big-float"),
        br#"{"target_url":"https://example.test/webhooks","payload":{"n":1e400}}"#.to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let expected = serde_json::from_str::<Value>("1e400").expect("valid number");
    assert_eq!(created["payload"]["n"], expected);

    let (status, fetched) = common::get_delivery(&router, created["id"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["payload"]["n"], expected);
}

#[tokio::test]
async fn get_invalid_percent_encoded_id_returns_404_envelope() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, body) = common::get_delivery(&router, "%FF").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    common::assert_error(&body, "delivery_not_found");

    let encoded_id = "3f2c%31a5e-9b8d-4e6f-a1c2-d3e4f5a6b7c8";
    let (status, body) = common::get_delivery(&router, encoded_id).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    common::assert_error(&body, "delivery_not_found");
}

#[tokio::test]
async fn post_accepts_utf8_idempotency_key() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, _) = common::post_json(
        &router,
        Some("clé-ünïcode"),
        &json!({ "target_url": "https://example.test/webhooks", "payload": null }),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn post_rejects_non_utf8_idempotency_key() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, body) = common::post_body_with_key_bytes(
        &router,
        Some(b"\xff"),
        br#"{"target_url":"https://example.test/webhooks","payload":null}"#.to_vec(),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    common::assert_error(&body, "invalid_idempotency_key");
}

#[tokio::test]
async fn unknown_path_returns_404_envelope() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let (status, body) = common::raw_request(&router, "GET", "/nope").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    common::assert_error(&body, "not_found");
}

#[tokio::test]
async fn unsupported_method_returns_405_envelope() {
    let db = common::TestDb::new();
    let router = db.router().await;

    for (method, uri) in [
        ("GET", "/v1/deliveries"),
        (
            "DELETE",
            "/v1/deliveries/3f2c1a5e-9b8d-4e6f-a1c2-d3e4f5a6b7c8",
        ),
        ("POST", "/health"),
    ] {
        let (status, body) = common::raw_request(&router, method, uri).await;

        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        common::assert_error(&body, "method_not_allowed");
    }
}

#[tokio::test]
async fn post_accepts_deeply_nested_payload_beyond_serde_json_limit() {
    let db = common::TestDb::new();
    let router = db.router().await;

    let mut payload: Value = json!(null);
    for _ in 0..300 {
        payload = json!([payload]);
    }
    let expected = payload.clone();

    let body = serde_json::to_vec(&json!({
        "target_url": "https://example.test/webhooks",
        "payload": payload
    }))
    .expect("serialize request body");
    let (status, created_bytes) = common::post_body_raw(&router, Some("deep"), body).await;
    assert_eq!(status, StatusCode::CREATED);
    let created: Value = relaybox::json::parse_value(&created_bytes).expect("response is JSON");

    let (status, fetched_bytes) =
        common::get_delivery_raw(&router, created["id"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let fetched: Value = relaybox::json::parse_value(&fetched_bytes).expect("response is JSON");
    assert_eq!(fetched["payload"], expected);
}

#[tokio::test]
async fn post_accepts_and_persists_payload_nested_beyond_stack_safe_depth() {
    let db = common::TestDb::new();
    let router = db.router().await;

    const DEPTH: usize = 2000;
    let mut body = String::from("{\"target_url\":\"https://example.test/webhooks\",\"payload\":");
    for _ in 0..DEPTH {
        body.push('[');
    }
    body.push_str("null");
    for _ in 0..DEPTH {
        body.push(']');
    }
    body.push('}');

    let payload_start = body.find("\"payload\":").unwrap() + "\"payload\":".len();
    let payload_text = body[payload_start..body.len() - 1].to_string();

    let (status, created_bytes) =
        common::post_body_raw(&router, Some("deep-stack"), body.into_bytes()).await;
    assert_eq!(status, StatusCode::CREATED);
    let created: Value = relaybox::json::parse_value(&created_bytes).expect("response is JSON");

    let (status, fetched_bytes) =
        common::get_delivery_raw(&router, created["id"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let fetched_text = std::str::from_utf8(&fetched_bytes).expect("response is UTF-8");
    assert!(fetched_text.contains(payload_text.as_str()));
}
