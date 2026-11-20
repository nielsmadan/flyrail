use super::{LoadoutDiagnostic, LoadoutHarness, LoadoutSelector, LoadoutSource};
use crate::authority::sha256_hex;
use crate::editors::DocumentBackend as _;
use crate::observation::failure;
use crate::observations::{ErrorCode, TargetError};
use crate::resource_io::{ancestors, read_file};
use crate::targets::Agent;
use crate::toml_document::TomlDocument;
use crate::toml_layout::header_path;
use crate::validation::{portable_path_key, validate_relative_path};
use crate::{ObjectValue, Value, ValueError};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

pub(super) type Table = BTreeMap<String, Value>;
type Origin = BTreeMap<Vec<String>, PathBuf>;
const MAX_INPUT_BYTES: u64 = 8 * 1024 * 1024;
pub(super) const CATEGORIES: &[&str] = &[
    "agents",
    "defaults",
    "hooks",
    "instructions",
    "kits",
    "mcp",
    "module-config",
    "permissions",
    "plugins",
    "settings",
    "skills",
];
const AUTOMATIC: &[&str] = &[
    "agents",
    "mcp",
    "mcp-permissions",
    "module-config",
    "permissions",
    "skills",
];
const NAMED: &[&str] = &["instructions", "permissions", "skills"];
const EXTRA: &[&str] = &["preserve", "settings", "substitute"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadPrecondition {
    path: PathBuf,
    resolved: PathBuf,
    identity: Option<(u64, u64, u32, u32, u32)>,
    digest: Option<String>,
}

impl ReadPrecondition {
    pub(crate) fn physical(&self) -> &Path {
        &self.resolved
    }

    pub(crate) fn require_binding(&self) -> Result<(), TargetError> {
        if resolved(&self.path).is_ok_and(|path| path == self.resolved) {
            Ok(())
        } else {
            Err(failure(
                ErrorCode::ConcurrentChange,
                "Loadout routing input changed",
                &self.path,
            ))
        }
    }

    pub fn require(&self) -> Result<(), TargetError> {
        if capture(&self.path, Some(&self.resolved)).is_ok_and(|(current, _)| current == *self) {
            Ok(())
        } else {
            Err(failure(
                ErrorCode::ConcurrentChange,
                "Loadout routing input changed",
                &self.path,
            ))
        }
    }
}

pub(super) fn resolved(path: &Path) -> std::io::Result<PathBuf> {
    let mut tail = Vec::new();
    let mut current = path;
    loop {
        match fs::symlink_metadata(current) {
            Ok(_) => {
                let mut root = current.canonicalize()?;
                root.extend(tail.into_iter().rev());
                return Ok(root);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let (Some(parent), Some(name)) = (current.parent(), current.file_name()) else {
                    return Err(error);
                };
                tail.push(name);
                current = parent;
            }
            Err(error) => return Err(error),
        }
    }
}

fn capture(
    path: &Path,
    expected: Option<&Path>,
) -> Result<(ReadPrecondition, Option<Vec<u8>>), TargetError> {
    let resolve =
        || resolved(path).map_err(|error| failure(ErrorCode::IoError, error.to_string(), path));
    let physical = resolve()?;
    if expected.is_some_and(|expected| expected != physical) {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "Loadout input path changed",
            path,
        ));
    }
    let parents = ancestors(&physical)?;
    let metadata = || match fs::symlink_metadata(&physical) {
        Ok(info) => Ok(Some(info)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failure(ErrorCode::IoError, error.to_string(), path)),
    };
    let before = metadata()?;
    let stamp = |info: &fs::Metadata| (info.dev(), info.ino(), info.mode(), info.uid(), info.gid());
    let identity = before.as_ref().map(stamp);
    let data = match before {
        Some(info) if info.is_dir() => None,
        Some(info) if info.is_file() && info.len() <= MAX_INPUT_BYTES && info.nlink() == 1 => {
            Some(read_file(&physical, &info, Some(MAX_INPUT_BYTES))?)
        }
        None => None,
        Some(_) => {
            return Err(failure(
                ErrorCode::UnsafePath,
                "Expected an ordinary Loadout input within the size limit",
                path,
            ));
        }
    };
    if metadata()?.as_ref().map(stamp) != identity
        || resolve()? != physical
        || ancestors(&physical)? != parents
    {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "Loadout input path changed",
            path,
        ));
    }
    Ok((
        ReadPrecondition {
            path: path.to_owned(),
            resolved: physical,
            identity,
            digest: data
                .as_deref()
                .map(|bytes| sha256_hex(&Sha256::digest(bytes))),
        },
        data,
    ))
}

#[derive(Default)]
pub(super) struct Inputs(BTreeMap<PathBuf, ReadPrecondition>);

impl Inputs {
    pub(super) fn conditions(self) -> Vec<ReadPrecondition> {
        self.0.into_values().collect()
    }

    fn read(
        &mut self,
        path: &Path,
        code: &'static str,
        within: Option<&Path>,
    ) -> Result<(ReadPrecondition, Option<Vec<u8>>), LoadoutDiagnostic> {
        let physical =
            resolved(path).map_err(|error| LoadoutDiagnostic::at(code, error.to_string(), path))?;
        if within.is_some_and(|root| !physical.starts_with(root)) {
            return Err(LoadoutDiagnostic::at(
                "escaped-path",
                "Loadout input escapes its declared source",
                path,
            ));
        }
        let (current, data) = capture(path, Some(&physical))
            .map_err(|error| LoadoutDiagnostic::at(code, error.message(), path))?;
        if self
            .0
            .insert(path.to_owned(), current.clone())
            .is_some_and(|previous| previous != current)
        {
            return Err(LoadoutDiagnostic::at(
                "concurrent-change",
                "Loadout input changed during preview",
                path,
            ));
        }
        Ok((current, data))
    }

    pub(super) fn file(
        &mut self,
        path: &Path,
        code: &'static str,
        within: Option<&Path>,
    ) -> Result<Option<Vec<u8>>, LoadoutDiagnostic> {
        let (current, data) = self.read(path, code, within)?;
        if current.identity.is_some() && data.is_none() {
            return Err(LoadoutDiagnostic::at(
                code,
                "Expected a regular Loadout input file",
                path,
            ));
        }
        Ok(data)
    }

    pub(super) fn table(
        &mut self,
        path: &Path,
        code: &'static str,
        within: Option<&Path>,
    ) -> Result<Option<Table>, LoadoutDiagnostic> {
        self.file(path, code, within)?
            .map(|data| {
                let text = std::str::from_utf8(&data)
                    .map_err(|error| LoadoutDiagnostic::at(code, error.to_string(), path))?;
                let value = TomlDocument::parse(text)
                    .and_then(|document| document.native(&[]))
                    .map_err(|error| LoadoutDiagnostic::at(code, error.message(), path))?;
                table(Some(&value), path, "Loadout TOML")
            })
            .transpose()
    }

    pub(super) fn directory(&mut self, path: &Path) -> Result<PathBuf, LoadoutDiagnostic> {
        let (current, data) = self.read(path, "invalid-source", None)?;
        if current.identity.is_none() || data.is_some() {
            return Err(LoadoutDiagnostic::at(
                "invalid-source",
                "Expected an existing Loadout source directory",
                path,
            ));
        }
        Ok(current.resolved)
    }
}

pub(super) fn absolute(path: &Path) -> Result<(), ValueError> {
    if !path.is_absolute() || path.to_str().is_none_or(|value| value.contains('\0')) {
        return Err(ValueError::new(
            "Loadout routes require absolute UTF-8 paths without NUL",
        ));
    }
    Ok(())
}

pub(super) fn source_path(
    value: &str,
    home: &Path,
    relative: Option<&Path>,
) -> Result<PathBuf, ValueError> {
    if value == "~" {
        return Ok(home.to_owned());
    }
    if let Some(suffix) = value.strip_prefix("~/") {
        return Ok(home.join(suffix.trim_start_matches('/')));
    }
    if value.starts_with('~') || value.contains('\0') {
        return Err(ValueError::new("Use ~/ for the explicit home directory"));
    }
    let path = Path::new(value);
    if let Some(parent) = relative.filter(|_| !path.is_absolute()) {
        return Ok(parent.join(path));
    }
    absolute(path)?;
    Ok(path.to_owned())
}

pub(super) fn profile_file(root: &Path, profile: &str) -> Result<PathBuf, LoadoutDiagnostic> {
    if profile.is_empty() || matches!(profile, "." | "..") || profile.contains(['/', '\\', '\0']) {
        return Err(LoadoutDiagnostic::at(
            "invalid-profile",
            "Profile must name a sibling manifest",
            root,
        ));
    }
    Ok(root.join(if profile == "default" {
        "loadout.toml".into()
    } else {
        format!("{profile}.toml")
    }))
}

pub(super) fn text(value: Option<&Value>) -> Option<&str> {
    match value? {
        Value::Scalar(scalar) => scalar.as_text(),
        _ => None,
    }
}

pub(super) fn strings(
    value: Option<&Value>,
    path: &Path,
    label: &str,
    code: &'static str,
) -> Result<Vec<String>, LoadoutDiagnostic> {
    if let Some(Value::Array(array)) = value
        && let Some(result) = array
            .items()
            .iter()
            .map(|item| text(Some(item)).map(str::to_owned))
            .collect()
    {
        return Ok(result);
    }
    Err(LoadoutDiagnostic::at(
        code,
        format!("{label} must be an array of strings"),
        path,
    ))
}

fn object(value: Option<&Value>) -> Option<&ObjectValue> {
    match value? {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

fn mutable(object: &ObjectValue) -> Table {
    object.items().iter().cloned().collect()
}

fn table(value: Option<&Value>, path: &Path, label: &str) -> Result<Table, LoadoutDiagnostic> {
    value.map_or_else(
        || Ok(Table::new()),
        |value| {
            object(Some(value)).map(mutable).ok_or_else(|| {
                LoadoutDiagnostic::at("invalid-manifest", format!("{label} must be a table"), path)
            })
        },
    )
}

fn assign(
    table: &mut Table,
    key: String,
    value: Value,
    prefix: &[String],
    file: &Path,
    origins: &mut Origin,
) {
    let path = [prefix, std::slice::from_ref(&key)].concat();
    if matches!(table.get(&key), Some(Value::Object(_))) {
        origins.retain(|known, _| !known.starts_with(&path));
    }
    origins.insert(path, file.to_owned());
    table.insert(key, value);
}

fn merge(
    data: &mut Table,
    origins: &mut Origin,
    layer: Table,
    file: &Path,
) -> Result<(), LoadoutDiagnostic> {
    if layer.contains_key("remove") {
        if !layer.contains_key("extends") {
            return Err(LoadoutDiagnostic::at(
                "invalid-profile",
                "Profile remove requires extends",
                file,
            ));
        }
        let mut seen: Vec<Vec<String>> = Vec::new();
        for item in strings(layer.get("remove"), file, "remove", "invalid-profile")? {
            let path = header_path(&item)
                .map_err(|error| LoadoutDiagnostic::at("invalid-profile", error.message(), file))?;
            let error = || {
                LoadoutDiagnostic::at(
                    "invalid-profile",
                    format!("Invalid or overlapping remove path {item:?}"),
                    file,
                )
            };
            if seen
                .iter()
                .any(|previous| previous.starts_with(&path) || path.starts_with(previous))
            {
                return Err(error());
            }
            let (leaf, parents) = path.split_last().ok_or_else(error)?;
            let mut current = std::mem::take(data);
            let mut ancestors = Vec::new();
            for key in parents {
                let original = current.remove(key).ok_or_else(error)?;
                let child = object(Some(&original)).ok_or_else(error)?;
                ancestors.push((key, current));
                current = mutable(child);
            }
            current.remove(leaf).ok_or_else(error)?;
            for (key, mut parent) in ancestors.into_iter().rev() {
                parent.insert(key.clone(), ObjectValue::from_pairs(current).into());
                current = parent;
            }
            *data = current;
            origins.retain(|known, _| !known.starts_with(&path));
            seen.push(path);
        }
    }
    for (key, value) in layer
        .into_iter()
        .filter(|(key, _)| !matches!(key.as_str(), "extends" | "remove"))
    {
        let Value::Object(fields) = value else {
            assign(data, key, value, &[], file, origins);
            continue;
        };
        let mut old = object(data.get(&key)).map_or_else(Table::new, mutable);
        for (name, value) in fields.items() {
            let deeper = object(Some(value)).filter(|_| NAMED.contains(&key.as_str()));
            if let Some(deeper) = deeper {
                let mut target = object(old.get(name)).map_or_else(Table::new, mutable);
                let prefix = [key.clone(), name.clone()];
                for (field, value) in deeper.items() {
                    assign(
                        &mut target,
                        field.clone(),
                        value.clone(),
                        &prefix,
                        file,
                        origins,
                    );
                }
                old.insert(name.clone(), ObjectValue::from_pairs(target).into());
            } else {
                assign(
                    &mut old,
                    name.clone(),
                    value.clone(),
                    std::slice::from_ref(&key),
                    file,
                    origins,
                );
            }
        }
        data.insert(key, ObjectValue::from_pairs(old).into());
    }
    Ok(())
}

#[derive(Debug)]
pub(super) struct Manifest {
    pub(super) chain: Vec<PathBuf>,
    pub(super) sources: Vec<LoadoutSource>,
    pub(super) harnesses: Vec<LoadoutHarness>,
    pub(super) selectors: Vec<LoadoutSelector>,
    pub(super) skill_policies: BTreeMap<String, BTreeSet<PathBuf>>,
    pub(super) custom_routes: BTreeSet<(Agent, String)>,
}

fn profile_chain(
    root: &Path,
    profile: &str,
    inputs: &mut Inputs,
) -> Result<Vec<(PathBuf, Table)>, LoadoutDiagnostic> {
    let marker = root.join("loadout.toml");
    let default = inputs
        .table(&marker, "invalid-manifest", Some(root))?
        .ok_or_else(|| {
            LoadoutDiagnostic::at(
                "invalid-manifest",
                "Loadout source has no loadout.toml",
                &marker,
            )
        })?;
    let mut path = profile_file(root, profile)?;
    let mut data = if path == marker {
        default
    } else if let Some(table) = inputs.table(&path, "invalid-profile", Some(root))? {
        table
    } else {
        path.clone_from(&marker);
        default
    };
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    loop {
        let physical = inputs
            .0
            .get(&path)
            .map(|read| read.resolved.clone())
            .ok_or_else(|| {
                LoadoutDiagnostic::at(
                    "invalid-profile",
                    "Profile provenance was not captured",
                    &path,
                )
            })?;
        if physical.parent() != Some(root) {
            return Err(LoadoutDiagnostic::at(
                "escaped-path",
                "Profile must resolve to a sibling manifest",
                &path,
            ));
        }
        if !seen.insert(physical.clone()) {
            return Err(LoadoutDiagnostic::at(
                "profile-cycle",
                "Loadout profile extends cycle",
                &path,
            ));
        }
        let parent = data
            .get("extends")
            .map(|value| {
                text(Some(value)).map(str::to_owned).ok_or_else(|| {
                    LoadoutDiagnostic::at(
                        "invalid-profile",
                        "Profile extends must name a sibling manifest",
                        &path,
                    )
                })
            })
            .transpose()?;
        chain.push((physical, data));
        let Some(parent) = parent else {
            return Ok(chain);
        };
        path = profile_file(root, &parent)?;
        data = inputs
            .table(&path, "invalid-profile", Some(root))?
            .ok_or_else(|| {
                LoadoutDiagnostic::at(
                    "invalid-profile",
                    format!("Missing parent profile {parent:?}"),
                    &path,
                )
            })?;
    }
}

#[allow(clippy::too_many_lines)]
fn source_declarations(
    data: &Table,
    home: &Path,
    root: &Path,
    file: &Path,
    inputs: &mut Inputs,
) -> Result<Vec<LoadoutSource>, LoadoutDiagnostic> {
    let error = |message| LoadoutDiagnostic::at("invalid-source", message, file);
    let Some(Value::Array(sources)) = data.get("source") else {
        return Err(LoadoutDiagnostic::at(
            "invalid-manifest",
            "Declare at least one [[source]]",
            file,
        ));
    };
    if sources.is_empty() {
        return Err(LoadoutDiagnostic::at(
            "invalid-manifest",
            "Declare at least one [[source]]",
            file,
        ));
    }
    let mut names = BTreeSet::new();
    let mut result: Vec<LoadoutSource> = Vec::new();
    for entry in sources.items() {
        let raw = table(Some(entry), file, "source")?;
        let name = text(raw.get("name"))
            .filter(|value| {
                !value.contains('/') && validate_relative_path(value, "source name").is_ok()
            })
            .ok_or_else(|| error("Expected a single component source name"))?;
        if !names.insert(portable_path_key(name)) {
            return Err(error("Duplicate Loadout source name"));
        }
        if raw
            .keys()
            .any(|key| !["name", "path", "use", "overrides"].contains(&key.as_str()))
        {
            return Err(LoadoutDiagnostic::at(
                "unsupported-manifest",
                "Unknown source declaration field",
                file,
            ));
        }
        let path = text(raw.get("path"))
            .filter(|item| !item.is_empty())
            .ok_or_else(|| error("Source path is missing"))?;
        let logical = source_path(path, home, Some(root))
            .map_err(|issue| LoadoutDiagnostic::at("invalid-source", issue.message(), file))?;
        let path = inputs.directory(&logical)?;
        let uses = if raw.contains_key("use") {
            strings(raw.get("use"), file, "source use", "invalid-source")?
        } else {
            CATEGORIES.iter().map(|item| (*item).to_owned()).collect()
        };
        let uses: BTreeSet<String> = uses.into_iter().collect();
        if uses.is_empty() || uses.iter().any(|item| !CATEGORIES.contains(&item.as_str())) {
            return Err(error("Source use contains an unknown or empty category"));
        }
        if result
            .iter()
            .any(|other| other.path == path && !other.uses.is_disjoint(&uses))
        {
            return Err(error("Source aliases an earlier declared source"));
        }
        if uses.contains("kits") {
            let (templates, _) =
                inputs.read(&path.join("templates"), "invalid-source", Some(&path))?;
            if templates.identity.is_some() {
                return Err(LoadoutDiagnostic::at(
                    "unsupported-manifest",
                    "Rename templates/ to kits/ in this source",
                    &path,
                ));
            }
        }
        let mut overrides = BTreeMap::new();
        for (category, raw) in table(raw.get("overrides"), file, "overrides")? {
            if !uses.contains(&category)
                || !["skills", "agents", "module-config"].contains(&category.as_str())
            {
                return Err(error("Unsupported source override category"));
            }
            let items = strings(Some(&raw), file, "overrides", "invalid-source")?;
            let values: BTreeSet<_> = items.iter().cloned().collect();
            if values.len() != items.len()
                || items.iter().any(|item| {
                    validate_relative_path(item, "source override").is_err()
                        || item.split('/').any(|part| part == ".git")
                        || if category == "module-config" {
                            item.split_once('/')
                                .is_none_or(|(agent, _)| parse_agent(agent).is_none())
                        } else {
                            item.contains('/')
                        }
                })
            {
                return Err(error("Invalid source override path"));
            }
            overrides.insert(category, values);
        }
        result.push(LoadoutSource {
            name: name.to_owned(),
            path,
            uses,
            overrides,
            declared_in: file.to_owned(),
        });
    }
    Ok(result)
}

pub(super) fn parse_agent(value: &str) -> Option<Agent> {
    value.parse().ok().filter(|agent| *agent != Agent::Copilot)
}

fn specifications(agent: Agent) -> BTreeSet<&'static str> {
    let mut offered: BTreeSet<_> = AUTOMATIC.iter().copied().collect();
    if agent != Agent::Claude {
        offered.remove("mcp-permissions");
    }
    if agent == Agent::Codex {
        offered.remove("module-config");
        offered.insert("defaults");
    }
    if matches!(
        agent,
        Agent::Claude | Agent::Codex | Agent::Droid | Agent::Pi
    ) {
        offered.insert("plugins");
    }
    offered.extend(["hooks", "instructions"]);
    offered
}

#[allow(clippy::too_many_lines, clippy::type_complexity)]
fn selectors(
    data: &Table,
    origins: &Origin,
    profile: &str,
    file: &Path,
) -> Result<
    (
        Vec<LoadoutHarness>,
        Vec<LoadoutSelector>,
        BTreeSet<(Agent, String)>,
        BTreeSet<String>,
    ),
    LoadoutDiagnostic,
> {
    let common = table(data.get("all"), file, "all")?;
    let mut harnesses: BTreeMap<&str, (Agent, BTreeSet<String>)> = BTreeMap::new();
    let mut routes = Vec::new();
    let mut custom = BTreeSet::new();
    let mut named = BTreeSet::new();
    let invalid = |message: String| LoadoutDiagnostic::at("invalid-manifest", message, file);
    let mut claimed = BTreeSet::new();
    for category in ["instructions", "permissions"] {
        for (name, entry) in table(data.get(category), file, category)? {
            let block = table(Some(&entry), file, &name)?;
            let targets = block.get("destinations").map_or_else(
                || Ok(Vec::new()),
                |value| strings(Some(value), file, "destinations", "invalid-manifest"),
            )?;
            let output = block.get("output");
            if output.is_none() && targets.is_empty()
                || targets
                    .iter()
                    .any(|entry| entry.is_empty() || entry.contains('\0'))
            {
                return Err(invalid(format!(
                    "Legacy target {name} needs valid destinations or output"
                )));
            }
            if let Some(value) = output {
                let out = text(Some(value))
                    .filter(|item| validate_relative_path(item, "legacy output").is_ok())
                    .ok_or_else(|| invalid(format!("Invalid legacy output in {name}")))?;
                if !claimed.insert(out.to_owned()) {
                    return Err(invalid(format!("Duplicate legacy output {out}")));
                }
            }
            let selected = block
                .get("profile")
                .map(|value| {
                    text(Some(value))
                        .ok_or_else(|| invalid("Legacy profile must be a string".into()))
                })
                .transpose()?;
            if let Some(name) = selected.filter(|name| !name.is_empty()) {
                named.insert(name.to_owned());
            }
            if category == "instructions" {
                let agent = parse_agent(&name).ok_or_else(|| {
                    LoadoutDiagnostic::at(
                        "unsupported-manifest",
                        format!("Unknown instruction harness {name}"),
                        file,
                    )
                })?;
                let entries = strings(block.get("order"), file, "order", "invalid-manifest")?;
                let swaps = string_table(block.get("substitute"), file)?;
                if selected.is_none_or(|name| name == profile) {
                    let key = vec![category.into(), name, "order".into()];
                    let manifest = origins
                        .get(&key)
                        .cloned()
                        .unwrap_or_else(|| file.to_owned());
                    routes.push(LoadoutSelector {
                        agent,
                        category,
                        manifest,
                        key,
                        entries,
                        swaps,
                        array: true,
                        destinations: targets.len() + usize::from(output.is_some()),
                        all_profiles: selected.is_none(),
                    });
                    harnesses
                        .entry(agent.as_str())
                        .or_insert_with(|| (agent, BTreeSet::new()))
                        .1
                        .insert(category.into());
                }
            } else {
                let raw = text(block.get("render")).unwrap_or_default();
                let (agent, slice) = legacy_renderer(raw)
                    .filter(|_| !block.contains_key("rules"))
                    .ok_or_else(|| {
                        LoadoutDiagnostic::at(
                            "unsupported-manifest",
                            format!("Unsupported legacy permission renderer {raw:?}"),
                            file,
                        )
                    })?;
                if selected.is_none_or(|name| name == profile) {
                    harnesses
                        .entry(agent.as_str())
                        .or_insert_with(|| (agent, BTreeSet::new()))
                        .1
                        .insert(slice.into());
                    custom.insert((agent, slice.into()));
                }
            }
        }
    }
    for (name, entry) in data.iter().filter(|(name, _)| parse_agent(name).is_some()) {
        let Some(agent) = parse_agent(name) else {
            continue;
        };
        let declared = table(Some(entry), file, name)?;
        let offered = specifications(agent);
        if declared
            .keys()
            .any(|key| !offered.contains(key.as_str()) && !EXTRA.contains(&key.as_str()))
            || agent == Agent::Codex
                && (declared.contains_key("settings") || declared.contains_key("preserve"))
        {
            return Err(invalid(format!("Unsupported fields in [{name}]")));
        }
        let block: Table = common
            .clone()
            .into_iter()
            .chain(declared.clone())
            .filter(|(key, _)| offered.contains(key.as_str()) || EXTRA.contains(&key.as_str()))
            .collect();
        let mut active: BTreeSet<String> = AUTOMATIC
            .iter()
            .filter(|key| offered.contains(**key) && !block.contains_key(**key))
            .map(|key| (*key).to_owned())
            .collect();
        for (category, value) in &block {
            let flag = match value {
                Value::Scalar(scalar) => scalar.as_boolean(),
                _ => None,
            };
            if AUTOMATIC.contains(&category.as_str())
                && flag.is_none()
                && !(matches!(category.as_str(), "mcp" | "mcp-permissions" | "permissions")
                    && matches!(value, Value::Array(array) if array.is_empty()))
            {
                return Err(invalid(format!(
                    "{name}.{category} must be enabled or disabled"
                )));
            }
            if category == "preserve"
                || matches!(category.as_str(), "settings" | "defaults" | "plugins")
                    && flag != Some(false)
                    && text(Some(value)).is_none()
            {
                strings(Some(value), file, category, "invalid-manifest")?;
            }
            if !offered.contains(category.as_str()) || flag == Some(false) {
                continue;
            }
            active.insert(category.clone());
            let slice = match category.as_str() {
                "instructions" => "instructions",
                "hooks" => "hooks",
                _ => continue,
            };
            let array = slice != "hooks" || text(Some(value)).is_none();
            let entries = if array {
                strings(Some(value), file, category, "invalid-manifest")?
            } else {
                text(Some(value)).into_iter().map(str::to_owned).collect()
            };
            let key = vec![
                if declared.contains_key(category) {
                    name.clone()
                } else {
                    "all".into()
                },
                category.clone(),
            ];
            let manifest = origins
                .get(&key)
                .cloned()
                .unwrap_or_else(|| file.to_owned());
            let swaps = if slice == "instructions" {
                string_table(block.get("substitute"), file)?
            } else {
                BTreeMap::new()
            };
            routes.push(LoadoutSelector {
                agent,
                category: slice,
                manifest,
                key,
                entries,
                swaps,
                array,
                destinations: 1,
                all_profiles: false,
            });
        }
        string_table(block.get("substitute"), file)?;
        let readers: &[&str] = match agent {
            Agent::Claude => &["permissions", "hooks", "plugins"],
            Agent::Cursor => &["permissions"],
            Agent::Droid => &["permissions", "plugins"],
            Agent::Opencode => &["permissions", "mcp"],
            Agent::Pi => &["plugins"],
            _ => &[],
        };
        if declared.contains_key("settings")
            && !readers.iter().any(|category| active.contains(*category))
        {
            return Err(invalid(format!(
                "{name}.settings needs an enabled slice that reads it"
            )));
        }
        harnesses
            .entry(agent.as_str())
            .or_insert_with(|| (agent, BTreeSet::new()))
            .1
            .extend(active);
    }
    if harnesses.values().all(|(_, values)| values.is_empty()) {
        return Err(invalid(
            "No active targets for the selected Loadout profile".into(),
        ));
    }
    Ok((
        harnesses
            .into_values()
            .filter(|(_, slices)| !slices.is_empty())
            .map(|(agent, slices)| LoadoutHarness { agent, slices })
            .collect(),
        routes,
        custom,
        named,
    ))
}

fn legacy_renderer(renderer: &str) -> Option<(Agent, &'static str)> {
    if let Some(agent) = parse_agent(renderer) {
        return Some((agent, "permissions"));
    }
    if let Some(agent) = renderer.strip_suffix("-servers").and_then(parse_agent) {
        return Some((agent, "mcp"));
    }
    if let Some(agent) = renderer.strip_suffix("-hooks").and_then(parse_agent) {
        return Some((agent, "hooks"));
    }
    if let Some(agent) = renderer.strip_suffix("-plugins").and_then(parse_agent) {
        return Some((agent, "plugins"));
    }
    match renderer {
        "codex-config" => Some((Agent::Codex, "mcp")),
        "codex-settings" => Some((Agent::Codex, "defaults")),
        "claude-mcp-permissions" => Some((Agent::Claude, "mcp-permissions")),
        _ => None,
    }
}

fn string_table(
    value: Option<&Value>,
    file: &Path,
) -> Result<BTreeMap<String, String>, LoadoutDiagnostic> {
    table(value, file, "substitute")?
        .into_iter()
        .map(|(name, value)| {
            let replacement = text(Some(&value)).ok_or_else(|| {
                LoadoutDiagnostic::at(
                    "invalid-manifest",
                    "Substitute targets must be strings",
                    file,
                )
            })?;
            Ok((name, replacement.to_owned()))
        })
        .collect()
}

pub(super) fn load(
    root: &Path,
    profile: &str,
    home: &Path,
    inputs: &mut Inputs,
) -> Result<Manifest, LoadoutDiagnostic> {
    let chain = profile_chain(root, profile, inputs)?;
    let Some((active, first)) = chain.first() else {
        return Err(LoadoutDiagnostic::at(
            "invalid-profile",
            "Empty profile chain",
            root,
        ));
    };
    let requested = profile_file(root, profile)?;
    let legacy = profile == "loadout"
        || !first.contains_key("extends")
        || inputs
            .0
            .get(&requested)
            .is_none_or(|read| read.identity.is_none());
    let mut data = Table::new();
    let mut origins = Origin::new();
    for (file, layer) in chain.iter().rev() {
        merge(&mut data, &mut origins, layer.clone(), file)?;
    }
    if data.keys().any(|key| {
        parse_agent(key).is_none()
            && !NAMED.contains(&key.as_str())
            && !["all", "source"].contains(&key.as_str())
    }) {
        return Err(LoadoutDiagnostic::at(
            "unsupported-manifest",
            "Unknown top-level manifest key",
            active,
        ));
    }
    let sources = source_declarations(
        &data,
        home,
        root,
        origins.get(&vec!["source".into()]).unwrap_or(active),
        inputs,
    )?;
    let (harnesses, selectors, custom_routes, named) = selectors(&data, &origins, profile, active)?;
    if profile != "default" && legacy && !named.contains(profile) {
        return Err(LoadoutDiagnostic::at(
            "invalid-profile",
            format!("Unknown profile {profile:?}"),
            active,
        ));
    }
    let mut skill_policies = BTreeMap::new();
    for (name, entry) in table(data.get("skills"), active, "skills")? {
        let block = table(Some(&entry), active, &name)?;
        let agents = strings(
            block.get("agents"),
            active,
            "skill agents",
            "invalid-manifest",
        )?;
        if name.is_empty()
            || name.contains(['/', '\\', '\0'])
            || matches!(name.as_str(), "." | "..")
            || agents.iter().collect::<BTreeSet<_>>().len() != agents.len()
            || agents.iter().any(|name| {
                !harnesses.iter().any(|harness| {
                    harness.agent.as_str() == name && harness.slices.contains("skills")
                })
            })
            || block
                .keys()
                .any(|key| !["agents", "render"].contains(&key.as_str()))
            || block.get("render").is_some_and(
                |value| !matches!(value, Value::Scalar(scalar) if scalar.as_boolean().is_some()),
            )
        {
            return Err(LoadoutDiagnostic::at(
                "invalid-manifest",
                format!("Invalid skill policy for {name}"),
                active,
            ));
        }
        let files = block
            .keys()
            .filter_map(|key| {
                origins
                    .get(&vec!["skills".into(), name.clone(), key.clone()])
                    .cloned()
            })
            .collect();
        skill_policies.insert(name, files);
    }
    Ok(Manifest {
        chain: chain.into_iter().map(|(file, _)| file).collect(),
        sources,
        harnesses,
        selectors,
        custom_routes,
        skill_policies,
    })
}
