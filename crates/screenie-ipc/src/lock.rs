//! Advisory file locks, for "only one of us at a time" between processes.

use std::fs::{File, OpenOptions, TryLockError};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

/// An exclusive `flock` on a file, released when dropped (or when the process dies, so
/// a crash never leaves it stuck).
#[derive(Debug)]
pub(crate) struct FileLock(#[allow(dead_code)] File);

impl FileLock {
    /// Wait as long as it takes for the lock.
    pub fn acquire(path: &Path) -> std::io::Result<FileLock> {
        let file = open(path)?;
        file.lock()?;
        Ok(FileLock(file))
    }

    /// Wait up to `patience` for the lock; `None` if it's still held then.
    pub fn acquire_within(path: &Path, patience: Duration) -> std::io::Result<Option<FileLock>> {
        let file = open(path)?;
        let deadline = Instant::now() + patience;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Some(FileLock(file))),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(TryLockError::WouldBlock) => return Ok(None),
                Err(TryLockError::Error(e)) => return Err(e),
            }
        }
    }
}

/// Open (creating) a lock file, in a private directory as the socket's is.
fn open(path: &Path) -> std::io::Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_lock_keeps_others_out_until_dropped() {
        let dir = std::env::temp_dir().join(format!("screenie-lock-{}", std::process::id()));
        let path = dir.join("test.lock");
        let held = FileLock::acquire(&path).unwrap();
        // flock locks belong to the open file, so a second open contends even in-process.
        assert!(
            FileLock::acquire_within(&path, Duration::ZERO)
                .unwrap()
                .is_none()
        );
        drop(held);
        assert!(
            FileLock::acquire_within(&path, Duration::ZERO)
                .unwrap()
                .is_some()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
