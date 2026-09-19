//! The visual editor supports conservative, explicit DDL. Advanced expressions
//! remain available in the SQL console rather than being guessed or silently lost.
use crate::{AlterOperation, ConnectionConfig, ColumnDef, pool_manager::PoolManager, quote_mysql_identifier, quote_pg_identifier, sql_literal};
use serde_json::Value;
use std::str::FromStr;

fn column_type(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_lowercase();
    let value = normalized.strip_suffix("[]").unwrap_or(&normalized);
    let value = value.strip_suffix(" unsigned").unwrap_or(value);
    let (name, args) = match value.split_once('(') {
        Some((name, tail)) => (name.trim(), Some(tail.strip_suffix(')').ok_or("Unsupported column type syntax")?)),
        None => (value, None),
    };
    let types = ["tinyint", "smallint", "mediumint", "int", "integer", "bigint", "int2", "int4", "int8", "serial", "bigserial", "smallserial",
        "varchar", "char", "character varying", "character", "text", "tinytext", "mediumtext", "longtext", "numeric", "decimal", "real", "float", "double", "double precision",
        "bool", "boolean", "date", "datetime", "timestamp", "timestamp without time zone", "timestamp with time zone", "timestamptz", "time", "time without time zone", "json", "jsonb", "uuid", "bytea", "binary", "varbinary", "blob", "tinyblob", "mediumblob", "longblob", "year", "bit"];
    if !types.contains(&name) || args.map(|a| a.is_empty() || !a.bytes().all(|c| c.is_ascii_digit() || c == b',' || c == b' ')).unwrap_or(false) {
        return Err("Unsupported visual-editor type; use the SQL console for custom types/expressions".into());
    }
    Ok(normalized)
}

fn default_sql(value: Option<&str>, dialect: &str) -> Result<Option<String>, String> {
    let Some(value) = value.map(str::trim).filter(|s| !s.is_empty()) else { return Ok(None); };
    let upper = value.to_uppercase();
    if ["NULL", "TRUE", "FALSE", "CURRENT_TIMESTAMP", "CURRENT_TIMESTAMP()", "CURRENT_DATE", "CURRENT_DATE()", "CURRENT_TIME", "CURRENT_TIME()", "NOW()"].contains(&upper.as_str()) {
        return Ok(Some(upper));
    }
    if sqlx::types::BigDecimal::from_str(value).is_ok() { return Ok(Some(value.to_string())); }
    if let Some(inner) = value.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        let mut chars = inner.chars(); let mut decoded = String::new();
        while let Some(c) = chars.next() {
            if c == '\\' { return Err("Use doubled quotes, not backslash escapes, in visual-editor defaults".into()); }
            if c == '\'' && chars.next() != Some('\'') { return Err("Invalid quoted default value".into()); }
            decoded.push(c);
        }
        return Ok(Some(sql_literal(Some(&Value::String(decoded)), dialect)));
    }
    Err("Unsupported default expression; use the SQL console rather than losing the existing definition".into())
}

fn definition(col: &ColumnDef, dialect: &str) -> Result<String, String> {
    let quote = if dialect == "mysql" { quote_mysql_identifier } else { quote_pg_identifier };
    let mut result = format!("{} {} {}", quote(&col.name), column_type(&col.type_name)?,
        if col.is_nullable == Some(false) || col.is_pk { "NOT NULL" } else { "NULL" });
    if let Some(default) = default_sql(col.default_value.as_deref(), dialect)? { result.push_str(&format!(" DEFAULT {default}")); }
    Ok(result)
}

pub fn queries(table: &str, operation: &AlterOperation, dialect: &str) -> Result<Vec<String>, String> {
    if table.is_empty() || table.contains('\0') { return Err("Invalid table name".into()); }
    let quote = if dialect == "mysql" { quote_mysql_identifier } else { quote_pg_identifier };
    let table = if dialect == "mysql" { quote(table) } else { format!("\"public\".{}", quote(table)) };
    let mut result = Vec::new();
    match operation.op_type.as_str() {
        "add" | "modify" => {
            let col = operation.column_def.as_ref().ok_or("Missing column definition")?;
            if col.name.is_empty() || col.name.contains('\0') { return Err("Invalid column name".into()); }
            let old = operation.column_name.as_deref().unwrap_or(&col.name);
            let comment = col.comment.as_ref().map(|s| sql_literal(Some(&Value::String(s.clone())), dialect)).unwrap_or("NULL".into());
            if operation.op_type == "add" {
                let pk = if col.is_pk { " PRIMARY KEY" } else { "" };
                let suffix = if dialect == "mysql" { format!(" COMMENT {}", col.comment.as_ref().map(|s| sql_literal(Some(&Value::String(s.clone())), dialect)).unwrap_or("''".into())) } else { String::new() };
                result.push(format!("ALTER TABLE {table} ADD COLUMN {}{pk}{suffix}", definition(col, dialect)?));
            } else if dialect == "mysql" {
                result.push(format!("ALTER TABLE {table} CHANGE COLUMN {} {} COMMENT {}", quote(old), definition(col, dialect)?,
                    col.comment.as_ref().map(|s| sql_literal(Some(&Value::String(s.clone())), dialect)).unwrap_or("''".into())));
            } else {
                result.push(format!("ALTER TABLE {table} ALTER COLUMN {} TYPE {}", quote(old), column_type(&col.type_name)?));
                result.push(format!("ALTER TABLE {table} ALTER COLUMN {} {} NOT NULL", quote(old), if col.is_nullable == Some(false) { "SET" } else { "DROP" }));
                result.push(match default_sql(col.default_value.as_deref(), dialect)? {
                    Some(default) => format!("ALTER TABLE {table} ALTER COLUMN {} SET DEFAULT {default}", quote(old)),
                    None => format!("ALTER TABLE {table} ALTER COLUMN {} DROP DEFAULT", quote(old)),
                });
                if old != col.name { result.push(format!("ALTER TABLE {table} RENAME COLUMN {} TO {}", quote(old), quote(&col.name))); }
            }
            if dialect == "postgresql" { result.push(format!("COMMENT ON COLUMN {table}.{} IS {comment}", quote(&col.name))); }
        }
        "drop" => result.push(format!("ALTER TABLE {table} DROP COLUMN {}", quote(operation.column_name.as_deref().ok_or("Missing column name")?))),
        "rename" => result.push(format!("ALTER TABLE {table} RENAME COLUMN {} TO {}", quote(operation.column_name.as_deref().ok_or("Missing column name")?), quote(operation.new_name.as_deref().ok_or("Missing new name")?))),
        "add_index" => {
            let index = operation.index_def.as_ref().ok_or("Missing index definition")?;
            if index.name.is_empty() || index.columns.is_empty() { return Err("Index name and columns are required".into()); }
            result.push(format!("CREATE {} INDEX {} ON {table} ({})", if index.is_unique { "UNIQUE" } else { "" }, quote(&index.name), index.columns.iter().map(|c| quote(c)).collect::<Vec<_>>().join(", ")));
        }
        "drop_index" => {
            let name = operation.index_name.as_deref().ok_or("Missing index name")?;
            if dialect == "mysql" && name.eq_ignore_ascii_case("PRIMARY") { return Err("Primary-key indexes cannot be dropped here".into()); }
            result.push(if dialect == "mysql" { format!("DROP INDEX {} ON {table}", quote(name)) } else { format!("DROP INDEX \"public\".{}", quote(name)) });
        }
        _ => return Err("Unsupported schema operation".into()),
    }
    Ok(result)
}

pub async fn alter(manager: &PoolManager, config: &ConnectionConfig, table: &str, operation: &AlterOperation) -> Result<(), String> {
    let mut statements = queries(table, operation, &config.db_type)?;
    match config.db_type.as_str() {
        "mysql" => {
            let pool = manager.get_mysql_pool(config, None).await?;
            if operation.op_type == "modify" {
                let old = operation.column_name.as_deref().ok_or("Missing original column name")?;
                let (extra, collation): (String, Option<String>) = sqlx::query_as("SELECT EXTRA, COLLATION_NAME FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? AND COLUMN_NAME = ?")
                    .bind(table).bind(old).fetch_one(&pool).await.map_err(|e| e.to_string())?;
                if !extra.is_empty() && extra != "DEFAULT_GENERATED" { return Err("This column has AUTO_INCREMENT, generated, or ON UPDATE attributes; edit it in the SQL console to preserve them".into()); }
                if let Some(collation) = collation {
                    let col = operation.column_def.as_ref().ok_or("Missing column definition")?;
                    let type_sql = column_type(&col.type_name)?;
                    if ["char", "text"].iter().any(|name| type_sql.contains(name)) {
                        statements[0] = statements[0].replacen(&format!("{} {}", quote_mysql_identifier(&col.name), type_sql),
                            &format!("{} {} COLLATE {}", quote_mysql_identifier(&col.name), type_sql, quote_mysql_identifier(&collation)), 1);
                    }
                }
            }
            for statement in statements { sqlx::query(&statement).execute(&pool).await.map_err(|e| e.to_string())?; }
        }
        "postgresql" => {
            let pool = manager.get_pg_pool(config, None).await?;
            let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
            if operation.op_type == "drop_index" {
                let primary: bool = sqlx::query_scalar("SELECT ix.indisprimary FROM pg_index ix JOIN pg_class i ON i.oid=ix.indexrelid JOIN pg_class t ON t.oid=ix.indrelid JOIN pg_namespace n ON n.oid=t.relnamespace WHERE n.nspname='public' AND t.relname=$1 AND i.relname=$2")
                    .bind(table).bind(operation.index_name.as_deref()).fetch_one(&mut *tx).await.map_err(|e| e.to_string())?;
                if primary { return Err("Primary-key indexes cannot be dropped here".into()); }
            }
            for statement in statements { sqlx::query(&statement).execute(&mut *tx).await.map_err(|e| e.to_string())?; }
            tx.commit().await.map_err(|e| e.to_string())?;
        }
        _ => return Err("Schema editing supports SQL databases only".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn operation() -> AlterOperation {
        AlterOperation { op_type: "modify".into(), column_name: Some("old\" name".into()), new_name: None,
            column_def: Some(ColumnDef { name: "new name".into(), type_name: "VARCHAR(40)".into(), is_pk: false,
                is_nullable: Some(false), default_value: Some("'it''s'".into()), comment: Some("owner's note".into()) }), index_def: None, index_name: None }
    }
    #[test]
    fn postgres_changes_include_nullability_default_comment_and_rename() {
        let sql = queries("order", &operation(), "postgresql").unwrap();
        assert_eq!(sql.len(), 5); assert!(sql[1].contains("SET NOT NULL")); assert!(sql[2].contains("SET DEFAULT"));
        assert!(sql[3].contains("RENAME COLUMN \"old\"\" name\" TO \"new name\"")); assert!(sql[4].contains("COMMENT ON COLUMN"));
    }
    #[test]
    fn mysql_rename_and_modify_are_one_statement() { assert_eq!(queries("order", &operation(), "mysql").unwrap().len(), 1); }
    #[test]
    fn unsupported_fragments_fail_before_any_ddl() {
        assert!(column_type("INT, DROP COLUMN secret").is_err()); assert!(column_type("VARCHAR(20); DROP TABLE t").is_err());
        assert!(default_sql(Some("0, DROP COLUMN secret"), "mysql").is_err());
        assert!(default_sql(Some("nextval('a')"), "postgresql").is_err());
        assert!(column_type("decimal(30,9)").is_ok());
    }
}
