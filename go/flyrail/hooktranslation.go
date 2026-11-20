package flyrail

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
)

const hookLauncherTimeoutSeconds = 310

type hookTranslation struct {
	artifacts    []RenderedArtifact
	dependencies []Dependency
	notices      []Notice
}

func fileURL(path string, platform Platform) (string, error) {
	if !utf8.ValidString(path) {
		return "", newValueError("hook import path must be UTF-8")
	}
	normalized := path
	if platform == PlatformWindows {
		normalized = strings.ReplaceAll(path, `\`, "/")
	}
	driveLetter := len(normalized) > 1 && normalized[1] == ':' &&
		(normalized[0] >= 'A' && normalized[0] <= 'Z' || normalized[0] >= 'a' && normalized[0] <= 'z')
	var prefix string
	switch {
	case platform == PlatformWindows && strings.HasPrefix(normalized, "//"):
		prefix = "file:"
	case platform == PlatformWindows && driveLetter:
		prefix = "file:///"
	case strings.HasPrefix(normalized, "/"):
		prefix = "file://"
	default:
		return "", newValueError("hook import path must be absolute")
	}
	var encoded strings.Builder
	encoded.WriteString(prefix)
	for index := range len(normalized) {
		b := normalized[index]
		driveColon := platform == PlatformWindows && index == 1 && b == ':'
		if b >= 'A' && b <= 'Z' || b >= 'a' && b <= 'z' || b >= '0' && b <= '9' || strings.IndexByte("/-._~", b) >= 0 || driveColon {
			encoded.WriteByte(b)
		} else {
			fmt.Fprintf(&encoded, "%%%02X", b)
		}
	}
	return encoded.String(), nil
}

func supportAssetArtifacts(b *Bundle, rc RenderContext) ([]RenderedArtifact, map[string]string, error) {
	root := rc.assetDirectory()
	rendered := make([]RenderedArtifact, 0, len(b.assets))
	destinations := make(map[string]string, len(b.assets))
	for _, asset := range b.assets {
		semantic, err := EncodeValue(asset.content.modelValue())
		if err != nil {
			return nil, nil, err
		}
		sum := sha256.Sum256(semantic)
		destination := pathx.Join(root, b.ID(), asset.id, hex.EncodeToString(sum[:]))
		destinations[asset.id] = destination
		artifact, err := NewRenderedArtifact(RenderedArtifactParams{ID: asset.id, Family: asset.family, Destination: destination, Content: asset.content})
		if err != nil {
			return nil, nil, err
		}
		rendered = append(rendered, artifact)
	}
	return rendered, destinations, nil
}

func translateNativeArtifact(artifact NativeArtifact, rc RenderContext) (RenderedArtifact, bool, error) {
	audience := artifact.audience
	if audience.agent != rc.audience.agent || audience.scope != rc.target.scope || audience.surface != rc.surface ||
		audience.platform != "" && audience.platform != rc.platform {
		return RenderedArtifact{}, false, nil
	}
	rendered, err := NewRenderedArtifact(RenderedArtifactParams{
		ID:          artifact.id,
		Family:      artifact.family,
		Destination: pathx.Join(rc.nativeDirectory(), artifact.destination),
		Content:     artifact.content,
	})
	if err != nil {
		return RenderedArtifact{}, false, err
	}
	return rendered, true, nil
}

func supportedHook(artifact HookArtifact, rc RenderContext) *UnsupportedTranslationError {
	agent := rc.audience.agent
	if failure := requireTranslatable(rc); failure != nil {
		return failure
	}
	if rc.surface == SurfaceVSCode {
		return unsupportedTranslation("hook-surface", "Portable hooks target Copilot CLI, not VS Code.")
	}
	if artifact.event == EventBeforeTool && slices.Contains(artifact.outcomes, OutcomeModifyInput) &&
		(agent == AgentCodex || agent == AgentCursor) {
		return unsupportedTranslation("hook-modification-approval",
			"This host has no verified argument-replacement channel preserving normal approval flow.")
	}
	outcomes := []HookOutcome{OutcomeContinue}
	switch artifact.event {
	case EventBeforeTool:
		outcomes = append(outcomes, OutcomeBlock, OutcomeModifyInput)
		if agent == AgentClaude || agent == AgentCopilot {
			outcomes = append(outcomes, OutcomeAsk)
		}
		if agent == AgentClaude || agent == AgentCodex {
			outcomes = append(outcomes, OutcomeContext)
		}
	case EventAfterTool:
		outcomes = append(outcomes, OutcomeContext)
	case EventSessionStart:
		if agent != AgentOpencode && agent != AgentPi {
			outcomes = append(outcomes, OutcomeContext)
		}
	case EventPrompt:
		if agent == AgentOpencode {
			outcomes = nil
		}
		if agent == AgentClaude || agent == AgentCodex || agent == AgentCursor {
			outcomes = append(outcomes, OutcomeBlock)
		}
		if agent == AgentClaude || agent == AgentCodex {
			outcomes = append(outcomes, OutcomeContext)
		}
		if agent == AgentPi {
			outcomes = append(outcomes, OutcomeModifyInput)
		}
	case EventStop:
		if agent == AgentOpencode {
			outcomes = nil
		}
	}
	var unsupported []string
	for _, outcome := range artifact.outcomes {
		if !slices.Contains(outcomes, outcome) {
			unsupported = append(unsupported, string(outcome))
		}
	}
	if len(unsupported) > 0 {
		return unsupportedTranslation("hook-outcome", fmt.Sprintf("%s cannot preserve the declared %s outcomes: %s",
			agent, artifact.event, strings.Join(unsupported, ", ")))
	}
	if rc.platform == PlatformWindows {
		names := make(map[string]struct{}, len(artifact.command.env))
		for _, variable := range artifact.command.env {
			names[strings.ToLower(variable.name)] = struct{}{}
		}
		if len(names) != len(artifact.command.env) {
			return unsupportedTranslation("windows-env-collision", "Hook environment names collide on Windows.")
		}
	}
	return nil
}

func isPOSIXShellSafe(value string) bool {
	safe := func(r rune) bool {
		return r >= 'A' && r <= 'Z' || r >= 'a' && r <= 'z' || r >= '0' && r <= '9' || strings.ContainsRune("_@%+=:,./-", r)
	}
	return value != "" && !strings.ContainsFunc(value, func(r rune) bool { return !safe(r) })
}

func hookShellCommand(argv []string, rc RenderContext) (string, *UnsupportedTranslationError) {
	runtime := rc.hookRuntime
	if runtime == nil {
		return "", unsupportedTranslation("hook-runtime", "Native hooks require an explicit Node 24+ executable.")
	}
	quoted := make([]string, len(argv))
	if rc.platform == PlatformWindows {
		if rc.audience.agent == AgentCursor {
			return "", unsupportedTranslation("cursor-windows-shell", "Cursor Windows hook shell selection is unverified; use native content.")
		}
		if runtime.shell != ShellCmd {
			return "", unsupportedTranslation("hook-shell", "Windows Codex requires a caller-confirmed cmd.exe /C hook shell.")
		}
		if slices.ContainsFunc(argv, func(value string) bool { return strings.ContainsAny(value, "%!&|<>^()\"\r\n") }) {
			return "", unsupportedTranslation("hook-shell-literal", "These native launcher paths cannot be represented by the cmd hook bridge.")
		}
		for index, value := range argv {
			quoted[index] = `"` + value + `"`
		}
		return strings.Join(quoted, " "), nil
	}
	if runtime.shell != ShellPOSIX {
		return "", unsupportedTranslation("hook-shell", "This native hook requires a caller-confirmed POSIX-compatible hook shell.")
	}
	for index, value := range argv {
		if isPOSIXShellSafe(value) {
			quoted[index] = value
		} else {
			quoted[index] = "'" + strings.ReplaceAll(value, "'", `'"'"'`) + "'"
		}
	}
	return strings.Join(quoted, " "), nil
}

func hookEventName(agent Agent, event HookEvent) string {
	switch agent {
	case AgentClaude, AgentCodex:
		switch event {
		case EventSessionStart:
			return "SessionStart"
		case EventBeforeTool:
			return "PreToolUse"
		case EventAfterTool:
			return "PostToolUse"
		case EventPrompt:
			return "UserPromptSubmit"
		case EventStop:
			return "Stop"
		}
	case AgentCursor, AgentCopilot:
		switch event {
		case EventSessionStart:
			return "sessionStart"
		case EventBeforeTool:
			return "preToolUse"
		case EventAfterTool:
			return "postToolUse"
		case EventPrompt:
			if agent == AgentCursor {
				return "beforeSubmitPrompt"
			}
			return "userPromptSubmitted"
		case EventStop:
			if agent == AgentCursor {
				return "stop"
			}
			return "agentStop"
		}
	}
	return ""
}

func hookUnsupported(code, message string) *translationFailure {
	return unsupportedFailure(unsupportedTranslation(code, message))
}

func hookRuntimeUnsupported() *translationFailure {
	return hookUnsupported("hook-runtime", "Native hooks require an explicit Node 24+ executable.")
}

func hookHandlerOf(hook HookArtifact, rc RenderContext, assets map[string]string) (HookHandler, error) {
	argv := make([]string, len(hook.command.argv))
	for index, argument := range hook.command.argv {
		text, err := assetArgument(argument, assets)
		if err != nil {
			return HookHandler{}, err
		}
		argv[index] = text
	}
	var cwd string
	switch value := hook.command.cwd.(type) {
	case nil:
		cwd = rc.executionDirectory()
	case ArgText:
		cwd = pathx.Join(rc.executionDirectory(), string(value))
	default:
		text, err := assetArgument(value, assets)
		if err != nil {
			return HookHandler{}, err
		}
		cwd = text
	}
	return NewHookHandler(HookHandlerParams{
		ID:            hook.id,
		Event:         hook.event,
		Argv:          argv,
		Environment:   hook.command.Env(),
		Cwd:           cwd,
		TimeoutMillis: hook.timeoutMillis,
		Outcomes:      hook.outcomes,
		Tools:         hook.tools,
	})
}

func hookConfigModule(config HookConfig) ([]byte, error) {
	parsed, err := jsonx.Parse(config.Bytes())
	if err != nil {
		return nil, valueErrorFromJSON(err)
	}
	quoted := jsonx.EncodePythonCompact(jsonx.String(string(jsonx.EncodePythonCompact(parsed))))
	return []byte("import type { Config } from './runner.mts';\nconst config: Config = JSON.parse(" + string(quoted) +
		");\nexport default config;\n"), nil
}

func hookNativeDirectory(rc RenderContext) (string, *translationFailure) {
	agent := rc.audience.agent
	if rc.target.scope == ScopeUser {
		directory, err := userDirectory(rc)
		if err != nil {
			return "", invalidFailure(err)
		}
		return directory, nil
	}
	var name string
	switch agent {
	case AgentClaude:
		name = ".claude"
	case AgentCodex:
		name = ".codex"
	case AgentCursor:
		name = ".cursor"
	case AgentCopilot:
		name = ".github"
	case AgentOpencode:
		name = ".opencode"
	case AgentPi:
		name = ".pi"
	default:
		return "", hookUnsupported("agent-unsupported", UntranslatableMessage(agent))
	}
	return pathx.Join(rc.Root(), name), nil
}

func hookArtifacts(b *Bundle, rc RenderContext, assets map[string]string) (hookTranslation, *translationFailure) {
	return hookArtifactsWithLauncher(b, rc, assets, false)
}

func hookArtifactsWithLauncher(b *Bundle, rc RenderContext, assets map[string]string, revisionLauncher bool) (hookTranslation, *translationFailure) {
	var hooks []HookArtifact
	for _, artifact := range b.artifacts {
		if hook, ok := artifact.(HookArtifact); ok {
			hooks = append(hooks, hook)
		}
	}
	if len(hooks) == 0 {
		return hookTranslation{}, nil
	}
	for _, hook := range hooks {
		if failure := supportedHook(hook, rc); failure != nil {
			return hookTranslation{}, unsupportedFailure(failure)
		}
	}
	for _, event := range hookEvents {
		count, timeout := 0, 0
		for _, hook := range hooks {
			if hook.event == event {
				count++
				timeout += hook.timeoutMillis
			}
		}
		if count > HookHandlerLimit || timeout > HookEventTimeoutLimitMillis {
			return hookTranslation{}, hookUnsupported("hook-event-budget",
				"Each event permits at most 64 handlers and 300000 ms total declared timeout.")
		}
	}
	agent := rc.audience.agent
	embedded := agent == AgentOpencode || agent == AgentPi
	runtime := rc.hookRuntime
	if runtime == nil || !embedded && runtime.node == "" {
		return hookTranslation{}, hookRuntimeUnsupported()
	}
	nativeDirectory, failure := hookNativeDirectory(rc)
	if failure != nil {
		return hookTranslation{}, failure
	}
	base := pathx.Join(rc.assetDirectory(), b.ID(), ".hooks", string(agent))
	if slices.ContainsFunc(pathx.Components(base), func(part string) bool { return asciiLower(part) == "node_modules" }) {
		return hookTranslation{}, hookUnsupported("hook-node-modules",
			"Node cannot load stripped TypeScript from a node_modules asset root.")
	}
	handlers := make([]HookHandler, len(hooks))
	for index, hook := range hooks {
		handler, err := hookHandlerOf(hook, rc, assets)
		if err != nil {
			return hookTranslation{}, invalidFailure(err)
		}
		handlers[index] = handler
	}
	config, err := NewHookConfig(string(agent), handlers)
	if err != nil {
		return hookTranslation{}, invalidFailure(err)
	}
	module, err := hookConfigModule(config)
	if err != nil {
		return hookTranslation{}, invalidFailure(err)
	}
	entries := append(runtime.Resources(), BundleEntry{Path: "config.mts", Data: module})
	digest := sha256.New()
	for _, entry := range entries {
		digest.Write(entry.Data)
	}
	runtimeVersion := hex.EncodeToString(digest.Sum(nil))
	revision := pathx.Join(base, "revisions", runtimeVersion)
	runtimeID := identifier(b.ID(), string(agent), "runtime")
	launcherID := identifier(b.ID(), string(agent), "launcher")
	versionID := identifier(b.ID(), string(agent), "version")
	authored := map[string]struct{}{}
	for _, artifact := range b.artifacts {
		authored[artifact.ID()] = struct{}{}
	}
	for _, asset := range b.assets {
		authored[asset.id] = struct{}{}
	}
	for _, id := range []string{runtimeID, launcherID, versionID} {
		if _, collides := authored[id]; collides {
			return hookTranslation{}, hookUnsupported("hook-generated-id",
				"An authored identifier collides with a generated hook identifier.")
		}
	}
	tree, err := NewTreeContent(entries)
	if err != nil {
		return hookTranslation{}, invalidFailure(err)
	}
	runtimeArtifact, err := NewRenderedArtifact(RenderedArtifactParams{ID: runtimeID, Family: FamilyHooks, Destination: revision, Content: tree})
	if err != nil {
		return hookTranslation{}, invalidFailure(err)
	}
	artifacts := []RenderedArtifact{runtimeArtifact}
	var dependencies []Dependency
	for _, edge := range b.dependencies {
		if !slices.ContainsFunc(hooks, func(hook HookArtifact) bool { return hook.id == edge.dependent }) {
			continue
		}
		dependency, err := NewDependency(runtimeID, edge.required, "")
		if err != nil {
			return hookTranslation{}, invalidFailure(err)
		}
		dependencies = append(dependencies, dependency)
	}
	bridgeURL, err := fileURL(pathx.Join(revision, "bridge.mts"), rc.platform)
	if err != nil {
		return hookTranslation{}, invalidFailure(err)
	}
	configURL, err := fileURL(pathx.Join(revision, "config.mts"), rc.platform)
	if err != nil {
		return hookTranslation{}, invalidFailure(err)
	}
	bridgeText := string(jsonx.Encode(jsonx.String(bridgeURL)))
	configText := string(jsonx.Encode(jsonx.String(configURL)))
	if embedded {
		output, failure := embeddedHookArtifacts(b, hooks, nativeDirectory, runtimeID, bridgeText, configText, agent)
		if failure != nil {
			return hookTranslation{}, failure
		}
		artifacts = append(artifacts, output.artifacts...)
		dependencies = append(dependencies, output.dependencies...)
	} else {
		launcherBase, mode := base, DependencyStableReference
		if revisionLauncher {
			launcherBase, mode = pathx.Join(base, "launchers", runtimeVersion), DependencyRevision
		}
		output, failure := nativeHookArtifacts(b, hooks, rc, nativeDirectory, launcherBase, runtimeID, launcherID, bridgeText, configText, mode)
		if failure != nil {
			return hookTranslation{}, failure
		}
		artifacts = append(artifacts, output.artifacts...)
		dependencies = append(dependencies, output.dependencies...)
	}
	slices.SortStableFunc(dependencies, compareDependencies)
	dependencies = slices.Compact(dependencies)
	noticeID := hooks[0].id
	return hookTranslation{
		artifacts:    artifacts,
		dependencies: dependencies,
		notices: []Notice{
			{
				ArtifactID: noticeID,
				Kind:       NoticePrerequisite,
				Message:    "Requires Node 24+ for native/Pi bridges, Bun 1.4.2+ for OpenCode; host APIs documented in the hook protocol.",
				Code:       "hook-runtime-prerequisite",
			},
			{
				ArtifactID: noticeID,
				Kind:       NoticeActivation,
				Message:    "Installation only writes assets/configuration. Host trust, reload and actual activation remain host-controlled.",
				Code:       "hook-activation",
			},
		},
	}, nil
}

func embeddedHookArtifacts(b *Bundle, hooks []HookArtifact, nativeDirectory, runtimeID, bridgeText, configText string, agent Agent) (hookTranslation, *translationFailure) {
	directory, code := "extensions", "import { registerPi, type Pi } from "+bridgeText+";\nimport config from "+configText+
		";\nexport default function(pi: Pi) { registerPi(config, pi); }\n"
	if agent == AgentOpencode {
		directory, code = "plugins", "import { openCode, type OpenCodeInput } from "+bridgeText+";\nimport config from "+configText+
			";\nexport default async function(host: OpenCodeInput) {\n  return openCode(config, host);\n}\n"
	}
	launcher := pathx.Join(nativeDirectory, directory, "flyrail-"+identifier(b.ID())+".ts")
	var output hookTranslation
	for _, hook := range hooks {
		artifact, err := NewRenderedArtifact(RenderedArtifactParams{
			ID:          hook.id,
			Family:      FamilyHooks,
			Destination: launcher,
			Content:     FileContent{data: []byte(code), mode: ModePrivate},
		})
		if err != nil {
			return hookTranslation{}, invalidFailure(err)
		}
		dependency, err := NewDependency(hook.id, runtimeID, "")
		if err != nil {
			return hookTranslation{}, invalidFailure(err)
		}
		output.artifacts = append(output.artifacts, artifact)
		output.dependencies = append(output.dependencies, dependency)
	}
	return output, nil
}

type nativeHookEntry struct {
	event HookEvent
	value Value
}

func nativeHookArtifacts(b *Bundle, hooks []HookArtifact, rc RenderContext, nativeDirectory, base, runtimeID, launcherID, bridgeText, configText string, mode DependencyMode) (hookTranslation, *translationFailure) {
	agent := rc.audience.agent
	launcher := pathx.Join(base, "entry.mjs")
	code := "try { const { native } = await import(" + bridgeText + "); const { default: config } = await import(" + configText +
		"); await native(config, process.argv[2]); } catch { process.stderr.write('flyrail-hook:bootstrap\\n'); process.stdout.write('{}\\n'); }\n"
	launcherArtifact, err := NewRenderedArtifact(RenderedArtifactParams{
		ID:          launcherID,
		Family:      FamilyHooks,
		Destination: launcher,
		Content:     FileContent{data: []byte(code), mode: ModePrivate},
	})
	if err != nil {
		return hookTranslation{}, invalidFailure(err)
	}
	launcherDependency, err := NewDependency(launcherID, runtimeID, "")
	if err != nil {
		return hookTranslation{}, invalidFailure(err)
	}
	output := hookTranslation{artifacts: []RenderedArtifact{launcherArtifact}, dependencies: []Dependency{launcherDependency}}
	node := displayPath(rc.hookRuntime.node)
	launcherText := displayPath(launcher)
	if agent == AgentClaude && (strings.Contains(node, "${") || strings.Contains(launcherText, "${")) {
		return hookTranslation{}, hookUnsupported("hook-literal-interpolation",
			"Claude launcher paths cannot contain host placeholder syntax.")
	}
	if extension, ok := pathExtension(node); rc.platform == PlatformWindows && ok {
		if lower := strings.ToLower(extension); lower == "cmd" || lower == "bat" {
			return hookTranslation{}, hookUnsupported("hook-executable",
				"Native hooks require a real executable, not a Windows command shim.")
		}
	}
	var events []HookEvent
	for _, hook := range hooks {
		if !slices.Contains(events, hook.event) {
			events = append(events, hook.event)
		}
	}
	slices.Sort(events)
	entries := make([]nativeHookEntry, 0, len(events))
	for _, event := range events {
		argv := []string{node, launcherText, string(event)}
		value, failure := nativeHookEntryValue(agent, argv, rc)
		if failure != nil {
			return hookTranslation{}, failure
		}
		entries = append(entries, nativeHookEntry{event: event, value: value})
	}
	for _, hook := range hooks {
		index := slices.IndexFunc(entries, func(entry nativeHookEntry) bool { return entry.event == hook.event })
		if index < 0 {
			return hookTranslation{}, invalidFailure(newValueError("missing hook event"))
		}
		params, err := nativeHookParams(b, hook, agent, nativeDirectory, node, launcherText, entries, entries[index].value)
		if err != nil {
			return hookTranslation{}, invalidFailure(err)
		}
		artifact, err := NewRenderedArtifact(params)
		if err != nil {
			return hookTranslation{}, invalidFailure(err)
		}
		dependency, err := NewDependency(hook.id, launcherID, mode)
		if err != nil {
			return hookTranslation{}, invalidFailure(err)
		}
		output.artifacts = append(output.artifacts, artifact)
		output.dependencies = append(output.dependencies, dependency)
	}
	return output, nil
}

func nativeHookEntryValue(agent Agent, argv []string, rc RenderContext) (Value, *translationFailure) {
	node := argv[0]
	args := textArray(argv[1:])
	timeout := Int(hookLauncherTimeoutSeconds)
	var members []ObjectMember
	switch agent {
	case AgentCopilot:
		members = []ObjectMember{
			field("type", textValue("command")), field("exec", textValue(node)), field("args", args), field("timeoutSec", timeout),
		}
	case AgentClaude:
		inner, err := objectValue(field("type", textValue("command")), field("command", textValue(node)), field("args", args), field("timeout", timeout))
		if err != nil {
			return nil, invalidFailure(err)
		}
		members = []ObjectMember{field("hooks", modelArray([]Value{inner}))}
	case AgentCodex:
		command, failure := hookShellCommand(argv, rc)
		if failure != nil {
			return nil, unsupportedFailure(failure)
		}
		inner, err := objectValue(field("type", textValue("command")), field("command", textValue(command)), field("timeout", timeout))
		if err != nil {
			return nil, invalidFailure(err)
		}
		members = []ObjectMember{field("hooks", modelArray([]Value{inner}))}
	case AgentCursor:
		command, failure := hookShellCommand(argv, rc)
		if failure != nil {
			return nil, unsupportedFailure(failure)
		}
		members = []ObjectMember{field("command", textValue(command)), field("timeout", timeout)}
	default:
		return nil, invalidFailure(newValueError("invalid hook entry"))
	}
	value, err := objectValue(members...)
	if err != nil {
		return nil, invalidFailure(err)
	}
	return value, nil
}

func nativeHookParams(b *Bundle, hook HookArtifact, agent Agent, nativeDirectory, node, launcher string, entries []nativeHookEntry, value Value) (RenderedArtifactParams, error) {
	if agent == AgentCopilot {
		events := make([]jsonx.Member, len(entries))
		for index, entry := range entries {
			events[index] = jsonx.Member{Key: hookEventName(agent, entry.event), Value: jsonx.Array(jsonx.Object(
				jsonx.Member{Key: "type", Value: jsonx.String("command")},
				jsonx.Member{Key: "exec", Value: jsonx.String(node)},
				jsonx.Member{Key: "args", Value: jsonx.Array(jsonx.String(launcher), jsonx.String(string(entry.event)))},
				jsonx.Member{Key: "timeoutSec", Value: jsonx.Int(hookLauncherTimeoutSeconds)},
			))}
		}
		encoded := jsonx.EncodePythonCompact(jsonx.Object(
			jsonx.Member{Key: "version", Value: jsonx.Int(1)},
			jsonx.Member{Key: "hooks", Value: jsonx.Object(events...)},
		))
		return RenderedArtifactParams{
			ID:          hook.id,
			Family:      FamilyHooks,
			Destination: pathx.Join(nativeDirectory, "hooks", "flyrail-"+identifier(b.ID())+".json"),
			Content:     FileContent{data: append(encoded, '\n'), mode: ModePrivate},
		}, nil
	}
	member, err := NewMember(value, nil)
	if agent == AgentCursor {
		object, _ := value.(Object)
		command, found := object.Get("command")
		if !found {
			return RenderedArtifactParams{}, newValueError("missing command")
		}
		text, _ := command.(String)
		member, err = NewMember(text, []string{"command"})
	}
	if err != nil {
		return RenderedArtifactParams{}, err
	}
	format := FormatJSONC
	if agent == AgentCursor {
		format = FormatJSON
	}
	hooksKey, err := NewKey("hooks")
	if err != nil {
		return RenderedArtifactParams{}, err
	}
	eventKey, err := NewKey(hookEventName(agent, hook.event))
	if err != nil {
		return RenderedArtifactParams{}, err
	}
	selector, err := NewSelector(hooksKey, eventKey, member)
	if err != nil {
		return RenderedArtifactParams{}, err
	}
	structured, err := NewStructuredContent(format, selector, value, nil)
	if err != nil {
		return RenderedArtifactParams{}, err
	}
	var schemas []DocumentSchema
	if agent == AgentCursor {
		schema, err := NewDocumentSchema(FormatJSON, "version", Int(1))
		if err != nil {
			return RenderedArtifactParams{}, err
		}
		schemas = []DocumentSchema{schema}
	}
	destination := pathx.Join(nativeDirectory, "hooks.json")
	if agent == AgentClaude {
		destination = pathx.Join(nativeDirectory, "settings.json")
	}
	return RenderedArtifactParams{
		ID:                 hook.id,
		Family:             FamilyHooks,
		Destination:        destination,
		Content:            structured,
		SchemaRequirements: schemas,
	}, nil
}
