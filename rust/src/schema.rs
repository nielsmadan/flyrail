use crate::authority::ClaimSelector;
use crate::content::{Content, DocumentFormat, DocumentSchema};
use crate::editors::{
    Acquisition, DocumentEdit, DocumentProvenance, DocumentRequest, EditObservation, EditStatus,
    OwnedSelection, SnapshotValue, content_claim, decode_utf8, edit_document, structured_snapshot,
};
use crate::json_document::JsonDocument;
use crate::values::{Value, ValueError};
use std::collections::HashSet;

pub fn matches(data: Option<&[u8]>, requirement: &DocumentSchema) -> Result<bool, ValueError> {
    let Some(data) = data.filter(|bytes| !bytes.iter().all(u8::is_ascii_whitespace)) else {
        return Ok(false);
    };
    let backend = json_document(data, requirement.format() == DocumentFormat::Jsonc)?;
    let snapshot = structured_snapshot(&backend, requirement.content()?.selector())?;
    Ok(snapshot.is_some_and(|item| {
        item.value() == &SnapshotValue::Typed(Value::Scalar(requirement.value().clone()))
    }))
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn edit_schema_document(
    data: Option<Vec<u8>>,
    requests: &[DocumentRequest],
    ownership: &[OwnedSelection],
    provenance: &DocumentProvenance,
    active_schemas: &[DocumentSchema],
    validate: bool,
) -> Result<DocumentEdit, ValueError> {
    let unique_schemas = active_schemas.iter().collect::<HashSet<_>>();
    let unique_keys = active_schemas
        .iter()
        .map(DocumentSchema::key)
        .collect::<HashSet<_>>();
    if unique_schemas.len() != unique_keys.len() {
        return Err(ValueError::new("incompatible document schema requirements"));
    }
    let requirements = active_schemas
        .iter()
        .enumerate()
        .filter_map(|(index, schema)| {
            active_schemas
                .iter()
                .take(index)
                .all(|existing| existing.key() != schema.key())
                .then_some(schema)
        })
        .collect::<Vec<_>>();
    let schema_claims = requirements
        .iter()
        .map(|schema| content_claim(&Content::Structured(schema.content()?)))
        .collect::<Result<Vec<_>, _>>()?;
    if schema_claims.iter().any(|schema| {
        requests
            .iter()
            .map(DocumentRequest::claim)
            .chain(ownership.iter().map(OwnedSelection::claim))
            .any(|claim| schema.overlaps(claim))
    }) {
        return Err(ValueError::new(
            "document schema overlaps an app-owned field",
        ));
    }
    if active_schemas.is_empty()
        && provenance.schema_fields().is_empty()
        && provenance.schema_empty_document().is_none()
    {
        return edit_document(data, requests, ownership, provenance);
    }

    let normalized = match &data {
        Some(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => Some(b"{}".to_vec()),
        _ => data.clone(),
    };
    let empty_source = if normalized == data {
        provenance.schema_empty_document().map(<[u8]>::to_vec)
    } else {
        data.clone()
    };
    let backend = json_document(normalized.as_deref().unwrap_or(b"{}"), true)?;
    let mut additions = Vec::new();
    if validate {
        for schema in requirements {
            let content = schema.content()?;
            match structured_snapshot(&backend, content.selector())? {
                None => additions.push(DocumentRequest::set(
                    Content::Structured(content),
                    Acquisition::Conflict,
                )?),
                Some(snapshot)
                    if snapshot.value()
                        != &SnapshotValue::Typed(Value::Scalar(schema.value().clone())) =>
                {
                    return Ok(DocumentEdit::new(
                        data.clone(),
                        data,
                        ownership.to_vec(),
                        provenance.clone(),
                        requests
                            .iter()
                            .map(|request| {
                                EditObservation::new(
                                    request.claim().clone(),
                                    EditStatus::Conflict,
                                    None,
                                    "document schema differs",
                                )
                            })
                            .collect(),
                    ));
                }
                Some(_) => {}
            }
        }
    }

    let mut created = Vec::new();
    for item in provenance.schema_fields() {
        let ClaimSelector::Structured(selector) = item.claim().selector() else {
            continue;
        };
        if structured_snapshot(&backend, selector)?.as_ref() == Some(item.installed())
            && !requests.iter().any(|request| {
                request
                    .content()
                    .is_some_and(|_| request.claim().overlaps(item.claim()))
            })
        {
            created.push(item.clone());
        }
    }
    let created_ids = created
        .iter()
        .map(OwnedSelection::claim)
        .chain(additions.iter().map(DocumentRequest::claim))
        .cloned()
        .collect::<HashSet<_>>();
    let ordinary = provenance_with_schema(provenance, Vec::new(), None)?;
    let combined_requests = requests
        .iter()
        .cloned()
        .chain(additions)
        .collect::<Vec<_>>();
    let combined_ownership = ownership.iter().cloned().chain(created).collect::<Vec<_>>();
    let edit = edit_document(
        normalized,
        &combined_requests,
        &combined_ownership,
        &ordinary,
    )?;
    if !edit.applicable() {
        return Ok(DocumentEdit::new(
            data.clone(),
            data,
            ownership.to_vec(),
            provenance.clone(),
            edit.observations().to_vec(),
        ));
    }

    let (fields, owned): (Vec<_>, Vec<_>) = edit
        .ownership()
        .iter()
        .cloned()
        .partition(|item| created_ids.contains(item.claim()));
    let observations = edit
        .observations()
        .iter()
        .filter(|item| !created_ids.contains(item.claim()))
        .cloned()
        .collect::<Vec<_>>();
    if active_schemas.is_empty() {
        let retirements = fields
            .iter()
            .map(|item| DocumentRequest::retire(item.claim().clone()))
            .collect::<Result<Vec<_>, _>>()?;
        let candidate = edit_document(
            edit.after().map(<[u8]>::to_vec),
            &retirements,
            edit.ownership(),
            edit.provenance(),
        )?;
        if candidate.applicable()
            && owned.is_empty()
            && candidate.after().is_none_or(is_empty_json_object)
        {
            return Ok(DocumentEdit::new(
                data,
                empty_source.or_else(|| candidate.after().map(<[u8]>::to_vec)),
                candidate.ownership().to_vec(),
                candidate.provenance().clone(),
                observations,
            ));
        }
        let schema_empty_document = if owned.is_empty() { None } else { empty_source };
        return Ok(DocumentEdit::new(
            data,
            edit.after().map(<[u8]>::to_vec),
            owned,
            provenance_with_schema(edit.provenance(), Vec::new(), schema_empty_document)?,
            observations,
        ));
    }
    Ok(DocumentEdit::new(
        data,
        edit.after().map(<[u8]>::to_vec),
        owned,
        provenance_with_schema(edit.provenance(), fields, empty_source)?,
        observations,
    ))
}

fn json_document(data: &[u8], comments: bool) -> Result<JsonDocument, ValueError> {
    let body = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
    JsonDocument::parse(&decode_utf8(body)?, comments)
}

fn provenance_with_schema(
    provenance: &DocumentProvenance,
    fields: Vec<OwnedSelection>,
    empty_document: Option<Vec<u8>>,
) -> Result<DocumentProvenance, ValueError> {
    DocumentProvenance::new(
        provenance.empty_document().map(<[u8]>::to_vec),
        provenance.containers().to_vec(),
        provenance.trailing_separators().to_vec(),
        fields,
        empty_document,
    )
}

fn is_empty_json_object(data: &[u8]) -> bool {
    let mut bytes = data
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace());
    bytes.next() == Some(b'{') && bytes.next() == Some(b'}') && bytes.next().is_none()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::content::{Key, Selector, SelectorPart, StructuredContent};
    use crate::values::Scalar;

    fn requirement(value: i64) -> DocumentSchema {
        DocumentSchema::new(DocumentFormat::Json, "version", Scalar::integer(value))
            .expect("schema")
    }

    fn request(key: &str, value: i64) -> DocumentRequest {
        let content = StructuredContent::new(
            DocumentFormat::Json,
            Selector::new([SelectorPart::Key(Key::new(key).expect("key"))]).expect("selector"),
            Value::from(value),
            None,
        )
        .expect("content");
        DocumentRequest::set(Content::Structured(content), Acquisition::Conflict).expect("request")
    }

    #[test]
    fn schema_matching_uses_typed_values_and_jsonc_rules() {
        let json = requirement(1);
        assert!(!matches(None, &json).expect("missing"));
        assert!(matches(Some(b"\xef\xbb\xbf{\"version\":1}"), &json).expect("match"));
        assert!(!matches(Some(b"{\"version\":1.0}"), &json).expect("typed mismatch"));
        let jsonc = DocumentSchema::new(DocumentFormat::Jsonc, "version", Scalar::integer(1))
            .expect("schema");
        assert!(matches(Some(b"{/* keep */\"version\":1}"), &jsonc).expect("jsonc"));
    }

    #[test]
    fn schema_scaffold_restores_blank_source_after_retirement() {
        let source = Some(b" \r\n".to_vec());
        let installed = edit_schema_document(
            source.clone(),
            &[],
            &[],
            &DocumentProvenance::default(),
            &[requirement(1)],
            true,
        )
        .expect("install");
        assert_eq!(installed.after(), Some(b"{\"version\":1}".as_slice()));
        assert_eq!(installed.ownership(), [] as [OwnedSelection; 0]);
        assert_eq!(installed.provenance().schema_fields().len(), 1);
        assert_eq!(
            installed.provenance().schema_empty_document(),
            source.as_deref()
        );

        let removed = edit_schema_document(
            installed.after().map(<[u8]>::to_vec),
            &[],
            installed.ownership(),
            installed.provenance(),
            &[],
            true,
        )
        .expect("remove");
        assert_eq!(removed.after(), source.as_deref());
        assert_eq!(
            removed.provenance().schema_fields(),
            [] as [OwnedSelection; 0]
        );
    }

    #[test]
    fn schema_validation_conflicts_without_editing() {
        let data = Some(b"{\"version\":2}".to_vec());
        let edit = edit_schema_document(
            data.clone(),
            &[request("x", 1)],
            &[],
            &DocumentProvenance::default(),
            &[requirement(1)],
            true,
        )
        .expect("conflict");
        assert_eq!(edit.before(), data.as_deref());
        assert_eq!(edit.after(), data.as_deref());
        let observation = edit.observations().first().expect("observation");
        assert_eq!(observation.status(), EditStatus::Conflict);
        assert_eq!(observation.message(), "document schema differs");
    }

    #[test]
    fn schema_rejects_duplicate_and_app_owned_keys() {
        let duplicate = [requirement(1), requirement(2)];
        let error = edit_schema_document(
            None,
            &[],
            &[],
            &DocumentProvenance::default(),
            &duplicate,
            true,
        )
        .expect_err("duplicate");
        assert_eq!(error.message(), "incompatible document schema requirements");
        let error = edit_schema_document(
            None,
            &[request("version", 1)],
            &[],
            &DocumentProvenance::default(),
            &[requirement(1)],
            true,
        )
        .expect_err("overlap");
        assert_eq!(
            error.message(),
            "document schema overlaps an app-owned field"
        );

        let repeated = requirement(1);
        let edit = edit_schema_document(
            None,
            &[],
            &[],
            &DocumentProvenance::default(),
            &[repeated.clone(), repeated],
            true,
        )
        .expect("identical requirements");
        assert_eq!(edit.after(), Some(b"{\"version\":1}".as_slice()));
    }
}
