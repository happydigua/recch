use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::mysql::MySqlConnectOptions;
use sqlx::postgres::PgConnectOptions;
use sqlx::ConnectOptions;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use sqlx::{Column, Row, TypeInfo};
use std::collections::HashMap;
use tauri::Manager;

mod ai_service;

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
fn delete_connection(app_handle: tauri::AppHandle, id: String) -> Result<(), String> {
    let path = get_config_path(&app_handle)?;
    if !path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut connections: Vec<ConnectionConfig> = serde_json::from_str(&content).unwrap_or_default();

    connections.retain(|c| c.id != id);

    let json = serde_json::to_string_pretty(&connections).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn get_databases(config: ConnectionConfig) -> Result<Vec<String>, String> {
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

            // Connect without specific DB to list them
            let mut conn = opts.connect().await.map_err(|e| e.to_string())?;
            let dbs: Vec<String> = sqlx::query_scalar("SHOW DATABASES")
                .fetch_all(&mut conn)
                .await
                .map_err(|e| e.to_string())?;
            Ok(dbs)
        }
        "postgresql" => {
            let mut opts = PgConnectOptions::new().host(&config.host).port(config.port);
            if let Some(user) = &config.username {
                opts = opts.username(user);
            }
            if let Some(pass) = &config.password {
                opts = opts.password(pass);
            }
            // For PG, usually connect to 'postgres' or template1 to listing, or user default
            // If explicit DB not provided, it tries user default.

            let mut conn = opts.connect().await.map_err(|e| e.to_string())?;
            let dbs: Vec<String> =
                sqlx::query_scalar("SELECT datname FROM pg_database WHERE datistemplate = false")
                    .fetch_all(&mut conn)
                    .await
                    .map_err(|e| e.to_string())?;
            Ok(dbs)
        }
        "redis" => {
            // Redis has 16 databases by default (0-15)
            // Query each one for key count using DBSIZE
            let url = format!("redis://{}:{}/", config.host, config.port);
            let client = redis::Client::open(url).map_err(|e| e.to_string())?;
            let mut con = client
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| e.to_string())?;

            // Auth if needed
            if let Some(pass) = &config.password {
                if !pass.is_empty() {
                    let _: () = redis::cmd("AUTH")
                        .arg(pass)
                        .query_async(&mut con)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }

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
    config: ConnectionConfig,
    database: Option<String>,
) -> Result<Vec<TableInfo>, String> {
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

            // Use provided database or config default
            let target_db = database.or(config.database);
            let mut db_name = String::new();
            if let Some(db) = &target_db {
                if !db.is_empty() {
                    opts = opts.database(db);
                    db_name = db.clone();
                }
            }

            let mut conn = opts.connect().await.map_err(|e| e.to_string())?;

            // Handle current_db safely
            let current_db: String = if !db_name.is_empty() {
                db_name
            } else {
                let row: Option<String> = sqlx::query_scalar("SELECT DATABASE()")
                    .fetch_one(&mut conn)
                    .await
                    .unwrap_or(None);
                row.unwrap_or_default()
            };

            // If we still don't have a DB name, we can't query information_schema for specific table schema easily
            // But if we are connected, `SHOW TABLES` works.
            // Let's rely on `SHOW TABLE STATUS` which provides size info and is safer than querying information_schema if DB is ambiguous
            // Actually `information_schema.TABLES` is standard.

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

            // Use Row to manually extract to avoid strict type mapping issues (u64 vs i64)
            let rows = sqlx::query(query)
                .bind(&current_db)
                .fetch_all(&mut conn)
                .await
                .map_err(|e| format!("Failed to fetch tables: {}", e))?;

            let mut tables = Vec::new();
            for row in rows {
                let name: String = row.try_get("TABLE_NAME").unwrap_or_default();
                // DATA_LENGTH is BIGINT UNSIGNED (u64), cast to i64
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
            let mut opts = PgConnectOptions::new().host(&config.host).port(config.port);
            if let Some(user) = &config.username {
                opts = opts.username(user);
            }
            if let Some(pass) = &config.password {
                opts = opts.password(pass);
            }

            let target_db = database.or(config.database);
            if let Some(db) = target_db {
                if !db.is_empty() {
                    opts = opts.database(&db);
                }
            }

            let mut conn = opts.connect().await.map_err(|e| e.to_string())?;

            // Query for tables + sizes
            // We use pg_total_relation_size(oid) and pg_relation_size(oid)
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
                .fetch_all(&mut conn)
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
            // For Redis, return all keys as "tables"
            let url = format!("redis://{}:{}/", config.host, config.port);
            let client = redis::Client::open(url).map_err(|e| e.to_string())?;
            let mut con = client
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| e.to_string())?;

            // Auth if needed
            if let Some(pass) = &config.password {
                if !pass.is_empty() {
                    let _: () = redis::cmd("AUTH")
                        .arg(pass)
                        .query_async(&mut con)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }

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
    config: ConnectionConfig,
    table: String,
    database: Option<String>,
) -> Result<Vec<ColumnDef>, String> {
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

            let target_db = database.clone().or(config.database.clone());
            if let Some(db) = &target_db {
                if !db.is_empty() {
                    opts = opts.database(db);
                }
            }

            let mut conn = opts.connect().await.map_err(|e| {
                println!("MySQL Connection Error: {}", e);
                e.to_string()
            })?;

            let db_name = target_db.unwrap_or_else(|| "".to_string());

            // Added IS_NULLABLE, COLUMN_DEFAULT
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

            // Read as bytes (Vec<u8>) to avoid "BLOB vs VARCHAR" type mismatch errors
            // Use Option for ALL fields to be safe against unexpected nulls
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

            let rows = q.fetch_all(&mut conn).await.map_err(|e| {
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
            let mut opts = PgConnectOptions::new().host(&config.host).port(config.port);
            if let Some(user) = &config.username {
                opts = opts.username(user);
            }
            if let Some(pass) = &config.password {
                opts = opts.password(pass);
            }
            let target_db = database.or(config.database);
            if let Some(db) = target_db {
                if !db.is_empty() {
                    opts = opts.database(&db);
                }
            }

            let mut conn = opts.connect().await.map_err(|e| e.to_string())?;

            // Postgres PK detection and Comments
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
                .fetch_all(&mut conn)
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
            // For Redis, return key type info instead of columns
            let url = format!("redis://{}:{}/", config.host, config.port);
            let client = redis::Client::open(url).map_err(|e| e.to_string())?;
            let mut con = client
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| e.to_string())?;

            // Auth if needed
            if let Some(pass) = &config.password {
                if !pass.is_empty() {
                    let _: () = redis::cmd("AUTH")
                        .arg(pass)
                        .query_async(&mut con)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }

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
async fn get_indexes(config: ConnectionConfig, table: String) -> Result<Vec<IndexDef>, String> {
    match config.db_type.as_str() {
        "mysql" => {
            // ... connection setup ...
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
            let mut conn = opts.connect().await.map_err(|e| e.to_string())?;

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
                .fetch_all(&mut conn)
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
            // ... connection setup ...
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
            let mut conn = opts.connect().await.map_err(|e| e.to_string())?;

            // Simple query over pg_indexes logic
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
            .fetch_all(&mut conn)
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

// ... existing code ...

#[tauri::command]
async fn alter_table(
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
            let mut conn = opts.connect().await.map_err(|e| e.to_string())?;
            sqlx::query(&query)
                .execute(&mut conn)
                .await
                .map_err(|e| e.to_string())?;
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
            let mut conn = opts.connect().await.map_err(|e| e.to_string())?;
            sqlx::query(&query)
                .execute(&mut conn)
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
                        let _ = sqlx::query(&comment_query).execute(&mut conn).await;
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
    config: ConnectionConfig,
    query: String,
) -> Result<Vec<HashMap<String, Value>>, String> {
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

            let mut conn = opts.connect().await.map_err(|e| e.to_string())?;

            // Simple approach: fetch all as generic rows and convert to JSON map
            // Note: sqlx generic query mapping is tricky without knowing types beforehand.
            // For a simple manager, we might need a more dynamic approach or stringify results.
            // Using sqlx::Any or distinct handling. Here we stick to specific implementation details.

            // MySQL specific dynamic row handling
            let rows = sqlx::query(&query)
                .fetch_all(&mut conn)
                .await
                .map_err(|e| e.to_string())?;
            let mut results = Vec::new();

            for row in rows {
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
                            // Try i64 first (handles TINYINT(1), INT(11), etc.)
                            if let Ok(v) = row.try_get::<Option<i64>, _>(col.ordinal()) {
                                json!(v)
                            } else if let Ok(v) = row.try_get::<Option<u64>, _>(col.ordinal()) {
                                json!(v)
                            } else if let Ok(v) = row.try_get::<Option<i32>, _>(col.ordinal()) {
                                json!(v)
                            } else if let Ok(v) = row.try_get::<Option<i8>, _>(col.ordinal()) {
                                json!(v)
                            } else {
                                // Fallback to string if strictly needed or overflow
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
                            // BIT often comes as bytes or int depending on driver/length
                            // Try u64 first
                            if let Ok(v) = row.try_get::<Option<u64>, _>(col.ordinal()) {
                                json!(v)
                            } else {
                                // Try bytes
                                match row.try_get::<Option<Vec<u8>>, _>(col.ordinal()) {
                                    Ok(Some(v)) => {
                                        // Simple binary string like "0x..."
                                        let hex: String =
                                            v.iter().map(|b| format!("{:02X}", b)).collect();
                                        json!(format!("0x{}", hex))
                                    }
                                    Ok(None) => Value::Null,
                                    Err(_) => Value::Null,
                                }
                            }
                        }
                        "JSON" => {
                            // Requires sqlx json feature
                            match row.try_get::<Option<serde_json::Value>, _>(col.ordinal()) {
                                Ok(v) => json!(v),
                                Err(_) => Value::Null,
                            }
                        }
                        "TIMESTAMP" | "DATETIME" => {
                            match row.try_get::<Option<chrono::NaiveDateTime>, _>(col.ordinal()) {
                                Ok(Some(v)) => json!(v.to_string()),
                                Ok(None) => Value::Null,
                                Err(_) => {
                                    // Fallback if it's maybe a string already?
                                    match row.try_get::<Option<String>, _>(col.ordinal()) {
                                        Ok(v) => json!(v),
                                        Err(_) => Value::Null,
                                    }
                                }
                            }
                        }
                        "DATE" => {
                            match row.try_get::<Option<chrono::NaiveDate>, _>(col.ordinal()) {
                                Ok(Some(v)) => json!(v.to_string()),
                                Ok(None) => Value::Null,
                                Err(_) => Value::Null,
                            }
                        }
                        "TIME" => {
                            match row.try_get::<Option<chrono::NaiveTime>, _>(col.ordinal()) {
                                Ok(Some(v)) => json!(v.to_string()),
                                Ok(None) => Value::Null,
                                Err(_) => Value::Null,
                            }
                        }
                        "YEAR" => {
                            match row.try_get::<Option<i32>, _>(col.ordinal()) {
                                Ok(Some(v)) => json!(v),
                                Ok(None) => Value::Null, // Or string
                                Err(_) => match row.try_get::<Option<String>, _>(col.ordinal()) {
                                    Ok(v) => json!(v),
                                    Err(_) => Value::Null,
                                },
                            }
                        }
                        _ if type_name.to_uppercase().contains("BINARY")
                            || type_name.to_uppercase().contains("BLOB")
                            || type_name.to_uppercase().contains("BYTEA") =>
                        {
                            // Handle binary types: VARBINARY, BINARY, BLOB, TINYBLOB, MEDIUMBLOB, LONGBLOB, BYTEA (PG)
                            match row.try_get::<Option<Vec<u8>>, _>(col.ordinal()) {
                                Ok(Some(v)) => {
                                    // Display as hex, truncated for readability
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
                        _ => {
                            // Fallback to string for TEXT, VARCHAR, etc.
                            match row.try_get::<Option<String>, _>(col.ordinal()) {
                                Ok(v) => json!(v),
                                Err(_) => {
                                    // Fallback to generic bytes debug view
                                    match row.try_get::<Option<Vec<u8>>, _>(col.ordinal()) {
                                        Ok(Some(v)) => {
                                            let hex: String = v
                                                .iter()
                                                .take(16)
                                                .map(|b| format!("{:02X}", b))
                                                .collect();
                                            let suffix = if v.len() > 16 { "..." } else { "" };
                                            json!(format!("[BLOB: 0x{}{}]", hex, suffix))
                                        }
                                        Ok(None) => Value::Null,
                                        Err(_) => Value::Null,
                                    }
                                }
                            }
                        }
                    };
                    map.insert(name.to_string(), value);
                }
                results.push(map);
            }
            Ok(results)
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

            let mut conn = opts.connect().await.map_err(|e| e.to_string())?;

            let rows = sqlx::query(&query)
                .fetch_all(&mut conn)
                .await
                .map_err(|e| e.to_string())?;
            let mut results = Vec::new();

            for row in rows {
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
                            // Use chrono::NaiveDateTime or DateTime<Utc>
                            // sqlx maps TIMESTAMP -> NaiveDateTime, TIMESTAMPTZ -> DateTime<Utc> or DateTime<Local>
                            // We try generic string first, if that fails, we try specific types
                            if let Ok(v) = row.try_get::<Option<String>, _>(col.ordinal()) {
                                json!(v)
                            } else if let Ok(v) =
                                row.try_get::<Option<chrono::NaiveDateTime>, _>(col.ordinal())
                            {
                                json!(v.map(|d| d.to_string()))
                            } else if let Ok(v) = row
                                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(col.ordinal())
                            {
                                json!(v.map(|d| d.to_string()))
                            } else {
                                Value::Null
                            }
                        }
                        "DATE" => {
                            if let Ok(v) = row.try_get::<Option<String>, _>(col.ordinal()) {
                                json!(v)
                            } else if let Ok(v) =
                                row.try_get::<Option<chrono::NaiveDate>, _>(col.ordinal())
                            {
                                json!(v.map(|d| d.to_string()))
                            } else {
                                Value::Null
                            }
                        }
                        "TIME" | "TIMETZ" => {
                            if let Ok(v) = row.try_get::<Option<String>, _>(col.ordinal()) {
                                json!(v)
                            } else if let Ok(v) =
                                row.try_get::<Option<chrono::NaiveTime>, _>(col.ordinal())
                            {
                                json!(v.map(|d| d.to_string()))
                            } else {
                                Value::Null
                            }
                        }
                        "JSON" | "JSONB" => {
                            if let Ok(v) =
                                row.try_get::<Option<serde_json::Value>, _>(col.ordinal())
                            {
                                json!(v)
                            } else if let Ok(v) = row.try_get::<Option<String>, _>(col.ordinal()) {
                                json!(v)
                            } else {
                                Value::Null
                            }
                        }
                        "BYTEA" | "VARBINARY" | "BINARY" | "BLOB" => {
                            // Handle binary types explicitly for Postgres/Generic
                            match row.try_get::<Option<Vec<u8>>, _>(col.ordinal()) {
                                Ok(Some(v)) => {
                                    // Display as hex, truncated for readability
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
                        _ => {
                            // PG also calls text TEXT, varchar VARCHAR
                            match row.try_get::<Option<String>, _>(col.ordinal()) {
                                Ok(v) => json!(v),
                                Err(_) => {
                                    // Fallback for unknown types (UUID, etc) usually behave as strings in simple fetch if cast,
                                    // but try_get::<String> might fail if sqlx strictly maps them.
                                    // Try simple ToString if possible or empty.
                                    // For now, let's try to get as ANY string representation or NULL

                                    // Second fallback: try as binary blob
                                    match row.try_get::<Option<Vec<u8>>, _>(col.ordinal()) {
                                        Ok(Some(v)) => {
                                            let hex: String = v
                                                .iter()
                                                .take(16)
                                                .map(|b| format!("{:02X}", b))
                                                .collect();
                                            let suffix = if v.len() > 16 { "..." } else { "" };
                                            json!(format!("[BLOB: 0x{}{}]", hex, suffix))
                                        }
                                        _ => Value::Null,
                                    }
                                }
                            }
                        }
                    };
                    map.insert(name.to_string(), value);
                }
                results.push(map);
            }
            Ok(results)
        }
        "redis" => {
            let url = format!("redis://{}:{}/", config.host, config.port);
            let client = redis::Client::open(url).map_err(|e| e.to_string())?;
            let mut con = client
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| e.to_string())?;

            // Password auth if needed
            if let Some(pass) = &config.password {
                if !pass.is_empty() {
                    let _: () = redis::cmd("AUTH")
                        .arg(pass)
                        .query_async(&mut con)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }

            // Select DB if provided (parse "db0 (15)", "db0", "0", etc.)
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

            // Helper to stringify Redis Value
            fn redis_value_to_string(v: redis::Value) -> String {
                match v {
                    redis::Value::Nil => "(nil)".to_string(),
                    redis::Value::Okay => "OK".to_string(),
                    _ => {
                        // Use FromRedisValue to convert complex types (Data/Bulk) to String
                        // This handles formatting logic internally
                        let s: redis::RedisResult<String> =
                            redis::FromRedisValue::from_redis_value(&v);
                        s.unwrap_or_else(|_| format!("{:?}", v))
                    }
                }
            }

            // Split query into lines and execute
            for line in query.lines() {
                let trimmed = line.trim();
                // Skip empty lines or comments
                if trimmed.is_empty() || trimmed.starts_with("#") || trimmed.starts_with("--") {
                    continue;
                }

                // Simple parser for quotes
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

                // Execute
                let result_val: Result<redis::Value, _> = cmd.query_async(&mut con).await;

                let result_str = match result_val {
                    Ok(v) => redis_value_to_string(v),
                    Err(e) => format!("Error: {}", e),
                };

                let mut map = HashMap::new();
                map.insert("command".to_string(), json!(trimmed));
                map.insert("result".to_string(), json!(result_str));
                results.push(map);
            }

            Ok(results)
        }
        _ => Err("Unsupported database type".to_string()),
    }
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
    config: ConnectionConfig,
    key: String,
    database: Option<String>,
) -> Result<RedisKeyInfo, String> {
    let url = format!("redis://{}:{}/", config.host, config.port);
    let client = redis::Client::open(url).map_err(|e| e.to_string())?;
    let mut con = client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| e.to_string())?;

    // Auth if needed
    if let Some(pass) = &config.password {
        if !pass.is_empty() {
            let _: () = redis::cmd("AUTH")
                .arg(pass)
                .query_async(&mut con)
                .await
                .map_err(|e| e.to_string())?;
        }
    }

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
        .invoke_handler(tauri::generate_handler![
            test_connection,
            save_connection,
            get_connections,
            delete_connection,
            get_tables,
            get_databases,
            get_columns,
            execute_query,
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
