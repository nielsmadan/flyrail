mod manifest;
mod render;

use crate::configuration::{
    InstallationObservation, InstallationResult, InstallationTarget, LifecyclePreview,
};
use crate::observations::{ErrorCode, TargetError};
use crate::{
    Acquisition, Agent, Bundle, HookRuntime, Notice, NoticeKind, OperationStatus, Platform,
    RenderedBundle, StateStore, ValueError,
};
pub(crate) use manifest::ReadPrecondition;
use manifest::{
    Inputs, Manifest, absolute, load, parse_agent, profile_file, source_path, strings, text,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutDiagnostic {
    code: &'static str,
    message: String,
    path: PathBuf,
}

impl LoadoutDiagnostic {
    fn at(code: &'static str, message: impl Into<String>, path: &Path) -> Self {
        Self {
            code,
            message: message.into(),
            path: path.to_owned(),
        }
    }
    fn changed(error: &TargetError, registry: &Path) -> Self {
        Self::at(
            "concurrent-change",
            error.message(),
            error.path().unwrap_or(registry),
        )
    }
    pub const fn code(&self) -> &'static str {
        self.code
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutSource {
    name: String,
    path: PathBuf,
    uses: BTreeSet<String>,
    declared_in: PathBuf,
    overrides: BTreeMap<String, BTreeSet<String>>,
}

impl LoadoutSource {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub const fn uses(&self) -> &BTreeSet<String> {
        &self.uses
    }
    pub fn declared_in(&self) -> &Path {
        &self.declared_in
    }
    pub const fn overrides(&self) -> &BTreeMap<String, BTreeSet<String>> {
        &self.overrides
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutHarness {
    agent: Agent,
    slices: BTreeSet<String>,
}

impl LoadoutHarness {
    pub const fn agent(&self) -> Agent {
        self.agent
    }
    pub const fn slices(&self) -> &BTreeSet<String> {
        &self.slices
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutSelector {
    agent: Agent,
    category: &'static str,
    manifest: PathBuf,
    key: Vec<String>,
    entries: Vec<String>,
    swaps: BTreeMap<String, String>,
    array: bool,
    destinations: usize,
    all_profiles: bool,
}

impl LoadoutSelector {
    pub const fn agent(&self) -> Agent {
        self.agent
    }
    pub const fn category(&self) -> &'static str {
        self.category
    }
    pub fn manifest(&self) -> &Path {
        &self.manifest
    }
    pub fn key(&self) -> &[String] {
        &self.key
    }
    pub fn entries(&self) -> &[String] {
        &self.entries
    }
    pub const fn substitutions(&self) -> &BTreeMap<String, String> {
        &self.swaps
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutLocation {
    root: PathBuf,
    profile: String,
    source_name: String,
    source_path: PathBuf,
}

impl LoadoutLocation {
    pub fn new(
        root: PathBuf,
        profile: String,
        source_name: String,
        source_path: PathBuf,
    ) -> Result<Self, ValueError> {
        absolute(&root)?;
        absolute(&source_path)?;
        profile_file(&root, &profile).map_err(|issue| ValueError::new(issue.message))?;
        if source_name.is_empty() || source_name.contains(['/', '\\', '\0']) {
            return Err(ValueError::new("Select one Loadout source name"));
        }
        Ok(Self {
            root,
            profile,
            source_name,
            source_path,
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn profile(&self) -> &str {
        &self.profile
    }
    pub fn source_name(&self) -> &str {
        &self.source_name
    }
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }
    pub fn installation(
        &self,
        store: Arc<dyn StateStore>,
    ) -> Result<InstallationTarget, ValueError> {
        InstallationTarget::new(
            store,
            [
                ("destination".into(), "loadout".into()),
                ("root".into(), self.root.display().to_string()),
                ("profile".into(), self.profile.clone()),
                ("source-name".into(), self.source_name.clone()),
                ("source-path".into(), self.source_path.display().to_string()),
            ],
            [],
        )
    }
}

#[derive(Debug)]
pub struct LoadoutSetup {
    registry: PathBuf,
    home: PathBuf,
    root: PathBuf,
    profile: String,
    manifest: Manifest,
    conditions: Vec<ReadPrecondition>,
}

impl LoadoutSetup {
    pub fn registry(&self) -> &Path {
        &self.registry
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn profile(&self) -> &str {
        &self.profile
    }
    pub fn profile_chain(&self) -> &[PathBuf] {
        &self.manifest.chain
    }
    pub fn sources(&self) -> &[LoadoutSource] {
        &self.manifest.sources
    }
    pub fn harnesses(&self) -> &[LoadoutHarness] {
        &self.manifest.harnesses
    }
    pub fn selectors(&self) -> &[LoadoutSelector] {
        &self.manifest.selectors
    }

    pub fn eligible_sources(&self, bundle: &Bundle, harnesses: &[Agent]) -> Vec<&LoadoutSource> {
        if harnesses.is_empty() || !render::hooks_supported(bundle, harnesses) {
            return Vec::new();
        }
        let required = render::required_slices(bundle, harnesses);
        if required.iter().any(|(agent, category)| {
            !harnesses.contains(agent)
                || self
                    .manifest
                    .custom_routes
                    .contains(&(*agent, (*category).to_owned()))
                || !self
                    .harnesses()
                    .iter()
                    .any(|harness| harness.agent == *agent && harness.slices.contains(*category))
        }) {
            return Vec::new();
        }
        self.sources()
            .iter()
            .filter(|source| required.iter().all(|(_, name)| source.uses.contains(*name)))
            .collect()
    }

    pub fn select(
        &self,
        name: &str,
        agents: &[Agent],
        accept_shared_scope: bool,
    ) -> Result<LoadoutDestination<'_>, ValueError> {
        let source = self
            .sources()
            .iter()
            .find(|source| source.name == name)
            .ok_or_else(|| {
                ValueError::new(format!(
                    "Select a declared Loadout source, found no source {name:?}"
                ))
            })?;
        let mut selected = agents.to_vec();
        selected.sort_by_key(|agent| agent.as_str());
        selected.dedup();
        if selected.is_empty() || selected.len() != agents.len() {
            return Err(ValueError::new(
                "Select one or more distinct Loadout harnesses",
            ));
        }
        Ok(LoadoutDestination {
            setup: self,
            source,
            agents: selected,
            accept_shared_scope,
        })
    }
}

#[derive(Debug)]
pub enum LoadoutDiscovery {
    Absent {
        registry: PathBuf,
    },
    Available(Box<LoadoutSetup>),
    Invalid {
        registry: PathBuf,
        diagnostic: LoadoutDiagnostic,
    },
}

#[derive(Debug)]
pub struct LoadoutDestination<'a> {
    setup: &'a LoadoutSetup,
    source: &'a LoadoutSource,
    agents: Vec<Agent>,
    accept_shared_scope: bool,
}

impl LoadoutDestination<'_> {
    pub fn location(&self) -> LoadoutLocation {
        LoadoutLocation {
            root: self.setup.root.clone(),
            profile: self.setup.profile.clone(),
            source_name: self.source.name.clone(),
            source_path: self.source.path.clone(),
        }
    }
    pub const fn source(&self) -> &LoadoutSource {
        self.source
    }
    pub fn harnesses(&self) -> &[Agent] {
        &self.agents
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutImpact {
    category: &'static str,
    path: PathBuf,
    harnesses: Vec<Agent>,
    shared: bool,
}

impl LoadoutImpact {
    pub const fn category(&self) -> &'static str {
        self.category
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn harnesses(&self) -> &[Agent] {
        &self.harnesses
    }
    pub const fn shared(&self) -> bool {
        self.shared
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LoadoutOptions<'a> {
    pub platform: Platform,
    pub hook_runtime: Option<&'a HookRuntime>,
    pub execution_root: Option<&'a Path>,
    pub replace_modified: bool,
}

impl LoadoutOptions<'_> {
    pub const fn new(platform: Platform) -> Self {
        Self {
            platform,
            hook_runtime: None,
            execution_root: None,
            replace_modified: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutPreview {
    location: LoadoutLocation,
    lifecycle: LifecyclePreview,
    impact: Vec<LoadoutImpact>,
    diagnostic: Option<LoadoutDiagnostic>,
}

impl LoadoutPreview {
    pub const fn location(&self) -> &LoadoutLocation {
        &self.location
    }
    pub const fn lifecycle(&self) -> &LifecyclePreview {
        &self.lifecycle
    }
    pub fn impact(&self) -> &[LoadoutImpact] {
        &self.impact
    }
    pub const fn diagnostic(&self) -> Option<&LoadoutDiagnostic> {
        self.diagnostic.as_ref()
    }
    pub fn applicable(&self) -> bool {
        self.lifecycle.applicable()
    }
    pub const fn scope_notice(&self) -> &'static str {
        "Other profiles and projects using this source may also receive its content."
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutResult {
    location: LoadoutLocation,
    lifecycle: InstallationResult,
}

impl LoadoutResult {
    pub const fn location(&self) -> &LoadoutLocation {
        &self.location
    }
    pub const fn lifecycle(&self) -> &InstallationResult {
        &self.lifecycle
    }
    pub const fn sync_required(&self) -> bool {
        !matches!(self.lifecycle.status(), OperationStatus::Failed)
    }
}

pub fn discover_loadout(
    home: &Path,
    config_home: Option<&Path>,
    env: &BTreeMap<String, String>,
) -> Result<LoadoutDiscovery, ValueError> {
    absolute(home)?;
    let config = config_home
        .map(Path::to_owned)
        .or_else(|| {
            env.get("XDG_CONFIG_HOME")
                .filter(|item| !item.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| home.join(".config"));
    absolute(&config)?;
    let registry = config.join("loadout/config.toml");
    Ok(match read_setup(home, &registry) {
        Ok(None) => LoadoutDiscovery::Absent { registry },
        Ok(Some(setup)) => LoadoutDiscovery::Available(Box::new(setup)),
        Err(diagnostic) => LoadoutDiscovery::Invalid {
            registry,
            diagnostic,
        },
    })
}

fn read_setup(home: &Path, registry: &Path) -> Result<Option<LoadoutSetup>, LoadoutDiagnostic> {
    let mut inputs = Inputs::default();
    let Some(data) = inputs.table(registry, "invalid-registry", None)? else {
        return Ok(None);
    };
    let invalid = |message| LoadoutDiagnostic::at("invalid-registry", message, registry);
    if data
        .keys()
        .any(|name| !["source", "profile", "harnesses"].contains(&name.as_str()))
    {
        return Err(invalid("Loadout registration has unknown keys"));
    }
    let path = text(data.get("source"))
        .filter(|item| !item.is_empty())
        .ok_or_else(|| invalid("Registration requires source"))?;
    let profile = if let Some(value) = data.get("profile") {
        text(Some(value))
            .filter(|item| !item.is_empty())
            .ok_or_else(|| invalid("Profile must be a nonempty string"))?
    } else {
        "default"
    };
    if data.contains_key("harnesses") {
        let names = strings(
            data.get("harnesses"),
            registry,
            "harnesses",
            "invalid-registry",
        )?;
        if names.is_empty()
            || names.iter().collect::<BTreeSet<_>>().len() != names.len()
            || names.iter().any(|name| parse_agent(name).is_none())
        {
            return Err(invalid(
                "Registration harnesses must be unique known harness names",
            ));
        }
    }
    let logical = source_path(path, home, None)
        .map_err(|error| LoadoutDiagnostic::at("invalid-registry", error.message(), registry))?;
    let root = inputs.directory(&logical)?;
    let manifest = load(&root, profile, home, &mut inputs)?;
    let conditions = inputs.conditions();
    for condition in &conditions {
        condition
            .require()
            .map_err(|error| LoadoutDiagnostic::changed(&error, registry))?;
    }
    Ok(Some(LoadoutSetup {
        registry: registry.to_owned(),
        home: home.to_owned(),
        root,
        profile: profile.to_owned(),
        manifest,
        conditions,
    }))
}

fn sync_notice(bundle_id: &str) -> Result<Notice, ValueError> {
    Notice::with_code(
        bundle_id,
        NoticeKind::Activation,
        "Run Loadout sync to reflect these source changes. An update or removal can retire \
         assets still referenced by generated hooks and MCP servers.",
        "loadout-sync-required",
    )
}

pub fn preview_loadout(
    bundle: &Bundle,
    destination: &LoadoutDestination<'_>,
    store: Arc<dyn StateStore>,
    options: LoadoutOptions<'_>,
) -> Result<LoadoutPreview, ValueError> {
    let location = destination.location();
    let target = location.installation(store)?;
    let prefix = scope_prefix(&target);
    let mut impact = Vec::new();
    let mut conditions = destination.setup.conditions.clone();
    let mut observed = Inputs::default();
    let require = |condition: &ReadPrecondition| {
        condition
            .require()
            .map_err(|error| LoadoutDiagnostic::changed(&error, &destination.setup.registry))
    };
    let output = conditions.iter().try_for_each(require).and_then(|()| {
        render::render(
            bundle,
            destination,
            options,
            &mut observed,
            &mut impact,
            &prefix,
        )
    });
    conditions.extend(observed.conditions());
    let output = output.and_then(|rendered| {
        conditions.iter().try_for_each(require)?;
        Ok(rendered)
    });
    let (lifecycle, diagnostic) = match output {
        Ok(rendered) => scope_ownership(
            crate::preview(
                bundle,
                &rendered,
                &target,
                Acquisition::Conflict,
                options.replace_modified,
            )?
            .with_read_preconditions(conditions),
        ),
        Err(issue) => {
            let code = if issue.code == "concurrent-change" {
                ErrorCode::ConcurrentChange
            } else {
                ErrorCode::Unsupported
            };
            let error = TargetError::new(code, &issue.message, Some(issue.path.clone()), None);
            (
                LifecyclePreview::new(
                    bundle.id().to_owned(),
                    Some(bundle.clone()),
                    RenderedBundle::empty(),
                    target,
                    Acquisition::Conflict,
                    options.replace_modified,
                    None,
                    None,
                    Vec::new(),
                    Some(error),
                ),
                Some(issue),
            )
        }
    };
    Ok(LoadoutPreview {
        location,
        lifecycle,
        impact,
        diagnostic,
    })
}

fn scope_prefix(target: &InstallationTarget) -> String {
    target.context_digest().chars().take(32).collect::<String>() + "-"
}

fn scope_ownership(lifecycle: LifecyclePreview) -> (LifecyclePreview, Option<LoadoutDiagnostic>) {
    let prefix = scope_prefix(lifecycle.target());
    let foreign = lifecycle.resources().iter().find(|plan| {
        plan.previous().is_some_and(|receipt| {
            receipt.claims().iter().any(|claim| {
                claim.bundle_id() == lifecycle.bundle_id()
                    && !claim.artifact_id().starts_with(&prefix)
            })
        })
    });
    if let Some(resource) = foreign {
        let path = resource.resource().destination().to_owned();
        let message = "This bundle already owns the resource through another installation scope";
        let diagnostic = LoadoutDiagnostic::at("source-ownership", message, &path);
        return (
            lifecycle.with_error(TargetError::new(
                ErrorCode::Conflict,
                message,
                Some(path),
                None,
            )),
            Some(diagnostic),
        );
    }
    (lifecycle, None)
}

pub fn preview_loadout_removal(
    bundle_id: &str,
    location: &LoadoutLocation,
    store: Arc<dyn StateStore>,
    replace_modified: bool,
) -> Result<LoadoutPreview, ValueError> {
    let target = location.installation(store)?;
    let rendered = RenderedBundle::with_notices([], [], [sync_notice(bundle_id)?], [])?;
    let (lifecycle, diagnostic) = scope_ownership(crate::lifecycle_impl::public_preview(
        bundle_id,
        None,
        &rendered,
        &target,
        Acquisition::Conflict,
        replace_modified,
    )?);
    Ok(LoadoutPreview {
        location: location.clone(),
        lifecycle,
        impact: Vec::new(),
        diagnostic,
    })
}

pub fn apply_loadout(plan: &LoadoutPreview, lock_timeout: Duration) -> LoadoutResult {
    let lifecycle = if plan.applicable() {
        crate::apply_preview(&plan.lifecycle, lock_timeout)
    } else {
        InstallationResult::new(
            OperationStatus::Failed,
            crate::lifecycle_impl::inspect(plan.lifecycle.bundle_id(), plan.lifecycle.target()),
            Vec::new(),
            plan.lifecycle
                .error()
                .cloned()
                .or_else(|| {
                    plan.lifecycle
                        .resources()
                        .iter()
                        .find_map(|item| item.error().cloned())
                })
                .or_else(|| {
                    Some(TargetError::new(
                        ErrorCode::RecoveryNeeded,
                        "Recover the Loadout source installation and preview again",
                        None,
                        None,
                    ))
                }),
            Vec::new(),
        )
    };
    LoadoutResult {
        location: plan.location.clone(),
        lifecycle,
    }
}

pub fn inspect_loadout(
    bundle_id: &str,
    location: &LoadoutLocation,
    store: Arc<dyn StateStore>,
) -> Result<InstallationObservation, ValueError> {
    crate::inspect_installation(bundle_id, &location.installation(store)?)
}

pub fn recover_loadout(
    bundle_id: &str,
    location: &LoadoutLocation,
    store: Arc<dyn StateStore>,
    lock_timeout: Duration,
) -> Result<LoadoutResult, ValueError> {
    Ok(LoadoutResult {
        location: location.clone(),
        lifecycle: crate::recover_installation(
            bundle_id,
            &location.installation(store)?,
            lock_timeout,
        )?,
    })
}
