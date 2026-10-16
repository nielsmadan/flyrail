#![allow(clippy::redundant_pub_crate)]

use crate::authority::{management_name, physical_location};
use crate::bundle::Bundle;
use crate::codec::encode;
use crate::configuration::{
    ClaimObservation, InstallationObservation, InstallationResult, InstallationTarget,
    LifecyclePreview, ResourceObservation, ResourcePlan, ResourceResult,
};
use crate::editors::{Acquisition, EditStatus};
use crate::models::OperationStatus;
use crate::observation::failure;
use crate::observations::{ErrorCode, TargetError};
use crate::rendered::{DependencyMode, RenderedArtifact, RenderedBundle};
use crate::resource_io::{error_detail, require, resource_ancestors};
use crate::resource_models::{Ancestor, Generation, Index, Preparation, Receipt, ResourceRef};
use crate::resource_plan::{
    artifact_claim, completed_index, expects_receipt, pending_index, plan_resource, resource,
    summary,
};
use crate::resource_transaction::{
    TransactionLocks, execute_transaction, pending_recovery, recover,
};
use crate::state::{index_key, read_record, require_index, resource_key, store_failure};
use crate::stores::StateStore;
use crate::validation::portable_path_key;
use crate::values::ValueError;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

enum BuildError {
    Invalid(ValueError),
    StoreOverlap(ValueError),
    Target(TargetError),
}

impl BuildError {
    fn into_value_error(self) -> Result<ValueError, TargetError> {
        match self {
            Self::Invalid(error) | Self::StoreOverlap(error) => Ok(error),
            Self::Target(error) => Err(error),
        }
    }
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

fn index_failure(target: &InstallationTarget, code: ErrorCode, message: &str) -> TargetError {
    TargetError::new(
        code,
        message,
        target.store().location().map(Path::to_path_buf),
        None,
    )
}

fn invalid_state(error: &ValueError) -> TargetError {
    TargetError::new(ErrorCode::InvalidState, error.message(), None, None)
}

fn installation_key(identifier: &str, target: &InstallationTarget) -> String {
    index_key(identifier, &target.context_digest())
}

fn installation_index(
    identifier: &str,
    target: &InstallationTarget,
) -> Result<Option<Index>, TargetError> {
    let index: Option<Index> = read_record(
        target.store().as_ref(),
        &installation_key(identifier, target),
        "Index",
    )?;
    if index.as_ref().is_some_and(|value| {
        value.bundle_id() != identifier || value.context_digest() != target.context_digest()
    }) {
        return Err(index_failure(
            target,
            ErrorCode::InvalidState,
            "installation index context disagrees",
        ));
    }
    Ok(index)
}

fn missing_receipt(reference: &ResourceRef) -> TargetError {
    failure(
        ErrorCode::InvalidState,
        "recorded ownership receipt is missing; restore metadata before retrying",
        reference.destination(),
    )
}

fn check_receipt(
    store: &dyn StateStore,
    index: Option<&Index>,
    reference: &ResourceRef,
) -> Result<(), TargetError> {
    if expects_receipt(index, reference)
        && read_record::<Receipt>(
            store,
            &resource_key(reference.destination(), "receipt"),
            "Receipt",
        )?
        .is_none()
    {
        return Err(missing_receipt(reference));
    }
    Ok(())
}

fn portable_parts(path: &Path) -> Vec<String> {
    path.components()
        .map(|part| portable_path_key(&part.as_os_str().to_string_lossy()))
        .collect()
}

fn overlaps_management(path: &Path, destination: &Path) -> bool {
    let parent = portable_parts(destination.parent().unwrap_or(destination));
    let selected = portable_parts(path);
    if selected.len() <= parent.len() {
        return parent.starts_with(&selected);
    }
    selected.starts_with(&parent)
        && path
            .components()
            .nth(parent.len())
            .is_some_and(|part| management_name(&part.as_os_str().to_string_lossy()))
}

fn store_paths(target: &InstallationTarget) -> Result<Vec<PathBuf>, TargetError> {
    let store = target.store();
    store
        .location()
        .map(Path::to_path_buf)
        .into_iter()
        .chain(store.staging_directory())
        .map(|path| physical_location(&path).map_err(error_detail))
        .collect()
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
    let stores = store_paths(target)?;
    for source in bundle.into_iter().flat_map(Bundle::source_roots) {
        if stores.iter().any(|store| path_overlap(source, store)) {
            return Err(ValueError::new("state store overlaps bundle source").into());
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
    crate::resource_plan::destination_order(&mut refs);
    refs.dedup();
    let mut destinations = HashSet::with_capacity(refs.len());
    if !refs
        .iter()
        .all(|reference| destinations.insert(reference.destination()))
    {
        return Err(ValueError::new("resource kinds disagree at the same destination").into());
    }
    for reference in &refs {
        if stores.iter().any(|store| {
            path_overlap(reference.destination(), store)
                || overlaps_management(store, reference.destination())
        }) {
            return Err(BuildError::StoreOverlap(ValueError::new(
                "state store overlaps a resource or authority",
            )));
        }
        for source in bundle.into_iter().flat_map(Bundle::source_roots) {
            if overlaps_management(source, reference.destination()) {
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
        crate::resource_plan::destination_order(&mut ready);
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
    let index = installation_index(identifier, target)?;
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
                target.store().as_ref(),
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
                if plan.previous().is_none() && expects_receipt(index.as_ref(), reference) {
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
    let error = (!rendered.supported()).then(|| {
        TargetError::new(
            ErrorCode::Unsupported,
            "rendered target has unsupported requirements",
            None,
            None,
        )
    });
    Ok(LifecyclePreview::new(
        identifier.to_owned(),
        bundle.cloned(),
        rendered.clone(),
        target.clone(),
        acquisition,
        replace_modified,
        index,
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
        Err(error) => match error.into_value_error() {
            Ok(error) => Err(error),
            Err(error) => Ok(LifecyclePreview::new(
                identifier.to_owned(),
                bundle.cloned(),
                rendered.clone(),
                target.clone(),
                acquisition,
                replace_modified,
                None,
                None,
                Vec::new(),
                Some(error),
            )),
        },
    }
}

fn public_summary(
    store: &dyn StateStore,
    reference: &ResourceRef,
) -> Result<ResourceObservation, TargetError> {
    let observed = summary(store, reference)?;
    Ok(ResourceObservation::observed(
        observed.destination().to_path_buf(),
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
        let index = installation_index(bundle_id, target)?;
        for reference in index.as_ref().into_iter().flat_map(Index::resources) {
            let store = target.store().as_ref();
            let item = check_receipt(store, index.as_ref(), reference)
                .and_then(|()| public_summary(store, reference));
            resources.push(match item {
                Ok(item) => item,
                Err(error) => ResourceObservation::failed(reference, error),
            });
        }
        let current = index.as_ref().and_then(Index::current);
        let pending = index.as_ref().is_some_and(|index| {
            index.pending().is_some() || index.previous().is_some() || !index.residual().is_empty()
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

fn fresh(
    preview: &LifecyclePreview,
    created: &[Ancestor],
) -> Result<LifecyclePreview, TargetError> {
    let target = preview.target();
    let store = target.store().as_ref();
    require_index(
        store,
        &installation_key(preview.bundle_id(), target),
        preview.index(),
    )?;
    let resources = preview
        .resources()
        .iter()
        .map(|plan| {
            let destination = plan.resource().destination();
            require(destination, plan.before())?;
            let stored = store
                .read(&resource_key(destination, "receipt"))
                .map_err(store_failure)?;
            if stored != plan.previous().map(encode) {
                return Err(failure(
                    ErrorCode::ConcurrentChange,
                    "preview receipt changed",
                    destination,
                ));
            }
            let mut recorded = false;
            for record in ["preparation", "journal"] {
                recorded |= store
                    .read(&resource_key(destination, record))
                    .map_err(store_failure)?
                    .is_some();
            }
            if recorded == plan.recovery_paths().is_empty() {
                return Err(failure(
                    ErrorCode::ConcurrentChange,
                    "preview recovery changed",
                    destination,
                ));
            }
            let staging = plan.transaction_directory().join("staging");
            let current = resource_ancestors(destination, &[&staging])?;
            let mut approved = plan.ancestors().to_vec();
            approved.extend(
                created
                    .iter()
                    .filter(|item| {
                        (destination.starts_with(item.path()) || staging.starts_with(item.path()))
                            && !plan.ancestors().contains(item)
                    })
                    .cloned(),
            );
            let same = current.iter().all(|item| approved.contains(item))
                && approved.iter().all(|item| current.contains(item));
            if !same {
                return Err(failure(
                    ErrorCode::ConcurrentChange,
                    "preview ancestor changed",
                    &staging,
                ));
            }
            Ok(plan.with_ancestors(current))
        })
        .collect::<Result<Vec<_>, TargetError>>()?;
    Ok(preview.with_resources(resources))
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

fn recovery_paths(
    store: &dyn StateStore,
    reference: &ResourceRef,
    directory: Option<&Path>,
) -> Vec<PathBuf> {
    match pending_recovery(store, reference) {
        Ok(paths) => return paths,
        Err(_) if directory.is_none() => return Vec::new(),
        Err(_) => {}
    }
    let preparation = read_record::<Preparation>(
        store,
        &resource_key(reference.destination(), "preparation"),
        "Preparation",
    );
    match (preparation, directory) {
        (Ok(Some(preparation)), Some(directory)) if preparation.directory() == directory => {
            vec![directory.to_path_buf()]
        }
        _ => Vec::new(),
    }
}

fn acquire_transaction_locks<'a>(
    identifier: &str,
    target: &'a InstallationTarget,
    expected_index: Option<&Index>,
    references: &[ResourceRef],
    timeout: Duration,
) -> Result<TransactionLocks<'a>, TargetError> {
    TransactionLocks::acquire_after_index_validation(
        target.store().as_ref(),
        &installation_key(identifier, target),
        references,
        timeout,
        || {
            if installation_index(identifier, target)?.as_ref() != expected_index {
                return Err(index_failure(
                    target,
                    ErrorCode::ConcurrentChange,
                    "index changed while acquiring locks",
                ));
            }
            Ok(())
        },
    )
}

#[allow(clippy::too_many_lines)]
fn run_locked(preview: &LifecyclePreview, locks: &TransactionLocks<'_>) -> InstallationResult {
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
    let refreshed = match fresh(preview, locks.created()) {
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
                index_failure(target, ErrorCode::InvalidState, error.message()),
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
                index_failure(target, ErrorCode::InvalidState, error.message()),
                Vec::new(),
            );
        }
    };
    let outcome = match execute_transaction(
        locks,
        refreshed.resources(),
        refreshed.index(),
        &pending,
        &completed,
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
            let paths = recovery_paths(
                target.store().as_ref(),
                plan.resource(),
                Some(plan.transaction_directory()),
            );
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
    if let Err(error) = fresh(preview, &[]) {
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

fn recover_locked(
    identifier: &str,
    target: &InstallationTarget,
    index: Option<&Index>,
    references: &[ResourceRef],
    locks: &TransactionLocks<'_>,
) -> InstallationResult {
    match installation_index(identifier, target) {
        Ok(current) if current.as_ref() == index => {}
        Ok(_) => {
            return failed_result(
                identifier,
                target,
                index_failure(
                    target,
                    ErrorCode::ConcurrentChange,
                    "index changed while acquiring locks",
                ),
                Vec::new(),
            );
        }
        Err(error) => return failed_result(identifier, target, error, Vec::new()),
    }
    let store = target.store().as_ref();
    for reference in references {
        if let Err(error) = check_receipt(store, index, reference) {
            return failed_result(identifier, target, error, Vec::new());
        }
    }
    let mut results = Vec::new();
    for guard in locks.resources() {
        let outcome = pending_recovery(store, guard.reference())
            .map(|paths| !paths.is_empty())
            .and_then(|pending| recover(store, guard).map(|()| pending));
        let pending = match outcome {
            Ok(pending) => pending,
            Err(error) => {
                let paths = recovery_paths(store, guard.reference(), None);
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
    let changed = results
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
    let index = match installation_index(identifier, target) {
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
        Err(error) => return failed_result(identifier, target, build_failure(error), Vec::new()),
    };
    match acquire_transaction_locks(identifier, target, index.as_ref(), &references, timeout) {
        Ok(locks) => recover_locked(identifier, target, index.as_ref(), &references, &locks),
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
    let index = match installation_index(identifier, target) {
        Ok(value) => value,
        Err(error) => return failed_result(identifier, target, error, Vec::new()),
    };
    let refs = match references(rendered, index.as_ref(), target, bundle, None) {
        Ok(value) => value,
        Err(error) => return failed_result(identifier, target, build_failure(error), Vec::new()),
    };
    let locks = match acquire_transaction_locks(identifier, target, index.as_ref(), &refs, timeout)
    {
        Ok(value) => value,
        Err(error) => return failed_result(identifier, target, error, Vec::new()),
    };
    let recovered = recover_locked(identifier, target, index.as_ref(), &refs, &locks);
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
        Err(error) => return failed_result(identifier, target, build_failure(error), Vec::new()),
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
        Err(BuildError::Invalid(error) | BuildError::StoreOverlap(error)) => Err(error),
        Err(BuildError::Target(error)) => Ok(Some(error)),
    }
}

pub(crate) fn validate_skill_references(
    rendered: &RenderedBundle,
    target: &InstallationTarget,
    bundle: &Bundle,
) -> Result<Option<TargetError>, ValueError> {
    match references(rendered, None, target, Some(bundle), None) {
        Ok(_) => Ok(None),
        Err(BuildError::Invalid(error)) => Err(error),
        Err(BuildError::StoreOverlap(error)) => Ok(Some(invalid_state(&error))),
        Err(BuildError::Target(error)) => Ok(Some(error)),
    }
}

fn build_failure(error: BuildError) -> TargetError {
    match error.into_value_error() {
        Ok(error) => invalid_state(&error),
        Err(error) => error,
    }
}
