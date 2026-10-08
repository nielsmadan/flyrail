use crate::json::Json;
use crate::models::{BundleIdentity, SkillSpec};
use crate::validation::portable_path_key;
use crate::values::ValueError;
use std::collections::HashSet;

pub fn read_manifest(data: &[u8]) -> Result<(BundleIdentity, Vec<SkillSpec>), ValueError> {
    let text =
        std::str::from_utf8(data).map_err(|_| ValueError::new("manifest is not valid UTF-8"))?;
    let raw = Json::parse(text)?;
    let manifest = object(&raw, &["schema_version", "id", "version", "skills"], &[])?;
    if manifest.get("schema_version").and_then(Json::as_int) != Some(1) {
        return Err(ValueError::new("schema_version must be the integer 1"));
    }
    let identity = BundleIdentity::new(
        string(manifest.get("id"), "bundle id")?,
        string(manifest.get("version"), "version")?,
    )?;
    let records = manifest
        .get("skills")
        .and_then(Json::as_array)
        .filter(|records| !records.is_empty())
        .ok_or_else(|| ValueError::new("skills must be a nonempty JSON array"))?;
    let mut specs = Vec::with_capacity(records.len());
    let mut names: HashSet<String> = HashSet::new();
    let mut paths: HashSet<String> = HashSet::new();
    for record in records {
        let fields = object(record, &["name", "path"], &["executables"])?;
        let spec = SkillSpec::new(
            string(fields.get("name"), "skill name")?,
            string(fields.get("path"), "skill path")?,
            executables(&fields)?,
        )?;
        let key = portable_path_key(spec.path());
        if names.contains(spec.name()) {
            return Err(ValueError::new(format!(
                "duplicate skill name: {}",
                spec.name()
            )));
        }
        if paths.iter().any(|other| overlapping(&key, other)) {
            return Err(ValueError::new("skill source paths must not overlap"));
        }
        names.insert(spec.name().to_owned());
        paths.insert(key);
        specs.push(spec);
    }
    Ok((identity, specs))
}

fn executables(fields: &ManifestObject<'_>) -> Result<Vec<String>, ValueError> {
    let Some(value) = fields.get("executables") else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| ValueError::new("executables must be a JSON array"))?;
    items
        .iter()
        .map(|item| string(Some(item), "executable path"))
        .collect()
}

fn overlapping(key: &str, other: &str) -> bool {
    key == other || key.starts_with(&format!("{other}/")) || other.starts_with(&format!("{key}/"))
}

pub struct ManifestObject<'a> {
    items: &'a [(String, Json)],
}

pub fn object<'a>(
    value: &'a Json,
    required: &[&str],
    optional: &[&str],
) -> Result<ManifestObject<'a>, ValueError> {
    let items = value
        .as_object()
        .ok_or_else(|| ValueError::new("manifest and skill records must be JSON objects"))?;
    let keys: HashSet<&str> = items.iter().map(|(key, _)| key.as_str()).collect();
    let unknown = keys
        .iter()
        .any(|key| !required.contains(key) && !optional.contains(key));
    if unknown || !required.iter().all(|field| keys.contains(field)) {
        return Err(ValueError::new(format!(
            "expected fields {}; optional fields {}",
            field_list(required),
            field_list(optional)
        )));
    }
    Ok(ManifestObject { items })
}

fn field_list(fields: &[&str]) -> String {
    let mut sorted = fields.to_vec();
    sorted.sort_unstable();
    let quoted: Vec<String> = sorted.iter().map(|field| format!("'{field}'")).collect();
    format!("[{}]", quoted.join(", "))
}

impl ManifestObject<'_> {
    pub fn get(&self, key: &str) -> Option<&Json> {
        self.items
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value)
    }
}

pub fn string(value: Option<&Json>, label: &str) -> Result<String, ValueError> {
    value
        .and_then(Json::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ValueError::new(format!("{label} must be a string")))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::read_manifest;

    const VALID: &[u8] = br#"{
        "schema_version": 1,
        "id": "team",
        "version": "1",
        "skills": [{"name": "review", "path": "skills/review", "executables": ["SKILL.md"]}]
    }"#;

    fn message(raw: &[u8]) -> String {
        read_manifest(raw).unwrap_err().message().to_owned()
    }

    #[test]
    fn reads_a_valid_schema_one_manifest() {
        let (identity, specs) = read_manifest(VALID).unwrap();
        assert_eq!(identity.id(), "team");
        assert_eq!(identity.version(), "1");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name(), "review");
        assert_eq!(specs[0].path(), "skills/review");
        assert_eq!(specs[0].executables(), ["SKILL.md"]);
    }

    #[test]
    fn rejects_malformed_documents() {
        let cases: [&[u8]; 8] = [
            b"not json",
            b"[]",
            b"{}",
            b"\xff",
            b"\xef\xbb\xbf{}",
            br#"{"schema_version":NaN}"#,
            br#"{"schema_version":Infinity}"#,
            br#"{"schema_version":-Infinity}"#,
        ];
        for raw in cases {
            assert!(read_manifest(raw).is_err(), "must reject {raw:?}");
        }
    }

    #[test]
    fn rejects_non_schema_one_versions() {
        let cases: [&[u8]; 5] = [
            br#"{"schema_version":true,"id":"team","version":"1","skills":[]}"#,
            br#"{"schema_version":1.0,"id":"team","version":"1","skills":[]}"#,
            br#"{"schema_version":"1","id":"team","version":"1","skills":[]}"#,
            br#"{"schema_version":2,"id":"team","version":"1","skills":[]}"#,
            br#"{"schema_version":null,"id":"team","version":"1","skills":[]}"#,
        ];
        for raw in cases {
            assert_eq!(message(raw), "schema_version must be the integer 1");
        }
    }

    #[test]
    fn rejects_duplicate_json_fields() {
        assert_eq!(
            message(
                br#"{"schema_version":1,"id":"team","id":"team","version":"1","skills":[{"name":"review","path":"review"}]}"#
            ),
            "duplicate JSON field: id"
        );
        assert_eq!(
            message(
                br#"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","name":"review","path":"review"}]}"#
            ),
            "duplicate JSON field: name"
        );
    }

    #[test]
    fn rejects_unknown_or_missing_manifest_fields() {
        assert_eq!(
            message(br#"{"schema_version":1,"id":"team","version":"1","skills":[],"unknown":1}"#),
            "expected fields ['id', 'schema_version', 'skills', 'version']; optional fields []"
        );
        assert_eq!(
            message(br#"{"schema_version":1}"#),
            "expected fields ['id', 'schema_version', 'skills', 'version']; optional fields []"
        );
        assert_eq!(
            message(br#"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"review","extra":null}]}"#),
            "expected fields ['name', 'path']; optional fields ['executables']"
        );
    }

    #[test]
    fn rejects_invalid_skill_records() {
        let cases: [&[u8]; 9] = [
            br#"{"schema_version":1,"id":"team","version":"1","skills":{}}"#,
            br#"{"schema_version":1,"id":"team","version":"1","skills":[]}"#,
            br#"{"schema_version":1,"id":"team","version":"1","skills":[null]}"#,
            br#"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review"}]}"#,
            br#"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"review","executables":"run.py"}]}"#,
            br#"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"review","executables":[7]}]}"#,
            br#"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"review","executables":["x","X"]}]}"#,
            br#"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"review","path":"other"}]}"#,
            br#"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"Bad","path":"review"}]}"#,
        ];
        let expected = [
            "skills must be a nonempty JSON array",
            "skills must be a nonempty JSON array",
            "manifest and skill records must be JSON objects",
            "expected fields ['name', 'path']; optional fields ['executables']",
            "executables must be a JSON array",
            "executable path must be a string",
            "executable paths must be unique across portable filesystems",
            "skill name must match its containing directory",
            "skill name must be 1-64 lowercase ASCII letters/digits with internal hyphens",
        ];
        for (raw, expected) in cases.into_iter().zip(expected) {
            assert_eq!(message(raw), expected);
        }
    }

    #[test]
    fn rejects_invalid_identity_fields() {
        assert_eq!(
            message(br#"{"schema_version":1,"id":1,"version":"1","skills":[]}"#),
            "bundle id must be a string"
        );
        assert_eq!(
            message(br#"{"schema_version":1,"id":"Bad","version":"1","skills":[]}"#),
            "bundle id must be 1-64 lowercase ASCII letters/digits with internal hyphens"
        );
        assert_eq!(
            message(br#"{"schema_version":1,"id":"team","version":1,"skills":[]}"#),
            "version must be a string"
        );
        assert_eq!(
            message(br#"{"schema_version":1,"id":"team","version":" \n\t","skills":[]}"#),
            "version must be nonblank"
        );
    }

    #[test]
    fn rejects_duplicate_and_overlapping_skill_paths() {
        assert_eq!(
            message(
                br#"{"schema_version":1,"id":"x","version":"1","skills":[{"name":"review","path":"one/review"},{"name":"review","path":"two/review"}]}"#
            ),
            "duplicate skill name: review"
        );
        assert_eq!(
            message(
                br#"{"schema_version":1,"id":"x","version":"1","skills":[{"name":"one","path":"one"},{"name":"two","path":"one/two"}]}"#
            ),
            "skill source paths must not overlap"
        );
        assert_eq!(
            message(
                br#"{"schema_version":1,"id":"x","version":"1","skills":[{"name":"two","path":"ONE/two"},{"name":"one","path":"one"}]}"#
            ),
            "skill source paths must not overlap"
        );
    }

    #[test]
    fn keeps_unicode_versions_and_rejects_lone_surrogates() {
        let (identity, _) = read_manifest(
            "{\"schema_version\":1,\"id\":\"team\",\"version\":\"  caf\u{e9} \u{1f680}  \",\"skills\":[{\"name\":\"review\",\"path\":\"review\"}]}".as_bytes(),
        )
        .unwrap();
        assert_eq!(identity.version(), "  caf\u{e9} \u{1f680}  ");
        assert!(
            message(
                br#"{"schema_version":1,"id":"team","version":"\ud800","skills":[{"name":"review","path":"review"}]}"#
            )
            .contains("surrogate")
        );
    }
}
