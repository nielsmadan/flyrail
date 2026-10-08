use flyrail::{
    Acquisition, Agent, HookRuntime, HookShell, InstallationObservation, InstallationResult,
    Notice, OperationStatus, Platform, RenderContext, ResourceKind, ResourceRef, SkillOptions,
    Target, TargetError, TargetInspection, TargetResult, inspect, inspect_installation, install,
    remove, render, sync, uninstall_with_options, update_with_options,
};
use flyrail_example_checksum::{
    BUNDLE_ID, CONFIG_BUNDLE_ID, GUIDANCE, configuration_bundle, packaged_bundle, sha256_reader,
};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::env;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::Component;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const USAGE: &str = "\
Usage:
  flyrail-checksum --version
  flyrail-checksum digest FILE
  flyrail-checksum ai status|install --target TARGET [--target TARGET ...]
  flyrail-checksum ai update|uninstall --target TARGET [--target TARGET ...] [--replace-modified]
  flyrail-checksum config status|update --project PROJECT --agent AGENT --mcp-command COMMAND --mcp-url URL [options]
  flyrail-checksum config uninstall --project PROJECT --agent AGENT [--replace-modified]

Config options: --node PATH --mcp-arg ARG (repeatable) --guidance TEXT --asset-root PATH
Agents: claude, codex, opencode, pi, cursor, copilot";

struct AiRequest {
    action: String,
    targets: Vec<PathBuf>,
    replace_modified: bool,
}

struct ConfigRequest {
    action: String,
    project: PathBuf,
    agent: Agent,
    node: Option<PathBuf>,
    mcp_command: Option<String>,
    mcp_args: Vec<String>,
    mcp_url: Option<String>,
    guidance: String,
    asset_root: Option<PathBuf>,
    replace_modified: bool,
}

fn usage_error(message: &str) -> i32 {
    eprintln!("flyrail-checksum: {message}\n{USAGE}");
    2
}

fn text(value: &OsStr, name: &str) -> Result<String, String> {
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{name} must be valid UTF-8"))
}

fn parse_ai(arguments: &[OsString]) -> Result<AiRequest, String> {
    let Some(action_value) = arguments.first() else {
        return Err(String::from("missing AI action"));
    };
    let action = text(action_value, "AI action")?;
    if !matches!(
        action.as_str(),
        "status" | "install" | "update" | "uninstall"
    ) {
        return Err(format!("unknown AI action '{action}'"));
    }
    let mut targets = Vec::new();
    let mut replace_modified = false;
    let mut index = 1;
    while let Some(argument) = arguments.get(index) {
        let option = text(argument, "option")?;
        match option.as_str() {
            "--target" => {
                targets.push(path_value(arguments, &mut index, &option)?);
            }
            "--replace-modified" => replace_modified = true,
            _ => return Err(format!("unknown option '{option}'")),
        }
        index += 1;
    }
    if targets.is_empty() {
        return Err(String::from("at least one --target is required"));
    }
    if replace_modified && !matches!(action.as_str(), "update" | "uninstall") {
        return Err(format!("{action} does not accept --replace-modified"));
    }
    Ok(AiRequest {
        action,
        targets,
        replace_modified,
    })
}

fn option_value(arguments: &[OsString], index: &mut usize, option: &str) -> Result<String, String> {
    *index += 1;
    let value = arguments
        .get(*index)
        .ok_or_else(|| format!("{option} requires a value"))?;
    if value == "--" {
        *index += 1;
        return arguments
            .get(*index)
            .ok_or_else(|| format!("{option} requires a value"))
            .and_then(|value| text(value, option));
    }
    if value.to_string_lossy().starts_with("--") {
        return Err(format!("{option} requires a value"));
    }
    text(value, option)
}

fn path_value(arguments: &[OsString], index: &mut usize, option: &str) -> Result<PathBuf, String> {
    *index += 1;
    let value = arguments
        .get(*index)
        .ok_or_else(|| format!("{option} requires a value"))?;
    if value == "--" {
        *index += 1;
        return arguments
            .get(*index)
            .map(PathBuf::from)
            .ok_or_else(|| format!("{option} requires a value"));
    }
    if value.to_string_lossy().starts_with("--") {
        return Err(format!("{option} requires a value"));
    }
    Ok(PathBuf::from(value))
}

fn parse_config(arguments: &[OsString]) -> Result<ConfigRequest, String> {
    let Some(action_value) = arguments.first() else {
        return Err(String::from("missing config action"));
    };
    let action = text(action_value, "config action")?;
    if !matches!(action.as_str(), "status" | "update" | "uninstall") {
        return Err(format!("unknown config action '{action}'"));
    }
    let mut project = None;
    let mut agent = None;
    let mut node = None;
    let mut mcp_command = None;
    let mut mcp_args = Vec::new();
    let mut mcp_url = None;
    let mut guidance = String::from(GUIDANCE);
    let mut asset_root = None;
    let mut replace_modified = false;
    let mut index = 1;
    while let Some(argument) = arguments.get(index) {
        let option = text(argument, "option")?;
        match option.as_str() {
            "--project" => {
                project = Some(path_value(arguments, &mut index, &option)?);
            }
            "--agent" => {
                let value = option_value(arguments, &mut index, &option)?;
                agent = Some(value.parse::<Agent>().map_err(|error| error.to_string())?);
            }
            "--node" => node = Some(path_value(arguments, &mut index, &option)?),
            "--mcp-command" => {
                mcp_command = Some(option_value(arguments, &mut index, &option)?);
            }
            "--mcp-arg" => mcp_args.push(option_value(arguments, &mut index, &option)?),
            "--mcp-url" => mcp_url = Some(option_value(arguments, &mut index, &option)?),
            "--guidance" => guidance = option_value(arguments, &mut index, &option)?,
            "--asset-root" => {
                asset_root = Some(path_value(arguments, &mut index, &option)?);
            }
            "--replace-modified" => replace_modified = true,
            _ => return Err(format!("unknown option '{option}'")),
        }
        index += 1;
    }
    let project = project.ok_or_else(|| String::from("--project is required"))?;
    let agent = agent.ok_or_else(|| String::from("--agent is required"))?;
    if action == "uninstall" {
        if node.is_some()
            || mcp_command.is_some()
            || !mcp_args.is_empty()
            || mcp_url.is_some()
            || asset_root.is_some()
            || guidance != GUIDANCE
        {
            return Err(String::from(
                "uninstall accepts only project, agent, and replacement options",
            ));
        }
    } else {
        if mcp_command.is_none() {
            return Err(String::from("--mcp-command is required"));
        }
        if mcp_url.is_none() {
            return Err(String::from("--mcp-url is required"));
        }
    }
    if action == "status" && replace_modified {
        return Err(String::from("status does not accept --replace-modified"));
    }
    Ok(ConfigRequest {
        action,
        project,
        agent,
        node,
        mcp_command,
        mcp_args,
        mcp_url,
        guidance,
        asset_root,
        replace_modified,
    })
}

fn error_json(error: Option<&TargetError>) -> Value {
    error.map_or(Value::Null, |error| {
        json!({
            "code": error.code().as_str(),
            "message": error.message(),
            "path": error.path().map(|path| path.display().to_string()),
            "errno": error.errno(),
        })
    })
}

fn installation_json(installed: &flyrail::Installation) -> Value {
    json!({
        "bundle_id": installed.bundle_id(),
        "version": installed.version(),
        "content_digest": installed.content_digest(),
        "transaction_id": installed.transaction_id(),
        "entries": installed.entries().iter().map(|entry| json!({
            "path": entry.path(), "size": entry.size(), "sha256": entry.sha256(),
            "executable": entry.executable(),
        })).collect::<Vec<_>>(),
    })
}

fn observation_json(observation: &flyrail::Observation) -> Value {
    json!({
        "state": observation.state().as_str(),
        "installed": observation.installed().map(installation_json),
        "installations": observation.installations().iter().map(installation_json).collect::<Vec<_>>(),
        "version_matches": observation.version_matches(),
        "recorded_content_matches": observation.recorded_content_matches(),
        "content_matches": observation.content_matches(),
        "modifications": observation.modifications().iter().map(|item| json!({
            "bundle_id": item.bundle_id(), "path": item.path(), "kind": item.kind().as_str(),
        })).collect::<Vec<_>>(),
        "conflicts": observation.conflicts().iter().map(|item| json!({
            "path": item.path(), "owner": item.owner(),
        })).collect::<Vec<_>>(),
        "error": error_json(observation.error()),
        "recovery_paths": observation.recovery_paths().iter()
            .map(|path| path.display().to_string()).collect::<Vec<_>>(),
    })
}

fn target_json(target: &Target) -> Value {
    json!({
        "root": target.root().display().to_string(),
        "agent": target.agent().map(Agent::as_str),
        "scope": target.scope().as_str(),
    })
}

fn inspection_json(result: &TargetInspection<Target>) -> Value {
    json!({
        "target": target_json(result.target()),
        "root": result.root().display().to_string(),
        "state_root": result.state_root().display().to_string(),
        "alias_of": result.alias_of(),
        "observation": observation_json(result.observation()),
        "is_current": result.observation().is_current(),
    })
}

fn result_json(result: &TargetResult<Target>) -> Value {
    json!({
        "target": target_json(result.target()),
        "root": result.root().display().to_string(),
        "state_root": result.state_root().display().to_string(),
        "alias_of": result.alias_of(),
        "status": result.status().as_str(),
        "observation": observation_json(result.observation()),
        "error": error_json(result.error()),
        "recovery_paths": result.recovery_paths().iter()
            .map(|path| path.display().to_string()).collect::<Vec<_>>(),
        "is_current": result.observation().is_current(),
    })
}

fn clean(result: &TargetResult<Target>) -> bool {
    matches!(
        result.status(),
        OperationStatus::Applied | OperationStatus::Unchanged
    ) && result.error().is_none()
        && result.observation().error().is_none()
        && result.recovery_paths().is_empty()
}

fn ai(request: &AiRequest) -> Result<i32, Box<dyn Error>> {
    let targets = request
        .targets
        .iter()
        .map(Target::directory)
        .collect::<Result<Vec<_>, _>>()?;
    if request.action == "uninstall" {
        let results = uninstall_with_options(
            BUNDLE_ID,
            targets,
            SkillOptions::new(request.replace_modified, Duration::ZERO),
        )?;
        let exit = i32::from(results.iter().any(|result| !clean(result)));
        print_ai(None, &results.iter().map(result_json).collect::<Vec<_>>());
        return Ok(exit);
    }
    let bundle = packaged_bundle()?;
    if bundle.id() != BUNDLE_ID {
        return Err("packaged bundle ID differs from the host's stable ID".into());
    }
    if request.action == "status" {
        let results = inspect(&bundle, targets)?;
        let exit = i32::from(
            results
                .iter()
                .any(|result| !result.observation().is_current()),
        );
        print_ai(
            Some(bundle.version()),
            &results.iter().map(inspection_json).collect::<Vec<_>>(),
        );
        return Ok(exit);
    }
    let results = if request.action == "install" {
        install(&bundle, targets)?
    } else {
        update_with_options(
            &bundle,
            targets,
            SkillOptions::new(request.replace_modified, Duration::ZERO),
        )?
    };
    let exit = i32::from(results.iter().any(|result| !clean(result)));
    print_ai(
        Some(bundle.version()),
        &results.iter().map(result_json).collect::<Vec<_>>(),
    );
    Ok(exit)
}

fn print_ai(bundle_version: Option<&str>, results: &[Value]) {
    println!(
        "{}",
        json!({
            "app_version": APP_VERSION,
            "bundle_id": BUNDLE_ID,
            "bundle_version": bundle_version,
            "results": results,
        })
    );
}

fn absolute(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };
    let mut pending = path_components(&absolute);
    let mut resolved = PathBuf::from("/");
    let mut links = 0_u8;
    while let Some(component) = pending.pop_front() {
        if component == "." {
            continue;
        }
        if component == ".." {
            resolved.pop();
            continue;
        }
        let candidate = resolved.join(&component);
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                links = links.saturating_add(1);
                if links > 40 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "too many symbolic links while resolving path",
                    )
                    .into());
                }
                let target = fs::read_link(&candidate)?;
                if target.is_absolute() {
                    resolved = PathBuf::from("/");
                }
                let mut target_components = path_components(&target);
                target_components.append(&mut pending);
                pending = target_components;
            }
            Ok(_) => resolved.push(component),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                resolved.push(component);
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(resolved)
}

fn path_components(path: &Path) -> VecDeque<OsString> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_owned()),
            Component::ParentDir => Some(OsString::from("..")),
            Component::CurDir | Component::RootDir | Component::Prefix(_) => None,
        })
        .collect()
}

const fn platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Linux
    }
}

fn notice_json(notice: &Notice) -> Value {
    json!({
        "artifact_id": notice.artifact_id(),
        "kind": notice.kind().as_str(),
        "message": notice.message(),
        "code": notice.code(),
    })
}

fn installation_observation_json(observation: &InstallationObservation) -> Value {
    json!({
        "bundle_id": observation.bundle_id(),
        "target": {
            "index_root": observation.target().index_root().display().to_string(),
            "context": observation.target().context(),
            "routing_context": observation.target().routing_context(),
        },
        "is_current": observation.is_current(),
        "pending": observation.pending(),
        "version": observation.version(),
        "bundle_digest": observation.bundle_digest(),
        "render_digest": observation.render_digest(),
        "error": error_json(observation.error()),
        "resources": observation.resources().iter().map(|resource| json!({
            "destination": resource.destination().display().to_string(),
            "state_root": resource.state_root().display().to_string(),
            "claims": resource.claims().iter().map(|claim| json!({
                "claim_id": claim.claim_id(),
                "bundle_id": claim.bundle_id(),
                "artifact_id": claim.artifact_id(),
                "version": claim.version(),
                "bundle_digest": claim.bundle_digest(),
                "render_digest": claim.render_digest(),
                "status": claim.status().as_str(),
            })).collect::<Vec<_>>(),
            "revision_digest": resource.revision_digest(),
            "error": error_json(resource.error()),
            "recovery_paths": resource.recovery_paths().iter()
                .map(|path| path.display().to_string()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

const fn resource_kind(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Document => "document",
        ResourceKind::Tree => "tree",
        ResourceKind::SkillContainer => "skill-container",
    }
}

fn resource_ref_json(resource: &ResourceRef) -> Value {
    json!({
        "destination": resource.destination().display().to_string(),
        "kind": resource_kind(resource.kind()),
        "state_root": resource.state_root().display().to_string(),
        "schema_version": resource.schema_version(),
    })
}

fn config_report(
    request: &ConfigRequest,
    bundle_version: Option<&str>,
    configured_current: Option<bool>,
    observation: &InstallationObservation,
    result: Option<&InstallationResult>,
    notices: &[Notice],
) {
    let prerequisites = if bundle_version.is_some() {
        let mut values = vec![
            String::from("Provide the selected stdio MCP server and HTTP endpoint."),
            String::from(
                "Provide CHECKSUM_MCP_TOKEN and CHECKSUM_HTTP_TOKEN in the agent environment.",
            ),
            String::from("Keep Python 3 available for the bundled hook."),
        ];
        values.extend(
            notices
                .iter()
                .filter(|notice| notice.kind() == flyrail::NoticeKind::Prerequisite)
                .map(|notice| notice.message().to_owned()),
        );
        values
    } else {
        Vec::new()
    };
    println!(
        "{}",
        json!({
            "app_version": APP_VERSION,
            "bundle_id": CONFIG_BUNDLE_ID,
            "bundle_version": bundle_version,
            "agent": request.agent.as_str(),
            "project": request.project.display().to_string(),
            "configured_current": configured_current,
            "recorded_current": observation.is_current(),
            "activation": "not-checked",
            "prerequisites": prerequisites,
            "notices": notices.iter().map(notice_json).collect::<Vec<_>>(),
            "observation": installation_observation_json(observation),
            "status": result.map(|item| item.status().as_str()),
            "error": result.map_or(Value::Null, |item| error_json(item.error())),
            "resources": result.map_or_else(Vec::new, |item| item.resources().iter().map(|resource| json!({
                "resource": resource_ref_json(resource.resource()),
                "status": resource.status().as_str(),
                "error": error_json(resource.error()),
                "recovery_paths": resource.recovery_paths().iter()
                    .map(|path| path.display().to_string()).collect::<Vec<_>>(),
            })).collect::<Vec<_>>()),
        })
    );
}

#[allow(clippy::too_many_lines)]
fn config(mut request: ConfigRequest) -> Result<i32, Box<dyn Error>> {
    request.project = absolute(&request.project)?;
    let base_context = RenderContext::new(
        Target::project(request.agent, &request.project)?,
        platform(),
    )?;
    let index_root = request
        .project
        .join(".cache")
        .join(format!("{CONFIG_BUNDLE_ID}-{}", request.agent));
    if request.action == "uninstall" {
        let target = base_context.installation(index_root)?;
        let result = remove(
            CONFIG_BUNDLE_ID,
            &target,
            request.replace_modified,
            Duration::ZERO,
        )?;
        let exit = i32::from(
            !matches!(
                result.status(),
                OperationStatus::Applied | OperationStatus::Unchanged
            ) || result.error().is_some()
                || result.observation().error().is_some()
                || result.resources().iter().any(|resource| {
                    resource.error().is_some() || !resource.recovery_paths().is_empty()
                }),
        );
        config_report(
            &request,
            None,
            None,
            result.observation(),
            Some(&result),
            &[],
        );
        return Ok(exit);
    }
    let command = request
        .mcp_command
        .as_deref()
        .ok_or("missing MCP command")?;
    let url = request.mcp_url.as_deref().ok_or("missing MCP URL")?;
    let mut argv = vec![command.to_owned()];
    argv.extend(request.mcp_args.iter().cloned());
    let bundle = configuration_bundle(&request.guidance, &argv, url)?;
    let shell = if cfg!(target_os = "windows") {
        HookShell::Cmd
    } else {
        HookShell::Posix
    };
    let runtime = HookRuntime::new(
        request.node.as_deref().map(absolute).transpose()?,
        Some(shell),
    )?;
    let mut context = base_context.with_hook_runtime(runtime);
    if let Some(root) = &request.asset_root {
        context = context.with_asset_root(absolute(root)?)?;
    }
    let target = context.installation(index_root)?;
    let desired = render(&bundle, &context)?;
    let result = if request.action == "update" {
        Some(sync(
            &bundle,
            &desired,
            &target,
            Acquisition::Conflict,
            request.replace_modified,
            Duration::ZERO,
        )?)
    } else {
        None
    };
    let observation = match &result {
        Some(value) => value.observation().clone(),
        None => inspect_installation(CONFIG_BUNDLE_ID, &target)?,
    };
    let matches = observation.matches(&bundle, &desired);
    let notices = desired.notices();
    let exit = result.as_ref().map_or_else(
        || i32::from(!matches),
        |value| {
            i32::from(
                !matches!(
                    value.status(),
                    OperationStatus::Applied | OperationStatus::Unchanged
                ) || value.error().is_some()
                    || observation.error().is_some()
                    || value.resources().iter().any(|resource| {
                        resource.error().is_some() || !resource.recovery_paths().is_empty()
                    })
                    || !matches,
            )
        },
    );
    config_report(
        &request,
        Some(bundle.version()),
        Some(matches),
        &observation,
        result.as_ref(),
        notices,
    );
    Ok(exit)
}

fn run(arguments: &[OsString]) -> Result<i32, Box<dyn Error>> {
    let Some(command_value) = arguments.first() else {
        return Ok(usage_error("missing command"));
    };
    let command = match text(command_value, "command") {
        Ok(value) => value,
        Err(message) => return Ok(usage_error(&message)),
    };
    match (command.as_str(), arguments.get(1..).unwrap_or_default()) {
        ("--version", []) => {
            println!("flyrail-checksum {APP_VERSION}");
            Ok(0)
        }
        ("digest", [file]) => {
            println!("{}", sha256_reader(fs::File::open(PathBuf::from(file))?)?);
            Ok(0)
        }
        ("ai", rest) => match parse_ai(rest) {
            Ok(request) => ai(&request),
            Err(message) => Ok(usage_error(&message)),
        },
        ("config", rest) => match parse_config(rest) {
            Ok(request) => config(request),
            Err(message) => Ok(usage_error(&message)),
        },
        _ => Ok(usage_error(&format!(
            "invalid command or arguments for '{command}'"
        ))),
    }
}

fn main() -> ExitCode {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    let code = match run(&arguments) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("flyrail-checksum: {error}");
            2
        }
    };
    ExitCode::from(u8::try_from(code).unwrap_or(2))
}
