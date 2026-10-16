use crate::authority::{os_error_text, sha256_hex};
use crate::json::python_str_repr;
use crate::observations::{ErrorCode, TargetError};
use crate::security::ensure_private;
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_KEY_LENGTH: usize = 200;
const MUTATION_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_POLL: Duration = Duration::from_millis(20);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreError {
    Busy {
        name: String,
    },
    Io {
        message: String,
        path: Option<PathBuf>,
        kind: io::ErrorKind,
        errno: Option<i32>,
    },
    InvalidKey {
        message: String,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy { name } => write!(formatter, "state store lock is busy: {name}"),
            Self::Io { message, .. } | Self::InvalidKey { message } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<StoreError> for TargetError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Busy { .. } => Self::new(ErrorCode::Busy, error.to_string(), None, None),
            StoreError::Io {
                message,
                path,
                errno,
                ..
            } => Self::new(crate::observation::io_code(errno), message, path, errno),
            StoreError::InvalidKey { message } => {
                Self::new(ErrorCode::InvalidState, message, None, None)
            }
        }
    }
}

#[must_use]
pub struct StoreLock<'a> {
    release: Option<Box<dyn FnOnce() + Send + 'a>>,
}

impl<'a> StoreLock<'a> {
    pub fn new(release: impl FnOnce() + Send + 'a) -> Self {
        Self {
            release: Some(Box::new(release)),
        }
    }
}

impl Drop for StoreLock<'_> {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}

impl fmt::Debug for StoreLock<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("StoreLock").finish_non_exhaustive()
    }
}

pub trait StateStore: Send + Sync {
    fn location(&self) -> Option<&Path> {
        None
    }
    fn staging_directory(&self) -> Option<PathBuf> {
        None
    }
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError>;
    fn write(&self, key: &str, value: &[u8], expected: Option<&[u8]>) -> Result<bool, StoreError>;
    fn delete(&self, key: &str, expected: &[u8]) -> Result<bool, StoreError>;
    fn keys(&self, prefix: &str) -> Result<Vec<String>, StoreError>;
    fn lock(&self, name: &str, timeout: Duration) -> Result<StoreLock<'_>, StoreError>;
}

fn valid_segment(segment: &str) -> bool {
    let mut bytes = segment.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

pub fn validate_key(key: &str, label: &str) -> Result<(), StoreError> {
    if key.len() > MAX_KEY_LENGTH || !key.split('/').all(valid_segment) {
        return Err(StoreError::InvalidKey {
            message: format!("{label} must be slash-separated lowercase segments"),
        });
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct MemoryStore {
    values: Mutex<BTreeMap<String, Vec<u8>>>,
    held: Mutex<BTreeSet<String>>,
    released: Condvar,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn values(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
        self.values.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn release(&self, name: &str) {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(name);
        self.released.notify_all();
    }
}

impl StateStore for MemoryStore {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        validate_key(key, "state key")?;
        Ok(self.values().get(key).cloned())
    }

    fn write(&self, key: &str, value: &[u8], expected: Option<&[u8]>) -> Result<bool, StoreError> {
        validate_key(key, "state key")?;
        let mut values = self.values();
        let matches = values.get(key).map(Vec::as_slice) == expected;
        if matches {
            values.insert(key.to_owned(), value.to_vec());
        }
        drop(values);
        Ok(matches)
    }

    fn delete(&self, key: &str, expected: &[u8]) -> Result<bool, StoreError> {
        validate_key(key, "state key")?;
        let mut values = self.values();
        let matches = values.get(key).map(Vec::as_slice) == Some(expected);
        if matches {
            values.remove(key);
        }
        drop(values);
        Ok(matches)
    }

    fn keys(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        Ok(self
            .values()
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect())
    }

    fn lock(&self, name: &str, timeout: Duration) -> Result<StoreLock<'_>, StoreError> {
        validate_key(name, "lock name")?;
        let started = Instant::now();
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        while held.contains(name) {
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(StoreError::Busy {
                    name: name.to_owned(),
                });
            }
            held = self
                .released
                .wait_timeout(held, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        held.insert(name.to_owned());
        drop(held);
        let owned = name.to_owned();
        Ok(StoreLock::new(move || self.release(&owned)))
    }
}

fn io_failure(error: &io::Error, path: &Path) -> StoreError {
    StoreError::Io {
        message: os_error_text(error, Some(path)),
        path: Some(path.to_path_buf()),
        kind: error.kind(),
        errno: error.raw_os_error(),
    }
}

fn require_private(paths: &[PathBuf]) -> Result<(), StoreError> {
    for path in paths {
        let Err(error) = ensure_private(path) else {
            continue;
        };
        match error.errno() {
            Some(libc::ENOENT) => {}
            Some(libc::ENOTSUP) => {
                return Err(StoreError::Io {
                    message: format!(
                        "[Errno {}] state store must be private to its owner: {}",
                        libc::EACCES,
                        python_str_repr(&path.to_string_lossy())
                    ),
                    path: Some(path.clone()),
                    kind: io::ErrorKind::PermissionDenied,
                    errno: Some(libc::EACCES),
                });
            }
            errno => {
                return Err(StoreError::Io {
                    message: error.message().to_owned(),
                    path: error.path().map(Path::to_path_buf),
                    kind: errno.map_or(io::ErrorKind::Other, |errno| {
                        io::Error::from_raw_os_error(errno).kind()
                    }),
                    errno,
                });
            }
        }
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), StoreError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_failure(&error, path))
}

fn private_directory(path: &Path) -> Result<(), StoreError> {
    let mut missing = Vec::new();
    let mut probe = path;
    while !probe.exists() {
        missing.push(probe);
        let Some(parent) = probe.parent() else {
            break;
        };
        probe = parent;
    }
    for directory in missing.iter().rev() {
        match fs::DirBuilder::new().mode(0o700).create(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_failure(&error, directory)),
        }
        if let Some(parent) = directory.parent() {
            sync_directory(parent)?;
        }
    }
    Ok(())
}

const fn busy_errno(errno: i32) -> bool {
    errno == libc::EACCES || errno == libc::EAGAIN || errno == libc::EDEADLK
}

fn hold_file_lock(
    directory: &Path,
    trusted: &[PathBuf],
    name: &str,
    timeout: Duration,
) -> Result<File, StoreError> {
    require_private(trusted)?;
    private_directory(directory)?;
    let path = directory.join(name.replace('/', "~"));
    let file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&path)
        .map_err(|error| io_failure(&error, &path))?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) if error.raw_os_error().is_some_and(busy_errno) => {}
            Err(TryLockError::Error(error)) => return Err(io_failure(&error, &path)),
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(StoreError::Busy {
                name: name.to_owned(),
            });
        }
        thread::sleep(remaining.min(LOCK_POLL));
    }
}

fn temporary_suffix() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(time.to_be_bytes());
    hasher.update(std::process::id().to_be_bytes());
    hasher.update(sequence.to_be_bytes());
    sha256_hex(&hasher.finalize()).chars().take(16).collect()
}

#[derive(Debug)]
pub struct DirectoryStore {
    root: PathBuf,
    records: PathBuf,
    trusted: [PathBuf; 4],
}

impl DirectoryStore {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        let root = std::path::absolute(path).map_err(|error| StoreError::Io {
            message: error.to_string(),
            path: Some(path.to_path_buf()),
            kind: error.kind(),
            errno: error.raw_os_error(),
        })?;
        let records = root.join("records");
        let trusted = [
            root.clone(),
            records.clone(),
            root.join("locks"),
            root.join("staging"),
        ];
        Ok(Self {
            root,
            records,
            trusted,
        })
    }

    fn record(&self, key: &str) -> Result<PathBuf, StoreError> {
        validate_key(key, "state key")?;
        Ok(self.records.join(key.replace('/', "~")))
    }

    fn mutation(&self) -> Result<StoreLock<'static>, StoreError> {
        let file = hold_file_lock(&self.root, &self.trusted, "mutation", MUTATION_TIMEOUT)?;
        Ok(StoreLock::new(move || drop(file.unlock())))
    }

    fn replace(&self, path: &Path, value: &[u8]) -> Result<(), StoreError> {
        private_directory(&self.records)?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let temporary = self
            .records
            .join(format!(".{name}.{}.tmp", temporary_suffix()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| io_failure(&error, &temporary))?;
        let published = file
            .write_all(value)
            .and_then(|()| file.sync_all())
            .map_err(|error| io_failure(&error, &temporary))
            .and_then(|()| fs::rename(&temporary, path).map_err(|error| io_failure(&error, path)));
        if published.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        published?;
        sync_directory(&self.records)
    }
}

impl StateStore for DirectoryStore {
    fn location(&self) -> Option<&Path> {
        Some(&self.root)
    }

    fn staging_directory(&self) -> Option<PathBuf> {
        Some(self.root.join("staging"))
    }

    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let path = self.record(key)?;
        require_private(&self.trusted)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_failure(&error, &path)),
        }
    }

    fn write(&self, key: &str, value: &[u8], expected: Option<&[u8]>) -> Result<bool, StoreError> {
        let path = self.record(key)?;
        let _mutation = self.mutation()?;
        if self.read(key)?.as_deref() != expected {
            return Ok(false);
        }
        self.replace(&path, value)?;
        Ok(true)
    }

    fn delete(&self, key: &str, expected: &[u8]) -> Result<bool, StoreError> {
        let path = self.record(key)?;
        let _mutation = self.mutation()?;
        if self.read(key)?.as_deref() != Some(expected) {
            return Ok(false);
        }
        fs::remove_file(&path).map_err(|error| io_failure(&error, &path))?;
        sync_directory(&self.records)?;
        Ok(true)
    }

    fn keys(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        require_private(&self.trusted)?;
        let entries = match fs::read_dir(&self.records) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io_failure(&error, &self.records)),
        };
        let mut keys = Vec::new();
        for entry in entries {
            let name = entry
                .map_err(|error| io_failure(&error, &self.records))?
                .file_name()
                .to_string_lossy()
                .into_owned();
            if !name.starts_with('.') {
                keys.push(name.replace('~', "/"));
            }
        }
        keys.retain(|key| key.starts_with(prefix));
        keys.sort();
        Ok(keys)
    }

    fn lock(&self, name: &str, timeout: Duration) -> Result<StoreLock<'_>, StoreError> {
        validate_key(name, "lock name")?;
        let file = hold_file_lock(&self.root.join("locks"), &self.trusted, name, timeout)?;
        Ok(StoreLock::new(move || drop(file.unlock())))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn memory_store_meets_the_contract() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStore::new());
        crate::testing::check_store(|| Arc::clone(&store)).unwrap();
    }

    #[test]
    fn directory_store_meets_the_contract() {
        let root = crate::test_support::checkout_temp_dir("directory-store-contract");
        let path = root.join("state");
        crate::testing::check_store(|| {
            Arc::new(DirectoryStore::new(&path).unwrap()) as Arc<dyn StateStore>
        })
        .unwrap();
    }

    #[test]
    fn check_store_names_the_guarantee_a_broken_store_misses() {
        struct IgnoresExpected(MemoryStore);
        impl StateStore for IgnoresExpected {
            fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
                self.0.read(key)
            }
            fn write(
                &self,
                key: &str,
                value: &[u8],
                _expected: Option<&[u8]>,
            ) -> Result<bool, StoreError> {
                let current = self.0.read(key)?;
                self.0.write(key, value, current.as_deref())?;
                Ok(true)
            }
            fn delete(&self, key: &str, expected: &[u8]) -> Result<bool, StoreError> {
                self.0.delete(key, expected)
            }
            fn keys(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
                self.0.keys(prefix)
            }
            fn lock(&self, name: &str, timeout: Duration) -> Result<StoreLock<'_>, StoreError> {
                self.0.lock(name, timeout)
            }
        }
        let store: Arc<dyn StateStore> = Arc::new(IgnoresExpected(MemoryStore::new()));
        let error = crate::testing::check_store(|| Arc::clone(&store)).unwrap_err();
        assert_eq!(error, "creating an existing key must fail");
    }

    #[test]
    fn directory_store_keeps_private_records_and_refuses_a_public_root() {
        use std::os::unix::fs::PermissionsExt;
        let root = crate::test_support::checkout_temp_dir("directory-store-private");
        let store = DirectoryStore::new(root.join("state")).unwrap();
        assert!(store.write("index/app/x", b"r", None).unwrap());
        let records = root.join("state/records");
        assert_eq!(
            std::fs::metadata(&records).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(records.join("index~app~x"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let public = root.join("public");
        std::fs::create_dir(&public).unwrap();
        std::fs::set_permissions(&public, std::fs::Permissions::from_mode(0o777)).unwrap();
        let refused = DirectoryStore::new(&public)
            .unwrap()
            .read("index/app/x")
            .unwrap_err();
        assert!(matches!(
            refused,
            StoreError::Io {
                kind: std::io::ErrorKind::PermissionDenied,
                ..
            }
        ));
    }

    #[test]
    fn io_errors_keep_their_errno_and_map_unsupported_operations() {
        let failure = |errno| StoreError::Io {
            message: String::from("failed"),
            path: Some(PathBuf::from("/store")),
            kind: io::Error::from_raw_os_error(errno).kind(),
            errno: Some(errno),
        };
        for errno in [libc::ENOTSUP, libc::ENOSYS, libc::EXDEV, libc::EINVAL] {
            let error = TargetError::from(failure(errno));
            assert_eq!(error.code(), ErrorCode::Unsupported, "{errno}");
            assert_eq!(error.errno(), Some(errno));
        }
        let error = TargetError::from(failure(libc::EIO));
        assert_eq!(error.code(), ErrorCode::IoError);
        assert_eq!(error.errno(), Some(libc::EIO));
        assert_eq!(error.path(), Some(Path::new("/store")));
    }

    #[test]
    fn staging_directories() {
        let root = crate::test_support::checkout_temp_dir("staging-directories");
        assert_eq!(
            DirectoryStore::new(root.join("state"))
                .unwrap()
                .staging_directory(),
            Some(root.join("state/staging"))
        );
        assert_eq!(MemoryStore::new().staging_directory(), None);
    }
}
