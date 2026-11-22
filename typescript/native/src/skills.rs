use crate::bundles::NativeBundle;
use crate::cancel::CancelToken;
use crate::convert::duration_ms;
use crate::lifecycle::mutation;
use crate::observations::{write_inspections, write_target_results};
use crate::pool::{Built, spawn_promise};
use crate::render::wrap_rendered;
use crate::stores::NativeStateStore;
use crate::targets::NativeTarget;
use flyrail::{SkillOptions, StateStore, Target};
use napi::Env;
use napi::bindgen_prelude::{ClassInstance, Object, PromiseRaw};
use napi_derive::napi;
use std::sync::Arc;

#[napi(object)]
pub struct JsSkillOptions {
    pub replace_modified: bool,
    pub lock_timeout_ms: f64,
}

fn targets_of(targets: &[ClassInstance<'_, NativeTarget>]) -> Vec<Target> {
    targets.iter().map(|target| target.inner.clone()).collect()
}

#[napi(
    js_name = "skillsRender",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<NativeRenderedBundle>>"
)]
pub fn skills_render<'env>(
    env: &'env Env,
    bundle: &NativeBundle,
    target: &NativeTarget,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let bundle = Arc::clone(&bundle.inner);
    let target = target.inner.clone();
    spawn_promise(
        env,
        None,
        move || Ok(flyrail::render_skills(&bundle, &target)?),
        |_, rendered| Ok(wrap_rendered(rendered)),
    )
}

#[napi(
    js_name = "skillsInspect",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<{ items: SkillInspectionWire[] }>>"
)]
pub fn skills_inspect<'env>(
    env: &'env Env,
    bundle: &NativeBundle,
    targets: Vec<ClassInstance<'_, NativeTarget>>,
    store: &NativeStateStore,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let bundle = Arc::clone(&bundle.inner);
    let targets = targets_of(&targets);
    let store: Arc<dyn StateStore> = Arc::clone(&store.inner);
    spawn_promise(
        env,
        None,
        move || Ok(flyrail::inspect(&bundle, targets, &store)?),
        |_, inspections| Ok(Built::new(inspections, write_inspections)),
    )
}

#[napi(
    js_name = "skillsInstall",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<{ items: SkillResultWire[] }>>"
)]
pub fn skills_install<'env>(
    env: &'env Env,
    bundle: &NativeBundle,
    targets: Vec<ClassInstance<'_, NativeTarget>>,
    store: &NativeStateStore,
    options: JsSkillOptions,
    token: Option<&CancelToken>,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let bundle = Arc::clone(&bundle.inner);
    let targets = targets_of(&targets);
    let store: Arc<dyn StateStore> = Arc::clone(&store.inner);
    mutation(
        env,
        token,
        move || {
            Ok(flyrail::install_with_timeout(
                &bundle,
                targets,
                &store,
                duration_ms(options.lock_timeout_ms)?,
            )?)
        },
        write_target_results,
    )
}

#[napi(
    js_name = "skillsUpdate",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<{ items: SkillResultWire[] }>>"
)]
pub fn skills_update<'env>(
    env: &'env Env,
    bundle: &NativeBundle,
    targets: Vec<ClassInstance<'_, NativeTarget>>,
    store: &NativeStateStore,
    options: JsSkillOptions,
    token: Option<&CancelToken>,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let bundle = Arc::clone(&bundle.inner);
    let targets = targets_of(&targets);
    let store: Arc<dyn StateStore> = Arc::clone(&store.inner);
    mutation(
        env,
        token,
        move || {
            Ok(flyrail::update_with_options(
                &bundle,
                targets,
                &store,
                SkillOptions::new(
                    options.replace_modified,
                    duration_ms(options.lock_timeout_ms)?,
                ),
            )?)
        },
        write_target_results,
    )
}

#[napi(
    js_name = "skillsUninstall",
    catch_unwind,
    ts_return_type = "Promise<NativeResult<{ items: SkillResultWire[] }>>"
)]
pub fn skills_uninstall<'env>(
    env: &'env Env,
    bundle_id: String,
    targets: Vec<ClassInstance<'_, NativeTarget>>,
    store: &NativeStateStore,
    options: JsSkillOptions,
    token: Option<&CancelToken>,
) -> napi::Result<PromiseRaw<'env, Object<'static>>> {
    let targets = targets_of(&targets);
    let store: Arc<dyn StateStore> = Arc::clone(&store.inner);
    mutation(
        env,
        token,
        move || {
            Ok(flyrail::uninstall_with_options(
                &bundle_id,
                targets,
                &store,
                SkillOptions::new(
                    options.replace_modified,
                    duration_ms(options.lock_timeout_ms)?,
                ),
            )?)
        },
        write_target_results,
    )
}
