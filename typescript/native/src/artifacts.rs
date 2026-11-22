use crate::authority::{NativeAuthority, read_claim, write_claim};
use crate::content::{
    Captured, Wire, node, parse, read_boundaries, read_content, read_key, read_member,
    read_placement, read_schema, read_selector, read_tree, status, unknown_kind, write_boundaries,
    write_content, write_key, write_member, write_placement, write_schema, write_selector,
    write_tree,
};
use crate::convert::whole;
use crate::editing::{read_editing_model, write_editing_model};
use crate::errors::{BindingError, BindingResult, tagged};
use crate::guard::{guarded, on_large_stack};
use flyrail::{
    Agent, Artifact, AssetRef, Audience, Claim, Command, CommandArgument, Content, Dependency,
    DependencyMode, DocumentSchema, EnvRef, EnvironmentValue, Family, HookArtifact, HookEvent,
    HookOutcome, HttpTransport, InstructionArtifact, Key, McpArtifact, McpTransport, Member,
    NativeArtifact, Placement, Platform, SectionBoundaries, Selector, SkillArtifact, SupportAsset,
    Surface, TargetScope,
};
use napi::Env;
use napi::bindgen_prelude::{Object, Unknown};
use napi_derive::napi;
use std::sync::Arc;

pub(crate) enum Model {
    Audience(Audience),
    EnvRef(EnvRef),
    AssetRef(AssetRef),
    Command(Command),
    Http(HttpTransport),
    Artifact(Artifact),
    Asset(SupportAsset),
    Content(Content),
    Boundaries(SectionBoundaries),
    Key(Key),
    Member(Member),
    Selector(Selector),
    Placement(Placement),
    Schema(DocumentSchema),
    Dependency(Dependency),
    Claim(Claim),
    Position(flyrail::ArrayPosition),
    Snapshot(Box<flyrail::SelectionSnapshot>),
    Selection(Box<flyrail::OwnedSelection>),
    Container(flyrail::CreatedContainer),
    Provenance(Box<flyrail::DocumentProvenance>),
    Request(Box<flyrail::DocumentRequest>),
    Location(flyrail::LoadoutLocation),
}

fn read_audience(wire: &Wire<'_>) -> BindingResult<Audience> {
    let platform = wire
        .optional::<String>("platform")?
        .map(|platform| parse::<Platform>(&platform))
        .transpose()?;
    Ok(Audience::new(
        parse::<Agent>(&wire.field::<String>("agent")?)?,
        parse::<TargetScope>(&wire.field::<String>("scope")?)?,
        parse::<Surface>(&wire.field::<String>("surface")?)?,
        platform,
    )?)
}

fn write_audience(env: &Env, audience: Audience) -> napi::Result<Object<'_>> {
    let mut object = node(env, "audience")?;
    object.set("agent", audience.agent().as_str())?;
    object.set("scope", audience.scope().as_str())?;
    object.set("surface", audience.surface().as_str())?;
    object.set("platform", audience.platform().map(Platform::as_str))?;
    Ok(object)
}

fn read_env_ref(wire: &Wire<'_>) -> BindingResult<EnvRef> {
    Ok(EnvRef::new(wire.field::<String>("name")?)?)
}

fn write_env_ref<'env>(env: &'env Env, reference: &EnvRef) -> napi::Result<Object<'env>> {
    let mut object = node(env, "env-ref")?;
    object.set("name", reference.name())?;
    Ok(object)
}

fn read_asset_ref(wire: &Wire<'_>) -> BindingResult<AssetRef> {
    let asset_id = wire.field::<String>("assetId")?;
    Ok(match wire.optional::<String>("path")? {
        Some(path) => AssetRef::with_path(asset_id, path)?,
        None => AssetRef::new(asset_id)?,
    })
}

fn write_asset_ref<'env>(env: &'env Env, reference: &AssetRef) -> napi::Result<Object<'env>> {
    let mut object = node(env, "asset-ref")?;
    object.set("assetId", reference.asset_id())?;
    object.set("path", reference.path())?;
    Ok(object)
}

fn read_argument(wire: &Wire<'_>) -> BindingResult<CommandArgument> {
    match wire.kind()?.as_str() {
        "text" => Ok(CommandArgument::Text(wire.field("text")?)),
        "asset-ref" => Ok(CommandArgument::Asset(read_asset_ref(wire)?)),
        other => Err(unknown_kind(other)),
    }
}

fn write_argument<'env>(env: &'env Env, argument: &CommandArgument) -> napi::Result<Object<'env>> {
    match argument {
        CommandArgument::Text(text) => {
            let mut object = node(env, "text")?;
            object.set("text", text.as_str())?;
            Ok(object)
        }
        CommandArgument::Asset(reference) => write_asset_ref(env, reference),
    }
}

fn read_environment_value(wire: &Wire<'_>) -> BindingResult<EnvironmentValue> {
    match wire.kind()?.as_str() {
        "text" => Ok(EnvironmentValue::Text(wire.field("text")?)),
        "env-ref" => Ok(EnvironmentValue::Reference(read_env_ref(wire)?)),
        other => Err(unknown_kind(other)),
    }
}

fn write_environment_value<'env>(
    env: &'env Env,
    value: &EnvironmentValue,
) -> napi::Result<Object<'env>> {
    match value {
        EnvironmentValue::Text(text) => {
            let mut object = node(env, "text")?;
            object.set("text", text.as_str())?;
            Ok(object)
        }
        EnvironmentValue::Reference(reference) => write_env_ref(env, reference),
    }
}

fn read_command(wire: &Wire<'_>) -> BindingResult<Command> {
    let argv = wire
        .children("argv")?
        .iter()
        .map(read_argument)
        .collect::<BindingResult<Vec<_>>>()?;
    let environment = wire
        .children("env")?
        .iter()
        .map(|entry| {
            Ok((
                entry.field::<String>("name")?,
                read_environment_value(&entry.child("value")?)?,
            ))
        })
        .collect::<BindingResult<Vec<_>>>()?;
    let cwd = wire
        .optional_child("cwd")
        .map(|cwd| read_argument(&cwd))
        .transpose()?;
    Ok(Command::new(argv, environment, cwd)?)
}

fn write_command<'env>(env: &'env Env, command: &Command) -> napi::Result<Object<'env>> {
    let argv = command
        .argv()
        .iter()
        .map(|argument| write_argument(env, argument))
        .collect::<napi::Result<Vec<_>>>()?;
    let environment = command
        .env()
        .iter()
        .map(|(name, value)| {
            let mut entry = Object::new(env)?;
            entry.set("name", name.as_str())?;
            entry.set("value", write_environment_value(env, value)?)?;
            Ok(entry)
        })
        .collect::<napi::Result<Vec<_>>>()?;
    let cwd = command
        .cwd()
        .map(|cwd| write_argument(env, cwd))
        .transpose()?;
    let mut object = node(env, "command")?;
    object.set("argv", argv)?;
    object.set("env", environment)?;
    object.set("cwd", cwd)?;
    Ok(object)
}

fn read_http(wire: &Wire<'_>) -> BindingResult<HttpTransport> {
    let token = wire
        .optional_child("bearerToken")
        .map(|token| read_env_ref(&token))
        .transpose()?;
    Ok(HttpTransport::new(wire.field::<String>("url")?, token)?)
}

fn write_http<'env>(env: &'env Env, http: &HttpTransport) -> napi::Result<Object<'env>> {
    let token = http
        .bearer_token()
        .map(|token| write_env_ref(env, token))
        .transpose()?;
    let mut object = node(env, "http")?;
    object.set("url", http.url())?;
    object.set("bearerToken", token)?;
    Ok(object)
}

fn read_transport(wire: &Wire<'_>) -> BindingResult<McpTransport> {
    match wire.kind()?.as_str() {
        "command" => Ok(McpTransport::Command(read_command(wire)?)),
        "http" => Ok(McpTransport::Http(read_http(wire)?)),
        other => Err(unknown_kind(other)),
    }
}

fn write_transport<'env>(env: &'env Env, transport: &McpTransport) -> napi::Result<Object<'env>> {
    match transport {
        McpTransport::Command(command) => write_command(env, command),
        McpTransport::Http(http) => write_http(env, http),
    }
}

fn timeout_of(wire: &Wire<'_>) -> BindingResult<i32> {
    // a non-integer timeout maps to 0 so the crate's range message is reported
    let milliseconds = whole(wire.field("timeoutMs")?, "hook timeout").unwrap_or(0);
    Ok(i32::try_from(milliseconds).unwrap_or(i32::MAX))
}

fn read_artifact(wire: &Wire<'_>) -> BindingResult<Artifact> {
    let kind = wire.kind()?;
    match kind.as_str() {
        "skill" => Ok(Artifact::Skill(SkillArtifact::new(
            wire.field::<String>("id")?,
            wire.field::<String>("name")?,
            read_tree(&wire.child("content")?)?,
        )?)),
        "instruction" => Ok(Artifact::Instruction(InstructionArtifact::new(
            wire.field::<String>("id")?,
            wire.field::<String>("text")?,
        )?)),
        "mcp" => Ok(Artifact::Mcp(McpArtifact::new(
            wire.field::<String>("id")?,
            wire.field::<String>("name")?,
            read_transport(&wire.child("transport")?)?,
        )?)),
        "hook" => {
            let outcomes = wire
                .field::<Vec<String>>("outcomes")?
                .iter()
                .map(|outcome| parse::<HookOutcome>(outcome))
                .collect::<BindingResult<Vec<_>>>()?;
            Ok(Artifact::Hook(HookArtifact::new(
                wire.field::<String>("id")?,
                parse::<HookEvent>(&wire.field::<String>("event")?)?,
                read_command(&wire.child("command")?)?,
                timeout_of(wire)?,
                outcomes,
                wire.field::<Vec<String>>("tools")?,
            )?))
        }
        "native" => Ok(Artifact::Native(NativeArtifact::new(
            wire.field::<String>("id")?,
            parse::<Family>(&wire.field::<String>("family")?)?,
            read_audience(&wire.child("audience")?)?,
            wire.field::<String>("destination")?,
            read_content(&wire.child("content")?)?,
        )?)),
        other => Err(unknown_kind(other)),
    }
}

fn write_artifact<'env>(env: &'env Env, artifact: &Artifact) -> napi::Result<Object<'env>> {
    match artifact {
        Artifact::Skill(skill) => {
            let mut object = node(env, "skill")?;
            object.set("id", skill.id())?;
            object.set("name", skill.name())?;
            object.set("content", write_tree(env, skill.content())?)?;
            Ok(object)
        }
        Artifact::Instruction(instruction) => {
            let mut object = node(env, "instruction")?;
            object.set("id", instruction.id())?;
            object.set("text", instruction.text())?;
            Ok(object)
        }
        Artifact::Mcp(mcp) => {
            let mut object = node(env, "mcp")?;
            object.set("id", mcp.id())?;
            object.set("name", mcp.name())?;
            object.set("transport", write_transport(env, mcp.transport())?)?;
            Ok(object)
        }
        Artifact::Hook(hook) => {
            let mut object = node(env, "hook")?;
            object.set("id", hook.id())?;
            object.set("event", hook.event().as_str())?;
            object.set("command", write_command(env, hook.command())?)?;
            object.set("timeoutMs", hook.timeout_ms())?;
            object.set(
                "outcomes",
                hook.outcomes()
                    .iter()
                    .map(|outcome| outcome.as_str())
                    .collect::<Vec<_>>(),
            )?;
            object.set("tools", hook.tools().to_vec())?;
            object.set("protocolVersion", hook.protocol_version())?;
            Ok(object)
        }
        Artifact::Native(native) => {
            let mut object = node(env, "native")?;
            object.set("id", native.id())?;
            object.set("family", native.family().as_str())?;
            object.set("audience", write_audience(env, *native.audience())?)?;
            object.set("destination", native.destination())?;
            object.set("content", write_content(env, native.content())?)?;
            Ok(object)
        }
    }
}

fn read_asset(wire: &Wire<'_>) -> BindingResult<SupportAsset> {
    Ok(SupportAsset::new(
        wire.field::<String>("id")?,
        parse::<Family>(&wire.field::<String>("family")?)?,
        read_tree(&wire.child("content")?)?,
    )?)
}

fn write_asset<'env>(env: &'env Env, asset: &SupportAsset) -> napi::Result<Object<'env>> {
    let mut object = node(env, "asset")?;
    object.set("id", asset.id())?;
    object.set("family", asset.family().as_str())?;
    object.set("content", write_tree(env, asset.content())?)?;
    Ok(object)
}

pub(crate) fn read_dependency(wire: &Wire<'_>) -> BindingResult<Dependency> {
    Ok(Dependency::with_mode(
        wire.field::<String>("dependent")?,
        wire.field::<String>("required")?,
        parse::<DependencyMode>(&wire.field::<String>("mode")?)?,
    )?)
}

pub(crate) fn write_dependency<'env>(
    env: &'env Env,
    dependency: &Dependency,
) -> napi::Result<Object<'env>> {
    let mut object = node(env, "dependency")?;
    object.set("dependent", dependency.dependent())?;
    object.set("required", dependency.required())?;
    object.set("mode", dependency.mode().as_str())?;
    Ok(object)
}

pub(crate) fn read_artifacts(wire: &Wire<'_>, name: &str) -> BindingResult<Vec<Artifact>> {
    wire.children(name)?.iter().map(read_artifact).collect()
}

pub(crate) fn read_assets(wire: &Wire<'_>, name: &str) -> BindingResult<Vec<SupportAsset>> {
    wire.children(name)?.iter().map(read_asset).collect()
}

pub(crate) fn read_dependencies(wire: &Wire<'_>, name: &str) -> BindingResult<Vec<Dependency>> {
    wire.children(name)?.iter().map(read_dependency).collect()
}

pub(crate) fn write_artifacts<'env>(
    env: &'env Env,
    artifacts: &[Artifact],
) -> napi::Result<Vec<Object<'env>>> {
    artifacts
        .iter()
        .map(|artifact| write_artifact(env, artifact))
        .collect()
}

pub(crate) fn write_assets<'env>(
    env: &'env Env,
    assets: &[SupportAsset],
) -> napi::Result<Vec<Object<'env>>> {
    assets.iter().map(|asset| write_asset(env, asset)).collect()
}

pub(crate) fn write_dependencies<'env>(
    env: &'env Env,
    dependencies: &[Dependency],
) -> napi::Result<Vec<Object<'env>>> {
    dependencies
        .iter()
        .map(|dependency| write_dependency(env, dependency))
        .collect()
}

fn read_model(wire: &Wire<'_>) -> BindingResult<Model> {
    let kind = wire.kind()?;
    match kind.as_str() {
        "audience" => Ok(Model::Audience(read_audience(wire)?)),
        "env-ref" => Ok(Model::EnvRef(read_env_ref(wire)?)),
        "asset-ref" => Ok(Model::AssetRef(read_asset_ref(wire)?)),
        "command" => Ok(Model::Command(read_command(wire)?)),
        "http" => Ok(Model::Http(read_http(wire)?)),
        "skill" | "instruction" | "mcp" | "hook" | "native" => {
            Ok(Model::Artifact(read_artifact(wire)?))
        }
        "asset" => Ok(Model::Asset(read_asset(wire)?)),
        "file" | "tree" | "section" | "structured" => Ok(Model::Content(read_content(wire)?)),
        "boundaries" | "boundaries-marker" => Ok(Model::Boundaries(read_boundaries(wire)?)),
        "key" => Ok(Model::Key(read_key(wire)?)),
        "member" => Ok(Model::Member(read_member(wire)?)),
        "selector" => Ok(Model::Selector(read_selector(wire)?)),
        "placement" => Ok(Model::Placement(read_placement(wire)?)),
        "schema" => Ok(Model::Schema(read_schema(wire)?)),
        "dependency" => Ok(Model::Dependency(read_dependency(wire)?)),
        "claim" => Ok(Model::Claim(read_claim(wire)?)),
        "loadout-location" => Ok(Model::Location(crate::loadout::read_location(wire)?)),
        other => read_editing_model(other, wire),
    }
}

fn write_model<'env>(env: &'env Env, model: &Model) -> napi::Result<Object<'env>> {
    match model {
        Model::Audience(audience) => write_audience(env, *audience),
        Model::EnvRef(reference) => write_env_ref(env, reference),
        Model::AssetRef(reference) => write_asset_ref(env, reference),
        Model::Command(command) => write_command(env, command),
        Model::Http(http) => write_http(env, http),
        Model::Artifact(artifact) => write_artifact(env, artifact),
        Model::Asset(asset) => write_asset(env, asset),
        Model::Content(content) => write_content(env, content),
        Model::Boundaries(boundaries) => write_boundaries(env, boundaries),
        Model::Key(key) => write_key(env, key),
        Model::Member(member) => write_member(env, member),
        Model::Selector(selector) => write_selector(env, selector),
        Model::Placement(placement) => write_placement(env, placement),
        Model::Schema(schema) => write_schema(env, schema),
        Model::Dependency(dependency) => write_dependency(env, dependency),
        Model::Claim(claim) => write_claim(env, claim),
        Model::Location(location) => crate::loadout::write_location(env, location),
        editing @ (Model::Position(_)
        | Model::Snapshot(_)
        | Model::Selection(_)
        | Model::Container(_)
        | Model::Provenance(_)
        | Model::Request(_)) => write_editing_model(env, editing)
            .unwrap_or_else(|| Err(napi::Error::from_reason("unknown model"))),
    }
}

#[napi]
pub struct NativeModel {
    pub(crate) inner: Arc<Model>,
}

#[napi]
#[allow(clippy::use_self)]
impl NativeModel {
    #[napi(catch_unwind, ts_return_type = "NativeResult<ModelWire>")]
    pub fn snapshot<'env>(&self, env: &'env Env) -> napi::Result<Object<'env>> {
        tagged(env, guarded(|| status(write_model(env, &self.inner))))
    }

    #[napi(catch_unwind, ts_return_type = "NativeResult<boolean>")]
    pub fn claim_overlaps<'env>(
        &self,
        env: &'env Env,
        other: &NativeModel,
    ) -> napi::Result<Object<'env>> {
        let (left, right) = (Arc::clone(&self.inner), Arc::clone(&other.inner));
        tagged(
            env,
            on_large_stack(move || match (&*left, &*right) {
                (Model::Claim(left), Model::Claim(right)) => Ok(left.overlaps(right)),
                _ => Err(not_a_claim()),
            }),
        )
    }

    #[napi(catch_unwind, ts_return_type = "NativeResult<boolean>")]
    pub fn claim_equals<'env>(
        &self,
        env: &'env Env,
        other: &NativeModel,
    ) -> napi::Result<Object<'env>> {
        let (left, right) = (Arc::clone(&self.inner), Arc::clone(&other.inner));
        tagged(
            env,
            on_large_stack(move || match (&*left, &*right) {
                (Model::Claim(left), Model::Claim(right)) => Ok(left == right),
                _ => Err(not_a_claim()),
            }),
        )
    }

    #[napi(catch_unwind, ts_return_type = "NativeResult<string>")]
    pub fn claim_identity<'env>(
        &self,
        env: &'env Env,
        authority: &NativeAuthority,
    ) -> napi::Result<Object<'env>> {
        let inner = Arc::clone(&self.inner);
        let authority = Arc::clone(&authority.inner);
        tagged(
            env,
            on_large_stack(move || match &*inner {
                Model::Claim(claim) => Ok(claim.identity(&authority)?),
                _ => Err(not_a_claim()),
            }),
        )
    }
}

fn not_a_claim() -> BindingError {
    BindingError::Argument(String::from("model is not a claim"))
}

#[napi(
    js_name = "modelsBuild",
    catch_unwind,
    ts_args_type = "wire: ModelWire",
    ts_return_type = "NativeResult<NativeModel>"
)]
pub fn models_build<'env>(env: &'env Env, wire: Unknown<'env>) -> napi::Result<Object<'env>> {
    let model = Captured::read(*env, wire).and_then(|captured| {
        on_large_stack(move || {
            read_model(&captured.wire()).map(|model| NativeModel {
                inner: Arc::new(model),
            })
        })
    });
    tagged(env, model)
}

#[napi(
    js_name = "modelsDependencyOrder",
    catch_unwind,
    ts_args_type = "names: string[], wire: { dependencies: ModelWire[] }",
    ts_return_type = "NativeResult<string[]>"
)]
pub fn models_dependency_order<'env>(
    env: &'env Env,
    names: Vec<String>,
    wire: Unknown<'env>,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| {
            let captured = Captured::read(*env, wire)?;
            let dependencies = read_dependencies(&captured.wire(), "dependencies")?;
            Ok(flyrail::dependency_order(&names, &dependencies)?)
        }),
    )
}
