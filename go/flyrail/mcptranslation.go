package flyrail

import (
	"strings"

	"github.com/nielsmadan/flyrail/go/internal/pathx"
)

func hasCopilotEnvSubstitution(value string) bool {
	for index := 0; index+1 < len(value); index++ {
		next := value[index+1]
		if value[index] == '$' && (next >= 'A' && next <= 'Z' || next >= 'a' && next <= 'z' || next == '_') {
			return true
		}
	}
	return false
}

func literal(value string, rc RenderContext, fieldName string) *UnsupportedTranslationError {
	agent := rc.audience.agent
	var substitution bool
	switch agent {
	case AgentOpencode:
		substitution = strings.Contains(value, "{env:") || strings.Contains(value, "{file:")
	case AgentPi:
		substitution = strings.Contains(value, "${") || strings.Contains(value, "$env:") || strings.Contains(value, "{env:")
	case AgentCodex:
		substitution = false
	default:
		substitution = strings.Contains(value, "${")
	}
	copilotCLI := agent == AgentCopilot && rc.surface == SurfaceCLI
	if !substitution && (!copilotCLI || fieldName != "env" || !hasCopilotEnvSubstitution(value)) {
		return nil
	}
	if copilotCLI && fieldName != "env" {
		message := "Literal substitution-like syntax is unverified for Copilot CLI."
		switch fieldName {
		case "argv", "cwd", "url", "headers":
			message = "Literal " + fieldName + " substitution-like syntax is unverified for Copilot CLI."
		}
		return unsupportedTranslation("literal-interpolation-unverified", message)
	}
	message := "value contains host interpolation syntax; use native content."
	switch fieldName {
	case "argv", "env", "cwd", "url", "headers":
		message = fieldName + " contains host interpolation syntax; use native content."
	}
	return unsupportedTranslation("literal-interpolation", message)
}

func reference(value EnvRef, rc RenderContext, fieldName string) (string, *UnsupportedTranslationError) {
	agent := rc.audience.agent
	if agent == AgentClaude || agent == AgentCopilot && rc.surface == SurfaceCLI && fieldName == "env" {
		return "${" + value.name + "}", nil
	}
	if agent == AgentOpencode {
		return "{env:" + value.name + "}", nil
	}
	if agent == AgentCursor || rc.surface == SurfaceVSCode {
		return "${env:" + value.name + "}", nil
	}
	return "", unsupportedTranslation("portable-env-reference-unverified", "This host cannot represent this environment reference.")
}

func assetArgument(value CommandArgument, assets map[string]string) (string, error) {
	switch argument := value.(type) {
	case ArgText:
		return string(argument), nil
	case AssetRef:
		root, ok := assets[argument.assetID]
		if !ok {
			return "", newValueError("unknown translated asset reference")
		}
		if argument.path == "" {
			return displayPath(root), nil
		}
		return displayPath(pathx.Join(root, argument.path)), nil
	}
	return "", newValueError("command arguments must be ArgText or AssetRef values")
}

func textValue(text string) Value {
	return String{text: text}
}

func textArray(values []string) Value {
	items := make([]Value, len(values))
	for index, value := range values {
		items[index] = textValue(value)
	}
	return modelArray(items)
}

func objectValue(members ...ObjectMember) (Value, error) {
	return NewObject(members...)
}

type translationFailure struct {
	unsupported *UnsupportedTranslationError
	invalid     error
}

func unsupportedFailure(err *UnsupportedTranslationError) *translationFailure {
	return &translationFailure{unsupported: err}
}

func invalidFailure(err error) *translationFailure {
	return &translationFailure{invalid: err}
}

func stdio(command Command, rc RenderContext, assets map[string]string) (Value, *translationFailure) {
	agent := rc.audience.agent
	if rc.platform == PlatformWindows {
		names := make(map[string]struct{}, len(command.env))
		for _, variable := range command.env {
			names[strings.ToLower(variable.name)] = struct{}{}
		}
		if len(names) != len(command.env) {
			return nil, unsupportedFailure(unsupportedTranslation("windows-env-collision",
				"Environment names collide under Windows case-insensitive lookup."))
		}
	}
	argv := make([]string, len(command.argv))
	for index, argument := range command.argv {
		text, err := assetArgument(argument, assets)
		if err != nil {
			return nil, invalidFailure(err)
		}
		argv[index] = text
	}
	for index, argument := range argv {
		if agent != AgentPi || index != 0 {
			if failure := literal(argument, rc, "argv"); failure != nil {
				return nil, unsupportedFailure(failure)
			}
		}
	}
	environment := make([]ObjectMember, 0, len(command.env))
	var forwarded []string
	for _, variable := range command.env {
		switch value := variable.value.(type) {
		case EnvRef:
			if agent == AgentCodex {
				if variable.name != value.name {
					return nil, unsupportedFailure(unsupportedTranslation("environment-alias",
						"Codex env_vars forwards a variable under its own name."))
				}
				forwarded = append(forwarded, variable.name)
				continue
			}
			text, failure := reference(value, rc, "env")
			if failure != nil {
				return nil, unsupportedFailure(failure)
			}
			environment = append(environment, field(variable.name, textValue(text)))
		case EnvText:
			if agent != AgentPi {
				if failure := literal(string(value), rc, "env"); failure != nil {
					return nil, unsupportedFailure(failure)
				}
			}
			environment = append(environment, field(variable.name, textValue(string(value))))
		}
	}
	env, err := objectValue(environment...)
	if err != nil {
		return nil, invalidFailure(err)
	}
	var record []ObjectMember
	if agent == AgentOpencode {
		record = []ObjectMember{
			field("type", textValue("local")),
			field("command", textArray(argv)),
			field("environment", env),
		}
	} else {
		if len(argv) == 0 {
			return nil, invalidFailure(newValueError("argv requires a command"))
		}
		record = []ObjectMember{
			field("command", textValue(argv[0])),
			field("args", textArray(argv[1:])),
			field("env", env),
		}
	}
	if agent == AgentCursor || rc.surface == SurfaceVSCode {
		record = append(record, field("type", textValue("stdio")))
	}
	if agent == AgentPi {
		record = append(record, field("literalEnv", Bool(true)))
	}
	if len(forwarded) > 0 {
		record = append(record, field("env_vars", textArray(forwarded)))
	}
	if command.cwd != nil {
		if agent == AgentClaude || agent == AgentCursor {
			return nil, unsupportedFailure(unsupportedTranslation("command-cwd", "The selected MCP schema cannot honor cwd."))
		}
		var cwd string
		switch value := command.cwd.(type) {
		case ArgText:
			cwd = displayPath(pathx.Join(rc.executionDirectory(), string(value)))
		default:
			text, err := assetArgument(value, assets)
			if err != nil {
				return nil, invalidFailure(err)
			}
			cwd = text
		}
		if failure := literal(cwd, rc, "cwd"); failure != nil {
			return nil, unsupportedFailure(failure)
		}
		record = append(record, field("cwd", textValue(cwd)))
	}
	value, err := objectValue(record...)
	if err != nil {
		return nil, invalidFailure(err)
	}
	return value, nil
}

func httpRecord(transport HTTPTransport, rc RenderContext) (Value, *translationFailure) {
	agent := rc.audience.agent
	if failure := literal(transport.url, rc, "url"); failure != nil {
		return nil, unsupportedFailure(failure)
	}
	record := []ObjectMember{field("url", textValue(transport.url))}
	switch agent {
	case AgentOpencode:
		record = append(record, field("type", textValue("remote")))
	case AgentCodex, AgentPi:
	default:
		record = append(record, field("type", textValue("http")))
	}
	if token, ok := transport.BearerToken(); ok {
		switch {
		case agent == AgentCodex:
			record = append(record, field("bearer_token_env_var", textValue(token.name)))
		case agent == AgentPi:
			record = append(record, field("auth", textValue("bearer")), field("bearerTokenEnv", textValue(token.name)))
		case agent == AgentCopilot && rc.surface == SurfaceCLI:
			return nil, unsupportedFailure(unsupportedTranslation("portable-auth-env-unverified",
				"Copilot CLI bearer environment syntax is unverified."))
		default:
			text, failure := reference(token, rc, "headers")
			if failure != nil {
				return nil, unsupportedFailure(failure)
			}
			headers, err := objectValue(field("Authorization", textValue("Bearer "+text)))
			if err != nil {
				return nil, invalidFailure(err)
			}
			record = append(record, field("headers", headers))
		}
	}
	value, err := objectValue(record...)
	if err != nil {
		return nil, invalidFailure(err)
	}
	return value, nil
}

func mcpContent(artifact MCPArtifact, rc RenderContext, path string, assets map[string]string) (StructuredContent, *translationFailure) {
	agent := rc.audience.agent
	if agent == AgentClaude {
		switch artifact.name {
		case "workspace", "claude-in-chrome", "computer-use":
			return StructuredContent{}, unsupportedFailure(unsupportedTranslation("reserved-server-name",
				"Claude reserves this MCP server name."))
		}
	}
	var value Value
	var failure *translationFailure
	switch transport := artifact.transport.(type) {
	case Command:
		value, failure = stdio(transport, rc, assets)
	case HTTPTransport:
		value, failure = httpRecord(transport, rc)
	default:
		failure = invalidFailure(newValueError("MCP transport must be a Command or HTTPTransport"))
	}
	if failure != nil {
		return StructuredContent{}, failure
	}
	extension, hasExtension := pathExtension(path)
	format := FormatJSON
	switch {
	case agent == AgentCodex:
		format = FormatTOML
	case agent == AgentOpencode || rc.surface == SurfaceVSCode || hasExtension && extension == "jsonc":
		format = FormatJSONC
	}
	key := "mcpServers"
	switch {
	case agent == AgentCodex:
		key = "mcp_servers"
	case agent == AgentOpencode:
		key = "mcp"
	case rc.surface == SurfaceVSCode:
		key = "servers"
	}
	outer, err := NewKey(key)
	if err != nil {
		return StructuredContent{}, invalidFailure(err)
	}
	inner, err := NewKey(artifact.name)
	if err != nil {
		return StructuredContent{}, invalidFailure(err)
	}
	selector, err := NewSelector(outer, inner)
	if err != nil {
		return StructuredContent{}, invalidFailure(err)
	}
	content, err := NewStructuredContent(format, selector, value, nil)
	if err != nil {
		return StructuredContent{}, invalidFailure(err)
	}
	return content, nil
}
