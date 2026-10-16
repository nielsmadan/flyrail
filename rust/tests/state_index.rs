#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use flyrail::{
    Acquisition, Agent, Artifact, Bundle, BundleEntry, BundleIdentity, Content, DirectoryStore,
    Family, HookRuntime, HookShell, InstallationTarget, InstructionArtifact, MemoryStore,
    OperationStatus, Platform, RenderContext, RenderedArtifact, RenderedBundle, SectionContent,
    SkillArtifact, StateStore, StoreError, StoreLock, Target, TreeContent, apply_preview,
    inspect_installation, install, preview, remove, render, sync,
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
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temporary root");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private root");
    fs::canonicalize(root).expect("canonical root")
}

fn skills() -> Bundle {
    let content = TreeContent::new([BundleEntry::file("SKILL.md", b"skill".to_vec()).unwrap()])
        .expect("tree");
    Bundle::from_artifacts(
        BundleIdentity::new("team", "1").expect("identity"),
        [Artifact::Skill(
            SkillArtifact::new("skill", "review", content).expect("skill"),
        )],
        [],
        [],
    )
    .expect("bundle")
}

fn instructions(text: &str) -> Bundle {
    Bundle::from_artifacts(
        BundleIdentity::new("team", "1").expect("identity"),
        [Artifact::Instruction(
            InstructionArtifact::new("guide", text).expect("instruction"),
        )],
        [],
        [],
    )
    .expect("bundle")
}

fn rendered(path: &Path) -> RenderedBundle {
    RenderedBundle::new(
        [RenderedArtifact::new(
            "guide",
            Family::Instructions,
            path.to_path_buf(),
            Content::Section(SectionContent::new("guide", "generated", None).expect("section")),
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

fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_owned(), value.to_owned())
}

fn directory_store(path: &Path) -> Arc<dyn StateStore> {
    Arc::new(DirectoryStore::new(path).expect("store"))
}

#[test]
fn a_target_without_context_accepts_any_store_and_its_digest_ignores_the_store() {
    let root = temp_root("state-index-digest");
    let memory: Arc<dyn StateStore> = Arc::new(MemoryStore::new());
    let first = InstallationTarget::without_context(Arc::clone(&memory)).expect("memory target");
    let second = InstallationTarget::without_context(directory_store(&root.join("state")))
        .expect("directory target");
    assert!(Arc::ptr_eq(first.store(), &memory));
    assert_eq!(
        first.context_digest(),
        "37517e5f3dc66819f61f5a7bb8ace1921282415f10551d2defa5c3eb0985b570"
    );
    assert_eq!(second.context_digest(), first.context_digest());
    let located =
        InstallationTarget::new(memory, [pair("destination", "/x")], []).expect("located target");
    assert_eq!(
        located.context_digest(),
        "5b134fc5c9711eb90652d0fecdbb9c2dc5c84098880e0ba529376e1a7a734c0c"
    );
    assert_eq!(fs::read_dir(&root).expect("root").count(), 0);
}

#[test]
fn one_store_keeps_independent_indexes_per_context() {
    let root = temp_root("state-index-contexts");
    let store: Arc<dyn StateStore> = Arc::new(MemoryStore::new());
    let bundle = skills();
    let first = install(&bundle, [root.join("one")], &store).expect("first");
    let second = install(&bundle, [root.join("two")], &store).expect("second");
    assert_eq!(
        [first[0].status(), second[0].status()],
        [OperationStatus::Applied, OperationStatus::Applied]
    );
    assert_eq!(store.keys("index/team/").expect("keys").len(), 2);
    assert_eq!(
        fs::read(root.join("two/review/SKILL.md")).expect("installed"),
        b"skill"
    );
}

#[test]
fn a_store_inside_a_managed_destination_is_refused() {
    let root = temp_root("state-index-inside");
    let container = root.join("skills");
    let store = directory_store(&container.join("state"));
    let results = install(&skills(), [container.clone()], &store).expect("install");
    assert_eq!(results[0].status(), OperationStatus::Failed);
    let message = results[0].error().expect("error").message().to_owned();
    assert!(message.contains("overlap"), "{message}");
    assert_eq!(store.keys("").expect("keys"), Vec::<String>::new());
    assert!(!container.exists());
}

#[test]
fn an_aliased_store_location_still_overlaps_a_destination() {
    for spelling in ["traversal", "symlink"] {
        let root = temp_root(&format!("state-index-alias-{spelling}"));
        fs::create_dir(root.join("other")).expect("other");
        symlink(&root, root.join("link")).expect("link");
        let parent = if spelling == "traversal" {
            root.join("other").join("..")
        } else {
            root.join("link")
        };
        let store = directory_store(&parent.join("AGENTS.md").join("state"));
        let target = InstallationTarget::without_context(Arc::clone(&store)).expect("target");
        let error = preview(
            &instructions("text"),
            &rendered(&root.join("AGENTS.md")),
            &target,
            Acquisition::Conflict,
            false,
        )
        .expect_err(spelling);
        assert!(
            error.message().contains("state store overlaps a resource"),
            "{spelling}: {}",
            error.message()
        );
        assert_eq!(store.keys("").expect("keys"), Vec::<String>::new());
    }
}

#[test]
fn a_store_behind_a_dangling_symlink_into_a_skill_container_overlaps_it() {
    let root = temp_root("state-index-dangling");
    let container = root.join("skills");
    symlink(&container, root.join("link")).expect("dangling link");
    let store = directory_store(&root.join("link/state"));
    let target = InstallationTarget::without_context(Arc::clone(&store)).expect("target");
    let content = TreeContent::new([BundleEntry::file("SKILL.md", b"skill".to_vec()).unwrap()])
        .expect("tree");
    let rendered = RenderedBundle::new(
        [RenderedArtifact::new(
            "skill",
            Family::Skills,
            container.clone(),
            Content::Tree(content),
            Some(String::from("review")),
            false,
            Vec::new(),
        )
        .expect("artifact")],
        [],
        [],
    )
    .expect("rendered");
    let error = preview(&skills(), &rendered, &target, Acquisition::Conflict, false)
        .expect_err("dangling store overlap");
    assert_eq!(
        error.message(),
        "state store overlaps a resource or authority"
    );
    assert_eq!(store.keys("").expect("keys"), Vec::<String>::new());
    assert!(!container.exists());
}

struct Staged {
    records: MemoryStore,
    staging: PathBuf,
}

impl StateStore for Staged {
    fn staging_directory(&self) -> Option<PathBuf> {
        Some(self.staging.clone())
    }
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.records.read(key)
    }
    fn write(&self, key: &str, value: &[u8], expected: Option<&[u8]>) -> Result<bool, StoreError> {
        self.records.write(key, value, expected)
    }
    fn delete(&self, key: &str, expected: &[u8]) -> Result<bool, StoreError> {
        self.records.delete(key, expected)
    }
    fn keys(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        self.records.keys(prefix)
    }
    fn lock(&self, name: &str, timeout: Duration) -> Result<StoreLock<'_>, StoreError> {
        self.records.lock(name, timeout)
    }
}

#[test]
fn a_staging_directory_that_overlaps_a_destination_is_refused() {
    let root = temp_root("state-index-staging");
    let store: Arc<dyn StateStore> = Arc::new(Staged {
        records: MemoryStore::new(),
        staging: root.join("state.staging"),
    });
    let target = InstallationTarget::without_context(store).expect("target");
    let error = preview(
        &instructions("text"),
        &rendered(&root.join("state.staging").join("AGENTS.md")),
        &target,
        Acquisition::Conflict,
        false,
    )
    .expect_err("staging overlap");
    assert!(
        error.message().contains("state store overlaps a resource"),
        "{}",
        error.message()
    );
    assert_eq!(fs::read_dir(&root).expect("root").count(), 0);
}

#[test]
fn preview_approves_parents_created_for_the_store() {
    let root = temp_root("state-index-parents");
    let target = InstallationTarget::without_context(directory_store(&root.join("project/state")))
        .expect("target");
    let plan = preview(
        &instructions("text"),
        &rendered(&root.join("project/AGENTS.md")),
        &target,
        Acquisition::Conflict,
        false,
    )
    .expect("preview");
    let result = apply_preview(&plan, Duration::ZERO);
    assert_eq!(
        result.status(),
        OperationStatus::Applied,
        "{:?}",
        result.error()
    );
    let written = fs::read_to_string(root.join("project/AGENTS.md")).expect("written");
    assert!(written.contains("generated"), "{written}");
}

#[test]
fn render_contexts_sharing_a_store_keep_separate_installations() {
    let root = temp_root("state-index-render-contexts");
    let store = directory_store(&root.join("state"));
    let project = root.join("project");
    let shared = instructions("Shared guidance.\n");
    let contexts = [Agent::Claude, Agent::Codex].map(|agent| {
        RenderContext::new(
            Target::project(agent, &project).expect("target"),
            Platform::Linux,
        )
        .expect("context")
    });
    let installations = contexts.each_ref().map(|context| {
        context
            .installation(Arc::clone(&store), None)
            .expect("installation")
    });
    let project_text = project.display().to_string();
    assert_eq!(
        installations[0].context(),
        [
            pair("agent", "claude"),
            pair("root", &project_text),
            pair("scope", "project"),
            pair("surface", "cli"),
        ]
    );
    assert_eq!(
        contexts[0]
            .installation(Arc::clone(&store), Some(vec![pair("app", "notes")]))
            .expect("explicit installation")
            .context(),
        [pair("app", "notes")]
    );
    for (context, installation) in contexts.iter().zip(&installations) {
        let result = sync(
            &shared,
            &render(&shared, context).expect("render"),
            installation,
            Acquisition::Conflict,
            false,
            Duration::ZERO,
        )
        .expect("sync");
        assert_eq!(
            result.status(),
            OperationStatus::Applied,
            "{:?}",
            result.error()
        );
    }
    for name in ["CLAUDE.md", "AGENTS.md"] {
        let text = fs::read_to_string(project.join(name)).expect("instructions");
        assert!(text.contains("Shared guidance."), "{name}: {text}");
    }
    for installation in &installations {
        assert!(
            inspect_installation("team", installation)
                .expect("inspection")
                .is_current()
        );
    }
}

#[test]
fn removal_from_a_context_without_a_hook_runtime_removes_the_content() {
    let root = temp_root("state-index-volatile");
    let store = directory_store(&root.join("state"));
    let shared = instructions("Shared.\n");
    let installed = RenderContext::new(
        Target::project(Agent::Codex, &root).expect("target"),
        Platform::Macos,
    )
    .expect("context")
    .with_hook_runtime(HookRuntime::new(None, Some(HookShell::Posix)).expect("runtime"))
    .with_asset_root(root.join("assets"))
    .expect("asset root");
    let removing = RenderContext::new(
        Target::project(Agent::Codex, &root).expect("target"),
        Platform::Linux,
    )
    .expect("context");
    let result = sync(
        &shared,
        &render(&shared, &installed).expect("render"),
        &installed
            .installation(Arc::clone(&store), None)
            .expect("installation"),
        Acquisition::Conflict,
        false,
        Duration::ZERO,
    )
    .expect("sync");
    assert_eq!(
        result.status(),
        OperationStatus::Applied,
        "{:?}",
        result.error()
    );
    let removed = remove(
        "team",
        &removing.installation(store, None).expect("installation"),
        false,
        Duration::ZERO,
    )
    .expect("remove");
    assert_eq!(
        removed.status(),
        OperationStatus::Applied,
        "{:?}",
        removed.error()
    );
    let mut names = fs::read_dir(&root)
        .expect("root")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["state"]);
}
