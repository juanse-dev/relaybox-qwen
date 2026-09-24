use std::net::SocketAddr;

use percent_encoding::percent_decode;
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

    #[error("RELAYBOX_DATABASE_URL must not use an in-memory database")]
    InMemoryDatabaseUrl,

    #[error("RELAYBOX_BIND must be a valid host:port address")]
    InvalidBindAddress,

    #[error("{0} is set to a non-Unicode value")]
    NonUnicodeEnvVar(String),
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url = resolve_env_var(
            "RELAYBOX_DATABASE_URL",
            std::env::var("RELAYBOX_DATABASE_URL"),
            DEFAULT_DATABASE_URL,
        )?;
        let bind_raw = resolve_env_var(
            "RELAYBOX_BIND",
            std::env::var("RELAYBOX_BIND"),
            DEFAULT_BIND_ADDR,
        )?;

        Ok(Self {
            database_url: parse_database_url(&database_url)?,
            bind_addr: parse_bind(&bind_raw)?,
        })
    }
}

pub(crate) fn resolve_env_var(
    name: &str,
    value: Result<String, std::env::VarError>,
    default: &str,
) -> Result<String, ConfigError> {
    match value {
        Ok(value) => Ok(value),
        Err(std::env::VarError::NotPresent) => Ok(default.to_owned()),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(ConfigError::NonUnicodeEnvVar(name.to_owned()))
        }
    }
}

pub(crate) fn parse_database_url(raw: &str) -> Result<String, ConfigError> {
    if !raw.starts_with("sqlite:") {
        return Err(ConfigError::InvalidDatabaseUrl);
    }

    let rest = &raw["sqlite:".len()..];
    let (base, query) = match rest.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (rest, None),
    };

    if base == ":memory:" || base == "//:memory:" {
        return Err(ConfigError::InMemoryDatabaseUrl);
    }

    if let Some(query) = query {
        for pair in query.split('&') {
            let decoded = percent_decode(pair.as_bytes())
                .decode_utf8_lossy()
                .into_owned();
            let (key, value) = match decoded.split_once('=') {
                Some((key, value)) => (key, value),
                None => continue,
            };
            if key == "mode" && value == "memory" {
                return Err(ConfigError::InMemoryDatabaseUrl);
            }
        }
    }

    Ok(raw.to_owned())
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
    fn parse_database_url_rejects_in_memory_databases() {
        for raw in [
            "sqlite::memory:",
            "sqlite://:memory:",
            "sqlite::memory:?cache=shared",
            "sqlite:///tmp/relaybox.db?mode=memory",
            "sqlite:///tmp/relaybox.db?cache=shared&mode=memory",
        ] {
            assert_eq!(
                parse_database_url(raw),
                Err(ConfigError::InMemoryDatabaseUrl)
            );
        }
    }

    #[test]
    fn parse_database_url_rejects_percent_encoded_in_memory_modes() {
        for raw in [
            "sqlite:///tmp/relaybox.db?mode=%6demory",
            "sqlite:///tmp/relaybox.db?mo%64e=memory",
            "sqlite:///tmp/relaybox.db?mode=mem%6Fry",
            "sqlite:///tmp/relaybox.db?mode%3Dmemory",
        ] {
            assert_eq!(
                parse_database_url(raw),
                Err(ConfigError::InMemoryDatabaseUrl)
            );
        }
    }

    #[test]
    fn parse_database_url_accepts_filenames_containing_mode_memory() {
        for raw in [
            "sqlite://mode=memory.db",
            "sqlite:///var/lib/mode=memory-backup.db",
            "sqlite://relaybox.db?cache=shared",
        ] {
            assert_eq!(parse_database_url(raw).unwrap(), raw);
        }
    }

    #[test]
    fn resolve_env_var_defaults_only_when_not_present() {
        let not_present = std::env::var("RELAYBOX_UNSET_VAR_FOR_TEST");
        match &not_present {
            Err(std::env::VarError::NotPresent) => {}
            other => panic!("expected NotPresent, got {other:?}"),
        }
        assert_eq!(
            resolve_env_var("RELAYBOX_DATABASE_URL", not_present, DEFAULT_DATABASE_URL).unwrap(),
            DEFAULT_DATABASE_URL
        );

        let set = Ok("sqlite://custom.db".to_owned());
        assert_eq!(
            resolve_env_var("RELAYBOX_DATABASE_URL", set, DEFAULT_DATABASE_URL).unwrap(),
            "sqlite://custom.db"
        );
    }

    #[test]
    fn resolve_env_var_rejects_non_unicode_values() {
        let not_unicode = Err(std::env::VarError::NotUnicode(std::ffi::OsString::from(
            "not-unicode",
        )));
        assert_eq!(
            resolve_env_var("RELAYBOX_BIND", not_unicode, DEFAULT_BIND_ADDR),
            Err(ConfigError::NonUnicodeEnvVar("RELAYBOX_BIND".to_owned()))
        );
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
