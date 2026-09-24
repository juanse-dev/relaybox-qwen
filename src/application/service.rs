use serde_json::Value;
use url::Url;
use uuid::Uuid;

use crate::application::{
    DeliveryRepository, EnqueueError, EnqueueOutcome, EnqueueResult, QueryError,
};
use crate::domain::{Delivery, NewDelivery};

pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;

#[derive(Debug, Clone)]
pub struct DeliveryService<R> {
    repository: R,
}

impl<R> DeliveryService<R> {
    pub fn new(repository: R) -> Self {
        Self { repository }
    }
}

impl<R: DeliveryRepository> DeliveryService<R> {
    pub async fn enqueue(
        &self,
        idempotency_key: Option<&str>,
        target_url: &str,
        payload: Value,
    ) -> Result<EnqueueResult, EnqueueError> {
        let key = normalize_idempotency_key(idempotency_key)?;
        validate_target_url(target_url)?;

        let new = NewDelivery::new(key, target_url.to_owned(), payload);

        match self.repository.enqueue(&new).await? {
            EnqueueOutcome::Created(delivery) => Ok(EnqueueResult {
                delivery,
                created: true,
            }),
            EnqueueOutcome::Existing(existing) => {
                if existing.target_url == new.target_url && existing.payload == new.payload {
                    Ok(EnqueueResult {
                        delivery: existing,
                        created: false,
                    })
                } else {
                    Err(EnqueueError::IdempotencyConflict)
                }
            }
        }
    }

    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<Delivery>, QueryError> {
        self.repository.get_by_id(id).await
    }
}

pub(crate) fn normalize_idempotency_key(key: Option<&str>) -> Result<String, EnqueueError> {
    let key = match key {
        Some(key) => key.trim_ascii(),
        None => return Err(EnqueueError::MissingIdempotencyKey),
    };

    if key.is_empty() || key.len() > MAX_IDEMPOTENCY_KEY_BYTES {
        return Err(EnqueueError::InvalidIdempotencyKey);
    }

    Ok(key.to_owned())
}

pub(crate) fn validate_target_url(target_url: &str) -> Result<(), EnqueueError> {
    let url = Url::parse(target_url).map_err(|_| {
        EnqueueError::InvalidTargetUrl("target_url must be an absolute URL".to_owned())
    })?;

    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(EnqueueError::InvalidTargetUrl(format!(
                "target_url scheme '{scheme}' is not supported, use http or https"
            )))
        }
    }

    if url.host_str().is_none() {
        return Err(EnqueueError::InvalidTargetUrl(
            "target_url must contain a host".to_owned(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_idempotency_key_trims_ascii_whitespace() {
        assert_eq!(
            normalize_idempotency_key(Some("  key-1 \t\n")).unwrap(),
            "key-1"
        );
    }

    #[test]
    fn normalize_idempotency_key_rejects_missing_empty_and_oversized_keys() {
        assert!(matches!(
            normalize_idempotency_key(None),
            Err(EnqueueError::MissingIdempotencyKey)
        ));
        assert!(matches!(
            normalize_idempotency_key(Some("   ")),
            Err(EnqueueError::InvalidIdempotencyKey)
        ));
        assert!(matches!(
            normalize_idempotency_key(Some(&"a".repeat(MAX_IDEMPOTENCY_KEY_BYTES + 1))),
            Err(EnqueueError::InvalidIdempotencyKey)
        ));
    }

    #[test]
    fn normalize_idempotency_key_accepts_128_byte_keys() {
        let key = "a".repeat(MAX_IDEMPOTENCY_KEY_BYTES);
        assert_eq!(normalize_idempotency_key(Some(&key)).unwrap(), key);
    }

    #[test]
    fn validate_target_url_accepts_http_and_https_urls_with_hosts() {
        assert!(validate_target_url("https://example.test/webhooks").is_ok());
        assert!(validate_target_url("http://localhost:8080/hook").is_ok());
    }

    #[test]
    fn validate_target_url_rejects_relative_urls_other_schemes_and_missing_hosts() {
        for url in ["/webhooks", "not a url", "ftp://example.test/hook"] {
            assert!(
                matches!(
                    validate_target_url(url),
                    Err(EnqueueError::InvalidTargetUrl(_))
                ),
                "expected rejection for {url}"
            );
        }
    }

    struct StubRepo {
        outcome: EnqueueOutcome,
    }

    #[async_trait::async_trait]
    impl DeliveryRepository for StubRepo {
        async fn enqueue(&self, _new: &NewDelivery) -> Result<EnqueueOutcome, EnqueueError> {
            Ok(self.outcome.clone())
        }

        async fn get_by_id(&self, _id: Uuid) -> Result<Option<Delivery>, QueryError> {
            Ok(None)
        }
    }

    struct CapturingRepo {
        captured: std::sync::Arc<std::sync::Mutex<Option<NewDelivery>>>,
    }

    #[async_trait::async_trait]
    impl DeliveryRepository for CapturingRepo {
        async fn enqueue(&self, new: &NewDelivery) -> Result<EnqueueOutcome, EnqueueError> {
            *self.captured.lock().unwrap() = Some(new.clone());
            Ok(EnqueueOutcome::Created(stub_delivery(
                &new.target_url,
                new.payload.clone(),
            )))
        }

        async fn get_by_id(&self, _id: Uuid) -> Result<Option<Delivery>, QueryError> {
            Ok(None)
        }
    }

    fn stub_delivery(target_url: &str, payload: Value) -> Delivery {
        Delivery {
            id: Uuid::new_v4(),
            target_url: target_url.to_owned(),
            payload,
            status: crate::domain::DeliveryStatus::Pending,
            attempts: 0,
            created_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn enqueue_returns_created_for_new_delivery() {
        let delivery = stub_delivery("https://example.test/hook", serde_json::json!({ "a": 1 }));
        let service = DeliveryService::new(StubRepo {
            outcome: EnqueueOutcome::Created(delivery.clone()),
        });

        let result = service
            .enqueue(
                Some("key-1"),
                "https://example.test/hook",
                serde_json::json!({ "a": 1 }),
            )
            .await
            .unwrap();

        assert!(result.created);
        assert_eq!(result.delivery, delivery);
    }

    #[tokio::test]
    async fn enqueue_replays_existing_delivery_with_identical_content() {
        let existing = stub_delivery("https://example.test/hook", serde_json::json!({ "a": 1 }));
        let service = DeliveryService::new(StubRepo {
            outcome: EnqueueOutcome::Existing(existing.clone()),
        });

        let result = service
            .enqueue(
                Some("key-1"),
                "https://example.test/hook",
                serde_json::json!({ "a": 1 }),
            )
            .await
            .unwrap();

        assert!(!result.created);
        assert_eq!(result.delivery, existing);
    }

    #[tokio::test]
    async fn enqueue_conflicts_when_existing_payload_differs() {
        let existing = stub_delivery("https://example.test/hook", serde_json::json!({ "a": 2 }));
        let service = DeliveryService::new(StubRepo {
            outcome: EnqueueOutcome::Existing(existing),
        });

        let result = service
            .enqueue(
                Some("key-1"),
                "https://example.test/hook",
                serde_json::json!({ "a": 1 }),
            )
            .await;

        assert!(matches!(result, Err(EnqueueError::IdempotencyConflict)));
    }

    #[tokio::test]
    async fn enqueue_conflicts_when_existing_target_url_differs() {
        let existing = stub_delivery("https://example.test/other", serde_json::json!({ "a": 1 }));
        let service = DeliveryService::new(StubRepo {
            outcome: EnqueueOutcome::Existing(existing),
        });

        let result = service
            .enqueue(
                Some("key-1"),
                "https://example.test/hook",
                serde_json::json!({ "a": 1 }),
            )
            .await;

        assert!(matches!(result, Err(EnqueueError::IdempotencyConflict)));
    }

    #[tokio::test]
    async fn enqueue_persists_initial_pending_state_with_zero_attempts() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
        let service = DeliveryService::new(CapturingRepo {
            captured: captured.clone(),
        });

        service
            .enqueue(
                Some("key-1"),
                "https://example.test/hook",
                serde_json::json!({}),
            )
            .await
            .unwrap();

        let new = captured
            .lock()
            .unwrap()
            .take()
            .expect("repository received the new delivery");
        assert_eq!(new.status, crate::domain::DeliveryStatus::Pending);
        assert_eq!(new.attempts, 0);
    }
}
