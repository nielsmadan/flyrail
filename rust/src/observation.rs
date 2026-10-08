use crate::authority::os_error_text;
use crate::observations::{ErrorCode, TargetError};
use crate::validation::portable_path_key;
use std::ffi::OsString;
use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};

pub fn failure(
    code: ErrorCode,
    message: impl Into<String>,
    path: impl Into<PathBuf>,
) -> TargetError {
    TargetError::new(code, message, Some(path.into()), None)
}

const fn is_unsupported(errno: i32) -> bool {
    errno == libc::ENOTSUP || errno == libc::ENOSYS || errno == libc::EXDEV || errno == libc::EINVAL
}

pub const fn io_code(errno: Option<i32>) -> ErrorCode {
    match errno {
        Some(errno) if is_unsupported(errno) => ErrorCode::Unsupported,
        _ => ErrorCode::IoError,
    }
}

pub fn io_failure(error: &io::Error, path: &Path) -> TargetError {
    io_failure_at(error, Some(path))
}

pub fn io_failure_at(error: &io::Error, path: Option<&Path>) -> TargetError {
    let errno = error.raw_os_error();
    TargetError::new(
        io_code(errno),
        os_error_text(error, path),
        path.map(Path::to_path_buf),
        errno,
    )
}

pub fn managed_metadata(metadata: &Metadata, path: &Path) -> Result<(), TargetError> {
    use std::os::unix::fs::MetadataExt as _;
    let file_type = metadata.file_type();
    if !(file_type.is_file() || file_type.is_dir())
        || (file_type.is_file() && metadata.nlink() != 1)
    {
        return Err(failure(
            ErrorCode::UnsafePath,
            "managed symlinks, reparse points and special files are unsupported",
            path,
        ));
    }
    Ok(())
}

pub fn managed_spelling(state_root: &Path) -> Result<(), TargetError> {
    let (Some(name), Some(parent)) = (state_root.file_name(), state_root.parent()) else {
        return Ok(());
    };
    match fs::symlink_metadata(parent) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_failure(&error, parent)),
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Err(failure(
                    ErrorCode::UnsafePath,
                    "resource ancestor is not an ordinary directory",
                    parent,
                ));
            }
        }
    }
    let key = portable_path_key(&name.to_string_lossy());
    let mut matches: Vec<OsString> = Vec::new();
    match fs::read_dir(parent) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_failure(&error, parent)),
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|error| io_failure(&error, parent))?;
                let stored = entry.file_name();
                if stored
                    .to_str()
                    .is_some_and(|text| portable_path_key(text) == key)
                {
                    matches.push(stored);
                }
            }
        }
    }
    if matches.len() > 1 {
        return Err(failure(
            ErrorCode::UnsafePath,
            "portable filename collision",
            state_root,
        ));
    }
    if matches.pop().is_some_and(|stored| stored != name) {
        return Err(failure(
            ErrorCode::UnsafePath,
            "managed directory spelling mismatch",
            state_root,
        ));
    }
    if let Ok(metadata) = fs::symlink_metadata(state_root) {
        managed_metadata(&metadata, state_root)?;
        if !metadata.is_dir() {
            return Err(failure(
                ErrorCode::UnsafePath,
                "expected a directory",
                state_root,
            ));
        }
    }
    Ok(())
}
