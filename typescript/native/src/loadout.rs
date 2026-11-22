use crate::artifacts::{Model, NativeModel};
use crate::bundles::NativeBundle;
use crate::cancel::CancelToken;
use crate::content::{Wire, node, parse};
use crate::convert::{absolute_path, duration_ms};
use crate::errors::{BindingError, BindingResult, tagged};
use crate::guard::guarded;
use crate::hooks::NativeHookRuntime;
use crate::lifecycle::{
    NativeInstallationTarget, NativeLifecyclePreview, mutation, write_installation_observation,
    write_installation_result,
};
use crate::pool::{Built, spawn_promise};
use crate::stores::NativeStateStore;
use crate::targets::{JsPair, lossy};
use flyrail::{
    Agent, LoadoutDiagnostic, LoadoutDiscovery, LoadoutImpact, LoadoutLocation, LoadoutOptions,
    LoadoutPreview, LoadoutResult, LoadoutSelector, LoadoutSetup, LoadoutSource, Platform,
    StateStore,
};
use napi::Env;
use napi::bindgen_prelude::{Object, PromiseRaw};
use napi_derive::napi;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

#[napi(object)]
pub struct JsLoadoutOverride {
    pub key: String,
    pub values: Vec<String>,
}

#[napi(object)]
pub struct JsLoadoutSource {
    pub name: String,
    pub path: String,
    pub uses: Vec<String>,
    pub declared_in: String,
    pub overrides: Vec<JsLoadoutOverride>,
}

#[napi(object)]
pub struct JsLoadoutHarness {
    pub agent: String,
    pub slices: Vec<String>,
}

#[napi(object)]
pub struct JsLoadoutSelector {
    pub agent: String,
    pub category: String,
    pub manifest: String,
    pub key: Vec<String>,
    pub entries: Vec<String>,
    pub substitutions: Vec<JsPair>,
}

#[napi(object)]
pub struct JsLoadoutSetup {
    pub registry: String,
    pub root: String,
    pub profile: String,
    pub profile_chain: Vec<String>,
    pub sources: Vec<JsLoadoutSource>,
    pub harnesses: Vec<JsLoadoutHarness>,
    pub selectors: Vec<JsLoadoutSelector>,
}

#[napi(object)]
pub struct JsLoadoutDiagnostic {
    pub code: String,
    pub message: String,
    pub path: String,
}

#[napi(object)]
pub struct JsLoadoutImpact {
    pub category: String,
    pub path: String,
    pub harnesses: Vec<String>,
    pub shared: bool,
}

#[napi(object)]
pub struct JsLoadoutLocation {
    pub root: String,
    pub profile: String,
    pub source_name: String,
    pub source_path: String,
}

#[napi(object)]
pub struct JsLoadoutPreview {
    pub location: JsLoadoutLocation,
    pub impact: Vec<JsLoadoutImpact>,
    pub diagnostic: Option<JsLoadoutDiagnostic>,
    pub applicable: bool,
    pub scope_notice: String,
}

#[napi(object)]
pub struct JsLoadoutDestination {
    pub location: JsLoadoutLocation,
    pub source: JsLoadoutSource,
    pub harnesses: Vec<String>,
}

#[napi(object)]
pub struct JsLoadoutOptions {
    pub platform: String,
    pub execution_root: Option<String>,
    pub replace_modified: bool,
}

#[napi(object)]
pub struct JsLoadoutLockOptions {
    pub lock_timeout_ms: f64,
}

fn agent_names(agents: &[Agent]) -> Vec<String> {
    agents
        .iter()
        .map(|agent| agent.as_str().to_owned())
        .collect()
}

fn source_view(source: &LoadoutSource) -> JsLoadoutSource {
    JsLoadoutSource {
        name: source.name().to_owned(),
        path: lossy(source.path()),
        uses: source.uses().iter().cloned().collect(),
        declared_in: lossy(source.declared_in()),
        overrides: source
            .overrides()
            .iter()
            .map(|(key, values)| JsLoadoutOverride {
                key: key.clone(),
                values: values.iter().cloned().collect(),
            })
            .collect(),
    }
}

fn selector_view(selector: &LoadoutSelector) -> JsLoadoutSelector {
    JsLoadoutSelector {
        agent: selector.agent().as_str().to_owned(),
        category: selector.category().to_owned(),
        manifest: lossy(selector.manifest()),
        key: selector.key().to_vec(),
        entries: selector.entries().to_vec(),
        substitutions: selector
            .substitutions()
            .iter()
            .map(|(key, value)| JsPair {
                key: key.clone(),
                value: value.clone(),
            })
            .collect(),
    }
}

fn setup_view(setup: &LoadoutSetup) -> JsLoadoutSetup {
    JsLoadoutSetup {
        registry: lossy(setup.registry()),
        root: lossy(setup.root()),
        profile: setup.profile().to_owned(),
        profile_chain: setup
            .profile_chain()
            .iter()
            .map(|path| lossy(path))
            .collect(),
        sources: setup.sources().iter().map(source_view).collect(),
        harnesses: setup
            .harnesses()
            .iter()
            .map(|harness| JsLoadoutHarness {
                agent: harness.agent().as_str().to_owned(),
                slices: harness.slices().iter().cloned().collect(),
            })
            .collect(),
        selectors: setup.selectors().iter().map(selector_view).collect(),
    }
}

fn diagnostic_view(diagnostic: &LoadoutDiagnostic) -> JsLoadoutDiagnostic {
    JsLoadoutDiagnostic {
        code: diagnostic.code().to_owned(),
        message: diagnostic.message().to_owned(),
        path: lossy(diagnostic.path()),
    }
}

fn impact_view(impact: &LoadoutImpact) -> JsLoadoutImpact {
    JsLoadoutImpact {
        category: impact.category().to_owned(),
        path: lossy(impact.path()),
        harnesses: agent_names(impact.harnesses()),
        shared: impact.shared(),
    }
}

fn location_view(location: &LoadoutLocation) -> JsLoadoutLocation {
    JsLoadoutLocation {
        root: lossy(location.root()),
        profile: location.profile().to_owned(),
        source_name: location.source_name().to_owned(),
        source_path: lossy(location.source_path()),
    }
}

pub(crate) fn read_location(wire: &Wire<'_>) -> BindingResult<LoadoutLocation> {
    Ok(LoadoutLocation::new(
        absolute_path(&wire.field::<String>("root")?)?,
        wire.field::<String>("profile")?,
        wire.field::<String>("sourceName")?,
        absolute_path(&wire.field::<String>("sourcePath")?)?,
    )?)
}

pub(crate) fn write_location<'env>(
    env: &'env Env,
    location: &LoadoutLocation,
) -> napi::Result<Object<'env>> {
    let mut object = node(env, "loadout-location")?;
    object.set("root", lossy(location.root()))?;
    object.set("profile", location.profile())?;
    object.set("sourceName", location.source_name())?;
    object.set("sourcePath", lossy(location.source_path()))?;
    Ok(object)
}

fn location_of(model: &NativeModel) -> BindingResult<LoadoutLocation> {
    match &*model.inner {
        Model::Location(location) => Ok(location.clone()),
        _ => Err(BindingError::Argument(String::from(
            "model is not a Loadout location",
        ))),
    }
}

#[napi(
    js_name = "loadoutLocationInstallation",
    catch_unwind,
    ts_return_type = "NativeResult<NativeInstallationTarget>"
)]
pub fn loadout_location_installation<'env>(
    env: &'env Env,
    location: &NativeModel,
    store: &NativeStateStore,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| {
            Ok(NativeInstallationTarget {
                inner: location_of(location)?.installation(Arc::clone(&store.inner))?,
            })
        }),
    )
}

#[napi]
pub struct NativeLoadoutSetup {
    inner: Arc<LoadoutSetup>,
}

#[napi]
impl NativeLoadoutSetup {
    #[napi(catch_unwind)]
    pub fn snapshot(&self) -> JsLoadoutSetup {
        setup_view(&self.inner)
    }

    #[napi(catch_unwind, ts_return_type = "NativeResult<JsLoadoutSource[]>")]
    pub fn eligible_sources<'env>(
        &self,
        env: &'env Env,
        bundle: &NativeBundle,
        harnesses: Vec<String>,
    ) -> napi::Result<Object<'env>> {
        tagged(
            env,
            guarded(|| {
                let agents = harnesses
                    .iter()
                    .map(|name| parse::<Agent>(name))
                    .collect::<BindingResult<Vec<_>>>()?;
                Ok(self
                    .inner
                    .eligible_sources(&bundle.inner, &agents)
                    .into_iter()
                    .map(source_view)
                    .collect::<Vec<_>>())
            }),
        )
    }

    #[napi(
        catch_unwind,
        ts_return_type = "NativeResult<NativeLoadoutDestination>"
    )]
    pub fn select<'env>(
        &self,
        env: &'env Env,
        name: String,
        harnesses: Vec<String>,
        accept_shared_scope: bool,
    ) -> napi::Result<Object<'env>> {
        tagged(
            env,
            guarded(|| {
                let agents = harnesses
                    .iter()
                    .map(|name| parse::<Agent>(name))
                    .collect::<BindingResult<Vec<_>>>()?;
                let selected = self.inner.select(&name, &agents, accept_shared_scope)?;
                Ok(NativeLoadoutDestination {
                    setup: Arc::clone(&self.inner),
                    source: name,
                    agents: selected.harnesses().to_vec(),
                    accept_shared_scope,
                })
            }),
        )
    }
}

#[napi]
pub struct NativeLoadoutDestination {
    setup: Arc<LoadoutSetup>,
    source: String,
    agents: Vec<Agent>,
    accept_shared_scope: bool,
}

#[napi]
impl NativeLoadoutDestination {
    #[napi(catch_unwind, ts_return_type = "NativeResult<JsLoadoutDestination>")]
    pub fn snapshot<'env>(&self, env: &'env Env) -> napi::Result<Object<'env>> {
        tagged(
            env,
            guarded(|| {
                let selected =
                    self.setup
                        .select(&self.source, &self.agents, self.accept_shared_scope)?;
                Ok(JsLoadoutDestination {
                    location: location_view(&selected.location()),
                    source: source_view(selected.source()),
                    harnesses: agent_names(selected.harnesses()),
                })
            }),
        )
    }
}

enum Discovered {
    Absent(PathBuf),
    Available(PathBuf, Arc<LoadoutSetup>),
    Invalid(PathBuf, LoadoutDiagnostic),
}

fn write_discovery<'env>(env: &'env Env, found: &Discovered) -> napi::Result<Object<'env>> {
    let mut object = Object::new(env)?;
    match found {
        Discovered::Absent(registry) => {
            object.set("status", "absent")?;
            object.set("registry", lossy(registry))?;
        }
        Discovered::Available(registry, setup) => {
            object.set("status", "available")?;
            object.set("registry", lossy(registry))?;
            object.set(
                "setup",
                NativeLoadoutSetup {
                    inner: Arc::clone(setup),
                },
            )?;
        }
        Discovered::Invalid(registry, diagnostic) => {
            object.set("status", "invalid")?;
            object.set("registry", lossy(registry))?;
            object.set("diagnostic", diagnostic_view(diagnostic))?;
        }
    }
    Ok(object)
}

#[napi(
    js_name = "loadoutDiscover",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<{ status: 'absent' | 'available' | 'invalid'; registry: string; setup?: NativeLoadoutSetup; diagnostic?: JsLoadoutDiagnostic }>>"
)]
pub fn loadout_discover(
    env: &Env,
    home: String,
    config_home: Option<String>,
    environment: Vec<JsPair>,
) -> napi::Result<PromiseRaw<'_, Object<'static>>> {
    spawn_promise(
        env,
        None,
        move || {
            let home = absolute_path(&home)?;
            let config_home = config_home.as_deref().map(absolute_path).transpose()?;
            let environment = environment
                .into_iter()
                .map(|pair| (pair.key, pair.value))
                .collect::<BTreeMap<_, _>>();
            Ok(
                match flyrail::discover_loadout(&home, config_home.as_deref(), &environment)? {
                    LoadoutDiscovery::Absent { registry } => Discovered::Absent(registry),
                    LoadoutDiscovery::Available(setup) => {
                        Discovered::Available(setup.registry().to_owned(), Arc::from(setup))
                    }
                    LoadoutDiscovery::Invalid {
                        registry,
                        diagnostic,
                    } => Discovered::Invalid(registry, diagnostic),
                },
            )
        },
        |_, found| Ok(Built::new(found, write_discovery)),
    )
}

#[napi]
pub struct NativeLoadoutPreview {
    inner: Arc<LoadoutPreview>,
}

#[napi]
impl NativeLoadoutPreview {
    #[napi(catch_unwind)]
    pub fn snapshot(&self) -> JsLoadoutPreview {
        JsLoadoutPreview {
            location: location_view(self.inner.location()),
            impact: self.inner.impact().iter().map(impact_view).collect(),
            diagnostic: self.inner.diagnostic().map(diagnostic_view),
            applicable: self.inner.applicable(),
            scope_notice: self.inner.scope_notice().to_owned(),
        }
    }

    #[napi(catch_unwind)]
    pub fn lifecycle(&self) -> NativeLifecyclePreview {
        NativeLifecyclePreview {
            inner: Arc::new(self.inner.lifecycle().clone()),
        }
    }
}

fn wrap_preview(preview: LoadoutPreview) -> NativeLoadoutPreview {
    NativeLoadoutPreview {
        inner: Arc::new(preview),
    }
}

#[napi(
    js_name = "loadoutPreview",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<NativeLoadoutPreview>>"
)]
pub fn loadout_preview<'env>(
    env: &'env Env,
    bundle: &NativeBundle,
    destination: &NativeLoadoutDestination,
    store: &NativeStateStore,
    runtime: Option<&NativeHookRuntime>,
    options: JsLoadoutOptions,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let bundle = Arc::clone(&bundle.inner);
    let setup = Arc::clone(&destination.setup);
    let (source, agents, accept) = (
        destination.source.clone(),
        destination.agents.clone(),
        destination.accept_shared_scope,
    );
    let store: Arc<dyn StateStore> = Arc::clone(&store.inner);
    let runtime = runtime.map(|runtime| runtime.inner.clone());
    spawn_promise(
        env,
        None,
        move || {
            let destination = setup.select(&source, &agents, accept)?;
            let execution_root = options
                .execution_root
                .as_deref()
                .map(absolute_path)
                .transpose()?;
            let mut loadout = LoadoutOptions::new(parse::<Platform>(&options.platform)?);
            loadout.hook_runtime = runtime.as_ref();
            loadout.execution_root = execution_root.as_deref();
            loadout.replace_modified = options.replace_modified;
            Ok(flyrail::preview_loadout(
                &bundle,
                &destination,
                store,
                loadout,
            )?)
        },
        |_, preview| Ok(wrap_preview(preview)),
    )
}

#[napi(
    js_name = "loadoutPreviewRemoval",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<NativeLoadoutPreview>>"
)]
pub fn loadout_preview_removal<'env>(
    env: &'env Env,
    bundle_id: String,
    location: &NativeModel,
    store: &NativeStateStore,
    replace_modified: bool,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let location = location_of(location);
    let store: Arc<dyn StateStore> = Arc::clone(&store.inner);
    spawn_promise(
        env,
        None,
        move || {
            Ok(flyrail::preview_loadout_removal(
                &bundle_id,
                &location?,
                store,
                replace_modified,
            )?)
        },
        |_, preview| Ok(wrap_preview(preview)),
    )
}

fn write_result<'env>(env: &'env Env, result: &LoadoutResult) -> napi::Result<Object<'env>> {
    let mut object = Object::new(env)?;
    object.set(
        "lifecycle",
        write_installation_result(env, result.lifecycle())?,
    )?;
    object.set("syncRequired", result.sync_required())?;
    Ok(object)
}

#[napi(
    js_name = "loadoutApply",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<{ lifecycle: InstallationResultWire; syncRequired: boolean }>>"
)]
pub fn loadout_apply<'env>(
    env: &'env Env,
    preview: &NativeLoadoutPreview,
    options: JsLoadoutLockOptions,
    token: Option<&CancelToken>,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let preview = Arc::clone(&preview.inner);
    mutation(
        env,
        token,
        move || {
            Ok(flyrail::apply_loadout(
                &preview,
                duration_ms(options.lock_timeout_ms)?,
            ))
        },
        write_result,
    )
}

#[napi(
    js_name = "loadoutRecover",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<{ lifecycle: InstallationResultWire; syncRequired: boolean }>>"
)]
pub fn loadout_recover<'env>(
    env: &'env Env,
    bundle_id: String,
    location: &NativeModel,
    store: &NativeStateStore,
    options: JsLoadoutLockOptions,
    token: Option<&CancelToken>,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let location = location_of(location);
    let store: Arc<dyn StateStore> = Arc::clone(&store.inner);
    mutation(
        env,
        token,
        move || {
            Ok(flyrail::recover_loadout(
                &bundle_id,
                &location?,
                store,
                duration_ms(options.lock_timeout_ms)?,
            )?)
        },
        write_result,
    )
}

#[napi(
    js_name = "loadoutInspect",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<InstallationObservationWire>>"
)]
pub fn loadout_inspect<'env>(
    env: &'env Env,
    bundle_id: String,
    location: &NativeModel,
    store: &NativeStateStore,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let location = location_of(location);
    let store: Arc<dyn StateStore> = Arc::clone(&store.inner);
    spawn_promise(
        env,
        None,
        move || Ok(flyrail::inspect_loadout(&bundle_id, &location?, store)?),
        |_, observation| Ok(Built::new(observation, write_installation_observation)),
    )
}
