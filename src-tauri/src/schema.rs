//! Visual schema operations: quote identifiers and reject unsupported SQL fragments.
use crate::{AlterOperation, ColumnDef};

fn identifier(name: &str, mysql: bool) -> Result<String, String> {
    if name.is_empty() || name.contains('\0') {
        return Err("Invalid SQL identifier".into());
    }
    Ok(if mysql {
        format!("`{}`", name.replace('`', "``"))
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    })
}
fn literal(value: &str, mysql: bool, backslash: bool) -> Result<String, String> {
    if value.contains('\0') {
        return Err("Zero bytes are not supported in schema text".into());
    }
    let escaped = if !mysql || backslash {
        value.replace('\\', "\\\\")
    } else {
        value.into()
    };
    Ok(format!(
        "{}'{}'",
        if mysql { "" } else { "E" },
        escaped.replace('\'', "''")
    ))
}
fn data_type(value: &str) -> Result<&str, String> {
    let allowed = [
        "INT",
        "INTEGER",
        "TINYINT",
        "SMALLINT",
        "MEDIUMINT",
        "BIGINT",
        "SERIAL",
        "BIGSERIAL",
        "SMALLSERIAL",
        "INT2",
        "INT4",
        "INT8",
        "FLOAT4",
        "FLOAT8",
        "VARCHAR",
        "CHAR",
        "CHARACTER",
        "VARYING",
        "TEXT",
        "TINYTEXT",
        "MEDIUMTEXT",
        "LONGTEXT",
        "DECIMAL",
        "NUMERIC",
        "REAL",
        "FLOAT",
        "DOUBLE",
        "PRECISION",
        "BOOL",
        "BOOLEAN",
        "DATE",
        "DATETIME",
        "TIME",
        "TIMESTAMP",
        "TIMESTAMPTZ",
        "WITH",
        "WITHOUT",
        "ZONE",
        "YEAR",
        "UNSIGNED",
        "ZEROFILL",
        "BINARY",
        "VARBINARY",
        "BLOB",
        "TINYBLOB",
        "MEDIUMBLOB",
        "LONGBLOB",
        "BYTEA",
        "JSON",
        "JSONB",
        "UUID",
    ];
    if value.is_empty()
        || value.len() > 128
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " (),".contains(c))
    {
        return Err(
            "This type is not supported by the visual editor; use reviewed SQL for advanced types"
                .into(),
        );
    }
    let upper = value.to_ascii_uppercase();
    for word in upper
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        if !word.bytes().all(|b| b.is_ascii_digit()) && !allowed.contains(&word) {
            return Err("Unsupported visual-editor column type".into());
        }
    }
    Ok(value)
}
fn expression(value: &str) -> Result<&str, String> {
    let mut quote = None;
    let mut depth = 0i32;
    let mut chars = value.chars().peekable();
    let mut outside = String::new();
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == '\\' {
                chars.next();
            } else if c == q {
                if chars.peek() == Some(&q) {
                    chars.next();
                } else {
                    quote = None;
                }
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                outside.push(' ');
            }
            '(' => {
                depth += 1;
                outside.push(' ');
            }
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return Err("Unbalanced default expression".into());
                }
                outside.push(' ');
            }
            ';' | '\0' => return Err("Multiple statements are not allowed in a default".into()),
            ',' if depth == 0 => {
                return Err("Multiple ALTER actions are not allowed in a default".into())
            }
            '-' if chars.peek() == Some(&'-') => {
                return Err("SQL comments are not allowed in a default".into())
            }
            '/' if chars.peek() == Some(&'*') => {
                return Err("SQL comments are not allowed in a default".into())
            }
            _ => outside.push(c),
        }
    }
    if depth != 0 || quote.is_some() || value.trim().is_empty() {
        return Err("Malformed default expression".into());
    }
    let denied = [
        "DROP",
        "ALTER",
        "ADD",
        "COMMENT",
        "RENAME",
        "CONSTRAINT",
        "PRIMARY",
        "REFERENCES",
        "CHECK",
        "FOREIGN",
        "CASCADE",
        "RESTRICT",
        "SET",
    ];
    if outside
        .to_ascii_uppercase()
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|w| denied.contains(&w))
    {
        return Err(
            "Only a default expression is allowed; use reviewed SQL for schema changes".into(),
        );
    }
    Ok(value)
}
fn default_value(col: &ColumnDef, mysql: bool, backslash: bool) -> Result<Option<String>, String> {
    col.default_value
        .as_deref()
        .map(|v| {
            let kind = col.type_name.to_ascii_uppercase();
            if mysql
                && (kind.contains("CHAR")
                    || kind.contains("TEXT")
                    || kind == "DATE"
                    || kind.starts_with("TIME")
                    || kind.starts_with("DATETIME"))
                && !((kind.starts_with("TIME") || kind.starts_with("DATETIME"))
                    && v.to_ascii_uppercase().starts_with("CURRENT_TIMESTAMP"))
            {
                literal(v, true, backslash)
            } else {
                expression(v).map(str::to_string)
            }
        })
        .transpose()
}

pub fn queries(
    table: &str,
    op: &AlterOperation,
    mysql: bool,
    backslash: bool,
    preserved_attributes: &str,
) -> Result<Vec<String>, String> {
    let table = identifier(table, mysql)?;
    let table = if mysql {
        table
    } else {
        format!("\"public\".{}", table)
    };
    let mut queries = Vec::new();
    match op.op_type.as_str() {
        "add" | "modify" => {
            let col = op.column_def.as_ref().ok_or("Missing column definition")?;
            let name = identifier(&col.name, mysql)?;
            let kind = data_type(&col.type_name)?;
            let default = default_value(col, mysql, backslash)?;
            let null = if col.is_nullable == Some(false) {
                "NOT NULL"
            } else {
                "NULL"
            };
            let original = identifier(op.column_name.as_deref().unwrap_or(&col.name), mysql)?;
            if mysql {
                let action = if op.op_type == "add" {
                    "ADD COLUMN".to_string()
                } else {
                    format!("CHANGE COLUMN {}", original)
                };
                let default = default
                    .map(|v| format!(" DEFAULT {}", v))
                    .unwrap_or_default();
                let pk = if op.op_type == "add" && col.is_pk {
                    " PRIMARY KEY"
                } else {
                    ""
                };
                let comment = literal(col.comment.as_deref().unwrap_or(""), true, backslash)?;
                queries.push(format!(
                    "ALTER TABLE {} {} {} {} {}{}{} {} COMMENT {}",
                    table, action, name, kind, null, default, pk, preserved_attributes, comment
                ));
            } else {
                if op.op_type == "add" {
                    let default = default
                        .map(|v| format!(" DEFAULT {}", v))
                        .unwrap_or_default();
                    let pk = if col.is_pk { " PRIMARY KEY" } else { "" };
                    queries.push(format!(
                        "ALTER TABLE {} ADD COLUMN {} {} {}{}{}",
                        table, name, kind, null, default, pk
                    ));
                } else {
                    if name != original {
                        queries.push(format!(
                            "ALTER TABLE {} RENAME COLUMN {} TO {}",
                            table, original, name
                        ));
                    }
                    queries.push(format!(
                        "ALTER TABLE {} ALTER COLUMN {} TYPE {}",
                        table, name, kind
                    ));
                    queries.push(format!(
                        "ALTER TABLE {} ALTER COLUMN {} {} NOT NULL",
                        table,
                        name,
                        if col.is_nullable == Some(false) {
                            "SET"
                        } else {
                            "DROP"
                        }
                    ));
                    queries.push(format!(
                        "ALTER TABLE {} ALTER COLUMN {} {}",
                        table,
                        name,
                        default
                            .map(|v| format!("SET DEFAULT {}", v))
                            .unwrap_or("DROP DEFAULT".into())
                    ));
                }
                queries.push(format!(
                    "COMMENT ON COLUMN {}.{} IS {}",
                    table,
                    name,
                    col.comment
                        .as_deref()
                        .map(|v| literal(v, false, true))
                        .transpose()?
                        .unwrap_or("NULL".into())
                ));
            }
        }
        "drop" => queries.push(format!(
            "ALTER TABLE {} DROP COLUMN {}",
            table,
            identifier(
                op.column_name.as_deref().ok_or("Missing column name")?,
                mysql
            )?
        )),
        "rename" => queries.push(format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {}",
            table,
            identifier(
                op.column_name.as_deref().ok_or("Missing column name")?,
                mysql
            )?,
            identifier(op.new_name.as_deref().ok_or("Missing new name")?, mysql)?
        )),
        "add_index" => {
            let index = op.index_def.as_ref().ok_or("Missing index")?;
            if index.columns.is_empty() {
                return Err("Index must contain columns".into());
            }
            let columns = index
                .columns
                .iter()
                .map(|c| identifier(c, mysql))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ");
            queries.push(format!(
                "CREATE {} INDEX {} ON {} ({})",
                if index.is_unique { "UNIQUE" } else { "" },
                identifier(&index.name, mysql)?,
                table,
                columns
            ));
        }
        "drop_index" => {
            let name = identifier(op.index_name.as_deref().ok_or("Missing index name")?, mysql)?;
            queries.push(if mysql {
                format!("DROP INDEX {} ON {}", name, table)
            } else {
                format!("DROP INDEX \"public\".{}", name)
            });
        }
        _ => return Err("Unknown schema operation".into()),
    }
    Ok(queries)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_additional_actions_and_accepts_quoted_defaults() {
        for v in [
            "0, DROP COLUMN secret",
            "0; DELETE FROM t",
            "0 --comment",
            "0 /*x*/",
            "(1",
        ] {
            assert!(expression(v).is_err());
        }
        assert!(expression("nextval('a_seq'::regclass)").is_ok());
        assert!(expression("'a,b;DROP'").is_ok());
        assert!(data_type("int, DROP COLUMN secret").is_err());
        assert_eq!(identifier("a\"b", false).unwrap(), "\"a\"\"b\"");
        assert_eq!(literal("x\\'", true, false).unwrap(), "'x\\'''");
    }
}
