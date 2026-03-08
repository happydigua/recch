use serde::{Deserialize, Serialize};
use serde_json::json;
use futures_util::TryStreamExt;
use sqlx::mysql::MySqlConnectOptions;
use sqlx::mysql::MySqlRow;
use sqlx::postgres::PgConnectOptions;
use sqlx::postgres::PgRow;
use sqlx::raw_sql;
use sqlx::ConnectOptions;
use std::fs;
use std::io::{BufWriter, Write};
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
mod pool_manager;
use pool_manager::PoolManager;

#[derive(Debug, Serialize, Deserialize, Clone)]
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

    async fn start_task(&self, task_id: &str) -> Arc<AtomicBool> {
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let mut tasks = self.tasks.write().await;
        tasks.insert(task_id.to_string(), cancel_flag.clone());
        cancel_flag
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

fn calculate_progress(processed_units: usize, total_units: usize, status: &str) -> u8 {
    if status == "completed" {
        return 100;
    }
    if total_units == 0 {
        return 0;
    }
    (((processed_units.saturating_mul(100)) / total_units).min(99)) as u8
}

fn ensure_not_cancelled(cancel_flag: &AtomicBool) -> Result<(), String> {
    if cancel_flag.load(Ordering::Relaxed) {
        Err("Export cancelled".to_string())
    } else {
        Ok(())
    }
}

fn resolve_target_database(
    config: &ConnectionConfig,
    database: Option<&str>,
) -> Result<String, String> {
    let db = database
        .or(config.database.as_deref())
        .unwrap_or("")
        .trim()
        .to_string();
    if db.is_empty() {
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

fn escape_sql_string(value: &str, db_type: &str) -> String {
    let escaped = value.replace('\'', "''");
    if db_type == "mysql" {
        escaped.replace('\\', "\\\\")
    } else {
        escaped
    }
}

fn sql_literal(value: Option<&Value>, db_type: &str) -> String {
    match value {
        None | Some(Value::Null) => "NULL".to_string(),
        Some(Value::Bool(v)) => {
            if db_type == "mysql" {
                if *v { "1".to_string() } else { "0".to_string() }
            } else if *v {
                "TRUE".to_string()
            } else {
                "FALSE".to_string()
            }
        }
        Some(Value::Number(v)) => v.to_string(),
        Some(Value::String(v)) => format!("'{}'", escape_sql_string(v, db_type)),
        Some(Value::Array(v)) => format!(
            "'{}'",
            escape_sql_string(&serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()), db_type)
        ),
        Some(Value::Object(v)) => format!(
            "'{}'",
            escape_sql_string(&serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()), db_type)
        ),
    }
}

fn normalize_pg_column_definition(
    data_type: &str,
    default_value: Option<&str>,
) -> (String, Option<String>, bool) {
    if let Some(default) = default_value {
        if default.starts_with("nextval(") {
            let normalized_type = match data_type {
                "smallint" => "smallserial".to_string(),
                "bigint" => "bigserial".to_string(),
                _ => "serial".to_string(),
            };
            return (normalized_type, None, true);
        }
    }

    (
        data_type.to_string(),
        default_value.map(|v| v.to_string()),
        false,
    )
}

fn mysql_row_to_json_map(row: &MySqlRow) -> HashMap<String, Value> {
    let mut map = HashMap::new();

    for col in row.columns() {
        let name = col.name();
        let type_name = col.type_info().name();

        let value: Value = match type_name {
            "BOOLEAN" | "BOOL" => {
                let v: Option<bool> = row.try_get(col.ordinal()).unwrap_or(None);
                json!(v)
            }
            _ if type_name.starts_with("TINYINT")
                || type_name.starts_with("SMALLINT")
                || type_name.starts_with("INT")
                || type_name.starts_with("INTEGER")
                || type_name.starts_with("BIGINT")
                || type_name.starts_with("MEDIUMINT")
                || type_name == "INT4"
                || type_name == "INT8" =>
            {
                if let Ok(v) = row.try_get::<Option<i64>, _>(col.ordinal()) {
                    json!(v)
                } else if let Ok(v) = row.try_get::<Option<u64>, _>(col.ordinal()) {
                    json!(v)
                } else if let Ok(v) = row.try_get::<Option<i32>, _>(col.ordinal()) {
                    json!(v)
                } else if let Ok(v) = row.try_get::<Option<i8>, _>(col.ordinal()) {
                    json!(v)
                } else {
                    match row.try_get::<Option<String>, _>(col.ordinal()) {
                        Ok(v) => json!(v),
                        Err(_) => Value::Null,
                    }
                }
            }
            "FLOAT" | "DOUBLE" | "REAL" | "NUMERIC" => {
                let v: Option<f64> = row.try_get(col.ordinal()).unwrap_or(None);
                json!(v)
            }
            "BIT" => {
                if let Ok(v) = row.try_get::<Option<u64>, _>(col.ordinal()) {
                    json!(v)
                } else {
                    match row.try_get::<Option<Vec<u8>>, _>(col.ordinal()) {
                        Ok(Some(v)) => {
                            let hex: String =
                                v.iter().map(|b| format!("{:02X}", b)).collect();
                            json!(format!("0x{}", hex))
                        }
                        Ok(None) => Value::Null,
                        Err(_) => Value::Null,
                    }
                }
            }
            "JSON" => match row.try_get::<Option<serde_json::Value>, _>(col.ordinal()) {
                Ok(v) => json!(v),
                Err(_) => Value::Null,
            },
            "TIMESTAMP" | "DATETIME" => {
                match row.try_get::<Option<chrono::NaiveDateTime>, _>(col.ordinal()) {
                    Ok(Some(v)) => json!(v.to_string()),
                    Ok(None) => Value::Null,
                    Err(_) => match row.try_get::<Option<String>, _>(col.ordinal()) {
                        Ok(v) => json!(v),
                        Err(_) => Value::Null,
                    },
                }
            }
            "DATE" => match row.try_get::<Option<chrono::NaiveDate>, _>(col.ordinal()) {
                Ok(Some(v)) => json!(v.to_string()),
                Ok(None) => Value::Null,
                Err(_) => Value::Null,
            },
            "TIME" => match row.try_get::<Option<chrono::NaiveTime>, _>(col.ordinal()) {
                Ok(Some(v)) => json!(v.to_string()),
                Ok(None) => Value::Null,
                Err(_) => Value::Null,
            },
            "YEAR" => match row.try_get::<Option<i32>, _>(col.ordinal()) {
                Ok(Some(v)) => json!(v),
                Ok(None) => Value::Null,
                Err(_) => match row.try_get::<Option<String>, _>(col.ordinal()) {
                    Ok(v) => json!(v),
                    Err(_) => Value::Null,
                },
            },
            _ if type_name.to_uppercase().contains("BINARY")
                || type_name.to_uppercase().contains("BLOB")
                || type_name.to_uppercase().contains("BYTEA") =>
            {
                match row.try_get::<Option<Vec<u8>>, _>(col.ordinal()) {
                    Ok(Some(v)) => {
                        let hex: String =
                            v.iter().take(32).map(|b| format!("{:02X}", b)).collect();
                        let suffix = if v.len() > 32 {
                            format!("... ({} bytes)", v.len())
                        } else {
                            String::new()
                        };
                        json!(format!("0x{}{}", hex, suffix))
                    }
                    Ok(None) => Value::Null,
                    Err(_) => Value::Null,
                }
            }
            _ => match row.try_get::<Option<String>, _>(col.ordinal()) {
                Ok(v) => json!(v),
                Err(_) => match row.try_get::<Option<Vec<u8>>, _>(col.ordinal()) {
                    Ok(Some(v)) => {
                        let hex: String =
                            v.iter().take(16).map(|b| format!("{:02X}", b)).collect();
                        let suffix = if v.len() > 16 { "..." } else { "" };
                        json!(format!("[BLOB: 0x{}{}]", hex, suffix))
                    }
                    Ok(None) => Value::Null,
                    Err(_) => Value::Null,
                },
            },
        };

        map.insert(name.to_string(), value);
    }

    map
}

fn pg_row_to_json_map(row: &PgRow) -> HashMap<String, Value> {
    let mut map = HashMap::new();

    for col in row.columns() {
        let name = col.name();
        let type_name = col.type_info().name();

        let value: Value = match type_name {
            "BOOL" => {
                let v: Option<bool> = row.try_get(col.ordinal()).unwrap_or(None);
                json!(v)
            }
            "INT2" | "INT4" | "INT8" => {
                let v: Option<i64> = row.try_get(col.ordinal()).unwrap_or(None);
                json!(v)
            }
            "FLOAT4" | "FLOAT8" | "NUMERIC" | "MONEY" => {
                let v: Option<f64> = row.try_get(col.ordinal()).unwrap_or(None);
                json!(v)
            }
            "TIMESTAMP" | "TIMESTAMPTZ" => {
                if let Ok(v) = row.try_get::<Option<String>, _>(col.ordinal()) {
                    json!(v)
                } else if let Ok(v) =
                    row.try_get::<Option<chrono::NaiveDateTime>, _>(col.ordinal())
                {
                    json!(v.map(|d| d.to_string()))
                } else if let Ok(v) =
                    row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(col.ordinal())
                {
                    json!(v.map(|d| d.to_string()))
                } else {
                    Value::Null
                }
            }
            "DATE" => {
                if let Ok(v) = row.try_get::<Option<String>, _>(col.ordinal()) {
                    json!(v)
                } else if let Ok(v) = row.try_get::<Option<chrono::NaiveDate>, _>(col.ordinal()) {
                    json!(v.map(|d| d.to_string()))
                } else {
                    Value::Null
                }
            }
            "TIME" | "TIMETZ" => {
                if let Ok(v) = row.try_get::<Option<String>, _>(col.ordinal()) {
                    json!(v)
                } else if let Ok(v) = row.try_get::<Option<chrono::NaiveTime>, _>(col.ordinal()) {
                    json!(v.map(|d| d.to_string()))
                } else {
                    Value::Null
                }
            }
            "JSON" | "JSONB" => {
                if let Ok(v) = row.try_get::<Option<serde_json::Value>, _>(col.ordinal()) {
                    json!(v)
                } else if let Ok(v) = row.try_get::<Option<String>, _>(col.ordinal()) {
                    json!(v)
                } else {
                    Value::Null
                }
            }
            "BYTEA" | "VARBINARY" | "BINARY" | "BLOB" => {
                match row.try_get::<Option<Vec<u8>>, _>(col.ordinal()) {
                    Ok(Some(v)) => {
                        let hex: String =
                            v.iter().take(32).map(|b| format!("{:02X}", b)).collect();
                        let suffix = if v.len() > 32 {
                            format!("... ({} bytes)", v.len())
                        } else {
                            String::new()
                        };
                        json!(format!("0x{}{}", hex, suffix))
                    }
                    Ok(None) => Value::Null,
                    Err(_) => Value::Null,
                }
            }
            _ => match row.try_get::<Option<String>, _>(col.ordinal()) {
                Ok(v) => json!(v),
                Err(_) => match row.try_get::<Option<Vec<u8>>, _>(col.ordinal()) {
                    Ok(Some(v)) => {
                        let hex: String =
                            v.iter().take(16).map(|b| format!("{:02X}", b)).collect();
                        let suffix = if v.len() > 16 { "..." } else { "" };
                        json!(format!("[BLOB: 0x{}{}]", hex, suffix))
                    }
                    _ => Value::Null,
                },
            },
        };

        map.insert(name.to_string(), value);
    }

    map
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

            let rows = sqlx::query(query)
                .fetch_all(&pool)
                .await
                .map_err(|e| e.to_string())?;
            let mut results = Vec::new();

            for row in rows {
                results.push(mysql_row_to_json_map(&row));
            }
            Ok(results)
        }
        "postgresql" => {
            let pool = pool_manager
                .get_pg_pool(config, config.database.as_deref())
                .await?;

            let rows = sqlx::query(query)
                .fetch_all(&pool)
                .await
                .map_err(|e| e.to_string())?;
            let mut results = Vec::new();

            for row in rows {
                results.push(pg_row_to_json_map(&row));
            }
            Ok(results)
        }
        "redis" => {
            let mut con = pool_manager.get_redis_conn(config).await?;

            if let Some(db) = &config.database {
                if !db.is_empty() {
                    let db_part = db.split_whitespace().next().unwrap_or("");
                    let db_index: i32 = if db_part.is_empty() {
                        0
                    } else if let Some(num_str) = db_part.strip_prefix("db") {
                        num_str.parse().unwrap_or(0)
                    } else {
                        db_part.parse().unwrap_or(0)
                    };
                    let _: () = redis::cmd("SELECT")
                        .arg(db_index)
                        .query_async(&mut con)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }

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

                let mut args = Vec::new();
                let mut current = String::new();
                let mut in_quotes = false;
                let mut escape = false;

                for c in trimmed.chars() {
                    if escape {
                        current.push(c);
                        escape = false;
                    } else if c == '\\' {
                        escape = true;
                    } else if c == '"' {
                        in_quotes = !in_quotes;
                    } else if c.is_whitespace() && !in_quotes {
                        if !current.is_empty() {
                            args.push(current.clone());
                            current.clear();
                        }
                    } else {
                        current.push(c);
                    }
                }
                if !current.is_empty() {
                    args.push(current);
                }

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
            let url = if let Some(pass) = &config.password {
                format!(
                    "redis://:{}@{}:{}/{}",
                    pass,
                    config.host,
                    config.port,
                    config.database.as_deref().unwrap_or("0")
                )
            } else {
                format!(
                    "redis://{}:{}/{}",
                    config.host,
                    config.port,
                    config.database.as_deref().unwrap_or("0")
                )
            };

            let client = redis::Client::open(url).map_err(|e| e.to_string())?;
            let mut con = client.get_connection().map_err(|e| e.to_string())?;
            let _: String = redis::cmd("PING")
                .query(&mut con)
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
    let mut connections = if path.exists() {
        let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str::<Vec<ConnectionConfig>>(&content).unwrap_or_default()
    } else {
        Vec::new()
    };

    // Update if exists, otherwise push
    if let Some(idx) = connections.iter().position(|c| c.id == config.id) {
        connections[idx] = config;
    } else {
        connections.push(config);
    }

    let json = serde_json::to_string_pretty(&connections).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn get_connections(app_handle: tauri::AppHandle) -> Result<Vec<ConnectionConfig>, String> {
    let path = get_config_path(&app_handle)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let connections = serde_json::from_str(&content).unwrap_or_default();
    Ok(connections)
}

#[tauri::command]
async fn delete_connection(
    pool_manager: tauri::State<'_, PoolManager>,
    app_handle: tauri::AppHandle,
    id: String,
) -> Result<(), String> {
    let path = get_config_path(&app_handle)?;
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut connections: Vec<ConnectionConfig> = serde_json::from_str(&content).unwrap_or_default();

    // Clean up pool for deleted connection
    if let Some(config) = connections.iter().find(|c| c.id == id) {
        pool_manager.remove_pool(config).await;
    }

    connections.retain(|c| c.id != id);

    let json = serde_json::to_string_pretty(&connections).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())?;
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

            let mut dbs = Vec::new();
            for i in 0..16 {
                // Select db
                let _: () = redis::cmd("SELECT")
                    .arg(i)
                    .query_async(&mut con)
                    .await
                    .map_err(|e| e.to_string())?;
                // Get key count
                let count: i64 = redis::cmd("DBSIZE")
                    .query_async(&mut con)
                    .await
                    .unwrap_or(0);
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
            let mut con = pool_manager.get_redis_conn(&config).await?;

            // Select DB if provided (database param could be "db0 (15)", "db0", "0", or empty)
            let db_str = database.or(config.database.clone()).unwrap_or_default();
            // Extract just the db part before any space (for "db0 (15)" -> "db0")
            let db_part = db_str.split_whitespace().next().unwrap_or("");
            let db_index: i32 = if db_part.is_empty() {
                0
            } else if let Some(num_str) = db_part.strip_prefix("db") {
                num_str.parse().unwrap_or(0)
            } else {
                db_part.parse().unwrap_or(0)
            };
            let _: () = redis::cmd("SELECT")
                .arg(db_index)
                .query_async(&mut con)
                .await
                .map_err(|e| e.to_string())?;

            // Get all keys (limited to 1000 for performance)
            let keys: Vec<String> = redis::cmd("KEYS")
                .arg("*")
                .query_async(&mut con)
                .await
                .map_err(|e| e.to_string())?;

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
                SELECT 
                    c.column_name, 
                    c.data_type,
                    CASE WHEN tc.constraint_type = 'PRIMARY KEY' THEN true ELSE false END as is_pk,
                    c.is_nullable, 
                    c.column_default,
                    pg_catalog.col_description(format('%s.%s', c.table_schema, c.table_name)::regclass::oid, c.ordinal_position) as comment
                FROM information_schema.columns c
                LEFT JOIN information_schema.key_column_usage kcu 
                    ON c.table_schema = kcu.table_schema 
                    AND c.table_name = kcu.table_name 
                    AND c.column_name = kcu.column_name
                LEFT JOIN information_schema.table_constraints tc 
                    ON kcu.constraint_name = tc.constraint_name 
                    AND kcu.table_schema = tc.table_schema
                    AND tc.constraint_type = 'PRIMARY KEY'
                WHERE c.table_schema = 'public' AND c.table_name = $1
                ORDER BY c.ordinal_position
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
            let mut con = pool_manager.get_redis_conn(&config).await?;

            // Select DB
            if let Some(db) = &database.or(config.database.clone()) {
                if !db.is_empty() {
                    let db_part = db.split_whitespace().next().unwrap_or("");
                    let db_index: i32 = if let Some(num_str) = db_part.strip_prefix("db") {
                        num_str.parse().unwrap_or(0)
                    } else {
                        db_part.parse().unwrap_or(0)
                    };
                    let _: () = redis::cmd("SELECT")
                        .arg(db_index)
                        .query_async(&mut con)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }

            // Get key type
            let key_type: String = redis::cmd("TYPE")
                .arg(&table)
                .query_async(&mut con)
                .await
                .unwrap_or_else(|_| "unknown".to_string());

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
    let cancel_flag = export_task_manager.start_task(&task_id).await;
    let mut total_tables = 0usize;
    let mut total_units = 1usize;
    let mut processed_units = 0usize;
    let mut processed_tables = 0usize;
    let mut current_table: Option<String> = None;
    let mut current_row = 0usize;
    let mut current_table_rows = 0usize;

    let emit_running = |stage: &str,
                        table_name: Option<&str>,
                        processed_rows: usize,
                        table_rows: usize,
                        processed_units: usize,
                        processed_tables: usize,
                        total_tables: usize,
                        total_units: usize| {
        emit_export_progress(
            &app_handle,
            DatabaseExportProgress {
                task_id: task_id.clone(),
                database: target_db.clone(),
                progress: calculate_progress(processed_units, total_units, "running"),
                status: "running".to_string(),
                stage: stage.to_string(),
                table_name: table_name.map(|value| value.to_string()),
                processed_tables,
                total_tables,
                processed_rows,
                table_rows,
                error: None,
            },
        );
    };

    emit_running(
        "preparing",
        None,
        0,
        0,
        processed_units,
        processed_tables,
        total_tables,
        total_units,
    );

    let export_result: Result<(), String> = async {
        let file = fs::File::create(&output_path).map_err(|e| e.to_string())?;
        let mut writer = BufWriter::new(file);

        match config.db_type.as_str() {
            "mysql" => {
                let pool = pool_manager.get_mysql_pool(&config, Some(&target_db)).await?;
                let tables: Vec<(String, Option<u64>)> = sqlx::query_as(
                    "
                    SELECT TABLE_NAME, TABLE_ROWS
                    FROM information_schema.TABLES
                    WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = 'BASE TABLE'
                    ORDER BY TABLE_NAME
                    ",
                )
                .bind(&target_db)
                .fetch_all(&pool)
                .await
                .map_err(|e| e.to_string())?;

                total_tables = tables.len();
                total_units = 1 + tables
                    .iter()
                    .map(|(_, row_count)| row_count.unwrap_or(0) as usize + 20)
                    .sum::<usize>();
                emit_running(
                    "preparing",
                    None,
                    0,
                    0,
                    processed_units,
                    processed_tables,
                    total_tables,
                    total_units,
                );

                writeln!(writer, "-- RECCH database export").map_err(|e| e.to_string())?;
                writeln!(writer, "-- Database: {}", target_db).map_err(|e| e.to_string())?;
                writeln!(writer, "SET FOREIGN_KEY_CHECKS=0;").map_err(|e| e.to_string())?;
                writeln!(writer).map_err(|e| e.to_string())?;

                for (table, estimated_rows) in tables {
                    ensure_not_cancelled(cancel_flag.as_ref())?;
                    current_table = Some(table.clone());
                    current_row = 0;
                    current_table_rows = estimated_rows.unwrap_or(0) as usize;

                    emit_running(
                        "schema",
                        current_table.as_deref(),
                        current_row,
                        current_table_rows,
                        processed_units,
                        processed_tables,
                        total_tables,
                        total_units,
                    );

                    let quoted_table = quote_mysql_identifier(&table);
                    let show_create_query = format!("SHOW CREATE TABLE {}", quoted_table);
                    let create_row = sqlx::query(&show_create_query)
                        .fetch_one(&pool)
                        .await
                        .map_err(|e| e.to_string())?;
                    let create_stmt: String =
                        create_row.try_get(1).map_err(|e| e.to_string())?;

                    writeln!(writer, "-- Table: {}", table).map_err(|e| e.to_string())?;
                    writeln!(writer, "DROP TABLE IF EXISTS {};", quoted_table)
                        .map_err(|e| e.to_string())?;
                    writeln!(writer, "{};", create_stmt).map_err(|e| e.to_string())?;

                    processed_units += 10;
                    emit_running(
                        "counting",
                        current_table.as_deref(),
                        current_row,
                        current_table_rows,
                        processed_units,
                        processed_tables,
                        total_tables,
                        total_units,
                    );

                    let count_query = format!("SELECT COUNT(*) FROM {}", quoted_table);
                    let count_row = sqlx::query(&count_query)
                        .fetch_one(&pool)
                        .await
                        .map_err(|e| e.to_string())?;
                    let exact_table_rows = count_row
                        .try_get::<i64, _>(0)
                        .map(|value| value.max(0) as usize)
                        .or_else(|_| count_row.try_get::<u64, _>(0).map(|value| value as usize))
                        .map_err(|e| e.to_string())?;

                    if exact_table_rows > current_table_rows {
                        total_units += exact_table_rows - current_table_rows;
                    } else {
                        total_units = total_units.saturating_sub(current_table_rows - exact_table_rows);
                    }
                    current_table_rows = exact_table_rows;

                    emit_running(
                        "fetching",
                        current_table.as_deref(),
                        current_row,
                        current_table_rows,
                        processed_units,
                        processed_tables,
                        total_tables,
                        total_units,
                    );

                    let columns: Vec<String> = sqlx::query_scalar(
                        "
                        SELECT COLUMN_NAME
                        FROM information_schema.COLUMNS
                        WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
                        ORDER BY ORDINAL_POSITION
                        ",
                    )
                    .bind(&target_db)
                    .bind(&table)
                    .fetch_all(&pool)
                    .await
                    .map_err(|e| e.to_string())?;

                    let column_sql = columns
                        .iter()
                        .map(|column| quote_mysql_identifier(column))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let mut actual_rows = 0usize;
                    let data_query = format!("SELECT * FROM {}", quoted_table);
                    let mut rows = sqlx::query(&data_query).fetch(&pool);

                    while let Some(row) = rows.try_next().await.map_err(|e| e.to_string())? {
                        ensure_not_cancelled(cancel_flag.as_ref())?;

                        let row_map = mysql_row_to_json_map(&row);
                        let values = columns
                            .iter()
                            .map(|column| sql_literal(row_map.get(column), "mysql"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        writeln!(
                            writer,
                            "INSERT INTO {} ({}) VALUES ({});",
                            quoted_table, column_sql, values
                        )
                        .map_err(|e| e.to_string())?;

                        processed_units += 1;
                        current_row += 1;
                        actual_rows += 1;
                        if current_row > current_table_rows {
                            total_units += current_row - current_table_rows;
                            current_table_rows = current_row;
                        }
                        if current_row % 100 == 0 || current_row == current_table_rows {
                            emit_running(
                                "data",
                                current_table.as_deref(),
                                current_row,
                                current_table_rows,
                                processed_units,
                                processed_tables,
                                total_tables,
                                total_units,
                            );
                        }
                    }

                    let row_units = current_table_rows.max(actual_rows);
                    if row_units > actual_rows {
                        processed_units += row_units - actual_rows;
                        current_table_rows = actual_rows;
                    }

                    writeln!(writer).map_err(|e| e.to_string())?;
                    processed_units += 10;
                    processed_tables += 1;
                    current_row = current_table_rows;
                    emit_running(
                        "table_complete",
                        current_table.as_deref(),
                        current_row,
                        current_table_rows,
                        processed_units,
                        processed_tables,
                        total_tables,
                        total_units,
                    );
                }

                writeln!(writer, "SET FOREIGN_KEY_CHECKS=1;").map_err(|e| e.to_string())?;
                writer.flush().map_err(|e| e.to_string())?;
                Ok(())
            }
            "postgresql" => {
                let pool = pool_manager.get_pg_pool(&config, Some(&target_db)).await?;
                let tables: Vec<(String, Option<i64>)> = sqlx::query_as(
                    "
                    SELECT c.relname AS table_name, CAST(c.reltuples AS BIGINT) AS row_count
                    FROM pg_class c
                    JOIN pg_namespace n ON n.oid = c.relnamespace
                    WHERE n.nspname = 'public' AND c.relkind = 'r'
                    ORDER BY c.relname
                    ",
                )
                .fetch_all(&pool)
                .await
                .map_err(|e| e.to_string())?;

                total_tables = tables.len();
                total_units = 1 + tables
                    .iter()
                    .map(|(_, row_count)| row_count.unwrap_or(0).max(0) as usize + 20)
                    .sum::<usize>();
                emit_running(
                    "preparing",
                    None,
                    0,
                    0,
                    processed_units,
                    processed_tables,
                    total_tables,
                    total_units,
                );

                let public_schema = quote_pg_identifier("public");

                writeln!(writer, "-- RECCH database export").map_err(|e| e.to_string())?;
                writeln!(writer, "-- Database: {}", target_db).map_err(|e| e.to_string())?;
                writeln!(writer, "BEGIN;").map_err(|e| e.to_string())?;
                writeln!(writer).map_err(|e| e.to_string())?;

                for (table, estimated_rows) in tables {
                    ensure_not_cancelled(cancel_flag.as_ref())?;
                    current_table = Some(table.clone());
                    current_row = 0;
                    current_table_rows = estimated_rows.unwrap_or(0).max(0) as usize;

                    emit_running(
                        "schema",
                        current_table.as_deref(),
                        current_row,
                        current_table_rows,
                        processed_units,
                        processed_tables,
                        total_tables,
                        total_units,
                    );

                    let quoted_table = quote_pg_identifier(&table);
                    let full_table_name = format!("{}.{}", public_schema, quoted_table);

                    let columns: Vec<(String, String, bool, Option<String>, Option<String>)> =
                        sqlx::query_as(
                            "
                            SELECT
                                a.attname AS column_name,
                                pg_catalog.format_type(a.atttypid, a.atttypmod) AS data_type,
                                NOT a.attnotnull AS is_nullable,
                                pg_get_expr(ad.adbin, ad.adrelid) AS column_default,
                                pg_catalog.col_description(a.attrelid, a.attnum) AS comment
                            FROM pg_attribute a
                            JOIN pg_class c ON a.attrelid = c.oid
                            JOIN pg_namespace n ON c.relnamespace = n.oid
                            LEFT JOIN pg_attrdef ad ON a.attrelid = ad.adrelid AND a.attnum = ad.adnum
                            WHERE n.nspname = 'public'
                              AND c.relname = $1
                              AND a.attnum > 0
                              AND NOT a.attisdropped
                            ORDER BY a.attnum
                            ",
                        )
                        .bind(&table)
                        .fetch_all(&pool)
                        .await
                        .map_err(|e| e.to_string())?;

                    let primary_key_columns: Vec<String> = sqlx::query_scalar(
                        "
                        SELECT a.attname
                        FROM pg_index i
                        JOIN pg_class c ON c.oid = i.indrelid
                        JOIN pg_namespace n ON n.oid = c.relnamespace
                        JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum = ANY(i.indkey)
                        WHERE n.nspname = 'public'
                          AND c.relname = $1
                          AND i.indisprimary
                        ORDER BY array_position(i.indkey, a.attnum)
                        ",
                    )
                    .bind(&table)
                    .fetch_all(&pool)
                    .await
                    .map_err(|e| e.to_string())?;

                    let table_comment: Option<String> = sqlx::query_scalar(
                        "
                        SELECT obj_description(c.oid, 'pg_class')
                        FROM pg_class c
                        JOIN pg_namespace n ON n.oid = c.relnamespace
                        WHERE n.nspname = 'public' AND c.relname = $1
                        ",
                    )
                    .bind(&table)
                    .fetch_one(&pool)
                    .await
                    .map_err(|e| e.to_string())?;

                    let index_statements: Vec<String> = sqlx::query_scalar(
                        "
                        SELECT indexdef
                        FROM pg_indexes
                        WHERE schemaname = 'public'
                          AND tablename = $1
                          AND indexname NOT LIKE '%_pkey'
                        ORDER BY indexname
                        ",
                    )
                    .bind(&table)
                    .fetch_all(&pool)
                    .await
                    .map_err(|e| e.to_string())?;

                    let foreign_keys: Vec<(String, String)> = sqlx::query_as(
                        "
                        SELECT conname, pg_get_constraintdef(oid)
                        FROM pg_constraint
                        WHERE conrelid = format('public.%I', $1)::regclass
                          AND contype = 'f'
                        ORDER BY conname
                        ",
                    )
                    .bind(&table)
                    .fetch_all(&pool)
                    .await
                    .map_err(|e| e.to_string())?;

                    let mut create_lines = Vec::new();
                    let mut ordered_columns = Vec::new();
                    let mut serial_columns = Vec::new();

                    for (name, data_type, is_nullable, default_value, _) in &columns {
                        ordered_columns.push(name.clone());

                        let (normalized_type, normalized_default, is_serial) =
                            normalize_pg_column_definition(data_type, default_value.as_deref());
                        if is_serial {
                            serial_columns.push(name.clone());
                        }

                        let mut line =
                            format!("{} {}", quote_pg_identifier(name), normalized_type);
                        if let Some(default_sql) = normalized_default {
                            line.push_str(&format!(" DEFAULT {}", default_sql));
                        }
                        if !is_nullable {
                            line.push_str(" NOT NULL");
                        }
                        create_lines.push(line);
                    }

                    if !primary_key_columns.is_empty() {
                        let pk_sql = primary_key_columns
                            .iter()
                            .map(|column| quote_pg_identifier(column))
                            .collect::<Vec<_>>()
                            .join(", ");
                        create_lines.push(format!("PRIMARY KEY ({})", pk_sql));
                    }

                    writeln!(writer, "-- Table: {}", table).map_err(|e| e.to_string())?;
                    writeln!(writer, "DROP TABLE IF EXISTS {} CASCADE;", full_table_name)
                        .map_err(|e| e.to_string())?;
                    writeln!(
                        writer,
                        "CREATE TABLE {} (\n    {}\n);",
                        full_table_name,
                        create_lines.join(",\n    ")
                    )
                    .map_err(|e| e.to_string())?;

                    if let Some(comment) = table_comment {
                        if !comment.is_empty() {
                            writeln!(
                                writer,
                                "COMMENT ON TABLE {} IS '{}';",
                                full_table_name,
                                escape_sql_string(&comment, "postgresql")
                            )
                            .map_err(|e| e.to_string())?;
                        }
                    }

                    for (name, _, _, _, comment) in &columns {
                        if let Some(comment) = comment {
                            if !comment.is_empty() {
                                writeln!(
                                    writer,
                                    "COMMENT ON COLUMN {}.{} IS '{}';",
                                    full_table_name,
                                    quote_pg_identifier(name),
                                    escape_sql_string(comment, "postgresql")
                                )
                                .map_err(|e| e.to_string())?;
                            }
                        }
                    }

                    processed_units += 10;
                    emit_running(
                        "counting",
                        current_table.as_deref(),
                        current_row,
                        current_table_rows,
                        processed_units,
                        processed_tables,
                        total_tables,
                        total_units,
                    );

                    let count_query = format!("SELECT COUNT(*) FROM {}", full_table_name);
                    let exact_table_rows: i64 = sqlx::query_scalar(&count_query)
                        .fetch_one(&pool)
                        .await
                        .map_err(|e| e.to_string())?;

                    let exact_table_rows = exact_table_rows.max(0) as usize;
                    if exact_table_rows > current_table_rows {
                        total_units += exact_table_rows - current_table_rows;
                    } else {
                        total_units = total_units.saturating_sub(current_table_rows - exact_table_rows);
                    }
                    current_table_rows = exact_table_rows;

                    emit_running(
                        "fetching",
                        current_table.as_deref(),
                        current_row,
                        current_table_rows,
                        processed_units,
                        processed_tables,
                        total_tables,
                        total_units,
                    );

                    let column_sql = ordered_columns
                        .iter()
                        .map(|column| quote_pg_identifier(column))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let mut actual_rows = 0usize;
                    let data_query = format!("SELECT * FROM {}", full_table_name);
                    let mut rows = sqlx::query(&data_query).fetch(&pool);

                    while let Some(row) = rows.try_next().await.map_err(|e| e.to_string())? {
                        ensure_not_cancelled(cancel_flag.as_ref())?;

                        let row_map = pg_row_to_json_map(&row);
                        let values = ordered_columns
                            .iter()
                            .map(|column| sql_literal(row_map.get(column), "postgresql"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        writeln!(
                            writer,
                            "INSERT INTO {} ({}) VALUES ({});",
                            full_table_name, column_sql, values
                        )
                        .map_err(|e| e.to_string())?;

                        processed_units += 1;
                        current_row += 1;
                        actual_rows += 1;
                        if current_row > current_table_rows {
                            total_units += current_row - current_table_rows;
                            current_table_rows = current_row;
                        }
                        if current_row % 100 == 0 || current_row == current_table_rows {
                            emit_running(
                                "data",
                                current_table.as_deref(),
                                current_row,
                                current_table_rows,
                                processed_units,
                                processed_tables,
                                total_tables,
                                total_units,
                            );
                        }
                    }

                    let row_units = current_table_rows.max(actual_rows);
                    if row_units > actual_rows {
                        processed_units += row_units - actual_rows;
                        current_table_rows = actual_rows;
                    }

                    for serial_column in serial_columns {
                        let relation_name = format!("{}.{}", public_schema, quoted_table);
                        let column_ident = quote_pg_identifier(&serial_column);
                        writeln!(
                            writer,
                            "SELECT setval(pg_get_serial_sequence('{}', '{}'), COALESCE((SELECT MAX({}) FROM {}), 1), COALESCE((SELECT MAX({}) IS NOT NULL FROM {}), false));",
                            escape_sql_string(&relation_name, "postgresql"),
                            escape_sql_string(&serial_column, "postgresql"),
                            column_ident,
                            full_table_name,
                            column_ident,
                            full_table_name
                        )
                        .map_err(|e| e.to_string())?;
                    }

                    for indexdef in index_statements {
                        writeln!(writer, "{};", indexdef).map_err(|e| e.to_string())?;
                    }

                    for (constraint_name, constraint_def) in foreign_keys {
                        writeln!(
                            writer,
                            "ALTER TABLE ONLY {} ADD CONSTRAINT {} {};",
                            full_table_name,
                            quote_pg_identifier(&constraint_name),
                            constraint_def
                        )
                        .map_err(|e| e.to_string())?;
                    }

                    writeln!(writer).map_err(|e| e.to_string())?;
                    processed_units += 10;
                    processed_tables += 1;
                    current_row = current_table_rows;
                    emit_running(
                        "table_complete",
                        current_table.as_deref(),
                        current_row,
                        current_table_rows,
                        processed_units,
                        processed_tables,
                        total_tables,
                        total_units,
                    );
                }

                writeln!(writer, "COMMIT;").map_err(|e| e.to_string())?;
                writer.flush().map_err(|e| e.to_string())?;
                Ok(())
            }
            _ => Err("Database export is only supported for MySQL and PostgreSQL".to_string()),
        }
    }
    .await;

    export_task_manager.remove_task(&task_id).await;

    let status = match &export_result {
        Ok(_) => "completed",
        Err(err) if err == "Export cancelled" => "cancelled",
        Err(_) => "error",
    };

    if export_result.is_err() {
        let _ = fs::remove_file(&output_path);
    }

    emit_export_progress(
        &app_handle,
        DatabaseExportProgress {
            task_id,
            database: target_db,
            progress: calculate_progress(processed_units, total_units, status),
            status: status.to_string(),
            stage: status.to_string(),
            table_name: current_table,
            processed_tables,
            total_tables,
            processed_rows: current_row,
            table_rows: current_table_rows,
            error: export_result
                .as_ref()
                .err()
                .and_then(|err| if status == "error" { Some(err.clone()) } else { None }),
        },
    );

    export_result
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
    let target_db = resolve_target_database(&config, database.as_deref())?;

    match config.db_type.as_str() {
        "mysql" => {
            let pool = pool_manager.get_mysql_pool(&config, Some(&target_db)).await?;
            let result = raw_sql(&script).execute(&pool).await.map_err(|e| e.to_string())?;
            Ok(result.rows_affected())
        }
        "postgresql" => {
            let pool = pool_manager.get_pg_pool(&config, Some(&target_db)).await?;
            let result = raw_sql(&script).execute(&pool).await.map_err(|e| e.to_string())?;
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
    let query = match config.db_type.as_str() {
        "mysql" => {
            match operation.op_type.as_str() {
                "add" => {
                    let col = operation
                        .column_def
                        .as_ref()
                        .ok_or("Missing column definition")?;
                    let comment = col
                        .comment
                        .as_ref()
                        .map(|c| format!("COMMENT '{}'", c.replace("'", "''")))
                        .unwrap_or_default();
                    let null_def = if col.is_nullable == Some(false) {
                        "NOT NULL"
                    } else {
                        "NULL"
                    };
                    let default_def = col
                        .default_value
                        .as_ref()
                        .map(|d| format!("DEFAULT {}", d))
                        .unwrap_or_default();
                    let pk_def = if col.is_pk { "PRIMARY KEY" } else { "" };

                    format!(
                        "ALTER TABLE {} ADD COLUMN {} {} {} {} {} {}",
                        table, col.name, col.type_name, null_def, default_def, pk_def, comment
                    )
                }
                "modify" => {
                    let col = operation
                        .column_def
                        .as_ref()
                        .ok_or("Missing column definition")?;
                    let comment = col
                        .comment
                        .as_ref()
                        .map(|c| format!("COMMENT '{}'", c.replace("'", "''")))
                        .unwrap_or_default();
                    let null_def = if col.is_nullable == Some(false) {
                        "NOT NULL"
                    } else {
                        "NULL"
                    };
                    let default_def = col
                        .default_value
                        .as_ref()
                        .map(|d| format!("DEFAULT {}", d))
                        .unwrap_or_default();

                    format!(
                        "ALTER TABLE {} MODIFY COLUMN {} {} {} {} {}",
                        table, col.name, col.type_name, null_def, default_def, comment
                    )
                }
                "drop" => {
                    let col_name = operation
                        .column_name
                        .as_ref()
                        .ok_or("Missing column name")?;
                    format!("ALTER TABLE {} DROP COLUMN {}", table, col_name)
                }
                "rename" => {
                    // MySQL RENAME COLUMN old TO new
                    let col_name = operation
                        .column_name
                        .as_ref()
                        .ok_or("Missing column name")?;
                    let new_name = operation.new_name.as_ref().ok_or("Missing new name")?;
                    format!(
                        "ALTER TABLE {} RENAME COLUMN {} TO {}",
                        table, col_name, new_name
                    )
                }
                "add_index" => {
                    let idx = operation
                        .index_def
                        .as_ref()
                        .ok_or("Missing index definition")?;
                    let cols = idx.columns.join(", ");
                    let unique = if idx.is_unique { "UNIQUE" } else { "" };
                    format!(
                        "CREATE {} INDEX {} ON {} ({})",
                        unique, idx.name, table, cols
                    )
                }
                "drop_index" => {
                    let idx_name = operation.index_name.as_ref().ok_or("Missing index name")?;
                    format!("DROP INDEX {} ON {}", idx_name, table)
                }
                _ => return Err("Unknown operation".to_string()),
            }
        }
        "postgresql" => {
            match operation.op_type.as_str() {
                "add" => {
                    let col = operation
                        .column_def
                        .as_ref()
                        .ok_or("Missing column definition")?;
                    // PG doesn't support comment in ADD COLUMN syntax directly usually, need separate COMMENT ON
                    // But for simplicity here, we might just add column first. Detailed comment support needs multiple queries or a transaction.
                    // For now: ALTER TABLE ... ADD COLUMN ...
                    format!(
                        "ALTER TABLE {} ADD COLUMN {} {}",
                        table, col.name, col.type_name
                    )
                }
                "modify" => {
                    let col = operation
                        .column_def
                        .as_ref()
                        .ok_or("Missing column definition")?;
                    // PG: ALTER TABLE ... ALTER COLUMN ... TYPE ...
                    format!(
                        "ALTER TABLE {} ALTER COLUMN {} TYPE {}",
                        table, col.name, col.type_name
                    )
                }
                "drop" => {
                    let col_name = operation
                        .column_name
                        .as_ref()
                        .ok_or("Missing column name")?;
                    format!("ALTER TABLE {} DROP COLUMN {}", table, col_name)
                }
                "rename" => {
                    let col_name = operation
                        .column_name
                        .as_ref()
                        .ok_or("Missing column name")?;
                    let new_name = operation.new_name.as_ref().ok_or("Missing new name")?;
                    format!(
                        "ALTER TABLE {} RENAME COLUMN {} TO {}",
                        table, col_name, new_name
                    )
                }
                "add_index" => {
                    let idx = operation
                        .index_def
                        .as_ref()
                        .ok_or("Missing index definition")?;
                    let cols = idx.columns.join(", ");
                    let unique = if idx.is_unique { "UNIQUE" } else { "" };
                    format!(
                        "CREATE {} INDEX {} ON {} ({})",
                        unique, idx.name, table, cols
                    )
                }
                "drop_index" => {
                    let idx_name = operation.index_name.as_ref().ok_or("Missing index name")?;
                    format!("DROP INDEX {}", idx_name)
                }
                _ => return Err("Unknown operation".to_string()),
            }
        }
        _ => return Err("Unsupported database".to_string()),
    };

    match config.db_type.as_str() {
        "mysql" => {
            let pool = pool_manager
                .get_mysql_pool(&config, config.database.as_deref())
                .await?;
            sqlx::query(&query)
                .execute(&pool)
                .await
                .map_err(|e| e.to_string())?;
        }
        "postgresql" => {
            let pool = pool_manager
                .get_pg_pool(&config, config.database.as_deref())
                .await?;
            sqlx::query(&query)
                .execute(&pool)
                .await
                .map_err(|e| e.to_string())?;

            // Handle comment for PG separately if it's ADD
            if operation.op_type == "add" && config.db_type == "postgresql" {
                if let Some(col) = operation.column_def.as_ref() {
                    if let Some(comment) = &col.comment {
                        let comment_query = format!(
                            "COMMENT ON COLUMN {}.{} IS '{}'",
                            table,
                            col.name,
                            comment.replace("'", "''")
                        );
                        let _ = sqlx::query(&comment_query).execute(&pool).await;
                    }
                }
            }
        }
        _ => {}
    }

    Ok(())
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

    if config_path.exists() {
        let content = fs::read_to_string(&config_path).map_err(|e| e.to_string())?;
        let config: ai_service::AIConfig = serde_json::from_str(&content).unwrap_or_default();
        Ok(config)
    } else {
        Ok(ai_service::AIConfig::default())
    }
}

#[tauri::command]
async fn save_ai_config(app: tauri::AppHandle, config: ai_service::AIConfig) -> Result<(), String> {
    let config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    let config_path = config_dir.join("ai_config.json");

    let content = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(&config_path, content).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn generate_sql_from_text(
    app: tauri::AppHandle,
    db_type: String,
    table_schemas: String,
    user_request: String,
) -> Result<String, String> {
    let config = get_ai_config(app).await?;

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
    let mut con = pool_manager.get_redis_conn(&config).await?;

    // Select DB
    let db_str = database.or(config.database).unwrap_or_default();
    let db_part = db_str.split_whitespace().next().unwrap_or("");
    let db_index: i32 = if db_part.is_empty() {
        0
    } else if let Some(num_str) = db_part.strip_prefix("db") {
        num_str.parse().unwrap_or(0)
    } else {
        db_part.parse().unwrap_or(0)
    };
    let _: () = redis::cmd("SELECT")
        .arg(db_index)
        .query_async(&mut con)
        .await
        .map_err(|e| e.to_string())?;

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
        .unwrap_or(-1);

    // Get value based on type
    let (value, length) = match key_type.as_str() {
        "string" => {
            let v: String = redis::cmd("GET")
                .arg(&key)
                .query_async(&mut con)
                .await
                .unwrap_or_default();
            (v, None)
        }
        "list" => {
            let len: i64 = redis::cmd("LLEN")
                .arg(&key)
                .query_async(&mut con)
                .await
                .unwrap_or(0);
            let items: Vec<String> = redis::cmd("LRANGE")
                .arg(&key)
                .arg(0)
                .arg(99)
                .query_async(&mut con)
                .await
                .unwrap_or_default();
            (
                serde_json::to_string_pretty(&items).unwrap_or_default(),
                Some(len),
            )
        }
        "set" => {
            let len: i64 = redis::cmd("SCARD")
                .arg(&key)
                .query_async(&mut con)
                .await
                .unwrap_or(0);
            let items: Vec<String> = redis::cmd("SMEMBERS")
                .arg(&key)
                .query_async(&mut con)
                .await
                .unwrap_or_default();
            (
                serde_json::to_string_pretty(&items).unwrap_or_default(),
                Some(len),
            )
        }
        "zset" => {
            let len: i64 = redis::cmd("ZCARD")
                .arg(&key)
                .query_async(&mut con)
                .await
                .unwrap_or(0);
            let items: Vec<String> = redis::cmd("ZRANGE")
                .arg(&key)
                .arg(0)
                .arg(99)
                .arg("WITHSCORES")
                .query_async(&mut con)
                .await
                .unwrap_or_default();
            (
                serde_json::to_string_pretty(&items).unwrap_or_default(),
                Some(len),
            )
        }
        "hash" => {
            let len: i64 = redis::cmd("HLEN")
                .arg(&key)
                .query_async(&mut con)
                .await
                .unwrap_or(0);
            let items: Vec<String> = redis::cmd("HGETALL")
                .arg(&key)
                .query_async(&mut con)
                .await
                .unwrap_or_default();
            // Convert flat list to key-value pairs
            let mut map = std::collections::HashMap::new();
            let mut iter = items.iter();
            while let (Some(k), Some(v)) = (iter.next(), iter.next()) {
                map.insert(k.clone(), v.clone());
            }
            (
                serde_json::to_string_pretty(&map).unwrap_or_default(),
                Some(len),
            )
        }
        _ => ("(unknown type)".to_string(), None),
    };

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
