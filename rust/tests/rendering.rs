#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::needless_pass_by_value,
    clippy::panic,
    clippy::too_many_lines
)]

use flyrail::{
    Agent, Bundle, Content, DependencyMode, HookRuntime, HookShell, NoticeKind, Platform,
    RenderContext, Surface, Target, render, render_many,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn hook(id: &str, event: &str, outcomes: &[&str]) -> Value {
    json!({
        "kind": "hook",
        "id": id,
        "protocol_version": 1,
        "event": event,
        "command": {
            "argv": ["/bin/echo", "space ü#%😀"],
            "env": {"UNICODE": "ü😀"},
            "cwd": "work ü😀"
        },
        "timeout_ms": 1000,
        "outcomes": outcomes,
        "tools": if matches!(event, "before-tool" | "after-tool") {
            json!(["Read"])
        } else {
            json!([])
        }
    })
}

fn bundle(artifacts: Vec<Value>) -> Bundle {
    let manifest = serde_json::to_vec(&json!({
        "schema_version": 2,
        "id": "hook-matrix",
        "version": "same",
        "artifacts": artifacts
    }))
    .expect("manifest");
    Bundle::from_memory(&manifest, []).expect("bundle")
}

fn context(agent: Agent, root: &Path, user: bool) -> RenderContext {
    let target = if user {
        Target::user_with_home_and_environment(agent, root, std::iter::empty::<(String, String)>())
            .expect("user target")
    } else {
        Target::project(agent, root).expect("project target")
    };
    RenderContext::new(target, Platform::Linux)
        .expect("context")
        .with_hook_runtime(
            HookRuntime::new(
                Some(PathBuf::from("/opt/node/bin/node")),
                Some(HookShell::Posix),
            )
            .expect("runtime"),
        )
}

fn file_bytes(content: &Content) -> &[u8] {
    match content {
        Content::File(file) => file.data(),
        _ => panic!("expected file"),
    }
}

#[test]
fn hook_render_matrix_emits_runtime_host_content_dependencies_and_notices() {
    let bundle = bundle(vec![hook("start", "session-start", &["continue"])]);
    let root = Path::new("/fixture/project");
    for agent in [
        Agent::Claude,
        Agent::Codex,
        Agent::Cursor,
        Agent::Copilot,
        Agent::Opencode,
        Agent::Pi,
    ] {
        for user in [false, true] {
            let rendered = render(&bundle, &context(agent, root, user)).expect("render");
            assert!(rendered.supported(), "{agent} user={user}");
            assert_eq!(rendered.notices().len(), 2);
            assert_eq!(
                rendered
                    .notices()
                    .iter()
                    .map(|notice| (notice.kind(), notice.code()))
                    .collect::<Vec<_>>(),
                [
                    (NoticeKind::Prerequisite, "hook-runtime-prerequisite"),
                    (NoticeKind::Activation, "hook-activation"),
                ]
            );
            let embedded = matches!(agent, Agent::Opencode | Agent::Pi);
            assert_eq!(
                rendered.artifacts().len(),
                if embedded { 2 } else { 3 },
                "{agent} user={user}"
            );
            let runtime = rendered
                .artifacts()
                .iter()
                .find(|artifact| matches!(artifact.content(), Content::Tree(_)))
                .expect("runtime tree");
            let Content::Tree(tree) = runtime.content() else {
                panic!("runtime tree")
            };
            assert_eq!(
                tree.entries()
                    .iter()
                    .map(flyrail::BundleEntry::path)
                    .collect::<Vec<_>>(),
                ["bridge.mts", "config.mts", "process.mts", "runner.mts"]
            );
            assert!(
                runtime
                    .destination()
                    .starts_with(root.join(".flyrail-assets/hook-matrix/.hooks"))
            );
            assert_eq!(rendered.dependencies().len(), if embedded { 1 } else { 2 });
            assert!(rendered.dependencies().iter().any(|edge| {
                edge.dependent() == "start"
                    && (embedded || edge.mode() == DependencyMode::StableReference)
            }));
            let host = rendered
                .artifacts()
                .iter()
                .find(|artifact| artifact.id() == "start")
                .expect("host artifact");
            if embedded {
                assert!(matches!(host.content(), Content::File(_)));
                let expected = if agent == Agent::Opencode {
                    "plugins"
                } else {
                    "extensions"
                };
                assert!(host.destination().to_string_lossy().contains(expected));
            } else if agent == Agent::Copilot {
                assert!(host.destination().to_string_lossy().ends_with(".json"));
                assert!(matches!(host.content(), Content::File(_)));
            } else {
                assert!(matches!(host.content(), Content::Structured(_)));
            }
        }
    }
}

#[test]
fn copilot_all_events_have_python_event_order_independent_of_artifact_order() {
    let artifacts = vec![
        hook("stop", "stop", &["continue"]),
        hook("start", "session-start", &["continue"]),
        hook("prompt", "prompt", &["continue"]),
        hook("before", "before-tool", &["continue"]),
        hook("after", "after-tool", &["continue"]),
    ];
    let rendered = render(
        &bundle(artifacts),
        &context(Agent::Copilot, Path::new("/fixture/project"), false),
    )
    .expect("render");
    let bytes = file_bytes(
        rendered
            .artifacts()
            .iter()
            .find(|artifact| artifact.id() == "after")
            .expect("Copilot host file")
            .content(),
    );
    let text = std::str::from_utf8(bytes).expect("UTF-8");
    let positions = [
        "\"postToolUse\"",
        "\"preToolUse\"",
        "\"userPromptSubmitted\"",
        "\"sessionStart\"",
        "\"agentStop\"",
    ]
    .map(|name| text.find(name).expect("event"));
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(text.ends_with('\n'));
}

#[test]
fn unicode_roots_use_python_path_as_uri_and_ascii_config_json() {
    let root = Path::new("/fixture/space ü#%😀");
    for agent in [Agent::Claude, Agent::Opencode] {
        let rendered = render(
            &bundle(vec![hook("start", "session-start", &["continue"])]),
            &context(agent, root, false),
        )
        .expect("render");
        let launcher = rendered
            .artifacts()
            .iter()
            .find(|artifact| {
                matches!(artifact.content(), Content::File(_))
                    && artifact.destination().extension().is_some()
            })
            .expect("launcher");
        let text = std::str::from_utf8(file_bytes(launcher.content())).expect("UTF-8");
        assert!(text.contains("file:///fixture/space%20%C3%BC%23%25%F0%9F%98%80/"));
        let runtime = rendered
            .artifacts()
            .iter()
            .find(|artifact| matches!(artifact.content(), Content::Tree(_)))
            .expect("runtime");
        let Content::Tree(tree) = runtime.content() else {
            panic!("tree")
        };
        let config = tree
            .entries()
            .iter()
            .find(|entry| entry.path() == "config.mts")
            .and_then(|entry| entry.data())
            .expect("config.mts");
        let config = std::str::from_utf8(config).expect("UTF-8");
        assert!(config.contains("\\\\u00fc\\\\ud83d\\\\ude00"));
        assert!(!config.contains("ü😀"));
    }
}

#[test]
fn unsupported_hook_boundaries_block_outputs_with_typed_notices() {
    let root = Path::new("/fixture/project");
    let cases = [
        (
            bundle(vec![hook("start", "session-start", &["continue"])]),
            RenderContext::new(
                Target::project(Agent::Claude, root).expect("target"),
                Platform::Linux,
            )
            .expect("context"),
            "hook-runtime",
        ),
        (
            bundle(vec![hook("before", "before-tool", &["modify-input"])]),
            context(Agent::Codex, root, false),
            "hook-modification-approval",
        ),
    ];
    for (bundle, selected, code) in cases {
        let rendered = render(&bundle, &selected).expect("unsupported render");
        assert!(!rendered.supported());
        assert_eq!(rendered.artifacts().len(), 0);
        assert_eq!(rendered.dependencies().len(), 0);
        assert_eq!(
            rendered
                .notices()
                .iter()
                .filter(|notice| notice.kind() == NoticeKind::Unsupported)
                .map(flyrail::Notice::code)
                .collect::<Vec<_>>(),
            [code]
        );
    }

    let vscode = RenderContext::with_surface(
        Target::project(Agent::Copilot, root).expect("target"),
        Platform::Linux,
        Surface::Vscode,
    )
    .expect("context")
    .with_hook_runtime(
        HookRuntime::new(
            Some(PathBuf::from("/opt/node/bin/node")),
            Some(HookShell::Posix),
        )
        .expect("runtime"),
    );
    let rendered = render(
        &bundle(vec![hook("start", "session-start", &["continue"])]),
        &vscode,
    )
    .expect("unsupported render");
    assert_eq!(rendered.notices()[0].code(), "hook-surface");
    assert_eq!(rendered.artifacts().len(), 0);
}

#[test]
fn render_many_rejects_empty_and_deduplicates_reverse_order_routes() {
    let bundle = bundle(vec![hook("start", "session-start", &["continue"])]);
    assert_eq!(
        render_many(&bundle, [])
            .expect_err("empty contexts")
            .message(),
        "render_many requires at least one explicit context"
    );
    let root = Path::new("/fixture/space ü😀");
    let claude = context(Agent::Claude, root, false);
    let codex = context(Agent::Codex, root, false);
    let forward =
        render_many(&bundle, [claude.clone(), codex.clone(), claude.clone()]).expect("forward");
    let reverse = render_many(&bundle, [claude, codex]).expect("reverse");
    assert_eq!(forward, reverse);
    assert_eq!(forward.routing_context().len(), 2);
    assert!(
        forward
            .routing_context()
            .iter()
            .all(|(_, route)| route.contains("\\u00fc\\ud83d\\ude00"))
    );
}
