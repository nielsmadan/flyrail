use crate::artifacts::{read_dependencies, write_dependencies};
use crate::bundles::NativeBundle;
use crate::content::{
    Captured, Wire, node, parse, read_content, read_schema, status, write_content, write_schema,
};
use crate::convert::absolute_path;
use crate::errors::{BindingResult, tagged};
use crate::guard::{guarded, on_large_stack};
use crate::hooks::NativeHookRuntime;
use crate::targets::{JsPair, NativeTarget, lossy, pair_view};
use flyrail::{
    Agent, Capability, DocumentSchema, Family, Notice, NoticeKind, Platform, RenderContext,
    RenderedArtifact, RenderedBundle, Surface,
};
use napi::Env;
use napi::bindgen_prelude::{ClassInstance, Object, Unknown};
use napi_derive::napi;
use std::path::PathBuf;
use std::sync::Arc;

#[napi(object)]
pub struct JsRenderOptions {
    pub execution_root: Option<String>,
    pub instruction_path: Option<String>,
    pub mcp_path: Option<String>,
    pub native_root: Option<String>,
    pub asset_root: Option<String>,
}

#[napi(object)]
pub struct JsRenderContext {
    pub agent: String,
    pub scope: String,
    pub surface: String,
    pub platform: String,
    pub root: String,
    pub skills: String,
    pub execution_root: Option<String>,
    pub instruction_path: Option<String>,
    pub mcp_path: Option<String>,
    pub native_root: Option<String>,
    pub asset_root: Option<String>,
    pub routing_context: Vec<JsPair>,
}

#[napi(object)]
pub struct JsCapability {
    pub family: String,
    pub portable: bool,
    pub native: bool,
    pub limitations: Vec<String>,
    pub prerequisites: Vec<String>,
}

#[napi]
pub struct NativeRenderContext {
    pub(crate) inner: Arc<RenderContext>,
}

fn optional_path(path: Option<&std::path::Path>) -> Option<String> {
    path.map(lossy)
}

#[napi]
impl NativeRenderContext {
    #[napi(catch_unwind)]
    pub fn snapshot(&self) -> JsRenderContext {
        let context = &self.inner;
        let audience = context.audience();
        JsRenderContext {
            agent: audience.agent().as_str().to_owned(),
            scope: audience.scope().as_str().to_owned(),
            surface: context.surface().as_str().to_owned(),
            platform: context.platform().as_str().to_owned(),
            root: lossy(context.root()),
            skills: lossy(&context.skills()),
            execution_root: optional_path(context.execution_root()),
            instruction_path: optional_path(context.instruction_path()),
            mcp_path: optional_path(context.mcp_path()),
            native_root: optional_path(context.native_root()),
            asset_root: optional_path(context.asset_root()),
            routing_context: context.routing_context().iter().map(pair_view).collect(),
        }
    }
}

fn with_option(
    context: RenderContext,
    path: Option<String>,
    apply: impl FnOnce(RenderContext, PathBuf) -> Result<RenderContext, flyrail::ValueError>,
) -> BindingResult<RenderContext> {
    match path {
        Some(path) => Ok(apply(context, absolute_path(&path)?)?),
        None => Ok(context),
    }
}

#[napi(
    js_name = "renderContextNew",
    catch_unwind,
    ts_return_type = "NativeResult<NativeRenderContext>"
)]
pub fn render_context_new<'env>(
    env: &'env Env,
    target: &NativeTarget,
    platform: String,
    surface: String,
    options: JsRenderOptions,
    runtime: Option<&NativeHookRuntime>,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| {
            let context = RenderContext::with_surface(
                target.inner.clone(),
                parse::<Platform>(&platform)?,
                parse::<Surface>(&surface)?,
            )?;
            let context = with_option(
                context,
                options.execution_root,
                RenderContext::with_execution_root,
            )?;
            let context = with_option(
                context,
                options.instruction_path,
                RenderContext::with_instruction_path,
            )?;
            let context = with_option(context, options.mcp_path, RenderContext::with_mcp_path)?;
            let context = with_option(
                context,
                options.native_root,
                RenderContext::with_native_root,
            )?;
            let context = with_option(context, options.asset_root, RenderContext::with_asset_root)?;
            let context = match runtime {
                Some(runtime) => context.with_hook_runtime(runtime.inner.clone()),
                None => context,
            };
            Ok(NativeRenderContext {
                inner: Arc::new(context),
            })
        }),
    )
}

#[napi(
    js_name = "renderInstructionDestination",
    catch_unwind,
    ts_return_type = "NativeResult<string | null>"
)]
pub fn render_instruction_destination<'env>(
    env: &'env Env,
    context: &NativeRenderContext,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| {
            Ok(optional_path(
                flyrail::instruction_destination(&context.inner)?.as_deref(),
            ))
        }),
    )
}

#[napi(
    js_name = "renderMcpDestination",
    catch_unwind,
    ts_return_type = "NativeResult<string | null>"
)]
pub fn render_mcp_destination<'env>(
    env: &'env Env,
    context: &NativeRenderContext,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| {
            Ok(optional_path(
                flyrail::mcp_destination(&context.inner)?.as_deref(),
            ))
        }),
    )
}

fn capability_view(capability: &Capability) -> JsCapability {
    JsCapability {
        family: capability.family().as_str().to_owned(),
        portable: capability.portable(),
        native: capability.native(),
        limitations: capability.limitations().to_vec(),
        prerequisites: capability.prerequisites().to_vec(),
    }
}

#[napi(
    js_name = "renderCapabilities",
    catch_unwind,
    ts_return_type = "NativeResult<JsCapability[]>"
)]
pub fn render_capabilities<'env>(
    env: &'env Env,
    context: &NativeRenderContext,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| {
            Ok(flyrail::capabilities(&context.inner)?
                .iter()
                .map(capability_view)
                .collect::<Vec<_>>())
        }),
    )
}

#[napi(
    js_name = "renderRequireTranslatable",
    catch_unwind,
    ts_return_type = "NativeResult<undefined>"
)]
pub fn render_require_translatable<'env>(
    env: &'env Env,
    context: &NativeRenderContext,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| Ok(flyrail::require_translatable(&context.inner)?)),
    )
}

#[napi(
    js_name = "renderTranslatableAgents",
    catch_unwind,
    ts_return_type = "NativeResult<string[]>"
)]
pub fn render_translatable_agents(env: &Env) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            Ok(flyrail::TRANSLATABLE_AGENTS
                .iter()
                .map(|agent| agent.as_str().to_owned())
                .collect::<Vec<_>>())
        }),
    )
}

#[napi(
    js_name = "renderIsTranslatable",
    catch_unwind,
    ts_return_type = "NativeResult<boolean>"
)]
pub fn render_is_translatable(env: &Env, agent: String) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| Ok(flyrail::is_translatable(parse::<Agent>(&agent)?))),
    )
}

#[napi(
    js_name = "renderUntranslatableMessage",
    catch_unwind,
    ts_return_type = "NativeResult<string>"
)]
pub fn render_untranslatable_message(env: &Env, agent: String) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| Ok(flyrail::untranslatable_message(parse::<Agent>(&agent)?))),
    )
}

fn read_notice(wire: &Wire<'_>) -> BindingResult<Notice> {
    Ok(Notice::with_code(
        wire.field::<String>("artifactId")?,
        parse::<NoticeKind>(&wire.field::<String>("noticeKind")?)?,
        wire.field::<String>("message")?,
        wire.optional::<String>("code")?.unwrap_or_default(),
    )?)
}

pub(crate) fn write_notice<'env>(env: &'env Env, notice: &Notice) -> napi::Result<Object<'env>> {
    let mut object = node(env, "notice")?;
    object.set("artifactId", notice.artifact_id())?;
    object.set("noticeKind", notice.kind().as_str())?;
    object.set("message", notice.message())?;
    object.set("code", Some(notice.code()).filter(|code| !code.is_empty()))?;
    Ok(object)
}

fn read_pairs(wire: &Wire<'_>, name: &str) -> BindingResult<Vec<(String, String)>> {
    wire.children(name)?
        .iter()
        .map(|pair| Ok((pair.field::<String>("key")?, pair.field::<String>("value")?)))
        .collect()
}

fn write_pairs<'env>(
    env: &'env Env,
    pairs: &[(String, String)],
) -> napi::Result<Vec<Object<'env>>> {
    pairs
        .iter()
        .map(|(key, value)| {
            let mut object = Object::new(env)?;
            object.set("key", key.as_str())?;
            object.set("value", value.as_str())?;
            Ok(object)
        })
        .collect()
}

fn read_rendered_artifact(wire: &Wire<'_>) -> BindingResult<RenderedArtifact> {
    let schemas = wire
        .children("schemaRequirements")?
        .iter()
        .map(read_schema)
        .collect::<BindingResult<Vec<DocumentSchema>>>()?;
    Ok(RenderedArtifact::new(
        wire.field::<String>("id")?,
        parse::<Family>(&wire.field::<String>("family")?)?,
        PathBuf::from(wire.field::<String>("destination")?),
        read_content(&wire.child("content")?)?,
        wire.optional::<String>("subtree")?,
        wire.field::<bool>("requireExisting")?,
        schemas,
    )?)
}

pub(crate) fn write_rendered_artifact<'env>(
    env: &'env Env,
    artifact: &RenderedArtifact,
) -> napi::Result<Object<'env>> {
    let schemas = artifact
        .schema_requirements()
        .iter()
        .map(|schema| write_schema(env, schema))
        .collect::<napi::Result<Vec<_>>>()?;
    let mut object = node(env, "rendered-artifact")?;
    object.set("id", artifact.id())?;
    object.set("family", artifact.family().as_str())?;
    object.set("destination", lossy(artifact.destination()))?;
    object.set("content", write_content(env, artifact.content())?)?;
    object.set("subtree", artifact.subtree())?;
    object.set("requireExisting", artifact.require_existing())?;
    object.set("schemaRequirements", schemas)?;
    Ok(object)
}

pub(crate) fn wrap_rendered(bundle: RenderedBundle) -> NativeRenderedBundle {
    NativeRenderedBundle {
        inner: Arc::new(bundle),
    }
}

#[napi]
pub struct NativeRenderedBundle {
    pub(crate) inner: Arc<RenderedBundle>,
}

#[napi]
impl NativeRenderedBundle {
    #[napi(catch_unwind, ts_return_type = "NativeResult<RenderedBundleWire>")]
    pub fn snapshot<'env>(&self, env: &'env Env) -> napi::Result<Object<'env>> {
        tagged(
            env,
            guarded(|| {
                status(|| -> napi::Result<Object<'env>> {
                    let bundle = &self.inner;
                    let artifacts = bundle
                        .artifacts()
                        .iter()
                        .map(|artifact| write_rendered_artifact(env, artifact))
                        .collect::<napi::Result<Vec<_>>>()?;
                    let notices = bundle
                        .notices()
                        .iter()
                        .map(|notice| write_notice(env, notice))
                        .collect::<napi::Result<Vec<_>>>()?;
                    let mut object = Object::new(env)?;
                    object.set("artifacts", artifacts)?;
                    object.set(
                        "dependencies",
                        write_dependencies(env, bundle.dependencies())?,
                    )?;
                    object.set("notices", notices)?;
                    object.set(
                        "routingContext",
                        write_pairs(env, bundle.routing_context())?,
                    )?;
                    object.set("contentDigest", bundle.content_digest())?;
                    object.set("supported", bundle.supported())?;
                    Ok(object)
                }())
            }),
        )
    }
}

#[napi(
    js_name = "renderedArtifactBuild",
    catch_unwind,
    ts_args_type = "wire: RenderedArtifactWire",
    ts_return_type = "NativeResult<RenderedArtifactWire>"
)]
pub fn rendered_artifact_build<'env>(
    env: &'env Env,
    wire: Unknown<'env>,
) -> napi::Result<Object<'env>> {
    let artifact = Captured::read(*env, wire)
        .and_then(|captured| on_large_stack(move || read_rendered_artifact(&captured.wire())));
    tagged(
        env,
        artifact.and_then(|artifact| status(write_rendered_artifact(env, &artifact))),
    )
}

#[napi(
    js_name = "renderedBundleBuild",
    catch_unwind,
    ts_args_type = "wire: RenderedBundleInputWire",
    ts_return_type = "NativeResult<NativeRenderedBundle>"
)]
pub fn rendered_bundle_build<'env>(
    env: &'env Env,
    wire: Unknown<'env>,
) -> napi::Result<Object<'env>> {
    let bundle = Captured::read(*env, wire).and_then(|captured| {
        on_large_stack(move || {
            let wire = captured.wire();
            let artifacts = wire
                .children("artifacts")?
                .iter()
                .map(read_rendered_artifact)
                .collect::<BindingResult<Vec<_>>>()?;
            let notices = wire
                .children("notices")?
                .iter()
                .map(read_notice)
                .collect::<BindingResult<Vec<_>>>()?;
            Ok(wrap_rendered(RenderedBundle::with_notices(
                artifacts,
                read_dependencies(&wire, "dependencies")?,
                notices,
                read_pairs(&wire, "routingContext")?,
            )?))
        })
    });
    tagged(env, bundle)
}

#[napi(
    js_name = "renderedBundleEmpty",
    catch_unwind,
    ts_return_type = "NativeResult<NativeRenderedBundle>"
)]
pub fn rendered_bundle_empty(env: &Env) -> napi::Result<Object<'_>> {
    tagged(env, guarded(|| Ok(wrap_rendered(RenderedBundle::empty()))))
}

#[napi(
    js_name = "renderBundle",
    catch_unwind,
    ts_return_type = "NativeResult<NativeRenderedBundle>"
)]
pub fn render_bundle<'env>(
    env: &'env Env,
    bundle: &NativeBundle,
    context: &NativeRenderContext,
) -> napi::Result<Object<'env>> {
    let (bundle, context) = (Arc::clone(&bundle.inner), Arc::clone(&context.inner));
    tagged(
        env,
        on_large_stack(move || Ok(wrap_rendered(flyrail::render(&bundle, &context)?))),
    )
}

#[napi(
    js_name = "renderBundleMany",
    catch_unwind,
    ts_return_type = "NativeResult<NativeRenderedBundle>"
)]
pub fn render_bundle_many<'env>(
    env: &'env Env,
    bundle: &NativeBundle,
    contexts: Vec<ClassInstance<'_, NativeRenderContext>>,
) -> napi::Result<Object<'env>> {
    let bundle = Arc::clone(&bundle.inner);
    let contexts = contexts
        .iter()
        .map(|context| (*context.inner).clone())
        .collect::<Vec<_>>();
    tagged(
        env,
        on_large_stack(move || Ok(wrap_rendered(flyrail::render_many(&bundle, contexts)?))),
    )
}
