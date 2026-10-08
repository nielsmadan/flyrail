#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl std::ops::Deref for TempRoot {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn checkout_temp_dir(tag: &str) -> TempRoot {
    use std::os::unix::fs::DirBuilderExt as _;
    let unique = format!(
        "{}-{}-{tag}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../.cache/rust-tests")
        .join(unique);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(&root)
        .expect("security test root must be creatable");
    TempRoot(std::fs::canonicalize(&root).expect("security test root must resolve"))
}

#[cfg(target_os = "macos")]
fn raw_cstring(path: &Path) -> std::ffi::CString {
    use std::os::unix::ffi::OsStrExt as _;
    std::ffi::CString::new(path.as_os_str().as_bytes()).expect("path cstring")
}

#[cfg(target_os = "macos")]
fn set_xattr(path: &Path, name: &[u8], value: &[u8]) {
    let raw = raw_cstring(path);
    let mut terminated = name.to_vec();
    terminated.push(0);
    let result = unsafe {
        libc::setxattr(
            raw.as_ptr(),
            terminated.as_ptr().cast(),
            value.as_ptr().cast(),
            value.len(),
            0,
            1,
        )
    };
    assert_eq!(result, 0, "test xattr must be set");
}

#[cfg(target_os = "macos")]
fn remove_xattr(path: &Path, name: &[u8]) {
    let raw = raw_cstring(path);
    let mut terminated = name.to_vec();
    terminated.push(0);
    let result = unsafe { libc::removexattr(raw.as_ptr(), terminated.as_ptr().cast(), 1) };
    assert_eq!(result, 0, "test xattr must be removed");
}

#[cfg(target_os = "macos")]
const USER_IMMUTABLE: libc::c_uint = 0x0004;

#[cfg(target_os = "macos")]
const PROVENANCE: &[u8] = b"com.apple.provenance";

#[cfg(target_os = "macos")]
const EVERYONE_ACL_TEXT: &[u8] =
    b"!#acl 1\ngroup:ABCDEFAB-CDEF-ABCD-EFAB-CDEF0000000C:everyone:12:allow:read\n";

#[cfg(target_os = "macos")]
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(target_os = "macos")]
fn provenance_value(path: &Path) -> Option<Vec<u8>> {
    let raw = raw_cstring(path);
    let mut name = PROVENANCE.to_vec();
    name.push(0);
    let size = unsafe {
        libc::getxattr(
            raw.as_ptr(),
            name.as_ptr().cast(),
            std::ptr::null_mut(),
            0,
            0,
            1,
        )
    };
    if size <= 0 {
        return None;
    }
    let mut buffer = vec![0u8; usize::try_from(size).ok()?];
    unsafe {
        libc::getxattr(
            raw.as_ptr(),
            name.as_ptr().cast(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            0,
            1,
        )
    };
    Some(buffer)
}

#[cfg(target_os = "macos")]
fn set_flags(path: &Path, flags: libc::c_uint) {
    let raw = raw_cstring(path);
    let result = unsafe { libc::chflags(raw.as_ptr(), flags) };
    assert_eq!(result, 0, "test flags must be set");
}

#[cfg(target_os = "macos")]
fn run(command: &[&str]) {
    let (program, arguments) = command
        .split_first()
        .expect("native command must name a program");
    let status = Command::new(program)
        .args(arguments)
        .status()
        .expect("native command must spawn");
    assert!(status.success(), "command must succeed: {command:?}");
}

#[cfg(target_os = "macos")]
#[test]
fn native_acl_and_foreign_xattr_refuse_capture_before_publication() {
    let tmp = checkout_temp_dir("security-native");
    let path = tmp.join("AGENTS.md");
    std::fs::write(&path, b"foreign").expect("resource payload");
    let before = flyrail::security_capture(&path, false).expect("initial capture");
    assert_eq!(std::fs::read(&path).expect("resource bytes"), b"foreign");

    run(&[
        "chmod",
        "+a",
        "everyone allow read",
        &path.display().to_string(),
    ]);
    let error = flyrail::security_capture(&path, false).expect_err("ACL refuses capture");
    assert_eq!(error.code(), flyrail::ErrorCode::Unsupported);
    assert_eq!(error.errno(), Some(libc::ENOTSUP));
    assert_eq!(error.path(), Some(path.as_path()));
    assert_eq!(
        error.message(),
        format!(
            "[Errno {}] extended ACLs are unsupported: PosixPath('{}')",
            libc::ENOTSUP,
            path.display()
        )
    );
    assert_eq!(
        std::fs::read(&path).expect("resource bytes"),
        b"foreign",
        "the refused capture must not touch the resource"
    );

    let ancestor = flyrail::security_capture(&path, true).expect("ancestors capture the ACL");
    let mut xattrs: Vec<String> = Vec::new();
    if let Some(value) = provenance_value(&path) {
        xattrs.push(format!("[\"{}\",\"{}\"]", hex(PROVENANCE), hex(&value)));
    }
    let expected = format!(
        "{{\"acl\":\"{}\",\"flags\":0,\"schema_version\":1,\"xattrs\":[{}]}}",
        hex(EVERYONE_ACL_TEXT),
        xattrs.join(",")
    );
    assert_eq!(
        ancestor,
        expected.into_bytes(),
        "ancestor capture must carry the live ACL text"
    );
    assert_eq!(
        std::fs::read(&path).expect("resource bytes"),
        b"foreign",
        "the ancestor capture must not touch the resource"
    );

    set_xattr(&path, b"flyrail.test", b"value");
    set_flags(&path, USER_IMMUTABLE);
    let error = flyrail::security_capture(&path, false).expect_err("flags refuse first");
    assert_eq!(
        error.message(),
        format!(
            "[Errno {}] file flags cannot be preserved safely: PosixPath('{}')",
            libc::ENOTSUP,
            path.display()
        )
    );
    set_flags(&path, 0);
    let error = flyrail::security_capture(&path, false).expect_err("xattrs refuse second");
    assert_eq!(
        error.message(),
        format!(
            "[Errno {}] extended attributes are unsupported: PosixPath('{}')",
            libc::ENOTSUP,
            path.display()
        )
    );
    remove_xattr(&path, b"flyrail.test");
    let error = flyrail::security_capture(&path, false).expect_err("ACL refuses last");
    assert_eq!(
        error.message(),
        format!(
            "[Errno {}] extended ACLs are unsupported: PosixPath('{}')",
            libc::ENOTSUP,
            path.display()
        )
    );

    run(&["chmod", "-N", &path.display().to_string()]);
    let after = flyrail::security_capture(&path, false).expect("clean capture");
    assert_eq!(
        after, before,
        "removing the native metadata restores the capture"
    );
    assert_eq!(std::fs::read(&path).expect("resource bytes"), b"foreign");
}
