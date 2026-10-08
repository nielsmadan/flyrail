use crate::artifacts::{Artifact, Family, InstructionArtifact, McpArtifact};
use crate::bundle::Bundle;
use crate::content::{Content, FileContent, FileMode, SectionContent, SelectorPart};
use crate::destinations::{RenderContext, instruction_destination, mcp_destination};
use crate::hook_translation::{
    HookTranslationError, hook_artifacts, support_asset_artifacts, translate_native_artifact,
};
use crate::json::Json;
use crate::mcp_translation::{TranslationFailure, UnsupportedTranslation, mcp_content};
use crate::rendered::{Dependency, Notice, NoticeKind, RenderedArtifact, RenderedBundle};
use crate::resource_plan::artifact_claim;
use crate::targets::{Agent, Surface, TargetScope};
use crate::values::ValueError;
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactTranslation {
    artifact: Option<RenderedArtifact>,
    notices: Vec<Notice>,
}

impl ArtifactTranslation {
    pub const fn artifact(&self) -> Option<&RenderedArtifact> {
        self.artifact.as_ref()
    }

    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }

    pub fn supported(&self) -> bool {
        self.artifact.is_some()
            && !self
                .notices
                .iter()
                .any(|notice| notice.kind() == NoticeKind::Unsupported)
    }
}

fn identifier(values: &[&str]) -> String {
    let mut encoded = String::new();
    Json::Array(
        values
            .iter()
            .map(|value| Json::Str((*value).to_owned()))
            .collect(),
    )
    .encode(&mut encoded);
    Sha256::digest(encoded.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut result, byte| {
            let _ = write!(result, "{byte:02x}");
            result
        })
}

fn unsupported(
    artifact_id: &str,
    error: &UnsupportedTranslation,
) -> Result<ArtifactTranslation, ValueError> {
    Ok(ArtifactTranslation {
        artifact: None,
        notices: vec![Notice::with_code(
            artifact_id,
            NoticeKind::Unsupported,
            error.message(),
            error.code(),
        )?],
    })
}

fn notices(
    artifact_id: &str,
    family: Family,
    context: &RenderContext,
) -> Result<Vec<Notice>, ValueError> {
    let mut result = Vec::new();
    let agent = context.audience().agent();
    if family == Family::Instructions {
        result.push(Notice::with_code(
            artifact_id,
            NoticeKind::Activation,
            "Instruction discovery and loading are controlled by the host and current session.",
            "instruction-loading",
        )?);
        if agent == Agent::Codex {
            result.push(Notice::with_code(
                artifact_id,
                NoticeKind::Activation,
                "AGENTS.override.md takes precedence; instructions default to a 32 KiB limit. Start a new session after changes and check host discovery.",
                "codex-instruction-discovery",
            )?);
        }
        if context.surface() == Surface::Vscode && context.target().scope() == TargetScope::User {
            result.push(Notice::with_code(
                artifact_id,
                NoticeKind::Activation,
                "User instructions target VS Code Agent Host; local extension-host sessions may use different discovery settings.",
                "vscode-agent-host-instructions",
            )?);
        }
    }
    if family == Family::Mcp {
        result.push(Notice::with_code(
            artifact_id,
            NoticeKind::Activation,
            "Registration does not grant trust, approve tools, supply credentials or prove server activation.",
            "mcp-activation",
        )?);
        if agent == Agent::Pi {
            result.push(Notice::with_code(
                artifact_id,
                NoticeKind::Prerequisite,
                "Requires externally installed pi-mcp-adapter; emitted schema targets v2.32.1.",
                "pi-mcp-adapter-v2-32-1",
            )?);
        }
        if context.surface() == Surface::Vscode {
            result.push(Notice::with_code(
                artifact_id,
                NoticeKind::Activation,
                "VS Code forwards these servers to Agent Host, which does not read this servers document directly.",
                "vscode-mcp-forwarding",
            )?);
        }
    }
    Ok(result)
}

pub fn translate_instruction_artifact(
    bundle_id: &str,
    artifact: &InstructionArtifact,
    context: &RenderContext,
) -> Result<ArtifactTranslation, ValueError> {
    let Some(mut path) = instruction_destination(context)? else {
        return unsupported(
            artifact.id(),
            &UnsupportedTranslation::new(
                "instruction-destination",
                "Cursor user rules have no verified filesystem preset.",
            ),
        );
    };
    let marker = identifier(&[bundle_id, artifact.id()]);
    let payload =
        if context.surface() == Surface::Vscode && context.target().scope() == TargetScope::User {
            if context.instruction_path().is_none() {
                path.push(format!("{marker}.instructions.md"));
            }
            if !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".instructions.md"))
            {
                return unsupported(
                    artifact.id(),
                    &UnsupportedTranslation::new(
                        "instruction-extension",
                        "VS Code user instructions require an .instructions.md file.",
                    ),
                );
            }
            Content::File(FileContent::new(
                format!("---\napplyTo: \"**\"\n---\n\n{}", artifact.text()).into_bytes(),
                FileMode::Private,
            ))
        } else {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if path.extension().is_some_and(|extension| extension == "mdc")
                || name.ends_with(".instructions.md")
            {
                return unsupported(
                    artifact.id(),
                    &UnsupportedTranslation::new(
                        "native-instruction-required",
                        "Use native content for authored modular rule metadata.",
                    ),
                );
            }
            Content::Section(SectionContent::new(marker, artifact.text(), None)?)
        };
    Ok(ArtifactTranslation {
        artifact: Some(RenderedArtifact::new(
            artifact.id(),
            Family::Instructions,
            path,
            payload,
            None,
            false,
            Vec::new(),
        )?),
        notices: notices(artifact.id(), Family::Instructions, context)?,
    })
}

pub fn translate_mcp_artifact(
    artifact: &McpArtifact,
    context: &RenderContext,
    asset_destinations: impl IntoIterator<Item = (String, PathBuf)>,
) -> Result<ArtifactTranslation, ValueError> {
    let Some(path) = mcp_destination(context)? else {
        let (code, message) = if context.surface() == Surface::Vscode {
            (
                "mcp-profile-path",
                "VS Code user MCP requires an explicit profile mcp.json path.",
            )
        } else {
            (
                "mcp-explicit-path",
                "Relocated Claude user MCP requires an explicit mcp_path.",
            )
        };
        return unsupported(artifact.id(), &UnsupportedTranslation::new(code, message));
    };
    if context.surface() == Surface::Vscode
        && path.file_name().and_then(|name| name.to_str()) != Some("mcp.json")
    {
        return unsupported(
            artifact.id(),
            &UnsupportedTranslation::new(
                "mcp-profile-schema",
                "VS Code routes require mcp.json with the servers schema; use the CLI surface for Agent Host native mcpServers files.",
            ),
        );
    }
    let assets: HashMap<String, PathBuf> = asset_destinations.into_iter().collect();
    let payload = match mcp_content(artifact, context, &path, &assets) {
        Ok(payload) => payload,
        Err(TranslationFailure::Unsupported(error)) => {
            return unsupported(artifact.id(), &error);
        }
        Err(TranslationFailure::Invalid(error)) => return Err(error),
    };
    Ok(ArtifactTranslation {
        artifact: Some(RenderedArtifact::new(
            artifact.id(),
            Family::Mcp,
            path,
            Content::Structured(payload),
            None,
            false,
            Vec::new(),
        )?),
        notices: notices(artifact.id(), Family::Mcp, context)?,
    })
}

fn collision_notices(artifacts: &[RenderedArtifact]) -> Result<Vec<Notice>, ValueError> {
    let mut selected: Vec<&RenderedArtifact> = Vec::new();
    let mut notices = Vec::new();
    for artifact in artifacts {
        for previous in &selected {
            if artifact.destination() != previous.destination() {
                continue;
            }
            if matches!(
                (artifact.content(), previous.content()),
                (Content::Structured(left), Content::Structured(right))
                    if left.format() != right.format()
            ) {
                notices.push(Notice::with_code(
                    artifact.id(),
                    NoticeKind::Unsupported,
                    format!(
                        "Incompatible document formats from {} and {} at {}.",
                        previous.id(),
                        artifact.id(),
                        artifact.destination().display()
                    ),
                    "document-format-collision",
                )?);
                continue;
            }
            let claim = artifact_claim(artifact)?;
            let previous_claim = artifact_claim(previous)?;
            if claim.overlaps(&previous_claim)
                && (claim != previous_claim
                    || artifact.content() != previous.content()
                    || artifact.schema_requirements() != previous.schema_requirements()
                    || artifact.family() != previous.family())
            {
                notices.push(Notice::with_code(
                    artifact.id(),
                    NoticeKind::Unsupported,
                    format!(
                        "Incompatible claims from {} and {} at {}.",
                        previous.id(),
                        artifact.id(),
                        artifact.destination().display()
                    ),
                    "destination-collision",
                )?);
            }
        }
        selected.push(artifact);
    }
    Ok(notices)
}

fn copilot_precedence_notices(
    artifacts: &[RenderedArtifact],
    contexts: &[RenderContext],
) -> Result<Vec<Notice>, ValueError> {
    let roots: BTreeSet<PathBuf> = contexts
        .iter()
        .filter(|context| {
            context.audience().agent() == Agent::Copilot
                && context.surface() == Surface::Cli
                && context.target().scope() == TargetScope::Project
        })
        .map(|context| context.root().to_path_buf())
        .collect();
    let mut notices = Vec::new();
    for root in roots {
        let higher = artifacts
            .iter()
            .filter(|artifact| artifact.destination() == root.join(".mcp.json"));
        let lower = artifacts
            .iter()
            .filter(|artifact| artifact.destination() == root.join(".github/mcp.json"));
        for artifact in lower {
            for previous in higher.clone() {
                match (artifact.content(), previous.content()) {
                    (Content::Structured(content), Content::Structured(previous_content))
                        if content.selector().parts().len() == 2
                            && previous_content.selector().parts().len() == 2
                            && matches!(
                                content.selector().parts().first(),
                                Some(SelectorPart::Key(key)) if key.name() == "mcpServers"
                            )
                            && matches!(
                                previous_content.selector().parts().first(),
                                Some(SelectorPart::Key(key)) if key.name() == "mcpServers"
                            ) =>
                    {
                        let Some(SelectorPart::Key(server)) = content.selector().parts().get(1)
                        else {
                            notices.push(precedence_unverified(artifact, previous)?);
                            continue;
                        };
                        if !matches!(
                            previous_content.selector().parts().get(1),
                            Some(SelectorPart::Key(_))
                        ) {
                            notices.push(precedence_unverified(artifact, previous)?);
                            continue;
                        }
                        if content.selector() == previous_content.selector()
                            && content.value() != previous_content.value()
                        {
                            notices.push(Notice::with_code(
                                artifact.id(),
                                NoticeKind::Unsupported,
                                format!(
                                    "Copilot CLI selects {} over {} for server {}; the selected records disagree.",
                                    previous.destination().display(),
                                    artifact.destination().display(),
                                    server.name()
                                ),
                                "copilot-mcp-precedence-collision",
                            )?);
                        }
                    }
                    _ => notices.push(precedence_unverified(artifact, previous)?),
                }
            }
        }
    }
    Ok(notices)
}

fn precedence_unverified(
    artifact: &RenderedArtifact,
    previous: &RenderedArtifact,
) -> Result<Notice, ValueError> {
    Notice::with_code(
        artifact.id(),
        NoticeKind::Activation,
        format!(
            "Copilot CLI gives {} precedence over {}; native content requires caller review of the effective server definitions.",
            previous.destination().display(),
            artifact.destination().display()
        ),
        "copilot-mcp-precedence-unverified",
    )
}

pub fn assemble_translations(
    translations: impl IntoIterator<Item = ArtifactTranslation>,
    dependencies: impl IntoIterator<Item = Dependency>,
    contexts: &[RenderContext],
    routing_context: impl IntoIterator<Item = (String, String)>,
) -> Result<RenderedBundle, ValueError> {
    let mut artifacts = Vec::new();
    let mut notices = Vec::new();
    for translation in translations {
        if let Some(artifact) = translation.artifact {
            artifacts.push(artifact);
        }
        notices.extend(translation.notices);
    }
    notices.extend(collision_notices(&artifacts)?);
    notices.extend(copilot_precedence_notices(&artifacts, contexts)?);
    if notices
        .iter()
        .any(|notice| notice.kind() == NoticeKind::Unsupported)
    {
        RenderedBundle::with_notices([], [], notices, routing_context)
    } else {
        RenderedBundle::with_notices(artifacts, dependencies, notices, routing_context)
    }
}

#[allow(clippy::too_many_lines)]
pub fn render(bundle: &Bundle, context: &RenderContext) -> Result<RenderedBundle, ValueError> {
    let (asset_artifacts, assets) = support_asset_artifacts(bundle, context)?;
    let mut artifacts = Vec::new();
    let mut notices = Vec::new();
    let mut hook_dependencies = Vec::new();
    match hook_artifacts(bundle, context, &assets) {
        Ok(output) => {
            artifacts.extend(output.artifacts().iter().cloned());
            hook_dependencies.extend(output.dependencies().iter().cloned());
            notices.extend(output.notices().iter().cloned());
        }
        Err(HookTranslationError::Unsupported(error)) => {
            for artifact in bundle.artifacts() {
                if let Artifact::Hook(hook) = artifact {
                    notices.push(Notice::with_code(
                        hook.id(),
                        NoticeKind::Unsupported,
                        error.message(),
                        error.code(),
                    )?);
                }
            }
        }
        Err(HookTranslationError::Invalid(error)) => return Err(error),
    }

    let mut selected = HashSet::new();
    for artifact in bundle.artifacts() {
        if let Artifact::Native(native) = artifact
            && translate_native_artifact(native, context)?.is_none()
        {
            continue;
        }
        selected.insert(artifact.id().to_owned());
        let translation = match artifact {
            Artifact::Skill(skill) => ArtifactTranslation {
                artifact: Some(RenderedArtifact::new(
                    skill.id(),
                    Family::Skills,
                    context.skills(),
                    Content::Tree(skill.content().clone()),
                    Some(skill.name().to_owned()),
                    false,
                    Vec::new(),
                )?),
                notices: Vec::new(),
            },
            Artifact::Instruction(instruction) => {
                translate_instruction_artifact(bundle.id(), instruction, context)?
            }
            Artifact::Mcp(mcp) => translate_mcp_artifact(
                mcp,
                context,
                assets.iter().map(|(id, path)| (id.clone(), path.clone())),
            )?,
            Artifact::Hook(_) => continue,
            Artifact::Native(native) => ArtifactTranslation {
                artifact: translate_native_artifact(native, context)?,
                notices: Vec::new(),
            },
        };
        if let Some(output) = translation.artifact {
            artifacts.push(output);
        }
        notices.extend(translation.notices);
    }

    let mut pending = selected.clone();
    while !pending.is_empty() {
        let required: HashSet<String> = bundle
            .dependencies()
            .iter()
            .filter(|edge| pending.contains(edge.dependent()))
            .map(|edge| edge.required().to_owned())
            .collect();
        pending = required.difference(&selected).cloned().collect();
        selected.extend(required);
    }
    artifacts.extend(
        asset_artifacts
            .into_iter()
            .filter(|asset| selected.contains(asset.id())),
    );
    let emitted: HashSet<String> = artifacts
        .iter()
        .map(|artifact| artifact.id().to_owned())
        .collect();
    let mut unavailable: Vec<_> = selected.difference(&emitted).cloned().collect();
    unavailable.sort();
    for name in unavailable {
        if !notices
            .iter()
            .any(|notice| notice.artifact_id() == name && notice.kind() == NoticeKind::Unsupported)
        {
            notices.push(Notice::with_code(
                name,
                NoticeKind::Unsupported,
                "A selected dependency is unavailable for this audience.",
                "dependency-audience",
            )?);
        }
    }
    notices.extend(collision_notices(&artifacts)?);
    notices.extend(copilot_precedence_notices(
        &artifacts,
        std::slice::from_ref(context),
    )?);
    let blocked = notices
        .iter()
        .any(|notice| notice.kind() == NoticeKind::Unsupported);
    let dependencies: Vec<_> = bundle
        .dependencies()
        .iter()
        .filter(|edge| emitted.contains(edge.dependent()))
        .cloned()
        .chain(hook_dependencies)
        .collect();
    RenderedBundle::with_notices(
        if blocked { Vec::new() } else { artifacts },
        if blocked { Vec::new() } else { dependencies },
        notices,
        context.routing_context(),
    )
}

fn route_json(context: &[(String, String)]) -> String {
    let mut route = String::new();
    Json::Array(
        context
            .iter()
            .map(|(key, value)| Json::Array(vec![Json::Str(key.clone()), Json::Str(value.clone())]))
            .collect(),
    )
    .encode_python(&mut route);
    route
}

pub fn render_many(
    bundle: &Bundle,
    contexts: impl IntoIterator<Item = RenderContext>,
) -> Result<RenderedBundle, ValueError> {
    let contexts: Vec<_> = contexts.into_iter().collect();
    if contexts.is_empty() {
        return Err(ValueError::new(
            "render_many requires at least one explicit context",
        ));
    }
    let mut unique = BTreeMap::new();
    for context in &contexts {
        let rendered = render(bundle, context)?;
        unique.insert(route_json(rendered.routing_context()), rendered);
    }
    let mut artifacts = Vec::new();
    let mut dependencies = Vec::new();
    let mut notices = Vec::new();
    let mut routes = Vec::new();
    for (route, rendered) in unique {
        routes.push((identifier(&[&route]), route.clone()));
        let names: HashMap<_, _> = rendered
            .artifacts()
            .iter()
            .map(|artifact| {
                (
                    artifact.id().to_owned(),
                    identifier(&[&route, artifact.id()]),
                )
            })
            .collect();
        for artifact in rendered.artifacts() {
            let id = names
                .get(artifact.id())
                .ok_or_else(|| ValueError::new("missing route artifact identifier"))?;
            artifacts.push(RenderedArtifact::new(
                id.clone(),
                artifact.family(),
                artifact.destination().to_path_buf(),
                artifact.content().clone(),
                artifact.subtree().map(str::to_owned),
                artifact.require_existing(),
                artifact.schema_requirements().to_vec(),
            )?);
        }
        for edge in rendered.dependencies() {
            let dependent = names
                .get(edge.dependent())
                .ok_or_else(|| ValueError::new("missing route dependency identifier"))?;
            let required = names
                .get(edge.required())
                .ok_or_else(|| ValueError::new("missing route dependency identifier"))?;
            dependencies.push(Dependency::with_mode(
                dependent.clone(),
                required.clone(),
                edge.mode(),
            )?);
        }
        for notice in rendered.notices() {
            notices.push(Notice::with_code(
                identifier(&[&route, notice.artifact_id()]),
                notice.kind(),
                notice.message(),
                notice.code(),
            )?);
        }
    }
    notices.extend(collision_notices(&artifacts)?);
    notices.extend(copilot_precedence_notices(&artifacts, &contexts)?);
    let blocked = notices
        .iter()
        .any(|notice| notice.kind() == NoticeKind::Unsupported);
    RenderedBundle::with_notices(
        if blocked { Vec::new() } else { artifacts },
        if blocked { Vec::new() } else { dependencies },
        notices,
        routes,
    )
}
