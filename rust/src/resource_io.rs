#![allow(clippy::redundant_pub_crate)]

use crate::authority::{AuthorityError, hierarchy_conflicts, os_error_move_text};
use crate::codec::{StateCodec, decode_metadata, encode};
use crate::filesystem::{open_lock_file, rename_exclusive};
use crate::observation::{
    failure, io_code, io_failure, io_failure_at, managed_metadata, managed_spelling,
};
use crate::observations::{ErrorCode, TargetError};
use crate::resource_models::{Ancestor, Count, Node, ResourceKind, ResourceRef, Revision};
use crate::security::{ensure_private, security};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

const STATE_ENTRIES: [&str; 9] = [
    "lock",
    "authority.json",
    "receipt.json",
    "transaction.json",
    "staging",
    "backup",
    "authority.json.next",
    "receipt.json.next",
    "transaction.json.next",
];

fn error_detail(error: AuthorityError) -> TargetError {
    match error {
        AuthorityError::Invalid(value) => {
            TargetError::new(ErrorCode::InvalidState, value.message(), None, None)
        }
        AuthorityError::Unsafe { message, path } => {
            TargetError::new(ErrorCode::UnsafePath, message, Some(path), None)
        }
        AuthorityError::Io {
            message,
            path,
            errno,
        } => TargetError::new(io_code(errno), message, path, errno),
    }
}

fn io_move_failure(error: &io::Error, from: &Path, to: &Path) -> TargetError {
    let errno = error.raw_os_error();
    TargetError::new(
        io_code(errno),
        os_error_move_text(error, from, to),
        Some(from.to_path_buf()),
        errno,
    )
}

#[allow(clippy::case_sensitive_file_extension_comparisons)]
pub fn validate_state(reference: &ResourceRef) -> Result<Vec<PathBuf>, TargetError> {
    let state_root = reference.state_root();
    managed_spelling(state_root)?;
    let snapshot = observe(state_root)?;
    if snapshot.nodes().is_empty() {
        return Ok(Vec::new());
    }
    let mut recovery: Vec<PathBuf> = Vec::new();
    for node in snapshot
        .nodes()
        .iter()
        .skip(1)
        .filter(|node| !node.path().contains('/'))
    {
        let name = node.path();
        let child = state_root.join(name);
        if !STATE_ENTRIES.contains(&name) {
            return Err(failure(
                ErrorCode::InvalidState,
                "unknown resource state entry",
                &child,
            ));
        }
        if name == "staging" || name == "backup" {
            if node.data().is_some() {
                return Err(failure(
                    ErrorCode::UnsafePath,
                    "expected a directory",
                    &child,
                ));
            }
            let prefix = format!("{name}/");
            if snapshot
                .nodes()
                .iter()
                .any(|candidate| candidate.path().starts_with(&prefix))
            {
                recovery.push(child.clone());
            }
        } else if node.data().is_none() {
            return Err(failure(
                ErrorCode::InvalidState,
                "metadata must be a regular file",
                &child,
            ));
        }
        if name == "transaction.json" || name.ends_with(".next") {
            recovery.push(child);
        }
    }
    require(state_root, &snapshot)?;
    let header = state_root.join("authority.json");
    if let Some(recorded) = read_reference(&header)?
        && &recorded != reference
    {
        return Err(failure(
            ErrorCode::InvalidState,
            "authority identity disagrees",
            state_root,
        ));
    }
    require(state_root, &snapshot)?;
    Ok(recovery)
}

fn read_reference(path: &Path) -> Result<Option<ResourceRef>, TargetError> {
    read(path, "ResourceRef")
}

fn atomic_header(header: &Path, reference: &ResourceRef) -> Result<(), TargetError> {
    atomic(header, reference, None)
}

pub(crate) fn read<T: StateCodec>(path: &Path, tag: &str) -> Result<Option<T>, TargetError> {
    let parents = ancestors(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            managed_metadata(&metadata, path)?;
            Some(metadata)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(io_failure(&error, path)),
    };
    let Some(metadata) = metadata else {
        if ancestors(path)? != parents {
            return Err(failure(
                ErrorCode::ConcurrentChange,
                "resource ancestor changed",
                path,
            ));
        }
        return Ok(None);
    };
    if !metadata.is_file() {
        return Err(failure(
            ErrorCode::InvalidState,
            "expected a regular file",
            path,
        ));
    }
    let bytes = read_file(path, &metadata)?;
    let value = decode_metadata(&bytes, tag)
        .map_err(|error| failure(ErrorCode::InvalidState, error.message(), path))?;
    if ancestors(path)? != parents {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "resource ancestor changed",
            path,
        ));
    }
    Ok(Some(value))
}

pub(crate) fn atomic<T: StateCodec>(
    path: &Path,
    value: &T,
    expected_ancestors: Option<&[Ancestor]>,
) -> Result<(), TargetError> {
    atomic_with_publication(path, value, expected_ancestors, || Ok(()))
}

pub(crate) fn atomic_with_publication<T: StateCodec>(
    path: &Path,
    value: &T,
    expected_ancestors: Option<&[Ancestor]>,
    published: impl FnOnce() -> Result<(), TargetError>,
) -> Result<(), TargetError> {
    let before = observe(path)?;
    let parents = ancestors(path)?;
    if expected_ancestors.is_some_and(|expected| expected != parents) {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "metadata ancestor changed",
            path,
        ));
    }
    let mut staged = path
        .file_name()
        .map_or_else(OsString::new, std::ffi::OsStr::to_os_string);
    staged.push(".next");
    let temporary = path.with_file_name(staged);
    match fs::symlink_metadata(&temporary) {
        Ok(_) => {
            return Err(failure(
                ErrorCode::RecoveryNeeded,
                "unrecognized metadata preparation",
                &temporary,
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_failure(&error, &temporary)),
    }
    let payload = encode(value);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| io_failure(&error, &temporary))?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| io_failure(&error, &temporary))?;
    ensure_private(&temporary)?;
    file.write_all(&payload)
        .map_err(|error| io_failure_at(&error, None))?;
    drop(file);
    let snapshot = observe(&temporary)?;
    if snapshot.data() != Some(payload.as_slice()) {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "metadata staging changed",
            &temporary,
        ));
    }
    require(path, &before)?;
    if ancestors(path)? != parents {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "metadata ancestor changed",
            path,
        ));
    }
    require(&temporary, &snapshot)?;
    fs::rename(&temporary, path).map_err(|error| io_move_failure(&error, &temporary, path))?;
    published()?;
    require(path, &snapshot)?;
    Ok(())
}

fn metadata_count(value: u64) -> Count {
    i64::try_from(value).map_or_else(|_| Count::Big(value.to_string()), Count::Small)
}

pub(crate) fn ancestors(path: &Path) -> Result<Vec<Ancestor>, TargetError> {
    let mut paths: Vec<&Path> = path.ancestors().skip(1).collect();
    paths.reverse();
    let capture = |parent: &Path| -> Result<Option<Ancestor>, TargetError> {
        let metadata = match fs::symlink_metadata(parent) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_failure(&error, parent)),
        };
        managed_metadata(&metadata, parent)?;
        if !metadata.is_dir() {
            return Err(failure(
                ErrorCode::UnsafePath,
                "resource ancestor is not an ordinary directory",
                parent,
            ));
        }
        Ancestor::new(
            parent.to_path_buf(),
            metadata_count(metadata.dev()),
            metadata_count(metadata.ino()),
            Count::Small(i64::from(metadata.mode() & 0o7777)),
            Count::Small(i64::from(metadata.uid())),
            Count::Small(i64::from(metadata.gid())),
            security(parent, true)?,
        )
        .map(Some)
        .map_err(|error| failure(ErrorCode::InvalidState, error.message(), parent))
    };
    let first = paths
        .iter()
        .map(|parent| capture(parent))
        .collect::<Result<Vec<_>, _>>()?;
    for (parent, expected) in paths.iter().zip(&first) {
        if capture(parent)? != *expected {
            return Err(failure(
                ErrorCode::ConcurrentChange,
                "ancestor changed during inspection",
                parent,
            ));
        }
    }
    Ok(first.into_iter().flatten().collect())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MetadataSignature {
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    uid: u32,
    gid: u32,
    length: u64,
    modified_seconds: i64,
    modified_nanos: i64,
    changed_seconds: i64,
    changed_nanos: i64,
}

fn metadata_signature(metadata: &Metadata) -> MetadataSignature {
    MetadataSignature {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        links: metadata.nlink(),
        uid: metadata.uid(),
        gid: metadata.gid(),
        length: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanos: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanos: metadata.ctime_nsec(),
    }
}

fn read_file(path: &Path, before: &Metadata) -> Result<Vec<u8>, TargetError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(failure(
                ErrorCode::ConcurrentChange,
                "file disappeared while opening",
                path,
            ));
        }
        Err(error) => return Err(io_failure(&error, path)),
    };
    let opened = file
        .metadata()
        .map_err(|error| io_failure_at(&error, None))?;
    if metadata_signature(&opened) != metadata_signature(before) {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "file changed while opening",
            path,
        ));
    }
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .map_err(|error| io_failure_at(&error, None))?;
    let completed = file
        .metadata()
        .map_err(|error| io_failure_at(&error, None))?;
    if metadata_signature(&completed) != metadata_signature(&opened) {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "file changed while reading",
            path,
        ));
    }
    let current = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            failure(
                ErrorCode::ConcurrentChange,
                "path changed during inspection",
                path,
            )
        } else {
            io_failure(&error, path)
        }
    })?;
    if metadata_signature(&current) != metadata_signature(&opened) {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "path changed during inspection",
            path,
        ));
    }
    Ok(data)
}

fn directory_children(path: &Path) -> Result<Vec<(String, PathBuf)>, TargetError> {
    let mut children = fs::read_dir(path)
        .map_err(|error| io_failure(&error, path))?
        .map(|entry| {
            let entry = entry.map_err(|error| io_failure(&error, path))?;
            let name = entry.file_name().into_string().map_err(|_| {
                failure(
                    ErrorCode::UnsafePath,
                    "nonportable resource path",
                    entry.path(),
                )
            })?;
            Ok((name, entry.path()))
        })
        .collect::<Result<Vec<_>, TargetError>>()?;
    children.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    let mut keys = std::collections::HashSet::with_capacity(children.len());
    for (name, child) in &children {
        if !keys.insert(crate::validation::portable_path_key(name)) {
            return Err(failure(
                ErrorCode::UnsafePath,
                "portable filename collision",
                child,
            ));
        }
    }
    Ok(children)
}

fn observe_path(
    path: &Path,
    relative: &str,
    nodes: &mut Vec<Node>,
    signatures: &mut HashMap<PathBuf, MetadataSignature>,
) -> Result<(), TargetError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_failure(&error, path)),
    };
    managed_metadata(&metadata, path)?;
    signatures.insert(path.to_path_buf(), metadata_signature(&metadata));
    if metadata.mode() & 0o7000 != 0 {
        return Err(failure(
            ErrorCode::UnsafePath,
            "special permission bits are unsupported",
            path,
        ));
    }
    let descriptor = security(path, false)?;
    let data = if metadata.is_dir() {
        None
    } else {
        Some(read_file(path, &metadata)?)
    };
    nodes.push(
        Node::new(
            relative.to_owned(),
            data,
            Count::Small(i64::from(metadata.mode() & 0o777)),
            Count::Small(i64::from(metadata.uid())),
            Count::Small(i64::from(metadata.gid())),
            metadata_count(metadata.dev()),
            metadata_count(metadata.ino()),
            descriptor.clone(),
        )
        .map_err(|error| failure(ErrorCode::InvalidState, error.message(), path))?,
    );
    if metadata.is_dir() {
        let children = directory_children(path)?;
        let names = children
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        for (name, child) in children {
            let child_relative = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            observe_path(&child, &child_relative, nodes, signatures)?;
        }
        let current_names = directory_children(path)?
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        if current_names != names {
            return Err(failure(
                ErrorCode::ConcurrentChange,
                "directory entries changed",
                path,
            ));
        }
    }
    let current = fs::symlink_metadata(path).map_err(|error| io_failure(&error, path))?;
    if metadata_signature(&metadata) != metadata_signature(&current) {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "path changed during inspection",
            path,
        ));
    }
    if security(path, false)? != descriptor {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "security metadata changed",
            path,
        ));
    }
    Ok(())
}

fn verify_observed(
    path: &Path,
    revision: &Revision,
    signatures: &HashMap<PathBuf, MetadataSignature>,
) -> Result<(), TargetError> {
    if revision.nodes().is_empty() {
        return match fs::symlink_metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Ok(_) => Err(failure(
                ErrorCode::ConcurrentChange,
                "path changed during inspection",
                path,
            )),
            Err(error) => Err(io_failure(&error, path)),
        };
    }
    for node in revision.nodes() {
        let selected = if node.path().is_empty() {
            path.to_path_buf()
        } else {
            path.join(node.path())
        };
        let metadata = fs::symlink_metadata(&selected).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                failure(
                    ErrorCode::ConcurrentChange,
                    "path changed during inspection",
                    &selected,
                )
            } else {
                io_failure(&error, &selected)
            }
        })?;
        managed_metadata(&metadata, &selected)?;
        let unchanged = signatures.get(&selected) == Some(&metadata_signature(&metadata))
            && node.device().as_u64() == Some(metadata.dev())
            && node.inode().as_u64() == Some(metadata.ino())
            && node.mode().as_u32() == Some(metadata.mode() & 0o777)
            && node.uid().as_u32() == Some(metadata.uid())
            && node.gid().as_u32() == Some(metadata.gid())
            && security(&selected, false)? == node.security();
        if !unchanged {
            return Err(failure(
                ErrorCode::ConcurrentChange,
                "path changed during inspection",
                &selected,
            ));
        }
        if node.data().is_some() {
            if read_file(&selected, &metadata)?.as_slice() != node.data().unwrap_or_default() {
                return Err(failure(
                    ErrorCode::ConcurrentChange,
                    "file changed while reading",
                    &selected,
                ));
            }
        } else {
            let prefix = if node.path().is_empty() {
                String::new()
            } else {
                format!("{}/", node.path())
            };
            let mut expected = revision
                .nodes()
                .iter()
                .filter_map(|candidate| {
                    let remainder = candidate.path().strip_prefix(&prefix)?;
                    (!remainder.is_empty() && !remainder.contains('/'))
                        .then(|| remainder.to_owned())
                })
                .collect::<Vec<_>>();
            expected.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            let actual = directory_children(&selected)?
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>();
            if actual != expected {
                return Err(failure(
                    ErrorCode::ConcurrentChange,
                    "directory entries changed",
                    &selected,
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn observe(path: &Path) -> Result<Revision, TargetError> {
    let parents = ancestors(path)?;
    let initial = match fs::symlink_metadata(path) {
        Ok(metadata) => Some(metadata_signature(&metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(io_failure(&error, path)),
    };
    let mut nodes = Vec::new();
    let mut signatures = HashMap::new();
    observe_path(path, "", &mut nodes, &mut signatures)?;
    let final_signature = match fs::symlink_metadata(path) {
        Ok(metadata) => Some(metadata_signature(&metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(io_failure(&error, path)),
    };
    if final_signature != initial {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "path changed during inspection",
            path,
        ));
    }
    if ancestors(path)? != parents {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "resource ancestor changed",
            path,
        ));
    }
    nodes.sort_by(|left, right| left.path().cmp(right.path()));
    let revision = Revision::new(nodes)
        .map_err(|error| failure(ErrorCode::InvalidState, error.message(), path))?;
    verify_observed(path, &revision, &signatures)?;
    Ok(revision)
}

pub(crate) fn require(path: &Path, revision: &Revision) -> Result<(), TargetError> {
    if &observe(path)? != revision {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "complete resource revision changed",
            path,
        ));
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn write_revision(path: &Path, revision: &Revision) -> Result<Revision, TargetError> {
    for node in revision.nodes() {
        let destination = if node.path().is_empty() {
            path.to_path_buf()
        } else {
            path.join(node.path())
        };
        if node.data().is_none() {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&destination)
                .map_err(|error| io_failure(&error, &destination))?;
        } else {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)
                .map_err(|error| io_failure(&error, &destination))?;
            file.write_all(node.data().unwrap_or_default())
                .map_err(|error| io_failure_at(&error, None))?;
        }
        let Some(mode) = node.mode().as_u32() else {
            return Err(failure(
                ErrorCode::InvalidState,
                "invalid revision metadata",
                &destination,
            ));
        };
        fs::set_permissions(&destination, fs::Permissions::from_mode(mode))
            .map_err(|error| io_failure(&error, &destination))?;
    }
    let result = observe(path)?;
    let intended: Vec<_> = revision
        .nodes()
        .iter()
        .map(|node| {
            (
                node.path(),
                node.data(),
                node.mode().as_u32(),
                node.uid().as_u32(),
                node.gid().as_u32(),
            )
        })
        .collect();
    let actual: Vec<_> = result
        .nodes()
        .iter()
        .map(|node| {
            (
                node.path(),
                node.data(),
                node.mode().as_u32(),
                node.uid().as_u32(),
                node.gid().as_u32(),
            )
        })
        .collect();
    if intended != actual {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "staged resource differs from intended revision",
            path,
        ));
    }
    Ok(result)
}

#[allow(dead_code)]
pub(crate) fn cleanup_revision(current: &Revision, expected: &Revision) -> bool {
    current.nodes()
        == expected
            .nodes()
            .get(..current.nodes().len())
            .unwrap_or_default()
}

#[allow(dead_code)]
pub(crate) fn destroy(path: &Path, revision: &Revision) -> Result<(), TargetError> {
    let actual = observe(path)?;
    if actual.nodes().is_empty() {
        return Ok(());
    }
    if &actual != revision {
        return Err(failure(
            ErrorCode::RecoveryNeeded,
            "cleanup revision changed",
            path,
        ));
    }
    for node in revision.nodes().iter().rev() {
        let selected = if node.path().is_empty() {
            path.to_path_buf()
        } else {
            path.join(node.path())
        };
        if node.data().is_none() {
            fs::remove_dir(&selected).map_err(|error| io_failure(&error, &selected))?;
        } else {
            fs::remove_file(&selected).map_err(|error| io_failure(&error, &selected))?;
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn move_revision(
    source: &Path,
    destination: &Path,
    revision: &Revision,
) -> Result<(), TargetError> {
    require(source, revision)?;
    rename_exclusive(source, destination)?;
    require(destination, revision)
}

pub(crate) fn validate_kind(reference: &ResourceRef) -> Result<(), TargetError> {
    let destination = reference.destination();
    match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            managed_metadata(&metadata, destination)?;
            if metadata.is_file() != (reference.kind() == ResourceKind::Document) {
                return Err(failure(
                    ErrorCode::UnsafePath,
                    "resource kind disagrees with existing destination",
                    destination,
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_failure(&error, destination)),
    }
    Ok(())
}

pub fn reserve(reference: &ResourceRef) -> Result<(), TargetError> {
    let state_root = reference.state_root();
    managed_spelling(state_root)?;
    let lock_path = state_root.join("lock");
    match fs::symlink_metadata(&lock_path) {
        Ok(metadata) => managed_metadata(&metadata, &lock_path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_failure(&error, &lock_path)),
    }
    let mut missing: Vec<PathBuf> = Vec::new();
    let mut parent = state_root.to_path_buf();
    while !parent.exists() {
        let Some(next) = parent.parent().map(Path::to_path_buf) else {
            break;
        };
        missing.push(parent.clone());
        parent = next;
    }
    for path in missing.iter().rev() {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|error| io_failure(&error, path))?;
        ensure_private(path)?;
    }
    ensure_private(state_root)?;
    open_lock_file(&lock_path)?;
    Ok(())
}

pub fn admit(reference: &ResourceRef) -> Result<(), TargetError> {
    let authority = reference.authority_checked().map_err(error_detail)?;
    validate_state(reference)?;
    let header = reference.state_root().join("authority.json");
    if read_reference(&header)?.is_none() {
        let prepared = reference.state_root().join("authority.json.next");
        if read_reference(&prepared)? == Some(reference.clone()) {
            fs::rename(&prepared, &header)
                .map_err(|error| io_move_failure(&error, &prepared, &header))?;
        } else {
            atomic_header(&header, reference)?;
        }
    }
    validate_kind(reference)?;
    let whole_tree = reference.kind() != ResourceKind::Document;
    let conflicts = hierarchy_conflicts(&authority, whole_tree).map_err(error_detail)?;
    if let Some(conflict) = conflicts.first() {
        return Err(failure(
            ErrorCode::Conflict,
            "ancestor or descendant resource fence conflicts",
            conflict,
        ));
    }
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
    use crate::authority::ResourceAuthority;
    use crate::codec::decode;
    use crate::test_support::{case_sensitive, checkout_temp_dir};

    fn document_reference(destination: &Path) -> ResourceRef {
        let authority = ResourceAuthority::new(destination).expect("authority");
        ResourceRef::new(
            authority.destination().to_path_buf(),
            ResourceKind::Document,
            authority.state_root().to_path_buf(),
        )
        .expect("resource")
    }

    fn tree_reference(destination: &Path) -> ResourceRef {
        let authority = ResourceAuthority::new(destination).expect("authority");
        ResourceRef::new(
            authority.destination().to_path_buf(),
            ResourceKind::Tree,
            authority.state_root().to_path_buf(),
        )
        .expect("resource")
    }

    fn prepare(destination: &Path) -> ResourceRef {
        let reference = document_reference(destination);
        reserve(&reference).expect("reserve");
        reference
    }

    fn header_bytes(reference: &ResourceRef) -> Vec<u8> {
        fs::read(reference.state_root().join("authority.json")).expect("authority.json")
    }

    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        fs::symlink_metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
    }

    #[test]
    fn admission_writes_and_replays_the_resource_header() {
        let tmp = checkout_temp_dir("admission-header");
        let reference = prepare(&tmp.join("AGENTS.md"));
        admit(&reference).expect("admit");
        let header = decode::<ResourceRef>(&header_bytes(&reference)).expect("header");
        assert_eq!(header, reference);
        admit(&reference).expect("re-admit");
        assert_eq!(
            decode::<ResourceRef>(&header_bytes(&reference)).expect("header"),
            reference
        );
        assert!(reference.state_root().join("lock").is_file());
        assert_eq!(mode_of(reference.state_root()) & 0o777, 0o700);
        assert_eq!(
            mode_of(&reference.state_root().join("authority.json")) & 0o777,
            0o600
        );
    }

    #[test]
    fn admission_adopts_a_matching_prepared_header() {
        let tmp = checkout_temp_dir("adoption");
        let reference = prepare(&tmp.join("AGENTS.md"));
        let prepared = reference.state_root().join("authority.json.next");
        let staged = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&prepared)
            .expect("prepared header");
        staged
            .set_permissions(fs::Permissions::from_mode(0o600))
            .expect("prepared permissions");
        drop(staged);
        fs::write(&prepared, encode(&reference)).expect("prepared payload");
        admit(&reference).expect("admit");
        assert!(!prepared.exists());
        assert_eq!(
            decode::<ResourceRef>(&header_bytes(&reference)).expect("header"),
            reference
        );
    }

    #[test]
    fn admission_refuses_unrecognized_metadata_preparation() {
        let tmp = checkout_temp_dir("unrecognized-preparation");
        let reference = prepare(&tmp.join("AGENTS.md"));
        let prepared = reference.state_root().join("authority.json.next");
        let staged = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&prepared)
            .expect("prepared header");
        staged
            .set_permissions(fs::Permissions::from_mode(0o600))
            .expect("prepared permissions");
        drop(staged);
        fs::write(
            &prepared,
            encode(&document_reference(&tmp.join("OTHER.md"))),
        )
        .expect("prepared payload");
        let error = admit(&reference).unwrap_err();
        assert_eq!(error.code(), ErrorCode::RecoveryNeeded);
        assert_eq!(error.message(), "unrecognized metadata preparation");
        assert_eq!(error.path(), Some(prepared.as_path()));
    }

    #[test]
    fn admission_refuses_unknown_state_entries() {
        let tmp = checkout_temp_dir("unknown-entry");
        let reference = prepare(&tmp.join("AGENTS.md"));
        fs::write(reference.state_root().join("rogue.json"), b"{}").expect("rogue entry");
        let error = admit(&reference).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidState);
        assert_eq!(error.message(), "unknown resource state entry");
        assert_eq!(
            error.path(),
            Some(reference.state_root().join("rogue.json")).as_deref()
        );
    }

    #[test]
    fn admission_refuses_disagreeing_authority_identity() {
        let tmp = checkout_temp_dir("identity");
        let reference = prepare(&tmp.join("AGENTS.md"));
        let header = reference.state_root().join("authority.json");
        let staged = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&header)
            .expect("header");
        staged
            .set_permissions(fs::Permissions::from_mode(0o600))
            .expect("header permissions");
        drop(staged);
        fs::write(&header, encode(&document_reference(&tmp.join("OTHER.md"))))
            .expect("header payload");
        let error = admit(&reference).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidState);
        assert_eq!(error.message(), "authority identity disagrees");
        assert_eq!(error.path(), Some(reference.state_root()));
    }

    #[test]
    fn admission_refuses_irregular_metadata() {
        let tmp = checkout_temp_dir("irregular-metadata");
        let reference = prepare(&tmp.join("AGENTS.md"));
        fs::create_dir(reference.state_root().join("receipt.json")).expect("metadata dir");
        let error = admit(&reference).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidState);
        assert_eq!(error.message(), "metadata must be a regular file");
        assert_eq!(
            error.path(),
            Some(reference.state_root().join("receipt.json")).as_deref()
        );
    }

    #[test]
    fn admission_refuses_managed_symlinked_header() {
        let tmp = checkout_temp_dir("symlinked-header");
        let reference = prepare(&tmp.join("AGENTS.md"));
        let header = reference.state_root().join("authority.json");
        fs::write(tmp.join("outside.json"), encode(&reference)).expect("outside payload");
        std::os::unix::fs::symlink(tmp.join("outside.json"), &header).expect("symlink");
        let error = admit(&reference).unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnsafePath);
        assert_eq!(
            error.message(),
            "managed symlinks, reparse points and special files are unsupported"
        );
    }

    #[test]
    fn admission_refuses_kind_disagreement() {
        let tmp = checkout_temp_dir("kind");
        let document = tmp.join("AGENTS.md");
        fs::write(&document, b"document").expect("document");
        prepare(&document);
        let tree = tree_reference(&document);
        let error = admit(&tree).unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnsafePath);
        assert_eq!(
            error.message(),
            "resource kind disagrees with existing destination"
        );
        assert_eq!(error.path(), Some(document.as_path()));
        let directory = tmp.join("tree");
        fs::create_dir(&directory).expect("tree dir");
        let as_document = prepare(&directory);
        let error = admit(&as_document).unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnsafePath);
        assert_eq!(
            error.message(),
            "resource kind disagrees with existing destination"
        );
    }

    #[test]
    fn admission_refuses_ancestor_and_descendant_fences() {
        let tmp = checkout_temp_dir("admission-fences");
        let tree_destination = tmp.join("config");
        let child_destination = tree_destination.join("child.md");
        fs::create_dir(&tree_destination).expect("tree dir");
        let tree = tree_reference(&tree_destination);
        reserve(&tree).expect("tree reserve");
        let child = prepare(&child_destination);
        let error = admit(&child).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert_eq!(
            error.message(),
            "ancestor or descendant resource fence conflicts"
        );
        assert_eq!(error.path(), Some(tree.state_root()));
        let error = admit(&tree).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert_eq!(
            error.message(),
            "ancestor or descendant resource fence conflicts"
        );
        assert_eq!(error.path(), Some(child.state_root()));
    }

    #[test]
    fn validate_state_reports_pending_recovery_without_refusing() {
        let tmp = checkout_temp_dir("recovery-state");
        let reference = prepare(&tmp.join("AGENTS.md"));
        let state_root = reference.state_root();
        let staging = state_root.join("staging");
        fs::create_dir(&staging).expect("staging dir");
        fs::write(staging.join("payload"), b"pending").expect("staged payload");
        let transaction = state_root.join("transaction.json");
        let staged = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&transaction)
            .expect("journal");
        staged
            .set_permissions(fs::Permissions::from_mode(0o600))
            .expect("journal permissions");
        drop(staged);
        fs::write(&transaction, b"{}").expect("journal payload");
        admit(&reference).expect("pending recovery does not refuse admission");
        let mut recovery = validate_state(&reference).expect("recovery paths");
        recovery.sort();
        let mut expected = vec![staging, transaction];
        expected.sort();
        assert_eq!(recovery, expected);
    }

    #[test]
    fn admission_without_a_state_root_reports_io_error() {
        let tmp = checkout_temp_dir("missing-state");
        let reference = document_reference(&tmp.join("AGENTS.md"));
        let prepared = reference.state_root().join("authority.json.next");
        let error = admit(&reference).unwrap_err();
        assert_eq!(error.code(), ErrorCode::IoError);
        assert_eq!(
            error.message(),
            format!(
                "[Errno 2] No such file or directory: {}",
                crate::json::python_str_repr(&prepared.to_string_lossy())
            )
        );
        assert_eq!(error.path(), Some(prepared.as_path()));
        assert_eq!(error.errno(), Some(libc::ENOENT));
    }

    #[test]
    fn reserve_is_idempotent_and_keeps_management_private() {
        let tmp = checkout_temp_dir("reserve");
        let reference = prepare(&tmp.join("AGENTS.md"));
        reserve(&reference).expect("second reserve");
        assert!(reference.state_root().join("lock").is_file());
        assert_eq!(mode_of(reference.state_root()) & 0o777, 0o700);
        fs::set_permissions(reference.state_root(), fs::Permissions::from_mode(0o750))
            .expect("permissive mode");
        let error = reserve(&reference).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Unsupported);
        assert_eq!(
            error.message(),
            format!(
                "[Errno {}] management storage must be private to its owner: PosixPath('{}')",
                libc::ENOTSUP,
                reference.state_root().display()
            )
        );
        assert_eq!(error.errno(), Some(libc::ENOTSUP));
    }

    #[test]
    fn admission_rejects_changed_canonical_destination() {
        let tmp = checkout_temp_dir("canonical-change");
        if case_sensitive(&tmp) {
            eprintln!("skipping: host is case sensitive");
            return;
        }
        let physical = tmp.join("Config");
        fs::create_dir(&physical).expect("physical dir");
        let requested = tmp.join("config").join("AGENTS.md");
        let state_root = crate::authority::state_path(&requested);
        let recorded =
            ResourceRef::new(requested, ResourceKind::Document, state_root).expect("resource");
        reserve(&recorded).expect("reserve");
        let error = admit(&recorded).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidState);
        assert_eq!(error.message(), "resource canonical destination changed");
    }

    fn write_header(path: &Path, payload: &[u8]) {
        let staged = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .expect("header");
        staged
            .set_permissions(fs::Permissions::from_mode(0o600))
            .expect("header permissions");
        drop(staged);
        fs::write(path, payload).expect("header payload");
    }

    #[test]
    fn admission_refuses_wrong_typed_authority_metadata() {
        let tmp = checkout_temp_dir("wrong-typed-header");
        let reference = prepare(&tmp.join("AGENTS.md"));
        let header = reference.state_root().join("authority.json");
        write_header(&header, b"{\"enum\": \"Ownership\", \"value\": \"file\"}\n");
        let error = admit(&reference).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidState);
        assert_eq!(error.message(), "metadata record has the wrong type");
        assert_eq!(error.path(), Some(header.as_path()));

        let noncanonical = prepare(&tmp.join("OTHER.md"));
        let foreign = noncanonical.state_root().join("authority.json");
        write_header(&foreign, b"{\"path\": \"/a/./b\"}\n");
        let error = admit(&noncanonical).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidState);
        assert_eq!(error.message(), "state record is not canonical");
        assert_eq!(error.path(), Some(foreign.as_path()));
    }

    #[test]
    fn management_spelling_rejects_file_ancestors_like_python() {
        let tmp = checkout_temp_dir("file-ancestor");
        let file = tmp.join("plainfile");
        fs::write(&file, b"file").expect("regular file");
        let destination = file.join("doc.md");
        let state_root = crate::authority::state_path(&destination);
        let reference =
            ResourceRef::new(destination, ResourceKind::Document, state_root).expect("resource");
        let error = validate_state(&reference).unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnsafePath);
        assert_eq!(
            error.message(),
            "resource ancestor is not an ordinary directory"
        );
        assert_eq!(error.path(), Some(file.as_path()));
        let error = reserve(&reference).unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnsafePath);
        assert_eq!(
            error.message(),
            "resource ancestor is not an ordinary directory"
        );
    }

    #[test]
    fn snapshots_detect_same_length_rewrites() {
        let tmp = checkout_temp_dir("same-length-rewrite");
        let path = tmp.join("AGENTS.md");
        fs::write(&path, b"before").expect("initial file");
        let expected = observe(&path).expect("snapshot");
        fs::write(&path, b"change").expect("same length rewrite");
        assert_eq!(
            require(&path, &expected).unwrap_err().code(),
            ErrorCode::ConcurrentChange
        );
    }

    #[test]
    fn directory_snapshots_detect_membership_changes() {
        let tmp = checkout_temp_dir("directory-membership");
        let path = tmp.join("skills");
        fs::create_dir(&path).expect("directory");
        fs::write(path.join("one"), b"one").expect("first member");
        let expected = observe(&path).expect("snapshot");
        fs::write(path.join("two"), b"two").expect("second member");
        assert_eq!(
            require(&path, &expected).unwrap_err().code(),
            ErrorCode::ConcurrentChange
        );
    }

    #[test]
    fn ancestor_snapshots_detect_parent_rebinding() {
        let tmp = checkout_temp_dir("ancestor-rebinding");
        let parent = tmp.join("parent");
        let replaced = tmp.join("replaced");
        fs::create_dir(&parent).expect("parent");
        let path = parent.join("AGENTS.md");
        fs::write(&path, b"content").expect("file");
        let expected = ancestors(&path).expect("ancestors");
        fs::rename(&parent, &replaced).expect("replace parent");
        fs::create_dir(&parent).expect("new parent");
        fs::write(&path, b"content").expect("replacement file");
        assert_ne!(ancestors(&path).expect("rebound ancestors"), expected);
    }
}
