use async_trait::async_trait;
use uuid::Uuid;

use crate::application::{EnqueueError, QueryError};
use crate::domain::{Delivery, NewDelivery};

#[derive(Debug, Clone)]
pub struct EnqueueResult {
    pub delivery: Delivery,
    pub created: bool,
}

#[derive(Debug, Clone)]
pub enum EnqueueOutcome {
    Created(Delivery),
    Existing(Delivery),
}

#[async_trait]
pub trait DeliveryRepository: Send + Sync {
    async fn enqueue(&self, new: &NewDelivery) -> Result<EnqueueOutcome, EnqueueError>;

    async fn get_by_id(&self, id: Uuid) -> Result<Option<Delivery>, QueryError>;
}
