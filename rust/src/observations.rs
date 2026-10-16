use crate::inventory::InventoryEntry;
use crate::models::OperationStatus;
use crate::validation::{validate_identifier, validate_version};
use crate::values::ValueError;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ErrorCode {
    Busy,
    Conflict,
    Modified,
    UpdateRequired,
    Unsupported,
    IoError,
    UnsafePath,
    InvalidState,
    ConcurrentChange,
    RecoveryNeeded,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Conflict => "conflict",
            Self::Modified => "modified",
            Self::UpdateRequired => "update_required",
            Self::Unsupported => "unsupported",
            Self::IoError => "io_error",
            Self::UnsafePath => "unsafe_path",
            Self::InvalidState => "invalid_state",
            Self::ConcurrentChange => "concurrent_change",
            Self::RecoveryNeeded => "recovery_needed",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ErrorCode {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "busy" => Ok(Self::Busy),
            "conflict" => Ok(Self::Conflict),
            "modified" => Ok(Self::Modified),
            "update_required" => Ok(Self::UpdateRequired),
            "unsupported" => Ok(Self::Unsupported),
            "io_error" => Ok(Self::IoError),
            "unsafe_path" => Ok(Self::UnsafePath),
            "invalid_state" => Ok(Self::InvalidState),
            "concurrent_change" => Ok(Self::ConcurrentChange),
            "recovery_needed" => Ok(Self::RecoveryNeeded),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid ErrorCode"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetError {
    code: ErrorCode,
    message: String,
    path: Option<PathBuf>,
    errno: Option<i32>,
}

impl TargetError {
    pub fn new(
        code: ErrorCode,
        message: impl Into<String>,
        path: Option<PathBuf>,
        errno: Option<i32>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            path,
            errno,
        }
    }

    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub const fn errno(&self) -> Option<i32> {
        self.errno
    }
}

impl fmt::Display for TargetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TargetError {}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ObservationState {
    Absent,
    Installed,
    RecoveryNeeded,
    Unknown,
}

impl ObservationState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Installed => "installed",
            Self::RecoveryNeeded => "recovery_needed",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for ObservationState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ObservationState {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "absent" => Ok(Self::Absent),
            "installed" => Ok(Self::Installed),
            "recovery_needed" => Ok(Self::RecoveryNeeded),
            "unknown" => Ok(Self::Unknown),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid ObservationState"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ModificationKind {
    Missing,
    Added,
    TypeChanged,
    ContentChanged,
    ExecutableChanged,
}

impl ModificationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Added => "added",
            Self::TypeChanged => "type_changed",
            Self::ContentChanged => "content_changed",
            Self::ExecutableChanged => "executable_changed",
        }
    }
}

impl fmt::Display for ModificationKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ModificationKind {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "missing" => Ok(Self::Missing),
            "added" => Ok(Self::Added),
            "type_changed" => Ok(Self::TypeChanged),
            "content_changed" => Ok(Self::ContentChanged),
            "executable_changed" => Ok(Self::ExecutableChanged),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid ModificationKind"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Installation {
    bundle_id: String,
    version: String,
    content_digest: String,
    transaction_id: String,
    entries: Vec<InventoryEntry>,
}

impl Installation {
    pub fn new(
        bundle_id: impl Into<String>,
        version: impl Into<String>,
        content_digest: impl Into<String>,
        transaction_id: impl Into<String>,
        entries: impl IntoIterator<Item = InventoryEntry>,
    ) -> Result<Self, ValueError> {
        let bundle_id = bundle_id.into();
        let version = version.into();
        validate_identifier(&bundle_id, "bundle id")?;
        validate_version(&version)?;
        Ok(Self {
            bundle_id,
            version,
            content_digest: content_digest.into(),
            transaction_id: transaction_id.into(),
            entries: entries.into_iter().collect(),
        })
    }

    pub fn bundle_id(&self) -> &str {
        &self.bundle_id
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn content_digest(&self) -> &str {
        &self.content_digest
    }

    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    pub fn entries(&self) -> &[InventoryEntry] {
        &self.entries
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Modification {
    bundle_id: String,
    path: String,
    kind: ModificationKind,
}

impl Modification {
    pub fn new(
        bundle_id: impl Into<String>,
        path: impl Into<String>,
        kind: ModificationKind,
    ) -> Result<Self, ValueError> {
        let bundle_id = bundle_id.into();
        validate_identifier(&bundle_id, "bundle id")?;
        Ok(Self {
            bundle_id,
            path: path.into(),
            kind,
        })
    }

    pub fn bundle_id(&self) -> &str {
        &self.bundle_id
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub const fn kind(&self) -> ModificationKind {
        self.kind
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Conflict {
    path: String,
    owner: Option<String>,
}

impl Conflict {
    pub fn new(path: impl Into<String>, owner: Option<String>) -> Self {
        Self {
            path: path.into(),
            owner,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Observation {
    state: ObservationState,
    installed: Option<Installation>,
    installations: Vec<Installation>,
    version_matches: Option<bool>,
    recorded_content_matches: Option<bool>,
    content_matches: Option<bool>,
    modifications: Vec<Modification>,
    conflicts: Vec<Conflict>,
    error: Option<TargetError>,
    recovery_paths: Vec<PathBuf>,
}

impl Observation {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state: ObservationState,
        installed: Option<Installation>,
        installations: impl IntoIterator<Item = Installation>,
        version_matches: Option<bool>,
        recorded_content_matches: Option<bool>,
        content_matches: Option<bool>,
        modifications: impl IntoIterator<Item = Modification>,
        conflicts: impl IntoIterator<Item = Conflict>,
        error: Option<TargetError>,
        recovery_paths: impl IntoIterator<Item = PathBuf>,
    ) -> Self {
        Self {
            state,
            installed,
            installations: installations.into_iter().collect(),
            version_matches,
            recorded_content_matches,
            content_matches,
            modifications: modifications.into_iter().collect(),
            conflicts: conflicts.into_iter().collect(),
            error,
            recovery_paths: recovery_paths.into_iter().collect(),
        }
    }

    pub(crate) fn unknown(error: TargetError) -> Self {
        Self::new(
            ObservationState::Unknown,
            None,
            [],
            None,
            None,
            None,
            [],
            [],
            Some(error),
            [],
        )
    }

    pub const fn state(&self) -> ObservationState {
        self.state
    }

    pub const fn installed(&self) -> Option<&Installation> {
        self.installed.as_ref()
    }

    pub fn installations(&self) -> &[Installation] {
        &self.installations
    }

    pub const fn version_matches(&self) -> Option<bool> {
        self.version_matches
    }

    pub const fn recorded_content_matches(&self) -> Option<bool> {
        self.recorded_content_matches
    }

    pub const fn content_matches(&self) -> Option<bool> {
        self.content_matches
    }

    pub fn modifications(&self) -> &[Modification] {
        &self.modifications
    }

    pub fn conflicts(&self) -> &[Conflict] {
        &self.conflicts
    }

    pub const fn error(&self) -> Option<&TargetError> {
        self.error.as_ref()
    }

    pub fn recovery_paths(&self) -> &[PathBuf] {
        &self.recovery_paths
    }

    pub fn is_current(&self) -> bool {
        self.state == ObservationState::Installed
            && self.error.is_none()
            && self.installed.is_some()
            && self.version_matches == Some(true)
            && self.recorded_content_matches == Some(true)
            && self.content_matches == Some(true)
            && self.conflicts.is_empty()
            && self.installed.as_ref().is_some_and(|installed| {
                self.modifications
                    .iter()
                    .all(|change| change.bundle_id() != installed.bundle_id())
            })
    }

    pub(crate) fn with_preview(
        mut self,
        state: ObservationState,
        conflicts: Vec<Conflict>,
        error: Option<TargetError>,
    ) -> Self {
        self.state = state;
        self.conflicts = conflicts;
        self.error = error;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetInspection<T> {
    target: T,
    root: PathBuf,
    alias_of: Option<usize>,
    observation: Observation,
}

impl<T> TargetInspection<T> {
    pub const fn new(
        target: T,
        root: PathBuf,
        alias_of: Option<usize>,
        observation: Observation,
    ) -> Self {
        Self {
            target,
            root,
            alias_of,
            observation,
        }
    }

    pub const fn target(&self) -> &T {
        &self.target
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub const fn alias_of(&self) -> Option<usize> {
        self.alias_of
    }

    pub const fn observation(&self) -> &Observation {
        &self.observation
    }

    pub(crate) fn aliased(&self, target: T, alias_of: usize) -> Self
    where
        T: Clone,
    {
        let mut result = self.clone();
        result.target = target;
        result.alias_of = Some(alias_of);
        result
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetResult<T> {
    target: T,
    root: PathBuf,
    alias_of: Option<usize>,
    status: OperationStatus,
    observation: Observation,
    error: Option<TargetError>,
    recovery_paths: Vec<PathBuf>,
}

impl<T> TargetResult<T> {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        target: T,
        root: PathBuf,
        alias_of: Option<usize>,
        status: OperationStatus,
        observation: Observation,
        error: Option<TargetError>,
        recovery_paths: Vec<PathBuf>,
    ) -> Self {
        Self {
            target,
            root,
            alias_of,
            status,
            observation,
            error,
            recovery_paths,
        }
    }

    pub const fn target(&self) -> &T {
        &self.target
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub const fn alias_of(&self) -> Option<usize> {
        self.alias_of
    }

    pub const fn status(&self) -> OperationStatus {
        self.status
    }

    pub const fn observation(&self) -> &Observation {
        &self.observation
    }

    pub const fn error(&self) -> Option<&TargetError> {
        self.error.as_ref()
    }

    pub fn recovery_paths(&self) -> &[PathBuf] {
        &self.recovery_paths
    }

    pub(crate) fn aliased(&self, target: T, alias_of: usize) -> Self
    where
        T: Clone,
    {
        let mut result = self.clone();
        result.target = target;
        result.alias_of = Some(alias_of);
        result
    }
}
