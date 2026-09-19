use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::mysql::MySqlConnectOptions;
use sqlx::mysql::MySqlRow;
use sqlx::postgres::PgConnectOptions;
use sqlx::postgres::PgRow;
use sqlx::raw_sql;
use sqlx::ConnectOptions;
use std::fs;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use serde_json::Value;
use sqlx::{Column, Row, TypeInfo};
use std::collections::HashMap;
use tauri::{Emitter, Manager};
use tokio::sync::RwLock;

mod ai_service;
mod config_store;
mod native_backup;
mod pool_manager;
mod redis_support;
mod schema;
use pool_manager::PoolManager;

#[derive(Serialize, Deserialize, Clone)]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    pub db_type: String, // "mysql", "postgresql", "redis"
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TableInfo {
    pub name: String,
    pub data_size: Option<i64>,  // bytes
    pub index_size: Option<i64>, // bytes
    pub total_size: Option<i64>, // bytes
    pub row_count: Option<i64>,  // rows
    pub comment: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
struct DatabaseExportProgress {
    task_id: String,
    database: String,
    progress: u8,
    status: String,
    stage: String,
    table_name: Option<String>,
    processed_tables: usize,
    total_tables: usize,
    processed_rows: usize,
    table_rows: usize,
    error: Option<String>,
}

struct ExportTaskManager {
    tasks: RwLock<HashMap<String, Arc<AtomicBool>>>,
}

impl ExportTaskManager {
    fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
        }
    }

    async fn start_task(&self, task_id: &str) -> Result<Arc<AtomicBool>, String> {
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let mut tasks = self.tasks.write().await;
        if tasks.contains_key(task_id) {
            return Err("Export task already exists".into());
        }
        tasks.insert(task_id.to_string(), cancel_flag.clone());
        Ok(cancel_flag)
    }

    async fn cancel_task(&self, task_id: &str) -> bool {
        let tasks = self.tasks.read().await;
        if let Some(flag) = tasks.get(task_id) {
            flag.store(true, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    async fn remove_task(&self, task_id: &str) {
        let mut tasks = self.tasks.write().await;
        tasks.remove(task_id);
    }
}

fn emit_export_progress(app_handle: &tauri::AppHandle, payload: DatabaseExportProgress) {
    let _ = app_handle.emit("database-export-progress", payload);
}

fn resolve_target_database(
    config: &ConnectionConfig,
    database: Option<&str>,
) -> Result<String, String> {
    let db = database
        .or(config.database.as_deref())
        .unwrap_or("")
        .to_string();
    if db.trim().is_empty() || db.contains('\0') {
        Err("No database selected".to_string())
    } else {
        Ok(db)
    }
}

fn quote_mysql_identifier(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

fn quote_pg_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn exact_integer(value: i128) -> Value {
    const MAX_SAFE: i128 = 9_007_199_254_740_991;
    if (-MAX_SAFE..=MAX_SAFE).contains(&value) {
        json!(value as i64)
    } else {
        Value::String(value.to_string())
    }
}

fn finite_float(value: f64) -> Value {
    if value.is_finite() {
        json!(value)
    } else {
        Value::String(value.to_string())
    }
}

fn hex_bytes(value: Vec<u8>) -> Value {
    // Never truncate stored values. Preview truncation belongs in the renderer.
    Value::String(format!(
        "0x{}",
        value
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<String>()
    ))
}

fn mysql_row_to_json_map(row: &MySqlRow) -> Result<HashMap<String, Value>, String> {
    let mut map = HashMap::new();
    for col in row.columns() {
        let name = col.name();
        let kind = col.type_info().name();
        if map.contains_key(name) {
            return Err(format!(
                "Duplicate result column '{}'; use distinct SQL aliases",
                name
            ));
        }
        macro_rules! get { ($ty:ty) => { row.try_get::<Option<$ty>, _>(col.ordinal())
            .map_err(|_| format!("Cannot decode column '{}' ({}) without data loss; cast it explicitly in SQL", name, kind))? }; }
        let value = match kind {
            "BOOLEAN" | "BOOL" => get!(i64)
                .map(|v| exact_integer(v as i128))
                .unwrap_or(Value::Null),
            kind if ["TINYINT", "SMALLINT", "MEDIUMINT", "INT", "BIGINT"]
                .iter()
                .any(|prefix| kind.starts_with(prefix)) =>
            {
                if kind.contains("UNSIGNED") {
                    get!(u64)
                        .map(|v| exact_integer(v as i128))
                        .unwrap_or(Value::Null)
                } else {
                    get!(i64)
                        .map(|v| exact_integer(v as i128))
                        .unwrap_or(Value::Null)
                }
            }
            "DECIMAL" | "NEWDECIMAL" | "NUMERIC" => {
                json!(get!(sqlx::types::BigDecimal).map(|v| v.to_string()))
            }
            "FLOAT" => get!(f32)
                .map(|v| finite_float(v as f64))
                .unwrap_or(Value::Null),
            "DOUBLE" | "REAL" => get!(f64).map(finite_float).unwrap_or(Value::Null),
            "JSON" => json!(get!(Value).map(|v| v.to_string())),
            "TIMESTAMP" | "DATETIME" => json!(get!(chrono::NaiveDateTime).map(|v| v.to_string())),
            "DATE" => json!(get!(chrono::NaiveDate).map(|v| v.to_string())),
            "TIME" => json!(get!(chrono::NaiveTime).map(|v| v.to_string())),
            "YEAR" => json!(get!(u16)),
            "BIT" => get!(u64)
                .map(|v| exact_integer(v as i128))
                .unwrap_or(Value::Null),
            kind if kind.contains("BINARY") || kind.contains("BLOB") => {
                get!(Vec<u8>).map(hex_bytes).unwrap_or(Value::Null)
            }
            _ => json!(get!(String)),
        };
        map.insert(name.to_string(), value);
    }
    Ok(map)
}

fn pg_row_to_json_map(row: &PgRow) -> Result<HashMap<String, Value>, String> {
    let mut map = HashMap::new();
    for col in row.columns() {
        let name = col.name();
        let kind = col.type_info().name();
        if map.contains_key(name) {
            return Err(format!(
                "Duplicate result column '{}'; use distinct SQL aliases",
                name
            ));
        }
        macro_rules! get { ($ty:ty) => { row.try_get::<Option<$ty>, _>(col.ordinal())
            .map_err(|_| format!("Cannot decode column '{}' ({}) without data loss; cast it explicitly in SQL", name, kind))? }; }
        let value = match kind {
            "BOOL" => json!(get!(bool)),
            "INT2" => get!(i16)
                .map(|v| exact_integer(v as i128))
                .unwrap_or(Value::Null),
            "INT4" => get!(i32)
                .map(|v| exact_integer(v as i128))
                .unwrap_or(Value::Null),
            "INT8" => get!(i64)
                .map(|v| exact_integer(v as i128))
                .unwrap_or(Value::Null),
            "FLOAT4" => get!(f32)
                .map(|v| finite_float(v as f64))
                .unwrap_or(Value::Null),
            "FLOAT8" => get!(f64).map(finite_float).unwrap_or(Value::Null),
            "NUMERIC" => json!(get!(sqlx::types::BigDecimal).map(|v| v.to_string())),
            "TIMESTAMP" => json!(get!(chrono::NaiveDateTime).map(|v| v.to_string())),
            "TIMESTAMPTZ" => json!(get!(chrono::DateTime<chrono::Utc>).map(|v| v.to_rfc3339())),
            "DATE" => json!(get!(chrono::NaiveDate).map(|v| v.to_string())),
            "TIME" => json!(get!(chrono::NaiveTime).map(|v| v.to_string())),
            "JSON" | "JSONB" => json!(get!(Value).map(|v| v.to_string())),
            "UUID" => json!(get!(uuid::Uuid).map(|v| v.to_string())),
            "BYTEA" => get!(Vec<u8>).map(hex_bytes).unwrap_or(Value::Null),
            _ => json!(get!(String)),
        };
        map.insert(name.to_string(), value);
    }
    Ok(map)
}

async fn execute_query_inner(
    pool_manager: &PoolManager,
    config: &ConnectionConfig,
    query: &str,
) -> Result<Vec<HashMap<String, Value>>, String> {
    match config.db_type.as_str() {
        "mysql" => {
            let pool = pool_manager
                .get_mysql_pool(config, config.database.as_deref())
                .await?;

            let mut connection = pool.acquire().await.map_err(|e| e.to_string())?.detach();
            let mut rows = sqlx::query(query).fetch(&mut connection);
            let mut results = Vec::new();
            let mut bytes = 0usize;
            while let Some(row) = rows.try_next().await.map_err(|e| e.to_string())? {
                let decoded = mysql_row_to_json_map(&row)?;
                bytes = bytes.saturating_add(
                    serde_json::to_vec(&decoded)
                        .map_err(|e| e.to_string())?
                        .len(),
                );
                if results.len() >= 10_000 || bytes > 32 * 1024 * 1024 {
                    return Err("Result exceeds 10,000 rows or 32 MiB. Use LIMIT/pagination or native database export; no truncated result is returned".into());
                }
                results.push(decoded);
            }
            Ok(results)
        }
        "postgresql" => {
            let pool = pool_manager
                .get_pg_pool(config, config.database.as_deref())
                .await?;

            let mut connection = pool.acquire().await.map_err(|e| e.to_string())?.detach();
            let mut rows = sqlx::query(query).fetch(&mut connection);
            let mut results = Vec::new();
            let mut bytes = 0usize;
            while let Some(row) = rows.try_next().await.map_err(|e| e.to_string())? {
                let decoded = pg_row_to_json_map(&row)?;
                bytes = bytes.saturating_add(
                    serde_json::to_vec(&decoded)
                        .map_err(|e| e.to_string())?
                        .len(),
                );
                if results.len() >= 10_000 || bytes > 32 * 1024 * 1024 {
                    return Err("Result exceeds 10,000 rows or 32 MiB. Use LIMIT/pagination or native database export; no truncated result is returned".into());
                }
                results.push(decoded);
            }
            Ok(results)
        }
        "redis" => {
            if query.len() > 1024 * 1024 {
                return Err("Redis command batch exceeds 1 MiB".into());
            }
            for line in query
                .lines()
                .map(str::trim)
                .filter(|v| !v.is_empty() && !v.starts_with('#') && !v.starts_with("--"))
            {
                redis_support::command_args(line)?;
            }
            let mut con = pool_manager.get_redis_conn(config).await?;

            let mut results = Vec::new();

            fn redis_value_to_string(v: redis::Value) -> String {
                match v {
                    redis::Value::Nil => "(nil)".to_string(),
                    redis::Value::Okay => "OK".to_string(),
                    _ => {
                        let s: redis::RedisResult<String> =
                            redis::FromRedisValue::from_redis_value(&v);
                        s.unwrap_or_else(|_| format!("{:?}", v))
                    }
                }
            }

            for line in query.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with("#") || trimmed.starts_with("--") {
                    continue;
                }

                let args = redis_support::command_args(trimmed)?;

                if args.is_empty() {
                    continue;
                }

                let cmd_name = &args[0];
                let mut cmd = redis::cmd(cmd_name);

                for arg in args.iter().skip(1) {
                    cmd.arg(arg);
                }

                let result_val: Result<redis::Value, _> = cmd.query_async(&mut con).await;
                let mut map = HashMap::new();
                match result_val {
                    Ok(val) => {
                        map.insert("result".to_string(), json!(redis_value_to_string(val)));
                    }
                    Err(e) => {
                        map.insert("error".to_string(), json!(e.to_string()));
                    }
                }
                results.push(map);
            }

            Ok(results)
        }
        _ => Err("Unsupported database type".to_string()),
    }
}

#[tauri::command]
async fn test_connection(config: ConnectionConfig) -> Result<String, String> {
    match config.db_type.as_str() {
        "mysql" => {
            let mut opts = MySqlConnectOptions::new()
                .host(&config.host)
                .port(config.port);

            if let Some(user) = &config.username {
                opts = opts.username(user);
            }
            if let Some(pass) = &config.password {
                opts = opts.password(pass);
            }
            if let Some(db) = &config.database {
                if !db.is_empty() {
                    opts = opts.database(db);
                }
            }

            let mut conn = opts.connect().await.map_err(|e| {
                let err_msg = e.to_string();
                if err_msg.contains("Access denied") || err_msg.contains("1045") {
                    return format!("连接失败: 用户名或密码错误 (Access denied)");
                }
                if err_msg.contains("Unknown database") || err_msg.contains("1049") {
                    return format!("连接失败: 数据库不存在");
                }
                if err_msg.contains("Connection refused") {
                    return format!("连接失败: 无法连接到服务器，请检查主机和端口");
                }
                format!("连接失败: {}", err_msg)
            })?;
            // Simple query to verify connection
            let _ = sqlx::query("SELECT 1")
                .fetch_one(&mut conn)
                .await
                .map_err(|e| e.to_string())?;
            Ok("MySQL 连接成功!".to_string())
        }
        "postgresql" => {
            let mut opts = PgConnectOptions::new().host(&config.host).port(config.port);

            if let Some(user) = &config.username {
                opts = opts.username(user);
            }
            if let Some(pass) = &config.password {
                opts = opts.password(pass);
            }
            if let Some(db) = &config.database {
                if !db.is_empty() {
                    opts = opts.database(db);
                }
            }

            let mut conn = opts.connect().await.map_err(|e| {
                let err_msg = e.to_string();
                if err_msg.contains("password authentication failed") || err_msg.contains("28P01") {
                    return format!("连接失败: 用户名或密码错误");
                }
                if err_msg.contains("database") && err_msg.contains("does not exist") {
                    return format!("连接失败: 数据库不存在");
                }
                if err_msg.contains("Connection refused") {
                    return format!("连接失败: 无法连接到服务器，请检查主机和端口");
                }
                format!("连接失败: {}", err_msg)
            })?;
            let _ = sqlx::query("SELECT 1")
                .fetch_one(&mut conn)
                .await
                .map_err(|e| e.to_string())?;
            Ok("PostgreSQL 连接成功!".to_string())
        }
        "redis" => {
            let mut con = redis_support::connect(&config).await?;
            let _: String = redis::cmd("PING")
                .query_async(&mut con)
                .await
                .map_err(|e| e.to_string())?;
            Ok("Redis Connection Successful!".to_string())
        }
        _ => Err("Unsupported database type".to_string()),
    }
}

fn get_config_path(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    Ok(config_dir.join("connections.json"))
}

#[tauri::command]
fn save_connection(app_handle: tauri::AppHandle, config: ConnectionConfig) -> Result<(), String> {
    let path = get_config_path(&app_handle)?;
    config_store::update::<Vec<ConnectionConfig>, _>(&path, |connections| {
        if let Some(existing) = connections.iter_mut().find(|c| c.id == config.id) {
            *existing = config;
        } else {
            connections.push(config);
        }
        Ok(())
    })
}

#[tauri::command]
fn get_connections(app_handle: tauri::AppHandle) -> Result<Vec<ConnectionConfig>, String> {
    config_store::read(&get_config_path(&app_handle)?)
}

#[tauri::command]
async fn delete_connection(
    pool_manager: tauri::State<'_, PoolManager>,
    app_handle: tauri::AppHandle,
    id: String,
) -> Result<(), String> {
    let removed = config_store::update::<Vec<ConnectionConfig>, _>(
        &get_config_path(&app_handle)?,
        |connections| {
            let removed = connections.iter().find(|c| c.id == id).cloned();
            connections.retain(|c| c.id != id);
            Ok(removed)
        },
    )?;
    if let Some(config) = removed {
        pool_manager.remove_pool(&config).await;
    }
    Ok(())
}

#[tauri::command]
async fn get_databases(
    pool_manager: tauri::State<'_, PoolManager>,
    config: ConnectionConfig,
) -> Result<Vec<String>, String> {
    match config.db_type.as_str() {
        "mysql" => {
            let pool = pool_manager.get_mysql_pool(&config, None).await?;
            let dbs: Vec<String> = sqlx::query_scalar("SHOW DATABASES")
                .fetch_all(&pool)
                .await
                .map_err(|e| e.to_string())?;
            Ok(dbs)
        }
        "postgresql" => {
            let pool = pool_manager.get_pg_pool(&config, None).await?;
            let dbs: Vec<String> =
                sqlx::query_scalar("SELECT datname FROM pg_database WHERE datistemplate = false")
                    .fetch_all(&pool)
                    .await
                    .map_err(|e| e.to_string())?;
            Ok(dbs)
        }
        "redis" => {
            // Redis has 16 databases by default (0-15)
            // Query each one for key count using DBSIZE
            let mut con = pool_manager.get_redis_conn(&config).await?;

            let configured: redis::RedisResult<Vec<String>> = redis::cmd("CONFIG")
                .arg("GET")
                .arg("databases")
                .query_async(&mut con)
                .await;
            let count = configured
                .ok()
                .and_then(|v| v.get(1)?.parse::<usize>().ok())
                .unwrap_or(16)
                .min(1024);
            let mut dbs = Vec::new();
            for i in 0..count {
                // Select db
                let selected: redis::RedisResult<()> =
                    redis::cmd("SELECT").arg(i).query_async(&mut con).await;
                if let Err(error) = selected {
                    if i > 0
                        && (error.to_string().contains("out of range")
                            || error.to_string().contains("cluster mode"))
                    {
                        break;
                    }
                    return Err(error.to_string());
                }
                // Get key count
                let count: i64 = redis::cmd("DBSIZE")
                    .query_async(&mut con)
                    .await
                    .map_err(|e| e.to_string())?;
                dbs.push(format!("db{} ({})", i, count));
            }
            Ok(dbs)
        }
        _ => Err("Unsupported database type for databases".to_string()),
    }
}

#[tauri::command]
async fn get_tables(
    pool_manager: tauri::State<'_, PoolManager>,
    config: ConnectionConfig,
    database: Option<String>,
) -> Result<Vec<TableInfo>, String> {
    match config.db_type.as_str() {
        "mysql" => {
            // Use provided database or config default
            let target_db = database.as_deref().or(config.database.as_deref());
            let db_name = target_db.unwrap_or("").to_string();

            let pool = pool_manager.get_mysql_pool(&config, target_db).await?;

            // Handle current_db safely
            let current_db: String = if !db_name.is_empty() {
                db_name
            } else {
                let row: Option<String> = sqlx::query_scalar("SELECT DATABASE()")
                    .fetch_one(&pool)
                    .await
                    .unwrap_or(None);
                row.unwrap_or_default()
            };

            let query = "
                SELECT 
                    TABLE_NAME, 
                    DATA_LENGTH, 
                    INDEX_LENGTH, 
                    TABLE_ROWS,
                    TABLE_COMMENT 
                FROM information_schema.TABLES 
                WHERE TABLE_SCHEMA = ?
            ";

            let rows = sqlx::query(query)
                .bind(&current_db)
                .fetch_all(&pool)
                .await
                .map_err(|e| format!("Failed to fetch tables: {}", e))?;

            let mut tables = Vec::new();
            for row in rows {
                let name: String = row.try_get("TABLE_NAME").unwrap_or_default();
                let data_len: Option<u64> = row.try_get("DATA_LENGTH").ok();
                let index_len: Option<u64> = row.try_get("INDEX_LENGTH").ok();
                let table_rows: Option<u64> = row.try_get("TABLE_ROWS").ok();
                let comment: Option<String> = row.try_get("TABLE_COMMENT").ok();

                let d_size = data_len.map(|v| v as i64);
                let i_size = index_len.map(|v| v as i64);
                let rows_count = table_rows.map(|v| v as i64);

                tables.push(TableInfo {
                    name,
                    data_size: d_size,
                    index_size: i_size,
                    total_size: Some(d_size.unwrap_or(0) + i_size.unwrap_or(0)),
                    row_count: rows_count,
                    comment,
                });
            }
            Ok(tables)
        }
        "postgresql" => {
            let target_db = database.as_deref().or(config.database.as_deref());
            let pool = pool_manager.get_pg_pool(&config, target_db).await?;

            let query = "
                SELECT 
                    c.relname as table_name,
                    pg_relation_size(c.oid) as data_size,
                    pg_indexes_size(c.oid) as index_size,
                    pg_total_relation_size(c.oid) as total_size,
                    CAST(c.reltuples AS BIGINT) as row_count,
                    obj_description(c.oid, 'pg_class') as comment
                FROM pg_class c
                JOIN pg_namespace n ON n.oid = c.relnamespace
                WHERE n.nspname = 'public' AND c.relkind = 'r'
            ";

            let rows: Vec<(
                String,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<String>,
            )> = sqlx::query_as(query)
                .fetch_all(&pool)
                .await
                .map_err(|e| e.to_string())?;

            let tables = rows
                .into_iter()
                .map(|(name, data, index, total, rows, comment)| TableInfo {
                    name,
                    data_size: data,
                    index_size: index,
                    total_size: total,
                    row_count: rows,
                    comment,
                })
                .collect();
            Ok(tables)
        }
        "redis" => {
            let effective = ConnectionConfig {
                database: database.or(config.database.clone()),
                ..config
            };
            let mut con = pool_manager.get_redis_conn(&effective).await?;
            // Bounded, incremental discovery; never issue blocking KEYS *.
            let mut cursor = 0u64;
            let mut seen = std::collections::HashSet::new();
            let mut keys = Vec::new();
            loop {
                let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("COUNT")
                    .arg(200)
                    .query_async(&mut con)
                    .await
                    .map_err(|e| e.to_string())?;
                for key in batch {
                    if seen.insert(key.clone()) {
                        keys.push(key);
                    }
                    if keys.len() >= 1000 {
                        break;
                    }
                }
                cursor = next;
                if cursor == 0 || keys.len() >= 1000 {
                    break;
                }
            }
            keys.sort();

            let tables = keys
                .into_iter()
                .map(|k| TableInfo {
                    name: k,
                    data_size: None,
                    index_size: None,
                    total_size: None,
                    row_count: None,
                    comment: None,
                })
                .collect();

            Ok(tables)
        }
        _ => Err("Unsupported database type for tables".to_string()),
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ColumnDef {
    pub name: String,
    pub type_name: String,
    pub is_pk: bool,
    pub is_nullable: Option<bool>,
    pub default_value: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IndexDef {
    pub name: String,
    pub columns: Vec<String>,
    pub is_unique: bool,
    pub is_pk: bool,
    pub comment: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AlterOperation {
    pub op_type: String, // "add", "modify", "drop", "rename", "add_index", "drop_index"
    pub column_name: Option<String>, // Optional now
    pub new_name: Option<String>,
    pub column_def: Option<ColumnDef>,
    pub index_def: Option<IndexDef>, // For add_index
    pub index_name: Option<String>,  // For drop_index
}

#[tauri::command]
async fn get_columns(
    pool_manager: tauri::State<'_, PoolManager>,
    config: ConnectionConfig,
    table: String,
    database: Option<String>,
) -> Result<Vec<ColumnDef>, String> {
    match config.db_type.as_str() {
        "mysql" => {
            let target_db = database.clone().or(config.database.clone());
            let pool = pool_manager
                .get_mysql_pool(&config, target_db.as_deref())
                .await?;

            let db_name = target_db.unwrap_or_else(|| "".to_string());

            let query = if !db_name.is_empty() {
                "SELECT COLUMN_NAME, COLUMN_TYPE, COLUMN_KEY, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_COMMENT 
                  FROM information_schema.COLUMNS 
                  WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
                  ORDER BY ORDINAL_POSITION"
            } else {
                "SELECT COLUMN_NAME, COLUMN_TYPE, COLUMN_KEY, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_COMMENT 
                  FROM information_schema.COLUMNS 
                  WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?
                  ORDER BY ORDINAL_POSITION"
            };

            let q = sqlx::query_as::<
                _,
                (
                    Option<Vec<u8>>,
                    Option<Vec<u8>>,
                    Option<Vec<u8>>,
                    Option<Vec<u8>>,
                    Option<Vec<u8>>,
                    Option<Vec<u8>>,
                ),
            >(query);
            let q = if !db_name.is_empty() {
                q.bind(db_name).bind(&table)
            } else {
                q.bind(&table)
            };

            let rows = q.fetch_all(&pool).await.map_err(|e| {
                println!("Error fetching columns: {}", e);
                e.to_string()
            })?;

            let mut result = Vec::new();
            for (name_bytes, dtype_bytes, key_bytes, null_bytes, default_bytes, comment_bytes) in
                rows
            {
                let name = name_bytes
                    .map(|b| String::from_utf8_lossy(&b).to_string())
                    .unwrap_or_default();
                let dtype = dtype_bytes
                    .map(|b| String::from_utf8_lossy(&b).to_string())
                    .unwrap_or_default();
                let key_str = key_bytes
                    .map(|b| String::from_utf8_lossy(&b).to_string())
                    .unwrap_or_default();
                let null_str = null_bytes
                    .map(|b| String::from_utf8_lossy(&b).to_string())
                    .unwrap_or_default();

                let def_val = default_bytes.map(|b| String::from_utf8_lossy(&b).to_string());
                let comment = comment_bytes.map(|b| String::from_utf8_lossy(&b).to_string());

                result.push(ColumnDef {
                    name,
                    type_name: dtype,
                    is_pk: key_str == "PRI",
                    is_nullable: Some(null_str == "YES"),
                    default_value: def_val,
                    comment,
                });
            }
            Ok(result)
        }
        "postgresql" => {
            let target_db = database.as_deref().or(config.database.as_deref());
            let pool = pool_manager.get_pg_pool(&config, target_db).await?;

            let query = "
                SELECT a.attname::text, pg_catalog.format_type(a.atttypid, a.atttypmod),
                    EXISTS (SELECT 1 FROM pg_index i WHERE i.indrelid = c.oid AND i.indisprimary AND a.attnum = ANY(i.indkey)),
                    CASE WHEN a.attnotnull THEN 'NO' ELSE 'YES' END,
                    pg_get_expr(d.adbin, d.adrelid), pg_catalog.col_description(c.oid, a.attnum)
                FROM pg_attribute a
                JOIN pg_class c ON a.attrelid = c.oid
                JOIN pg_namespace n ON n.oid = c.relnamespace
                LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
                WHERE n.nspname = 'public' AND c.relname = $1 AND a.attnum > 0 AND NOT a.attisdropped
                ORDER BY a.attnum
            ";
            let rows: Vec<(
                String,
                String,
                Option<bool>,
                Option<String>,
                Option<String>,
                Option<String>,
            )> = sqlx::query_as(query)
                .bind(&table)
                .fetch_all(&pool)
                .await
                .map_err(|e| e.to_string())?;

            let mut result = Vec::new();
            for (name, dtype, is_pk, is_null, def, comment) in rows {
                result.push(ColumnDef {
                    name,
                    type_name: dtype,
                    is_pk: is_pk.unwrap_or(false),
                    is_nullable: Some(is_null.unwrap_or("YES".to_string()) == "YES"),
                    default_value: def,
                    comment,
                });
            }
            Ok(result)
        }
        "redis" => {
            let effective = ConnectionConfig {
                database: database.or(config.database.clone()),
                ..config
            };
            let mut con = pool_manager.get_redis_conn(&effective).await?;

            // Get key type
            let key_type: String = redis::cmd("TYPE")
                .arg(&table)
                .query_async(&mut con)
                .await
                .map_err(|e| e.to_string())?;

            // Return a single "column" representing the key type
            Ok(vec![ColumnDef {
                name: "value".to_string(),
                type_name: key_type,
                is_pk: false,
                is_nullable: Some(false),
                default_value: None,
                comment: Some(format!("Redis key: {}", table)),
            }])
        }
        _ => Err("Unsupported database type".to_string()),
    }
}

#[tauri::command]
async fn get_indexes(
    pool_manager: tauri::State<'_, PoolManager>,
    config: ConnectionConfig,
    table: String,
) -> Result<Vec<IndexDef>, String> {
    match config.db_type.as_str() {
        "mysql" => {
            let pool = pool_manager
                .get_mysql_pool(&config, config.database.as_deref())
                .await?;

            let rows: Vec<(Option<Vec<u8>>, Option<Vec<u8>>, i32, Option<Vec<u8>>)> =
                sqlx::query_as(
                    "
                SELECT INDEX_NAME, COLUMN_NAME, NON_UNIQUE, INDEX_COMMENT 
                FROM information_schema.STATISTICS 
                WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?
                ORDER BY INDEX_NAME, SEQ_IN_INDEX
            ",
                )
                .bind(&table)
                .fetch_all(&pool)
                .await
                .map_err(|e| e.to_string())?;

            // Group by index name
            let mut indexes: Vec<IndexDef> = Vec::new();
            for (idx_name_bytes, col_name_bytes, non_unique, comment_bytes) in rows {
                let idx_name = idx_name_bytes
                    .map(|b| String::from_utf8_lossy(&b).to_string())
                    .unwrap_or_default();
                let col_name = col_name_bytes
                    .map(|b| String::from_utf8_lossy(&b).to_string())
                    .unwrap_or_default();
                let comment = comment_bytes
                    .map(|b| String::from_utf8_lossy(&b).to_string())
                    .unwrap_or_default();

                if let Some(last) = indexes.last_mut() {
                    if last.name == idx_name {
                        last.columns.push(col_name);
                        continue;
                    }
                }
                indexes.push(IndexDef {
                    name: idx_name.clone(),
                    columns: vec![col_name],
                    is_unique: non_unique == 0,
                    is_pk: idx_name == "PRIMARY",
                    comment: if comment.is_empty() {
                        None
                    } else {
                        Some(comment)
                    },
                });
            }
            Ok(indexes)
        }
        "postgresql" => {
            let pool = pool_manager
                .get_pg_pool(&config, config.database.as_deref())
                .await?;

            let rows: Vec<(String, String, bool)> = sqlx::query_as(
                "
                select
                    i.relname as index_name,
                    array_to_string(array_agg(a.attname), ',') as column_names,
                    ix.indisunique as is_unique
                from
                    pg_class t,
                    pg_class i,
                    pg_index ix,
                    pg_attribute a
                where
                    t.oid = ix.indrelid
                    and i.oid = ix.indexrelid
                    and a.attrelid = t.oid
                    and a.attnum = ANY(ix.indkey)
                    and t.relkind = 'r'
                    and t.relname = $1
                group by
                    t.relname,
                    i.relname,
                    ix.indisunique
            ",
            )
            .bind(&table)
            .fetch_all(&pool)
            .await
            .map_err(|e| e.to_string())?;

            let mut indexes = Vec::new();
            for (name, cols, unique) in rows {
                indexes.push(IndexDef {
                    name: name.clone(),
                    columns: cols.split(',').map(|s| s.to_string()).collect(),
                    is_unique: unique,
                    is_pk: name.ends_with("_pkey"), // Heuristic or check indisprimary?
                    comment: None,
                });
            }
            Ok(indexes)
        }
        _ => Ok(Vec::new()),
    }
}

#[tauri::command]
async fn export_database_sql(
    app_handle: tauri::AppHandle,
    pool_manager: tauri::State<'_, PoolManager>,
    export_task_manager: tauri::State<'_, ExportTaskManager>,
    config: ConnectionConfig,
    database: Option<String>,
    task_id: String,
    output_path: String,
) -> Result<(), String> {
    let target_db = resolve_target_database(&config, database.as_deref())?;
    let cancel_flag = export_task_manager.start_task(&task_id).await?;
    let progress = |status: &str, error: Option<String>| {
        emit_export_progress(
            &app_handle,
            DatabaseExportProgress {
                task_id: task_id.clone(),
                database: target_db.clone(),
                progress: if status == "completed" { 100 } else { 0 },
                status: status.into(),
                stage: if status == "running" {
                    "native_dump".into()
                } else {
                    status.into()
                },
                table_name: None,
                processed_tables: 0,
                total_tables: 0,
                processed_rows: 0,
                table_rows: 0,
                error,
            },
        )
    };
    progress("running", None);
    let result = async {
        if config.db_type == "mysql" {
            let pool = pool_manager.get_mysql_pool(&config, Some(&target_db)).await?;
            let unsafe_tables: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = 'BASE TABLE' AND ENGINE <> 'InnoDB'")
                .bind(&target_db).fetch_one(&pool).await.map_err(|e| e.to_string())?;
            if unsafe_tables > 0 { return Err("Consistent export requires InnoDB tables. Use a DBA-managed backup for non-transactional tables.".into()); }
        }
        native_backup::export(&config, &target_db, std::path::Path::new(&output_path), &cancel_flag).await
    }.await;
    export_task_manager.remove_task(&task_id).await;
    match &result {
        Ok(()) => progress("completed", None),
        Err(error) if error == "Export cancelled" => progress("cancelled", None),
        Err(error) => progress("error", Some(error.clone())),
    }
    result
}

#[tauri::command]
async fn cancel_database_export(
    export_task_manager: tauri::State<'_, ExportTaskManager>,
    task_id: String,
) -> Result<(), String> {
    if export_task_manager.cancel_task(&task_id).await {
        Ok(())
    } else {
        Err("Export task not found".to_string())
    }
}

#[tauri::command]
async fn import_database_sql(
    pool_manager: tauri::State<'_, PoolManager>,
    config: ConnectionConfig,
    database: Option<String>,
    script: String,
) -> Result<u64, String> {
    if script.len() > 64 * 1024 * 1024 {
        return Err(
            "SQL script exceeds 64 MiB; use the native database client for large restores".into(),
        );
    }
    let target_db = resolve_target_database(&config, database.as_deref())?;

    match config.db_type.as_str() {
        "mysql" => {
            let pool = pool_manager
                .get_mysql_pool(&config, Some(&target_db))
                .await?;
            let mut connection = pool.acquire().await.map_err(|e| e.to_string())?.detach();
            let script = native_backup::sql_without_psql_wrapper(&script);
            let result = sqlx::Executor::execute(&mut connection, script.as_str())
                .await
                .map_err(|e| {
                    format!(
                        "Import failed; some statements may already be committed: {}",
                        e
                    )
                })?;
            Ok(result.rows_affected())
        }
        "postgresql" => {
            let pool = pool_manager.get_pg_pool(&config, Some(&target_db)).await?;
            let mut connection = pool.acquire().await.map_err(|e| e.to_string())?.detach();
            let script = native_backup::sql_without_psql_wrapper(&script);
            let result = sqlx::Executor::execute(&mut connection, script.as_str())
                .await
                .map_err(|e| {
                    format!(
                        "Import failed; some statements may already be committed: {}",
                        e
                    )
                })?;
            Ok(result.rows_affected())
        }
        _ => Err("Database import is only supported for MySQL and PostgreSQL".to_string()),
    }
}

// ... existing code ...

#[tauri::command]
async fn alter_table(
    pool_manager: tauri::State<'_, PoolManager>,
    config: ConnectionConfig,
    table: String,
    operation: AlterOperation,
) -> Result<(), String> {
    match config.db_type.as_str() {
        "mysql" => {
            let pool = pool_manager
                .get_mysql_pool(&config, config.database.as_deref())
                .await?;
            let mut connection = pool.acquire().await.map_err(|e| e.to_string())?.detach();
            let mode: String = sqlx::query_scalar("SELECT @@SESSION.sql_mode")
                .fetch_one(&mut connection)
                .await
                .map_err(|e| e.to_string())?;
            let mut attributes = String::new();
            if operation.op_type == "modify" {
                let col = operation
                    .column_def
                    .as_ref()
                    .ok_or("Missing column definition")?;
                let name = operation.column_name.as_deref().unwrap_or(&col.name);
                let (extra, generated, collation): (String, String, Option<String>) = sqlx::query_as("SELECT EXTRA, GENERATION_EXPRESSION, COLLATION_NAME FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME=? AND COLUMN_NAME=?")
                    .bind(&table).bind(name).fetch_one(&mut connection).await.map_err(|e| e.to_string())?;
                if !generated.is_empty()
                    || (!extra.is_empty() && !extra.eq_ignore_ascii_case("auto_increment"))
                {
                    return Err("This column has generated, automatic-update or other special attributes. Use reviewed SQL; visual editing will not silently discard them".into());
                }
                if extra.eq_ignore_ascii_case("auto_increment") {
                    attributes.push_str("AUTO_INCREMENT ");
                }
                if let Some(collation) = collation {
                    if col.type_name.to_ascii_uppercase().contains("CHAR")
                        || col.type_name.to_ascii_uppercase().contains("TEXT")
                    {
                        if !collation
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
                        {
                            return Err("Unsupported collation".into());
                        }
                        attributes.push_str(&format!("COLLATE {} ", collation));
                    }
                }
            }
            for query in schema::queries(
                &table,
                &operation,
                true,
                !mode.split(',').any(|v| v == "NO_BACKSLASH_ESCAPES"),
                &attributes,
            )? {
                sqlx::query(&query)
                    .execute(&mut connection)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        "postgresql" => {
            let queries = schema::queries(&table, &operation, false, true, "")?;
            let pool = pool_manager
                .get_pg_pool(&config, config.database.as_deref())
                .await?;
            let mut transaction = pool.begin().await.map_err(|e| e.to_string())?;
            for query in queries {
                sqlx::query(&query)
                    .execute(&mut *transaction)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            transaction.commit().await.map_err(|e| e.to_string())?;
        }
        _ => return Err("Unsupported database for schema editing".into()),
    }
    Ok(())
}

async fn import_table_rows_inner(
    pool_manager: &PoolManager,
    config: &ConnectionConfig,
    table: &str,
    queries: &[String],
) -> Result<usize, String> {
    if table.is_empty()
        || table.contains('\0')
        || queries.len() > 10_000
        || queries.iter().map(String::len).sum::<usize>() > 16 * 1024 * 1024
    {
        return Err("Invalid table or import exceeds 10,000 rows / 16 MiB; use native tools for large imports".into());
    }
    let prefix = format!(
        "INSERT INTO {} ",
        match config.db_type.as_str() {
            "mysql" => quote_mysql_identifier(table),
            "postgresql" => quote_pg_identifier(table),
            _ => return Err("Unsupported import database".into()),
        }
    );
    if queries.iter().any(|q| !q.starts_with(&prefix)) {
        return Err("Table import accepts only INSERT statements for the selected table".into());
    }
    macro_rules! insert_all {
        ($transaction:ident) => {{
            for query in queries {
                if let Err(error) = sqlx::query(query).execute(&mut *$transaction).await {
                    return match $transaction.rollback().await {
                        Ok(_) => Err(format!("Import transaction rolled back: {}", error)),
                        Err(_) => Err("Import failed and rollback could not be confirmed; inspect the database before retrying".into()),
                    };
                }
            }
            $transaction.commit().await.map_err(|_| "Import commit outcome is unknown; inspect the database before retrying".to_string())?;
        }};
    }
    match config.db_type.as_str() {
        "mysql" => {
            let pool = pool_manager
                .get_mysql_pool(config, config.database.as_deref())
                .await?;
            let engine: Option<String> = sqlx::query_scalar("SELECT ENGINE FROM information_schema.TABLES WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME=?")
                .bind(table).fetch_optional(&pool).await.map_err(|e| e.to_string())?.flatten();
            if !engine.is_some_and(|e| e.eq_ignore_ascii_case("InnoDB")) {
                return Err("Transactional table import requires an InnoDB base table".into());
            }
            let mut transaction = pool.begin().await.map_err(|e| e.to_string())?;
            insert_all!(transaction);
        }
        "postgresql" => {
            let pool = pool_manager
                .get_pg_pool(config, config.database.as_deref())
                .await?;
            let mut transaction = pool.begin().await.map_err(|e| e.to_string())?;
            insert_all!(transaction);
        }
        _ => unreachable!(),
    }
    Ok(queries.len())
}

#[tauri::command]
async fn import_table_rows(
    pool_manager: tauri::State<'_, PoolManager>,
    config: ConnectionConfig,
    table: String,
    queries: Vec<String>,
) -> Result<usize, String> {
    import_table_rows_inner(pool_manager.inner(), &config, &table, &queries).await
}

#[tauri::command]
async fn execute_query(
    pool_manager: tauri::State<'_, PoolManager>,
    config: ConnectionConfig,
    query: String,
) -> Result<Vec<HashMap<String, Value>>, String> {
    execute_query_inner(pool_manager.inner(), &config, &query).await
}

// ============ AI Commands ============

#[tauri::command]
async fn get_ai_config(app: tauri::AppHandle) -> Result<ai_service::AIConfig, String> {
    let config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let config_path = config_dir.join("ai_config.json");

    config_store::read(&config_path)
}

#[tauri::command]
async fn save_ai_config(app: tauri::AppHandle, config: ai_service::AIConfig) -> Result<(), String> {
    let config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    let config_path = config_dir.join("ai_config.json");

    config_store::save(&config_path, config)
}

#[tauri::command]
async fn generate_sql_from_text(
    app: tauri::AppHandle,
    db_type: String,
    table_schemas: String,
    user_request: String,
    consent: bool,
    expected_api_url: String,
) -> Result<String, String> {
    if !consent {
        return Err("AI transmission requires explicit consent".into());
    }
    let config = get_ai_config(app).await?;
    if config.api_url != expected_api_url {
        return Err("AI endpoint changed; reopen the AI dialog and review the destination".into());
    }

    ai_service::generate_sql(
        &config.api_key,
        &config.api_url,
        &config.model,
        &db_type,
        &table_schemas,
        &user_request,
    )
    .await
}

// ============ Redis Specific Commands ============

#[derive(Debug, Serialize, Deserialize)]
pub struct RedisKeyInfo {
    pub key: String,
    pub key_type: String,
    pub ttl: i64, // -1 = no expiry, -2 = key doesn't exist
    pub value: String,
    pub length: Option<i64>, // For lists, sets, hashes, zsets
}

#[tauri::command]
async fn get_redis_key_value(
    pool_manager: tauri::State<'_, PoolManager>,
    config: ConnectionConfig,
    key: String,
    database: Option<String>,
) -> Result<RedisKeyInfo, String> {
    let effective = ConnectionConfig {
        database: database.or(config.database.clone()),
        ..config
    };
    let mut con = pool_manager.get_redis_conn(&effective).await?;

    // Get key type
    let key_type: String = redis::cmd("TYPE")
        .arg(&key)
        .query_async(&mut con)
        .await
        .map_err(|e| e.to_string())?;

    // Get TTL
    let ttl: i64 = redis::cmd("TTL")
        .arg(&key)
        .query_async(&mut con)
        .await
        .map_err(|e| e.to_string())?;

    // Get value based on type
    let (value, length) = match key_type.as_str() {
        "string" => {
            let len: i64 = redis::cmd("STRLEN")
                .arg(&key)
                .query_async(&mut con)
                .await
                .map_err(|e| e.to_string())?;
            let v: String = redis::cmd("GETRANGE")
                .arg(&key)
                .arg(0)
                .arg(65535)
                .query_async(&mut con)
                .await
                .map_err(|e| e.to_string())?;
            (v, Some(len))
        }
        "list" => {
            let len: i64 = redis::cmd("LLEN")
                .arg(&key)
                .query_async(&mut con)
                .await
                .map_err(|e| e.to_string())?;
            let items: Vec<String> = redis::cmd("LRANGE")
                .arg(&key)
                .arg(0)
                .arg(99)
                .query_async(&mut con)
                .await
                .map_err(|e| e.to_string())?;
            (
                serde_json::to_string_pretty(&items.into_iter().take(100).collect::<Vec<_>>())
                    .map_err(|e| e.to_string())?,
                Some(len),
            )
        }
        "set" => {
            let len: i64 = redis::cmd("SCARD")
                .arg(&key)
                .query_async(&mut con)
                .await
                .map_err(|e| e.to_string())?;
            let (_, mut items): (u64, Vec<String>) = redis::cmd("SSCAN")
                .arg(&key)
                .arg(0)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut con)
                .await
                .map_err(|e| e.to_string())?;
            items.truncate(100);
            (
                serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?,
                Some(len),
            )
        }
        "zset" => {
            let len: i64 = redis::cmd("ZCARD")
                .arg(&key)
                .query_async(&mut con)
                .await
                .map_err(|e| e.to_string())?;
            let items: Vec<String> = redis::cmd("ZRANGE")
                .arg(&key)
                .arg(0)
                .arg(99)
                .arg("WITHSCORES")
                .query_async(&mut con)
                .await
                .map_err(|e| e.to_string())?;
            (
                serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?,
                Some(len),
            )
        }
        "hash" => {
            let len: i64 = redis::cmd("HLEN")
                .arg(&key)
                .query_async(&mut con)
                .await
                .map_err(|e| e.to_string())?;
            let (_, mut items): (u64, Vec<String>) = redis::cmd("HSCAN")
                .arg(&key)
                .arg(0)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut con)
                .await
                .map_err(|e| e.to_string())?;
            items.truncate(200);
            // Convert flat list to key-value pairs
            let mut map = std::collections::HashMap::new();
            let mut iter = items.iter();
            while let (Some(k), Some(v)) = (iter.next(), iter.next()) {
                map.insert(k.clone(), v.clone());
            }
            (
                serde_json::to_string_pretty(&map).map_err(|e| e.to_string())?,
                Some(len),
            )
        }
        "none" => ("(key expired or does not exist)".to_string(), None),
        _ => {
            return Err(format!(
                "Preview is not supported for Redis type: {}",
                key_type
            ))
        }
    };

    if value.len() > 2 * 1024 * 1024 {
        return Err("Redis preview exceeds 2 MiB; use the native CLI for this key".into());
    }
    Ok(RedisKeyInfo {
        key,
        key_type,
        ttl,
        value,
        length,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(PoolManager::new())
        .manage(ExportTaskManager::new())
        .invoke_handler(tauri::generate_handler![
            test_connection,
            save_connection,
            get_connections,
            delete_connection,
            get_tables,
            get_databases,
            get_columns,
            execute_query,
            export_database_sql,
            cancel_database_export,
            import_database_sql,
            import_table_rows,
            alter_table,
            get_indexes,
            get_ai_config,
            save_ai_config,
            generate_sql_from_text,
            get_redis_key_value
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod integration_tests;
