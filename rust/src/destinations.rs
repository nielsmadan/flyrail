use crate::artifacts::{Audience, Family};
use crate::configuration::InstallationTarget;
pub use crate::hooks::{HookRuntime, HookShell};
use crate::mcp_translation::UnsupportedTranslation;
use crate::stores::StateStore;
use crate::targets::{Agent, Platform, Surface, Target, TargetScope, relocated, validate_route};
use crate::values::ValueError;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const IDENTITY_KEYS: [&str; 4] = ["agent", "scope", "surface", "root"];

pub const TRANSLATABLE_AGENTS: [Agent; 6] = [
    Agent::Claude,
    Agent::Codex,
    Agent::Opencode,
    Agent::Pi,
    Agent::Cursor,
    Agent::Copilot,
];

#[must_use]
pub const fn is_translatable(agent: Agent) -> bool {
    matches!(
        agent,
        Agent::Claude | Agent::Codex | Agent::Opencode | Agent::Pi | Agent::Cursor | Agent::Copilot
    )
}

#[must_use]
pub fn untranslatable_message(agent: Agent) -> String {
    format!(
        "Flyrail detects {} but has no verified configuration translation.",
        agent.as_str()
    )
}

pub fn require_translatable(context: &RenderContext) -> Result<(), UnsupportedTranslation> {
    let agent = context.audience().agent();
    if is_translatable(agent) {
        Ok(())
    } else {
        Err(UnsupportedTranslation::new(
            "agent-unsupported",
            untranslatable_message(agent),
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderContext {
    target: Target,
    audience: Audience,
    platform: Platform,
    surface: Surface,
    execution_root: Option<PathBuf>,
    instruction_path: Option<PathBuf>,
    mcp_path: Option<PathBuf>,
    native_root: Option<PathBuf>,
    asset_root: Option<PathBuf>,
    hook_runtime: Option<HookRuntime>,
}

impl RenderContext {
    pub fn new(target: Target, platform: Platform) -> Result<Self, ValueError> {
        Self::with_surface(target, platform, Surface::Cli)
    }

    pub fn with_surface(
        target: Target,
        platform: Platform,
        surface: Surface,
    ) -> Result<Self, ValueError> {
        let agent = target.agent().ok_or_else(|| {
            ValueError::new("render context requires a project or user agent Target")
        })?;
        let audience = Audience::new(agent, target.scope(), surface, Some(platform))?;
        let result = Self {
            target,
            audience,
            platform,
            surface,
            execution_root: None,
            instruction_path: None,
            mcp_path: None,
            native_root: None,
            asset_root: None,
            hook_runtime: None,
        };
        result.validate()
    }

    pub fn with_execution_root(self, value: impl AsRef<Path>) -> Result<Self, ValueError> {
        self.with_path(value.as_ref(), |context, path| {
            context.execution_root = Some(path);
        })
    }

    pub fn with_instruction_path(self, value: impl AsRef<Path>) -> Result<Self, ValueError> {
        self.with_path(value.as_ref(), |context, path| {
            context.instruction_path = Some(path);
        })
    }

    pub fn with_mcp_path(self, value: impl AsRef<Path>) -> Result<Self, ValueError> {
        self.with_path(value.as_ref(), |context, path| {
            context.mcp_path = Some(path);
        })
    }

    pub fn with_native_root(self, value: impl AsRef<Path>) -> Result<Self, ValueError> {
        self.with_path(value.as_ref(), |context, path| {
            context.native_root = Some(path);
        })
    }

    pub fn with_asset_root(self, value: impl AsRef<Path>) -> Result<Self, ValueError> {
        self.with_path(value.as_ref(), |context, path| {
            context.asset_root = Some(path);
        })
    }

    #[must_use]
    pub fn with_hook_runtime(mut self, runtime: HookRuntime) -> Self {
        self.hook_runtime = Some(runtime);
        self
    }

    fn with_path(
        mut self,
        value: &Path,
        assign: impl FnOnce(&mut Self, PathBuf),
    ) -> Result<Self, ValueError> {
        assign(&mut self, validate_route(value)?);
        self.validate()
    }

    fn validate(self) -> Result<Self, ValueError> {
        for path in [
            Some(self.target.root()),
            Some(self.root()),
            self.execution_root.as_deref(),
            self.instruction_path.as_deref(),
            self.mcp_path.as_deref(),
            self.native_root.as_deref(),
            self.asset_root.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            validate_route(path)?;
        }
        Ok(self)
    }

    pub const fn target(&self) -> &Target {
        &self.target
    }

    pub const fn platform(&self) -> Platform {
        self.platform
    }

    pub const fn surface(&self) -> Surface {
        self.surface
    }

    pub fn root(&self) -> &Path {
        self.target.home().unwrap_or_else(|| self.target.anchor())
    }

    pub const fn audience(&self) -> Audience {
        self.audience
    }

    pub fn skills(&self) -> PathBuf {
        if self.surface == Surface::Vscode && self.target.scope() == TargetScope::User {
            self.root().join(".copilot/skills")
        } else {
            self.target.root().to_path_buf()
        }
    }

    pub fn execution_root(&self) -> Option<&Path> {
        self.execution_root.as_deref()
    }

    pub fn instruction_path(&self) -> Option<&Path> {
        self.instruction_path.as_deref()
    }

    pub fn mcp_path(&self) -> Option<&Path> {
        self.mcp_path.as_deref()
    }

    pub fn native_root(&self) -> Option<&Path> {
        self.native_root.as_deref()
    }

    pub fn asset_root(&self) -> Option<&Path> {
        self.asset_root.as_deref()
    }

    pub const fn hook_runtime(&self) -> Option<&HookRuntime> {
        self.hook_runtime.as_ref()
    }

    pub fn routing_context(&self) -> Vec<(String, String)> {
        let empty = String::new;
        let mut context = vec![
            (String::from("agent"), self.audience().agent().to_string()),
            (String::from("scope"), self.target.scope().to_string()),
            (String::from("surface"), self.surface.to_string()),
            (String::from("platform"), self.platform.to_string()),
            (String::from("root"), self.root().display().to_string()),
            (String::from("skills"), self.skills().display().to_string()),
            (
                String::from("execution"),
                self.execution_root
                    .as_deref()
                    .unwrap_or_else(|| self.root())
                    .display()
                    .to_string(),
            ),
            (
                String::from("instructions"),
                self.instruction_path
                    .as_ref()
                    .map_or_else(empty, |path| path.display().to_string()),
            ),
            (
                String::from("mcp"),
                self.mcp_path
                    .as_ref()
                    .map_or_else(empty, |path| path.display().to_string()),
            ),
            (
                String::from("native"),
                self.native_root
                    .as_deref()
                    .unwrap_or_else(|| self.root())
                    .display()
                    .to_string(),
            ),
            (
                String::from("assets"),
                self.asset_root
                    .clone()
                    .unwrap_or_else(|| self.root().join(".flyrail-assets"))
                    .display()
                    .to_string(),
            ),
            (
                String::from("hook-node"),
                self.hook_runtime
                    .as_ref()
                    .and_then(HookRuntime::node)
                    .map_or_else(empty, |path| path.display().to_string()),
            ),
            (
                String::from("hook-shell"),
                self.hook_runtime
                    .as_ref()
                    .and_then(HookRuntime::shell)
                    .map_or_else(empty, |shell| shell.as_str().to_owned()),
            ),
        ];
        context.extend(
            self.target
                .environment()
                .iter()
                .map(|(key, value)| (format!("env:{key}"), value.clone())),
        );
        context.sort();
        context
    }

    pub fn installation(
        &self,
        store: Arc<dyn StateStore>,
        context: Option<Vec<(String, String)>>,
    ) -> Result<InstallationTarget, ValueError> {
        let routing = self.routing_context();
        let context = context.unwrap_or_else(|| {
            routing
                .iter()
                .filter(|(key, _)| IDENTITY_KEYS.contains(&key.as_str()))
                .cloned()
                .collect()
        });
        InstallationTarget::new(store, context, routing)
    }
}

pub(crate) fn user_directory(context: &RenderContext) -> Result<PathBuf, ValueError> {
    let agent = context.audience().agent();
    let (mut variable, suffix) = match agent {
        Agent::Claude => ("CLAUDE_CONFIG_DIR", ".claude"),
        Agent::Codex => ("CODEX_HOME", ".codex"),
        Agent::Opencode => ("XDG_CONFIG_HOME", ".config"),
        Agent::Pi => ("PI_CODING_AGENT_DIR", ".pi/agent"),
        Agent::Cursor => ("", ".cursor"),
        Agent::Copilot => ("COPILOT_HOME", ".copilot"),
        Agent::Droid => {
            return Err(ValueError::new(untranslatable_message(agent)));
        }
    };
    if context.surface == Surface::Vscode {
        variable = "";
    }
    let configured = context
        .target
        .environment()
        .iter()
        .find_map(|(key, value)| (key == variable).then_some(value));
    let mut root = if let Some(value) = configured.filter(|value| !value.is_empty()) {
        relocated(value, context.root())?
    } else {
        context.root().join(suffix)
    };
    if agent == Agent::Opencode {
        root.push("opencode");
    }
    Ok(root)
}

pub fn instruction_destination(context: &RenderContext) -> Result<Option<PathBuf>, ValueError> {
    if let Some(path) = &context.instruction_path {
        return Ok(Some(path.clone()));
    }
    if !is_translatable(context.audience().agent()) {
        return Ok(None);
    }
    let agent = context.audience().agent();
    if context.target.scope() == TargetScope::Project {
        let path = match agent {
            Agent::Claude => "CLAUDE.md",
            Agent::Codex | Agent::Opencode | Agent::Pi | Agent::Cursor => "AGENTS.md",
            Agent::Copilot => ".github/copilot-instructions.md",
            Agent::Droid => return Ok(None),
        };
        return Ok(Some(context.root().join(path)));
    }
    if agent == Agent::Cursor {
        return Ok(None);
    }
    let directory = user_directory(context)?;
    if context.surface == Surface::Vscode {
        return Ok(Some(directory.join("instructions")));
    }
    let path = match agent {
        Agent::Claude => "CLAUDE.md",
        Agent::Codex | Agent::Opencode | Agent::Pi => "AGENTS.md",
        Agent::Copilot => "copilot-instructions.md",
        Agent::Cursor | Agent::Droid => return Ok(None),
    };
    Ok(Some(directory.join(path)))
}

pub fn mcp_destination(context: &RenderContext) -> Result<Option<PathBuf>, ValueError> {
    if let Some(path) = &context.mcp_path {
        return Ok(Some(path.clone()));
    }
    if !is_translatable(context.audience().agent()) {
        return Ok(None);
    }
    let agent = context.audience().agent();
    if context.target.scope() == TargetScope::Project {
        if context.surface == Surface::Vscode {
            return Ok(Some(context.root().join(".vscode/mcp.json")));
        }
        let path = match agent {
            Agent::Claude | Agent::Copilot => ".mcp.json",
            Agent::Codex => ".codex/config.toml",
            Agent::Opencode => "opencode.json",
            Agent::Pi => ".pi/mcp.json",
            Agent::Cursor => ".cursor/mcp.json",
            Agent::Droid => return Ok(None),
        };
        return Ok(Some(context.root().join(path)));
    }
    if context.surface == Surface::Vscode {
        return Ok(None);
    }
    if agent == Agent::Claude {
        let configured = environment_value(context, "CLAUDE_CONFIG_DIR");
        return Ok(configured
            .is_none()
            .then(|| context.root().join(".claude.json")));
    }
    if agent == Agent::Opencode
        && let Some(configured) = environment_value(context, "OPENCODE_CONFIG")
    {
        return Ok(Some(relocated(configured, context.root())?));
    }
    let path = match agent {
        Agent::Codex => "config.toml",
        Agent::Opencode => "opencode.json",
        Agent::Pi | Agent::Cursor => "mcp.json",
        Agent::Copilot => "mcp-config.json",
        Agent::Claude | Agent::Droid => return Ok(None),
    };
    Ok(Some(user_directory(context)?.join(path)))
}

fn environment_value<'a>(context: &'a RenderContext, key: &str) -> Option<&'a str> {
    context
        .target
        .environment()
        .iter()
        .find_map(|(name, value)| (name == key && !value.is_empty()).then_some(value.as_str()))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Capability {
    family: Family,
    portable: bool,
    native: bool,
    limitations: Vec<String>,
    prerequisites: Vec<String>,
}

impl Capability {
    pub fn new(
        family: Family,
        portable: bool,
        native: bool,
        limitations: impl IntoIterator<Item = String>,
        prerequisites: impl IntoIterator<Item = String>,
    ) -> Result<Self, ValueError> {
        let limitations = limitations.into_iter().collect::<Vec<_>>();
        let prerequisites = prerequisites.into_iter().collect::<Vec<_>>();
        if limitations
            .iter()
            .chain(&prerequisites)
            .any(|value| value.trim().is_empty())
        {
            return Err(ValueError::new(
                "capability details must be nonblank strings",
            ));
        }
        Ok(Self {
            family,
            portable,
            native,
            limitations,
            prerequisites,
        })
    }

    pub const fn family(&self) -> Family {
        self.family
    }

    pub const fn portable(&self) -> bool {
        self.portable
    }

    pub const fn native(&self) -> bool {
        self.native
    }

    pub fn limitations(&self) -> &[String] {
        &self.limitations
    }

    pub fn prerequisites(&self) -> &[String] {
        &self.prerequisites
    }
}

pub fn capabilities(context: &RenderContext) -> Result<Vec<Capability>, ValueError> {
    if !is_translatable(context.audience().agent()) {
        return [
            Family::Skills,
            Family::Instructions,
            Family::Mcp,
            Family::Hooks,
        ]
        .into_iter()
        .map(|family| Capability::new(family, false, false, [], []))
        .collect();
    }
    Ok(vec![
        Capability::new(Family::Skills, true, true, [], [])?,
        Capability::new(
            Family::Instructions,
            instruction_destination(context)?.is_some(),
            true,
            [String::from(
                "Discovery, overrides, byte limits and session loading remain host-controlled.",
            )],
            [],
        )?,
        Capability::new(
            Family::Mcp,
            mcp_destination(context)?.is_some(),
            true,
            [String::from(
                "Transport, interpolation, environment references and cwd are checked per request.",
            )],
            if context.audience().agent() == Agent::Pi {
                vec![String::from("pi-mcp-adapter@2.32.1")]
            } else {
                Vec::new()
            },
        )?,
        Capability::new(
            Family::Hooks,
            context.surface != Surface::Vscode && context.hook_runtime.is_some(),
            true,
            [String::from(
                "Event/outcome and native shell support are validated for each request.",
            )],
            [String::from(
                "Node 24+; explicit executable for native hosts. See the hook protocol matrix.",
            )],
        )?,
    ])
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_support::checkout_temp_dir;

    fn project(root: &Path, agent: Agent) -> RenderContext {
        RenderContext::new(
            Target::project(agent, root).expect("target"),
            Platform::Linux,
        )
        .expect("context")
    }

    #[test]
    fn installations_default_to_the_stable_routing_identity() {
        let root = checkout_temp_dir("destinations-installation-identity");
        let store: Arc<dyn StateStore> = Arc::new(crate::stores::MemoryStore::new());
        let context = project(&root, Agent::Codex)
            .with_hook_runtime(HookRuntime::new(None, Some(HookShell::Posix)).expect("runtime"));
        let installation = context
            .installation(Arc::clone(&store), None)
            .expect("installation");
        let root_text = root.display().to_string();
        assert_eq!(
            installation.context(),
            [
                (String::from("agent"), String::from("codex")),
                (String::from("root"), root_text),
                (String::from("scope"), String::from("project")),
                (String::from("surface"), String::from("cli")),
            ]
        );
        assert_eq!(installation.routing_context(), context.routing_context());
        assert_eq!(
            installation.context_digest(),
            project(&root, Agent::Codex)
                .installation(Arc::clone(&store), None)
                .expect("plain installation")
                .context_digest()
        );
        let explicit = context
            .installation(
                store,
                Some(vec![(String::from("app"), String::from("notes"))]),
            )
            .expect("explicit installation");
        assert_eq!(
            explicit.context(),
            [(String::from("app"), String::from("notes"))]
        );
    }

    #[test]
    fn destinations_cover_agent_scope_and_surface_matrix() {
        let root = checkout_temp_dir("destinations");
        let expected = [
            (Agent::Claude, "CLAUDE.md", ".mcp.json"),
            (Agent::Codex, "AGENTS.md", ".codex/config.toml"),
            (Agent::Opencode, "AGENTS.md", "opencode.json"),
            (Agent::Pi, "AGENTS.md", ".pi/mcp.json"),
            (Agent::Cursor, "AGENTS.md", ".cursor/mcp.json"),
            (
                Agent::Copilot,
                ".github/copilot-instructions.md",
                ".mcp.json",
            ),
        ];
        for (agent, instructions, mcp) in expected {
            let context = project(&root, agent);
            assert_eq!(
                instruction_destination(&context).expect("instructions"),
                Some(root.join(instructions))
            );
            assert_eq!(
                mcp_destination(&context).expect("mcp"),
                Some(root.join(mcp))
            );
            assert_eq!(context.routing_context().len(), 13);
        }
    }

    #[test]
    fn relocations_capabilities_and_overrides_match_reference() {
        let root = checkout_temp_dir("destination-relocations");
        let context = RenderContext::new(
            Target::user_with_home_and_environment(
                Agent::Claude,
                &*root,
                [(
                    String::from("CLAUDE_CONFIG_DIR"),
                    String::from("~/selected"),
                )],
            )
            .expect("target"),
            Platform::Linux,
        )
        .expect("context");
        assert_eq!(
            instruction_destination(&context).expect("instructions"),
            Some(root.join("selected/CLAUDE.md"))
        );
        assert_eq!(mcp_destination(&context).expect("mcp"), None);
        assert_eq!(context.routing_context().len(), 14);

        let context = context
            .with_mcp_path(root.join("chosen.json"))
            .expect("override");
        let mcp = capabilities(&context)
            .expect("capabilities")
            .into_iter()
            .find(|item| item.family() == Family::Mcp)
            .expect("mcp");
        assert!(mcp.portable());
    }

    #[test]
    fn invalid_audiences_and_routes_are_rejected() {
        let root = checkout_temp_dir("destination-boundaries");
        assert!(
            RenderContext::new(
                Target::directory(&*root).expect("directory"),
                Platform::Linux
            )
            .is_err()
        );
        assert!(
            RenderContext::with_surface(
                Target::project(Agent::Codex, &*root).expect("target"),
                Platform::Linux,
                Surface::Ide
            )
            .is_err()
        );
        assert!(
            project(&root, Agent::Codex)
                .with_mcp_path(Path::new("relative"))
                .is_err()
        );
        assert!(
            project(&root, Agent::Codex)
                .with_mcp_path(root.join("a/../b"))
                .is_err()
        );
    }

    #[test]
    fn droid_has_no_translation_destinations_or_capabilities() {
        let root = checkout_temp_dir("destination-droid");
        for target in [
            Target::project(Agent::Droid, &*root).expect("project"),
            Target::user_with_home_and_environment(
                Agent::Droid,
                &*root,
                std::iter::empty::<(String, String)>(),
            )
            .expect("user"),
        ] {
            let context = RenderContext::new(target, Platform::Linux).expect("context");
            assert_eq!(
                instruction_destination(&context).expect("instructions"),
                None
            );
            assert_eq!(mcp_destination(&context).expect("mcp"), None);
            assert!(
                capabilities(&context)
                    .expect("capabilities")
                    .iter()
                    .all(|capability| !capability.portable() && !capability.native())
            );
            let overridden = context
                .with_instruction_path(root.join("instructions.md"))
                .expect("instructions")
                .with_mcp_path(root.join("mcp.json"))
                .expect("mcp");
            assert_eq!(
                instruction_destination(&overridden).expect("instructions"),
                Some(root.join("instructions.md"))
            );
            assert_eq!(
                mcp_destination(&overridden).expect("mcp"),
                Some(root.join("mcp.json"))
            );
            assert!(
                capabilities(&overridden)
                    .expect("capabilities")
                    .iter()
                    .all(|capability| !capability.portable() && !capability.native())
            );
        }
    }
}
