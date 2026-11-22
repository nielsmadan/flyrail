use crate::artifacts::{Model, read_dependencies};
use crate::content::{Captured, status};
use crate::convert::absolute_path;
use crate::errors::{BindingError, BindingResult, tagged};
use crate::guard::{guarded, on_large_stack};
use crate::render::{
    NativeRenderContext, NativeRenderedBundle, wrap_rendered, write_notice, write_rendered_artifact,
};
use crate::targets::{JsPair, pair_parts};
use flyrail::{Artifact, ArtifactTranslation};
use napi::Env;
use napi::bindgen_prelude::{ClassInstance, Object, Unknown};
use napi_derive::napi;
use std::path::PathBuf;
use std::sync::Arc;

#[napi]
pub struct NativeTranslation {
    pub(crate) inner: Arc<ArtifactTranslation>,
}

#[napi]
impl NativeTranslation {
    #[napi(catch_unwind, ts_return_type = "NativeResult<TranslationWire>")]
    pub fn snapshot<'env>(&self, env: &'env Env) -> napi::Result<Object<'env>> {
        tagged(
            env,
            guarded(|| {
                status(|| -> napi::Result<Object<'env>> {
                    let translation = &self.inner;
                    let artifact = translation
                        .artifact()
                        .map(|artifact| write_rendered_artifact(env, artifact))
                        .transpose()?;
                    let notices = translation
                        .notices()
                        .iter()
                        .map(|notice| write_notice(env, notice))
                        .collect::<napi::Result<Vec<_>>>()?;
                    let mut object = Object::new(env)?;
                    object.set("artifact", artifact)?;
                    object.set("notices", notices)?;
                    object.set("supported", translation.supported())?;
                    Ok(object)
                }())
            }),
        )
    }
}

fn wrap(translation: ArtifactTranslation) -> NativeTranslation {
    NativeTranslation {
        inner: Arc::new(translation),
    }
}

fn not_artifact(kind: &str) -> BindingError {
    BindingError::Argument(format!("model is not an {kind} artifact"))
}

#[napi(
    js_name = "translateInstruction",
    catch_unwind,
    ts_return_type = "NativeResult<NativeTranslation>"
)]
pub fn translate_instruction<'env>(
    env: &'env Env,
    bundle_id: String,
    artifact: &crate::artifacts::NativeModel,
    context: &NativeRenderContext,
) -> napi::Result<Object<'env>> {
    let (model, context) = (Arc::clone(&artifact.inner), Arc::clone(&context.inner));
    tagged(
        env,
        on_large_stack(move || match &*model {
            Model::Artifact(Artifact::Instruction(instruction)) => Ok(wrap(
                flyrail::translate_instruction_artifact(&bundle_id, instruction, &context)?,
            )),
            _ => Err(not_artifact("instruction")),
        }),
    )
}

#[napi(
    js_name = "translateMcp",
    catch_unwind,
    ts_return_type = "NativeResult<NativeTranslation>"
)]
pub fn translate_mcp<'env>(
    env: &'env Env,
    artifact: &crate::artifacts::NativeModel,
    context: &NativeRenderContext,
    asset_destinations: Vec<JsPair>,
) -> napi::Result<Object<'env>> {
    let (model, context) = (Arc::clone(&artifact.inner), Arc::clone(&context.inner));
    tagged(
        env,
        on_large_stack(move || {
            let destinations = asset_destinations
                .into_iter()
                .map(pair_parts)
                .map(|(id, path)| absolute_path(&path).map(|path: PathBuf| (id, path)))
                .collect::<BindingResult<Vec<_>>>()?;
            match &*model {
                Model::Artifact(Artifact::Mcp(mcp)) => Ok(wrap(flyrail::translate_mcp_artifact(
                    mcp,
                    &context,
                    destinations,
                )?)),
                _ => Err(not_artifact("MCP")),
            }
        }),
    )
}

#[napi(
    js_name = "translationsAssemble",
    catch_unwind,
    ts_args_type = "translations: NativeTranslation[], wire: { dependencies: ModelWire[] }, contexts: NativeRenderContext[], routingContext: JsPair[]",
    ts_return_type = "NativeResult<NativeRenderedBundle>"
)]
pub fn translations_assemble<'env>(
    env: &'env Env,
    translations: Vec<ClassInstance<'_, NativeTranslation>>,
    wire: Unknown<'env>,
    contexts: Vec<ClassInstance<'_, NativeRenderContext>>,
    routing_context: Vec<JsPair>,
) -> napi::Result<Object<'env>> {
    let translations = translations
        .iter()
        .map(|translation| (*translation.inner).clone())
        .collect::<Vec<_>>();
    let contexts = contexts
        .iter()
        .map(|context| (*context.inner).clone())
        .collect::<Vec<_>>();
    let routing = routing_context
        .into_iter()
        .map(pair_parts)
        .collect::<Vec<_>>();
    let bundle: BindingResult<NativeRenderedBundle> =
        Captured::read(*env, wire).and_then(|captured| {
            on_large_stack(move || {
                let dependencies = read_dependencies(&captured.wire(), "dependencies")?;
                Ok(wrap_rendered(flyrail::assemble_translations(
                    translations,
                    dependencies,
                    &contexts,
                    routing,
                )?))
            })
        });
    tagged(env, bundle)
}
