package flyrail

import (
	"slices"
	"strings"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
)

var identityKeys = []string{"agent", "scope", "surface", "root"}

// TranslatableAgents returns the agents Flyrail has verified configuration translations for, in Rust's order.
func TranslatableAgents() []Agent {
	return []Agent{AgentClaude, AgentCodex, AgentOpencode, AgentPi, AgentCursor, AgentCopilot}
}

// IsTranslatable reports whether Flyrail can translate portable artifacts for agent.
func IsTranslatable(a Agent) bool {
	return slices.Contains(TranslatableAgents(), a)
}

// UntranslatableMessage returns the notice text for an agent without a verified translation.
func UntranslatableMessage(a Agent) string {
	return "Flyrail detects " + string(a) + " but has no verified configuration translation."
}

// UnsupportedTranslationError reports that a request cannot be translated faithfully for its audience.
type UnsupportedTranslationError struct {
	Code    string
	Message string
}

// Error returns the message.
func (e *UnsupportedTranslationError) Error() string {
	if e == nil {
		return ""
	}
	return e.Message
}

func unsupportedTranslation(code, message string) *UnsupportedTranslationError {
	return &UnsupportedTranslationError{Code: code, Message: message}
}

// RequireTranslatable returns an *UnsupportedTranslationError when the context's agent has no verified translation.
func RequireTranslatable(rc RenderContext) error {
	if err := rc.check(); err != nil {
		return err
	}
	if failure := requireTranslatable(rc); failure != nil {
		return failure
	}
	return nil
}

func requireTranslatable(rc RenderContext) *UnsupportedTranslationError {
	agent := rc.audience.agent
	if IsTranslatable(agent) {
		return nil
	}
	return unsupportedTranslation("agent-unsupported", UntranslatableMessage(agent))
}

// RenderOptions are the optional routes and runtime of a RenderContext.
type RenderOptions struct {
	Surface         Surface      // "" = SurfaceCLI
	ExecutionRoot   string       // "" = the context's Root()
	InstructionPath string       // "" = the agent's default instruction destination
	MCPPath         string       // "" = the agent's default MCP destination
	NativeRoot      string       // "" = the context's Root()
	AssetRoot       string       // "" = Root()/.flyrail-assets
	HookRuntime     *HookRuntime // nil = no hook runtime; hooks render unsupported
}

// RenderContext is the audience and explicit routes one rendering targets.
type RenderContext struct {
	target          Target
	audience        Audience
	platform        Platform
	surface         Surface
	executionRoot   string
	instructionPath string
	mcpPath         string
	nativeRoot      string
	assetRoot       string
	hookRuntime     *HookRuntime
}

// NewRenderContext returns a validated context for an agent target on platform; option paths must be absolute routes.
func NewRenderContext(target Target, platform Platform, opts RenderOptions) (RenderContext, error) {
	if target.agent == "" {
		return RenderContext{}, newValueError("render context requires a project or user agent Target")
	}
	if err := checkEnum(platform, "Platform", PlatformMacOS, PlatformLinux, PlatformWindows); err != nil {
		return RenderContext{}, err
	}
	surface := opts.Surface
	if surface == "" {
		surface = SurfaceCLI
	}
	audience, err := NewAudience(target.agent, target.scope, surface, platform)
	if err != nil {
		return RenderContext{}, err
	}
	rc := RenderContext{
		target:          target,
		audience:        audience,
		platform:        platform,
		surface:         surface,
		executionRoot:   opts.ExecutionRoot,
		instructionPath: opts.InstructionPath,
		mcpPath:         opts.MCPPath,
		nativeRoot:      opts.NativeRoot,
		assetRoot:       opts.AssetRoot,
		hookRuntime:     opts.HookRuntime,
	}
	routes := []string{target.root, rc.Root(), rc.executionRoot, rc.instructionPath, rc.mcpPath, rc.nativeRoot, rc.assetRoot}
	for index, route := range routes {
		if index >= 2 && route == "" {
			continue
		}
		if _, err := validateRoute(route); err != nil {
			return RenderContext{}, err
		}
	}
	return rc, nil
}

func (rc RenderContext) check() error {
	if rc.audience.agent == "" {
		return newValueError("render context requires a project or user agent Target")
	}
	return nil
}

// Target returns the target.
func (rc RenderContext) Target() Target { return rc.target }

// Platform returns the platform.
func (rc RenderContext) Platform() Platform { return rc.platform }

// Surface returns the surface.
func (rc RenderContext) Surface() Surface { return rc.surface }

// Root returns the directory routes are resolved against: the user's home, or the project root.
func (rc RenderContext) Root() string {
	if rc.target.home != "" {
		return rc.target.home
	}
	return rc.target.anchor
}

// Audience returns the agent, scope, surface and platform rendered for.
func (rc RenderContext) Audience() Audience { return rc.audience }

// Skills returns the skill container directory.
func (rc RenderContext) Skills() string {
	if rc.surface == SurfaceVSCode && rc.target.scope == ScopeUser {
		return pathx.Join(rc.Root(), ".copilot/skills")
	}
	return rc.target.root
}

// ExecutionRoot returns the explicit execution root, or "" when not set.
func (rc RenderContext) ExecutionRoot() string { return rc.executionRoot }

// InstructionPath returns the explicit instruction destination, or "" when not set.
func (rc RenderContext) InstructionPath() string { return rc.instructionPath }

// MCPPath returns the explicit MCP destination, or "" when not set.
func (rc RenderContext) MCPPath() string { return rc.mcpPath }

// NativeRoot returns the explicit native artifact root, or "" when not set.
func (rc RenderContext) NativeRoot() string { return rc.nativeRoot }

// AssetRoot returns the explicit support asset root, or "" when not set.
func (rc RenderContext) AssetRoot() string { return rc.assetRoot }

// HookRuntime returns the hook runtime, or nil for none.
func (rc RenderContext) HookRuntime() *HookRuntime { return rc.hookRuntime }

func (rc RenderContext) executionDirectory() string {
	if rc.executionRoot != "" {
		return rc.executionRoot
	}
	return rc.Root()
}

func (rc RenderContext) nativeDirectory() string {
	if rc.nativeRoot != "" {
		return rc.nativeRoot
	}
	return rc.Root()
}

func (rc RenderContext) assetDirectory() string {
	if rc.assetRoot != "" {
		return rc.assetRoot
	}
	return pathx.Join(rc.Root(), ".flyrail-assets")
}

// RoutingContext returns the entries identifying every route this context renders to.
func (rc RenderContext) RoutingContext() map[string]string {
	return contextMap(rc.routingEntries())
}

func (rc RenderContext) routingEntries() []contextEntry {
	if rc.check() != nil {
		return nil
	}
	entries := []contextEntry{
		{"agent", string(rc.audience.agent)},
		{"scope", string(rc.target.scope)},
		{"surface", string(rc.surface)},
		{"platform", string(rc.platform)},
		{"root", displayPath(rc.Root())},
		{"skills", displayPath(rc.Skills())},
		{"execution", displayPath(rc.executionDirectory())},
		{"instructions", displayPath(rc.instructionPath)},
		{"mcp", displayPath(rc.mcpPath)},
		{"native", displayPath(rc.nativeDirectory())},
		{"assets", displayPath(rc.assetDirectory())},
		{"hook-node", displayPath(rc.hookRuntime.Node())},
		{"hook-shell", string(rc.hookRuntime.Shell())},
	}
	for _, variable := range rc.target.environment {
		entries = append(entries, contextEntry{"env:" + variable.key, variable.value})
	}
	slices.SortFunc(entries, func(a, b contextEntry) int {
		if order := strings.Compare(a.key, b.key); order != 0 {
			return order
		}
		return strings.Compare(a.value, b.value)
	})
	return entries
}

// Installation returns the installation target for this context in st; a nil context uses the agent, scope, surface and root routing entries as the identity.
func (rc RenderContext) Installation(st store.Store, context map[string]string) (*InstallationTarget, error) {
	if context != nil {
		return rc.InstallationWithContext(st, context)
	}
	if err := rc.check(); err != nil {
		return nil, err
	}
	routing := rc.routingEntries()
	identity := make([]contextEntry, 0, len(identityKeys))
	for _, entry := range routing {
		if slices.Contains(identityKeys, entry.key) {
			identity = append(identity, entry)
		}
	}
	return newInstallationTarget(st, identity, routing)
}

// InstallationWithContext returns the installation target for this context in st identified by context exactly as given.
func (rc RenderContext) InstallationWithContext(st store.Store, context map[string]string) (*InstallationTarget, error) {
	if err := rc.check(); err != nil {
		return nil, err
	}
	return newInstallationTarget(st, contextEntries(context), rc.routingEntries())
}

func userDirectory(rc RenderContext) (string, error) {
	agent := rc.audience.agent
	var variable, suffix string
	switch agent {
	case AgentClaude:
		variable, suffix = "CLAUDE_CONFIG_DIR", ".claude"
	case AgentCodex:
		variable, suffix = "CODEX_HOME", ".codex"
	case AgentOpencode:
		variable, suffix = "XDG_CONFIG_HOME", ".config"
	case AgentPi:
		variable, suffix = "PI_CODING_AGENT_DIR", ".pi/agent"
	case AgentCursor:
		variable, suffix = "", ".cursor"
	case AgentCopilot:
		variable, suffix = "COPILOT_HOME", ".copilot"
	default:
		return "", newValueError(UntranslatableMessage(agent))
	}
	if rc.surface == SurfaceVSCode {
		variable = ""
	}
	root := pathx.Join(rc.Root(), suffix)
	if configured, found := rc.target.environmentValue(variable); found && variable != "" && configured != "" {
		relocatedRoot, err := relocated(configured, rc.Root())
		if err != nil {
			return "", err
		}
		root = relocatedRoot
	}
	if agent == AgentOpencode {
		root = pathx.Join(root, "opencode")
	}
	return root, nil
}

// InstructionDestination returns where instructions render for rc, or "" when the audience has no instruction preset.
func InstructionDestination(rc RenderContext) (string, error) {
	if err := rc.check(); err != nil {
		return "", err
	}
	return instructionDestination(rc)
}

func instructionDestination(rc RenderContext) (string, error) {
	if rc.instructionPath != "" {
		return rc.instructionPath, nil
	}
	agent := rc.audience.agent
	if !IsTranslatable(agent) {
		return "", nil
	}
	if rc.target.scope == ScopeProject {
		var path string
		switch agent {
		case AgentClaude:
			path = "CLAUDE.md"
		case AgentCodex, AgentOpencode, AgentPi, AgentCursor:
			path = "AGENTS.md"
		case AgentCopilot:
			path = ".github/copilot-instructions.md"
		case AgentDroid:
			return "", nil
		default:
			return "", unhandledAgent(agent)
		}
		return pathx.Join(rc.Root(), path), nil
	}
	if agent == AgentCursor {
		return "", nil
	}
	directory, err := userDirectory(rc)
	if err != nil {
		return "", err
	}
	if rc.surface == SurfaceVSCode {
		return pathx.Join(directory, "instructions"), nil
	}
	var path string
	switch agent {
	case AgentClaude:
		path = "CLAUDE.md"
	case AgentCodex, AgentOpencode, AgentPi:
		path = "AGENTS.md"
	case AgentCopilot:
		path = "copilot-instructions.md"
	case AgentCursor, AgentDroid:
		return "", nil
	default:
		return "", unhandledAgent(agent)
	}
	return pathx.Join(directory, path), nil
}

// MCPDestination returns where MCP registrations render for rc, or "" when the audience has no MCP preset.
func MCPDestination(rc RenderContext) (string, error) {
	if err := rc.check(); err != nil {
		return "", err
	}
	return mcpDestination(rc)
}

func mcpDestination(rc RenderContext) (string, error) {
	if rc.mcpPath != "" {
		return rc.mcpPath, nil
	}
	agent := rc.audience.agent
	if !IsTranslatable(agent) {
		return "", nil
	}
	if rc.target.scope == ScopeProject {
		if rc.surface == SurfaceVSCode {
			return pathx.Join(rc.Root(), ".vscode/mcp.json"), nil
		}
		var path string
		switch agent {
		case AgentClaude, AgentCopilot:
			path = ".mcp.json"
		case AgentCodex:
			path = ".codex/config.toml"
		case AgentOpencode:
			path = "opencode.json"
		case AgentPi:
			path = ".pi/mcp.json"
		case AgentCursor:
			path = ".cursor/mcp.json"
		case AgentDroid:
			return "", nil
		default:
			return "", unhandledAgent(agent)
		}
		return pathx.Join(rc.Root(), path), nil
	}
	if rc.surface == SurfaceVSCode {
		return "", nil
	}
	if agent == AgentClaude {
		if _, configured := nonemptyEnvironment(rc, "CLAUDE_CONFIG_DIR"); configured {
			return "", nil
		}
		return pathx.Join(rc.Root(), ".claude.json"), nil
	}
	if agent == AgentOpencode {
		if configured, ok := nonemptyEnvironment(rc, "OPENCODE_CONFIG"); ok {
			return relocated(configured, rc.Root())
		}
	}
	var path string
	switch agent {
	case AgentCodex:
		path = "config.toml"
	case AgentOpencode:
		path = "opencode.json"
	case AgentPi, AgentCursor:
		path = "mcp.json"
	case AgentCopilot:
		path = "mcp-config.json"
	case AgentClaude, AgentDroid:
		return "", nil
	default:
		return "", unhandledAgent(agent)
	}
	directory, err := userDirectory(rc)
	if err != nil {
		return "", err
	}
	return pathx.Join(directory, path), nil
}

func nonemptyEnvironment(rc RenderContext, key string) (string, bool) {
	value, found := rc.target.environmentValue(key)
	return value, found && value != ""
}

// Capability describes how well Flyrail supports one artifact family for a render context.
type Capability struct {
	Family        Family
	Portable      bool
	Native        bool
	Limitations   []string
	Prerequisites []string
}

// Capabilities returns the support of each family for rc, in skills, instructions, MCP, hooks order.
func Capabilities(rc RenderContext) ([]Capability, error) {
	if err := rc.check(); err != nil {
		return nil, err
	}
	if !IsTranslatable(rc.audience.agent) {
		return []Capability{
			{Family: FamilySkills, Limitations: []string{}, Prerequisites: []string{}},
			{Family: FamilyInstructions, Limitations: []string{}, Prerequisites: []string{}},
			{Family: FamilyMCP, Limitations: []string{}, Prerequisites: []string{}},
			{Family: FamilyHooks, Limitations: []string{}, Prerequisites: []string{}},
		}, nil
	}
	instructions, err := instructionDestination(rc)
	if err != nil {
		return nil, err
	}
	mcp, err := mcpDestination(rc)
	if err != nil {
		return nil, err
	}
	mcpPrerequisites := []string{}
	if rc.audience.agent == AgentPi {
		mcpPrerequisites = []string{"pi-mcp-adapter@2.32.1"}
	}
	return []Capability{
		{Family: FamilySkills, Portable: true, Native: true, Limitations: []string{}, Prerequisites: []string{}},
		{
			Family:        FamilyInstructions,
			Portable:      instructions != "",
			Native:        true,
			Limitations:   []string{"Discovery, overrides, byte limits and session loading remain host-controlled."},
			Prerequisites: []string{},
		},
		{
			Family:        FamilyMCP,
			Portable:      mcp != "",
			Native:        true,
			Limitations:   []string{"Transport, interpolation, environment references and cwd are checked per request."},
			Prerequisites: mcpPrerequisites,
		},
		{
			Family:        FamilyHooks,
			Portable:      rc.surface != SurfaceVSCode && rc.hookRuntime != nil,
			Native:        true,
			Limitations:   []string{"Event/outcome and native shell support are validated for each request."},
			Prerequisites: []string{"Node 24+; explicit executable for native hosts. See the hook protocol matrix."},
		},
	}, nil
}
