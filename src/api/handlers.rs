use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use percent_encoding::percent_decode;
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
        Some(value) => match std::str::from_utf8(value.as_bytes()) {
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

    let parsed = match crate::json::parse_value(&body) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body must be valid JSON",
            )
        }
    };
    // Release the raw request buffer before enqueue so large payloads do not stay resident.
    drop(body);

    let mut object = match parsed {
        Value::Object(object) => object,
        other => {
            crate::json::deep_drop(other);
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "request body must be a JSON object with target_url and payload fields",
            );
        }
    };

    let target_url = match object.remove("target_url") {
        None => {
            crate::json::deep_drop(Value::Object(object));
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "target_url is required and must be a string",
            );
        }
        Some(Value::String(url)) => url,
        Some(other) => {
            crate::json::deep_drop(other);
            crate::json::deep_drop(Value::Object(object));
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_target_url",
                "target_url must be a string",
            );
        }
    };

    let payload = match object.remove("payload") {
        Some(payload) => payload,
        None => {
            crate::json::deep_drop(Value::Object(object));
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "payload is required and may be any JSON value including null",
            );
        }
    };

    crate::json::deep_drop(Value::Object(object));

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
            let body = delivery_body(&result.delivery);
            crate::json::deep_drop(result.delivery.payload);
            json_response(status, body)
        }
        Err(err) => enqueue_error_response(err),
    }
}

pub(crate) async fn get_delivery<R>(
    State(service): State<Arc<DeliveryService<R>>>,
    request: Request,
) -> Response
where
    R: DeliveryRepository + Send + Sync + 'static,
{
    let id = match delivery_id_from_request(&request) {
        Some(id) => id,
        None => return not_found(),
    };

    match service.get_by_id(id).await {
        Ok(Some(delivery)) => {
            let body = delivery_body(&delivery);
            crate::json::deep_drop(delivery.payload);
            json_response(StatusCode::OK, body)
        }
        Ok(None) => not_found(),
        Err(QueryError::NotFound) => not_found(),
        Err(QueryError::Repository(_)) => internal_error(),
    }
}

fn delivery_id_from_request(request: &Request) -> Option<Uuid> {
    let segment = request.uri().path().strip_prefix("/v1/deliveries/")?;
    if segment.is_empty() || segment.contains('/') {
        return None;
    }
    let decoded = percent_decode(segment.as_bytes()).decode_utf8().ok()?;
    Uuid::parse_str(&decoded).ok()
}

pub(crate) async fn health() -> Response {
    (StatusCode::OK, Json(json!({ "status": "ok" }))).into_response()
}

fn delivery_body(delivery: &Delivery) -> String {
    let mut out = String::from("{\"attempts\":");
    out.push_str(&delivery.attempts.to_string());
    out.push_str(",\"created_at\":");
    crate::json::append_string(&mut out, &delivery.created_at_rfc3339());
    out.push_str(",\"id\":");
    crate::json::append_string(&mut out, &delivery.id.to_string());
    out.push_str(",\"payload\":");
    crate::json::append_value(&delivery.payload, &mut out);
    out.push_str(",\"status\":");
    crate::json::append_string(&mut out, delivery.status.as_str());
    out.push_str(",\"target_url\":");
    crate::json::append_string(&mut out, &delivery.target_url);
    out.push('}');
    out
}

fn json_response(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
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

pub(crate) async fn unknown_path() -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        "not_found",
        "no route matches the given path",
    )
}

pub(crate) async fn unsupported_method() -> Response {
    error_response(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "the request method is not allowed for this route",
    )
}
