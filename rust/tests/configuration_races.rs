#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use flyrail::{
    Acquisition, Artifact, Bundle, BundleIdentity, Content, Family, FileContent, FileMode,
    InstallationTarget, InstructionArtifact, OperationStatus, RenderedArtifact, RenderedBundle,
    ResourceAuthority, SectionContent, SkillArtifact, TargetError, TreeContent, apply_preview,
    preview, sync,
};
use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

fn temp_root(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../.cache/rust-tests")
        .join(format!(
            "{}-{}-{label}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
    fs::create_dir_all(&root).expect("temporary root");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private root");
    fs::canonicalize(root).expect("canonical root")
}

fn bundle(version: &str) -> Bundle {
    let tree =
        TreeContent::new(
            [flyrail::BundleEntry::file("SKILL.md", b"skill".to_vec()).expect("file")],
        )
        .expect("tree");
    Bundle::from_artifacts(
        BundleIdentity::new("team", version).expect("identity"),
        [Artifact::Skill(
            SkillArtifact::new("artifact", "skill", tree).expect("skill"),
        )],
        [],
        [],
    )
    .expect("bundle")
}

fn rendered(path: &Path, data: &[u8]) -> RenderedBundle {
    RenderedBundle::new(
        [RenderedArtifact::new(
            "artifact",
            Family::Skills,
            path.to_path_buf(),
            Content::File(FileContent::new(data, FileMode::Readable)),
            None,
            false,
            Vec::new(),
        )
        .expect("artifact")],
        [],
        [],
    )
    .expect("rendered")
}

fn instruction_bundle(version: &str) -> Bundle {
    Bundle::from_artifacts(
        BundleIdentity::new("team", version).expect("identity"),
        [Artifact::Instruction(
            InstructionArtifact::new("artifact", "text").expect("instruction"),
        )],
        [],
        [],
    )
    .expect("bundle")
}

fn section(path: &Path, text: &str) -> RenderedBundle {
    RenderedBundle::new(
        [RenderedArtifact::new(
            "artifact",
            Family::Instructions,
            path.to_path_buf(),
            Content::Section(SectionContent::new("artifact", text, None).expect("section")),
            None,
            false,
            Vec::new(),
        )
        .expect("artifact")],
        [],
        [],
    )
    .expect("rendered")
}

#[test]
#[allow(clippy::too_many_lines)]
fn previews_reject_all_observed_revision_changes_without_overwriting_payloads() {
    for change in [
        "foreign", "owned", "receipt", "index", "mode", "ancestor", "recovery", "hardlink",
        "symlink",
    ] {
        let root = temp_root(&format!("preview-race-{change}"));
        let parent = root.join("project");
        fs::create_dir(&parent).expect("project");
        let path = parent.join("AGENTS.md");
        fs::write(&path, b"# foreign\n").expect("foreign");
        let target = InstallationTarget::without_context(root.join("index")).expect("target");
        sync(
            &instruction_bundle("1"),
            &section(&path, "generated"),
            &target,
            Acquisition::Conflict,
            false,
            Duration::ZERO,
        )
        .expect("install");
        let proposal = preview(
            &instruction_bundle("2"),
            &section(&path, "second"),
            &target,
            Acquisition::Conflict,
            false,
        )
        .expect("preview");
        let state = ResourceAuthority::new(&path)
            .expect("authority")
            .state_root()
            .to_path_buf();
        match change {
            "foreign" => {
                let bytes = fs::read(&path)
                    .expect("payload")
                    .into_iter()
                    .map(|byte| if byte == b'f' { b'x' } else { byte })
                    .collect::<Vec<_>>();
                fs::write(&path, bytes).expect("foreign change");
            }
            "owned" => {
                let bytes = fs::read(&path)
                    .expect("payload")
                    .windows(b"generated".len())
                    .position(|part| part == b"generated")
                    .expect("owned bytes");
                let mut value = fs::read(&path).expect("payload");
                value[bytes] = b'G';
                fs::write(&path, value).expect("owned change");
            }
            "receipt" => {
                let receipt = state.join("receipt.json");
                let mut bytes = fs::read(&receipt).expect("receipt");
                let marker = b"\"transaction_id\":\"";
                let start = bytes
                    .windows(marker.len())
                    .position(|part| part == marker)
                    .expect("transaction")
                    + marker.len();
                bytes[start] = if bytes[start] == b'f' { b'e' } else { b'f' };
                fs::write(receipt, bytes).expect("receipt change");
            }
            "index" => {
                let index = target.index_root().join("index.json");
                let bytes = fs::read(&index).expect("index");
                let changed = String::from_utf8(bytes)
                    .expect("utf8")
                    .replace("\"version\":\"1\"", "\"version\":\"different\"");
                fs::write(index, changed).expect("index change");
            }
            "mode" => {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).expect("chmod");
            }
            "ancestor" => {
                fs::rename(&parent, root.join("old-project")).expect("move ancestor");
                fs::create_dir(&parent).expect("replace ancestor");
                fs::write(&path, b"replacement").expect("replace payload");
            }
            "recovery" => {
                fs::write(state.join("transaction.json"), b"{}").expect("recovery marker");
            }
            "hardlink" => {
                fs::hard_link(&path, root.join("linked")).expect("hardlink");
            }
            "symlink" => {
                fs::rename(&path, root.join("actual")).expect("move payload");
                symlink(root.join("actual"), &path).expect("symlink");
            }
            _ => unreachable!(),
        }
        let before = fs::read(&path).expect("current payload");
        let result = apply_preview(&proposal, Duration::ZERO);
        assert_eq!(result.status(), OperationStatus::Failed, "{change}");
        assert!(
            matches!(
                result.error().map(TargetError::code),
                Some(flyrail::ErrorCode::ConcurrentChange | flyrail::ErrorCode::UnsafePath)
            ),
            "{change}: {:?}",
            result.error()
        );
        assert_eq!(
            fs::read(&path).expect("retained payload"),
            before,
            "{change}"
        );
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn tombstoned_empty_fences_exclude_child_and_tree_boundaries() {
    let root = temp_root("tombstone-race");
    let tree = root.join("tree");
    fs::create_dir(&tree).expect("tree");
    let state = ResourceAuthority::new(&tree)
        .expect("authority")
        .state_root()
        .to_path_buf();
    fs::create_dir(&state).expect("state");
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).expect("private state");

    let child = rendered(&tree.join("child"), b"x");
    let child_result = sync(
        &bundle("1"),
        &child,
        &InstallationTarget::without_context(root.join("index")).expect("target"),
        Acquisition::Conflict,
        false,
        Duration::ZERO,
    )
    .expect("child sync");
    assert_eq!(child_result.status(), OperationStatus::Failed);
    assert_eq!(
        child_result.error().map(TargetError::code),
        Some(flyrail::ErrorCode::Conflict)
    );

    let desired_tree = RenderedBundle::new(
        [RenderedArtifact::new(
            "artifact",
            Family::Skills,
            tree,
            Content::Tree(
                TreeContent::new([
                    flyrail::BundleEntry::file("x", b"x".to_vec()).expect("tree file")
                ])
                .expect("tree"),
            ),
            None,
            false,
            Vec::new(),
        )
        .expect("tree artifact")],
        [],
        [],
    )
    .expect("rendered tree");
    let result = sync(
        &bundle("1"),
        &desired_tree,
        &InstallationTarget::without_context(root.join("other-index")).expect("target"),
        Acquisition::Conflict,
        false,
        Duration::ZERO,
    )
    .expect("tree sync");
    assert_eq!(result.status(), OperationStatus::Failed);
    assert_eq!(
        result.error().map(TargetError::code),
        Some(flyrail::ErrorCode::Conflict)
    );
    let _ = fs::remove_dir_all(root);
}
