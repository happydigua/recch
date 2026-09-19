from pathlib import Path

p = Path('src-tauri/src/row_values.rs'); s = p.read_text()
old = '        if row.try_get_raw(i).map_err(|e| e.to_string())?.is_null()'
assert s.count(old) == 2
s = s.replace(old, '        if map.contains_key(name) { return Err(format!("Duplicate result column {name:?}; use distinct SQL aliases")); }\n' + old)
p.write_text(s)
p = Path('src-tauri/src/lib.rs'); s = p.read_text()
old = 'SELECT c.column_name, c.data_type,'
assert old in s
s = s.replace(old, 'SELECT c.column_name, pg_catalog.format_type(attr.atttypid, attr.atttypmod),', 1)
old = "FROM information_schema.columns c\n                WHERE c.table_schema='public'"
assert old in s
s = s.replace(old, "FROM information_schema.columns c\n                JOIN pg_namespace ns ON ns.nspname=c.table_schema\n                JOIN pg_class rel ON rel.relnamespace=ns.oid AND rel.relname=c.table_name\n                JOIN pg_attribute attr ON attr.attrelid=rel.oid AND attr.attname=c.column_name\n                WHERE c.table_schema='public'", 1)
p.write_text(s)

p = Path('src-tauri/src/schema_edits.rs'); s = p.read_text()
s += r'''
#[cfg(test)]
mod integration_tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires disposable CI database services"]
    async fn integration_visual_ddl_preserves_names_defaults_and_nullability() {
        let name = format!("recch_ddl_{}", uuid::Uuid::new_v4().simple());
        let manager = PoolManager::new();
        let mut config = ConnectionConfig { id: "ddl-fixture".into(), name: "Fixture".into(), db_type: "mysql".into(),
            host: "127.0.0.1".into(), port: 3306, username: Some("root".into()), password: Some("recch_ci_only".into()), database: Some("recch_test".into()) };
        let operation = AlterOperation { op_type: "add".into(), column_name: None, new_name: None,
            column_def: Some(ColumnDef { name: "old name".into(), type_name: "VARCHAR(40)".into(), is_pk: false,
                is_nullable: Some(false), default_value: Some("'it''s'".into()), comment: Some("owner's note".into()) }), index_def: None, index_name: None };
        let mysql = manager.get_mysql_pool(&config, None).await.unwrap();
        sqlx::query(&format!("CREATE TABLE `{name}` (id INT PRIMARY KEY AUTO_INCREMENT) ENGINE=InnoDB")).execute(&mysql).await.unwrap();
        alter(&manager, &config, &name, &operation).await.unwrap();
        let mut modify = operation;
        modify.op_type = "modify".into(); modify.column_name = Some("old name".into());
        modify.column_def.as_mut().unwrap().name = "new name".into();
        alter(&manager, &config, &name, &modify).await.unwrap();
        sqlx::query(&format!("INSERT INTO `{name}` (id) VALUES (1)")).execute(&mysql).await.unwrap();
        let text: String = sqlx::query_scalar(&format!("SELECT `new name` FROM `{name}` WHERE id=1")).fetch_one(&mysql).await.unwrap();
        assert_eq!(text, "it's");
        assert!(sqlx::query(&format!("INSERT INTO `{name}` (id, `new name`) VALUES (2,NULL)")).execute(&mysql).await.is_err());
        sqlx::query(&format!("DROP TABLE `{name}`")).execute(&mysql).await.unwrap();
        config.db_type = "postgresql".into(); config.port = 5432; config.username = Some("postgres".into());
        let pg = manager.get_pg_pool(&config, None).await.unwrap();
        sqlx::query(&format!("CREATE TABLE \"{name}\" (id INT PRIMARY KEY, \"old name\" VARCHAR(40))")).execute(&pg).await.unwrap();
        alter(&manager, &config, &name, &modify).await.unwrap();
        sqlx::query(&format!("INSERT INTO \"{name}\" (id) VALUES (1)")).execute(&pg).await.unwrap();
        let text: String = sqlx::query_scalar(&format!("SELECT \"new name\" FROM \"{name}\" WHERE id=1")).fetch_one(&pg).await.unwrap(); assert_eq!(text, "it's");
        assert!(sqlx::query(&format!("INSERT INTO \"{name}\" (id, \"new name\") VALUES (2,NULL)")).execute(&pg).await.is_err());
        let comment: Option<String> = sqlx::query_scalar("SELECT col_description(c.oid,a.attnum) FROM pg_class c JOIN pg_attribute a ON a.attrelid=c.oid WHERE c.relname=$1 AND a.attname='new name'")
            .bind(&name).fetch_one(&pg).await.unwrap(); assert_eq!(comment.as_deref(), Some("owner's note"));
        // Failure in the final rename must also roll back earlier type/default changes.
        let mut failed = modify; failed.column_name = Some("new name".into());
        let definition = failed.column_def.as_mut().unwrap(); definition.name = "id".into(); definition.default_value = Some("'changed'".into());
        assert!(alter(&manager, &config, &name, &failed).await.is_err());
        sqlx::query(&format!("INSERT INTO \"{name}\" (id) VALUES (3)")).execute(&pg).await.unwrap();
        let text: String = sqlx::query_scalar(&format!("SELECT \"new name\" FROM \"{name}\" WHERE id=3")).fetch_one(&pg).await.unwrap(); assert_eq!(text, "it's");
        sqlx::query(&format!("DROP TABLE \"{name}\"")).execute(&pg).await.unwrap();
    }
}
'''
p.write_text(s)
print('Additional decoder and DDL regression safeguards applied.')
