use crate::artifacts::{
    Artifact, AssetRef, Audience, Command, CommandArgument, EnvRef, EnvironmentValue, Family,
    HookArtifact, HookEvent, HookOutcome, HttpTransport, InstructionArtifact, McpArtifact,
    McpTransport, NativeArtifact, SkillArtifact, SupportAsset,
};
use crate::bundle::BundleError;
use crate::content::{
    Content, DocumentFormat, FileContent, FileMode, Key, Member, Placement, Position,
    SectionBoundaries, SectionContent, Selector, SelectorPart, StructuredContent, TreeContent,
};
use crate::json::Json;
use crate::manifest::{ManifestObject, object, string};
use crate::memory::MemorySource;
use crate::models::{BundleEntry, BundleIdentity};
use crate::rendered::{Dependency, DependencyMode};
use crate::sources::{DirectorySource, ZipSource};
use crate::targets::{Agent, Platform, Surface, TargetScope};
use crate::validation::validate_relative_path;
use crate::values::{Value, ValueError, value_from_json, value_from_record};
use std::collections::HashSet;
use std::str::FromStr;

pub trait ConfigSource {
    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, BundleError>;
    fn tree_entries(
        &mut self,
        path: &str,
        name: &str,
        executables: &[String],
    ) -> Result<Vec<BundleEntry>, BundleError>;
}

impl ConfigSource for DirectorySource {
    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, BundleError> {
        Self::read_file(self, path)
    }

    fn tree_entries(
        &mut self,
        path: &str,
        name: &str,
        executables: &[String],
    ) -> Result<Vec<BundleEntry>, BundleError> {
        Self::tree_entries(self, path, name, executables)
    }
}

impl ConfigSource for ZipSource {
    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, BundleError> {
        Self::read_file(self, path)
    }

    fn tree_entries(
        &mut self,
        path: &str,
        name: &str,
        executables: &[String],
    ) -> Result<Vec<BundleEntry>, BundleError> {
        Self::tree_entries(self, path, name, executables)
    }
}

impl ConfigSource for MemorySource {
    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, BundleError> {
        Self::read_file(self, path)
    }

    fn tree_entries(
        &mut self,
        path: &str,
        name: &str,
        executables: &[String],
    ) -> Result<Vec<BundleEntry>, BundleError> {
        Self::tree_entries(self, path, name, executables)
    }
}

pub struct ConfigManifest {
    pub identity: BundleIdentity,
    pub artifacts: Vec<Artifact>,
    pub assets: Vec<SupportAsset>,
    pub dependencies: Vec<Dependency>,
}

fn invalid(message: impl Into<String>) -> BundleError {
    BundleError::Invalid(ValueError::new(message))
}

fn list(value: Option<&Json>) -> Result<&[Json], ValueError> {
    match value {
        None => Ok(&[]),
        Some(Json::Array(items)) => Ok(items),
        Some(_) => Err(ValueError::new("expected a JSON array")),
    }
}

fn source_path(value: Option<&Json>) -> Result<String, ValueError> {
    let path = string(value, "source path")?;
    validate_relative_path(&path, "source path")?;
    Ok(path)
}

fn text(fields: &ManifestObject<'_>, source: &mut dyn ConfigSource) -> Result<String, BundleError> {
    let has_text = fields.get("text").is_some();
    let has_path = fields.get("path").is_some();
    if has_text == has_path {
        return Err(invalid("provide exactly one of text or path"));
    }
    if let Some(value) = fields.get("text") {
        return Ok(string(Some(value), "text")?);
    }
    let path = source_path(fields.get("path"))?;
    let data = source.read_file(&path)?;
    String::from_utf8(data).map_err(|_| invalid("text is not valid UTF-8"))
}

fn tree(
    fields: &ManifestObject<'_>,
    source: &mut dyn ConfigSource,
) -> Result<TreeContent, BundleError> {
    let executables: Vec<String> = list(fields.get("executables"))?
        .iter()
        .map(|item| source_path(Some(item)))
        .collect::<Result<_, _>>()?;
    if executables
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>()
        .len()
        != executables.len()
    {
        return Err(invalid("duplicate executable path"));
    }
    let path = source_path(fields.get("path"))?;
    let entries = source.tree_entries(&path, "content", &executables)?;
    let mut rebased = Vec::with_capacity(entries.len());
    for entry in &entries {
        if entry.path() == "content" {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix("content/")
            .unwrap_or_else(|| entry.path());
        rebased.push(BundleEntry::new(
            relative,
            entry.data().map(<[u8]>::to_vec),
            entry.executable(),
        )?);
    }
    let content = TreeContent::new(rebased)?;
    let files: HashSet<&str> = content
        .entries()
        .iter()
        .filter(|entry| !entry.is_directory())
        .map(BundleEntry::path)
        .collect();
    if executables
        .iter()
        .any(|executable| !files.contains(executable.as_str()))
    {
        return Err(invalid("every executable must name an existing file"));
    }
    Ok(content)
}

fn argument(value: &Json) -> Result<CommandArgument, ValueError> {
    if value.as_object().is_none() {
        return Ok(CommandArgument::Text(string(
            Some(value),
            "command argument",
        )?));
    }
    let fields = object(value, &["asset"], &["path"])?;
    let asset_id = string(fields.get("asset"), "referenced asset id")?;
    match fields.get("path") {
        None => AssetRef::new(asset_id).map(CommandArgument::Asset),
        Some(path) => {
            AssetRef::with_path(asset_id, source_path(Some(path))?).map(CommandArgument::Asset)
        }
    }
}

fn command(value: &Json) -> Result<Command, BundleError> {
    let fields = object(value, &["argv"], &["env", "cwd"])?;
    let env = match fields.get("env") {
        None => Vec::new(),
        Some(Json::Object(items)) => {
            let mut env: Vec<(String, EnvironmentValue)> = Vec::with_capacity(items.len());
            for (key, item) in items {
                if item.as_object().is_some() {
                    let reference = object(item, &["env"], &[])?;
                    env.push((
                        key.clone(),
                        EnvironmentValue::Reference(EnvRef::new(string(
                            reference.get("env"),
                            "environment reference",
                        )?)?),
                    ));
                } else {
                    env.push((
                        key.clone(),
                        EnvironmentValue::Text(string(Some(item), "environment value")?),
                    ));
                }
            }
            env
        }
        Some(_) => return Err(invalid("command env must be a JSON object")),
    };
    let argv: Vec<CommandArgument> = list(fields.get("argv"))?
        .iter()
        .map(argument)
        .collect::<Result<Vec<_>, _>>()?;
    let cwd = match fields.get("cwd") {
        Some(value) => Some(argument(value)?),
        None => None,
    };
    Ok(Command::new(argv, env, cwd)?)
}

fn member(value: &Json) -> Result<Member, ValueError> {
    let fields = object(value, &[], &["identity", "semantic_identity", "key"])?;
    let identity = config_value(&fields, "identity", "semantic_identity")?;
    let key = list(fields.get("key"))?
        .iter()
        .map(|name| string(Some(name), "member key"))
        .collect::<Result<Vec<_>, _>>()?;
    Member::new(identity, key)
}

fn config_value(
    fields: &ManifestObject<'_>,
    plain: &str,
    typed: &str,
) -> Result<Value, ValueError> {
    if fields.get(plain).is_some() == fields.get(typed).is_some() {
        return Err(ValueError::new(format!(
            "provide exactly one of {plain} or {typed}"
        )));
    }
    if let Some(value) = fields.get(plain) {
        return value_from_json(value);
    }
    fields.get(typed).map_or_else(
        || {
            Err(ValueError::new(format!(
                "provide exactly one of {plain} or {typed}"
            )))
        },
        value_from_record,
    )
}

fn selector(value: &Json) -> Result<Selector, ValueError> {
    let parts: Vec<SelectorPart> = list(Some(value))?
        .iter()
        .map(|item| {
            if let Some(name) = item.as_str() {
                Ok(SelectorPart::Key(Key::new(name)?))
            } else {
                member(item).map(SelectorPart::Member)
            }
        })
        .collect::<Result<_, _>>()?;
    Selector::new(parts)
}

fn content(value: &Json, source: &mut dyn ConfigSource) -> Result<Content, BundleError> {
    object(
        value,
        &["kind"],
        &[
            "path",
            "text",
            "mode",
            "executables",
            "marker",
            "format",
            "selector",
            "value",
            "semantic_value",
            "placement",
            "boundaries",
        ],
    )?;
    match value.get("kind").and_then(Json::as_str) {
        Some("file") => {
            let fields = object(value, &["kind", "path"], &["mode"])?;
            let path = source_path(fields.get("path"))?;
            let data = source.read_file(&path)?;
            let mode = match fields.get("mode") {
                Some(mode) => FileMode::from_str(&string(Some(mode), "file mode")?)?,
                None => FileMode::Private,
            };
            Ok(Content::File(FileContent::new(data, mode)))
        }
        Some("tree") => {
            let fields = object(value, &["kind", "path"], &["executables"])?;
            Ok(Content::Tree(tree(&fields, source)?))
        }
        Some("section") => {
            let fields = object(value, &["kind", "marker"], &["text", "path", "boundaries"])?;
            let boundaries = match fields.get("boundaries") {
                Some(raw) => {
                    let raw_boundaries = object(raw, &["start", "end"], &[])?;
                    Some(SectionBoundaries::new(
                        string(raw_boundaries.get("start"), "start boundary")?,
                        string(raw_boundaries.get("end"), "end boundary")?,
                    )?)
                }
                None => None,
            };
            let marker = string(fields.get("marker"), "section marker")?;
            let section_text = text(&fields, source)?;
            Ok(Content::Section(SectionContent::new(
                marker,
                section_text,
                boundaries,
            )?))
        }
        Some("structured") => {
            let fields = object(
                value,
                &["kind", "format", "selector"],
                &["value", "semantic_value", "placement"],
            )?;
            let placement = match fields.get("placement") {
                Some(raw) => {
                    let raw_placement = object(raw, &["position"], &["anchor"])?;
                    let position = Position::from_str(&string(
                        raw_placement.get("position"),
                        "placement position",
                    )?)?;
                    let anchor = match raw_placement.get("anchor") {
                        Some(raw) => Some(member(raw)?),
                        None => None,
                    };
                    Some(Placement::new(position, anchor)?)
                }
                None => None,
            };
            let format =
                DocumentFormat::from_str(&string(fields.get("format"), "document format")?)?;
            let Some(selector_value) = fields.get("selector") else {
                return Err(invalid("structured content requires a selector"));
            };
            let selector = selector(selector_value)?;
            let value = config_value(&fields, "value", "semantic_value")?;
            Ok(Content::Structured(StructuredContent::new(
                format, selector, value, placement,
            )?))
        }
        _ => Err(invalid("unknown native content kind")),
    }
}

fn artifact(value: &Json, source: &mut dyn ConfigSource) -> Result<Artifact, BundleError> {
    let header = object(
        value,
        &["kind", "id"],
        &[
            "name",
            "path",
            "executables",
            "text",
            "transport",
            "event",
            "command",
            "timeout_ms",
            "outcomes",
            "tools",
            "protocol_version",
            "family",
            "audience",
            "destination",
            "content",
        ],
    )?;
    let identifier = string(header.get("id"), "artifact id")?;
    match header.get("kind").and_then(Json::as_str) {
        Some("skill") => {
            let fields = object(value, &["kind", "id", "name", "path"], &["executables"])?;
            let name = string(fields.get("name"), "skill name")?;
            let content = tree(&fields, source)?;
            Ok(Artifact::Skill(SkillArtifact::new(
                identifier, name, content,
            )?))
        }
        Some("instruction") => {
            let fields = object(value, &["kind", "id"], &["text", "path"])?;
            let instruction_text = text(&fields, source)?;
            Ok(Artifact::Instruction(InstructionArtifact::new(
                identifier,
                instruction_text,
            )?))
        }
        Some("mcp") => mcp_artifact(value, identifier),
        Some("hook") => hook_artifact(value, identifier),
        Some("native") => native_artifact(value, identifier, source),
        _ => Err(invalid("unknown artifact kind")),
    }
}

fn mcp_artifact(value: &Json, identifier: String) -> Result<Artifact, BundleError> {
    let fields = object(value, &["kind", "id", "name", "transport"], &[])?;
    let Some(transport_value) = fields.get("transport") else {
        return Err(invalid("MCP artifacts require a transport"));
    };
    object(
        transport_value,
        &["kind"],
        &["command", "url", "bearer_token_env"],
    )?;
    let transport = match transport_value.get("kind").and_then(Json::as_str) {
        Some("stdio") => {
            let transport = object(transport_value, &["kind", "command"], &[])?;
            let Some(command_value) = transport.get("command") else {
                return Err(invalid("stdio transports require a command"));
            };
            McpTransport::Command(command(command_value)?)
        }
        Some("http") => {
            let transport = object(transport_value, &["kind", "url"], &["bearer_token_env"])?;
            let url = string(transport.get("url"), "MCP URL")?;
            let bearer_token = match transport.get("bearer_token_env") {
                Some(value) => Some(EnvRef::new(string(
                    Some(value),
                    "bearer environment reference",
                )?)?),
                None => None,
            };
            McpTransport::Http(HttpTransport::new(url, bearer_token)?)
        }
        _ => return Err(invalid("unknown MCP transport")),
    };
    let name = string(fields.get("name"), "MCP server name")?;
    Ok(Artifact::Mcp(McpArtifact::new(
        identifier, name, transport,
    )?))
}

fn hook_artifact(value: &Json, identifier: String) -> Result<Artifact, BundleError> {
    let fields = object(
        value,
        &["kind", "id", "event", "command", "protocol_version"],
        &["timeout_ms", "outcomes", "tools"],
    )?;
    if fields.get("protocol_version").and_then(Json::as_int) != Some(1) {
        return Err(invalid("hook protocol_version must be integer 1"));
    }
    let timeout_ms = match fields.get("timeout_ms") {
        Some(value) => match value.as_int() {
            Some(number) => i32::try_from(number)
                .map_err(|_| invalid("hook timeout must be 1-300000 integer milliseconds"))?,
            None => return Err(invalid("hook timeout must be an integer")),
        },
        None => 10_000,
    };
    let event = HookEvent::from_str(&string(fields.get("event"), "hook event")?)?;
    let Some(command_value) = fields.get("command") else {
        return Err(invalid("hooks require a command"));
    };
    let command = command(command_value)?;
    let outcomes = match fields.get("outcomes") {
        Some(value) => list(Some(value))?
            .iter()
            .map(|item| HookOutcome::from_str(&string(Some(item), "hook outcome")?))
            .collect::<Result<_, _>>()?,
        None => vec![HookOutcome::Continue],
    };
    let tools = match fields.get("tools") {
        Some(value) => list(Some(value))?
            .iter()
            .map(|item| string(Some(item), "hook tool"))
            .collect::<Result<_, _>>()?,
        None => Vec::new(),
    };
    Ok(Artifact::Hook(HookArtifact::new(
        identifier, event, command, timeout_ms, outcomes, tools,
    )?))
}

fn native_artifact(
    value: &Json,
    identifier: String,
    source: &mut dyn ConfigSource,
) -> Result<Artifact, BundleError> {
    let fields = object(
        value,
        &["kind", "id", "family", "audience", "destination", "content"],
        &[],
    )?;
    let Some(audience_value) = fields.get("audience") else {
        return Err(invalid("native artifacts require an audience"));
    };
    let audience_fields = object(
        audience_value,
        &["agent", "scope"],
        &["surface", "platform"],
    )?;
    let family = Family::from_str(&string(fields.get("family"), "native family")?)?;
    let agent = Agent::from_str(&string(audience_fields.get("agent"), "agent")?)?;
    let scope = TargetScope::from_str(&string(audience_fields.get("scope"), "scope")?)?;
    let surface = match audience_fields.get("surface") {
        Some(value) => Surface::from_str(&string(Some(value), "surface")?)?,
        None => Surface::Cli,
    };
    let platform = match audience_fields.get("platform") {
        Some(value) => Some(Platform::from_str(&string(Some(value), "platform")?)?),
        None => None,
    };
    let audience = Audience::new(agent, scope, surface, platform)?;
    let destination = source_path(fields.get("destination"))?;
    let Some(content_value) = fields.get("content") else {
        return Err(invalid("native artifacts require content"));
    };
    let native_content = content(content_value, source)?;
    Ok(Artifact::Native(NativeArtifact::new(
        identifier,
        family,
        audience,
        destination,
        native_content,
    )?))
}

pub fn read_config_manifest(
    raw: &Json,
    source: &mut dyn ConfigSource,
) -> Result<ConfigManifest, BundleError> {
    let fields = object(
        raw,
        &["schema_version", "id", "version", "artifacts"],
        &["assets", "dependencies"],
    )?;
    if fields.get("schema_version").and_then(Json::as_int) != Some(2) {
        return Err(invalid("configuration schema_version must be integer 2"));
    }
    let artifacts = list(fields.get("artifacts"))?
        .iter()
        .map(|item| artifact(item, source))
        .collect::<Result<_, _>>()?;
    let assets = list(fields.get("assets"))?
        .iter()
        .map(|item| support_asset(item, source))
        .collect::<Result<_, _>>()?;
    let dependencies = list(fields.get("dependencies"))?
        .iter()
        .map(dependency)
        .collect::<Result<_, _>>()?;
    let identity = BundleIdentity::new(
        string(fields.get("id"), "bundle id")?,
        string(fields.get("version"), "version")?,
    )?;
    Ok(ConfigManifest {
        identity,
        artifacts,
        assets,
        dependencies,
    })
}

fn support_asset(value: &Json, source: &mut dyn ConfigSource) -> Result<SupportAsset, BundleError> {
    let fields = object(value, &["id", "family", "path"], &["executables"])?;
    let id = string(fields.get("id"), "asset id")?;
    let family = Family::from_str(&string(fields.get("family"), "asset family")?)?;
    let content = tree(&fields, source)?;
    Ok(SupportAsset::new(id, family, content)?)
}

fn dependency(value: &Json) -> Result<Dependency, BundleError> {
    let fields = object(value, &["dependent", "required"], &["mode"])?;
    let dependent = string(fields.get("dependent"), "dependent")?;
    let required = string(fields.get("required"), "required")?;
    let mode = match fields.get("mode") {
        Some(value) => DependencyMode::from_str(&string(Some(value), "dependency mode")?)?,
        None => DependencyMode::Revision,
    };
    Dependency::with_mode(dependent, required, mode).map_err(BundleError::from)
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
    use super::read_config_manifest;
    use crate::artifacts::{Artifact, CommandArgument, EnvRef, EnvironmentValue, McpTransport};
    use crate::bundle::BundleError;
    use crate::content::{Content, FileMode, SelectorPart};
    use crate::json::Json;
    use crate::memory::MemorySource;
    use crate::models::BundleEntry;
    use crate::targets::{Agent, Surface, TargetScope};

    fn message(result: Result<crate::config_manifest::ConfigManifest, BundleError>) -> String {
        match result {
            Ok(_) => panic!("expected an error"),
            Err(BundleError::Invalid(value)) => value.message().to_owned(),
            Err(BundleError::Io(error)) => panic!("expected invalid input, got {error}"),
        }
    }

    fn load(
        manifest: &str,
        entries: impl IntoIterator<Item = BundleEntry>,
    ) -> Result<crate::config_manifest::ConfigManifest, BundleError> {
        let raw = Json::parse(manifest).expect("test manifest must be valid JSON");
        let mut source = MemorySource::new(manifest.as_bytes().to_vec(), entries)?;
        read_config_manifest(&raw, &mut source)
    }

    fn manifest(artifacts: &str) -> String {
        format!(r#"{{"schema_version":2,"id":"team","version":"1","artifacts":{artifacts}}}"#)
    }

    #[test]
    fn parses_instructions_from_text_or_source_paths() {
        let parsed = load(
            &manifest(r#"[{"kind":"instruction","id":"guide","text":"Hello"}]"#),
            [],
        )
        .unwrap();
        assert_eq!(parsed.identity.id(), "team");
        assert_eq!(parsed.artifacts.len(), 1);
        assert!(parsed.assets.is_empty());
        assert!(parsed.dependencies.is_empty());

        let sourced = load(
            &manifest(r#"[{"kind":"instruction","id":"guide","path":"first.md"}]"#),
            [BundleEntry::file("first.md", b"Hello".to_vec()).unwrap()],
        )
        .unwrap();
        assert_eq!(sourced.artifacts, parsed.artifacts);

        assert_eq!(
            message(load(
                &manifest(r#"[{"kind":"instruction","id":"guide"}]"#),
                []
            )),
            "provide exactly one of text or path"
        );
        assert_eq!(
            message(load(
                &manifest(r#"[{"kind":"instruction","id":"guide","text":"a","path":"b"}]"#),
                []
            )),
            "provide exactly one of text or path"
        );
        assert_eq!(
            message(load(
                &manifest(r#"[{"kind":"instruction","id":"guide","path":"missing.md"}]"#),
                []
            )),
            "expected a source file: missing.md"
        );
        assert_eq!(
            message(load(
                &manifest(r#"[{"kind":"instruction","id":"guide","path":"../escape"}]"#),
                []
            )),
            "source path must be an unambiguous relative path"
        );
        assert_eq!(
            message(load(
                &manifest(r#"[{"kind":"instruction","id":"guide","text":"a","unknown":true}]"#),
                []
            )),
            "expected fields ['id', 'kind']; optional fields ['audience', 'command', 'content', 'destination', 'event', 'executables', 'family', 'name', 'outcomes', 'path', 'protocol_version', 'text', 'timeout_ms', 'tools', 'transport']"
        );
    }

    #[test]
    fn parses_skill_trees_and_validates_executables() {
        let entries = [
            BundleEntry::file("skills/review/SKILL.md", b"markdown".to_vec()).unwrap(),
            BundleEntry::file("skills/review/run.py", b"print(1)".to_vec()).unwrap(),
            BundleEntry::directory("skills/review/empty").unwrap(),
        ];
        let parsed = load(
            &manifest(
                r#"[{"kind":"skill","id":"review-skill","name":"review","path":"skills/review","executables":["run.py"]}]"#,
            ),
            entries,
        )
        .unwrap();
        let Artifact::Skill(skill) = &parsed.artifacts[0] else {
            panic!("expected a skill artifact");
        };
        assert_eq!(skill.name(), "review");
        assert_eq!(
            skill
                .content()
                .entries()
                .iter()
                .map(BundleEntry::path)
                .collect::<Vec<_>>(),
            ["SKILL.md", "empty", "run.py"]
        );
        assert!(skill.content().entries()[2].executable());
        assert_eq!(
            message(load(
                &manifest(
                    r#"[{"kind":"skill","id":"s","name":"review","path":"skills/review","executables":["missing.py"]}]"#
                ),
                [BundleEntry::file("skills/review/SKILL.md", b"m".to_vec()).unwrap(),]
            )),
            "every executable must name an existing file"
        );
        assert_eq!(
            message(load(
                &manifest(
                    r#"[{"kind":"skill","id":"s","name":"review","path":"skills/review","executables":["run.py","run.py"]}]"#
                ),
                []
            )),
            "duplicate executable path"
        );
        assert_eq!(
            message(load(
                &manifest(r#"[{"kind":"skill","id":"s","name":"review","path":"missing"}]"#),
                []
            )),
            "skill source must be an existing directory: missing"
        );
    }

    #[test]
    fn parses_mcp_transports_and_hooks() {
        let parsed = load(
            &manifest(
                concat!(
                    r#"[{"kind":"mcp","id":"local","name":"local","transport":{"kind":"stdio","#,
                    r#""command":{"argv":["python","server.py"],"env":{"API_TOKEN":{"env":"TOOLS_TOKEN"},"MODE":"local"},"cwd":"tools"}}},"#,
                    r#"{"kind":"mcp","id":"remote","name":"remote","transport":{"kind":"http","#,
                    r#""url":"https://tools.example.test/mcp","bearer_token_env":"REMOTE_TOKEN"}}]"#
                ),
            ),
            [],
        )
        .unwrap();
        let Artifact::Mcp(local) = &parsed.artifacts[0] else {
            panic!("expected an MCP artifact");
        };
        let McpTransport::Command(command) = local.transport() else {
            panic!("expected a command transport");
        };
        assert_eq!(
            command.argv(),
            &[
                CommandArgument::from("python"),
                CommandArgument::from("server.py"),
            ]
        );
        assert_eq!(
            command.env(),
            &[
                (
                    "API_TOKEN".to_owned(),
                    EnvironmentValue::from(EnvRef::new("TOOLS_TOKEN").unwrap())
                ),
                ("MODE".to_owned(), EnvironmentValue::from("local")),
            ]
        );
        assert_eq!(command.cwd(), Some(&CommandArgument::from("tools")));
        let Artifact::Mcp(remote) = &parsed.artifacts[1] else {
            panic!("expected an MCP artifact");
        };
        let McpTransport::Http(transport) = remote.transport() else {
            panic!("expected an HTTP transport");
        };
        assert_eq!(transport.url(), "https://tools.example.test/mcp");

        let hook = load(
            &manifest(concat!(
                r#"[{"kind":"hook","id":"check","event":"before-tool","protocol_version":1,"#,
                r#""command":{"argv":["python","check.py"]},"outcomes":["continue","block"],"#,
                r#""tools":["shell"]}]"#
            )),
            [],
        )
        .unwrap();
        let Artifact::Hook(hook) = &hook.artifacts[0] else {
            panic!("expected a hook artifact");
        };
        assert_eq!(hook.timeout_ms(), 10_000);
        assert_eq!(hook.protocol_version(), 1);
        assert_eq!(hook.outcomes().len(), 2);
        let defaulted = load(
            &manifest(r#"[{"kind":"hook","id":"check","event":"stop","command":{"argv":["test"]},"protocol_version":1}]"#),
            [],
        )
        .unwrap();
        let Artifact::Hook(hook) = &defaulted.artifacts[0] else {
            panic!("expected a hook artifact");
        };
        assert_eq!(hook.outcomes(), &[crate::artifacts::HookOutcome::Continue]);
        assert!(hook.tools().is_empty());
        assert_eq!(
            message(load(
                &manifest(
                    r#"[{"kind":"hook","id":"h","event":"stop","command":{"argv":["t"]},"protocol_version":true}]"#
                ),
                []
            )),
            "hook protocol_version must be integer 1"
        );
        assert_eq!(
            message(load(
                &manifest(
                    r#"[{"kind":"hook","id":"h","event":"stop","command":{"argv":["t"]},"protocol_version":1,"timeout_ms":true}]"#
                ),
                []
            )),
            "hook timeout must be an integer"
        );
        assert_eq!(
            message(load(
                &manifest(r#"[{"kind":"mcp","id":"s","name":"s","transport":{"kind":"other"}}]"#),
                []
            )),
            "unknown MCP transport"
        );
        assert_eq!(
            message(load(
                &manifest(
                    r#"[{"kind":"mcp","id":"s","name":"s","transport":{"kind":"stdio","command":{"argv":[],"env":[]}}}]"#
                ),
                []
            )),
            "command env must be a JSON object"
        );
    }

    #[test]
    fn parses_native_content_of_every_kind() {
        let entries = [
            BundleEntry::file("native.md", b"# Native\n".to_vec()).unwrap(),
            BundleEntry::file("skills/review/SKILL.md", b"markdown".to_vec()).unwrap(),
        ];
        let parsed = load(
            &manifest(
                concat!(
                    r#"[{"kind":"native","id":"native-guide","family":"instructions","#,
                    r#""audience":{"agent":"claude","scope":"project"},"destination":"guidance/custom.md","#,
                    r#""content":{"kind":"file","path":"native.md","mode":"readable"}},"#,
                    r#"{"kind":"native","id":"native-section","family":"instructions","#,
                    r#""audience":{"agent":"codex","scope":"project"},"destination":"AGENTS.md","#,
                    r#""content":{"kind":"section","marker":"guide","text":"Use the project checks.\n"}},"#,
                    r#"{"kind":"native","id":"native-array","family":"hooks","#,
                    r#""audience":{"agent":"claude","scope":"user"},"destination":".claude/settings.json","#,
                    r#""content":{"kind":"structured","format":"json","selector":["hooks","PreToolUse","#,
                    r#"{"identity":{"command":"check"}}],"value":{"command":"check"},"#,
                    r#""placement":{"position":"after","anchor":{"identity":{"command":"existing"}}}}},"#,
                    r#"{"kind":"native","id":"native-key","family":"mcp","#,
                    r#""audience":{"agent":"codex","scope":"user","platform":"linux"},"destination":".codex/config.toml","#,
                    r#""content":{"kind":"structured","format":"toml","selector":["mcp_servers","review"],"#,
                    r#""value":{"command":"review"}}},"#,
                    r#"{"kind":"native","id":"native-tree","family":"skills","#,
                    r#""audience":{"agent":"cursor","scope":"project","surface":"ide"},"destination":".cursor/skills/review","#,
                    r#""content":{"kind":"tree","path":"skills/review"}}]"#
                ),
            ),
            entries,
        )
        .unwrap();
        let Artifact::Native(guide) = &parsed.artifacts[0] else {
            panic!("expected a native artifact");
        };
        assert_eq!(guide.audience().agent(), Agent::Claude);
        let Content::File(file) = guide.content() else {
            panic!("expected file content");
        };
        assert_eq!(file.data(), b"# Native\n".as_slice());
        assert_eq!(file.mode(), FileMode::Readable);
        let Artifact::Native(section) = &parsed.artifacts[1] else {
            panic!("expected a native artifact");
        };
        let Content::Section(section) = section.content() else {
            panic!("expected section content");
        };
        assert_eq!(section.text(), "Use the project checks.\n");
        assert_eq!(
            section.selector().unwrap().start(),
            "<!-- flyrail:guide:start -->"
        );
        let Artifact::Native(array) = &parsed.artifacts[2] else {
            panic!("expected a native artifact");
        };
        let Content::Structured(structured) = array.content() else {
            panic!("expected structured content");
        };
        let [
            SelectorPart::Key(hooks),
            SelectorPart::Key(pre_tool),
            SelectorPart::Member(member),
        ] = structured.selector().parts()
        else {
            panic!("expected a hooks selector");
        };
        assert_eq!(hooks.name(), "hooks");
        assert_eq!(pre_tool.name(), "PreToolUse");
        assert_eq!(
            member.key().len(),
            0,
            "member key defaults to an empty sequence"
        );
        assert_eq!(
            structured
                .placement()
                .map(|placement| placement.position().as_str()),
            Some("after")
        );
        let Artifact::Native(keyed) = &parsed.artifacts[3] else {
            panic!("expected a native artifact");
        };
        let Content::Structured(structured) = keyed.content() else {
            panic!("expected structured content");
        };
        assert_eq!(structured.format().as_str(), "toml");
        assert_eq!(
            keyed.audience().platform(),
            Some(crate::targets::Platform::Linux)
        );
        let Artifact::Native(native_tree) = &parsed.artifacts[4] else {
            panic!("expected a native artifact");
        };
        let Content::Tree(tree) = native_tree.content() else {
            panic!("expected tree content");
        };
        assert_eq!(tree.entries()[0].path(), "SKILL.md");
        assert_eq!(native_tree.audience().surface(), Surface::Ide);
        assert_eq!(native_tree.audience().scope(), TargetScope::Project);
    }

    #[test]
    fn structured_values_are_plain_or_semantic_but_never_both() {
        let native = concat!(
            r#"[{"kind":"native","id":"native-key","family":"mcp","#,
            r#""audience":{"agent":"codex","scope":"user"},"destination":".codex/config.toml","#,
            r#""content":{"kind":"structured","format":"toml","selector":["mcp_servers","review"],"#,
        );
        let semantic = load(
            &manifest(&[native, r#""semantic_value":["date","2026-01-02"]}}]"#].concat()),
            [],
        )
        .unwrap();
        let Artifact::Native(semantic_native) = &semantic.artifacts[0] else {
            panic!("expected a native artifact");
        };
        let Content::Structured(structured) = semantic_native.content() else {
            panic!("expected structured content");
        };
        assert_eq!(
            structured.value().semantic_bytes(),
            crate::values::Value::Scalar(crate::values::Scalar::date(
                crate::temporal::Date::new(2026, 1, 2).unwrap(),
            ))
            .semantic_bytes()
        );
        assert_eq!(
            message(load(
                &manifest(&[native, r#""value":1,"semantic_value":["integer","1"]}}]"#,].concat()),
                []
            )),
            "provide exactly one of value or semantic_value"
        );
        let plain = load(&manifest(&[native, r#""value":1}}]"#].concat()), []).unwrap();
        let Artifact::Native(plain_native) = &plain.artifacts[0] else {
            panic!("expected a native artifact");
        };
        let Content::Structured(structured) = plain_native.content() else {
            panic!("expected structured content");
        };
        assert_eq!(structured.value(), &crate::values::Value::from(1_i64));
        let membered = concat!(
            r#"[{"kind":"native","id":"m","family":"mcp","#,
            r#""audience":{"agent":"codex","scope":"user"},"destination":"c.toml","#,
            r#""content":{"kind":"structured","format":"toml","selector":["mcp_servers","#,
            r#"{"identity":"review","key":["name"]}],"#,
            r#""semantic_value":["object",[["command",["string","run"]],["name",["string","review"]]]]}"#,
        );
        let parsed = load(&manifest(&[membered, r"}]"].concat()), []).unwrap();
        let Artifact::Native(native) = &parsed.artifacts[0] else {
            panic!("expected a native artifact");
        };
        let Content::Structured(structured) = native.content() else {
            panic!("expected structured content");
        };
        let [_, SelectorPart::Member(member)] = structured.selector().parts() else {
            panic!("expected a member selector part");
        };
        assert_eq!(
            member.identity(),
            &crate::values::Value::Scalar(crate::values::Scalar::text("review"))
        );
        assert_eq!(member.key(), &["name".to_owned()]);
    }

    #[test]
    fn parses_assets_dependencies_and_enforces_scope() {
        let manifest = concat!(
            r#"{"schema_version":2,"id":"team","version":"1","#,
            r#""artifacts":[{"kind":"instruction","id":"guide","text":"Hello"}],"#,
            r#""assets":[{"id":"hook-support","family":"hooks","path":"support"}],"#,
            r#""dependencies":[{"dependent":"guide","required":"hook-support"}]}"#
        );
        let parsed = load(
            manifest,
            [BundleEntry::file("support/check.py", b"pass\n".to_vec()).unwrap()],
        )
        .unwrap();
        assert_eq!(parsed.assets.len(), 1);
        assert_eq!(parsed.assets[0].id(), "hook-support");
        assert_eq!(parsed.dependencies[0].dependent(), "guide");

        assert_eq!(
            message(load(
                r#"{"schema_version":2.0,"id":"team","version":"1","artifacts":[{"kind":"instruction","id":"guide","text":"x"}]}"#,
                []
            )),
            "configuration schema_version must be integer 2"
        );
        assert_eq!(
            message(load(
                r#"{"schema_version":2,"id":"team","version":"1","artifacts":[],"permissions":{}}"#,
                []
            )),
            "expected fields ['artifacts', 'id', 'schema_version', 'version']; optional fields ['assets', 'dependencies']"
        );
        assert_eq!(
            message(load(
                r#"{"schema_version":2,"id":"team","version":"1","artifacts":{}}"#,
                []
            )),
            "expected a JSON array"
        );
        assert_eq!(
            message(load(
                r#"{"schema_version":2,"id":"team","version":"1","artifacts":[{"kind":"settings","id":"settings"}]}"#,
                []
            )),
            "unknown artifact kind"
        );
        assert_eq!(
            message(load(
                concat!(
                    r#"{"schema_version":2,"id":"team","version":"1","#,
                    r#""artifacts":[{"kind":"native","id":"n","family":"instructions","#,
                    r#""audience":{"agent":"codex","scope":"project"},"destination":"AGENTS.md","#,
                    r#""content":{"kind":"yaml"}}]}"#
                ),
                []
            )),
            "unknown native content kind"
        );
        assert_eq!(
            message(load(
                concat!(
                    r#"{"schema_version":2,"id":"team","version":"1","#,
                    r#""artifacts":[{"kind":"native","id":"n","family":"instructions","#,
                    r#""audience":{"agent":"codex","scope":"directory"},"destination":"AGENTS.md","#,
                    r#""content":{"kind":"section","marker":"guide","text":"hi"}}]}"#
                ),
                []
            )),
            "native audiences require project or user scope"
        );
    }
}
