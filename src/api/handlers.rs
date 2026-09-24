use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::application::{DeliveryRepository, DeliveryService, EnqueueError, QueryError};
use crate::domain::Delivery;

enum IdempotencyKeyInput {
    Missing,
    InvalidEncoding,
    Value(String),
}

fn extract_idempotency_key(headers: &HeaderMap) -> IdempotencyKeyInput {
    match headers.get("Idempotency-Key") {
        None => IdempotencyKeyInput::Missing,
        Some(value) => match value.to_str() {
            Ok(text) => IdempotencyKeyInput::Value(text.to_owned()),
            Err(_) => IdempotencyKeyInput::InvalidEncoding,
        },
    }
}

pub(crate) async fn create_delivery<R>(
    State(service): State<Arc<DeliveryService<R>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response
where
    R: DeliveryRepository + Send + Sync + 'static,
{
    let key = match extract_idempotency_key(&headers) {
        IdempotencyKeyInput::Missing => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "missing_idempotency_key",
                "Idempotency-Key header is required",
            )
        }
        IdempotencyKeyInput::InvalidEncoding => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_idempotency_key",
                "Idempotency-Key must be valid UTF-8",
            )
        }
        IdempotencyKeyInput::Value(value) => value,
    };

    let parsed = match serde_json::from_slice::<Value>(&body) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body must be valid JSON",
            )
        }
    };

    let object = match parsed.as_object() {
        Some(object) => object,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "request body must be a JSON object with target_url and payload fields",
            )
        }
    };

    let target_url = match object.get("target_url").and_then(Value::as_str) {
        Some(url) => url.to_owned(),
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "target_url is required and must be a string",
            )
        }
    };

    let payload = match object.get("payload") {
        Some(payload) => payload.clone(),
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "payload is required and may be any JSON value including null",
            )
        }
    };

    match service
        .enqueue(Some(key.as_str()), &target_url, payload)
        .await
    {
        Ok(result) => {
            let status = if result.created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (status, Json(delivery_json(&result.delivery))).into_response()
        }
        Err(err) => enqueue_error_response(err),
    }
}

pub(crate) async fn get_delivery<R>(
    State(service): State<Arc<DeliveryService<R>>>,
    Path(id_text): Path<String>,
) -> Response
where
    R: DeliveryRepository + Send + Sync + 'static,
{
    let id = match Uuid::parse_str(&id_text) {
        Ok(id) => id,
        Err(_) => return not_found(),
    };

    match service.get_by_id(id).await {
        Ok(Some(delivery)) => (StatusCode::OK, Json(delivery_json(&delivery))).into_response(),
        Ok(None) => not_found(),
        Err(QueryError::NotFound) => not_found(),
        Err(QueryError::Repository(_)) => internal_error(),
    }
}

pub(crate) async fn health() -> Response {
    (StatusCode::OK, Json(json!({ "status": "ok" }))).into_response()
}

fn delivery_json(delivery: &Delivery) -> Value {
    json!({
        "id": delivery.id.to_string(),
        "status": delivery.status.as_str(),
        "attempts": delivery.attempts,
        "target_url": delivery.target_url,
        "payload": delivery.payload,
        "created_at": delivery.created_at_rfc3339(),
    })
}

fn enqueue_error_response(err: EnqueueError) -> Response {
    match err {
        EnqueueError::MissingIdempotencyKey => error_response(
            StatusCode::BAD_REQUEST,
            "missing_idempotency_key",
            "Idempotency-Key header is required",
        ),
        EnqueueError::InvalidIdempotencyKey => error_response(
            StatusCode::BAD_REQUEST,
            "invalid_idempotency_key",
            "Idempotency-Key must not be empty and must be at most 128 bytes after trimming",
        ),
        EnqueueError::InvalidTargetUrl(message) => error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_target_url",
            &message,
        ),
        EnqueueError::IdempotencyConflict => error_response(
            StatusCode::CONFLICT,
            "idempotency_conflict",
            "idempotency key was already used with different content",
        ),
        EnqueueError::Repository(_) => internal_error(),
    }
}

fn not_found() -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        "delivery_not_found",
        "no delivery exists with the given id",
    )
}

fn internal_error() -> Response {
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        "an unexpected error occurred",
    )
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({ "error": { "code": code, "message": message } })),
    )
        .into_response()
}
