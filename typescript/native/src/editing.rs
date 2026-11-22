use crate::artifacts::{Model, NativeModel};
use crate::authority::{read_claim, write_claim};
use crate::content::{
    Wire, cloned_tree, node, parse, read_content, read_member, read_selector, unknown_kind,
    write_content, write_member, write_selector,
};
use crate::errors::{BindingError, BindingResult, status, tagged};
use crate::guard::{guarded, on_large_stack};
use flyrail::{
    Acquisition, ArrayPosition, CreatedContainer, Disposition, DocumentEdit, DocumentFormat,
    DocumentProvenance, DocumentRequest, EditObservation, OwnedSelection, SelectionSnapshot,
    SnapshotValue, content_claim, edit_document,
};
use napi::Env;
use napi::bindgen_prelude::{ClassInstance, Object, Uint8Array};
use napi_derive::napi;
use std::sync::Arc;

fn bytes(data: &[u8]) -> Uint8Array {
    Uint8Array::from(data.to_vec())
}

fn read_position(wire: &Wire<'_>) -> BindingResult<ArrayPosition> {
    let member = |name: &str| {
        wire.optional_child(name)
            .map(|member| read_member(&member))
            .transpose()
    };
    Ok(ArrayPosition::new(
        member("previous")?,
        member("following")?,
        wire.field::<Vec<u8>>("prefix")?,
    )?)
}

fn write_position<'env>(env: &'env Env, position: &ArrayPosition) -> napi::Result<Object<'env>> {
    let member =
        |member: Option<&flyrail::Member>| member.map(|item| write_member(env, item)).transpose();
    let mut object = node(env, "array-position")?;
    object.set("previous", member(position.previous())?)?;
    object.set("following", member(position.following())?)?;
    object.set("prefix", bytes(position.prefix()))?;
    Ok(object)
}

fn read_snapshot_value(wire: &Wire<'_>) -> BindingResult<SnapshotValue> {
    match wire.kind()?.as_str() {
        "bytes" => Ok(SnapshotValue::Bytes(wire.field("bytes")?)),
        "typed" => Ok(SnapshotValue::Typed(wire.value("value")?)),
        other => Err(unknown_kind(other)),
    }
}

fn write_snapshot_value<'env>(env: &'env Env, value: &SnapshotValue) -> napi::Result<Object<'env>> {
    match value {
        SnapshotValue::Bytes(data) => {
            let mut object = node(env, "bytes")?;
            object.set("bytes", bytes(data))?;
            Ok(object)
        }
        SnapshotValue::Typed(value) => {
            let mut object = node(env, "typed")?;
            object.set("value", cloned_tree(value)?)?;
            Ok(object)
        }
    }
}

fn read_snapshot(wire: &Wire<'_>) -> BindingResult<SelectionSnapshot> {
    let position = wire
        .optional_child("position")
        .map(|position| read_position(&position))
        .transpose()?;
    Ok(SelectionSnapshot::new(
        read_snapshot_value(&wire.child("value")?)?,
        wire.field::<Vec<u8>>("syntax")?,
        wire.field::<Vec<u8>>("ownedSyntax")?,
        position,
        wire.field::<Vec<u8>>("layout")?,
    )?)
}

fn write_snapshot<'env>(
    env: &'env Env,
    snapshot: &SelectionSnapshot,
) -> napi::Result<Object<'env>> {
    let position = snapshot
        .position()
        .map(|position| write_position(env, position))
        .transpose()?;
    let mut object = node(env, "snapshot")?;
    object.set("value", write_snapshot_value(env, snapshot.value())?)?;
    object.set("syntax", bytes(snapshot.syntax()))?;
    object.set("ownedSyntax", bytes(snapshot.owned_syntax()))?;
    object.set("position", position)?;
    object.set("layout", bytes(snapshot.layout()))?;
    Ok(object)
}

fn read_selection(wire: &Wire<'_>) -> BindingResult<OwnedSelection> {
    let format = wire
        .optional::<String>("format")?
        .map(|format| parse::<DocumentFormat>(&format))
        .transpose()?;
    let baseline = wire
        .optional_child("baseline")
        .map(|baseline| read_snapshot(&baseline))
        .transpose()?;
    Ok(OwnedSelection::new(
        read_claim(&wire.child("claim")?)?,
        format,
        read_snapshot(&wire.child("installed")?)?,
        parse::<Disposition>(&wire.field::<String>("disposition")?)?,
        baseline,
        wire.optional::<Vec<u8>>("separator")?.unwrap_or_default(),
    )?)
}

fn write_selection<'env>(env: &'env Env, selection: &OwnedSelection) -> napi::Result<Object<'env>> {
    let baseline = selection
        .baseline()
        .map(|baseline| write_snapshot(env, baseline))
        .transpose()?;
    let mut object = node(env, "owned-selection")?;
    object.set("claim", write_claim(env, selection.claim())?)?;
    object.set("format", selection.format().map(DocumentFormat::as_str))?;
    object.set("installed", write_snapshot(env, selection.installed())?)?;
    object.set("disposition", selection.disposition().as_str())?;
    object.set("baseline", baseline)?;
    object.set(
        "separator",
        Some(selection.separator())
            .filter(|separator| !separator.is_empty())
            .map(bytes),
    )?;
    Ok(object)
}

fn read_container(wire: &Wire<'_>) -> BindingResult<CreatedContainer> {
    Ok(CreatedContainer::new(
        read_selector(&wire.child("selector")?)?,
        wire.field::<Vec<u8>>("emptySyntax")?,
    ))
}

fn write_container<'env>(
    env: &'env Env,
    container: &CreatedContainer,
) -> napi::Result<Object<'env>> {
    let mut object = node(env, "created-container")?;
    object.set("selector", write_selector(env, container.selector())?)?;
    object.set("emptySyntax", bytes(container.empty_syntax()))?;
    Ok(object)
}

fn read_provenance(wire: &Wire<'_>) -> BindingResult<DocumentProvenance> {
    let containers = wire
        .children("containers")?
        .iter()
        .map(read_container)
        .collect::<BindingResult<Vec<_>>>()?;
    let fields = wire
        .children("schemaFields")?
        .iter()
        .map(read_selection)
        .collect::<BindingResult<Vec<_>>>()?;
    Ok(DocumentProvenance::new(
        wire.optional::<Vec<u8>>("emptyDocument")?,
        containers,
        wire.field::<Vec<String>>("trailingSeparators")?,
        fields,
        wire.optional::<Vec<u8>>("schemaEmptyDocument")?,
    )?)
}

fn write_provenance<'env>(
    env: &'env Env,
    provenance: &DocumentProvenance,
) -> napi::Result<Object<'env>> {
    let containers = provenance
        .containers()
        .iter()
        .map(|container| write_container(env, container))
        .collect::<napi::Result<Vec<_>>>()?;
    let fields = provenance
        .schema_fields()
        .iter()
        .map(|field| write_selection(env, field))
        .collect::<napi::Result<Vec<_>>>()?;
    let mut object = node(env, "provenance")?;
    object.set("emptyDocument", provenance.empty_document().map(bytes))?;
    object.set("containers", containers)?;
    object.set(
        "trailingSeparators",
        provenance.trailing_separators().to_vec(),
    )?;
    object.set("schemaFields", fields)?;
    object.set(
        "schemaEmptyDocument",
        provenance.schema_empty_document().map(bytes),
    )?;
    Ok(object)
}

fn read_request(wire: &Wire<'_>) -> BindingResult<DocumentRequest> {
    let content = wire
        .optional_child("content")
        .map(|content| read_content(&content))
        .transpose()?;
    Ok(DocumentRequest::new(
        read_claim(&wire.child("claim")?)?,
        content,
        parse::<Acquisition>(&wire.field::<String>("acquisition")?)?,
    )?)
}

fn write_request<'env>(env: &'env Env, request: &DocumentRequest) -> napi::Result<Object<'env>> {
    let content = request
        .content()
        .map(|content| write_content(env, content))
        .transpose()?;
    let mut object = node(env, "request")?;
    object.set("claim", write_claim(env, request.claim())?)?;
    object.set("content", content)?;
    object.set("acquisition", request.acquisition().as_str())?;
    Ok(object)
}

fn write_observation<'env>(
    env: &'env Env,
    observation: &EditObservation,
) -> napi::Result<Object<'env>> {
    let current = observation
        .current()
        .map(|current| write_snapshot(env, current))
        .transpose()?;
    let mut object = node(env, "edit-observation")?;
    object.set("claim", write_claim(env, observation.claim())?)?;
    object.set("status", observation.status().as_str())?;
    object.set("current", current)?;
    object.set("message", observation.message())?;
    Ok(object)
}

fn write_edit<'env>(env: &'env Env, edit: &DocumentEdit) -> napi::Result<Object<'env>> {
    let ownership = edit
        .ownership()
        .iter()
        .map(|selection| write_selection(env, selection))
        .collect::<napi::Result<Vec<_>>>()?;
    let observations = edit
        .observations()
        .iter()
        .map(|observation| write_observation(env, observation))
        .collect::<napi::Result<Vec<_>>>()?;
    let mut object = Object::new(env)?;
    object.set("before", edit.before().map(bytes))?;
    object.set("after", edit.after().map(bytes))?;
    object.set("ownership", ownership)?;
    object.set("provenance", write_provenance(env, edit.provenance())?)?;
    object.set("observations", observations)?;
    object.set("applicable", edit.applicable())?;
    Ok(object)
}

pub(crate) fn read_editing_model(kind: &str, wire: &Wire<'_>) -> BindingResult<Model> {
    match kind {
        "array-position" => Ok(Model::Position(read_position(wire)?)),
        "snapshot" => Ok(Model::Snapshot(Box::new(read_snapshot(wire)?))),
        "owned-selection" => Ok(Model::Selection(Box::new(read_selection(wire)?))),
        "created-container" => Ok(Model::Container(read_container(wire)?)),
        "provenance" => Ok(Model::Provenance(Box::new(read_provenance(wire)?))),
        "request" => Ok(Model::Request(Box::new(read_request(wire)?))),
        other => Err(unknown_kind(other)),
    }
}

pub(crate) fn write_editing_model<'env>(
    env: &'env Env,
    model: &Model,
) -> Option<napi::Result<Object<'env>>> {
    match model {
        Model::Position(position) => Some(write_position(env, position)),
        Model::Snapshot(snapshot) => Some(write_snapshot(env, snapshot)),
        Model::Selection(selection) => Some(write_selection(env, selection)),
        Model::Container(container) => Some(write_container(env, container)),
        Model::Provenance(provenance) => Some(write_provenance(env, provenance)),
        Model::Request(request) => Some(write_request(env, request)),
        _ => None,
    }
}

fn not_model(kind: &str) -> BindingError {
    BindingError::Argument(format!("model is not a {kind}"))
}

fn models_of(
    models: &[ClassInstance<'_, NativeModel>],
    kind: &str,
    is_kind: impl Fn(&Model) -> bool,
) -> BindingResult<Vec<Arc<Model>>> {
    models
        .iter()
        .map(|model| {
            if is_kind(&model.inner) {
                Ok(Arc::clone(&model.inner))
            } else {
                Err(not_model(kind))
            }
        })
        .collect()
}

#[napi(
    js_name = "editingEditDocument",
    catch_unwind,
    ts_return_type = "NativeResult<DocumentEditWire>"
)]
pub fn editing_edit_document<'env>(
    env: &'env Env,
    source: Option<Uint8Array>,
    requests: Vec<ClassInstance<'_, NativeModel>>,
    ownership: Vec<ClassInstance<'_, NativeModel>>,
    provenance: &NativeModel,
) -> napi::Result<Object<'env>> {
    let data = source.map(|source| source.to_vec());
    let edit: BindingResult<DocumentEdit> = (|| {
        let requests = models_of(&requests, "document request", |model| {
            matches!(model, Model::Request(_))
        })?;
        let ownership = models_of(&ownership, "owned selection", |model| {
            matches!(model, Model::Selection(_))
        })?;
        if !matches!(&*provenance.inner, Model::Provenance(_)) {
            return Err(not_model("document provenance"));
        }
        let provenance = Arc::clone(&provenance.inner);
        on_large_stack(move || {
            let requests: Vec<_> = requests
                .iter()
                .filter_map(|model| match &**model {
                    Model::Request(request) => Some((**request).clone()),
                    _ => None,
                })
                .collect();
            let ownership: Vec<_> = ownership
                .iter()
                .filter_map(|model| match &**model {
                    Model::Selection(selection) => Some((**selection).clone()),
                    _ => None,
                })
                .collect();
            let Model::Provenance(provenance) = &*provenance else {
                return Err(not_model("document provenance"));
            };
            Ok(edit_document(data, &requests, &ownership, provenance)?)
        })
    })();
    tagged(
        env,
        edit.and_then(|edit| guarded(|| status(write_edit(env, &edit)))),
    )
}

#[napi(
    js_name = "editingContentClaim",
    catch_unwind,
    ts_return_type = "NativeResult<ClaimWire>"
)]
pub fn editing_content_claim<'env>(
    env: &'env Env,
    content: &NativeModel,
) -> napi::Result<Object<'env>> {
    tagged(
        env,
        guarded(|| match &*content.inner {
            Model::Content(content) => {
                let claim = content_claim(content)?;
                status(write_claim(env, &claim))
            }
            _ => Err(not_model("content")),
        }),
    )
}
