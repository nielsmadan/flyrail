#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::ffi::OsString;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct TempRoot(PathBuf);

impl Deref for TempRoot {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        if self.0.exists() {
            fs::remove_dir_all(&self.0).expect("remove test root");
        }
    }
}

fn root(name: &str) -> TempRoot {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos());
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(".cache/tmp")
        .join(format!("flyrail-checksum-{name}-{nonce}"));
    fs::create_dir_all(&root).expect("create temp root");
    TempRoot(fs::canonicalize(root).expect("canonical temp root"))
}

#[test]
fn temp_root_cleanup_runs_on_drop() {
    let path = {
        let root = root("drop");
        let path = root.to_path_buf();
        assert!(path.exists());
        path
    };
    assert!(!path.exists());
}

struct BoundedReader {
    remaining: usize,
    largest: usize,
}

impl std::io::Read for BoundedReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        assert!(buffer.len() <= self.largest, "oversized read");
        let count = buffer.len().min(self.remaining);
        buffer[..count].fill(0x5a);
        self.remaining -= count;
        Ok(count)
    }
}

#[test]
fn digest_reader_uses_bounded_reads() {
    let reader = BoundedReader {
        remaining: 32 * 1024 * 1024,
        largest: 64 * 1024,
    };
    assert_eq!(
        flyrail_example_checksum::sha256_reader(reader).expect("digest"),
        "371036ccdfc733fa30542a26a4f276536147c906d1570f0becc1d6f8b868c311"
    );
}

#[test]
fn value_options_reject_following_options_without_writes() {
    let root = root("missing-values");
    for option in [
        "--target",
        "--project",
        "--agent",
        "--node",
        "--mcp-command",
        "--mcp-arg",
        "--mcp-url",
        "--guidance",
        "--asset-root",
    ] {
        let output = if option == "--target" {
            run(&["ai", "install", option, "--replace-modified"])
        } else {
            run(&["config", "status", option, "--replace-modified"])
        };
        assert_eq!(output.status.code(), Some(2), "{option}: {output:?}");
        assert!(output.stdout.is_empty(), "{option}");
        assert!(!output.stderr.is_empty(), "{option}");
    }
    assert_eq!(fs::read_dir(&*root).expect("root empty").count(), 0);
}

fn run_os(arguments: &[OsString]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_flyrail-checksum"))
        .args(arguments)
        .output()
        .expect("run checksum")
}

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_flyrail-checksum"))
        .args(arguments)
        .output()
        .expect("run checksum")
}

fn report(arguments: &[&str], code: i32) -> Value {
    let output = run(arguments);
    assert_eq!(output.status.code(), Some(code), "{output:?}");
    serde_json::from_slice(&output.stdout).expect("JSON report")
}

#[cfg(unix)]
#[test]
fn config_paths_match_strict_false_resolution() {
    use std::os::unix::fs::symlink;

    let root = root("path-resolution");
    let actual = root.join("actual");
    let child = actual.join("child");
    fs::create_dir_all(&child).expect("actual child");
    symlink("actual/child", root.join("link")).expect("normal symlink");
    symlink("actual/missing", root.join("broken")).expect("broken symlink");

    let cases = [
        (root.join("link/../future"), actual.join("future")),
        (root.join("broken/suffix"), actual.join("missing/suffix")),
        (
            actual.join("ordinary/missing"),
            actual.join("ordinary/missing"),
        ),
        (root.join("link"), child),
    ];
    for (input, expected) in cases {
        let input = input.to_str().expect("UTF-8 path");
        let expected = expected.to_str().expect("UTF-8 path");
        let value = report(
            &[
                "config",
                "status",
                "--project",
                input,
                "--agent",
                "claude",
                "--mcp-command",
                "checksum-mcp",
                "--mcp-url",
                "https://checksum.example.invalid/mcp",
            ],
            1,
        );
        assert_eq!(value["project"], expected);
        assert_eq!(
            value["observation"]["target"]["context"],
            serde_json::json!([])
        );
        let routing = value["observation"]["target"]["routing_context"]
            .as_array()
            .expect("routing pairs");
        assert!(routing.contains(&serde_json::json!(["root", expected])));
        assert!(routing.contains(&serde_json::json!(["execution", expected])));
        assert_eq!(
            value["observation"]["target"]["index_root"],
            format!("{expected}/.cache/example-checksum-config-claude")
        );
        assert!(value["observation"]["render_digest"].is_null());
        assert_eq!(value["configured_current"], false);
        assert_eq!(value["recorded_current"], false);
    }
}

fn keys(value: &Value) -> Vec<&str> {
    let mut keys = value
        .as_object()
        .expect("JSON object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys
}

fn node_24() -> Option<PathBuf> {
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    let node = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())?;
    let output = Command::new(&node).arg("--version").output().ok()?;
    let version = String::from_utf8(output.stdout).ok()?;
    let major = version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse::<u32>()
        .ok()?;
    (output.status.success() && major >= 24).then_some(node)
}

#[test]
fn report_shape_tracks_python_dataclass_fields() {
    let Some(python) = std::env::var_os("FLYRAIL_DIFFERENTIAL_PYTHON") else {
        eprintln!("SKIP: report shape comparison requires FLYRAIL_DIFFERENTIAL_PYTHON");
        return;
    };
    let script = r"
import dataclasses, json
from flyrail import (InstallationObservation, InstallationResult, InstallationTarget,
                     TargetInspection, TargetResult)
from flyrail._inventory import InventoryEntry
from flyrail._resource_models import ResourceRef
from flyrail.configuration import ClaimObservation, ResourceObservation, ResourceResult
from flyrail.observations import Installation, Observation
classes = (TargetInspection, TargetResult, Observation, Installation, InventoryEntry,
           InstallationTarget, InstallationObservation, ResourceObservation, ClaimObservation,
           InstallationResult, ResourceResult, ResourceRef)
print(json.dumps({c.__name__: sorted(f.name for f in dataclasses.fields(c)) for c in classes},
                 sort_keys=True))
";
    let python = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(python);
    let output = Command::new(python)
        .arg("-c")
        .arg(script)
        .output()
        .expect("run Python shape probe");
    assert!(output.status.success(), "{output:?}");
    let actual: Value = serde_json::from_slice(&output.stdout).expect("shape JSON");
    assert_eq!(
        actual,
        serde_json::json!({
            "ClaimObservation": ["artifact_id", "bundle_digest", "bundle_id", "claim_id",
                "render_digest", "status", "version"],
            "Installation": ["bundle_id", "content_digest", "entries", "transaction_id", "version"],
            "InstallationObservation": ["bundle_digest", "bundle_id", "error", "is_current",
                "pending", "render_digest", "resources", "target", "version"],
            "InstallationResult": ["error", "notices", "observation", "resources", "status"],
            "InstallationTarget": ["context", "index_root", "routing_context"],
            "InventoryEntry": ["executable", "path", "sha256", "size"],
            "Observation": ["conflicts", "content_matches", "error", "installations", "installed",
                "modifications", "recorded_content_matches", "recovery_paths", "state",
                "version_matches"],
            "ResourceObservation": ["claims", "destination", "error", "recovery_paths",
                "revision_digest", "state_root"],
            "ResourceRef": ["destination", "kind", "schema_version", "state_root"],
            "ResourceResult": ["error", "recovery_paths", "resource", "status"],
            "TargetInspection": ["alias_of", "observation", "root", "state_root", "target"],
            "TargetResult": ["alias_of", "error", "observation", "recovery_paths", "root", "state_root",
                "status", "target"],
        })
    );
}

#[test]
fn version_digest_usage_and_embedded_resources() {
    let root = root("host");
    let sample = root.join("sample.bin");
    fs::write(&sample, b"\x00one\ntwo").expect("write sample");
    let output = run(&["--version"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "flyrail-checksum 2.1.0"
    );
    let output = run_os(&[OsString::from("digest"), sample.into_os_string()]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("{:x}", Sha256::digest(b"\x00one\ntwo"))
    );
    assert_eq!(run(&["digest", "missing"]).status.code(), Some(2));
    assert_eq!(run(&["ai", "install"]).status.code(), Some(2));

    let bundle = flyrail_example_checksum::packaged_bundle().expect("embedded bundle");
    assert_eq!(bundle.id(), "example-checksum");
    let entries = bundle
        .entries()
        .iter()
        .map(|entry| (entry.path(), entry))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        entries["checksum-review/assets/sample.bin"].data(),
        Some(b"\x00\x01\x02\n\r\x7f\x80\xfe\xffFLYRAIL\x00".as_slice())
    );
    assert!(entries["checksum-review/scripts/verify_checksum.py"].executable());
    assert!(entries.contains_key("checksum-review/scratch/README.txt"));
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_work_and_non_utf8_text_is_rejected_without_panic() {
    use std::os::unix::ffi::OsStringExt as _;

    let root = root("non-utf8");
    let file = root.join(OsString::from_vec(b"sample-\xff.bin".to_vec()));
    if let Err(error) = fs::write(&file, b"payload") {
        if error.kind() == std::io::ErrorKind::PermissionDenied && std::env::var_os("CI").is_none()
        {
            eprintln!("SKIP: active sandbox rejects non-UTF-8 filesystem names");
            return;
        }
        panic!("write non-UTF-8 path: {error}");
    }
    let output = run_os(&[OsString::from("digest"), file.into_os_string()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("{:x}", Sha256::digest(b"payload"))
    );

    let missing = root.join(OsString::from_vec(b"missing-\xff".to_vec()));
    let output = run_os(&[OsString::from("digest"), missing.into_os_string()]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(!output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));

    let output = run_os(&[
        OsString::from("config"),
        OsString::from("status"),
        OsString::from("--project"),
        root.as_os_str().to_owned(),
        OsString::from("--agent"),
        OsString::from_vec(b"bad-\xff".to_vec()),
    ]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
}

#[test]
fn digest_streams_a_bounded_generated_file() {
    use std::io::Write as _;

    let root = root("large-digest");
    let file = root.join("large.bin");
    let chunk = vec![0x5a; 1024 * 1024];
    let mut expected = Sha256::new();
    let mut output_file = fs::File::create(&file).expect("create generated file");
    for _ in 0..32 {
        output_file.write_all(&chunk).expect("write generated file");
        expected.update(&chunk);
    }
    drop(output_file);
    let output = run_os(&[OsString::from("digest"), file.into_os_string()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("{:x}", expected.finalize())
    );
}

#[test]
fn packaged_lifecycle_is_repeatable_multi_target_and_source_independent() {
    let root = root("lifecycle");
    let first = root.join("first");
    let second = root.join("second");
    let args = [
        "--target",
        first.to_str().expect("UTF-8"),
        "--target",
        second.to_str().expect("UTF-8"),
    ];
    let mut command = vec!["ai", "status"];
    command.extend(args);
    let status = report(&command, 1);
    assert_eq!(status["results"][0]["observation"]["state"], "absent");
    assert_eq!(
        keys(&status["results"][0]),
        [
            "alias_of",
            "is_current",
            "observation",
            "root",
            "state_root",
            "target"
        ]
    );
    assert_eq!(
        keys(&status["results"][0]["observation"]),
        [
            "conflicts",
            "content_matches",
            "error",
            "installations",
            "installed",
            "modifications",
            "recorded_content_matches",
            "recovery_paths",
            "state",
            "version_matches",
        ]
    );
    command[1] = "install";
    let installed = report(&command, 0);
    assert_eq!(installed["results"][0]["status"], "applied");
    assert_eq!(
        keys(&installed["results"][0]["observation"]["installed"]),
        [
            "bundle_id",
            "content_digest",
            "entries",
            "transaction_id",
            "version"
        ]
    );
    assert!(
        installed["results"][0]["observation"]["installed"]["entries"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty())
    );
    let bundle = flyrail_example_checksum::packaged_bundle().expect("packaged bundle");
    let inspected = flyrail::inspect(&bundle, [first.clone()]).expect("inspect installation");
    let expected = inspected[0]
        .observation()
        .installed()
        .expect("installed model");
    assert_eq!(
        installed["results"][0]["observation"]["installed"],
        serde_json::json!({
            "bundle_id": expected.bundle_id(),
            "version": expected.version(),
            "content_digest": expected.content_digest(),
            "transaction_id": expected.transaction_id(),
            "entries": expected.entries().iter().map(|entry| serde_json::json!({
                "path": entry.path(),
                "size": entry.size(),
                "sha256": entry.sha256(),
                "executable": entry.executable(),
            })).collect::<Vec<_>>(),
        })
    );
    assert_eq!(report(&command, 0)["results"][1]["status"], "unchanged");
    let script = first.join("checksum-review/scripts/verify_checksum.py");
    assert!(script.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&script)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }
    fs::write(first.join("checksum-review/SKILL.md"), "local edit").expect("edit");
    command[1] = "update";
    let modified = report(&command, 1);
    assert_eq!(modified["results"][0]["error"]["code"], "modified");
    assert_eq!(
        modified["results"][0]["observation"]["modifications"][0],
        serde_json::json!({
            "bundle_id": "example-checksum",
            "kind": "content_changed",
            "path": "checksum-review"
        })
    );
    assert_eq!(
        modified["results"][0]["recovery_paths"],
        serde_json::json!([])
    );
    command.push("--replace-modified");
    assert_eq!(report(&command, 0)["results"][0]["status"], "applied");
    command = vec!["ai", "uninstall"];
    command.extend(args);
    assert_eq!(report(&command, 0)["results"][0]["status"], "applied");
    assert!(!first.exists());
}

#[test]
fn combined_configuration_executes_hook_and_covers_every_agent() {
    let Some(node) = node_24() else {
        let notice = "SKIP: combined configuration example requires Node 24+";
        assert!(
            std::env::var_os("CI").is_none(),
            "{notice}; CI may not skip"
        );
        eprintln!("{notice}");
        return;
    };
    let root = root("config");
    let project = root.join("project");
    fs::create_dir_all(&project).expect("project");
    fs::write(project.join("CLAUDE.md"), "# Existing\n").expect("existing guidance");
    #[cfg(unix)]
    let project_argument = {
        let alias = root.join("project-alias");
        std::os::unix::fs::symlink(&project, &alias).expect("project symlink");
        alias
    };
    #[cfg(not(unix))]
    let project_argument = project.clone();
    let node = node.to_str().expect("UTF-8 Node path");
    let base = [
        "--project",
        project_argument.to_str().expect("UTF-8"),
        "--agent",
        "claude",
        "--node",
        node,
        "--mcp-command",
        "checksum-mcp",
        "--mcp-arg",
        "--",
        "--stdio",
        "--mcp-arg",
        "second",
        "--mcp-url",
        "https://checksum.example.invalid/mcp",
    ];
    let mut command = vec!["config", "status"];
    command.extend(base);
    assert_eq!(report(&command, 1)["configured_current"], false);
    command[1] = "update";
    let installed = report(&command, 0);
    let bundle = flyrail_example_checksum::configuration_bundle(
        flyrail_example_checksum::GUIDANCE,
        &[
            String::from("checksum-mcp"),
            String::from("--stdio"),
            String::from("second"),
        ],
        "https://checksum.example.invalid/mcp",
    )
    .expect("configuration bundle");
    let platform = if cfg!(target_os = "macos") {
        flyrail::Platform::Macos
    } else if cfg!(target_os = "windows") {
        flyrail::Platform::Windows
    } else {
        flyrail::Platform::Linux
    };
    let context = flyrail::RenderContext::new(
        flyrail::Target::project("claude".parse().expect("agent"), &project)
            .expect("project target"),
        platform,
    )
    .expect("render context")
    .with_hook_runtime(
        flyrail::HookRuntime::new(
            Some(fs::canonicalize(node).expect("canonical Node path")),
            Some(if cfg!(target_os = "windows") {
                flyrail::HookShell::Cmd
            } else {
                flyrail::HookShell::Posix
            }),
        )
        .expect("hook runtime"),
    );
    let target = context
        .installation(
            project
                .join(".cache")
                .join("example-checksum-config-claude"),
        )
        .expect("installation target");
    let rendered = flyrail::render(&bundle, &context).expect("render configuration");
    let expected =
        flyrail::inspect_installation(flyrail_example_checksum::CONFIG_BUNDLE_ID, &target)
            .expect("inspect configuration");
    assert_eq!(installed["configured_current"], true);
    assert_eq!(installed["recorded_current"], true);
    assert_eq!(installed["activation"], "not-checked");
    assert_eq!(installed["observation"]["bundle_id"], expected.bundle_id());
    assert_eq!(
        installed["observation"]["version"],
        serde_json::json!(expected.version())
    );
    assert_eq!(
        installed["observation"]["bundle_digest"],
        serde_json::json!(expected.bundle_digest())
    );
    assert_eq!(
        installed["observation"]["render_digest"],
        serde_json::json!(expected.render_digest())
    );
    assert_eq!(expected.bundle_digest(), Some(bundle.content_digest()));
    assert_eq!(expected.render_digest(), Some(rendered.content_digest()));
    assert_eq!(
        installed["observation"]["target"]["context"],
        serde_json::json!(expected.target().context())
    );
    assert_eq!(
        installed["observation"]["target"]["routing_context"],
        serde_json::json!(expected.target().routing_context())
    );
    assert_eq!(
        installed["observation"]["target"]["index_root"],
        expected.target().index_root().display().to_string()
    );
    assert_eq!(
        keys(&installed["observation"]),
        [
            "bundle_digest",
            "bundle_id",
            "error",
            "is_current",
            "pending",
            "render_digest",
            "resources",
            "target",
            "version",
        ]
    );
    assert_eq!(
        keys(&installed["observation"]["target"]),
        ["context", "index_root", "routing_context"]
    );
    assert_eq!(
        installed["observation"]["target"]["context"],
        serde_json::json!([])
    );
    let routing = installed["observation"]["target"]["routing_context"]
        .as_array()
        .expect("routing context");
    assert!(routing.contains(&serde_json::json!([
        "root",
        project.to_str().expect("UTF-8 project")
    ])));
    assert!(routing.contains(&serde_json::json!(["agent", "claude"])));
    assert!(
        installed["observation"]["resources"]
            .as_array()
            .is_some_and(|resources| resources.iter().all(|resource| {
                keys(resource)
                    == [
                        "claims",
                        "destination",
                        "error",
                        "recovery_paths",
                        "revision_digest",
                        "state_root",
                    ]
            }))
    );
    let expected_resources = expected
        .resources()
        .iter()
        .map(|resource| {
            serde_json::json!({
                "destination": resource.destination().display().to_string(),
                "state_root": resource.state_root().display().to_string(),
                "claims": resource.claims().iter().map(|claim| serde_json::json!({
                    "claim_id": claim.claim_id(),
                    "bundle_id": claim.bundle_id(),
                    "artifact_id": claim.artifact_id(),
                    "version": claim.version(),
                    "bundle_digest": claim.bundle_digest(),
                    "render_digest": claim.render_digest(),
                    "status": claim.status().as_str(),
                })).collect::<Vec<_>>(),
                "revision_digest": resource.revision_digest(),
                "error": null,
                "recovery_paths": resource.recovery_paths().iter()
                    .map(|path| path.display().to_string()).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        installed["observation"]["resources"],
        serde_json::json!(expected_resources)
    );
    let preview = flyrail::preview(
        &bundle,
        &rendered,
        &target,
        flyrail::Acquisition::Conflict,
        false,
    )
    .expect("preview configuration");
    for (actual, planned) in installed["resources"]
        .as_array()
        .expect("operation resources")
        .iter()
        .zip(preview.resources())
    {
        let expected_ref = planned.resource();
        assert_eq!(
            actual["resource"]["destination"],
            expected_ref.destination().display().to_string()
        );
        assert_eq!(
            actual["resource"]["state_root"],
            expected_ref.state_root().display().to_string()
        );
        let kind = match expected_ref.kind() {
            flyrail::ResourceKind::Document => "document",
            flyrail::ResourceKind::Tree => "tree",
            flyrail::ResourceKind::SkillContainer => "skill-container",
        };
        assert_eq!(actual["resource"]["kind"], kind);
        assert_eq!(
            actual["resource"]["schema_version"],
            expected_ref.schema_version()
        );
    }
    assert_eq!(
        installed["resources"]
            .as_array()
            .expect("operation resources")
            .len(),
        preview.resources().len()
    );
    assert!(
        installed["resources"]
            .as_array()
            .is_some_and(|resources| resources.iter().all(|resource| {
                keys(resource) == ["error", "recovery_paths", "resource", "status"]
                    && keys(&resource["resource"])
                        == ["destination", "kind", "schema_version", "state_root"]
            }))
    );
    let observation_resource = &installed["observation"]["resources"][0];
    assert_eq!(observation_resource["error"], Value::Null);
    assert_eq!(
        observation_resource["recovery_paths"],
        serde_json::json!([])
    );
    assert!(
        observation_resource["destination"]
            .as_str()
            .is_some_and(|path| path.ends_with("/.claude/skills"))
    );
    assert!(
        observation_resource["state_root"]
            .as_str()
            .is_some_and(|path| path.contains("/.claude/.flyrail-"))
    );
    let expected_resource = &expected.resources()[0];
    assert_eq!(
        observation_resource["destination"],
        expected_resource.destination().display().to_string()
    );
    assert_eq!(
        observation_resource["state_root"],
        expected_resource.state_root().display().to_string()
    );
    assert_eq!(
        observation_resource["revision_digest"],
        serde_json::json!(expected_resource.revision_digest())
    );
    let claim = &observation_resource["claims"][0];
    let expected_claim = &expected_resource.claims()[0];
    assert_eq!(claim["artifact_id"], expected_claim.artifact_id());
    assert_eq!(claim["bundle_id"], expected_claim.bundle_id());
    assert_eq!(claim["version"], expected_claim.version());
    assert_eq!(claim["bundle_digest"], expected_claim.bundle_digest());
    assert_eq!(claim["render_digest"], expected_claim.render_digest());
    assert_eq!(claim["status"], expected_claim.status().as_str());
    assert_eq!(claim["claim_id"], expected_claim.claim_id());
    let resource = &installed["resources"][0];
    assert_eq!(resource["status"], "applied");
    assert_eq!(resource["error"], Value::Null);
    assert_eq!(resource["recovery_paths"], serde_json::json!([]));
    assert_eq!(resource["resource"]["kind"], "skill-container");
    assert_eq!(resource["resource"]["schema_version"], 2);
    assert_eq!(
        resource["resource"]["destination"],
        observation_resource["destination"]
    );
    assert_eq!(
        resource["resource"]["state_root"],
        observation_resource["state_root"]
    );
    assert!(
        installed["prerequisites"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(
        project
            .join(".claude/skills/checksum-review/SKILL.md")
            .is_file()
    );
    assert!(project.join("CLAUDE.md").is_file());
    assert!(project.join(".mcp.json").is_file());
    assert!(project.join(".claude/settings.json").is_file());
    assert!(project.join(".flyrail-assets").is_dir());
    let mcp: Value =
        serde_json::from_slice(&fs::read(project.join(".mcp.json")).expect("MCP config"))
            .expect("MCP JSON");
    assert_eq!(
        mcp["mcpServers"]["checksum-local"]["args"],
        serde_json::json!(["--stdio", "second"])
    );
    assert_eq!(
        mcp["mcpServers"]["checksum-local"]["env"],
        serde_json::json!({"CHECKSUM_MCP_TOKEN": "${CHECKSUM_MCP_TOKEN}"})
    );
    assert_eq!(
        mcp["mcpServers"]["checksum-remote"]["headers"],
        serde_json::json!({"Authorization": "Bearer ${CHECKSUM_HTTP_TOKEN}"})
    );
    let settings: Value =
        serde_json::from_slice(&fs::read(project.join(".claude/settings.json")).expect("settings"))
            .expect("settings JSON");
    let hook = &settings["hooks"]["PreToolUse"][0]["hooks"][0];
    let hook_command = hook["command"].as_str().expect("hook command");
    let hook_args = hook["args"]
        .as_array()
        .expect("hook args")
        .iter()
        .map(|item| item.as_str().expect("hook string"))
        .collect::<Vec<_>>();
    let reminder_assets = installed["resources"]
        .as_array()
        .expect("resource results")
        .iter()
        .map(|resource| {
            resource["resource"]["destination"]
                .as_str()
                .expect("resource destination")
        })
        .find(|destination| destination.contains("/reminder-assets/"))
        .expect("reminder asset resource");
    let hook_output = Command::new(hook_command)
        .args(&hook_args)
        .current_dir(&project)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child
                .stdin
                .as_mut()
                .expect("hook stdin")
                .write_all(br#"{"tool_name":"Bash","tool_input":{}}"#)?;
            child.wait_with_output()
        })
        .expect("execute installed hook");
    assert!(hook_output.status.success(), "{hook_output:?}");
    assert!(hook_output.stderr.is_empty(), "{hook_output:?}");
    assert_eq!(
        serde_json::from_slice::<Value>(&hook_output.stdout).expect("hook JSON"),
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext":
                    "Verify a downloaded file with flyrail-checksum digest FILE before using it."
            }
        })
    );
    assert_eq!(
        fs::read(Path::new(reminder_assets).join("reminder.txt")).expect("installed reminder"),
        include_bytes!("../resources/assets/reminder.txt")
    );
    assert_eq!(report(&command, 0)["status"], "unchanged");
    let mut real_route = command.clone();
    real_route[3] = project.to_str().expect("UTF-8");
    let aliased = report(&command, 0);
    let real = report(&real_route, 0);
    assert_eq!(aliased["project"], real["project"]);
    assert_eq!(
        aliased["observation"]["target"]["routing_context"],
        real["observation"]["target"]["routing_context"]
    );
    assert_eq!(
        aliased["observation"]["render_digest"],
        real["observation"]["render_digest"]
    );
    assert_eq!(aliased["configured_current"], real["configured_current"]);

    command[1] = "status";
    command.extend(["--guidance", "Changed guidance"]);
    let drift = report(&command, 1);
    assert_eq!(drift["recorded_current"], true);
    assert_eq!(drift["configured_current"], false);

    for (agent, supported) in [
        ("claude", true),
        ("codex", true),
        ("opencode", false),
        ("pi", false),
        ("cursor", false),
        ("copilot", false),
    ] {
        let candidate = root.join(format!("agent-{agent}"));
        let candidate_text = candidate.to_str().expect("UTF-8");
        let arguments = [
            "config",
            "status",
            "--project",
            candidate_text,
            "--agent",
            agent,
            "--node",
            node,
            "--mcp-command",
            "checksum-mcp",
            "--mcp-arg",
            "one",
            "--mcp-arg",
            "two",
            "--mcp-url",
            "https://checksum.example.invalid/mcp",
        ];
        let status = report(&arguments, 1);
        assert!(!candidate.exists(), "status wrote for {agent}");
        let mut update = arguments;
        update[1] = "update";
        let updated = report(&update, i32::from(!supported));
        if supported {
            assert_eq!(updated["status"], "applied", "{agent}");
            let removed = report(
                &[
                    "config",
                    "uninstall",
                    "--project",
                    candidate_text,
                    "--agent",
                    agent,
                ],
                0,
            );
            assert_eq!(removed["status"], "applied", "{agent}");
        } else {
            assert!(
                status["notices"].as_array().is_some_and(|notices| notices
                    .iter()
                    .any(|notice| notice["kind"] == "unsupported")),
                "{agent}"
            );
            assert_eq!(updated["error"]["code"], "unsupported", "{agent}");
            assert!(!candidate.exists(), "unsupported update wrote for {agent}");
        }
    }

    let removed = report(
        &[
            "config",
            "uninstall",
            "--project",
            project.to_str().expect("UTF-8"),
            "--agent",
            "claude",
        ],
        0,
    );
    assert_eq!(removed["status"], "applied");
    assert_eq!(removed["bundle_version"], Value::Null);
    assert_eq!(
        fs::read(project.join("CLAUDE.md")).expect("guidance"),
        b"# Existing\n"
    );
}
