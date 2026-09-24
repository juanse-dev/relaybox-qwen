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
async fn post_missing_or_invalid_target_url_returns_400() {
    let db = common::TestDb::new();
    let router = db.router().await;

    for body in [
        json!({ "payload": null }),
        json!({ "target_url": 42, "payload": null }),
    ] {
        let (status, response) = common::post_json(&router, Some("key-1"), &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        common::assert_error(&response, "invalid_request");
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

    for target_url in ["/hooks", "ftp://example.test/hook"] {
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
