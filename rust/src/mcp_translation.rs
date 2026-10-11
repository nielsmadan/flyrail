use crate::artifacts::{
    Command, CommandArgument, EnvRef, EnvironmentValue, McpArtifact, McpTransport,
};
use crate::content::{DocumentFormat, Key, Selector, SelectorPart, StructuredContent};
use crate::destinations::RenderContext;
use crate::targets::{Agent, Platform, Surface};
use crate::values::{ArrayValue, ObjectValue, Scalar, Value, ValueError};
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnsupportedTranslation {
    code: &'static str,
    message: String,
}

impl UnsupportedTranslation {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for UnsupportedTranslation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for UnsupportedTranslation {}

fn unsupported<T>(code: &'static str, message: &'static str) -> Result<T, UnsupportedTranslation> {
    Err(UnsupportedTranslation::new(code, message))
}

fn has_copilot_env_substitution(value: &str) -> bool {
    value.as_bytes().windows(2).any(|pair| {
        pair.first() == Some(&b'$')
            && pair
                .get(1)
                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
    })
}

fn literal<'a>(
    value: &'a str,
    context: &RenderContext,
    field: &'static str,
) -> Result<&'a str, UnsupportedTranslation> {
    let agent = context.audience().agent();
    let substitution = match agent {
        Agent::Opencode => value.contains("{env:") || value.contains("{file:"),
        Agent::Pi => value.contains("${") || value.contains("$env:") || value.contains("{env:"),
        Agent::Codex => false,
        _ => value.contains("${"),
    };
    let copilot_cli = agent == Agent::Copilot && context.surface() == Surface::Cli;
    if substitution || (copilot_cli && field == "env" && has_copilot_env_substitution(value)) {
        if copilot_cli && field != "env" {
            return unsupported(
                "literal-interpolation-unverified",
                match field {
                    "argv" => {
                        "Literal argv substitution-like syntax is unverified for Copilot CLI."
                    }
                    "cwd" => "Literal cwd substitution-like syntax is unverified for Copilot CLI.",
                    "url" => "Literal url substitution-like syntax is unverified for Copilot CLI.",
                    "headers" => {
                        "Literal headers substitution-like syntax is unverified for Copilot CLI."
                    }
                    _ => "Literal substitution-like syntax is unverified for Copilot CLI.",
                },
            );
        }
        return unsupported(
            "literal-interpolation",
            match field {
                "argv" => "argv contains host interpolation syntax; use native content.",
                "env" => "env contains host interpolation syntax; use native content.",
                "cwd" => "cwd contains host interpolation syntax; use native content.",
                "url" => "url contains host interpolation syntax; use native content.",
                "headers" => "headers contains host interpolation syntax; use native content.",
                _ => "value contains host interpolation syntax; use native content.",
            },
        );
    }
    Ok(value)
}

fn reference(
    value: &EnvRef,
    context: &RenderContext,
    field: &'static str,
) -> Result<String, UnsupportedTranslation> {
    let agent = context.audience().agent();
    if agent == Agent::Claude
        || (agent == Agent::Copilot && context.surface() == Surface::Cli && field == "env")
    {
        return Ok(format!("${{{}}}", value.name()));
    }
    if agent == Agent::Opencode {
        return Ok(format!("{{env:{}}}", value.name()));
    }
    if agent == Agent::Cursor || context.surface() == Surface::Vscode {
        return Ok(format!("${{env:{}}}", value.name()));
    }
    unsupported(
        "portable-env-reference-unverified",
        "This host cannot represent this environment reference.",
    )
}

fn asset(value: &CommandArgument, assets: &HashMap<String, PathBuf>) -> Result<String, ValueError> {
    match value {
        CommandArgument::Text(text) => Ok(text.clone()),
        CommandArgument::Asset(reference) => {
            let root = assets
                .get(reference.asset_id())
                .ok_or_else(|| ValueError::new("unknown translated asset reference"))?;
            Ok(reference
                .path()
                .map_or_else(|| root.clone(), |path| root.join(path))
                .display()
                .to_string())
        }
    }
}

fn scalar(value: impl Into<String>) -> Value {
    Value::Scalar(Scalar::text(value))
}

fn array(values: impl IntoIterator<Item = Value>) -> Value {
    Value::Array(ArrayValue::new(values))
}

fn object(
    values: impl IntoIterator<Item = (impl Into<String>, Value)>,
) -> Result<Value, ValueError> {
    Ok(Value::Object(ObjectValue::new(values)?))
}

#[allow(clippy::too_many_lines)]
fn stdio(
    command: &Command,
    context: &RenderContext,
    assets: &HashMap<String, PathBuf>,
) -> Result<Value, TranslationFailure> {
    let agent = context.audience().agent();
    if context.platform() == Platform::Windows {
        let names: Vec<String> = command
            .env()
            .iter()
            .map(|(key, _)| key.to_lowercase())
            .collect();
        if names.iter().collect::<HashSet<_>>().len() != names.len() {
            return Err(UnsupportedTranslation::new(
                "windows-env-collision",
                "Environment names collide under Windows case-insensitive lookup.",
            )
            .into());
        }
    }
    let argv = command
        .argv()
        .iter()
        .map(|argument| asset(argument, assets))
        .collect::<Result<Vec<_>, _>>()?;
    for (index, argument) in argv.iter().enumerate() {
        if agent != Agent::Pi || index != 0 {
            literal(argument, context, "argv")?;
        }
    }
    let mut environment = Vec::with_capacity(command.env().len());
    let mut forwarded = Vec::new();
    for (key, value) in command.env() {
        match value {
            EnvironmentValue::Reference(reference_value) if agent == Agent::Codex => {
                if key != reference_value.name() {
                    return Err(UnsupportedTranslation::new(
                        "environment-alias",
                        "Codex env_vars forwards a variable under its own name.",
                    )
                    .into());
                }
                forwarded.push(scalar(key));
            }
            EnvironmentValue::Reference(reference_value) => {
                environment.push((
                    key.clone(),
                    scalar(reference(reference_value, context, "env")?),
                ));
            }
            EnvironmentValue::Text(text) => {
                if agent != Agent::Pi {
                    literal(text, context, "env")?;
                }
                environment.push((key.clone(), scalar(text)));
            }
        }
    }
    let mut record = if agent == Agent::Opencode {
        vec![
            ("type".to_owned(), scalar("local")),
            ("command".to_owned(), array(argv.iter().map(scalar))),
            ("environment".to_owned(), object(environment)?),
        ]
    } else {
        let command_name = argv
            .first()
            .ok_or_else(|| ValueError::new("argv requires a command"))?;
        vec![
            ("command".to_owned(), scalar(command_name)),
            ("args".to_owned(), array(argv.iter().skip(1).map(scalar))),
            ("env".to_owned(), object(environment)?),
        ]
    };
    if agent == Agent::Cursor || context.surface() == Surface::Vscode {
        record.push(("type".to_owned(), scalar("stdio")));
    }
    if agent == Agent::Pi {
        record.push((
            "literalEnv".to_owned(),
            Value::Scalar(Scalar::boolean(true)),
        ));
    }
    if !forwarded.is_empty() {
        record.push(("env_vars".to_owned(), array(forwarded)));
    }
    if let Some(cwd) = command.cwd() {
        if matches!(agent, Agent::Claude | Agent::Cursor) {
            return Err(UnsupportedTranslation::new(
                "command-cwd",
                "The selected MCP schema cannot honor cwd.",
            )
            .into());
        }
        let cwd = match cwd {
            CommandArgument::Asset(_) => asset(cwd, assets)?,
            CommandArgument::Text(relative) => context
                .execution_root()
                .unwrap_or_else(|| context.root())
                .join(relative)
                .display()
                .to_string(),
        };
        literal(&cwd, context, "cwd")?;
        record.push(("cwd".to_owned(), scalar(cwd)));
    }
    object(record).map_err(Into::into)
}

fn http(
    transport: &crate::artifacts::HttpTransport,
    context: &RenderContext,
) -> Result<Value, TranslationFailure> {
    let agent = context.audience().agent();
    literal(transport.url(), context, "url")?;
    let mut result = vec![("url".to_owned(), scalar(transport.url()))];
    if agent == Agent::Opencode {
        result.push(("type".to_owned(), scalar("remote")));
    } else if !matches!(agent, Agent::Codex | Agent::Pi) {
        result.push(("type".to_owned(), scalar("http")));
    }
    if let Some(token) = transport.bearer_token() {
        if agent == Agent::Codex {
            result.push(("bearer_token_env_var".to_owned(), scalar(token.name())));
        } else if agent == Agent::Pi {
            result.push(("auth".to_owned(), scalar("bearer")));
            result.push(("bearerTokenEnv".to_owned(), scalar(token.name())));
        } else if agent == Agent::Copilot && context.surface() == Surface::Cli {
            return Err(UnsupportedTranslation::new(
                "portable-auth-env-unverified",
                "Copilot CLI bearer environment syntax is unverified.",
            )
            .into());
        } else {
            result.push((
                "headers".to_owned(),
                object([(
                    "Authorization",
                    scalar(format!("Bearer {}", reference(token, context, "headers")?)),
                )])?,
            ));
        }
    }
    object(result).map_err(Into::into)
}

#[derive(Debug)]
pub enum TranslationFailure {
    Unsupported(UnsupportedTranslation),
    Invalid(ValueError),
}

impl From<UnsupportedTranslation> for TranslationFailure {
    fn from(value: UnsupportedTranslation) -> Self {
        Self::Unsupported(value)
    }
}

impl From<ValueError> for TranslationFailure {
    fn from(value: ValueError) -> Self {
        Self::Invalid(value)
    }
}

pub fn mcp_content(
    artifact: &McpArtifact,
    context: &RenderContext,
    path: &Path,
    assets: &HashMap<String, PathBuf>,
) -> Result<StructuredContent, TranslationFailure> {
    let agent = context.audience().agent();
    if agent == Agent::Claude
        && matches!(
            artifact.name(),
            "workspace" | "claude-in-chrome" | "computer-use"
        )
    {
        return Err(UnsupportedTranslation::new(
            "reserved-server-name",
            "Claude reserves this MCP server name.",
        )
        .into());
    }
    let value = match artifact.transport() {
        McpTransport::Command(command) => stdio(command, context, assets)?,
        McpTransport::Http(transport) => http(transport, context)?,
    };
    let format = if agent == Agent::Codex {
        DocumentFormat::Toml
    } else if agent == Agent::Opencode
        || context.surface() == Surface::Vscode
        || path
            .extension()
            .is_some_and(|extension| extension == "jsonc")
    {
        DocumentFormat::Jsonc
    } else {
        DocumentFormat::Json
    };
    let key = if agent == Agent::Codex {
        "mcp_servers"
    } else if agent == Agent::Opencode {
        "mcp"
    } else if context.surface() == Surface::Vscode {
        "servers"
    } else {
        "mcpServers"
    };
    StructuredContent::new(
        format,
        Selector::new([
            SelectorPart::Key(Key::new(key)?),
            SelectorPart::Key(Key::new(artifact.name())?),
        ])?,
        value,
        None,
    )
    .map_err(Into::into)
}
