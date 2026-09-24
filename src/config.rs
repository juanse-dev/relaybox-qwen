use std::net::SocketAddr;

use thiserror::Error;

pub const DEFAULT_DATABASE_URL: &str = "sqlite://relaybox.db";
pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:3000";

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: SocketAddr,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("RELAYBOX_DATABASE_URL must start with 'sqlite:'")]
    InvalidDatabaseUrl,

    #[error("RELAYBOX_BIND must be a valid host:port address")]
    InvalidBindAddress,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url = std::env::var("RELAYBOX_DATABASE_URL")
            .unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned());
        let bind_raw =
            std::env::var("RELAYBOX_BIND").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_owned());

        Ok(Self {
            database_url: parse_database_url(&database_url)?,
            bind_addr: parse_bind(&bind_raw)?,
        })
    }
}

pub(crate) fn parse_database_url(raw: &str) -> Result<String, ConfigError> {
    if raw.starts_with("sqlite:") {
        Ok(raw.to_owned())
    } else {
        Err(ConfigError::InvalidDatabaseUrl)
    }
}

pub(crate) fn parse_bind(raw: &str) -> Result<SocketAddr, ConfigError> {
    raw.parse::<SocketAddr>()
        .map_err(|_| ConfigError::InvalidBindAddress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_database_url_accepts_sqlite_urls() {
        assert_eq!(
            parse_database_url("sqlite://relaybox.db").unwrap(),
            "sqlite://relaybox.db"
        );
        assert_eq!(
            parse_database_url("sqlite::memory:").unwrap(),
            "sqlite::memory:"
        );
    }

    #[test]
    fn parse_database_url_rejects_non_sqlite_urls() {
        for raw in ["", "postgres://localhost/db", "relaybox.db"] {
            assert_eq!(
                parse_database_url(raw),
                Err(ConfigError::InvalidDatabaseUrl)
            );
        }
    }

    #[test]
    fn parse_bind_accepts_host_port_and_rejects_garbage() {
        assert_eq!(
            parse_bind("127.0.0.1:3000").unwrap(),
            "127.0.0.1:3000".parse().unwrap()
        );
        assert_eq!(
            parse_bind("[::1]:8443").unwrap(),
            "[::1]:8443".parse().unwrap()
        );
        for raw in ["", "localhost", "127.0.0.1", "not-an-addr"] {
            assert_eq!(parse_bind(raw), Err(ConfigError::InvalidBindAddress));
        }
    }
}
