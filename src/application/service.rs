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
        let key = match normalize_idempotency_key(idempotency_key) {
            Ok(key) => key,
            Err(err) => {
                crate::json::deep_drop(payload);
                return Err(err);
            }
        };
        if let Err(err) = validate_target_url(target_url) {
            crate::json::deep_drop(payload);
            return Err(err);
        }

        let new = NewDelivery::new(key, target_url.to_owned(), payload);

        match self.repository.enqueue(&new).await {
            Ok(EnqueueOutcome::Created(delivery)) => Ok(EnqueueResult {
                delivery,
                created: true,
            }),
            Ok(EnqueueOutcome::Existing(existing)) => {
                if existing.target_url == new.target_url
                    && crate::json::values_equal(&existing.payload, &new.payload)
                {
                    Ok(EnqueueResult {
                        delivery: existing,
                        created: false,
                    })
                } else {
                    crate::json::deep_drop(existing.payload);
                    Err(EnqueueError::IdempotencyConflict)
                }
            }
            Err(err) => Err(err),
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

    if url.host_str().is_none() || has_empty_authority(target_url) {
        return Err(EnqueueError::InvalidTargetUrl(
            "target_url must contain a host".to_owned(),
        ));
    }

    Ok(())
}

fn has_empty_authority(raw: &str) -> bool {
    let Some(colon) = raw.find(':') else {
        return true;
    };
    let Some(rest) = raw[colon + 1..].strip_prefix("//") else {
        return true;
    };
    let rest = rest.trim_start_matches(|c: char| c <= ' ');
    rest.is_empty() || matches!(rest.as_bytes()[0], b'/' | b'\\' | b'?' | b'#')
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
        for url in [
            "https://example.test/webhooks",
            "http://localhost:8080/hook",
            "HTTP://EXAMPLE.TEST/X",
            "http://user@example.test/hook",
            "http://[::1]/x",
        ] {
            assert!(
                validate_target_url(url).is_ok(),
                "expected acceptance for {url}"
            );
        }
    }

    #[test]
    fn validate_target_url_rejects_relative_urls_other_schemes_and_missing_hosts() {
        for url in [
            "/webhooks",
            "not a url",
            "ftp://example.test/hook",
            "https:///path",
            "http:////example.com/x",
            "http:/path",
            "http:\\path",
            "https://\n/path",
            "https://\t/path",
            "https://\r/path",
            "http://\n\n/x",
        ] {
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
                &new.payload,
            )))
        }

        async fn get_by_id(&self, _id: Uuid) -> Result<Option<Delivery>, QueryError> {
            Ok(None)
        }
    }

    fn stub_delivery(target_url: &str, payload: &Value) -> Delivery {
        Delivery {
            id: Uuid::new_v4(),
            target_url: target_url.to_owned(),
            payload: payload.clone(),
            status: crate::domain::DeliveryStatus::Pending,
            attempts: 0,
            created_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn enqueue_returns_created_for_new_delivery() {
        let delivery = stub_delivery("https://example.test/hook", &serde_json::json!({ "a": 1 }));
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
        let existing = stub_delivery("https://example.test/hook", &serde_json::json!({ "a": 1 }));
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
        let existing = stub_delivery("https://example.test/hook", &serde_json::json!({ "a": 2 }));
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
        let existing = stub_delivery("https://example.test/other", &serde_json::json!({ "a": 1 }));
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

    fn build_deep_payload(depth: usize) -> Value {
        let mut payload = Value::from(0);
        for _ in 0..depth {
            payload = Value::Array(vec![payload]);
        }
        payload
    }

    struct HangingRepo;

    #[async_trait::async_trait]
    impl DeliveryRepository for HangingRepo {
        async fn enqueue(&self, _new: &NewDelivery) -> Result<EnqueueOutcome, EnqueueError> {
            std::future::pending::<()>().await;
            unreachable!("repository never completes")
        }

        async fn get_by_id(&self, _id: Uuid) -> Result<Option<Delivery>, QueryError> {
            Ok(None)
        }
    }

    #[test]
    fn dropping_cancelled_enqueue_future_with_deep_payload_is_stack_safe() {
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .expect("current-thread runtime builds");

                runtime.block_on(async {
                    let service = DeliveryService::new(HangingRepo);
                    let payload = build_deep_payload(50_000);
                    let handle = tokio::spawn(async move {
                        let _ = service
                            .enqueue(Some("key-1"), "https://example.test/hook", payload)
                            .await;
                    });

                    // Let the spawned task run until it suspends inside
                    // HangingRepo::enqueue, then cancel it. Dropping the suspended
                    // future must destroy the deep payload iteratively instead of
                    // recursing through it on this small stack.
                    tokio::task::yield_now().await;
                    handle.abort();
                });
            })
            .expect("small-stack test thread starts")
            .join()
            .expect("small-stack test thread completes");
    }
}
