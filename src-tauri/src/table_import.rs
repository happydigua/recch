use crate::{ConnectionConfig, pool_manager::PoolManager, quote_mysql_identifier, quote_pg_identifier, sql_literal};
use serde_json::Value;
use std::collections::HashMap;
type Record = HashMap<String, Value>;

fn statements(table: &str, rows: &[Record], dialect: &str) -> Result<Vec<String>, String> {
    if table.is_empty() || table.contains('\0') { return Err("Invalid table name".into()); }
    if rows.len() > 10_000 { return Err("Import supports at most 10000 rows per transaction".into()); }
    let quote = if dialect == "mysql" { quote_mysql_identifier } else { quote_pg_identifier };
    let mut total_bytes = 0usize;
    rows.iter().map(|row| {
        let mut names: Vec<_> = row.keys().collect(); names.sort();
        if names.iter().any(|name| name.is_empty() || name.contains('\0')) { return Err("Invalid column name".into()); }
        let query = if names.is_empty() {
            if dialect == "mysql" { format!("INSERT INTO {} () VALUES ()", quote(table)) }
            else { format!("INSERT INTO {} DEFAULT VALUES", quote(table)) }
        } else {
            format!("INSERT INTO {} ({}) VALUES ({})", quote(table),
                names.iter().map(|n| quote(n)).collect::<Vec<_>>().join(", "),
                names.iter().map(|n| sql_literal(row.get(*n), dialect)).collect::<Vec<_>>().join(", "))
        };
        total_bytes = total_bytes.saturating_add(query.len());
        if total_bytes > 32 * 1024 * 1024 { return Err("Import transaction exceeds 32 MiB".into()); }
        Ok(query)
    }).collect()
}

pub async fn import_rows(manager: &PoolManager, config: &ConnectionConfig, table: &str, rows: &[Record]) -> Result<u64, String> {
    let queries = statements(table, rows, &config.db_type)?;
    match config.db_type.as_str() {
        "mysql" => {
            let pool = manager.get_mysql_pool(config, config.database.as_deref()).await?;
            let engine: Option<String> = sqlx::query_scalar::<_, Option<String>>("SELECT ENGINE FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?")
                .bind(table).fetch_optional(&pool).await.map_err(|e| e.to_string())?.flatten();
            if !engine.as_deref().map(|e| e.eq_ignore_ascii_case("InnoDB")).unwrap_or(false) {
                return Err("Atomic MySQL row import requires an InnoDB base table".into());
            }
            let mut transaction = pool.begin().await.map_err(|e| e.to_string())?;
            for (index, query) in queries.iter().enumerate() {
                if let Err(e) = sqlx::query(query).execute(&mut *transaction).await {
                    let rollback = transaction.rollback().await;
                    return Err(match rollback {
                        Ok(()) => format!("Import row {} failed; entire transaction rolled back: {e}", index + 1),
                        Err(r) => format!("Import failed: {e}; rollback could not be confirmed: {r}"),
                    });
                }
            }
            transaction.commit().await.map_err(|e| format!("Commit could not be confirmed; inspect the database before retrying: {e}"))?;
        }
        "postgresql" => {
            let pool = manager.get_pg_pool(config, config.database.as_deref()).await?;
            let mut transaction = pool.begin().await.map_err(|e| e.to_string())?;
            for (index, query) in queries.iter().enumerate() {
                if let Err(e) = sqlx::query(query).execute(&mut *transaction).await {
                    let rollback = transaction.rollback().await;
                    return Err(match rollback {
                        Ok(()) => format!("Import row {} failed; entire transaction rolled back: {e}", index + 1),
                        Err(r) => format!("Import failed: {e}; rollback could not be confirmed: {r}"),
                    });
                }
            }
            transaction.commit().await.map_err(|e| format!("Commit could not be confirmed; inspect the database before retrying: {e}"))?;
        }
        _ => return Err("Row import supports MySQL and PostgreSQL only".into()),
    }
    Ok(rows.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn row_import_preserves_null_and_empty_string_and_quotes_identifiers() {
        let row = HashMap::from([("select".into(), json!("")), ("a`b".into(), Value::Null)]);
        let mysql = statements("order", &[row.clone()], "mysql").unwrap();
        assert!(mysql[0].contains("`order`")); assert!(mysql[0].contains("`a``b`")); assert!(mysql[0].contains("NULL"));
        assert!(statements("order", &[row], "postgresql").unwrap()[0].contains("\"select\""));
    }
    #[tokio::test]
    #[ignore = "requires disposable CI database services"]
    async fn integration_failed_second_row_rolls_back_first_for_both_databases() {
        let name = format!("recch_import_{}", uuid::Uuid::new_v4().simple());
        let manager = PoolManager::new();
        let rows = vec![HashMap::from([("id".into(), json!(1))]), HashMap::from([("id".into(), json!(1))])];
        let mut config = ConnectionConfig { id: "fixture".into(), name: "fixture".into(), db_type: "mysql".into(),
            host: "127.0.0.1".into(), port: 3306, username: Some("root".into()), password: Some("recch_ci_only".into()), database: Some("recch_test".into()) };
        let mysql = manager.get_mysql_pool(&config, None).await.unwrap();
        sqlx::query(&format!("CREATE TABLE `{name}` (id INT PRIMARY KEY) ENGINE=InnoDB")).execute(&mysql).await.unwrap();
        assert!(import_rows(&manager, &config, &name, &rows).await.unwrap_err().contains("rolled back"));
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM `{name}`")).fetch_one(&mysql).await.unwrap(); assert_eq!(count, 0);
        assert_eq!(import_rows(&manager, &config, &name, &rows[..1]).await.unwrap(), 1);
        sqlx::query(&format!("DROP TABLE `{name}`")).execute(&mysql).await.unwrap();
        config.db_type = "postgresql".into(); config.port = 5432; config.username = Some("postgres".into());
        let pg = manager.get_pg_pool(&config, None).await.unwrap();
        sqlx::query(&format!("CREATE TABLE \"{name}\" (id INT PRIMARY KEY)")).execute(&pg).await.unwrap();
        assert!(import_rows(&manager, &config, &name, &rows).await.unwrap_err().contains("rolled back"));
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM \"{name}\"")).fetch_one(&pg).await.unwrap(); assert_eq!(count, 0);
        assert_eq!(import_rows(&manager, &config, &name, &rows[..1]).await.unwrap(), 1);
        sqlx::query(&format!("DROP TABLE \"{name}\"")).execute(&pg).await.unwrap();
    }
}
