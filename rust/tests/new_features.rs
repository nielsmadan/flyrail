#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

mod support;

use flyrail::{
    Acquisition, Agent, Artifact, Audience, Bundle, BundleEntry, BundleIdentity, ChangeAction,
    Command, Content, DirectoryStore, ErrorCode, Family, FileContent, FileMode, HookArtifact,
    HookEvent, HookOutcome, InstructionArtifact, McpArtifact, McpTransport, NativeArtifact,
    NoticeKind, ObservationState, OperationStatus, Platform, RenderContext, SkillArtifact,
    StateStore, SupportAsset, Surface, Target, TargetScope, TreeContent, detect_agents, inspect,
    install, preview, preview_removal, render, render_many, render_skills, summarize, sync,
    translate_instruction_artifact, translate_mcp_artifact, uninstall, update,
};
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct TempRoot(PathBuf);

impl Deref for TempRoot {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<Path> for TempRoot {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_root(name: &str) -> TempRoot {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repository root")
        .join(".cache")
        .join(format!("rust-new-features-{name}-{nonce}"));
    fs::create_dir_all(&root).expect("root");
    TempRoot(root)
}

fn directory_store(path: &Path) -> Arc<dyn StateStore> {
    Arc::new(DirectoryStore::new(path).expect("store"))
}

fn instruction_bundle(version: &str, artifacts: &[(&str, &str)]) -> Bundle {
    Bundle::from_artifacts(
        BundleIdentity::new("guidance", version).expect("identity"),
        artifacts.iter().map(|(id, text)| {
            Artifact::Instruction(InstructionArtifact::new(*id, *text).expect("instruction"))
        }),
        Vec::<SupportAsset>::new(),
        [],
    )
    .expect("bundle")
}

fn skill_bundle() -> Bundle {
    let content = TreeContent::new([BundleEntry::new(
        "SKILL.md",
        Some(b"---\nname: review\n---\nCheck it.\n".to_vec()),
        false,
    )
    .expect("entry")])
    .expect("content");
    Bundle::from_artifacts(
        BundleIdentity::new("skills", "one").expect("identity"),
        [Artifact::Skill(
            SkillArtifact::new("review", "review", content).expect("skill"),
        )],
        Vec::<SupportAsset>::new(),
        [],
    )
    .expect("bundle")
}

#[test]
fn droid_refuses_selected_artifacts_and_blocks_mixed_rendering() {
    let root = temp_root("droid-render");
    let bundle = instruction_bundle("one", &[("guide", "Be careful.\n")]);
    let supported = RenderContext::new(
        Target::project(Agent::Claude, &root).expect("target"),
        Platform::Linux,
    )
    .expect("context");
    let droid = RenderContext::new(
        Target::project(Agent::Droid, &root).expect("target"),
        Platform::Linux,
    )
    .expect("context");

    let refused = render(&bundle, &droid).expect("render");
    assert_eq!(refused.artifacts(), []);
    assert_eq!(refused.notices().len(), 1);
    assert_eq!(refused.notices()[0].artifact_id(), "guide");
    assert_eq!(refused.notices()[0].code(), "agent-unsupported");
    let proposal = preview(
        &bundle,
        &refused,
        &droid
            .installation(directory_store(&root.join(".droid-state")), None)
            .expect("target"),
        Acquisition::Conflict,
        false,
    )
    .expect("preview");
    assert!(!proposal.applicable());

    assert_eq!(
        render_many(&bundle, [supported.clone()])
            .expect("render")
            .artifacts()
            .len(),
        1
    );
    let mixed = render_many(&bundle, [supported, droid]).expect("render");
    assert_eq!(mixed.artifacts(), []);
    assert_eq!(mixed.dependencies(), []);
    assert!(
        mixed
            .notices()
            .iter()
            .any(|notice| notice.code() == "agent-unsupported")
    );
}

#[test]
fn droid_refusal_cannot_be_bypassed_through_public_translation_helpers() {
    let root = temp_root("droid-direct-translation");
    let context = RenderContext::new(
        Target::project(Agent::Droid, &root).expect("target"),
        Platform::Linux,
    )
    .expect("context")
    .with_instruction_path(root.join("instructions.md"))
    .expect("instruction path")
    .with_mcp_path(root.join("mcp.json"))
    .expect("MCP path");
    let instruction =
        InstructionArtifact::new("guide", "Be careful.\n").expect("instruction artifact");
    let mcp = McpArtifact::new(
        "tools",
        "tools",
        McpTransport::Command(
            Command::new(["server"], std::iter::empty::<(String, String)>(), None)
                .expect("command"),
        ),
    )
    .expect("MCP artifact");
    let translated = [
        translate_instruction_artifact("guidance", &instruction, &context).expect("instruction"),
        translate_mcp_artifact(&mcp, &context, std::iter::empty::<(String, PathBuf)>())
            .expect("MCP"),
    ];
    assert!(translated.iter().all(|item| item.artifact().is_none()));
    assert!(translated.iter().all(|item| {
        item.notices().len() == 1
            && item.notices()[0].kind() == NoticeKind::Unsupported
            && item.notices()[0].code() == "agent-unsupported"
    }));
}

#[test]
fn droid_refusal_covers_hooks_and_only_selected_native_artifacts() {
    let root = temp_root("droid-artifacts");
    let droid = RenderContext::new(
        Target::project(Agent::Droid, &root).expect("target"),
        Platform::Linux,
    )
    .expect("context");
    let command =
        Command::new(["check"], std::iter::empty::<(String, String)>(), None).expect("command");
    let bundle = Bundle::from_artifacts(
        BundleIdentity::new("mixed", "one").expect("identity"),
        [
            Artifact::Hook(
                HookArtifact::new(
                    "guard",
                    HookEvent::SessionStart,
                    command,
                    1_000,
                    [HookOutcome::Continue],
                    std::iter::empty::<String>(),
                )
                .expect("hook"),
            ),
            Artifact::Native(
                NativeArtifact::new(
                    "claude-only",
                    Family::Instructions,
                    Audience::new(Agent::Claude, TargetScope::Project, Surface::Cli, None)
                        .expect("audience"),
                    "notes.md",
                    Content::File(FileContent::new(b"Claude.\n".to_vec(), FileMode::Private)),
                )
                .expect("native"),
            ),
            Artifact::Native(
                NativeArtifact::new(
                    "droid-native",
                    Family::Instructions,
                    Audience::new(Agent::Droid, TargetScope::Project, Surface::Cli, None)
                        .expect("audience"),
                    "notes.md",
                    Content::File(FileContent::new(b"Droid.\n".to_vec(), FileMode::Private)),
                )
                .expect("native"),
            ),
        ],
        Vec::<SupportAsset>::new(),
        [],
    )
    .expect("bundle");
    let rendered = render(&bundle, &droid).expect("render");
    assert_eq!(rendered.artifacts(), []);
    assert_eq!(
        rendered
            .notices()
            .iter()
            .map(flyrail::Notice::artifact_id)
            .collect::<Vec<_>>(),
        ["guard", "droid-native"]
    );
    assert!(
        rendered
            .notices()
            .iter()
            .all(|notice| notice.code() == "agent-unsupported")
    );
}

#[test]
fn droid_skill_lifecycle_refuses_before_touching_the_target() {
    let root = temp_root("droid-lifecycle");
    let store = directory_store(&root.join(".state"));
    let bundle = skill_bundle();
    let target = Target::project(Agent::Droid, &root).expect("target");

    let rendered = render_skills(&bundle, &target).expect("render");
    assert_eq!(rendered.artifacts(), []);
    assert_eq!(rendered.notices()[0].code(), "agent-unsupported");
    assert_eq!(rendered.routing_context(), []);

    for result in [
        install(&bundle, [target.clone()], &store)
            .expect("install")
            .remove(0),
        update(&bundle, [target.clone()], &store)
            .expect("update")
            .remove(0),
        uninstall("skills", [target.clone()], &store)
            .expect("uninstall")
            .remove(0),
    ] {
        assert_eq!(result.status(), OperationStatus::Failed);
        assert_eq!(
            result.error().expect("error").code(),
            ErrorCode::Unsupported
        );
        assert!(fs::read_dir(&root).expect("root").next().is_none());
    }

    let inspected = inspect(&bundle, [target], &store)
        .expect("inspect")
        .remove(0);
    assert_eq!(inspected.observation().state(), ObservationState::Unknown);
    assert_eq!(
        inspected.observation().error().expect("error").code(),
        ErrorCode::Unsupported
    );
    assert!(fs::read_dir(&root).expect("root").next().is_none());
}

#[test]
fn summaries_report_create_unchanged_update_delete_and_shared_destinations() {
    let root = temp_root("summary");
    let first = instruction_bundle("one", &[("alpha", "Alpha.\n"), ("beta", "Beta.\n")]);
    let context = RenderContext::new(
        Target::project(Agent::Claude, &root).expect("target"),
        Platform::Linux,
    )
    .expect("context");
    let installation = context
        .installation(directory_store(&root.join(".state")), None)
        .expect("installation");
    let rendered = render(&first, &context).expect("render");
    let creation = preview(
        &first,
        &rendered,
        &installation,
        Acquisition::Conflict,
        false,
    )
    .expect("preview");
    let summary = summarize(&creation).expect("summary");
    assert_eq!(summary.agent(), Some(Agent::Claude));
    assert!(summary.applicable());
    assert_eq!(summary.notices(), rendered.notices());
    assert!(summary.error().is_none());
    assert_eq!(summary.changes().len(), 1);
    assert_eq!(summary.changes()[0].action(), ChangeAction::Create);
    assert_eq!(
        summary.changes()[0].artifact_ids(),
        &[String::from("alpha"), String::from("beta")]
    );
    assert_eq!(
        summary.changes()[0].families(),
        &[Family::Instructions, Family::Instructions]
    );

    sync(
        &first,
        &rendered,
        &installation,
        Acquisition::Conflict,
        false,
        Duration::ZERO,
    )
    .expect("sync");
    let unchanged = preview(
        &first,
        &rendered,
        &installation,
        Acquisition::Conflict,
        false,
    )
    .expect("preview");
    assert_eq!(
        summarize(&unchanged).expect("summary").changes()[0].action(),
        ChangeAction::Unchanged
    );

    let second = instruction_bundle("two", &[("alpha", "Changed.\n"), ("beta", "Beta.\n")]);
    let changed = render(&second, &context).expect("render");
    let update = preview(
        &second,
        &changed,
        &installation,
        Acquisition::Conflict,
        false,
    )
    .expect("preview");
    assert_eq!(
        summarize(&update).expect("summary").changes()[0].action(),
        ChangeAction::Update
    );

    let removal = preview_removal(first.id(), &installation, false).expect("preview");
    assert_eq!(
        summarize(&removal).expect("summary").changes()[0].action(),
        ChangeAction::Delete
    );

    fs::write(root.join("CLAUDE.md"), "Local change.\n").expect("local change");
    let conflict = preview(
        &second,
        &changed,
        &installation,
        Acquisition::Conflict,
        false,
    )
    .expect("preview");
    let summary = summarize(&conflict).expect("summary");
    assert!(!summary.applicable());
    assert_eq!(summary.changes()[0].action(), ChangeAction::Conflict);
}

#[test]
fn summaries_report_recovery_paths_as_conflicts() {
    let root = temp_root("summary-recovery");
    let bundle = instruction_bundle("one", &[("guide", "Be careful.\n")]);
    let context = RenderContext::new(
        Target::project(Agent::Claude, &root).expect("target"),
        Platform::Linux,
    )
    .expect("context");
    let installation = context
        .installation(directory_store(&root.join(".state")), None)
        .expect("installation");
    let rendered = render(&bundle, &context).expect("render");
    sync(
        &bundle,
        &rendered,
        &installation,
        Acquisition::Conflict,
        false,
        Duration::ZERO,
    )
    .expect("sync");
    let observed = preview(
        &bundle,
        &rendered,
        &installation,
        Acquisition::Conflict,
        false,
    )
    .expect("preview");
    let resource = observed.resources()[0].resource().clone();
    let transaction = "f".repeat(32);
    let recovery = flyrail::authority::transaction_path(resource.destination(), &transaction)
        .expect("transaction path");
    let preparation = flyrail::Preparation::new(resource.clone(), transaction, recovery.clone())
        .expect("preparation");
    assert!(
        installation
            .store()
            .write(
                &support::resource_key(resource.destination(), "preparation"),
                &flyrail::encode(&preparation),
                None
            )
            .expect("interrupted preparation")
    );

    let interrupted = preview(
        &bundle,
        &rendered,
        &installation,
        Acquisition::Conflict,
        false,
    )
    .expect("preview");
    let summary = summarize(&interrupted).expect("summary");
    assert!(!summary.applicable());
    assert!(summary.error().is_none());
    assert_eq!(summary.changes()[0].action(), ChangeAction::Conflict);
    assert!(summary.changes()[0].error().is_none());
    assert_eq!(summary.changes()[0].recovery_paths(), &[recovery]);
}

#[test]
fn empty_explicit_discovery_inputs_do_not_select_installed_agents() {
    assert_eq!(
        detect_agents(Some(""), Some(&[]))
            .expect("detection")
            .as_slice(),
        []
    );
}
