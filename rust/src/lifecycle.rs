use crate::bundle::Bundle;
use crate::configuration::{
    InstallationObservation, InstallationResult, InstallationTarget, LifecyclePreview,
};
use crate::editors::Acquisition;
use crate::models::OperationStatus;
use crate::observations::{ErrorCode, Observation, TargetError, TargetResult};
use crate::rendered::RenderedBundle;
use crate::values::ValueError;
use crate::{SkillTarget, inspection};
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
    operation: SkillOperation,
    options: SkillOptions,
) -> Result<Vec<TargetResult<T>>, ValueError> {
    let mut results: Vec<TargetResult<T>> = Vec::new();
    for item in inspection::prepare_skills(bundle, identifier, targets)? {
        if let Some(alias) = item.alias_of
            && let Some(prior) = results.get(alias)
        {
            results.push(prior.aliased(item.target, alias));
            continue;
        }
        let state_root = crate::authority::state_path(&item.root);
        let installation = match item.installation {
            Ok(installation) => installation,
            Err(error) => {
                results.push(TargetResult::new(
                    item.target,
                    item.root,
                    state_root,
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
                state_root,
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
            state_root,
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
) -> Result<Vec<TargetResult<T>>, ValueError> {
    install_with_timeout(bundle, targets, Duration::ZERO)
}

pub fn install_with_timeout<T: SkillTarget>(
    bundle: &Bundle,
    targets: impl IntoIterator<Item = T>,
    lock_timeout: Duration,
) -> Result<Vec<TargetResult<T>>, ValueError> {
    skills(
        Some(bundle),
        bundle.id(),
        targets,
        SkillOperation::Install,
        SkillOptions::new(false, lock_timeout),
    )
}

pub fn update<T: SkillTarget>(
    bundle: &Bundle,
    targets: impl IntoIterator<Item = T>,
) -> Result<Vec<TargetResult<T>>, ValueError> {
    update_with_options(bundle, targets, SkillOptions::default())
}

pub fn update_with_options<T: SkillTarget>(
    bundle: &Bundle,
    targets: impl IntoIterator<Item = T>,
    options: SkillOptions,
) -> Result<Vec<TargetResult<T>>, ValueError> {
    skills(
        Some(bundle),
        bundle.id(),
        targets,
        SkillOperation::Update,
        options,
    )
}

pub fn uninstall<T: SkillTarget>(
    bundle_id: &str,
    targets: impl IntoIterator<Item = T>,
) -> Result<Vec<TargetResult<T>>, ValueError> {
    uninstall_with_options(bundle_id, targets, SkillOptions::default())
}

pub fn uninstall_with_options<T: SkillTarget>(
    bundle_id: &str,
    targets: impl IntoIterator<Item = T>,
    options: SkillOptions,
) -> Result<Vec<TargetResult<T>>, ValueError> {
    skills(None, bundle_id, targets, SkillOperation::Uninstall, options)
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
    use crate::configuration::InstallationTarget;
    use crate::content::{Content, FileContent, FileMode, SectionContent};
    use crate::models::{BundleIdentity, OperationStatus};
    use crate::rendered::{Notice, NoticeKind, RenderedArtifact, RenderedBundle};
    use crate::resource_models::{Index, ResourceKind, ResourceRef};
    use crate::resource_transaction::{set_fault, set_fault_after};
    use crate::test_support::checkout_temp_dir;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

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

    #[test]
    fn lifecycle_creates_updates_and_removes_source_free() {
        let root = checkout_temp_dir("lifecycle-basic");
        let destination = root.join("AGENTS.md");
        fs::write(&destination, b"# foreign\n").expect("foreign");
        let target = InstallationTarget::without_context(root.join("index")).expect("target");
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
            let target = InstallationTarget::without_context(root.join("index")).expect("target");
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
        let target = InstallationTarget::without_context(root.join("index")).expect("target");
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
        fs::create_dir(target.index_root()).expect("index");
        fs::write(target.index_root().join("unknown"), b"x").expect("unknown");
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
        let target = InstallationTarget::without_context(root.join("index")).expect("target");
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
        let target = InstallationTarget::without_context(root.join("index")).expect("target");
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
            let target = InstallationTarget::without_context(root.join("index")).expect("target");
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
                fs::write(
                    target.index_root().join("index.json"),
                    crate::codec::encode(&previous),
                )
                .expect("replace index");
            }
            fs::remove_file(reference.state_root().join("receipt.json")).expect("remove receipt");
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
    fn missing_receipt_referenced_only_by_completed_preparation_blocks_all_writes() {
        let root = checkout_temp_dir("lifecycle-missing-prepared-receipt");
        let destination = root.join("AGENTS.md");
        let target = InstallationTarget::without_context(root.join("index")).expect("target");
        let desired = bundle("1");
        let rendered = section(&destination, "generated");
        sync(
            &desired,
            &rendered,
            &target,
            Acquisition::Conflict,
            false,
            Duration::ZERO,
        )
        .expect("install");
        let proposal = preview_removal("team", &target, false).expect("preview");
        let generation = proposal
            .index()
            .and_then(Index::current)
            .expect("generation")
            .clone();
        let reference = proposal.resources()[0].resource().clone();
        let pending = crate::resource_plan::pending_index(
            "team",
            &target.context_digest(),
            None,
            Some(generation.clone()),
            vec![reference.clone()],
        )
        .expect("pending");
        let completed = crate::resource_plan::completed_index(
            "team",
            &target.context_digest(),
            Some(generation),
        )
        .expect("completed");
        let index_path = target.index_root().join("index.json");
        let prepared_path = target.index_root().join("index.json.next");
        fs::write(&index_path, crate::codec::encode(&pending)).expect("pending index");
        fs::write(&prepared_path, crate::codec::encode(&completed)).expect("completed index");
        fs::remove_file(reference.state_root().join("receipt.json")).expect("remove receipt");
        let before = (
            fs::read(&destination).expect("payload"),
            fs::read(&index_path).expect("index"),
            fs::read(&prepared_path).expect("prepared"),
        );

        let observed = inspect_installation("team", &target).expect("inspect");
        assert_eq!(
            observed.resources()[0].error().map(TargetError::code),
            Some(crate::ErrorCode::InvalidState)
        );
        for result in [
            sync(
                &desired,
                &rendered,
                &target,
                Acquisition::Conflict,
                true,
                Duration::ZERO,
            )
            .expect("sync"),
            remove("team", &target, true, Duration::ZERO).expect("remove"),
            recover_installation("team", &target, Duration::ZERO).expect("recover"),
        ] {
            assert_eq!(result.status(), OperationStatus::Failed);
            assert_eq!(
                result.error().map(TargetError::code),
                Some(crate::ErrorCode::InvalidState)
            );
        }
        assert_eq!(
            (
                fs::read(&destination).expect("payload retained"),
                fs::read(&index_path).expect("index retained"),
                fs::read(&prepared_path).expect("prepared retained"),
            ),
            before
        );
        assert!(!reference.state_root().join("transaction.json").exists());
    }

    #[test]
    fn recovery_is_explicit_and_preserves_pending_membership() {
        let root = checkout_temp_dir("lifecycle-recovery");
        let destination = root.join("AGENTS.md");
        let target = InstallationTarget::without_context(root.join("index")).expect("target");
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
        let target = InstallationTarget::without_context(destination.clone()).expect("target");
        let reference = ResourceRef::new(
            destination,
            ResourceKind::Document,
            authority.state_root().to_path_buf(),
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
        fs::create_dir_all(target.index_root()).expect("index root");
        fs::set_permissions(target.index_root(), fs::Permissions::from_mode(0o700))
            .expect("private index root");
        fs::write(
            target.index_root().join("index.json"),
            crate::codec::encode(&index),
        )
        .expect("index");
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
            assert!(result.resources().is_empty());
            assert_eq!(
                result.error().map(TargetError::code),
                Some(crate::ErrorCode::InvalidState)
            );
            assert_eq!(
                result.error().map(TargetError::message),
                Some("installation index overlaps a resource or authority")
            );
            assert_eq!(result.error().and_then(TargetError::path), None);
            assert_eq!(filesystem_snapshot(&root), before);
            assert!(!reference.state_root().exists());
        }
    }

    #[test]
    fn preview_binds_receipts_security_ancestors_and_index_state() {
        let root = checkout_temp_dir("lifecycle-bindings");
        let destination = root.join("project/AGENTS.md");
        fs::create_dir(destination.parent().expect("parent")).expect("project");
        fs::write(&destination, b"# foreign\n").expect("foreign");
        let target = InstallationTarget::without_context(root.join("index")).expect("target");
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
        let receipt_path = receipt_preview.resources()[0]
            .resource()
            .state_root()
            .join("receipt.json");
        let bytes = fs::read(&receipt_path).expect("receipt");
        fs::rename(&receipt_path, receipt_path.with_extension("saved")).expect("move receipt");
        fs::write(&receipt_path, &bytes).expect("replace receipt");
        assert_eq!(
            apply_preview(&receipt_preview, Duration::ZERO)
                .error()
                .map(TargetError::code),
            Some(crate::ErrorCode::ConcurrentChange)
        );
        fs::remove_file(&receipt_path).expect("remove replacement");
        fs::rename(receipt_path.with_extension("saved"), &receipt_path).expect("restore receipt");

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
            root.join("index"),
            [(String::from("surface"), String::from("application"))],
            [(String::from("agent"), String::from("claude"))],
        )
        .expect("target");
        let second = InstallationTarget::new(
            root.join("index"),
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
        let target = InstallationTarget::without_context(root.join("index")).expect("target");
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
        let target = InstallationTarget::without_context(root.join("index")).expect("target");
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
        let target = InstallationTarget::without_context(root.join("index")).expect("target");
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
            let target = InstallationTarget::without_context(root.join("index")).expect("target");
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
            let installed: crate::Receipt =
                crate::resource_io::read(&authority.state_root().join("receipt.json"), "Receipt")
                    .expect("receipt read")
                    .expect("receipt");
            assert_eq!(installed.claims().len(), 2, "run {run}");
        }
    }

    #[test]
    fn corrupt_resource_state_never_authorizes_payload_changes() {
        for damage in ["receipt", "unknown-entry"] {
            let root = checkout_temp_dir(&format!("lifecycle-corrupt-resource-{damage}"));
            let destination = root.join("AGENTS.md");
            let target = InstallationTarget::without_context(root.join("index")).expect("target");
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
            let state_root = proposal.resources()[0].resource().state_root();
            if damage == "receipt" {
                fs::write(state_root.join("receipt.json"), b"{}").expect("corrupt receipt");
            } else {
                fs::write(state_root.join("future"), b"unknown").expect("unknown state");
            }
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
    fn unrecognized_prepared_index_does_not_admit_resources() {
        let root = checkout_temp_dir("lifecycle-invalid-prepared");
        let destination = root.join("AGENTS.md");
        let target = InstallationTarget::without_context(root.join("index")).expect("target");
        let desired = bundle("1");
        let rendered = section(&destination, "generated");
        let proposal =
            preview(&desired, &rendered, &target, Acquisition::Conflict, false).expect("preview");
        let generation = proposal.generation().expect("generation").clone();
        let invalid = Index::new(
            "team".to_owned(),
            target.context_digest(),
            Some(generation),
            None,
            None,
            Vec::new(),
            2,
        )
        .expect("index");
        fs::create_dir(target.index_root()).expect("index root");
        fs::set_permissions(target.index_root(), fs::Permissions::from_mode(0o700))
            .expect("private index root");
        fs::write(
            target.index_root().join("index.json.next"),
            crate::codec::encode(&invalid),
        )
        .expect("prepared index");
        let state_root = ResourceAuthority::new(&destination)
            .expect("authority")
            .state_root()
            .to_path_buf();

        let result = sync(
            &desired,
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
            Some(crate::ErrorCode::RecoveryNeeded)
        );
        assert!(!state_root.exists());
        assert!(!destination.exists());
    }

    #[test]
    fn malformed_prepared_index_does_not_admit_resources() {
        let root = checkout_temp_dir("lifecycle-malformed-prepared");
        let destination = root.join("AGENTS.md");
        let target = InstallationTarget::without_context(root.join("index")).expect("target");
        fs::create_dir(target.index_root()).expect("index root");
        fs::set_permissions(target.index_root(), fs::Permissions::from_mode(0o700))
            .expect("private index root");
        fs::write(target.index_root().join("index.json.next"), b"{}").expect("prepared index");
        let state_root = ResourceAuthority::new(&destination)
            .expect("authority")
            .state_root()
            .to_path_buf();

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
        assert!(!state_root.exists());
        assert!(!destination.exists());
    }

    #[test]
    fn ordinary_plan_failures_return_empty_notices() {
        let root = checkout_temp_dir("lifecycle-failure-notices");
        let destination = root.join("owned");
        fs::write(&destination, b"foreign").expect("foreign");
        let target = InstallationTarget::without_context(root.join("index")).expect("target");
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
        assert!(applied.notices().is_empty());

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
        assert!(synchronized.notices().is_empty());
    }

    #[test]
    fn public_lifecycle_entry_points_validate_bundle_ids_before_io() {
        let root = checkout_temp_dir("lifecycle-public-validation");
        let target = InstallationTarget::without_context(root.join("index")).expect("target");

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
        assert!(!target.index_root().exists());
    }

    #[test]
    fn installation_targets_validate_context_before_io() {
        let root = checkout_temp_dir("lifecycle-target-validation");
        let error = InstallationTarget::new(
            root.join("index"),
            [
                (String::from("surface"), String::from("application")),
                (String::from("surface"), String::from("project")),
            ],
            std::iter::empty(),
        )
        .expect_err("duplicate context");
        assert_eq!(error.to_string(), "target context keys must be unique");
        assert!(!root.join("index").exists());
    }
}
