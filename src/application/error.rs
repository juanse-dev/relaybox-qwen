use thiserror::Error;

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("database error: {0}")]
    Database(String),
}

#[derive(Debug, Error)]
pub enum EnqueueError {
    #[error("Idempotency-Key header is required")]
    MissingIdempotencyKey,

    #[error("Idempotency-Key must not be empty and must be at most 128 bytes after trimming")]
    InvalidIdempotencyKey,

    #[error("{0}")]
    InvalidTargetUrl(String),

    #[error("idempotency key was already used with different content")]
    IdempotencyConflict,

    #[error(transparent)]
    Repository(#[from] RepositoryError),
}

#[derive(Debug, Error)]
pub enum QueryError {
    #[error("delivery not found")]
    NotFound,

    #[error(transparent)]
    Repository(#[from] RepositoryError),
}
