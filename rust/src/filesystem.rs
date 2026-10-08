use crate::authority::{os_error_builtin_text, strerror_text};
use crate::observation::{
    failure, io_code, io_failure, io_failure_at, managed_metadata, managed_spelling,
};
use crate::observations::{ErrorCode, TargetError};
use crate::security::ensure_private;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

#[allow(dead_code)]
pub fn rename_exclusive(source: &Path, destination: &Path) -> Result<(), TargetError> {
    let renamed = rustix::fs::renameat_with(
        rustix::fs::CWD,
        source,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    );
    if let Err(errno) = renamed {
        let error = io::Error::from(errno);
        let code = error.raw_os_error().unwrap_or_default();
        return Err(TargetError::new(
            io_code(Some(code)),
            os_error_builtin_text(code, &strerror_text(&error, code), destination),
            Some(destination.to_path_buf()),
            Some(code),
        ));
    }
    Ok(())
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct TargetLockGuard {
    file: File,
}

impl Drop for TargetLockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[allow(dead_code)]
pub fn target_lock(
    state: &Path,
    timeout: Duration,
    mut on_created: Option<&mut dyn FnMut(&Path)>,
) -> Result<TargetLockGuard, TargetError> {
    managed_spelling(state)?;
    let lock_path = state.join("lock");
    match fs::symlink_metadata(&lock_path) {
        Ok(metadata) => managed_metadata(&metadata, &lock_path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_failure(&error, &lock_path)),
    }
    let mut missing: Vec<PathBuf> = Vec::new();
    let mut parent = state.to_path_buf();
    while !parent.exists() {
        let Some(next) = parent.parent().map(Path::to_path_buf) else {
            break;
        };
        missing.push(parent);
        parent = next;
    }
    for path in missing.iter().rev() {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|error| io_failure(&error, path))?;
        ensure_private(path)?;
        if let Some(callback) = on_created.as_mut() {
            callback(path);
        }
    }
    ensure_private(state)?;
    let file = open_lock_file(&lock_path)?;
    verify_lock_identity(state, &file)?;
    acquire_lock(|| attempt_lock(&file), timeout, &lock_path)?;
    Ok(TargetLockGuard { file })
}

pub fn open_lock_file(lock_path: &Path) -> Result<File, TargetError> {
    let created = OpenOptions::new()
        .read(true)
        .append(true)
        .create_new(true)
        .open(lock_path);
    match created {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(lock_path)
            .map_err(|error| io_failure(&error, lock_path)),
        Err(error) => Err(io_failure(&error, lock_path)),
    }
}

fn verify_lock_identity(state: &Path, file: &File) -> Result<(), TargetError> {
    let lock_path = state.join("lock");
    let observed = match fs::symlink_metadata(&lock_path) {
        Ok(metadata) => {
            managed_metadata(&metadata, &lock_path)?;
            Some(metadata)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(io_failure(&error, &lock_path)),
    };
    let opened = file
        .metadata()
        .map_err(|error| io_failure_at(&error, None))?;
    let changed = observed
        .is_none_or(|observed| (observed.dev(), observed.ino()) != (opened.dev(), opened.ino()));
    if changed {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "lock file changed",
            &lock_path,
        ));
    }
    Ok(())
}

enum LockAttempt {
    Acquired,
    Busy,
    Failed(io::Error),
}

const fn busy_errno(errno: i32) -> bool {
    errno == libc::EACCES || errno == libc::EAGAIN || errno == libc::EDEADLK
}

fn classify_lock_error(result: Result<(), std::fs::TryLockError>) -> LockAttempt {
    match result {
        Ok(()) => LockAttempt::Acquired,
        Err(std::fs::TryLockError::WouldBlock) => LockAttempt::Busy,
        Err(std::fs::TryLockError::Error(error)) => match error.raw_os_error() {
            Some(errno) if busy_errno(errno) => LockAttempt::Busy,
            _ => LockAttempt::Failed(error),
        },
    }
}

fn attempt_lock(file: &File) -> LockAttempt {
    classify_lock_error(file.try_lock())
}

fn acquire_lock(
    mut attempt: impl FnMut() -> LockAttempt,
    timeout: Duration,
    lock_path: &Path,
) -> Result<(), TargetError> {
    let started = Instant::now();
    loop {
        match attempt() {
            LockAttempt::Acquired => return Ok(()),
            LockAttempt::Failed(error) => return Err(io_failure_at(&error, None)),
            LockAttempt::Busy => {
                let remaining = timeout.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return Err(failure(ErrorCode::Busy, "target lock is busy", lock_path));
                }
                thread::sleep(remaining.min(Duration::from_millis(20)));
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::test_support::checkout_temp_dir;
    use std::fs::TryLockError;
    use std::os::unix::fs::PermissionsExt as _;

    fn mode_of(path: &Path) -> u32 {
        fs::symlink_metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
    }

    #[test]
    fn exclusive_rename_preserves_colliding_destinations() {
        for kind in ["empty", "nonempty", "file"] {
            let tmp = checkout_temp_dir(&format!("rename-{kind}"));
            let source = tmp.join("source");
            let destination = tmp.join("destination");
            fs::create_dir(&source).expect("source dir");
            fs::write(source.join("original"), b"source").expect("source payload");
            if kind == "file" {
                fs::write(&destination, b"destination").expect("destination file");
            } else {
                fs::create_dir(&destination).expect("destination dir");
                if kind == "nonempty" {
                    fs::write(destination.join("original"), b"destination")
                        .expect("destination payload");
                }
            }
            let error = rename_exclusive(&source, &destination).expect_err("collision must refuse");
            assert_eq!(error.code(), ErrorCode::IoError, "kind {kind}");
            assert_eq!(error.errno(), Some(libc::EEXIST), "kind {kind}");
            assert_eq!(error.path(), Some(destination.as_path()), "kind {kind}");
            assert_eq!(
                error.message(),
                format!(
                    "[Errno {}] File exists: PosixPath('{}')",
                    libc::EEXIST,
                    destination.display()
                ),
                "kind {kind}"
            );
            assert_eq!(
                fs::read(source.join("original")).expect("source intact"),
                b"source",
                "kind {kind}"
            );
            if kind == "file" {
                assert_eq!(
                    fs::read(&destination).expect("destination intact"),
                    b"destination",
                    "kind {kind}"
                );
            } else if kind == "nonempty" {
                assert_eq!(
                    fs::read(destination.join("original")).expect("destination intact"),
                    b"destination",
                    "kind {kind}"
                );
            } else {
                assert_eq!(
                    fs::read_dir(&destination)
                        .expect("destination intact")
                        .count(),
                    0,
                    "kind {kind}"
                );
            }
            let final_path = tmp.join("final");
            rename_exclusive(&source, &final_path).expect("absent destination renames");
            assert_eq!(
                fs::read(final_path.join("original")).expect("moved payload"),
                b"source",
                "kind {kind}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_exclusive_rename_propagates_the_noreplace_collision() {
        let tmp = checkout_temp_dir("rename-linux");
        let source = tmp.join("source");
        let destination = tmp.join("destination");
        fs::write(&source, b"source").expect("source payload");
        fs::write(&destination, b"destination").expect("destination payload");
        let error = rename_exclusive(&source, &destination).expect_err("collision refuses");
        assert_eq!(error.errno(), Some(libc::EEXIST));
        assert_eq!(fs::read(&source).expect("source intact"), b"source");
        assert_eq!(
            fs::read(&destination).expect("destination intact"),
            b"destination"
        );
    }

    #[test]
    fn lock_io_error_is_not_retried() {
        let mut calls = 0;
        let error = acquire_lock(
            || {
                calls += 1;
                LockAttempt::Failed(io::Error::from_raw_os_error(libc::EIO))
            },
            Duration::from_millis(500),
            Path::new("/state/lock"),
        )
        .expect_err("fatal lock errors propagate");
        assert_eq!(calls, 1);
        assert_eq!(error.code(), ErrorCode::IoError);
        assert_eq!(error.errno(), Some(libc::EIO));
        assert_eq!(error.message(), "[Errno 5] Input/output error");
        assert_eq!(error.path(), None);
    }

    #[test]
    fn busy_lock_errnos_retry_and_fatal_errnos_fail() {
        assert!(matches!(classify_lock_error(Ok(())), LockAttempt::Acquired));
        assert!(matches!(
            classify_lock_error(Err(TryLockError::WouldBlock)),
            LockAttempt::Busy
        ));
        for errno in [libc::EACCES, libc::EAGAIN, libc::EDEADLK] {
            assert!(
                matches!(
                    classify_lock_error(Err(TryLockError::Error(io::Error::from_raw_os_error(
                        errno
                    )))),
                    LockAttempt::Busy
                ),
                "errno {errno}"
            );
        }
        assert!(matches!(
            classify_lock_error(Err(TryLockError::Error(io::Error::from_raw_os_error(
                libc::EIO
            )))),
            LockAttempt::Failed(_)
        ));
    }

    #[test]
    fn zero_timeout_attempts_the_lock_once() {
        let mut calls = 0;
        let error = acquire_lock(
            || {
                calls += 1;
                LockAttempt::Busy
            },
            Duration::ZERO,
            Path::new("/state/lock"),
        )
        .expect_err("a zero timeout is a single attempt");
        assert_eq!(calls, 1);
        assert_eq!(error.code(), ErrorCode::Busy);
        assert_eq!(error.errno(), None);
        assert_eq!(error.message(), "target lock is busy");
        assert_eq!(error.path(), Some(Path::new("/state/lock")));
    }

    #[test]
    fn zero_timeout_locking_is_one_attempt_then_busy() {
        let tmp = checkout_temp_dir("lock-zero");
        let state = tmp.join("state");
        let guard = target_lock(&state, Duration::ZERO, None).expect("first lock acquires");
        let lock_path = state.join("lock");
        assert!(lock_path.is_file());
        assert_eq!(fs::read(&lock_path).expect("lock bytes"), b"");
        let started = Instant::now();
        let error = target_lock(&state, Duration::ZERO, None).expect_err("held lock is busy");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "a zero timeout must not wait"
        );
        assert_eq!(error.code(), ErrorCode::Busy);
        assert_eq!(error.message(), "target lock is busy");
        assert_eq!(error.path(), Some(lock_path.as_path()));
        assert_eq!(error.errno(), None);
        drop(guard);
        let released = target_lock(&state, Duration::ZERO, None).expect("release reacquires");
        assert_eq!(
            fs::read(state.join("lock")).expect("lock bytes stay empty"),
            b""
        );
        drop(released);
    }

    #[test]
    fn positive_deadline_retries_until_the_deadline() {
        let tmp = checkout_temp_dir("lock-deadline");
        let state = tmp.join("state");
        let _holder = target_lock(&state, Duration::ZERO, None).expect("holder acquires");
        let started = Instant::now();
        let error = target_lock(&state, Duration::from_millis(300), None)
            .expect_err("held past the deadline");
        assert!(started.elapsed() >= Duration::from_millis(300));
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(error.code(), ErrorCode::Busy);
        assert_eq!(error.path(), Some(state.join("lock").as_path()));
    }

    #[test]
    fn positive_deadline_acquires_a_released_lock() {
        let tmp = checkout_temp_dir("lock-release");
        let state = tmp.join("state");
        let holder = target_lock(&state, Duration::ZERO, None).expect("holder acquires");
        let releaser = thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            drop(holder);
        });
        let acquired =
            target_lock(&state, Duration::from_secs(5), None).expect("retries to acquire");
        releaser.join().expect("releaser thread");
        drop(acquired);
    }

    #[test]
    fn replaced_lock_is_detected() {
        let tmp = checkout_temp_dir("lock-replaced");
        let state = tmp.join("state");
        let first = target_lock(&state, Duration::ZERO, None).expect("first lock acquires");
        drop(first);
        let lock_path = state.join("lock");
        let opened = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&lock_path)
            .expect("open lock");
        let replacement = tmp.join("replacement");
        fs::write(&replacement, b"replacement").expect("replacement payload");
        fs::rename(&replacement, &lock_path).expect("replace the lock file");
        let error = verify_lock_identity(&state, &opened).expect_err("replaced lock refuses");
        assert_eq!(error.code(), ErrorCode::ConcurrentChange);
        assert_eq!(error.message(), "lock file changed");
        assert_eq!(error.path(), Some(lock_path.as_path()));
        fs::remove_file(&lock_path).expect("remove the lock file");
        let error = verify_lock_identity(&state, &opened).expect_err("vanished lock refuses");
        assert_eq!(error.code(), ErrorCode::ConcurrentChange);
        assert_eq!(error.message(), "lock file changed");
        assert_eq!(error.path(), Some(lock_path.as_path()));
    }

    #[test]
    fn first_lock_creates_the_permanent_lock_and_reuses_it() {
        let tmp = checkout_temp_dir("lock-first");
        let state = tmp.join("state");
        target_lock(&state, Duration::ZERO, None).expect("first install creates the lock");
        let lock_path = state.join("lock");
        assert!(lock_path.is_file());
        let first = fs::symlink_metadata(&lock_path).expect("lock metadata");
        target_lock(&state, Duration::ZERO, None).expect("existing lock is reused");
        let second = fs::symlink_metadata(&lock_path).expect("lock metadata again");
        assert_eq!(
            (first.dev(), first.ino()),
            (second.dev(), second.ino()),
            "the permanent lock file must be reused, not replaced"
        );
        assert_eq!(fs::read(&lock_path).expect("lock bytes"), b"");
    }

    #[test]
    fn target_lock_creates_private_missing_parents_shallowest_first() {
        let tmp = checkout_temp_dir("lock-missing");
        let created: std::cell::RefCell<Vec<PathBuf>> = std::cell::RefCell::new(Vec::new());
        let deep = tmp.join("a/b/c/state");
        let guard = target_lock(
            &deep,
            Duration::ZERO,
            Some(&mut |path: &Path| {
                created.borrow_mut().push(path.to_path_buf());
            }),
        )
        .expect("missing parents are created");
        drop(guard);
        let created = created.into_inner();
        assert_eq!(
            created,
            vec![
                tmp.join("a"),
                tmp.join("a/b"),
                tmp.join("a/b/c"),
                tmp.join("a/b/c/state"),
            ],
            "parents are created shallowest first"
        );
        for path in &created {
            assert_eq!(mode_of(path) & 0o777, 0o700, "parent {path:?}");
        }
        assert!(deep.join("lock").is_file());
        assert_eq!(mode_of(&deep) & 0o777, 0o700);
    }

    #[test]
    fn target_lock_refuses_permissive_management_state() {
        let tmp = checkout_temp_dir("lock-private");
        let state = tmp.join("state");
        fs::create_dir(&state).expect("state dir");
        fs::set_permissions(&state, fs::Permissions::from_mode(0o750)).expect("permissive mode");
        let error = target_lock(&state, Duration::ZERO, None).expect_err("permissive state");
        assert_eq!(error.code(), ErrorCode::Unsupported);
        assert_eq!(error.errno(), Some(libc::ENOTSUP));
        assert_eq!(
            error.message(),
            format!(
                "[Errno {}] management storage must be private to its owner: PosixPath('{}')",
                libc::ENOTSUP,
                state.display()
            )
        );
    }

    #[test]
    fn target_lock_refuses_symlinked_lock_files() {
        let tmp = checkout_temp_dir("lock-symlink");
        let state = tmp.join("state");
        fs::create_dir(&state).expect("state dir");
        let outside = tmp.join("outside");
        fs::write(&outside, b"outside").expect("outside payload");
        std::os::unix::fs::symlink(&outside, state.join("lock")).expect("lock symlink");
        let error = target_lock(&state, Duration::ZERO, None).expect_err("symlinked lock");
        assert_eq!(error.code(), ErrorCode::UnsafePath);
        assert_eq!(
            error.message(),
            "managed symlinks, reparse points and special files are unsupported"
        );
        assert_eq!(error.path(), Some(state.join("lock").as_path()));
    }

    #[test]
    fn target_lock_refuses_case_aliased_state_names() {
        let tmp = checkout_temp_dir("lock-alias");
        if crate::test_support::case_sensitive(&tmp) {
            eprintln!("skipping: host is case sensitive");
            return;
        }
        let physical = tmp.join("State");
        fs::create_dir(&physical).expect("physical state");
        let alias = tmp.join("state");
        let error = target_lock(&alias, Duration::ZERO, None).expect_err("case alias refuses");
        assert_eq!(error.code(), ErrorCode::UnsafePath);
        assert_eq!(error.message(), "managed directory spelling mismatch");
        assert_eq!(error.path(), Some(alias.as_path()));
    }
}
