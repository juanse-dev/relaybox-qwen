use std::sync::Arc;

use axum::{extract::DefaultBodyLimit, routing::get, routing::post, Router};

use crate::api::handlers;
use crate::application::{DeliveryRepository, DeliveryService};

pub fn router<R>(service: DeliveryService<R>) -> Router
where
    R: DeliveryRepository + Send + Sync + 'static,
{
    let state = Arc::new(service);

    Router::new()
        .route(
            "/v1/deliveries",
            post(handlers::create_delivery).layer(DefaultBodyLimit::disable()),
        )
        .route("/v1/deliveries/{id}", get(handlers::get_delivery))
        .route("/health", get(handlers::health))
        .with_state(state)
}
