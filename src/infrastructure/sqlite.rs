use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions, SqliteRow};
use sqlx::Error as SqlxError;
use sqlx::Row;
use uuid::Uuid;

use crate::application::{
    DeliveryRepository, EnqueueError, EnqueueOutcome, QueryError, RepositoryError,
};
use crate::domain::{Delivery, DeliveryStatus, NewDelivery};

pub struct SqliteDeliveryRepository {
    pool: SqlitePool,
}

impl SqliteDeliveryRepository {
    pub async fn connect(database_url: &str) -> Result<Self, SqlxError> {
        let options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true)
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;

        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&pool)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool })
    }

    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl DeliveryRepository for SqliteDeliveryRepository {
    async fn enqueue(&self, new: &NewDelivery) -> Result<EnqueueOutcome, EnqueueError> {
        let id = Uuid::new_v4();
        let created_at = format_created_at(&Utc::now());
        let payload_text = crate::json::to_string(&new.payload);

        match sqlx::query(
            "INSERT INTO deliveries (id, idempotency_key, target_url, payload, status, attempts, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             RETURNING id, target_url, payload, status, attempts, created_at",
        )
        .bind(id.to_string())
        .bind(&new.idempotency_key)
        .bind(&new.target_url)
        .bind(&payload_text)
        .bind(new.status.as_str())
        .bind(i64::from(new.attempts))
        .bind(&created_at)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(Some(row)) => Ok(EnqueueOutcome::Created(delivery_from_row(&row)?)),
            Ok(None) => Err(RepositoryError::Database("insert returned no row".to_owned()).into()),
            Err(err) if is_unique_violation(&err) => {
                let row = sqlx::query(
                    "SELECT id, target_url, payload, status, attempts, created_at \
                     FROM deliveries WHERE idempotency_key = ?",
                )
                .bind(&new.idempotency_key)
                .fetch_optional(&self.pool)
                .await
                .map_err(|err| RepositoryError::Database(err.to_string()))?;

                let row = row.ok_or_else(|| {
                    RepositoryError::Database("idempotent delivery row disappeared".to_owned())
                })?;
                Ok(EnqueueOutcome::Existing(delivery_from_row(&row)?))
            }
            Err(err) => Err(RepositoryError::Database(err.to_string()).into()),
        }
    }

    async fn get_by_id(&self, id: Uuid) -> Result<Option<Delivery>, QueryError> {
        let row = sqlx::query(
            "SELECT id, target_url, payload, status, attempts, created_at \
             FROM deliveries WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| RepositoryError::Database(err.to_string()))?;

        match row {
            Some(row) => Ok(Some(delivery_from_row(&row)?)),
            None => Ok(None),
        }
    }
}

fn delivery_from_row(row: &SqliteRow) -> Result<Delivery, RepositoryError> {
    let id_text = row.try_get::<String, _>("id").map_err(db_error)?;
    let target_url = row.try_get::<String, _>("target_url").map_err(db_error)?;
    let payload_text = row.try_get::<String, _>("payload").map_err(db_error)?;
    let status_text = row.try_get::<String, _>("status").map_err(db_error)?;
    let attempts_raw = row.try_get::<i64, _>("attempts").map_err(db_error)?;
    let created_at_text = row.try_get::<String, _>("created_at").map_err(db_error)?;

    let id = Uuid::parse_str(&id_text)
        .map_err(|err| RepositoryError::Database(format!("invalid stored delivery id: {err}")))?;
    let payload = crate::json::parse_value(payload_text.as_bytes())
        .map_err(|err| RepositoryError::Database(format!("invalid stored payload: {err:?}")))?;
    let status = DeliveryStatus::parse(&status_text)
        .ok_or_else(|| RepositoryError::Database("unknown stored delivery status".to_owned()))?;
    let attempts = u32::try_from(attempts_raw)
        .map_err(|_| RepositoryError::Database("invalid stored attempt count".to_owned()))?;
    let created_at = parse_created_at(&created_at_text)?;

    Ok(Delivery {
        id,
        target_url,
        payload,
        status,
        attempts,
        created_at,
    })
}

fn db_error(err: SqlxError) -> RepositoryError {
    RepositoryError::Database(err.to_string())
}

fn is_unique_violation(err: &SqlxError) -> bool {
    match err {
        SqlxError::Database(db_err) => {
            matches!(db_err.code().as_deref(), Some("19") | Some("2067"))
                || db_err.message().contains("UNIQUE constraint failed")
        }
        _ => false,
    }
}

fn format_created_at(value: &DateTime<Utc>) -> String {
    value.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn parse_created_at(value: &str) -> Result<DateTime<Utc>, RepositoryError> {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|err| RepositoryError::Database(format!("invalid stored created_at: {err}")))
}
