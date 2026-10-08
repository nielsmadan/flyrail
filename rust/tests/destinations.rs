#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use flyrail::{
    Agent, Platform, RenderContext, Surface, Target, instruction_destination, mcp_destination,
};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[test]
fn every_translation_fixture_uses_the_real_destination_resolver() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../spec/fixtures/translations.json");
    if !fixture.is_file() {
        eprintln!("skipping destination fixture test: shared fixtures are absent");
        return;
    }
    let document: Value =
        serde_json::from_slice(&fs::read(fixture).expect("fixture")).expect("JSON fixture");
    let cases = document["cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 35);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repository root")
        .join(".cache/destination-fixture-root");

    for case in cases {
        let name = case["name"].as_str().expect("name");
        let agent = Agent::from_str(case["agent"].as_str().expect("agent")).expect("agent value");
        let surface =
            Surface::from_str(case["surface"].as_str().expect("surface")).expect("surface value");
        let target = if case["scope"] == "project" {
            Target::project(agent, &root).expect("project target")
        } else {
            Target::user_with_home_and_environment(
                agent,
                &root,
                std::iter::empty::<(String, String)>(),
            )
            .expect("user target")
        };
        let mut context =
            RenderContext::with_surface(target, Platform::Linux, surface).expect("context");
        if let Some(path) = case.get("mcp_path").and_then(Value::as_str) {
            context = context
                .with_mcp_path(root.join(path))
                .expect("MCP override");
        }

        let expected = &case["expected"];
        let actual = match case["artifact"]["kind"].as_str().expect("kind") {
            "instruction" => instruction_destination(&context).expect("instruction destination"),
            "mcp" => mcp_destination(&context).expect("MCP destination"),
            other => panic!("unexpected fixture kind {other}"),
        };
        if let Some(path) = expected.get("destination").and_then(Value::as_str) {
            assert_eq!(actual, Some(root.join(path)), "{name}");
        } else if let Some(path) = expected.get("directory").and_then(Value::as_str) {
            assert_eq!(actual, Some(root.join(path)), "{name}");
        } else if case["artifact"]["kind"] == "instruction" {
            assert_eq!(actual, None, "{name}");
        } else {
            assert_eq!(actual, Some(root.join(".mcp.json")), "{name}");
        }
    }
}

#[test]
fn user_destination_and_capability_boundary_matrix_is_explicit() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repository root")
        .join(".cache/destination-matrix");
    let cases = [
        (
            Agent::Claude,
            Some(".claude/CLAUDE.md"),
            Some(".claude.json"),
        ),
        (
            Agent::Codex,
            Some(".codex/AGENTS.md"),
            Some(".codex/config.toml"),
        ),
        (
            Agent::Opencode,
            Some(".config/opencode/AGENTS.md"),
            Some(".config/opencode/opencode.json"),
        ),
        (
            Agent::Pi,
            Some(".pi/agent/AGENTS.md"),
            Some(".pi/agent/mcp.json"),
        ),
        (Agent::Cursor, None, Some(".cursor/mcp.json")),
        (
            Agent::Copilot,
            Some(".copilot/copilot-instructions.md"),
            Some(".copilot/mcp-config.json"),
        ),
    ];
    for (agent, instructions, mcp) in cases {
        let context = RenderContext::new(
            Target::user_with_home_and_environment(
                agent,
                &root,
                std::iter::empty::<(String, String)>(),
            )
            .expect("target"),
            Platform::Macos,
        )
        .expect("context");
        assert_eq!(
            instruction_destination(&context).expect("instructions"),
            instructions.map(|path| root.join(path))
        );
        assert_eq!(
            mcp_destination(&context).expect("mcp"),
            mcp.map(|path| root.join(path))
        );
    }
}
