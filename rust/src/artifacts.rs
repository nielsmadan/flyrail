use crate::content::{Content, TreeContent};
use crate::rendered::{ModelValue, typed_record};
use crate::targets::{Agent, Platform, Surface, TargetScope};
use crate::validation::{is_python_space, validate_identifier, validate_relative_path};
use crate::values::{ArrayValue, Scalar, Value, ValueError};
use std::collections::HashSet;
use std::str::FromStr;
use unicode_normalization::UnicodeNormalization as _;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Family {
    Skills,
    Instructions,
    Mcp,
    Hooks,
}

impl Family {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Skills => "skills",
            Self::Instructions => "instructions",
            Self::Mcp => "mcp",
            Self::Hooks => "hooks",
        }
    }
}

impl FromStr for Family {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "skills" => Ok(Self::Skills),
            "instructions" => Ok(Self::Instructions),
            "mcp" => Ok(Self::Mcp),
            "hooks" => Ok(Self::Hooks),
            _ => Err(ValueError::new(format!("'{value}' is not a valid Family"))),
        }
    }
}

impl ModelValue for Family {
    fn model_value(&self) -> Value {
        Value::Scalar(Scalar::text(self.as_str()))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Audience {
    agent: Agent,
    scope: TargetScope,
    surface: Surface,
    platform: Option<Platform>,
}

impl Audience {
    pub fn new(
        agent: Agent,
        scope: TargetScope,
        surface: Surface,
        platform: Option<Platform>,
    ) -> Result<Self, ValueError> {
        if scope == TargetScope::Directory {
            return Err(ValueError::new(
                "native audiences require project or user scope",
            ));
        }
        if surface == Surface::Ide && agent != Agent::Cursor {
            return Err(ValueError::new("IDE surface is only defined for Cursor"));
        }
        if surface == Surface::Vscode && agent != Agent::Copilot {
            return Err(ValueError::new(
                "VSCode surface is only defined for Copilot",
            ));
        }
        Ok(Self {
            agent,
            scope,
            surface,
            platform,
        })
    }

    pub const fn agent(&self) -> Agent {
        self.agent
    }

    pub const fn scope(&self) -> TargetScope {
        self.scope
    }

    pub const fn surface(&self) -> Surface {
        self.surface
    }

    pub const fn platform(&self) -> Option<Platform> {
        self.platform
    }
}

impl ModelValue for Audience {
    fn model_value(&self) -> Value {
        typed_record(
            "Audience",
            [
                ("agent", self.agent.model_value()),
                ("platform", self.platform.as_ref().model_value()),
                ("scope", self.scope.model_value()),
                ("surface", self.surface.model_value()),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EnvRef {
    name: String,
}

impl EnvRef {
    pub fn new(name: impl Into<String>) -> Result<Self, ValueError> {
        let name = name.into();
        let mut characters = name.chars();
        let valid = matches!(characters.next(), Some('A'..='Z' | 'a'..='z' | '_'))
            && characters
                .all(|character| matches!(character, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_'));
        if !valid {
            return Err(ValueError::new("invalid environment reference"));
        }
        Ok(Self { name })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl ModelValue for EnvRef {
    fn model_value(&self) -> Value {
        typed_record(
            "EnvRef",
            [("name", Value::Scalar(Scalar::text(self.name.as_str())))],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AssetRef {
    asset_id: String,
    path: Option<String>,
}

impl AssetRef {
    pub fn new(asset_id: impl Into<String>) -> Result<Self, ValueError> {
        let asset_id = asset_id.into();
        validate_identifier(&asset_id, "referenced asset id")?;
        Ok(Self {
            asset_id,
            path: None,
        })
    }

    pub fn with_path(
        asset_id: impl Into<String>,
        path: impl Into<String>,
    ) -> Result<Self, ValueError> {
        let reference = Self::new(asset_id)?;
        let path = path.into();
        validate_relative_path(&path, "asset reference path")?;
        Ok(Self {
            asset_id: reference.asset_id,
            path: Some(path),
        })
    }

    pub fn asset_id(&self) -> &str {
        &self.asset_id
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
}

impl ModelValue for AssetRef {
    fn model_value(&self) -> Value {
        typed_record(
            "AssetRef",
            [
                (
                    "asset_id",
                    Value::Scalar(Scalar::text(self.asset_id.as_str())),
                ),
                ("path", self.path.as_ref().model_value()),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum CommandArgument {
    Text(String),
    Asset(AssetRef),
}

impl From<String> for CommandArgument {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for CommandArgument {
    fn from(value: &str) -> Self {
        Self::Text(String::from(value))
    }
}

impl From<AssetRef> for CommandArgument {
    fn from(value: AssetRef) -> Self {
        Self::Asset(value)
    }
}

impl ModelValue for CommandArgument {
    fn model_value(&self) -> Value {
        match self {
            Self::Text(text) => Value::Scalar(Scalar::text(text.as_str())),
            Self::Asset(reference) => reference.model_value(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum EnvironmentValue {
    Text(String),
    Reference(EnvRef),
}

impl From<String> for EnvironmentValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for EnvironmentValue {
    fn from(value: &str) -> Self {
        Self::Text(String::from(value))
    }
}

impl From<EnvRef> for EnvironmentValue {
    fn from(value: EnvRef) -> Self {
        Self::Reference(value)
    }
}

impl ModelValue for EnvironmentValue {
    fn model_value(&self) -> Value {
        match self {
            Self::Text(text) => Value::Scalar(Scalar::text(text.as_str())),
            Self::Reference(reference) => reference.model_value(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Command {
    argv: Vec<CommandArgument>,
    env: Vec<(String, EnvironmentValue)>,
    cwd: Option<CommandArgument>,
}

impl Command {
    pub fn new<A, K, V>(
        argv: impl IntoIterator<Item = A>,
        env: impl IntoIterator<Item = (K, V)>,
        cwd: Option<CommandArgument>,
    ) -> Result<Self, ValueError>
    where
        A: Into<CommandArgument>,
        K: Into<String>,
        V: Into<EnvironmentValue>,
    {
        let argv: Vec<CommandArgument> = argv.into_iter().map(Into::into).collect();
        let mut env: Vec<(String, EnvironmentValue)> = env
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        if argv.is_empty() {
            return Err(ValueError::new("argv requires a command"));
        }
        for argument in &argv {
            if let CommandArgument::Text(text) = argument
                && text.contains('\0')
            {
                return Err(ValueError::new("command arguments cannot contain NUL"));
            }
        }
        if let Some(CommandArgument::Text(text)) = argv.first()
            && text.is_empty()
        {
            return Err(ValueError::new("command cannot be empty"));
        }
        let mut seen: HashSet<String> = HashSet::with_capacity(env.len());
        for (key, value) in &env {
            EnvRef::new(key.as_str())?;
            if !seen.insert(key.clone()) {
                return Err(ValueError::new("duplicate environment name"));
            }
            if let EnvironmentValue::Text(text) = value
                && text.contains('\0')
            {
                return Err(ValueError::new("environment values cannot contain NUL"));
            }
        }
        if let Some(CommandArgument::Text(text)) = &cwd {
            validate_relative_path(text, "command cwd")?;
        }
        env.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
        Ok(Self { argv, env, cwd })
    }

    pub fn argv(&self) -> &[CommandArgument] {
        &self.argv
    }

    pub fn env(&self) -> &[(String, EnvironmentValue)] {
        &self.env
    }

    pub const fn cwd(&self) -> Option<&CommandArgument> {
        self.cwd.as_ref()
    }
}

impl ModelValue for Command {
    fn model_value(&self) -> Value {
        typed_record(
            "Command",
            [
                (
                    "argv",
                    Value::Array(ArrayValue::new(
                        self.argv.iter().map(ModelValue::model_value),
                    )),
                ),
                ("cwd", self.cwd.as_ref().model_value()),
                (
                    "env",
                    Value::Array(ArrayValue::new(self.env.iter().map(|(key, value)| {
                        Value::Array(ArrayValue::new([
                            Value::Scalar(Scalar::text(key.as_str())),
                            value.model_value(),
                        ]))
                    }))),
                ),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HttpTransport {
    url: String,
    bearer_token: Option<EnvRef>,
}

impl HttpTransport {
    pub fn new(url: impl Into<String>, bearer_token: Option<EnvRef>) -> Result<Self, ValueError> {
        let url = url.into();
        let absolute = || ValueError::new("MCP URL must be an absolute HTTP or HTTPS URL");
        let Some((scheme, rest)) = url.split_once(':') else {
            return Err(absolute());
        };
        let lowered = scheme.to_ascii_lowercase();
        if lowered != "http" && lowered != "https" {
            return Err(absolute());
        }
        let Some(authority) = rest.strip_prefix("//") else {
            return Err(absolute());
        };
        let mut segments = authority.splitn(2, ['/', '?', '#']);
        let netloc = segments.next().unwrap_or("");
        validate_authority(netloc)?;
        validate_netloc_normalization(netloc)?;
        let (hostinfo, credentials) = match netloc.rsplit_once('@') {
            Some((userinfo, host)) => (host, Some(userinfo)),
            None => (netloc, None),
        };
        let (host, port) = host_and_port(hostinfo)?;
        if host.is_empty() {
            return Err(absolute());
        }
        if hostinfo.starts_with('[') {
            validate_bracketed_host(host)?;
        }
        if credentials.is_some()
            || url
                .split_once('#')
                .is_some_and(|(_, fragment)| !fragment.is_empty())
        {
            return Err(ValueError::new(
                "MCP URL cannot embed credentials or a fragment",
            ));
        }
        if url
            .chars()
            .any(|character| is_python_space(character) || u32::from(character) < 32)
        {
            return Err(ValueError::new(
                "MCP URL cannot contain whitespace or control characters",
            ));
        }
        if !port.is_empty() {
            if !port.chars().all(|character| character.is_ascii_digit()) {
                return Err(ValueError::new("MCP URL port must be 1-65535"));
            }
            let number = port
                .parse::<u32>()
                .map_err(|_| ValueError::new("MCP URL port must be 1-65535"))?;
            if !(1..=65_535).contains(&number) {
                return Err(ValueError::new("MCP URL port must be 1-65535"));
            }
        }
        Ok(Self { url, bearer_token })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub const fn bearer_token(&self) -> Option<&EnvRef> {
        self.bearer_token.as_ref()
    }
}

fn validate_authority(netloc: &str) -> Result<(), ValueError> {
    let hostinfo = netloc.rsplit_once('@').map_or(netloc, |(_, host)| host);
    if hostinfo.contains(['[', ']']) {
        let Some(bracketed) = hostinfo.strip_prefix('[') else {
            return Err(ValueError::new("Invalid IPv6 URL"));
        };
        let Some((_, suffix)) = bracketed.split_once(']') else {
            return Err(ValueError::new("Invalid IPv6 URL"));
        };
        if suffix.contains(['[', ']']) || (!suffix.is_empty() && !suffix.starts_with(':')) {
            return Err(ValueError::new("Invalid IPv6 URL"));
        }
    }
    Ok(())
}

fn validate_netloc_normalization(netloc: &str) -> Result<(), ValueError> {
    if netloc.is_ascii() {
        return Ok(());
    }
    let stripped: String = netloc
        .chars()
        .filter(|character| !matches!(character, '@' | ':' | '#' | '?'))
        .collect();
    let normalized: String = stripped.nfkc().collect();
    if stripped != normalized
        && normalized
            .chars()
            .any(|character| matches!(character, '/' | '?' | '#' | '@' | ':'))
    {
        return Err(ValueError::new(format!(
            "netloc '{netloc}' contains invalid characters under NFKC normalization"
        )));
    }
    Ok(())
}

fn host_and_port(hostinfo: &str) -> Result<(&str, &str), ValueError> {
    if let Some(bracketed) = hostinfo.strip_prefix('[') {
        let Some((host, suffix)) = bracketed.split_once(']') else {
            return Err(ValueError::new("Invalid IPv6 URL"));
        };
        return Ok((host, suffix.strip_prefix(':').unwrap_or("")));
    }
    Ok(match hostinfo.split_once(':') {
        Some((host, port)) => (host, port),
        None => (hostinfo, ""),
    })
}

fn validate_bracketed_host(host: &str) -> Result<(), ValueError> {
    if host.is_empty() || host.contains(['[', ']']) {
        return Err(ValueError::new("Invalid IPv6 URL"));
    }
    if let Some(future) = host.strip_prefix('v') {
        let Some((version, address)) = future.split_once('.') else {
            return Err(ValueError::new("IPvFuture address is invalid"));
        };
        if version.is_empty()
            || !version.bytes().all(|byte| byte.is_ascii_hexdigit())
            || address.is_empty()
        {
            return Err(ValueError::new("IPvFuture address is invalid"));
        }
        return Ok(());
    }
    validate_ipv6_address(host)
}

fn validate_ipv6_address(value: &str) -> Result<(), ValueError> {
    let address = value.split_once('%').map_or(
        value,
        |(address, zone)| {
            if zone.is_empty() { "" } else { address }
        },
    );
    if address.is_empty()
        || value
            .split_once('%')
            .is_some_and(|(_, zone)| zone.is_empty())
    {
        return Err(ValueError::new(format!(
            "'{value}' does not appear to be an IPv4 or IPv6 address"
        )));
    }
    let compressed = address.matches("::").count();
    if compressed > 1 {
        return Err(ValueError::new(format!(
            "'{value}' does not appear to be an IPv4 or IPv6 address"
        )));
    }
    let groups = if let Some((left, right)) = address.split_once("::") {
        let mut groups = count_ipv6_groups(left)?;
        groups += count_ipv6_groups(right)?;
        if groups >= 8 {
            return Err(ValueError::new(format!(
                "'{value}' does not appear to be an IPv4 or IPv6 address"
            )));
        }
        groups
    } else {
        let groups = count_ipv6_groups(address)?;
        if groups != 8 {
            return Err(ValueError::new(format!(
                "'{value}' does not appear to be an IPv4 or IPv6 address"
            )));
        }
        groups
    };
    if compressed == 0 && groups == 0 {
        return Err(ValueError::new(format!(
            "'{value}' does not appear to be an IPv4 or IPv6 address"
        )));
    }
    Ok(())
}

fn count_ipv6_groups(value: &str) -> Result<usize, ValueError> {
    if value.is_empty() {
        return Ok(0);
    }
    let parts: Vec<&str> = value.split(':').collect();
    let mut count = 0;
    for (index, part) in parts.iter().enumerate() {
        if part.contains('.') {
            if index + 1 != parts.len() || !valid_ipv4_tail(part) {
                return Err(ValueError::new(
                    "bracketed host is not a valid IPv6 address",
                ));
            }
            count += 2;
        } else if (1..=4).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            count += 1;
        } else {
            return Err(ValueError::new(
                "bracketed host is not a valid IPv6 address",
            ));
        }
    }
    Ok(count)
}

fn valid_ipv4_tail(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|part| {
            !part.is_empty()
                && (part.len() == 1 || !part.starts_with('0'))
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && part.parse::<u16>().is_ok_and(|number| number <= 255)
        })
}

impl ModelValue for HttpTransport {
    fn model_value(&self) -> Value {
        typed_record(
            "HttpTransport",
            [
                ("bearer_token", self.bearer_token.as_ref().model_value()),
                ("url", Value::Scalar(Scalar::text(self.url.as_str()))),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SkillArtifact {
    id: String,
    name: String,
    content: TreeContent,
}

impl SkillArtifact {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        content: TreeContent,
    ) -> Result<Self, ValueError> {
        let id = id.into();
        let name = name.into();
        validate_identifier(&id, "artifact id")?;
        validate_identifier(&name, "skill name")?;
        if !content
            .entries()
            .iter()
            .any(|entry| entry.path() == "SKILL.md" && entry.data().is_some())
        {
            return Err(ValueError::new("skill must contain a SKILL.md file"));
        }
        Ok(Self { id, name, content })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn content(&self) -> &TreeContent {
        &self.content
    }
}

impl ModelValue for SkillArtifact {
    fn model_value(&self) -> Value {
        typed_record(
            "SkillArtifact",
            [
                ("content", self.content.model_value()),
                ("id", Value::Scalar(Scalar::text(self.id.as_str()))),
                ("name", Value::Scalar(Scalar::text(self.name.as_str()))),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InstructionArtifact {
    id: String,
    text: String,
}

impl InstructionArtifact {
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Result<Self, ValueError> {
        let id = id.into();
        let text = text.into();
        validate_identifier(&id, "artifact id")?;
        if text.contains('\0') {
            return Err(ValueError::new("instruction text cannot contain NUL"));
        }
        Ok(Self { id, text })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

impl ModelValue for InstructionArtifact {
    fn model_value(&self) -> Value {
        typed_record(
            "InstructionArtifact",
            [
                ("id", Value::Scalar(Scalar::text(self.id.as_str()))),
                ("text", Value::Scalar(Scalar::text(self.text.as_str()))),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum McpTransport {
    Command(Command),
    Http(HttpTransport),
}

impl ModelValue for McpTransport {
    fn model_value(&self) -> Value {
        match self {
            Self::Command(command) => command.model_value(),
            Self::Http(transport) => transport.model_value(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct McpArtifact {
    id: String,
    name: String,
    transport: McpTransport,
}

impl McpArtifact {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        transport: McpTransport,
    ) -> Result<Self, ValueError> {
        let id = id.into();
        let name = name.into();
        validate_identifier(&id, "artifact id")?;
        validate_identifier(&name, "MCP server name")?;
        Ok(Self {
            id,
            name,
            transport,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn transport(&self) -> &McpTransport {
        &self.transport
    }
}

impl ModelValue for McpArtifact {
    fn model_value(&self) -> Value {
        typed_record(
            "McpArtifact",
            [
                ("id", Value::Scalar(Scalar::text(self.id.as_str()))),
                ("name", Value::Scalar(Scalar::text(self.name.as_str()))),
                ("transport", self.transport.model_value()),
            ],
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HookEvent {
    SessionStart,
    BeforeTool,
    AfterTool,
    Prompt,
    Stop,
}

impl HookEvent {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "session-start",
            Self::BeforeTool => "before-tool",
            Self::AfterTool => "after-tool",
            Self::Prompt => "prompt",
            Self::Stop => "stop",
        }
    }
}

impl FromStr for HookEvent {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "session-start" => Ok(Self::SessionStart),
            "before-tool" => Ok(Self::BeforeTool),
            "after-tool" => Ok(Self::AfterTool),
            "prompt" => Ok(Self::Prompt),
            "stop" => Ok(Self::Stop),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid HookEvent"
            ))),
        }
    }
}

impl ModelValue for HookEvent {
    fn model_value(&self) -> Value {
        Value::Scalar(Scalar::text(self.as_str()))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HookOutcome {
    Continue,
    Block,
    Ask,
    ModifyInput,
    Context,
}

impl HookOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Block => "block",
            Self::Ask => "ask",
            Self::ModifyInput => "modify-input",
            Self::Context => "context",
        }
    }
}

impl FromStr for HookOutcome {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "continue" => Ok(Self::Continue),
            "block" => Ok(Self::Block),
            "ask" => Ok(Self::Ask),
            "modify-input" => Ok(Self::ModifyInput),
            "context" => Ok(Self::Context),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid HookOutcome"
            ))),
        }
    }
}

impl ModelValue for HookOutcome {
    fn model_value(&self) -> Value {
        Value::Scalar(Scalar::text(self.as_str()))
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HookArtifact {
    id: String,
    event: HookEvent,
    command: Command,
    timeout_ms: i32,
    outcomes: Vec<HookOutcome>,
    tools: Vec<String>,
    protocol_version: i32,
}

impl HookArtifact {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        event: HookEvent,
        command: Command,
        timeout_ms: i32,
        outcomes: impl IntoIterator<Item = HookOutcome>,
        tools: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, ValueError> {
        let id = id.into();
        validate_identifier(&id, "artifact id")?;
        if !(1..=300_000).contains(&timeout_ms) {
            return Err(ValueError::new(
                "hook timeout must be 1-300000 integer milliseconds",
            ));
        }
        let mut outcomes: Vec<HookOutcome> = outcomes.into_iter().collect();
        if outcomes.is_empty() {
            return Err(ValueError::new("hook requires declared HookOutcome values"));
        }
        let mut seen_outcomes: HashSet<HookOutcome> = HashSet::with_capacity(outcomes.len());
        for outcome in &outcomes {
            if !seen_outcomes.insert(*outcome) {
                return Err(ValueError::new("hook outcomes must be unique"));
            }
        }
        let mut tools: Vec<String> = tools.into_iter().map(Into::into).collect();
        for name in &tools {
            if name.is_empty() || name.contains('\0') {
                return Err(ValueError::new(
                    "tool names must be nonempty and cannot contain NUL",
                ));
            }
        }
        let mut seen_tools: HashSet<&str> = HashSet::with_capacity(tools.len());
        for name in &tools {
            if !seen_tools.insert(name.as_str()) {
                return Err(ValueError::new("tool names must be unique"));
            }
        }
        if !tools.is_empty() && !matches!(event, HookEvent::BeforeTool | HookEvent::AfterTool) {
            return Err(ValueError::new("tool selection requires a tool event"));
        }
        outcomes.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        tools.sort();
        Ok(Self {
            id,
            event,
            command,
            timeout_ms,
            outcomes,
            tools,
            protocol_version: 1,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub const fn event(&self) -> HookEvent {
        self.event
    }

    pub const fn command(&self) -> &Command {
        &self.command
    }

    pub const fn timeout_ms(&self) -> i32 {
        self.timeout_ms
    }

    pub fn outcomes(&self) -> &[HookOutcome] {
        &self.outcomes
    }

    pub fn tools(&self) -> &[String] {
        &self.tools
    }

    pub const fn protocol_version(&self) -> i32 {
        self.protocol_version
    }
}

impl ModelValue for HookArtifact {
    fn model_value(&self) -> Value {
        typed_record(
            "HookArtifact",
            [
                ("command", self.command.model_value()),
                ("event", self.event.model_value()),
                ("id", Value::Scalar(Scalar::text(self.id.as_str()))),
                (
                    "outcomes",
                    Value::Array(ArrayValue::new(
                        self.outcomes.iter().map(ModelValue::model_value),
                    )),
                ),
                (
                    "protocol_version",
                    Value::Scalar(Scalar::integer(i64::from(self.protocol_version))),
                ),
                (
                    "timeout_ms",
                    Value::Scalar(Scalar::integer(i64::from(self.timeout_ms))),
                ),
                (
                    "tools",
                    Value::Array(ArrayValue::new(
                        self.tools.iter().map(ModelValue::model_value),
                    )),
                ),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NativeArtifact {
    id: String,
    family: Family,
    audience: Audience,
    destination: String,
    content: Content,
}

impl NativeArtifact {
    pub fn new(
        id: impl Into<String>,
        family: Family,
        audience: Audience,
        destination: impl Into<String>,
        content: Content,
    ) -> Result<Self, ValueError> {
        let id = id.into();
        validate_identifier(&id, "artifact id")?;
        let destination = destination.into();
        validate_relative_path(&destination, "native destination")?;
        Ok(Self {
            id,
            family,
            audience,
            destination,
            content,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub const fn family(&self) -> Family {
        self.family
    }

    pub const fn audience(&self) -> &Audience {
        &self.audience
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }

    pub const fn content(&self) -> &Content {
        &self.content
    }
}

impl ModelValue for NativeArtifact {
    fn model_value(&self) -> Value {
        typed_record(
            "NativeArtifact",
            [
                ("audience", self.audience.model_value()),
                ("content", self.content.model_value()),
                (
                    "destination",
                    Value::Scalar(Scalar::text(self.destination.as_str())),
                ),
                ("family", self.family.model_value()),
                ("id", Value::Scalar(Scalar::text(self.id.as_str()))),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SupportAsset {
    id: String,
    family: Family,
    content: TreeContent,
}

impl SupportAsset {
    pub fn new(
        id: impl Into<String>,
        family: Family,
        content: TreeContent,
    ) -> Result<Self, ValueError> {
        let id = id.into();
        validate_identifier(&id, "asset id")?;
        Ok(Self {
            id,
            family,
            content,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub const fn family(&self) -> Family {
        self.family
    }

    pub const fn content(&self) -> &TreeContent {
        &self.content
    }
}

impl ModelValue for SupportAsset {
    fn model_value(&self) -> Value {
        typed_record(
            "SupportAsset",
            [
                ("content", self.content.model_value()),
                ("family", self.family.model_value()),
                ("id", Value::Scalar(Scalar::text(self.id.as_str()))),
            ],
        )
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Artifact {
    Skill(SkillArtifact),
    Instruction(InstructionArtifact),
    Mcp(McpArtifact),
    Hook(HookArtifact),
    Native(NativeArtifact),
}

impl Artifact {
    pub fn id(&self) -> &str {
        match self {
            Self::Skill(artifact) => artifact.id(),
            Self::Instruction(artifact) => artifact.id(),
            Self::Mcp(artifact) => artifact.id(),
            Self::Hook(artifact) => artifact.id(),
            Self::Native(artifact) => artifact.id(),
        }
    }

    pub(crate) const fn command(&self) -> Option<&Command> {
        match self {
            Self::Hook(artifact) => Some(artifact.command()),
            Self::Mcp(artifact) => match artifact.transport() {
                McpTransport::Command(command) => Some(command),
                McpTransport::Http(_) => None,
            },
            _ => None,
        }
    }
}

impl ModelValue for Artifact {
    fn model_value(&self) -> Value {
        match self {
            Self::Skill(artifact) => artifact.model_value(),
            Self::Instruction(artifact) => artifact.model_value(),
            Self::Mcp(artifact) => artifact.model_value(),
            Self::Hook(artifact) => artifact.model_value(),
            Self::Native(artifact) => artifact.model_value(),
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines,
    clippy::unwrap_used
)]
mod tests {
    use super::{
        Artifact, AssetRef, Audience, Command, CommandArgument, EnvRef, EnvironmentValue, Family,
        HookArtifact, HookEvent, HookOutcome, HttpTransport, InstructionArtifact, McpArtifact,
        McpTransport, NativeArtifact, SkillArtifact, SupportAsset,
    };
    use crate::content::{Content, FileContent, FileMode, TreeContent};
    use crate::models::BundleEntry;
    use crate::targets::{Agent, Platform, Surface, TargetScope};

    const NO_ENV: [(String, EnvironmentValue); 0] = [];
    const NO_OUTCOMES: [HookOutcome; 0] = [];
    const NO_TOOLS: [String; 0] = [];

    fn message<T: std::fmt::Debug, E: std::fmt::Display>(result: Result<T, E>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn commands_snapshot_and_sort_their_environment() {
        let mut argv = vec![
            "python".to_owned(),
            "space and 'quotes'".to_owned(),
            "$(not-executed)".to_owned(),
        ];
        let command = Command::new(
            argv.clone(),
            [
                (
                    "TOKEN",
                    EnvironmentValue::from(EnvRef::new("SOURCE_TOKEN").unwrap()),
                ),
                ("MODE", EnvironmentValue::from("quiet")),
            ],
            Some(CommandArgument::from("support")),
        )
        .unwrap();
        argv.clear();
        assert_eq!(
            command.argv(),
            &[
                CommandArgument::from("python"),
                CommandArgument::from("space and 'quotes'"),
                CommandArgument::from("$(not-executed)"),
            ]
        );
        assert_eq!(
            command.env(),
            &[
                ("MODE".to_owned(), EnvironmentValue::from("quiet")),
                (
                    "TOKEN".to_owned(),
                    EnvironmentValue::from(EnvRef::new("SOURCE_TOKEN").unwrap()),
                ),
            ]
        );
        assert_eq!(command.cwd(), Some(&CommandArgument::from("support")));
    }

    #[test]
    fn commands_reject_unsafe_or_ambiguous_input() {
        let empty: [CommandArgument; 0] = [];
        assert_eq!(
            message(Command::new(empty, NO_ENV, None)),
            "argv requires a command"
        );
        assert_eq!(
            message(Command::new([String::new()], NO_ENV, None)),
            "command cannot be empty"
        );
        assert_eq!(
            message(Command::new(["\0".to_owned()], NO_ENV, None)),
            "command arguments cannot contain NUL"
        );
        assert_eq!(
            message(Command::new(
                ["x".to_owned()],
                [("A", "x"), ("A", "y")],
                None
            )),
            "duplicate environment name"
        );
        assert_eq!(
            message(Command::new(
                ["x".to_owned()],
                [("A", EnvironmentValue::from("\0"))],
                None
            )),
            "environment values cannot contain NUL"
        );
        assert_eq!(
            message(Command::new(
                ["x".to_owned()],
                NO_ENV,
                Some(CommandArgument::from("../escape")),
            )),
            "command cwd must be an unambiguous relative path"
        );
        assert_eq!(
            message(Command::new(
                ["x".to_owned()],
                [("9BAD", EnvironmentValue::from("x"))],
                None
            )),
            "invalid environment reference"
        );
        let asset = AssetRef::with_path("runtime", "tools").unwrap();
        let referenced = Command::new(
            [asset],
            NO_ENV,
            Some(AssetRef::new("runtime").unwrap().into()),
        );
        assert!(referenced.is_ok());
    }

    #[test]
    fn http_transports_require_absolute_clean_http_urls() {
        let transport = HttpTransport::new(
            "https://tools.example.test:8443/mcp",
            Some(EnvRef::new("REMOTE_TOKEN").unwrap()),
        )
        .unwrap();
        assert_eq!(transport.url(), "https://tools.example.test:8443/mcp");
        assert_eq!(
            transport.bearer_token().map(EnvRef::name),
            Some("REMOTE_TOKEN")
        );
        for url in [
            "stdio:python",
            "https://user:pass@example.test/x",
            "https://example.test/a b",
            "http:///x",
            "http:////x",
            "http://",
            "http://:80/",
            "http://user@h/",
            "http://h/#frag",
            "http://h:0/",
            "http://h:65536/",
            "http://h:a/",
            "http://h:+80/",
            "http://h: 80/",
            "http://h/\u{2028}",
            "http://[abc]/",
            "http://[]/",
            "http://[::1/",
            "http://[1:2:3]/",
            "http://[v1.]/",
            "1http://x",
        ] {
            assert!(HttpTransport::new(url, None).is_err(), "must reject {url}");
        }
        assert!(HttpTransport::new("HTTP://H/", None).is_ok());
        assert!(HttpTransport::new("http://h:80/", None).is_ok());
        assert!(HttpTransport::new("http://h/#", None).is_ok());
        assert!(HttpTransport::new("http://h:", None).is_ok());
        assert!(HttpTransport::new("http://h/?q=1", None).is_ok());
        assert!(HttpTransport::new("http://[::1]:8080/", None).is_ok());
        assert!(HttpTransport::new("http://[v1.x]/", None).is_ok());
        assert!(HttpTransport::new("http://[::ffff:192.0.2.1]/", None).is_ok());
        assert!(HttpTransport::new("http://h/\u{7f}", None).is_ok());
    }

    #[test]
    fn http_transport_authority_cases_match_python_urlsplit() {
        let invalid_ipv6 = [
            "http://host]",
            "http://]",
            "http://host]:80",
            "https://[::1]junk/",
            "https://prefix[::1]/",
        ];
        for url in invalid_ipv6 {
            assert_eq!(
                message(HttpTransport::new(url, None)),
                "Invalid IPv6 URL",
                "{url}"
            );
        }

        for (url, netloc) in [
            ("https://example／evil/path", "example／evil"),
            ("https://example？evil/path", "example？evil"),
            ("https://example＃evil/path", "example＃evil"),
            ("https://example＠evil/path", "example＠evil"),
            ("https://example：80/path", "example：80"),
        ] {
            assert_eq!(
                message(HttpTransport::new(url, None)),
                format!("netloc '{netloc}' contains invalid characters under NFKC normalization"),
                "{url}"
            );
        }

        for url in ["https://[::1]/", "https://[::1]:8080/", "https://[v1.x]/"] {
            assert!(HttpTransport::new(url, None).is_ok(), "{url}");
        }
        for url in ["https://\u{212a}.example/", "https://\u{ff45}xample.test/"] {
            assert!(HttpTransport::new(url, None).is_ok(), "{url}");
        }
        assert_eq!(
            message(HttpTransport::new("https://user@[::1]:8080/", None)),
            "MCP URL cannot embed credentials or a fragment"
        );
    }

    #[test]
    fn hooks_validate_events_outcomes_tools_and_timeouts() {
        let command = Command::new(["test".to_owned()], NO_ENV, None).unwrap();
        let hook = HookArtifact::new(
            "check-tool",
            HookEvent::BeforeTool,
            command,
            10_000,
            [HookOutcome::Continue, HookOutcome::Block],
            ["shell"],
        )
        .unwrap();
        assert_eq!(hook.protocol_version(), 1);
        assert_eq!(hook.outcomes(), [HookOutcome::Block, HookOutcome::Continue]);
        assert_eq!(hook.tools(), ["shell".to_owned()]);
        let same = HookArtifact::new(
            "check-tool",
            HookEvent::Stop,
            Command::new(["test".to_owned()], NO_ENV, None).unwrap(),
            1,
            [HookOutcome::Continue],
            NO_TOOLS,
        )
        .unwrap();
        assert_eq!(same.tools(), [] as [String; 0]);
        assert_eq!(
            message(HookArtifact::new(
                "x",
                HookEvent::Stop,
                Command::new(["x".to_owned()], NO_ENV, None).unwrap(),
                0,
                [HookOutcome::Continue],
                NO_TOOLS
            )),
            "hook timeout must be 1-300000 integer milliseconds"
        );
        assert_eq!(
            message(HookArtifact::new(
                "x",
                HookEvent::Stop,
                Command::new(["x".to_owned()], NO_ENV, None).unwrap(),
                300_001,
                [HookOutcome::Continue],
                NO_TOOLS
            )),
            "hook timeout must be 1-300000 integer milliseconds"
        );
        assert_eq!(
            message(HookArtifact::new(
                "x",
                HookEvent::Stop,
                Command::new(["x".to_owned()], NO_ENV, None).unwrap(),
                10,
                NO_OUTCOMES,
                NO_TOOLS
            )),
            "hook requires declared HookOutcome values"
        );
        assert_eq!(
            message(HookArtifact::new(
                "x",
                HookEvent::Stop,
                Command::new(["x".to_owned()], NO_ENV, None).unwrap(),
                10,
                [HookOutcome::Continue, HookOutcome::Continue],
                NO_TOOLS
            )),
            "hook outcomes must be unique"
        );
        assert_eq!(
            message(HookArtifact::new(
                "x",
                HookEvent::Stop,
                Command::new(["x".to_owned()], NO_ENV, None).unwrap(),
                10,
                [HookOutcome::Continue],
                ["x", "x"]
            )),
            "tool names must be unique"
        );
        assert_eq!(
            message(HookArtifact::new(
                "x",
                HookEvent::Stop,
                Command::new(["x".to_owned()], NO_ENV, None).unwrap(),
                10,
                [HookOutcome::Continue],
                ["x"]
            )),
            "tool selection requires a tool event"
        );
        assert_eq!(
            message(HookArtifact::new(
                "x",
                HookEvent::BeforeTool,
                Command::new(["x".to_owned()], NO_ENV, None).unwrap(),
                10,
                [HookOutcome::Continue],
                [""]
            )),
            "tool names must be nonempty and cannot contain NUL"
        );
    }

    #[test]
    fn audiences_rebind_surfaces_to_their_agents() {
        let cursor = Audience::new(
            Agent::Cursor,
            TargetScope::Project,
            Surface::Ide,
            Some(Platform::Windows),
        )
        .unwrap();
        assert_eq!(cursor.platform(), Some(Platform::Windows));
        let copilot =
            Audience::new(Agent::Copilot, TargetScope::User, Surface::Vscode, None).unwrap();
        assert_eq!(copilot.surface(), Surface::Vscode);
        assert_eq!(
            message(Audience::new(
                Agent::Codex,
                TargetScope::Directory,
                Surface::Cli,
                None
            )),
            "native audiences require project or user scope"
        );
        assert_eq!(
            message(Audience::new(
                Agent::Codex,
                TargetScope::Project,
                Surface::Ide,
                None
            )),
            "IDE surface is only defined for Cursor"
        );
        assert_eq!(
            message(Audience::new(
                Agent::Cursor,
                TargetScope::Project,
                Surface::Vscode,
                None
            )),
            "VSCode surface is only defined for Copilot"
        );
    }

    #[test]
    fn environment_and_asset_references_validate_their_names() {
        assert!(EnvRef::new("SOURCE_TOKEN").is_ok());
        assert!(EnvRef::new("_private9").is_ok());
        for name in ["BAD-NAME", "9BAD", "", "WITH SPACE"] {
            assert_eq!(message(EnvRef::new(name)), "invalid environment reference");
        }
        assert!(AssetRef::new("runtime").is_ok());
        assert!(AssetRef::with_path("runtime", "server.js").is_ok());
        assert_eq!(
            message(AssetRef::with_path("runtime", "../escape")),
            "asset reference path must be an unambiguous relative path"
        );
        assert!(AssetRef::new("Bad").is_err());
    }

    #[test]
    fn skills_require_a_markdown_file_entry() {
        let content = TreeContent::new([
            BundleEntry::file("SKILL.md", b"# Review\n".to_vec()).unwrap(),
            BundleEntry::executable_file("run.py", b"print(1)\n".to_vec()).unwrap(),
        ])
        .unwrap();
        let skill = SkillArtifact::new("review-skill", "review", content).unwrap();
        assert_eq!(skill.id(), "review-skill");
        assert_eq!(skill.name(), "review");
        let empty = TreeContent::new([BundleEntry::directory("SKILL.md").unwrap()]).unwrap();
        assert_eq!(
            message(SkillArtifact::new("x", "x", empty)),
            "skill must contain a SKILL.md file"
        );
        let instruction = InstructionArtifact::new("guide", "hello").unwrap();
        assert_eq!(instruction.text(), "hello");
        assert_eq!(
            message(InstructionArtifact::new("guide", "\0")),
            "instruction text cannot contain NUL"
        );
    }

    #[test]
    fn native_artifacts_validate_destinations() {
        let native = NativeArtifact::new(
            "guide",
            Family::Instructions,
            Audience::new(Agent::Codex, TargetScope::Project, Surface::Cli, None).unwrap(),
            "rules/guide.md",
            Content::File(FileContent::new(b"hi".to_vec(), FileMode::Readable)),
        )
        .unwrap();
        assert_eq!(native.destination(), "rules/guide.md");
        assert_eq!(
            message(NativeArtifact::new(
                "guide",
                Family::Instructions,
                Audience::new(Agent::Codex, TargetScope::Project, Surface::Cli, None).unwrap(),
                "../escape",
                Content::File(FileContent::new(b"hi".to_vec(), FileMode::Readable)),
            )),
            "native destination must be an unambiguous relative path"
        );
        let asset = SupportAsset::new(
            "hook-support",
            Family::Hooks,
            TreeContent::new([BundleEntry::file("check.py", b"pass\n".to_vec()).unwrap()]).unwrap(),
        )
        .unwrap();
        assert_eq!(asset.family(), Family::Hooks);
        let mcp = McpArtifact::new(
            "local-tools",
            "local-tools",
            McpTransport::Command(
                Command::new(
                    ["python".to_owned()],
                    NO_ENV,
                    Some(CommandArgument::from("tools")),
                )
                .unwrap(),
            ),
        )
        .unwrap();
        assert_eq!(mcp.id(), "local-tools");
        assert_eq!(
            Artifact::Skill(
                SkillArtifact::new(
                    "review-skill",
                    "review",
                    TreeContent::new([BundleEntry::file("SKILL.md", Vec::new()).unwrap()]).unwrap(),
                )
                .unwrap()
            )
            .id(),
            "review-skill"
        );
    }
}
