#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

use flyrail::{
    Agent, AssetRef, Bundle, BundleEntry, Command, CommandArgument, Content, Dependency,
    DocumentFormat, EnvRef, EnvironmentValue, HttpTransport, InstructionArtifact, McpArtifact,
    McpTransport, NoticeKind, ObjectValue, Platform, RenderContext, Scalar, SelectorPart, Surface,
    Target, Value, assemble_translations, dependency_order, render, render_many,
    translate_instruction_artifact, translate_mcp_artifact,
};
use serde_json::Value as JsonValue;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repository root")
        .join(".cache/translation-fixture-root")
}

fn context(case: &JsonValue, platform: Platform) -> RenderContext {
    let root = root();
    let agent = Agent::from_str(case["agent"].as_str().expect("agent")).expect("agent value");
    let surface =
        Surface::from_str(case["surface"].as_str().expect("surface")).expect("surface value");
    let target = if case["scope"] == "project" {
        Target::project(agent, &root).expect("project target")
    } else {
        Target::user_with_home_and_environment(agent, &root, std::iter::empty::<(String, String)>())
            .expect("user target")
    };
    let mut context = RenderContext::with_surface(target, platform, surface).expect("context");
    if let Some(path) = case.get("execution_root").and_then(JsonValue::as_str) {
        context = context
            .with_execution_root(root.join(path))
            .expect("execution root");
    }
    if let Some(path) = case.get("mcp_path").and_then(JsonValue::as_str) {
        context = context.with_mcp_path(root.join(path)).expect("MCP path");
    }
    context
}

fn json_value(value: &JsonValue) -> Value {
    match value {
        JsonValue::Null => Scalar::null().into(),
        JsonValue::Bool(value) => Scalar::boolean(*value).into(),
        JsonValue::Number(value) => {
            Scalar::integer(value.as_i64().expect("fixture integer")).into()
        }
        JsonValue::String(value) => Scalar::text(value).into(),
        JsonValue::Array(items) => flyrail::ArrayValue::new(items.iter().map(json_value)).into(),
        JsonValue::Object(items) => ObjectValue::new(
            items
                .iter()
                .map(|(key, value)| (key.clone(), json_value(value))),
        )
        .expect("object")
        .into(),
    }
}

fn command(value: &JsonValue) -> Command {
    let argv = value["argv"]
        .as_array()
        .expect("argv")
        .iter()
        .map(|argument| {
            argument.as_str().map_or_else(
                || {
                    AssetRef::with_path(
                        argument["asset"].as_str().expect("asset id"),
                        argument["path"].as_str().expect("asset path"),
                    )
                    .expect("asset reference")
                    .into()
                },
                CommandArgument::from,
            )
        })
        .collect::<Vec<_>>();
    let env = value
        .get("env")
        .and_then(JsonValue::as_object)
        .into_iter()
        .flat_map(|items| items.iter())
        .map(|(key, value)| {
            let value = value.as_str().map_or_else(
                || {
                    EnvironmentValue::from(
                        EnvRef::new(value["env"].as_str().expect("environment reference"))
                            .expect("reference"),
                    )
                },
                EnvironmentValue::from,
            );
            (key.clone(), value)
        })
        .collect::<Vec<_>>();
    Command::new(
        argv,
        env,
        value.get("cwd").map(|cwd| {
            cwd.as_str().map_or_else(
                || {
                    AssetRef::with_path(
                        cwd["asset"].as_str().expect("asset id"),
                        cwd["path"].as_str().expect("asset path"),
                    )
                    .expect("asset reference")
                    .into()
                },
                CommandArgument::from,
            )
        }),
    )
    .expect("command")
}

fn notice_records(result: &flyrail::ArtifactTranslation) -> Vec<(&str, NoticeKind, &str, &str)> {
    result
        .notices()
        .iter()
        .map(|notice| {
            (
                notice.artifact_id(),
                notice.kind(),
                notice.code(),
                notice.message(),
            )
        })
        .collect()
}

fn bundle_notice_records(result: &flyrail::RenderedBundle) -> Vec<(&str, NoticeKind, &str, &str)> {
    result
        .notices()
        .iter()
        .map(|notice| {
            (
                notice.artifact_id(),
                notice.kind(),
                notice.code(),
                notice.message(),
            )
        })
        .collect()
}

fn assert_unsupported(
    result: &flyrail::ArtifactTranslation,
    artifact_id: &str,
    code: &str,
    message: &str,
) {
    assert!(!result.supported());
    assert!(result.artifact().is_none());
    assert_eq!(
        notice_records(result),
        [(artifact_id, NoticeKind::Unsupported, code, message)]
    );
}

fn unsupported_message(code: &str) -> &'static str {
    match code {
        "instruction-destination" => "Cursor user rules have no verified filesystem preset.",
        "portable-auth-env-unverified" => "Copilot CLI bearer environment syntax is unverified.",
        "literal-interpolation" => "env contains host interpolation syntax; use native content.",
        _ => panic!("unsupported fixture code is not pinned: {code}"),
    }
}

#[test]
fn all_shared_instruction_and_mcp_vectors_translate_real_content() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../spec/fixtures/translations.json");
    if !fixture.is_file() {
        eprintln!("skipping translation fixture test: shared fixtures are absent");
        return;
    }
    let document: JsonValue =
        serde_json::from_slice(&fs::read(fixture).expect("fixture")).expect("JSON fixture");
    let cases = document["cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 35);
    let mut instructions = 0;
    let mut mcp = 0;
    for platform in [Platform::Macos, Platform::Linux, Platform::Windows] {
        for case in cases {
            let name = case["name"].as_str().expect("name");
            let selected = context(case, platform);
            let source = &case["artifact"];
            let result = if source["kind"] == "instruction" {
                instructions += 1;
                translate_instruction_artifact(
                    "translation-demo",
                    &InstructionArtifact::new(
                        source["id"].as_str().expect("id"),
                        source["text"].as_str().expect("text"),
                    )
                    .expect("instruction"),
                    &selected,
                )
                .expect("translation")
            } else {
                mcp += 1;
                let transport = &source["transport"];
                let transport = if transport["kind"] == "stdio" {
                    McpTransport::Command(command(&transport["command"]))
                } else {
                    McpTransport::Http(
                        HttpTransport::new(
                            transport["url"].as_str().expect("URL"),
                            transport
                                .get("bearer_token_env")
                                .and_then(JsonValue::as_str)
                                .map(|name| EnvRef::new(name).expect("token")),
                        )
                        .expect("HTTP transport"),
                    )
                };
                translate_mcp_artifact(
                    &McpArtifact::new(
                        source["id"].as_str().expect("id"),
                        source["name"].as_str().expect("server name"),
                        transport,
                    )
                    .expect("MCP"),
                    &selected,
                    [],
                )
                .expect("translation")
            };
            let expected = &case["expected"];
            if let Some(code) = expected.get("unsupported").and_then(JsonValue::as_str) {
                assert_unsupported(
                    &result,
                    source["id"].as_str().expect("id"),
                    code,
                    unsupported_message(code),
                );
                continue;
            }
            assert!(result.supported(), "{name}");
            let artifact = result.artifact().expect("artifact");
            assert!(artifact.schema_requirements().is_empty(), "{name}");
            if let Some(destination) = expected.get("destination").and_then(JsonValue::as_str) {
                assert_eq!(artifact.destination(), root().join(destination), "{name}");
            } else {
                let directory = expected["directory"].as_str().expect("directory");
                let suffix = expected["suffix"].as_str().expect("suffix");
                assert_eq!(
                    artifact.destination().parent(),
                    Some(root().join(directory).as_path())
                );
                assert!(
                    artifact
                        .destination()
                        .file_name()
                        .and_then(|value| value.to_str())
                        .is_some_and(|value| value.ends_with(suffix)),
                    "{name}"
                );
            }
            match artifact.content() {
                Content::Structured(content) => {
                    assert_eq!(
                        content.format(),
                        DocumentFormat::from_str(expected["format"].as_str().expect("format"))
                            .expect("format"),
                        "{name}"
                    );
                    let mut expected_value = expected["value"].clone();
                    if let Some(relative) =
                        expected.get("cwd_from_root").and_then(JsonValue::as_str)
                    {
                        expected_value["cwd"] =
                            JsonValue::String(root().join(relative).display().to_string());
                    }
                    assert_eq!(content.value(), &json_value(&expected_value), "{name}");
                    let parts = content.selector().parts();
                    assert_eq!(parts.len(), 2, "{name}");
                    let keys = parts
                        .iter()
                        .map(|part| match part {
                            SelectorPart::Key(key) => key.name(),
                            SelectorPart::Member(_) => panic!("fixture selector is keyed"),
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(keys, [expected["key"].as_str().expect("key"), "tools"]);
                }
                Content::Section(content) => {
                    assert_eq!(
                        content.text(),
                        expected["text"].as_str().expect("text"),
                        "{name}"
                    );
                }
                Content::File(content) => {
                    assert_eq!(
                        std::str::from_utf8(content.data()).expect("UTF-8"),
                        expected["text"].as_str().expect("text"),
                        "{name}"
                    );
                }
                Content::Tree(_) => panic!("translation does not emit trees"),
            }
        }
    }
    assert_eq!(instructions, 48);
    assert_eq!(mcp, 57);
}

#[test]
fn unsupported_boundaries_preserve_exact_codes_messages_and_empty_outputs() {
    let root = root();
    let context = |agent| {
        RenderContext::new(
            Target::project(agent, &root).expect("target"),
            Platform::Linux,
        )
        .expect("context")
    };
    let cases = [
        (
            Agent::Claude,
            McpArtifact::new(
                "tools",
                "workspace",
                McpTransport::Command(
                    Command::new(
                        ["server"],
                        std::iter::empty::<(String, EnvironmentValue)>(),
                        None,
                    )
                    .expect("command"),
                ),
            )
            .expect("MCP"),
            "reserved-server-name",
            "Claude reserves this MCP server name.",
        ),
        (
            Agent::Codex,
            McpArtifact::new(
                "tools",
                "tools",
                McpTransport::Command(
                    Command::new(
                        ["server"],
                        [(
                            "ALIAS",
                            EnvironmentValue::from(EnvRef::new("TOKEN").expect("reference")),
                        )],
                        None,
                    )
                    .expect("command"),
                ),
            )
            .expect("MCP"),
            "environment-alias",
            "Codex env_vars forwards a variable under its own name.",
        ),
    ];
    for (agent, artifact, code, message) in cases {
        let result = translate_mcp_artifact(&artifact, &context(agent), []).expect("translation");
        assert_unsupported(&result, "tools", code, message);
    }

    let relocated_claude = RenderContext::new(
        Target::user_with_home_and_environment(
            Agent::Claude,
            &root,
            [(
                String::from("CLAUDE_CONFIG_DIR"),
                String::from("~/selected"),
            )],
        )
        .expect("target"),
        Platform::Linux,
    )
    .expect("context");
    let plain = McpArtifact::new(
        "explicit-path",
        "tools",
        McpTransport::Command(
            Command::new(
                ["server"],
                std::iter::empty::<(String, EnvironmentValue)>(),
                None,
            )
            .expect("command"),
        ),
    )
    .expect("MCP");
    assert_unsupported(
        &translate_mcp_artifact(&plain, &relocated_claude, []).expect("translation"),
        "explicit-path",
        "mcp-explicit-path",
        "Relocated Claude user MCP requires an explicit mcp_path.",
    );

    let windows_collision = McpArtifact::new(
        "windows-environment",
        "tools",
        McpTransport::Command(
            Command::new(
                ["server"],
                [
                    ("Mode", EnvironmentValue::from("one")),
                    ("MODE", EnvironmentValue::from("two")),
                ],
                None,
            )
            .expect("command"),
        ),
    )
    .expect("MCP");
    let windows = RenderContext::new(
        Target::project(Agent::Claude, &root).expect("target"),
        Platform::Windows,
    )
    .expect("context");
    assert_unsupported(
        &translate_mcp_artifact(&windows_collision, &windows, []).expect("translation"),
        "windows-environment",
        "windows-env-collision",
        "Environment names collide under Windows case-insensitive lookup.",
    );

    let portable_reference = McpArtifact::new(
        "portable-reference",
        "tools",
        McpTransport::Command(
            Command::new(
                ["server"],
                [(
                    "TOKEN",
                    EnvironmentValue::from(EnvRef::new("TOKEN").expect("reference")),
                )],
                None,
            )
            .expect("command"),
        ),
    )
    .expect("MCP");
    assert_unsupported(
        &translate_mcp_artifact(&portable_reference, &context(Agent::Pi), []).expect("translation"),
        "portable-reference",
        "portable-env-reference-unverified",
        "This host cannot represent this environment reference.",
    );
}

#[test]
fn interpolation_cwd_profile_and_instruction_boundaries_match_reference() {
    let root = root();
    let project = |agent, surface| {
        RenderContext::with_surface(
            Target::project(agent, &root).expect("target"),
            Platform::Linux,
            surface,
        )
        .expect("context")
    };
    let mcp = |argument: &str| {
        McpArtifact::new(
            "tools",
            "tools",
            McpTransport::Command(
                Command::new(
                    ["server", argument],
                    std::iter::empty::<(String, EnvironmentValue)>(),
                    None,
                )
                .expect("command"),
            ),
        )
        .expect("MCP")
    };
    for (agent, surface, literal, code, message) in [
        (
            Agent::Claude,
            Surface::Cli,
            "${TOKEN}",
            "literal-interpolation",
            "argv contains host interpolation syntax; use native content.",
        ),
        (
            Agent::Cursor,
            Surface::Ide,
            "${workspaceFolder}",
            "literal-interpolation",
            "argv contains host interpolation syntax; use native content.",
        ),
        (
            Agent::Opencode,
            Surface::Cli,
            "{file:secret}",
            "literal-interpolation",
            "argv contains host interpolation syntax; use native content.",
        ),
        (
            Agent::Copilot,
            Surface::Cli,
            "${TOKEN}",
            "literal-interpolation-unverified",
            "Literal argv substitution-like syntax is unverified for Copilot CLI.",
        ),
        (
            Agent::Copilot,
            Surface::Vscode,
            "${input:secret}",
            "literal-interpolation",
            "argv contains host interpolation syntax; use native content.",
        ),
        (
            Agent::Pi,
            Surface::Cli,
            "$env:TOKEN",
            "literal-interpolation",
            "argv contains host interpolation syntax; use native content.",
        ),
    ] {
        let result = translate_mcp_artifact(&mcp(literal), &project(agent, surface), [])
            .expect("translation");
        assert_unsupported(&result, "tools", code, message);
    }

    let cwd = McpArtifact::new(
        "tools",
        "tools",
        McpTransport::Command(
            Command::new(
                ["server"],
                std::iter::empty::<(String, EnvironmentValue)>(),
                Some(CommandArgument::from("work")),
            )
            .expect("command"),
        ),
    )
    .expect("MCP");
    for agent in [Agent::Claude, Agent::Cursor] {
        assert_unsupported(
            &translate_mcp_artifact(&cwd, &project(agent, Surface::Cli), []).expect("translation"),
            "tools",
            "command-cwd",
            "The selected MCP schema cannot honor cwd.",
        );
    }

    let vscode_user = RenderContext::with_surface(
        Target::user_with_home_and_environment(
            Agent::Copilot,
            &root,
            std::iter::empty::<(String, String)>(),
        )
        .expect("target"),
        Platform::Linux,
        Surface::Vscode,
    )
    .expect("context");
    let plain = mcp("safe");
    assert_unsupported(
        &translate_mcp_artifact(&plain, &vscode_user, []).expect("translation"),
        "tools",
        "mcp-profile-path",
        "VS Code user MCP requires an explicit profile mcp.json path.",
    );
    assert_unsupported(
        &translate_mcp_artifact(
            &plain,
            &vscode_user
                .clone()
                .with_mcp_path(root.join("profile/wrong.json"))
                .expect("path"),
            [],
        )
        .expect("translation"),
        "tools",
        "mcp-profile-schema",
        "VS Code routes require mcp.json with the servers schema; use the CLI surface for Agent Host native mcpServers files.",
    );

    let instruction = InstructionArtifact::new("guide", "Always.").expect("instruction");
    assert_unsupported(
        &translate_instruction_artifact(
            "translation-demo",
            &instruction,
            &project(Agent::Cursor, Surface::Ide)
                .with_instruction_path(root.join("custom.mdc"))
                .expect("path"),
        )
        .expect("translation"),
        "guide",
        "native-instruction-required",
        "Use native content for authored modular rule metadata.",
    );
    assert_unsupported(
        &translate_instruction_artifact(
            "translation-demo",
            &instruction,
            &vscode_user
                .with_instruction_path(root.join("bad.md"))
                .expect("path"),
        )
        .expect("translation"),
        "guide",
        "instruction-extension",
        "VS Code user instructions require an .instructions.md file.",
    );
}

#[test]
fn instruction_marker_and_render_digest_remain_wire_compatible() {
    let root = PathBuf::from("/fixture/project");
    let context = RenderContext::new(
        Target::project(Agent::Codex, &root).expect("target"),
        Platform::Linux,
    )
    .expect("context");
    let translation = translate_instruction_artifact(
        "translation-demo",
        &InstructionArtifact::new("guide", "Run the checks.\n").expect("instruction"),
        &context,
    )
    .expect("translation");
    let Content::Section(section) = translation.artifact().expect("artifact").content() else {
        panic!("instruction should be a section");
    };
    assert_eq!(
        section.marker(),
        "81a6fb0b74cac727508df8661502ff555a19792d232631d282cc3bfcd196dc93"
    );
    let rendered = assemble_translations(
        [translation],
        [],
        std::slice::from_ref(&context),
        context.routing_context(),
    )
    .expect("rendered bundle");
    assert_eq!(
        rendered.content_digest(),
        "51ff0633093b74dd5486198a0145530c3883c3f68c30f8140805a66f536f6f8b"
    );
}

#[test]
fn notices_and_instruction_metadata_match_each_surface() {
    let root = root();
    let instruction = InstructionArtifact::new("guide", "Always.").expect("instruction");
    let project = |agent, surface| {
        RenderContext::with_surface(
            Target::project(agent, &root).expect("target"),
            Platform::Linux,
            surface,
        )
        .expect("context")
    };
    let user_vscode = RenderContext::with_surface(
        Target::user_with_home_and_environment(
            Agent::Copilot,
            &root,
            std::iter::empty::<(String, String)>(),
        )
        .expect("target"),
        Platform::Linux,
        Surface::Vscode,
    )
    .expect("context");
    let mcp = McpArtifact::new(
        "tools",
        "tools",
        McpTransport::Command(
            Command::new(
                ["server"],
                std::iter::empty::<(String, EnvironmentValue)>(),
                None,
            )
            .expect("command"),
        ),
    )
    .expect("MCP");
    let cases = [
        (
            translate_instruction_artifact(
                "translation-demo",
                &instruction,
                &project(Agent::Codex, Surface::Cli),
            )
            .expect("Codex instructions"),
            vec![
                (
                    "guide",
                    NoticeKind::Activation,
                    "instruction-loading",
                    "Instruction discovery and loading are controlled by the host and current session.",
                ),
                (
                    "guide",
                    NoticeKind::Activation,
                    "codex-instruction-discovery",
                    "AGENTS.override.md takes precedence; instructions default to a 32 KiB limit. Start a new session after changes and check host discovery.",
                ),
            ],
        ),
        (
            translate_instruction_artifact("translation-demo", &instruction, &user_vscode)
                .expect("VS Code user instructions"),
            vec![
                (
                    "guide",
                    NoticeKind::Activation,
                    "instruction-loading",
                    "Instruction discovery and loading are controlled by the host and current session.",
                ),
                (
                    "guide",
                    NoticeKind::Activation,
                    "vscode-agent-host-instructions",
                    "User instructions target VS Code Agent Host; local extension-host sessions may use different discovery settings.",
                ),
            ],
        ),
        (
            translate_mcp_artifact(&mcp, &project(Agent::Claude, Surface::Cli), [])
                .expect("ordinary MCP"),
            vec![(
                "tools",
                NoticeKind::Activation,
                "mcp-activation",
                "Registration does not grant trust, approve tools, supply credentials or prove server activation.",
            )],
        ),
        (
            translate_mcp_artifact(&mcp, &project(Agent::Pi, Surface::Cli), []).expect("Pi MCP"),
            vec![
                (
                    "tools",
                    NoticeKind::Activation,
                    "mcp-activation",
                    "Registration does not grant trust, approve tools, supply credentials or prove server activation.",
                ),
                (
                    "tools",
                    NoticeKind::Prerequisite,
                    "pi-mcp-adapter-v2-32-1",
                    "Requires externally installed pi-mcp-adapter; emitted schema targets v2.32.1.",
                ),
            ],
        ),
        (
            translate_mcp_artifact(
                &mcp,
                &user_vscode
                    .with_mcp_path(root.join("profile/mcp.json"))
                    .expect("path"),
                [],
            )
            .expect("VS Code MCP"),
            vec![
                (
                    "tools",
                    NoticeKind::Activation,
                    "mcp-activation",
                    "Registration does not grant trust, approve tools, supply credentials or prove server activation.",
                ),
                (
                    "tools",
                    NoticeKind::Activation,
                    "vscode-mcp-forwarding",
                    "VS Code forwards these servers to Agent Host, which does not read this servers document directly.",
                ),
            ],
        ),
    ];
    for (result, expected) in cases {
        assert_eq!(notice_records(&result), expected);
    }
}

#[test]
fn shared_asset_manifest_substitutes_the_pinned_revision_destination() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../spec/fixtures/translations.json");
    let document: JsonValue =
        serde_json::from_slice(&fs::read(fixture).expect("fixture")).expect("JSON fixture");
    let source = &document["asset_manifest"]["artifacts"][0];
    let transport = &source["transport"];
    let artifact = McpArtifact::new(
        source["id"].as_str().expect("id"),
        source["name"].as_str().expect("name"),
        McpTransport::Command(command(&transport["command"])),
    )
    .expect("MCP");
    let selected = RenderContext::new(
        Target::project(Agent::Codex, Path::new("/fixture/project")).expect("target"),
        Platform::Linux,
    )
    .expect("context");
    let revision = PathBuf::from("/fixture/assets/runtime/revisions/sha256-pinned");
    let result = translate_mcp_artifact(
        &artifact,
        &selected,
        [("runtime".to_owned(), revision.clone())],
    )
    .expect("translation");
    let rendered = result.artifact().expect("artifact");
    assert_eq!(
        rendered.destination(),
        Path::new("/fixture/project/.codex/config.toml")
    );
    let Content::Structured(content) = rendered.content() else {
        panic!("MCP translation should be structured");
    };
    assert_eq!(content.format(), DocumentFormat::Toml);
    assert_eq!(
        content
            .selector()
            .parts()
            .iter()
            .map(|part| match part {
                SelectorPart::Key(key) => key.name(),
                SelectorPart::Member(_) => panic!("MCP selector is keyed"),
            })
            .collect::<Vec<_>>(),
        ["mcp_servers", "tools"]
    );
    assert_eq!(
        content.value(),
        &json_value(&serde_json::json!({
            "command": "node",
            "args": [revision.join("server.js").display().to_string()],
            "env": {},
            "cwd": revision.join("data").display().to_string(),
        }))
    );
}

#[test]
fn shared_combined_vectors_match_render_many_and_are_order_independent() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../spec/fixtures/translations.json");
    let document: JsonValue =
        serde_json::from_slice(&fs::read(fixture).expect("fixture")).expect("JSON fixture");
    let cases = document["combined_cases"]
        .as_array()
        .expect("combined cases");
    assert_eq!(cases.len(), 4);

    for case in cases {
        let manifest = serde_json::to_vec(&serde_json::json!({
            "schema_version": 2,
            "id": "combined",
            "version": "same",
            "artifacts": case["artifacts"],
        }))
        .expect("manifest");
        let bundle = Bundle::from_memory(&manifest, []).expect("combined bundle");
        let contexts = case["contexts"]
            .as_array()
            .expect("contexts")
            .iter()
            .map(|item| {
                let agent = Agent::from_str(item["agent"].as_str().expect("agent")).expect("agent");
                RenderContext::new(
                    Target::project(agent, root()).expect("target"),
                    Platform::Linux,
                )
                .expect("context")
            })
            .collect::<Vec<_>>();
        let forward = render_many(&bundle, contexts.clone()).expect("forward render");
        let reverse = render_many(&bundle, contexts.iter().cloned().rev()).expect("reverse render");
        assert_eq!(forward, reverse, "{}", case["name"]);

        if let Some(code) = case["expected"]
            .get("unsupported")
            .and_then(JsonValue::as_str)
        {
            assert!(!forward.supported(), "{}", case["name"]);
            assert!(forward.artifacts().is_empty(), "{}", case["name"]);
            assert!(forward.dependencies().is_empty(), "{}", case["name"]);
            assert_eq!(
                forward
                    .notices()
                    .iter()
                    .filter(|notice| notice.kind() == NoticeKind::Unsupported)
                    .map(flyrail::Notice::code)
                    .collect::<Vec<_>>(),
                [code],
                "{}",
                case["name"]
            );
        } else {
            assert!(forward.supported(), "{}", case["name"]);
            assert_eq!(forward.artifacts().len(), 2, "{}", case["name"]);
            assert!(forward.dependencies().is_empty(), "{}", case["name"]);
            assert_eq!(
                forward
                    .notices()
                    .iter()
                    .map(|notice| (notice.kind(), notice.code()))
                    .collect::<Vec<_>>(),
                [
                    (NoticeKind::Activation, "mcp-activation"),
                    (NoticeKind::Activation, "mcp-activation"),
                ],
                "{}",
                case["name"]
            );
            assert_eq!(forward.routing_context().len(), 2, "{}", case["name"]);
        }
    }
}

#[test]
fn shared_asset_manifest_renders_exact_revision_and_references() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../spec/fixtures/translations.json");
    let document: JsonValue =
        serde_json::from_slice(&fs::read(fixture).expect("fixture")).expect("JSON fixture");
    let manifest = serde_json::to_vec(&document["asset_manifest"]).expect("asset manifest");
    let bundle = Bundle::from_memory(
        &manifest,
        [
            BundleEntry::file("runtime/server.js", b"console.log(\"MCP\");".to_vec())
                .expect("server"),
            BundleEntry::directory("runtime/data").expect("data"),
        ],
    )
    .expect("asset bundle");
    let selected = RenderContext::new(
        Target::project(Agent::Codex, root()).expect("target"),
        Platform::Linux,
    )
    .expect("context")
    .with_asset_root(root().join("assets"))
    .expect("asset root");
    let rendered = render(&bundle, &selected).expect("render");
    assert!(rendered.supported());
    assert_eq!(rendered.artifacts().len(), 2);
    assert_eq!(rendered.dependencies().len(), 1);
    let runtime = rendered
        .artifacts()
        .iter()
        .find(|artifact| artifact.id() == "runtime")
        .expect("runtime");
    assert_eq!(
        runtime.destination().parent(),
        Some(root().join("assets/support-demo/runtime").as_path())
    );
    assert_eq!(
        runtime
            .destination()
            .file_name()
            .and_then(|part| part.to_str())
            .map(str::len),
        Some(64)
    );
    let tools = rendered
        .artifacts()
        .iter()
        .find(|artifact| artifact.id() == "tools")
        .expect("tools");
    let Content::Structured(content) = tools.content() else {
        panic!("tools should be structured");
    };
    assert_eq!(
        content.value(),
        &json_value(&serde_json::json!({
            "command": "node",
            "args": [runtime.destination().join("server.js").display().to_string()],
            "env": {},
            "cwd": runtime.destination().join("data").display().to_string(),
        }))
    );
}

#[test]
fn assembly_preserves_dependencies_and_propagates_dependency_errors() {
    let selected = RenderContext::new(
        Target::project(Agent::Claude, root()).expect("target"),
        Platform::Linux,
    )
    .expect("context");
    let translated = |id: &str, name: &str| {
        translate_mcp_artifact(
            &McpArtifact::new(
                id,
                name,
                McpTransport::Command(
                    Command::new(
                        ["server"],
                        std::iter::empty::<(String, EnvironmentValue)>(),
                        None,
                    )
                    .expect("command"),
                ),
            )
            .expect("MCP"),
            &selected,
            [],
        )
        .expect("translation")
    };
    let dependency = Dependency::new("tools", "runtime").expect("dependency");
    let assembled = assemble_translations(
        [
            translated("runtime", "runtime"),
            translated("tools", "tools"),
        ],
        [dependency.clone()],
        std::slice::from_ref(&selected),
        selected.routing_context(),
    )
    .expect("assembly");
    assert!(assembled.supported());
    assert_eq!(assembled.dependencies(), std::slice::from_ref(&dependency));
    let names = assembled
        .artifacts()
        .iter()
        .map(|artifact| artifact.id().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        dependency_order(&names, assembled.dependencies()).expect("dependency order"),
        ["runtime", "tools"]
    );

    let unknown = assemble_translations(
        [
            translated("runtime", "runtime"),
            translated("tools", "tools"),
        ],
        [Dependency::new("tools", "missing").expect("dependency")],
        std::slice::from_ref(&selected),
        selected.routing_context(),
    )
    .expect_err("unknown dependency should fail");
    assert_eq!(
        unknown.message(),
        "dependency references an unknown artifact or asset"
    );

    let cyclic = assemble_translations(
        [
            translated("runtime", "runtime"),
            translated("tools", "tools"),
        ],
        [
            Dependency::new("tools", "runtime").expect("dependency"),
            Dependency::new("runtime", "tools").expect("dependency"),
        ],
        std::slice::from_ref(&selected),
        selected.routing_context(),
    )
    .expect_err("cyclic dependencies should fail");
    assert_eq!(cyclic.message(), "cyclic artifact dependencies");
}

#[test]
fn assembly_blocks_destination_format_and_copilot_precedence_collisions() {
    let root = root();
    let project = |agent| {
        RenderContext::new(
            Target::project(agent, &root).expect("target"),
            Platform::Linux,
        )
        .expect("context")
    };
    let first = McpArtifact::new(
        "first",
        "tools",
        McpTransport::Command(
            Command::new(
                ["first-server"],
                std::iter::empty::<(String, EnvironmentValue)>(),
                None,
            )
            .expect("command"),
        ),
    )
    .expect("MCP");
    let second = McpArtifact::new(
        "second",
        "tools",
        McpTransport::Command(
            Command::new(
                ["second-server"],
                std::iter::empty::<(String, EnvironmentValue)>(),
                None,
            )
            .expect("command"),
        ),
    )
    .expect("MCP");

    let claude = project(Agent::Claude);
    let compatible = McpArtifact::new(
        "compatible",
        "tools",
        McpTransport::Command(
            Command::new(
                ["first-server"],
                std::iter::empty::<(String, EnvironmentValue)>(),
                None,
            )
            .expect("command"),
        ),
    )
    .expect("MCP");
    let common = assemble_translations(
        [
            translate_mcp_artifact(&first, &claude, []).expect("first"),
            translate_mcp_artifact(&compatible, &claude, []).expect("compatible"),
        ],
        [],
        std::slice::from_ref(&claude),
        claude.routing_context(),
    )
    .expect("assembly");
    assert!(common.supported());
    assert_eq!(common.artifacts().len(), 2);

    let collision = assemble_translations(
        [
            translate_mcp_artifact(&first, &claude, []).expect("first"),
            translate_mcp_artifact(&second, &claude, []).expect("second"),
        ],
        [],
        std::slice::from_ref(&claude),
        claude.routing_context(),
    )
    .expect("assembly");
    assert!(!collision.supported());
    assert_eq!(collision.artifacts().len(), 0);
    assert_eq!(collision.dependencies().len(), 0);
    let destination_message = format!(
        "Incompatible claims from first and second at {}.",
        root.join(".mcp.json").display()
    );
    assert_eq!(
        bundle_notice_records(&collision),
        [
            (
                "first",
                NoticeKind::Activation,
                "mcp-activation",
                "Registration does not grant trust, approve tools, supply credentials or prove server activation.",
            ),
            (
                "second",
                NoticeKind::Activation,
                "mcp-activation",
                "Registration does not grant trust, approve tools, supply credentials or prove server activation.",
            ),
            (
                "second",
                NoticeKind::Unsupported,
                "destination-collision",
                destination_message.as_str(),
            ),
        ]
    );

    let shared = root.join("shared");
    let codex = project(Agent::Codex)
        .with_mcp_path(&shared)
        .expect("shared path");
    let claude_shared = claude.clone().with_mcp_path(&shared).expect("shared path");
    let format_collision = assemble_translations(
        [
            translate_mcp_artifact(&first, &codex, []).expect("codex"),
            translate_mcp_artifact(&second, &claude_shared, []).expect("claude"),
        ],
        [],
        &[codex.clone(), claude_shared.clone()],
        [],
    )
    .expect("assembly");
    assert!(!format_collision.supported());
    assert_eq!(format_collision.artifacts().len(), 0);
    let format_message = format!(
        "Incompatible document formats from first and second at {}.",
        shared.display()
    );
    assert_eq!(
        bundle_notice_records(&format_collision),
        [
            (
                "first",
                NoticeKind::Activation,
                "mcp-activation",
                "Registration does not grant trust, approve tools, supply credentials or prove server activation.",
            ),
            (
                "second",
                NoticeKind::Activation,
                "mcp-activation",
                "Registration does not grant trust, approve tools, supply credentials or prove server activation.",
            ),
            (
                "second",
                NoticeKind::Unsupported,
                "document-format-collision",
                format_message.as_str(),
            ),
        ]
    );

    let copilot = project(Agent::Copilot)
        .with_mcp_path(root.join(".github/mcp.json"))
        .expect("alternate");
    let precedence = assemble_translations(
        [
            translate_mcp_artifact(&first, &claude, []).expect("claude"),
            translate_mcp_artifact(&second, &copilot, []).expect("copilot"),
        ],
        [],
        &[claude, copilot],
        [],
    )
    .expect("assembly");
    assert!(!precedence.supported());
    assert_eq!(precedence.artifacts().len(), 0);
    let precedence_collision_message = format!(
        "Copilot CLI selects {} over {} for server tools; the selected records disagree.",
        root.join(".mcp.json").display(),
        root.join(".github/mcp.json").display()
    );
    assert_eq!(
        bundle_notice_records(&precedence),
        [
            (
                "first",
                NoticeKind::Activation,
                "mcp-activation",
                "Registration does not grant trust, approve tools, supply credentials or prove server activation.",
            ),
            (
                "second",
                NoticeKind::Activation,
                "mcp-activation",
                "Registration does not grant trust, approve tools, supply credentials or prove server activation.",
            ),
            (
                "second",
                NoticeKind::Unsupported,
                "copilot-mcp-precedence-collision",
                precedence_collision_message.as_str(),
            ),
        ]
    );

    let codex = project(Agent::Codex)
        .with_mcp_path(root.join(".mcp.json"))
        .expect("alternate");
    let copilot = project(Agent::Copilot)
        .with_mcp_path(root.join(".github/mcp.json"))
        .expect("alternate");
    let unverified = assemble_translations(
        [
            translate_mcp_artifact(&first, &codex, []).expect("codex"),
            translate_mcp_artifact(&second, &copilot, []).expect("copilot"),
        ],
        [],
        &[codex, copilot],
        [],
    )
    .expect("assembly");
    assert!(unverified.supported());
    let expected_precedence = format!(
        "Copilot CLI gives {} precedence over {}; native content requires caller review of the effective server definitions.",
        root.join(".mcp.json").display(),
        root.join(".github/mcp.json").display()
    );
    assert_eq!(
        bundle_notice_records(&unverified),
        [
            (
                "first",
                NoticeKind::Activation,
                "mcp-activation",
                "Registration does not grant trust, approve tools, supply credentials or prove server activation.",
            ),
            (
                "second",
                NoticeKind::Activation,
                "mcp-activation",
                "Registration does not grant trust, approve tools, supply credentials or prove server activation.",
            ),
            (
                "second",
                NoticeKind::Activation,
                "copilot-mcp-precedence-unverified",
                expected_precedence.as_str(),
            ),
        ]
    );
}
