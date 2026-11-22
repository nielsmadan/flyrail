use crate::bundles::NativeBundle;
use crate::cancel::{CancelToken, run_cancellable};
use crate::content::{parse, status};
use crate::convert::duration_ms;
use crate::errors::{BindingResult, tagged};
use crate::guard::guarded;
use crate::observations::{path_texts, resource_kind, write_target_error};
use crate::pool::{Built, spawn_promise};
use crate::render::{NativeRenderContext, NativeRenderedBundle, wrap_rendered, write_notice};
use crate::stores::NativeStateStore;
use crate::targets::{JsPair, lossy, pair_parts, pair_view};
use flyrail::{
    Acquisition, ClaimObservation, InstallationObservation, InstallationResult, InstallationTarget,
    LifecyclePreview, PlanSummary, PlannedChange, ResourceObservation, ResourcePlan, ResourceRef,
    ResourceResult,
};
use napi::Env;
use napi::bindgen_prelude::{Object, PromiseRaw};
use napi_derive::napi;
use std::sync::Arc;

#[napi(object)]
pub struct JsInstallationTarget {
    pub context: Vec<JsPair>,
    pub routing_context: Vec<JsPair>,
    pub context_digest: String,
}

#[napi(object)]
pub struct JsLifecycleOptions {
    pub acquisition: String,
    pub replace_modified: bool,
    pub lock_timeout_ms: f64,
}

#[napi]
pub struct NativeInstallationTarget {
    pub(crate) inner: InstallationTarget,
}

#[napi]
#[allow(clippy::use_self)]
impl NativeInstallationTarget {
    #[napi(catch_unwind)]
    pub fn snapshot(&self) -> JsInstallationTarget {
        JsInstallationTarget {
            context: self.inner.context().iter().map(pair_view).collect(),
            routing_context: self.inner.routing_context().iter().map(pair_view).collect(),
            context_digest: self.inner.context_digest(),
        }
    }

    #[napi(catch_unwind)]
    pub fn equals(&self, other: &NativeInstallationTarget) -> bool {
        self.inner == other.inner
    }
}

#[napi(
    js_name = "installationTargetNew",
    catch_unwind,
    ts_return_type = "NativeResult<NativeInstallationTarget>"
)]
pub fn installation_target_new<'env>(
    env: &'env Env,
    store: &NativeStateStore,
    context: Vec<JsPair>,
    routing_context: Vec<JsPair>,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| {
            Ok(NativeInstallationTarget {
                inner: InstallationTarget::new(
                    Arc::clone(&store.inner),
                    context.into_iter().map(pair_parts),
                    routing_context.into_iter().map(pair_parts),
                )?,
            })
        }),
    )
}

#[napi(
    js_name = "renderContextInstallation",
    catch_unwind,
    ts_return_type = "NativeResult<NativeInstallationTarget>"
)]
pub fn render_context_installation<'env>(
    env: &'env Env,
    context: &NativeRenderContext,
    store: &NativeStateStore,
    pairs: Option<Vec<JsPair>>,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| {
            Ok(NativeInstallationTarget {
                inner: context.inner.installation(
                    Arc::clone(&store.inner),
                    pairs.map(|pairs| pairs.into_iter().map(pair_parts).collect()),
                )?,
            })
        }),
    )
}

fn write_resource_ref<'env>(env: &'env Env, resource: &ResourceRef) -> napi::Result<Object<'env>> {
    let mut object = Object::new(env)?;
    object.set("destination", lossy(resource.destination()))?;
    object.set("kind", resource_kind(resource.kind()))?;
    Ok(object)
}

fn write_resource_plan<'env>(env: &'env Env, plan: &ResourcePlan) -> napi::Result<Object<'env>> {
    let mut object = Object::new(env)?;
    object.set("resource", write_resource_ref(env, plan.resource())?)?;
    object.set("transactionDirectory", lossy(plan.transaction_directory()))?;
    object.set("recoveryPaths", path_texts(plan.recovery_paths()))?;
    object.set("requireExisting", plan.require_existing())?;
    object.set("error", write_target_error(env, plan.error())?)?;
    Ok(object)
}

fn write_preview<'env>(env: &'env Env, preview: &LifecyclePreview) -> napi::Result<Object<'env>> {
    let resources = preview
        .resources()
        .iter()
        .map(|plan| write_resource_plan(env, plan))
        .collect::<napi::Result<Vec<_>>>()?;
    let mut object = Object::new(env)?;
    object.set("bundleId", preview.bundle_id())?;
    object.set("acquisition", preview.acquisition().as_str())?;
    object.set("replaceModified", preview.replace_modified())?;
    object.set("resources", resources)?;
    object.set("error", write_target_error(env, preview.error())?)?;
    object.set("applicable", preview.applicable())?;
    Ok(object)
}

#[napi]
pub struct NativeLifecyclePreview {
    pub(crate) inner: Arc<LifecyclePreview>,
}

#[napi]
impl NativeLifecyclePreview {
    #[napi(catch_unwind, ts_return_type = "NativeResult<PreviewWire>")]
    pub fn snapshot<'env>(&self, env: &'env Env) -> napi::Result<Object<'env>> {
        tagged(env, guarded(|| status(write_preview(env, &self.inner))))
    }

    #[napi(catch_unwind)]
    pub fn rendered(&self) -> NativeRenderedBundle {
        wrap_rendered(self.inner.rendered().clone())
    }
}

#[napi]
pub struct NativeInstallationObservation {
    pub(crate) inner: Arc<InstallationObservation>,
}

#[napi]
impl NativeInstallationObservation {
    #[napi(catch_unwind)]
    pub fn matches(&self, bundle: &NativeBundle, rendered: &NativeRenderedBundle) -> bool {
        self.inner.matches(&bundle.inner, &rendered.inner)
    }
}

fn write_claim_observation<'env>(
    env: &'env Env,
    claim: &ClaimObservation,
) -> napi::Result<Object<'env>> {
    let mut object = Object::new(env)?;
    object.set("claimId", claim.claim_id())?;
    object.set("bundleId", claim.bundle_id())?;
    object.set("artifactId", claim.artifact_id())?;
    object.set("version", claim.version())?;
    object.set("bundleDigest", claim.bundle_digest())?;
    object.set("renderDigest", claim.render_digest())?;
    object.set("status", claim.status().as_str())?;
    Ok(object)
}

fn write_resource_observation<'env>(
    env: &'env Env,
    resource: &ResourceObservation,
) -> napi::Result<Object<'env>> {
    let claims = resource
        .claims()
        .iter()
        .map(|claim| write_claim_observation(env, claim))
        .collect::<napi::Result<Vec<_>>>()?;
    let mut object = Object::new(env)?;
    object.set("destination", lossy(resource.destination()))?;
    object.set("claims", claims)?;
    object.set("revisionDigest", resource.revision_digest())?;
    object.set("error", write_target_error(env, resource.error())?)?;
    object.set("recoveryPaths", path_texts(resource.recovery_paths()))?;
    Ok(object)
}

pub(crate) fn write_installation_observation<'env>(
    env: &'env Env,
    observation: &InstallationObservation,
) -> napi::Result<Object<'env>> {
    let resources = observation
        .resources()
        .iter()
        .map(|resource| write_resource_observation(env, resource))
        .collect::<napi::Result<Vec<_>>>()?;
    let mut object = Object::new(env)?;
    object.set(
        "handle",
        NativeInstallationObservation {
            inner: Arc::new(observation.clone()),
        },
    )?;
    object.set("bundleId", observation.bundle_id())?;
    object.set("resources", resources)?;
    object.set("isCurrent", observation.is_current())?;
    object.set("pending", observation.pending())?;
    object.set("version", observation.version())?;
    object.set("bundleDigest", observation.bundle_digest())?;
    object.set("renderDigest", observation.render_digest())?;
    object.set("error", write_target_error(env, observation.error())?)?;
    Ok(object)
}

fn write_resource_result<'env>(
    env: &'env Env,
    result: &ResourceResult,
) -> napi::Result<Object<'env>> {
    let mut object = Object::new(env)?;
    object.set("resource", write_resource_ref(env, result.resource())?)?;
    object.set("status", result.status().as_str())?;
    object.set("error", write_target_error(env, result.error())?)?;
    object.set("recoveryPaths", path_texts(result.recovery_paths()))?;
    Ok(object)
}

pub(crate) fn write_installation_result<'env>(
    env: &'env Env,
    result: &InstallationResult,
) -> napi::Result<Object<'env>> {
    let resources = result
        .resources()
        .iter()
        .map(|resource| write_resource_result(env, resource))
        .collect::<napi::Result<Vec<_>>>()?;
    let notices = result
        .notices()
        .iter()
        .map(|notice| write_notice(env, notice))
        .collect::<napi::Result<Vec<_>>>()?;
    let mut object = Object::new(env)?;
    object.set("status", result.status().as_str())?;
    object.set(
        "observation",
        write_installation_observation(env, result.observation())?,
    )?;
    object.set("resources", resources)?;
    object.set("notices", notices)?;
    object.set("error", write_target_error(env, result.error())?)?;
    Ok(object)
}

fn wrap_preview(preview: LifecyclePreview) -> NativeLifecyclePreview {
    NativeLifecyclePreview {
        inner: Arc::new(preview),
    }
}

pub(crate) fn mutation<'env, T: Send + 'static>(
    env: &'env Env,
    token: Option<&CancelToken>,
    work: impl FnOnce() -> BindingResult<T> + Send + 'static,
    write: crate::pool::Writer<T>,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let token = token.map(|token| token.inner.clone());
    let cancellation = token.clone();
    spawn_promise(
        env,
        token,
        move || run_cancellable(cancellation.as_ref(), work),
        move |_, value| Ok(Built::new(value, write)),
    )
}

#[napi(
    js_name = "lifecyclePreview",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<NativeLifecyclePreview>>"
)]
pub fn lifecycle_preview<'env>(
    env: &'env Env,
    bundle: &NativeBundle,
    rendered: &NativeRenderedBundle,
    target: &NativeInstallationTarget,
    options: JsLifecycleOptions,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let (bundle, rendered) = (Arc::clone(&bundle.inner), Arc::clone(&rendered.inner));
    let target = target.inner.clone();
    spawn_promise(
        env,
        None,
        move || {
            Ok(flyrail::preview(
                &bundle,
                &rendered,
                &target,
                parse::<Acquisition>(&options.acquisition)?,
                options.replace_modified,
            )?)
        },
        |_, preview| Ok(wrap_preview(preview)),
    )
}

#[napi(
    js_name = "lifecyclePreviewRemoval",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<NativeLifecyclePreview>>"
)]
pub fn lifecycle_preview_removal<'env>(
    env: &'env Env,
    bundle_id: String,
    target: &NativeInstallationTarget,
    options: JsLifecycleOptions,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let target = target.inner.clone();
    spawn_promise(
        env,
        None,
        move || {
            Ok(flyrail::preview_removal(
                &bundle_id,
                &target,
                options.replace_modified,
            )?)
        },
        |_, preview| Ok(wrap_preview(preview)),
    )
}

#[napi(
    js_name = "lifecycleApplyPreview",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<InstallationResultWire>>"
)]
pub fn lifecycle_apply_preview<'env>(
    env: &'env Env,
    preview: &NativeLifecyclePreview,
    options: JsLifecycleOptions,
    token: Option<&CancelToken>,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let preview = Arc::clone(&preview.inner);
    mutation(
        env,
        token,
        move || {
            Ok(flyrail::apply_preview(
                &preview,
                duration_ms(options.lock_timeout_ms)?,
            ))
        },
        write_installation_result,
    )
}

#[napi(
    js_name = "lifecycleSync",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<InstallationResultWire>>"
)]
pub fn lifecycle_sync<'env>(
    env: &'env Env,
    bundle: &NativeBundle,
    rendered: &NativeRenderedBundle,
    target: &NativeInstallationTarget,
    options: JsLifecycleOptions,
    token: Option<&CancelToken>,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let (bundle, rendered) = (Arc::clone(&bundle.inner), Arc::clone(&rendered.inner));
    let target = target.inner.clone();
    mutation(
        env,
        token,
        move || {
            Ok(flyrail::sync(
                &bundle,
                &rendered,
                &target,
                parse::<Acquisition>(&options.acquisition)?,
                options.replace_modified,
                duration_ms(options.lock_timeout_ms)?,
            )?)
        },
        write_installation_result,
    )
}

#[napi(
    js_name = "lifecycleRecoverInstallation",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<InstallationResultWire>>"
)]
pub fn lifecycle_recover_installation<'env>(
    env: &'env Env,
    bundle_id: String,
    target: &NativeInstallationTarget,
    options: JsLifecycleOptions,
    token: Option<&CancelToken>,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let target = target.inner.clone();
    mutation(
        env,
        token,
        move || {
            Ok(flyrail::recover_installation(
                &bundle_id,
                &target,
                duration_ms(options.lock_timeout_ms)?,
            )?)
        },
        write_installation_result,
    )
}

#[napi(
    js_name = "lifecycleRemove",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<InstallationResultWire>>"
)]
pub fn lifecycle_remove<'env>(
    env: &'env Env,
    bundle_id: String,
    target: &NativeInstallationTarget,
    options: JsLifecycleOptions,
    token: Option<&CancelToken>,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let target = target.inner.clone();
    mutation(
        env,
        token,
        move || {
            Ok(flyrail::remove(
                &bundle_id,
                &target,
                options.replace_modified,
                duration_ms(options.lock_timeout_ms)?,
            )?)
        },
        write_installation_result,
    )
}

#[napi(
    js_name = "lifecycleInspectInstallation",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<InstallationObservationWire>>"
)]
pub fn lifecycle_inspect_installation<'env>(
    env: &'env Env,
    bundle_id: String,
    target: &NativeInstallationTarget,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let target = target.inner.clone();
    spawn_promise(
        env,
        None,
        move || Ok(flyrail::inspect_installation(&bundle_id, &target)?),
        |_, observation| Ok(Built::new(observation, write_installation_observation)),
    )
}

fn write_planned_change<'env>(
    env: &'env Env,
    change: &PlannedChange,
) -> napi::Result<Object<'env>> {
    let mut object = Object::new(env)?;
    object.set("artifactIds", change.artifact_ids().to_vec())?;
    object.set(
        "families",
        change
            .families()
            .iter()
            .map(|family| family.as_str())
            .collect::<Vec<_>>(),
    )?;
    object.set("kind", resource_kind(change.kind()))?;
    object.set("destination", lossy(change.destination()))?;
    object.set("action", change.action().as_str())?;
    object.set("error", write_target_error(env, change.error())?)?;
    object.set("recoveryPaths", path_texts(change.recovery_paths()))?;
    Ok(object)
}

fn write_summary<'env>(env: &'env Env, summary: &PlanSummary) -> napi::Result<Object<'env>> {
    let changes = summary
        .changes()
        .iter()
        .map(|change| write_planned_change(env, change))
        .collect::<napi::Result<Vec<_>>>()?;
    let notices = summary
        .notices()
        .iter()
        .map(|notice| write_notice(env, notice))
        .collect::<napi::Result<Vec<_>>>()?;
    let mut object = Object::new(env)?;
    object.set("agent", summary.agent().map(flyrail::Agent::as_str))?;
    object.set("scope", summary.scope().map(flyrail::TargetScope::as_str))?;
    object.set("surface", summary.surface().map(flyrail::Surface::as_str))?;
    object.set("applicable", summary.applicable())?;
    object.set("changes", changes)?;
    object.set("notices", notices)?;
    object.set("error", write_target_error(env, summary.error())?)?;
    Ok(object)
}

#[napi(
    js_name = "lifecycleSummarize",
    catch_unwind,
    ts_return_type = "NativeResult<PlanSummaryWire>"
)]
pub fn lifecycle_summarize<'env>(
    env: &'env Env,
    preview: &NativeLifecyclePreview,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| {
            let summary = flyrail::summarize(&preview.inner)?;
            status(write_summary(env, &summary))
        }),
    )
}
