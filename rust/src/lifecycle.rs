use crate::bundle::Bundle;
use crate::configuration::{
    InstallationObservation, InstallationResult, InstallationTarget, LifecyclePreview,
};
use crate::editors::Acquisition;
use crate::models::OperationStatus;
use crate::observations::{ErrorCode, Observation, TargetError, TargetResult};
use crate::rendered::RenderedBundle;
use crate::stores::StateStore;
use crate::values::ValueError;
use crate::{SkillTarget, inspection};
use std::sync::Arc;
use std::time::Duration;

pub fn preview(
    bundle: &Bundle,
    rendered: &RenderedBundle,
    target: &InstallationTarget,
    acquisition: Acquisition,
    replace_modified: bool,
) -> Result<LifecyclePreview, ValueError> {
    crate::lifecycle_impl::public_preview(
        bundle.id(),
        Some(bundle),
        rendered,
        target,
        acquisition,
        replace_modified,
    )
}

pub fn preview_removal(
    bundle_id: &str,
    target: &InstallationTarget,
    replace_modified: bool,
) -> Result<LifecyclePreview, ValueError> {
    crate::configuration::validate_bundle_id(bundle_id)?;
    crate::lifecycle_impl::public_preview(
        bundle_id,
        None,
        &RenderedBundle::empty(),
        target,
        Acquisition::Conflict,
        replace_modified,
    )
}

pub fn apply_preview(preview: &LifecyclePreview, lock_timeout: Duration) -> InstallationResult {
    crate::lifecycle_impl::apply(preview, lock_timeout)
}

pub fn inspect_installation(
    bundle_id: &str,
    target: &InstallationTarget,
) -> Result<InstallationObservation, ValueError> {
    crate::configuration::validate_bundle_id(bundle_id)?;
    Ok(crate::lifecycle_impl::inspect(bundle_id, target))
}

pub fn recover_installation(
    bundle_id: &str,
    target: &InstallationTarget,
    lock_timeout: Duration,
) -> Result<InstallationResult, ValueError> {
    crate::configuration::validate_bundle_id(bundle_id)?;
    Ok(crate::lifecycle_impl::recover_installation(
        bundle_id,
        target,
        lock_timeout,
    ))
}

pub fn sync(
    bundle: &Bundle,
    rendered: &RenderedBundle,
    target: &InstallationTarget,
    acquisition: Acquisition,
    replace_modified: bool,
    lock_timeout: Duration,
) -> Result<InstallationResult, ValueError> {
    if let Some(error) =
        crate::lifecycle_impl::validate_reference_request(rendered, target, bundle)?
    {
        return Ok(failed(bundle.id(), rendered, target, error));
    }
    Ok(crate::lifecycle_impl::synchronize(
        bundle.id(),
        Some(bundle),
        rendered,
        target,
        acquisition,
        replace_modified,
        lock_timeout,
    ))
}

pub fn remove(
    bundle_id: &str,
    target: &InstallationTarget,
    replace_modified: bool,
    lock_timeout: Duration,
) -> Result<InstallationResult, ValueError> {
    crate::configuration::validate_bundle_id(bundle_id)?;
    Ok(crate::lifecycle_impl::synchronize(
        bundle_id,
        None,
        &RenderedBundle::empty(),
        target,
        Acquisition::Conflict,
        replace_modified,
        lock_timeout,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SkillOptions {
    replace_modified: bool,
    lock_timeout: Duration,
}

impl SkillOptions {
    pub const fn new(replace_modified: bool, lock_timeout: Duration) -> Self {
        Self {
            replace_modified,
            lock_timeout,
        }
    }

    pub const fn replace_modified(self) -> bool {
        self.replace_modified
    }

    pub const fn lock_timeout(self) -> Duration {
        self.lock_timeout
    }
}

impl Default for SkillOptions {
    fn default() -> Self {
        Self::new(false, Duration::ZERO)
    }
}

#[derive(Clone, Copy)]
enum SkillOperation {
    Install,
    Update,
    Uninstall,
}

fn skills<T: SkillTarget>(
    bundle: Option<&Bundle>,
    identifier: &str,
    targets: impl IntoIterator<Item = T>,
    store: &Arc<dyn StateStore>,
    operation: SkillOperation,
    options: SkillOptions,
) -> Result<Vec<TargetResult<T>>, ValueError> {
    let mut results: Vec<TargetResult<T>> = Vec::new();
    for item in inspection::prepare_skills(bundle, identifier, targets, store)? {
        if let Some(alias) = item.alias_of
            && let Some(prior) = results.get(alias)
        {
            results.push(prior.aliased(item.target, alias));
            continue;
        }
        let installation = match item.installation {
            Ok(installation) => installation,
            Err(error) => {
                results.push(TargetResult::new(
                    item.target,
                    item.root,
                    None,
                    OperationStatus::Failed,
                    Observation::unknown(error.clone()),
                    Some(error),
                    Vec::new(),
                ));
                continue;
            }
        };
        let prior = inspect_installation(identifier, &installation)?;
        if matches!(operation, SkillOperation::Install)
            && prior.version().is_some()
            && bundle.is_some_and(|bundle| {
                prior.bundle_digest() != Some(bundle.content_digest())
                    || prior.version() != Some(bundle.version())
            })
        {
            let error = TargetError::new(
                ErrorCode::UpdateRequired,
                "owned revision requires explicit update",
                Some(item.root.clone()),
                None,
            );
            results.push(TargetResult::new(
                item.target,
                item.root,
                None,
                OperationStatus::Failed,
                inspection::skill_observation(&prior, bundle, item.rendered.as_ref())?,
                Some(error),
                Vec::new(),
            ));
            continue;
        }
        let result = match (bundle, item.rendered.as_ref()) {
            (Some(bundle), Some(rendered)) => sync(
                bundle,
                rendered,
                &installation,
                Acquisition::Conflict,
                options.replace_modified,
                options.lock_timeout,
            )?,
            _ => remove(
                identifier,
                &installation,
                options.replace_modified,
                options.lock_timeout,
            )?,
        };
        let observation =
            inspection::skill_observation(result.observation(), bundle, item.rendered.as_ref())?;
        let recovery_paths = result
            .resources()
            .iter()
            .flat_map(|resource| resource.recovery_paths().iter().cloned())
            .collect();
        results.push(TargetResult::new(
            item.target,
            item.root,
            None,
            result.status(),
            observation,
            result.error().cloned(),
            recovery_paths,
        ));
    }
    Ok(results)
}

pub fn install<T: SkillTarget>(
    bundle: &Bundle,
    targets: impl IntoIterator<Item = T>,
    store: &Arc<dyn StateStore>,
) -> Result<Vec<TargetResult<T>>, ValueError> {
    install_with_timeout(bundle, targets, store, Duration::ZERO)
}

pub fn install_with_timeout<T: SkillTarget>(
    bundle: &Bundle,
    targets: impl IntoIterator<Item = T>,
    store: &Arc<dyn StateStore>,
    lock_timeout: Duration,
) -> Result<Vec<TargetResult<T>>, ValueError> {
    skills(
        Some(bundle),
        bundle.id(),
        targets,
        store,
        SkillOperation::Install,
        SkillOptions::new(false, lock_timeout),
    )
}

pub fn update<T: SkillTarget>(
    bundle: &Bundle,
    targets: impl IntoIterator<Item = T>,
    store: &Arc<dyn StateStore>,
) -> Result<Vec<TargetResult<T>>, ValueError> {
    update_with_options(bundle, targets, store, SkillOptions::default())
}

pub fn update_with_options<T: SkillTarget>(
    bundle: &Bundle,
    targets: impl IntoIterator<Item = T>,
    store: &Arc<dyn StateStore>,
    options: SkillOptions,
) -> Result<Vec<TargetResult<T>>, ValueError> {
    skills(
        Some(bundle),
        bundle.id(),
        targets,
        store,
        SkillOperation::Update,
        options,
    )
}

pub fn uninstall<T: SkillTarget>(
    bundle_id: &str,
    targets: impl IntoIterator<Item = T>,
    store: &Arc<dyn StateStore>,
) -> Result<Vec<TargetResult<T>>, ValueError> {
    uninstall_with_options(bundle_id, targets, store, SkillOptions::default())
}

pub fn uninstall_with_options<T: SkillTarget>(
    bundle_id: &str,
    targets: impl IntoIterator<Item = T>,
    store: &Arc<dyn StateStore>,
    options: SkillOptions,
) -> Result<Vec<TargetResult<T>>, ValueError> {
    skills(
        None,
        bundle_id,
        targets,
        store,
        SkillOperation::Uninstall,
        options,
    )
}

fn failed(
    bundle_id: &str,
    rendered: &RenderedBundle,
    target: &InstallationTarget,
    error: TargetError,
) -> InstallationResult {
    InstallationResult::new(
        OperationStatus::Failed,
        crate::lifecycle_impl::inspect(bundle_id, target),
        Vec::new(),
        Some(error),
        rendered.notices().to_vec(),
    )
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::artifacts::{Family, InstructionArtifact};
    use crate::authority::ResourceAuthority;
    use crate::configuration::{InstallationTarget, ResourceResult};
    use crate::content::{Content, FileContent, FileMode, SectionContent};
    use crate::models::{BundleIdentity, OperationStatus};
    use crate::rendered::{Notice, NoticeKind, RenderedArtifact, RenderedBundle};
    use crate::resource_models::{Index, ResourceKind, ResourceRef};
    use crate::resource_transaction::{set_fault, set_fault_after};
    use crate::test_support::checkout_temp_dir;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    fn target(root: &std::path::Path) -> InstallationTarget {
        InstallationTarget::without_context(Arc::new(
            crate::stores::DirectoryStore::new(root.join("state")).expect("store"),
        ))
        .expect("target")
    }

    fn put_index(target: &InstallationTarget, data: &[u8]) {
        let store = target.store();
        let key = crate::state::index_key("team", &target.context_digest());
        let current = store.read(&key).expect("index bytes");
        assert!(
            store
                .write(&key, data, current.as_deref())
                .expect("write index")
        );
    }

    fn bundle(version: &str) -> Bundle {
        Bundle::from_artifacts(
            BundleIdentity::new("team", version).expect("identity"),
            [crate::artifacts::Artifact::Instruction(
                InstructionArtifact::new("guide", "text").expect("instruction"),
            )],
            [],
            [],
        )
        .expect("bundle")
    }

    fn section(path: &std::path::Path, text: &str) -> RenderedBundle {
        let artifact = RenderedArtifact::new(
            "guide",
            Family::Instructions,
            path.to_path_buf(),
            Content::Section(SectionContent::new("guide", text, None).expect("section")),
            None,
            false,
            Vec::new(),
        )
        .expect("artifact");
        RenderedBundle::new([artifact], [], []).expect("render")
    }

    fn file(path: &std::path::Path, bytes: &[u8]) -> RenderedBundle {
        let artifact = RenderedArtifact::new(
            "guide",
            Family::Instructions,
            path.to_path_buf(),
            Content::File(FileContent::new(bytes, FileMode::Readable)),
            None,
            false,
            Vec::new(),
        )
        .expect("artifact");
        RenderedBundle::new([artifact], [], []).expect("render")
    }

    fn filesystem_snapshot(root: &std::path::Path) -> Vec<(std::path::PathBuf, Option<Vec<u8>>)> {
        fn visit(
            root: &std::path::Path,
            current: &std::path::Path,
            snapshot: &mut Vec<(std::path::PathBuf, Option<Vec<u8>>)>,
        ) {
            let mut entries = fs::read_dir(current)
                .expect("read directory")
                .collect::<Result<Vec<_>, _>>()
                .expect("directory entries");
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                let path = entry.path();
                let relative = path
                    .strip_prefix(root)
                    .expect("relative path")
                    .to_path_buf();
                if entry.file_type().expect("file type").is_dir() {
                    snapshot.push((relative, None));
                    visit(root, &path, snapshot);
                } else {
                    snapshot.push((relative, Some(fs::read(path).expect("file bytes"))));
                }
            }
        }

        let mut snapshot = Vec::new();
        visit(root, root, &mut snapshot);
        snapshot
    }

    fn names(directory: &std::path::Path) -> Vec<String> {
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

    fn interrupted(seam: &'static str, operation: impl FnOnce()) {
        crate::resource_transaction::set_crash(Some(seam));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation));
        crate::resource_transaction::set_crash(None);
        assert!(outcome.is_err(), "{seam} did not interrupt the operation");
    }

    fn run(
        target: &InstallationTarget,
        destination: &std::path::Path,
        version: &str,
    ) -> OperationStatus {
        sync(
            &bundle(version),
            &section(destination, if version == "1" { "one" } else { "two" }),
            target,
            Acquisition::Conflict,
            false,
            Duration::ZERO,
        )
        .expect("sync")
        .status()
    }

    #[test]
    fn a_crash_after_creating_the_transaction_directory_is_cleaned_up_by_the_next_run() {
        for directory_backed in [true, false] {
            let root = checkout_temp_dir("lifecycle-crash-after-directory");
            let target = if directory_backed {
                target(&root)
            } else {
                InstallationTarget::without_context(Arc::new(crate::stores::MemoryStore::new()))
                    .expect("target")
            };
            let destination = root.join("AGENTS.md");
            fs::write(&destination, b"# Foreign\n").expect("foreign");
            assert_eq!(run(&target, &destination, "1"), OperationStatus::Applied);
            interrupted("directory", || {
                run(&target, &destination, "2");
            });
            let store = target.store().as_ref();
            let preparation: crate::resource_models::Preparation = crate::state::read_record(
                store,
                &crate::state::resource_key(&destination, "preparation"),
                "Preparation",
            )
            .expect("preparation read")
            .expect("preparation");
            assert_eq!(
                store
                    .read(&crate::state::resource_key(&destination, "journal"))
                    .expect("journal read"),
                None
            );
            assert_eq!(names(preparation.directory()), ["backup", "staging"]);
            let observed = inspect_installation("team", &target).expect("inspect");
            assert_eq!(
                observed.resources()[0].recovery_paths(),
                [preparation.directory().to_path_buf()]
            );
            assert_eq!(run(&target, &destination, "2"), OperationStatus::Applied);
            assert_eq!(
                store
                    .read(&crate::state::resource_key(&destination, "preparation"))
                    .expect("preparation read"),
                None
            );
            assert!(!preparation.directory().exists());
            assert_eq!(
                names(&root),
                if directory_backed {
                    vec!["AGENTS.md", "state"]
                } else {
                    vec!["AGENTS.md"]
                }
            );
        }
    }

    #[test]
    fn a_same_filesystem_store_leaves_nothing_beside_the_destination_at_filesystem_crash_points() {
        for seam in ["directory", "backup", "publish"] {
            let root = checkout_temp_dir("lifecycle-never-beside");
            let project = root.join("project");
            fs::create_dir(&project).expect("project");
            let destination = project.join("AGENTS.md");
            fs::write(&destination, b"# Foreign\n").expect("foreign");
            let target = target(&root);
            assert_eq!(run(&target, &destination, "1"), OperationStatus::Applied);
            interrupted(seam, || {
                run(&target, &destination, "2");
            });
            assert!(
                names(&project).iter().all(|name| name == "AGENTS.md"),
                "{seam}: {:?}",
                names(&project)
            );
            assert_eq!(
                run(&target, &destination, "2"),
                OperationStatus::Applied,
                "{seam}"
            );
            assert_eq!(names(&project), ["AGENTS.md"], "{seam}");
            assert_eq!(names(&root.join("state/staging")), Vec::<String>::new());
            assert!(
                fs::read_to_string(&destination)
                    .expect("published")
                    .contains("two"),
                "{seam}"
            );
        }
    }

    #[test]
    fn lifecycle_creates_updates_and_removes_source_free() {
        let root = checkout_temp_dir("lifecycle-basic");
        let destination = root.join("AGENTS.md");
        fs::write(&destination, b"# foreign\n").expect("foreign");
        let target = target(&root);
        let first = preview(
            &bundle("1"),
            &section(&destination, "generated"),
            &target,
            Acquisition::Conflict,
            false,
        )
        .expect("preview");
        assert!(first.applicable());
        assert_eq!(
            apply_preview(&first, Duration::ZERO).status(),
            OperationStatus::Applied
        );
        assert!(
            inspect_installation("team", &target)
                .expect("inspection")
                .is_current()
        );
        assert_eq!(
            sync(
                &bundle("1"),
                &section(&destination, "generated"),
                &target,
                Acquisition::Conflict,
                false,
                Duration::ZERO,
            )
            .expect("sync")
            .status(),
            OperationStatus::Unchanged
        );
        assert_eq!(
            sync(
                &bundle("2"),
                &section(&destination, "second"),
                &target,
                Acquisition::Conflict,
                false,
                Duration::ZERO,
            )
            .expect("update")
            .status(),
            OperationStatus::Applied
        );
        assert_eq!(
            remove("team", &target, false, Duration::ZERO)
                .expect("remove")
                .status(),
            OperationStatus::Applied
        );
        assert_eq!(fs::read(&destination).expect("read"), b"# foreign\n");
    }

    #[test]
    fn takeover_restores_and_adoption_removes() {
        for (name, acquisition, remains) in [
            ("takeover", Acquisition::Takeover, true),
            ("adoption", Acquisition::Adopt, false),
        ] {
            let root = checkout_temp_dir(name);
            let destination = root.join("owned");
            fs::write(
                &destination,
                if acquisition == Acquisition::Adopt {
                    b"desired".as_slice()
                } else {
                    b"original".as_slice()
                },
            )
            .expect("original");
            let target = target(&root);
            let result = sync(
                &bundle("1"),
                &file(&destination, b"desired"),
                &target,
                acquisition,
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
            assert_eq!(
                remove("team", &target, false, Duration::ZERO)
                    .expect("remove")
                    .status(),
                OperationStatus::Applied
            );
            assert_eq!(destination.exists(), remains);
            if remains {
                assert_eq!(fs::read(destination).expect("restored"), b"original");
            }
        }
    }

    #[test]
    fn preview_rejects_changed_content_index_and_receipt_state() {
        let root = checkout_temp_dir("lifecycle-stale");
        let destination = root.join("AGENTS.md");
        fs::write(&destination, b"# foreign\n").expect("foreign");
        let target = target(&root);
        let proposal = preview(
            &bundle("1"),
            &section(&destination, "generated"),
            &target,
            Acquisition::Conflict,
            false,
        )
        .expect("preview");
        fs::write(&destination, b"# changed\n").expect("changed");
        let stale = apply_preview(&proposal, Duration::ZERO);
        assert_eq!(stale.status(), OperationStatus::Failed);
        assert_eq!(
            stale.error().map(TargetError::code),
            Some(crate::ErrorCode::ConcurrentChange)
        );

        let fresh = preview(
            &bundle("1"),
            &section(&destination, "generated"),
            &target,
            Acquisition::Conflict,
            false,
        )
        .expect("preview");
        let concurrent =
            crate::resource_plan::completed_index("team", &target.context_digest(), None)
                .expect("concurrent index");
        put_index(&target, &crate::codec::encode(&concurrent));
        let stale = apply_preview(&fresh, Duration::ZERO);
        assert_eq!(
            stale.error().map(TargetError::code),
            Some(crate::ErrorCode::ConcurrentChange)
        );
    }

    #[test]
    fn unsupported_notices_block_all_publication() {
        let root = checkout_temp_dir("lifecycle-unsupported");
        let destination = root.join("AGENTS.md");
        let target = target(&root);
        let rendered = RenderedBundle::with_notices(
            section(&destination, "generated").artifacts().to_vec(),
            [],
            [Notice::new("guide", NoticeKind::Unsupported, "unsupported event").expect("notice")],
            [],
        )
        .expect("render");
        let result = sync(
            &bundle("1"),
            &rendered,
            &target,
            Acquisition::Conflict,
            false,
            Duration::ZERO,
        )
        .expect("sync");
        assert_eq!(result.status(), OperationStatus::Failed);
        assert_eq!(
            result.error().map(TargetError::code),
            Some(crate::ErrorCode::Unsupported)
        );
        assert_eq!(result.notices(), rendered.notices());
        assert!(!destination.exists());
    }

    #[test]
    fn desired_generation_comparison_checks_every_identity() {
        let root = checkout_temp_dir("lifecycle-comparison");
        let destination = root.join("AGENTS.md");
        let target = target(&root);
        let desired = bundle("1");
        let rendered = section(&destination, "generated");
        let installed = sync(
            &desired,
            &rendered,
            &target,
            Acquisition::Conflict,
            false,
            Duration::ZERO,
        )
        .expect("sync");
        let observed = installed.observation();
        assert!(observed.matches(&desired, &rendered));
        assert!(!observed.matches(&bundle("2"), &rendered));
        assert!(!observed.matches(&desired, &section(&destination, "changed")));
        fs::write(
            &destination,
            fs::read(&destination)
                .expect("read")
                .into_iter()
                .map(|byte| if byte == b'g' { b'x' } else { byte })
                .collect::<Vec<_>>(),
        )
        .expect("change");
        let modified = inspect_installation("team", &target).expect("inspect");
        assert!(!modified.is_current());
        assert!(!modified.matches(&desired, &rendered));
    }

    #[test]
    fn missing_committed_receipt_blocks_inspection_recovery_and_removal() {
        for membership in ["current", "previous"] {
            let root = checkout_temp_dir(&format!("lifecycle-missing-receipt-{membership}"));
            let destination = root.join("AGENTS.md");
            let target = target(&root);
            sync(
                &bundle("1"),
                &section(&destination, "generated"),
                &target,
                Acquisition::Conflict,
                false,
                Duration::ZERO,
            )
            .expect("sync");
            let proposal = preview_removal("team", &target, false).expect("preview");
            let reference = proposal.resources()[0].resource().clone();
            if membership == "previous" {
                let installed = proposal.index().expect("index");
                let previous = Index::new(
                    installed.bundle_id().to_owned(),
                    installed.context_digest().to_owned(),
                    None,
                    None,
                    installed.current().cloned(),
                    Vec::new(),
                    2,
                )
                .expect("previous index");
                put_index(&target, &crate::codec::encode(&previous));
            }
            let key = crate::state::resource_key(reference.destination(), "receipt");
            let recorded = target
                .store()
                .read(&key)
                .expect("receipt")
                .expect("receipt");
            assert!(
                target
                    .store()
                    .delete(&key, &recorded)
                    .expect("delete receipt")
            );
            let before = fs::read(&destination).expect("payload");
            let observed = inspect_installation("team", &target).expect("inspect");
            assert_eq!(
                observed.resources()[0].error().map(TargetError::code),
                Some(crate::ErrorCode::InvalidState),
                "{membership}"
            );
            for result in [
                recover_installation("team", &target, Duration::ZERO).expect("recover"),
                remove("team", &target, true, Duration::ZERO).expect("remove"),
            ] {
                assert_eq!(result.status(), OperationStatus::Failed, "{membership}");
                assert_eq!(
                    result.error().map(TargetError::code),
                    Some(crate::ErrorCode::InvalidState),
                    "{membership}"
                );
            }
            assert_eq!(fs::read(&destination).expect("payload retained"), before);
        }
    }

    #[test]
    fn recovery_is_explicit_and_preserves_pending_membership() {
        let root = checkout_temp_dir("lifecycle-recovery");
        let destination = root.join("AGENTS.md");
        let target = target(&root);
        set_fault(Some("cleanup"));
        let interrupted = sync(
            &bundle("1"),
            &section(&destination, "generated"),
            &target,
            Acquisition::Conflict,
            false,
            Duration::ZERO,
        )
        .expect("sync");
        set_fault(None);
        assert_eq!(interrupted.status(), OperationStatus::Partial);
        assert_eq!(interrupted.resources().len(), 1);
        assert_eq!(
            interrupted.resources()[0].status(),
            OperationStatus::Applied
        );
        assert!(interrupted.observation().pending());
        let recovered = recover_installation("team", &target, Duration::ZERO).expect("recovery");
        assert_eq!(recovered.status(), OperationStatus::Applied);
        assert!(recovered.observation().pending());
        let removal = preview_removal("team", &target, false).expect("removal preview");
        assert!(removal.applicable());
        assert_eq!(
            apply_preview(&removal, Duration::ZERO).status(),
            OperationStatus::Applied
        );
        assert!(!destination.exists());
    }

    #[test]
    fn lifecycle_operations_reject_index_destination_overlap_without_mutation() {
        let root = checkout_temp_dir("lifecycle-recovery-destination-overlap");
        let destination = root.join("payload");
        let authority = ResourceAuthority::new(&destination).expect("authority");
        let target = InstallationTarget::without_context(Arc::new(
            crate::stores::DirectoryStore::new(&destination).expect("store"),
        ))
        .expect("target");
        let reference = ResourceRef::new(
            authority.destination().to_path_buf(),
            ResourceKind::Document,
        )
        .expect("resource");
        let index = Index::new(
            "team".to_owned(),
            target.context_digest(),
            None,
            None,
            None,
            vec![reference.clone()],
            2,
        )
        .expect("index");
        put_index(&target, &crate::codec::encode(&index));
        let before = filesystem_snapshot(&root);

        let results = [
            recover_installation("team", &target, Duration::ZERO).expect("recovery result"),
            remove("team", &target, false, Duration::ZERO).expect("removal result"),
            sync(
                &bundle("1"),
                &section(&root.join("other"), "generated"),
                &target,
                Acquisition::Conflict,
                false,
                Duration::ZERO,
            )
            .expect("sync result"),
        ];

        for result in results {
            assert_eq!(result.status(), OperationStatus::Failed);
            assert_eq!(result.resources(), [] as [ResourceResult; 0]);
            assert_eq!(
                result.error().map(TargetError::code),
                Some(crate::ErrorCode::InvalidState)
            );
            assert_eq!(
                result.error().map(TargetError::message),
                Some("state store overlaps a resource or authority")
            );
            assert_eq!(result.error().and_then(TargetError::path), None);
            assert_eq!(filesystem_snapshot(&root), before);
            assert_eq!(
                target
                    .store()
                    .read(&crate::state::resource_key(
                        reference.destination(),
                        "authority"
                    ))
                    .expect("authority read"),
                None
            );
        }
    }

    #[test]
    fn preview_binds_receipts_security_ancestors_and_index_state() {
        let root = checkout_temp_dir("lifecycle-bindings");
        let destination = root.join("project/AGENTS.md");
        fs::create_dir(destination.parent().expect("parent")).expect("project");
        fs::write(&destination, b"# foreign\n").expect("foreign");
        let target = target(&root);
        sync(
            &bundle("1"),
            &section(&destination, "generated"),
            &target,
            Acquisition::Conflict,
            false,
            Duration::ZERO,
        )
        .expect("install");

        let receipt_preview = preview(
            &bundle("2"),
            &section(&destination, "second"),
            &target,
            Acquisition::Conflict,
            false,
        )
        .expect("receipt preview");
        let receipt_key = crate::state::resource_key(
            receipt_preview.resources()[0].resource().destination(),
            "receipt",
        );
        let store = target.store();
        let bytes = store.read(&receipt_key).expect("receipt").expect("receipt");
        let mut changed = bytes.clone();
        changed.insert(1, b' ');
        assert!(
            store
                .write(&receipt_key, &changed, Some(&bytes))
                .expect("replace receipt")
        );
        assert_eq!(
            apply_preview(&receipt_preview, Duration::ZERO)
                .error()
                .map(TargetError::code),
            Some(crate::ErrorCode::ConcurrentChange)
        );
        assert!(
            store
                .write(&receipt_key, &bytes, Some(&changed))
                .expect("restore receipt")
        );

        let ancestor_preview = preview(
            &bundle("2"),
            &section(&destination, "second"),
            &target,
            Acquisition::Conflict,
            false,
        )
        .expect("ancestor preview");
        let parent = destination.parent().expect("parent");
        let mut permissions = fs::metadata(parent).expect("metadata").permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(parent, permissions).expect("chmod");
        assert_eq!(
            apply_preview(&ancestor_preview, Duration::ZERO)
                .error()
                .map(TargetError::code),
            Some(crate::ErrorCode::ConcurrentChange)
        );
    }

    #[test]
    fn routing_context_is_frozen_but_not_part_of_index_identity() {
        let root = checkout_temp_dir("lifecycle-context");
        let first = InstallationTarget::new(
            Arc::clone(target(&root).store()),
            [(String::from("surface"), String::from("application"))],
            [(String::from("agent"), String::from("claude"))],
        )
        .expect("target");
        let second = InstallationTarget::new(
            Arc::clone(first.store()),
            [(String::from("surface"), String::from("application"))],
            [(String::from("agent"), String::from("codex"))],
        )
        .expect("target");
        assert_eq!(first.context_digest(), second.context_digest());
        assert_ne!(first.routing_context(), second.routing_context());
    }

    #[test]
    fn preflight_conflicts_prevent_all_resource_publication() {
        let root = checkout_temp_dir("lifecycle-preflight");
        let first = root.join("first");
        let second = root.join("second");
        fs::write(&second, b"foreign").expect("foreign");
        let target = target(&root);
        let rendered = RenderedBundle::new(
            [
                RenderedArtifact::new(
                    "first",
                    Family::Instructions,
                    first.clone(),
                    Content::File(FileContent::new(b"new", FileMode::Readable)),
                    None,
                    false,
                    Vec::new(),
                )
                .expect("first"),
                RenderedArtifact::new(
                    "second",
                    Family::Instructions,
                    second.clone(),
                    Content::File(FileContent::new(b"new", FileMode::Readable)),
                    None,
                    false,
                    Vec::new(),
                )
                .expect("second"),
            ],
            [],
            [],
        )
        .expect("rendered");
        let result = sync(
            &bundle("1"),
            &rendered,
            &target,
            Acquisition::Conflict,
            false,
            Duration::ZERO,
        )
        .expect("sync");
        assert_eq!(result.status(), OperationStatus::Failed);
        assert!(!first.exists());
        assert_eq!(fs::read(second).expect("foreign remains"), b"foreign");
    }

    #[test]
    fn index_completion_failure_aggregates_applied_resources_as_partial() {
        let root = checkout_temp_dir("lifecycle-partial");
        let first = root.join("first");
        let second = root.join("second");
        let target = target(&root);
        let rendered = RenderedBundle::new(
            [
                RenderedArtifact::new(
                    "first",
                    Family::Instructions,
                    first.clone(),
                    Content::File(FileContent::new(b"one", FileMode::Readable)),
                    None,
                    false,
                    Vec::new(),
                )
                .expect("first"),
                RenderedArtifact::new(
                    "second",
                    Family::Instructions,
                    second.clone(),
                    Content::File(FileContent::new(b"two", FileMode::Readable)),
                    None,
                    false,
                    Vec::new(),
                )
                .expect("second"),
            ],
            [],
            [],
        )
        .expect("rendered");
        set_fault(Some("index-completion"));
        let result = sync(
            &bundle("1"),
            &rendered,
            &target,
            Acquisition::Conflict,
            false,
            Duration::ZERO,
        )
        .expect("sync");
        set_fault(None);
        assert_eq!(result.status(), OperationStatus::Partial);
        assert_eq!(result.resources().len(), 2);
        assert!(
            result
                .resources()
                .iter()
                .all(|resource| resource.status() == OperationStatus::Applied)
        );
        assert_eq!(fs::read(first).expect("first"), b"one");
        assert_eq!(fs::read(second).expect("second"), b"two");
        assert_eq!(
            recover_installation("team", &target, Duration::ZERO)
                .expect("recover")
                .status(),
            OperationStatus::Unchanged
        );
        assert_eq!(
            sync(
                &bundle("1"),
                &rendered,
                &target,
                Acquisition::Conflict,
                false,
                Duration::ZERO,
            )
            .expect("reconcile")
            .status(),
            OperationStatus::Applied
        );
        assert!(
            inspect_installation("team", &target)
                .expect("inspect")
                .is_current()
        );
    }

    #[test]
    fn later_resource_precondition_failure_is_partial_and_ordered() {
        let root = checkout_temp_dir("lifecycle-later-precondition");
        let target = target(&root);
        let rendered = RenderedBundle::new(
            [
                RenderedArtifact::new(
                    "first",
                    Family::Instructions,
                    root.join("first"),
                    Content::File(FileContent::new(b"one", FileMode::Readable)),
                    None,
                    false,
                    Vec::new(),
                )
                .expect("first"),
                RenderedArtifact::new(
                    "second",
                    Family::Instructions,
                    root.join("second"),
                    Content::File(FileContent::new(b"two", FileMode::Readable)),
                    None,
                    false,
                    Vec::new(),
                )
                .expect("second"),
            ],
            [],
            [],
        )
        .expect("rendered");
        let proposal = preview(
            &bundle("1"),
            &rendered,
            &target,
            Acquisition::Conflict,
            false,
        )
        .expect("preview");
        let order = proposal
            .resources()
            .iter()
            .map(|plan| plan.resource().clone())
            .collect::<Vec<_>>();

        set_fault_after("resource-precondition", 1);
        let result = apply_preview(&proposal, Duration::ZERO);
        set_fault(None);

        assert_eq!(result.status(), OperationStatus::Partial);
        assert_eq!(result.resources().len(), 2);
        assert_eq!(result.resources()[0].resource(), &order[0]);
        assert_eq!(result.resources()[0].status(), OperationStatus::Applied);
        assert_eq!(result.resources()[1].resource(), &order[1]);
        assert_eq!(result.resources()[1].status(), OperationStatus::Failed);
    }

    #[test]
    fn shared_sections_are_planned_in_artifact_id_order() {
        for run in 0..16 {
            let root = checkout_temp_dir(&format!("lifecycle-section-order-{run}"));
            let destination = root.join("AGENTS.md");
            fs::write(&destination, b"# Foreign\n").expect("foreign");
            let target = target(&root);
            let desired = Bundle::from_artifacts(
                BundleIdentity::new("team", "1").expect("identity"),
                [
                    crate::artifacts::Artifact::Instruction(
                        InstructionArtifact::new("zeta", "z").expect("zeta"),
                    ),
                    crate::artifacts::Artifact::Instruction(
                        InstructionArtifact::new("alpha", "a").expect("alpha"),
                    ),
                ],
                [],
                [],
            )
            .expect("bundle");
            let alpha = RenderedArtifact::new(
                "alpha",
                Family::Instructions,
                destination.clone(),
                Content::Section(
                    SectionContent::new("alpha", "Alpha", None).expect("alpha section"),
                ),
                None,
                false,
                Vec::new(),
            )
            .expect("alpha artifact");
            let rendered = RenderedBundle::new(
                [
                    RenderedArtifact::new(
                        "zeta",
                        Family::Instructions,
                        destination.clone(),
                        Content::Section(
                            SectionContent::new("zeta", "Zeta", None).expect("zeta section"),
                        ),
                        None,
                        false,
                        Vec::new(),
                    )
                    .expect("zeta artifact"),
                    RenderedArtifact::new(
                        "beta",
                        alpha.family(),
                        alpha.destination().to_path_buf(),
                        alpha.content().clone(),
                        alpha.subtree().map(str::to_owned),
                        false,
                        alpha.schema_requirements().to_vec(),
                    )
                    .expect("physical alias"),
                    alpha,
                ],
                [],
                [],
            )
            .expect("rendered");

            let result = sync(
                &desired,
                &rendered,
                &target,
                Acquisition::Conflict,
                false,
                Duration::ZERO,
            )
            .expect("sync");

            assert_eq!(result.status(), OperationStatus::Applied);
            assert_eq!(
                fs::read(&destination).expect("document"),
                b"# Foreign\n\n<!-- flyrail:alpha:start -->\nAlpha\n<!-- flyrail:alpha:end -->\n\n<!-- flyrail:zeta:start -->\nZeta\n<!-- flyrail:zeta:end -->\n",
                "run {run}"
            );
            let authority = ResourceAuthority::new(&destination).expect("authority");
            let installed: crate::Receipt = crate::state::read_record(
                target.store().as_ref(),
                &crate::state::resource_key(authority.destination(), "receipt"),
                "Receipt",
            )
            .expect("receipt read")
            .expect("receipt");
            assert_eq!(installed.claims().len(), 2, "run {run}");
        }
    }

    #[test]
    fn corrupt_resource_state_never_authorizes_payload_changes() {
        for damage in ["receipt", "receipt-resource"] {
            let root = checkout_temp_dir(&format!("lifecycle-corrupt-resource-{damage}"));
            let destination = root.join("AGENTS.md");
            let target = target(&root);
            sync(
                &bundle("1"),
                &section(&destination, "generated"),
                &target,
                Acquisition::Conflict,
                false,
                Duration::ZERO,
            )
            .expect("install");
            let proposal = preview_removal("team", &target, false).expect("preview");
            let key = crate::state::resource_key(
                proposal.resources()[0].resource().destination(),
                "receipt",
            );
            let recorded = target
                .store()
                .read(&key)
                .expect("receipt")
                .expect("receipt");
            let corrupt = if damage == "receipt" {
                b"{}".to_vec()
            } else {
                let other = ResourceRef::new(root.join("OTHER.md"), ResourceKind::Document)
                    .expect("other resource");
                crate::codec::encode(
                    &crate::Receipt::new(
                        other,
                        "1".repeat(32),
                        Vec::new(),
                        crate::editors::DocumentProvenance::default(),
                        Vec::new(),
                        2,
                    )
                    .expect("other receipt"),
                )
            };
            assert!(
                target
                    .store()
                    .write(&key, &corrupt, Some(&recorded))
                    .expect("corrupt receipt")
            );
            let before = fs::read(&destination).expect("payload");

            let observed = inspect_installation("team", &target).expect("inspect");
            assert_eq!(
                observed.resources()[0].error().map(TargetError::code),
                Some(crate::ErrorCode::InvalidState),
                "{damage}"
            );
            for result in [
                sync(
                    &bundle("2"),
                    &section(&destination, "changed"),
                    &target,
                    Acquisition::Conflict,
                    true,
                    Duration::ZERO,
                )
                .expect("sync"),
                remove("team", &target, true, Duration::ZERO).expect("remove"),
            ] {
                assert_eq!(result.status(), OperationStatus::Failed, "{damage}");
            }
            assert_eq!(fs::read(&destination).expect("payload retained"), before);
        }
    }

    #[test]
    fn malformed_index_record_does_not_admit_resources() {
        let root = checkout_temp_dir("lifecycle-malformed-index");
        let destination = root.join("AGENTS.md");
        let target = target(&root);
        put_index(&target, b"{}");

        let result = sync(
            &bundle("1"),
            &section(&destination, "generated"),
            &target,
            Acquisition::Conflict,
            false,
            Duration::ZERO,
        )
        .expect("sync");

        assert_eq!(result.status(), OperationStatus::Failed);
        assert_eq!(
            result.error().map(TargetError::code),
            Some(crate::ErrorCode::InvalidState)
        );
        assert_eq!(
            target.store().keys("resource/").expect("resource keys"),
            Vec::<String>::new()
        );
        assert!(!destination.exists());
    }

    #[test]
    fn ordinary_plan_failures_return_empty_notices() {
        let root = checkout_temp_dir("lifecycle-failure-notices");
        let destination = root.join("owned");
        fs::write(&destination, b"foreign").expect("foreign");
        let target = target(&root);
        let rendered = RenderedBundle::with_notices(
            file(&destination, b"desired").artifacts().to_vec(),
            [],
            [Notice::new("guide", NoticeKind::Prerequisite, "install helper").expect("notice")],
            [],
        )
        .expect("render");

        let proposal = preview(
            &bundle("1"),
            &rendered,
            &target,
            Acquisition::Conflict,
            false,
        )
        .expect("preview");
        let applied = apply_preview(&proposal, Duration::ZERO);
        assert_eq!(applied.status(), OperationStatus::Failed);
        assert_eq!(applied.notices(), [] as [Notice; 0]);

        let synchronized = sync(
            &bundle("1"),
            &rendered,
            &target,
            Acquisition::Conflict,
            false,
            Duration::ZERO,
        )
        .expect("sync");
        assert_eq!(synchronized.status(), OperationStatus::Failed);
        assert_eq!(synchronized.notices(), [] as [Notice; 0]);
    }

    #[test]
    fn public_lifecycle_entry_points_validate_bundle_ids_before_io() {
        let root = checkout_temp_dir("lifecycle-public-validation");
        let target = target(&root);

        for error in [
            preview_removal("Bad ID", &target, false).expect_err("preview id"),
            inspect_installation("Bad ID", &target).expect_err("inspect id"),
            recover_installation("Bad ID", &target, Duration::ZERO).expect_err("recover id"),
            remove("Bad ID", &target, false, Duration::ZERO).expect_err("remove id"),
        ] {
            assert_eq!(
                error.message(),
                "bundle id must be 1-64 lowercase ASCII letters/digits with internal hyphens"
            );
        }
        assert!(!root.join("state").exists());
    }

    #[test]
    fn installation_targets_validate_context_before_io() {
        let root = checkout_temp_dir("lifecycle-target-validation");
        let error = InstallationTarget::new(
            Arc::new(crate::stores::DirectoryStore::new(root.join("state")).expect("store")),
            [
                (String::from("surface"), String::from("application")),
                (String::from("surface"), String::from("project")),
            ],
            std::iter::empty(),
        )
        .expect_err("duplicate context");
        assert_eq!(error.to_string(), "target context keys must be unique");
        assert!(!root.join("state").exists());
    }
}
