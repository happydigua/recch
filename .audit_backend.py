from pathlib import Path
import re
import subprocess

root = Path('.')
def replace(text, old, new, count=1):
    assert text.count(old) == count, (old[:100], text.count(old), count)
    return text.replace(old, new)
def section(text, start, end, transform):
    a = text.index(start); b = text.index(end, a)
    return text[:a] + transform(text[a:b]) + text[b:]

path = root / 'src-tauri/src/lib.rs'
assert subprocess.check_output(['git', 'hash-object', str(path)], text=True).strip() == '26c62cc895477c067a47708cd53ec120e8da3083'
s = path.read_text()
s = replace(s, 'mod pool_manager;', 'mod pool_manager;\nmod storage;\nmod redis_support;\nmod row_values;\nmod table_import;\nmod schema_edits;\nuse row_values::{mysql_row_to_json_map, pg_row_to_json_map, mysql_export_values, pg_export_values};')
s = replace(s, '#[derive(Debug, Serialize, Deserialize, Clone)]\npub struct ConnectionConfig', '#[derive(Serialize, Deserialize, Clone)]\npub struct ConnectionConfig')
s = section(s, 'fn mysql_row_to_json_map(', 'async fn execute_query_inner(', lambda _: '')
s = replace(s, 'use sqlx::mysql::MySqlRow;\n', '')
s = replace(s, 'use sqlx::postgres::PgRow;\n', '')
s = replace(s, '        .trim()\n        .to_string();', '        .to_string();')
s = replace(s, 'async fn start_task(&self, task_id: &str) -> Arc<AtomicBool>', 'async fn start_task(&self, task_id: &str) -> Result<Arc<AtomicBool>, String>')
s = replace(s, '        tasks.insert(task_id.to_string(), cancel_flag.clone());\n        cancel_flag', '        if task_id.is_empty() || tasks.contains_key(task_id) { return Err("Duplicate or empty export task ID".into()); }\n        tasks.insert(task_id.to_string(), cancel_flag.clone());\n        Ok(cancel_flag)')
s = replace(s, 'export_task_manager.start_task(&task_id).await;', 'export_task_manager.start_task(&task_id).await?;')
s = section(s, 'fn sql_literal(', 'fn normalize_pg_column_definition(', lambda _: r'''fn sql_literal(value: Option<&Value>, db_type: &str) -> String {
    match value {
        None | Some(Value::Null) => "NULL".into(),
        Some(Value::Bool(v)) => if db_type == "mysql" { if *v { "1".into() } else { "0".into() } } else { v.to_string().to_uppercase() },
        Some(Value::Number(v)) => v.to_string(),
        Some(v) => {
            let text = match v { Value::String(v) => v.clone(), _ => v.to_string() };
            if db_type == "mysql" {
                let hex: String = text.as_bytes().iter().map(|b| format!("{b:02X}")).collect();
                format!("CONVERT(X'{hex}' USING utf8mb4)")
            } else {
                format!("E'{}'", text.replace('\\', "\\\\").replace('\'', "''"))
            }
        }
    }
}

''')

def query_inner(part):
    part = replace(part, '            let rows = sqlx::query(query)\n                .fetch_all(&pool)', '            let mut connection = pool.acquire().await.map_err(|e| e.to_string())?;\n            connection.close_on_drop();\n            let rows = sqlx::query(query)\n                .fetch_all(&mut *connection)', 2)
    part = replace(part, 'results.push(mysql_row_to_json_map(&row));', 'results.push(mysql_row_to_json_map(&row)?);')
    part = replace(part, 'results.push(pg_row_to_json_map(&row));', 'results.push(pg_row_to_json_map(&row)?);')
    a = part.index('            if let Some(db) = &config.database {'); b = part.index('            let mut results', a)
    part = part[:a] + part[b:]
    a = part.index('            for line in query.lines() {'); b = part.index('                let cmd_name', a)
    part = part[:a] + '            for args in redis_support::parse_commands(query)? {\n' + part[b:]
    part = replace(part, '                results.push(map);', '                let failed = map.contains_key("error");\n                results.push(map);\n                if failed { break; }')
    return part
s = section(s, 'async fn execute_query_inner(', '#[tauri::command]\nasync fn test_connection', query_inner)

def test_connection(part):
    a = part.index('        "redis" => {'); b = part.index('        _ => Err', a)
    return part[:a] + r'''        "redis" => {
            let mut con = redis_support::connect(&config).await?;
            let _: String = redis::cmd("PING").query_async(&mut con).await.map_err(|e| e.to_string())?;
            Ok("Redis Connection Successful!".into())
        }
''' + part[b:]
s = section(s, '#[tauri::command]\nasync fn test_connection', 'fn get_config_path', test_connection)
s = section(s, '#[tauri::command]\nfn save_connection', '#[tauri::command]\nasync fn get_databases', lambda _: r'''#[tauri::command]
async fn save_connection(pool_manager: tauri::State<'_, PoolManager>, app_handle: tauri::AppHandle, config: ConnectionConfig) -> Result<(), String> {
    let path = get_config_path(&app_handle)?;
    let invalidate = config.clone();
    storage::update_json(&path, |connections: &mut Vec<ConnectionConfig>| {
        if let Some(index) = connections.iter().position(|c| c.id == config.id) { connections[index] = config; }
        else { connections.push(config); }
    })?;
    pool_manager.remove_pool(&invalidate).await;
    Ok(())
}
#[tauri::command]
fn get_connections(app_handle: tauri::AppHandle) -> Result<Vec<ConnectionConfig>, String> {
    storage::read_json(&get_config_path(&app_handle)?)
}
#[tauri::command]
async fn delete_connection(pool_manager: tauri::State<'_, PoolManager>, app_handle: tauri::AppHandle, id: String) -> Result<(), String> {
    let path = get_config_path(&app_handle)?;
    let removed = storage::update_json(&path, |connections: &mut Vec<ConnectionConfig>| {
        let removed = connections.iter().find(|c| c.id == id).cloned();
        connections.retain(|c| c.id != id); removed
    })?;
    if let Some(config) = removed { pool_manager.remove_pool(&config).await; }
    Ok(())
}

''')

def redis_databases(part):
    a = part.index('        "redis" => {'); b = part.index('        _ => Err', a)
    return part[:a] + r'''        "redis" => {
            let selected = redis_support::selected_config(&config, Some("0"))?;
            let mut con = pool_manager.get_redis_conn(&selected).await?;
            let settings: Result<Vec<String>, _> = redis::cmd("CONFIG").arg("GET").arg("databases").query_async(&mut con).await;
            let count: usize = settings.ok().and_then(|v| v.get(1).and_then(|s| s.parse().ok())).unwrap_or(16);
            if count > 1024 { return Err("Redis browser supports up to 1024 databases".into()); }
            let mut databases = Vec::new();
            for index in 0..count {
                let select: Result<(), _> = redis::cmd("SELECT").arg(index).query_async(&mut con).await;
                if let Err(error) = select {
                    if error.to_string().contains("DB index is out of range") { break; }
                    return Err(error.to_string());
                }
                let size: i64 = redis::cmd("DBSIZE").query_async(&mut con).await.map_err(|e| e.to_string())?;
                databases.push(format!("db{index} ({size})"));
            }
            Ok(databases)
        }
''' + part[b:]
s = section(s, '#[tauri::command]\nasync fn get_databases', '#[tauri::command]\nasync fn get_tables', redis_databases)

def redis_tables(part):
    a = part.index('        "redis" => {'); b = part.index('            let tables = keys', a)
    return part[:a] + r'''        "redis" => {
            let selected = redis_support::selected_config(&config, database.as_deref())?;
            let mut con = pool_manager.get_redis_conn(&selected).await?;
            let keys = redis_support::scan_keys(&mut con).await?;

''' + part[b:]
s = section(s, '#[tauri::command]\nasync fn get_tables', '#[derive(Debug, Serialize, Deserialize, Clone)]\npub struct ColumnDef', redis_tables)

def columns(part):
    pg = part.index('        "postgresql" => {'); a = part.index('            let query = "', pg); b = part.index('            let rows: Vec<(', a)
    part = part[:a] + r'''            let query = "
                SELECT c.column_name, c.data_type,
                    EXISTS (SELECT 1 FROM information_schema.table_constraints tc
                      JOIN information_schema.key_column_usage kcu
                        ON tc.constraint_catalog=kcu.constraint_catalog AND tc.constraint_schema=kcu.constraint_schema
                        AND tc.constraint_name=kcu.constraint_name AND tc.table_name=kcu.table_name
                      WHERE tc.constraint_type='PRIMARY KEY' AND tc.table_schema=c.table_schema
                        AND tc.table_name=c.table_name AND kcu.column_name=c.column_name) AS is_pk,
                    c.is_nullable, c.column_default,
                    pg_catalog.col_description(format('%I.%I', c.table_schema, c.table_name)::regclass::oid, c.ordinal_position)
                FROM information_schema.columns c
                WHERE c.table_schema='public' AND c.table_name=$1 ORDER BY c.ordinal_position
            ";
''' + part[b:]
    a = part.index('        "redis" => {'); b = part.index('            // Get key type', a)
    return part[:a] + r'''        "redis" => {
            let selected = redis_support::selected_config(&config, database.as_deref())?;
            let mut con = pool_manager.get_redis_conn(&selected).await?;
''' + part[b:]
s = section(s, '#[tauri::command]\nasync fn get_columns', '#[tauri::command]\nasync fn get_indexes', columns)

def indexes(part):
    a = part.index('        "postgresql" => {'); b = part.index('        _ => Ok(Vec::new())', a)
    return part[:a] + r'''        "postgresql" => {
            let pool = pool_manager.get_pg_pool(&config, config.database.as_deref()).await?;
            let rows: Vec<(String, Vec<String>, bool, bool)> = sqlx::query_as("
                SELECT i.relname,
                  ARRAY(SELECT pg_get_indexdef(ix.indexrelid, k.position::int, true)
                    FROM unnest(ix.indkey) WITH ORDINALITY AS k(attnum, position) ORDER BY k.position),
                  ix.indisunique, ix.indisprimary
                FROM pg_index ix JOIN pg_class i ON i.oid=ix.indexrelid
                JOIN pg_class t ON t.oid=ix.indrelid JOIN pg_namespace n ON n.oid=t.relnamespace
                WHERE n.nspname='public' AND t.relname=$1 ORDER BY i.relname
            ").bind(&table).fetch_all(&pool).await.map_err(|e| e.to_string())?;
            Ok(rows.into_iter().map(|(name, columns, is_unique, is_pk)| IndexDef { name, columns, is_unique, is_pk, comment: None }).collect())
        }
''' + part[b:]
s = section(s, '#[tauri::command]\nasync fn get_indexes', '#[tauri::command]\nasync fn export_database_sql', indexes)

def export(part):
    part = replace(part, 'let file = fs::File::create(&output_path).map_err(|e| e.to_string())?;', 'let file = storage::AtomicFile::new(&output_path)?;')
    part = replace(part, '        match config.db_type.as_str() {', '        let write_result: Result<(), String> = match config.db_type.as_str() {')
    part = replace(part, '        }\n    }\n    .await;', '        };\n        write_result?;\n        ensure_not_cancelled(cancel_flag.as_ref())?;\n        writer.into_inner().map_err(|e| e.error().to_string())?.commit()\n    }\n    .await;')
    part = replace(part, '    if export_result.is_err() {\n        let _ = fs::remove_file(&output_path);\n    }\n', '')
    part = replace(part, 'writeln!(writer, "-- Database: {}", target_db)', 'writeln!(writer, "-- Database: {}", target_db.replace([\'\\r\', \'\\n\'], " "))', 2)
    part = replace(part, 'writeln!(writer, "-- Table: {}", table)', 'writeln!(writer, "-- Table: {}", table.replace([\'\\r\', \'\\n\'], " "))', 2)
    part, n = re.subn(r'let row_map = mysql_row_to_json_map\(&row\);\s*let values = columns\s*\.iter\(\)\s*\.map\(\|column\| sql_literal\(row_map.get\(column\), "mysql"\)\)\s*\.collect::<Vec<_>>\(\)\s*\.join\(", "\);', 'let values = mysql_export_values(&row, &columns)?;', part); assert n == 1
    part, n = re.subn(r'let row_map = pg_row_to_json_map\(&row\);\s*let values = ordered_columns\s*\.iter\(\)\s*\.map\(\|column\| sql_literal\(row_map.get\(column\), "postgresql"\)\)\s*\.collect::<Vec<_>>\(\)\s*\.join\(", "\);', 'let values = pg_export_values(&row, &ordered_columns)?;', part); assert n == 1
    # All export reads must use one repeatable-read snapshot, not random pool sessions.
    part = part.replace('.fetch_all(&pool)', '.fetch_all(&mut *source)').replace('.fetch_one(&pool)', '.fetch_one(&mut *source)').replace('.fetch_optional(&pool)', '.fetch_optional(&mut *source)').replace('.fetch(&pool)', '.fetch(&mut *source)')
    mysql = 'let pool = pool_manager.get_mysql_pool(&config, Some(&target_db)).await?;'
    part = replace(part, mysql, mysql + r'''
                let mut source = pool.acquire().await.map_err(|e| e.to_string())?;
                source.close_on_drop();
                raw_sql("SET SESSION sql_mode='NO_AUTO_VALUE_ON_ZERO'; SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ; START TRANSACTION WITH CONSISTENT SNAPSHOT;")
                    .execute(&mut *source).await.map_err(|e| e.to_string())?;
                let unsupported: i64 = sqlx::query_scalar("SELECT
                    (SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA=? AND (TABLE_TYPE<>'BASE TABLE' OR ENGINE<>'InnoDB')) +
                    (SELECT COUNT(*) FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA=?) +
                    (SELECT COUNT(*) FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA=?) +
                    (SELECT COUNT(*) FROM information_schema.EVENTS WHERE EVENT_SCHEMA=?) +
                    (SELECT COUNT(*) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND GENERATION_EXPRESSION<>'')")
                    .bind(&target_db).bind(&target_db).bind(&target_db).bind(&target_db).bind(&target_db)
                    .fetch_one(&mut *source).await.map_err(|e| e.to_string())?;
                if unsupported != 0 { return Err("This database contains non-InnoDB tables, views, generated columns, triggers, routines or events. Use mysqldump for a complete backup; the previous backup was kept.".into()); }
''')
    pg = 'let pool = pool_manager.get_pg_pool(&config, Some(&target_db)).await?;'
    part = replace(part, pg, pg + r'''
                let mut source = pool.acquire().await.map_err(|e| e.to_string())?;
                source.close_on_drop();
                raw_sql("BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY;")
                    .execute(&mut *source).await.map_err(|e| e.to_string())?;
                let unsupported: bool = sqlx::query_scalar("SELECT
                    EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_%' AND ((c.relkind IN ('r','p','v','m') AND (n.nspname<>'public' OR c.relkind<>'r')) OR c.relrowsecurity)) OR
                    EXISTS(SELECT 1 FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND (a.attidentity<>'' OR a.attgenerated<>'')) OR
                    EXISTS(SELECT 1 FROM pg_constraint c JOIN pg_namespace n ON n.oid=c.connamespace WHERE n.nspname='public' AND c.contype IN ('c','x')) OR
                    EXISTS(SELECT 1 FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND NOT t.tgisinternal) OR
                    EXISTS(SELECT 1 FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public')")
                    .fetch_one(&mut *source).await.map_err(|e| e.to_string())?;
                if unsupported { return Err("This database contains objects unsupported by the simple public-table exporter. Use pg_dump for a complete backup; the previous backup was kept.".into()); }
                let mut deferred_constraints = Vec::new();
''')
    part = replace(part, 'writeln!(writer, "SET FOREIGN_KEY_CHECKS=0;")', 'writeln!(writer, "SET @RECCH_OLD_SQL_MODE=@@SQL_MODE; SET SQL_MODE=\'NO_AUTO_VALUE_ON_ZERO\'; SET FOREIGN_KEY_CHECKS=0;")')
    part = replace(part, 'writeln!(writer, "SET FOREIGN_KEY_CHECKS=1;")', 'writeln!(writer, "SET FOREIGN_KEY_CHECKS=1; SET SQL_MODE=@RECCH_OLD_SQL_MODE;")')
    old = '''                        writeln!(
                            writer,
                            "ALTER TABLE ONLY {} ADD CONSTRAINT {} {};",
                            full_table_name,
                            quote_pg_identifier(&constraint_name),
                            constraint_def
                        )
                        .map_err(|e| e.to_string())?;'''
    part = replace(part, old, '''                        deferred_constraints.push(format!("ALTER TABLE ONLY {} ADD CONSTRAINT {} {};", full_table_name, quote_pg_identifier(&constraint_name), constraint_def));''')
    part = replace(part, '                writeln!(writer, "COMMIT;")', '                for constraint in deferred_constraints { writeln!(writer, "{}", constraint).map_err(|e| e.to_string())?; }\n                writeln!(writer, "COMMIT;")')
    part = replace(part, 'writeln!(writer, "BEGIN;")', 'writeln!(writer, "BEGIN; SET LOCAL standard_conforming_strings=on;")')
    return part
s = section(s, '#[tauri::command]\nasync fn export_database_sql', '#[tauri::command]\nasync fn cancel_database_export', export)

def import_database(part):
    return replace(part, 'let result = raw_sql(&script).execute(&pool).await.map_err(|e| e.to_string())?;', 'let mut connection = pool.acquire().await.map_err(|e| e.to_string())?;\n            connection.close_on_drop();\n            let result = raw_sql(&script).execute(&mut *connection).await.map_err(|e| e.to_string())?;', 2)
s = section(s, '#[tauri::command]\nasync fn import_database_sql', '#[tauri::command]\nasync fn alter_table', import_database)
s = section(s, '#[tauri::command]\nasync fn alter_table', '#[tauri::command]\nasync fn execute_query', lambda _: r'''#[tauri::command]
async fn alter_table(pool_manager: tauri::State<'_, PoolManager>, config: ConnectionConfig, table: String, operation: AlterOperation) -> Result<(), String> {
    schema_edits::alter(pool_manager.inner(), &config, &table, &operation).await
}
#[tauri::command]
async fn import_table_rows(pool_manager: tauri::State<'_, PoolManager>, config: ConnectionConfig, table: String, rows: Vec<HashMap<String, Value>>) -> Result<u64, String> {
    table_import::import_rows(pool_manager.inner(), &config, &table, &rows).await
}

''')
s = replace(s, '            import_database_sql,', '            import_database_sql,\n            import_table_rows,')
s = section(s, '#[tauri::command]\nasync fn get_ai_config', '#[tauri::command]\nasync fn generate_sql_from_text', lambda _: r'''#[tauri::command]
async fn get_ai_config(app: tauri::AppHandle) -> Result<ai_service::AIConfig, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    storage::read_json(&dir.join("ai_config.json"))
}
#[tauri::command]
async fn save_ai_config(app: tauri::AppHandle, config: ai_service::AIConfig) -> Result<(), String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    storage::write_json(&dir.join("ai_config.json"), config)
}

''')

def key_value(part):
    a = part.index('    let mut con ='); b = part.index('    // Get key type', a)
    part = part[:a] + '''    let selected = redis_support::selected_config(&config, database.as_deref())?;
    let mut con = pool_manager.get_redis_conn(&selected).await?;

''' + part[b:]
    part = part.replace('.unwrap_or(-1)', '.map_err(|e| e.to_string())?').replace('.unwrap_or(0)', '.map_err(|e| e.to_string())?').replace('.unwrap_or_default()', '.map_err(|e| e.to_string())?')
    part = replace(part, '    // Get value based on type', '''    // Fail explicitly on oversized previews rather than freezing the desktop.
    let size_command = match key_type.as_str() { "string" => Some(("STRLEN", 1_048_576)), "hash" => Some(("HLEN", 1000)), "set" => Some(("SCARD", 1000)), _ => None };
    if let Some((command, limit)) = size_command {
        let size: i64 = redis::cmd(command).arg(&key).query_async(&mut con).await.map_err(|e| e.to_string())?;
        if size > limit { return Err("Value is too large for a complete preview; use bounded Redis commands in the console".into()); }
    }
    // Get value based on type''')
    return part
s = section(s, '#[tauri::command]\nasync fn get_redis_key_value', '#[cfg_attr(mobile', key_value)
path.write_text(s)

# Cached SQL pools are created without awaiting network I/O under the global lock.
path = root / 'src-tauri/src/pool_manager.rs'; s = path.read_text()
s = replace(s, '    Redis(redis::aio::MultiplexedConnection),\n', '')
a = s.index('    /// Get or create a Redis multiplexed connection'); b = s.index('    /// Remove this saved connection', a)
s = s[:a] + '''    /// Redis stateful commands require a new physical connection per operation.
    pub async fn get_redis_conn(&self, config: &ConnectionConfig) -> Result<redis::aio::MultiplexedConnection, String> {
        crate::redis_support::connect(config).await
    }

''' + s[b:]
s = s.replace('.min_connections(1)', '.min_connections(0)\n            .acquire_timeout(std::time::Duration::from_secs(15))')
s = replace(s, '.connect_with(Self::mysql_options(config, database))\n            .await\n            .map_err(|e| e.to_string())?;', '.connect_lazy_with(Self::mysql_options(config, database));')
s = replace(s, '.connect_with(Self::pg_options(config, database))\n            .await\n            .map_err(|e| e.to_string())?;', '.connect_lazy_with(Self::pg_options(config, database));')
path.write_text(s)
path = root / 'src-tauri/Cargo.toml'; s = path.read_text(); s = replace(s, 'edition = "2021"', 'edition = "2021"\nrust-version = "1.89"'); s = replace(s, '"json", "chrono"]', '"json", "chrono", "bigdecimal", "uuid"]'); path.write_text(s)
# Fix the test's borrowed string so DeserializeOwned is satisfied.
path = root / 'src-tauri/src/storage.rs'; s = path.read_text(); s = replace(s, 'vec!["secret"]', 'vec!["secret".to_string()]'); path.write_text(s)
# DDL string literals cannot be CONVERT expressions; use a dedicated session
# with backslash escaping enabled and close that session afterwards.
path = root / 'src-tauri/src/schema_edits.rs'; s = path.read_text()
s = replace(s, 'fn default_sql(', '''fn ddl_string(value: &str, dialect: &str) -> String {
    if dialect == "mysql" { format!("'{}'", crate::escape_sql_string(value, dialect)) }
    else { sql_literal(Some(&Value::String(value.to_string())), dialect) }
}

fn default_sql(''')
s = s.replace('sql_literal(Some(&Value::String(decoded)), dialect)', 'ddl_string(&decoded, dialect)').replace('sql_literal(Some(&Value::String(s.clone())), dialect)', 'ddl_string(s, dialect)')
s = replace(s, '            for statement in statements { sqlx::query(&statement).execute(&pool).await.map_err(|e| e.to_string())?; }', '''            let mut connection = pool.acquire().await.map_err(|e| e.to_string())?;
            connection.close_on_drop();
            let modes: String = sqlx::query_scalar("SELECT @@SESSION.sql_mode").fetch_one(&mut *connection).await.map_err(|e| e.to_string())?;
            let modes = modes.split(',').filter(|mode| *mode != "NO_BACKSLASH_ESCAPES").collect::<Vec<_>>().join(",");
            sqlx::query("SET SESSION sql_mode = ?").bind(modes).execute(&mut *connection).await.map_err(|e| e.to_string())?;
            for statement in statements { sqlx::query(&statement).execute(&mut *connection).await.map_err(|e| e.to_string())?; }''')
path.write_text(s)
print('Backend audit patches applied; no production database was accessed.')
