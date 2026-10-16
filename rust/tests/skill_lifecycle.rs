#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use flyrail::{
    Artifact, AuthorityError, Bundle, BundleEntry, BundleIdentity, DirectoryStore, ErrorCode,
    ObservationState, OperationStatus, RenderedBundle, SkillArtifact, SkillOptions, SkillTarget,
    StateStore, TreeContent, inspect, install, render_skills, uninstall, update_with_options,
};
use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;
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

fn directory_store(root: &Path) -> Arc<dyn StateStore> {
    Arc::new(DirectoryStore::new(root.join("state")).expect("store"))
}

fn skill(identifier: &str, version: &str, data: &[u8]) -> Bundle {
    let content = TreeContent::new([
        BundleEntry::file("SKILL.md", data.to_vec()).expect("skill file"),
        BundleEntry::directory("empty").expect("skill directory"),
        BundleEntry::executable_file("run", b"run".to_vec()).expect("executable"),
    ])
    .expect("tree");
    Bundle::from_artifacts(
        BundleIdentity::new(identifier, version).expect("identity"),
        [Artifact::Skill(
            SkillArtifact::new("skill", "review", content).expect("skill"),
        )],
        [],
        [],
    )
    .expect("bundle")
}

#[test]
fn skill_lifecycle_tracks_inventory_aliases_updates_and_source_free_removal() {
    let root = temp_root("skill-lifecycle");
    let store = directory_store(&root);
    let target = root.join("skills");
    let first = skill("team", "1", b"original");

    let absent = inspect(&first, [target.clone()], &store).expect("inspect");
    assert_eq!(absent[0].observation().state(), ObservationState::Absent);
    let installed = install(&first, [target.clone(), target.clone()], &store).expect("install");
    assert_eq!(installed[0].status(), OperationStatus::Applied);
    assert_eq!(installed[1].status(), OperationStatus::Applied);
    assert_eq!(installed[1].alias_of(), Some(0));
    assert!(
        store
            .read(&support::resource_key(&target, "authority"))
            .expect("authority read")
            .is_some()
    );
    assert_eq!(store.keys("index/team/").expect("index keys").len(), 1);
    assert_eq!(
        fs::read(target.join("review/SKILL.md")).expect("installed skill"),
        b"original"
    );

    let observed = inspect(&first, [target.clone()], &store).expect("inspect installed");
    assert!(observed[0].observation().is_current());
    let inventory = observed[0]
        .observation()
        .installed()
        .expect("installation")
        .entries();
    assert!(inventory.iter().any(flyrail::InventoryEntry::is_directory));
    let script = inventory
        .iter()
        .find(|entry| entry.path() == "review/run")
        .expect("script inventory");
    assert_eq!(script.size(), Some(3));
    assert!(script.executable());
    assert_eq!(
        script.sha256(),
        Some("acba25512100f80b56fc3ccd14c65be55d94800cda77585c5f41a887e398f9be")
    );

    let second = skill("team", "2", b"changed");
    let refused = install(&second, [target.clone()], &store).expect("install requires update");
    assert_eq!(
        refused[0].error().map(flyrail::TargetError::code),
        Some(ErrorCode::UpdateRequired)
    );
    assert_eq!(
        update_with_options(&second, [target.clone()], &store, SkillOptions::default())
            .expect("update")[0]
            .status(),
        OperationStatus::Applied
    );
    fs::write(target.join("review/SKILL.md"), b"user").expect("modify skill");
    assert_eq!(
        uninstall("team", [target.clone()], &store).expect("safe removal")[0].status(),
        OperationStatus::Failed
    );
    assert_eq!(
        update_with_options(
            &second,
            [target.clone()],
            &store,
            SkillOptions::new(true, Duration::ZERO),
        )
        .expect("replace modified")[0]
            .status(),
        OperationStatus::Applied
    );
    drop(first);
    drop(second);
    assert_eq!(
        uninstall("team", [target.clone()], &store).expect("source-free removal")[0].status(),
        OperationStatus::Applied
    );
    assert!(!target.join("review").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn filesystem_bundle_inspection_survives_source_removal() {
    let root = temp_root("skill-source-free-inspection");
    let store = directory_store(&root);
    let source = root.join("source");
    fs::create_dir_all(source.join("review/empty")).expect("source directories");
    fs::write(source.join("review/SKILL.md"), b"snapshot").expect("skill");
    fs::write(source.join("review/run"), b"run").expect("script");
    fs::write(
        source.join("flyrail.json"),
        br#"{
            "schema_version": 1,
            "id": "team",
            "version": "1",
            "skills": [{"name": "review", "path": "review", "executables": ["run"]}]
        }"#,
    )
    .expect("manifest");
    let bundle = Bundle::from_directory(&source).expect("directory bundle");
    let target = root.join("skills");
    assert_eq!(
        install(&bundle, [target.clone()], &store).expect("install")[0].status(),
        OperationStatus::Applied
    );
    fs::remove_dir_all(source).expect("remove source");

    let observed = inspect(&bundle, [target.clone()], &store).expect("source-free inspection");
    assert!(observed[0].observation().is_current());
    assert_eq!(
        uninstall("team", [target], &store).expect("source-free uninstall")[0].status(),
        OperationStatus::Applied
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn independent_target_failure_does_not_block_healthy_target() {
    let root = temp_root("skill-partial-targets");
    let store = directory_store(&root);
    let failed = root.join("failed");
    let healthy = root.join("healthy");
    symlink(root.join("missing"), &failed).expect("unsafe target link");
    let desired = skill("team", "1", b"content");

    let results = install(
        &desired,
        [failed.clone(), healthy.clone(), healthy.clone()],
        &store,
    )
    .expect("typed target failures");

    assert_eq!(results.len(), 3);
    assert_eq!(results[0].status(), OperationStatus::Failed);
    assert_eq!(
        results[0].error().map(flyrail::TargetError::code),
        Some(ErrorCode::UnsafePath)
    );
    assert_eq!(results[1].status(), OperationStatus::Applied);
    assert_eq!(results[2].alias_of(), Some(1));
    assert_eq!(
        fs::read(healthy.join("review/SKILL.md")).expect("healthy target"),
        b"content"
    );
    assert_eq!(
        fs::read_link(failed).expect("link retained"),
        root.join("missing")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn skill_conflicts_are_reported_without_inspection_writes() {
    let root = temp_root("skill-conflict");
    let store = directory_store(&root);
    let target = root.join("skills");
    fs::create_dir_all(target.join("review")).expect("foreign skill");
    fs::write(target.join("review/SKILL.md"), b"foreign").expect("foreign content");
    let desired = skill("team", "1", b"desired");

    let before = fs::read(target.join("review/SKILL.md")).expect("before");
    let observation = inspect(&desired, [target.clone()], &store).expect("inspection");
    assert_eq!(
        observation[0].observation().state(),
        ObservationState::Absent
    );
    assert_eq!(observation[0].observation().conflicts().len(), 1);
    assert_eq!(
        observation[0].observation().conflicts()[0].path(),
        target.display().to_string()
    );
    assert_eq!(
        fs::read(target.join("review/SKILL.md")).expect("after"),
        before
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn corrupt_receipts_produce_unknown_observations_and_block_installation() {
    let root = temp_root("skill-corrupt-receipt");
    let store = directory_store(&root);
    let target = root.join("skills");
    let desired = skill("team", "1", b"content");
    assert_eq!(
        install(&desired, [target.clone()], &store).expect("install")[0].status(),
        OperationStatus::Applied
    );
    let receipt = support::resource_key(&target, "receipt");
    let recorded = store.read(&receipt).expect("receipt").expect("receipt");
    assert!(
        store
            .write(&receipt, b"{}", Some(&recorded))
            .expect("corrupt receipt")
    );

    let observed = inspect(&desired, [target.clone()], &store).expect("typed inspection");
    assert_eq!(observed[0].observation().state(), ObservationState::Unknown);
    assert_eq!(
        observed[0]
            .observation()
            .error()
            .map(flyrail::TargetError::code),
        Some(ErrorCode::InvalidState)
    );
    assert_eq!(
        install(&desired, [target], &store).expect("blocked install")[0].status(),
        OperationStatus::Failed
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn render_skills_rejects_non_skill_bundles_before_writes() {
    let root = temp_root("skill-render-boundary");
    let bundle = Bundle::from_artifacts(
        BundleIdentity::new("team", "1").expect("identity"),
        [Artifact::Instruction(
            flyrail::InstructionArtifact::new("guide", "text").expect("instruction"),
        )],
        [],
        [],
    )
    .expect("bundle");
    let error = render_skills(&bundle, &root.join("skills")).expect_err("explicit rendering");
    assert_eq!(
        error.to_string(),
        "use explicit rendering for non-skill artifacts"
    );
    assert_eq!(fs::read_dir(&root).expect("root").count(), 0);
    let _ = fs::remove_dir_all(root);
}

#[derive(Clone)]
struct CountingTarget {
    root: PathBuf,
    calls: Arc<AtomicU64>,
}

impl SkillTarget for CountingTarget {
    fn root(&self) -> &Path {
        &self.root
    }

    fn render_bundle(
        &self,
        bundle: &Bundle,
        destination: &Path,
    ) -> Result<RenderedBundle, AuthorityError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        render_skills(bundle, &destination.to_path_buf())
    }
}

#[test]
fn schema_one_skills_use_the_render_callback_once_for_aliases() {
    let root = temp_root("skill-render-seam");
    let store = directory_store(&root);
    let manifest = br#"{
        "schema_version": 1,
        "id": "team",
        "version": "1",
        "skills": [{"name": "review", "path": "review", "executables": ["run"]}]
    }"#;
    let bundle = Bundle::from_memory(
        manifest,
        [
            BundleEntry::file("review/SKILL.md", b"schema one".to_vec()).expect("skill file"),
            BundleEntry::executable_file("review/run", b"run".to_vec()).expect("skill executable"),
        ],
    )
    .expect("schema-one bundle");
    let calls = Arc::new(AtomicU64::new(0));
    let target = CountingTarget {
        root: root.join("skills"),
        calls: Arc::clone(&calls),
    };

    let results = install(&bundle, [target.clone(), target], &store).expect("install aliases");

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(results[1].alias_of(), Some(0));
    assert_eq!(
        fs::read(root.join("skills/review/SKILL.md")).expect("installed schema-one skill"),
        b"schema one"
    );
    let _ = fs::remove_dir_all(root);
}
