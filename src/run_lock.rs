use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;

/// Name of the persistent lifetime lock inside the main memory root.
pub const LOCK_FILE: &str = "dispatcher.lock";

/// Exclusive dispatcher ownership of one data directory.
///
/// The descriptor is held for as long as this value lives, so a second process
/// fails closed before it can recover or write Session/Run state. The lock file
/// itself is never removed: the kernel releases the lock when the owning
/// process exits or is killed, so no stale-file heuristic is needed.
#[derive(Debug)]
pub struct DispatcherLock {
    file: File,
}

impl DispatcherLock {
    pub fn acquire(memory_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(memory_dir).map_err(|error| {
            format!(
                "Cannot create the lock directory {}: {error}",
                memory_dir.display()
            )
        })?;
        let path = memory_dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|error| open_error(&path, error))?;

        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            return Err(open_error(&path, io::Error::last_os_error()));
        }

        Ok(Self { file })
    }
}

impl Drop for DispatcherLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn open_error(path: &Path, error: io::Error) -> String {
    if error.kind() == io::ErrorKind::WouldBlock {
        return format!(
            "Another Iskra dispatcher already owns this data directory (lock: {})",
            path.display()
        );
    }
    if error.raw_os_error() == Some(libc::ELOOP) {
        return format!("Dispatcher lock must not be a symlink: {}", path.display());
    }
    format!("Cannot lock {}: {error}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "iskra-run-lock-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_second_dispatcher_fails_until_the_first_releases() {
        let dir = temp_dir("exclusive");
        let first = DispatcherLock::acquire(&dir).unwrap();
        let error = DispatcherLock::acquire(&dir).expect_err("a second lock must conflict");
        assert!(error.contains("Another Iskra dispatcher"), "{error}");

        drop(first);
        let third = DispatcherLock::acquire(&dir).unwrap();
        drop(third);

        assert!(
            dir.join(LOCK_FILE).exists(),
            "the lock file must survive a normal release"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_lock_file_is_refused() {
        let dir = temp_dir("symlink");
        let target = dir.join("target");
        fs::write(&target, b"").unwrap();
        std::os::unix::fs::symlink(&target, dir.join(LOCK_FILE)).unwrap();

        let error = DispatcherLock::acquire(&dir).expect_err("a symlinked lock file must fail");
        assert!(error.contains("symlink"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }
}
