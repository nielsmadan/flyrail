use crate::bundles::{JsBundleEntry, entry_view};
use crate::content::parse;
use crate::convert::{absolute_path, whole};
use crate::errors::{BindingError, BindingResult, outside_i64, tagged};
use crate::guard::{guarded, on_large_stack};
use flyrail::hooks::{
    ERROR_LIMIT, EVENT_TIMEOUT_LIMIT_MS, HANDLER_LIMIT, INPUT_LIMIT, JSON_DEPTH_LIMIT,
    OUTPUT_LIMIT, TEXT_LIMIT,
};
use flyrail::{
    HookConfig, HookEffects, HookEnvironment, HookEvent, HookHandler, HookJson, HookOutcome,
    HookRequest, HookResponse, HookRuntime, HookShell, HookText,
};
use napi::Env;
use napi::bindgen_prelude::{BigInt, ClassInstance, Object, Uint8Array, Uint16Array};
use napi_derive::napi;
use std::sync::Arc;

#[napi(object)]
pub struct JsHookRuntime {
    pub node: Option<String>,
    pub shell: Option<String>,
    pub resources: Vec<JsBundleEntry>,
}

#[napi]
pub struct NativeHookRuntime {
    pub(crate) inner: HookRuntime,
}

#[napi]
impl NativeHookRuntime {
    #[napi(catch_unwind)]
    pub fn snapshot(&self) -> JsHookRuntime {
        JsHookRuntime {
            node: self
                .inner
                .node()
                .map(|node| node.to_string_lossy().into_owned()),
            shell: self.inner.shell().map(|shell| shell.as_str().to_owned()),
            resources: self.inner.resources().iter().map(entry_view).collect(),
        }
    }
}

#[napi(
    js_name = "hookRuntimeNew",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookRuntime>"
)]
pub fn hook_runtime_new(
    env: &Env,
    node: Option<String>,
    shell: Option<String>,
) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            let node = node.as_deref().map(absolute_path).transpose()?;
            let shell = shell.as_deref().map(parse::<HookShell>).transpose()?;
            Ok(NativeHookRuntime {
                inner: HookRuntime::new(node, shell)?,
            })
        }),
    )
}

#[napi(object)]
pub struct JsHookLimits {
    pub input: u32,
    pub output: u32,
    pub error: u32,
    pub text: u32,
    pub json_depth: u32,
    pub handler: u32,
    pub event_timeout_ms: u32,
}

fn limit(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[napi(js_name = "hookLimits", catch_unwind)]
pub fn hook_limits() -> JsHookLimits {
    JsHookLimits {
        input: limit(INPUT_LIMIT),
        output: limit(OUTPUT_LIMIT),
        error: limit(ERROR_LIMIT),
        text: limit(TEXT_LIMIT),
        json_depth: limit(JSON_DEPTH_LIMIT),
        handler: limit(HANDLER_LIMIT),
        event_timeout_ms: EVENT_TIMEOUT_LIMIT_MS,
    }
}

fn text_of(units: &Uint16Array) -> HookText {
    HookText::from_utf16(units.to_vec())
}

fn units_of(text: &HookText) -> Uint16Array {
    Uint16Array::new(text.units().to_vec())
}

fn event_of(text: &str) -> BindingResult<HookEvent> {
    crate::content::parse(text)
}

fn outcomes_of(names: &[String]) -> BindingResult<Vec<HookOutcome>> {
    names
        .iter()
        .map(|name| crate::content::parse::<HookOutcome>(name))
        .collect()
}

#[napi]
pub struct NativeHookJson {
    pub(crate) inner: Arc<HookJson>,
}

fn json(value: HookJson) -> NativeHookJson {
    NativeHookJson {
        inner: Arc::new(value),
    }
}

fn owned(value: &NativeHookJson) -> HookJson {
    (*value.inner).clone()
}

#[napi]
impl NativeHookJson {
    #[napi(catch_unwind)]
    pub fn is_object(&self) -> bool {
        self.inner.is_object()
    }

    #[napi(catch_unwind)]
    pub fn text(&self) -> Option<Uint16Array> {
        self.inner.as_text().map(units_of)
    }

    #[napi(catch_unwind)]
    pub fn bytes(&self) -> Uint8Array {
        Uint8Array::from(self.inner.to_bytes())
    }
}

#[napi(
    js_name = "hookJsonParse",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookJson>"
)]
pub fn hook_json_parse(env: &Env, data: Uint8Array) -> napi::Result<Object<'_>> {
    let data = data.to_vec();
    tagged(
        env,
        on_large_stack(move || Ok(json(HookJson::parse(&data)?))),
    )
}

#[napi(
    js_name = "hookJsonObject",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookJson>"
)]
pub fn hook_json_object<'env>(
    env: &'env Env,
    keys: Vec<String>,
    values: Vec<ClassInstance<'_, NativeHookJson>>,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| {
            let members = keys
                .into_iter()
                .zip(values.iter().map(|value| owned(value)));
            Ok(json(HookJson::object(members)?))
        }),
    )
}

#[napi(
    js_name = "hookJsonString",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookJson>"
)]
pub fn hook_json_string(env: &Env, units: Uint16Array) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| Ok(json(HookJson::string(text_of(&units))?))),
    )
}

#[napi(
    js_name = "hookJsonBoolean",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookJson>"
)]
pub fn hook_json_boolean(env: &Env, value: bool) -> napi::Result<Object<'_>> {
    tagged(env, guarded(|| Ok(json(HookJson::boolean(value)?))))
}

#[napi(
    js_name = "hookJsonInteger",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookJson>"
)]
pub fn hook_json_integer(env: &Env, value: BigInt) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            let (integer, lossless) = value.get_i64();
            if lossless {
                Ok(json(HookJson::integer(integer)?))
            } else {
                Err(outside_i64())
            }
        }),
    )
}

#[napi(
    js_name = "hookJsonArray",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookJson>"
)]
pub fn hook_json_array<'env>(
    env: &'env Env,
    items: Vec<ClassInstance<'_, NativeHookJson>>,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| Ok(json(HookJson::array(items.iter().map(|item| owned(item)))?))),
    )
}

#[napi(
    js_name = "hookJsonNull",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookJson>"
)]
pub fn hook_json_null(env: &Env) -> napi::Result<Object<'_>> {
    tagged(env, guarded(|| Ok(json(HookJson::null()?))))
}

#[napi]
pub struct NativeHookRequest {
    inner: Arc<HookRequest>,
}

#[napi]
impl NativeHookRequest {
    #[napi(catch_unwind)]
    pub fn event(&self) -> String {
        self.inner.event().as_str().to_owned()
    }

    #[napi(catch_unwind)]
    pub fn context(&self) -> NativeHookJson {
        json(self.inner.context().clone())
    }

    #[napi(catch_unwind)]
    pub fn tool(&self) -> Option<Uint16Array> {
        self.inner.tool().map(units_of)
    }

    #[napi(catch_unwind)]
    pub fn input(&self) -> Option<NativeHookJson> {
        self.inner.input().cloned().map(json)
    }

    #[napi(catch_unwind)]
    pub fn result(&self) -> Option<NativeHookJson> {
        self.inner.result().cloned().map(json)
    }

    #[napi(catch_unwind)]
    pub fn bytes(&self) -> Uint8Array {
        Uint8Array::from(self.inner.to_bytes())
    }
}

fn request(value: HookRequest) -> NativeHookRequest {
    NativeHookRequest {
        inner: Arc::new(value),
    }
}

#[napi(
    js_name = "hookRequestNew",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookRequest>"
)]
pub fn hook_request_new<'env>(
    env: &'env Env,
    event: String,
    context: &NativeHookJson,
    tool: Option<Uint16Array>,
    input: Option<ClassInstance<'_, NativeHookJson>>,
    result: Option<ClassInstance<'_, NativeHookJson>>,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| {
            Ok(request(HookRequest::new_text(
                event_of(&event)?,
                owned(context),
                tool.as_ref().map(text_of),
                input.as_deref().map(owned),
                result.as_deref().map(owned),
            )?))
        }),
    )
}

#[napi(
    js_name = "hookRequestParse",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookRequest>"
)]
pub fn hook_request_parse(env: &Env, data: Uint8Array) -> napi::Result<Object<'_>> {
    let data = data.to_vec();
    tagged(
        env,
        on_large_stack(move || Ok(request(HookRequest::parse(&data)?))),
    )
}

#[napi]
pub struct NativeHookResponse {
    inner: Arc<HookResponse>,
}

#[napi]
impl NativeHookResponse {
    #[napi(catch_unwind)]
    pub fn outcome(&self) -> String {
        self.inner.outcome().as_str().to_owned()
    }

    #[napi(catch_unwind)]
    pub fn value(&self) -> Option<NativeHookJson> {
        self.inner.value().cloned().map(json)
    }

    #[napi(catch_unwind)]
    pub fn bytes(&self) -> Uint8Array {
        Uint8Array::from(self.inner.to_bytes())
    }
}

fn response(value: HookResponse) -> NativeHookResponse {
    NativeHookResponse {
        inner: Arc::new(value),
    }
}

#[napi(
    js_name = "hookResponseContinuing",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookResponse>"
)]
pub fn hook_response_continuing(env: &Env) -> napi::Result<Object<'_>> {
    tagged(env, guarded(|| Ok(response(HookResponse::continuing()))))
}

#[napi(
    js_name = "hookResponseText",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookResponse>"
)]
pub fn hook_response_text(env: &Env, kind: String, text: Uint16Array) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            let text = text_of(&text);
            Ok(response(match kind.as_str() {
                "block" => HookResponse::block(text)?,
                "ask" => HookResponse::ask(text)?,
                "context" => HookResponse::context(text)?,
                other => {
                    return Err(BindingError::Argument(format!(
                        "unknown text response '{other}'"
                    )));
                }
            }))
        }),
    )
}

#[napi(
    js_name = "hookResponseModifyInput",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookResponse>"
)]
pub fn hook_response_modify_input<'env>(
    env: &'env Env,
    event: String,
    input: &NativeHookJson,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| {
            Ok(response(HookResponse::modify_input(
                event_of(&event)?,
                owned(input),
            )?))
        }),
    )
}

#[napi(
    js_name = "hookResponseParse",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookResponse>"
)]
pub fn hook_response_parse(
    env: &Env,
    data: Uint8Array,
    event: String,
    declared: Vec<String>,
) -> napi::Result<Object<'_>> {
    let data = data.to_vec();
    let parsed = event_of(&event).and_then(|event| {
        let declared = outcomes_of(&declared)?;
        on_large_stack(move || Ok(response(HookResponse::parse(&data, event, &declared)?)))
    });
    tagged(env, parsed)
}

#[napi]
pub struct NativeHookEffects {
    inner: Arc<HookEffects>,
}

#[napi]
impl NativeHookEffects {
    #[napi(catch_unwind)]
    pub fn input(&self) -> Option<NativeHookJson> {
        self.inner.input().cloned().map(json)
    }

    #[napi(catch_unwind)]
    pub fn contexts(&self) -> Vec<Uint16Array> {
        self.inner.contexts().iter().map(units_of).collect()
    }

    #[napi(catch_unwind)]
    pub fn decision(&self) -> Option<String> {
        self.inner
            .decision()
            .map(|decision| decision.as_str().to_owned())
    }

    #[napi(catch_unwind)]
    pub fn reason(&self) -> Option<Uint16Array> {
        self.inner.reason().map(units_of)
    }

    #[napi(catch_unwind)]
    pub fn bytes(&self) -> Uint8Array {
        Uint8Array::from(self.inner.to_bytes())
    }
}

fn effects(value: HookEffects) -> NativeHookEffects {
    NativeHookEffects {
        inner: Arc::new(value),
    }
}

#[napi(
    js_name = "hookEffectsParse",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookEffects>"
)]
pub fn hook_effects_parse(env: &Env, data: Uint8Array) -> napi::Result<Object<'_>> {
    let data = data.to_vec();
    tagged(
        env,
        on_large_stack(move || Ok(effects(HookEffects::parse(&data)?))),
    )
}

#[napi(
    js_name = "hookEffectsNone",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookEffects>"
)]
pub fn hook_effects_none(env: &Env) -> napi::Result<Object<'_>> {
    tagged(env, guarded(|| Ok(effects(HookEffects::default()))))
}

#[napi(object)]
pub struct JsHookEnvironment {
    pub name: String,
    pub reference: bool,
    pub value: String,
}

#[napi(object)]
pub struct JsHookHandler {
    pub id: String,
    pub event: String,
    pub argv: Vec<String>,
    pub environment: Vec<JsHookEnvironment>,
    pub cwd: String,
    pub timeout_ms: f64,
    pub outcomes: Vec<String>,
    pub tools: Vec<String>,
}

#[napi]
pub struct NativeHookHandler {
    inner: Arc<HookHandler>,
}

#[napi]
impl NativeHookHandler {
    #[napi(catch_unwind)]
    pub fn snapshot(&self) -> JsHookHandler {
        let handler = &self.inner;
        JsHookHandler {
            id: handler.id().to_owned(),
            event: handler.event().as_str().to_owned(),
            argv: handler.argv().to_vec(),
            environment: handler
                .environment()
                .iter()
                .map(|(name, value)| match value {
                    HookEnvironment::Text(text) => JsHookEnvironment {
                        name: name.clone(),
                        reference: false,
                        value: text.clone(),
                    },
                    HookEnvironment::Reference(reference) => JsHookEnvironment {
                        name: name.clone(),
                        reference: true,
                        value: reference.clone(),
                    },
                })
                .collect(),
            cwd: handler.cwd().to_string_lossy().into_owned(),
            timeout_ms: f64::from(handler.timeout_ms()),
            outcomes: handler
                .outcomes()
                .iter()
                .map(|outcome| outcome.as_str().to_owned())
                .collect(),
            tools: handler.tools().to_vec(),
        }
    }
}

#[napi(
    js_name = "hookHandlerNew",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookHandler>"
)]
pub fn hook_handler_new(env: &Env, init: JsHookHandler) -> napi::Result<Object<'_>> {
    tagged(
        env,
        guarded(|| {
            let timeout = whole(init.timeout_ms, "hook timeout")?;
            let environment = init.environment.into_iter().map(|entry| {
                let value = if entry.reference {
                    HookEnvironment::Reference(entry.value)
                } else {
                    HookEnvironment::Text(entry.value)
                };
                (entry.name, value)
            });
            Ok(NativeHookHandler {
                inner: Arc::new(HookHandler::new(
                    init.id,
                    event_of(&init.event)?,
                    init.argv,
                    environment,
                    init.cwd,
                    u32::try_from(timeout).unwrap_or(u32::MAX),
                    outcomes_of(&init.outcomes)?,
                    init.tools,
                )?),
            })
        }),
    )
}

#[napi]
pub struct NativeHookConfig {
    inner: Arc<HookConfig>,
}

#[napi]
impl NativeHookConfig {
    #[napi(catch_unwind)]
    pub fn host(&self) -> String {
        self.inner.host().to_owned()
    }

    #[napi(catch_unwind)]
    pub fn handlers(&self) -> Vec<NativeHookHandler> {
        self.inner
            .handlers()
            .iter()
            .map(|handler| NativeHookHandler {
                inner: Arc::new(handler.clone()),
            })
            .collect()
    }

    #[napi(catch_unwind)]
    pub fn bytes(&self) -> Uint8Array {
        Uint8Array::from(self.inner.to_bytes())
    }
}

#[napi(
    js_name = "hookConfigNew",
    catch_unwind,
    ts_return_type = "NativeResult<NativeHookConfig>"
)]
pub fn hook_config_new<'env>(
    env: &'env Env,
    host: String,
    handlers: Vec<ClassInstance<'_, NativeHookHandler>>,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| {
            let handlers = handlers.iter().map(|handler| (*handler.inner).clone());
            Ok(NativeHookConfig {
                inner: Arc::new(HookConfig::new(host, handlers)?),
            })
        }),
    )
}
