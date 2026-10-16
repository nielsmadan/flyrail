use flyrail::{
    Bundle, DirectoryStore, Observation, OperationStatus, SkillOptions, StateStore, Target,
    TargetError, TargetInspection, TargetResult, inspect, install, uninstall_with_options,
    update_with_options,
};
use serde_json::{Value, json};
use std::env;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const USAGE: &str = "\
Usage:
  flyrail-notes --version
  flyrail-notes words FILE
  flyrail-notes ai status --source SOURCE --target TARGET [--target TARGET ...] --state STATE
  flyrail-notes ai install --source SOURCE --target TARGET [--target TARGET ...] --state STATE
  flyrail-notes ai update --source SOURCE --target TARGET [--target TARGET ...] --state STATE [--replace-modified]
  flyrail-notes ai uninstall --bundle-id ID --target TARGET [--target TARGET ...] --state STATE [--replace-modified]";

struct AiRequest {
    action: String,
    source: Option<PathBuf>,
    bundle_id: Option<String>,
    targets: Vec<PathBuf>,
    state: PathBuf,
    replace_modified: bool,
}

fn usage_error(message: &str) -> i32 {
    eprintln!("flyrail-notes: {message}\n{USAGE}");
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
    let mut source = None;
    let mut bundle_id = None;
    let mut targets = Vec::new();
    let mut state = None;
    let mut replace_modified = false;
    let mut index = 1;
    while let Some(argument) = arguments.get(index) {
        let option = text(argument, "option")?;
        match option.as_str() {
            "--source" | "--bundle-id" | "--target" | "--state" => {
                index += 1;
                let Some(value) = arguments.get(index) else {
                    return Err(format!("{option} requires a value"));
                };
                let value = if value == "--" {
                    index += 1;
                    arguments
                        .get(index)
                        .ok_or_else(|| format!("{option} requires a value"))?
                } else {
                    if value.to_string_lossy().starts_with("--") {
                        return Err(format!("{option} requires a value"));
                    }
                    value
                };
                match option.as_str() {
                    "--source" => source = Some(PathBuf::from(value)),
                    "--bundle-id" => bundle_id = Some(text(value, "--bundle-id")?),
                    "--state" => state = Some(PathBuf::from(value)),
                    _ => targets.push(PathBuf::from(value)),
                }
            }
            "--replace-modified" => replace_modified = true,
            _ => return Err(format!("unknown option '{option}'")),
        }
        index += 1;
    }
    if targets.is_empty() {
        return Err(String::from("at least one --target is required"));
    }
    let state = state.ok_or_else(|| String::from("--state is required"))?;
    if action == "uninstall" {
        if source.is_some() {
            return Err(String::from("uninstall does not accept --source"));
        }
        if bundle_id.is_none() {
            return Err(String::from("uninstall requires --bundle-id"));
        }
    } else {
        if bundle_id.is_some() {
            return Err(String::from("only uninstall accepts --bundle-id"));
        }
        if source.is_none() {
            return Err(format!("{action} requires --source"));
        }
    }
    if replace_modified && !matches!(action.as_str(), "update" | "uninstall") {
        return Err(format!("{action} does not accept --replace-modified"));
    }
    Ok(AiRequest {
        action,
        source,
        bundle_id,
        targets,
        state,
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
            "path": entry.path(),
            "size": entry.size(),
            "sha256": entry.sha256(),
            "executable": entry.executable(),
        })).collect::<Vec<_>>(),
    })
}

fn observation_json(observation: &Observation) -> Value {
    json!({
        "state": observation.state().as_str(),
        "installed": observation.installed().map(installation_json),
        "installations": observation.installations().iter().map(installation_json).collect::<Vec<_>>(),
        "version_matches": observation.version_matches(),
        "recorded_content_matches": observation.recorded_content_matches(),
        "content_matches": observation.content_matches(),
        "modifications": observation.modifications().iter().map(|item| json!({
            "bundle_id": item.bundle_id(),
            "path": item.path(),
            "kind": item.kind().as_str(),
        })).collect::<Vec<_>>(),
        "conflicts": observation.conflicts().iter().map(|item| json!({
            "path": item.path(),
            "owner": item.owner(),
        })).collect::<Vec<_>>(),
        "error": error_json(observation.error()),
        "recovery_paths": observation.recovery_paths().iter()
            .map(|path| path.display().to_string()).collect::<Vec<_>>(),
    })
}

fn target_json(target: &Target) -> Value {
    json!({
        "root": target.root().display().to_string(),
        "agent": target.agent().map(flyrail::Agent::as_str),
        "scope": target.scope().as_str(),
    })
}

fn inspection_json(result: &TargetInspection<Target>) -> Value {
    json!({
        "target": target_json(result.target()),
        "root": result.root().display().to_string(),
        "alias_of": result.alias_of(),
        "observation": observation_json(result.observation()),
        "is_current": result.observation().is_current(),
    })
}

fn result_json(result: &TargetResult<Target>) -> Value {
    json!({
        "target": target_json(result.target()),
        "root": result.root().display().to_string(),
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

fn print_report(bundle_id: &str, bundle_version: Option<&str>, results: &[Value]) {
    println!(
        "{}",
        json!({
            "app_version": APP_VERSION,
            "bundle_id": bundle_id,
            "bundle_version": bundle_version,
            "results": results,
        })
    );
}

fn targets(paths: &[PathBuf]) -> Result<Vec<Target>, Box<dyn Error>> {
    paths
        .iter()
        .map(Target::directory)
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn ai(request: &AiRequest) -> Result<i32, Box<dyn Error>> {
    let targets = targets(&request.targets)?;
    let store: Arc<dyn StateStore> = Arc::new(DirectoryStore::new(&request.state)?);
    if request.action == "uninstall" {
        let bundle_id = request
            .bundle_id
            .as_deref()
            .ok_or("uninstall requires --bundle-id")?;
        let results = uninstall_with_options(
            bundle_id,
            targets,
            &store,
            SkillOptions::new(request.replace_modified, std::time::Duration::ZERO),
        )?;
        let exit = i32::from(results.iter().any(|result| !clean(result)));
        print_report(
            bundle_id,
            None,
            &results.iter().map(result_json).collect::<Vec<_>>(),
        );
        return Ok(exit);
    }
    let source = request.source.as_deref().ok_or("source is required")?;
    let bundle = Bundle::from_directory(source)?;
    if request.action == "status" {
        let results = inspect(&bundle, targets, &store)?;
        let exit = i32::from(
            results
                .iter()
                .any(|result| !result.observation().is_current()),
        );
        print_report(
            bundle.id(),
            Some(bundle.version()),
            &results.iter().map(inspection_json).collect::<Vec<_>>(),
        );
        return Ok(exit);
    }
    let results = if request.action == "install" {
        install(&bundle, targets, &store)?
    } else {
        update_with_options(
            &bundle,
            targets,
            &store,
            SkillOptions::new(request.replace_modified, std::time::Duration::ZERO),
        )?
    };
    let exit = i32::from(results.iter().any(|result| !clean(result)));
    print_report(
        bundle.id(),
        Some(bundle.version()),
        &results.iter().map(result_json).collect::<Vec<_>>(),
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
            println!("flyrail-notes {APP_VERSION}");
            Ok(0)
        }
        ("words", [file]) => {
            let text = fs::read_to_string(PathBuf::from(file))?;
            println!("{}", text.split_whitespace().count());
            Ok(0)
        }
        ("ai", rest) => match parse_ai(rest) {
            Ok(request) => ai(&request),
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
            eprintln!("flyrail-notes: {error}");
            2
        }
    };
    ExitCode::from(u8::try_from(code).unwrap_or(2))
}
