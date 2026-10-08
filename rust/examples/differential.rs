#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use flyrail::{
    Acquisition, Artifact, Bundle, BundleEntry, BundleIdentity, Claim, ClaimSelector, Content,
    Dependency, DocumentFormat, DocumentProvenance, DocumentRequest, Family, FileContent, FileMode,
    InstallationResult, InstallationTarget, InstructionArtifact, Key, ModelValue, OwnedSelection,
    Ownership, RenderedArtifact, RenderedBundle, ResourceAuthority, Scalar, SectionContent,
    Selector, SelectorPart, StructuredContent, TargetError, Value, decode, edit_document, encode,
    inspect_installation, recover_installation, remove, sync,
};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args().skip(1);
    let mut corpus = None;
    let mut root = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--corpus" => corpus = arguments.next().map(PathBuf::from),
            "--root" => root = arguments.next().map(PathBuf::from),
            _ => return Err(usage()),
        }
    }
    let corpus = corpus.ok_or_else(usage)?;
    let root_argument = root.ok_or_else(usage)?;
    let parent = root_argument.parent().ok_or_else(usage)?;
    let name = root_argument.file_name().ok_or_else(usage)?;
    let root = fs::canonicalize(parent)
        .map_err(|error| error.to_string())?
        .join(name);
    fs::create_dir(&root).map_err(|error| error.to_string())?;
    let rows = read_rows(&corpus)?;
    run_values(&rows)?;
    run_bundles(&rows)?;
    run_rendered(&rows, &root)?;
    run_edits(&rows)?;
    run_lifecycle(&rows, &root)
}

fn usage() -> String {
    String::from("usage: differential --corpus FILE --root DIRECTORY")
}

fn read_rows(path: &Path) -> Result<Vec<Vec<String>>, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    Ok(text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split('\t').map(str::to_owned).collect())
        .collect())
}

fn hex(data: &[u8]) -> String {
    data.iter()
        .fold(String::with_capacity(data.len() * 2), |mut text, byte| {
            use std::fmt::Write as _;
            let _ = write!(text, "{byte:02x}");
            text
        })
}

fn unhex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err(String::from("odd-length fixture hex"));
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .map_err(|error| error.to_string())
                .and_then(|value| u8::from_str_radix(value, 16).map_err(|error| error.to_string()))
        })
        .collect()
}

fn emit(category: &str, case: &str, field: &str, value: &[u8]) {
    println!("{category}\t{case}\t{field}\t{}", hex(value));
}

fn selected<'a>(rows: &'a [Vec<String>], category: &str) -> impl Iterator<Item = &'a [String]> {
    rows.iter()
        .filter(move |row| row.first().is_some_and(|value| value == category))
        .map(Vec::as_slice)
}

fn run_values(rows: &[Vec<String>]) -> Result<(), String> {
    for row in selected(rows, "value") {
        let bytes = unhex(&row[2])?;
        let value = Value::from_semantic_bytes(&bytes).map_err(|error| error.to_string())?;
        emit("value", &row[1], "semantic", &value.semantic_bytes());
    }
    Ok(())
}

fn model_sequence(items: impl IntoIterator<Item = Value>) -> Value {
    Value::Array(flyrail::ArrayValue::new(items))
}

fn run_bundles(rows: &[Vec<String>]) -> Result<(), String> {
    for row in selected(rows, "bundle") {
        let count = row[4].parse::<usize>().map_err(|error| error.to_string())?;
        if row.len() != 5 + count * 3 {
            return Err(format!("{}: invalid bundle entry count", row[1]));
        }
        let entries = row[5..]
            .chunks_exact(3)
            .map(|entry| {
                let data = if entry[1] == "-" {
                    None
                } else {
                    Some(unhex(&entry[1])?)
                };
                BundleEntry::new(&entry[0], data, entry[2] == "1")
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let bundle =
            Bundle::from_memory(&unhex(&row[3])?, entries).map_err(|error| error.to_string())?;
        emit(
            "bundle",
            &row[1],
            "digest",
            bundle.content_digest().as_bytes(),
        );
        let canonical = model_sequence([
            model_sequence(bundle.artifacts().iter().map(ModelValue::model_value)),
            model_sequence(bundle.assets().iter().map(ModelValue::model_value)),
            model_sequence(bundle.dependencies().iter().map(ModelValue::model_value)),
        ]);
        emit("bundle", &row[1], "canonical", &canonical.semantic_bytes());
    }
    Ok(())
}

fn rendered_case(root: &Path, row: &[String]) -> Result<(RenderedBundle, Vec<Claim>), String> {
    let suffix = row[1].rsplit('-').next().unwrap_or_default();
    let first = RenderedArtifact::new(
        format!("base-{suffix}"),
        Family::Instructions,
        root.join(&row[2]),
        Content::File(FileContent::new(unhex(&row[3])?, FileMode::Readable)),
        None,
        false,
        Vec::new(),
    )
    .map_err(|error| error.to_string())?;
    let section = SectionContent::new(
        format!("section-{suffix}"),
        format!("value-{}\n", row[4]),
        None,
    )
    .map_err(|error| error.to_string())?;
    let section_claim = Claim::new(
        Ownership::Section,
        ClaimSelector::Section(section.selector().map_err(|error| error.to_string())?),
    )
    .map_err(|error| error.to_string())?;
    let second = RenderedArtifact::new(
        format!("section-{suffix}"),
        Family::Instructions,
        root.join("AGENTS.md"),
        Content::Section(section),
        None,
        false,
        Vec::new(),
    )
    .map_err(|error| error.to_string())?;
    let dependency = Dependency::new(second.id(), first.id()).map_err(|error| error.to_string())?;
    let first_id = String::from(first.id());
    let second_id = String::from(second.id());
    let artifacts = if row[5] == "file-first" {
        vec![first, second]
    } else {
        vec![second, first]
    };
    let context = if row[6] == "alpha-first" {
        vec![
            (String::from("alpha"), String::from("Straße")),
            (String::from("zeta"), row[4].clone()),
        ]
    } else {
        vec![
            (String::from("zeta"), row[4].clone()),
            (String::from("alpha"), String::from("Straße")),
        ]
    };
    let rendered =
        RenderedBundle::new(artifacts, [dependency], context).map_err(|error| error.to_string())?;
    let file_claim =
        Claim::new(Ownership::File, ClaimSelector::Whole).map_err(|error| error.to_string())?;
    let claims = BTreeMap::from([(first_id, file_claim), (second_id, section_claim)]);
    let ordered_claims = rendered
        .artifacts()
        .iter()
        .map(|artifact| {
            claims
                .get(artifact.id())
                .cloned()
                .ok_or_else(|| String::from("missing rendered claim"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((rendered, ordered_claims))
}

fn run_rendered(rows: &[Vec<String>], root: &Path) -> Result<(), String> {
    for row in selected(rows, "render") {
        let case_root = root.join(&row[1]);
        fs::create_dir(&case_root).map_err(|error| error.to_string())?;
        let (rendered, claims) = rendered_case(&case_root, row)?;
        emit(
            "render",
            &row[1],
            "digest",
            rendered.content_digest().as_bytes(),
        );
        let canonical = model_sequence([
            model_sequence(rendered.artifacts().iter().map(ModelValue::model_value)),
            model_sequence(rendered.dependencies().iter().map(ModelValue::model_value)),
        ]);
        emit("render", &row[1], "canonical", &canonical.semantic_bytes());
        let context = model_sequence(rendered.routing_context().iter().map(|(key, value)| {
            model_sequence([
                Value::Scalar(Scalar::text(key)),
                Value::Scalar(Scalar::text(value)),
            ])
        }));
        emit("render", &row[1], "context", &context.semantic_bytes());
        for (index, (artifact, claim)) in rendered.artifacts().iter().zip(claims).enumerate() {
            let authority = ResourceAuthority::new(artifact.destination())
                .map_err(|error| error.to_string())?;
            let identity = claim
                .identity(&authority)
                .map_err(|error| error.to_string())?;
            emit(
                "render",
                &row[1],
                &format!("claim-{index}"),
                identity.as_bytes(),
            );
        }
    }
    Ok(())
}

fn edit_content(format: &str, text: &str) -> Result<Content, String> {
    if format == "section" {
        return SectionContent::new("managed", text, None)
            .map(Content::Section)
            .map_err(|error| error.to_string());
    }
    let format = match format {
        "json" => DocumentFormat::Json,
        "jsonc" => DocumentFormat::Jsonc,
        "toml" => DocumentFormat::Toml,
        _ => return Err(String::from("unknown edit format")),
    };
    let selector = Selector::new([SelectorPart::Key(
        Key::new("tool").map_err(|error| error.to_string())?,
    )])
    .map_err(|error| error.to_string())?;
    StructuredContent::new(format, selector, Value::Scalar(Scalar::text(text)), None)
        .map(Content::Structured)
        .map_err(|error| error.to_string())
}

fn snapshot_edit(
    name: &str,
    step: usize,
    data: Option<&[u8]>,
    ownership: &[OwnedSelection],
    provenance: &DocumentProvenance,
    statuses: &str,
) {
    emit("edit", name, &format!("{step}-bytes"), data.unwrap_or(b"-"));
    emit("edit", name, &format!("{step}-status"), statuses.as_bytes());
    emit(
        "edit",
        name,
        &format!("{step}-provenance"),
        &encode(provenance),
    );
    emit(
        "edit",
        name,
        &format!("{step}-ownership-count"),
        ownership.len().to_string().as_bytes(),
    );
    for (index, item) in ownership.iter().enumerate() {
        emit(
            "edit",
            name,
            &format!("{step}-ownership-{index}"),
            &encode(item),
        );
    }
}

fn run_edits(rows: &[Vec<String>]) -> Result<(), String> {
    for row in selected(rows, "edit") {
        let mut data = if row[3] == "-" {
            None
        } else {
            Some(unhex(&row[3])?)
        };
        let mut ownership = Vec::new();
        let mut provenance = DocumentProvenance::default();
        let count = row[4].parse::<usize>().map_err(|error| error.to_string())?;
        if row.len() != 5 + count * 3 {
            return Err(format!("{}: invalid edit operation count", row[1]));
        }
        let mut claim = None;
        let requests = row[5..]
            .chunks_exact(3)
            .map(|operation| match operation[0].as_str() {
                "set" => {
                    let request = DocumentRequest::set(
                        edit_content(&row[2], &operation[1])?,
                        operation[2]
                            .parse::<Acquisition>()
                            .map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| error.to_string())?;
                    claim = Some(request.claim().clone());
                    Ok(request)
                }
                "retire" => DocumentRequest::retire(
                    claim
                        .clone()
                        .ok_or_else(|| format!("{}: retire before set", row[1]))?,
                )
                .map_err(|error| error.to_string()),
                _ => Err(format!("{}: invalid edit operation", row[1])),
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (index, request) in requests.iter().enumerate() {
            let edit = edit_document(data, std::slice::from_ref(request), &ownership, &provenance)
                .map_err(|error| error.to_string())?;
            data = edit.after().map(<[u8]>::to_vec);
            ownership = edit.ownership().to_vec();
            provenance = edit.provenance().clone();
            let statuses = edit
                .observations()
                .iter()
                .map(|item| item.status().as_str())
                .collect::<Vec<_>>()
                .join(",");
            snapshot_edit(
                &row[1],
                index + 1,
                data.as_deref(),
                &ownership,
                &provenance,
                &statuses,
            );
        }
    }
    Ok(())
}

fn lifecycle_bundle(version: &str, ids: &[String]) -> Result<Bundle, String> {
    let artifacts = ids
        .iter()
        .map(|identifier| {
            InstructionArtifact::new(identifier, identifier)
                .map(Artifact::Instruction)
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Bundle::from_artifacts(
        BundleIdentity::new("team", version).map_err(|error| error.to_string())?,
        artifacts,
        [],
        [],
    )
    .map_err(|error| error.to_string())
}

fn lifecycle_rendered(
    root: &Path,
    kind: &str,
    text: &str,
) -> Result<(RenderedBundle, Vec<String>), String> {
    let destination = root.join(match kind {
        "file" | "alias" => "owned",
        "casefold" => "Straße",
        "toml" => "config.toml",
        "json" => "config.json",
        _ => "AGENTS.md",
    });
    if kind == "shared" {
        let mut artifacts = ["alpha", "zeta"]
            .into_iter()
            .map(|identifier| {
                RenderedArtifact::new(
                    identifier,
                    Family::Instructions,
                    destination.clone(),
                    Content::Section(
                        SectionContent::new(identifier, format!("{identifier}-{text}\n"), None)
                            .map_err(|error| error.to_string())?,
                    ),
                    None,
                    false,
                    Vec::new(),
                )
                .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, String>>()?;
        artifacts.reverse();
        return RenderedBundle::new(artifacts, [], [])
            .map(|rendered| (rendered, vec![String::from("alpha"), String::from("zeta")]))
            .map_err(|error| error.to_string());
    }
    if kind == "alias" {
        let artifacts = ["alpha", "zeta"]
            .into_iter()
            .map(|identifier| {
                RenderedArtifact::new(
                    identifier,
                    Family::Instructions,
                    destination.clone(),
                    Content::File(FileContent::new(text.as_bytes(), FileMode::Readable)),
                    None,
                    false,
                    Vec::new(),
                )
                .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        return RenderedBundle::new(artifacts, [], [])
            .map(|rendered| (rendered, vec![String::from("alpha"), String::from("zeta")]))
            .map_err(|error| error.to_string());
    }
    let content = if matches!(kind, "file" | "casefold") {
        Content::File(FileContent::new(text.as_bytes(), FileMode::Readable))
    } else {
        edit_content(kind, text)?
    };
    let artifact = RenderedArtifact::new(
        "managed",
        Family::Instructions,
        destination,
        content,
        None,
        false,
        Vec::new(),
    )
    .map_err(|error| error.to_string())?;
    RenderedBundle::new(
        [artifact],
        [],
        [
            (String::from("seed"), String::from("1592639710")),
            (String::from("surface"), String::from("differential")),
        ],
    )
    .map(|rendered| (rendered, vec![String::from("managed")]))
    .map_err(|error| error.to_string())
}

fn json_string(value: &str) -> String {
    let mut result = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\u{08}' => result.push_str("\\b"),
            '\u{0c}' => result.push_str("\\f"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            value if value <= '\u{1f}' => {
                use std::fmt::Write as _;
                let _ = write!(result, "\\u{:04x}", u32::from(value));
            }
            value => result.push(value),
        }
    }
    result.push('"');
    result
}

fn json_object(fields: impl IntoIterator<Item = (&'static str, String)>) -> String {
    let ordered: BTreeMap<_, _> = fields.into_iter().collect();
    format!(
        "{{{}}}",
        ordered
            .into_iter()
            .map(|(key, value)| format!("{}:{value}", json_string(key)))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn json_array(items: impl IntoIterator<Item = String>) -> String {
    format!("[{}]", items.into_iter().collect::<Vec<_>>().join(","))
}

fn optional(value: Option<&str>) -> String {
    value.map_or_else(|| String::from("null"), json_string)
}

fn relative_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root).map_or_else(
        |_| path.to_string_lossy().replace('\\', "/"),
        |relative| format!("$ROOT/{}", relative.to_string_lossy().replace('\\', "/")),
    )
}

fn error_record(error: Option<&TargetError>, root: &Path) -> String {
    error.map_or_else(
        || String::from("null"),
        |error| {
            json_object([
                ("code", json_string(error.code().as_str())),
                ("message", json_string(error.message())),
                (
                    "path",
                    error.path().map_or_else(
                        || String::from("null"),
                        |path| json_string(&relative_path(path, root)),
                    ),
                ),
            ])
        },
    )
}

fn result_record(result: &InstallationResult, root: &Path) -> Vec<u8> {
    let observation = result.observation();
    let observed_resources = json_array(observation.resources().iter().map(|resource| {
        let claims = json_array(resource.claims().iter().map(|claim| {
            json_object([
                ("artifact", json_string(claim.artifact_id())),
                ("bundle", json_string(claim.bundle_id())),
                ("bundle_digest", json_string(claim.bundle_digest())),
                ("claim", json_string(claim.claim_id())),
                ("render_digest", json_string(claim.render_digest())),
                ("status", json_string(claim.status().as_str())),
                ("version", json_string(claim.version())),
            ])
        }));
        json_object([
            ("claims", claims),
            (
                "destination",
                json_string(&relative_path(resource.destination(), root)),
            ),
            ("error", error_record(resource.error(), root)),
            ("revision", optional(resource.revision_digest())),
            (
                "state_root",
                json_string(&relative_path(resource.state_root(), root)),
            ),
        ])
    }));
    let resources = json_array(result.resources().iter().map(|resource| {
        json_object([
            (
                "destination",
                json_string(&relative_path(resource.resource().destination(), root)),
            ),
            ("error", error_record(resource.error(), root)),
            ("status", json_string(resource.status().as_str())),
        ])
    }));
    json_object([
        ("error", error_record(result.error(), root)),
        (
            "observation",
            json_object([
                ("bundle_digest", optional(observation.bundle_digest())),
                ("current", observation.is_current().to_string()),
                ("error", error_record(observation.error(), root)),
                ("pending", observation.pending().to_string()),
                ("render_digest", optional(observation.render_digest())),
                ("resources", observed_resources),
                ("version", optional(observation.version())),
            ]),
        ),
        ("resources", resources),
        ("status", json_string(result.status().as_str())),
    ])
    .into_bytes()
}

fn state_snapshot(
    name: &str,
    step: &str,
    root: &Path,
    target: &InstallationTarget,
) -> Result<(), String> {
    let mut candidates = vec![
        target.index_root().join("index.json"),
        target.index_root().join("index.json.next"),
    ];
    collect_receipts(root, &mut candidates).map_err(|error| error.to_string())?;
    candidates.sort();
    candidates.dedup();
    for path in candidates {
        if !path.is_file() {
            continue;
        }
        let raw = fs::read(&path).map_err(|error| error.to_string())?;
        if path.file_name().and_then(|name| name.to_str()) == Some("receipt.json") {
            let value: flyrail::Receipt = decode(&raw).map_err(|error| error.to_string())?;
            if encode(&value) != raw {
                return Err(String::from("receipt is not canonical"));
            }
        } else {
            let value: flyrail::Index = decode(&raw).map_err(|error| error.to_string())?;
            if encode(&value) != raw {
                return Err(String::from("index is not canonical"));
            }
        }
        let normalized = normalize_state(&raw, root)?;
        emit(
            "lifecycle",
            name,
            &format!("{step}-state-{}", relative_path(&path, root)),
            &normalized,
        );
    }
    Ok(())
}

fn normalize_state(raw: &[u8], root: &Path) -> Result<Vec<u8>, String> {
    fn normalize(value: &mut serde_json::Value, root: &Path) {
        match value {
            serde_json::Value::Array(items) => {
                for item in items {
                    normalize(item, root);
                }
            }
            serde_json::Value::Object(items) => {
                for item in items.values_mut() {
                    normalize(item, root);
                }
                if items.len() == 1
                    && let Some(serde_json::Value::String(path)) = items.get_mut("path")
                {
                    *path = relative_path(Path::new(path), root);
                }
                let type_name = items
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                if let Some(serde_json::Value::Object(fields)) = items.get_mut("fields") {
                    if matches!(type_name.as_deref(), Some("Node" | "Ancestor")) {
                        for key in ["device", "inode", "uid", "gid"] {
                            fields.insert(key.to_owned(), serde_json::Value::from(0));
                        }
                        fields.insert(String::from("security"), serde_json::json!({"bytes": ""}));
                    } else if type_name.as_deref() == Some("Receipt") {
                        fields.insert(
                            String::from("transaction_id"),
                            serde_json::Value::String("0".repeat(32)),
                        );
                    }
                }
            }
            _ => {}
        }
    }

    let mut value: serde_json::Value =
        serde_json::from_slice(raw).map_err(|error| error.to_string())?;
    normalize(&mut value, root);
    let mut encoded = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    encoded.push(b'\n');
    Ok(encoded)
}

fn collect_receipts(root: &Path, paths: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_receipts(&path, paths)?;
        } else if entry.file_name() == "receipt.json" {
            paths.push(path);
        }
    }
    Ok(())
}

fn payload_snapshot(root: &Path) -> Result<Vec<u8>, String> {
    fn collect(
        root: &Path,
        current: &Path,
        items: &mut Vec<(String, String)>,
    ) -> std::io::Result<()> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                collect(root, &path, items)?;
            } else {
                let name = path.to_string_lossy();
                if !name.contains(".flyrail")
                    && path.file_name().and_then(|item| item.to_str()) != Some("lock")
                {
                    items.push((relative_path(&path, root), hex(&fs::read(&path)?)));
                }
            }
        }
        Ok(())
    }
    let mut items = Vec::new();
    collect(root, root, &mut items).map_err(|error| error.to_string())?;
    items.sort();
    Ok(json_array(
        items
            .into_iter()
            .map(|(path, data)| json_array([json_string(&path), json_string(&data)])),
    )
    .into_bytes())
}

fn lifecycle_result(
    name: &str,
    step: &str,
    result: &InstallationResult,
    root: &Path,
    target: &InstallationTarget,
) -> Result<(), String> {
    emit(
        "lifecycle",
        name,
        &format!("{step}-result"),
        &result_record(result, root),
    );
    state_snapshot(name, step, root, target)?;
    emit(
        "lifecycle",
        name,
        &format!("{step}-payloads"),
        &payload_snapshot(root)?,
    );
    Ok(())
}

fn run_lifecycle(rows: &[Vec<String>], root: &Path) -> Result<(), String> {
    for row in selected(rows, "lifecycle") {
        let case_root = root.join(&row[1]);
        fs::create_dir(&case_root).map_err(|error| error.to_string())?;
        let target = InstallationTarget::without_context(case_root.join("index"))
            .map_err(|error| error.to_string())?;
        let (first_rendered, ids) = lifecycle_rendered(&case_root, &row[2], &row[4])?;
        let destination = first_rendered.artifacts()[0].destination();
        if row[3] != "-" {
            fs::write(
                destination,
                if row[6] == "adopt" {
                    row[4].as_bytes().to_vec()
                } else {
                    unhex(&row[3])?
                },
            )
            .map_err(|error| error.to_string())?;
        }
        let acquisition = row[6]
            .parse::<Acquisition>()
            .map_err(|error| error.to_string())?;
        let created = sync(
            &lifecycle_bundle("1", &ids)?,
            &first_rendered,
            &target,
            acquisition,
            false,
            Duration::ZERO,
        )
        .map_err(|error| error.to_string())?;
        lifecycle_result(&row[1], "1-create", &created, &case_root, &target)?;
        let unchanged = sync(
            &lifecycle_bundle("1", &ids)?,
            &first_rendered,
            &target,
            Acquisition::Conflict,
            false,
            Duration::ZERO,
        )
        .map_err(|error| error.to_string())?;
        lifecycle_result(&row[1], "2-unchanged", &unchanged, &case_root, &target)?;
        let (second_rendered, _) = lifecycle_rendered(&case_root, &row[2], &row[5])?;
        let updated = sync(
            &lifecycle_bundle("2", &ids)?,
            &second_rendered,
            &target,
            Acquisition::Conflict,
            false,
            Duration::ZERO,
        )
        .map_err(|error| error.to_string())?;
        lifecycle_result(&row[1], "3-update", &updated, &case_root, &target)?;
        fs::copy(
            target.index_root().join("index.json"),
            target.index_root().join("index.json.next"),
        )
        .map_err(|error| error.to_string())?;
        let recovered = recover_installation("team", &target, Duration::ZERO)
            .map_err(|error| error.to_string())?;
        lifecycle_result(&row[1], "4-recover", &recovered, &case_root, &target)?;
        let observed = inspect_installation("team", &target).map_err(|error| error.to_string())?;
        emit(
            "lifecycle",
            &row[1],
            "5-inspect",
            json_object([
                ("current", observed.is_current().to_string()),
                ("pending", observed.pending().to_string()),
                (
                    "revision",
                    json_array(
                        observed
                            .resources()
                            .iter()
                            .map(|item| optional(item.revision_digest())),
                    ),
                ),
            ])
            .as_bytes(),
        );
        let removed =
            remove("team", &target, false, Duration::ZERO).map_err(|error| error.to_string())?;
        lifecycle_result(&row[1], "6-remove", &removed, &case_root, &target)?;
    }
    Ok(())
}
