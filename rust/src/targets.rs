use crate::inspection::SkillTarget;
use crate::rendered::ModelValue;
use crate::values::{Scalar, Value, ValueError};
use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Agent {
    Claude,
    Codex,
    Opencode,
    Pi,
    Cursor,
    Copilot,
    Droid,
}

impl Agent {
    pub const ALL: [Self; 7] = [
        Self::Claude,
        Self::Codex,
        Self::Opencode,
        Self::Pi,
        Self::Cursor,
        Self::Copilot,
        Self::Droid,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Pi => "pi",
            Self::Cursor => "cursor",
            Self::Copilot => "copilot",
            Self::Droid => "droid",
        }
    }
}

impl fmt::Display for Agent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Agent {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "opencode" => Ok(Self::Opencode),
            "pi" => Ok(Self::Pi),
            "cursor" => Ok(Self::Cursor),
            "copilot" => Ok(Self::Copilot),
            "droid" => Ok(Self::Droid),
            _ => Err(ValueError::new(format!("'{value}' is not a valid Agent"))),
        }
    }
}

impl ModelValue for Agent {
    fn model_value(&self) -> Value {
        Value::Scalar(Scalar::text(self.as_str()))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TargetScope {
    Directory,
    User,
    Project,
}

impl TargetScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Directory => "directory",
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

impl fmt::Display for TargetScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for TargetScope {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "directory" => Ok(Self::Directory),
            "user" => Ok(Self::User),
            "project" => Ok(Self::Project),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid TargetScope"
            ))),
        }
    }
}

impl ModelValue for TargetScope {
    fn model_value(&self) -> Value {
        Value::Scalar(Scalar::text(self.as_str()))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Surface {
    Cli,
    Ide,
    Vscode,
}

impl Surface {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Ide => "ide",
            Self::Vscode => "vscode",
        }
    }
}

impl fmt::Display for Surface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Surface {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "cli" => Ok(Self::Cli),
            "ide" => Ok(Self::Ide),
            "vscode" => Ok(Self::Vscode),
            _ => Err(ValueError::new(format!("'{value}' is not a valid Surface"))),
        }
    }
}

impl ModelValue for Surface {
    fn model_value(&self) -> Value {
        Value::Scalar(Scalar::text(self.as_str()))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Platform {
    Macos,
    Linux,
    Windows,
}

impl Platform {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Macos => "macos",
            Self::Linux => "linux",
            Self::Windows => "windows",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Platform {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "macos" => Ok(Self::Macos),
            "linux" => Ok(Self::Linux),
            "windows" => Ok(Self::Windows),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid Platform"
            ))),
        }
    }
}

impl ModelValue for Platform {
    fn model_value(&self) -> Value {
        Value::Scalar(Scalar::text(self.as_str()))
    }
}

const ENVIRONMENT_KEYS: [&str; 9] = [
    "HOME",
    "USERPROFILE",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "XDG_CONFIG_HOME",
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_DIR",
    "PI_CODING_AGENT_DIR",
    "COPILOT_HOME",
];

#[derive(Clone, Debug)]
pub struct Target {
    root: PathBuf,
    agent: Option<Agent>,
    scope: TargetScope,
    anchor: PathBuf,
    suffix: Vec<&'static str>,
    home: Option<PathBuf>,
    environment: Vec<(String, String)>,
}

impl Target {
    pub fn directory(root: impl AsRef<Path>) -> Result<Self, ValueError> {
        Self::create(
            absolute(root.as_ref())?,
            &[],
            None,
            TargetScope::Directory,
            None,
            &[],
        )
    }

    pub fn project(agent: Agent, root: impl AsRef<Path>) -> Result<Self, ValueError> {
        let suffix = project_suffix(agent);
        Self::create(
            absolute(root.as_ref())?,
            suffix,
            Some(agent),
            TargetScope::Project,
            None,
            &[],
        )
    }

    pub fn user(agent: Agent) -> Result<Self, ValueError> {
        let environment = ambient_environment()?;
        let home = match environment.get("HOME") {
            Some(value) if value.is_empty() => PathBuf::from("/"),
            Some(value) => absolute(Path::new(value))?,
            None => account_home()?,
        };
        Self::user_parts(agent, home, &environment)
    }

    pub fn user_with_home(agent: Agent, home: impl AsRef<Path>) -> Result<Self, ValueError> {
        Self::user_parts(agent, absolute(home.as_ref())?, &BTreeMap::new())
    }

    pub fn user_with_environment(
        agent: Agent,
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ValueError> {
        let environment = environment.into_iter().collect::<BTreeMap<_, _>>();
        let home = environment
            .get("HOME")
            .or_else(|| environment.get("USERPROFILE"))
            .ok_or_else(|| {
                ValueError::new("an explicit env requires home, HOME, or USERPROFILE")
            })?;
        Self::user_parts(agent, absolute_home(home)?, &environment)
    }

    pub fn user_with_home_and_environment(
        agent: Agent,
        home: impl AsRef<Path>,
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ValueError> {
        let environment = environment.into_iter().collect::<BTreeMap<_, _>>();
        Self::user_parts(agent, absolute(home.as_ref())?, &environment)
    }

    fn user_parts(
        agent: Agent,
        home: PathBuf,
        environment: &BTreeMap<String, String>,
    ) -> Result<Self, ValueError> {
        let (anchor, suffix) = match relocation(agent)
            .and_then(|(variable, suffix)| environment.get(variable).map(|value| (value, suffix)))
            .filter(|(value, _)| !value.is_empty())
        {
            Some((value, suffix)) => (relocated(value, &home)?, suffix),
            None => (home.clone(), user_suffix(agent)),
        };
        let retained = environment
            .iter()
            .filter(|(key, _)| ENVIRONMENT_KEYS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        Self::create(
            anchor,
            suffix,
            Some(agent),
            TargetScope::User,
            Some(home),
            &retained,
        )
    }

    fn create(
        anchor: PathBuf,
        suffix: &[&'static str],
        agent: Option<Agent>,
        scope: TargetScope,
        home: Option<PathBuf>,
        environment: &[(String, String)],
    ) -> Result<Self, ValueError> {
        let root = suffix
            .iter()
            .fold(anchor.clone(), |path, part| path.join(part));
        if root.parent().is_none() || root.parent() == Some(root.as_path()) {
            return Err(ValueError::new(
                "a filesystem root cannot be a skill container",
            ));
        }
        Ok(Self {
            root,
            agent,
            scope,
            anchor,
            suffix: suffix.to_vec(),
            home,
            environment: environment.to_vec(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub const fn agent(&self) -> Option<Agent> {
        self.agent
    }

    pub const fn scope(&self) -> TargetScope {
        self.scope
    }

    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    pub fn environment(&self) -> &[(String, String)] {
        &self.environment
    }

    pub(crate) fn anchor(&self) -> &Path {
        &self.anchor
    }
}

impl PartialEq for Target {
    fn eq(&self, other: &Self) -> bool {
        (
            &self.root,
            self.agent,
            self.scope,
            &self.anchor,
            &self.suffix,
        ) == (
            &other.root,
            other.agent,
            other.scope,
            &other.anchor,
            &other.suffix,
        )
    }
}

impl Eq for Target {}

impl Hash for Target {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.root.hash(state);
        self.agent.hash(state);
        self.scope.hash(state);
        self.anchor.hash(state);
        self.suffix.hash(state);
    }
}

impl SkillTarget for Target {
    fn root(&self) -> &Path {
        &self.root
    }

    fn routing_context(&self) -> Vec<(String, String)> {
        let mut context = vec![
            (
                String::from("agent"),
                self.agent
                    .map_or_else(String::new, |agent| agent.to_string()),
            ),
            (String::from("scope"), self.scope.to_string()),
            (
                String::from("home"),
                self.home
                    .as_ref()
                    .map_or_else(String::new, |path| path.display().to_string()),
            ),
        ];
        context.extend(
            self.environment
                .iter()
                .map(|(key, value)| (format!("env:{key}"), value.clone())),
        );
        context
    }

    fn agent(&self) -> Option<Agent> {
        self.agent
    }
}

fn absolute(path: &Path) -> Result<PathBuf, ValueError> {
    let value = path
        .to_str()
        .ok_or_else(|| ValueError::new("target roots must be string paths"))?;
    if value.is_empty() || value.contains('\0') {
        return Err(ValueError::new(
            "target roots must be nonempty paths without NUL",
        ));
    }
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir()
            .map(|current| current.join(path))
            .map_err(|error| ValueError::new(error.to_string()))
    }
}

fn absolute_home(value: &str) -> Result<PathBuf, ValueError> {
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(ValueError::new("environment home must be absolute"));
    }
    absolute(path)
}

pub(crate) fn relocated(value: &str, home: &Path) -> Result<PathBuf, ValueError> {
    if value == "~" {
        return Ok(home.to_path_buf());
    }
    if let Some(suffix) = value.strip_prefix("~/") {
        return absolute(&home.join(suffix));
    }
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(ValueError::new(
            "environment paths must be absolute or start with ~/",
        ));
    }
    absolute(path)
}

pub(crate) fn validate_route(path: &Path) -> Result<PathBuf, ValueError> {
    if !path.is_absolute() {
        return Err(ValueError::new("render routes must be absolute Paths"));
    }
    let Some(value) = path.to_str() else {
        return Err(ValueError::new(
            "render routes must be unambiguous paths without NUL",
        ));
    };
    if value.contains('\0') || path.components().any(|part| part == Component::ParentDir) {
        return Err(ValueError::new(
            "render routes must be unambiguous paths without NUL",
        ));
    }
    Ok(path.to_path_buf())
}

fn ambient_environment() -> Result<BTreeMap<String, String>, ValueError> {
    env::vars_os()
        .filter_map(|(key, value)| {
            let key = key.into_string().ok()?;
            ENVIRONMENT_KEYS
                .contains(&key.as_str())
                .then_some((key, value))
        })
        .map(|(key, value)| {
            value
                .into_string()
                .map(|value| (key, value))
                .map_err(|_| ValueError::new("env keys and values must be strings"))
        })
        .collect()
}

#[cfg(unix)]
#[allow(deprecated)]
fn account_home() -> Result<PathBuf, ValueError> {
    let home = env::home_dir()
        .ok_or_else(|| ValueError::new("current account has no home directory entry"))?;
    if !home.is_absolute() {
        return Err(ValueError::new("environment home must be absolute"));
    }
    absolute(&home)
}

#[cfg(not(unix))]
fn account_home() -> Result<PathBuf, ValueError> {
    Err(ValueError::new(
        "current account home directory lookup is unsupported",
    ))
}

const fn project_suffix(agent: Agent) -> &'static [&'static str] {
    match agent {
        Agent::Claude => &[".claude", "skills"],
        Agent::Codex => &[".agents", "skills"],
        Agent::Opencode => &[".opencode", "skills"],
        Agent::Pi => &[".pi", "skills"],
        Agent::Cursor => &[".cursor", "skills"],
        Agent::Copilot => &[".github", "skills"],
        Agent::Droid => &[".factory", "skills"],
    }
}

const fn user_suffix(agent: Agent) -> &'static [&'static str] {
    match agent {
        Agent::Claude => &[".claude", "skills"],
        Agent::Codex => &[".agents", "skills"],
        Agent::Opencode => &[".config", "opencode", "skills"],
        Agent::Pi => &[".pi", "agent", "skills"],
        Agent::Cursor => &[".cursor", "skills"],
        Agent::Copilot => &[".copilot", "skills"],
        Agent::Droid => &[".factory", "skills"],
    }
}

const fn relocation(agent: Agent) -> Option<(&'static str, &'static [&'static str])> {
    match agent {
        Agent::Claude => Some(("CLAUDE_CONFIG_DIR", &["skills"])),
        Agent::Opencode => Some(("XDG_CONFIG_HOME", &["opencode", "skills"])),
        Agent::Pi => Some(("PI_CODING_AGENT_DIR", &["skills"])),
        Agent::Copilot => Some(("COPILOT_HOME", &["skills"])),
        Agent::Codex | Agent::Cursor | Agent::Droid => None,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_support::checkout_temp_dir;
    #[cfg(unix)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;
    #[cfg(unix)]
    use std::process::Command;

    #[test]
    fn presets_relocations_and_identity_match_reference() {
        let home = checkout_temp_dir("targets");
        let expected = [
            (Agent::Claude, ".claude/skills", ".claude/skills"),
            (Agent::Codex, ".agents/skills", ".agents/skills"),
            (
                Agent::Opencode,
                ".config/opencode/skills",
                ".opencode/skills",
            ),
            (Agent::Pi, ".pi/agent/skills", ".pi/skills"),
            (Agent::Cursor, ".cursor/skills", ".cursor/skills"),
            (Agent::Copilot, ".copilot/skills", ".github/skills"),
        ];
        for (agent, user, project) in expected {
            assert_eq!(
                Target::user_with_home(agent, &*home).expect("user").root(),
                home.join(user)
            );
            assert_eq!(
                Target::project(agent, &*home).expect("project").root(),
                home.join(project)
            );
        }

        let relocated = Target::user_with_home_and_environment(
            Agent::Claude,
            &*home,
            [(String::from("CLAUDE_CONFIG_DIR"), String::from("~/chosen"))],
        )
        .expect("relocated");
        assert_eq!(relocated.root(), home.join("chosen/skills"));
        assert_eq!(
            relocated,
            Target::user_with_home_and_environment(
                Agent::Claude,
                &*home,
                [(
                    String::from("CLAUDE_CONFIG_DIR"),
                    home.join("chosen").display().to_string()
                )],
            )
            .expect("equivalent")
        );
    }

    #[test]
    fn target_boundaries_are_checked_without_ambient_state() {
        assert!(Target::directory("").is_err());
        assert!(Target::directory("a\0b").is_err());
        assert!(Target::directory(Path::new("/")).is_err());
        assert!(
            Target::user_with_environment(Agent::Codex, std::iter::empty::<(String, String)>())
                .is_err()
        );
        assert!(
            Target::user_with_environment(
                Agent::Codex,
                [(String::from("HOME"), String::from("relative"))]
            )
            .is_err()
        );
    }

    #[test]
    fn route_validation_matches_reference_messages() {
        let relative = validate_route(Path::new("relative")).expect_err("relative route");
        assert_eq!(relative.to_string(), "render routes must be absolute Paths");

        let traversal = validate_route(Path::new("/tmp/a/../b")).expect_err("ambiguous route");
        assert_eq!(
            traversal.to_string(),
            "render routes must be unambiguous paths without NUL"
        );

        let nul = validate_route(Path::new("/tmp/a\0b")).expect_err("NUL route");
        assert_eq!(
            nul.to_string(),
            "render routes must be unambiguous paths without NUL"
        );
    }

    #[cfg(unix)]
    #[test]
    fn ambient_user_ignores_unrelated_non_unicode_environment() {
        if env::var_os("FLYRAIL_AMBIENT_TEST").is_some() {
            let target = Target::user(Agent::Codex).expect("ambient target");
            assert!(target.root().ends_with(".agents/skills"));
            return;
        }

        let home = account_home().expect("account home");
        let output = Command::new(env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "targets::tests::ambient_user_ignores_unrelated_non_unicode_environment",
            ])
            .env_clear()
            .env("FLYRAIL_AMBIENT_TEST", "child")
            .env("HOME", home)
            .env(
                "FLYRAIL_UNRELATED",
                OsString::from_vec(vec![b'f', b'o', 0x80]),
            )
            .output()
            .expect("child test");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn ambient_user_rejects_retained_non_unicode_environment_without_panicking() {
        if env::var_os("FLYRAIL_AMBIENT_TEST").is_some() {
            let error = Target::user(Agent::Claude).expect_err("non-Unicode retained value");
            assert_eq!(error.to_string(), "env keys and values must be strings");
            return;
        }

        let output = Command::new(env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "targets::tests::ambient_user_rejects_retained_non_unicode_environment_without_panicking",
            ])
            .env_clear()
            .env("FLYRAIL_AMBIENT_TEST", "child")
            .env(
                "HOME",
                OsString::from_vec(vec![b'/', b't', b'm', b'p', 0x80]),
            )
            .output()
            .expect("child test");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn ambient_user_falls_back_to_current_account_home() {
        if env::var_os("FLYRAIL_AMBIENT_TEST").is_some() {
            let expected = account_home().expect("account home");
            let target = Target::user(Agent::Codex).expect("ambient target");
            assert_eq!(target.home(), Some(expected.as_path()));
            let explicit = Target::user_with_environment(
                Agent::Codex,
                [(
                    String::from("USERPROFILE"),
                    String::from("/tmp/flyrail-misleading-userprofile"),
                )],
            )
            .expect("explicit target");
            assert_eq!(
                explicit.home(),
                Some(Path::new("/tmp/flyrail-misleading-userprofile"))
            );
            return;
        }

        let output = Command::new(env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "targets::tests::ambient_user_falls_back_to_current_account_home",
            ])
            .env_clear()
            .env("FLYRAIL_AMBIENT_TEST", "child")
            .env("USERPROFILE", "/tmp/flyrail-misleading-userprofile")
            .output()
            .expect("child test");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn ambient_user_treats_an_empty_home_as_the_filesystem_root() {
        if env::var_os("FLYRAIL_EMPTY_HOME_TEST").is_some() {
            let target = Target::user(Agent::Codex).expect("ambient target");
            assert_eq!(target.home(), Some(Path::new("/")));
            assert_eq!(target.root(), Path::new("/.agents/skills"));
            return;
        }

        let output = Command::new(env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "targets::tests::ambient_user_treats_an_empty_home_as_the_filesystem_root",
            ])
            .env_clear()
            .env("FLYRAIL_EMPTY_HOME_TEST", "child")
            .env("HOME", "")
            .output()
            .expect("child test");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
