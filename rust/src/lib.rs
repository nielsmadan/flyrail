pub mod artifacts;
pub mod authority;
pub mod bundle;
mod codec;
mod config_manifest;
pub mod configuration;
pub mod content;
pub mod destinations;
pub mod discovery;
pub mod editors;
mod filesystem;
mod hook_translation;
pub mod hooks;
pub mod inspection;
mod inventory;
mod json;
mod json_document;
pub mod lifecycle;
mod lifecycle_impl;
mod manifest;
mod mcp_translation;
mod memory;
pub mod models;
mod observation;
pub mod observations;
pub mod rendered;
pub(crate) mod resource_io;
mod resource_models;
pub(crate) mod resource_plan;
pub(crate) mod resource_transaction;
mod schema;
mod security;
mod sources;
pub mod summary;
pub mod targets;
mod temporal;
mod toml_document;
mod toml_layout;
pub mod translation;
mod validation;
pub mod values;
mod zip;

#[cfg(test)]
mod test_support;

pub use artifacts::{
    Artifact, AssetRef, Audience, Command, CommandArgument, EnvRef, EnvironmentValue, Family,
    HookArtifact, HookEvent, HookOutcome, HttpTransport, InstructionArtifact, McpArtifact,
    McpTransport, NativeArtifact, SkillArtifact, SupportAsset,
};
pub use authority::{AuthorityError, Claim, ClaimSelector, Ownership, ResourceAuthority};
pub use bundle::{Bundle, BundleError};
pub use codec::{decode, encode};
pub use configuration::{
    ClaimObservation, InstallationObservation, InstallationResult, InstallationTarget,
    LifecyclePreview, ResourceObservation, ResourcePlan, ResourceResult,
};
pub use content::{
    Content, DocumentFormat, DocumentSchema, FileContent, FileMode, Key, Member, Placement,
    Position, SectionBoundaries, SectionContent, Selector, SelectorPart, StructuredContent,
    TreeContent,
};
pub use destinations::{
    Capability, RenderContext, TRANSLATABLE_AGENTS, capabilities, instruction_destination,
    is_translatable, mcp_destination, require_translatable, untranslatable_message,
};
pub use discovery::{AgentPresence, detect_agents};
pub use editors::{
    Acquisition, ArrayPosition, CreatedContainer, Disposition, DocumentEdit, DocumentProvenance,
    DocumentRequest, EditObservation, EditStatus, OwnedSelection, SelectionSnapshot, SharedContent,
    SnapshotValue, content_claim, edit_document,
};
pub use hooks::{
    HookConfig, HookEffects, HookEnvironment, HookFailure, HookHandler, HookJson, HookRequest,
    HookResponse, HookRuntime, HookShell, HookText,
};
pub use inspection::{SkillTarget, inspect, render_skills};
pub use inventory::InventoryEntry;
pub use lifecycle::{
    SkillOptions, apply_preview, inspect_installation, install, install_with_timeout, preview,
    preview_removal, recover_installation, remove, sync, uninstall, uninstall_with_options, update,
    update_with_options,
};
pub use mcp_translation::UnsupportedTranslation;
pub use models::{BundleEntry, BundleIdentity, OperationStatus, SkillSpec};
pub use observations::{
    Conflict, ErrorCode, Installation, Modification, ModificationKind, Observation,
    ObservationState, TargetError, TargetInspection, TargetResult,
};
pub use rendered::{
    Dependency, DependencyMode, ModelValue, Notice, NoticeKind, RenderedArtifact, RenderedBundle,
    dependency_order,
};
#[doc(hidden)]
pub use resource_io::{admit, reserve};
pub use resource_models::{
    Ancestor, Count, Generation, Index, Node, OwnedClaim, Receipt, ResourceKind, ResourceRef,
    Revision,
};
pub use resource_transaction::Journal;
#[cfg(feature = "interop-test-seams")]
#[doc(hidden)]
pub use resource_transaction::{
    InteropResourceLockGuard, interop_resource_lock, set_interop_test_barrier,
    set_interop_test_fault,
};
#[doc(hidden)]
pub use security::security as security_capture;
pub use summary::{ChangeAction, PlanSummary, PlannedChange, summarize};
pub use targets::{Agent, Platform, Surface, Target, TargetScope};
pub use temporal::{Date, DateTime, Time};
pub use translation::{
    ArtifactTranslation, assemble_translations, render, render_many,
    translate_instruction_artifact, translate_mcp_artifact,
};
pub use values::{ArrayValue, ObjectValue, Scalar, Value, ValueError};
