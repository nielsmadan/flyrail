use crate::artifacts::{HookEvent, HookOutcome};
use crate::models::BundleEntry;
use crate::targets::validate_route;
use crate::validation::validate_identifier;
use crate::values::ValueError;
use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub const INPUT_LIMIT: usize = 1_048_576;
pub const OUTPUT_LIMIT: usize = 65_536;
pub const ERROR_LIMIT: usize = 16_384;
pub const TEXT_LIMIT: usize = 8_192;
pub const JSON_DEPTH_LIMIT: usize = 64;
pub const HANDLER_LIMIT: usize = 64;
pub const EVENT_TIMEOUT_LIMIT_MS: u32 = 300_000;

const RUNTIME_FILES: [(&str, &[u8]); 3] = [
    ("runner.mts", include_bytes!("runtime/runner.mts")),
    ("bridge.mts", include_bytes!("runtime/bridge.mts")),
    ("process.mts", include_bytes!("runtime/process.mts")),
];

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HookShell {
    Posix,
    Cmd,
}

impl HookShell {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Posix => "posix",
            Self::Cmd => "cmd",
        }
    }
}

impl FromStr for HookShell {
    type Err = ValueError;

    fn from_str(value: &str) -> Result<Self, ValueError> {
        match value {
            "posix" => Ok(Self::Posix),
            "cmd" => Ok(Self::Cmd),
            _ => Err(ValueError::new(format!(
                "'{value}' is not a valid HookShell"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HookRuntime {
    node: Option<PathBuf>,
    shell: Option<HookShell>,
    resources: Vec<BundleEntry>,
}

impl HookRuntime {
    pub fn new(node: Option<PathBuf>, shell: Option<HookShell>) -> Result<Self, ValueError> {
        if let Some(path) = &node {
            let invalid = validate_route(path).is_err()
                || path.parent().is_none()
                || path.parent() == Some(path.as_path())
                || path
                    .to_str()
                    .is_none_or(|text| text.chars().any(|character| character < ' '));
            if invalid {
                return Err(ValueError::new(
                    "hook runtime requires an absolute Node 24+ executable path",
                ));
            }
        }
        let resources = RUNTIME_FILES
            .iter()
            .map(|(name, data)| BundleEntry::file(*name, data.to_vec()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            node,
            shell,
            resources,
        })
    }

    pub fn node(&self) -> Option<&Path> {
        self.node.as_deref()
    }

    pub const fn shell(&self) -> Option<HookShell> {
        self.shell
    }

    pub fn resources(&self) -> &[BundleEntry] {
        &self.resources
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HookJson(HookValue);

impl HookJson {
    pub fn parse(data: &[u8]) -> Result<Self, HookFailure> {
        parse_json(data, INPUT_LIMIT).map(Self)
    }

    pub fn object(
        members: impl IntoIterator<Item = (impl Into<String>, Self)>,
    ) -> Result<Self, HookFailure> {
        let members = members
            .into_iter()
            .map(|(key, value)| (key.into(), value.0))
            .collect::<Vec<_>>();
        Self::checked(HookValue::object(members))
    }

    pub fn string(value: impl Into<HookText>) -> Result<Self, HookFailure> {
        Self::checked(HookValue::string(value))
    }

    pub fn boolean(value: bool) -> Result<Self, HookFailure> {
        Self::checked(HookValue::Bool(value))
    }

    pub fn integer(value: i64) -> Result<Self, HookFailure> {
        let number = value
            .to_string()
            .parse::<f64>()
            .map_err(|_| HookFailure::new("json-number"))?;
        Self::checked(HookValue::Float(number))
    }

    pub fn array(values: impl IntoIterator<Item = Self>) -> Result<Self, HookFailure> {
        Self::checked(HookValue::Array(
            values.into_iter().map(|value| value.0).collect(),
        ))
    }

    pub fn null() -> Result<Self, HookFailure> {
        Self::checked(HookValue::Null)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        encode_hook_json(&self.0)
    }

    pub const fn is_object(&self) -> bool {
        matches!(self.0, HookValue::Object(_))
    }

    pub fn as_str(&self) -> Option<&str> {
        self.0.as_str()
    }

    pub const fn as_text(&self) -> Option<&HookText> {
        self.0.as_text()
    }

    fn checked(value: HookValue) -> Result<Self, HookFailure> {
        validate_encoded_json(&value, INPUT_LIMIT)?;
        Ok(Self(value))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HookRequest {
    event: HookEvent,
    context: HookJson,
    tool: Option<HookText>,
    input: Option<HookJson>,
    result: Option<HookJson>,
}

impl HookRequest {
    pub fn new(
        event: HookEvent,
        context: HookJson,
        tool: Option<String>,
        input: Option<HookJson>,
        result: Option<HookJson>,
    ) -> Result<Self, HookFailure> {
        Self::new_text(event, context, tool.map(HookText::from), input, result)
    }

    pub fn new_text(
        event: HookEvent,
        context: HookJson,
        tool: Option<HookText>,
        input: Option<HookJson>,
        result: Option<HookJson>,
    ) -> Result<Self, HookFailure> {
        if !context.is_object() {
            return Err(HookFailure::new("host-input"));
        }
        let tool_event = matches!(event, HookEvent::BeforeTool | HookEvent::AfterTool);
        if tool_event && tool.as_ref().is_none_or(HookText::is_empty) {
            return Err(HookFailure::new("host-tool"));
        }
        if tool_event && input.is_none() {
            return Err(HookFailure::new("host-arguments"));
        }
        if event == HookEvent::AfterTool && result.is_none() {
            return Err(HookFailure::new("host-result"));
        }
        if event == HookEvent::Prompt && input.as_ref().and_then(HookJson::as_text).is_none() {
            return Err(HookFailure::new("host-prompt"));
        }
        if !tool_event && tool.is_some()
            || !matches!(
                event,
                HookEvent::BeforeTool | HookEvent::AfterTool | HookEvent::Prompt
            ) && input.is_some()
            || event != HookEvent::AfterTool && result.is_some()
        {
            return Err(HookFailure::new("input-fields"));
        }
        let request = Self {
            event,
            context,
            tool,
            input,
            result,
        };
        validate_encoded_json(&request.as_json(), INPUT_LIMIT)?;
        Ok(request)
    }

    pub fn parse(data: &[u8]) -> Result<Self, HookFailure> {
        let value = parse_json(data, INPUT_LIMIT)?;
        let object = value
            .as_object()
            .ok_or_else(|| HookFailure::new("host-input"))?;
        let expected = ["version", "event", "context", "tool", "input", "result"];
        if object
            .iter()
            .any(|(key, _)| !expected.iter().any(|value| key.equals(value)))
        {
            return Err(HookFailure::new("input-fields"));
        }
        if !field(object, "version").is_some_and(is_one) {
            return Err(HookFailure::new("input-version"));
        }
        let event = field(object, "event")
            .and_then(HookValue::as_str)
            .and_then(|text| HookEvent::from_str(text).ok())
            .ok_or_else(|| HookFailure::new("input-event"))?;
        let context = field(object, "context")
            .cloned()
            .map(HookJson)
            .ok_or_else(|| HookFailure::new("host-input"))?;
        let tool = match field(object, "tool") {
            Some(HookValue::Str(value)) => Some(value.clone()),
            Some(_) => return Err(HookFailure::new("host-tool")),
            None => None,
        };
        let input = field(object, "input").cloned().map(HookJson);
        let result = field(object, "result").cloned().map(HookJson);
        Self::new_text(event, context, tool, input, result)
    }

    pub const fn event(&self) -> HookEvent {
        self.event
    }

    pub const fn context(&self) -> &HookJson {
        &self.context
    }

    pub const fn tool(&self) -> Option<&HookText> {
        self.tool.as_ref()
    }

    pub const fn input(&self) -> Option<&HookJson> {
        self.input.as_ref()
    }

    pub const fn result(&self) -> Option<&HookJson> {
        self.result.as_ref()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        encode_hook_json(&self.as_json())
    }

    fn as_json(&self) -> HookValue {
        let mut members = vec![
            ("version".to_owned(), HookValue::Int(1)),
            (
                "event".to_owned(),
                HookValue::string(self.event.as_str().to_owned()),
            ),
            ("context".to_owned(), self.context.0.clone()),
        ];
        if let Some(tool) = &self.tool {
            members.push(("tool".to_owned(), HookValue::string(tool.clone())));
        }
        if let Some(input) = &self.input {
            members.push(("input".to_owned(), input.0.clone()));
        }
        if let Some(result) = &self.result {
            members.push(("result".to_owned(), result.0.clone()));
        }
        HookValue::object(members)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HookResponse {
    outcome: HookOutcome,
    value: Option<HookJson>,
}

impl HookResponse {
    #[must_use]
    pub const fn continuing() -> Self {
        Self {
            outcome: HookOutcome::Continue,
            value: None,
        }
    }

    pub fn block(reason: impl Into<HookText>) -> Result<Self, HookFailure> {
        Self::text(HookOutcome::Block, reason.into(), "reason")
    }

    pub fn ask(reason: impl Into<HookText>) -> Result<Self, HookFailure> {
        Self::text(HookOutcome::Ask, reason.into(), "reason")
    }

    pub fn context(text: impl Into<HookText>) -> Result<Self, HookFailure> {
        Self::text(HookOutcome::Context, text.into(), "context")
    }

    pub fn modify_input(event: HookEvent, input: HookJson) -> Result<Self, HookFailure> {
        let valid = if event == HookEvent::Prompt {
            input.as_text().is_some()
        } else {
            input.is_object()
        };
        if !valid {
            return Err(HookFailure::new("replacement"));
        }
        let response = Self {
            outcome: HookOutcome::ModifyInput,
            value: Some(input),
        };
        validate_encoded_json(&response.as_json(), INPUT_LIMIT)?;
        Ok(response)
    }

    fn text(outcome: HookOutcome, text: HookText, code: &'static str) -> Result<Self, HookFailure> {
        if text.is_blank() || text.utf8_len() > TEXT_LIMIT {
            return Err(HookFailure::new(code));
        }
        Ok(Self {
            outcome,
            value: Some(HookJson::checked(HookValue::string(text))?),
        })
    }

    pub fn parse(
        data: &[u8],
        event: HookEvent,
        declared: &[HookOutcome],
    ) -> Result<Self, HookFailure> {
        if data.len() > OUTPUT_LIMIT {
            return Err(HookFailure::new("stdout-limit"));
        }
        let raw = parse_json(data, OUTPUT_LIMIT).map_err(|_| HookFailure::new("invalid-output"))?;
        let object = raw
            .as_object()
            .ok_or_else(|| HookFailure::new("undeclared-outcome"))?;
        if !field(object, "version").is_some_and(is_one) {
            return Err(HookFailure::new("undeclared-outcome"));
        }
        let outcome = field(object, "outcome")
            .and_then(HookValue::as_str)
            .and_then(|value| HookOutcome::from_str(value).ok())
            .filter(|value| declared.contains(value))
            .ok_or_else(|| HookFailure::new("undeclared-outcome"))?;
        let field_name = match outcome {
            HookOutcome::Block | HookOutcome::Ask => Some("reason"),
            HookOutcome::Context => Some("text"),
            HookOutcome::ModifyInput => Some("input"),
            HookOutcome::Continue => None,
        };
        let expected = 2 + usize::from(field_name.is_some());
        if object.len() != expected
            || object.iter().any(|(key, _)| {
                !key.equals("version") && !key.equals("outcome") && key.as_str() != field_name
            })
        {
            return Err(HookFailure::new("output-fields"));
        }
        match outcome {
            HookOutcome::Continue => Ok(Self::continuing()),
            HookOutcome::Block => Self::block(
                field(object, "reason")
                    .and_then(HookValue::as_text)
                    .cloned()
                    .unwrap_or_default(),
            ),
            HookOutcome::Ask => Self::ask(
                field(object, "reason")
                    .and_then(HookValue::as_text)
                    .cloned()
                    .unwrap_or_default(),
            ),
            HookOutcome::Context => Self::context(
                field(object, "text")
                    .and_then(HookValue::as_text)
                    .cloned()
                    .unwrap_or_default(),
            ),
            HookOutcome::ModifyInput => {
                let input = field(object, "input")
                    .cloned()
                    .map(HookJson)
                    .ok_or_else(|| HookFailure::new("replacement"))?;
                Self::modify_input(event, input)
            }
        }
    }

    pub const fn outcome(&self) -> HookOutcome {
        self.outcome
    }

    pub const fn value(&self) -> Option<&HookJson> {
        self.value.as_ref()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        encode_hook_json(&self.as_json())
    }

    fn as_json(&self) -> HookValue {
        let mut members = vec![
            ("version".to_owned(), HookValue::Int(1)),
            (
                "outcome".to_owned(),
                HookValue::string(self.outcome.as_str().to_owned()),
            ),
        ];
        if let Some(value) = &self.value {
            let name = match self.outcome {
                HookOutcome::Block | HookOutcome::Ask => "reason",
                HookOutcome::Context => "text",
                HookOutcome::ModifyInput => "input",
                HookOutcome::Continue => return HookValue::object(members),
            };
            members.push((name.to_owned(), value.0.clone()));
        }
        HookValue::object(members)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HookEnvironment {
    Text(String),
    Reference(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookHandler {
    id: String,
    event: HookEvent,
    argv: Vec<String>,
    environment: Vec<(String, HookEnvironment)>,
    cwd: PathBuf,
    timeout_ms: u32,
    outcomes: Vec<HookOutcome>,
    tools: Vec<String>,
}

impl HookHandler {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        event: HookEvent,
        argv: impl IntoIterator<Item = String>,
        environment: impl IntoIterator<Item = (String, HookEnvironment)>,
        cwd: impl AsRef<Path>,
        timeout_ms: u32,
        outcomes: impl IntoIterator<Item = HookOutcome>,
        tools: impl IntoIterator<Item = String>,
    ) -> Result<Self, ValueError> {
        let id = id.into();
        validate_identifier(&id, "artifact id")?;
        let argv = argv.into_iter().collect::<Vec<_>>();
        if argv.is_empty() {
            return Err(ValueError::new("argv requires a command"));
        }
        if argv.iter().any(|value| value.contains('\0')) {
            return Err(ValueError::new("command arguments cannot contain NUL"));
        }
        if argv.first().is_none_or(String::is_empty) {
            return Err(ValueError::new("command cannot be empty"));
        }
        let cwd = validate_route(cwd.as_ref())?;
        if !(1..=EVENT_TIMEOUT_LIMIT_MS).contains(&timeout_ms) {
            return Err(ValueError::new(
                "hook timeout must be 1-300000 integer milliseconds",
            ));
        }
        let mut outcomes = outcomes.into_iter().collect::<Vec<_>>();
        if outcomes.is_empty() {
            return Err(ValueError::new("hook requires declared HookOutcome values"));
        }
        if outcomes.iter().collect::<HashSet<_>>().len() != outcomes.len() {
            return Err(ValueError::new("hook outcomes must be unique"));
        }
        let mut tools = tools.into_iter().collect::<Vec<_>>();
        if tools
            .iter()
            .any(|name| name.is_empty() || name.contains('\0'))
        {
            return Err(ValueError::new(
                "tool names must be nonempty and cannot contain NUL",
            ));
        }
        if tools.iter().collect::<HashSet<_>>().len() != tools.len() {
            return Err(ValueError::new("tool names must be unique"));
        }
        if !tools.is_empty() && !matches!(event, HookEvent::BeforeTool | HookEvent::AfterTool) {
            return Err(ValueError::new("tool selection requires a tool event"));
        }
        let mut environment = environment.into_iter().collect::<Vec<_>>();
        let mut names = HashSet::new();
        for (name, value) in &environment {
            crate::artifacts::EnvRef::new(name)?;
            if !names.insert(name) {
                return Err(ValueError::new("duplicate environment name"));
            }
            match value {
                HookEnvironment::Text(text) if text.contains('\0') => {
                    return Err(ValueError::new("environment values cannot contain NUL"));
                }
                HookEnvironment::Reference(name) => {
                    crate::artifacts::EnvRef::new(name)?;
                }
                HookEnvironment::Text(_) => {}
            }
        }
        outcomes.sort_by_key(|value| value.as_str());
        tools.sort();
        environment.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(Self {
            id,
            event,
            argv,
            environment,
            cwd,
            timeout_ms,
            outcomes,
            tools,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub const fn event(&self) -> HookEvent {
        self.event
    }

    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    pub fn environment(&self) -> &[(String, HookEnvironment)] {
        &self.environment
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub const fn timeout_ms(&self) -> u32 {
        self.timeout_ms
    }

    pub fn outcomes(&self) -> &[HookOutcome] {
        &self.outcomes
    }

    pub fn tools(&self) -> &[String] {
        &self.tools
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookConfig {
    host: String,
    handlers: Vec<HookHandler>,
}

impl HookConfig {
    pub fn new(
        host: impl Into<String>,
        handlers: impl IntoIterator<Item = HookHandler>,
    ) -> Result<Self, ValueError> {
        let host = host.into();
        if host.trim().is_empty() {
            return Err(ValueError::new("hook host must be a nonblank string"));
        }
        let mut handlers = handlers.into_iter().collect::<Vec<_>>();
        handlers.sort_by(|left, right| left.id.cmp(&right.id));
        for event in [
            HookEvent::SessionStart,
            HookEvent::BeforeTool,
            HookEvent::AfterTool,
            HookEvent::Prompt,
            HookEvent::Stop,
        ] {
            let selected = handlers
                .iter()
                .filter(|handler| handler.event == event)
                .collect::<Vec<_>>();
            if selected.len() > HANDLER_LIMIT {
                return Err(ValueError::new(
                    "hook event cannot have more than 64 handlers",
                ));
            }
            if selected
                .iter()
                .map(|handler| u64::from(handler.timeout_ms))
                .sum::<u64>()
                > u64::from(EVENT_TIMEOUT_LIMIT_MS)
            {
                return Err(ValueError::new(
                    "hook event timeout sum cannot exceed 300000 milliseconds",
                ));
            }
        }
        Ok(Self { host, handlers })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn handlers(&self) -> &[HookHandler] {
        &self.handlers
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let handlers = self
            .handlers
            .iter()
            .map(|handler| {
                let environment = handler
                    .environment
                    .iter()
                    .map(|(name, value)| {
                        let value = match value {
                            HookEnvironment::Text(text) => HookValue::string(text.clone()),
                            HookEnvironment::Reference(name) => HookValue::object(vec![(
                                "ref".to_owned(),
                                HookValue::string(name.clone()),
                            )]),
                        };
                        (name.clone(), value)
                    })
                    .collect();
                HookValue::object(vec![
                    ("id".to_owned(), HookValue::string(handler.id.clone())),
                    (
                        "event".to_owned(),
                        HookValue::string(handler.event.as_str().to_owned()),
                    ),
                    (
                        "argv".to_owned(),
                        HookValue::Array(
                            handler
                                .argv
                                .iter()
                                .cloned()
                                .map(HookValue::string)
                                .collect(),
                        ),
                    ),
                    (
                        "cwd".to_owned(),
                        HookValue::string(handler.cwd.display().to_string()),
                    ),
                    ("env".to_owned(), HookValue::object(environment)),
                    (
                        "timeout_ms".to_owned(),
                        HookValue::Int(i64::from(handler.timeout_ms)),
                    ),
                    (
                        "outcomes".to_owned(),
                        HookValue::Array(
                            handler
                                .outcomes
                                .iter()
                                .map(|value| HookValue::string(value.as_str().to_owned()))
                                .collect(),
                        ),
                    ),
                    (
                        "tools".to_owned(),
                        HookValue::Array(
                            handler
                                .tools
                                .iter()
                                .cloned()
                                .map(HookValue::string)
                                .collect(),
                        ),
                    ),
                ])
            })
            .collect();
        HookJson(HookValue::object(vec![
            ("host".to_owned(), HookValue::string(self.host.clone())),
            ("handlers".to_owned(), HookValue::Array(handlers)),
        ]))
        .to_bytes()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HookEffects {
    input: Option<HookJson>,
    contexts: Vec<HookText>,
    decision: Option<HookOutcome>,
    reason: Option<HookText>,
}

impl HookEffects {
    pub fn parse(data: &[u8]) -> Result<Self, HookFailure> {
        let raw = parse_json(data, OUTPUT_LIMIT).map_err(|_| HookFailure::new("invalid-output"))?;
        let object = raw
            .as_object()
            .ok_or_else(|| HookFailure::new("output-fields"))?;
        if object.iter().any(|(key, _)| {
            !["input", "contexts", "decision", "reason"]
                .iter()
                .any(|value| key.equals(value))
        }) {
            return Err(HookFailure::new("output-fields"));
        }
        let input = field(object, "input").cloned().map(HookJson);
        let contexts = field(object, "contexts")
            .and_then(HookValue::as_array)
            .ok_or_else(|| HookFailure::new("output-fields"))?
            .iter()
            .map(|value| match value {
                HookValue::Str(text) => Ok(text.clone()),
                _ => Err(HookFailure::new("context")),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if contexts.iter().any(HookText::is_blank) || joined_utf8_len(&contexts) > TEXT_LIMIT {
            return Err(HookFailure::new("context-limit"));
        }
        let decision = match field(object, "decision") {
            Some(HookValue::Str(value)) if value.equals("block") => Some(HookOutcome::Block),
            Some(HookValue::Str(value)) if value.equals("ask") => Some(HookOutcome::Ask),
            Some(_) => return Err(HookFailure::new("undeclared-outcome")),
            None => None,
        };
        let reason = match field(object, "reason") {
            Some(HookValue::Str(value)) if !value.is_blank() && value.utf8_len() <= TEXT_LIMIT => {
                Some(value.clone())
            }
            Some(_) => return Err(HookFailure::new("reason")),
            None => None,
        };
        if decision.is_some() != reason.is_some() {
            return Err(HookFailure::new("output-fields"));
        }
        Ok(Self {
            input,
            contexts,
            decision,
            reason,
        })
    }

    pub const fn input(&self) -> Option<&HookJson> {
        self.input.as_ref()
    }

    pub fn contexts(&self) -> &[HookText] {
        &self.contexts
    }

    pub const fn decision(&self) -> Option<HookOutcome> {
        self.decision
    }

    pub const fn reason(&self) -> Option<&HookText> {
        self.reason.as_ref()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut members = vec![(
            "contexts".to_owned(),
            HookValue::Array(
                self.contexts
                    .iter()
                    .cloned()
                    .map(HookValue::string)
                    .collect(),
            ),
        )];
        if let Some(input) = &self.input {
            members.push(("input".to_owned(), input.0.clone()));
        }
        if let Some(decision) = self.decision {
            members.push((
                "decision".to_owned(),
                HookValue::string(decision.as_str().to_owned()),
            ));
        }
        if let Some(reason) = &self.reason {
            members.push(("reason".to_owned(), HookValue::string(reason.clone())));
        }
        HookJson(HookValue::object(members)).to_bytes()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookFailure {
    code: String,
}

impl HookFailure {
    #[must_use]
    pub fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }

    pub fn code(&self) -> &str {
        &self.code
    }
}

impl fmt::Display for HookFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "flyrail-hook:{}", self.code)
    }
}

impl Error for HookFailure {}

#[derive(Clone, Debug, PartialEq)]
enum HookValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(HookText),
    Array(Vec<Self>),
    Object(Vec<(HookText, Self)>),
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct HookText {
    units: Vec<u16>,
    scalar: Option<String>,
}

impl HookText {
    #[must_use]
    pub fn from_utf16(units: impl Into<Vec<u16>>) -> Self {
        Self::from_units(units.into())
    }

    fn from_units(units: Vec<u16>) -> Self {
        let scalar = String::from_utf16(&units).ok();
        Self { units, scalar }
    }

    pub fn as_str(&self) -> Option<&str> {
        self.scalar.as_deref()
    }

    pub fn units(&self) -> &[u16] {
        &self.units
    }

    pub const fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    fn is_blank(&self) -> bool {
        self.units.iter().copied().all(is_javascript_whitespace)
    }

    fn utf8_len(&self) -> usize {
        char::decode_utf16(self.units.iter().copied())
            .map(|result| result.unwrap_or(char::REPLACEMENT_CHARACTER).len_utf8())
            .sum()
    }

    fn equals(&self, value: &str) -> bool {
        self.units.iter().copied().eq(value.encode_utf16())
    }
}

impl From<String> for HookText {
    fn from(value: String) -> Self {
        Self {
            units: value.encode_utf16().collect(),
            scalar: Some(value),
        }
    }
}

impl From<&str> for HookText {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

impl PartialEq<str> for HookText {
    fn eq(&self, other: &str) -> bool {
        self.equals(other)
    }
}

impl PartialEq<&str> for HookText {
    fn eq(&self, other: &&str) -> bool {
        self.equals(other)
    }
}

const fn is_javascript_whitespace(unit: u16) -> bool {
    matches!(
        unit,
        0x0009..=0x000d
            | 0x0020
            | 0x00a0
            | 0x1680
            | 0x2000..=0x200a
            | 0x2028
            | 0x2029
            | 0x202f
            | 0x205f
            | 0x3000
            | 0xfeff
    )
}

fn joined_utf8_len(values: &[HookText]) -> usize {
    values
        .iter()
        .map(HookText::utf8_len)
        .sum::<usize>()
        .saturating_add(values.len().saturating_sub(1))
}

impl HookValue {
    fn string(value: impl Into<HookText>) -> Self {
        Self::Str(value.into())
    }

    fn object<K: Into<HookText>>(values: Vec<(K, Self)>) -> Self {
        Self::Object(
            values
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(value) => value.as_str(),
            _ => None,
        }
    }

    const fn as_text(&self) -> Option<&HookText> {
        match self {
            Self::Str(value) => Some(value),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(value) => Some(value),
            _ => None,
        }
    }

    fn as_object(&self) -> Option<&[(HookText, Self)]> {
        match self {
            Self::Object(value) => Some(value),
            _ => None,
        }
    }
}

fn parse_json(data: &[u8], limit: usize) -> Result<HookValue, HookFailure> {
    if data.len() > limit {
        return Err(HookFailure::new("input-limit"));
    }
    HookParser::new(data).parse()
}

#[derive(Clone, Copy)]
enum SyntaxExpectation {
    Value,
    ArrayFirst,
    ArrayNext,
    ObjectFirst,
    ObjectKey,
    Colon,
    ObjectNext,
    End,
}

struct HookParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> HookParser<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn parse(mut self) -> Result<HookValue, HookFailure> {
        std::str::from_utf8(self.bytes).map_err(|_| HookFailure::new("invalid-json"))?;
        if let Some(code) = self.validate_syntax()? {
            return Err(HookFailure::new(code));
        }
        self.pos = 0;
        let value = self.value(0)?;
        self.whitespace();
        if self.pos != self.bytes.len() {
            return Err(HookFailure::new("invalid-json"));
        }
        validate_json(&value, 0)?;
        Ok(value)
    }

    #[allow(clippy::too_many_lines)]
    fn validate_syntax(&mut self) -> Result<Option<&'static str>, HookFailure> {
        let mut stack = vec![SyntaxExpectation::End, SyntaxExpectation::Value];
        let mut object_keys = Vec::<HashSet<HookText>>::new();
        let mut depth = 0_usize;
        let mut semantic_error = None;
        while let Some(expectation) = stack.pop() {
            self.whitespace();
            match expectation {
                SyntaxExpectation::Value => match self.peek() {
                    Some(b'[') => {
                        self.pos += 1;
                        depth += 1;
                        if depth > JSON_DEPTH_LIMIT && semantic_error.is_none() {
                            semantic_error = Some("json-depth");
                        }
                        stack.push(SyntaxExpectation::ArrayFirst);
                    }
                    Some(b'{') => {
                        self.pos += 1;
                        depth += 1;
                        if depth > JSON_DEPTH_LIMIT && semantic_error.is_none() {
                            semantic_error = Some("json-depth");
                        }
                        object_keys.push(HashSet::new());
                        stack.push(SyntaxExpectation::ObjectFirst);
                    }
                    Some(b'"') => self.skip_string()?,
                    Some(b't') => self.literal(b"true")?,
                    Some(b'f') => self.literal(b"false")?,
                    Some(b'n') => self.literal(b"null")?,
                    Some(b'-' | b'0'..=b'9') => {
                        let text = self.number_text()?;
                        let finite = std::str::from_utf8(text)
                            .ok()
                            .and_then(|value| value.parse::<f64>().ok())
                            .is_some_and(f64::is_finite);
                        if !finite && semantic_error.is_none() {
                            semantic_error = Some("json-number");
                        }
                    }
                    _ => return Err(HookFailure::new("invalid-json")),
                },
                SyntaxExpectation::ArrayFirst => {
                    if self.peek() == Some(b']') {
                        self.pos += 1;
                        depth -= 1;
                    } else {
                        stack.push(SyntaxExpectation::ArrayNext);
                        stack.push(SyntaxExpectation::Value);
                    }
                }
                SyntaxExpectation::ArrayNext => match self.peek() {
                    Some(b',') => {
                        self.pos += 1;
                        stack.push(SyntaxExpectation::ArrayNext);
                        stack.push(SyntaxExpectation::Value);
                    }
                    Some(b']') => {
                        self.pos += 1;
                        depth -= 1;
                    }
                    _ => return Err(HookFailure::new("invalid-json")),
                },
                SyntaxExpectation::ObjectFirst => {
                    if self.peek() == Some(b'}') {
                        self.pos += 1;
                        depth -= 1;
                        object_keys.pop();
                    } else {
                        stack.push(SyntaxExpectation::ObjectKey);
                    }
                }
                SyntaxExpectation::ObjectKey => {
                    let key = self.string()?;
                    let invalid = object_keys.last_mut().is_none_or(|keys| {
                        !keys.insert(key.clone())
                            || ["__proto__", "constructor", "prototype"]
                                .iter()
                                .any(|value| key.equals(value))
                    });
                    if invalid && semantic_error.is_none() {
                        semantic_error = Some("json-key");
                    }
                    stack.push(SyntaxExpectation::Colon);
                }
                SyntaxExpectation::Colon => {
                    if self.peek() != Some(b':') {
                        return Err(HookFailure::new("invalid-json"));
                    }
                    self.pos += 1;
                    stack.push(SyntaxExpectation::ObjectNext);
                    stack.push(SyntaxExpectation::Value);
                }
                SyntaxExpectation::ObjectNext => match self.peek() {
                    Some(b',') => {
                        self.pos += 1;
                        stack.push(SyntaxExpectation::ObjectKey);
                    }
                    Some(b'}') => {
                        self.pos += 1;
                        depth -= 1;
                        object_keys.pop();
                    }
                    _ => return Err(HookFailure::new("invalid-json")),
                },
                SyntaxExpectation::End => {
                    if self.pos != self.bytes.len() {
                        return Err(HookFailure::new("invalid-json"));
                    }
                }
            }
        }
        Ok(semantic_error)
    }

    fn value(&mut self, depth: usize) -> Result<HookValue, HookFailure> {
        self.whitespace();
        match self.peek() {
            Some(b'[') => self.array(depth),
            Some(b'{') => self.object(depth),
            Some(b'"') => self.string().map(HookValue::string),
            Some(b't') => self.literal(b"true").map(|()| HookValue::Bool(true)),
            Some(b'f') => self.literal(b"false").map(|()| HookValue::Bool(false)),
            Some(b'n') => self.literal(b"null").map(|()| HookValue::Null),
            Some(b'-' | b'0'..=b'9') => {
                let text = self.number_text()?;
                let number = std::str::from_utf8(text)
                    .ok()
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| HookFailure::new("json-number"))?;
                Ok(HookValue::Float(number))
            }
            _ => Err(HookFailure::new("invalid-json")),
        }
    }

    fn array(&mut self, depth: usize) -> Result<HookValue, HookFailure> {
        if depth >= JSON_DEPTH_LIMIT {
            return Err(HookFailure::new("json-depth"));
        }
        self.pos += 1;
        self.whitespace();
        let mut values = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(HookValue::Array(values));
        }
        loop {
            values.push(self.value(depth + 1)?);
            self.whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(HookValue::Array(values));
                }
                _ => return Err(HookFailure::new("invalid-json")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<HookValue, HookFailure> {
        if depth >= JSON_DEPTH_LIMIT {
            return Err(HookFailure::new("json-depth"));
        }
        self.pos += 1;
        self.whitespace();
        let mut values = Vec::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(HookValue::object(values));
        }
        loop {
            self.whitespace();
            let key = self.string()?;
            self.whitespace();
            if self.peek() != Some(b':') {
                return Err(HookFailure::new("invalid-json"));
            }
            self.pos += 1;
            let value = self.value(depth + 1)?;
            values.push((key, value));
            self.whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(HookValue::object(values));
                }
                _ => return Err(HookFailure::new("invalid-json")),
            }
        }
    }

    fn string(&mut self) -> Result<HookText, HookFailure> {
        if self.peek() != Some(b'"') {
            return Err(HookFailure::new("invalid-json"));
        }
        self.pos += 1;
        let mut units = Vec::new();
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(HookText::from_units(units));
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self
                        .take()
                        .ok_or_else(|| HookFailure::new("invalid-json"))?
                    {
                        b'"' => units.push(u16::from(b'"')),
                        b'\\' => units.push(u16::from(b'\\')),
                        b'/' => units.push(u16::from(b'/')),
                        b'b' => units.push(8),
                        b'f' => units.push(12),
                        b'n' => units.push(10),
                        b'r' => units.push(13),
                        b't' => units.push(9),
                        b'u' => units.push(self.hex4()?),
                        _ => return Err(HookFailure::new("invalid-json")),
                    }
                }
                Some(0..=0x1f) | None => return Err(HookFailure::new("invalid-json")),
                Some(_) => {
                    let start = self.pos;
                    while matches!(self.peek(), Some(0x20..=0x7f) if self.peek() != Some(b'"') && self.peek() != Some(b'\\'))
                    {
                        self.pos += 1;
                    }
                    if self.pos == start {
                        let text = self
                            .bytes
                            .get(self.pos..)
                            .and_then(|bytes| std::str::from_utf8(bytes).ok())
                            .ok_or_else(|| HookFailure::new("invalid-json"))?;
                        let character = text
                            .chars()
                            .next()
                            .ok_or_else(|| HookFailure::new("invalid-json"))?;
                        units.extend(character.encode_utf16(&mut [0; 2]).iter().copied());
                        self.pos += character.len_utf8();
                    } else {
                        let bytes = self
                            .bytes
                            .get(start..self.pos)
                            .ok_or_else(|| HookFailure::new("invalid-json"))?;
                        units.extend(bytes.iter().map(|byte| u16::from(*byte)));
                    }
                }
            }
        }
    }

    fn skip_string(&mut self) -> Result<(), HookFailure> {
        self.string().map(|_| ())
    }

    fn number_text(&mut self) -> Result<&'a [u8], HookFailure> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(HookFailure::new("invalid-json"));
                }
            }
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(HookFailure::new("invalid-json")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(HookFailure::new("invalid-json"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(HookFailure::new("invalid-json"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        self.bytes
            .get(start..self.pos)
            .ok_or_else(|| HookFailure::new("invalid-json"))
    }

    fn literal(&mut self, literal: &[u8]) -> Result<(), HookFailure> {
        let end = self.pos.saturating_add(literal.len());
        if self.bytes.get(self.pos..end) != Some(literal) {
            return Err(HookFailure::new("invalid-json"));
        }
        self.pos = end;
        Ok(())
    }

    fn hex4(&mut self) -> Result<u16, HookFailure> {
        let digits = self
            .bytes
            .get(self.pos..self.pos.saturating_add(4))
            .ok_or_else(|| HookFailure::new("invalid-json"))?;
        let mut value = 0_u16;
        for digit in digits {
            value = value * 16
                + u16::from(match digit {
                    b'0'..=b'9' => digit - b'0',
                    b'a'..=b'f' => digit - b'a' + 10,
                    b'A'..=b'F' => digit - b'A' + 10,
                    _ => return Err(HookFailure::new("invalid-json")),
                });
        }
        self.pos += 4;
        Ok(value)
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn take(&mut self) -> Option<u8> {
        let value = self.peek()?;
        self.pos += 1;
        Some(value)
    }
}

fn validate_encoded_json(value: &HookValue, limit: usize) -> Result<(), HookFailure> {
    validate_json(value, 0)?;
    if encode_hook_json(value).len() > limit {
        return Err(HookFailure::new("input-limit"));
    }
    Ok(())
}

fn encode_hook_json(value: &HookValue) -> Vec<u8> {
    let mut output = String::new();
    encode_hook_value(value, &mut output);
    output.into_bytes()
}

fn write_hook_string(value: &HookText, output: &mut String) {
    use std::fmt::Write as _;

    output.push('"');
    let mut index = 0;
    while let Some(unit) = value.units.get(index).copied() {
        match unit {
            0x22 => output.push_str("\\\""),
            0x5c => output.push_str("\\\\"),
            0x08 => output.push_str("\\b"),
            0x0c => output.push_str("\\f"),
            0x0a => output.push_str("\\n"),
            0x0d => output.push_str("\\r"),
            0x09 => output.push_str("\\t"),
            0x00..=0x1f | 0xd800..=0xdfff => {
                if let Some(next) = value.units.get(index + 1).copied().filter(|next| {
                    (0xd800..=0xdbff).contains(&unit) && (0xdc00..=0xdfff).contains(next)
                }) {
                    let scalar =
                        0x10000 + (u32::from(unit - 0xd800) << 10) + u32::from(next - 0xdc00);
                    if let Some(character) = char::from_u32(scalar) {
                        output.push(character);
                        index += 1;
                    }
                } else {
                    let _ = write!(output, "\\u{unit:04x}");
                }
            }
            _ => {
                if let Some(character) = char::from_u32(u32::from(unit)) {
                    output.push(character);
                }
            }
        }
        index += 1;
    }
    output.push('"');
}

fn encode_hook_value(value: &HookValue, output: &mut String) {
    match value {
        HookValue::Null => output.push_str("null"),
        HookValue::Bool(true) => output.push_str("true"),
        HookValue::Bool(false) => output.push_str("false"),
        HookValue::Int(number) => output.push_str(&number.to_string()),
        HookValue::Float(number) => output.push_str(&js_number(*number)),
        HookValue::Str(text) => write_hook_string(text, output),
        HookValue::Array(items) => {
            output.push('[');
            for (index, item) in items.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                encode_hook_value(item, output);
            }
            output.push(']');
        }
        HookValue::Object(items) => {
            output.push('{');
            for (index, (key, item)) in items.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write_hook_string(key, output);
                output.push(':');
                encode_hook_value(item, output);
            }
            output.push('}');
        }
    }
}

fn js_number(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    let mut buffer = ryu::Buffer::new();
    let shortest = buffer.format_finite(value);
    let negative = shortest.starts_with('-');
    let unsigned = shortest
        .trim_start_matches('-')
        .strip_suffix(".0")
        .unwrap_or_else(|| shortest.trim_start_matches('-'));
    let (coefficient, explicit_exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or((unsigned, 0), |(coefficient, exponent)| {
            (coefficient, exponent.parse::<i32>().unwrap_or_default())
        });
    let digits = coefficient.replace('.', "");
    let decimal_index = i32::try_from(coefficient.find('.').unwrap_or(coefficient.len()))
        .unwrap_or_default()
        + explicit_exponent;
    let exponent = decimal_index - 1;
    let sign = if negative { "-" } else { "" };
    if (-6..21).contains(&exponent) {
        if decimal_index <= 0 {
            return format!(
                "{sign}0.{}{}",
                "0".repeat(decimal_index.unsigned_abs() as usize),
                digits
            );
        }
        let decimal_index = usize::try_from(decimal_index).unwrap_or_default();
        if decimal_index >= digits.len() {
            return format!("{sign}{digits}{}", "0".repeat(decimal_index - digits.len()));
        }
        return format!(
            "{sign}{}.{}",
            &digits[..decimal_index],
            &digits[decimal_index..]
        );
    }
    let tail = if digits.len() == 1 {
        String::new()
    } else {
        format!(".{}", &digits[1..])
    };
    format!(
        "{sign}{}{tail}e{}{exponent}",
        &digits[..1],
        if exponent >= 0 { "+" } else { "" }
    )
}

fn validate_json(value: &HookValue, depth: usize) -> Result<(), HookFailure> {
    match value {
        HookValue::Float(number) if !number.is_finite() => Err(HookFailure::new("json-number")),
        HookValue::Array(items) => {
            if depth >= JSON_DEPTH_LIMIT {
                return Err(HookFailure::new("json-depth"));
            }
            for item in items {
                validate_json(item, depth + 1)?;
            }
            Ok(())
        }
        HookValue::Object(items) => {
            if depth >= JSON_DEPTH_LIMIT {
                return Err(HookFailure::new("json-depth"));
            }
            let mut keys = HashSet::with_capacity(items.len());
            for (key, item) in items {
                if !keys.insert(key)
                    || ["__proto__", "constructor", "prototype"]
                        .iter()
                        .any(|value| key.equals(value))
                {
                    return Err(HookFailure::new("json-key"));
                }
                validate_json(item, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn field<'a>(object: &'a [(HookText, HookValue)], name: &str) -> Option<&'a HookValue> {
    object
        .iter()
        .find_map(|(key, value)| key.equals(name).then_some(value))
}

const fn is_one(value: &HookValue) -> bool {
    matches!(value, HookValue::Int(1))
        || matches!(value, HookValue::Float(number) if number.to_bits() == 1.0_f64.to_bits())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn runtime_snapshots_packaged_bytes_and_validates_node() {
        let runtime =
            HookRuntime::new(Some(PathBuf::from("/opt/node/bin/node")), None).expect("runtime");
        assert_eq!(
            runtime
                .resources()
                .iter()
                .map(BundleEntry::path)
                .collect::<Vec<_>>(),
            ["runner.mts", "bridge.mts", "process.mts"]
        );
        assert!(
            HookRuntime::new(Some(PathBuf::from("node")), None)
                .unwrap_err()
                .to_string()
                .contains("absolute Node")
        );
        assert!(HookRuntime::new(Some(PathBuf::from("/")), None).is_err());
    }

    #[test]
    fn wire_records_are_closed_bounded_and_canonical() {
        let request = HookRequest::parse(
            br#"{"version":1,"event":"before-tool","tool":"Bash","input":{"command":"pwd"},"context":{}}"#,
        )
        .expect("request");
        assert_eq!(
            request.to_bytes(),
            br#"{"version":1,"event":"before-tool","context":{},"tool":"Bash","input":{"command":"pwd"}}"#
        );
        assert_eq!(
            HookRequest::parse(br#"{"version":1,"event":"stop","context":{},"extra":1}"#)
                .unwrap_err()
                .to_string(),
            "flyrail-hook:input-fields"
        );
        assert_eq!(
            HookJson::parse(br#"{"__proto__":{}}"#)
                .unwrap_err()
                .to_string(),
            "flyrail-hook:json-key"
        );
    }

    #[test]
    fn output_validation_matches_runtime_codes() {
        let declared = [
            HookOutcome::Continue,
            HookOutcome::Block,
            HookOutcome::ModifyInput,
        ];
        let response = HookResponse::parse(
            br#"{"version":1,"outcome":"block","reason":"stop"}"#,
            HookEvent::BeforeTool,
            &declared,
        )
        .expect("response");
        assert_eq!(response.outcome(), HookOutcome::Block);
        assert_eq!(
            HookResponse::parse(
                br#"{"version":1,"outcome":"block"}"#,
                HookEvent::BeforeTool,
                &declared
            )
            .unwrap_err()
            .to_string(),
            "flyrail-hook:output-fields"
        );
    }

    #[test]
    fn hook_json_matches_javascript_number_bytes() {
        for (source, expected) in [
            ("9007199254740991", "9007199254740991"),
            ("9007199254740992", "9007199254740992"),
            ("9007199254740993", "9007199254740992"),
            ("100000000000000000000", "100000000000000000000"),
            ("1e21", "1e+21"),
            ("5e-324", "5e-324"),
            ("-0", "0"),
            ("-0.0", "0"),
        ] {
            assert_eq!(
                HookJson::parse(source.as_bytes())
                    .expect("number")
                    .to_bytes(),
                expected.as_bytes(),
                "{source}"
            );
        }
    }

    #[test]
    fn hook_json_constructors_enforce_depth_and_encoded_size() {
        let exact = format!("\"{}\"", "x".repeat(INPUT_LIMIT - 2));
        assert_eq!(
            HookJson::parse(exact.as_bytes())
                .expect("exact parse")
                .to_bytes()
                .len(),
            INPUT_LIMIT
        );
        assert_eq!(
            HookJson::string("x".repeat(INPUT_LIMIT - 2))
                .expect("exact string")
                .to_bytes()
                .len(),
            INPUT_LIMIT
        );
        assert_eq!(
            HookJson::parse(format!("\"{}\"", "x".repeat(INPUT_LIMIT - 1)).as_bytes())
                .unwrap_err()
                .code(),
            "input-limit"
        );
        assert_eq!(
            HookJson::string("x".repeat(INPUT_LIMIT - 1))
                .unwrap_err()
                .code(),
            "input-limit"
        );

        let mut parsed = b"0".to_vec();
        for _ in 0..JSON_DEPTH_LIMIT {
            parsed.insert(0, b'[');
            parsed.push(b']');
        }
        HookJson::parse(&parsed).expect("depth 64 parse");
        parsed.insert(0, b'[');
        parsed.push(b']');
        assert_eq!(HookJson::parse(&parsed).unwrap_err().code(), "json-depth");

        let mut constructed = HookJson::null().expect("null");
        for _ in 0..JSON_DEPTH_LIMIT {
            constructed = HookJson::array([constructed]).expect("depth 64 constructor");
        }
        assert_eq!(
            HookJson::array([constructed]).unwrap_err().code(),
            "json-depth"
        );
    }

    #[test]
    fn hook_json_deep_inputs_are_bounded_before_recursive_parsing() {
        let result = std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(|| {
                for depth in [65, 513] {
                    let source = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
                    assert_eq!(
                        HookJson::parse(source.as_bytes()).unwrap_err().code(),
                        "json-depth"
                    );
                    let request = format!(
                        "{{\"version\":1,\"event\":\"stop\",\"context\":{{\"value\":{source}}}}}"
                    );
                    assert_eq!(
                        HookRequest::parse(request.as_bytes()).unwrap_err().code(),
                        "json-depth"
                    );
                }
                let prefix = "[".repeat(513);
                let suffix = "]".repeat(513);
                let padding = " ".repeat(INPUT_LIMIT - prefix.len() - suffix.len() - 1);
                let source = format!("{prefix}{padding}0{suffix}");
                assert_eq!(source.len(), INPUT_LIMIT);
                assert_eq!(
                    HookJson::parse(source.as_bytes()).unwrap_err().code(),
                    "json-depth"
                );
                assert_eq!(
                    HookJson::parse(
                        source
                            .as_bytes()
                            .get(..source.len() - 1)
                            .expect("truncated deep JSON"),
                    )
                    .unwrap_err()
                    .code(),
                    "invalid-json"
                );
            })
            .expect("spawn small-stack parser test");
        result.join().expect("small-stack parser test");
    }

    #[test]
    fn hook_json_semantic_errors_follow_token_order_after_syntax() {
        let deep = format!("{}0{}", "[".repeat(65), "]".repeat(65));
        for (source, code) in [
            (format!(r#"{{"__proto__":0,"deep":{deep}}}"#), "json-key"),
            (format!(r#"{{"value":1e400,"deep":{deep}}}"#), "json-number"),
            (format!(r#"{{"a":0,"a":1,"deep":{deep}}}"#), "json-key"),
            (format!(r#"{{"deep":{deep},"__proto__":0}}"#), "json-depth"),
            (format!(r#"{{"deep":{deep},"value":1e400}}"#), "json-depth"),
            (format!(r#"{{"deep":{deep},"a":0,"a":1}}"#), "json-depth"),
        ] {
            assert_eq!(
                HookJson::parse(source.as_bytes()).unwrap_err().code(),
                code,
                "{source}"
            );
        }
        let malformed = format!(r#"{{"__proto__":0,"deep":{deep}"#);
        assert_eq!(
            HookJson::parse(malformed.as_bytes()).unwrap_err().code(),
            "invalid-json"
        );
    }

    #[test]
    fn hook_json_preserves_javascript_utf16_strings() {
        for (source, expected) in [
            (r#""\ud800""#, r#""\ud800""#),
            (r#""\udfff""#, r#""\udfff""#),
            (r#""\ud83d\ude00""#, "\"😀\""),
            (r#"{"\ud800":"\udfff"}"#, r#"{"\ud800":"\udfff"}"#),
        ] {
            let value = HookJson::parse(source.as_bytes()).expect("JavaScript JSON string");
            assert_eq!(value.to_bytes(), expected.as_bytes(), "{source}");
        }
        assert!(
            HookJson::parse(br#""\ud800""#)
                .expect("lone surrogate")
                .as_str()
                .is_none()
        );
        assert_eq!(
            HookJson::parse(br#"{"\ud800":1,"\uD800":2}"#)
                .unwrap_err()
                .code(),
            "json-key"
        );
        assert_eq!(
            HookJson::parse(br#"{"\u0063onstructor":1}"#)
                .unwrap_err()
                .code(),
            "json-key"
        );
    }

    #[test]
    fn typed_packets_preserve_javascript_utf16_text() {
        for (escaped, units) in [
            (r"\ud800", vec![0xd800]),
            (r"\udfff", vec![0xdfff]),
            (r"\ud83d\ude00", vec![0xd83d, 0xde00]),
        ] {
            let canonical = if escaped == r"\ud83d\ude00" {
                "😀"
            } else {
                escaped
            };
            let prompt =
                format!(r#"{{"version":1,"event":"prompt","context":{{}},"input":"{escaped}"}}"#);
            let request = HookRequest::parse(prompt.as_bytes()).expect("prompt request");
            assert_eq!(
                request
                    .input()
                    .and_then(HookJson::as_text)
                    .expect("prompt text")
                    .units(),
                units
            );
            assert_eq!(
                request.to_bytes(),
                format!(r#"{{"version":1,"event":"prompt","context":{{}},"input":"{canonical}"}}"#)
                    .as_bytes()
            );

            let tool = format!(
                r#"{{"version":1,"event":"before-tool","context":{{}},"tool":"{escaped}","input":{{}}}}"#
            );
            let request = HookRequest::parse(tool.as_bytes()).expect("tool request");
            assert_eq!(request.tool().expect("tool").units(), units);
            assert_eq!(
                request.to_bytes(),
                format!(
                    r#"{{"version":1,"event":"before-tool","context":{{}},"tool":"{canonical}","input":{{}}}}"#
                )
                .as_bytes()
            );

            for (outcome, field) in [("block", "reason"), ("ask", "reason"), ("context", "text")] {
                let response =
                    format!(r#"{{"version":1,"outcome":"{outcome}","{field}":"{escaped}"}}"#);
                let parsed = HookResponse::parse(
                    response.as_bytes(),
                    HookEvent::BeforeTool,
                    &[HookOutcome::Block, HookOutcome::Ask, HookOutcome::Context],
                )
                .expect("response");
                assert_eq!(
                    parsed
                        .value()
                        .and_then(HookJson::as_text)
                        .expect("response text")
                        .units(),
                    units
                );
                assert_eq!(
                    parsed.to_bytes(),
                    format!(r#"{{"version":1,"outcome":"{outcome}","{field}":"{canonical}"}}"#)
                        .as_bytes()
                );
            }

            let effects =
                format!(r#"{{"contexts":["{escaped}"],"decision":"block","reason":"{escaped}"}}"#);
            let parsed = HookEffects::parse(effects.as_bytes()).expect("effects");
            assert_eq!(parsed.contexts().first().expect("context").units(), units);
            assert_eq!(parsed.reason().expect("reason").units(), units);
            assert_eq!(
                parsed.to_bytes(),
                format!(
                    r#"{{"contexts":["{canonical}"],"decision":"block","reason":"{canonical}"}}"#
                )
                .as_bytes()
            );
        }
    }

    #[test]
    fn hook_text_uses_javascript_trim_and_text_encoder_size() {
        assert_eq!(
            HookResponse::context(HookText::from_utf16([0xfeff, 0x3000]))
                .unwrap_err()
                .code(),
            "context"
        );
        let exact = HookText::from_utf16(vec![0xd800; TEXT_LIMIT / 3]);
        HookResponse::context(exact).expect("replacement-sized text");
        let over = HookText::from_utf16(vec![0xd800; TEXT_LIMIT / 3 + 1]);
        assert_eq!(HookResponse::context(over).unwrap_err().code(), "context");
    }

    #[test]
    fn assembled_packets_are_revalidated() {
        let empty_context =
            HookJson::object([("data", HookJson::string("").expect("string"))]).expect("context");
        let baseline = HookRequest::new(HookEvent::Stop, empty_context, None, None, None)
            .expect("baseline request")
            .to_bytes()
            .len();
        let context = HookJson::object([(
            "data",
            HookJson::string("x".repeat(INPUT_LIMIT - baseline)).expect("exact string"),
        )])
        .expect("exact context");
        assert_eq!(
            HookRequest::new(HookEvent::Stop, context, None, None, None)
                .expect("exact request")
                .to_bytes()
                .len(),
            INPUT_LIMIT
        );
        let context = HookJson::object([(
            "data",
            HookJson::string("x".repeat(INPUT_LIMIT - baseline + 1)).expect("over string"),
        )])
        .expect("over context");
        assert_eq!(
            HookRequest::new(HookEvent::Stop, context, None, None, None)
                .unwrap_err()
                .code(),
            "input-limit"
        );

        let empty_replacement =
            HookJson::object([("data", HookJson::string("").expect("string"))]).expect("input");
        let baseline = HookResponse::modify_input(HookEvent::BeforeTool, empty_replacement)
            .expect("baseline response")
            .to_bytes()
            .len();
        let replacement = HookJson::object([(
            "data",
            HookJson::string("x".repeat(INPUT_LIMIT - baseline)).expect("exact string"),
        )])
        .expect("exact replacement");
        assert_eq!(
            HookResponse::modify_input(HookEvent::BeforeTool, replacement)
                .expect("exact response")
                .to_bytes()
                .len(),
            INPUT_LIMIT
        );
        let replacement = HookJson::object([(
            "data",
            HookJson::string("x".repeat(INPUT_LIMIT - baseline + 1)).expect("over string"),
        )])
        .expect("over replacement");
        assert_eq!(
            HookResponse::modify_input(HookEvent::BeforeTool, replacement)
                .unwrap_err()
                .code(),
            "input-limit"
        );

        let mut context = HookJson::null().expect("null");
        for _ in 0..JSON_DEPTH_LIMIT - 1 {
            context = HookJson::array([context]).expect("context depth");
        }
        let context = HookJson::object([("nested", context)]).expect("depth 64 context");
        assert_eq!(
            HookRequest::new(HookEvent::Stop, context, None, None, None)
                .unwrap_err()
                .code(),
            "json-depth"
        );
        let mut replacement = HookJson::null().expect("null");
        for _ in 0..JSON_DEPTH_LIMIT - 1 {
            replacement = HookJson::array([replacement]).expect("replacement depth");
        }
        let replacement =
            HookJson::object([("nested", replacement)]).expect("depth 64 replacement");
        assert_eq!(
            HookResponse::modify_input(HookEvent::BeforeTool, replacement)
                .unwrap_err()
                .code(),
            "json-depth"
        );
    }

    #[test]
    fn canonical_records_match_authoritative_insertion_order() {
        let request = HookRequest::new(
            HookEvent::AfterTool,
            HookJson::object([("session", HookJson::string("s").expect("string"))])
                .expect("context"),
            Some("Bash".to_owned()),
            Some(
                HookJson::object([("command", HookJson::string("pwd").expect("string"))])
                    .expect("input"),
            ),
            Some(HookJson::boolean(true).expect("boolean")),
        )
        .expect("request");
        assert_eq!(
            request.to_bytes(),
            br#"{"version":1,"event":"after-tool","context":{"session":"s"},"tool":"Bash","input":{"command":"pwd"},"result":true}"#
        );

        let effects = HookEffects::parse(
            br#"{"input":{"command":"changed"},"contexts":["notice"],"decision":"ask","reason":"why"}"#,
        )
        .expect("effects");
        assert_eq!(
            effects.to_bytes(),
            br#"{"contexts":["notice"],"input":{"command":"changed"},"decision":"ask","reason":"why"}"#
        );

        let handler = HookHandler::new(
            "handler",
            HookEvent::BeforeTool,
            ["node".to_owned(), "handler.mjs".to_owned()],
            [(
                "TOKEN".to_owned(),
                HookEnvironment::Text("value".to_owned()),
            )],
            "/work",
            1000,
            [HookOutcome::Continue],
            ["Bash".to_owned()],
        )
        .expect("handler");
        let config = HookConfig::new("test", [handler]).expect("config");
        assert_eq!(
            config.to_bytes(),
            br#"{"host":"test","handlers":[{"id":"handler","event":"before-tool","argv":["node","handler.mjs"],"cwd":"/work","env":{"TOKEN":"value"},"timeout_ms":1000,"outcomes":["continue"],"tools":["Bash"]}]}"#
        );
    }
}
