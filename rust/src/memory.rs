use crate::bundle::BundleError;
use crate::content::snapshot_entries;
use crate::models::{BundleEntry, SkillSpec};
use crate::values::ValueError;

pub struct MemorySource {
    manifest: Vec<u8>,
    entries: Vec<BundleEntry>,
}

impl MemorySource {
    pub fn new(
        manifest: Vec<u8>,
        entries: impl IntoIterator<Item = BundleEntry>,
    ) -> Result<Self, BundleError> {
        Ok(Self {
            manifest,
            entries: snapshot_entries(entries)?,
        })
    }

    pub fn read_manifest(&self) -> Vec<u8> {
        self.manifest.clone()
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, BundleError> {
        self.entries
            .iter()
            .find(|entry| entry.path() == path)
            .and_then(BundleEntry::data)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| {
                BundleError::Invalid(ValueError::new(format!("expected a source file: {path}")))
            })
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
        let is_directory_root = self
            .entries
            .iter()
            .any(|entry| entry.path() == path && entry.is_directory());
        if !is_directory_root {
            return Err(BundleError::Invalid(ValueError::new(format!(
                "skill source must be an existing directory: {path}"
            ))));
        }
        let mut result = vec![BundleEntry::directory(name.to_owned())?];
        let prefix = format!("{path}/");
        for entry in &self.entries {
            if let Some(relative) = entry.path().strip_prefix(&prefix) {
                let executable = executables.iter().any(|item| item == relative);
                result.push(BundleEntry::new(
                    format!("{name}/{relative}"),
                    entry.data().map(<[u8]>::to_vec),
                    executable,
                )?);
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]
mod tests {
    use super::MemorySource;
    use crate::bundle::BundleError;
    use crate::models::BundleEntry;

    const NONE: [String; 0] = [];

    fn paths(entries: &[BundleEntry]) -> Vec<&str> {
        entries.iter().map(BundleEntry::path).collect()
    }

    fn message<T>(result: Result<T, BundleError>) -> String {
        match result {
            Ok(_) => panic!("expected an error"),
            Err(BundleError::Invalid(value)) => value.message().to_owned(),
            Err(BundleError::Io(error)) => panic!("expected invalid input, got {error}"),
        }
    }

    #[test]
    fn reads_files_and_rebases_skill_trees() {
        let source = MemorySource::new(
            b"{}".to_vec(),
            [
                BundleEntry::file("skills/review/SKILL.md", b"markdown".to_vec()).unwrap(),
                BundleEntry::file("skills/review/tools/run.sh", b"#!/bin/sh".to_vec()).unwrap(),
            ],
        )
        .expect("memory source must accept the entries");

        assert_eq!(source.read_manifest(), b"{}");
        assert_eq!(
            source.read_file("skills/review/SKILL.md").unwrap(),
            b"markdown"
        );
        assert_eq!(
            message(source.read_file("skills/review")),
            "expected a source file: skills/review"
        );
        assert_eq!(
            message(source.read_file("absent")),
            "expected a source file: absent"
        );
        let entries = source
            .tree_entries("skills/review", "review", &["tools/run.sh".to_owned()])
            .unwrap();
        assert_eq!(
            paths(&entries),
            [
                "review",
                "review/SKILL.md",
                "review/tools",
                "review/tools/run.sh"
            ]
        );
        assert!(entries[3].executable());
        assert_eq!(
            message(source.tree_entries("skills/review/SKILL.md", "review", &NONE)),
            "skill source must be an existing directory: skills/review/SKILL.md"
        );
    }
}
