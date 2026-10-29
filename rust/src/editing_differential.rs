#![allow(clippy::expect_used, clippy::indexing_slicing)]

use crate::{
    Acquisition, ArrayValue, Claim, ClaimSelector, Content, DocumentEdit, DocumentFormat,
    DocumentProvenance, DocumentRequest, DocumentSchema, Key, Member, ObjectValue, OwnedSelection,
    Ownership, Placement, Position, Scalar, SectionBoundaries, SectionContent, SelectionSnapshot,
    Selector, SelectorPart, SnapshotValue, StructuredContent, Value, ValueError, edit_document,
};
use serde_json::{Value as Json, json};

#[test]
fn complete_editing_records() {
    let Ok(path) = std::env::var("FLYRAIL_EDIT_CORPUS") else {
        return;
    };
    let bytes = std::fs::read(path).expect("read editing corpus");
    let corpus: Json = serde_json::from_slice(&bytes).expect("parse editing corpus");
    for case in corpus.as_array().expect("editing cases") {
        let mut source = case["before_hex"].as_str().map_or_else(
            || case["before"].as_str().map(|text| text.as_bytes().to_vec()),
            |hex| Some(unhex(hex)),
        );
        let mut ownership = Vec::new();
        let mut provenance = DocumentProvenance::default();
        for (index, step) in case["steps"]
            .as_array()
            .expect("editing steps")
            .iter()
            .enumerate()
        {
            let observed = step.get("observed").map_or_else(
                || source.clone(),
                |value| value.as_str().map(|text| text.as_bytes().to_vec()),
            );
            let result = edit_step(case, step, observed, &ownership, &provenance);
            let record = match result {
                Ok(edit) => {
                    let record = edit_record(&edit);
                    source = edit.after().map(<[u8]>::to_vec);
                    ownership = edit.ownership().to_vec();
                    provenance = edit.provenance().clone();
                    record
                }
                Err(error) => json!({"error":error.message()}),
            };
            println!(
                "{}\t{index}\t{record}",
                case["name"].as_str().expect("case name")
            );
        }
    }
}

fn edit_step(
    case: &Json,
    step: &Json,
    source: Option<Vec<u8>>,
    ownership: &[OwnedSelection],
    provenance: &DocumentProvenance,
) -> Result<DocumentEdit, ValueError> {
    let format = case["format"].as_str().expect("format");
    let records = step
        .get("requests")
        .and_then(Json::as_array)
        .map_or_else(|| vec![step], |items| items.iter().collect());
    let requests = records
        .into_iter()
        .map(|item| request(format, item))
        .collect::<Result<Vec<_>, _>>()?;
    if case["schema"].as_bool() == Some(true) {
        let schemas = step["schemas"]
            .as_array()
            .expect("schemas")
            .iter()
            .map(|item| {
                let Value::Scalar(value) = native(&item["value"])? else {
                    return Err(ValueError::new("invalid schema fixture"));
                };
                DocumentSchema::new(
                    document_format(format),
                    item["key"].as_str().expect("schema key"),
                    value,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        crate::schema::edit_schema_document(
            source,
            &requests,
            ownership,
            provenance,
            &schemas,
            step["validate"].as_bool().unwrap_or(true),
        )
    } else {
        edit_document(source, &requests, ownership, provenance)
    }
}

fn document_format(text: &str) -> DocumentFormat {
    match text {
        "json" => DocumentFormat::Json,
        "jsonc" => DocumentFormat::Jsonc,
        "toml" => DocumentFormat::Toml,
        _ => unreachable!("fixture document format"),
    }
}

fn request(format: &str, record: &Json) -> Result<DocumentRequest, ValueError> {
    let selector = if format == "text" {
        ClaimSelector::Section(SectionBoundaries::new(
            record["selector"]["start"].as_str().expect("start"),
            record["selector"]["end"].as_str().expect("end"),
        )?)
    } else {
        ClaimSelector::Structured(Selector::new(
            record["selector"]
                .as_array()
                .expect("selector")
                .iter()
                .map(|part| {
                    part.as_str().map_or_else(
                        || member(part).map(SelectorPart::Member),
                        |name| Key::new(name).map(SelectorPart::Key),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
        )?)
    };
    let claim = Claim::new(
        if format == "text" {
            Ownership::Section
        } else {
            Ownership::Structured
        },
        selector.clone(),
    )?;
    if record["action"].as_str() == Some("retire") {
        return DocumentRequest::retire(claim);
    }
    let acquisition = match record["acquisition"].as_str().unwrap_or("conflict") {
        "takeover" => Acquisition::Takeover,
        "adopt" => Acquisition::Adopt,
        _ => Acquisition::Conflict,
    };
    let content = match selector {
        ClaimSelector::Section(boundaries) => Content::Section(SectionContent::new(
            record["marker"].as_str().unwrap_or("fixture"),
            record["text"].as_str().expect("section text"),
            Some(boundaries),
        )?),
        ClaimSelector::Structured(selector) => {
            let value = typed(record, "semantic_value", "value")?;
            let placement = record
                .get("placement")
                .map(|placement| {
                    let position = match placement["position"].as_str().expect("position") {
                        "first" => Position::First,
                        "last" => Position::Last,
                        "before" => Position::Before,
                        "after" => Position::After,
                        _ => unreachable!("fixture position"),
                    };
                    Placement::new(position, placement.get("anchor").map(member).transpose()?)
                })
                .transpose()?;
            Content::Structured(StructuredContent::new(
                document_format(format),
                selector,
                value,
                placement,
            )?)
        }
        _ => unreachable!("fixture claim"),
    };
    DocumentRequest::set(content, acquisition)
}

fn member(record: &Json) -> Result<Member, ValueError> {
    Member::new(
        typed(record, "semantic_identity", "identity")?,
        record
            .get("key")
            .and_then(Json::as_array)
            .map_or_else(Vec::new, |items| {
                items
                    .iter()
                    .map(|item| item.as_str().expect("member key").to_owned())
                    .collect()
            }),
    )
}

fn typed(record: &Json, semantic: &str, plain: &str) -> Result<Value, ValueError> {
    record.get(semantic).map_or_else(
        || native(&record[plain]),
        |value| Value::from_semantic_bytes(value.to_string().as_bytes()),
    )
}

fn native(value: &Json) -> Result<Value, ValueError> {
    Ok(match value {
        Json::Null => Value::Scalar(Scalar::null()),
        Json::Bool(value) => Value::Scalar(Scalar::boolean(*value)),
        Json::String(value) => Value::Scalar(Scalar::text(value)),
        Json::Number(value) => {
            if let Some(value) = value.as_i64() {
                Value::Scalar(Scalar::integer(value))
            } else {
                Value::Scalar(Scalar::float(value.as_f64().expect("float"))?)
            }
        }
        Json::Array(items) => Value::Array(ArrayValue::new(
            items.iter().map(native).collect::<Result<Vec<_>, _>>()?,
        )),
        Json::Object(fields) => Value::Object(ObjectValue::new(
            fields
                .iter()
                .map(|(key, value)| native(value).map(|value| (key.clone(), value)))
                .collect::<Result<Vec<_>, _>>()?,
        )?),
    })
}

fn edit_record(edit: &DocumentEdit) -> Json {
    let provenance = edit.provenance();
    json!({
        "before":edit.before().map(hex), "after":edit.after().map(hex),
        "applicable":edit.applicable(),
        "ownership":edit.ownership().iter().map(owned_record).collect::<Vec<_>>(),
        "provenance":{
            "empty_document":provenance.empty_document().map(hex),
            "containers":provenance.containers().iter().map(|item| json!({
                "selector":selector_record(item.selector()), "empty_syntax":hex(item.empty_syntax()),
            })).collect::<Vec<_>>(),
            "trailing_separators":provenance.trailing_separators(),
            "schema_fields":provenance.schema_fields().iter().map(owned_record).collect::<Vec<_>>(),
            "schema_empty_document":provenance.schema_empty_document().map(hex),
        },
        "observations":edit.observations().iter().map(|item| json!({
            "claim":claim_record(item.claim()), "status":item.status().as_str(),
            "current":item.current().map(snapshot_record), "message":item.message(),
        })).collect::<Vec<_>>(),
    })
}

fn owned_record(owned: &OwnedSelection) -> Json {
    json!({
        "claim":claim_record(owned.claim()), "format":owned.format().map(DocumentFormat::as_str),
        "installed":snapshot_record(owned.installed()), "disposition":owned.disposition().as_str(),
        "baseline":owned.baseline().map(snapshot_record), "separator":hex(owned.separator()),
    })
}

fn snapshot_record(snapshot: &SelectionSnapshot) -> Json {
    let value = match snapshot.value() {
        SnapshotValue::Bytes(bytes) => json!({"bytes":hex(bytes)}),
        SnapshotValue::Typed(value) => json!({"semantic":hex(&value.semantic_bytes())}),
    };
    json!({
        "value":value, "syntax":hex(snapshot.syntax()), "owned_syntax":hex(snapshot.owned_syntax()),
        "layout":hex(snapshot.layout()),
        "position":snapshot.position().map(|position| json!({
            "previous":position.previous().map(member_record),
            "following":position.following().map(member_record), "prefix":hex(position.prefix()),
        })),
    })
}

fn claim_record(claim: &Claim) -> Json {
    let selector = match claim.selector() {
        ClaimSelector::Section(boundaries) => {
            json!({"start":boundaries.start(),"end":boundaries.end()})
        }
        ClaimSelector::Structured(selector) => selector_record(selector),
        _ => unreachable!("fixture selector"),
    };
    json!({"kind":claim.kind().as_str(),"selector":selector})
}

fn selector_record(selector: &Selector) -> Json {
    Json::Array(
        selector
            .parts()
            .iter()
            .map(|part| match part {
                SelectorPart::Key(key) => json!(key.name()),
                SelectorPart::Member(member) => member_record(member),
            })
            .collect(),
    )
}

fn member_record(member: &Member) -> Json {
    json!({"identity":hex(&member.identity().semantic_bytes()),"key":member.key()})
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut text, byte| {
        write!(text, "{byte:02x}").expect("format hex");
        text
    })
}

fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("hex UTF-8"), 16).expect("hex byte")
        })
        .collect()
}
