use crate::content::{SectionBoundaries, Selector, SelectorPart};
use crate::json::Json;
use crate::validation::{portable_path_key, validate_relative_path};
use crate::values::{ValueError, semantic_record, value_from_json};
use sha2::{Digest as _, Sha256};
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthorityError {
    Invalid(ValueError),
    Unsafe {
        message: String,
        path: PathBuf,
    },
    Io {
        message: String,
        path: Option<PathBuf>,
        errno: Option<i32>,
    },
}

impl AuthorityError {
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(error) => error.message(),
            Self::Unsafe { message, .. } | Self::Io { message, .. } => message,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Invalid(_) => None,
            Self::Unsafe { path, .. } => Some(path),
            Self::Io { path, .. } => path.as_deref(),
        }
    }
}

impl From<ValueError> for AuthorityError {
    fn from(error: ValueError) -> Self {
        Self::Invalid(error)
    }
}

impl fmt::Display for AuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl Error for AuthorityError {}

fn unsafe_destination(message: &str, path: impl Into<PathBuf>) -> AuthorityError {
    AuthorityError::Unsafe {
        message: String::from(message),
        path: path.into(),
    }
}

fn io_destination(error: &io::Error, path: Option<&Path>) -> AuthorityError {
    AuthorityError::Io {
        message: os_error_text(error, path),
        path: path.map(Path::to_path_buf),
        errno: error.raw_os_error(),
    }
}

fn oserror_message(errno: i32, message: &str, path: Option<&Path>) -> String {
    path.map_or_else(
        || format!("[Errno {errno}] {message}"),
        |path| {
            format!(
                "[Errno {errno}] {message}: {}",
                crate::json::python_str_repr(&path.to_string_lossy())
            )
        },
    )
}

pub(crate) fn strerror_text(error: &io::Error, errno: i32) -> String {
    let text = error.to_string();
    let suffix = format!(" (os error {errno})");
    text.strip_suffix(&suffix).unwrap_or(&text).to_owned()
}

pub(crate) fn os_error_text(error: &io::Error, path: Option<&Path>) -> String {
    error.raw_os_error().map_or_else(
        || error.to_string(),
        |errno| oserror_message(errno, &strerror_text(error, errno), path),
    )
}

pub(crate) fn os_error_builtin_text(errno: i32, message: &str, path: &Path) -> String {
    format!(
        "[Errno {errno}] {message}: PosixPath({})",
        crate::json::python_str_repr(&path.to_string_lossy())
    )
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ownership {
    File,
    Tree,
    Subtree,
    Section,
    Structured,
}

impl Ownership {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Tree => "tree",
            Self::Subtree => "subtree",
            Self::Section => "section",
            Self::Structured => "structured",
        }
    }
}

impl FromStr for Ownership {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "file" => Ok(Self::File),
            "tree" => Ok(Self::Tree),
            "subtree" => Ok(Self::Subtree),
            "section" => Ok(Self::Section),
            "structured" => Ok(Self::Structured),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid Ownership"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ClaimSelector {
    Whole,
    Subtree(String),
    Section(SectionBoundaries),
    Structured(Selector),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Claim {
    kind: Ownership,
    selector: ClaimSelector,
}

impl Claim {
    pub fn new(kind: Ownership, selector: ClaimSelector) -> Result<Self, ValueError> {
        let selector = match kind {
            Ownership::File | Ownership::Tree => {
                if matches!(selector, ClaimSelector::Whole) {
                    selector
                } else {
                    return Err(ValueError::new(
                        "whole-resource claims cannot have selectors",
                    ));
                }
            }
            Ownership::Structured => match selector {
                ClaimSelector::Structured(_) => selector,
                _ => {
                    return Err(ValueError::type_error(
                        "structured claims require a Selector",
                    ));
                }
            },
            Ownership::Section => match selector {
                ClaimSelector::Subtree(marker) => {
                    ClaimSelector::Section(SectionBoundaries::from_marker(&marker)?)
                }
                ClaimSelector::Section(_) => selector,
                _ => {
                    return Err(ValueError::type_error(
                        "section claims require SectionBoundaries or a marker name",
                    ));
                }
            },
            Ownership::Subtree => match selector {
                ClaimSelector::Subtree(path) => {
                    validate_relative_path(&path, "subtree selector")?;
                    ClaimSelector::Subtree(path)
                }
                _ => {
                    return Err(ValueError::type_error(
                        "subtree claims require a string selector",
                    ));
                }
            },
        };
        Ok(Self { kind, selector })
    }

    pub const fn kind(&self) -> Ownership {
        self.kind
    }

    pub const fn selector(&self) -> &ClaimSelector {
        &self.selector
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        if matches!(self.kind, Ownership::File | Ownership::Tree)
            || matches!(other.kind, Ownership::File | Ownership::Tree)
        {
            return true;
        }
        if self.kind != other.kind {
            return true;
        }
        match (&self.selector, &other.selector) {
            (ClaimSelector::Section(left), ClaimSelector::Section(right)) => {
                let right_bounds = [right.start(), right.end()];
                right_bounds.contains(&left.start()) || right_bounds.contains(&left.end())
            }
            (ClaimSelector::Structured(left), ClaimSelector::Structured(right)) => {
                for (left_part, right_part) in left.parts().iter().zip(right.parts()) {
                    if left_part == right_part {
                        continue;
                    }
                    return match (left_part, right_part) {
                        (SelectorPart::Key(_), SelectorPart::Key(_)) => false,
                        (SelectorPart::Member(left_member), SelectorPart::Member(right_member)) => {
                            !(left_member.key() == right_member.key()
                                && left_member.identity() != right_member.identity())
                        }
                        _ => true,
                    };
                }
                true
            }
            (ClaimSelector::Subtree(left), ClaimSelector::Subtree(right)) => {
                let left_key = portable_path_key(left);
                let right_key = portable_path_key(right);
                let left_scope = format!("{right_key}/");
                let right_scope = format!("{left_key}/");
                left_key == right_key
                    || left_key.starts_with(&left_scope)
                    || right_key.starts_with(&right_scope)
            }
            _ => true,
        }
    }

    pub fn identity(&self, authority: &ResourceAuthority) -> Result<String, ValueError> {
        let selected = match &self.selector {
            ClaimSelector::Whole => Json::Null,
            ClaimSelector::Subtree(path) => Json::Str(path.clone()),
            ClaimSelector::Section(boundaries) => Json::Array(vec![
                Json::Str(boundaries.start().to_owned()),
                Json::Str(boundaries.end().to_owned()),
            ]),
            ClaimSelector::Structured(selector) => {
                Json::Array(selector.parts().iter().map(part_record).collect())
            }
        };
        let record = Json::Array(vec![
            Json::Str(crate::codec::posix_path(authority.destination())),
            Json::Str(self.kind.as_str().to_owned()),
            selected,
        ]);
        let value = value_from_json(&record)?;
        let mut hasher = Sha256::new();
        hasher.update(b"flyrail-claim-v1\0");
        hasher.update(value.semantic_bytes());
        Ok(sha256_hex(&hasher.finalize()))
    }
}

fn part_record(part: &SelectorPart) -> Json {
    match part {
        SelectorPart::Key(key) => Json::Array(vec![
            Json::Str(String::from("key")),
            Json::Str(key.name().to_owned()),
        ]),
        SelectorPart::Member(member) => Json::Array(vec![
            Json::Str(String::from("member")),
            Json::Array(
                member
                    .key()
                    .iter()
                    .map(|name| Json::Str(name.clone()))
                    .collect(),
            ),
            semantic_record(member.identity()),
        ]),
    }
}

pub(crate) fn sha256_hex(digest: &[u8]) -> String {
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn name_key(destination: &Path) -> String {
    let name = match destination.components().next_back() {
        Some(Component::Normal(name)) => name.to_string_lossy().into_owned(),
        Some(Component::ParentDir) => String::from(".."),
        _ => String::new(),
    };
    sha256_hex(&Sha256::digest(portable_path_key(&name).as_bytes()))
}

pub fn transaction_path(destination: &Path, transaction_id: &str) -> Result<PathBuf, ValueError> {
    if transaction_id.len() != 32
        || !transaction_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ValueError::new(
            "transaction id must be 32 lowercase hexadecimal characters",
        ));
    }
    let name = format!(".flyrail-{}-{transaction_id}.txn", name_key(destination));
    Ok(match destination.parent() {
        Some(parent) => parent.join(name),
        None if destination.is_absolute() => PathBuf::from("/").join(name),
        None => PathBuf::from(name),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceAuthority {
    destination: PathBuf,
}

impl ResourceAuthority {
    pub fn new(destination: impl AsRef<Path>) -> Result<Self, AuthorityError> {
        Ok(Self {
            destination: canonical_destination(destination)?,
        })
    }

    pub(crate) const fn unchecked(destination: PathBuf) -> Self {
        Self { destination }
    }

    pub fn destination(&self) -> &Path {
        &self.destination
    }
}

fn check_safe(metadata: &std::fs::Metadata, path: &Path) -> Result<(), AuthorityError> {
    use std::os::unix::fs::MetadataExt as _;
    if !(metadata.is_file() || metadata.is_dir()) {
        return Err(unsafe_destination(
            &format!(
                "resource links and special files are unsupported: {}",
                path.display()
            ),
            path,
        ));
    }
    if metadata.is_file() && metadata.nlink() != 1 {
        return Err(unsafe_destination(
            &format!("hard-linked resources are ambiguous: {}", path.display()),
            path,
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn physical_directory(path: &Path) -> Result<PathBuf, AuthorityError> {
    use std::ffi::CString;
    use std::fs::File;
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
    use std::os::unix::fs::MetadataExt as _;

    let encoded = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| ValueError::new("embedded null byte"))?;
    let descriptor = unsafe {
        libc::open(
            encoded.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if descriptor < 0 {
        return Err(io_destination(&io::Error::last_os_error(), Some(path)));
    }
    let opened = unsafe { File::from_raw_fd(descriptor) };
    let opened_metadata = opened
        .metadata()
        .map_err(|error| io_destination(&error, None))?;
    let mut buffer = [0u8; 1024];
    let result = unsafe { libc::fcntl(opened.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr()) };
    if result < 0 {
        return Err(io_destination(&io::Error::last_os_error(), None));
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    let physical = PathBuf::from(OsString::from_vec(
        buffer.iter().copied().take(end).collect(),
    ));
    let current = std::fs::symlink_metadata(&physical)
        .map_err(|error| io_destination(&error, Some(physical.as_path())))?;
    if (opened_metadata.dev(), opened_metadata.ino()) != (current.dev(), current.ino()) {
        return Err(unsafe_destination(
            "resource parent changed during canonicalization",
            path,
        ));
    }
    Ok(physical)
}

#[cfg(not(target_os = "macos"))]
fn physical_directory(path: &Path) -> Result<PathBuf, AuthorityError> {
    std::fs::canonicalize(path).map_err(|error| io_destination(&error, Some(path)))
}

fn canonical_child(parent: &Path, name: &str) -> Result<PathBuf, AuthorityError> {
    use std::os::unix::fs::MetadataExt as _;

    let candidate = parent.join(name);
    let key = portable_path_key(name);
    let entries =
        std::fs::read_dir(parent).map_err(|error| io_destination(&error, Some(parent)))?;
    let mut matches: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| io_destination(&error, Some(parent)))?;
        let stored = entry.file_name();
        let Some(text) = stored.to_str() else {
            continue;
        };
        if portable_path_key(text) == key {
            matches.push(parent.join(&stored));
        }
    }
    if matches.len() > 1 {
        return Err(unsafe_destination(
            "portable resource path collision",
            &candidate,
        ));
    }
    let Some(physical) = matches.pop() else {
        return Ok(candidate);
    };
    let metadata = std::fs::symlink_metadata(&physical)
        .map_err(|error| io_destination(&error, Some(&physical)))?;
    check_safe(&metadata, &physical)?;
    if physical
        .file_name()
        .is_some_and(|stored| stored != OsStr::new(name))
    {
        let alias = match std::fs::symlink_metadata(&candidate) {
            Ok(alias) => alias,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(unsafe_destination(
                    "resource path has a conflicting portable spelling",
                    &candidate,
                ));
            }
            Err(error) => return Err(io_destination(&error, Some(&candidate))),
        };
        if (alias.dev(), alias.ino()) != (metadata.dev(), metadata.ino()) {
            return Err(unsafe_destination(
                "resource path alias is ambiguous",
                &candidate,
            ));
        }
    }
    Ok(physical)
}

fn surrogate_component_error(bytes: &[u8]) -> ValueError {
    let mut cursor = 0;
    while let Some(lead) = bytes.get(cursor).copied() {
        let width = match lead {
            0x00..=0x7F => 1,
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => 0,
        };
        let character = bytes
            .get(cursor..cursor + width)
            .and_then(|window| std::str::from_utf8(window).ok())
            .and_then(|text| text.chars().next());
        if let Some(character) = character {
            let code = u32::from(character);
            if code < 0x20 || (0x7F..=0x9F).contains(&code) || b"<>:\"\\|?*".contains(&lead) {
                return ValueError::new("resource path component contains a nonportable character");
            }
            cursor += width;
        } else {
            cursor += 1;
        }
    }
    ValueError::new("resource path component must contain NFC-normalized Unicode scalar values")
}

fn validate_destination(path: &Path) -> Result<(), AuthorityError> {
    use std::os::unix::ffi::OsStrExt as _;
    for component in path.components().skip(1) {
        let Component::Normal(name) = component else {
            continue;
        };
        let bytes = name.as_bytes();
        let Ok(text) = std::str::from_utf8(bytes) else {
            return Err(surrogate_component_error(bytes).into());
        };
        validate_relative_path(text, "resource path component")?;
        if management_name(text) {
            return Err(
                ValueError::new("resource destination enters reserved management state").into(),
            );
        }
    }
    Ok(())
}

pub fn canonical_destination(path: impl AsRef<Path>) -> Result<PathBuf, AuthorityError> {
    use std::os::unix::ffi::OsStrExt as _;
    let bytes = path.as_ref().as_os_str().as_bytes();
    if bytes.is_empty() || bytes.contains(&0) {
        return Err(ValueError::new(
            "resource destination must be a nonempty string path without NUL",
        )
        .into());
    }
    let mut selected = path.as_ref().to_path_buf();
    if !selected.is_absolute() {
        let current = std::env::current_dir().map_err(|error| io_destination(&error, None))?;
        selected = current.join(selected);
    }
    if selected
        .components()
        .any(|part| matches!(part, Component::ParentDir))
        || selected.parent().is_none()
    {
        return Err(
            ValueError::new("resource destination cannot be a root or contain traversal").into(),
        );
    }
    validate_destination(&selected)?;
    let components: Vec<Component> = selected.components().skip(1).collect();
    let Some(final_component) = components.last().copied() else {
        return Err(
            ValueError::new("resource destination cannot be a root or contain traversal").into(),
        );
    };
    let file_name = final_component.as_os_str().to_str().ok_or_else(|| {
        ValueError::new("resource destination must be a nonempty string path without NUL")
    })?;
    let mut current = PathBuf::from("/");
    let mut missing: Vec<OsString> = Vec::new();
    for component in components.iter().copied().take(components.len() - 1) {
        let Component::Normal(name) = component else {
            continue;
        };
        if !missing.is_empty() {
            missing.push(name.to_os_string());
            continue;
        }
        let candidate = current.join(name);
        match std::fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(name.to_os_string());
            }
            Err(error) => return Err(io_destination(&error, Some(&candidate))),
            Ok(metadata) => {
                check_safe(&metadata, &candidate)?;
                if !metadata.is_dir() {
                    return Err(unsafe_destination(
                        "resource ancestor must be a directory",
                        &candidate,
                    ));
                }
                current = candidate;
            }
        }
    }
    let parent = physical_directory(&current)?;
    let canonical = if let Some(first) = missing.first() {
        let first = first.to_str().ok_or_else(|| {
            AuthorityError::Invalid(ValueError::new(
                "resource destination must be a nonempty string path without NUL",
            ))
        })?;
        let mut staged = canonical_child(&parent, first)?;
        for name in missing.iter().skip(1) {
            staged.push(name);
        }
        staged.push(file_name);
        staged
    } else {
        canonical_child(&parent, file_name)?
    };
    validate_destination(&canonical)?;
    Ok(canonical)
}

fn owned_components(path: &Path) -> Vec<Option<OsString>> {
    path.components()
        .filter_map(|component| match component {
            Component::Prefix(_) | Component::RootDir => Some(None),
            Component::CurDir => None,
            Component::ParentDir => Some(Some(OsString::from(".."))),
            Component::Normal(name) => Some(Some(name.to_os_string())),
        })
        .collect()
}

fn lenient_resolve(path: &Path) -> PathBuf {
    const MAX_LINKS: usize = 40;
    let mut pending: std::collections::VecDeque<Option<OsString>> = owned_components(path).into();
    let mut links = 0;
    let mut resolved = PathBuf::from("/");
    while let Some(component) = pending.pop_front() {
        let Some(name) = component else {
            resolved = PathBuf::from("/");
            continue;
        };
        if name == ".." {
            resolved.pop();
            continue;
        }
        let candidate = resolved.join(&name);
        let target = std::fs::symlink_metadata(&candidate)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
            .then(|| std::fs::read_link(&candidate).ok())
            .flatten()
            .filter(|_| links < MAX_LINKS);
        if let Some(target) = target {
            links += 1;
            for (offset, part) in owned_components(&target).into_iter().enumerate() {
                pending.insert(offset, part);
            }
        } else {
            resolved = candidate;
        }
    }
    resolved
}

pub(crate) fn physical_location(path: &Path) -> Result<PathBuf, AuthorityError> {
    let absolute = std::path::absolute(path).map_err(|error| io_destination(&error, Some(path)))?;
    let mut existing = lenient_resolve(&absolute);
    let mut missing: Vec<OsString> = Vec::new();
    while !existing.is_dir() {
        let Some(name) = existing.file_name().map(OsStr::to_os_string) else {
            break;
        };
        missing.insert(0, name);
        existing.pop();
    }
    let mut physical = physical_directory(&existing)?;
    physical.extend(missing);
    Ok(physical)
}

#[allow(clippy::case_sensitive_file_extension_comparisons)]
pub(crate) fn management_name(name: &str) -> bool {
    let folded = portable_path_key(name);
    folded.starts_with(".flyrail-") && folded.ends_with(".txn")
}

pub(crate) fn transaction_entries(destination: &Path) -> Result<Vec<PathBuf>, AuthorityError> {
    let mut entries = Vec::new();
    let mut pending: Vec<PathBuf> = vec![destination.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let metadata = std::fs::symlink_metadata(&directory)
            .map_err(|error| io_destination(&error, Some(&directory)))?;
        check_safe(&metadata, &directory)?;
        if !metadata.is_dir() {
            return Err(unsafe_destination(
                "whole-tree resource must be a directory",
                &directory,
            ));
        }
        let children = std::fs::read_dir(&directory)
            .map_err(|error| io_destination(&error, Some(&directory)))?;
        for entry in children {
            let entry = entry.map_err(|error| io_destination(&error, Some(&directory)))?;
            let child = entry.path();
            if management_name(&entry.file_name().to_string_lossy()) {
                entries.push(child);
                continue;
            }
            let metadata = std::fs::symlink_metadata(&child)
                .map_err(|error| io_destination(&error, Some(&child)))?;
            check_safe(&metadata, &child)?;
            if metadata.is_dir() {
                pending.push(child);
            }
        }
    }
    Ok(entries)
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
    use crate::test_support::fixture;

    fn claim_vector(kind: &str, selector: &Json) -> Result<Claim, ValueError> {
        let ownership = Ownership::from_str(kind)?;
        let selector = claim_selector_vector(selector)?;
        Claim::new(ownership, selector)
    }

    fn claim_selector_vector(selector: &Json) -> Result<ClaimSelector, ValueError> {
        match selector {
            Json::Null => Ok(ClaimSelector::Whole),
            Json::Str(path) => Ok(ClaimSelector::Subtree(path.clone())),
            Json::Array(items)
                if items.len() == 2 && items.iter().all(|item| item.as_str().is_some()) =>
            {
                let start = items[0].as_str().expect("two strings");
                let end = items[1].as_str().expect("two strings");
                Ok(ClaimSelector::Section(SectionBoundaries::new(start, end)?))
            }
            Json::Array(items) => {
                let mut parts = Vec::new();
                for item in items {
                    let part = item
                        .as_array()
                        .ok_or_else(|| ValueError::new("invalid claim vector selector"))?;
                    match part.first().and_then(Json::as_str) {
                        Some("key") => {
                            let name = part
                                .get(1)
                                .and_then(Json::as_str)
                                .ok_or_else(|| ValueError::new("invalid claim vector selector"))?;
                            parts.push(SelectorPart::Key(crate::content::Key::new(name)?));
                        }
                        Some("member") => {
                            let keys = part
                                .get(1)
                                .and_then(Json::as_array)
                                .ok_or_else(|| ValueError::new("invalid claim vector selector"))?
                                .iter()
                                .map(|item| {
                                    item.as_str().map(str::to_owned).ok_or_else(|| {
                                        ValueError::new("invalid claim vector selector")
                                    })
                                })
                                .collect::<Result<Vec<String>, _>>()?;
                            let identity =
                                crate::values::value_from_record(part.get(2).ok_or_else(
                                    || ValueError::new("invalid claim vector selector"),
                                )?)?;
                            parts.push(SelectorPart::Member(crate::content::Member::new(
                                identity, keys,
                            )?));
                        }
                        _ => return Err(ValueError::new("invalid claim vector selector")),
                    }
                }
                Ok(ClaimSelector::Structured(Selector::new(parts)?))
            }
            _ => Err(ValueError::new("invalid claim vector selector")),
        }
    }

    #[test]
    fn fixture_claim_identity_vectors_match() {
        let Some(data) = fixture("configurations.json") else {
            eprintln!("skipping: shared fixtures are unavailable");
            return;
        };
        let Some(claims) = data.get("claims") else {
            panic!("fixture is missing claims");
        };
        let items = claims.as_array().expect("claims must be an array");
        for item in items {
            let name = item.get("name").and_then(Json::as_str).expect("claim name");
            let destination = PathBuf::from(
                item.get("destination")
                    .and_then(Json::as_str)
                    .expect("claim destination"),
            );
            let kind = item.get("kind").and_then(Json::as_str).expect("claim kind");
            let selector = item.get("selector").expect("claim selector");
            let expected = item
                .get("identity")
                .and_then(Json::as_str)
                .expect("claim identity");
            let claim =
                claim_vector(kind, selector).unwrap_or_else(|error| panic!("{name}: {error}"));
            let authority = ResourceAuthority::unchecked(destination.clone());
            let actual = claim
                .identity(&authority)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(actual, expected, "claim identity mismatch for {name}");
        }
    }

    #[test]
    fn fixture_claim_overlap_vectors_match() {
        let Some(data) = fixture("configurations.json") else {
            eprintln!("skipping: shared fixtures are unavailable");
            return;
        };
        let claims = data.get("claims").and_then(Json::as_array).expect("claims");
        let by_name: Vec<(String, Claim)> = claims
            .iter()
            .map(|item| {
                let name = item
                    .get("name")
                    .and_then(Json::as_str)
                    .expect("claim name")
                    .to_owned();
                let kind = item.get("kind").and_then(Json::as_str).expect("claim kind");
                let selector = item.get("selector").expect("claim selector");
                (name, claim_vector(kind, selector).expect("claim vector"))
            })
            .collect();
        for item in data
            .get("claim_overlaps")
            .and_then(Json::as_array)
            .expect("claim_overlaps")
        {
            let first = item
                .get("first")
                .and_then(Json::as_str)
                .expect("first claim");
            let second = item
                .get("second")
                .and_then(Json::as_str)
                .expect("second claim");
            let expected = item
                .get("overlaps")
                .and_then(Json::as_bool)
                .expect("overlaps");
            let left = by_name
                .iter()
                .find(|(name, _)| name == first)
                .map(|(_, claim)| claim)
                .expect("first claim exists");
            let right = by_name
                .iter()
                .find(|(name, _)| name == second)
                .map(|(_, claim)| claim)
                .expect("second claim exists");
            assert_eq!(
                left.overlaps(right),
                expected,
                "overlap mismatch for {first} vs {second}"
            );
        }
    }

    #[test]
    fn fixture_authority_vectors_match() {
        let Some(data) = fixture("configurations.json") else {
            eprintln!("skipping: shared fixtures are unavailable");
            return;
        };
        for item in data
            .get("authorities")
            .and_then(Json::as_array)
            .expect("authorities")
        {
            let name = item
                .get("name")
                .and_then(Json::as_str)
                .expect("authority name");
            let destination = PathBuf::from(
                item.get("destination")
                    .and_then(Json::as_str)
                    .expect("authority destination"),
            );
            assert_eq!(
                crate::state::destination_key(&destination),
                item.get("destination_key")
                    .and_then(Json::as_str)
                    .expect("destination_key"),
                "destination key mismatch for {name}"
            );
            let transaction_id = item
                .get("transaction_id")
                .and_then(Json::as_str)
                .expect("transaction_id");
            assert_eq!(
                transaction_path(&destination, transaction_id)
                    .expect("transaction path")
                    .to_string_lossy()
                    .as_ref(),
                item.get("transaction_path")
                    .and_then(Json::as_str)
                    .expect("transaction_path"),
                "transaction path mismatch for {name}"
            );
        }
    }

    const TRANSACTION: &str = "0123456789abcdef0123456789abcdef";

    fn admitted(
        store: &crate::stores::MemoryStore,
        authority: &ResourceAuthority,
        kind: crate::resource_models::ResourceKind,
    ) {
        let reference =
            crate::resource_models::ResourceRef::new(authority.destination().to_path_buf(), kind)
                .expect("reference");
        crate::state::write_record(
            store,
            &crate::state::resource_key(authority.destination(), "authority"),
            &reference,
            None,
        )
        .expect("authority record");
    }

    fn conflicts(
        store: &crate::stores::MemoryStore,
        authority: &ResourceAuthority,
        whole_tree: bool,
    ) -> Result<Vec<PathBuf>, crate::observations::TargetError> {
        crate::state::hierarchy_conflicts(store, authority, whole_tree)
    }

    #[test]
    fn transaction_path_hash_uses_portable_name_only() {
        let destination = Path::new("/tmp/targets/AGENTS.md");
        let expected = format!(
            ".flyrail-{}-{TRANSACTION}.txn",
            sha256_hex(&Sha256::digest(portable_path_key("AGENTS.md").as_bytes()))
        );
        assert_eq!(
            transaction_path(destination, TRANSACTION).expect("path"),
            Path::new("/tmp/targets").join(&expected)
        );
        assert_eq!(
            transaction_path(destination, TRANSACTION)
                .expect("path")
                .file_name(),
            transaction_path(Path::new("/elsewhere/AGENTS.md"), TRANSACTION)
                .expect("path")
                .file_name(),
            "transaction path name depends on the file name only"
        );
    }

    #[test]
    fn transaction_path_hashes_trailing_parent_dir_like_python() {
        let expected = Path::new("/a").join(format!(
            ".flyrail-5ec1f7e700f37c3d0b2981d04855fc34b94aaa15457b05ca571817442d228f81-{TRANSACTION}.txn"
        ));
        assert_eq!(
            transaction_path(Path::new("/a/.."), TRANSACTION).expect("path"),
            expected
        );
        assert_eq!(
            transaction_path(Path::new("/a/."), TRANSACTION).expect("path"),
            transaction_path(Path::new("/a"), TRANSACTION).expect("path"),
            "a trailing current-directory component folds into the previous name"
        );
    }

    #[test]
    fn transaction_paths_require_a_transaction_id() {
        for transaction_id in [
            "",
            &"0".repeat(31),
            &"A".repeat(32),
            &format!("../{}", "0".repeat(29)),
        ] {
            assert_eq!(
                transaction_path(Path::new("/tmp/AGENTS.md"), transaction_id)
                    .unwrap_err()
                    .message(),
                "transaction id must be 32 lowercase hexadecimal characters",
                "{transaction_id:?}"
            );
        }
    }

    #[test]
    fn canonical_destination_messages_match_python() {
        let cases = [
            (
                "",
                "resource destination must be a nonempty string path without NUL",
            ),
            (
                "/a\0b",
                "resource destination must be a nonempty string path without NUL",
            ),
            (
                "/",
                "resource destination cannot be a root or contain traversal",
            ),
            (
                "/a/../b",
                "resource destination cannot be a root or contain traversal",
            ),
            (
                "/tmp/.FLYRAIL-x.TXN",
                "resource destination enters reserved management state",
            ),
            (
                "/tmp/CON",
                "resource path component contains a reserved Windows name",
            ),
            (
                "/tmp/a<b",
                "resource path component contains a nonportable character",
            ),
        ];
        for (path, expected) in cases {
            assert_eq!(
                canonical_destination(path).unwrap_err().message(),
                expected,
                "destination {path:?}"
            );
        }
    }

    #[test]
    fn non_utf8_destinations_reject_with_python_messages() {
        fn byte_path(bytes: &[u8]) -> PathBuf {
            use std::os::unix::ffi::OsStringExt as _;
            PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()))
        }
        let cases: Vec<(&[u8], &str)> = vec![
            (
                b"/tmp/\xff".as_slice(),
                "resource path component must contain NFC-normalized Unicode scalar values",
            ),
            (
                b"/tmp/a\xc2\x85\xff".as_slice(),
                "resource path component contains a nonportable character",
            ),
            (
                b"rel\xff".as_slice(),
                "resource path component must contain NFC-normalized Unicode scalar values",
            ),
            (
                b"/tmp/\0\xff".as_slice(),
                "resource destination must be a nonempty string path without NUL",
            ),
            (
                b"/tmp/\xff/../b".as_slice(),
                "resource destination cannot be a root or contain traversal",
            ),
            (
                b"/tmp/.flyrail-x.txn/\xff".as_slice(),
                "resource destination enters reserved management state",
            ),
        ];
        for (bytes, expected) in cases {
            assert_eq!(
                canonical_destination(byte_path(bytes))
                    .unwrap_err()
                    .message(),
                expected,
                "destination {bytes:?}"
            );
        }
    }

    #[test]
    fn io_failures_surface_python_oserror_text() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = crate::test_support::checkout_temp_dir("io-text");
        let locked = tmp.join("locked");
        std::fs::create_dir(&locked).expect("locked dir");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))
            .expect("locked mode");
        let error = canonical_destination(locked.join("child.md")).unwrap_err();
        assert_eq!(
            error.message(),
            format!(
                "[Errno 13] Permission denied: {}",
                crate::json::python_str_repr(&locked.to_string_lossy())
            )
        );
        assert_eq!(error.path(), Some(locked.as_path()));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700))
            .expect("restore mode");
    }

    #[test]
    fn rejects_ambiguous_resource_destinations() {
        for path in ["", "bad\0path", "/", "../other", "a/../other"] {
            assert!(
                canonical_destination(path).is_err(),
                "destination {path:?} must be rejected"
            );
        }
    }

    #[test]
    fn relative_resource_destinations_snapshot_the_physical_working_directory() {
        let current = std::env::current_dir().expect("working directory");
        let expected = physical_directory(&current)
            .expect("physical working directory")
            .join("flyrail-probe/rel");
        let resolved = canonical_destination("flyrail-probe/rel").expect("relative destination");
        assert_eq!(resolved, expected);
    }

    fn directory_names(path: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(path)
            .expect("directory listing")
            .map(|entry| {
                entry
                    .expect("directory entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    fn structured_claim(parts: Vec<SelectorPart>) -> Claim {
        let selector = Selector::new(parts).expect("selector");
        Claim::new(Ownership::Structured, ClaimSelector::Structured(selector)).expect("claim")
    }

    fn key_part(name: &str) -> SelectorPart {
        SelectorPart::Key(crate::content::Key::new(name).expect("key"))
    }

    fn member_part(identity: crate::values::Value, key: &[&str]) -> SelectorPart {
        let keys: Vec<String> = key.iter().map(|name| (*name).to_owned()).collect();
        SelectorPart::Member(crate::content::Member::new(identity, keys).expect("member"))
    }

    #[test]
    fn routes_to_same_document_share_one_authority_without_writes() {
        let tmp = crate::test_support::checkout_temp_dir("same-document");
        let project = tmp.join("home/work/project");
        std::fs::create_dir_all(&project).expect("project dir");
        let document = project.join("AGENTS.md");
        std::fs::write(&document, b"owned and foreign text").expect("document");
        let from_home = ResourceAuthority::new(tmp.join("home").join("work/project/AGENTS.md"))
            .expect("home authority");
        let from_project = ResourceAuthority::new(&document).expect("project authority");
        assert_eq!(from_home, from_project);
        assert_eq!(
            transaction_path(from_home.destination(), TRANSACTION)
                .expect("path")
                .parent(),
            Some(project.as_path())
        );
        assert_eq!(
            conflicts(&crate::stores::MemoryStore::new(), &from_home, false).expect("conflicts"),
            Vec::<PathBuf>::new()
        );
        assert_eq!(directory_names(&project), vec![String::from("AGENTS.md")]);
    }

    #[test]
    fn absent_destinations_and_nested_roots_keep_disjoint_authorities() {
        let tmp = crate::test_support::checkout_temp_dir("disjoint-authorities");
        let home = ResourceAuthority::new(tmp.join("home/AGENTS.md")).expect("home authority");
        let project =
            ResourceAuthority::new(tmp.join("home/project/AGENTS.md")).expect("project authority");
        assert_eq!(home.destination(), &tmp.join("home/AGENTS.md"));
        assert_ne!(
            transaction_path(project.destination(), TRANSACTION).expect("path"),
            transaction_path(home.destination(), TRANSACTION).expect("path")
        );
        let store = crate::stores::MemoryStore::new();
        admitted(
            &store,
            &home,
            crate::resource_models::ResourceKind::Document,
        );
        assert_eq!(
            conflicts(&store, &project, false).expect("conflicts"),
            Vec::<PathBuf>::new()
        );
        assert_eq!(directory_names(&tmp), [] as [String; 0]);
        assert_eq!(
            transaction_path(&tmp.join("ABSENT.md"), TRANSACTION).expect("path"),
            transaction_path(&tmp.join("absent.md"), TRANSACTION).expect("path")
        );
        assert_eq!(
            crate::state::destination_key(&tmp.join("ABSENT.md")),
            crate::state::destination_key(&tmp.join("absent.md"))
        );
    }

    #[test]
    fn long_resource_names_have_bounded_sibling_state_names() {
        let tmp = crate::test_support::checkout_temp_dir("bounded-state");
        let authority = ResourceAuthority::new(tmp.join("a".repeat(240))).expect("long authority");
        assert_eq!(
            transaction_path(authority.destination(), TRANSACTION)
                .expect("path")
                .file_name()
                .expect("transaction name")
                .len(),
            110
        );
    }

    #[test]
    fn ancestor_case_aliases_preserve_claim_identity() {
        let tmp = crate::test_support::checkout_temp_dir("case-ancestor");
        let parent = tmp.join("Config");
        std::fs::create_dir(&parent).expect("parent dir");
        let alias = tmp.join("config");
        if !alias.is_dir() {
            eprintln!("skipping: host is case sensitive");
            return;
        }
        let first =
            ResourceAuthority::new(parent.join("absent/config.json")).expect("first authority");
        let second =
            ResourceAuthority::new(alias.join("absent/config.json")).expect("second authority");
        assert_eq!(first, second);
        let claim = structured_claim(vec![key_part("mcp")]);
        assert_eq!(
            claim.identity(&first).expect("identity"),
            claim.identity(&second).expect("identity")
        );
    }

    #[test]
    fn persistent_fences_detect_tree_child_conflict_even_without_receipts() {
        use crate::resource_models::ResourceKind;
        let tmp = crate::test_support::checkout_temp_dir("persistent-fences");
        let tree = ResourceAuthority::new(tmp.join("config")).expect("tree authority");
        let child =
            ResourceAuthority::new(tmp.join("config/nested/hooks.json")).expect("child authority");
        let store = crate::stores::MemoryStore::new();
        admitted(&store, &child, ResourceKind::Document);
        admitted(&store, &tree, ResourceKind::Tree);
        assert_eq!(
            conflicts(&store, &tree, true).expect("tree conflicts"),
            vec![child.destination().to_path_buf()]
        );
        assert_eq!(
            conflicts(&store, &child, false).expect("child conflicts"),
            vec![tree.destination().to_path_buf()]
        );
        let sibling = ResourceAuthority::new(tmp.join("sibling.md")).expect("sibling authority");
        assert_eq!(
            conflicts(&store, &sibling, false).expect("sibling conflicts"),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn fence_order_excludes_both_parent_and_child_admission() {
        use crate::resource_models::ResourceKind;
        let tmp = crate::test_support::checkout_temp_dir("fence-order");
        let store = crate::stores::MemoryStore::new();
        let tree = ResourceAuthority::new(tmp.join("config")).expect("tree authority");
        admitted(&store, &tree, ResourceKind::Tree);
        assert_eq!(
            conflicts(&store, &tree, true).expect("tree conflicts"),
            Vec::<PathBuf>::new()
        );
        let child = ResourceAuthority::new(tmp.join("config/child.md")).expect("child authority");
        admitted(&store, &child, ResourceKind::Document);
        assert_eq!(
            conflicts(&store, &child, false).expect("child conflicts"),
            vec![tree.destination().to_path_buf()]
        );
        assert_eq!(
            conflicts(&store, &tree, true).expect("tree conflicts"),
            vec![child.destination().to_path_buf()]
        );
    }

    #[test]
    fn resource_cannot_enter_management_state_or_regular_ancestor() {
        let tmp = crate::test_support::checkout_temp_dir("reserved-destinations");
        let reserved = ResourceAuthority::new(tmp.join(".flyrail-deadbeef.txn/staging"));
        assert_eq!(
            reserved.unwrap_err().message(),
            "resource destination enters reserved management state"
        );
        let file = tmp.join("file");
        std::fs::write(&file, b"file").expect("regular file");
        let ancestor = ResourceAuthority::new(file.join("child"));
        assert_eq!(
            ancestor.unwrap_err().message(),
            "resource ancestor must be a directory"
        );
        let authority = ResourceAuthority::new(&file).expect("file authority");
        let whole_tree = conflicts(&crate::stores::MemoryStore::new(), &authority, true);
        assert_eq!(
            whole_tree.unwrap_err().message(),
            "whole-tree resource must be a directory"
        );
    }

    #[test]
    fn resource_rejects_portable_management_names() {
        let tmp = crate::test_support::checkout_temp_dir("portable-management");
        for name in [".flyrail-deadbeef.txn", ".FLYRAIL-deadbeef.TXN"] {
            for destination in [tmp.join(name), tmp.join(name).join("staging")] {
                let rejected = ResourceAuthority::new(&destination);
                assert_eq!(
                    rejected.unwrap_err().message(),
                    "resource destination enters reserved management state",
                    "destination {destination:?}"
                );
            }
        }
    }

    #[test]
    fn canonical_physical_path_cannot_enter_management_state() {
        let tmp = crate::test_support::checkout_temp_dir("physical-management");
        let physical = tmp.join(".FLYRAIL-deadbeef.TXN");
        std::fs::create_dir(&physical).expect("management dir");
        let direct = canonical_child(&physical, "receipt.json").expect("canonical child");
        assert_eq!(
            validate_destination(&direct).unwrap_err().message(),
            "resource destination enters reserved management state"
        );
        let nested = canonical_child(&physical, "absent")
            .expect("canonical child")
            .join("receipt.json");
        assert_eq!(
            validate_destination(&nested).unwrap_err().message(),
            "resource destination enters reserved management state"
        );
    }

    #[test]
    fn tree_hierarchy_detects_case_aliased_management_fences() {
        let tmp = crate::test_support::checkout_temp_dir("aliased-fences");
        let tree = tmp.join("tree");
        let fence = tree.join(".FLYRAIL-deadbeef.TXN");
        std::fs::create_dir_all(&fence).expect("case-aliased fence");
        let authority = ResourceAuthority::new(&tree).expect("tree authority");
        assert_eq!(
            conflicts(&crate::stores::MemoryStore::new(), &authority, true).expect("conflicts"),
            vec![fence]
        );
    }

    #[test]
    fn case_aliases_share_authority_only_on_case_insensitive_hosts() {
        let tmp = crate::test_support::checkout_temp_dir("document-aliases");
        let original = tmp.join("AGENTS.md");
        std::fs::write(&original, b"text").expect("document");
        let alias = tmp.join("agents.md");
        if alias.exists() {
            let aliased = ResourceAuthority::new(&alias).expect("aliased authority");
            let exact = ResourceAuthority::new(&original).expect("exact authority");
            assert_eq!(aliased, exact);
        } else {
            let spelling = ResourceAuthority::new(&alias);
            assert_eq!(
                spelling.unwrap_err().message(),
                "resource path has a conflicting portable spelling"
            );
            std::fs::write(&alias, b"different file").expect("second document");
            let collision = ResourceAuthority::new(&original);
            assert_eq!(
                collision.unwrap_err().message(),
                "portable resource path collision"
            );
        }
    }

    #[test]
    fn hardlinked_documents_cannot_create_split_authorities() {
        let tmp = crate::test_support::checkout_temp_dir("hardlinked-documents");
        let first = tmp.join("first.md");
        let second = tmp.join("second.md");
        std::fs::write(&first, b"same inode").expect("document");
        if std::fs::hard_link(&first, &second).is_err() {
            eprintln!("skipping: host does not permit hard links");
            return;
        }
        for path in [&first, &second] {
            let rejected = ResourceAuthority::new(path);
            assert_eq!(
                rejected.unwrap_err().message(),
                format!("hard-linked resources are ambiguous: {}", path.display()),
                "document {path:?}"
            );
        }
    }

    #[test]
    fn unsafe_links_are_rejected_before_hierarchy_traversal() {
        let tmp = crate::test_support::checkout_temp_dir("unsafe-links");
        let tree = tmp.join("tree");
        std::fs::create_dir(&tree).expect("tree dir");
        let external = tmp.join("external");
        std::fs::create_dir(&external).expect("external dir");
        let link = tree.join("link");
        if std::os::unix::fs::symlink(&external, &link).is_err() {
            eprintln!("skipping: host does not permit symlinks");
            return;
        }
        let routed = ResourceAuthority::new(link.join("file"));
        assert_eq!(
            routed.unwrap_err().message(),
            format!(
                "resource links and special files are unsupported: {}",
                link.display()
            )
        );
        let authority = ResourceAuthority::new(&tree).expect("tree authority");
        let walked = conflicts(&crate::stores::MemoryStore::new(), &authority, true);
        assert_eq!(
            walked.unwrap_err().message(),
            format!(
                "resource links and special files are unsupported: {}",
                tree.join("link").display()
            )
        );
    }

    #[test]
    fn claim_collision_algebra_is_symmetric() {
        let file = Claim::new(Ownership::File, ClaimSelector::Whole).expect("file claim");
        let tree = Claim::new(Ownership::Tree, ClaimSelector::Whole).expect("tree claim");
        let section = |marker: &str| {
            Claim::new(
                Ownership::Section,
                ClaimSelector::Subtree(marker.to_owned()),
            )
            .expect("section claim")
        };
        let subtree = |path: &str| {
            Claim::new(Ownership::Subtree, ClaimSelector::Subtree(path.to_owned()))
                .expect("subtree claim")
        };
        let text_member = |text: &str| {
            structured_claim(vec![member_part(
                crate::values::Value::Scalar(crate::values::Scalar::text(text)),
                &[],
            )])
        };
        let rows: Vec<(Claim, Claim, bool)> = vec![
            (file, section("guide"), true),
            (tree, subtree("review"), true),
            (subtree("review"), subtree("review/scripts"), true),
            (subtree("review"), subtree("other"), false),
            (section("guide"), section("other"), false),
            (
                section("guide"),
                structured_claim(vec![key_part("x")]),
                true,
            ),
            (
                structured_claim(vec![key_part("a")]),
                structured_claim(vec![key_part("b")]),
                false,
            ),
            (
                structured_claim(vec![key_part("a")]),
                structured_claim(vec![key_part("a"), key_part("b")]),
                true,
            ),
            (text_member("a"), text_member("b"), false),
            (
                structured_claim(vec![member_part(
                    crate::values::Value::Scalar(crate::values::Scalar::text("a")),
                    &["name"],
                )]),
                structured_claim(vec![member_part(
                    crate::values::Value::Scalar(crate::values::Scalar::text("b")),
                    &["id"],
                )]),
                true,
            ),
            (
                structured_claim(vec![key_part("a")]),
                text_member("a"),
                true,
            ),
        ];
        for (first, second, overlap) in rows {
            assert_eq!(
                first.overlaps(&second),
                overlap,
                "overlap mismatch for {first:?} vs {second:?}"
            );
            assert_eq!(
                second.overlaps(&first),
                overlap,
                "overlap mismatch for {second:?} vs {first:?}"
            );
        }
    }

    #[test]
    fn claim_identity_uses_physical_selector_not_attribution() {
        let tmp = crate::test_support::checkout_temp_dir("claim-identity");
        let authority = ResourceAuthority::new(tmp.join("AGENTS.md")).expect("authority");
        let boundaries = SectionBoundaries::new("start", "end").expect("section boundaries");
        let first = Claim::new(
            Ownership::Section,
            ClaimSelector::Section(boundaries.clone()),
        )
        .expect("section claim");
        let shares_start = Claim::new(
            Ownership::Section,
            ClaimSelector::Section(
                SectionBoundaries::new("start", "other end").expect("section boundaries"),
            ),
        )
        .expect("section claim");
        let shares_end = Claim::new(
            Ownership::Section,
            ClaimSelector::Section(
                SectionBoundaries::new("end", "other end").expect("section boundaries"),
            ),
        )
        .expect("section claim");
        assert!(first.overlaps(&shares_start));
        assert!(first.overlaps(&shares_end));
        let again =
            Claim::new(Ownership::Section, ClaimSelector::Section(boundaries)).expect("claim");
        assert_eq!(
            first.identity(&authority).expect("identity"),
            again.identity(&authority).expect("identity")
        );
        let moved = Claim::new(
            Ownership::Section,
            ClaimSelector::Section(
                SectionBoundaries::new("new start", "new end").expect("section boundaries"),
            ),
        )
        .expect("section claim");
        assert_ne!(
            first.identity(&authority).expect("identity"),
            moved.identity(&authority).expect("identity")
        );
        let integer = structured_claim(vec![
            key_part("hooks"),
            member_part(
                crate::values::Value::Scalar(crate::values::Scalar::integer(1)),
                &[],
            ),
        ]);
        let truthy = structured_claim(vec![
            key_part("hooks"),
            member_part(
                crate::values::Value::Scalar(crate::values::Scalar::boolean(true)),
                &[],
            ),
        ]);
        assert_ne!(
            integer.identity(&authority).expect("identity"),
            truthy.identity(&authority).expect("identity")
        );
    }

    #[test]
    fn invalid_claim_selectors() {
        let cases = [
            (
                Claim::new(Ownership::File, ClaimSelector::Subtree(String::from("x"))),
                "whole-resource claims cannot have selectors",
            ),
            (
                Claim::new(
                    Ownership::Structured,
                    ClaimSelector::Subtree(String::from("x")),
                ),
                "structured claims require a Selector",
            ),
            (
                Claim::new(Ownership::Section, ClaimSelector::Whole),
                "section claims require SectionBoundaries or a marker name",
            ),
            (
                Claim::new(Ownership::Subtree, ClaimSelector::Whole),
                "subtree claims require a string selector",
            ),
        ];
        for (result, expected) in cases {
            assert_eq!(result.unwrap_err().message(), expected);
        }
    }
}
