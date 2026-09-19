//! Native logical dumps: never serialize grid previews into a backup.
use crate::ConnectionConfig;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::process::Command;

fn option_value(value: &str) -> Result<String, String> {
    if value.contains('\0') {
        return Err("Invalid zero byte in connection option".into());
    }
    Ok(format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    ))
}

fn command(
    config: &ConnectionConfig,
    database: &str,
    credentials: &mut tempfile::NamedTempFile,
) -> Result<Command, String> {
    match config.db_type.as_str() {
        "postgresql" => {
            let mut command = Command::new("pg_dump");
            command.args([
                "--format=plain",
                "--inserts",
                "--no-owner",
                "--no-privileges",
                "--no-password",
            ]);
            command
                .env("PGHOST", &config.host)
                .env("PGPORT", config.port.to_string())
                .env("PGDATABASE", database)
                .env("PGCONNECT_TIMEOUT", "10");
            if let Some(user) = &config.username {
                command.env("PGUSER", user);
            }
            if let Some(password) = &config.password {
                command.env("PGPASSWORD", password);
            }
            Ok(command)
        }
        "mysql" => {
            // An owner-only option file avoids passwords in command arguments.
            writeln!(
                credentials,
                "[client]\nhost={}\nport={}\nprotocol=tcp",
                option_value(&config.host)?,
                config.port
            )
            .map_err(|e| e.to_string())?;
            if let Some(user) = &config.username {
                writeln!(credentials, "user={}", option_value(user)?).map_err(|e| e.to_string())?;
            }
            if let Some(password) = &config.password {
                writeln!(credentials, "password={}", option_value(password)?)
                    .map_err(|e| e.to_string())?;
            }
            credentials.flush().map_err(|e| e.to_string())?;
            let mut command = Command::new("mysqldump");
            command.arg(format!("--defaults-file={}", credentials.path().display()));
            command.args([
                "--single-transaction",
                "--quick",
                "--hex-blob",
                "--routines",
                "--events",
                "--triggers",
                "--default-character-set=utf8mb4",
                "--set-gtid-purged=OFF",
                "--column-statistics=0",
                "--",
                database,
            ]);
            Ok(command)
        }
        _ => Err("Native export supports only MySQL and PostgreSQL".into()),
    }
}

async fn run_to_file(
    mut command: Command,
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Export cancelled".into());
    }
    let parent = path
        .parent()
        .filter(|v| !v.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    // Create beside the destination so final replacement is atomic. Any error,
    // cancellation or process failure drops only this temporary file.
    let staging = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    let output = staging.reopen().map_err(|e| e.to_string())?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(output))
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|e| format!("Cannot start native dump tool: {}. Install a compatible pg_dump or MySQL mysqldump and add it to PATH.", e))?;
    let start = Instant::now();
    let status = loop {
        tokio::select! {
            status = child.wait() => break status.map_err(|e| e.to_string())?,
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                if cancelled.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(3600) {
                    child.kill().await.map_err(|e| e.to_string())?;
                    return Err(if cancelled.load(Ordering::Relaxed) { "Export cancelled" } else { "Export timed out after 60 minutes" }.into());
                }
            }
        }
    };
    if !status.success() {
        return Err(format!("Native dump failed ({}); previous backup preserved. Check tool/server compatibility and backup privileges.", status));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("Export cancelled".into());
    }
    staging.as_file().sync_all().map_err(|e| e.to_string())?;
    staging.persist(path).map_err(|e| e.error.to_string())?;
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn export(
    config: &ConnectionConfig,
    database: &str,
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let mut credentials = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    let command = command(config, database, &mut credentials)?;
    run_to_file(command, path, cancelled).await
}

/// New pg_dump versions wrap plain SQL with matching psql restriction markers.
/// Strip ONLY a matching outer pair; never execute psql client/shell commands.
pub fn sql_without_psql_wrapper(script: &str) -> String {
    let lines: Vec<_> = script.lines().collect();
    let significant: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with("--"))
        .collect();
    if let (Some((first, begin)), Some((last, end))) = (significant.first(), significant.last()) {
        if let Some(key) = begin.trim().strip_prefix("\\restrict ") {
            if !key.is_empty()
                && key.bytes().all(|b| b.is_ascii_alphanumeric())
                && end.trim() == format!("\\unrestrict {}", key)
                && first < last
            {
                return lines
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| i != first && i != last)
                    .map(|(_, l)| *l)
                    .collect::<Vec<_>>()
                    .join("\n");
            }
        }
    }
    script.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn option_values_cannot_inject_option_file_sections() {
        assert_eq!(
            option_value("x\n[client]\npassword=p\\\"").unwrap(),
            "\"x\\n[client]\\npassword=p\\\\\\\"\""
        );
        assert!(option_value("\0").is_err());
    }
    #[test]
    fn strips_only_matching_outer_psql_markers() {
        assert_eq!(
            sql_without_psql_wrapper(
                "-- dump\n\\restrict abc123\nSELECT 1;\n\\unrestrict abc123\n"
            ),
            "-- dump\nSELECT 1;"
        );
        let script = "SELECT '\\restrict abc';\n\\unrestrict different";
        assert_eq!(sql_without_psql_wrapper(script), script);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn process_failure_and_cancellation_preserve_existing_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("backup.sql");
        std::fs::write(&path, "previous backup").unwrap();
        let mut fail = Command::new("sh");
        fail.args(["-c", "printf partial; exit 1"]);
        assert!(run_to_file(fail, &path, &AtomicBool::new(false))
            .await
            .is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "previous backup");
        assert!(
            run_to_file(Command::new("sh"), &path, &AtomicBool::new(true))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "previous backup");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn successful_dump_replaces_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("backup.sql");
        std::fs::write(&path, "old").unwrap();
        let mut success = Command::new("sh");
        success.args(["-c", "printf complete"]);
        run_to_file(success, &path, &AtomicBool::new(false))
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "complete");
    }
}
