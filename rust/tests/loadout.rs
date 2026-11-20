#![allow(clippy::expect_used)]

use flyrail::{
    Agent, Artifact, Bundle, BundleEntry, BundleIdentity, Command, DirectoryStore, EnvRef,
    ErrorCode, HttpTransport, InstructionArtifact, LoadoutDiscovery, LoadoutOptions, LoadoutSource,
    McpArtifact, McpTransport, OperationStatus, Platform, SkillArtifact, TreeContent,
    apply_loadout, discover_loadout, inspect_loadout, preview_loadout, preview_loadout_removal,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Root(PathBuf);
impl Root {
    fn new(tag: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.cache/loadout-tests")
            .join(format!("{}-{tag}-{unique}", std::process::id()));
        fs::create_dir_all(&path).expect("test root");
        Self(path.canonicalize().expect("physical test root"))
    }
    fn write(&self, relative: &str, contents: &str) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("test parent");
        fs::write(
            path,
            contents.replace("{root}", &self.0.display().to_string()),
        )
        .expect("test input");
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn strings(test: &Value, key: &str) -> Vec<String> {
    test.get(key)
        .and_then(Value::as_array)
        .expect(key)
        .iter()
        .map(|item| item.as_str().expect("string").to_owned())
        .collect()
}

fn shared_vectors() -> Option<Value> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../spec/fixtures/loadout.json");
    let read = fs::read(fixture);
    if read
        .as_ref()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        && !Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../spec")
            .exists()
    {
        eprintln!("Shared Loadout vectors require a Flyrail checkout");
        return None;
    }
    let bytes = read.expect("read shared Loadout fixture");
    Some(serde_json::from_slice(&bytes).expect("Loadout test vectors"))
}

fn check_source_paths(setup: &flyrail::LoadoutSetup, example: &Value, root: &Path) {
    if example.get("source_paths").is_some() {
        let paths = setup
            .sources()
            .iter()
            .map(|source| {
                source
                    .path()
                    .strip_prefix(root)
                    .expect("source within fixture")
                    .display()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(paths, strings(example, "source_paths"));
    }
}

#[test]
fn discovery_uses_shared_loadout_vectors() {
    let Some(vectors) = shared_vectors() else {
        return;
    };
    for example in vectors
        .get("discovery")
        .and_then(Value::as_array)
        .expect("discovery vectors")
    {
        let root = Root::new("discovery");
        for (name, text) in example
            .get("files")
            .and_then(Value::as_object)
            .expect("files")
        {
            root.write(name, text.as_str().expect("file content"));
        }
        let env: BTreeMap<String, String> = example
            .get("env")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .expect("environment value")
                        .replace("{root}", &root.0.display().to_string()),
                )
            })
            .collect();
        let result = discover_loadout(&root.0.join("home"), None, &env).expect("explicit context");
        let expected = example
            .get("status")
            .and_then(Value::as_str)
            .expect("status");
        match result {
            LoadoutDiscovery::Absent { .. } => assert_eq!(expected, "absent", "{example}"),
            LoadoutDiscovery::Invalid { diagnostic, .. } => {
                assert_eq!(expected, "invalid", "{example}: {diagnostic:?}");
                assert_eq!(
                    Some(diagnostic.code()),
                    example.get("code").and_then(Value::as_str),
                    "{example}"
                );
            }
            LoadoutDiscovery::Available(setup) => {
                assert_eq!(expected, "available", "{example}");
                assert_eq!(
                    Some(setup.profile()),
                    example.get("profile").and_then(Value::as_str)
                );
                let chain = setup
                    .profile_chain()
                    .iter()
                    .map(|path| {
                        path.strip_prefix(&root.0)
                            .expect("under fixture")
                            .display()
                            .to_string()
                    })
                    .collect::<Vec<_>>();
                assert_eq!(chain, strings(example, "chain"));
                assert_eq!(
                    setup
                        .sources()
                        .iter()
                        .map(LoadoutSource::name)
                        .collect::<Vec<_>>(),
                    strings(example, "sources")
                );
                check_source_paths(&setup, example, &root.0);
                assert_eq!(
                    setup
                        .harnesses()
                        .iter()
                        .map(|item| item.agent().as_str())
                        .collect::<Vec<_>>(),
                    strings(example, "harnesses")
                );
                if let Some(path) = example.get("instruction_path").and_then(Value::as_str) {
                    let [selector] = setup.selectors() else {
                        unreachable!("expected one selector")
                    };
                    assert_eq!(selector.manifest(), root.0.join(path));
                    assert_eq!(selector.key(), strings(example, "instruction_key"));
                }
            }
        }
    }
}

#[test]
fn previews_use_shared_mcp_vectors() {
    let Some(vectors) = shared_vectors() else {
        return;
    };
    for example in vectors
        .get("mcp_previews")
        .and_then(Value::as_array)
        .expect("MCP preview vectors")
    {
        let root = Root::new("preview");
        for (path, contents) in example["files"].as_object().expect("fixture files") {
            root.write(path, contents.as_str().expect("file contents"));
        }
        let LoadoutDiscovery::Available(setup) =
            discover_loadout(&root.0.join("home"), None, &BTreeMap::new()).expect("context")
        else {
            unreachable!("fixture setup")
        };
        let agents = strings(example, "harnesses")
            .iter()
            .map(|value| Agent::from_str(value).expect("agent"))
            .collect::<Vec<_>>();
        let selected = setup
            .select("local", &agents, false)
            .expect("selected destination");
        let store = Arc::new(DirectoryStore::new(root.0.join("state")).expect("state store"));
        let environment = (example["env_reference"].as_bool() == Some(true))
            .then(|| ("TOKEN", EnvRef::new("REVIEW_TOKEN").expect("reference")));
        let command = Command::new(["review"], environment, None).expect("command");
        let artifact = McpArtifact::new("server", "review", McpTransport::Command(command))
            .expect("MCP server");
        let bundle = Bundle::from_artifacts(
            BundleIdentity::new("toolbox", "1").expect("bundle id"),
            [Artifact::Mcp(artifact)],
            [],
            [],
        )
        .expect("bundle");
        let plan = preview_loadout(
            &bundle,
            &selected,
            store,
            LoadoutOptions::new(Platform::Macos),
        )
        .expect("preview");
        assert_eq!(
            plan.diagnostic().map(flyrail::LoadoutDiagnostic::code),
            example["code"].as_str(),
            "{example}"
        );
        assert_eq!(plan.applicable(), example["code"].is_null(), "{plan:?}");
    }
}

fn desired(version: &str, instruction: &str) -> Bundle {
    Bundle::from_artifacts(
        BundleIdentity::new("loadout-probe", version).expect("bundle identity"),
        [
            Artifact::Instruction(
                InstructionArtifact::new("guide", instruction).expect("instruction"),
            ),
            Artifact::Mcp(
                McpArtifact::new(
                    "server",
                    "review-http",
                    McpTransport::Http(
                        HttpTransport::new(
                            "https://example.test/mcp",
                            Some(EnvRef::new("REVIEW_TOKEN").expect("token")),
                        )
                        .expect("transport"),
                    ),
                )
                .expect("MCP"),
            ),
            Artifact::Skill(
                SkillArtifact::new(
                    "skill",
                    "review",
                    TreeContent::new([
                        BundleEntry::file(
                            "SKILL.md",
                            b"---\nname: review\ndescription: Review\n---\n".to_vec(),
                        )
                        .expect("document"),
                        BundleEntry::file("assets/input.bin", vec![0, 255]).expect("asset"),
                    ])
                    .expect("tree"),
                )
                .expect("skill"),
            ),
        ],
        [],
        [],
    )
    .expect("bundle")
}

#[test]
fn loadout_source_lifecycle() {
    let root = Root::new("lifecycle");
    let registration = "home/.config/loadout/config.toml";
    let valid = format!("source = '{}'\n", root.0.join("source").display());
    let manifest = "# keep me\n[[source]]\nname='local'\npath='.'\n[claude]\ninstructions=[]\n[codex]\ninstructions=[]\n";
    root.write(registration, &valid);
    root.write("source/loadout.toml", manifest);
    let store = Arc::new(DirectoryStore::new(root.0.join("state")).expect("state store"));
    let options = LoadoutOptions::new(Platform::Macos);
    let preview = |bundle: &Bundle, agents: &[Agent]| {
        let LoadoutDiscovery::Available(setup) =
            discover_loadout(&root.0.join("home"), None, &BTreeMap::new()).expect("context")
        else {
            unreachable!("fixture setup")
        };
        let destination = setup
            .select("local", agents, false)
            .expect("explicit selection");
        preview_loadout(bundle, &destination, store.clone(), options).expect("preview")
    };
    let first = desired("1", "Read the diff.");
    let denied = preview(&first, &[Agent::Claude]);
    assert_eq!(
        denied.diagnostic().map(flyrail::LoadoutDiagnostic::code),
        Some("shared-scope")
    );
    let agents = &[Agent::Claude, Agent::Codex];
    let stale = preview(&first, agents);
    root.write(registration, &format!("{valid}# changed\n"));
    let stale = apply_loadout(&stale, Duration::ZERO);
    assert_eq!(
        stale.lifecycle().error().map(flyrail::TargetError::code),
        Some(ErrorCode::ConcurrentChange)
    );
    let plan = preview(&first, agents);
    let applied = apply_loadout(&plan, Duration::ZERO);
    assert!(applied.sync_required(), "{applied:?}");
    assert_eq!(
        applied.lifecycle().status(),
        OperationStatus::Applied,
        "{applied:?}"
    );
    assert_eq!(
        fs::read(root.0.join("source/skills/review/assets/input.bin")).expect("skill binary asset"),
        [0, 255]
    );
    let mcp = fs::read_to_string(root.0.join("source/mcp.toml")).expect("MCP file");
    assert!(mcp.contains("auth_env_var = \"REVIEW_TOKEN\""), "{mcp}");
    let repeated = apply_loadout(&preview(&first, agents), Duration::ZERO);
    assert_eq!(repeated.lifecycle().status(), OperationStatus::Unchanged);
    let upgraded = apply_loadout(
        &preview(&desired("2", "Review the patch."), agents),
        Duration::ZERO,
    );
    assert_eq!(
        upgraded.lifecycle().status(),
        OperationStatus::Applied,
        "{upgraded:?}"
    );
    root.write(registration, "source = '~/missing'\n");
    assert!(
        inspect_loadout(first.id(), plan.location(), store.clone())
            .expect("inspect by locator")
            .is_current()
    );
    let removal =
        preview_loadout_removal(first.id(), plan.location(), store, false).expect("removal");
    let removed = apply_loadout(&removal, Duration::ZERO);
    assert!(removed.sync_required());
    assert_eq!(
        removed.lifecycle().status(),
        OperationStatus::Applied,
        "{removed:?}"
    );
    assert_eq!(
        fs::read_to_string(root.0.join("source/loadout.toml")).expect("restored manifest"),
        manifest
    );
}
