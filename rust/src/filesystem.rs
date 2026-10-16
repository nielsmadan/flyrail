use crate::authority::{os_error_builtin_text, strerror_text};
use crate::observation::io_code;
use crate::observations::TargetError;
use crate::seam::{self, Seam};
use std::io;
use std::path::Path;

#[allow(dead_code)]
pub fn rename_exclusive(source: &Path, destination: &Path) -> Result<(), TargetError> {
    let event = |done| Seam::Rename {
        source,
        destination,
        done,
    };
    seam::fire(&event(false));
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
    seam::fire(&event(true));
    Ok(())
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
    use crate::observations::ErrorCode;
    use crate::test_support::checkout_temp_dir;
    use std::fs;

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
}
