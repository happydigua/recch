//! Locked, atomic configuration writes. A corrupt file is never an empty config.
use fs2::FileExt;
use serde::{de::DeserializeOwned, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn lock(path: &Path) -> Result<File, String> {
    let parent = path
        .parent()
        .ok_or("Configuration has no parent directory")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let file = private_options()
        .open(path.with_extension("lock"))
        .map_err(|e| e.to_string())?;
    file.lock_exclusive().map_err(|e| e.to_string())?;
    Ok(file) // OS releases the lock on drop, including error paths.
}

fn read_unlocked<T: DeserializeOwned + Default>(path: &Path) -> Result<T, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| "Configuration is invalid; original file preserved. Restore a valid backup before saving.".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e.to_string()),
    }
}

fn write_unlocked<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let parent = path
        .parent()
        .ok_or("Configuration has no parent directory")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    // NamedTempFile is owner-only on Unix. Persist atomically replaces the old
    // destination on Windows too, unlike a remove-then-rename sequence.
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.error.to_string())?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn read<T: DeserializeOwned + Default>(path: &Path) -> Result<T, String> {
    let _lock = lock(path)?;
    read_unlocked(path)
}

pub fn update<T, R>(
    path: &Path,
    change: impl FnOnce(&mut T) -> Result<R, String>,
) -> Result<R, String>
where
    T: DeserializeOwned + Serialize + Default,
{
    let _lock = lock(path)?;
    let mut value = read_unlocked::<T>(path)?;
    let result = change(&mut value)?;
    write_unlocked(path, &value)?;
    Ok(result)
}

pub fn save<T: DeserializeOwned + Serialize + Default>(
    path: &Path,
    value: T,
) -> Result<(), String> {
    update(path, |existing| {
        *existing = value;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corrupt_configuration_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        fs::write(&path, b"{broken").unwrap();
        assert!(save(&path, vec!["new".to_string()]).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{broken");
        assert!(read::<Vec<String>>(&path).is_err());
    }
    #[test]
    fn failed_update_keeps_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        save(&path, vec![1]).unwrap();
        let result = update::<Vec<i32>, ()>(&path, |v| {
            v.push(2);
            Err("failure".into())
        });
        assert!(result.is_err());
        assert_eq!(read::<Vec<i32>>(&path).unwrap(), vec![1]);
    }
    #[test]
    fn concurrent_updates_do_not_lose_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        let threads: Vec<_> = (0..20)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    update::<Vec<i32>, _>(&path, |v| {
                        v.push(i);
                        Ok(())
                    })
                    .unwrap()
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let mut result = read::<Vec<i32>>(&path).unwrap();
        result.sort();
        assert_eq!(result, (0..20).collect::<Vec<_>>());
    }
    #[cfg(unix)]
    #[test]
    fn written_secrets_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ai_config.json");
        save(&path, vec!["secret".to_string()]).unwrap();
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
