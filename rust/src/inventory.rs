use crate::models::BundleEntry;
use crate::validation::validate_relative_path;
use crate::values::ValueError;
use sha2::{Digest as _, Sha256};
use std::fmt::Write as _;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InventoryEntry {
    path: String,
    size: Option<u64>,
    sha256: Option<String>,
    executable: bool,
}

impl InventoryEntry {
    pub fn new(
        path: impl Into<String>,
        size: Option<u64>,
        sha256: Option<String>,
        executable: bool,
    ) -> Result<Self, ValueError> {
        let path = path.into();
        validate_relative_path(&path, "inventory path")?;
        Ok(Self {
            path,
            size,
            sha256,
            executable,
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub const fn size(&self) -> Option<u64> {
        self.size
    }

    pub fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }

    pub const fn executable(&self) -> bool {
        self.executable
    }

    pub const fn is_directory(&self) -> bool {
        self.size.is_none()
    }
}

pub fn inventory(
    entries: impl IntoIterator<Item = BundleEntry>,
) -> Result<Vec<InventoryEntry>, ValueError> {
    let mut entries = entries.into_iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.path().as_bytes().cmp(right.path().as_bytes()));
    entries
        .into_iter()
        .map(|entry| {
            let (size, sha256) = match entry.data() {
                None => (None, None),
                Some(data) => {
                    let size = u64::try_from(data.len()).map_err(|_| {
                        ValueError::new("inventory metadata must be immutable scalar values")
                    })?;
                    let digest = Sha256::digest(data);
                    let sha256 = digest
                        .iter()
                        .fold(String::with_capacity(64), |mut text, byte| {
                            let _ = write!(text, "{byte:02x}");
                            text
                        });
                    (Some(size), Some(sha256))
                }
            };
            InventoryEntry::new(entry.path(), size, sha256, entry.executable())
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn inventory_sorts_paths_and_hashes_file_bytes() {
        let values = inventory([
            BundleEntry::file("z", b"last".to_vec()).unwrap(),
            BundleEntry::directory("a").unwrap(),
            BundleEntry::executable_file("a/run", b"run".to_vec()).unwrap(),
        ])
        .unwrap();
        assert_eq!(
            values.iter().map(InventoryEntry::path).collect::<Vec<_>>(),
            ["a", "a/run", "z"]
        );
        assert!(values[0].is_directory());
        assert_eq!(values[1].size(), Some(3));
        assert_eq!(
            values[1].sha256(),
            Some("acba25512100f80b56fc3ccd14c65be55d94800cda77585c5f41a887e398f9be")
        );
        assert!(values[1].executable());
    }

    #[test]
    fn inventory_paths_are_strictly_validated() {
        assert_eq!(
            InventoryEntry::new("../escape", None, None, false)
                .unwrap_err()
                .message(),
            "inventory path must be an unambiguous relative path"
        );
    }
}
