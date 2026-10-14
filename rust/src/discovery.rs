use crate::{Agent, ValueError};
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentPresence {
    agent: Agent,
    executable: Option<PathBuf>,
    application: Option<PathBuf>,
}

impl AgentPresence {
    pub fn new(
        agent: Agent,
        executable: Option<PathBuf>,
        application: Option<PathBuf>,
    ) -> Result<Self, ValueError> {
        if executable.is_none() && application.is_none() {
            return Err(ValueError::new(
                "agent presence requires at least one piece of evidence",
            ));
        }
        if executable
            .iter()
            .chain(&application)
            .any(|path| !path.is_absolute())
        {
            return Err(ValueError::new(
                "agent presence evidence must be an absolute Path",
            ));
        }
        Ok(Self {
            agent,
            executable,
            application,
        })
    }

    pub const fn agent(&self) -> Agent {
        self.agent
    }

    pub fn executable(&self) -> Option<&Path> {
        self.executable.as_deref()
    }

    pub fn application(&self) -> Option<&Path> {
        self.application.as_deref()
    }
}

pub fn detect_agents(
    path: Option<&str>,
    application_directories: Option<&[PathBuf]>,
) -> Result<Vec<AgentPresence>, ValueError> {
    let path = path
        .map(str::to_owned)
        .or_else(|| env::var_os("PATH").and_then(|value| value.into_string().ok()))
        .unwrap_or_default();
    let directories = if path.is_empty() {
        Vec::new()
    } else {
        path.split(':')
            .map(|value| {
                if value.is_empty() {
                    PathBuf::from(".")
                } else {
                    PathBuf::from(value)
                }
            })
            .collect::<Vec<_>>()
    };
    let applications =
        application_directories.map_or_else(default_application_directories, <[PathBuf]>::to_vec);
    let mut found = Vec::new();
    for agent in Agent::ALL {
        let executable = find_executable(executable(agent), &directories);
        let application = application_evidence(agent, &applications);
        if executable.is_some() || application.is_some() {
            found.push(AgentPresence::new(agent, executable, application)?);
        }
    }
    Ok(found)
}

const fn executable(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
        Agent::Opencode => "opencode",
        Agent::Pi => "pi",
        Agent::Cursor => "cursor",
        Agent::Copilot => "copilot",
        Agent::Droid => "droid",
    }
}

fn find_executable(name: &str, directories: &[PathBuf]) -> Option<PathBuf> {
    directories.iter().find_map(|directory| {
        let candidate = directory.join(name);
        is_executable(&candidate).then(|| absolute(&candidate))
    })
}

fn absolute(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    path.components().fold(PathBuf::new(), |mut result, part| {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            _ => result.push(part.as_os_str()),
        }
        result
    })
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
        && rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_ok()
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(target_os = "macos")]
fn application_evidence(agent: Agent, directories: &[PathBuf]) -> Option<PathBuf> {
    let name = match agent {
        Agent::Cursor => "Cursor.app",
        Agent::Copilot => "Visual Studio Code.app",
        _ => return None,
    };
    directories
        .iter()
        .map(|directory| directory.join(name))
        .find(|path| path.is_dir())
}

#[cfg(not(target_os = "macos"))]
const fn application_evidence(_agent: Agent, _directories: &[PathBuf]) -> Option<PathBuf> {
    None
}

fn default_application_directories() -> Vec<PathBuf> {
    let mut directories = vec![PathBuf::from("/Applications")];
    #[allow(deprecated)]
    if let Some(home) = env::home_dir() {
        directories.push(home.join("Applications"));
    }
    directories
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_support::checkout_temp_dir;
    use std::fs::{self, File};

    #[test]
    fn finds_executables_in_agent_order_with_absolute_evidence() {
        let root = checkout_temp_dir("agent-discovery");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("bin");
        for name in ["droid", "claude", "cursor"] {
            let path = bin.join(name);
            File::create(&path).expect("executable");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("permissions");
            }
        }
        File::create(bin.join("codex")).expect("non-executable");
        let found = detect_agents(Some(bin.to_str().expect("path")), Some(&[])).expect("detection");
        assert_eq!(
            found.iter().map(AgentPresence::agent).collect::<Vec<_>>(),
            [Agent::Claude, Agent::Cursor, Agent::Droid]
        );
        assert!(
            found
                .iter()
                .filter_map(AgentPresence::executable)
                .all(Path::is_absolute)
        );
    }

    #[test]
    fn rejects_missing_or_relative_evidence() {
        assert!(AgentPresence::new(Agent::Claude, None, None).is_err());
        assert!(AgentPresence::new(Agent::Claude, Some(PathBuf::from("claude")), None).is_err());
    }

    #[test]
    fn normalizes_parent_components_in_executable_evidence() {
        let root = checkout_temp_dir("agent-discovery-normalized");
        let bin = root.join("bin");
        fs::create_dir_all(root.join("nested")).expect("nested");
        fs::create_dir_all(&bin).expect("bin");
        let executable = bin.join("claude");
        File::create(&executable).expect("executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
                .expect("permissions");
        }
        let path = root.join("nested/../bin");
        let found = find_executable("claude", &[path]).expect("discovery");
        assert_eq!(found, executable);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finds_supported_macos_applications_after_executables() {
        let root = checkout_temp_dir("agent-app-discovery");
        fs::create_dir_all(root.join("Cursor.app")).expect("cursor");
        fs::create_dir_all(root.join("Visual Studio Code.app")).expect("vscode");
        let found = detect_agents(Some(""), Some(std::slice::from_ref(&root))).expect("detection");
        assert_eq!(
            found.iter().map(AgentPresence::agent).collect::<Vec<_>>(),
            [Agent::Cursor, Agent::Copilot]
        );
        let mut found = found.iter();
        assert_eq!(
            found.next().expect("cursor").application(),
            Some(root.join("Cursor.app").as_path())
        );
        assert_eq!(
            found.next().expect("copilot").application(),
            Some(root.join("Visual Studio Code.app").as_path())
        );
    }
}
