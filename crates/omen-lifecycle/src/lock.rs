//! Local lifecycle-operation mutual exclusion (H item 70).
//!
//! Prevents two consequential lifecycle mutations racing (update+update,
//! update+uninstall, repair+update, gc+migration). Smallest deterministic
//! coordination: an OS-advisory file lock in the state base. This is NOT
//! Resolve admission and grants no authority; it only serializes local
//! lifecycle mutation.
use crate::error::LifecycleError;
use std::path::{Path, PathBuf};

pub struct LifecycleLock {
    _file: std::fs::File,
    #[allow(dead_code)]
    path: PathBuf,
}

pub fn lock_path(base: &Path) -> PathBuf {
    base.join("update").join("lifecycle.lock")
}

/// Non-blocking acquire. Fails with [`LifecycleError::LockUnavailable`]
/// naming the holder path instead of waiting forever (IDO: nothing waits
/// forever).
pub fn acquire(base: &Path) -> Result<LifecycleLock, LifecycleError> {
    use fs2::FileExt;
    let path = lock_path(base);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LifecycleError::Io(e.to_string()))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| LifecycleError::Io(e.to_string()))?;
    file.try_lock_exclusive().map_err(|_| {
        LifecycleError::LockUnavailable(format!(
            "another lifecycle operation holds {}",
            path.display()
        ))
    })?;
    Ok(LifecycleLock { _file: file, path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_mutation_waits_nobody_it_fails() {
        let base = tempfile::tempdir().unwrap();
        let _first = acquire(base.path()).unwrap();
        match acquire(base.path()) {
            Err(e) => assert_eq!(e.phase(), "lock"),
            Ok(_) => panic!("second concurrent lifecycle mutation must fail"),
        }
    }
}
