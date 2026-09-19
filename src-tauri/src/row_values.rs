//! Preserve database values, and fail explicitly rather than turning decode errors into NULL.
use crate::{escape_sql_string, sql_literal};
use serde_json::{json, Value};
use sqlx::{Column, Row, TypeInfo, ValueRef};
use sqlx::mysql::MySqlRow;
use sqlx::postgres::PgRow;
use std::collections::HashMap;
pub type Record = HashMap<String, Value>;
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
fn signed(value: i64) -> Value {
    if !(-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&value) { json!(value.to_string()) } else { json!(value) }
}
fn unsigned(value: u64) -> Value {
    if value > MAX_SAFE_INTEGER as u64 { json!(value.to_string()) } else { json!(value) }
}
fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02X}")).collect() }
fn binary_type(name: &str) -> bool {
    name.contains("BINARY") || name.contains("BLOB") || name == "BYTEA" || name == "BIT"
}
fn decode_error(name: &str, kind: &str, error: sqlx::Error) -> String {
    format!("Cannot decode column {name:?} ({kind}); refusing a lossy result: {error}")
}

pub fn mysql_row_to_json_map(row: &MySqlRow) -> Result<Record, String> {
    let mut map = HashMap::new();
    for col in row.columns() {
        let i = col.ordinal(); let name = col.name(); let kind = col.type_info().name();
        if map.contains_key(name) { return Err(format!("Duplicate result column {name:?}; use distinct SQL aliases")); }
        if row.try_get_raw(i).map_err(|e| e.to_string())?.is_null() { map.insert(name.into(), Value::Null); continue; }
        let decoded: Result<Value, sqlx::Error> = match kind {
            "BOOLEAN" | "BOOL" => row.try_get::<bool, _>(i).map(|v| json!(v)),
            _ if kind.starts_with("TINYINT") || kind.starts_with("SMALLINT") || kind.starts_with("MEDIUMINT")
                || kind.starts_with("INT") || kind.starts_with("BIGINT") =>
                row.try_get::<i64, _>(i).map(signed).or_else(|_| row.try_get::<u64, _>(i).map(unsigned)),
            "DECIMAL" | "NUMERIC" | "NEWDECIMAL" => row.try_get::<sqlx::types::BigDecimal, _>(i).map(|v| json!(v.to_string())),
            "FLOAT" => row.try_get::<f32, _>(i).map(|v| json!(v)),
            "DOUBLE" | "REAL" => row.try_get::<f64, _>(i).map(|v| json!(v)),
            "JSON" => row.try_get::<Value, _>(i),
            "TIMESTAMP" | "DATETIME" => row.try_get::<chrono::NaiveDateTime, _>(i).map(|v| json!(v.to_string())),
            "DATE" => row.try_get::<chrono::NaiveDate, _>(i).map(|v| json!(v.to_string())),
            "TIME" => row.try_get::<chrono::NaiveTime, _>(i).map(|v| json!(v.to_string())),
            "YEAR" => row.try_get::<u16, _>(i).map(|v| json!(v)),
            _ if binary_type(kind) => row.try_get::<Vec<u8>, _>(i).map(|v| json!(format!("0x{}", hex(&v)))),
            _ => row.try_get::<String, _>(i).map(Value::String),
        };
        let value = decoded.map_err(|e| decode_error(name, kind, e))?;
        if value.is_null() && kind != "JSON" { return Err(format!("Non-finite/unsupported value in column {name:?}; refusing to return NULL")); }
        if map.insert(name.into(), value).is_some() { return Err(format!("Duplicate result column {name:?}; use distinct SQL aliases")); }
    }
    Ok(map)
}

pub fn pg_row_to_json_map(row: &PgRow) -> Result<Record, String> {
    let mut map = HashMap::new();
    for col in row.columns() {
        let i = col.ordinal(); let name = col.name(); let kind = col.type_info().name();
        if map.contains_key(name) { return Err(format!("Duplicate result column {name:?}; use distinct SQL aliases")); }
        if row.try_get_raw(i).map_err(|e| e.to_string())?.is_null() { map.insert(name.into(), Value::Null); continue; }
        let decoded: Result<Value, sqlx::Error> = match kind {
            "BOOL" => row.try_get::<bool, _>(i).map(|v| json!(v)),
            "INT2" => row.try_get::<i16, _>(i).map(|v| signed(v as i64)),
            "INT4" => row.try_get::<i32, _>(i).map(|v| signed(v as i64)),
            "INT8" => row.try_get::<i64, _>(i).map(signed),
            "FLOAT4" => row.try_get::<f32, _>(i).map(|v| json!(v)),
            "FLOAT8" => row.try_get::<f64, _>(i).map(|v| json!(v)),
            "NUMERIC" => row.try_get::<sqlx::types::BigDecimal, _>(i).map(|v| json!(v.to_string())),
            "UUID" => row.try_get::<uuid::Uuid, _>(i).map(|v| json!(v.to_string())),
            "TIMESTAMP" => row.try_get::<chrono::NaiveDateTime, _>(i).map(|v| json!(v.to_string())),
            "TIMESTAMPTZ" => row.try_get::<chrono::DateTime<chrono::Utc>, _>(i).map(|v| json!(v.to_rfc3339())),
            "DATE" => row.try_get::<chrono::NaiveDate, _>(i).map(|v| json!(v.to_string())),
            "TIME" => row.try_get::<chrono::NaiveTime, _>(i).map(|v| json!(v.to_string())),
            "JSON" | "JSONB" => row.try_get::<Value, _>(i),
            "BYTEA" => row.try_get::<Vec<u8>, _>(i).map(|v| json!(format!("0x{}", hex(&v)))),
            _ => row.try_get::<String, _>(i).map(Value::String),
        };
        let value = decoded.map_err(|e| decode_error(name, kind, e))?;
        if value.is_null() && kind != "JSON" && kind != "JSONB" { return Err(format!("Non-finite/unsupported value in column {name:?}; refusing to return NULL")); }
        if map.insert(name.into(), value).is_some() { return Err(format!("Duplicate result column {name:?}; use distinct SQL aliases")); }
    }
    Ok(map)
}

pub fn mysql_export_values(row: &MySqlRow, columns: &[String]) -> Result<String, String> {
    let values = mysql_row_to_json_map(row)?;
    columns.iter().map(|name| {
        let col = row.columns().iter().find(|c| c.name() == name).ok_or("Missing export column")?;
        if row.try_get_raw(col.ordinal()).map_err(|e| e.to_string())?.is_null() { return Ok("NULL".into()); }
        let kind = col.type_info().name();
        if binary_type(kind) {
            let bytes: Vec<u8> = row.try_get(col.ordinal()).map_err(|e| e.to_string())?;
            return Ok(format!("X'{}'", hex(&bytes)));
        }
        let value = values.get(name).ok_or("Missing export value")?;
        if kind == "JSON" { return Ok(sql_literal(Some(&Value::String(value.to_string())), "mysql")); }
        Ok(sql_literal(Some(value), "mysql"))
    }).collect::<Result<Vec<_>, String>>().map(|v| v.join(", "))
}

pub fn pg_export_values(row: &PgRow, columns: &[String]) -> Result<String, String> {
    let values = pg_row_to_json_map(row)?;
    columns.iter().map(|name| {
        let col = row.columns().iter().find(|c| c.name() == name).ok_or("Missing export column")?;
        if row.try_get_raw(col.ordinal()).map_err(|e| e.to_string())?.is_null() { return Ok("NULL".into()); }
        let kind = col.type_info().name();
        if kind == "BYTEA" {
            let bytes: Vec<u8> = row.try_get(col.ordinal()).map_err(|e| e.to_string())?;
            return Ok(format!("decode('{}', 'hex')", hex(&bytes)));
        }
        let value = values.get(name).ok_or("Missing export value")?;
        if kind == "JSON" || kind == "JSONB" { return Ok(format!("'{}'", escape_sql_string(&value.to_string(), "postgresql"))); }
        Ok(sql_literal(Some(value), "postgresql"))
    }).collect::<Result<Vec<_>, String>>().map(|v| v.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integers_outside_javascript_exact_range_are_strings() {
        assert_eq!(signed(42), json!(42)); assert_eq!(signed(i64::MAX), json!(i64::MAX.to_string()));
        assert_eq!(signed(i64::MIN), json!(i64::MIN.to_string())); assert_eq!(unsigned(u64::MAX), json!(u64::MAX.to_string()));
    }
    #[tokio::test]
    #[ignore = "requires the disposable CI PostgreSQL service"]
    async fn integration_postgres_values_and_binary_dump_round_trip() {
        let url = std::env::var("RECCH_TEST_PG_URL").expect("CI fixture URL required");
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let row = sqlx::query("SELECT 7::smallint AS small, 8::int AS ordinary, 9223372036854775807::bigint AS large, 12345678901234567890.123456789::numeric AS amount, decode(repeat('00FF27', 50),'hex') AS bytes, 'null'::jsonb AS document, NULL::text AS absent, '12345678-1234-1234-1234-123456789abc'::uuid AS uuid").fetch_one(&pool).await.unwrap();
        let values = pg_row_to_json_map(&row).unwrap();
        assert_eq!(values["small"], json!(7)); assert_eq!(values["ordinary"], json!(8));
        assert_eq!(values["large"], json!("9223372036854775807"));
        assert_eq!(values["amount"], json!("12345678901234567890.123456789"));
        assert_eq!(values["bytes"].as_str().unwrap().len(), 302);
        let expression = pg_export_values(&row, &["bytes".into(), "document".into(), "absent".into()]).unwrap();
        let restored = sqlx::query(&format!("SELECT {expression}")).fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<Vec<u8>, _>("bytes"), restored.get::<Vec<u8>, _>(0));
        assert_eq!(restored.get::<String, _>(1), "null"); assert!(restored.try_get_raw(2).unwrap().is_null());
        let unsupported = sqlx::query("SELECT ARRAY[1,2] AS unsupported").fetch_one(&pool).await.unwrap();
        assert!(pg_row_to_json_map(&unsupported).is_err());
    }
    #[tokio::test]
    #[ignore = "requires the disposable CI MySQL service"]
    async fn integration_mysql_values_and_binary_dump_round_trip() {
        let url = std::env::var("RECCH_TEST_MYSQL_URL").expect("CI fixture URL required");
        let pool = sqlx::MySqlPool::connect(&url).await.unwrap();
        let row = sqlx::query("SELECT CAST(18446744073709551615 AS UNSIGNED) AS large, CAST(12345678901234567890.123456789 AS DECIMAL(30,9)) AS amount, UNHEX(REPEAT('00FF27',50)) AS bytes, CAST('null' AS JSON) AS document, NULL AS absent").fetch_one(&pool).await.unwrap();
        let values = mysql_row_to_json_map(&row).unwrap();
        assert_eq!(values["large"], json!("18446744073709551615")); assert_eq!(values["amount"], json!("12345678901234567890.123456789"));
        let expression = mysql_export_values(&row, &["bytes".into(), "document".into(), "absent".into()]).unwrap();
        let restored = sqlx::query(&format!("SELECT {expression}")).fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<Vec<u8>, _>("bytes"), restored.get::<Vec<u8>, _>(0));
        assert_eq!(restored.get::<String, _>(1), "null"); assert!(restored.try_get_raw(2).unwrap().is_null());
    }
}
