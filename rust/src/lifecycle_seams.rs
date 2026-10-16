#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::artifacts::{Artifact, Family, InstructionArtifact};
use crate::bundle::Bundle;
use crate::configuration::{InstallationResult, InstallationTarget, LifecyclePreview};
use crate::content::{Content, SectionContent, TreeContent};
use crate::editors::Acquisition;
use crate::lifecycle::{apply_preview, inspect_installation, preview, recover_installation, sync};
use crate::models::{BundleEntry, BundleIdentity, OperationStatus};
use crate::observations::ErrorCode;
use crate::rendered::{RenderedArtifact, RenderedBundle};
use crate::resource_io::observe;
use crate::resource_models::Preparation;
use crate::resource_plan::ResourcePlan;
use crate::resource_transaction::set_fault;
use crate::seam::{Hook, Seam, set_hook};
use crate::state::{choose_transaction_directory, read_record, resource_key};
use crate::stores::{DirectoryStore, MemoryStore};
use crate::test_support::checkout_temp_dir;
use std::cell::RefCell;
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

const FOREIGN: &[u8] = b"foreign edit\n";
const REPAIRED: &[u8] =
    b"# mine\n\n<!-- flyrail:guide:start -->\none\n<!-- flyrail:guide:end -->\n";
const DRIFTED: &[u8] =
    b"# mine\n\n<!-- flyrail:guide:start -->\nEDITED\n<!-- flyrail:guide:end -->\n";

fn at(when: impl Fn(&Seam<'_>) -> bool + 'static, action: impl FnOnce() + 'static) -> Hook {
    let mut action = Some(action);
    Box::new(move |seam| {
        if when(seam)
            && let Some(action) = action.take()
        {
            action();
        }
    })
}

fn both(mut first: Hook, mut second: Hook) -> Hook {
    Box::new(move |seam| {
        first(seam);
        second(seam);
    })
}

fn parent_is(path: &Path, name: &str) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|parent| parent == name)
}

fn boundary(name: &'static str) -> impl Fn(&Seam<'_>) -> bool {
    move |seam| matches!(seam, Seam::Boundary(selected) if *selected == name)
}

fn into(name: &'static str, finished: bool) -> impl Fn(&Seam<'_>) -> bool {
    move |seam| {
        matches!(seam, Seam::Rename { destination, done, .. }
            if *done == finished && parent_is(destination, name))
    }
}

fn out_of(name: &'static str, finished: bool) -> impl Fn(&Seam<'_>) -> bool {
    move |seam| {
        matches!(seam, Seam::Rename { source, done, .. }
            if *done == finished && parent_is(source, name))
    }
}

fn crash() {
    resume_unwind(Box::new("interrupted"));
}

fn interrupted(hook: Hook, operation: impl FnOnce()) {
    set_hook(Some(hook));
    let outcome = catch_unwind(AssertUnwindSafe(operation));
    set_hook(None);
    assert!(outcome.is_err(), "the operation was not interrupted");
}

fn bundle(version: &str) -> Bundle {
    Bundle::from_artifacts(
        BundleIdentity::new("team", version).expect("identity"),
        [Artifact::Instruction(
            InstructionArtifact::new("guide", "text").expect("instruction"),
        )],
        [],
        [],
    )
    .expect("bundle")
}

fn rendered(path: &Path, content: Content) -> RenderedBundle {
    RenderedBundle::new(
        [RenderedArtifact::new(
            "guide",
            Family::Instructions,
            path.to_path_buf(),
            content,
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

fn section(path: &Path, text: &str) -> RenderedBundle {
    rendered(
        path,
        Content::Section(SectionContent::new("guide", text, None).expect("section")),
    )
}

fn tree(path: &Path, text: &str) -> RenderedBundle {
    rendered(
        path,
        Content::Tree(
            TreeContent::new([
                BundleEntry::file("SKILL.md", text.as_bytes().to_vec()).expect("entry")
            ])
            .expect("tree"),
        ),
    )
}

fn directory_target(path: &Path) -> InstallationTarget {
    InstallationTarget::without_context(Arc::new(DirectoryStore::new(path).expect("store")))
        .expect("target")
}

fn memory_target() -> InstallationTarget {
    InstallationTarget::without_context(Arc::new(MemoryStore::new())).expect("target")
}

fn synced(
    version: &str,
    rendered: &RenderedBundle,
    target: &InstallationTarget,
    replace_modified: bool,
) -> InstallationResult {
    sync(
        &bundle(version),
        rendered,
        target,
        Acquisition::Conflict,
        replace_modified,
        Duration::ZERO,
    )
    .expect("sync")
}

fn recovered(target: &InstallationTarget) -> InstallationResult {
    recover_installation("team", target, Duration::ZERO).expect("recover")
}

fn record(target: &InstallationTarget, destination: &Path, name: &str) -> Option<Vec<u8>> {
    target
        .store()
        .read(&resource_key(destination, name))
        .expect("record read")
}

fn journal_directory(target: &InstallationTarget, destination: &Path) -> PathBuf {
    let preparation: Preparation = read_record(
        target.store().as_ref(),
        &resource_key(destination, "preparation"),
        "Preparation",
    )
    .expect("preparation read")
    .expect("preparation");
    preparation.directory().to_path_buf()
}

fn transaction_directories(destination: &Path, target: &InstallationTarget) -> Vec<PathBuf> {
    let listing = |directory: &Path| -> Vec<PathBuf> {
        fs::read_dir(directory).map_or_else(
            |_| Vec::new(),
            |entries| entries.map(|entry| entry.expect("entry").path()).collect(),
        )
    };
    let beside = listing(destination.parent().expect("parent"))
        .into_iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| crate::authority::management_name(&name.to_string_lossy()))
        });
    let staged = target
        .store()
        .staging_directory()
        .map(|staging| listing(&staging))
        .unwrap_or_default();
    let mut entries = beside.chain(staged).collect::<Vec<_>>();
    entries.sort();
    entries
}

fn later_than_boundary(path: &Path, directory: &Path) {
    let later = fs::symlink_metadata(directory)
        .and_then(|metadata| metadata.modified())
        .expect("boundary")
        .checked_add(Duration::from_secs(60))
        .expect("later time");
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for revised time")
        .set_times(fs::FileTimes::new().set_modified(later))
        .expect("revised modification time");
}

fn drifted_installation(root: &Path) -> (PathBuf, InstallationTarget) {
    let path = root.join("AGENTS.md");
    fs::write(&path, b"# mine\n").expect("foreign");
    let target = directory_target(&root.join("index"));
    assert_eq!(
        synced("1", &section(&path, "one"), &target, false).status(),
        OperationStatus::Applied
    );
    let edited = String::from_utf8(fs::read(&path).expect("installed"))
        .expect("text")
        .replace("one", "EDITED");
    fs::write(&path, edited).expect("drift");
    assert_eq!(fs::read(&path).expect("drifted"), DRIFTED);
    (path, target)
}

#[test]
fn an_interrupted_drift_repair_converges_on_the_next_sync() {
    for point in ["staged", "backup", "published", "committed"] {
        let root = checkout_temp_dir("seams-drift-repair");
        let (path, target) = drifted_installation(&root);
        let hook = match point {
            "staged" => at(boundary("staged"), crash),
            "backup" => at(boundary("backup"), crash),
            "published" => at(out_of("staging", true), crash),
            _ => at(boundary("receipt-committed"), crash),
        };
        interrupted(hook, || {
            synced("1", &section(&path, "one"), &target, true);
        });
        assert_eq!(path.exists(), point != "backup", "{point}");

        let result = synced("1", &section(&path, "one"), &target, true);

        assert_eq!(
            result.status(),
            OperationStatus::Applied,
            "{point}: {:?}",
            result.error()
        );
        assert_eq!(fs::read(&path).expect("repaired"), REPAIRED, "{point}");
        let observed = inspect_installation("team", &target).expect("inspect");
        assert_eq!(
            observed.resources()[0].recovery_paths(),
            [] as [PathBuf; 0],
            "{point}"
        );
        assert_eq!(
            synced("1", &section(&path, "one"), &target, false).status(),
            OperationStatus::Unchanged,
            "{point}"
        );
    }
}

#[test]
fn a_failed_repair_staging_does_not_block_the_next_repair() {
    let root = checkout_temp_dir("seams-failed-staging");
    let (path, target) = drifted_installation(&root);
    set_fault(Some("staged"));
    let failed = synced("1", &section(&path, "one"), &target, true);
    set_fault(None);
    assert_eq!(
        failed.status(),
        OperationStatus::Failed,
        "{:?}",
        failed.error()
    );
    assert_eq!(transaction_directories(&path, &target), [] as [PathBuf; 0]);

    let result = synced("1", &section(&path, "one"), &target, true);

    assert_eq!(
        result.status(),
        OperationStatus::Applied,
        "{:?}",
        result.error()
    );
    assert_eq!(fs::read(&path).expect("repaired"), REPAIRED);
}

fn reusing_previous(preview: &LifecyclePreview) -> LifecyclePreview {
    let store = preview.target().store().as_ref();
    let resources = preview
        .resources()
        .iter()
        .map(|plan| {
            let previous = plan.previous().expect("previous receipt").clone();
            assert_ne!(plan.after(), plan.before());
            let directory = choose_transaction_directory(
                store,
                plan.resource().destination(),
                previous.transaction_id(),
            )
            .expect("directory");
            ResourcePlan::new(
                plan.resource().clone(),
                plan.before().clone(),
                plan.after().clone(),
                Some(previous.clone()),
                previous,
                plan.ancestors().to_vec(),
                directory,
                plan.recovery_paths().to_vec(),
                plan.require_existing(),
                plan.error().cloned(),
            )
            .expect("plan")
        })
        .collect();
    LifecyclePreview::new(
        preview.bundle_id().to_owned(),
        preview.bundle().cloned(),
        preview.rendered().clone(),
        preview.target().clone(),
        preview.acquisition(),
        preview.replace_modified(),
        preview.index().cloned(),
        preview.generation().cloned(),
        resources,
        preview.error().cloned(),
    )
}

#[test]
fn a_journal_repeating_the_previous_receipt_is_refused_with_evidence_retained() {
    let root = checkout_temp_dir("seams-repeated-receipt");
    let (path, target) = drifted_installation(&root);
    let planned = preview(
        &bundle("1"),
        &section(&path, "one"),
        &target,
        Acquisition::Conflict,
        true,
    )
    .expect("preview");
    let hacked = reusing_previous(&planned);
    interrupted(at(boundary("backup"), crash), || {
        apply_preview(&hacked, Duration::ZERO);
    });
    let journal = record(&target, &path, "journal").expect("journal");
    let directory = journal_directory(&target, &path);
    let staged = directory.join("staging").join("resource");
    let backup = directory.join("backup").join("resource");
    let evidence = (observe(&staged).unwrap(), observe(&backup).unwrap());
    assert!(!path.exists());
    assert_eq!(fs::read(&backup).expect("backup"), DRIFTED);

    let result = recovered(&target);

    assert_eq!(result.status(), OperationStatus::Incomplete);
    assert_eq!(
        result.error().map(crate::TargetError::code),
        Some(ErrorCode::RecoveryNeeded)
    );
    assert_eq!(result.resources()[0].recovery_paths(), [directory]);
    assert!(!path.exists());
    assert_eq!(
        (observe(&staged).unwrap(), observe(&backup).unwrap()),
        evidence
    );
    assert_eq!(record(&target, &path, "journal"), Some(journal));
}

fn replacement_setup(root: &Path, location: &str) -> (PathBuf, InstallationTarget) {
    let project = root.join("project");
    fs::create_dir(&project).expect("project");
    let destination = project.join("AGENTS.md");
    fs::write(&destination, b"# Foreign\n").expect("foreign");
    let target = if location == "store" {
        directory_target(&root.join("state"))
    } else {
        memory_target()
    };
    assert_eq!(
        synced("1", &section(&destination, "one"), &target, false).status(),
        OperationStatus::Applied
    );
    (destination, target)
}

#[test]
fn recovery_refuses_a_destination_replaced_after_preparation() {
    for revision in ["published", "previous"] {
        for location in ["store", "beside"] {
            let root = checkout_temp_dir("seams-replaced");
            let (destination, target) = replacement_setup(&root, location);
            let original = fs::read(&destination).expect("original");
            let hook = if revision == "published" {
                at(out_of("staging", true), crash)
            } else {
                at(boundary("backup"), crash)
            };
            interrupted(hook, || {
                synced("2", &section(&destination, "two"), &target, false);
            });
            let directory = journal_directory(&target, &destination);
            if revision == "previous" {
                let backup = directory.join("backup").join("resource");
                fs::write(&destination, fs::read(&backup).expect("backup")).expect("copy");
                let mode = fs::metadata(&backup).expect("backup metadata").mode() & 0o777;
                fs::set_permissions(&destination, fs::Permissions::from_mode(mode)).expect("mode");
            }
            later_than_boundary(&destination, &directory);
            let replaced = fs::read(&destination).expect("replaced");

            let result = recovered(&target);

            assert_eq!(
                result.status(),
                OperationStatus::Incomplete,
                "{revision} {location}"
            );
            assert_eq!(
                result.error().map(crate::TargetError::code),
                Some(ErrorCode::RecoveryNeeded),
                "{revision} {location}"
            );
            assert_eq!(fs::read(&destination).expect("retained"), replaced);
            assert!(directory.is_dir(), "{revision} {location}");
            assert!(record(&target, &destination, "journal").is_some());
            if revision == "previous" {
                assert_eq!(replaced, original);
            }
        }
    }
}

#[test]
fn recovery_accepts_its_own_untouched_revisions() {
    for point in ["before-publication", "after-backup-move", "after-publish"] {
        for location in ["store", "beside"] {
            let root = checkout_temp_dir("seams-untouched");
            let (destination, target) = replacement_setup(&root, location);
            let original = fs::read(&destination).expect("original");
            let hook = match point {
                "before-publication" => at(boundary("prepare"), crash),
                "after-backup-move" => at(boundary("backup"), crash),
                _ => at(out_of("staging", true), crash),
            };
            interrupted(hook, || {
                synced("2", &section(&destination, "two"), &target, false);
            });
            assert!(record(&target, &destination, "journal").is_some());

            let result = recovered(&target);

            assert_eq!(
                result.status(),
                OperationStatus::Applied,
                "{point} {location}: {:?}",
                result.error()
            );
            assert_eq!(fs::read(&destination).expect("restored"), original);
            assert_eq!(
                synced("2", &section(&destination, "two"), &target, false).status(),
                OperationStatus::Applied
            );
            let text = String::from_utf8(fs::read(&destination).expect("updated")).expect("text");
            assert!(text.contains("two"), "{point} {location}");
            assert_eq!(
                transaction_directories(&destination, &target),
                [] as [PathBuf; 0]
            );
        }
    }
}

const PUBLICATION: [&str; 4] = [
    "before-publish-move",
    "after-publish-move",
    "before-receipt",
    "after-receipt",
];
const REPLACEMENT: [&str; 8] = [
    "before-backup-move",
    "during-backup-move",
    "before-publish-move",
    "after-publish-move",
    "before-receipt",
    "after-receipt",
    "rollback-unpublish",
    "rollback-restore",
];

fn publication_points(shape: &str) -> Vec<&'static str> {
    if shape.starts_with("replace") {
        REPLACEMENT.to_vec()
    } else {
        PUBLICATION
            .into_iter()
            .chain(std::iter::once("rollback-unpublish"))
            .collect()
    }
}

fn append_foreign(destination: &Path, tree: bool) {
    let path = if tree {
        if !destination.exists() {
            fs::create_dir(destination).expect("foreign tree");
        }
        destination.join("foreign")
    } else {
        destination.to_path_buf()
    };
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("foreign writer")
        .write_all(FOREIGN)
        .expect("foreign edit");
}

fn files(path: &Path) -> Vec<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::read_dir(path)
            .expect("listing")
            .flat_map(|entry| files(&entry.expect("entry").path()))
            .collect(),
        Ok(metadata) if metadata.is_file() => vec![path.to_path_buf()],
        _ => Vec::new(),
    }
}

fn children(path: &Path) -> Vec<PathBuf> {
    fs::read_dir(path).map_or_else(
        |_| Vec::new(),
        |entries| entries.map(|entry| entry.expect("entry").path()).collect(),
    )
}

fn retained(root: &Path, destination: &Path) -> bool {
    let beside = children(root).into_iter().filter(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(crate::authority::management_name)
    });
    let staged = children(&root.join("state/staging"));
    let evidence = beside
        .chain(staged)
        .flat_map(|directory| children(&directory))
        .map(|area| area.join("resource"));
    std::iter::once(destination.to_path_buf())
        .chain(evidence)
        .flat_map(|path| files(&path))
        .any(|file| {
            fs::read(file).is_ok_and(|data| data.windows(FOREIGN.len()).any(|part| part == FOREIGN))
        })
}

fn edit_hook(point: &'static str, edit: impl FnOnce() + 'static) -> Hook {
    match point {
        "before-backup-move" => at(boundary("backup-move"), edit),
        "during-backup-move" => at(into("backup", false), edit),
        "before-publish-move" => at(out_of("staging", false), edit),
        "after-publish-move" => at(out_of("staging", true), edit),
        "before-receipt" => at(boundary("publish-receipt"), edit),
        "after-receipt" => at(boundary("receipt-committed"), edit),
        "rollback-unpublish" => at(into("staging", false), edit),
        _ => at(out_of("backup", false), edit),
    }
}

fn foreign_edits_during_publication_are_never_lost(shape: &str) {
    for point in publication_points(shape) {
        let root = checkout_temp_dir("seams-publication");
        let target = directory_target(&root.join("state"));
        let is_tree = shape.ends_with("tree");
        let destination = root.join(if is_tree { "skill" } else { "AGENTS.md" });
        let render = |text: &str| {
            if is_tree {
                tree(&destination, text)
            } else {
                section(&destination, text)
            }
        };
        if shape.starts_with("replace") {
            let installed = synced("1", &render("one"), &target, false);
            assert_eq!(installed.status(), OperationStatus::Applied, "{shape}");
        }
        let edits = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&edits);
        let edited = destination.clone();
        set_hook(Some(edit_hook(point, move || {
            append_foreign(&edited, is_tree);
            recorded.borrow_mut().push(point);
        })));
        if point.starts_with("rollback") {
            set_fault(Some("publish-receipt"));
        }
        let result = synced("2", &render("two"), &target, false);
        set_fault(None);
        set_hook(None);
        assert_eq!(*edits.borrow(), [point], "{shape} {point}: {result:?}");
        assert!(retained(&root, &destination), "{shape} {point}: {result:?}");
    }
}

#[test]
fn a_foreign_edit_while_replacing_a_document_is_never_lost() {
    foreign_edits_during_publication_are_never_lost("replace-document");
}

#[test]
fn a_foreign_edit_while_replacing_a_tree_is_never_lost() {
    foreign_edits_during_publication_are_never_lost("replace-tree");
}

#[test]
fn a_foreign_edit_while_creating_a_document_is_never_lost() {
    foreign_edits_during_publication_are_never_lost("create-document");
}

#[test]
fn a_foreign_edit_while_creating_a_tree_is_never_lost() {
    foreign_edits_during_publication_are_never_lost("create-tree");
}

fn held_descriptor_write(root: &Path, hooks: Hook) -> (bool, InstallationResult) {
    let target = directory_target(&root.join("state"));
    let agents = root.join("AGENTS.md");
    assert_eq!(
        synced("1", &section(&agents, "one"), &target, false).status(),
        OperationStatus::Applied
    );
    set_hook(Some(hooks));
    let result = synced("2", &section(&agents, "two"), &target, false);
    set_hook(None);
    (retained(root, &agents), result)
}

fn descriptor_hooks(
    root: &Path,
    write_when: impl Fn(&Seam<'_>) -> bool + 'static,
) -> (Hook, Rc<RefCell<bool>>) {
    let agents = root.join("AGENTS.md");
    let held: Rc<RefCell<Option<fs::File>>> = Rc::new(RefCell::new(None));
    let written = Rc::new(RefCell::new(false));
    let opener = Rc::clone(&held);
    let open = at(boundary("backup-move"), move || {
        *opener.borrow_mut() = Some(
            fs::OpenOptions::new()
                .append(true)
                .open(&agents)
                .expect("held descriptor"),
        );
    });
    let flag = Rc::clone(&written);
    let write = at(write_when, move || {
        let mut stream = held.borrow_mut().take().expect("descriptor is held");
        stream.write_all(FOREIGN).expect("held write");
        stream.flush().expect("held flush");
        *flag.borrow_mut() = true;
    });
    (both(open, write), written)
}

#[test]
fn a_descriptor_held_across_the_backup_move_keeps_a_write_before_the_receipt() {
    let root = checkout_temp_dir("seams-descriptor-receipt");
    let (hooks, written) = descriptor_hooks(&root, boundary("publish-receipt"));
    let (kept, result) = held_descriptor_write(&root, hooks);
    assert!(
        *written.borrow(),
        "the held descriptor was not written: {result:?}"
    );
    assert!(kept, "{result:?}");
}

#[test]
fn pins_known_window_a_held_descriptor_write_before_the_backup_unlink_is_lost() {
    let root = checkout_temp_dir("seams-descriptor-unlink");
    let unlink = |seam: &Seam<'_>| matches!(seam, Seam::Unlink(path) if parent_is(path, "backup"));
    let (hooks, written) = descriptor_hooks(&root, unlink);
    let (kept, result) = held_descriptor_write(&root, hooks);
    assert!(
        *written.borrow(),
        "the held descriptor was not written: {result:?}"
    );
    assert!(
        !kept,
        "the documented descriptor window closed; update spec/transaction-protocol.md"
    );
}
