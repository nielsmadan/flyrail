#![allow(
    clippy::case_sensitive_file_extension_comparisons,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::significant_drop_tightening,
    clippy::unwrap_used
)]

mod support;

use flyrail::{
    Acquisition, Artifact, Bundle, BundleEntry, BundleIdentity, Content, DirectoryStore, ErrorCode,
    Family, InstallationResult, InstallationTarget, InstructionArtifact, MemoryStore,
    OperationStatus, Preparation, RenderedArtifact, RenderedBundle, SectionContent, StateStore,
    StoreError, StoreLock, TargetError, TreeContent, apply_preview, decode, encode,
    inspect_installation, preview, recover_installation, remove, sync,
};
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _, symlink};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use support::{destination_key, resource_key};

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

fn bundle(version: &str) -> Bundle {
    instruction_bundle("team", version, "guide")
}

fn instruction_bundle(identifier: &str, version: &str, artifact: &str) -> Bundle {
    Bundle::from_artifacts(
        BundleIdentity::new(identifier, version).expect("identity"),
        [Artifact::Instruction(
            InstructionArtifact::new(artifact, "text").expect("instruction"),
        )],
        [],
        [],
    )
    .expect("bundle")
}

fn section(artifact: &str, path: &Path, text: &str) -> RenderedBundle {
    RenderedBundle::new(
        [RenderedArtifact::new(
            artifact,
            Family::Instructions,
            path.to_path_buf(),
            Content::Section(SectionContent::new(artifact, text, None).expect("section")),
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

fn rendered(path: &Path, text: &str) -> RenderedBundle {
    section("guide", path, text)
}

fn directory_store(path: &Path) -> Arc<dyn StateStore> {
    Arc::new(DirectoryStore::new(path).expect("store"))
}

fn target_of(store: Arc<dyn StateStore>) -> InstallationTarget {
    InstallationTarget::without_context(store).expect("target")
}

fn target(root: &Path) -> InstallationTarget {
    target_of(directory_store(&root.join("state")))
}

fn run(
    bundle: &Bundle,
    rendered: &RenderedBundle,
    target: &InstallationTarget,
) -> InstallationResult {
    sync(
        bundle,
        rendered,
        target,
        Acquisition::Conflict,
        false,
        Duration::ZERO,
    )
    .expect("sync")
}

fn applied(bundle: &Bundle, rendered: &RenderedBundle, target: &InstallationTarget) {
    let result = run(bundle, rendered, target);
    assert_eq!(
        result.status(),
        OperationStatus::Applied,
        "{:?}",
        result.error()
    );
}

fn recover(target: &InstallationTarget) -> InstallationResult {
    recover_installation("team", target, Duration::ZERO).expect("recover")
}

fn names(directory: &Path) -> Vec<String> {
    let mut names = fs::read_dir(directory)
        .expect("listing")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn management_name(name: &str) -> bool {
    let folded = name.to_lowercase();
    folded.starts_with(".flyrail-") && folded.ends_with(".txn")
}

fn transaction_directories(destination: &Path, store: Option<&dyn StateStore>) -> Vec<PathBuf> {
    let parent = destination.parent().expect("parent");
    let mut result = fs::read_dir(parent)
        .expect("listing")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| management_name(&name.to_string_lossy()))
        })
        .collect::<Vec<_>>();
    if let Some(staging) = store.and_then(StateStore::staging_directory)
        && staging.exists()
    {
        result.extend(
            fs::read_dir(staging)
                .expect("staging listing")
                .map(|entry| entry.expect("entry").path()),
        );
    }
    result.sort();
    result
}

fn preparation(store: &dyn StateStore, destination: &Path) -> Option<Preparation> {
    store
        .read(&resource_key(destination, "preparation"))
        .expect("preparation read")
        .map(|bytes| decode::<Preparation>(&bytes).expect("preparation record"))
}

fn journal_directory(store: &dyn StateStore, destination: &Path) -> PathBuf {
    preparation(store, destination)
        .expect("preparation")
        .directory()
        .to_path_buf()
}

fn io_failure(message: &str) -> StoreError {
    StoreError::Io {
        message: message.to_owned(),
        path: None,
        kind: io::ErrorKind::Other,
        errno: Some(libc::EIO),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    Read,
    Write,
    Delete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Before,
    After,
}

struct Call<'a> {
    op: Op,
    phase: Phase,
    key: &'a str,
    inner: &'a dyn StateStore,
}

type Hook = Box<dyn FnMut(&Call<'_>) -> Result<(), StoreError> + Send>;

struct Hooked {
    inner: Box<dyn StateStore>,
    hook: Mutex<Option<Hook>>,
}

impl Hooked {
    fn new(inner: impl StateStore + 'static) -> Arc<Self> {
        Arc::new(Self {
            inner: Box::new(inner),
            hook: Mutex::new(None),
        })
    }

    fn set(&self, hook: impl FnMut(&Call<'_>) -> Result<(), StoreError> + Send + 'static) {
        *self.hook.lock().unwrap_or_else(PoisonError::into_inner) = Some(Box::new(hook));
    }

    fn clear(&self) {
        *self.hook.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn call(&self, op: Op, phase: Phase, key: &str) -> Result<(), StoreError> {
        let mut hook = self.hook.lock().unwrap_or_else(PoisonError::into_inner);
        hook.as_mut().map_or(Ok(()), |hook| {
            hook(&Call {
                op,
                phase,
                key,
                inner: self.inner.as_ref(),
            })
        })
    }
}

impl StateStore for Hooked {
    fn location(&self) -> Option<&Path> {
        self.inner.location()
    }
    fn staging_directory(&self) -> Option<PathBuf> {
        self.inner.staging_directory()
    }
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.call(Op::Read, Phase::Before, key)?;
        let result = self.inner.read(key)?;
        self.call(Op::Read, Phase::After, key)?;
        Ok(result)
    }
    fn write(&self, key: &str, value: &[u8], expected: Option<&[u8]>) -> Result<bool, StoreError> {
        self.call(Op::Write, Phase::Before, key)?;
        let result = self.inner.write(key, value, expected)?;
        self.call(Op::Write, Phase::After, key)?;
        Ok(result)
    }
    fn delete(&self, key: &str, expected: &[u8]) -> Result<bool, StoreError> {
        self.call(Op::Delete, Phase::Before, key)?;
        let result = self.inner.delete(key, expected)?;
        self.call(Op::Delete, Phase::After, key)?;
        Ok(result)
    }
    fn keys(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        self.inner.keys(prefix)
    }
    fn lock(&self, name: &str, timeout: Duration) -> Result<StoreLock<'_>, StoreError> {
        self.inner.lock(name, timeout)
    }
}

fn hooked_target(store: &Arc<Hooked>) -> InstallationTarget {
    target_of(Arc::clone(store) as Arc<dyn StateStore>)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Crash {
    AfterPreparation,
    AfterStaging,
    BeforeCommit,
    AfterCommit,
    BeforeJournalDelete,
    BeforePreparationDelete,
}

impl Crash {
    fn matches(self, call: &Call<'_>) -> bool {
        let (op, phase, suffix) = match self {
            Self::AfterPreparation => (Op::Write, Phase::After, "/preparation"),
            Self::AfterStaging => (Op::Write, Phase::Before, "/journal"),
            Self::BeforeCommit => (Op::Write, Phase::Before, "/receipt"),
            Self::AfterCommit => (Op::Write, Phase::After, "/receipt"),
            Self::BeforeJournalDelete => (Op::Delete, Phase::Before, "/journal"),
            Self::BeforePreparationDelete => (Op::Delete, Phase::Before, "/preparation"),
        };
        call.op == op && call.phase == phase && call.key.ends_with(suffix)
    }
}

fn arm_crash(store: &Hooked, crash: Crash) -> Arc<AtomicBool> {
    let fired = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&fired);
    store.set(move |call| {
        if crash.matches(call) && !seen.swap(true, Ordering::SeqCst) {
            resume_unwind(Box::new("interrupted"));
        }
        Ok(())
    });
    fired
}

fn crashed(store: &Hooked, crash: Crash, operation: impl FnOnce()) {
    let fired = arm_crash(store, crash);
    let outcome = catch_unwind(AssertUnwindSafe(operation));
    store.clear();
    assert!(
        outcome.is_err(),
        "{crash:?} did not interrupt the operation"
    );
    assert!(fired.load(Ordering::SeqCst), "{crash:?} never fired");
}

fn tree_state(directory: &Path) -> Vec<(String, u32, Option<Vec<u8>>)> {
    fn visit(root: &Path, current: &Path, items: &mut Vec<(String, u32, Option<Vec<u8>>)>) {
        for entry in fs::read_dir(current).expect("listing") {
            let path = entry.expect("entry").path();
            let metadata = fs::symlink_metadata(&path).expect("metadata");
            let relative = path
                .strip_prefix(root)
                .expect("relative")
                .to_string_lossy()
                .into_owned();
            let data = metadata.is_file().then(|| fs::read(&path).expect("bytes"));
            items.push((relative, metadata.permissions().mode(), data));
            if metadata.is_dir() {
                visit(root, &path, items);
            }
        }
    }
    let mut items = Vec::new();
    visit(directory, directory, &mut items);
    items.sort();
    items
}

#[test]
fn nothing_persistent_is_left_beside_the_destination() {
    let root = temp_root("resources-nothing-beside");
    let selected = target(&root);
    let destination = root.join("AGENTS.md");
    fs::write(&destination, "foreign\n").expect("foreign");
    let steps: [&dyn Fn() -> InstallationResult; 3] = [
        &|| run(&bundle("1"), &rendered(&destination, "one"), &selected),
        &|| run(&bundle("2"), &rendered(&destination, "two"), &selected),
        &|| remove("team", &selected, false, Duration::ZERO).expect("remove"),
    ];
    for step in steps {
        assert_eq!(step().status(), OperationStatus::Applied);
        assert_eq!(names(&root), ["AGENTS.md", "state"]);
        assert_eq!(
            transaction_directories(&destination, Some(selected.store().as_ref())),
            Vec::<PathBuf>::new()
        );
    }
    assert_eq!(
        fs::read_to_string(&destination).expect("foreign"),
        "foreign\n"
    );
}

#[test]
fn an_unwritable_store_fails_without_panicking() {
    let root = temp_root("resources-unwritable");
    let selected = target(&root);
    let destination = root.join("AGENTS.md");
    fs::create_dir(root.join("state")).expect("state");
    fs::set_permissions(root.join("state"), fs::Permissions::from_mode(0o500)).expect("chmod");
    let result = catch_unwind(AssertUnwindSafe(|| {
        run(
            &bundle("1"),
            &rendered(&destination, "generated"),
            &selected,
        )
    }));
    fs::set_permissions(root.join("state"), fs::Permissions::from_mode(0o700)).expect("chmod");
    let result = result.expect("no panic");
    assert_eq!(result.status(), OperationStatus::Failed);
    assert_eq!(
        result.error().map(TargetError::code),
        Some(ErrorCode::IoError)
    );
    assert!(!destination.exists());
}

#[test]
fn a_store_others_can_write_fails_without_panicking() {
    let root = temp_root("resources-public-store");
    let location = root.join("state");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&location)
        .expect("state");
    fs::set_permissions(&location, fs::Permissions::from_mode(0o777)).expect("chmod");
    let result = run(
        &bundle("1"),
        &rendered(&root.join("AGENTS.md"), "generated"),
        &target_of(directory_store(&location)),
    );
    assert_eq!(result.status(), OperationStatus::Failed);
    assert_eq!(
        result.error().map(TargetError::code),
        Some(ErrorCode::IoError)
    );
    assert_eq!(
        result.error().and_then(TargetError::path),
        Some(location.as_path())
    );
    assert_eq!(names(&root), ["state"]);
    assert_eq!(names(&location), Vec::<String>::new());
}

#[test]
fn a_store_failure_at_each_protocol_point_keeps_the_destination_consistent() {
    for fragment in [
        "index/",
        "/authority",
        "/preparation",
        "/journal",
        "/receipt",
    ] {
        let root = temp_root("resources-store-failure");
        let store = Hooked::new(DirectoryStore::new(root.join("state")).expect("store"));
        let selected = hooked_target(&store);
        let destination = root.join("AGENTS.md");
        let second = if fragment == "/authority" {
            root.join("CLAUDE.md")
        } else {
            destination.clone()
        };
        applied(&bundle("1"), &rendered(&destination, "one"), &selected);
        let before = fs::read(&destination).expect("before");
        store.set(move |call| {
            if call.op == Op::Write && call.phase == Phase::Before && call.key.contains(fragment) {
                return Err(io_failure("injected store failure"));
            }
            Ok(())
        });
        let failed = run(&bundle("2"), &rendered(&second, "two"), &selected);
        store.clear();
        assert!(
            matches!(
                failed.status(),
                OperationStatus::Failed | OperationStatus::Incomplete
            ),
            "{fragment}: {:?}",
            failed.status()
        );
        assert_eq!(
            fs::read(&destination).expect("retained"),
            before,
            "{fragment}"
        );
        assert_eq!(
            transaction_directories(&destination, Some(store.as_ref())),
            Vec::<PathBuf>::new(),
            "{fragment}"
        );
        if second != destination {
            assert!(!second.exists(), "{fragment}");
        }
        applied(&bundle("2"), &rendered(&second, "two"), &selected);
    }
}

#[test]
fn an_interrupted_transaction_is_reported_and_recovered() {
    let root = temp_root("resources-interrupted");
    let store = Hooked::new(DirectoryStore::new(root.join("state")).expect("store"));
    let selected = hooked_target(&store);
    let destination = root.join("AGENTS.md");
    crashed(&store, Crash::BeforeCommit, || {
        run(
            &bundle("1"),
            &rendered(&destination, "generated"),
            &selected,
        );
    });
    let pending = journal_directory(store.as_ref(), &destination);
    let observation = inspect_installation("team", &selected).expect("inspect");
    assert_eq!(
        observation
            .resources()
            .iter()
            .map(|resource| resource.recovery_paths().to_vec())
            .collect::<Vec<_>>(),
        [vec![pending]]
    );
    applied(
        &bundle("1"),
        &rendered(&destination, "generated"),
        &selected,
    );
    assert_eq!(names(&root), ["AGENTS.md", "state"]);
}

#[test]
fn cleanup_interrupted_after_removing_transaction_directories_is_recovered() {
    let root = temp_root("resources-interrupted-cleanup");
    let store = Hooked::new(DirectoryStore::new(root.join("state")).expect("store"));
    let selected = hooked_target(&store);
    let destination = root.join("AGENTS.md");
    applied(&bundle("1"), &rendered(&destination, "one"), &selected);
    crashed(&store, Crash::BeforeJournalDelete, || {
        run(&bundle("2"), &rendered(&destination, "two"), &selected);
    });
    let pending = journal_directory(store.as_ref(), &destination);
    assert!(!pending.exists());
    let observation = inspect_installation("team", &selected).expect("inspect");
    assert_eq!(observation.resources()[0].recovery_paths(), [pending]);
    assert_eq!(recover(&selected).status(), OperationStatus::Applied);
    assert_eq!(
        store
            .read(&resource_key(&destination, "journal"))
            .expect("journal read"),
        None
    );
    assert_eq!(
        transaction_directories(&destination, Some(store.as_ref())),
        Vec::<PathBuf>::new()
    );
    assert!(
        fs::read_to_string(&destination)
            .expect("published")
            .contains("two")
    );
    applied(&bundle("2"), &rendered(&destination, "two"), &selected);
}

#[test]
fn a_transaction_directory_without_a_journal_is_left_untouched() {
    let layouts: [&[&str]; 4] = [
        &[],
        &["staging/"],
        &["staging/", "backup/"],
        &["staging/", "backup/", "staging/resource"],
    ];
    for layout in layouts {
        let root = temp_root("resources-unrecorded");
        let selected = target(&root);
        let destination = root.join("AGENTS.md");
        applied(&bundle("1"), &rendered(&destination, "one"), &selected);
        let unrecorded =
            flyrail::authority::transaction_path(&destination, &"f".repeat(32)).expect("path");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&unrecorded)
            .expect("unrecorded");
        for name in layout {
            if let Some(directory) = name.strip_suffix('/') {
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(unrecorded.join(directory))
                    .expect("directory");
            } else {
                fs::write(unrecorded.join(name), b"retained").expect("file");
            }
        }
        let observation = inspect_installation("team", &selected).expect("inspect");
        assert_eq!(
            observation.resources()[0].recovery_paths(),
            [] as [PathBuf; 0]
        );
        assert_eq!(recover(&selected).status(), OperationStatus::Unchanged);
        applied(&bundle("2"), &rendered(&destination, "two"), &selected);
        assert!(
            fs::read_to_string(&destination)
                .expect("published")
                .contains("two")
        );
        assert_eq!(
            transaction_directories(&destination, None),
            std::slice::from_ref(&unrecorded)
        );
        let mut expected = layout
            .iter()
            .map(|name| name.trim_end_matches('/').to_owned())
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(
            tree_state(&unrecorded)
                .into_iter()
                .map(|(path, _, _)| path)
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn a_crash_before_the_journal_is_cleaned_up_by_the_next_run() {
    for crash in [Crash::AfterPreparation, Crash::AfterStaging] {
        for directory_backed in [true, false] {
            let root = temp_root("resources-crash-before-journal");
            let store = if directory_backed {
                Hooked::new(DirectoryStore::new(root.join("state")).expect("store"))
            } else {
                Hooked::new(MemoryStore::new())
            };
            let selected = hooked_target(&store);
            let destination = root.join("AGENTS.md");
            fs::write(&destination, b"# Foreign\n").expect("foreign");
            applied(&bundle("1"), &rendered(&destination, "one"), &selected);
            crashed(&store, crash, || {
                run(&bundle("2"), &rendered(&destination, "two"), &selected);
            });
            let recorded = preparation(store.as_ref(), &destination).expect("preparation");
            assert_eq!(
                store
                    .read(&resource_key(&destination, "journal"))
                    .expect("journal read"),
                None
            );
            assert_eq!(recorded.directory().exists(), crash == Crash::AfterStaging);
            let observation = inspect_installation("team", &selected).expect("inspect");
            assert_eq!(
                observation.resources()[0].recovery_paths(),
                [recorded.directory().to_path_buf()],
                "{crash:?}"
            );
            applied(&bundle("2"), &rendered(&destination, "two"), &selected);
            assert_eq!(preparation(store.as_ref(), &destination), None);
            assert_eq!(
                names(&root),
                if directory_backed {
                    vec!["AGENTS.md", "state"]
                } else {
                    vec!["AGENTS.md"]
                }
            );
            assert_eq!(
                transaction_directories(&destination, Some(store.as_ref())),
                Vec::<PathBuf>::new()
            );
        }
    }
}

fn victim(root: &Path) -> PathBuf {
    let victim = root.join("victim");
    for folder in [
        victim.clone(),
        victim.join("staging"),
        victim.join("backup"),
    ] {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(folder)
            .expect("victim");
    }
    fs::write(victim.join("staging/resource"), b"victim").expect("victim data");
    victim
}

fn damage(pending: &Path, root: &Path, kind: &str) -> PathBuf {
    match kind {
        "symlink" => {
            let victim = root.join("victim");
            fs::rename(pending, &victim).expect("move aside");
            symlink(&victim, pending).expect("symlink");
            victim.join("staging/resource")
        }
        "staging-link" => {
            let victim = victim(root);
            fs::remove_file(pending.join("staging/resource")).expect("staged");
            fs::remove_dir(pending.join("staging")).expect("staging");
            symlink(victim.join("staging"), pending.join("staging")).expect("symlink");
            victim.join("staging/resource")
        }
        "nonprivate" => {
            fs::set_permissions(pending, fs::Permissions::from_mode(0o755)).expect("chmod");
            pending.join("staging/resource")
        }
        _ => {
            fs::write(pending.join(kind), b"retained").expect("evidence");
            pending.join(kind)
        }
    }
}

#[test]
fn unrecognized_data_beside_a_preparation_is_retained() {
    for kind in [
        "unknown",
        "backup/resource",
        "symlink",
        "staging-link",
        "nonprivate",
    ] {
        let root = temp_root("resources-unrecognized");
        let store = Hooked::new(DirectoryStore::new(root.join("state")).expect("store"));
        let selected = hooked_target(&store);
        let destination = root.join("AGENTS.md");
        applied(&bundle("1"), &rendered(&destination, "one"), &selected);
        let before = fs::read(&destination).expect("before");
        crashed(&store, Crash::AfterStaging, || {
            run(&bundle("2"), &rendered(&destination, "two"), &selected);
        });
        let key = resource_key(&destination, "preparation");
        let recorded = store.read(&key).expect("preparation read");
        let pending = journal_directory(store.as_ref(), &destination);
        let evidence = damage(&pending, &root, kind);
        let retained = fs::read(&evidence).expect("evidence");
        let result = recover(&selected);
        assert_eq!(result.status(), OperationStatus::Incomplete, "{kind}");
        assert_eq!(
            result.error().map(TargetError::code),
            Some(ErrorCode::RecoveryNeeded),
            "{kind}"
        );
        assert_eq!(
            result.error().and_then(TargetError::path),
            Some(pending.as_path()),
            "{kind}"
        );
        assert_eq!(
            result
                .resources()
                .iter()
                .map(|item| item.recovery_paths().to_vec())
                .collect::<Vec<_>>(),
            [vec![pending.clone()]],
            "{kind}"
        );
        assert_eq!(
            store.read(&key).expect("preparation read"),
            recorded,
            "{kind}"
        );
        assert_eq!(fs::read(&evidence).expect("evidence"), retained, "{kind}");
        assert_eq!(
            fs::read(&destination).expect("destination"),
            before,
            "{kind}"
        );
        let _ = fs::set_permissions(&pending, fs::Permissions::from_mode(0o700));
    }
}

#[test]
fn a_journal_without_its_preparation_is_retained_as_invalid_state() {
    let root = temp_root("resources-lone-journal");
    let store = Hooked::new(DirectoryStore::new(root.join("state")).expect("store"));
    let selected = hooked_target(&store);
    let destination = root.join("AGENTS.md");
    applied(&bundle("1"), &rendered(&destination, "one"), &selected);
    crashed(&store, Crash::BeforeCommit, || {
        run(&bundle("2"), &rendered(&destination, "two"), &selected);
    });
    let pending = journal_directory(store.as_ref(), &destination);
    let preparation_key = resource_key(&destination, "preparation");
    let journal_key = resource_key(&destination, "journal");
    let recorded = store
        .read(&preparation_key)
        .expect("read")
        .expect("preparation");
    assert!(store.delete(&preparation_key, &recorded).expect("delete"));
    let journal = store.read(&journal_key).expect("journal read");
    let published = fs::read(&destination).expect("published");
    let layout = tree_state(&pending);
    assert!(String::from_utf8_lossy(&published).contains("two"));
    let observation = inspect_installation("team", &selected).expect("inspect");
    let recovered = recover(&selected);
    let synced = run(&bundle("2"), &rendered(&destination, "two"), &selected);
    assert_eq!(
        observation
            .resources()
            .iter()
            .filter_map(|item| item.error().map(TargetError::code))
            .collect::<Vec<_>>(),
        [ErrorCode::InvalidState]
    );
    for result in [recovered, synced] {
        assert_eq!(result.status(), OperationStatus::Incomplete);
        assert_eq!(
            result.error().map(TargetError::code),
            Some(ErrorCode::InvalidState)
        );
    }
    assert_eq!(store.read(&journal_key).expect("journal read"), journal);
    assert_eq!(tree_state(&pending), layout);
    assert_eq!(fs::read(&destination).expect("destination"), published);
}

#[test]
fn cleanup_removes_the_preparation_after_the_journal() {
    let root = temp_root("resources-cleanup-records");
    let selected = target(&root);
    applied(
        &bundle("1"),
        &rendered(&root.join("AGENTS.md"), "generated"),
        &selected,
    );
    assert_eq!(
        selected
            .store()
            .keys("resource/")
            .expect("keys")
            .into_iter()
            .filter(|key| key.ends_with("/journal") || key.ends_with("/preparation"))
            .collect::<Vec<_>>(),
        Vec::<String>::new()
    );
}

#[test]
fn a_cleanup_interrupted_before_deleting_the_preparation_is_recovered() {
    let root = temp_root("resources-cleanup-preparation");
    let store = Hooked::new(DirectoryStore::new(root.join("state")).expect("store"));
    let selected = hooked_target(&store);
    let destination = root.join("AGENTS.md");
    applied(&bundle("1"), &rendered(&destination, "one"), &selected);
    crashed(&store, Crash::BeforePreparationDelete, || {
        run(&bundle("2"), &rendered(&destination, "two"), &selected);
    });
    assert_eq!(
        store
            .read(&resource_key(&destination, "journal"))
            .expect("journal read"),
        None
    );
    assert!(preparation(store.as_ref(), &destination).is_some());
    assert_eq!(recover(&selected).status(), OperationStatus::Applied);
    assert_eq!(preparation(store.as_ref(), &destination), None);
    assert!(
        fs::read_to_string(&destination)
            .expect("published")
            .contains("two")
    );
    assert_eq!(names(&root), ["AGENTS.md", "state"]);
    assert_eq!(
        transaction_directories(&destination, Some(store.as_ref())),
        Vec::<PathBuf>::new()
    );
}

#[test]
fn a_lost_store_makes_installed_content_foreign() {
    let root = temp_root("resources-lost-store");
    let destination = root.join("AGENTS.md");
    applied(
        &bundle("1"),
        &rendered(&destination, "generated"),
        &target(&root),
    );
    let fresh = target(&root.join("replacement"));
    let conflicted = run(&bundle("1"), &rendered(&destination, "generated"), &fresh);
    assert_eq!(conflicted.status(), OperationStatus::Failed);
    let codes = conflicted
        .error()
        .into_iter()
        .chain(
            conflicted
                .resources()
                .iter()
                .filter_map(|item| item.error()),
        )
        .map(TargetError::code)
        .collect::<Vec<_>>();
    assert!(codes.contains(&ErrorCode::Conflict), "{codes:?}");
    let adopted = sync(
        &bundle("1"),
        &rendered(&destination, "generated"),
        &fresh,
        Acquisition::Adopt,
        false,
        Duration::ZERO,
    )
    .expect("adopt");
    assert_eq!(adopted.status(), OperationStatus::Applied);
}

#[test]
fn another_store_leaves_a_live_transaction_directory_untouched() {
    let root = temp_root("resources-another-store");
    let destination = root.join("AGENTS.md");
    fs::write(&destination, "# project\n").expect("project");
    let first = Hooked::new(MemoryStore::new());
    let second = target(&root.join("state-b"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let pause = destination.clone();
    let paused = second.clone();
    first.set(move |call| {
        if call.op == Op::Write && call.phase == Phase::After && call.key.ends_with("/journal") {
            let record = call
                .inner
                .read(&resource_key(&pause, "preparation"))
                .expect("preparation")
                .expect("preparation");
            let directory = decode::<Preparation>(&record)
                .expect("preparation")
                .directory()
                .to_path_buf();
            let before = tree_state(&directory);
            let result = run(
                &instruction_bundle("other", "1", "notes"),
                &section("notes", &pause, "from B"),
                &paused,
            );
            assert_eq!(tree_state(&directory), before);
            recorded
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((directory, before, result));
        }
        Ok(())
    });
    let result = run(
        &bundle("1"),
        &rendered(&destination, "from A"),
        &hooked_target(&first),
    );
    first.clear();
    let seen = seen.lock().unwrap_or_else(PoisonError::into_inner);
    let [(directory, contents, from_second)] = seen.as_slice() else {
        panic!("the second store never ran");
    };
    assert_eq!(directory.parent(), Some(root.as_path()));
    assert_ne!(contents.len(), 0);
    assert_eq!(from_second.status(), OperationStatus::Applied);
    assert_eq!(
        from_second
            .resources()
            .iter()
            .map(|item| item.recovery_paths().to_vec())
            .collect::<Vec<_>>(),
        [Vec::<PathBuf>::new()]
    );
    assert!(
        fs::read_to_string(&destination)
            .expect("destination")
            .contains("from B")
    );
    assert_eq!(
        result.error().map(TargetError::code),
        Some(ErrorCode::ConcurrentChange)
    );
    assert_eq!(
        recover_installation("other", &second, Duration::ZERO)
            .expect("recover")
            .status(),
        OperationStatus::Unchanged
    );
}

fn tree_bundle(path: &Path, text: &str) -> RenderedBundle {
    RenderedBundle::new(
        [RenderedArtifact::new(
            "guide",
            Family::Instructions,
            path.to_path_buf(),
            Content::Tree(
                TreeContent::new([
                    BundleEntry::file("a.txt", text.as_bytes().to_vec()).expect("entry")
                ])
                .expect("tree"),
            ),
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
fn a_store_read_failure_after_staging_discards_the_staged_revision() {
    for kind in ["document", "tree"] {
        let root = temp_root("resources-read-failure");
        let store = Hooked::new(DirectoryStore::new(root.join("state")).expect("store"));
        let selected = hooked_target(&store);
        let destination = root.join(if kind == "document" {
            "AGENTS.md"
        } else {
            "out"
        });
        let desired = |text: &str| {
            if kind == "document" {
                rendered(&destination, text)
            } else {
                tree_bundle(&destination, text)
            }
        };
        applied(&bundle("1"), &desired("one"), &selected);
        let before = if kind == "document" {
            vec![(
                String::new(),
                0,
                Some(fs::read(&destination).expect("before")),
            )]
        } else {
            tree_state(&destination)
        };
        let mut armed = false;
        store.set(move |call| {
            if call.op == Op::Write
                && call.phase == Phase::After
                && call.key.ends_with("/preparation")
            {
                armed = true;
            }
            if armed && call.op == Op::Read && call.key.ends_with("/authority") {
                return Err(io_failure("injected store read failure"));
            }
            Ok(())
        });
        let failed = run(&bundle("2"), &desired("two"), &selected);
        store.clear();
        assert_eq!(failed.status(), OperationStatus::Failed, "{kind}");
        assert_eq!(
            failed.error().map(TargetError::code),
            Some(ErrorCode::IoError),
            "{kind}"
        );
        let after = if kind == "document" {
            vec![(
                String::new(),
                0,
                Some(fs::read(&destination).expect("after")),
            )]
        } else {
            tree_state(&destination)
        };
        assert_eq!(after, before, "{kind}");
        assert_eq!(
            transaction_directories(&destination, Some(store.as_ref())),
            Vec::<PathBuf>::new(),
            "{kind}"
        );
        assert_eq!(preparation(store.as_ref(), &destination), None, "{kind}");
        applied(&bundle("2"), &desired("two"), &selected);
        let published = if kind == "document" {
            fs::read_to_string(&destination)
        } else {
            fs::read_to_string(destination.join("a.txt"))
        };
        assert!(published.expect("published").contains("two"), "{kind}");
    }
}

fn record_listings(
    store: &Hooked,
    project: &Path,
    crash: Option<Crash>,
) -> Arc<Mutex<Vec<Vec<String>>>> {
    let listings = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&listings);
    let project = project.to_path_buf();
    let mut fired = false;
    store.set(move |call| {
        if matches!(call.op, Op::Write | Op::Delete) {
            recorded
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(names(&project));
        }
        if let Some(crash) = crash
            && crash.matches(call)
            && !fired
        {
            fired = true;
            resume_unwind(Box::new("interrupted"));
        }
        Ok(())
    });
    listings
}

#[test]
fn a_same_filesystem_store_never_creates_anything_beside_the_destination() {
    for crash in [
        Crash::AfterPreparation,
        Crash::AfterStaging,
        Crash::AfterCommit,
    ] {
        let root = temp_root("resources-never-beside");
        let project = root.join("project");
        fs::create_dir(&project).expect("project");
        let destination = project.join("AGENTS.md");
        fs::write(&destination, b"# Foreign\n").expect("foreign");
        let store = Hooked::new(DirectoryStore::new(root.join("state")).expect("store"));
        let selected = hooked_target(&store);
        let listings = record_listings(&store, &project, None);
        applied(&bundle("1"), &rendered(&destination, "one"), &selected);
        let crash_listings = record_listings(&store, &project, Some(crash));
        let interrupted = catch_unwind(AssertUnwindSafe(|| {
            run(&bundle("2"), &rendered(&destination, "two"), &selected);
        }));
        assert!(interrupted.is_err(), "{crash:?}");
        let after_crash = names(&project);
        let recovery_listings = record_listings(&store, &project, None);
        applied(&bundle("2"), &rendered(&destination, "two"), &selected);
        store.clear();
        let mut all = Vec::new();
        for recorded in [listings, crash_listings, recovery_listings] {
            all.extend(
                recorded
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone(),
            );
        }
        assert_ne!(all.len(), 0);
        assert!(
            all.iter()
                .all(|listing| listing.iter().all(|name| name == "AGENTS.md")),
            "{crash:?}: {all:?}"
        );
        assert!(
            after_crash.iter().all(|name| name == "AGENTS.md"),
            "{crash:?}"
        );
        assert_eq!(names(&project), ["AGENTS.md"]);
        assert_eq!(
            transaction_directories(&destination, Some(store.as_ref())),
            Vec::<PathBuf>::new()
        );
        assert!(root.join("state/staging").is_dir());
        assert!(
            fs::read_to_string(&destination)
                .expect("published")
                .contains("two")
        );
    }
}

fn capture_transaction_directories(store: &Hooked, destination: &Path) -> Arc<Mutex<Vec<PathBuf>>> {
    let directories = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&directories);
    let destination = destination.to_path_buf();
    store.set(move |call| {
        if call.op == Op::Write && call.phase == Phase::Before && call.key.ends_with("/journal") {
            let record = call
                .inner
                .read(&resource_key(&destination, "preparation"))?
                .expect("preparation");
            let directory = decode::<Preparation>(&record)
                .expect("preparation")
                .directory()
                .to_path_buf();
            assert!(directory.is_dir());
            recorded
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(directory);
        }
        Ok(())
    });
    directories
}

#[test]
fn a_store_without_a_staging_directory_falls_back_beside_the_destination() {
    let root = temp_root("resources-memory-fallback");
    let destination = root.join("AGENTS.md");
    let store = Hooked::new(MemoryStore::new());
    let directories = capture_transaction_directories(&store, &destination);
    applied(
        &bundle("1"),
        &rendered(&destination, "generated"),
        &hooked_target(&store),
    );
    store.clear();
    let directories = directories.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(
        directories
            .iter()
            .map(|directory| directory.parent().map(Path::to_path_buf))
            .collect::<Vec<_>>(),
        [Some(root.clone())]
    );
    let name = directories[0]
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    assert!(
        name.starts_with(".flyrail-") && name.ends_with(".txn"),
        "{name}"
    );
    assert_eq!(names(&root), ["AGENTS.md"]);
}

#[test]
fn a_same_filesystem_store_stages_in_its_staging_directory() {
    let root = temp_root("resources-store-staging");
    let destination = root.join("AGENTS.md");
    let store = Hooked::new(DirectoryStore::new(root.join("state")).expect("store"));
    let directories = capture_transaction_directories(&store, &destination);
    applied(
        &bundle("1"),
        &rendered(&destination, "generated"),
        &hooked_target(&store),
    );
    store.clear();
    let directories = directories.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(
        directories
            .iter()
            .map(|directory| directory.parent().map(Path::to_path_buf))
            .collect::<Vec<_>>(),
        [Some(root.join("state/staging"))]
    );
    assert!(
        directories[0]
            .file_name()
            .expect("name")
            .to_string_lossy()
            .starts_with(&format!("{}-", destination_key(&destination)))
    );
    assert_eq!(names(&root.join("state/staging")), Vec::<String>::new());
}

#[test]
fn a_preparation_naming_a_foreign_directory_is_refused() {
    for named in ["elsewhere", "another-transaction", "another-destination"] {
        let root = temp_root("resources-foreign-directory");
        let store = Hooked::new(DirectoryStore::new(root.join("state")).expect("store"));
        let selected = hooked_target(&store);
        let destination = root.join("AGENTS.md");
        applied(&bundle("1"), &rendered(&destination, "one"), &selected);
        let before = fs::read(&destination).expect("before");
        crashed(&store, Crash::AfterStaging, || {
            run(&bundle("2"), &rendered(&destination, "two"), &selected);
        });
        let key = resource_key(&destination, "preparation");
        let original = store.read(&key).expect("read").expect("preparation");
        let recorded = decode::<Preparation>(&original).expect("preparation");
        let decoy = match named {
            "elsewhere" => root.join("decoy"),
            "another-transaction" => recorded.directory().with_file_name(format!(
                "{}-{}",
                destination_key(&destination),
                "f".repeat(32)
            )),
            _ => flyrail::authority::transaction_path(
                &root.join("CLAUDE.md"),
                recorded.transaction_id(),
            )
            .expect("path"),
        };
        fs::rename(victim(&root), &decoy).expect("decoy");
        let corrupted = encode(
            &Preparation::new(
                recorded.resource().clone(),
                recorded.transaction_id().to_owned(),
                decoy.clone(),
            )
            .expect("corrupted"),
        );
        assert!(
            store
                .write(&key, &corrupted, Some(&original))
                .expect("write")
        );
        for result in [
            recover(&selected),
            run(&bundle("2"), &rendered(&destination, "two"), &selected),
        ] {
            assert_eq!(result.status(), OperationStatus::Incomplete, "{named}");
            assert_eq!(
                result.error().map(TargetError::code),
                Some(ErrorCode::RecoveryNeeded),
                "{named}"
            );
            assert!(
                result
                    .resources()
                    .iter()
                    .all(|item| item.recovery_paths().is_empty()),
                "{named}"
            );
        }
        assert_eq!(store.read(&key).expect("read"), Some(corrupted), "{named}");
        assert_eq!(
            fs::read(decoy.join("staging/resource")).expect("decoy"),
            b"victim",
            "{named}"
        );
        assert_eq!(
            fs::read(&destination).expect("destination"),
            before,
            "{named}"
        );
    }
}

fn crash_then_move_store(root: &Path, crash: Crash) -> (InstallationTarget, PathBuf, Preparation) {
    let store = Hooked::new(DirectoryStore::new(root.join("state")).expect("store"));
    let selected = hooked_target(&store);
    let destination = root.join("AGENTS.md");
    applied(&bundle("1"), &rendered(&destination, "one"), &selected);
    crashed(&store, crash, || {
        run(&bundle("2"), &rendered(&destination, "two"), &selected);
    });
    let recorded = preparation(store.as_ref(), &destination).expect("preparation");
    assert_eq!(
        store
            .read(&resource_key(&destination, "journal"))
            .expect("journal read"),
        None
    );
    fs::rename(root.join("state"), root.join("moved")).expect("move store");
    (
        target_of(directory_store(&root.join("moved"))),
        destination,
        recorded,
    )
}

#[test]
fn a_preparation_left_behind_by_a_moved_store_is_released() {
    for recover_first in [true, false] {
        let root = temp_root("resources-moved-store");
        let (moved, destination, recorded) = crash_then_move_store(&root, Crash::AfterPreparation);
        let store = moved.store().as_ref();
        assert_eq!(
            fs::symlink_metadata(recorded.directory())
                .map_err(|error| error.kind())
                .err(),
            Some(io::ErrorKind::NotFound)
        );
        if recover_first {
            assert_eq!(recover(&moved).status(), OperationStatus::Applied);
            assert!(preparation(store, &destination).is_none());
        }
        applied(&bundle("2"), &rendered(&destination, "two"), &moved);
        assert!(preparation(store, &destination).is_none());
        assert!(
            fs::read_to_string(&destination)
                .expect("destination")
                .contains("two")
        );
        assert_eq!(
            transaction_directories(&destination, Some(store)),
            Vec::<PathBuf>::new()
        );
    }
}

#[test]
fn a_moved_store_refuses_a_preparation_whose_old_directory_still_exists() {
    let root = temp_root("resources-moved-store-retained");
    let (moved, destination, recorded) = crash_then_move_store(&root, Crash::AfterStaging);
    let store = moved.store().as_ref();
    let before = fs::read(&destination).expect("before");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(recorded.directory().parent().expect("parent"))
        .expect("old staging directory");
    fs::rename(
        store
            .staging_directory()
            .expect("staging")
            .join(recorded.directory().file_name().expect("name")),
        recorded.directory(),
    )
    .expect("old transaction directory");
    let retained = tree_state(recorded.directory());
    let key = resource_key(&destination, "preparation");
    let original = store.read(&key).expect("read");
    for result in [
        recover(&moved),
        run(&bundle("2"), &rendered(&destination, "two"), &moved),
    ] {
        assert_eq!(result.status(), OperationStatus::Incomplete);
        assert_eq!(
            result.error().map(TargetError::code),
            Some(ErrorCode::RecoveryNeeded)
        );
        assert!(
            result
                .resources()
                .iter()
                .all(|item| item.recovery_paths().is_empty())
        );
    }
    assert_eq!(store.read(&key).expect("read"), original);
    assert_eq!(tree_state(recorded.directory()), retained);
    assert_eq!(fs::read(&destination).expect("destination"), before);
}

#[test]
fn a_staging_directory_another_writer_creates_before_locking_is_not_approved() {
    let root = temp_root("resources-foreign-staging");
    let selected = target(&root);
    let destination = root.join("AGENTS.md");
    let proposal = preview(
        &bundle("1"),
        &rendered(&destination, "generated"),
        &selected,
        Acquisition::Conflict,
        false,
    )
    .expect("preview");
    for directory in [root.join("state"), root.join("state/staging")] {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(directory)
            .expect("another writer");
    }
    let result = apply_preview(&proposal, Duration::ZERO);
    assert_eq!(result.status(), OperationStatus::Failed);
    assert_eq!(
        result.error().map(TargetError::code),
        Some(ErrorCode::ConcurrentChange)
    );
    assert!(!destination.exists());
    assert!(root.join("state/staging").is_dir());
}

#[test]
fn a_parent_recreated_during_the_lock_wait_is_not_approved() {
    let root = temp_root("resources-recreated-parent");
    let location = root.join("new/state");
    let store = directory_store(&location);
    let selected = target_of(Arc::clone(&store));
    let destination = root.join("AGENTS.md");
    let index = format!("index/team/{}", selected.context_digest());
    let held = store.lock(&index, Duration::ZERO).expect("index lock");
    let proposal = preview(
        &bundle("1"),
        &rendered(&destination, "generated"),
        &selected,
        Acquisition::Conflict,
        false,
    )
    .expect("preview");
    let staging = location.join("staging");
    assert!(!staging.exists());
    let result = std::thread::scope(|scope| {
        let applying = scope.spawn(|| apply_preview(&proposal, Duration::from_secs(20)));
        while !staging.exists() {
            assert!(
                !applying.is_finished(),
                "apply finished before creating staging"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            !applying.is_finished(),
            "apply finished without the index lock"
        );
        fs::rename(&staging, root.join("replaced")).expect("move staging aside");
        fs::DirBuilder::new()
            .mode(0o500)
            .create(&staging)
            .expect("recreate staging");
        drop(held);
        applying.join().expect("apply thread")
    });
    assert_eq!(
        result.status(),
        OperationStatus::Failed,
        "{:?}",
        result.error()
    );
    assert_eq!(
        result.error().map(TargetError::code),
        Some(ErrorCode::ConcurrentChange)
    );
    assert!(!destination.exists());
    assert_eq!(store.read(&index).expect("index read"), None);
    assert_eq!(
        store
            .read(&resource_key(&destination, "receipt"))
            .expect("receipt read"),
        None
    );
}

#[test]
fn an_unreadable_journal_reports_only_the_directory_its_preparation_names() {
    for (unreadable, reported) in [
        (&["/journal"][..], true),
        (&["/journal", "/preparation"][..], false),
    ] {
        let root = temp_root("resources-unreadable-journal");
        let store = Hooked::new(DirectoryStore::new(root.join("state")).expect("store"));
        let selected = hooked_target(&store);
        let destination = root.join("AGENTS.md");
        applied(&bundle("1"), &rendered(&destination, "one"), &selected);
        let directories = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&directories);
        let pending = destination.clone();
        let mut armed = false;
        store.set(move |call| {
            if armed
                && call.op == Op::Read
                && unreadable.iter().any(|suffix| call.key.ends_with(suffix))
            {
                return Err(io_failure("injected store read failure"));
            }
            if call.op == Op::Write && call.key.ends_with("/journal") {
                if call.phase == Phase::Before {
                    let record = call
                        .inner
                        .read(&resource_key(&pending, "preparation"))?
                        .expect("preparation");
                    recorded
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(
                            decode::<Preparation>(&record)
                                .expect("preparation")
                                .directory()
                                .to_path_buf(),
                        );
                } else {
                    armed = true;
                }
            }
            Ok(())
        });
        let result = run(&bundle("2"), &rendered(&destination, "two"), &selected);
        store.clear();
        let directory = directories.lock().unwrap_or_else(PoisonError::into_inner)[0].clone();
        assert_eq!(
            directory.parent(),
            Some(root.join("state/staging").as_path())
        );
        assert_eq!(
            result
                .resources()
                .iter()
                .map(|item| item.recovery_paths().to_vec())
                .collect::<Vec<_>>(),
            [if reported {
                vec![directory]
            } else {
                Vec::new()
            }]
        );
        assert_eq!(
            result.status(),
            if reported {
                OperationStatus::Incomplete
            } else {
                OperationStatus::Failed
            }
        );
        assert_eq!(recover(&selected).status(), OperationStatus::Applied);
        applied(&bundle("2"), &rendered(&destination, "two"), &selected);
        assert!(
            fs::read_to_string(&destination)
                .expect("published")
                .contains("two")
        );
    }
}

fn other_group() -> Option<u32> {
    let mut groups = vec![0; 64];
    let capacity = libc::c_int::try_from(groups.len()).ok()?;
    let count = unsafe { libc::getgroups(capacity, groups.as_mut_ptr()) };
    groups.truncate(usize::try_from(count).ok()?);
    let primary = unsafe { libc::getegid() };
    groups.into_iter().find(|group| *group != primary)
}

#[test]
fn a_store_owned_by_another_group_stages_files_with_the_intended_group() {
    let Some(group) = other_group() else {
        eprintln!("skipping: the user belongs to a single group");
        return;
    };
    let root = temp_root("resources-store-group");
    let location = root.join("state");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&location)
        .expect("state");
    std::os::unix::fs::chown(&location, None, Some(group)).expect("store group");
    let destination = root.join("owned");
    let rendered = RenderedBundle::new(
        [RenderedArtifact::new(
            "guide",
            Family::Instructions,
            destination.clone(),
            Content::File(flyrail::FileContent::new(
                b"whole\n",
                flyrail::FileMode::Readable,
            )),
            None,
            false,
            Vec::new(),
        )
        .expect("artifact")],
        [],
        [],
    )
    .expect("rendered");
    applied(
        &bundle("1"),
        &rendered,
        &target_of(directory_store(&location)),
    );
    assert_eq!(fs::read(&destination).expect("published"), b"whole\n");
}
