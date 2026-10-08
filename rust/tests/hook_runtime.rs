#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::too_many_lines)]

use base64::Engine as _;
use flyrail::{
    HookEffects, HookEvent, HookJson, HookOutcome, HookRequest, HookResponse, HookRuntime,
};
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn executable(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

fn versions_supported(ci: bool, node: &str, npm: &str, bun: &str) -> bool {
    if ci {
        return node == "24.21.0" && npm == "11.19.0" && bun == "1.4.2";
    }
    let Some(node_major) = node
        .split('.')
        .next()
        .and_then(|value| value.parse::<u32>().ok())
    else {
        return false;
    };
    let bun_parts = bun
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>();
    node_major >= 24 && bun_parts.is_ok_and(|parts| parts.as_slice() >= &[1, 4, 2])
}

fn tools() -> Option<(PathBuf, PathBuf)> {
    let node = executable("node")?;
    let npm = executable("npm")?;
    let bun = executable("bun")?;
    let node_version = Command::new(&node).arg("--version").output().ok()?;
    let npm_version = Command::new(&npm).arg("--version").output().ok()?;
    let bun_version = Command::new(&bun).arg("--version").output().ok()?;
    if !node_version.status.success()
        || !npm_version.status.success()
        || !bun_version.status.success()
    {
        return None;
    }
    let node_text = String::from_utf8(node_version.stdout).ok()?;
    let npm_text = String::from_utf8(npm_version.stdout).ok()?;
    let bun_text = String::from_utf8(bun_version.stdout).ok()?;
    let node_text = node_text.trim().trim_start_matches('v');
    let supported = versions_supported(
        env::var_os("CI").is_some(),
        node_text,
        npm_text.trim(),
        bun_text.trim(),
    );
    supported.then_some((node, bun))
}

#[test]
fn ci_requires_the_exact_runtime_toolchain() {
    assert!(versions_supported(true, "24.21.0", "11.19.0", "1.4.2"));
    assert!(!versions_supported(true, "24.21.0", "11.18.0", "1.4.2"));
    assert!(!versions_supported(true, "24.20.0", "11.19.0", "1.4.2"));
    assert!(!versions_supported(true, "24.21.0", "11.19.0", "1.4.1"));
    assert!(versions_supported(false, "25.0.0", "10.0.0", "1.5.0"));
}

fn require_tools() -> Option<(PathBuf, PathBuf)> {
    let found = tools();
    if found.is_none() {
        let notice = "SKIP: hook runtime integration requires Node 24+, npm and Bun 1.4.2+";
        assert!(
            env::var_os("CI").is_none(),
            "{notice}; CI acceptance jobs may not skip"
        );
        eprintln!("{notice}");
    }
    found
}

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../spec/fixtures/hooks.json")
}

fn temporary(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos());
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../.cache/tmp")
        .join(format!("rust-hook-{name}-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&path).expect("create temporary hook root");
    path.canonicalize().expect("canonical temporary hook root")
}

fn write_runtime(root: &Path) {
    let runtime = HookRuntime::new(None, None).expect("runtime");
    for entry in runtime.resources() {
        fs::write(
            root.join(entry.path()),
            entry.data().expect("runtime file data"),
        )
        .expect("write runtime file");
    }
}

fn invoke(binary: &Path, request: &Value) -> Value {
    let mut child = Command::new(binary)
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/hook_runtime_harness.mjs"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start hook harness");
    serde_json::to_writer(child.stdin.take().expect("stdin"), request).expect("write request");
    let output = child.wait_with_output().expect("wait for hook harness");
    assert!(
        output.status.success(),
        "hook harness failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "hook harness stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse hook harness output")
}

#[test]
fn packaged_runtime_matches_authoritative_python_bytes() {
    let python = Path::new(env!("CARGO_MANIFEST_DIR")).join("../python/src/flyrail/runtime");
    if !python.is_dir() {
        eprintln!("SKIP: Python runtime source is absent from this package checkout");
        return;
    }
    let runtime = HookRuntime::new(None, None).expect("runtime");
    assert_eq!(runtime.resources().len(), 3);
    for entry in runtime.resources() {
        assert_eq!(
            entry.data().expect("runtime bytes"),
            fs::read(python.join(entry.path())).expect("read Python runtime")
        );
    }
}

#[test]
fn all_shared_protocol_vectors_execute_through_packaged_runtime() {
    let Some((node, bun)) = require_tools() else {
        return;
    };
    let fixture = fixture_path();
    if !fixture.is_file() {
        eprintln!("SKIP: shared hooks fixture is absent from this package checkout");
        return;
    }
    let document: Value =
        serde_json::from_slice(&fs::read(fixture).expect("read hook fixtures")).expect("fixtures");
    let cases = document["cases"].as_array().expect("fixture cases");
    assert_eq!(cases.len(), 58);
    let root = temporary("vectors");
    write_runtime(&root);
    for case in cases {
        let capture = root.join("capture.json");
        let binary = if case["host"] == "opencode" {
            &bun
        } else {
            &node
        };
        let result = invoke(
            binary,
            &json!({
                "action": "case",
                "runtime": root,
                "capture": capture,
                "cwd": root,
                "node": node,
                "case": case,
            }),
        );
        let name = case["name"].as_str().expect("case name");
        assert_eq!(
            result["observed"], case["expected"],
            "{name}: native output"
        );
        let mut context = case["context"].as_object().expect("context").clone();
        if case["host"] == "pi" {
            let mut ordered = serde_json::Map::new();
            ordered.insert("cwd".into(), json!(root));
            ordered.insert("execution_mode".into(), json!("print"));
            ordered.insert("has_ui".into(), json!(false));
            for key in ["tool_call_id", "source"] {
                if let Some(value) = context.get(key) {
                    ordered.insert(key.to_owned(), value.clone());
                }
            }
            context = ordered;
        } else if case["host"] == "opencode" {
            let mut ordered = serde_json::Map::new();
            ordered.insert("cwd".into(), json!(root));
            for key in ["session_id", "tool_call_id"] {
                if let Some(value) = context.get(key) {
                    ordered.insert(key.to_owned(), value.clone());
                }
            }
            context = ordered;
        }
        if case["host"] == "copilot" {
            let mut ordered = serde_json::Map::new();
            for key in [
                "cwd",
                "session_id",
                "timestamp",
                "stop_hook_active",
                "stop_reason",
                "tool_call_id",
            ] {
                if let Some(value) = context.get(key) {
                    ordered.insert(key.to_owned(), value.clone());
                }
            }
            context = ordered;
        }
        let canonical = case["canonical"].as_object().expect("canonical");
        let mut expected = format!(
            "{{\"version\":1,\"event\":{},\"context\":{}",
            serde_json::to_string(&case["event"]).expect("event JSON"),
            serde_json::to_string(&Value::Object(context)).expect("context JSON")
        );
        for key in ["tool", "input", "result"] {
            if let Some(value) = canonical.get(key) {
                expected.push(',');
                expected.push_str(&serde_json::to_string(key).expect("key JSON"));
                expected.push(':');
                expected.push_str(&serde_json::to_string(value).expect("value JSON"));
            }
        }
        expected.push('}');
        let packet = base64::engine::general_purpose::STANDARD
            .decode(result["packetBase64"].as_str().expect("packet base64"))
            .expect("decode packet");
        assert_eq!(
            packet,
            expected.as_bytes(),
            "{name}: canonical packet bytes"
        );
    }
    fs::remove_dir_all(root).expect("remove temporary hook root");
}

#[test]
fn node_and_bun_enforce_protocol_and_process_bounds() {
    let Some((node, bun)) = require_tools() else {
        return;
    };
    let root = temporary("bounds");
    write_runtime(&root);
    for binary in [&node, &bun] {
        for (data, error) in [
            ("eyJhIjoxLCJhIjoyfQ==", "flyrail-hook:json-key"),
            ("eyJfX3Byb3RvX18iOnt9fQ==", "flyrail-hook:json-key"),
            ("eyJ4IjoxZTQwMH0=", "flyrail-hook:json-number"),
            ("/w==", "flyrail-hook:invalid-json"),
        ] {
            let invalid = invoke(
                binary,
                &json!({"action": "parse", "runtime": root, "data": data}),
            );
            assert_eq!(invalid["error"], error);
        }
        for (script, error) in [
            ("process.exit(7)", "flyrail-hook:exit"),
            ("process.kill(process.pid,'SIGTERM')", "flyrail-hook:exit"),
            ("setInterval(()=>{},1000)", "flyrail-hook:timeout"),
            (
                "process.stdout.write('x'.repeat(65537))",
                "flyrail-hook:stdout-limit",
            ),
            (
                "process.stderr.write('x'.repeat(16385))",
                "flyrail-hook:stderr-limit",
            ),
            (
                "process.stdout.write(Buffer.from([255]))",
                "flyrail-hook:invalid-output",
            ),
            ("process.stdout.write('{')", "flyrail-hook:invalid-output"),
            (
                "process.stdout.write('{\"version\":1,\"outcome\":\"continue\",\"extra\":true}')",
                "flyrail-hook:output-fields",
            ),
            (
                "process.stdout.write('{\"version\":1,\"outcome\":\"block\"}')",
                "flyrail-hook:output-fields",
            ),
            (
                "process.stdout.write('{\"version\":1,\"outcome\":\"block\",\"reason\":\"\"}')",
                "flyrail-hook:reason",
            ),
            (
                "process.stdout.write('{\"version\":1,\"outcome\":\"context\",\"text\":\"\"}')",
                "flyrail-hook:context",
            ),
            (
                "process.stdout.write('{\"version\":1,\"outcome\":\"modify-input\",\"input\":[]}')",
                "flyrail-hook:replacement",
            ),
            (
                "process.stdout.write('{\"version\":2,\"outcome\":\"continue\"}')",
                "flyrail-hook:undeclared-outcome",
            ),
            (
                "process.stdout.write('{\"version\":1,\"outcome\":\"permit\"}')",
                "flyrail-hook:undeclared-outcome",
            ),
            (
                "process.stdout.write('{\"version\":1,\"version\":1,\"outcome\":\"continue\"}')",
                "flyrail-hook:invalid-output",
            ),
            (
                "process.stdout.write('{\"version\":1,\"outcome\":\"continue\"}');process.stderr.write(Buffer.from([255]))",
                "flyrail-hook:invalid-output",
            ),
        ] {
            let timeout = if script.starts_with("setInterval") {
                20
            } else {
                2_000
            };
            let result = invoke(
                binary,
                &json!({
                    "action": "run",
                    "runtime": root,
                    "config": {
                        "host": "test",
                        "handlers": [{
                            "id": "handler",
                            "event": "before-tool",
                            "argv": [node, "-e", script],
                            "env": {},
                            "cwd": root,
                            "timeout_ms": timeout,
                            "outcomes": ["continue", "block", "ask", "context", "modify-input"],
                            "tools": [],
                        }],
                    },
                    "input": {
                        "version": 1,
                        "event": "before-tool",
                        "tool": "Bash",
                        "input": {},
                        "context": {},
                    },
                }),
            );
            assert_eq!(result["error"], error, "{script}");
        }
        for changes in [
            json!({"argv": [root.join("missing-private-command")]}),
            json!({"cwd": root.join("missing-private-cwd")}),
        ] {
            let mut handler = json!({
                "id": "handler",
                "event": "before-tool",
                "argv": [node, "-e", ""],
                "env": {},
                "cwd": root,
                "timeout_ms": 2_000,
                "outcomes": ["continue"],
                "tools": [],
            });
            for (key, value) in changes.as_object().expect("changes") {
                handler[key] = value.clone();
            }
            let result = invoke(
                binary,
                &json!({
                    "action": "run",
                    "runtime": root,
                    "config": {"host": "test", "handlers": [handler]},
                    "input": {
                        "version": 1,
                        "event": "before-tool",
                        "tool": "Bash",
                        "input": {},
                        "context": {},
                    },
                }),
            );
            assert_eq!(result, json!({"error": "flyrail-hook:spawn"}));
        }
        let deadline = invoke(
            binary,
            &json!({
                "action": "run",
                "runtime": root,
                "clockStep": 150_001,
                "config": {
                    "host": "test",
                    "handlers": [{
                        "id": "handler",
                        "event": "before-tool",
                        "argv": [node, "-e", "process.stdout.write('{\"version\":1,\"outcome\":\"continue\"}')"],
                        "env": {},
                        "cwd": root,
                        "timeout_ms": 1000,
                        "outcomes": ["continue"],
                        "tools": [],
                    }],
                },
                "input": {
                    "version": 1,
                    "event": "before-tool",
                    "tool": "Bash",
                    "input": {},
                    "context": {},
                },
            }),
        );
        assert_eq!(deadline, json!({"error": "flyrail-hook:event-timeout"}));
        for (held_open, error) in [
            (false, "flyrail-hook:input-limit"),
            (true, "flyrail-hook:stdin-timeout"),
        ] {
            let result = invoke(
                binary,
                &json!({
                    "action": "readInput",
                    "runtime": root,
                    "heldOpen": held_open,
                }),
            );
            assert_eq!(result, json!({"error": error}));
        }
        if cfg!(unix) {
            let marker = root.join(format!(
                "descendant-{}",
                binary
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("runtime")
            ));
            let child = format!(
                "setTimeout(()=>require('node:fs').writeFileSync({},'ran'),500)",
                serde_json::to_string(&marker).expect("marker path")
            );
            let script = format!(
                "require('node:child_process').spawn(process.execPath,['-e',{}],{{stdio:'inherit'}});process.stdout.write('{{\"version\":1,\"outcome\":\"continue\"}}');process.exit(0)",
                serde_json::to_string(&child).expect("child script")
            );
            let result = invoke(
                binary,
                &json!({
                    "action": "run",
                    "runtime": root,
                    "config": {
                        "host": "test",
                        "handlers": [{
                            "id": "handler",
                            "event": "before-tool",
                            "argv": [node, "-e", script],
                            "env": {},
                            "cwd": root,
                            "timeout_ms": 1000,
                            "outcomes": ["continue"],
                            "tools": [],
                        }],
                    },
                    "input": {
                        "version": 1,
                        "event": "before-tool",
                        "tool": "Bash",
                        "input": {},
                        "context": {},
                    },
                }),
            );
            assert_eq!(result, json!({"result": {"contexts": []}}));
            std::thread::sleep(std::time::Duration::from_millis(600));
            assert!(!marker.exists(), "descendant pipe holder survived cleanup");
        }
        let environment = invoke(
            binary,
            &json!({
                "action": "run",
                "runtime": root,
                "config": {
                    "host": "test",
                    "handlers": [{
                        "id": "handler",
                        "event": "before-tool",
                        "argv": [node, "-e", "process.stdout.write(JSON.stringify({version:1,outcome:'context',text:JSON.stringify([process.env.__proto__,process.env.FOUND,process.cwd()])}))"],
                        "env": {
                            "__proto__": "literal",
                            "FOUND": {"ref": "PATH"},
                        },
                        "cwd": root,
                        "timeout_ms": 1000,
                        "outcomes": ["context"],
                        "tools": ["Bash"],
                    }],
                },
                "input": {
                    "version": 1,
                    "event": "before-tool",
                    "tool": "Bash",
                    "input": {},
                    "context": {},
                },
            }),
        );
        let captured: Value = serde_json::from_str(
            environment["result"]["contexts"]
                .get(0)
                .and_then(Value::as_str)
                .expect("captured environment"),
        )
        .expect("environment JSON");
        assert_eq!(captured[0], "literal");
        assert_eq!(captured[1], env::var("PATH").expect("PATH"));
        assert_eq!(captured[2], root.display().to_string());
    }
    fs::remove_dir_all(root).expect("remove temporary hook root");
}

#[test]
fn rust_utf16_string_bytes_match_packaged_node_runtime() {
    let Some((node, _)) = require_tools() else {
        return;
    };
    let root = temporary("utf16");
    write_runtime(&root);
    for source in [
        r#""\ud800""#,
        r#""\udfff""#,
        r#""\ud83d\ude00""#,
        r#"{"\ud800":"\udfff"}"#,
        r#"{"\ud83d\ude00":"pair"}"#,
    ] {
        let result = invoke(
            &node,
            &json!({
                "action": "parse",
                "runtime": root,
                "data": base64::engine::general_purpose::STANDARD.encode(source),
            }),
        );
        let runtime = base64::engine::general_purpose::STANDARD
            .decode(result["bytesBase64"].as_str().expect("runtime bytes"))
            .expect("decode runtime bytes");
        assert_eq!(
            HookJson::parse(source.as_bytes())
                .expect("Rust UTF-16 JSON")
                .to_bytes(),
            runtime,
            "{source}"
        );
    }
    fs::remove_dir_all(root).expect("remove temporary hook root");
}

#[test]
fn typed_utf16_packets_match_packaged_node_runtime() {
    let Some((node, _)) = require_tools() else {
        return;
    };
    let root = temporary("typed-utf16");
    write_runtime(&root);
    for escaped in [r"\ud800", r"\udfff", r"\ud83d\ude00"] {
        for (input, final_outcome) in [
            (
                format!(r#"{{"version":1,"event":"prompt","context":{{}},"input":"{escaped}"}}"#),
                "block",
            ),
            (
                format!(
                    r#"{{"version":1,"event":"before-tool","context":{{}},"tool":"{escaped}","input":{{}}}}"#
                ),
                "ask",
            ),
        ] {
            let mut responses = Vec::new();
            if final_outcome == "block" {
                responses.push(format!(
                    r#"{{"version":1,"outcome":"modify-input","input":"{escaped}"}}"#
                ));
            }
            responses.extend([
                format!(r#"{{"version":1,"outcome":"context","text":"{escaped}"}}"#),
                format!(r#"{{"version":1,"outcome":"{final_outcome}","reason":"{escaped}"}}"#),
            ]);
            let result = invoke(
                &node,
                &json!({
                    "action": "typed",
                    "runtime": root,
                    "cwd": root,
                    "node": node,
                    "input": base64::engine::general_purpose::STANDARD.encode(&input),
                    "responses": responses
                        .iter()
                        .map(|response| base64::engine::general_purpose::STANDARD.encode(response))
                        .collect::<Vec<_>>(),
                }),
            );
            assert!(result.get("error").is_none(), "{input}: {result}");
            let runtime_input = base64::engine::general_purpose::STANDARD
                .decode(result["inputBase64"].as_str().expect("input bytes"))
                .expect("decode input");
            assert_eq!(
                HookRequest::parse(input.as_bytes())
                    .expect("Rust request")
                    .to_bytes(),
                runtime_input,
                "{input}"
            );
            for (index, response) in responses.iter().enumerate() {
                let runtime_response = base64::engine::general_purpose::STANDARD
                    .decode(
                        result["responseBase64"][index]
                            .as_str()
                            .expect("response bytes"),
                    )
                    .expect("decode response");
                assert_eq!(
                    HookResponse::parse(
                        response.as_bytes(),
                        if final_outcome == "block" {
                            HookEvent::Prompt
                        } else {
                            HookEvent::BeforeTool
                        },
                        &[
                            HookOutcome::Context,
                            HookOutcome::Block,
                            HookOutcome::Ask,
                            HookOutcome::ModifyInput,
                        ],
                    )
                    .expect("Rust response")
                    .to_bytes(),
                    runtime_response,
                    "{response}"
                );
            }
            let runtime_effects = base64::engine::general_purpose::STANDARD
                .decode(result["effectsBase64"].as_str().expect("effects bytes"))
                .expect("decode effects");
            assert_eq!(
                HookEffects::parse(&runtime_effects)
                    .expect("Rust effects")
                    .to_bytes(),
                runtime_effects,
                "{input}"
            );
        }
    }
    fs::remove_dir_all(root).expect("remove temporary hook root");
}

#[test]
fn mixed_semantic_error_precedence_matches_packaged_node_runtime() {
    let Some((node, _)) = require_tools() else {
        return;
    };
    let root = temporary("semantic-order");
    write_runtime(&root);
    let deep = format!("{}0{}", "[".repeat(65), "]".repeat(65));
    for source in [
        format!(r#"{{"__proto__":0,"deep":{deep}}}"#),
        format!(r#"{{"a":0,"a":1,"deep":{deep}}}"#),
        format!(r#"{{"value":1e400,"deep":{deep}}}"#),
        format!(r#"{{"deep":{deep},"__proto__":0}}"#),
        format!(r#"{{"deep":{deep},"a":0,"a":1}}"#),
        format!(r#"{{"deep":{deep},"value":1e400}}"#),
        format!(r#"{{"__proto__":0,"deep":{deep}"#),
    ] {
        let runtime = invoke(
            &node,
            &json!({
                "action": "parse",
                "runtime": root,
                "data": base64::engine::general_purpose::STANDARD.encode(&source),
            }),
        );
        assert_eq!(
            HookJson::parse(source.as_bytes())
                .expect_err("Rust semantic failure")
                .to_string(),
            runtime["error"].as_str().expect("runtime semantic failure"),
            "{source}"
        );
    }
    fs::remove_dir_all(root).expect("remove temporary hook root");
}

#[test]
fn rust_number_bytes_match_packaged_node_runtime() {
    let Some((node, _)) = require_tools() else {
        return;
    };
    let root = temporary("numbers");
    write_runtime(&root);
    for source in [
        "9007199254740991",
        "9007199254740992",
        "9007199254740993",
        "100000000000000000000",
        "1e21",
        "5e-324",
        "-0",
        "-0.0",
    ] {
        let result = invoke(
            &node,
            &json!({
                "action": "parse",
                "runtime": root,
                "data": base64::engine::general_purpose::STANDARD.encode(source),
            }),
        );
        let runtime = base64::engine::general_purpose::STANDARD
            .decode(result["bytesBase64"].as_str().expect("runtime bytes"))
            .expect("decode runtime bytes");
        assert_eq!(
            HookJson::parse(source.as_bytes())
                .expect("Rust number")
                .to_bytes(),
            runtime,
            "{source}"
        );
    }
    fs::remove_dir_all(root).expect("remove temporary hook root");
}
