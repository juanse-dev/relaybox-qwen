use std::sync::Arc;

use axum::{
    extract::DefaultBodyLimit,
    routing::{any, get, post},
    Router,
};

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
            post(handlers::create_delivery)
                .layer(DefaultBodyLimit::disable())
                .merge(any(handlers::unsupported_method)),
        )
        .route(
            "/v1/deliveries/{id}",
            get(handlers::get_delivery).merge(any(handlers::unsupported_method)),
        )
        .route(
            "/health",
            get(handlers::health).merge(any(handlers::unsupported_method)),
        )
        .fallback(handlers::unknown_path)
        .with_state(state)
}
