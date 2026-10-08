use crate::artifacts::{Artifact, AssetRef, CommandArgument, SkillArtifact, SupportAsset};
use crate::config_manifest::{ConfigSource, read_config_manifest};
use crate::content::TreeContent;
use crate::json::Json;
use crate::manifest::read_manifest;
use crate::memory::MemorySource;
use crate::models::{BundleEntry, BundleIdentity, SkillSpec};
use crate::rendered::{Dependency, DependencyMode, ModelValue, dependency_order};
use crate::sources::{DirectorySource, ZipSource};
use crate::validation::validate_relative_path;
use crate::values::{ArrayValue, ScalarKind, Value, ValueError};
use sha2::{Digest as _, Sha256};
use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum BundleError {
    Invalid(ValueError),
    Io(io::Error),
}

impl From<ValueError> for BundleError {
    fn from(value: ValueError) -> Self {
        Self::Invalid(value)
    }
}

impl From<io::Error> for BundleError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl fmt::Display for BundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(value) => value.fmt(formatter),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl Error for BundleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Invalid(value) => Some(value),
            Self::Io(error) => Some(error),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Bundle {
    identity: BundleIdentity,
    skills: Vec<SkillSpec>,
    entries: Vec<BundleEntry>,
    content_digest: String,
    source_roots: Vec<PathBuf>,
    artifacts: Vec<Artifact>,
    assets: Vec<SupportAsset>,
    dependencies: Vec<Dependency>,
    schema_version: u8,
}

impl Bundle {
    pub fn from_directory(root: impl AsRef<Path>) -> Result<Self, BundleError> {
        let mut source = DirectorySource::new(root.as_ref())?;
        let roots = vec![source.root().to_path_buf()];
        Self::from_source(&mut source, roots)
    }

    pub fn from_zip(path: impl AsRef<Path>, resource: &str) -> Result<Self, BundleError> {
        validate_relative_path(resource, "archive resource")?;
        let origin = path.as_ref();
        let parent = origin
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut source = DirectorySource::new(parent)?;
        source.observe(origin)?;
        let bytes = std::fs::read(origin).map_err(BundleError::Io)?;
        let archive = crate::zip::Archive::open(bytes).map_err(BundleError::Io)?;
        let bundle = Self::from_source(&mut ZipSource::new(archive, resource)?, {
            vec![std::fs::canonicalize(origin).map_err(BundleError::Io)?]
        })?;
        source.finish()?;
        Ok(bundle)
    }

    pub fn from_memory(
        manifest: &[u8],
        entries: impl IntoIterator<Item = BundleEntry>,
    ) -> Result<Self, BundleError> {
        let mut source = MemorySource::new(manifest.to_vec(), entries)?;
        Self::from_source(&mut source, Vec::new())
    }

    pub fn from_memory_value(
        manifest: &Value,
        entries: impl IntoIterator<Item = BundleEntry>,
    ) -> Result<Self, BundleError> {
        let mut encoded = String::new();
        value_to_json(manifest)?.encode_python(&mut encoded);
        Self::from_memory(encoded.as_bytes(), entries)
    }

    #[allow(clippy::too_many_lines)]
    pub fn from_artifacts(
        identity: BundleIdentity,
        artifacts: impl IntoIterator<Item = Artifact>,
        assets: impl IntoIterator<Item = SupportAsset>,
        dependencies: impl IntoIterator<Item = Dependency>,
    ) -> Result<Self, BundleError> {
        let artifacts: Vec<Artifact> = artifacts.into_iter().collect();
        let assets: Vec<SupportAsset> = assets.into_iter().collect();
        let dependencies: Vec<Dependency> = dependencies.into_iter().collect();
        if artifacts.is_empty() {
            return Err(BundleError::Invalid(ValueError::new(
                "a configuration bundle requires at least one artifact",
            )));
        }
        let mut ids = HashSet::with_capacity(artifacts.len() + assets.len());
        for artifact in &artifacts {
            if !ids.insert(artifact.id()) {
                return Err(BundleError::Invalid(ValueError::new(
                    "artifact and asset ids must be unique",
                )));
            }
        }
        for asset in &assets {
            if !ids.insert(asset.id()) {
                return Err(BundleError::Invalid(ValueError::new(
                    "artifact and asset ids must be unique",
                )));
            }
        }
        let mut skill_names = HashSet::new();
        for artifact in &artifacts {
            if let Artifact::Skill(skill) = artifact
                && !skill_names.insert(skill.name())
            {
                return Err(BundleError::Invalid(ValueError::new(
                    "duplicate skill name",
                )));
            }
        }
        let names: Vec<String> = artifacts
            .iter()
            .map(Artifact::id)
            .chain(assets.iter().map(SupportAsset::id))
            .map(str::to_owned)
            .collect();
        dependency_order(&names, &dependencies)?;
        asset_references(&artifacts, &assets, &dependencies)?;
        for asset in &assets {
            if !dependencies
                .iter()
                .any(|edge| edge.required() == asset.id())
            {
                return Err(BundleError::Invalid(ValueError::new(
                    "support assets must have an explicit dependent",
                )));
            }
        }

        let mut ordered = artifacts;
        let mut ordered_assets = assets;
        let mut ordered_dependencies = dependencies;
        ordered.sort_by(|left, right| left.id().cmp(right.id()));
        ordered_assets.sort_by(|left, right| left.id().cmp(right.id()));
        ordered_dependencies.sort_by(|left, right| {
            left.dependent()
                .cmp(right.dependent())
                .then_with(|| left.required().cmp(right.required()))
        });

        let skills: Vec<SkillSpec> = ordered
            .iter()
            .filter_map(|artifact| match artifact {
                Artifact::Skill(skill) => Some(
                    SkillSpec::new(
                        skill.name(),
                        skill.name(),
                        skill
                            .content()
                            .entries()
                            .iter()
                            .filter(|entry| entry.executable())
                            .map(|entry| entry.path().to_owned()),
                    )
                    .map_err(BundleError::from),
                ),
                _ => None,
            })
            .collect::<Result<_, _>>()?;
        let mut entries = Vec::new();
        for artifact in &ordered {
            let Artifact::Skill(skill) = artifact else {
                continue;
            };
            entries.push(BundleEntry::directory(skill.name().to_owned())?);
            for entry in skill.content().entries() {
                entries.push(BundleEntry::new(
                    format!("{}/{}", skill.name(), entry.path()),
                    entry.data().map(<[u8]>::to_vec),
                    entry.executable(),
                )?);
            }
        }
        entries.sort_by(|left, right| left.path().as_bytes().cmp(right.path().as_bytes()));
        let digest = config_digest(&ordered, &ordered_assets, &ordered_dependencies);
        Ok(Self {
            identity,
            skills,
            entries,
            content_digest: digest,
            source_roots: Vec::new(),
            artifacts: ordered,
            assets: ordered_assets,
            dependencies: ordered_dependencies,
            schema_version: 2,
        })
    }

    fn from_source<S: Source>(source: &mut S, roots: Vec<PathBuf>) -> Result<Self, BundleError> {
        let data = source.manifest_bytes()?;
        let raw = Json::parse(
            std::str::from_utf8(&data)
                .map_err(|_| ValueError::new("manifest is not valid UTF-8"))?,
        )?;
        let is_schema_two = match raw.get("schema_version") {
            Some(Json::Int(2)) => true,
            Some(Json::Float(value)) => value.to_bits() == 2.0_f64.to_bits(),
            _ => false,
        };
        if is_schema_two {
            let parsed = read_config_manifest(&raw, source)?;
            let mut bundle = Self::from_artifacts(
                parsed.identity,
                parsed.artifacts,
                parsed.assets,
                parsed.dependencies,
            )?;
            source.finish_loading()?;
            bundle.source_roots = roots;
            return Ok(bundle);
        }
        let (identity, specs) = read_manifest(&data)?;
        let mut entries = Vec::new();
        for spec in &specs {
            entries.extend(source.skill_tree(spec)?);
        }
        entries.sort_by(|left, right| left.path().as_bytes().cmp(right.path().as_bytes()));
        let files: HashSet<&str> = entries
            .iter()
            .filter(|entry| !entry.is_directory())
            .map(BundleEntry::path)
            .collect();
        for spec in &specs {
            let markdown = format!("{}/SKILL.md", spec.name());
            if !files.contains(markdown.as_str()) {
                return Err(BundleError::Invalid(ValueError::new(format!(
                    "skill must contain a SKILL.md file: {}",
                    spec.name()
                ))));
            }
            for executable in spec.executables() {
                let path = format!("{}/{}", spec.name(), executable);
                if !files.contains(path.as_str()) {
                    return Err(BundleError::Invalid(ValueError::new(format!(
                        "every executable must name an existing file: {}",
                        spec.name()
                    ))));
                }
            }
        }
        source.finish_loading()?;
        let artifacts: Vec<Artifact> = specs
            .iter()
            .map(|spec| {
                let prefix = format!("{}/", spec.name());
                let tree_entries = entries
                    .iter()
                    .filter_map(|entry| {
                        entry.path().strip_prefix(&prefix).map(|path| {
                            BundleEntry::new(
                                path,
                                entry.data().map(<[u8]>::to_vec),
                                entry.executable(),
                            )
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Artifact::Skill(SkillArtifact::new(
                    spec.name(),
                    spec.name(),
                    TreeContent::new(tree_entries)?,
                )?))
            })
            .collect::<Result<_, BundleError>>()?;
        let digest = content_digest(&entries);
        Ok(Self {
            identity,
            skills: specs,
            entries,
            content_digest: digest,
            source_roots: roots,
            artifacts,
            assets: Vec::new(),
            dependencies: Vec::new(),
            schema_version: 1,
        })
    }

    pub const fn identity(&self) -> &BundleIdentity {
        &self.identity
    }

    pub fn skills(&self) -> &[SkillSpec] {
        &self.skills
    }

    pub fn entries(&self) -> &[BundleEntry] {
        &self.entries
    }

    pub fn content_digest(&self) -> &str {
        &self.content_digest
    }

    pub fn id(&self) -> &str {
        self.identity.id()
    }

    pub fn version(&self) -> &str {
        self.identity.version()
    }

    pub fn source_roots(&self) -> &[PathBuf] {
        &self.source_roots
    }

    pub fn artifacts(&self) -> &[Artifact] {
        &self.artifacts
    }

    pub fn assets(&self) -> &[SupportAsset] {
        &self.assets
    }

    pub fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }

    pub const fn schema_version(&self) -> u8 {
        self.schema_version
    }
}

trait Source: ConfigSource {
    fn manifest_bytes(&mut self) -> Result<Vec<u8>, BundleError>;
    fn skill_tree(&mut self, spec: &SkillSpec) -> Result<Vec<BundleEntry>, BundleError>;
    fn finish_loading(&mut self) -> Result<(), BundleError> {
        Ok(())
    }
}

impl Source for DirectorySource {
    fn manifest_bytes(&mut self) -> Result<Vec<u8>, BundleError> {
        self.read_manifest()
    }

    fn skill_tree(&mut self, spec: &SkillSpec) -> Result<Vec<BundleEntry>, BundleError> {
        self.skill_entries(spec)
    }

    fn finish_loading(&mut self) -> Result<(), BundleError> {
        self.finish()
    }
}

impl Source for ZipSource {
    fn manifest_bytes(&mut self) -> Result<Vec<u8>, BundleError> {
        self.read_manifest()
    }

    fn skill_tree(&mut self, spec: &SkillSpec) -> Result<Vec<BundleEntry>, BundleError> {
        self.skill_entries(spec)
    }
}

impl Source for MemorySource {
    fn manifest_bytes(&mut self) -> Result<Vec<u8>, BundleError> {
        Ok(self.read_manifest())
    }

    fn skill_tree(&mut self, spec: &SkillSpec) -> Result<Vec<BundleEntry>, BundleError> {
        self.skill_entries(spec)
    }
}

fn asset_references(
    artifacts: &[Artifact],
    assets: &[SupportAsset],
    dependencies: &[Dependency],
) -> Result<(), BundleError> {
    for artifact in artifacts {
        let Some(command) = artifact.command() else {
            continue;
        };
        for (index, argument) in command.argv().iter().enumerate() {
            let CommandArgument::Asset(reference) = argument else {
                continue;
            };
            validate_asset_reference(artifact.id(), reference, assets, dependencies)?;
            if index == 0 {
                let entry = referenced_entry(reference, assets);
                if entry.is_none_or(BundleEntry::is_directory) {
                    return Err(BundleError::Invalid(ValueError::new(
                        "asset command must reference a file",
                    )));
                }
            }
        }
        if let Some(CommandArgument::Asset(reference)) = command.cwd() {
            validate_asset_reference(artifact.id(), reference, assets, dependencies)?;
            if let Some(entry) = referenced_entry(reference, assets)
                && !entry.is_directory()
            {
                return Err(BundleError::Invalid(ValueError::new(
                    "asset cwd must reference a directory",
                )));
            }
        }
    }
    Ok(())
}

fn validate_asset_reference(
    artifact_id: &str,
    reference: &AssetRef,
    assets: &[SupportAsset],
    dependencies: &[Dependency],
) -> Result<(), BundleError> {
    if !assets
        .iter()
        .any(|asset| asset.id() == reference.asset_id())
    {
        return Err(BundleError::Invalid(ValueError::new(
            "asset reference names an unknown support asset",
        )));
    }
    if !dependencies.iter().any(|edge| {
        edge.dependent() == artifact_id
            && edge.required() == reference.asset_id()
            && edge.mode() == DependencyMode::Revision
    }) {
        return Err(BundleError::Invalid(ValueError::new(
            "asset reference requires a direct dependency",
        )));
    }
    if reference.path().is_some() && referenced_entry(reference, assets).is_none() {
        return Err(BundleError::Invalid(ValueError::new(
            "asset reference path does not exist in snapshot",
        )));
    }
    Ok(())
}

fn referenced_entry<'a>(
    reference: &AssetRef,
    assets: &'a [SupportAsset],
) -> Option<&'a BundleEntry> {
    let path = reference.path()?;
    assets
        .iter()
        .find(|asset| asset.id() == reference.asset_id())
        .and_then(|asset| {
            asset
                .content()
                .entries()
                .iter()
                .find(|entry| entry.path() == path)
        })
}

fn config_digest(
    artifacts: &[Artifact],
    assets: &[SupportAsset],
    dependencies: &[Dependency],
) -> String {
    let model = Value::Array(ArrayValue::new([
        Value::Array(ArrayValue::new(
            artifacts.iter().map(ModelValue::model_value),
        )),
        Value::Array(ArrayValue::new(assets.iter().map(ModelValue::model_value))),
        Value::Array(ArrayValue::new(
            dependencies.iter().map(ModelValue::model_value),
        )),
    ]));
    let mut digest = Sha256::new();
    digest.update(b"flyrail-config-v2\0");
    digest.update(model.semantic_bytes());
    let mut encoded = String::with_capacity(64);
    for byte in digest.finalize() {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

pub(crate) fn content_digest(entries: &[BundleEntry]) -> String {
    let mut ordered: Vec<&BundleEntry> = entries.iter().collect();
    ordered.sort_by(|left, right| left.path().as_bytes().cmp(right.path().as_bytes()));
    let mut digest = Sha256::new();
    digest.update(b"flyrail-content-v1\0");
    digest.update(u64::try_from(ordered.len()).unwrap_or(0).to_be_bytes());
    for entry in ordered {
        let path = entry.path().as_bytes();
        digest.update(if entry.is_directory() { b"D" } else { b"F" });
        digest.update(u64::try_from(path.len()).unwrap_or(0).to_be_bytes());
        digest.update(path);
        if let Some(data) = entry.data() {
            digest.update([u8::from(entry.executable())]);
            digest.update(u64::try_from(data.len()).unwrap_or(0).to_be_bytes());
            digest.update(data);
        }
    }
    let mut encoded = String::with_capacity(64);
    for byte in digest.finalize() {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn value_to_json(value: &Value) -> Result<Json, BundleError> {
    match value {
        Value::Scalar(scalar) => {
            let member = match scalar.kind() {
                ScalarKind::Null => Json::Null,
                ScalarKind::Boolean(value) => Json::Bool(*value),
                ScalarKind::Integer(value) => Json::Int(*value),
                ScalarKind::Float(value) => Json::Float(*value),
                ScalarKind::Text(value) => Json::Str(value.clone()),
                ScalarKind::Date(_) | ScalarKind::Time(_) | ScalarKind::DateTime(_) => {
                    return Err(BundleError::Invalid(ValueError::new(
                        "manifest values must be JSON objects, arrays or scalars",
                    )));
                }
            };
            Ok(member)
        }
        Value::Array(array) => array
            .items()
            .iter()
            .map(value_to_json)
            .collect::<Result<Vec<_>, _>>()
            .map(Json::Array),
        Value::Object(object) => {
            let mut members = Vec::with_capacity(object.items().len());
            for (key, item) in object.items() {
                members.push((key.clone(), value_to_json(item)?));
            }
            Ok(Json::Object(members))
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines,
    clippy::unwrap_used
)]
mod tests {
    use super::{Bundle, BundleError, content_digest};
    use crate::artifacts::{
        Artifact, AssetRef, Command, EnvironmentValue, Family, McpArtifact, McpTransport,
        SupportAsset,
    };
    use crate::content::TreeContent;
    use crate::json::Json;
    use crate::models::{BundleEntry, BundleIdentity, SkillSpec};
    use crate::rendered::{Dependency, DependencyMode};
    use crate::temporal::Date;
    use crate::test_support::{
        DEFAULT_MANIFEST, fixture, from_hex, temp_dir, write, write_bundle, write_str,
    };
    use crate::values::{ObjectValue, Scalar, Value};
    use crate::zip::test_archive::{
        directory as archive_directory, file as archive_file, write_archive,
    };
    use sha2::{Digest as _, Sha256};
    use std::path::Path;

    const NONE: [String; 0] = [];

    fn message<T>(result: Result<T, BundleError>) -> String {
        match result {
            Ok(_) => panic!("expected an error"),
            Err(BundleError::Invalid(value)) => value.message().to_owned(),
            Err(BundleError::Io(error)) => panic!("expected invalid input, got {error}"),
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        let mut digest = Sha256::new();
        digest.update(bytes);
        digest
            .finalize()
            .iter()
            .fold(String::new(), |mut out, byte| {
                let _ = write!(out, "{byte:02x}");
                out
            })
    }

    fn text<'a>(case: &'a Json, field: &str) -> &'a str {
        case.get(field)
            .and_then(Json::as_str)
            .expect("fixture field must be a string")
    }

    fn array<'a>(case: &'a Json, field: &str) -> &'a [Json] {
        case.get(field)
            .and_then(Json::as_array)
            .expect("fixture field must be an array")
    }

    fn manifest_text(case: &Json) -> String {
        let mut encoded = String::new();
        case.get("manifest")
            .expect("fixture case must carry a manifest")
            .encode(&mut encoded);
        encoded
    }

    fn write_case(root: &Path, case: &Json) {
        write_str(&root.join("flyrail.json"), &manifest_text(case));
        for entry in array(case, "directories") {
            let path = root.join(entry.as_str().expect("directory must be a string"));
            std::fs::create_dir_all(path).expect("fixture directory must be creatable");
        }
        for entry in array(case, "files") {
            write(
                &root.join(text(entry, "path")),
                &from_hex(text(entry, "data_hex")),
            );
        }
    }

    fn case_archive(case: &Json, resource: &str) -> Vec<u8> {
        let mut specs = vec![archive_file(
            format!("{resource}/flyrail.json"),
            manifest_text(case).into_bytes(),
        )];
        for entry in array(case, "directories") {
            let path = entry.as_str().expect("directory must be a string");
            specs.push(archive_directory(format!("{resource}/{path}")));
        }
        for entry in array(case, "files") {
            specs.push(archive_file(
                format!("{resource}/{}", text(entry, "path")),
                from_hex(text(entry, "data_hex")),
            ));
        }
        write_archive(&specs)
    }

    fn case_entries(case: &Json) -> Vec<BundleEntry> {
        array(case, "entries")
            .iter()
            .map(|entry| {
                let data = entry.get("data_hex").and_then(Json::as_str).map(from_hex);
                let executable = entry
                    .get("executable")
                    .and_then(Json::as_bool)
                    .expect("fixture entry must carry executable intent");
                BundleEntry::new(text(entry, "path"), data, executable)
                    .expect("fixture entry must be valid")
            })
            .collect()
    }

    fn configuration_entries(case: &Json) -> Vec<BundleEntry> {
        array(case, "entries")
            .iter()
            .map(|entry| {
                let data = entry.get("data_hex").and_then(Json::as_str).map(from_hex);
                BundleEntry::new(text(entry, "path"), data, false)
                    .expect("fixture entry must be valid")
            })
            .collect()
    }

    fn write_configuration_case(root: &Path, case: &Json) {
        write_str(&root.join("flyrail.json"), &manifest_text(case));
        for entry in configuration_entries(case) {
            let path = root.join(entry.path());
            if entry.is_directory() {
                std::fs::create_dir_all(path).expect("fixture directory must be creatable");
            } else {
                write(&path, entry.data().expect("fixture file must carry data"));
            }
        }
    }

    fn configuration_archive(case: &Json) -> Vec<u8> {
        let mut members = vec![archive_file(
            "flyrail/flyrail.json",
            manifest_text(case).into_bytes(),
        )];
        for entry in configuration_entries(case) {
            let path = format!("flyrail/{}", entry.path());
            if entry.is_directory() {
                members.push(archive_directory(path));
            } else {
                members.push(archive_file(
                    path,
                    entry.data().expect("fixture file must carry data").to_vec(),
                ));
            }
        }
        write_archive(&members)
    }

    fn memory_entries(case: &Json) -> Vec<BundleEntry> {
        let mut entries: Vec<BundleEntry> = array(case, "directories")
            .iter()
            .map(|entry| {
                BundleEntry::directory(entry.as_str().expect("directory must be a string"))
                    .expect("fixture directory must be valid")
            })
            .collect();
        for entry in array(case, "files") {
            entries.push(
                BundleEntry::file(text(entry, "path"), from_hex(text(entry, "data_hex")))
                    .expect("fixture file must be valid"),
            );
        }
        entries
    }

    #[test]
    fn matches_the_shared_bundle_fixtures() {
        let Some(fixtures) = fixture("bundles.json") else {
            eprintln!("skipping: shared fixtures are unavailable");
            return;
        };
        let cases = array(&fixtures, "cases");
        assert!(!cases.is_empty());
        for case in cases {
            let name = text(case, "name");
            let expected = case_entries(case);
            let digest = text(case, "content_digest");
            let identity = BundleIdentity::new(
                text(case.get("manifest").expect("manifest"), "id"),
                text(case.get("manifest").expect("manifest"), "version"),
            )
            .expect("fixture identity must be valid");
            let parent = temp_dir(name);
            let root = parent.join("flyrail");
            write_case(&root, case);

            let bundle = Bundle::from_directory(&root).expect(name);
            assert_eq!(bundle.entries(), expected, "{name} directory entries");
            assert_eq!(bundle.content_digest(), digest, "{name} directory digest");
            assert_eq!(bundle.identity(), &identity, "{name} identity");
            assert_eq!(
                bundle.source_roots(),
                std::slice::from_ref(&root),
                "{name} source roots"
            );
            assert_eq!(bundle.schema_version(), 1);

            let archive = parent.join("package.zip");
            write(&archive, &case_archive(case, "flyrail"));
            let zipped = Bundle::from_zip(&archive, "flyrail").expect(name);
            assert_eq!(zipped.entries(), expected, "{name} archive entries");
            assert_eq!(zipped.content_digest(), digest, "{name} archive digest");
            assert_eq!(zipped.source_roots(), [archive], "{name} archive root");

            let remembered =
                Bundle::from_memory(manifest_text(case).as_bytes(), memory_entries(case))
                    .expect(name);
            assert_eq!(remembered.entries(), expected, "{name} memory entries");
            assert_eq!(remembered.content_digest(), digest, "{name} memory digest");
            assert!(remembered.source_roots().is_empty());

            if let Some(framing) = case.get("framing_hex").and_then(Json::as_str) {
                assert_eq!(sha256_hex(&from_hex(framing)), digest, "{name} framing");
            }
        }
    }

    #[test]
    fn matches_the_shared_configuration_fixtures() {
        let Some(fixtures) = fixture("configurations.json") else {
            eprintln!("skipping: shared fixtures are unavailable");
            return;
        };
        for case in array(&fixtures, "cases") {
            let name = text(case, "name");
            let manifest = manifest_text(case);
            let expected_digest = text(case, "content_digest");
            let remembered =
                Bundle::from_memory(manifest.as_bytes(), configuration_entries(case)).expect(name);
            let parent = temp_dir(name);
            let root = parent.join("flyrail");
            write_configuration_case(&root, case);
            let directory = Bundle::from_directory(&root).expect(name);
            let archive = parent.join("package.zip");
            write(&archive, &configuration_archive(case));
            let zipped = Bundle::from_zip(&archive, "flyrail").expect(name);

            for bundle in [&remembered, &directory, &zipped] {
                assert_eq!(bundle.schema_version(), 2, "{name} schema");
                assert_eq!(bundle.content_digest(), expected_digest, "{name} digest");
                assert_eq!(
                    bundle.artifacts(),
                    remembered.artifacts(),
                    "{name} artifacts"
                );
                assert_eq!(bundle.assets(), remembered.assets(), "{name} assets");
                assert_eq!(
                    bundle.dependencies(),
                    remembered.dependencies(),
                    "{name} dependencies"
                );
            }
            assert!(remembered.source_roots().is_empty(), "{name} memory roots");
            assert_eq!(directory.source_roots(), [root], "{name} directory roots");
            assert_eq!(zipped.source_roots(), [archive], "{name} archive roots");

            let rebuilt = Bundle::from_artifacts(
                remembered.identity().clone(),
                remembered.artifacts().to_owned(),
                remembered.assets().to_owned(),
                remembered.dependencies().to_owned(),
            )
            .expect(name);
            assert_eq!(
                rebuilt.content_digest(),
                remembered.content_digest(),
                "{name} rebuild"
            );
        }
    }

    #[test]
    fn validates_asset_references_against_dependencies_and_snapshots() {
        let identity = BundleIdentity::new("team", "1").unwrap();
        let asset = SupportAsset::new(
            "support",
            Family::Hooks,
            TreeContent::new([
                BundleEntry::file("run.sh", b"run".to_vec()).unwrap(),
                BundleEntry::directory("tools").unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        let command = |argument: AssetRef, cwd: Option<AssetRef>| {
            Command::new(
                [argument],
                Vec::<(String, EnvironmentValue)>::new(),
                cwd.map(Into::into),
            )
            .unwrap()
        };
        let server = |command| {
            Artifact::Mcp(
                McpArtifact::new("server", "server", McpTransport::Command(command)).unwrap(),
            )
        };
        let dependency = Dependency::new("server", "support").unwrap();
        let bundle = Bundle::from_artifacts(
            identity.clone(),
            [server(command(
                AssetRef::with_path("support", "run.sh").unwrap(),
                None,
            ))],
            [asset.clone()],
            [dependency.clone()],
        )
        .unwrap();
        assert_eq!(bundle.schema_version(), 2);

        assert_eq!(
            message(Bundle::from_artifacts(
                identity.clone(),
                [server(command(AssetRef::new("unknown").unwrap(), None))],
                [asset.clone()],
                [dependency.clone()],
            )),
            "asset reference names an unknown support asset"
        );
        assert_eq!(
            message(Bundle::from_artifacts(
                identity.clone(),
                [server(command(
                    AssetRef::with_path("support", "missing.sh").unwrap(),
                    None,
                ))],
                [asset.clone()],
                [dependency.clone()],
            )),
            "asset reference path does not exist in snapshot"
        );
        assert_eq!(
            message(Bundle::from_artifacts(
                identity.clone(),
                [server(command(
                    AssetRef::with_path("support", "run.sh").unwrap(),
                    None,
                ))],
                [asset.clone()],
                [
                    Dependency::with_mode("server", "support", DependencyMode::StableReference,)
                        .unwrap()
                ],
            )),
            "asset reference requires a direct dependency"
        );
        assert_eq!(
            message(Bundle::from_artifacts(
                identity.clone(),
                [server(command(
                    AssetRef::with_path("support", "run.sh").unwrap(),
                    Some(AssetRef::with_path("support", "run.sh").unwrap()),
                ))],
                [asset.clone()],
                [dependency.clone()],
            )),
            "asset cwd must reference a directory"
        );
        assert_eq!(
            message(Bundle::from_artifacts(
                identity,
                [server(command(
                    AssetRef::with_path("support", "tools").unwrap(),
                    None,
                ))],
                [asset],
                [dependency],
            )),
            "asset command must reference a file"
        );
    }

    #[test]
    fn digest_is_independent_of_entry_order_and_covers_every_field() {
        let entries = [
            BundleEntry::directory("review").unwrap(),
            BundleEntry::file("review/SKILL.md", b"markdown".to_vec()).unwrap(),
            BundleEntry::directory("review/empty").unwrap(),
        ];
        let reversed: Vec<BundleEntry> = entries.iter().rev().cloned().collect();
        assert_eq!(content_digest(&entries), content_digest(&reversed));
        for changed in [
            vec![entries[0].clone(), entries[1].clone()],
            vec![
                entries[0].clone(),
                BundleEntry::executable_file("review/SKILL.md", b"markdown".to_vec()).unwrap(),
                entries[2].clone(),
            ],
            vec![
                entries[0].clone(),
                BundleEntry::file("review/SKILL.md", b"markdow".to_vec()).unwrap(),
                entries[2].clone(),
            ],
            vec![
                entries[0].clone(),
                entries[1].clone(),
                BundleEntry::file("review/empty", Vec::new()).unwrap(),
            ],
        ] {
            assert_ne!(content_digest(&entries), content_digest(&changed));
        }
    }

    #[test]
    fn snapshots_content_and_keeps_it_after_the_source_changes() {
        let root = temp_dir("bundle-snapshot");
        let skill = write_bundle(&root, DEFAULT_MANIFEST);
        std::fs::create_dir(skill.join("empty")).expect("dir must be creatable");
        let bundle = Bundle::from_directory(&root).expect("bundle must load");
        let original = std::fs::read(skill.join("SKILL.md")).expect("skill must be readable");
        write(&skill.join("SKILL.md"), b"changed");
        std::fs::remove_dir(skill.join("empty")).expect("dir must be removable");
        write(&skill.join("later.bin"), b"later");
        write_str(&root.join("flyrail.json"), "{}");

        assert_eq!(bundle.id(), "team");
        assert_eq!(bundle.version(), "1");
        assert_eq!(
            bundle.skills(),
            [SkillSpec::new("review", "skills/review", NONE).unwrap()]
        );
        assert_eq!(
            bundle.entries(),
            [
                BundleEntry::directory("review").unwrap(),
                BundleEntry::file("review/SKILL.md", original).unwrap(),
                BundleEntry::directory("review/empty").unwrap(),
            ]
        );
        assert_eq!(content_digest(bundle.entries()), bundle.content_digest());
    }

    #[test]
    fn separates_identity_from_content() {
        let root = temp_dir("bundle-identity");
        let skill = write_bundle(&root, DEFAULT_MANIFEST);
        let first = Bundle::from_directory(&root).expect("bundle must load");
        std::fs::create_dir(root.join("other")).expect("dir must be creatable");
        std::fs::rename(&skill, root.join("other/review")).expect("rename must work");
        std::fs::remove_dir(root.join("skills")).expect("dir must be removable");
        write_str(
            &root.join("flyrail.json"),
            concat!(
                r#"{"schema_version":1,"id":"renamed","version":"  older-looking  ","#,
                r#""skills":[{"name":"review","path":"other/review"}]}"#
            ),
        );
        let second = Bundle::from_directory(&root).expect("bundle must load");

        assert_eq!(second.id(), "renamed");
        assert_eq!(second.version(), "  older-looking  ");
        assert_eq!(second.entries(), first.entries());
        assert_eq!(second.content_digest(), first.content_digest());
        write(&root.join("other/review/SKILL.md"), b"new content");
        let third = Bundle::from_directory(&root).expect("bundle must load");
        assert_eq!(third.version(), second.version());
        assert_ne!(third.content_digest(), second.content_digest());
    }

    #[test]
    fn tracks_empty_directories_and_declared_executable_intent() {
        let root = temp_dir("bundle-intent");
        let skill = write_bundle(&root, DEFAULT_MANIFEST);
        let initial = Bundle::from_directory(&root).expect("bundle must load");
        std::fs::create_dir(skill.join("empty")).expect("dir must be creatable");
        let with_directory = Bundle::from_directory(&root).expect("bundle must load");
        assert_ne!(with_directory.content_digest(), initial.content_digest());
        write_str(
            &root.join("flyrail.json"),
            concat!(
                r#"{"schema_version":1,"id":"team","version":"1","skills":"#,
                r#"[{"name":"review","path":"skills/review","executables":["SKILL.md"]}]}"#
            ),
        );
        let executable = Bundle::from_directory(&root).expect("bundle must load");
        assert_ne!(executable.content_digest(), with_directory.content_digest());
        assert!(executable.entries()[1].executable());

        let permissions = std::os::unix::fs::PermissionsExt::from_mode(0o700);
        std::fs::set_permissions(skill.join("SKILL.md"), permissions)
            .expect("permissions must be settable");
        let after_chmod = Bundle::from_directory(&root).expect("bundle must load");
        assert_eq!(after_chmod.content_digest(), executable.content_digest());
    }

    #[test]
    fn ignores_physical_executable_bits_without_declared_intent() {
        let root = temp_dir("bundle-permissions");
        let skill = write_bundle(&root, DEFAULT_MANIFEST);
        let initial = Bundle::from_directory(&root).expect("bundle must load");
        let permissions = std::os::unix::fs::PermissionsExt::from_mode(0o700);
        std::fs::set_permissions(skill.join("SKILL.md"), permissions)
            .expect("permissions must be settable");
        let after_chmod = Bundle::from_directory(&root).expect("bundle must load");

        assert!(!after_chmod.entries()[1].executable());
        assert_eq!(after_chmod.content_digest(), initial.content_digest());
    }

    #[test]
    fn ignores_manifest_and_executable_order() {
        let root = temp_dir("bundle-order");
        write_str(
            &root.join("flyrail.json"),
            concat!(
                r#"{"schema_version":1,"id":"team","version":"1","skills":[{"name":"notes","#,
                r#""path":"notes"},{"name":"review","path":"skills/review","#,
                r#""executables":["tools/run.sh","SKILL.md"]}]}"#
            ),
        );
        let skill = root.join("skills/review");
        write(&skill.join("SKILL.md"), b"markdown");
        write(&skill.join("tools/run.sh"), b"#!/bin/sh\n");
        write(&root.join("notes/SKILL.md"), b"# Notes\n");
        let initial = Bundle::from_directory(&root).expect("bundle must load");
        write_str(
            &root.join("flyrail.json"),
            concat!(
                r#"{  "id"  :  "team" , "schema_version" : 1 , "skills" : ["#,
                r#"{"executables":["SKILL.md","tools/run.sh"],"name":"review","#,
                r#""path":"skills/review"},{"name":"notes","path":"notes"}],"#,
                r#""version":"1"}"#
            ),
        );
        let reordered = Bundle::from_directory(&root).expect("bundle must load");

        assert_eq!(
            reordered
                .skills()
                .iter()
                .map(SkillSpec::name)
                .collect::<Vec<_>>(),
            ["review", "notes"]
        );
        assert_eq!(
            reordered.skills()[0].executables(),
            ["SKILL.md", "tools/run.sh"]
        );
        assert_eq!(reordered.content_digest(), initial.content_digest());
        let notes: Vec<BundleEntry> = initial
            .entries()
            .iter()
            .filter(|entry| entry.path().split('/').next() == Some("notes"))
            .cloned()
            .collect();
        let only_notes = temp_dir("bundle-order-notes");
        write_str(
            &only_notes.join("flyrail.json"),
            concat!(
                r#"{"schema_version":1,"id":"notes","version":"1","#,
                r#""skills":[{"name":"notes","path":"notes"}]}"#
            ),
        );
        write(&only_notes.join("notes/SKILL.md"), b"# Notes\n");

        assert_eq!(
            content_digest(&notes),
            Bundle::from_directory(&only_notes)
                .expect("bundle must load")
                .content_digest()
        );
    }

    #[test]
    fn requires_a_skill_markdown_file_and_existing_executables() {
        let root = temp_dir("bundle-required");
        let skill = write_bundle(&root, DEFAULT_MANIFEST);
        std::fs::remove_file(skill.join("SKILL.md")).expect("file must be removable");
        assert_eq!(
            message(Bundle::from_directory(&root)),
            "skill must contain a SKILL.md file: review"
        );
        write(&skill.join("skill.md"), b"markdown");
        assert_eq!(
            message(Bundle::from_directory(&root)),
            "skill must contain a SKILL.md file: review"
        );
        std::fs::remove_file(skill.join("skill.md")).expect("file must be removable");
        std::fs::create_dir(skill.join("SKILL.md")).expect("dir must be creatable");
        assert_eq!(
            message(Bundle::from_directory(&root)),
            "skill must contain a SKILL.md file: review"
        );
        std::fs::remove_dir(skill.join("SKILL.md")).expect("dir must be removable");
        write(&skill.join("SKILL.md"), b"markdown");
        std::fs::create_dir(skill.join("empty")).expect("dir must be creatable");
        for target in ["missing.py", "empty", "skill.md"] {
            write_str(
                &root.join("flyrail.json"),
                &format!(
                    concat!(
                        r#"{{"schema_version":1,"id":"team","version":"1","skills":"#,
                        r#"[{{"name":"review","path":"skills/review","executables":["{}"]}}]}}"#
                    ),
                    target
                ),
            );
            assert_eq!(
                message(Bundle::from_directory(&root)),
                "every executable must name an existing file: review",
                "{target}"
            );
        }
    }

    #[test]
    fn rejects_missing_and_wrong_source_types() {
        let root = temp_dir("bundle-kinds");
        match Bundle::from_directory(root.join("absent")) {
            Err(BundleError::Io(error)) => {
                assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected a missing-root I/O error, got {other:?}"),
        }
        assert_eq!(
            message(Bundle::from_directory(&root)),
            "missing, case-mismatched or ambiguous source path: flyrail.json"
        );
        std::fs::create_dir(root.join("flyrail.json")).expect("dir must be creatable");
        assert_eq!(
            message(Bundle::from_directory(&root)),
            format!(
                "expected a source file: {}",
                root.join("flyrail.json").display()
            )
        );
        std::fs::remove_dir(root.join("flyrail.json")).expect("dir must be removable");
        let skill = write_bundle(&root, DEFAULT_MANIFEST);
        assert_eq!(
            message(Bundle::from_directory(root.join("flyrail.json"))),
            "bundle root must be a directory"
        );
        std::fs::remove_file(skill.join("SKILL.md")).expect("file must be removable");
        std::fs::remove_dir(&skill).expect("dir must be removable");
        write(&skill, b"not a directory");
        assert_eq!(
            message(Bundle::from_directory(&root)),
            "skill source must be a directory: skills/review"
        );
    }

    #[test]
    fn loads_bundles_from_memory_manifests_and_values() {
        let manifest = concat!(
            r#"{"schema_version": 1, "id": "team", "version": "1", "#,
            r#""skills": [{"name": "review", "path": "review"}]}"#
        );
        let entries = [BundleEntry::file("review/SKILL.md", b"markdown".to_vec()).unwrap()];
        let expected =
            Bundle::from_memory(manifest.as_bytes(), entries.clone()).expect("bundle must load");
        assert_eq!(
            expected.entries(),
            [
                BundleEntry::directory("review").unwrap(),
                BundleEntry::file("review/SKILL.md", b"markdown".to_vec()).unwrap(),
            ]
        );

        let record = ObjectValue::new([
            ("schema_version", Value::Scalar(Scalar::integer(1))),
            ("id", Value::from("team")),
            ("version", Value::from("1")),
            (
                "skills",
                Value::Array(crate::values::ArrayValue::new([Value::Object(
                    ObjectValue::new([
                        ("name", Value::from("review")),
                        ("path", Value::from("review")),
                    ])
                    .unwrap(),
                )])),
            ),
        ])
        .unwrap();
        let from_value =
            Bundle::from_memory_value(&Value::Object(record), entries).expect("bundle must load");
        assert_eq!(from_value.content_digest(), expected.content_digest());
        assert_eq!(from_value.identity(), expected.identity());

        let dated = ObjectValue::new([(
            "schema_version",
            Value::Scalar(Scalar::date(Date::new(2024, 1, 1).unwrap())),
        )])
        .unwrap();
        assert_eq!(
            message(Bundle::from_memory_value(
                &Value::Object(dated),
                NONE.iter().map(|_| unreachable!())
            )),
            "manifest values must be JSON objects, arrays or scalars"
        );
        assert_eq!(
            message(Bundle::from_memory(b"{}", [])),
            "expected fields ['id', 'schema_version', 'skills', 'version']; optional fields []"
        );
        assert_eq!(
            message(Bundle::from_memory(manifest.as_bytes(), [])),
            "skill source must be an existing directory: review"
        );
    }

    #[test]
    fn rejects_nonportable_zip_resources() {
        let root = temp_dir("bundle-resource");
        let archive = root.join("package.zip");
        write(
            &archive,
            &write_archive(&[archive_file("flyrail/x", Vec::new())]),
        );
        for resource in ["", ".", "../flyrail", "/flyrail", "data\\flyrail", "CON"] {
            assert!(
                message(Bundle::from_zip(&archive, resource)).starts_with("archive resource "),
                "{resource}"
            );
        }
    }
}
