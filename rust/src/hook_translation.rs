use crate::artifacts::{
    Artifact, CommandArgument, EnvironmentValue, Family, HookArtifact, HookEvent, HookOutcome,
    NativeArtifact, SupportAsset,
};
use crate::bundle::Bundle;
use crate::content::{
    Content, DocumentFormat, DocumentSchema, FileContent, FileMode, Key, Member, Selector,
    SelectorPart, StructuredContent, TreeContent,
};
use crate::destinations::{
    RenderContext, require_translatable, untranslatable_message, user_directory,
};
use crate::hooks::{
    EVENT_TIMEOUT_LIMIT_MS, HANDLER_LIMIT, HookConfig, HookEnvironment, HookHandler, HookShell,
};
use crate::json::Json;
use crate::mcp_translation::UnsupportedTranslation;
use crate::models::BundleEntry;
use crate::rendered::{
    Dependency, DependencyMode, ModelValue, Notice, NoticeKind, RenderedArtifact,
};
use crate::targets::{Agent, Platform, Surface, TargetScope};
use crate::values::{ArrayValue, ObjectValue, Scalar, Value, ValueError};
use sha2::{Digest as _, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookTranslation {
    artifacts: Vec<RenderedArtifact>,
    dependencies: Vec<Dependency>,
    notices: Vec<Notice>,
}

impl HookTranslation {
    pub fn artifacts(&self) -> &[RenderedArtifact] {
        &self.artifacts
    }
    pub fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }
    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }
}

fn identifier(values: &[&str]) -> String {
    let mut encoded = String::new();
    Json::Array(values.iter().map(|v| Json::Str((*v).to_owned())).collect()).encode(&mut encoded);
    Sha256::digest(encoded.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn hex_digest<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part);
    }
    digest
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn file_url(path: &Path, platform: Platform) -> Result<String, ValueError> {
    let text = path
        .to_str()
        .ok_or_else(|| ValueError::new("hook import path must be UTF-8"))?;
    let normalized = if platform == Platform::Windows {
        text.replace('\\', "/")
    } else {
        text.to_owned()
    };
    let (prefix, path_part) = if platform == Platform::Windows && normalized.starts_with("//") {
        ("file:", normalized.as_str())
    } else if platform == Platform::Windows
        && normalized.as_bytes().get(1) == Some(&b':')
        && normalized
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
    {
        ("file:///", normalized.as_str())
    } else if normalized.starts_with('/') {
        ("file://", normalized.as_str())
    } else {
        return Err(ValueError::new("hook import path must be absolute"));
    };
    let mut encoded = String::from(prefix);
    for (index, byte) in path_part.bytes().enumerate() {
        let drive_colon = platform == Platform::Windows
            && index == 1
            && path_part.as_bytes().get(1) == Some(&b':');
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) || drive_colon {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    Ok(encoded)
}

pub fn support_asset_artifacts(
    bundle: &Bundle,
    context: &RenderContext,
) -> Result<(Vec<RenderedArtifact>, HashMap<String, PathBuf>), ValueError> {
    let root = context
        .asset_root()
        .map_or_else(|| context.root().join(".flyrail-assets"), Path::to_path_buf);
    let mut rendered = Vec::new();
    let mut destinations = HashMap::new();
    for asset in bundle.assets() {
        let digest = hex_digest([asset.content().model_value().semantic_bytes().as_slice()]);
        let destination = root.join(bundle.id()).join(asset.id()).join(digest);
        destinations.insert(asset.id().to_owned(), destination.clone());
        rendered.push(RenderedArtifact::new(
            asset.id(),
            asset.family(),
            destination,
            Content::Tree(asset.content().clone()),
            None,
            false,
            Vec::new(),
        )?);
    }
    Ok((rendered, destinations))
}

pub fn translate_native_artifact(
    artifact: &NativeArtifact,
    context: &RenderContext,
) -> Result<Option<RenderedArtifact>, ValueError> {
    let audience = artifact.audience();
    if audience.agent() != context.audience().agent()
        || audience.scope() != context.target().scope()
        || audience.surface() != context.surface()
        || audience
            .platform()
            .is_some_and(|platform| platform != context.platform())
    {
        return Ok(None);
    }
    Ok(Some(RenderedArtifact::new(
        artifact.id(),
        artifact.family(),
        context
            .native_root()
            .unwrap_or_else(|| context.root())
            .join(artifact.destination()),
        artifact.content().clone(),
        None,
        false,
        Vec::new(),
    )?))
}

fn supported(
    artifact: &HookArtifact,
    context: &RenderContext,
) -> Result<(), UnsupportedTranslation> {
    let agent = context.audience().agent();
    require_translatable(context)?;
    if context.surface() == Surface::Vscode {
        return Err(UnsupportedTranslation::new(
            "hook-surface",
            "Portable hooks target Copilot CLI, not VS Code.",
        ));
    }
    if artifact.event() == HookEvent::BeforeTool
        && artifact.outcomes().contains(&HookOutcome::ModifyInput)
        && matches!(agent, Agent::Codex | Agent::Cursor)
    {
        return Err(UnsupportedTranslation::new(
            "hook-modification-approval",
            "This host has no verified argument-replacement channel preserving normal approval flow.",
        ));
    }
    let mut outcomes = vec![HookOutcome::Continue];
    match artifact.event() {
        HookEvent::BeforeTool => {
            outcomes.extend([HookOutcome::Block, HookOutcome::ModifyInput]);
            if matches!(agent, Agent::Claude | Agent::Copilot) {
                outcomes.push(HookOutcome::Ask);
            }
            if matches!(agent, Agent::Claude | Agent::Codex) {
                outcomes.push(HookOutcome::Context);
            }
        }
        HookEvent::AfterTool => outcomes.push(HookOutcome::Context),
        HookEvent::SessionStart if !matches!(agent, Agent::Opencode | Agent::Pi) => {
            outcomes.push(HookOutcome::Context);
        }
        HookEvent::Prompt => {
            if agent == Agent::Opencode {
                outcomes.clear();
            }
            if matches!(agent, Agent::Claude | Agent::Codex | Agent::Cursor) {
                outcomes.push(HookOutcome::Block);
            }
            if matches!(agent, Agent::Claude | Agent::Codex) {
                outcomes.push(HookOutcome::Context);
            }
            if agent == Agent::Pi {
                outcomes.push(HookOutcome::ModifyInput);
            }
        }
        HookEvent::Stop if agent == Agent::Opencode => outcomes.clear(),
        _ => {}
    }
    let unsupported: Vec<_> = artifact
        .outcomes()
        .iter()
        .filter(|item| !outcomes.contains(item))
        .collect();
    if !unsupported.is_empty() {
        let names = unsupported
            .iter()
            .map(|item| item.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(UnsupportedTranslation::new(
            "hook-outcome",
            format!(
                "{agent} cannot preserve the declared {} outcomes: {names}",
                artifact.event().as_str()
            ),
        ));
    }
    if context.platform() == Platform::Windows {
        let names: Vec<_> = artifact
            .command()
            .env()
            .iter()
            .map(|(name, _)| name.to_lowercase())
            .collect();
        if names.iter().collect::<HashSet<_>>().len() != names.len() {
            return Err(UnsupportedTranslation::new(
                "windows-env-collision",
                "Hook environment names collide on Windows.",
            ));
        }
    }
    Ok(())
}

fn shell(argv: &[String], context: &RenderContext) -> Result<String, UnsupportedTranslation> {
    let runtime = context.hook_runtime().ok_or_else(|| {
        UnsupportedTranslation::new(
            "hook-runtime",
            "Native hooks require an explicit Node 24+ executable.",
        )
    })?;
    if context.platform() == Platform::Windows {
        if context.audience().agent() == Agent::Cursor {
            return Err(UnsupportedTranslation::new(
                "cursor-windows-shell",
                "Cursor Windows hook shell selection is unverified; use native content.",
            ));
        }
        if runtime.shell() != Some(HookShell::Cmd) {
            return Err(UnsupportedTranslation::new(
                "hook-shell",
                "Windows Codex requires a caller-confirmed cmd.exe /C hook shell.",
            ));
        }
        if argv.iter().any(|value| {
            value.contains(['%', '!', '&', '|', '<', '>', '^', '(', ')', '"', '\r', '\n'])
        }) {
            return Err(UnsupportedTranslation::new(
                "hook-shell-literal",
                "These native launcher paths cannot be represented by the cmd hook bridge.",
            ));
        }
        return Ok(argv
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(" "));
    }
    if runtime.shell() != Some(HookShell::Posix) {
        return Err(UnsupportedTranslation::new(
            "hook-shell",
            "This native hook requires a caller-confirmed POSIX-compatible hook shell.",
        ));
    }
    Ok(argv
        .iter()
        .map(|value| {
            if !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte))
            {
                value.clone()
            } else {
                format!("'{}'", value.replace('\'', "'\"'\"'"))
            }
        })
        .collect::<Vec<_>>()
        .join(" "))
}

fn scalar(text: impl Into<String>) -> Value {
    Value::Scalar(Scalar::text(text))
}
fn array(values: impl IntoIterator<Item = Value>) -> Value {
    Value::Array(ArrayValue::new(values))
}
fn object(
    values: impl IntoIterator<Item = (impl Into<String>, Value)>,
) -> Result<Value, ValueError> {
    Ok(Value::Object(ObjectValue::new(values)?))
}

fn asset_argument<S: std::hash::BuildHasher>(
    value: &CommandArgument,
    assets: &HashMap<String, PathBuf, S>,
) -> Result<String, ValueError> {
    match value {
        CommandArgument::Text(text) => Ok(text.clone()),
        CommandArgument::Asset(reference) => {
            let root = assets
                .get(reference.asset_id())
                .ok_or_else(|| ValueError::new("unknown translated asset reference"))?;
            Ok(reference
                .path()
                .map_or_else(|| root.clone(), |path| root.join(path))
                .display()
                .to_string())
        }
    }
}

const fn event_name(agent: Agent, event: HookEvent) -> &'static str {
    match (agent, event) {
        (Agent::Claude | Agent::Codex, HookEvent::SessionStart) => "SessionStart",
        (Agent::Claude | Agent::Codex, HookEvent::BeforeTool) => "PreToolUse",
        (Agent::Claude | Agent::Codex, HookEvent::AfterTool) => "PostToolUse",
        (Agent::Claude | Agent::Codex, HookEvent::Prompt) => "UserPromptSubmit",
        (Agent::Claude | Agent::Codex, HookEvent::Stop) => "Stop",
        (Agent::Cursor | Agent::Copilot, HookEvent::SessionStart) => "sessionStart",
        (Agent::Cursor | Agent::Copilot, HookEvent::BeforeTool) => "preToolUse",
        (Agent::Cursor | Agent::Copilot, HookEvent::AfterTool) => "postToolUse",
        (Agent::Cursor, HookEvent::Prompt) => "beforeSubmitPrompt",
        (Agent::Copilot, HookEvent::Prompt) => "userPromptSubmitted",
        (Agent::Cursor, HookEvent::Stop) => "stop",
        (Agent::Copilot, HookEvent::Stop) => "agentStop",
        _ => "",
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HookTranslationError {
    Unsupported(UnsupportedTranslation),
    Invalid(ValueError),
}

impl From<ValueError> for HookTranslationError {
    fn from(error: ValueError) -> Self {
        Self::Invalid(error)
    }
}

const fn unsupported(
    error: UnsupportedTranslation,
) -> Result<HookTranslation, HookTranslationError> {
    Err(HookTranslationError::Unsupported(error))
}

#[allow(clippy::too_many_lines)]
pub fn hook_artifacts<S: std::hash::BuildHasher>(
    bundle: &Bundle,
    context: &RenderContext,
    assets: &HashMap<String, PathBuf, S>,
) -> Result<HookTranslation, HookTranslationError> {
    let hooks: Vec<&HookArtifact> = bundle
        .artifacts()
        .iter()
        .filter_map(|item| match item {
            Artifact::Hook(hook) => Some(hook),
            _ => None,
        })
        .collect();
    if hooks.is_empty() {
        return Ok(HookTranslation {
            artifacts: Vec::new(),
            dependencies: Vec::new(),
            notices: Vec::new(),
        });
    }
    for hook in &hooks {
        if let Err(error) = supported(hook, context) {
            return unsupported(error);
        }
    }
    for event in [
        HookEvent::SessionStart,
        HookEvent::BeforeTool,
        HookEvent::AfterTool,
        HookEvent::Prompt,
        HookEvent::Stop,
    ] {
        let selected: Vec<_> = hooks.iter().filter(|hook| hook.event() == event).collect();
        let timeout: i64 = selected
            .iter()
            .map(|hook| i64::from(hook.timeout_ms()))
            .sum();
        if selected.len() > HANDLER_LIMIT || timeout > i64::from(EVENT_TIMEOUT_LIMIT_MS) {
            return unsupported(UnsupportedTranslation::new(
                "hook-event-budget",
                "Each event permits at most 64 handlers and 300000 ms total declared timeout.",
            ));
        }
    }
    let agent = context.audience().agent();
    let embedded = matches!(agent, Agent::Opencode | Agent::Pi);
    let Some(runtime) = context.hook_runtime() else {
        return unsupported(UnsupportedTranslation::new(
            "hook-runtime",
            "Native hooks require an explicit Node 24+ executable.",
        ));
    };
    if !embedded && runtime.node().is_none() {
        return unsupported(UnsupportedTranslation::new(
            "hook-runtime",
            "Native hooks require an explicit Node 24+ executable.",
        ));
    }
    let native_directory = if context.target().scope() == TargetScope::User {
        user_directory(context)?
    } else {
        context.root().join(match agent {
            Agent::Claude => ".claude",
            Agent::Codex => ".codex",
            Agent::Cursor => ".cursor",
            Agent::Copilot => ".github",
            Agent::Opencode => ".opencode",
            Agent::Pi => ".pi",
            Agent::Droid => {
                return Err(HookTranslationError::Unsupported(
                    UnsupportedTranslation::new("agent-unsupported", untranslatable_message(agent)),
                ));
            }
        })
    };
    let base = context
        .asset_root()
        .map_or_else(|| context.root().join(".flyrail-assets"), Path::to_path_buf)
        .join(bundle.id())
        .join(".hooks")
        .join(agent.to_string());
    if base.components().any(|part| {
        part.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("node_modules")
    }) {
        return unsupported(UnsupportedTranslation::new(
            "hook-node-modules",
            "Node cannot load stripped TypeScript from a node_modules asset root.",
        ));
    }
    let handlers = hooks
        .iter()
        .map(|hook| {
            let command = hook.command();
            let argv = command
                .argv()
                .iter()
                .map(|value| asset_argument(value, assets))
                .collect::<Result<Vec<_>, _>>()?;
            let cwd = match command.cwd() {
                Some(value @ CommandArgument::Asset(_)) => {
                    PathBuf::from(asset_argument(value, assets)?)
                }
                Some(CommandArgument::Text(path)) => context
                    .execution_root()
                    .unwrap_or_else(|| context.root())
                    .join(path),
                None => context
                    .execution_root()
                    .unwrap_or_else(|| context.root())
                    .to_path_buf(),
            };
            HookHandler::new(
                hook.id(),
                hook.event(),
                argv,
                command.env().iter().map(|(key, value)| {
                    (
                        key.clone(),
                        match value {
                            EnvironmentValue::Text(text) => HookEnvironment::Text(text.clone()),
                            EnvironmentValue::Reference(reference) => {
                                HookEnvironment::Reference(reference.name().to_owned())
                            }
                        },
                    )
                }),
                cwd,
                u32::try_from(hook.timeout_ms()).unwrap_or_default(),
                hook.outcomes().iter().copied(),
                hook.tools().iter().cloned(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let config = HookConfig::new(agent.to_string(), handlers)?;
    let mut entries = runtime.resources().to_vec();
    let config_json = String::from_utf8(config.to_bytes())
        .map_err(|_| ValueError::new("hook config must be UTF-8"))?;
    let config_value = Json::parse(&config_json)?;
    let mut config_json = String::new();
    config_value.encode_python_compact(&mut config_json);
    let mut quoted = String::new();
    Json::Str(config_json).encode_python_compact(&mut quoted);
    entries.push(BundleEntry::file("config.mts", format!("import type {{ Config }} from './runner.mts';\nconst config: Config = JSON.parse({quoted});\nexport default config;\n").into_bytes())?);
    let digest = hex_digest(entries.iter().map(|entry| entry.data().unwrap_or_default()));
    let revision = base.join("revisions").join(digest);
    let runtime_id = identifier(&[bundle.id(), &agent.to_string(), "runtime"]);
    let launcher_id = identifier(&[bundle.id(), &agent.to_string(), "launcher"]);
    let version_id = identifier(&[bundle.id(), &agent.to_string(), "version"]);
    let authored: HashSet<&str> = bundle
        .artifacts()
        .iter()
        .map(Artifact::id)
        .chain(bundle.assets().iter().map(SupportAsset::id))
        .collect();
    if [&runtime_id, &launcher_id, &version_id]
        .iter()
        .any(|id| authored.contains(id.as_str()))
    {
        return unsupported(UnsupportedTranslation::new(
            "hook-generated-id",
            "An authored identifier collides with a generated hook identifier.",
        ));
    }
    let mut artifacts = vec![RenderedArtifact::new(
        runtime_id.clone(),
        Family::Hooks,
        revision.clone(),
        Content::Tree(TreeContent::new(entries)?),
        None,
        false,
        Vec::new(),
    )?];
    let hook_ids: HashSet<&str> = hooks.iter().map(|hook| hook.id()).collect();
    let mut dependencies = bundle
        .dependencies()
        .iter()
        .filter(|edge| hook_ids.contains(edge.dependent()))
        .map(|edge| Dependency::new(&runtime_id, edge.required()))
        .collect::<Result<Vec<_>, _>>()?;
    let bridge = Json::Str(file_url(&revision.join("bridge.mts"), context.platform())?);
    let config_url = Json::Str(file_url(&revision.join("config.mts"), context.platform())?);
    let mut bridge_text = String::new();
    bridge.encode(&mut bridge_text);
    let mut config_text = String::new();
    config_url.encode(&mut config_text);
    if embedded {
        let launcher = native_directory
            .join(if agent == Agent::Opencode {
                "plugins"
            } else {
                "extensions"
            })
            .join(format!("flyrail-{}.ts", identifier(&[bundle.id()])));
        let code = if agent == Agent::Opencode {
            format!(
                "import {{ openCode, type OpenCodeInput }} from {bridge_text};\nimport config from {config_text};\nexport default async function(host: OpenCodeInput) {{\n  return openCode(config, host);\n}}\n"
            )
        } else {
            format!(
                "import {{ registerPi, type Pi }} from {bridge_text};\nimport config from {config_text};\nexport default function(pi: Pi) {{ registerPi(config, pi); }}\n"
            )
        };
        for hook in &hooks {
            artifacts.push(RenderedArtifact::new(
                hook.id(),
                Family::Hooks,
                launcher.clone(),
                Content::File(FileContent::new(
                    code.as_bytes().to_vec(),
                    FileMode::Private,
                )),
                None,
                false,
                Vec::new(),
            )?);
            dependencies.push(Dependency::new(hook.id(), &runtime_id)?);
        }
    } else {
        let launcher = base.join("entry.mjs");
        let code = format!(
            "try {{ const {{ native }} = await import({bridge_text}); const {{ default: config }} = await import({config_text}); await native(config, process.argv[2]); }} catch {{ process.stderr.write('flyrail-hook:bootstrap\\n'); process.stdout.write('{{}}\\n'); }}\n"
        );
        artifacts.push(RenderedArtifact::new(
            &launcher_id,
            Family::Hooks,
            launcher.clone(),
            Content::File(FileContent::new(code.into_bytes(), FileMode::Private)),
            None,
            false,
            Vec::new(),
        )?);
        dependencies.push(Dependency::new(&launcher_id, &runtime_id)?);
        let node = runtime
            .node()
            .map_or_else(String::new, |path| path.display().to_string());
        if agent == Agent::Claude
            && [node.as_str(), &launcher.display().to_string()]
                .iter()
                .any(|value| value.contains("${"))
        {
            return unsupported(UnsupportedTranslation::new(
                "hook-literal-interpolation",
                "Claude launcher paths cannot contain host placeholder syntax.",
            ));
        }
        if context.platform() == Platform::Windows
            && matches!(
                Path::new(&node)
                    .extension()
                    .and_then(|value| value.to_str())
                    .map(str::to_lowercase)
                    .as_deref(),
                Some("cmd" | "bat")
            )
        {
            return unsupported(UnsupportedTranslation::new(
                "hook-executable",
                "Native hooks require a real executable, not a Windows command shim.",
            ));
        }
        let mut events: Vec<HookEvent> = hooks
            .iter()
            .map(|hook| hook.event())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        events.sort_by_key(|event| event.as_str());
        let mut native = Vec::new();
        for event in events {
            let argv = vec![
                node.clone(),
                launcher.display().to_string(),
                event.as_str().to_owned(),
            ];
            let entry = match agent {
                Agent::Copilot => object([
                    ("type", scalar("command")),
                    ("exec", scalar(&node)),
                    ("args", array(argv.iter().skip(1).map(scalar))),
                    ("timeoutSec", Value::Scalar(Scalar::integer(310))),
                ])?,
                Agent::Claude => object([(
                    "hooks",
                    array([object([
                        ("type", scalar("command")),
                        ("command", scalar(&node)),
                        ("args", array(argv.iter().skip(1).map(scalar))),
                        ("timeout", Value::Scalar(Scalar::integer(310))),
                    ])?]),
                )])?,
                Agent::Codex => object([(
                    "hooks",
                    array([object([
                        ("type", scalar("command")),
                        (
                            "command",
                            scalar(
                                shell(&argv, context).map_err(HookTranslationError::Unsupported)?,
                            ),
                        ),
                        ("timeout", Value::Scalar(Scalar::integer(310))),
                    ])?]),
                )])?,
                Agent::Cursor => object([
                    (
                        "command",
                        scalar(shell(&argv, context).map_err(HookTranslationError::Unsupported)?),
                    ),
                    ("timeout", Value::Scalar(Scalar::integer(310))),
                ])?,
                _ => unreachable!(),
            };
            native.push((event, entry));
        }
        for hook in &hooks {
            let name = event_name(agent, hook.event());
            let value = native
                .iter()
                .find(|(event, _)| *event == hook.event())
                .map(|(_, value)| value.clone())
                .ok_or_else(|| ValueError::new("missing hook event"))?;
            let (destination, payload, schemas) = if agent == Agent::Copilot {
                let mut encoded = String::new();
                Json::Object(vec![
                    ("version".to_owned(), Json::Int(1)),
                    (
                        "hooks".to_owned(),
                        Json::Object(
                            native
                                .iter()
                                .map(|(event, _)| {
                                    (
                                        event_name(agent, *event).to_owned(),
                                        Json::Array(vec![Json::Object(vec![
                                            ("type".to_owned(), Json::Str("command".to_owned())),
                                            ("exec".to_owned(), Json::Str(node.clone())),
                                            (
                                                "args".to_owned(),
                                                Json::Array(vec![
                                                    Json::Str(launcher.display().to_string()),
                                                    Json::Str(event.as_str().to_owned()),
                                                ]),
                                            ),
                                            ("timeoutSec".to_owned(), Json::Int(310)),
                                        ])]),
                                    )
                                })
                                .collect(),
                        ),
                    ),
                ])
                .encode_python_compact(&mut encoded);
                (
                    native_directory
                        .join("hooks")
                        .join(format!("flyrail-{}.json", identifier(&[bundle.id()]))),
                    Content::File(FileContent::new(
                        format!("{encoded}\n").into_bytes(),
                        FileMode::Private,
                    )),
                    Vec::new(),
                )
            } else {
                let (member, selected) = if agent == Agent::Cursor {
                    let command = match &value {
                        Value::Object(object) => object
                            .items()
                            .iter()
                            .find(|(key, _)| key.as_str() == "command")
                            .map(|(_, value)| value.clone())
                            .ok_or_else(|| ValueError::new("missing command"))?,
                        _ => return Err(ValueError::new("invalid hook entry").into()),
                    };
                    (
                        Member::new(
                            Scalar::text(match &command {
                                Value::Scalar(value) => value.as_text().unwrap_or_default(),
                                _ => "",
                            }),
                            ["command"],
                        )?,
                        value,
                    )
                } else {
                    (Member::new(value.clone(), Vec::<String>::new())?, value)
                };
                let format = if agent == Agent::Cursor {
                    DocumentFormat::Json
                } else {
                    DocumentFormat::Jsonc
                };
                let structured = StructuredContent::new(
                    format,
                    Selector::new([
                        SelectorPart::Key(Key::new("hooks")?),
                        SelectorPart::Key(Key::new(name)?),
                        SelectorPart::Member(member),
                    ])?,
                    selected,
                    None,
                )?;
                let schemas = if agent == Agent::Cursor {
                    vec![DocumentSchema::new(
                        DocumentFormat::Json,
                        "version",
                        Scalar::integer(1),
                    )?]
                } else {
                    Vec::new()
                };
                (
                    native_directory.join(if agent == Agent::Claude {
                        "settings.json"
                    } else {
                        "hooks.json"
                    }),
                    Content::Structured(structured),
                    schemas,
                )
            };
            artifacts.push(RenderedArtifact::new(
                hook.id(),
                Family::Hooks,
                destination,
                payload,
                None,
                false,
                schemas,
            )?);
            dependencies.push(Dependency::with_mode(
                hook.id(),
                &launcher_id,
                DependencyMode::StableReference,
            )?);
        }
    }
    dependencies.sort_by(|a, b| (a.dependent(), a.required()).cmp(&(b.dependent(), b.required())));
    dependencies.dedup();
    let notice_id = hooks
        .first()
        .map(|hook| hook.id())
        .ok_or_else(|| ValueError::new("hook translation requires a hook"))?;
    let notices = vec![
        Notice::with_code(
            notice_id,
            NoticeKind::Prerequisite,
            "Requires Node 24+ for native/Pi bridges, Bun 1.4.2+ for OpenCode; host APIs documented in the hook protocol.",
            "hook-runtime-prerequisite",
        )?,
        Notice::with_code(
            notice_id,
            NoticeKind::Activation,
            "Installation only writes assets/configuration. Host trust, reload and actual activation remain host-controlled.",
            "hook-activation",
        )?,
    ];
    Ok(HookTranslation {
        artifacts,
        dependencies,
        notices,
    })
}
