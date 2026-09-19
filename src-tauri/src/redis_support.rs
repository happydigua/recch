//! Redis connections are request-scoped: SELECT, MULTI and AUTH must not leak
//! from one UI operation into another through cloned multiplexed connections.
use crate::ConnectionConfig;
use redis::{aio::MultiplexedConnection, ConnectionAddr, ConnectionInfo, RedisConnectionInfo};
use std::collections::HashSet;
use std::time::Duration;

pub fn parse_database(value: Option<&str>) -> Result<i64, String> {
    let value = value.unwrap_or("").trim();
    if value.is_empty() { return Ok(0); }
    let mut parts = value.split_whitespace();
    let first = parts.next().unwrap_or("");
    let digits = first.strip_prefix("db").unwrap_or(first);
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return Err("Invalid Redis database; use 0, db0, or db0 (count)".into());
    }
    if let Some(count) = parts.next() {
        let count = count.strip_prefix('(').and_then(|v| v.strip_suffix(')')).ok_or("Invalid Redis database label")?;
        if count.is_empty() || !count.bytes().all(|c| c.is_ascii_digit()) || parts.next().is_some() {
            return Err("Invalid Redis database label".into());
        }
    }
    digits.parse().map_err(|_| "Redis database index is out of range".into())
}

pub fn selected_config(config: &ConnectionConfig, database: Option<&str>) -> Result<ConnectionConfig, String> {
    let mut selected = config.clone();
    selected.database = Some(parse_database(database.or(config.database.as_deref()))?.to_string());
    Ok(selected)
}

fn connection_info(config: &ConnectionConfig) -> Result<ConnectionInfo, String> {
    let username = config.username.clone().filter(|v| !v.is_empty());
    let password = config.password.clone().filter(|v| !v.is_empty())
        .or_else(|| username.as_ref().map(|_| String::new()));
    Ok(ConnectionInfo {
        addr: ConnectionAddr::Tcp(config.host.clone(), config.port),
        redis: RedisConnectionInfo {
            db: parse_database(config.database.as_deref())?, username, password,
            ..Default::default()
        },
    })
}

pub async fn connect(config: &ConnectionConfig) -> Result<MultiplexedConnection, String> {
    let client = redis::Client::open(connection_info(config)?).map_err(|e| e.to_string())?;
    let options = redis::AsyncConnectionConfig::new()
        .set_connection_timeout(Duration::from_secs(15))
        .set_response_timeout(Duration::from_secs(30));
    client.get_multiplexed_async_connection_with_config(&options).await.map_err(|e| e.to_string())
}

/// SCAN rather than KEYS prevents a full keyspace scan from blocking the server.
/// The UI's non-paginated API fails explicitly at the bound rather than lying
/// about having loaded every key. COUNT is a hint, so also check the actual size.
pub async fn scan_keys(connection: &mut MultiplexedConnection) -> Result<Vec<String>, String> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut cursor = 0u64;
        let mut keys = HashSet::new();
        for _ in 0..10_000 {
            let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN").arg(cursor)
                .arg("COUNT").arg(200).query_async(connection).await.map_err(|e| e.to_string())?;
            keys.extend(batch);
            if keys.len() > 10_000 { return Err("Redis key browser supports at most 10000 keys; use cursor-based SCAN in the console for larger databases".into()); }
            cursor = next;
            if cursor == 0 { let mut result: Vec<_> = keys.into_iter().collect(); result.sort(); return Ok(result); }
        }
        Err("Redis scan exceeded its iteration limit".into())
    }).await.map_err(|_| "Redis scan timed out".to_string())?
}

/// Parse the complete batch before executing anything: malformed final lines
/// must not leave preceding mutations partially applied. This is not a transaction.
pub fn parse_commands(query: &str) -> Result<Vec<Vec<String>>, String> {
    let mut commands = Vec::new();
    for line in query.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("--") { continue; }
        let mut args = Vec::new();
        let mut current = String::new();
        let mut quote = None;
        let mut escaped = false;
        let mut started = false;
        for c in line.chars() {
            if escaped {
                current.push(match c { 'n' => '\n', 'r' => '\r', 't' => '\t', _ => c });
                escaped = false; started = true;
            } else if c == '\\' { escaped = true; started = true;
            } else if let Some(q) = quote {
                if c == q { quote = None; } else { current.push(c); }
            } else if c == '\'' || c == '"' { quote = Some(c); started = true;
            } else if c.is_whitespace() {
                if started { args.push(std::mem::take(&mut current)); started = false; }
            } else { current.push(c); started = true; }
        }
        if quote.is_some() || escaped { return Err("Unterminated quote or escape in Redis command".into()); }
        if started { args.push(current); }
        if args.first().map(|s| s.is_empty()).unwrap_or(true) { return Err("Missing Redis command name".into()); }
        commands.push(args);
    }
    Ok(commands)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> ConnectionConfig {
        ConnectionConfig { id: "test".into(), name: "test".into(), db_type: "redis".into(),
            host: "127.0.0.1".into(), port: 6379, username: None, password: None, database: None }
    }
    #[test]
    fn database_labels_are_strict_and_never_fall_back_on_typos() {
        for input in [None, Some(""), Some("0"), Some("db0 (12)")] { assert_eq!(parse_database(input).unwrap(), 0); }
        assert_eq!(parse_database(Some(" db12 (34) ")).unwrap(), 12);
        for input in ["prod", "db-1", "1abc", "db", "db2 garbage", "db3 (x)", "2 (1) extra", "999999999999999999999999"] {
            assert!(parse_database(Some(input)).is_err(), "{input}");
        }
    }
    #[test]
    fn structured_credentials_preserve_acl_and_reserved_characters() {
        let mut config = config(); config.host = "::1".into(); config.username = Some("alice@team".into());
        config.password = Some("p@ss:/?#%".into()); config.database = Some("db2 (0)".into());
        let info = connection_info(&config).unwrap();
        assert_eq!(info.redis.username.as_deref(), Some("alice@team"));
        assert_eq!(info.redis.password.as_deref(), Some("p@ss:/?#%")); assert_eq!(info.redis.db, 2);
        assert!(matches!(info.addr, ConnectionAddr::Tcp(host, 6379) if host == "::1"));
    }
    #[test]
    fn parser_preserves_empty_quoted_arguments_and_spaces() {
        assert_eq!(parse_commands("SET key \"\"\nSET 'two words' 'a \\\' quote'\n# skip").unwrap(),
            vec![vec!["SET", "key", ""], vec!["SET", "two words", "a ' quote"]]);
        assert!(parse_commands("SET good 1\nSET bad \"unfinished").is_err());
        assert!(parse_commands("GET key\\").is_err());
    }
    #[tokio::test]
    #[ignore = "requires the disposable CI Redis service"]
    async fn integration_requests_have_independent_database_and_transaction_state() {
        let config = config();
        let mut first = connect(&config).await.unwrap(); let mut second = connect(&config).await.unwrap();
        let key = format!("recch-isolation-{}", uuid::Uuid::new_v4());
        let _: () = redis::cmd("SELECT").arg(1).query_async(&mut first).await.unwrap();
        let _: () = redis::cmd("SET").arg(&key).arg("only-db1").query_async(&mut first).await.unwrap();
        let value: Option<String> = redis::cmd("GET").arg(&key).query_async(&mut second).await.unwrap();
        assert_eq!(value, None);
        let _: () = redis::cmd("MULTI").query_async(&mut first).await.unwrap();
        let pong: String = redis::cmd("PING").query_async(&mut second).await.unwrap(); assert_eq!(pong, "PONG");
        let _: () = redis::cmd("DISCARD").query_async(&mut first).await.unwrap();
        assert!(scan_keys(&mut first).await.unwrap().contains(&key));
        let _: i64 = redis::cmd("DEL").arg(&key).query_async(&mut first).await.unwrap();
    }
}
