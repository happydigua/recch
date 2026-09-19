//! Destructive tests use only disposable CI services, never saved user connections.
use super::*;

fn config(kind: &str, database: &str) -> ConnectionConfig {
    assert_eq!(
        std::env::var("RECCH_INTEGRATION").as_deref(),
        Ok("1"),
        "Run only against disposable local CI services"
    );
    ConnectionConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name: "ephemeral integration fixture".into(),
        db_type: kind.into(),
        host: "127.0.0.1".into(),
        port: match kind {
            "mysql" => 3306,
            "postgresql" => 5432,
            _ => 6379,
        },
        username: if kind == "redis" {
            None
        } else {
            Some(if kind == "mysql" { "root" } else { "postgres" }.into())
        },
        password: if kind == "redis" {
            None
        } else {
            Some("integration-only".into())
        },
        database: Some(database.into()),
    }
}
fn unique(prefix: &str) -> String {
    format!("recch_audit_{}_{}", prefix, uuid::Uuid::new_v4().simple())
}

#[tokio::test]
#[ignore = "requires isolated local database services"]
async fn postgres_values_are_lossless_and_sessions_do_not_leak() {
    let manager = PoolManager::new();
    let config = config("postgresql", "postgres");
    let result = execute_query_inner(&manager, &config, "SELECT 7::int2 AS small, 9::int4 AS ordinary, 9223372036854775807::int8 AS big, 1234.1234567890123456789::numeric AS amount, decode(repeat('AB', 80), 'hex') AS binary, '{\"n\":9007199254740993}'::jsonb AS document, NULL::int4 AS absent").await.unwrap();
    let row = &result[0];
    assert_eq!(row["small"], json!(7));
    assert_eq!(row["ordinary"], json!(9));
    assert_eq!(row["big"], json!("9223372036854775807"));
    assert_eq!(row["amount"], json!("1234.1234567890123456789"));
    assert_eq!(row["binary"], json!(format!("0x{}", "AB".repeat(80))));
    assert!(row["document"]
        .as_str()
        .unwrap()
        .contains("9007199254740993"));
    assert_eq!(row["absent"], Value::Null);
    assert!(
        execute_query_inner(&manager, &config, "SELECT 1 AS x, 2 AS x")
            .await
            .is_err()
    );
    assert!(
        execute_query_inner(&manager, &config, "SELECT ARRAY[1,2] AS unsupported")
            .await
            .is_err()
    );
    execute_query_inner(&manager, &config, "SET search_path = pg_catalog")
        .await
        .unwrap();
    let value = execute_query_inner(&manager, &config, "SHOW search_path")
        .await
        .unwrap();
    assert_ne!(value[0]["search_path"], json!("pg_catalog"));
    assert!(
        execute_query_inner(&manager, &config, "SELECT generate_series(1,10001) AS n")
            .await
            .unwrap_err()
            .contains("10,000")
    );
}

#[tokio::test]
#[ignore = "requires isolated local database services"]
async fn mysql_values_are_lossless_and_sessions_do_not_leak() {
    let manager = PoolManager::new();
    let config = config("mysql", "mysql");
    let result = execute_query_inner(&manager, &config, "SELECT CAST(7 AS SIGNED) AS ordinary, CAST('18446744073709551615' AS UNSIGNED) AS big, CAST('1234.1234567890123456789' AS DECIMAL(40,19)) AS amount, CAST(UNHEX(REPEAT('AB',80)) AS BINARY(80)) AS binary, CAST('{\"n\":9007199254740993}' AS JSON) AS document").await.unwrap();
    let row = &result[0];
    assert_eq!(row["ordinary"], json!(7));
    assert_eq!(row["big"], json!("18446744073709551615"));
    assert_eq!(row["amount"], json!("1234.1234567890123456789"));
    assert_eq!(row["binary"], json!(format!("0x{}", "AB".repeat(80))));
    assert!(row["document"]
        .as_str()
        .unwrap()
        .contains("9007199254740993"));
    let pool = manager.get_mysql_pool(&config, None).await.unwrap();
    let mut connection = pool.acquire().await.unwrap().detach();
    raw_sql("CREATE TEMPORARY TABLE decoder_types(flag tinyint(1), year_value YEAR, day_value DATE); INSERT INTO decoder_types VALUES(2,2026,'2026-09-19');").execute(&mut connection).await.unwrap();
    let typed = sqlx::query("SELECT * FROM decoder_types")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    let typed = mysql_row_to_json_map(&typed).unwrap();
    assert_eq!(typed["flag"], json!(2));
    assert_eq!(typed["year_value"], json!(2026));
    assert_eq!(typed["day_value"], json!("2026-09-19"));
    execute_query_inner(&manager, &config, "SET @recch_session_marker = 123")
        .await
        .unwrap();
    let rows = execute_query_inner(
        &manager,
        &config,
        "SELECT CAST(@recch_session_marker AS SIGNED) AS marker",
    )
    .await
    .unwrap();
    assert_eq!(rows[0]["marker"], Value::Null);
}

#[tokio::test]
#[ignore = "requires isolated local database services"]
async fn redis_select_acl_and_empty_arguments_are_isolated() {
    let manager = PoolManager::new();
    let config = config("redis", "0");
    let key = unique("redis");
    let first = execute_query_inner(
        &manager,
        &config,
        &format!("SELECT 1\nSET {} \"\"\nGET {}", key, key),
    )
    .await
    .unwrap();
    assert_eq!(first[2]["result"], json!(""));
    let second = execute_query_inner(&manager, &config, &format!("GET {}", key))
        .await
        .unwrap();
    assert_eq!(second[0]["result"], json!("(nil)"));
    let username = unique("acl");
    let mut admin = redis_support::connect(&config).await.unwrap();
    let _: () = redis::cmd("ACL")
        .arg("SETUSER")
        .arg(&username)
        .arg("on")
        .arg(">p@ss:/?#%")
        .arg("~*")
        .arg("+@all")
        .query_async(&mut admin)
        .await
        .unwrap();
    let account = ConnectionConfig {
        username: Some(username.clone()),
        password: Some("p@ss:/?#%".into()),
        ..config.clone()
    };
    let mut authenticated = redis_support::connect(&account).await.unwrap();
    let who: String = redis::cmd("ACL")
        .arg("WHOAMI")
        .query_async(&mut authenticated)
        .await
        .unwrap();
    assert_eq!(who, username);
    let _: i64 = redis::cmd("ACL")
        .arg("DELUSER")
        .arg(&username)
        .query_async(&mut admin)
        .await
        .unwrap();
    let _ = execute_query_inner(&manager, &config, &format!("SELECT 1\nDEL {}", key))
        .await
        .unwrap();
    assert!(
        execute_query_inner(&manager, &config, "SET key \"unterminated")
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires isolated local database services and matching pg_dump"]
async fn postgres_native_dump_restores_dependencies_and_values() {
    let manager = PoolManager::new();
    let admin_config = config("postgresql", "postgres");
    let admin = manager.get_pg_pool(&admin_config, None).await.unwrap();
    let source = unique("pg_src");
    let target = unique("pg_dst");
    for name in [&source, &target] {
        sqlx::query(&format!("CREATE DATABASE {}", quote_pg_identifier(name)))
            .execute(&admin)
            .await
            .unwrap();
    }
    let source_config = ConnectionConfig {
        database: Some(source.clone()),
        ..admin_config.clone()
    };
    let pool = manager.get_pg_pool(&source_config, None).await.unwrap();
    raw_sql("CREATE TABLE z_parent(id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, amount numeric(40,19), payload bytea, document jsonb); CREATE TABLE a_child(id int PRIMARY KEY, parent_id bigint REFERENCES z_parent(id)); INSERT INTO z_parent(amount,payload,document) VALUES (1234.1234567890123456789,decode(repeat('AB',80),'hex'),'{\"n\":9007199254740993}'); INSERT INTO a_child VALUES(1,1); CREATE VIEW parent_view AS SELECT id, amount FROM z_parent; CREATE SCHEMA extra; CREATE TABLE extra.kept(id int); INSERT INTO extra.kept VALUES(42); COMMENT ON COLUMN z_parent.amount IS 'exact decimal';").execute(&pool).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("backup.sql");
    native_backup::export(&source_config, &source, &output, &AtomicBool::new(false))
        .await
        .unwrap();
    let target_config = ConnectionConfig {
        database: Some(target.clone()),
        ..admin_config
    };
    let destination = manager.get_pg_pool(&target_config, None).await.unwrap();
    let mut connection = destination.acquire().await.unwrap().detach();
    raw_sql(&native_backup::sql_without_psql_wrapper(
        &fs::read_to_string(output).unwrap(),
    ))
    .execute(&mut connection)
    .await
    .unwrap();
    let amount: String = sqlx::query_scalar("SELECT amount::text FROM parent_view")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(amount, "1234.1234567890123456789");
    let size: i32 = sqlx::query_scalar("SELECT octet_length(payload) FROM z_parent")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(size, 80);
    let id: i32 = sqlx::query_scalar("SELECT id FROM extra.kept")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(id, 42);
    assert!(sqlx::query("INSERT INTO a_child VALUES(2,999)")
        .execute(&mut connection)
        .await
        .is_err());
    let id: i64 = sqlx::query_scalar("INSERT INTO z_parent DEFAULT VALUES RETURNING id")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(id, 2);
    drop(connection);
    pool.close().await;
    destination.close().await;
    for name in [&source, &target] {
        sqlx::query(&format!(
            "DROP DATABASE {} WITH (FORCE)",
            quote_pg_identifier(name)
        ))
        .execute(&admin)
        .await
        .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires isolated local database services and MySQL 8 mysqldump"]
async fn mysql_native_dump_restores_binary_decimal_and_foreign_keys() {
    let manager = PoolManager::new();
    let admin_config = config("mysql", "mysql");
    let admin = manager.get_mysql_pool(&admin_config, None).await.unwrap();
    let source = unique("my_src");
    let target = unique("my_dst");
    for name in [&source, &target] {
        sqlx::query(&format!("CREATE DATABASE {}", quote_mysql_identifier(name)))
            .execute(&admin)
            .await
            .unwrap();
    }
    let source_config = ConnectionConfig {
        database: Some(source.clone()),
        ..admin_config.clone()
    };
    let pool = manager.get_mysql_pool(&source_config, None).await.unwrap();
    raw_sql("CREATE TABLE z_parent(id bigint PRIMARY KEY AUTO_INCREMENT, amount decimal(40,19), payload blob) ENGINE=InnoDB; CREATE TABLE a_child(id int PRIMARY KEY, parent_id bigint, FOREIGN KEY(parent_id) REFERENCES z_parent(id)) ENGINE=InnoDB; INSERT INTO z_parent(amount,payload) VALUES (1234.1234567890123456789,UNHEX(REPEAT('AB',80))); INSERT INTO a_child VALUES(1,1);").execute(&pool).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("backup.sql");
    native_backup::export(&source_config, &source, &output, &AtomicBool::new(false))
        .await
        .unwrap();
    let destination_config = ConnectionConfig {
        database: Some(target.clone()),
        ..admin_config
    };
    let destination = manager
        .get_mysql_pool(&destination_config, None)
        .await
        .unwrap();
    let mut connection = destination.acquire().await.unwrap().detach();
    raw_sql(&fs::read_to_string(output).unwrap())
        .execute(&mut connection)
        .await
        .unwrap();
    let (amount, size): (String, i64) = sqlx::query_as(
        "SELECT CAST(amount AS CHAR), CAST(OCTET_LENGTH(payload) AS SIGNED) FROM z_parent",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(amount, "1234.1234567890123456789");
    assert_eq!(size, 80);
    assert!(sqlx::query("INSERT INTO a_child VALUES(2,999)")
        .execute(&mut connection)
        .await
        .is_err());
    drop(connection);
    pool.close().await;
    destination.close().await;
    for name in [&source, &target] {
        sqlx::query(&format!("DROP DATABASE {}", quote_mysql_identifier(name)))
            .execute(&admin)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires isolated local database services"]
async fn failed_row_imports_roll_back_in_postgres_and_innodb() {
    let manager = PoolManager::new();
    for kind in ["postgresql", "mysql"] {
        let cfg = config(kind, if kind == "mysql" { "mysql" } else { "postgres" });
        let table = unique("batch");
        let quoted = if kind == "mysql" {
            quote_mysql_identifier(&table)
        } else {
            quote_pg_identifier(&table)
        };
        execute_query_inner(
            &manager,
            &cfg,
            &format!(
                "CREATE TABLE {} (id int PRIMARY KEY) {}",
                quoted,
                if kind == "mysql" { "ENGINE=InnoDB" } else { "" }
            ),
        )
        .await
        .unwrap();
        let query = format!("INSERT INTO {} (id) VALUES (1)", quoted);
        let error =
            import_table_rows_inner(&manager, &cfg, &table, &[query.clone(), query.clone()])
                .await
                .unwrap_err();
        assert!(error.contains("rolled back"), "{error}");
        let rows = execute_query_inner(
            &manager,
            &cfg,
            &format!("SELECT COUNT(*) AS n FROM {}", quoted),
        )
        .await
        .unwrap();
        assert_eq!(rows[0]["n"], json!(0));
        assert_eq!(
            import_table_rows_inner(&manager, &cfg, &table, &[query])
                .await
                .unwrap(),
            1
        );
        execute_query_inner(&manager, &cfg, &format!("DROP TABLE {}", quoted))
            .await
            .unwrap();
    }
}
