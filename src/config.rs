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

    #[error("RELAYBOX_DATABASE_URL must specify a non-empty database path")]
    InvalidDatabasePath,

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

    // SQLx ignores URL fragments, so validation must see the same string it will use.
    // SQLite decodes percent-escapes before interpreting `?` and `#`, so the
    // encoded forms `%3F`/`%23` are structural delimiters too.
    let fragment_idx = match (raw.find('#'), find_percent_encoded(raw, "%23")) {
        (Some(literal), Some(encoded)) => Some(literal.min(encoded)),
        (literal, encoded) => literal.or(encoded),
    };
    let without_fragment = match fragment_idx {
        Some(idx) => &raw[..idx],
        None => raw,
    };

    let rest = &without_fragment["sqlite:".len()..];
    let (base, query) = split_at_query_delimiter(rest);

    let decoded_base = percent_decode(base.as_bytes())
        .decode_utf8_lossy()
        .into_owned();

    if decoded_base.is_empty() || decoded_base == "//" {
        return Err(ConfigError::InvalidDatabasePath);
    }

    if decoded_base == ":memory:" || decoded_base == "//:memory:" {
        return Err(ConfigError::InMemoryDatabaseUrl);
    }

    let stripped_base = decoded_base.trim_start_matches('/');
    // Compare bytes so a multibyte character crossing offset five cannot panic.
    let stripped_bytes = stripped_base.as_bytes();
    if stripped_bytes.len() >= 5 && stripped_bytes[..5].eq_ignore_ascii_case(b"file:") {
        // The first five bytes are ASCII, so this slice is on a char boundary.
        let suffix = &stripped_base[5..];
        if suffix.is_empty() || suffix.bytes().all(|b| b == b'/') {
            return Err(ConfigError::InvalidDatabasePath);
        }
        if suffix.eq_ignore_ascii_case(":memory:") {
            return Err(ConfigError::InMemoryDatabaseUrl);
        }
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

    Ok(without_fragment.to_owned())
}

fn find_percent_encoded(haystack: &str, escape: &str) -> Option<usize> {
    let hay = haystack.as_bytes();
    let pat = escape.as_bytes();
    if hay.len() < pat.len() {
        return None;
    }
    (0..=hay.len() - pat.len()).find(|&i| hay[i..i + pat.len()].eq_ignore_ascii_case(pat))
}

fn split_at_query_delimiter(rest: &str) -> (&str, Option<&str>) {
    let literal = rest.find('?');
    let encoded = find_percent_encoded(rest, "%3F");
    match (literal, encoded) {
        (Some(l), Some(e)) if l <= e => (&rest[..l], Some(&rest[l + 1..])),
        (_, Some(e)) => (&rest[..e], Some(&rest[e + 3..])),
        (Some(l), None) => (&rest[..l], Some(&rest[l + 1..])),
        (None, None) => (rest, None),
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
    fn parse_database_url_rejects_percent_encoded_in_memory_paths() {
        for raw in [
            "sqlite:%3Amemory%3A",
            "sqlite://%3Amemory%3A",
            "sqlite:%3amemory%3a?cache=shared",
        ] {
            assert_eq!(
                parse_database_url(raw),
                Err(ConfigError::InMemoryDatabaseUrl)
            );
        }
    }

    #[test]
    fn parse_database_url_rejects_file_uri_in_memory_forms() {
        for raw in [
            "sqlite:file::memory:",
            "sqlite://file::memory:",
            "sqlite:file:%3Amemory%3A",
            "sqlite:file::memory:?cache=shared",
        ] {
            assert_eq!(
                parse_database_url(raw),
                Err(ConfigError::InMemoryDatabaseUrl)
            );
        }
    }

    #[test]
    fn parse_database_url_rejects_bare_file_uri_temporary_databases() {
        for raw in ["sqlite:file:", "sqlite://file:"] {
            assert_eq!(
                parse_database_url(raw),
                Err(ConfigError::InvalidDatabasePath)
            );
        }
    }

    #[test]
    fn parse_database_url_rejects_empty_file_uri_paths() {
        for raw in [
            "sqlite:file://",
            "sqlite://file://",
            "sqlite:file:%2F%2F",
            "sqlite:file://%2f%2f",
            "sqlite:file://?cache=shared",
        ] {
            assert_eq!(
                parse_database_url(raw),
                Err(ConfigError::InvalidDatabasePath)
            );
        }
    }

    #[test]
    fn parse_database_url_accepts_file_uri_paths() {
        for raw in [
            "sqlite:file:/tmp/relaybox.db",
            "sqlite://file:///var/lib/relaybox/db.sqlite",
        ] {
            assert_eq!(parse_database_url(raw).unwrap(), raw);
        }
    }

    #[test]
    fn parse_database_url_rejects_empty_database_path() {
        for raw in ["sqlite:", "sqlite://"] {
            assert_eq!(
                parse_database_url(raw),
                Err(ConfigError::InvalidDatabasePath)
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
    fn parse_database_url_strips_fragment_before_validation() {
        for raw in [
            "sqlite:///tmp/relaybox.db?mode=memory#ignored",
            "sqlite::memory:#frag",
            "sqlite://relaybox.db?cache=shared&mode=memory#x",
        ] {
            assert_eq!(
                parse_database_url(raw),
                Err(ConfigError::InMemoryDatabaseUrl)
            );
        }
    }

    #[test]
    fn parse_database_url_returns_fragment_stripped_url() {
        assert_eq!(
            parse_database_url("sqlite://relaybox.db#session").unwrap(),
            "sqlite://relaybox.db"
        );
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

    #[test]
    fn parse_database_url_handles_multibyte_filenames_without_panic() {
        assert_eq!(
            parse_database_url("sqlite:abcdé.db").unwrap(),
            "sqlite:abcdé.db"
        );
        assert_eq!(
            parse_database_url("sqlite://file:/var/lib/abç.db").unwrap(),
            "sqlite://file:/var/lib/abç.db"
        );
    }

    #[test]
    fn parse_database_url_rejects_mixed_case_file_uri_memory() {
        assert_eq!(
            parse_database_url("sqlite:File::memory:"),
            Err(ConfigError::InMemoryDatabaseUrl)
        );
    }

    #[test]
    fn parse_database_url_treats_encoded_query_delimiter_as_structural() {
        for raw in [
            "sqlite:///tmp/relaybox.db%3Fmode=memory",
            "sqlite:file:relaybox.db%3Fmode=memory",
            "sqlite://relaybox.db%3fmode=memory",
        ] {
            assert_eq!(
                parse_database_url(raw),
                Err(ConfigError::InMemoryDatabaseUrl)
            );
        }
    }

    #[test]
    fn parse_database_url_treats_encoded_fragment_as_structural() {
        assert_eq!(
            parse_database_url("sqlite://relaybox.db%23session").unwrap(),
            "sqlite://relaybox.db"
        );
        assert_eq!(
            parse_database_url("sqlite::memory:%23frag"),
            Err(ConfigError::InMemoryDatabaseUrl)
        );
    }

    #[test]
    fn parse_database_url_keeps_double_encoded_delimiters_in_path() {
        assert_eq!(
            parse_database_url("sqlite://db%253Fx").unwrap(),
            "sqlite://db%253Fx"
        );
    }
}
