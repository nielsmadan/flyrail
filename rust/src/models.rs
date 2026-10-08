use crate::validation::{
    portable_path_key, validate_identifier, validate_relative_path, validate_version,
};
use crate::values::ValueError;
use std::collections::HashSet;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BundleIdentity {
    id: String,
    version: String,
}

impl BundleIdentity {
    pub fn new(id: impl Into<String>, version: impl Into<String>) -> Result<Self, ValueError> {
        let id = id.into();
        let version = version.into();
        validate_identifier(&id, "bundle id")?;
        validate_version(&version)?;
        Ok(Self { id, version })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SkillSpec {
    name: String,
    path: String,
    executables: Vec<String>,
}

impl SkillSpec {
    pub fn new(
        name: impl Into<String>,
        path: impl Into<String>,
        executables: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, ValueError> {
        let name = name.into();
        let path = path.into();
        validate_identifier(&name, "skill name")?;
        validate_relative_path(&path, "skill path")?;
        if path.rsplit('/').next() != Some(name.as_str()) {
            return Err(ValueError::new(
                "skill name must match its containing directory",
            ));
        }
        let executables: Vec<String> = executables.into_iter().map(Into::into).collect();
        let mut seen: HashSet<String> = HashSet::with_capacity(executables.len());
        for executable in &executables {
            validate_relative_path(executable, "executable path")?;
            if !seen.insert(portable_path_key(executable)) {
                return Err(ValueError::new(
                    "executable paths must be unique across portable filesystems",
                ));
            }
        }
        Ok(Self {
            name,
            path,
            executables,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn executables(&self) -> &[String] {
        &self.executables
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BundleEntry {
    path: String,
    data: Option<Vec<u8>>,
    executable: bool,
}

impl BundleEntry {
    pub fn new(
        path: impl Into<String>,
        data: Option<Vec<u8>>,
        executable: bool,
    ) -> Result<Self, ValueError> {
        let path = path.into();
        validate_relative_path(&path, "entry path")?;
        if data.is_none() && executable {
            return Err(ValueError::new(
                "a directory cannot have executable file intent",
            ));
        }
        Ok(Self {
            path,
            data,
            executable,
        })
    }

    pub fn directory(path: impl Into<String>) -> Result<Self, ValueError> {
        Self::new(path, None, false)
    }

    pub fn file(path: impl Into<String>, data: impl Into<Vec<u8>>) -> Result<Self, ValueError> {
        Self::new(path, Some(data.into()), false)
    }

    pub fn executable_file(
        path: impl Into<String>,
        data: impl Into<Vec<u8>>,
    ) -> Result<Self, ValueError> {
        Self::new(path, Some(data.into()), true)
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn data(&self) -> Option<&[u8]> {
        self.data.as_deref()
    }

    pub const fn executable(&self) -> bool {
        self.executable
    }

    pub const fn is_directory(&self) -> bool {
        self.data.is_none()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OperationStatus {
    Applied,
    Unchanged,
    Failed,
    Partial,
    Incomplete,
}

impl OperationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Unchanged => "unchanged",
            Self::Failed => "failed",
            Self::Partial => "partial",
            Self::Incomplete => "incomplete",
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::{BundleEntry, BundleIdentity, SkillSpec};

    #[test]
    fn identities_validate_their_parts() {
        let identity = BundleIdentity::new("team", "  older-looking  ").unwrap();
        assert_eq!(identity.id(), "team");
        assert_eq!(identity.version(), "  older-looking  ");
        assert!(BundleIdentity::new("Bad", "1").is_err());
        assert!(BundleIdentity::new("team", " \n\t").is_err());
    }

    #[test]
    fn skill_names_must_match_their_directory() {
        let spec = SkillSpec::new("review", "skills/review", ["run.py"]).unwrap();
        assert_eq!(spec.path(), "skills/review");
        assert_eq!(spec.executables(), ["run.py"]);
        assert!(SkillSpec::new("review", "skills/other", Vec::<String>::new()).is_err());
        assert!(SkillSpec::new("review", "review/", Vec::<String>::new()).is_err());
    }

    #[test]
    fn executables_must_be_portably_distinct() {
        assert!(SkillSpec::new("review", "review", ["x", "X"]).is_err());
        assert!(SkillSpec::new("review", "review", ["x", "y"]).is_ok());
    }

    #[test]
    fn directories_carry_no_data_or_executable_intent() {
        let directory = BundleEntry::directory("review").unwrap();
        assert!(directory.is_directory());
        assert!(!directory.executable());
        assert_eq!(directory.data(), None);
        let file = BundleEntry::file("review/SKILL.md", b"x".to_vec()).unwrap();
        assert!(!file.is_directory());
        assert!(!file.executable());
        assert_eq!(file.data(), Some(b"x".as_slice()));
        assert!(
            BundleEntry::executable_file("review/SKILL.md", b"x".to_vec())
                .unwrap()
                .executable()
        );
        assert_eq!(
            BundleEntry::new("review", None, true)
                .unwrap_err()
                .message(),
            "a directory cannot have executable file intent"
        );
        assert!(BundleEntry::directory("../escape").is_err());
    }
}
