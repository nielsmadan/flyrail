use crate::artifacts::{Artifact, Family};
use crate::authority::{AuthorityError, ClaimSelector, ResourceAuthority, state_path};
use crate::bundle::Bundle;
use crate::configuration::{InstallationObservation, InstallationTarget};
use crate::content::{Content, TreeContent};
use crate::destinations::{is_translatable, untranslatable_message};
use crate::inventory::inventory;
use crate::models::BundleEntry;
use crate::observations::{
    Conflict, ErrorCode, Installation, Modification, ModificationKind, Observation,
    ObservationState, TargetError, TargetInspection,
};
use crate::rendered::{Notice, NoticeKind, RenderedArtifact, RenderedBundle};
use crate::resource_models::Receipt;
use crate::values::ValueError;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub trait SkillTarget: Clone {
    fn root(&self) -> &Path;

    fn agent(&self) -> Option<crate::Agent> {
        None
    }

    fn routing_context(&self) -> Vec<(String, String)> {
        vec![
            (String::from("agent"), String::new()),
            (String::from("scope"), String::from("directory")),
            (String::from("home"), String::new()),
        ]
    }

    fn render_bundle(
        &self,
        bundle: &Bundle,
        destination: &Path,
    ) -> Result<RenderedBundle, AuthorityError> {
        render_skill_bundle_at(bundle, destination).map_err(AuthorityError::from)
    }
}

impl SkillTarget for PathBuf {
    fn root(&self) -> &Path {
        self
    }
}

impl SkillTarget for &Path {
    fn root(&self) -> &Path {
        self
    }
}

impl<T: SkillTarget> SkillTarget for &T {
    fn root(&self) -> &Path {
        (*self).root()
    }

    fn routing_context(&self) -> Vec<(String, String)> {
        (*self).routing_context()
    }

    fn agent(&self) -> Option<crate::Agent> {
        (*self).agent()
    }

    fn render_bundle(
        &self,
        bundle: &Bundle,
        destination: &Path,
    ) -> Result<RenderedBundle, AuthorityError> {
        (*self).render_bundle(bundle, destination)
    }
}

#[derive(Clone)]
pub(crate) struct PreparedSkill<T> {
    pub target: T,
    pub installation: Result<InstallationTarget, TargetError>,
    pub root: PathBuf,
    pub alias_of: Option<usize>,
    pub rendered: Option<RenderedBundle>,
}

pub fn render_skills<T: SkillTarget>(
    bundle: &Bundle,
    target: &T,
) -> Result<RenderedBundle, AuthorityError> {
    validate_skill_bundle(bundle)?;
    if let Some(agent) = target.agent().filter(|agent| !is_translatable(*agent)) {
        let notices = bundle
            .skills()
            .iter()
            .map(|skill| {
                Notice::with_code(
                    skill.name(),
                    NoticeKind::Unsupported,
                    untranslatable_message(agent),
                    "agent-unsupported",
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        return RenderedBundle::with_notices([], [], notices, []).map_err(AuthorityError::from);
    }
    let destination = ResourceAuthority::new(target.root())?
        .destination()
        .to_path_buf();
    target.render_bundle(bundle, &destination)
}

fn validate_skill_bundle(bundle: &Bundle) -> Result<(), ValueError> {
    if bundle
        .artifacts()
        .iter()
        .any(|artifact| !matches!(artifact, Artifact::Skill(_)))
    {
        return Err(ValueError::new(
            "use explicit rendering for non-skill artifacts",
        ));
    }
    if !bundle.assets().is_empty() || !bundle.dependencies().is_empty() {
        return Err(ValueError::new(
            "use explicit rendering for skill bundles with assets or dependencies",
        ));
    }
    Ok(())
}

fn render_skill_bundle_at(
    bundle: &Bundle,
    destination: &Path,
) -> Result<RenderedBundle, ValueError> {
    let mut rendered = Vec::new();
    for skill in bundle.skills() {
        let prefix = format!("{}/", skill.name());
        let entries = bundle
            .entries()
            .iter()
            .filter_map(|entry| {
                entry.path().strip_prefix(&prefix).map(|path| {
                    BundleEntry::new(path, entry.data().map(<[u8]>::to_vec), entry.executable())
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        rendered.push(RenderedArtifact::new(
            skill.name(),
            Family::Skills,
            destination.to_path_buf(),
            Content::Tree(TreeContent::new(entries)?),
            Some(skill.name().to_owned()),
            false,
            Vec::new(),
        )?);
    }
    if rendered.is_empty() {
        for artifact in bundle.artifacts() {
            if let Artifact::Skill(skill) = artifact {
                rendered.push(RenderedArtifact::new(
                    skill.id(),
                    Family::Skills,
                    destination.to_path_buf(),
                    Content::Tree(skill.content().clone()),
                    Some(skill.name().to_owned()),
                    false,
                    Vec::new(),
                )?);
            }
        }
    }
    RenderedBundle::new(rendered, [], [])
}

fn authority_error(error: AuthorityError) -> Result<TargetError, ValueError> {
    match error {
        AuthorityError::Invalid(error) => Err(error),
        AuthorityError::Unsafe { message, path } => Ok(TargetError::new(
            ErrorCode::UnsafePath,
            message,
            Some(path),
            None,
        )),
        AuthorityError::Io {
            message,
            path,
            errno,
        } => Ok(TargetError::new(
            crate::observation::io_code(errno),
            message,
            path,
            errno,
        )),
    }
}

fn prepare_target<T: SkillTarget>(
    target: T,
    identifier: &str,
) -> Result<PreparedSkill<T>, ValueError> {
    if let Some(agent) = target.agent().filter(|agent| !is_translatable(*agent)) {
        let root = target.root().to_path_buf();
        return Ok(PreparedSkill {
            target,
            installation: Err(TargetError::new(
                ErrorCode::Unsupported,
                untranslatable_message(agent),
                Some(root.clone()),
                None,
            )),
            root,
            alias_of: None,
            rendered: None,
        });
    }
    let root = match ResourceAuthority::new(target.root()) {
        Ok(authority) => authority.destination().to_path_buf(),
        Err(error) => {
            let root = target.root().to_path_buf();
            return Ok(PreparedSkill {
                target,
                installation: Err(authority_error(error)?),
                root,
                alias_of: None,
                rendered: None,
            });
        }
    };
    let Some(index_root) = root.parent().map(|parent| {
        parent.join(format!(
            ".{}.flyrail-index-{identifier}",
            root.file_name().unwrap_or_default().to_string_lossy()
        ))
    }) else {
        return Err(ValueError::new(
            "resource destination cannot be a root or contain traversal",
        ));
    };
    let mut routing_context = vec![(String::from("destination"), root.display().to_string())];
    routing_context.extend(target.routing_context());
    let installation = match InstallationTarget::new(
        index_root,
        [(String::from("destination"), root.display().to_string())],
        routing_context,
    ) {
        Ok(value) => value,
        Err(error) => {
            return Ok(PreparedSkill {
                target,
                installation: Err(authority_error(error)?),
                root,
                alias_of: None,
                rendered: None,
            });
        }
    };
    Ok(PreparedSkill {
        target,
        installation: Ok(installation),
        root,
        alias_of: None,
        rendered: None,
    })
}

pub(crate) fn prepare_skills<T: SkillTarget>(
    bundle: Option<&Bundle>,
    identifier: &str,
    targets: impl IntoIterator<Item = T>,
) -> Result<Vec<PreparedSkill<T>>, ValueError> {
    crate::configuration::validate_bundle_id(identifier)?;
    if let Some(bundle) = bundle {
        validate_skill_bundle(bundle)?;
    }
    let targets = targets.into_iter().collect::<Vec<_>>();
    if targets.is_empty() {
        return Err(ValueError::new("at least one target is required"));
    }
    let mut prepared: Vec<PreparedSkill<T>> = Vec::with_capacity(targets.len());
    let mut known: Vec<(PathBuf, usize)> = Vec::new();
    for target in targets {
        let mut item = prepare_target(target, identifier)?;
        item.alias_of = item.installation.as_ref().ok().and_then(|_| {
            known
                .iter()
                .find_map(|(root, index)| (root == &item.root).then_some(*index))
        });
        if let Some(alias) = item.alias_of {
            item.rendered = prepared.get(alias).and_then(|prior| prior.rendered.clone());
        } else if item.installation.is_ok() {
            known.push((item.root.clone(), prepared.len()));
            if let (Some(bundle), Ok(installation)) = (bundle, &item.installation) {
                match item.target.render_bundle(bundle, &item.root) {
                    Ok(rendered) => {
                        if let Some(error) = crate::lifecycle_impl::validate_reference_request(
                            &rendered,
                            installation,
                            bundle,
                        )? {
                            item.installation = Err(error);
                        } else {
                            item.rendered = Some(rendered);
                        }
                    }
                    Err(error) => item.installation = Err(authority_error(error)?),
                }
            }
        }
        prepared.push(item);
    }
    Ok(prepared)
}

pub(crate) fn skill_observation(
    observation: &InstallationObservation,
    bundle: Option<&Bundle>,
    rendered: Option<&RenderedBundle>,
) -> Result<Observation, ValueError> {
    let (installations, metadata_error) = installed_inventory(observation)?;
    let installed = installations
        .iter()
        .find(|item| item.bundle_id() == observation.bundle_id())
        .cloned();
    let modifications = observation
        .resources()
        .iter()
        .flat_map(crate::configuration::ResourceObservation::claims)
        .filter(|claim| claim.status() != crate::editors::EditStatus::Current)
        .map(|claim| {
            Modification::new(
                claim.bundle_id(),
                claim.artifact_id(),
                ModificationKind::ContentChanged,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let recovery_paths = observation
        .resources()
        .iter()
        .flat_map(|resource| resource.recovery_paths().iter().cloned())
        .collect::<Vec<_>>();
    let error = observation.error().cloned().or(metadata_error).or_else(|| {
        observation
            .resources()
            .iter()
            .find_map(|resource| resource.error().cloned())
    });
    let state = if recovery_paths.is_empty() {
        if error.is_some() {
            ObservationState::Unknown
        } else if installed.is_some() {
            ObservationState::Installed
        } else {
            ObservationState::Absent
        }
    } else {
        ObservationState::RecoveryNeeded
    };
    Ok(Observation::new(
        state,
        installed.clone(),
        installations,
        bundle
            .zip(installed.as_ref())
            .map(|(bundle, installed)| installed.version() == bundle.version()),
        bundle
            .zip(installed.as_ref())
            .map(|(bundle, installed)| installed.content_digest() == bundle.content_digest()),
        bundle.map(|bundle| {
            observation.is_current()
                && observation.bundle_digest() == Some(bundle.content_digest())
                && rendered.is_none_or(|rendered| {
                    observation.render_digest() == Some(rendered.content_digest())
                })
        }),
        modifications,
        [],
        error,
        recovery_paths,
    ))
}

fn installed_inventory(
    observation: &InstallationObservation,
) -> Result<(Vec<Installation>, Option<TargetError>), ValueError> {
    let mut installations = Vec::new();
    let mut metadata_error = None;
    for observed in observation.resources() {
        let receipt_path = observed.state_root().join("receipt.json");
        let stored = match crate::resource_io::read::<Receipt>(&receipt_path, "Receipt") {
            Ok(value) => value,
            Err(error) => {
                metadata_error = Some(error);
                continue;
            }
        };
        let Some(stored) = stored else {
            continue;
        };
        let identifiers = stored
            .claims()
            .iter()
            .map(crate::resource_models::OwnedClaim::bundle_id)
            .collect::<BTreeSet<_>>();
        for identifier in identifiers {
            let claims = stored
                .claims()
                .iter()
                .filter(|claim| claim.bundle_id() == identifier)
                .collect::<Vec<_>>();
            let mut entries = Vec::new();
            for claim in &claims {
                if let (Some(installed), ClaimSelector::Subtree(selector)) =
                    (claim.installed(), claim.claim().selector())
                {
                    for node in installed.nodes() {
                        let name = if node.path().is_empty() {
                            selector.clone()
                        } else {
                            format!("{selector}/{}", node.path())
                        };
                        entries.push(BundleEntry::new(
                            name,
                            node.data().map(<[u8]>::to_vec),
                            node.data().is_some()
                                && node.mode().as_u32().is_some_and(|mode| mode & 0o111 != 0),
                        )?);
                    }
                }
            }
            if let Some(first) = claims.first() {
                installations.push(Installation::new(
                    identifier,
                    first.version(),
                    first.bundle_digest(),
                    stored.transaction_id(),
                    inventory(entries)?,
                )?);
            }
        }
    }
    Ok((installations, metadata_error))
}

pub fn inspect<T: SkillTarget>(
    bundle: &Bundle,
    targets: impl IntoIterator<Item = T>,
) -> Result<Vec<TargetInspection<T>>, ValueError> {
    let mut result: Vec<TargetInspection<T>> = Vec::new();
    for item in prepare_skills(Some(bundle), bundle.id(), targets)? {
        if let Some(alias) = item.alias_of
            && let Some(prior) = result.get(alias)
        {
            result.push(prior.aliased(item.target, alias));
            continue;
        }
        let state_root = state_path(&item.root);
        let observation = match (&item.installation, &item.rendered) {
            (Err(error), _) => Observation::unknown(error.clone()),
            (Ok(installation), Some(rendered)) => {
                let observed = crate::lifecycle::inspect_installation(bundle.id(), installation)?;
                let mut observation = skill_observation(&observed, Some(bundle), Some(rendered))?;
                let planned = crate::lifecycle::preview(
                    bundle,
                    rendered,
                    installation,
                    crate::editors::Acquisition::Conflict,
                    false,
                )?;
                let conflicts = planned
                    .resources()
                    .iter()
                    .filter(|plan| {
                        plan.error()
                            .is_some_and(|error| error.code() == ErrorCode::Conflict)
                    })
                    .map(|plan| {
                        Conflict::new(plan.resource().destination().display().to_string(), None)
                    })
                    .collect::<Vec<_>>();
                let error = observation
                    .error()
                    .cloned()
                    .or_else(|| planned.error().cloned())
                    .or_else(|| {
                        planned.resources().iter().find_map(|plan| {
                            plan.error()
                                .filter(|error| error.code() != ErrorCode::Conflict)
                                .cloned()
                        })
                    });
                let state =
                    if error.is_some() && observation.state() != ObservationState::RecoveryNeeded {
                        ObservationState::Unknown
                    } else {
                        observation.state()
                    };
                observation = observation.with_preview(state, conflicts, error);
                observation
            }
            (Ok(_), None) => {
                return Err(ValueError::new(
                    "skill inspection requires rendered content",
                ));
            }
        };
        result.push(TargetInspection::new(
            item.target,
            item.root,
            state_root,
            None,
            observation,
        ));
    }
    Ok(result)
}
