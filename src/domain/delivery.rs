use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

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
    pub payload: Value,
}
