use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::PathBuf;
use std::str::FromStr as _;
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use flyrail::{
    Acquisition, Artifact, Bundle, BundleIdentity, Content, Family, FileContent, FileMode, Index,
    InstallationObservation, InstallationResult, InstallationTarget, InstructionArtifact, Journal,
    Receipt, RenderedArtifact, RenderedBundle, ResourceAuthority, ResourceKind, ResourceRef,
    TargetError, admit, decode, encode, inspect_installation, interop_resource_lock,
    recover_installation, remove, set_interop_test_barrier, set_interop_test_fault, sync,
};
use serde_json::{Value, json};

const MAX_PAYLOAD_SIZE: u64 = 1024 * 1024;

fn main() {
    if let Err(error) = run() {
        if env::args().nth(1).as_deref() == Some("encode") {
            eprintln!("{error}");
        } else {
            let value =
                json!({"error": {"code": Value::Null, "message": error, "path": Value::Null}});
            println!("{value}");
        }
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut arguments = Arguments::new(env::args().skip(1))?;
    match arguments.command.as_str() {
        "encode" => encode_command(&mut arguments),
        "write" | "install" | "update" => write_command(&mut arguments),
        "inspect" => inspect_command(&mut arguments),
        "remove" => remove_command(&mut arguments),
        "recover" => recover_command(&mut arguments),
        "records" => records_command(&mut arguments),
        "hold" => hold_command(&mut arguments),
        _ => Err(usage()),
    }
}

struct Arguments {
    command: String,
    values: BTreeMap<String, String>,
    flags: BTreeSet<String>,
}

impl Arguments {
    fn new(arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut arguments = arguments;
        let command = arguments.next().ok_or_else(usage)?;
        let (value_options, flag_options): (&[&str], &[&str]) = match command.as_str() {
            "encode" => (&["--type"], &[]),
            "write" | "install" | "update" => (
                &[
                    "--index",
                    "--bundle-id",
                    "--lock-timeout",
                    "--destination",
                    "--version",
                    "--payload",
                    "--large",
                    "--mode",
                    "--acquisition",
                    "--barrier-phase",
                    "--barrier-ready",
                    "--barrier-release",
                    "--barrier-timeout",
                ],
                &["--replace-modified", "--fail-index-completion"],
            ),
            "inspect" | "recover" => (&["--index", "--bundle-id", "--lock-timeout"], &[]),
            "remove" => (
                &["--index", "--bundle-id", "--lock-timeout"],
                &["--replace-modified"],
            ),
            "records" => (&["--index", "--destination"], &[]),
            "hold" => (
                &[
                    "--destination",
                    "--barrier",
                    "--kind",
                    "--lock-timeout",
                    "--wait-timeout",
                ],
                &[],
            ),
            _ => return Err(usage()),
        };
        let mut values = BTreeMap::new();
        let mut flags = BTreeSet::new();
        while let Some(argument) = arguments.next() {
            if flag_options.contains(&argument.as_str()) {
                if !flags.insert(argument.clone()) {
                    return Err(format!("duplicate option: {argument}"));
                }
            } else if value_options.contains(&argument.as_str()) {
                let value = arguments
                    .next()
                    .ok_or_else(|| format!("missing value for {argument}"))?;
                if value.starts_with("--") {
                    return Err(format!("missing value for {argument}"));
                }
                if values.insert(argument.clone(), value).is_some() {
                    return Err(format!("duplicate option: {argument}"));
                }
            } else if argument.starts_with("--") {
                return Err(format!("unknown option: {argument}"));
            } else {
                return Err(usage());
            }
        }
        Ok(Self {
            command,
            values,
            flags,
        })
    }

    fn required(&mut self, name: &str) -> Result<String, String> {
        self.values
            .remove(name)
            .ok_or_else(|| format!("missing {name}"))
    }

    fn value(&mut self, name: &str, default: &str) -> String {
        self.values
            .remove(name)
            .unwrap_or_else(|| default.to_owned())
    }

    fn optional(&mut self, name: &str) -> Option<String> {
        self.values.remove(name)
    }

    fn flag(&mut self, name: &str) -> bool {
        self.flags.remove(name)
    }

    fn finish(&self) -> Result<(), String> {
        if self.values.is_empty() && self.flags.is_empty() {
            Ok(())
        } else {
            Err(usage())
        }
    }
}

fn target(arguments: &mut Arguments) -> Result<InstallationTarget, String> {
    let index = absolute(arguments.required("--index")?)?;
    InstallationTarget::without_context(index).map_err(|error| error.to_string())
}

fn absolute(path: String) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        Ok(path)
    } else {
        env::current_dir()
            .map(|root| root.join(path))
            .map_err(|error| error.to_string())
    }
}

fn bundle(version: &str) -> Result<Bundle, String> {
    Bundle::from_artifacts(
        BundleIdentity::new("team", version).map_err(|error| error.to_string())?,
        [Artifact::Instruction(
            InstructionArtifact::new("guide", "text").map_err(|error| error.to_string())?,
        )],
        [],
        [],
    )
    .map_err(|error| error.to_string())
}

fn payload(arguments: &mut Arguments) -> Result<Vec<u8>, String> {
    if let Some(size) = arguments.optional("--large") {
        let size = size
            .parse::<u64>()
            .map_err(|error| format!("invalid --large: {error}"))?;
        if size > MAX_PAYLOAD_SIZE {
            return Err(format!(
                "large payload size must not exceed {MAX_PAYLOAD_SIZE}"
            ));
        }
        let size = usize::try_from(size).map_err(|error| format!("invalid --large: {error}"))?;
        let pattern = b"flyrail-interop-payload\n";
        return Ok(pattern.iter().copied().cycle().take(size).collect());
    }
    Ok(arguments.value("--payload", "generated\n").into_bytes())
}

fn rendered(arguments: &mut Arguments) -> Result<RenderedBundle, String> {
    let destination = absolute(arguments.required("--destination")?)?;
    let mode = match arguments.value("--mode", "readable").as_str() {
        "private" => FileMode::Private,
        "readable" => FileMode::Readable,
        "executable" => FileMode::Executable,
        value => return Err(format!("invalid mode: {value}")),
    };
    let artifact = RenderedArtifact::new(
        "guide",
        Family::Instructions,
        destination,
        Content::File(FileContent::new(payload(arguments)?, mode)),
        None,
        false,
        Vec::new(),
    )
    .map_err(|error| error.to_string())?;
    RenderedBundle::new([artifact], [], []).map_err(|error| error.to_string())
}

fn timeout(arguments: &mut Arguments) -> Result<Duration, String> {
    duration("--lock-timeout", &arguments.value("--lock-timeout", "0"))
}

fn duration(name: &str, value: &str) -> Result<Duration, String> {
    let seconds = value
        .parse::<f64>()
        .map_err(|error| format!("invalid {name}: {error}"))?;
    Duration::try_from_secs_f64(seconds).map_err(|error| format!("invalid {name}: {error}"))
}

fn configure_interop_seam(arguments: &mut Arguments) -> Result<(), String> {
    let phase = arguments.optional("--barrier-phase");
    let ready = arguments.optional("--barrier-ready");
    let release = arguments.optional("--barrier-release");
    let barrier_timeout = arguments.optional("--barrier-timeout");
    let index_fault = arguments.flag("--fail-index-completion");
    if index_fault
        && (phase.is_some() || ready.is_some() || release.is_some() || barrier_timeout.is_some())
    {
        return Err(String::from("fault and barrier options cannot be combined"));
    }
    if index_fault {
        return set_interop_test_fault(String::from("index-completion"));
    }
    match (phase, ready, release) {
        (None, None, None) => {
            if barrier_timeout.is_some() {
                Err(String::from("--barrier-timeout requires --barrier-phase"))
            } else {
                Ok(())
            }
        }
        (Some(phase), Some(ready), Some(release)) => {
            if !matches!(phase.as_str(), "prepare" | "receipt-committed") {
                return Err(format!("invalid --barrier-phase: {phase}"));
            }
            let wait = duration(
                "--barrier-timeout",
                barrier_timeout.as_deref().unwrap_or("30"),
            )?;
            set_interop_test_barrier(phase, absolute(ready)?, absolute(release)?, wait)
        }
        _ => Err(String::from(
            "--barrier-phase requires --barrier-ready and --barrier-release",
        )),
    }
}

fn write_command(arguments: &mut Arguments) -> Result<(), String> {
    let installation = target(arguments)?;
    let version = arguments.value("--version", "1");
    let acquisition = Acquisition::from_str(&arguments.value("--acquisition", "conflict"))
        .map_err(|error| error.to_string())?;
    let replace_modified = arguments.flag("--replace-modified");
    let rendered = rendered(arguments)?;
    let lock_timeout = timeout(arguments)?;
    arguments.optional("--bundle-id");
    configure_interop_seam(arguments)?;
    arguments.finish()?;
    let result = sync(
        &bundle(&version)?,
        &rendered,
        &installation,
        acquisition,
        replace_modified,
        lock_timeout,
    )
    .map_err(|error| error.to_string())?;
    println!("{}", result_value(&result));
    Ok(())
}

fn inspect_command(arguments: &mut Arguments) -> Result<(), String> {
    let installation = target(arguments)?;
    let bundle_id = arguments.value("--bundle-id", "team");
    arguments.optional("--lock-timeout");
    arguments.finish()?;
    let observation =
        inspect_installation(&bundle_id, &installation).map_err(|error| error.to_string())?;
    println!("{}", observation_value(&observation));
    Ok(())
}

fn remove_command(arguments: &mut Arguments) -> Result<(), String> {
    let installation = target(arguments)?;
    let bundle_id = arguments.value("--bundle-id", "team");
    let replace_modified = arguments.flag("--replace-modified");
    let lock_timeout = timeout(arguments)?;
    arguments.finish()?;
    let result = remove(&bundle_id, &installation, replace_modified, lock_timeout)
        .map_err(|error| error.to_string())?;
    println!("{}", result_value(&result));
    Ok(())
}

fn recover_command(arguments: &mut Arguments) -> Result<(), String> {
    let installation = target(arguments)?;
    let bundle_id = arguments.value("--bundle-id", "team");
    let lock_timeout = timeout(arguments)?;
    arguments.finish()?;
    let result = recover_installation(&bundle_id, &installation, lock_timeout)
        .map_err(|error| error.to_string())?;
    println!("{}", result_value(&result));
    Ok(())
}

fn error_value(error: Option<&TargetError>) -> Value {
    error.map_or(Value::Null, |error| {
        json!({
            "code": error.code().as_str(),
            "message": error.message(),
            "path": error.path().map(|path| path.display().to_string()),
        })
    })
}

fn observation_value(observation: &InstallationObservation) -> Value {
    json!({
        "bundle_id": observation.bundle_id(),
        "is_current": observation.is_current(),
        "pending": observation.pending(),
        "version": observation.version(),
        "error": error_value(observation.error()),
        "resources": observation.resources().iter().map(|resource| json!({
            "destination": resource.destination().display().to_string(),
            "state_root": resource.state_root().display().to_string(),
            "claims": resource.claims().iter().map(|claim| json!({
                "artifact_id": claim.artifact_id(),
                "disposition": claim.status().as_str(),
            })).collect::<Vec<_>>(),
            "error": error_value(resource.error()),
            "recovery_paths": resource.recovery_paths().iter()
                .map(|path| path.display().to_string()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn result_value(result: &InstallationResult) -> Value {
    json!({
        "status": result.status().as_str(),
        "error": error_value(result.error()),
        "observation": observation_value(result.observation()),
        "resources": result.resources().iter().map(|resource| json!({
            "destination": resource.resource().destination().display().to_string(),
            "status": resource.status().as_str(),
            "error": error_value(resource.error()),
            "recovery_paths": resource.recovery_paths().iter()
                .map(|path| path.display().to_string()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn encode_command(arguments: &mut Arguments) -> Result<(), String> {
    let state_type = arguments.required("--type")?;
    arguments.finish()?;
    let mut input = Vec::new();
    io::stdin()
        .read_to_end(&mut input)
        .map_err(|error| format!("failed to read stdin: {error}"))?;
    let output = canonical_record(&state_type, &input)?;
    io::stdout()
        .write_all(&output)
        .map_err(|error| format!("failed to write stdout: {error}"))
}

fn canonical_record(state_type: &str, input: &[u8]) -> Result<Vec<u8>, String> {
    match state_type {
        "Index" => Ok(encode(
            &decode::<Index>(input).map_err(|error| format!("decode Index: {error}"))?,
        )),
        "Journal" => Ok(encode(
            &decode::<Journal>(input).map_err(|error| format!("decode Journal: {error}"))?,
        )),
        "Receipt" => Ok(encode(
            &decode::<Receipt>(input).map_err(|error| format!("decode Receipt: {error}"))?,
        )),
        "ResourceRef" => Ok(encode(
            &decode::<ResourceRef>(input)
                .map_err(|error| format!("decode ResourceRef: {error}"))?,
        )),
        _ => Err(format!("unsupported state type: {state_type}")),
    }
}

fn records_command(arguments: &mut Arguments) -> Result<(), String> {
    let index = absolute(arguments.required("--index")?)?;
    let destination = arguments
        .optional("--destination")
        .map(absolute)
        .transpose()?;
    arguments.finish()?;
    let mut roots = vec![index];
    if let Some(destination) = destination {
        roots.push(
            ResourceAuthority::new(destination)
                .map_err(|error| error.to_string())?
                .state_root()
                .to_path_buf(),
        );
    }
    let mut paths = roots
        .into_iter()
        .filter(|root| root.is_dir())
        .flat_map(|root| {
            fs::read_dir(root)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|entry| entry.path())
        })
        .filter(|path| {
            path.is_file()
                && matches!(
                    path.file_name().and_then(|name| name.to_str()),
                    Some(
                        "authority.json"
                            | "authority.json.next"
                            | "index.json"
                            | "index.json.next"
                            | "receipt.json"
                            | "receipt.json.next"
                            | "transaction.json"
                            | "transaction.json.next"
                    )
                )
        })
        .collect::<Vec<_>>();
    paths.sort();
    let mut records = Vec::new();
    for path in paths {
        let raw = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let parsed: Value = serde_json::from_slice(&raw).map_err(|error| error.to_string())?;
        let state_type = parsed
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("record type missing: {}", path.display()))?;
        let canonical = canonical_record(state_type, &raw)?;
        records.push(json!({
            "path": path.display().to_string(),
            "type": state_type,
            "bytes": base64::engine::general_purpose::STANDARD.encode(&raw),
            "canonical": base64::engine::general_purpose::STANDARD.encode(canonical),
        }));
    }
    println!("{}", json!({"records": records}));
    Ok(())
}

fn hold_command(arguments: &mut Arguments) -> Result<(), String> {
    let destination = absolute(arguments.required("--destination")?)?;
    let barrier = absolute(arguments.required("--barrier")?)?;
    let kind = ResourceKind::from_str(&arguments.value("--kind", "document"))
        .map_err(|e| e.to_string())?;
    let wait_timeout = duration("--wait-timeout", &arguments.value("--wait-timeout", "30"))?;
    let lock_timeout = timeout(arguments)?;
    arguments.finish()?;
    let authority = ResourceAuthority::new(&destination).map_err(|error| error.to_string())?;
    let reference = ResourceRef::new(
        authority.destination().to_path_buf(),
        kind,
        authority.state_root().to_path_buf(),
    )
    .map_err(|error| error.to_string())?;
    let _guard =
        interop_resource_lock(&reference, lock_timeout).map_err(|error| error.to_string())?;
    println!("{}", json!({"status": "reserved"}));
    io::stdout().flush().map_err(|error| error.to_string())?;
    let deadline = Instant::now()
        .checked_add(wait_timeout)
        .ok_or_else(|| String::from("invalid --wait-timeout: duration is too large"))?;
    while !barrier.exists() {
        if Instant::now() >= deadline {
            return Err(String::from("barrier timed out"));
        }
        thread::sleep(Duration::from_millis(5));
    }
    match admit(&reference) {
        Ok(()) => println!("{}", json!({"status": "admitted"})),
        Err(error) => println!("{}", json!({"status": error.code().as_str()})),
    }
    Ok(())
}

fn usage() -> String {
    String::from(
        "usage: interop encode|write|install|update|inspect|remove|recover|records|hold [options]",
    )
}
