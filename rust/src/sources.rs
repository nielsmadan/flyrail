use crate::bundle::BundleError;
use crate::models::{BundleEntry, SkillSpec};
use crate::validation::{ancestors, portable_path_key, validate_relative_path};
use crate::values::ValueError;
use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::hash::Hash;
use std::io::Read as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

pub const S_IFMT: u32 = 0o170_000;
pub const S_IFREG: u32 = 0o100_000;
pub const S_IFDIR: u32 = 0o040_000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Signature {
    dev: u64,
    ino: u64,
    mode: u32,
    size: u64,
    mtime_ns: i128,
    ctime_ns: i128,
}

impl Signature {
    fn from(metadata: &fs::Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            mode: metadata.mode(),
            size: metadata.len(),
            mtime_ns: i128::from(metadata.mtime()) * 1_000_000_000
                + i128::from(metadata.mtime_nsec()),
            ctime_ns: i128::from(metadata.ctime()) * 1_000_000_000
                + i128::from(metadata.ctime_nsec()),
        }
    }
}

const fn is_regular(mode: u32) -> bool {
    mode & S_IFMT == S_IFREG
}

const fn is_dir(mode: u32) -> bool {
    mode & S_IFMT == S_IFDIR
}

fn ordinary(metadata: &fs::Metadata, path: &Path) -> Result<(), BundleError> {
    let mode = metadata.mode();
    if !(is_regular(mode) || is_dir(mode)) {
        return Err(BundleError::Invalid(ValueError::new(format!(
            "source symlinks and special files are not supported: {}",
            path.display()
        ))));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> BundleError {
    BundleError::Invalid(ValueError::new(message))
}

struct Ordered<K, V> {
    positions: HashMap<K, usize>,
    items: Vec<(K, V)>,
}

impl<K: Clone + Eq + Hash, V> Ordered<K, V> {
    fn new() -> Self {
        Self {
            positions: HashMap::new(),
            items: Vec::new(),
        }
    }

    fn get<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.positions
            .get(key)
            .and_then(|at| self.items.get(*at))
            .map(|(_, value)| value)
    }

    fn contains<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.positions.contains_key(key)
    }

    fn insert(&mut self, key: K, value: V) {
        if let Some(at) = self.positions.get(&key) {
            if let Some(slot) = self.items.get_mut(*at) {
                slot.1 = value;
            }
        } else {
            self.positions.insert(key.clone(), self.items.len());
            self.items.push((key, value));
        }
    }

    fn keys(&self) -> Vec<K> {
        self.items.iter().map(|(key, _)| key.clone()).collect()
    }
}

pub struct DirectorySource {
    root: PathBuf,
    observed: Ordered<PathBuf, Signature>,
    directories: Ordered<PathBuf, Vec<OsString>>,
    matches: Ordered<(PathBuf, String), Vec<OsString>>,
    listed: HashSet<PathBuf>,
}

impl DirectorySource {
    pub fn new(root: &Path) -> Result<Self, BundleError> {
        let metadata = root.symlink_metadata().map_err(BundleError::Io)?;
        ordinary(&metadata, root)?;
        let resolved = fs::canonicalize(root).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                invalid(format!(
                    "bundle source changed while loading: {}",
                    root.display()
                ))
            } else {
                BundleError::Io(error)
            }
        })?;
        let resolved_metadata = resolved.symlink_metadata().map_err(BundleError::Io)?;
        if Signature::from(&resolved_metadata) != Signature::from(&metadata) {
            return Err(invalid(format!(
                "bundle source changed while loading: {}",
                root.display()
            )));
        }
        let mut source = Self {
            root: resolved,
            observed: Ordered::new(),
            directories: Ordered::new(),
            matches: Ordered::new(),
            listed: HashSet::new(),
        };
        let root = source.root.clone();
        if !is_dir(source.observe(&root)?.mode) {
            return Err(invalid("bundle root must be a directory"));
        }
        source.directory(&root, None)?;
        Ok(source)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn observe(&mut self, path: &Path) -> Result<Signature, BundleError> {
        let metadata = path.symlink_metadata().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                if self.observed.contains(path) || self.listed.contains(path) {
                    invalid(format!(
                        "bundle source changed while loading: {}",
                        path.display()
                    ))
                } else {
                    BundleError::Io(error)
                }
            } else {
                BundleError::Io(error)
            }
        })?;
        ordinary(&metadata, path)?;
        let signature = Signature::from(&metadata);
        if self
            .observed
            .get(path)
            .is_some_and(|previous| *previous != signature)
        {
            return Err(invalid(format!(
                "bundle source changed while loading: {}",
                path.display()
            )));
        }
        self.observed.insert(path.to_path_buf(), signature);
        Ok(signature)
    }

    fn directory(
        &mut self,
        path: &Path,
        matching: Option<&str>,
    ) -> Result<Vec<PathBuf>, BundleError> {
        let mut children: Vec<PathBuf> = fs::read_dir(path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    invalid(format!(
                        "bundle source changed while loading: {}",
                        path.display()
                    ))
                } else {
                    BundleError::Io(error)
                }
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(BundleError::Io)?
            .into_iter()
            .map(|entry| entry.path())
            .collect();
        if let Some(matching) = matching {
            let key = portable_path_key(matching);
            children.retain(|child| matches_component(child, &key));
        }
        let mut portable = HashSet::new();
        for child in &children {
            if let Some(name) = child.file_name().and_then(|name| name.to_str()) {
                let key = portable_path_key(name);
                if !portable.insert(key) {
                    return Err(invalid(format!(
                        "portable source name collision: {}",
                        child.display()
                    )));
                }
            }
            self.listed.insert(child.clone());
        }
        let mut names: Vec<OsString> = children
            .iter()
            .filter_map(|child| child.file_name().map(ToOwned::to_owned))
            .collect();
        names.sort();
        if let Some(matching) = matching {
            let key = (path.to_path_buf(), matching.to_owned());
            if self
                .matches
                .get(&key)
                .is_some_and(|previous| *previous != names)
            {
                return Err(invalid(format!(
                    "bundle source changed while loading: {}",
                    path.display()
                )));
            }
            self.matches.insert(key, names);
        } else {
            if self
                .directories
                .get(path)
                .is_some_and(|previous| *previous != names)
            {
                return Err(invalid(format!(
                    "bundle source changed while loading: {}",
                    path.display()
                )));
            }
            self.directories.insert(path.to_path_buf(), names);
        }
        Ok(children)
    }

    fn find(&mut self, relative: &str) -> Result<PathBuf, BundleError> {
        let mut parent = self.root.clone();
        for part in relative.split('/') {
            self.observe(&parent)?;
            let matches = self.directory(&parent, Some(part))?;
            let selected = matches
                .first()
                .filter(|child| {
                    matches.len() == 1
                        && child.file_name().and_then(|name| name.to_str()) == Some(part)
                })
                .ok_or_else(|| {
                    invalid(format!(
                        "missing, case-mismatched or ambiguous source path: {relative}"
                    ))
                })?;
            parent.clone_from(selected);
            self.observe(&parent)?;
        }
        Ok(parent)
    }

    fn read(&mut self, path: &Path) -> Result<Vec<u8>, BundleError> {
        let before = self.observe(path)?;
        if !is_regular(before.mode) {
            return Err(invalid(format!(
                "expected a source file: {}",
                path.display()
            )));
        }
        let mut file = fs::File::open(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                invalid(format!(
                    "bundle source changed while opening: {}",
                    path.display()
                ))
            } else {
                BundleError::Io(error)
            }
        })?;
        let opened = file.metadata().map_err(BundleError::Io)?;
        if Signature::from(&opened) != before {
            return Err(invalid(format!(
                "bundle source changed while opening: {}",
                path.display()
            )));
        }
        let mut data = Vec::new();
        file.read_to_end(&mut data).map_err(BundleError::Io)?;
        let after = file.metadata().map_err(BundleError::Io)?;
        if Signature::from(&after) != Signature::from(&opened) {
            return Err(invalid(format!(
                "bundle source changed while reading: {}",
                path.display()
            )));
        }
        self.observe(path)?;
        Ok(data)
    }

    pub fn read_manifest(&mut self) -> Result<Vec<u8>, BundleError> {
        let path = self.find("flyrail.json")?;
        self.read(&path)
    }

    pub fn read_file(&mut self, path: &str) -> Result<Vec<u8>, BundleError> {
        let path = self.find(path)?;
        self.read(&path)
    }

    pub fn skill_entries(&mut self, spec: &SkillSpec) -> Result<Vec<BundleEntry>, BundleError> {
        self.tree_entries(spec.path(), spec.name(), spec.executables())
    }

    pub fn tree_entries(
        &mut self,
        path: &str,
        name: &str,
        executables: &[String],
    ) -> Result<Vec<BundleEntry>, BundleError> {
        let root = self.find(path)?;
        if !is_dir(self.observe(&root)?.mode) {
            return Err(invalid(format!("skill source must be a directory: {path}")));
        }
        let mut entries = Vec::new();
        self.walk(&root, name, "", executables, &mut entries)?;
        Ok(entries)
    }

    fn walk(
        &mut self,
        directory: &Path,
        relative: &str,
        root_relative: &str,
        executables: &[String],
        entries: &mut Vec<BundleEntry>,
    ) -> Result<(), BundleError> {
        self.observe(directory)?;
        entries.push(BundleEntry::directory(relative.to_owned())?);
        let children = self.directory(directory, None)?;
        for child in &children {
            let child_name = child
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    invalid("source name must contain NFC-normalized Unicode scalar values")
                })?
                .to_owned();
            validate_relative_path(&child_name, "source name")?;
            let entry_path = format!("{relative}/{child_name}");
            let root_path = if root_relative.is_empty() {
                child_name.clone()
            } else {
                format!("{root_relative}/{child_name}")
            };
            if is_dir(self.observe(child)?.mode) {
                self.walk(child, &entry_path, &root_path, executables, entries)?;
            } else {
                let data = self.read(child)?;
                let executable = executables.iter().any(|item| item == &root_path);
                entries.push(BundleEntry::new(entry_path, Some(data), executable)?);
            }
        }
        self.observe(directory)?;
        self.directory(directory, None)?;
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), BundleError> {
        for path in self.observed.keys() {
            self.observe(&path)?;
        }
        for path in self.directories.keys() {
            self.directory(&path, None)?;
        }
        for (path, matching) in self.matches.keys() {
            self.directory(&path, Some(&matching))?;
        }
        Ok(())
    }
}

fn matches_component(path: &Path, portable_key: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| portable_path_key(name) == portable_key)
}

pub struct ZipSource {
    archive: crate::zip::Archive,
    root: String,
    entries: BTreeMap<String, Option<usize>>,
}

impl ZipSource {
    pub fn new(archive: crate::zip::Archive, root_value: &str) -> Result<Self, BundleError> {
        let root = root_value.trim_end_matches('/');
        let root_key = portable_path_key(root);
        let mut entries: BTreeMap<String, Option<usize>> = BTreeMap::new();
        let mut explicit = HashSet::new();
        let mut portable: HashMap<String, String> = HashMap::new();
        for (index, member) in archive.members().iter().enumerate() {
            let name = member.filename.trim_end_matches('/');
            let name_key = portable_path_key(name);
            let selected = name_key == root_key
                || name_key.starts_with(&format!("{root_key}/"))
                || root_key.starts_with(&format!("{name_key}/"));
            for path in ancestors(name) {
                let key = portable_path_key(&path);
                if !(key == root_key
                    || key.starts_with(&format!("{root_key}/"))
                    || root_key.starts_with(&format!("{key}/")))
                {
                    continue;
                }
                validate_relative_path(&path, "archive path")?;
                if portable.get(&key).is_some_and(|previous| *previous != path) {
                    return Err(invalid(format!("portable archive name collision: {path}")));
                }
                portable.insert(key, path);
            }
            if !selected {
                continue;
            }
            if member.orig_filename.contains('\0') {
                return Err(invalid(format!(
                    "ambiguous archive entry: {}",
                    python_repr(&member.orig_filename)
                )));
            }
            validate_relative_path(member.orig_filename.trim_end_matches('/'), "archive path")?;
            if member.orig_filename != member.filename {
                return Err(invalid(format!(
                    "ambiguous archive entry: {}",
                    python_repr(&member.orig_filename)
                )));
            }
            let suffix = if member.is_dir { "/" } else { "" };
            let expected = format!("{name}{suffix}");
            if member.filename != expected || explicit.contains(name) {
                return Err(invalid(format!(
                    "duplicate or ambiguous archive entry: {}",
                    member.filename
                )));
            }
            explicit.insert(name.to_owned());
            let mode = member.external_attr >> 16;
            let kind = if member.create_system == 3 {
                mode & S_IFMT
            } else {
                0
            };
            let expected_kind = if member.is_dir { S_IFDIR } else { S_IFREG };
            if kind != 0 && kind != expected_kind {
                return Err(invalid(format!(
                    "archive symlinks and special files are not supported: {name}"
                )));
            }
            if member.external_attr & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(invalid(format!(
                    "archive reparse points are not supported: {name}"
                )));
            }
            for path in ancestors(name) {
                let is_file = path == name && !member.is_dir;
                if entries
                    .get(&path)
                    .is_some_and(|previous| is_file || previous.is_some())
                {
                    return Err(invalid(format!("archive file/directory collision: {path}")));
                }
                entries.insert(path, if is_file { Some(index) } else { None });
            }
        }
        if entries.get(root).copied() != Some(None) {
            return Err(invalid("package resource must be an existing directory"));
        }
        Ok(Self {
            archive,
            root: root.to_owned(),
            entries,
        })
    }

    pub fn read_manifest(&self) -> Result<Vec<u8>, BundleError> {
        let path = format!("{}/flyrail.json", self.root);
        let missing = || invalid("package resource must contain a flyrail.json file");
        let index = self
            .entries
            .get(&path)
            .copied()
            .flatten()
            .ok_or_else(&missing)?;
        let member = self.archive.members().get(index).ok_or_else(missing)?;
        self.archive.read(member).map_err(BundleError::Io)
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, BundleError> {
        let full = format!("{}/{}", self.root, path);
        let missing = || invalid(format!("expected a source file: {path}"));
        let index = self
            .entries
            .get(&full)
            .copied()
            .flatten()
            .ok_or_else(&missing)?;
        let member = self.archive.members().get(index).ok_or_else(missing)?;
        self.archive.read(member).map_err(BundleError::Io)
    }

    pub fn skill_entries(&self, spec: &SkillSpec) -> Result<Vec<BundleEntry>, BundleError> {
        self.tree_entries(spec.path(), spec.name(), spec.executables())
    }

    pub fn tree_entries(
        &self,
        path: &str,
        name: &str,
        executables: &[String],
    ) -> Result<Vec<BundleEntry>, BundleError> {
        let root = format!("{}/{}", self.root, path);
        if self.entries.get(&root).copied() != Some(None) {
            return Err(invalid(format!(
                "skill source must be an existing directory: {path}"
            )));
        }
        let mut result = vec![BundleEntry::directory(name.to_owned())?];
        for (entry_path, index) in &self.entries {
            if entry_path == &root {
                continue;
            }
            if let Some(relative) = entry_path.strip_prefix(&format!("{root}/")) {
                let data = match index {
                    Some(index) => {
                        let member =
                            self.archive.members().get(*index).ok_or_else(|| {
                                invalid(format!("expected a source file: {path}"))
                            })?;
                        Some(self.archive.read(member).map_err(BundleError::Io)?)
                    }
                    None => None,
                };
                let executable = executables.iter().any(|item| item == relative);
                result.push(BundleEntry::new(
                    format!("{name}/{relative}"),
                    data,
                    executable,
                )?);
            }
        }
        Ok(result)
    }
}

fn python_repr(text: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if u32::from(other) < 0x20 => {
                let _ = write!(out, "\\x{:02x}", u32::from(other));
            }
            other => out.push(other),
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]
mod tests {
    use super::{DirectorySource, ZipSource, matches_component};
    use crate::bundle::BundleError;
    use crate::models::BundleEntry;
    use crate::test_support::{DEFAULT_MANIFEST, case_sensitive, temp_dir, write, write_bundle};
    use crate::zip::Archive;
    use crate::zip::test_archive::{Spec, blank, file, write_archive};
    use std::path::Path;

    const NONE: [String; 0] = [];
    const ARCHIVE_MANIFEST: &str = concat!(
        r#"{"schema_version":1,"id":"tiny","version":"1","#,
        r#""skills":[{"name":"a","path":"a"}]}"#
    );

    fn message(error: BundleError) -> String {
        match error {
            BundleError::Invalid(value) => value.message().to_owned(),
            BundleError::Io(error) => panic!("expected invalid input, got {error}"),
        }
    }

    fn failure<T>(result: Result<T, BundleError>) -> String {
        match result {
            Ok(_) => panic!("expected an error"),
            Err(error) => message(error),
        }
    }

    fn members() -> Vec<Spec> {
        vec![
            file("flyrail/flyrail.json", ARCHIVE_MANIFEST.as_bytes().to_vec()),
            file("flyrail/a/SKILL.md", Vec::new()),
        ]
    }

    fn source(specs: &[Spec], resource: &str) -> Result<ZipSource, BundleError> {
        let archive = Archive::open(write_archive(specs)).expect("test archive must parse");
        ZipSource::new(archive, resource)
    }

    fn with_extra(names: &[&str]) -> Vec<Spec> {
        let mut specs = members();
        specs.extend(
            names
                .iter()
                .map(|name| file(format!("flyrail/{name}"), b"bad".to_vec())),
        );
        specs
    }

    fn preserved(directory: &Path, expected: &str) -> bool {
        std::fs::read_dir(directory)
            .expect("directory must be readable")
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_str() == Some(expected))
    }

    #[test]
    fn reads_the_manifest_and_walks_skill_trees() {
        let root = temp_dir("directory-source");
        let skill = write_bundle(&root, DEFAULT_MANIFEST);
        std::fs::create_dir(skill.join("empty")).expect("dir must be creatable");
        write(&skill.join("tools/run.sh"), b"#!/bin/sh\n");
        let mut source = DirectorySource::new(&root).expect("source must open");

        assert_eq!(source.read_manifest().unwrap(), DEFAULT_MANIFEST.as_bytes());
        assert_eq!(
            source.read_file("skills/review/tools/run.sh").unwrap(),
            b"#!/bin/sh\n"
        );
        let mut entries = source
            .tree_entries("skills/review", "review", &["tools/run.sh".to_owned()])
            .unwrap();
        entries.sort_by(|left, right| left.path().cmp(right.path()));
        let observed: Vec<(&str, bool, bool)> = entries
            .iter()
            .map(|entry| (entry.path(), entry.is_directory(), entry.executable()))
            .collect();
        assert_eq!(
            observed,
            [
                ("review", true, false),
                ("review/SKILL.md", false, false),
                ("review/empty", true, false),
                ("review/tools", true, false),
                ("review/tools/run.sh", false, true),
            ]
        );
        source.finish().unwrap();
    }

    #[test]
    fn rejects_missing_case_mismatched_and_nonfile_paths() {
        let root = temp_dir("directory-source-paths");
        write_bundle(&root, DEFAULT_MANIFEST);
        std::fs::rename(root.join("skills"), root.join("SKILLS")).expect("rename must work");
        let mut source = DirectorySource::new(&root).expect("source must open");

        assert_eq!(
            failure(source.read_file("skills/review/SKILL.md")),
            "missing, case-mismatched or ambiguous source path: skills/review/SKILL.md"
        );
        assert_eq!(
            failure(source.read_file("SKILLS")),
            format!("expected a source file: {}", root.join("SKILLS").display())
        );
        assert_eq!(
            failure(source.tree_entries("SKILLS/review/SKILL.md", "review", &NONE)),
            "skill source must be a directory: SKILLS/review/SKILL.md"
        );
    }

    #[test]
    fn rejects_a_nondirectory_root_and_symlinked_sources() {
        let root = temp_dir("directory-source-kinds");
        let skill = write_bundle(&root, DEFAULT_MANIFEST);
        assert_eq!(
            failure(DirectorySource::new(&root.join("flyrail.json"))),
            "bundle root must be a directory"
        );
        let external = temp_dir("directory-source-external");
        write(&external.join("SKILL.md"), b"external");
        std::os::unix::fs::symlink(&external, skill.join("linked")).expect("symlink must work");
        let mut source = DirectorySource::new(&root).expect("source must open");

        assert_eq!(
            failure(source.tree_entries("skills/review", "review", &NONE)),
            format!(
                "source symlinks and special files are not supported: {}",
                skill.join("linked").display()
            )
        );
        assert_eq!(
            failure(DirectorySource::new(&skill.join("linked"))),
            format!(
                "source symlinks and special files are not supported: {}",
                skill.join("linked").display()
            )
        );
    }

    #[test]
    fn rejects_special_files_without_opening_them() {
        let root = temp_dir("directory-source-special");
        let skill = write_bundle(&root, DEFAULT_MANIFEST);
        let pipe = skill.join("pipe");
        let path =
            std::ffi::CString::new(pipe.as_os_str().as_encoded_bytes()).expect("path encodes");
        // SAFETY: mkfifo only creates the named FIFO at this test-owned path.
        let created = unsafe { libc::mkfifo(path.as_ptr(), 0o600) };
        assert_eq!(created, 0);
        let mut source = DirectorySource::new(&root).expect("source must open");

        assert_eq!(
            failure(source.tree_entries("skills/review", "review", &NONE)),
            format!(
                "source symlinks and special files are not supported: {}",
                pipe.display()
            )
        );
    }

    #[test]
    fn rejects_nonportable_actual_source_names() {
        let root = temp_dir("directory-source-names");
        let skill = write_bundle(&root, DEFAULT_MANIFEST);
        let mut checked = 0;
        for name in [
            "CON.txt",
            "trailing.",
            "bad\u{7f}",
            "bad\u{85}",
            "bad?",
            "cafe\u{301}",
        ] {
            let selected = skill.join(name);
            if std::fs::create_dir(&selected).is_err() || !preserved(&skill, name) {
                continue;
            }
            let mut source = DirectorySource::new(&root).expect("source must open");
            let failure = message(
                source
                    .tree_entries("skills/review", "review", &NONE)
                    .unwrap_err(),
            );
            assert!(failure.starts_with("source name "), "{name}: {failure}");
            std::fs::remove_dir(&selected).expect("dir must be removable");
            checked += 1;
        }
        assert!(checked > 0, "host rejected every nonportable name");
    }

    #[test]
    fn rejects_source_names_that_are_not_unicode_scalars() {
        use std::os::unix::ffi::OsStringExt as _;

        let root = temp_dir("directory-source-raw-bytes");
        let skill = write_bundle(&root, DEFAULT_MANIFEST);
        let raw = std::ffi::OsString::from_vec(vec![b'r', b'a', b'w', 0xFF]);
        if std::fs::create_dir(skill.join(&raw)).is_err() {
            eprintln!("skipping: host cannot create non-UTF-8 source names");
            return;
        }
        let mut source = DirectorySource::new(&root).expect("source must open");

        assert_eq!(
            failure(source.tree_entries("skills/review", "review", &NONE)),
            "source name must contain NFC-normalized Unicode scalar values"
        );
    }

    #[test]
    fn non_unicode_names_do_not_match_unicode_lookup_components() {
        use std::os::unix::ffi::OsStringExt as _;

        let raw = Path::new("root").join(std::ffi::OsString::from_vec(vec![0xff]));
        assert!(!matches_component(
            &raw,
            &crate::validation::portable_path_key("\u{fffd}")
        ));
        assert!(matches_component(
            Path::new("root/\u{fffd}"),
            &crate::validation::portable_path_key("\u{fffd}")
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bundles_reject_unicode_paths_backed_only_by_non_unicode_names() {
        use crate::Bundle;
        use std::os::unix::ffi::OsStringExt as _;

        let root = temp_dir("directory-source-non-unicode-bundle");
        write(
            &root.join("flyrail.json"),
            concat!(
                r#"{"schema_version":2,"id":"raw","version":"1","artifacts":["#,
                r#"{"kind":"instruction","id":"guide","path":"\ufffd"}]}"#
            )
            .as_bytes(),
        );
        write(
            &root.join(std::ffi::OsString::from_vec(vec![0xff])),
            b"wrong file",
        );

        assert_eq!(
            failure(Bundle::from_directory(&root)),
            "missing, case-mismatched or ambiguous source path: \u{fffd}"
        );
    }

    #[test]
    fn rejects_portable_source_name_collisions() {
        let root = temp_dir("directory-source-collision");
        if !case_sensitive(&root) {
            eprintln!("skipping: case-insensitive filesystem cannot hold colliding names");
            return;
        }
        let skill = write_bundle(&root, DEFAULT_MANIFEST);
        std::fs::create_dir(skill.join("same")).expect("dir must be creatable");
        std::fs::create_dir(skill.join("SAME")).expect("dir must be creatable");
        let mut source = DirectorySource::new(&root).expect("source must open");

        assert!(
            failure(source.tree_entries("skills/review", "review", &NONE))
                .starts_with("portable source name collision: ")
        );
    }

    #[test]
    fn detects_sources_that_change_after_being_read() {
        let root = temp_dir("directory-source-change");
        let skill = write_bundle(&root, DEFAULT_MANIFEST);
        let mut source = DirectorySource::new(&root).expect("source must open");
        source.read_manifest().unwrap();
        source
            .tree_entries("skills/review", "review", &NONE)
            .unwrap();
        write(&skill.join("SKILL.md"), b"concurrent edit");

        assert_eq!(
            failure(source.finish()),
            format!(
                "bundle source changed while loading: {}",
                skill.join("SKILL.md").display()
            )
        );
    }

    #[test]
    fn infers_archive_directories_and_reads_members() {
        let source = source(&members(), "flyrail").expect("archive source must open");

        assert_eq!(source.read_manifest().unwrap(), ARCHIVE_MANIFEST.as_bytes());
        assert_eq!(source.read_file("a/SKILL.md").unwrap(), Vec::<u8>::new());
        let mut entries = source.tree_entries("a", "a", &NONE).unwrap();
        entries.sort_by(|left, right| left.path().cmp(right.path()));
        assert_eq!(
            entries,
            [
                BundleEntry::directory("a").unwrap(),
                BundleEntry::file("a/SKILL.md", Vec::new()).unwrap(),
            ]
        );
        assert_eq!(
            failure(source.read_file("absent")),
            "expected a source file: absent"
        );
    }

    #[test]
    fn rejects_nonportable_and_traversal_archive_names() {
        for name in [
            "a/CON.txt",
            "a/trailing.",
            "a/trailing ",
            "a/bad\u{7f}",
            "a/bad\u{85}",
            "a/bad?",
            "a/cafe\u{301}",
            "a/../outside",
            "a//double",
            "a/sub/../../outside",
            "a/C:/escape",
            "a/back\\slash",
            "a/double//slash",
        ] {
            let failure = failure(source(&with_extra(&[name]), "flyrail"));
            assert!(failure.starts_with("archive path "), "{name}: {failure}");
        }
    }

    #[test]
    fn rejects_archive_collisions_and_ambiguous_directories() {
        for names in [
            ["a/same/", "a/SAME/"].as_slice(),
            ["a/stra\u{df}e", "a/strasse"].as_slice(),
            ["a/parent/a", "a/PARENT/b"].as_slice(),
            ["a/kept", "a/kept/child"].as_slice(),
            ["a/kept/child", "a/kept"].as_slice(),
            ["a/extra//"].as_slice(),
        ] {
            let failure = failure(source(&with_extra(names), "flyrail"));
            assert!(
                failure.contains("collision") || failure.contains("ambiguous"),
                "{names:?}: {failure}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_archive_members() {
        let mut specs = members();
        specs.push(file("flyrail/a/SKILL.md", Vec::new()));
        assert_eq!(
            failure(source(&specs, "flyrail")),
            "duplicate or ambiguous archive entry: flyrail/a/SKILL.md"
        );
        let mut directories = members();
        directories.push(file("flyrail/a/", Vec::new()));
        directories.push(file("flyrail/a/", Vec::new()));
        assert_eq!(
            failure(source(&directories, "flyrail")),
            "duplicate or ambiguous archive entry: flyrail/a/"
        );
    }

    #[test]
    fn rejects_archive_symlinks_special_files_and_reparse_points() {
        for kind in [0o120_000, 0o010_000, 0o140_000, 0o020_000, super::S_IFDIR] {
            let mut specs = members();
            specs.push(Spec {
                name: "flyrail/a/special".to_owned(),
                data: b"target".to_vec(),
                create_system: 3,
                external_attr: (kind | 0o600) << 16,
                ..blank()
            });
            assert_eq!(
                failure(source(&specs, "flyrail")),
                "archive symlinks and special files are not supported: flyrail/a/special"
            );
        }
        let mut specs = members();
        specs.push(Spec {
            name: "flyrail/a/reparse".to_owned(),
            data: b"target".to_vec(),
            create_system: 0,
            external_attr: 0x400,
            ..blank()
        });
        assert_eq!(
            failure(source(&specs, "flyrail")),
            "archive reparse points are not supported: flyrail/a/reparse"
        );
    }

    #[test]
    fn rejects_nul_truncated_archive_names() {
        let mut specs = members();
        specs.push(Spec {
            raw_name: Some(b"flyrail/a/name\0hidden".to_vec()),
            data: b"data".to_vec(),
            ..blank()
        });
        assert_eq!(
            failure(source(&specs, "flyrail")),
            "ambiguous archive entry: 'flyrail/a/name\\x00hidden'"
        );
    }

    #[test]
    fn rejects_ambiguous_resource_roots_and_colliding_ancestors() {
        let mut shouted = members();
        shouted.push(file("FLYRAIL/", Vec::new()));
        assert_eq!(
            failure(source(&shouted, "flyrail")),
            "portable archive name collision: FLYRAIL"
        );
        let nested: Vec<Spec> = members()
            .into_iter()
            .map(|spec| Spec {
                name: spec.name.replace("flyrail/", "resources/flyrail/"),
                ..spec
            })
            .collect();
        for (unrelated_first, reported) in [(false, "RESOURCES"), (true, "resources")] {
            let mut specs = nested.clone();
            let member = file("RESOURCES/unrelated/file", Vec::new());
            if unrelated_first {
                specs.insert(0, member);
            } else {
                specs.push(member);
            }
            assert_eq!(
                failure(source(&specs, "resources/flyrail")),
                format!("portable archive name collision: {reported}")
            );
        }
    }

    #[test]
    fn allows_unrelated_nonportable_archive_members() {
        let mut specs = members();
        specs.push(file("unrelated/CON.txt", Vec::new()));
        specs.push(file("UNRELATED/file", Vec::new()));
        specs.push(file("outside/CON.txt", Vec::new()));
        let source = source(&specs, "flyrail").expect("archive source must open");

        assert_eq!(
            source.tree_entries("a", "a", &NONE).unwrap(),
            [
                BundleEntry::directory("a").unwrap(),
                BundleEntry::file("a/SKILL.md", Vec::new()).unwrap(),
            ]
        );
    }

    #[test]
    fn ignores_encrypted_unrelated_members_until_they_are_read() {
        let mut specs = members();
        specs.push(Spec {
            name: "flyrail/unrelated/secret.bin".to_owned(),
            data: b"secret".to_vec(),
            flag_bits: 1,
            ..blank()
        });
        let source = source(&specs, "flyrail").expect("archive source must open");

        assert_eq!(source.read_manifest().unwrap(), ARCHIVE_MANIFEST.as_bytes());
        assert_eq!(
            source.tree_entries("a", "a", &NONE).unwrap(),
            [
                BundleEntry::directory("a").unwrap(),
                BundleEntry::file("a/SKILL.md", Vec::new()).unwrap(),
            ]
        );
        match source.read_file("unrelated/secret.bin") {
            Err(BundleError::Io(error)) => {
                assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            }
            other => panic!("expected an I/O error, got {other:?}"),
        }
    }

    #[test]
    fn requires_an_existing_resource_directory_with_a_manifest() {
        assert_eq!(
            failure(source(&members(), "missing")),
            "package resource must be an existing directory"
        );
        assert_eq!(
            failure(source(&members(), "flyrail/flyrail.json")),
            "package resource must be an existing directory"
        );
        let source = source(&members()[1..], "flyrail").expect("archive source must open");
        assert_eq!(
            failure(source.read_manifest()),
            "package resource must contain a flyrail.json file"
        );
        assert_eq!(
            failure(source.tree_entries("absent", "absent", &NONE)),
            "skill source must be an existing directory: absent"
        );
    }
}
