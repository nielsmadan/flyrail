use crate::authority::os_error_builtin_text;
use crate::observation::{io_code, io_failure};
use crate::observations::{ErrorCode, TargetError};
use std::ffi::CString;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

const PROVENANCE: &[u8] = b"com.apple.provenance";

#[cfg(target_os = "macos")]
const XATTR_NOFOLLOW: libc::c_int = 1;

#[cfg(target_os = "macos")]
const ACL_TYPE_EXTENDED: libc::c_int = 0x100;

#[cfg(target_os = "macos")]
const ACL_FIRST_ENTRY: libc::c_int = 0;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn acl_get_file(path: *const libc::c_char, acl_type: libc::c_int) -> *mut libc::c_void;
    fn acl_get_entry(
        acl: *mut libc::c_void,
        entry_id: libc::c_int,
        entry: *mut *mut libc::c_void,
    ) -> libc::c_int;
    fn acl_to_text(acl: *mut libc::c_void, length: *mut libc::ssize_t) -> *mut libc::c_char;
    fn acl_free(acl: *mut libc::c_void) -> libc::c_int;
}

fn path_cstring(path: &Path) -> Result<CString, TargetError> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| TargetError::new(ErrorCode::InvalidState, "embedded null byte", None, None))
}

fn refused(errno: i32, message: &str, path: &Path) -> TargetError {
    TargetError::new(
        io_code(Some(errno)),
        os_error_builtin_text(errno, message, path),
        Some(path.to_path_buf()),
        Some(errno),
    )
}

fn last_errno() -> i32 {
    io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or_default()
}

fn positive_size(size: libc::ssize_t) -> usize {
    usize::try_from(size).unwrap_or_default()
}

pub fn security(path: &Path, ancestor: bool) -> Result<Vec<u8>, TargetError> {
    let flags = file_flags(path)?;
    if flags != 0 && !ancestor {
        return Err(refused(
            libc::ENOTSUP,
            "file flags cannot be preserved safely",
            path,
        ));
    }
    #[cfg(target_os = "macos")]
    return darwin_capture(path, ancestor, flags);
    #[cfg(not(target_os = "macos"))]
    return linux_capture(path, ancestor, flags);
}

#[cfg(target_os = "macos")]
fn file_flags(path: &Path) -> Result<u32, TargetError> {
    let raw = path_cstring(path)?;
    let mut metadata: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::lstat(raw.as_ptr(), std::ptr::addr_of_mut!(metadata)) } != 0 {
        return Err(io_failure(&io::Error::last_os_error(), path));
    }
    Ok(metadata.st_flags)
}

#[cfg(not(target_os = "macos"))]
fn file_flags(path: &Path) -> Result<u32, TargetError> {
    fs::symlink_metadata(path).map_err(|error| io_failure(&error, path))?;
    Ok(0)
}

#[cfg(target_os = "macos")]
fn attribute_names(buffer: &[u8]) -> Vec<Vec<u8>> {
    let stripped = strip_trailing_nuls(buffer);
    let mut names: Vec<Vec<u8>> = if stripped.is_empty() {
        vec![Vec::new()]
    } else {
        stripped
            .split(|byte| *byte == 0)
            .map(<[u8]>::to_vec)
            .collect()
    };
    names.sort();
    names.dedup();
    names
}

fn strip_trailing_nuls(buffer: &[u8]) -> &[u8] {
    let end = buffer
        .iter()
        .rposition(|byte| *byte != 0)
        .map_or(0, |end| end + 1);
    buffer.get(..end).unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn attribute_value(raw: &CString, attribute: &[u8], path: &Path) -> Result<Vec<u8>, TargetError> {
    let mut name = attribute.to_vec();
    name.push(0);
    let size = unsafe {
        libc::getxattr(
            raw.as_ptr(),
            name.as_ptr().cast(),
            std::ptr::null_mut(),
            0,
            0,
            XATTR_NOFOLLOW,
        )
    };
    if size < 0 {
        return Err(refused(last_errno(), "cannot read extended metadata", path));
    }
    let mut payload = vec![0u8; positive_size(size)];
    let filled = unsafe {
        libc::getxattr(
            raw.as_ptr(),
            name.as_ptr().cast(),
            payload.as_mut_ptr().cast(),
            payload.len(),
            0,
            XATTR_NOFOLLOW,
        )
    };
    if filled != size {
        return Err(refused(libc::EAGAIN, "extended metadata changed", path));
    }
    Ok(payload)
}

#[cfg(target_os = "macos")]
fn darwin_capture(path: &Path, ancestor: bool, flags: u32) -> Result<Vec<u8>, TargetError> {
    let raw = path_cstring(path)?;
    let count = unsafe { libc::listxattr(raw.as_ptr(), std::ptr::null_mut(), 0, XATTR_NOFOLLOW) };
    if count < 0 {
        return Err(refused(
            last_errno(),
            "cannot inspect extended attributes",
            path,
        ));
    }
    let mut values: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut provenance: Vec<u8> = Vec::new();
    if count > 0 {
        let mut names = vec![0u8; positive_size(count)];
        let filled = unsafe {
            libc::listxattr(
                raw.as_ptr(),
                names.as_mut_ptr().cast(),
                names.len(),
                XATTR_NOFOLLOW,
            )
        };
        if filled != count {
            return Err(refused(libc::EAGAIN, "extended attributes changed", path));
        }
        let attributes = attribute_names(&names);
        if !ancestor && attributes.iter().any(|name| name != PROVENANCE) {
            return Err(refused(
                libc::ENOTSUP,
                "extended attributes are unsupported",
                path,
            ));
        }
        for attribute in &attributes {
            let payload = attribute_value(&raw, attribute, path)?;
            values.push((attribute.clone(), payload));
        }
        if !ancestor && let Some((_, payload)) = values.first() {
            provenance.extend_from_slice(PROVENANCE);
            provenance.push(0);
            provenance.extend_from_slice(payload);
        }
    }
    let acl_text = darwin_acl_text(path, &raw, ancestor)?;
    Ok(if ancestor {
        ancestor_metadata(flags, values, &acl_text)
    } else {
        provenance
    })
}

#[cfg(target_os = "macos")]
fn darwin_acl_text(path: &Path, raw: &CString, ancestor: bool) -> Result<Vec<u8>, TargetError> {
    let acl = unsafe { acl_get_file(raw.as_ptr(), ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        let errno = last_errno();
        if errno == libc::ENOENT {
            fs::symlink_metadata(path).map_err(|error| io_failure(&error, path))?;
            return Ok(Vec::new());
        }
        return Err(refused(errno, "cannot inspect file ACL", path));
    }
    let outcome = unsafe {
        let mut entry: *mut libc::c_void = std::ptr::null_mut();
        let code = acl_get_entry(acl, ACL_FIRST_ENTRY, std::ptr::addr_of_mut!(entry));
        let mut refusal: Option<TargetError> = None;
        let mut text: Vec<u8> = Vec::new();
        if code != -1 {
            if ancestor {
                let mut length: libc::ssize_t = 0;
                let payload = acl_to_text(acl, std::ptr::addr_of_mut!(length));
                if payload.is_null() {
                    refusal = Some(refused(last_errno(), "cannot capture ancestor ACL", path));
                } else {
                    if length > 0 {
                        text.extend_from_slice(std::slice::from_raw_parts(
                            payload.cast(),
                            positive_size(length),
                        ));
                    }
                    acl_free(payload.cast());
                }
            } else {
                refusal = Some(refused(
                    libc::ENOTSUP,
                    "extended ACLs are unsupported",
                    path,
                ));
            }
        }
        acl_free(acl);
        (text, refusal)
    };
    match outcome {
        (text, None) => Ok(text),
        (_, Some(refusal)) => Err(refusal),
    }
}

#[cfg(not(target_os = "macos"))]
fn linux_capture(path: &Path, ancestor: bool, flags: u32) -> Result<Vec<u8>, TargetError> {
    let raw = path_cstring(path)?;
    let names = list_attribute_names(&raw, path)?;
    if !names.is_empty() && !ancestor {
        return Err(refused(
            libc::ENOTSUP,
            "extended attributes and ACLs are unsupported",
            path,
        ));
    }
    let mut values: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    if ancestor {
        for name in names {
            let payload = attribute_payload(&raw, &name, path)?;
            values.push((name, payload));
        }
    }
    Ok(if ancestor {
        ancestor_metadata(flags, values, b"")
    } else {
        Vec::new()
    })
}

#[cfg(not(target_os = "macos"))]
fn list_attribute_names(raw: &CString, path: &Path) -> Result<Vec<Vec<u8>>, TargetError> {
    loop {
        let size = unsafe { libc::llistxattr(raw.as_ptr(), std::ptr::null_mut(), 0) };
        if size < 0 {
            return Err(io_failure(&io::Error::last_os_error(), path));
        }
        if size == 0 {
            return Ok(Vec::new());
        }
        let mut buffer = vec![0u8; positive_size(size)];
        let filled =
            unsafe { libc::llistxattr(raw.as_ptr(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if filled < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ERANGE) {
                continue;
            }
            return Err(io_failure(&error, path));
        }
        if positive_size(filled) == buffer.len() {
            return Ok(parse_names(&buffer));
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn parse_names(buffer: &[u8]) -> Vec<Vec<u8>> {
    let mut names: Vec<Vec<u8>> = Vec::new();
    let mut start = 0;
    for (offset, byte) in buffer.iter().enumerate() {
        if *byte == 0 {
            names.push(buffer.get(start..offset).unwrap_or_default().to_vec());
            start = offset + 1;
        }
    }
    if start < buffer.len() {
        names.push(buffer.get(start..).unwrap_or_default().to_vec());
    }
    names
}

#[cfg(not(target_os = "macos"))]
fn attribute_payload(raw: &CString, name: &[u8], path: &Path) -> Result<Vec<u8>, TargetError> {
    let mut terminated = name.to_vec();
    terminated.push(0);
    loop {
        let size = unsafe {
            libc::lgetxattr(
                raw.as_ptr(),
                terminated.as_ptr().cast(),
                std::ptr::null_mut(),
                0,
            )
        };
        if size < 0 {
            return Err(io_failure(&io::Error::last_os_error(), path));
        }
        let mut buffer = vec![0u8; positive_size(size)];
        let filled = unsafe {
            libc::lgetxattr(
                raw.as_ptr(),
                terminated.as_ptr().cast(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            )
        };
        if filled < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ERANGE) {
                continue;
            }
            return Err(io_failure(&error, path));
        }
        if positive_size(filled) == buffer.len() {
            return Ok(buffer);
        }
    }
}

fn ancestor_metadata(flags: u32, attributes: Vec<(Vec<u8>, Vec<u8>)>, acl: &[u8]) -> Vec<u8> {
    let mut sorted = attributes;
    sorted.sort();
    let mut out = String::from("{\"acl\":\"");
    extend_hex(&mut out, acl);
    out.push_str("\",\"flags\":");
    let _ = write!(out, "{flags}");
    out.push_str(",\"schema_version\":1,\"xattrs\":[");
    for (index, (name, value)) in sorted.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("[\"");
        extend_hex(&mut out, name);
        out.push_str("\",\"");
        extend_hex(&mut out, value);
        out.push_str("\"]");
    }
    out.push_str("]}");
    out.into_bytes()
}

fn extend_hex(out: &mut String, bytes: &[u8]) {
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
}

pub fn ensure_private(path: &Path) -> Result<(), TargetError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_failure(&error, path))?;
    if metadata.mode() & 0o077 != 0 || metadata.uid() != effective_uid() {
        return Err(refused(
            libc::ENOTSUP,
            "management storage must be private to its owner",
            path,
        ));
    }
    security(path, false).map(|_| ())
}

fn effective_uid() -> u32 {
    unsafe { libc::geteuid() }
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

    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("mode");
    }

    fn raw_cstring(path: &Path) -> CString {
        CString::new(path.as_os_str().as_bytes()).expect("path cstring")
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
                XATTR_NOFOLLOW,
            )
        };
        if size <= 0 {
            return None;
        }
        let mut buffer = vec![0u8; positive_size(size)];
        unsafe {
            libc::getxattr(
                raw.as_ptr(),
                name.as_ptr().cast(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                0,
                XATTR_NOFOLLOW,
            )
        };
        Some(buffer)
    }

    #[cfg(target_os = "macos")]
    fn set_test_xattr(path: &Path, name: &[u8], value: &[u8]) {
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
                XATTR_NOFOLLOW,
            )
        };
        assert_eq!(result, 0, "test xattr must be set");
    }

    #[cfg(target_os = "macos")]
    fn remove_test_xattr(path: &Path, name: &[u8]) {
        let raw = raw_cstring(path);
        let mut terminated = name.to_vec();
        terminated.push(0);
        let result =
            unsafe { libc::removexattr(raw.as_ptr(), terminated.as_ptr().cast(), XATTR_NOFOLLOW) };
        assert_eq!(result, 0, "test xattr must be removed");
    }

    #[cfg(target_os = "macos")]
    const USER_IMMUTABLE: libc::c_uint = 0x0004;

    #[cfg(target_os = "macos")]
    fn set_flags(path: &Path, flags: libc::c_uint) {
        let raw = raw_cstring(path);
        let result = unsafe { libc::chflags(raw.as_ptr(), flags) };
        assert_eq!(result, 0, "test flags must be set");
    }

    #[cfg(not(target_os = "macos"))]
    fn set_test_xattr(path: &Path, name: &[u8], value: &[u8]) {
        let raw = raw_cstring(path);
        let mut terminated = name.to_vec();
        terminated.push(0);
        let result = unsafe {
            libc::lsetxattr(
                raw.as_ptr(),
                terminated.as_ptr().cast(),
                value.as_ptr().cast(),
                value.len(),
                0,
            )
        };
        assert_eq!(result, 0, "test xattr must be set");
    }

    #[cfg(not(target_os = "macos"))]
    fn remove_test_xattr(path: &Path, name: &[u8]) {
        let raw = raw_cstring(path);
        let mut terminated = name.to_vec();
        terminated.push(0);
        let result = unsafe { libc::lremovexattr(raw.as_ptr(), terminated.as_ptr().cast()) };
        assert_eq!(result, 0, "test xattr must be removed");
    }

    fn hex_text(bytes: &[u8]) -> String {
        let mut out = String::new();
        extend_hex(&mut out, bytes);
        out
    }

    #[test]
    fn ancestor_metadata_pins_exact_bytes() {
        assert_eq!(
            ancestor_metadata(0, Vec::new(), b""),
            b"{\"acl\":\"\",\"flags\":0,\"schema_version\":1,\"xattrs\":[]}".to_vec()
        );
        assert_eq!(
            ancestor_metadata(
                0,
                vec![(b"flyrail.test".to_vec(), b"value".to_vec())],
                b""
            ),
            b"{\"acl\":\"\",\"flags\":0,\"schema_version\":1,\"xattrs\":[[\"666c797261696c2e74657374\",\"76616c7565\"]]}"
                .to_vec()
        );
        assert_eq!(
            ancestor_metadata(
                4,
                vec![
                    (b"zeta".to_vec(), b"2".to_vec()),
                    (b"alpha".to_vec(), b"1".to_vec()),
                ],
                b"user:joe:allow:read\n"
            ),
            format!(
                "{{\"acl\":\"{}\",\"flags\":4,\"schema_version\":1,\"xattrs\":[[\"{}\",\"{}\"],[\"{}\",\"{}\"]]}}",
                hex_text(b"user:joe:allow:read\n"),
                hex_text(b"alpha"),
                hex_text(b"1"),
                hex_text(b"zeta"),
                hex_text(b"2")
            )
            .into_bytes()
        );
    }

    #[test]
    fn capture_of_a_missing_path_reports_the_python_oserror() {
        let tmp = checkout_temp_dir("security-missing");
        let missing = tmp.join("missing.txt");
        for ancestor in [false, true] {
            let error = security(&missing, ancestor).expect_err("missing path refuses");
            assert_eq!(error.code(), ErrorCode::IoError, "ancestor {ancestor}");
            assert_eq!(error.errno(), Some(libc::ENOENT), "ancestor {ancestor}");
            assert_eq!(
                error.message(),
                format!(
                    "[Errno {}] No such file or directory: {}",
                    libc::ENOENT,
                    crate::json::python_str_repr(&missing.to_string_lossy())
                ),
                "ancestor {ancestor}"
            );
            assert_eq!(error.path(), Some(missing.as_path()), "ancestor {ancestor}");
        }
    }

    #[test]
    fn ensure_private_refuses_foreign_group_or_other_access() {
        let tmp = checkout_temp_dir("security-private-modes");
        for mode in [0o750u32, 0o755, 0o070, 0o007] {
            let state = tmp.join(format!("state-{mode:o}"));
            fs::create_dir(&state).expect("state dir");
            set_mode(&state, mode);
            let error = ensure_private(&state).expect_err("non-private mode refuses");
            assert_eq!(error.code(), ErrorCode::Unsupported, "mode {mode:o}");
            assert_eq!(error.errno(), Some(libc::ENOTSUP), "mode {mode:o}");
            assert_eq!(
                error.message(),
                format!(
                    "[Errno {}] management storage must be private to its owner: PosixPath('{}')",
                    libc::ENOTSUP,
                    state.display()
                ),
                "mode {mode:o}"
            );
        }
        let state = tmp.join("state-private");
        fs::create_dir(&state).expect("state dir");
        set_mode(&state, 0o700);
        ensure_private(&state).expect("owner-only mode passes");
    }

    #[test]
    fn ensure_private_checks_the_payload_file_not_only_its_parent() {
        let tmp = checkout_temp_dir("security-private-payload");
        let parent = tmp.join("state");
        fs::create_dir(&parent).expect("parent dir");
        set_mode(&parent, 0o700);
        let backup = parent.join("backup");
        fs::write(&backup, b"backup").expect("backup payload");
        set_mode(&backup, 0o640);
        let error = ensure_private(&backup).expect_err("group-readable payload refuses");
        assert_eq!(error.code(), ErrorCode::Unsupported);
        assert_eq!(error.errno(), Some(libc::ENOTSUP));
        assert_eq!(error.path(), Some(backup.as_path()));
        set_mode(&backup, 0o600);
        ensure_private(&backup).expect("owner-only payload passes");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn resource_capture_frames_the_provenance_value() {
        let tmp = checkout_temp_dir("security-provenance");
        let target = tmp.join("resource.txt");
        fs::write(&target, b"payload").expect("payload");
        let capture = security(&target, false).expect("resource capture");
        let expected = provenance_value(&target).map_or_else(Vec::new, |value| {
            let mut expected = PROVENANCE.to_vec();
            expected.push(0);
            expected.extend_from_slice(&value);
            expected
        });
        assert_eq!(capture, expected);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ancestor_capture_pins_the_json_framing_on_real_files() {
        let tmp = checkout_temp_dir("security-ancestor-framing");
        let target = tmp.join("resource.txt");
        fs::write(&target, b"payload").expect("payload");
        set_test_xattr(&target, b"flyrail.test", b"value");
        let capture = security(&target, true).expect("ancestor capture");
        let provenance = provenance_value(&target);
        let mut expected =
            String::from("{\"acl\":\"\",\"flags\":0,\"schema_version\":1,\"xattrs\":[");
        if let Some(value) = &provenance {
            let _ = write!(
                expected,
                "[\"{}\",\"{}\"],",
                hex_text(PROVENANCE),
                hex_text(value)
            );
        }
        let _ = write!(
            expected,
            "[\"{}\",\"{}\"]",
            hex_text(b"flyrail.test"),
            hex_text(b"value")
        );
        expected.push_str("]}");
        assert_eq!(capture, expected.into_bytes());
        remove_test_xattr(&target, b"flyrail.test");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn foreign_xattrs_refuse_resources_and_management_storage() {
        let tmp = checkout_temp_dir("security-xattr");
        let target = tmp.join("resource.txt");
        fs::write(&target, b"payload").expect("payload");
        set_test_xattr(&target, b"flyrail.test", b"value");
        let error = security(&target, false).expect_err("foreign xattr refuses capture");
        assert_eq!(error.code(), ErrorCode::Unsupported);
        assert_eq!(error.errno(), Some(libc::ENOTSUP));
        assert_eq!(error.path(), Some(target.as_path()));
        assert_eq!(
            error.message(),
            format!(
                "[Errno {}] extended attributes are unsupported: PosixPath('{}')",
                libc::ENOTSUP,
                target.display()
            )
        );
        let ancestor = security(&target, true).expect("ancestors capture foreign xattrs");
        let framing = String::from_utf8_lossy(&ancestor).into_owned();
        assert!(
            framing.contains(&format!(
                "[\"{}\",\"{}\"]",
                hex_text(b"flyrail.test"),
                hex_text(b"value")
            )),
            "ancestor capture must include the foreign xattr: {framing}"
        );
        let state = tmp.join("state");
        fs::create_dir(&state).expect("state dir");
        set_mode(&state, 0o700);
        set_test_xattr(&state, b"flyrail.test", b"value");
        let error = ensure_private(&state).expect_err("foreign xattr refuses management storage");
        assert_eq!(error.code(), ErrorCode::Unsupported);
        assert_eq!(
            error.message(),
            format!(
                "[Errno {}] extended attributes are unsupported: PosixPath('{}')",
                libc::ENOTSUP,
                state.display()
            )
        );
        remove_test_xattr(&target, b"flyrail.test");
        remove_test_xattr(&state, b"flyrail.test");
        security(&target, false).expect("clean capture");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn file_flags_refuse_resources_and_are_captured_for_ancestors() {
        let tmp = checkout_temp_dir("security-flags");
        let target = tmp.join("resource.txt");
        fs::write(&target, b"payload").expect("payload");
        set_test_xattr(&target, b"flyrail.test", b"value");
        set_flags(&target, USER_IMMUTABLE);
        let error = security(&target, false).expect_err("file flags refuse resources");
        assert_eq!(error.code(), ErrorCode::Unsupported);
        assert_eq!(error.errno(), Some(libc::ENOTSUP));
        assert_eq!(
            error.message(),
            format!(
                "[Errno {}] file flags cannot be preserved safely: PosixPath('{}')",
                libc::ENOTSUP,
                target.display()
            )
        );
        let ancestor = security(&target, true).expect("ancestors capture file flags");
        assert!(
            ancestor.starts_with(b"{\"acl\":\"\",\"flags\":4,\"schema_version\":1,\"xattrs\":["),
            "ancestor capture must frame the flags: {}",
            String::from_utf8_lossy(&ancestor)
        );
        set_flags(&target, 0);
        let error = security(&target, false).expect_err("xattrs refuse after flags clear");
        assert_eq!(
            error.message(),
            format!(
                "[Errno {}] extended attributes are unsupported: PosixPath('{}')",
                libc::ENOTSUP,
                target.display()
            )
        );
        remove_test_xattr(&target, b"flyrail.test");
        set_flags(&target, USER_IMMUTABLE);
        let error = security(&target, false).expect_err("flags refuse without xattrs");
        assert_eq!(
            error.message(),
            format!(
                "[Errno {}] file flags cannot be preserved safely: PosixPath('{}')",
                libc::ENOTSUP,
                target.display()
            )
        );
        set_flags(&target, 0);
        security(&target, false).expect("clean capture");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn linux_xattrs_refuse_resources_and_capture_for_ancestors() {
        let tmp = checkout_temp_dir("security-linux-xattr");
        let target = tmp.join("resource.txt");
        fs::write(&target, b"payload").expect("payload");
        set_test_xattr(&target, b"user.flyrail", b"private");
        let error = security(&target, false).expect_err("linux xattrs refuse resources");
        assert_eq!(error.code(), ErrorCode::Unsupported);
        assert_eq!(error.errno(), Some(libc::ENOTSUP));
        assert_eq!(error.path(), Some(target.as_path()));
        assert_eq!(
            error.message(),
            format!(
                "[Errno {}] extended attributes and ACLs are unsupported: PosixPath('{}')",
                libc::ENOTSUP,
                target.display()
            )
        );
        assert_eq!(
            fs::read(&target).expect("resource bytes"),
            b"payload",
            "the refused capture must not touch the resource"
        );
        let value = attribute_payload(&raw_cstring(&target), b"user.flyrail", &target)
            .expect("linux xattr value");
        assert_eq!(value, b"private");
        let ancestor = security(&target, true).expect("ancestors capture linux xattrs");
        let framing = String::from_utf8_lossy(&ancestor).into_owned();
        assert!(
            framing.contains(&format!(
                "[\"{}\",\"{}\"]",
                hex_text(b"user.flyrail"),
                hex_text(b"private")
            )),
            "ancestor capture must include the xattr: {framing}"
        );
        remove_test_xattr(&target, b"user.flyrail");
        assert_eq!(
            security(&target, false).expect("clean capture"),
            Vec::<u8>::new()
        );
    }
}
