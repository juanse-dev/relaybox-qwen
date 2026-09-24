use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::json::DeepDroppableValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryStatus {
    Pending,
}

impl DeliveryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Delivery {
    pub id: Uuid,
    pub target_url: String,
    pub payload: Value,
    pub status: DeliveryStatus,
    pub attempts: u32,
    pub created_at: DateTime<Utc>,
}

impl Delivery {
    pub fn created_at_rfc3339(&self) -> String {
        self.created_at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    }
}

#[derive(Debug, Clone)]
pub struct NewDelivery {
    pub idempotency_key: String,
    pub target_url: String,
    pub payload: DeepDroppableValue,
    pub status: DeliveryStatus,
    pub attempts: u32,
}

impl NewDelivery {
    pub fn new(idempotency_key: String, target_url: String, payload: Value) -> Self {
        Self {
            idempotency_key,
            target_url,
            payload: DeepDroppableValue::new(payload),
            status: DeliveryStatus::Pending,
            attempts: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_delivery_starts_pending_with_zero_attempts() {
        let new = NewDelivery::new(
            "key-1".to_owned(),
            "https://example.test/hook".to_owned(),
            serde_json::json!({}),
        );
        assert_eq!(new.status, DeliveryStatus::Pending);
        assert_eq!(new.attempts, 0);
    }
}
