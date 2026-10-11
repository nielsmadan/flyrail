use crate::{
    Agent, Family, LifecyclePreview, Notice, ResourceKind, Surface, TargetError, TargetScope,
    ValueError,
};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ChangeAction {
    Create,
    Update,
    Delete,
    Unchanged,
    Conflict,
}

impl ChangeAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Unchanged => "unchanged",
            Self::Conflict => "conflict",
        }
    }
}

impl Display for ChangeAction {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ChangeAction {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "create" => Ok(Self::Create),
            "update" => Ok(Self::Update),
            "delete" => Ok(Self::Delete),
            "unchanged" => Ok(Self::Unchanged),
            "conflict" => Ok(Self::Conflict),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid ChangeAction"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedChange {
    artifact_ids: Vec<String>,
    families: Vec<Family>,
    kind: ResourceKind,
    destination: PathBuf,
    action: ChangeAction,
    error: Option<TargetError>,
    recovery_paths: Vec<PathBuf>,
}

impl PlannedChange {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        artifact_ids: impl IntoIterator<Item = String>,
        families: impl IntoIterator<Item = Family>,
        kind: ResourceKind,
        destination: PathBuf,
        action: ChangeAction,
        error: Option<TargetError>,
        recovery_paths: impl IntoIterator<Item = PathBuf>,
    ) -> Result<Self, ValueError> {
        let artifact_ids = artifact_ids.into_iter().collect::<Vec<_>>();
        let families = families.into_iter().collect::<Vec<_>>();
        let recovery_paths = recovery_paths.into_iter().collect::<Vec<_>>();
        if artifact_ids.len() != families.len() {
            return Err(ValueError::new(
                "planned change artifact ids and families must agree",
            ));
        }
        if !destination.is_absolute() {
            return Err(ValueError::new(
                "planned change destination must be an absolute Path",
            ));
        }
        if recovery_paths.iter().any(|path| !path.is_absolute()) {
            return Err(ValueError::new("expected an absolute concrete path"));
        }
        Ok(Self {
            artifact_ids,
            families,
            kind,
            destination,
            action,
            error,
            recovery_paths,
        })
    }

    pub fn artifact_ids(&self) -> &[String] {
        &self.artifact_ids
    }

    pub fn families(&self) -> &[Family] {
        &self.families
    }

    pub const fn kind(&self) -> ResourceKind {
        self.kind
    }

    pub fn destination(&self) -> &Path {
        &self.destination
    }

    pub const fn action(&self) -> ChangeAction {
        self.action
    }

    pub const fn error(&self) -> Option<&TargetError> {
        self.error.as_ref()
    }

    pub fn recovery_paths(&self) -> &[PathBuf] {
        &self.recovery_paths
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanSummary {
    agent: Option<Agent>,
    scope: Option<TargetScope>,
    surface: Option<Surface>,
    applicable: bool,
    changes: Vec<PlannedChange>,
    notices: Vec<Notice>,
    error: Option<TargetError>,
}

impl PlanSummary {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        agent: Option<Agent>,
        scope: Option<TargetScope>,
        surface: Option<Surface>,
        applicable: bool,
        changes: impl IntoIterator<Item = PlannedChange>,
        notices: impl IntoIterator<Item = Notice>,
        error: Option<TargetError>,
    ) -> Self {
        Self {
            agent,
            scope,
            surface,
            applicable,
            changes: changes.into_iter().collect(),
            notices: notices.into_iter().collect(),
            error,
        }
    }

    pub const fn agent(&self) -> Option<Agent> {
        self.agent
    }

    pub const fn scope(&self) -> Option<TargetScope> {
        self.scope
    }

    pub const fn surface(&self) -> Option<Surface> {
        self.surface
    }

    pub const fn applicable(&self) -> bool {
        self.applicable
    }

    pub fn changes(&self) -> &[PlannedChange] {
        &self.changes
    }

    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }

    pub const fn error(&self) -> Option<&TargetError> {
        self.error.as_ref()
    }
}

pub fn summarize(preview: &LifecyclePreview) -> Result<PlanSummary, ValueError> {
    let routing = preview
        .target()
        .routing_context()
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<HashMap<_, _>>();
    let mut artifacts = HashMap::<&Path, Vec<_>>::new();
    for artifact in preview.rendered().artifacts() {
        artifacts
            .entry(artifact.destination())
            .or_default()
            .push(artifact);
    }
    let changes = preview
        .resources()
        .iter()
        .map(|plan| {
            let mut matched = artifacts
                .get(plan.resource().destination())
                .cloned()
                .unwrap_or_default();
            matched.sort_by_key(|artifact| artifact.id());
            PlannedChange::new(
                matched.iter().map(|artifact| artifact.id().to_owned()),
                matched.iter().map(|artifact| artifact.family()),
                plan.resource().kind(),
                plan.resource().destination().to_path_buf(),
                action(plan),
                plan.error().cloned(),
                plan.recovery_paths().iter().cloned(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PlanSummary::new(
        parse_routing(routing.get("agent").copied())?,
        parse_routing(routing.get("scope").copied())?,
        parse_routing(routing.get("surface").copied())?,
        preview.applicable(),
        changes,
        preview.rendered().notices().to_vec(),
        preview.error().cloned(),
    ))
}

fn action(plan: &crate::ResourcePlan) -> ChangeAction {
    if plan.error().is_some() || !plan.recovery_paths().is_empty() {
        ChangeAction::Conflict
    } else if plan.previous() == Some(plan.receipt()) && plan.before() == plan.after() {
        ChangeAction::Unchanged
    } else if plan.after().nodes().is_empty() {
        ChangeAction::Delete
    } else if plan.before().nodes().is_empty() {
        ChangeAction::Create
    } else {
        ChangeAction::Update
    }
}

fn parse_routing<T: FromStr<Err = ValueError>>(
    value: Option<&str>,
) -> Result<Option<T>, ValueError> {
    value
        .filter(|value| !value.is_empty())
        .map(str::parse)
        .transpose()
}
