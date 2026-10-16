#![allow(clippy::redundant_pub_crate)]

use crate::authority::AuthorityError;
use crate::filesystem::rename_exclusive;
use crate::observation::{failure, io_code, io_failure, io_failure_at, managed_metadata};
use crate::observations::{ErrorCode, TargetError};
use crate::resource_models::{Ancestor, Count, Node, ResourceKind, ResourceRef, Revision};
use crate::seam::{self, Seam};
use crate::security::security;
use crate::state::{hierarchy_conflicts, read_record, resource_key, write_record};
use crate::stores::StateStore;
use std::collections::HashMap;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub(crate) fn error_detail(error: AuthorityError) -> TargetError {
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

pub(crate) fn resource_ancestors(
    destination: &Path,
    paths: &[&Path],
) -> Result<Vec<Ancestor>, TargetError> {
    let mut result: Vec<Ancestor> = Vec::new();
    for path in std::iter::once(destination).chain(paths.iter().copied()) {
        for item in ancestors(path)? {
            if !result.contains(&item) {
                result.push(item);
            }
        }
    }
    Ok(result)
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

pub(crate) fn require_unchanged_since(
    path: &Path,
    revision: &Revision,
    boundary: SystemTime,
) -> Result<(), TargetError> {
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
        let modified = metadata
            .modified()
            .map_err(|error| io_failure(&error, &selected))?;
        let born = metadata.created().ok();
        if changed_after(boundary, modified, born, cfg!(target_os = "macos")) {
            return Err(failure(
                ErrorCode::RecoveryNeeded,
                "destination revision changed after preparation",
                &selected,
            ));
        }
    }
    Ok(())
}

fn changed_after(
    boundary: SystemTime,
    modified: SystemTime,
    born: Option<SystemTime>,
    birth_time: bool,
) -> bool {
    modified > boundary || (birth_time && born.is_some_and(|born| born > boundary))
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
        let (Some(mode), Some(uid), Some(gid)) = (
            node.mode().as_u32(),
            node.uid().as_u32(),
            node.gid().as_u32(),
        ) else {
            return Err(failure(
                ErrorCode::InvalidState,
                "invalid revision metadata",
                &destination,
            ));
        };
        let metadata =
            fs::metadata(&destination).map_err(|error| io_failure(&error, &destination))?;
        if (metadata.uid(), metadata.gid()) != (uid, gid) {
            std::os::unix::fs::chown(&destination, Some(uid), Some(gid))
                .map_err(|error| io_failure(&error, &destination))?;
        }
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
            seam::fire(&Seam::Unlink(&selected));
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

pub fn admit(store: &dyn StateStore, reference: &ResourceRef) -> Result<(), TargetError> {
    let authority = reference.authority_checked().map_err(error_detail)?;
    let key = resource_key(reference.destination(), "authority");
    match read_record::<ResourceRef>(store, &key, "ResourceRef")? {
        None => write_record(store, &key, reference, None)?,
        Some(current) if &current != reference => {
            return Err(failure(
                ErrorCode::InvalidState,
                "authority identity disagrees",
                reference.destination(),
            ));
        }
        Some(_) => {}
    }
    validate_kind(reference)?;
    let whole_tree = reference.kind() != ResourceKind::Document;
    let conflicts = hierarchy_conflicts(store, &authority, whole_tree)?;
    if let Some(conflict) = conflicts.first() {
        return Err(failure(
            ErrorCode::Conflict,
            "ancestor or descendant resource conflicts",
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
    use crate::codec::{decode, encode};
    use crate::stores::MemoryStore;
    use crate::test_support::{case_sensitive, checkout_temp_dir};

    fn reference(destination: &Path, kind: ResourceKind) -> ResourceRef {
        let authority = ResourceAuthority::new(destination).expect("authority");
        ResourceRef::new(authority.destination().to_path_buf(), kind).expect("resource")
    }

    fn document_reference(destination: &Path) -> ResourceRef {
        reference(destination, ResourceKind::Document)
    }

    fn tree_reference(destination: &Path) -> ResourceRef {
        reference(destination, ResourceKind::Tree)
    }

    fn authority_key(reference: &ResourceRef) -> String {
        resource_key(reference.destination(), "authority")
    }

    #[test]
    fn admission_writes_and_replays_the_authority_record() {
        let tmp = checkout_temp_dir("admission-header");
        let store = MemoryStore::new();
        let reference = document_reference(&tmp.join("AGENTS.md"));
        admit(&store, &reference).expect("admit");
        let recorded = store.read(&authority_key(&reference)).unwrap().unwrap();
        assert_eq!(decode::<ResourceRef>(&recorded).expect("record"), reference);
        admit(&store, &reference).expect("re-admit");
        assert_eq!(
            store.read(&authority_key(&reference)).unwrap(),
            Some(recorded)
        );
        assert_eq!(fs::read_dir(&*tmp).expect("listing").count(), 0);
    }

    #[test]
    fn admission_refuses_disagreeing_authority_identity() {
        let tmp = checkout_temp_dir("identity");
        let store = MemoryStore::new();
        let reference = document_reference(&tmp.join("AGENTS.md"));
        let other = tree_reference(&tmp.join("AGENTS.md"));
        assert!(
            store
                .write(&authority_key(&reference), &encode(&other), None)
                .unwrap()
        );
        let error = admit(&store, &reference).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidState);
        assert_eq!(error.message(), "authority identity disagrees");
        assert_eq!(error.path(), Some(reference.destination()));
    }

    #[test]
    fn admission_refuses_kind_disagreement() {
        let tmp = checkout_temp_dir("kind");
        let store = MemoryStore::new();
        let document = tmp.join("AGENTS.md");
        fs::write(&document, b"document").expect("document");
        let tree = tree_reference(&document);
        let error = admit(&store, &tree).unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnsafePath);
        assert_eq!(
            error.message(),
            "resource kind disagrees with existing destination"
        );
        assert_eq!(error.path(), Some(document.as_path()));
        let directory = tmp.join("tree");
        fs::create_dir(&directory).expect("tree dir");
        let as_document = document_reference(&directory);
        let error = admit(&store, &as_document).unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnsafePath);
        assert_eq!(
            error.message(),
            "resource kind disagrees with existing destination"
        );
    }

    #[test]
    fn admission_refuses_ancestor_and_descendant_resources() {
        let tmp = checkout_temp_dir("admission-fences");
        let store = MemoryStore::new();
        let tree_destination = tmp.join("config");
        let child_destination = tree_destination.join("child.md");
        fs::create_dir(&tree_destination).expect("tree dir");
        let tree = tree_reference(&tree_destination);
        admit(&store, &tree).expect("tree admission");
        let child = document_reference(&child_destination);
        let error = admit(&store, &child).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert_eq!(error.message(), "ancestor or descendant resource conflicts");
        assert_eq!(error.path(), Some(tree.destination()));
        let error = admit(&store, &tree).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert_eq!(error.message(), "ancestor or descendant resource conflicts");
        assert_eq!(error.path(), Some(child.destination()));
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
        let recorded = ResourceRef::new(requested, ResourceKind::Document).expect("resource");
        let error = admit(&MemoryStore::new(), &recorded).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidState);
        assert_eq!(error.message(), "resource canonical destination changed");
    }

    #[test]
    fn admission_refuses_wrong_typed_and_noncanonical_authority_records() {
        let tmp = checkout_temp_dir("wrong-typed-header");
        let store = MemoryStore::new();
        let reference = document_reference(&tmp.join("AGENTS.md"));
        let key = authority_key(&reference);
        assert!(
            store
                .write(
                    &key,
                    b"{\"enum\": \"Ownership\", \"value\": \"file\"}\n",
                    None
                )
                .unwrap()
        );
        let error = admit(&store, &reference).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidState);
        assert_eq!(error.message(), format!("{key}: record has the wrong type"));
        assert_eq!(error.path(), None);

        let other = document_reference(&tmp.join("OTHER.md"));
        let foreign = authority_key(&other);
        let mut spaced = encode(&other);
        spaced.insert(1, b' ');
        assert!(store.write(&foreign, &spaced, None).unwrap());
        let error = admit(&store, &other).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidState);
        assert_eq!(
            error.message(),
            format!("{foreign}: record is not canonical")
        );
    }

    #[test]
    fn observation_rejects_file_ancestors_like_python() {
        let tmp = checkout_temp_dir("file-ancestor");
        let file = tmp.join("plainfile");
        fs::write(&file, b"file").expect("regular file");
        let error = observe(&file.join("doc.md")).unwrap_err();
        assert_eq!(error.code(), ErrorCode::UnsafePath);
        assert_eq!(
            error.message(),
            "resource ancestor is not an ordinary directory"
        );
        assert_eq!(error.path(), Some(file.as_path()));
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

    #[test]
    fn birth_time_is_compared_only_on_macos() {
        let boundary = SystemTime::UNIX_EPOCH
            .checked_add(std::time::Duration::from_secs(1_000_000))
            .expect("boundary");
        let born = boundary
            .checked_add(std::time::Duration::from_secs(60))
            .expect("birth time");
        assert!(changed_after(boundary, boundary, Some(born), true));
        assert!(!changed_after(boundary, boundary, Some(born), false));
    }

    #[test]
    fn a_destination_modified_before_the_boundary_is_unchanged_whenever_it_was_born() {
        let tmp = checkout_temp_dir("birth-time");
        let path = tmp.join("AGENTS.md");
        fs::write(&path, b"# mine\n").expect("file");
        let earlier = SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(120))
            .expect("earlier time");
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open")
            .set_times(fs::FileTimes::new().set_modified(earlier))
            .expect("earlier modification time");
        let revision = observe(&path).expect("revision");
        let boundary = earlier
            .checked_add(std::time::Duration::from_secs(60))
            .expect("boundary");
        assert!(require_unchanged_since(&path, &revision, boundary).is_ok());
    }
}
