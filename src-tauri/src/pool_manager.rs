use sqlx::mysql::MySqlPool;
use sqlx::mysql::MySqlPoolOptions;
use sqlx::postgres::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::ConnectionConfig;

/// Enum to hold different types of database connection pools
pub enum PoolEntry {
    MySql(MySqlPool),
    Postgres(PgPool),
    Redis(redis::aio::MultiplexedConnection),
}

/// Manages connection pools for all active database connections
pub struct PoolManager {
    pools: RwLock<HashMap<String, PoolEntry>>,
}

impl PoolManager {
    pub fn new() -> Self {
        Self {
            pools: RwLock::new(HashMap::new()),
        }
    }

    /// Generate a unique key for a connection config
    fn pool_key(config: &ConnectionConfig, database: Option<&str>) -> String {
        let db = database.or(config.database.as_deref()).unwrap_or("");
        format!("{}:{}:{}:{}", config.db_type, config.host, config.port, db)
    }

    /// Get or create a MySQL connection pool
    pub async fn get_mysql_pool(
        &self,
        config: &ConnectionConfig,
        database: Option<&str>,
    ) -> Result<MySqlPool, String> {
        let key = Self::pool_key(config, database);

        // Try read lock first (fast path)
        {
            let pools = self.pools.read().await;
            if let Some(PoolEntry::MySql(pool)) = pools.get(&key) {
                return Ok(pool.clone());
            }
        }

        // Need to create - acquire write lock
        let mut pools = self.pools.write().await;
        // Double-check after acquiring write lock
        if let Some(PoolEntry::MySql(pool)) = pools.get(&key) {
            return Ok(pool.clone());
        }

        let db = database.or(config.database.as_deref()).unwrap_or("");
        let mut url = format!("mysql://");
        if let Some(user) = &config.username {
            url.push_str(user);
            if let Some(pass) = &config.password {
                url.push(':');
                url.push_str(&urlencoding::encode(pass));
            }
            url.push('@');
        }
        url.push_str(&format!("{}:{}", config.host, config.port));
        if !db.is_empty() {
            url.push('/');
            url.push_str(db);
        }

        let pool = MySqlPoolOptions::new()
            .max_connections(5)
            .min_connections(1)
            .idle_timeout(std::time::Duration::from_secs(300))
            .connect(&url)
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

        let db = database.or(config.database.as_deref()).unwrap_or("");
        let mut url = format!("postgres://");
        if let Some(user) = &config.username {
            url.push_str(user);
            if let Some(pass) = &config.password {
                url.push(':');
                url.push_str(&urlencoding::encode(pass));
            }
            url.push('@');
        }
        url.push_str(&format!("{}:{}", config.host, config.port));
        if !db.is_empty() {
            url.push('/');
            url.push_str(db);
        }

        let pool = PgPoolOptions::new()
            .max_connections(5)
            .min_connections(1)
            .idle_timeout(std::time::Duration::from_secs(300))
            .connect(&url)
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

    /// Remove all pools associated with a connection config
    pub async fn remove_pool(&self, config: &ConnectionConfig) {
        let prefix = format!("{}:{}:{}", config.db_type, config.host, config.port);
        let mut pools = self.pools.write().await;
        pools.retain(|key, _| !key.starts_with(&prefix));
    }
}
