#![allow(clippy::redundant_pub_crate)]

use crate::authority::{
    ResourceAuthority, physical_location, sha256_hex, transaction_entries, transaction_path,
};
use crate::codec::{StateCodec, decode_metadata, encode};
use crate::observations::{ErrorCode, TargetError};
use crate::resource_io::error_detail;
use crate::resource_models::{Index, ResourceRef};
use crate::stores::{StateStore, StoreError, StoreLock};
use crate::validation::portable_path_key;
use sha2::{Digest as _, Sha256};
use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) fn index_key(bundle_id: &str, context_digest: &str) -> String {
    format!("index/{bundle_id}/{context_digest}")
}

pub(crate) fn destination_key(destination: &Path) -> String {
    sha256_hex(&Sha256::digest(
        portable_path_key(&destination.to_string_lossy()).as_bytes(),
    ))
}

pub(crate) fn resource_key(destination: &Path, record: &str) -> String {
    format!("resource/{}/{record}", destination_key(destination))
}

fn location(store: &dyn StateStore) -> Option<PathBuf> {
    store.location().map(Path::to_path_buf)
}

fn state_failure(store: &dyn StateStore, code: ErrorCode, message: String) -> TargetError {
    TargetError::new(code, message, location(store), None)
}

pub(crate) fn store_failure(error: StoreError) -> TargetError {
    error.into()
}

pub(crate) fn read_record<T: StateCodec>(
    store: &dyn StateStore,
    key: &str,
    tag: &str,
) -> Result<Option<T>, TargetError> {
    let Some(data) = store.read(key).map_err(store_failure)? else {
        return Ok(None);
    };
    let value = decode_metadata::<T>(&data, tag).map_err(|error| {
        let message = if error.message() == "metadata record has the wrong type" {
            "record has the wrong type"
        } else {
            error.message()
        };
        state_failure(store, ErrorCode::InvalidState, format!("{key}: {message}"))
    })?;
    if encode(&value) != data {
        return Err(state_failure(
            store,
            ErrorCode::InvalidState,
            format!("{key}: record is not canonical"),
        ));
    }
    Ok(Some(value))
}

pub(crate) fn write_record<T: StateCodec>(
    store: &dyn StateStore,
    key: &str,
    value: &T,
    expected: Option<&T>,
) -> Result<(), TargetError> {
    let expected = expected.map(encode);
    if store
        .write(key, &encode(value), expected.as_deref())
        .map_err(store_failure)?
    {
        Ok(())
    } else {
        Err(state_failure(
            store,
            ErrorCode::ConcurrentChange,
            format!("{key}: state changed"),
        ))
    }
}

pub(crate) fn delete_record<T: StateCodec>(
    store: &dyn StateStore,
    key: &str,
    expected: &T,
) -> Result<(), TargetError> {
    if store
        .delete(key, &encode(expected))
        .map_err(store_failure)?
    {
        Ok(())
    } else {
        Err(state_failure(
            store,
            ErrorCode::ConcurrentChange,
            format!("{key}: state changed"),
        ))
    }
}

pub(crate) fn require_index(
    store: &dyn StateStore,
    key: &str,
    expected: Option<&Index>,
) -> Result<(), TargetError> {
    if store.read(key).map_err(store_failure)? != expected.map(encode) {
        return Err(state_failure(
            store,
            ErrorCode::ConcurrentChange,
            String::from("installation index changed"),
        ));
    }
    Ok(())
}

pub(crate) fn store_lock<'a>(
    store: &'a dyn StateStore,
    name: &str,
    timeout: Duration,
) -> Result<StoreLock<'a>, TargetError> {
    store.lock(name, timeout).map_err(|error| match error {
        StoreError::Busy { .. } => state_failure(store, ErrorCode::Busy, error.to_string()),
        other => other.into(),
    })
}

fn device(path: &Path) -> io::Result<u64> {
    for candidate in path.ancestors() {
        match std::fs::metadata(candidate) {
            Ok(metadata) => return Ok(metadata.dev()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::from(io::ErrorKind::NotFound))
}

fn mount_point(field: &str) -> PathBuf {
    let bytes = field.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let escape = bytes
            .get(index + 1..index + 4)
            .filter(|digits| {
                byte == b'\\' && digits.iter().all(|digit| (b'0'..=b'7').contains(digit))
            })
            .and_then(|digits| {
                let value = digits
                    .iter()
                    .fold(0u32, |total, digit| total * 8 + u32::from(digit - b'0'));
                u8::try_from(value).ok()
            });
        if let Some(value) = escape {
            decoded.push(value);
            index += 4;
        } else {
            decoded.push(byte);
            index += 1;
        }
    }
    PathBuf::from(OsString::from_vec(decoded))
}

fn mount_id(entries: &[(u64, PathBuf)], path: &Path) -> Option<u64> {
    let mut selected: Option<(u64, usize)> = None;
    for (identifier, point) in entries {
        let length = point.components().count();
        if path.starts_with(point) && selected.is_none_or(|(_, best)| length >= best) {
            selected = Some((*identifier, length));
        }
    }
    selected.map(|(identifier, _)| identifier)
}

fn digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn shared_mount(mountinfo: &[u8], first: &Path, second: &Path) -> bool {
    const BREAKS: [char; 9] = [
        '\r', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}', '\u{2029}',
    ];
    let Ok(text) = std::str::from_utf8(mountinfo) else {
        return false;
    };
    if text.contains(BREAKS) {
        return false;
    }
    let mut entries = Vec::new();
    for line in text.split_terminator('\n') {
        let fields = line.split(' ').collect::<Vec<_>>();
        let device = fields.get(2).and_then(|field| field.split_once(':'));
        let valid = fields.len() >= 10
            && fields.iter().skip(6).any(|field| *field == "-")
            && fields.first().is_some_and(|field| digits(field))
            && fields.get(1).is_some_and(|field| digits(field))
            && device.is_some_and(|(major, minor)| digits(major) && digits(minor))
            && fields.get(4).is_some_and(|field| field.starts_with('/'));
        let identifier = fields.first().and_then(|field| field.parse::<u64>().ok());
        let (true, Some(identifier), Some(point)) = (valid, identifier, fields.get(4)) else {
            return false;
        };
        entries.push((identifier, mount_point(point)));
    }
    mount_id(&entries, first).is_some_and(|mount| mount_id(&entries, second) == Some(mount))
}

fn mountinfo() -> Option<Vec<u8>> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    Some(std::fs::read("/proc/self/mountinfo").unwrap_or_default())
}

fn same_mount(first: &Path, second: &Path) -> io::Result<bool> {
    if device(first)? != device(second)? {
        return Ok(false);
    }
    Ok(mountinfo().is_none_or(|text| shared_mount(&text, first, second)))
}

fn staged(staging: &Path, destination: &Path, transaction_id: &str) -> Option<PathBuf> {
    physical_location(staging)
        .ok()
        .map(|root| root.join(format!("{}-{transaction_id}", destination_key(destination))))
}

pub(crate) fn choose_transaction_directory(
    store: &dyn StateStore,
    destination: &Path,
    transaction_id: &str,
) -> Result<PathBuf, TargetError> {
    let fallback = transaction_path(destination, transaction_id)
        .map_err(|error| TargetError::new(ErrorCode::InvalidState, error.message(), None, None))?;
    let Some(staging) = store.staging_directory() else {
        return Ok(fallback);
    };
    let selected = staged(&staging, destination, transaction_id);
    let shared =
        selected
            .as_ref()
            .is_some_and(|selected| match (selected.parent(), destination.parent()) {
                (Some(left), Some(right)) => same_mount(left, right).unwrap_or(false),
                _ => false,
            });
    Ok(match selected {
        Some(selected) if shared => selected,
        _ => fallback,
    })
}

pub(crate) fn recognized_transaction_directory(
    store: &dyn StateStore,
    destination: &Path,
    transaction_id: &str,
    directory: &Path,
) -> bool {
    if transaction_path(destination, transaction_id).is_ok_and(|path| path == directory) {
        return true;
    }
    store.staging_directory().is_some_and(|staging| {
        staged(&staging, destination, transaction_id).is_some_and(|path| path == directory)
    })
}

pub(crate) fn hierarchy_conflicts(
    store: &dyn StateStore,
    authority: &ResourceAuthority,
    whole_tree: bool,
) -> Result<Vec<PathBuf>, TargetError> {
    let destination = authority.destination();
    let mut conflicts = Vec::new();
    for parent in destination.ancestors().skip(1) {
        if parent.parent().is_some()
            && store
                .read(&resource_key(parent, "authority"))
                .map_err(store_failure)?
                .is_some()
        {
            conflicts.push(parent.to_path_buf());
        }
    }
    if whole_tree {
        for key in store.keys("resource/").map_err(store_failure)? {
            if key.ends_with("/authority")
                && let Some(reference) = read_record::<ResourceRef>(store, &key, "ResourceRef")?
                && reference
                    .destination()
                    .ancestors()
                    .skip(1)
                    .any(|parent| parent == destination)
            {
                conflicts.push(reference.destination().to_path_buf());
            }
        }
        if std::fs::metadata(destination).is_ok() {
            conflicts.extend(transaction_entries(destination).map_err(error_detail)?);
        }
    }
    conflicts.sort_by(|left, right| {
        left.as_os_str()
            .as_bytes()
            .cmp(right.as_os_str().as_bytes())
    });
    conflicts.dedup();
    Ok(conflicts)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::stores::MemoryStore;

    fn index() -> Index {
        Index::new(
            String::from("team"),
            "a".repeat(64),
            None,
            None,
            None,
            Vec::new(),
            2,
        )
        .unwrap()
    }

    #[test]
    fn keys_follow_the_shared_scheme() {
        assert_eq!(index_key("team", "abc"), "index/team/abc");
        assert_eq!(
            destination_key(Path::new("/tmp/AGENTS.md")),
            sha256_hex(&Sha256::digest(b"/tmp/agents.md"))
        );
        assert_eq!(
            resource_key(Path::new("/tmp/AGENTS.md"), "receipt"),
            format!(
                "resource/{}/receipt",
                destination_key(Path::new("/tmp/agents.md"))
            )
        );
    }

    #[test]
    fn records_round_trip_with_compare_and_set() {
        let store = MemoryStore::new();
        let key = index_key("team", &"a".repeat(64));
        assert_eq!(read_record::<Index>(&store, &key, "Index").unwrap(), None);
        write_record(&store, &key, &index(), None).unwrap();
        assert_eq!(
            read_record::<Index>(&store, &key, "Index").unwrap(),
            Some(index())
        );
        let conflict = write_record(&store, &key, &index(), None).unwrap_err();
        assert_eq!(conflict.code(), ErrorCode::ConcurrentChange);
        assert_eq!(conflict.message(), format!("{key}: state changed"));
        delete_record(&store, &key, &index()).unwrap();
        assert_eq!(store.read(&key).unwrap(), None);
        assert_eq!(
            delete_record(&store, &key, &index()).unwrap_err().code(),
            ErrorCode::ConcurrentChange
        );
    }

    #[test]
    fn non_canonical_and_mistyped_records_are_invalid_state() {
        let store = MemoryStore::new();
        let key = index_key("team", &"a".repeat(64));
        let mut spaced = encode(&index());
        spaced.insert(1, b' ');
        assert!(store.write(&key, &spaced, None).unwrap());
        let error = read_record::<Index>(&store, &key, "Index").unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidState);
        assert_eq!(error.message(), format!("{key}: record is not canonical"));
        let error =
            read_record::<crate::resource_models::Receipt>(&store, &key, "Receipt").unwrap_err();
        assert_eq!(error.message(), format!("{key}: record has the wrong type"));
    }

    const ROOT: &str = "22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw";
    const HOME: &str = "30 22 8:2 / /home rw,relatime shared:2 - ext4 /dev/sda2 rw";

    fn info(lines: &[&str]) -> String {
        lines.iter().flat_map(|line| [*line, "\n"]).collect()
    }

    fn shared(mountinfo: &str, first: &str, second: &str) -> bool {
        shared_mount(mountinfo.as_bytes(), Path::new(first), Path::new(second))
    }

    #[test]
    fn paths_share_a_mount_only_on_the_same_mount_entry() {
        let mounts = info(&[ROOT, HOME]);
        assert!(shared(&mounts, "/home/u/.state/staging", "/home/u/project"));
        assert!(!shared(&mounts, "/home/u/.state/staging", "/srv/project"));
        let bound = info(&[
            ROOT,
            HOME,
            "41 30 8:2 /u/.state /work/state rw,relatime - ext4 /dev/sda2 rw",
            "42 30 8:2 /u/project /work/project rw,relatime - ext4 /dev/sda2 rw",
        ]);
        assert!(!shared(&bound, "/work/state/staging", "/work/project"));
        assert!(shared(&bound, "/work/project/a", "/work/project/b"));
    }

    #[test]
    fn escaped_and_stacked_mount_points_are_resolved() {
        let escaped = info(&[
            ROOT,
            HOME,
            "43 30 8:2 /x /home/u/my\\040state rw - ext4 /dev/sda2 rw",
        ]);
        assert!(!shared(
            &escaped,
            "/home/u/my state/staging",
            "/home/u/project"
        ));
        assert!(shared(
            &escaped,
            "/home/u/my state/staging",
            "/home/u/my state/p"
        ));
        let stacked = info(&[ROOT, HOME, "44 30 8:3 / /home rw - tmpfs tmpfs rw"]);
        assert!(!shared(&stacked, "/home/u/staging", "/"));
        assert!(shared(&stacked, "/home/u/staging", "/home/v"));
    }

    #[test]
    fn doubtful_mount_information_shares_no_mount() {
        for mountinfo in [
            String::new(),
            String::from("garbage\n"),
            info(&[
                ROOT,
                "30 22 8:2 / /home rw,relatime shared:2 ext4 /dev/sda2 rw",
            ]),
            info(&[ROOT, "x 22 8:2 / /home rw - ext4 /dev/sda2 rw"]),
            info(&[ROOT, "30 22 8-2 / /home rw - ext4 /dev/sda2 rw"]),
            info(&[ROOT, "", HOME]),
            info(&[ROOT, "30 22 8:2 / home rw - ext4 /dev/sda2 rw"]),
        ] {
            assert!(
                !shared(&mountinfo, "/home/u/.state/staging", "/home/u/project"),
                "{mountinfo:?}"
            );
        }
        assert!(!shared(&info(&[HOME]), "/srv/state", "/srv/project"));
    }

    #[test]
    fn a_mount_point_that_is_not_utf8_shares_no_mount() {
        let mut mountinfo = info(&[ROOT, HOME]).into_bytes();
        mountinfo.extend_from_slice(b"45 30 8:2 /x /home/u/st\xffate rw - ext4 /dev/sda2 rw\n");
        let staging = PathBuf::from(OsString::from_vec(b"/home/u/st\xffate/staging".to_vec()));
        assert!(!shared_mount(
            &mountinfo,
            &staging,
            Path::new("/home/u/project")
        ));
        assert!(!shared_mount(
            &mountinfo,
            Path::new("/home/u/.state/staging"),
            Path::new("/home/u/project")
        ));
    }

    #[test]
    fn a_held_lock_reports_busy() {
        let store = MemoryStore::new();
        let _held = store_lock(&store, "index/team/x", Duration::ZERO).unwrap();
        let error = store_lock(&store, "index/team/x", Duration::ZERO).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Busy);
    }
}
