#![allow(clippy::redundant_pub_crate)]

use crate::bundle::Bundle;
use crate::configuration::{
    ClaimObservation, InstallationObservation, InstallationResult, InstallationTarget,
    LifecyclePreview, ResourceObservation, ResourcePlan, ResourceResult,
};
use crate::editors::{Acquisition, EditStatus};
use crate::models::OperationStatus;
use crate::observation::failure;
use crate::observations::{ErrorCode, TargetError};
use crate::rendered::{DependencyMode, RenderedArtifact, RenderedBundle};
use crate::resource_io::{ancestors, observe, read, require, validate_state};
use crate::resource_models::{Ancestor, Generation, Index, Receipt, ResourceRef, Revision};
use crate::resource_plan::{
    artifact_claim, completed_index, expects_receipt, index_with_prepared_resources, pending_index,
    plan_resource, prepared_index_promotes, resource, summary,
};
use crate::resource_transaction::{
    TransactionLocks, execute_transaction, reconcile_prepared_index, recover,
};
use crate::validation::portable_path_key;
use crate::values::ValueError;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

enum BuildError {
    Invalid(ValueError),
    Target(TargetError),
}

impl From<ValueError> for BuildError {
    fn from(error: ValueError) -> Self {
        Self::Invalid(error)
    }
}

impl From<TargetError> for BuildError {
    fn from(error: TargetError) -> Self {
        Self::Target(error)
    }
}

#[allow(clippy::needless_pass_by_value)]
fn invalid(error: ValueError, path: &Path) -> TargetError {
    failure(ErrorCode::InvalidState, error.message(), path)
}

fn invalid_state(error: &ValueError) -> TargetError {
    TargetError::new(ErrorCode::InvalidState, error.message(), None, None)
}

fn index_records(
    identifier: &str,
    target: &InstallationTarget,
) -> Result<(Option<Index>, Option<Index>), TargetError> {
    let revision = observe(target.index_root())?;
    for node in revision
        .nodes()
        .iter()
        .filter(|node| !node.path().is_empty() && !node.path().contains('/'))
    {
        if !matches!(node.path(), "lock" | "index.json" | "index.json.next") {
            return Err(failure(
                ErrorCode::InvalidState,
                "unknown installation index entry",
                target.index_root().join(node.path()),
            ));
        }
        if node.data().is_none() {
            return Err(failure(
                ErrorCode::InvalidState,
                "expected a regular file",
                target.index_root().join(node.path()),
            ));
        }
    }
    require(target.index_root(), &revision)?;
    let current: Option<Index> = read(&target.index_root().join("index.json"), "Index")?;
    let prepared: Option<Index> = read(&target.index_root().join("index.json.next"), "Index")?;
    let digest = target.context_digest();
    for value in [&current, &prepared].into_iter().flatten() {
        if value.bundle_id() != identifier || value.context_digest() != digest {
            return Err(failure(
                ErrorCode::InvalidState,
                "installation index context disagrees",
                target.index_root(),
            ));
        }
    }
    Ok((current, prepared))
}

fn installation_index(
    identifier: &str,
    target: &InstallationTarget,
) -> Result<(Option<Index>, Option<Index>), TargetError> {
    let (current, prepared) = index_records(identifier, target)?;
    let visible = index_with_prepared_resources(
        current.as_ref(),
        prepared.as_ref(),
        identifier,
        &target.context_digest(),
    )
    .map_err(|error| invalid(error, target.index_root()))?;
    Ok((visible, prepared))
}

fn missing_receipt(reference: &ResourceRef) -> TargetError {
    failure(
        ErrorCode::InvalidState,
        "recorded ownership receipt is missing; restore metadata before retrying",
        reference.state_root().join("receipt.json"),
    )
}

fn check_receipt(
    index: Option<&Index>,
    prepared: Option<&Index>,
    reference: &ResourceRef,
) -> Result<(), TargetError> {
    if expects_receipt(index, prepared, reference)
        && read::<Receipt>(&reference.state_root().join("receipt.json"), "Receipt")?.is_none()
    {
        return Err(missing_receipt(reference));
    }
    Ok(())
}

fn path_overlap(left: &Path, right: &Path) -> bool {
    let first = left
        .components()
        .map(|part| portable_path_key(&part.as_os_str().to_string_lossy()))
        .collect::<Vec<_>>();
    let second = right
        .components()
        .map(|part| portable_path_key(&part.as_os_str().to_string_lossy()))
        .collect::<Vec<_>>();
    first.iter().zip(&second).all(|(left, right)| left == right)
}

fn references(
    rendered: &RenderedBundle,
    index: Option<&Index>,
    target: &InstallationTarget,
    bundle: Option<&Bundle>,
    resolved: Option<&HashMap<String, ResourceRef>>,
) -> Result<Vec<ResourceRef>, BuildError> {
    for source in bundle.into_iter().flat_map(Bundle::source_roots) {
        if path_overlap(source, target.index_root()) {
            return Err(ValueError::new("installation index overlaps bundle source").into());
        }
    }
    let owned;
    let resolved = if let Some(resolved) = resolved {
        resolved
    } else {
        owned = rendered
            .artifacts()
            .iter()
            .map(|artifact| {
                resource(artifact)
                    .map(|reference| (artifact.id().to_owned(), reference))
                    .map_err(BuildError::Target)
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        &owned
    };
    let mut outputs: HashMap<ResourceRef, Vec<PathBuf>> = HashMap::new();
    for artifact in rendered.artifacts() {
        let reference = resolved
            .get(artifact.id())
            .ok_or_else(|| ValueError::new("rendered artifact reference is missing"))?;
        outputs
            .entry(reference.clone())
            .or_default()
            .push(artifact.subtree().map_or_else(
                || reference.destination().to_path_buf(),
                |subtree| reference.destination().join(subtree),
            ));
    }
    let mut refs = resolved.values().cloned().collect::<Vec<_>>();
    refs.extend(index.into_iter().flat_map(Index::resources).cloned());
    crate::resource_plan::state_root_order(&mut refs);
    refs.dedup();
    let mut destinations = HashSet::with_capacity(refs.len());
    if !refs
        .iter()
        .all(|reference| destinations.insert(reference.destination()))
    {
        return Err(ValueError::new("resource kinds disagree at the same destination").into());
    }
    for reference in &refs {
        if path_overlap(reference.destination(), target.index_root())
            || path_overlap(reference.state_root(), target.index_root())
        {
            return Err(
                ValueError::new("installation index overlaps a resource or authority").into(),
            );
        }
        for source in bundle.into_iter().flat_map(Bundle::source_roots) {
            if path_overlap(source, reference.state_root()) {
                return Err(ValueError::new("resource authority overlaps bundle source").into());
            }
            if outputs
                .get(reference)
                .into_iter()
                .flatten()
                .any(|output| path_overlap(source, output))
            {
                return Err(ValueError::new("resource output overlaps bundle source").into());
            }
        }
    }
    Ok(refs)
}

#[allow(clippy::too_many_lines)]
fn order_plans(
    plans: Vec<ResourcePlan>,
    rendered: &RenderedBundle,
    by_id: &HashMap<String, ResourceRef>,
) -> Result<Vec<ResourcePlan>, ValueError> {
    let refs = plans
        .iter()
        .map(|plan| {
            (
                plan.resource().destination().to_path_buf(),
                plan.resource().clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut edges = HashSet::new();
    for edge in rendered.dependencies() {
        let dependent = by_id
            .get(edge.dependent())
            .ok_or_else(|| ValueError::new("planned artifact reference is missing"))?;
        let required = by_id
            .get(edge.required())
            .ok_or_else(|| ValueError::new("planned artifact reference is missing"))?;
        if dependent != required {
            edges.insert((dependent.clone(), required.clone()));
        }
    }
    for plan in &plans {
        for old in plan.previous().into_iter().flat_map(Receipt::claims) {
            let new = plan
                .receipt()
                .claims()
                .iter()
                .find(|item| item.claim_id() == old.claim_id());
            for (path, claim_id) in old.requirements() {
                let Some(required) = refs.get(path) else {
                    continue;
                };
                if required == plan.resource() {
                    continue;
                }
                let required_plan = plans
                    .iter()
                    .find(|item| item.resource() == required)
                    .ok_or_else(|| ValueError::new("planned dependency resource is missing"))?;
                let old_asset = required_plan
                    .previous()
                    .into_iter()
                    .flat_map(Receipt::claims)
                    .find(|item| item.claim_id() == claim_id);
                let new_asset = required_plan
                    .receipt()
                    .claims()
                    .iter()
                    .find(|item| item.claim_id() == claim_id);
                if old_asset.is_some()
                    && new_asset.is_some()
                    && old_asset.and_then(crate::resource_models::OwnedClaim::installed)
                        != new_asset.and_then(crate::resource_models::OwnedClaim::installed)
                    && !old
                        .stable_requirements()
                        .contains(&(path.clone(), claim_id.clone()))
                {
                    return Err(ValueError::new(
                        "referenced asset changes require a distinct revision destination",
                    ));
                }
                if old_asset.is_some() && new_asset.is_none() {
                    if new.is_some_and(|item| {
                        item.requirements()
                            .contains(&(path.clone(), claim_id.clone()))
                    }) {
                        return Err(ValueError::new(
                            "cannot retire an asset with an active reference",
                        ));
                    }
                    edges.insert((required.clone(), plan.resource().clone()));
                }
            }
        }
    }
    let mut pending = plans
        .into_iter()
        .map(|plan| (plan.resource().clone(), plan))
        .collect::<HashMap<_, _>>();
    let mut ordered = Vec::with_capacity(pending.len());
    while !pending.is_empty() {
        let mut ready = pending
            .keys()
            .filter(|reference| {
                !edges
                    .iter()
                    .any(|(left, right)| left == *reference && pending.contains_key(right))
            })
            .cloned()
            .collect::<Vec<_>>();
        crate::resource_plan::state_root_order(&mut ready);
        if ready.is_empty() {
            return Err(ValueError::new("cyclic resource dependencies"));
        }
        for reference in ready {
            if let Some(plan) = pending.remove(&reference) {
                ordered.push(plan);
            }
        }
    }
    Ok(ordered)
}

#[allow(clippy::too_many_lines)]
fn build_preview(
    identifier: &str,
    bundle: Option<&Bundle>,
    rendered: &RenderedBundle,
    target: &InstallationTarget,
    acquisition: Acquisition,
    replace_modified: bool,
) -> Result<LifecyclePreview, BuildError> {
    let (index, prepared) = installation_index(identifier, target)?;
    let resolved = rendered
        .artifacts()
        .iter()
        .map(|artifact| {
            resource(artifact)
                .map(|reference| (artifact.id().to_owned(), reference))
                .map_err(BuildError::Target)
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    let refs = references(rendered, index.as_ref(), target, bundle, Some(&resolved))?;
    let by_artifact = rendered
        .artifacts()
        .iter()
        .map(|artifact| (artifact.id(), artifact))
        .collect::<HashMap<_, _>>();
    let mut requirements = rendered
        .artifacts()
        .iter()
        .map(|artifact| (artifact.id().to_owned(), HashSet::new()))
        .collect::<HashMap<_, HashSet<(PathBuf, String)>>>();
    let mut stable = requirements.clone();
    for edge in rendered.dependencies() {
        let required = resolved
            .get(edge.required())
            .ok_or_else(|| ValueError::new("planned dependency reference is missing"))?;
        let artifact = by_artifact
            .get(edge.required())
            .ok_or_else(|| ValueError::new("planned dependency artifact is missing"))?;
        let claim_id = artifact_claim(artifact)?.identity(&required.recorded_authority())?;
        let pair = (required.destination().to_path_buf(), claim_id);
        requirements
            .get_mut(edge.dependent())
            .ok_or_else(|| ValueError::new("planned artifact requirements are missing"))?
            .insert(pair.clone());
        if edge.mode() == DependencyMode::StableReference {
            stable
                .get_mut(edge.dependent())
                .ok_or_else(|| ValueError::new("planned artifact requirements are missing"))?
                .insert(pair);
        }
    }
    let mut grouped: HashMap<(ResourceRef, crate::authority::Claim), RenderedArtifact> =
        HashMap::new();
    for artifact in rendered.artifacts() {
        let reference = resolved
            .get(artifact.id())
            .ok_or_else(|| ValueError::new("planned artifact reference is missing"))?;
        let claim = artifact_claim(artifact)?;
        let key = (reference.clone(), claim);
        if let Some(previous) = grouped.get(&key) {
            if previous.content() != artifact.content()
                || previous.family() != artifact.family()
                || previous.schema_requirements() != artifact.schema_requirements()
            {
                return Err(
                    ValueError::new("physical aliases have incompatible desired content").into(),
                );
            }
            if stable.get(previous.id()) != stable.get(artifact.id()) {
                return Err(ValueError::new(
                    "physical aliases have incompatible stable-reference requirements",
                )
                .into());
            }
            let aliases = requirements
                .get(artifact.id())
                .cloned()
                .ok_or_else(|| ValueError::new("planned artifact requirements are missing"))?;
            requirements
                .get_mut(previous.id())
                .ok_or_else(|| ValueError::new("planned artifact requirements are missing"))?
                .extend(aliases);
            if artifact.require_existing() && !previous.require_existing() {
                grouped.insert(
                    key,
                    RenderedArtifact::new(
                        previous.id(),
                        previous.family(),
                        previous.destination().to_path_buf(),
                        previous.content().clone(),
                        previous.subtree().map(str::to_owned),
                        true,
                        previous.schema_requirements().to_vec(),
                    )?,
                );
            }
        } else {
            grouped.insert(key, artifact.clone());
        }
    }
    let mut selected: HashMap<ResourceRef, Vec<RenderedArtifact>> = HashMap::new();
    for ((reference, _), artifact) in grouped {
        selected.entry(reference).or_default().push(artifact);
    }
    for artifacts in selected.values_mut() {
        artifacts.sort_by(|left, right| left.id().cmp(right.id()));
    }
    let normalized = requirements
        .into_iter()
        .map(|(key, values)| {
            let mut values = values.into_iter().collect::<Vec<_>>();
            values.sort();
            (key, values)
        })
        .collect::<HashMap<_, _>>();
    let stable = stable
        .into_iter()
        .map(|(key, values)| {
            let mut values = values.into_iter().collect::<Vec<_>>();
            values.sort();
            (key, values)
        })
        .collect::<HashMap<_, _>>();
    let mut plans = refs
        .iter()
        .map(|reference| {
            plan_resource(
                reference,
                identifier,
                bundle,
                rendered,
                selected.get(reference).map_or(&[], Vec::as_slice),
                acquisition,
                replace_modified,
                &normalized,
                &stable,
            )
            .map(|plan| {
                if plan.previous().is_none()
                    && expects_receipt(index.as_ref(), prepared.as_ref(), reference)
                {
                    plan.with_error(missing_receipt(reference))
                } else {
                    plan
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !plans.iter().any(|plan| plan.error().is_some()) {
        plans = order_plans(plans, rendered, &resolved)?;
    }
    let generation = if let Some(bundle) = bundle {
        let mut membership = Vec::new();
        for reference in &refs {
            if let Some(artifacts) = selected.get(reference) {
                let mut claims = artifacts
                    .iter()
                    .map(|artifact| {
                        artifact_claim(artifact)
                            .and_then(|claim| claim.identity(&reference.recorded_authority()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                claims.sort();
                membership.push((reference.clone(), claims));
            }
        }
        Some(Generation::new(
            bundle.version().to_owned(),
            bundle.content_digest().to_owned(),
            rendered.content_digest().to_owned(),
            membership,
        )?)
    } else {
        None
    };
    let mut error = (!rendered.supported()).then(|| {
        TargetError::new(
            ErrorCode::Unsupported,
            "rendered target has unsupported requirements",
            None,
            None,
        )
    });
    let prepared_path = target.index_root().join("index.json.next");
    if fs::symlink_metadata(&prepared_path).is_ok() {
        error = Some(failure(
            ErrorCode::RecoveryNeeded,
            "installation index has prepared metadata",
            prepared_path,
        ));
    }
    Ok(LifecyclePreview::new(
        identifier.to_owned(),
        bundle.cloned(),
        rendered.clone(),
        target.clone(),
        acquisition,
        replace_modified,
        index,
        Some(observe(target.index_root())?),
        ancestors(&target.index_root().join("index.json"))?,
        generation,
        plans,
        error,
    ))
}

pub(crate) fn public_preview(
    identifier: &str,
    bundle: Option<&Bundle>,
    rendered: &RenderedBundle,
    target: &InstallationTarget,
    acquisition: Acquisition,
    replace_modified: bool,
) -> Result<LifecyclePreview, ValueError> {
    match build_preview(
        identifier,
        bundle,
        rendered,
        target,
        acquisition,
        replace_modified,
    ) {
        Ok(preview) => Ok(preview),
        Err(BuildError::Invalid(error)) => Err(error),
        Err(BuildError::Target(error)) => Ok(LifecyclePreview::new(
            identifier.to_owned(),
            bundle.cloned(),
            rendered.clone(),
            target.clone(),
            acquisition,
            replace_modified,
            None,
            None,
            Vec::new(),
            None,
            Vec::new(),
            Some(error),
        )),
    }
}

fn public_summary(reference: &ResourceRef) -> Result<ResourceObservation, TargetError> {
    let observed = summary(reference)?;
    Ok(ResourceObservation::observed(
        observed.destination().to_path_buf(),
        observed.state_root().to_path_buf(),
        observed
            .claims()
            .iter()
            .map(|claim| {
                ClaimObservation::new(
                    claim.claim_id().to_owned(),
                    claim.bundle_id().to_owned(),
                    claim.artifact_id().to_owned(),
                    claim.version().to_owned(),
                    claim.bundle_digest().to_owned(),
                    claim.render_digest().to_owned(),
                    claim.status(),
                )
            })
            .collect(),
        observed.content_digest().to_owned(),
        observed.recovery_paths().to_vec(),
    ))
}

pub(crate) fn inspect(bundle_id: &str, target: &InstallationTarget) -> InstallationObservation {
    let mut resources = Vec::new();
    let result = (|| {
        let (index, prepared) = installation_index(bundle_id, target)?;
        for reference in index.as_ref().into_iter().flat_map(Index::resources) {
            let item = check_receipt(index.as_ref(), prepared.as_ref(), reference)
                .and_then(|()| public_summary(reference));
            resources.push(match item {
                Ok(item) => item,
                Err(error) => ResourceObservation::failed(reference, error),
            });
        }
        let current = index.as_ref().and_then(Index::current);
        let prepared_exists =
            fs::symlink_metadata(target.index_root().join("index.json.next")).is_ok();
        let pending = prepared_exists
            || index.as_ref().is_some_and(|index| {
                index.pending().is_some()
                    || index.previous().is_some()
                    || !index.residual().is_empty()
            });
        let actual = resources
            .iter()
            .flat_map(|resource| {
                resource
                    .claims()
                    .iter()
                    .filter(|claim| claim.bundle_id() == bundle_id)
                    .map(|claim| {
                        (
                            resource.destination().to_path_buf(),
                            claim.claim_id().to_owned(),
                        )
                    })
            })
            .collect::<HashSet<_>>();
        let expected = current
            .into_iter()
            .flat_map(Generation::membership)
            .flat_map(|(reference, claims)| {
                claims
                    .iter()
                    .cloned()
                    .map(|claim| (reference.destination().to_path_buf(), claim))
            })
            .collect::<HashSet<_>>();
        let intact = resources.iter().all(|resource| {
            resource.error().is_none()
                && resource.recovery_paths().is_empty()
                && resource
                    .claims()
                    .iter()
                    .filter(|claim| claim.bundle_id() == bundle_id)
                    .all(|claim| {
                        current.is_some_and(|generation| {
                            claim.version() == generation.version()
                                && claim.bundle_digest() == generation.bundle_digest()
                                && claim.render_digest() == generation.render_digest()
                                && claim.status() == EditStatus::Current
                        })
                    })
        });
        Ok::<_, TargetError>(InstallationObservation::new(
            bundle_id.to_owned(),
            target.clone(),
            resources.clone(),
            current.is_some() && !pending && intact && actual == expected,
            pending,
            current.map(|value| value.version().to_owned()),
            current.map(|value| value.bundle_digest().to_owned()),
            current.map(|value| value.render_digest().to_owned()),
            None,
        ))
    })();
    result.unwrap_or_else(|error| {
        InstallationObservation::new(
            bundle_id.to_owned(),
            target.clone(),
            resources,
            false,
            false,
            None,
            None,
            None,
            Some(error),
        )
    })
}

fn state_matches(path: &Path, expected: &Revision, initial: bool) -> Result<Revision, TargetError> {
    let current = observe(path)?;
    if initial {
        if &current != expected {
            return Err(failure(
                ErrorCode::ConcurrentChange,
                "preview metadata changed",
                path,
            ));
        }
        return Ok(current);
    }
    let expected_nodes = expected
        .nodes()
        .iter()
        .map(|node| (node.path(), node))
        .collect::<HashMap<_, _>>();
    let current_nodes = current
        .nodes()
        .iter()
        .map(|node| (node.path(), node))
        .collect::<HashMap<_, _>>();
    if expected_nodes
        .iter()
        .any(|(key, value)| current_nodes.get(key).copied() != Some(*value))
        || current_nodes
            .keys()
            .filter(|key| !expected_nodes.contains_key(*key))
            .any(|key| !matches!(*key, "" | "lock" | "authority.json"))
    {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "preview metadata changed",
            path,
        ));
    }
    Ok(current)
}

fn checked_ancestors(
    path: &Path,
    expected: &[Ancestor],
    created: &[Ancestor],
) -> Result<Vec<Ancestor>, TargetError> {
    let current = ancestors(path)?;
    let mut approved = expected.to_vec();
    let additions = created
        .iter()
        .filter(|item| path.ancestors().any(|parent| parent == item.path()))
        .filter(|item| !approved.contains(item))
        .cloned()
        .collect::<Vec<_>>();
    approved.extend(additions);
    approved.sort_by_key(|item| item.path().components().count());
    if current != approved {
        return Err(failure(
            ErrorCode::ConcurrentChange,
            "preview ancestor changed",
            path,
        ));
    }
    Ok(approved)
}

fn fresh(
    preview: &LifecyclePreview,
    initial: bool,
    created: &[Ancestor],
) -> Result<LifecyclePreview, TargetError> {
    let Some(expected_index) = preview.index_revision() else {
        return Err(failure(
            ErrorCode::InvalidState,
            "preview metadata was not observed",
            preview.target().index_root(),
        ));
    };
    let index_revision = state_matches(preview.target().index_root(), expected_index, initial)?;
    let index_ancestors = checked_ancestors(
        &preview.target().index_root().join("index.json"),
        preview.index_ancestors(),
        created,
    )?;
    let resources = preview
        .resources()
        .iter()
        .map(|plan| {
            require(plan.resource().destination(), plan.before())?;
            let state_revision =
                state_matches(plan.resource().state_root(), plan.state_revision(), initial)?;
            let ancestors = checked_ancestors(
                &plan.resource().state_root().join("transaction.json"),
                plan.ancestors(),
                created,
            )?;
            Ok(plan.with_observations(state_revision, ancestors))
        })
        .collect::<Result<Vec<_>, TargetError>>()?;
    Ok(preview.with_observations(index_revision, index_ancestors, resources))
}

fn failed_result(
    identifier: &str,
    target: &InstallationTarget,
    error: TargetError,
    notices: Vec<crate::rendered::Notice>,
) -> InstallationResult {
    InstallationResult::new(
        OperationStatus::Failed,
        inspect(identifier, target),
        Vec::new(),
        Some(error),
        notices,
    )
}

fn recovery_paths(reference: &ResourceRef) -> Vec<PathBuf> {
    validate_state(reference).unwrap_or_else(|_| vec![reference.state_root().to_path_buf()])
}

fn acquire_transaction_locks(
    identifier: &str,
    target: &InstallationTarget,
    expected_index: Option<&Index>,
    expected_prepared: Option<&Index>,
    references: &[ResourceRef],
    timeout: Duration,
) -> Result<TransactionLocks, TargetError> {
    TransactionLocks::acquire_after_index_validation(
        target.index_root(),
        references,
        timeout,
        || {
            let (recorded, prepared) = index_records(identifier, target)?;
            if let Some(prepared) = prepared.as_ref() {
                prepared_index_promotes(recorded.as_ref(), prepared).map_err(|error| {
                    failure(
                        ErrorCode::RecoveryNeeded,
                        error.message(),
                        target.index_root().join("index.json.next"),
                    )
                })?;
            }
            let visible = index_with_prepared_resources(
                recorded.as_ref(),
                prepared.as_ref(),
                identifier,
                &target.context_digest(),
            )
            .map_err(|error| invalid(error, target.index_root()))?;
            if visible.as_ref() != expected_index || prepared.as_ref() != expected_prepared {
                return Err(failure(
                    ErrorCode::ConcurrentChange,
                    "index changed while acquiring locks",
                    target.index_root(),
                ));
            }
            Ok(())
        },
    )
}

#[allow(clippy::too_many_lines)]
fn run_locked(preview: &LifecyclePreview, locks: &TransactionLocks) -> InstallationResult {
    let identifier = preview.bundle_id();
    let target = preview.target();
    let initial_error = preview.error().cloned().or_else(|| {
        preview
            .resources()
            .iter()
            .find_map(|plan| plan.error().cloned())
    });
    if let Some(error) = initial_error {
        return failed_result(identifier, target, error, Vec::new());
    }
    if preview
        .resources()
        .iter()
        .any(|plan| !plan.recovery_paths().is_empty())
    {
        return failed_result(
            identifier,
            target,
            TargetError::new(
                ErrorCode::RecoveryNeeded,
                "preview requires recovery and re-observation",
                None,
                None,
            ),
            Vec::new(),
        );
    }
    let refreshed = match fresh(preview, false, locks.created()) {
        Ok(value) => value,
        Err(error) => {
            return failed_result(identifier, target, error, Vec::new());
        }
    };
    let unchanged = refreshed
        .resources()
        .iter()
        .all(|plan| plan.before() == plan.after() && plan.previous() == Some(plan.receipt()));
    if unchanged
        && refreshed.index().is_some_and(|index| {
            index.current() == refreshed.generation()
                && index.pending().is_none()
                && index.residual().is_empty()
        })
    {
        return InstallationResult::new(
            OperationStatus::Unchanged,
            inspect(identifier, target),
            Vec::new(),
            None,
            refreshed.rendered().notices().to_vec(),
        );
    }
    if unchanged && refreshed.index().is_none() && refreshed.bundle().is_none() {
        return InstallationResult::new(
            OperationStatus::Unchanged,
            inspect(identifier, target),
            Vec::new(),
            None,
            Vec::new(),
        );
    }
    let pending = match pending_index(
        identifier,
        &target.context_digest(),
        refreshed.index(),
        refreshed.generation().cloned(),
        refreshed
            .resources()
            .iter()
            .map(|plan| plan.resource().clone())
            .collect(),
    ) {
        Ok(value) => value,
        Err(error) => {
            return failed_result(
                identifier,
                target,
                invalid(error, target.index_root()),
                Vec::new(),
            );
        }
    };
    let completed = match completed_index(
        identifier,
        &target.context_digest(),
        refreshed.generation().cloned(),
    ) {
        Ok(value) => value,
        Err(error) => {
            return failed_result(
                identifier,
                target,
                invalid(error, target.index_root()),
                Vec::new(),
            );
        }
    };
    let Some(index_revision) = refreshed.index_revision() else {
        return failed_result(
            identifier,
            target,
            failure(
                ErrorCode::InvalidState,
                "preview metadata was not observed",
                target.index_root(),
            ),
            Vec::new(),
        );
    };
    let outcome = match execute_transaction(
        locks,
        refreshed.resources(),
        &pending,
        &completed,
        index_revision,
        refreshed.index_ancestors(),
    ) {
        Ok(value) => value,
        Err(error) => {
            return failed_result(identifier, target, error, Vec::new());
        }
    };
    let completed_refs = outcome.completed().iter().cloned().collect::<HashSet<_>>();
    let unchanged_refs = outcome.unchanged().iter().cloned().collect::<HashSet<_>>();
    let mut results = Vec::new();
    for plan in refreshed.resources() {
        if outcome.failed() == Some(plan.resource()) {
            let paths = recovery_paths(plan.resource());
            let committed = completed_refs.contains(plan.resource());
            results.push(ResourceResult::new(
                plan.resource().clone(),
                if committed {
                    OperationStatus::Applied
                } else if paths.is_empty() {
                    OperationStatus::Failed
                } else {
                    OperationStatus::Incomplete
                },
                outcome.error().cloned(),
                paths,
            ));
            break;
        } else if completed_refs.contains(plan.resource()) {
            results.push(ResourceResult::new(
                plan.resource().clone(),
                OperationStatus::Applied,
                None,
                Vec::new(),
            ));
        } else if unchanged_refs.contains(plan.resource()) {
            results.push(ResourceResult::new(
                plan.resource().clone(),
                OperationStatus::Unchanged,
                None,
                Vec::new(),
            ));
        }
    }
    let status = if outcome.error().is_none() {
        OperationStatus::Applied
    } else if !completed_refs.is_empty() {
        OperationStatus::Partial
    } else if results
        .iter()
        .any(|result| result.status() == OperationStatus::Incomplete)
    {
        OperationStatus::Incomplete
    } else {
        OperationStatus::Failed
    };
    InstallationResult::new(
        status,
        inspect(identifier, target),
        results,
        outcome.error().cloned(),
        refreshed.rendered().notices().to_vec(),
    )
}

pub(crate) fn apply(preview: &LifecyclePreview, timeout: Duration) -> InstallationResult {
    if let Some(error) = preview.error() {
        return failed_result(
            preview.bundle_id(),
            preview.target(),
            error.clone(),
            preview.rendered().notices().to_vec(),
        );
    }
    if !preview.rendered().supported() {
        return unsupported_result(preview.bundle_id(), preview.rendered(), preview.target());
    }
    if let Err(error) = fresh(preview, true, &[]) {
        return failed_result(preview.bundle_id(), preview.target(), error, Vec::new());
    }
    let references = preview
        .resources()
        .iter()
        .map(|plan| plan.resource().clone())
        .collect::<Vec<_>>();
    match acquire_transaction_locks(
        preview.bundle_id(),
        preview.target(),
        preview.index(),
        None,
        &references,
        timeout,
    ) {
        Ok(locks) => run_locked(preview, &locks),
        Err(error) => failed_result(preview.bundle_id(), preview.target(), error, Vec::new()),
    }
}

fn unsupported_result(
    identifier: &str,
    rendered: &RenderedBundle,
    target: &InstallationTarget,
) -> InstallationResult {
    failed_result(
        identifier,
        target,
        TargetError::new(
            ErrorCode::Unsupported,
            "rendered target has unsupported requirements",
            None,
            None,
        ),
        rendered.notices().to_vec(),
    )
}

#[allow(clippy::too_many_lines)]
fn recover_locked(
    identifier: &str,
    target: &InstallationTarget,
    index: Option<&Index>,
    prepared: Option<&Index>,
    references: &[ResourceRef],
    locks: &TransactionLocks,
) -> InstallationResult {
    let current = match installation_index(identifier, target) {
        Ok(value) => value,
        Err(error) => return failed_result(identifier, target, error, Vec::new()),
    };
    if current.0.as_ref() != index || current.1.as_ref() != prepared {
        return failed_result(
            identifier,
            target,
            failure(
                ErrorCode::ConcurrentChange,
                "index changed while acquiring locks",
                target.index_root(),
            ),
            Vec::new(),
        );
    }
    let (recorded, prepared_record) = match index_records(identifier, target) {
        Ok(value) => value,
        Err(error) => return failed_result(identifier, target, error, Vec::new()),
    };
    if let Some(prepared) = prepared_record.as_ref()
        && let Err(error) = prepared_index_promotes(recorded.as_ref(), prepared)
    {
        return failed_result(
            identifier,
            target,
            failure(
                ErrorCode::RecoveryNeeded,
                error.message(),
                target.index_root().join("index.json.next"),
            ),
            Vec::new(),
        );
    }
    for reference in references {
        if let Err(error) = check_receipt(index, prepared_record.as_ref(), reference) {
            return failed_result(identifier, target, error, Vec::new());
        }
    }
    let revision = match observe(target.index_root()) {
        Ok(value) => value,
        Err(error) => return failed_result(identifier, target, error, Vec::new()),
    };
    let parents = match ancestors(&target.index_root().join("index.json")) {
        Ok(value) => value,
        Err(error) => return failed_result(identifier, target, error, Vec::new()),
    };
    let mut results = Vec::new();
    for guard in locks.resources() {
        let outcome = validate_state(guard.reference())
            .map(|paths| !paths.is_empty())
            .and_then(|pending| recover(guard).map(|()| pending));
        let pending = match outcome {
            Ok(pending) => pending,
            Err(error) => {
                let paths = recovery_paths(guard.reference());
                results.push(ResourceResult::new(
                    guard.reference().clone(),
                    OperationStatus::Incomplete,
                    Some(error.clone()),
                    paths,
                ));
                return InstallationResult::new(
                    OperationStatus::Incomplete,
                    inspect(identifier, target),
                    results,
                    Some(error),
                    Vec::new(),
                );
            }
        };
        results.push(ResourceResult::new(
            guard.reference().clone(),
            if pending {
                OperationStatus::Applied
            } else {
                OperationStatus::Unchanged
            },
            None,
            Vec::new(),
        ));
    }
    if let Some(prepared) = prepared_record.as_ref()
        && let Err(error) =
            reconcile_prepared_index(locks, recorded.as_ref(), prepared, &revision, &parents)
    {
        return failed_result(identifier, target, error, Vec::new());
    }
    let changed = prepared_record.is_some()
        || results
            .iter()
            .any(|result| result.status() == OperationStatus::Applied);
    InstallationResult::new(
        if changed {
            OperationStatus::Applied
        } else {
            OperationStatus::Unchanged
        },
        inspect(identifier, target),
        results,
        None,
        Vec::new(),
    )
}

pub(crate) fn recover_installation(
    identifier: &str,
    target: &InstallationTarget,
    timeout: Duration,
) -> InstallationResult {
    let (index, prepared) = match installation_index(identifier, target) {
        Ok(value) => value,
        Err(error) => return failed_result(identifier, target, error, Vec::new()),
    };
    let Some(index_value) = index.as_ref() else {
        return InstallationResult::new(
            OperationStatus::Unchanged,
            inspect(identifier, target),
            Vec::new(),
            None,
            Vec::new(),
        );
    };
    let references = match references(
        &RenderedBundle::empty(),
        Some(index_value),
        target,
        None,
        None,
    ) {
        Ok(value) => value,
        Err(BuildError::Invalid(error)) => {
            return failed_result(identifier, target, invalid_state(&error), Vec::new());
        }
        Err(BuildError::Target(error)) => {
            return failed_result(identifier, target, error, Vec::new());
        }
    };
    match acquire_transaction_locks(
        identifier,
        target,
        index.as_ref(),
        prepared.as_ref(),
        &references,
        timeout,
    ) {
        Ok(locks) => recover_locked(
            identifier,
            target,
            index.as_ref(),
            prepared.as_ref(),
            &references,
            &locks,
        ),
        Err(error) => failed_result(identifier, target, error, Vec::new()),
    }
}

pub(crate) fn synchronize(
    identifier: &str,
    bundle: Option<&Bundle>,
    rendered: &RenderedBundle,
    target: &InstallationTarget,
    acquisition: Acquisition,
    replace_modified: bool,
    timeout: Duration,
) -> InstallationResult {
    if !rendered.supported() {
        return unsupported_result(identifier, rendered, target);
    }
    let (index, prepared) = match installation_index(identifier, target) {
        Ok(value) => value,
        Err(error) => return failed_result(identifier, target, error, Vec::new()),
    };
    let refs = match references(rendered, index.as_ref(), target, bundle, None) {
        Ok(value) => value,
        Err(BuildError::Invalid(error)) => {
            return failed_result(identifier, target, invalid_state(&error), Vec::new());
        }
        Err(BuildError::Target(error)) => {
            return failed_result(identifier, target, error, Vec::new());
        }
    };
    let locks = match acquire_transaction_locks(
        identifier,
        target,
        index.as_ref(),
        prepared.as_ref(),
        &refs,
        timeout,
    ) {
        Ok(value) => value,
        Err(error) => return failed_result(identifier, target, error, Vec::new()),
    };
    let recovered = recover_locked(
        identifier,
        target,
        index.as_ref(),
        prepared.as_ref(),
        &refs,
        &locks,
    );
    if recovered.error().is_some() {
        return recovered;
    }
    let preview = match build_preview(
        identifier,
        bundle,
        rendered,
        target,
        acquisition,
        replace_modified,
    ) {
        Ok(value) => value,
        Err(BuildError::Invalid(error)) => {
            return failed_result(identifier, target, invalid_state(&error), Vec::new());
        }
        Err(BuildError::Target(error)) => {
            return failed_result(identifier, target, error, Vec::new());
        }
    };
    run_locked(&preview, &locks)
}

pub(crate) fn validate_reference_request(
    rendered: &RenderedBundle,
    target: &InstallationTarget,
    bundle: &Bundle,
) -> Result<Option<TargetError>, ValueError> {
    match references(rendered, None, target, Some(bundle), None) {
        Ok(_) => Ok(None),
        Err(BuildError::Invalid(error)) => Err(error),
        Err(BuildError::Target(error)) => Ok(Some(error)),
    }
}
