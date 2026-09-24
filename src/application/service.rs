use serde_json::Value;
use url::Url;
use uuid::Uuid;

use crate::application::{DeliveryRepository, EnqueueError, EnqueueResult, QueryError};
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

        self.repository
            .enqueue(&NewDelivery {
                idempotency_key: key,
                target_url: target_url.to_owned(),
                payload,
            })
            .await
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
}
