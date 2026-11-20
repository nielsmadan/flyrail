use super::manifest::{Inputs, resolved, strings, text};
use super::{
    LoadoutDestination, LoadoutDiagnostic, LoadoutImpact, LoadoutOptions, LoadoutSelector,
    LoadoutSource, sync_notice,
};
use crate::authority::sha256_hex;
use crate::hook_translation::{
    HookTranslationError, asset_argument, event_name, hook_artifacts_with_launcher,
    support_asset_artifacts,
};
use crate::json_document::encode_value;
use crate::mcp_translation::{UnsupportedTranslation, literal};
use crate::validation::portable_path_key;
use crate::{
    Agent, ArrayValue, Artifact, Bundle, Content, Dependency, DependencyMode, DocumentFormat,
    EnvironmentValue, Family, FileContent, FileMode, InstructionArtifact, Key, McpArtifact,
    McpTransport, Member, NativeArtifact, Notice, ObjectValue, RenderContext, RenderedArtifact,
    RenderedBundle, Selector, SelectorPart, SkillArtifact, StructuredContent, Surface, Target,
    TargetScope, Value, ValueError,
};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

const FILTERED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    "node_modules",
    ".venv",
    "__pycache__",
    ".mypy_cache",
    ".ruff_cache",
    ".pytest_cache",
];

fn id(values: &[&str]) -> String {
    sha256_hex(&Sha256::digest(values.join("\0").as_bytes()))
}

const fn module(agent: Agent) -> Option<(&'static str, &'static str)> {
    match agent {
        Agent::Opencode => Some(("plugins", ".config/opencode/plugins/")),
        Agent::Pi => Some(("extensions", ".pi/agent/extensions/")),
        _ => None,
    }
}

fn artifact_slices(artifact: &Artifact, agents: &[Agent]) -> Vec<(Agent, &'static str)> {
    if let Artifact::Native(item) = artifact {
        return vec![(item.audience().agent(), "module-config")];
    }
    agents
        .iter()
        .map(|agent| {
            (
                *agent,
                match artifact {
                    Artifact::Skill(_) => "skills",
                    Artifact::Instruction(_) => "instructions",
                    Artifact::Mcp(_) => "mcp",
                    Artifact::Hook(_) if module(*agent).is_some() => "module-config",
                    Artifact::Hook(_) => "hooks",
                    Artifact::Native(_) => "module-config",
                },
            )
        })
        .collect()
}

pub(super) fn hooks_supported(bundle: &Bundle, agents: &[Agent]) -> bool {
    !agents.contains(&Agent::Droid)
        || !bundle
            .artifacts()
            .iter()
            .any(|item| matches!(item, Artifact::Hook(_)))
}

pub(super) fn required_slices(
    bundle: &Bundle,
    agents: &[Agent],
) -> BTreeSet<(Agent, &'static str)> {
    bundle
        .artifacts()
        .iter()
        .flat_map(|item| artifact_slices(item, agents))
        .collect()
}

fn filtered(path: &Path) -> bool {
    path.iter().any(|part| {
        part.to_str()
            .is_some_and(|part| FILTERED_DIRS.contains(&part))
    }) || path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| [".DS_Store", ".loadout-bundle.toml"].contains(&name))
        || path
            .extension()
            .and_then(|suffix| suffix.to_str())
            .is_some_and(|suffix| ["pyc", "pyo", "swp"].contains(&suffix))
}

fn mcp_fields(fields: &ObjectValue, http: bool) -> bool {
    let Some(headers) = mcp_object(fields.get("headers")) else {
        return false;
    };
    let Some(env) = mcp_object(fields.get("env")) else {
        return false;
    };
    let auth = fields.get("auth_env_var");
    let args = fields.get("args");
    auth.is_none_or(|value| text(Some(value)).is_some())
        && args.is_none_or(|value| matches!(value, Value::Array(array) if array.items().iter().all(|item| text(Some(item)).is_some())))
        && env.iter().all(|(_, value)| text(Some(value)).is_some())
        && (headers.is_empty() || http)
        && headers.iter().all(|(key, value)| {
            text(Some(value)).is_some_and(|header| !header.contains(['$', '{']) && !header.starts_with('!'))
                && !(text(auth).is_some_and(|token| !token.is_empty()) && key.eq_ignore_ascii_case("authorization"))
        })
}

fn mcp_object(value: Option<&Value>) -> Option<&[(String, Value)]> {
    match value {
        None => Some(&[]),
        Some(Value::Object(fields)) => Some(fields.items()),
        _ => None,
    }
}

fn mcp_namespace(value: &str, pi: bool) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric()
                || "_*?".contains(character)
                || (!pi && character == '-')
            {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn mcp_permission_overlap(other: &str, tool: &str, server: &str, pi: bool) -> bool {
    let separator = if pi { "__" } else { "_" };
    let prefix = format!("{}{separator}", mcp_namespace(server, pi));
    let namespace = format!("{}{separator}", mcp_namespace(other, pi));
    if other.contains(['*', '?']) {
        return false;
    }
    if prefix == namespace {
        return other != server;
    }
    let pattern = namespace + &mcp_namespace(tool, pi);
    for (actual, expected) in pattern.bytes().zip(prefix.bytes()) {
        if actual == b'*' {
            return true;
        }
        if actual != b'?' && actual != expected {
            return false;
        }
    }
    pattern.len() > prefix.len()
}

fn structured(keys: &[String], value: Value, member: bool) -> Result<Content, ValueError> {
    let mut path = keys
        .iter()
        .map(|key| Key::new(key).map(SelectorPart::Key))
        .collect::<Result<Vec<_>, _>>()?;
    if member {
        path.push(SelectorPart::Member(Member::new(
            value.clone(),
            std::iter::empty::<String>(),
        )?));
    }
    Ok(Content::Structured(StructuredContent::new(
        DocumentFormat::Toml,
        Selector::new(path)?,
        value,
        None,
    )?))
}

struct Rendering<'a, 'b> {
    bundle: &'a Bundle,
    destination: &'a LoadoutDestination<'a>,
    options: LoadoutOptions<'a>,
    inputs: &'b mut Inputs,
    impacts: &'b mut Vec<LoadoutImpact>,
    artifacts: Vec<RenderedArtifact>,
    edges: HashSet<Dependency>,
    notices: Vec<Notice>,
    assets: HashMap<String, PathBuf>,
}

impl<'a> Rendering<'a, '_> {
    fn active_file(&self) -> &Path {
        self.destination
            .setup
            .profile_chain()
            .first()
            .map_or_else(|| self.destination.setup.root(), PathBuf::as_path)
    }
    fn fail(&self, code: &'static str, message: impl Into<String>) -> LoadoutDiagnostic {
        LoadoutDiagnostic::at(code, message, self.active_file())
    }
    fn invalid(&self, error: &ValueError) -> LoadoutDiagnostic {
        self.fail("source-plan", error.message())
    }
    fn unsupported(&self, error: &UnsupportedTranslation) -> LoadoutDiagnostic {
        self.fail(error.code(), error.message())
    }
    fn active(&self, category: &str) -> Vec<Agent> {
        self.destination
            .setup
            .harnesses()
            .iter()
            .filter(|harness| harness.slices.contains(category))
            .map(|harness| harness.agent)
            .collect()
    }
    fn gate(&self) -> Result<(), LoadoutDiagnostic> {
        for agent in &self.destination.agents {
            if *agent == Agent::Copilot {
                return Err(self.fail(
                    "unsupported-harness",
                    "Copilot is outside the Loadout harness set",
                ));
            }
            if !self
                .destination
                .setup
                .harnesses()
                .iter()
                .any(|harness| harness.agent == *agent)
            {
                return Err(self.fail(
                    "disabled-harness",
                    format!("{agent} is absent from this profile"),
                ));
            }
        }
        if !hooks_supported(self.bundle, &self.destination.agents) {
            return Err(self.fail(
                "unsupported-hook",
                "Droid hooks have no verified Loadout mapping",
            ));
        }
        for (agent, category) in required_slices(self.bundle, &self.destination.agents) {
            if !self.destination.source.uses.contains(category) {
                return Err(LoadoutDiagnostic::at(
                    "source-use",
                    format!(
                        "Source {} does not offer {category}",
                        self.destination.source.name
                    ),
                    &self.destination.source.declared_in,
                ));
            }
            if !self.destination.agents.contains(&agent) {
                return Err(self.fail(
                    "native-audience",
                    format!("Native artifact targets {agent} outside this selection"),
                ));
            }
            if !self.active(category).contains(&agent) {
                return Err(self.fail(
                    "disabled-slice",
                    format!("{agent}.{category} is disabled in this profile"),
                ));
            }
            if self
                .destination
                .setup
                .manifest
                .custom_routes
                .contains(&(agent, category.into()))
            {
                return Err(self.fail(
                    "unsupported-slice",
                    format!("{agent}.{category} uses a custom legacy renderer"),
                ));
            }
        }
        Ok(())
    }
    fn context(&self, agent: Agent) -> Result<RenderContext, LoadoutDiagnostic> {
        let make = || {
            let target =
                Target::user_with_home_and_environment(agent, &self.destination.setup.home, [])?;
            let mut context = RenderContext::new(target, self.options.platform)?
                .with_asset_root(self.destination.source.path.join(".flyrail-assets"))?;
            if let Some(root) = self.options.execution_root {
                context = context.with_execution_root(root)?;
            }
            if let Some(runtime) = self.options.hook_runtime {
                context = context.with_hook_runtime(runtime.clone());
            }
            Ok(context)
        };
        make().map_err(|error| self.invalid(&error))
    }
    fn destination(&self, path: PathBuf) -> Result<PathBuf, LoadoutDiagnostic> {
        if !path.starts_with(&self.destination.source.path)
            || !resolved(&path).is_ok_and(|value| value == path)
        {
            return Err(LoadoutDiagnostic::at(
                "escaped-path",
                "Source destination must stay within the selected source without aliases",
                &path,
            ));
        }
        Ok(path)
    }
    fn impact(&mut self, category: &'static str, path: &Path, harnesses: Vec<Agent>, shared: bool) {
        let shared = shared
            || harnesses
                .iter()
                .any(|agent| !self.destination.agents.contains(agent));
        let entry = LoadoutImpact {
            category,
            path: path.to_owned(),
            harnesses,
            shared,
        };
        if !self.impacts.contains(&entry) {
            self.impacts.push(entry);
        }
    }
    fn file_seen(
        &mut self,
        source: &LoadoutSource,
        relative: &str,
    ) -> Result<bool, LoadoutDiagnostic> {
        Ok(self
            .inputs
            .file(
                &source.path.join(relative),
                "invalid-source",
                Some(&source.path),
            )?
            .is_some())
    }
    fn write(
        &mut self,
        name: &str,
        family: Family,
        path: PathBuf,
        content: Content,
        subtree: Option<String>,
    ) -> Result<(), LoadoutDiagnostic> {
        let item = RenderedArtifact::new(name, family, path, content, subtree, false, Vec::new())
            .map_err(|error| self.invalid(&error))?;
        self.artifacts.push(item);
        Ok(())
    }
    fn edge(
        &mut self,
        dependent: &str,
        required: &str,
        mode: DependencyMode,
    ) -> Result<(), LoadoutDiagnostic> {
        self.edges.insert(
            Dependency::with_mode(dependent, required, mode)
                .map_err(|error| self.invalid(&error))?,
        );
        Ok(())
    }
    fn skill(&mut self, artifact: &SkillArtifact) -> Result<(), LoadoutDiagnostic> {
        let name = artifact.name();
        let root = self.destination(self.destination.source.path.join("skills"))?;
        if filtered(Path::new(name))
            || artifact
                .content()
                .entries()
                .iter()
                .any(|entry| filtered(Path::new(entry.path())))
        {
            return Err(self.fail(
                "skill-filter",
                format!("Loadout excludes part of skill {name}"),
            ));
        }
        for source in self
            .destination
            .setup
            .sources()
            .iter()
            .filter(|item| item.uses.contains("skills"))
        {
            if source
                .overrides
                .get("skills")
                .is_some_and(|items| items.contains(name))
                || self.file_seen(source, &format!("skills/{name}.local/SKILL.md"))?
                || source != self.destination.source
                    && self.file_seen(source, &format!("skills/{name}/SKILL.md"))?
            {
                return Err(LoadoutDiagnostic::at(
                    "source-collision",
                    format!("Skill {name} also resolves from {}", source.name),
                    &source.path,
                ));
            }
        }
        let path = if let Some(files) = self.destination.setup.manifest.skill_policies.get(name) {
            if files.len() != 1 {
                return Err(self.fail(
                    "skill-policy",
                    format!("Split declarations control skill {name}"),
                ));
            }
            files
                .first()
                .cloned()
                .unwrap_or_else(|| self.active_file().to_owned())
        } else {
            self.active_file().to_owned()
        };
        self.impact(
            "skills",
            &path,
            self.destination.agents.clone(),
            path != self.active_file(),
        );
        let policy_id = id(&["skill-policy", artifact.id()]);
        let policy = ObjectValue::from_pairs([
            (
                "agents",
                Value::from(ArrayValue::new(
                    self.destination
                        .agents
                        .iter()
                        .map(|agent| Value::from(agent.as_str())),
                )),
            ),
            ("render", Value::from(false)),
        ]);
        let content = structured(&["skills".into(), name.into()], policy.into(), false)
            .map_err(|error| self.invalid(&error))?;
        self.write(&policy_id, Family::Skills, path, content, None)?;
        self.write(
            artifact.id(),
            Family::Skills,
            root,
            Content::Tree(artifact.content().clone()),
            Some(name.into()),
        )?;
        self.edge(artifact.id(), &policy_id, DependencyMode::Revision)
    }
    fn routes(&self, category: &str) -> Vec<Vec<LoadoutSelector>> {
        let mut grouped = BTreeMap::<_, Vec<_>>::new();
        for route in self
            .destination
            .setup
            .selectors()
            .iter()
            .filter(|route| route.category == category)
        {
            grouped
                .entry((&route.manifest, &route.key))
                .or_default()
                .push(route.clone());
        }
        grouped
            .into_values()
            .filter(|routes| {
                routes
                    .iter()
                    .any(|route| self.destination.agents.contains(&route.agent))
            })
            .collect()
    }
    fn entry(
        &mut self,
        routes: &[LoadoutSelector],
        name: &str,
        family: Family,
        required: &str,
    ) -> Result<(), LoadoutDiagnostic> {
        let Some(route) = routes.first() else {
            return Err(self.fail("disabled-slice", "No Loadout selector for this fragment"));
        };
        let mut agents: Vec<_> = routes.iter().map(|item| item.agent).collect();
        agents.sort_by_key(|agent| agent.as_str());
        agents.dedup();
        let path = &route.manifest;
        self.impact(
            route.category,
            path,
            agents,
            path != self.active_file()
                || routes
                    .iter()
                    .any(|item| item.destinations > 1 || item.all_profiles),
        );
        if !route.array {
            return Err(LoadoutDiagnostic::at(
                "selector-not-array",
                "Write the fragment selector as an array before adding a member",
                path,
            ));
        }
        let qualified = format!("{}/{name}", self.destination.source.name);
        if routes.iter().any(|item| {
            item.swaps.contains_key(&qualified)
                || item
                    .entries
                    .iter()
                    .filter(|entry| *entry != &qualified)
                    .any(|entry| {
                        let selected = item.swaps.get(entry).unwrap_or(entry);
                        selected == name || selected == &qualified
                    })
        }) {
            return Err(LoadoutDiagnostic::at(
                "selector-shadowed",
                format!("An existing selector or substitution shadows {qualified}"),
                path,
            ));
        }
        let key = std::iter::once("selector")
            .chain(path.to_str())
            .chain(route.key.iter().map(String::as_str))
            .chain(std::iter::once(qualified.as_str()))
            .collect::<Vec<_>>();
        let identifier = id(&key);
        let content =
            structured(&route.key, qualified.into(), true).map_err(|error| self.invalid(&error))?;
        self.write(&identifier, family, path.clone(), content, None)?;
        self.edge(&identifier, required, DependencyMode::StableReference)
    }
    fn instruction(&mut self, artifact: &InstructionArtifact) -> Result<(), LoadoutDiagnostic> {
        let name = format!(
            "flyrail-{}",
            id(&["instructions", self.bundle.id(), artifact.id()])
        );
        let relative = format!("instructions/{name}.md");
        let path = self.destination(self.destination.source.path.join(&relative))?;
        for source in self.destination.setup.sources() {
            if source != self.destination.source
                && source.uses.contains("instructions")
                && self.file_seen(source, &relative)?
            {
                return Err(LoadoutDiagnostic::at(
                    "source-collision",
                    format!("Instruction {name} already exists in {}", source.name),
                    &source.path,
                ));
            }
        }
        let routes = self.routes("instructions");
        self.check_unselected("instructions", &name, &routes)?;
        self.write(
            artifact.id(),
            Family::Instructions,
            path,
            Content::File(FileContent::new(
                artifact.text().as_bytes(),
                FileMode::Private,
            )),
            None,
        )?;
        for group in &routes {
            self.entry(group, &name, Family::Instructions, artifact.id())?;
        }
        Ok(())
    }
    fn check_unselected(
        &self,
        category: &str,
        name: &str,
        routes: &[Vec<LoadoutSelector>],
    ) -> Result<(), LoadoutDiagnostic> {
        let qualified = format!("{}/{name}", self.destination.source.name);
        for route in self
            .destination
            .setup
            .selectors()
            .iter()
            .filter(|route| route.category == category)
        {
            if !routes.iter().any(|items| items.contains(route))
                && route.entries.iter().any(|entry| {
                    let effective = route.swaps.get(entry).unwrap_or(entry);
                    effective == name || effective == &qualified
                })
            {
                return Err(LoadoutDiagnostic::at(
                    "selector-shadowed",
                    format!("An unselected {category} target refers to {qualified}"),
                    &route.manifest,
                ));
            }
        }
        Ok(())
    }
    fn mcp_tables(&mut self) -> Result<Vec<(&'a LoadoutSource, Vec<String>)>, LoadoutDiagnostic> {
        self.destination
            .setup
            .sources()
            .iter()
            .filter(|source| source.uses.contains("mcp"))
            .map(|source| {
                let path = source.path.join("mcp.toml");
                let table = self
                    .inputs
                    .table(&path, "invalid-mcp", Some(&source.path))?
                    .unwrap_or_default();
                for (name, value) in &table {
                    let invalid = || {
                        LoadoutDiagnostic::at(
                            "invalid-mcp",
                            format!("Invalid Loadout MCP definition {name:?}"),
                            &path,
                        )
                    };
                    let Value::Object(fields) = value else {
                        return Err(invalid());
                    };
                    let field = match text(fields.get("transport")) {
                        Some("http") => "url",
                        Some("stdio") => "command",
                        _ => return Err(invalid()),
                    };
                    if text(fields.get(field)).is_none_or(str::is_empty)
                        || !mcp_fields(fields, field == "url")
                        || fields.items().iter().any(|(key, _)| {
                            ![
                                "transport",
                                "command",
                                "args",
                                "env",
                                "url",
                                "auth_env_var",
                                "headers",
                            ]
                            .contains(&key.as_str())
                        })
                    {
                        return Err(invalid());
                    }
                }
                Ok((source, table.into_keys().collect()))
            })
            .collect()
    }

    fn check_mcp_permissions(&mut self, servers: &BTreeSet<&str>) -> Result<(), LoadoutDiagnostic> {
        let agents = self.active("permissions");
        let opencode = agents.contains(&Agent::Opencode);
        let pi = agents.contains(&Agent::Pi);
        if !opencode && !pi {
            return Ok(());
        }
        for source in self
            .destination
            .setup
            .sources()
            .iter()
            .filter(|s| s.uses.contains("permissions"))
        {
            let path = source.path.join("permissions.toml");
            let table = self
                .inputs
                .table(&path, "invalid-permissions", Some(&source.path))?
                .unwrap_or_default();
            let rules = mcp_object(table.get("mcp")).ok_or_else(|| {
                LoadoutDiagnostic::at(
                    "invalid-permissions",
                    "Expected the MCP permissions table",
                    &path,
                )
            })?;
            for (decision, value) in rules {
                if !["allow", "deny", "ask"].contains(&decision.as_str()) {
                    return Err(LoadoutDiagnostic::at(
                        "invalid-permissions",
                        "Expected MCP permission decision lists",
                        &path,
                    ));
                }
                for rule in strings(Some(value), &path, decision, "invalid-permissions")? {
                    let (other, tool) = rule
                        .split_once('/')
                        .filter(|(a, b)| !a.is_empty() && !b.is_empty())
                        .ok_or_else(|| {
                            LoadoutDiagnostic::at(
                                "invalid-permissions",
                                "MCP permissions must use server/tool entries",
                                &path,
                            )
                        })?;
                    if servers.iter().any(|server| {
                        (opencode && mcp_permission_overlap(other, tool, server, false))
                            || (pi && mcp_permission_overlap(other, tool, server, true))
                    }) {
                        return Err(LoadoutDiagnostic::at(
                            "mcp-permission-overlap",
                            format!("MCP permission {rule:?} overlaps a proposed server namespace"),
                            &path,
                        ));
                    }
                }
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_lines)]
    fn mcp(
        &mut self,
        artifact: &McpArtifact,
        tables: &[(&LoadoutSource, Vec<String>)],
    ) -> Result<(), LoadoutDiagnostic> {
        let path = self.destination(self.destination.source.path.join("mcp.toml"))?;
        let affected = self.active("mcp");
        self.impact("mcp", &path, affected.clone(), false);
        if affected.iter().any(|agent| {
            self.destination
                .setup
                .manifest
                .custom_routes
                .contains(&(*agent, "mcp".into()))
        }) {
            return Err(self.fail(
                "unsupported-slice",
                "MCP also reaches a custom legacy renderer",
            ));
        }
        let name = artifact.name();
        for (source, table) in tables {
            if table.iter().any(|other| {
                (name != other || *source != self.destination.source)
                    && (portable_path_key(name) == portable_path_key(other)
                        || affected.contains(&Agent::Pi)
                            && name.replace('-', "_") == other.replace('-', "_"))
            }) {
                return Err(LoadoutDiagnostic::at(
                    "source-collision",
                    format!("MCP name {name} collides in {}", source.name),
                    &source.path.join("mcp.toml"),
                ));
            }
        }
        let contexts = affected
            .iter()
            .map(|agent| self.context(*agent))
            .collect::<Result<Vec<_>, _>>()?;
        let server = match artifact.transport() {
            McpTransport::Http(transport) => {
                for context in &contexts {
                    literal(transport.url(), context, "url")
                        .map_err(|error| self.unsupported(&error))?;
                }
                let mut fields = vec![
                    ("transport", Value::from("http")),
                    ("url", Value::from(transport.url())),
                ];
                if let Some(token) = transport.bearer_token() {
                    fields.push(("auth_env_var", Value::from(token.name())));
                }
                ObjectValue::from_pairs(fields)
            }
            McpTransport::Command(command) => {
                if command.cwd().is_some() {
                    return Err(
                        self.fail("mcp-cwd", format!("Loadout cannot preserve cwd for {name}"))
                    );
                }
                let argv = command
                    .argv()
                    .iter()
                    .map(|item| asset_argument(item, &self.assets))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| self.invalid(&error))?;
                let env = command
                    .env()
                    .iter()
                    .map(|(key, value)| match value {
                        EnvironmentValue::Text(text) => Ok((key.clone(), text.clone())),
                        EnvironmentValue::Reference(_) => Err(self.fail(
                            "mcp-env-reference",
                            format!(
                                "Loadout cannot preserve stdio environment references for {name}"
                            ),
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                for context in &contexts {
                    for (key, value) in argv
                        .iter()
                        .map(|value| ("argv", value))
                        .chain(env.iter().map(|(_, value)| ("env", value)))
                    {
                        literal(value, context, key).map_err(|error| self.unsupported(&error))?;
                        if context.audience().agent() == Agent::Pi && value.starts_with('!') {
                            return Err(
                                self.fail("mcp-literal", "Pi could interpret a literal MCP value")
                            );
                        }
                    }
                }
                let (program, arguments) = argv
                    .split_first()
                    .ok_or_else(|| self.fail("mcp-command", "Expected an MCP command"))?;
                ObjectValue::from_pairs([
                    ("transport", Value::from("stdio")),
                    ("command", Value::from(program.as_str())),
                    (
                        "args",
                        ArrayValue::new(arguments.iter().map(|arg| Value::from(arg.as_str())))
                            .into(),
                    ),
                    (
                        "env",
                        ObjectValue::from_pairs(
                            env.into_iter()
                                .map(|(key, value)| (key, Value::from(value))),
                        )
                        .into(),
                    ),
                ])
            }
        };
        let content = structured(&[name.into()], server.into(), false)
            .map_err(|error| self.invalid(&error))?;
        self.write(artifact.id(), Family::Mcp, path, content, None)
    }
    fn module_path(&mut self, agent: Agent, relative: &str) -> Result<PathBuf, LoadoutDiagnostic> {
        let tail = format!("{agent}/{relative}");
        let path = format!("module-config/{tail}");
        for source in self
            .destination
            .setup
            .sources()
            .iter()
            .filter(|source| source.uses.contains("module-config"))
        {
            if source
                .overrides
                .get("module-config")
                .is_some_and(|items| items.contains(&tail))
                || source != self.destination.source && self.file_seen(source, &path)?
            {
                return Err(LoadoutDiagnostic::at(
                    "source-collision",
                    format!("Module {tail} also resolves from {}", source.name),
                    &source.path,
                ));
            }
        }
        self.destination(self.destination.source.path.join(path))
    }
    fn native(&mut self, item: &NativeArtifact) -> Result<(), LoadoutDiagnostic> {
        let audience = item.audience();
        let agent = audience.agent();
        let invalid = || {
            self.fail(
                "native-path",
                format!(
                    "No Loadout source mapping for native artifact {}",
                    item.id()
                ),
            )
        };
        let (directory, prefix) = module(agent).ok_or_else(invalid)?;
        let relative = item
            .destination()
            .strip_prefix(prefix)
            .ok_or_else(invalid)?;
        if relative.contains('/')
            || filtered(Path::new(relative))
            || ["loadout-hooks.js", "loadout-hooks.ts"].contains(&relative)
            || !Path::new(relative)
                .extension()
                .and_then(|suffix| suffix.to_str())
                .is_some_and(|suffix| ["ts", "js", "mjs"].contains(&suffix))
            || audience.scope() != TargetScope::User
            || audience.surface() != Surface::Cli
            || audience
                .platform()
                .is_some_and(|platform| platform != self.options.platform)
            || !matches!(item.content(), Content::File(_))
        {
            return Err(invalid());
        }
        let path = self.module_path(agent, &format!("{directory}/{relative}"))?;
        self.impact("module-config", &path, vec![agent], false);
        self.write(item.id(), item.family(), path, item.content().clone(), None)
    }
    fn hook_fragment(
        &mut self,
        agent: Agent,
        items: &[(&crate::HookArtifact, &RenderedArtifact)],
    ) -> Result<String, LoadoutDiagnostic> {
        let name = format!(
            "flyrail-{}",
            id(&["hooks", self.bundle.id(), agent.as_str()])
        );
        let relative = format!("hooks/{name}.json");
        let path = self.destination(self.destination.source.path.join(&relative))?;
        for source in self.destination.setup.sources() {
            if source != self.destination.source
                && source.uses.contains("hooks")
                && self.file_seen(source, &relative)?
            {
                return Err(LoadoutDiagnostic::at(
                    "source-collision",
                    format!("Hook fragment {name} already exists in {}", source.name),
                    &source.path,
                ));
            }
        }
        let mut events = BTreeMap::new();
        for (hook, item) in items {
            let Content::Structured(content) = item.content() else {
                return Err(self.fail(
                    "hook-source",
                    "Expected structured native hook registration",
                ));
            };
            let raw = if agent == Agent::Cursor {
                let Value::Object(fields) = content.value() else {
                    return Err(self.fail("hook-source", "Expected an object for Cursor hook"));
                };
                let fields = ObjectValue::from_pairs(
                    fields
                        .items()
                        .iter()
                        .cloned()
                        .chain([("type".into(), Value::from("command"))]),
                );
                let wrapped = ObjectValue::from_pairs([(
                    "hooks",
                    Value::from(ArrayValue::new([fields.into()])),
                )]);
                wrapped.into()
            } else {
                content.value().clone()
            };
            events.insert(
                event_name(Agent::Codex, hook.event()),
                Value::from(ArrayValue::new([raw])),
            );
        }
        let mut json = String::new();
        encode_value(&ObjectValue::from_pairs(events).into(), &mut json, 0)
            .map_err(|error| self.invalid(&error))?;
        json.push('\n');
        let identifier = id(&["hook-fragment", self.bundle.id(), agent.as_str()]);
        self.write(
            &identifier,
            Family::Hooks,
            path,
            Content::File(FileContent::new(json.into_bytes(), FileMode::Private)),
            None,
        )?;
        let selected = self
            .routes("hooks")
            .into_iter()
            .filter(|routes| routes.iter().any(|route| route.agent == agent))
            .collect::<Vec<_>>();
        self.check_unselected("hooks", &name, &selected)?;
        for routes in selected {
            if routes.iter().any(|route| route.agent != agent) {
                return Err(self.fail("hook-shared-selector", "A shared hook selector cannot preserve separate harness ABIs. Declare separate arrays."));
            }
            self.entry(&routes, &name, Family::Hooks, &identifier)?;
        }
        Ok(identifier)
    }
    fn hooks(&mut self) -> Result<BTreeMap<String, Vec<String>>, LoadoutDiagnostic> {
        let originals: BTreeMap<_, _> = self
            .bundle
            .artifacts()
            .iter()
            .filter_map(|artifact| match artifact {
                Artifact::Hook(hook) => Some((hook.id(), hook)),
                _ => None,
            })
            .collect();
        let mut alias: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if originals.is_empty() {
            return Ok(alias);
        }
        for agent in &self.destination.agents {
            let context = self.context(*agent)?;
            let output = hook_artifacts_with_launcher(self.bundle, &context, &self.assets, true)
                .map_err(|error| match error {
                    HookTranslationError::Unsupported(issue) => self.unsupported(&issue),
                    HookTranslationError::Invalid(issue) => self.invalid(&issue),
                })?;
            let mut selected = Vec::new();
            for item in output.artifacts() {
                if let Some(hook) = originals.get(item.id()) {
                    selected.push((*hook, item));
                } else {
                    self.destination(item.destination().to_owned())?;
                    self.artifacts.push(item.clone());
                }
            }
            let identifier = if let Some((folder, _)) = module(*agent) {
                let item = selected
                    .first()
                    .map(|(_, item)| *item)
                    .ok_or_else(|| self.fail("hook-source", "No generated bridge"))?;
                let name = item
                    .destination()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| self.fail("hook-source", "Missing bridge filename"))?;
                let path = self.module_path(*agent, &format!("{folder}/{name}"))?;
                self.impact("module-config", &path, vec![*agent], false);
                let identifier = id(&["hook-module", self.bundle.id(), agent.as_str()]);
                self.write(
                    &identifier,
                    Family::Hooks,
                    path,
                    item.content().clone(),
                    None,
                )?;
                identifier
            } else {
                self.hook_fragment(*agent, &selected)?
            };
            for (hook, _) in selected {
                alias
                    .entry(hook.id().into())
                    .or_default()
                    .push(identifier.clone());
            }
            for edge in output.dependencies() {
                if originals.contains_key(edge.dependent()) {
                    self.edge(&identifier, edge.required(), edge.mode())?;
                } else if !originals.contains_key(edge.required()) {
                    self.edges.insert(edge.clone());
                }
            }
            self.notices.extend_from_slice(output.notices());
        }
        Ok(alias)
    }
}

pub(super) fn render(
    bundle: &Bundle,
    destination: &LoadoutDestination<'_>,
    options: LoadoutOptions<'_>,
    inputs: &mut Inputs,
    impacts: &mut Vec<LoadoutImpact>,
    prefix: &str,
) -> Result<RenderedBundle, LoadoutDiagnostic> {
    let mut state = Rendering {
        bundle,
        destination,
        options,
        inputs,
        impacts,
        artifacts: Vec::new(),
        edges: HashSet::new(),
        notices: Vec::new(),
        assets: HashMap::new(),
    };
    state.gate()?;
    let context = state.context(
        destination
            .agents
            .first()
            .copied()
            .ok_or_else(|| state.fail("disabled-harness", "No selected harness"))?,
    )?;
    let (assets, paths) =
        support_asset_artifacts(bundle, &context).map_err(|error| state.invalid(&error))?;
    for asset in &assets {
        state.destination(asset.destination().to_owned())?;
    }
    state.assets = paths;
    let mut servers = BTreeSet::new();
    for item in bundle.artifacts() {
        if let Artifact::Mcp(mcp) = item
            && !servers.insert(mcp.name())
        {
            return Err(state.fail(
                "source-collision",
                "A bundle may declare each MCP server once",
            ));
        }
    }
    let tables = if servers.is_empty() {
        Vec::new()
    } else {
        let tables = state.mcp_tables()?;
        state.check_mcp_permissions(&servers)?;
        tables
    };
    for item in bundle.artifacts() {
        match item {
            Artifact::Skill(skill) => state.skill(skill)?,
            Artifact::Instruction(instruction) => state.instruction(instruction)?,
            Artifact::Mcp(mcp) => state.mcp(mcp, &tables)?,
            Artifact::Native(artifact) => state.native(artifact)?,
            Artifact::Hook(_) => {}
        }
    }
    let aliases = state.hooks()?;
    for edge in bundle.dependencies() {
        let original_dependent = [edge.dependent().to_owned()];
        let original_required = [edge.required().to_owned()];
        let dependents = aliases
            .get(edge.dependent())
            .map_or(original_dependent.as_slice(), Vec::as_slice);
        let required = aliases
            .get(edge.required())
            .map_or(original_required.as_slice(), Vec::as_slice);
        for dependent in dependents {
            for requirement in required.iter().filter(|item| *item != dependent) {
                state.edge(dependent, requirement, edge.mode())?;
            }
        }
    }
    if !destination.accept_shared_scope
        && let Some(shared) = state.impacts.iter().find(|impact| impact.shared)
    {
        return Err(LoadoutDiagnostic::at(
            "shared-scope",
            "Approve the shared source and selector scope shown in the preview",
            &shared.path,
        ));
    }
    state.artifacts.extend(assets);
    state
        .notices
        .push(sync_notice(bundle.id()).map_err(|error| state.invalid(&error))?);
    scoped_render(state.artifacts, state.edges, state.notices, prefix).map_err(|error| {
        LoadoutDiagnostic::at("source-plan", error.message(), &destination.source.path)
    })
}

fn scoped_render(
    artifacts: Vec<RenderedArtifact>,
    edges: HashSet<Dependency>,
    notices: Vec<Notice>,
    prefix: &str,
) -> Result<RenderedBundle, ValueError> {
    let qualify = |name: &str| {
        prefix.to_owned() + &id(&["loadout", name]).chars().take(31).collect::<String>()
    };
    let edges = edges
        .into_iter()
        .map(|edge| {
            Dependency::with_mode(
                qualify(edge.dependent()),
                qualify(edge.required()),
                edge.mode(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let artifacts = artifacts.into_iter().map(|item| {
        let scoped_id = qualify(item.id());
        item.with_id(scoped_id)
    });
    RenderedBundle::with_notices(artifacts, edges, notices, [])
}
