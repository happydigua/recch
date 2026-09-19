use crate::ConnectionConfig;
use std::time::Duration;

pub fn database_index(database: Option<&str>) -> Result<i64, String> {
    let text = database.unwrap_or("").trim();
    if text.is_empty() {
        return Ok(0);
    }
    let (number, suffix) = text.split_once(' ').unwrap_or((text, ""));
    if !suffix.is_empty() {
        let count = suffix
            .trim()
            .strip_prefix('(')
            .and_then(|v| v.strip_suffix(')'))
            .ok_or("Invalid Redis database label")?;
        if count.is_empty() || !count.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Invalid Redis database label".into());
        }
    }
    let number = number.strip_prefix("db").unwrap_or(number);
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Redis database must be a nonnegative integer (e.g. 0 or db0)".into());
    }
    number
        .parse()
        .map_err(|_| "Redis database index is too large".into())
}

/// Each command invocation owns its own session. SELECT, MULTI and AUTH in
/// the console must not change a key browser or another command's connection.
pub async fn connect(
    config: &ConnectionConfig,
) -> Result<redis::aio::MultiplexedConnection, String> {
    let info = redis::ConnectionInfo {
        addr: redis::ConnectionAddr::Tcp(config.host.clone(), config.port),
        redis: redis::RedisConnectionInfo {
            db: database_index(config.database.as_deref())?,
            username: config.username.clone().filter(|v| !v.is_empty()),
            password: if config.username.as_deref().is_some_and(|u| !u.is_empty()) {
                Some(config.password.clone().unwrap_or_default())
            } else {
                config.password.clone().filter(|v| !v.is_empty())
            },
            ..Default::default()
        },
    };
    let client = redis::Client::open(info).map_err(|e| e.to_string())?;
    let mut connection = tokio::time::timeout(
        Duration::from_secs(10),
        client.get_multiplexed_async_connection(),
    )
    .await
    .map_err(|_| "Redis connection timed out".to_string())?
    .map_err(|e| e.to_string())?;
    connection.set_response_timeout(Duration::from_secs(30));
    Ok(connection)
}

/// Parse one CLI line, preserving empty arguments and rejecting malformed input
/// before any command in a batch is sent. No shell is involved.
pub fn command_args(line: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for ch in line.chars() {
        if escaped {
            current.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
            started = true;
        } else if Some(ch) == quote {
            quote = None;
        } else if quote.is_some() {
            current.push(ch);
        } else if ch == '"' || ch == '\'' {
            quote = Some(ch);
            started = true;
        } else if ch.is_whitespace() {
            if started {
                args.push(std::mem::take(&mut current));
                started = false;
            }
        } else {
            current.push(ch);
            started = true;
        }
    }
    if quote.is_some() || escaped {
        return Err("Unclosed Redis quote or trailing escape".into());
    }
    if started {
        args.push(current);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_database_labels_without_silent_fallback() {
        for text in ["0", "db0", "db0 (42)", ""] {
            assert_eq!(database_index(Some(text)).unwrap(), 0);
        }
        assert_eq!(database_index(Some("db15 (0)")).unwrap(), 15);
        for text in [
            "-1",
            "dbx",
            "db",
            "1 junk",
            "1 (oops)",
            "9999999999999999999999",
        ] {
            assert!(database_index(Some(text)).is_err(), "{text}");
        }
    }
    #[test]
    fn preserves_quoted_empty_and_space_arguments() {
        assert_eq!(
            command_args("SET 'key name' \"\"").unwrap(),
            vec!["SET", "key name", ""]
        );
        assert_eq!(
            command_args(r#"SET key "a\"b\\c""#).unwrap(),
            vec!["SET", "key", "a\"b\\c"]
        );
        assert!(command_args("SET key \"oops").is_err());
        assert!(command_args("SET key trailing\\").is_err());
    }
}
