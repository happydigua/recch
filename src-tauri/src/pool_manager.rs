use sqlx::mysql::{MySqlConnectOptions, MySqlPool, MySqlPoolOptions};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::ConnectionConfig;

/// Deliberately not Debug: connection credentials must never appear in logs.
#[derive(Clone, PartialEq, Eq, Hash)]
struct PoolKey {
    connection_id: String,
    db_type: String,
    host: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
    database: String,
}

/// Enum to hold different types of database connection pools
pub enum PoolEntry {
    MySql(MySqlPool),
    Postgres(PgPool),
    Redis(redis::aio::MultiplexedConnection),
}

/// Manages connection pools for all active database connections
pub struct PoolManager {
    pools: RwLock<HashMap<PoolKey, PoolEntry>>,
}

impl PoolManager {
    pub fn new() -> Self {
        Self {
            pools: RwLock::new(HashMap::new()),
        }
    }

    /// Include identity and credentials so edits cannot reuse an old login.
    fn pool_key(config: &ConnectionConfig, database: Option<&str>) -> PoolKey {
        PoolKey {
            connection_id: config.id.clone(),
            db_type: config.db_type.clone(),
            host: config.host.clone(),
            port: config.port,
            username: config.username.clone(),
            password: config.password.clone(),
            database: database.or(config.database.as_deref()).unwrap_or("").to_string(),
        }
    }

    fn mysql_options(config: &ConnectionConfig, database: Option<&str>) -> MySqlConnectOptions {
        let mut options = MySqlConnectOptions::new().host(&config.host).port(config.port);
        if let Some(username) = &config.username {
            options = options.username(username);
        }
        if let Some(password) = &config.password {
            options = options.password(password);
        }
        if let Some(database) = database.or(config.database.as_deref()).filter(|db| !db.is_empty()) {
            options = options.database(database);
        }
        options
    }

    fn pg_options(config: &ConnectionConfig, database: Option<&str>) -> PgConnectOptions {
        let mut options = PgConnectOptions::new().host(&config.host).port(config.port);
        if let Some(username) = &config.username {
            options = options.username(username);
        }
        if let Some(password) = &config.password {
            options = options.password(password);
        }
        if let Some(database) = database.or(config.database.as_deref()).filter(|db| !db.is_empty()) {
            options = options.database(database);
        }
        options
    }

    /// Get or create a MySQL connection pool
    pub async fn get_mysql_pool(
        &self,
        config: &ConnectionConfig,
        database: Option<&str>,
    ) -> Result<MySqlPool, String> {
        let key = Self::pool_key(config, database);

        {
            let pools = self.pools.read().await;
            if let Some(PoolEntry::MySql(pool)) = pools.get(&key) {
                return Ok(pool.clone());
            }
        }

        let mut pools = self.pools.write().await;
        if let Some(PoolEntry::MySql(pool)) = pools.get(&key) {
            return Ok(pool.clone());
        }

        // Use structured options, like test_connection, rather than a URL with
        // unescaped usernames/database names (or an unbracketed IPv6 address).
        let pool = MySqlPoolOptions::new()
            .max_connections(5)
            .min_connections(1)
            .idle_timeout(std::time::Duration::from_secs(300))
            .connect_with(Self::mysql_options(config, database))
            .await
            .map_err(|e| e.to_string())?;

        pools.insert(key, PoolEntry::MySql(pool.clone()));
        Ok(pool)
    }

    /// Get or create a PostgreSQL connection pool
    pub async fn get_pg_pool(
        &self,
        config: &ConnectionConfig,
        database: Option<&str>,
    ) -> Result<PgPool, String> {
        let key = Self::pool_key(config, database);

        {
            let pools = self.pools.read().await;
            if let Some(PoolEntry::Postgres(pool)) = pools.get(&key) {
                return Ok(pool.clone());
            }
        }

        let mut pools = self.pools.write().await;
        if let Some(PoolEntry::Postgres(pool)) = pools.get(&key) {
            return Ok(pool.clone());
        }

        let pool = PgPoolOptions::new()
            .max_connections(5)
            .min_connections(1)
            .idle_timeout(std::time::Duration::from_secs(300))
            .connect_with(Self::pg_options(config, database))
            .await
            .map_err(|e| e.to_string())?;

        pools.insert(key, PoolEntry::Postgres(pool.clone()));
        Ok(pool)
    }

    /// Get or create a Redis multiplexed connection
    pub async fn get_redis_conn(
        &self,
        config: &ConnectionConfig,
    ) -> Result<redis::aio::MultiplexedConnection, String> {
        let key = Self::pool_key(config, None);

        {
            let pools = self.pools.read().await;
            if let Some(PoolEntry::Redis(conn)) = pools.get(&key) {
                return Ok(conn.clone());
            }
        }

        let mut pools = self.pools.write().await;
        if let Some(PoolEntry::Redis(conn)) = pools.get(&key) {
            return Ok(conn.clone());
        }

        let url = if let Some(pass) = &config.password {
            if !pass.is_empty() {
                format!(
                    "redis://:{}@{}:{}/",
                    urlencoding::encode(pass),
                    config.host,
                    config.port
                )
            } else {
                format!("redis://{}:{}/", config.host, config.port)
            }
        } else {
            format!("redis://{}:{}/", config.host, config.port)
        };

        let client = redis::Client::open(url).map_err(|e| e.to_string())?;
        let conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| e.to_string())?;

        pools.insert(key, PoolEntry::Redis(conn.clone()));
        Ok(conn)
    }

    /// Remove this saved connection's pools, including previous configurations.
    pub async fn remove_pool(&self, config: &ConnectionConfig) {
        let mut pools = self.pools.write().await;
        pools.retain(|key, _| key.connection_id != config.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ConnectionConfig {
        ConnectionConfig {
            id: "connection-a".into(),
            name: "Test connection".into(),
            db_type: "mysql".into(),
            host: "localhost".into(),
            port: 3306,
            username: Some("alice".into()),
            password: Some("first-password".into()),
            database: Some("app".into()),
        }
    }

    #[test]
    fn cache_key_separates_logins_and_credential_changes() {
        let original = config();
        let key = PoolManager::pool_key(&original, None);
        let mut changed = original.clone();
        changed.username = Some("bob".into());
        assert!(key != PoolManager::pool_key(&changed, None));
        changed = original.clone();
        changed.password = Some("new-password".into());
        assert!(key != PoolManager::pool_key(&changed, None));
        changed = original.clone();
        changed.id = "connection-b".into();
        assert!(key != PoolManager::pool_key(&changed, None));
    }

    #[test]
    fn cache_key_uses_effective_database_but_not_display_name() {
        let original = config();
        let mut renamed = original.clone();
        renamed.name = "Renamed".into();
        assert!(PoolManager::pool_key(&original, None) == PoolManager::pool_key(&renamed, Some("app")));
        assert!(PoolManager::pool_key(&original, None) != PoolManager::pool_key(&original, Some("other")));
    }

    #[test]
    fn connection_options_preserve_reserved_characters_and_ipv6() {
        let mut special = config();
        special.host = "::1".into();
        special.username = Some("user@company:team".into());
        special.password = Some("p@ss:/?#%".into());
        special.database = Some("app/name?sslmode=disable".into());
        let mysql = PoolManager::mysql_options(&special, None);
        assert_eq!(mysql.get_host(), "::1");
        assert_eq!(mysql.get_username(), "user@company:team");
        assert_eq!(mysql.get_database(), special.database.as_deref());
        let pg = PoolManager::pg_options(&special, Some("override/name"));
        assert_eq!(pg.get_host(), "::1");
        assert_eq!(pg.get_username(), "user@company:team");
        assert_eq!(pg.get_database(), Some("override/name"));
    }

    #[tokio::test]
    async fn removing_connection_preserves_other_ids_and_similar_ports() {
        let manager = PoolManager::new();
        let original = config();
        let mut other = original.clone();
        other.id = "connection-b".into();
        let mut similar_port = original.clone();
        similar_port.id = "connection-c".into();
        similar_port.port = 33060;
        {
            let mut pools = manager.pools.write().await;
            for cfg in [&original, &other, &similar_port] {
                // Lazy, zero-minimum pools need no database service for this test.
                let pool = MySqlPoolOptions::new()
                    .min_connections(0)
                    .connect_lazy("mysql://localhost/app")
                    .unwrap();
                pools.insert(PoolManager::pool_key(cfg, None), PoolEntry::MySql(pool));
            }
            let pool = MySqlPoolOptions::new()
                .min_connections(0)
                .connect_lazy("mysql://localhost/other")
                .unwrap();
            pools.insert(PoolManager::pool_key(&original, Some("other")), PoolEntry::MySql(pool));
        }
        let mut edited = original.clone();
        edited.host = "new-host".into();
        edited.password = Some("updated".into());
        manager.remove_pool(&edited).await;
        let pools = manager.pools.read().await;
        assert_eq!(pools.len(), 2);
        assert!(pools.contains_key(&PoolManager::pool_key(&other, None)));
        assert!(pools.contains_key(&PoolManager::pool_key(&similar_port, None)));
    }
}
