//! Crash-resistant local writes. A failed/cancelled operation never deletes the old file.
use serde::{de::DeserializeOwned, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
}

fn destination(path: &Path) -> Result<PathBuf, String> {
    let name = path.file_name().ok_or("Missing file name")?;
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let target = parent.canonicalize().map_err(|e| e.to_string())?.join(name);
    match fs::symlink_metadata(&target) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() =>
            return Err("Refusing to replace a directory or symbolic link".into()),
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.to_string()),
        _ => {}
    }
    Ok(target)
}

fn lock_target(target: &Path) -> Result<File, String> {
    let mut name = std::ffi::OsString::from(".");
    name.push(target.file_name().ok_or("Missing file name")?);
    name.push(".recch-lock");
    let path = target.with_file_name(name);
    if fs::symlink_metadata(&path).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
        return Err("Refusing a symbolic-link lock file".into());
    }
    let file = private_options().create(true).truncate(false).open(path).map_err(|e| e.to_string())?;
    file.try_lock().map_err(|_| "File is in use by another RECCH operation; retry after it finishes".to_string())?;
    Ok(file)
}

/// The sibling lock file stays in place: deleting it would permit inode-lock races.
/// OS locks are released automatically even when the process exits unexpectedly.
pub struct AtomicFile {
    target: PathBuf,
    temporary: PathBuf,
    file: Option<File>,
    _lock: File,
    committed: bool,
}

impl AtomicFile {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, String> {
        let target = destination(path.as_ref())?;
        let lock = lock_target(&target)?;
        let temporary = target.with_file_name(format!(".recch-{}.tmp", uuid::Uuid::new_v4()));
        let file = private_options().create_new(true).open(&temporary).map_err(|e| e.to_string())?;
        Ok(Self { target, temporary, file: Some(file), _lock: lock, committed: false })
    }

    pub fn commit(mut self) -> Result<(), String> {
        let file = self.file.take().ok_or("File already closed")?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file); // Close the temporary handle before replacement on Windows.
        fs::rename(&self.temporary, &self.target).map_err(|e| e.to_string())?;
        self.committed = true;
        // The rename is the commit point. Directory fsync is best-effort because
        // some filesystems do not support it; never report that the old file remains.
        #[cfg(unix)]
        if let Some(parent) = self.target.parent() {
            if let Ok(dir) = File::open(parent) { let _ = dir.sync_all(); }
        }
        Ok(())
    }
}

impl Write for AtomicFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.file.as_mut().ok_or_else(|| io::Error::other("File closed"))?.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.as_mut().ok_or_else(|| io::Error::other("File closed"))?.flush()
    }
}

impl Drop for AtomicFile {
    fn drop(&mut self) {
        drop(self.file.take());
        if !self.committed { let _ = fs::remove_file(&self.temporary); }
    }
}

fn read_unlocked<T: DeserializeOwned + Default>(path: &Path) -> Result<T, String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
        Err(e) => return Err(e.to_string()),
    };
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    serde_json::from_str(&content).map_err(|e| format!("Invalid configuration JSON; existing file was NOT overwritten: {e}"))
}

pub fn read_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T, String> {
    let path = destination(path)?;
    let _lock = lock_target(&path)?;
    read_unlocked(&path)
}

pub fn update_json<T, R>(path: &Path, update: impl FnOnce(&mut T) -> R) -> Result<R, String>
where T: DeserializeOwned + Default + Serialize {
    let mut file = AtomicFile::new(path)?;
    let mut data: T = read_unlocked(&file.target)?;
    let result = update(&mut data);
    serde_json::to_writer_pretty(&mut file, &data).map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())?;
    file.commit()?;
    Ok(result)
}

pub fn write_json<T: DeserializeOwned + Default + Serialize>(path: &Path, value: T) -> Result<(), String> {
    update_json(path, |current: &mut T| *current = value)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("recch-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap(); Self(path)
        }
        fn file(&self) -> PathBuf { self.0.join("data.json") }
    }
    impl Drop for TestDir { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
    #[test]
    fn cancellation_keeps_original_and_removes_only_temporary() {
        let dir = TestDir::new(); let target = dir.file();
        fs::write(&target, "old backup").unwrap();
        let temp;
        { let mut file = AtomicFile::new(&target).unwrap(); temp = file.temporary.clone(); file.write_all(b"partial").unwrap(); }
        assert_eq!(fs::read_to_string(&target).unwrap(), "old backup"); assert!(!temp.exists());
    }
    #[test]
    fn successful_commit_replaces_original_without_partial_visibility() {
        let dir = TestDir::new(); let target = dir.file(); fs::write(&target, "old").unwrap();
        let mut file = AtomicFile::new(&target).unwrap(); file.write_all(b"complete").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "old"); file.commit().unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "complete");
    }
    #[test]
    fn corrupt_configuration_is_not_silently_reset() {
        let dir = TestDir::new(); let target = dir.file(); fs::write(&target, "{broken").unwrap();
        assert!(update_json(&target, |values: &mut Vec<String>| values.push("new".into())).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "{broken");
    }
    #[test]
    fn missing_configuration_defaults_but_updates_preserve_existing_entries() {
        let dir = TestDir::new(); let target = dir.file();
        assert_eq!(read_json::<Vec<String>>(&target).unwrap(), Vec::<String>::new());
        update_json(&target, |v: &mut Vec<String>| v.push("a".into())).unwrap();
        update_json(&target, |v: &mut Vec<String>| v.push("b".into())).unwrap();
        assert_eq!(read_json::<Vec<String>>(&target).unwrap(), vec!["a", "b"]);
    }
    #[test]
    fn concurrent_writers_fail_instead_of_overwriting_each_other() {
        let dir = TestDir::new(); let target = dir.file();
        let first = AtomicFile::new(&target).unwrap(); assert!(AtomicFile::new(&target).is_err());
        drop(first); assert!(AtomicFile::new(&target).is_ok());
    }
    #[test]
    fn failed_replacement_keeps_destination_and_cleans_temporary() {
        let dir = TestDir::new(); let target = dir.file();
        let mut file = AtomicFile::new(&target).unwrap(); let temp = file.temporary.clone();
        file.write_all(b"new").unwrap(); fs::create_dir(&target).unwrap();
        assert!(file.commit().is_err()); assert!(target.is_dir()); assert!(!temp.exists());
    }
    #[cfg(unix)]
    #[test]
    fn saved_credentials_have_owner_only_permissions() {
        let dir = TestDir::new(); let target = dir.file();
        write_json(&target, vec!["secret".to_string()]).unwrap();
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
