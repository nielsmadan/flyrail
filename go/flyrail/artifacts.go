package flyrail

import (
	"maps"
	"slices"
	"strconv"
	"strings"

	"golang.org/x/text/unicode/norm"

	"github.com/nielsmadan/flyrail/go/internal/unicodex"
)

// Family is the kind of agent configuration an artifact manages.
type Family string

// The artifact families.
const (
	FamilySkills       Family = "skills"
	FamilyInstructions Family = "instructions"
	FamilyMCP          Family = "mcp"
	FamilyHooks        Family = "hooks"
)

// ParseFamily returns the family spelled s.
func ParseFamily(s string) (Family, error) {
	return parseEnum(s, "Family", FamilySkills, FamilyInstructions, FamilyMCP, FamilyHooks)
}

func checkFamily(family Family) error {
	return checkEnum(family, "Family", FamilySkills, FamilyInstructions, FamilyMCP, FamilyHooks)
}

// Audience is the agent, scope, surface and optional platform a native artifact is written for.
type Audience struct {
	agent    Agent
	scope    TargetScope
	surface  Surface
	platform Platform
}

// NewAudience returns a validated audience; platform "" means any platform.
func NewAudience(agent Agent, scope TargetScope, surface Surface, platform Platform) (Audience, error) {
	if err := checkEnum(agent, "Agent", Agents()...); err != nil {
		return Audience{}, err
	}
	if err := checkEnum(scope, "TargetScope", ScopeDirectory, ScopeUser, ScopeProject); err != nil {
		return Audience{}, err
	}
	if err := checkEnum(surface, "Surface", SurfaceCLI, SurfaceIDE, SurfaceVSCode); err != nil {
		return Audience{}, err
	}
	if platform != "" {
		if err := checkEnum(platform, "Platform", PlatformMacOS, PlatformLinux, PlatformWindows); err != nil {
			return Audience{}, err
		}
	}
	if scope == ScopeDirectory {
		return Audience{}, newValueError("native audiences require project or user scope")
	}
	if surface == SurfaceIDE && agent != AgentCursor {
		return Audience{}, newValueError("IDE surface is only defined for Cursor")
	}
	if surface == SurfaceVSCode && agent != AgentCopilot {
		return Audience{}, newValueError("VSCode surface is only defined for Copilot")
	}
	return Audience{agent: agent, scope: scope, surface: surface, platform: platform}, nil
}

// Agent returns the agent.
func (a Audience) Agent() Agent { return a.agent }

// Scope returns the target scope.
func (a Audience) Scope() TargetScope { return a.scope }

// Surface returns the surface.
func (a Audience) Surface() Surface { return a.surface }

// Platform returns the platform, or "" for any platform.
func (a Audience) Platform() Platform { return a.platform }

func (a Audience) modelValue() Value {
	return typedRecord("Audience",
		field("agent", modelText(string(a.agent))),
		field("platform", modelEnum(a.platform)),
		field("scope", modelText(string(a.scope))),
		field("surface", modelText(string(a.surface))),
	)
}

// EnvRef names an environment variable whose value is resolved by the agent at run time.
type EnvRef struct {
	name string
}

// NewEnvRef returns a reference to the environment variable name.
func NewEnvRef(name string) (EnvRef, error) {
	valid := name != ""
	for index, r := range name {
		letter := r >= 'A' && r <= 'Z' || r >= 'a' && r <= 'z' || r == '_'
		valid = valid && (letter || index > 0 && r >= '0' && r <= '9')
	}
	if !valid {
		return EnvRef{}, newValueError("invalid environment reference")
	}
	return EnvRef{name: name}, nil
}

// Name returns the variable name.
func (r EnvRef) Name() string { return r.name }

func (r EnvRef) modelValue() Value {
	return typedRecord("EnvRef", field("name", modelText(r.name)))
}

// AssetRef refers to a support asset, or to a path inside one.
type AssetRef struct {
	assetID string
	path    string
}

// NewAssetRef returns a reference to asset assetID; path "" refers to the whole asset.
func NewAssetRef(assetID, path string) (AssetRef, error) {
	if err := validateIdentifier(assetID, "referenced asset id"); err != nil {
		return AssetRef{}, err
	}
	if path != "" {
		if err := validateRelativePath(path, "asset reference path"); err != nil {
			return AssetRef{}, err
		}
	}
	return AssetRef{assetID: assetID, path: path}, nil
}

// AssetID returns the referenced asset id.
func (r AssetRef) AssetID() string { return r.assetID }

// Path returns the path inside the asset, or "" for the whole asset.
func (r AssetRef) Path() string { return r.path }

func (r AssetRef) modelValue() Value {
	path := Value(Null{})
	if r.path != "" {
		path = modelText(r.path)
	}
	return typedRecord("AssetRef", field("asset_id", modelText(r.assetID)), field("path", path))
}

// CommandArgument is one argv element or working directory: literal ArgText or an AssetRef.
type CommandArgument interface {
	isCommandArgument()
}

// ArgText is a literal command argument.
type ArgText string

func (ArgText) isCommandArgument()  {}
func (AssetRef) isCommandArgument() {}

func modelArgument(argument CommandArgument) Value {
	if reference, ok := argument.(AssetRef); ok {
		return reference.modelValue()
	}
	if text, ok := argument.(ArgText); ok {
		return modelText(string(text))
	}
	return Null{}
}

// EnvironmentValue is the value of a command environment variable: literal EnvText or an EnvRef.
type EnvironmentValue interface {
	isEnvironmentValue()
}

// EnvText is a literal environment value.
type EnvText string

func (EnvText) isEnvironmentValue() {}
func (EnvRef) isEnvironmentValue()  {}

func modelEnvironmentValue(value EnvironmentValue) Value {
	if reference, ok := value.(EnvRef); ok {
		return reference.modelValue()
	}
	if text, ok := value.(EnvText); ok {
		return modelText(string(text))
	}
	return Null{}
}

type environmentVariable struct {
	name  string
	value EnvironmentValue
}

// Command is an argv, environment and optional working directory for a local process.
type Command struct {
	argv []CommandArgument
	env  []environmentVariable
	cwd  CommandArgument
}

func checkAssetRef(reference AssetRef) error {
	return validateIdentifier(reference.assetID, "referenced asset id")
}

// NewCommand returns a validated command; cwd nil means none.
func NewCommand(argv []CommandArgument, env map[string]EnvironmentValue, cwd CommandArgument) (Command, error) {
	names := slices.Sorted(maps.Keys(env))
	return newCommand(argv, names, env, cwd)
}

func newCommand(argv []CommandArgument, names []string, env map[string]EnvironmentValue, cwd CommandArgument) (Command, error) {
	if len(argv) == 0 {
		return Command{}, newValueError("argv requires a command")
	}
	for _, argument := range argv {
		switch a := argument.(type) {
		case ArgText:
			if err := validText(string(a), "command argument"); err != nil {
				return Command{}, err
			}
			if strings.Contains(string(a), "\x00") {
				return Command{}, newValueError("command arguments cannot contain NUL")
			}
		case AssetRef:
			if err := checkAssetRef(a); err != nil {
				return Command{}, err
			}
		default:
			return Command{}, newValueError("command arguments must be ArgText or AssetRef values")
		}
	}
	if text, ok := argv[0].(ArgText); ok && text == "" {
		return Command{}, newValueError("command cannot be empty")
	}
	variables := make([]environmentVariable, 0, len(names))
	for _, name := range names {
		if _, err := NewEnvRef(name); err != nil {
			return Command{}, err
		}
		switch v := env[name].(type) {
		case EnvText:
			if err := validText(string(v), "environment value"); err != nil {
				return Command{}, err
			}
			if strings.Contains(string(v), "\x00") {
				return Command{}, newValueError("environment values cannot contain NUL")
			}
		case EnvRef:
			if _, err := NewEnvRef(v.name); err != nil {
				return Command{}, err
			}
		default:
			return Command{}, newValueError("environment values must be EnvText or EnvRef values")
		}
		variables = append(variables, environmentVariable{name: name, value: env[name]})
	}
	switch c := cwd.(type) {
	case nil:
	case ArgText:
		if err := validateRelativePath(string(c), "command cwd"); err != nil {
			return Command{}, err
		}
	case AssetRef:
		if err := checkAssetRef(c); err != nil {
			return Command{}, err
		}
	default:
		return Command{}, newValueError("command arguments must be ArgText or AssetRef values")
	}
	slices.SortFunc(variables, func(left, right environmentVariable) int { return strings.Compare(left.name, right.name) })
	return Command{argv: slices.Clone(argv), env: variables, cwd: cwd}, nil
}

// Argv returns a copy of the arguments.
func (c Command) Argv() []CommandArgument { return slices.Clone(c.argv) }

// Env returns a copy of the environment.
func (c Command) Env() map[string]EnvironmentValue {
	env := make(map[string]EnvironmentValue, len(c.env))
	for _, variable := range c.env {
		env[variable.name] = variable.value
	}
	return env
}

// Cwd returns the working directory, or nil for none.
func (c Command) Cwd() CommandArgument { return c.cwd }

func (c Command) modelValue() Value {
	cwd := Value(Null{})
	if c.cwd != nil {
		cwd = modelArgument(c.cwd)
	}
	return typedRecord("Command",
		field("argv", modelList(c.argv, modelArgument)),
		field("cwd", cwd),
		field("env", modelList(c.env, func(variable environmentVariable) Value {
			return modelArray([]Value{modelText(variable.name), modelEnvironmentValue(variable.value)})
		})),
	)
}

// HTTPTransport is a remote MCP server reached over HTTP or HTTPS.
type HTTPTransport struct {
	url         string
	bearerToken *EnvRef
}

func asciiLower(s string) string {
	return strings.Map(func(r rune) rune {
		if r >= 'A' && r <= 'Z' {
			return r + 'a' - 'A'
		}
		return r
	}, s)
}

// NewHTTPTransport returns a transport for url, an absolute HTTP or HTTPS URL; bearerToken nil means none.
func NewHTTPTransport(url string, bearerToken *EnvRef) (HTTPTransport, error) {
	if err := validText(url, "MCP URL"); err != nil {
		return HTTPTransport{}, err
	}
	absolute := newValueError("MCP URL must be an absolute HTTP or HTTPS URL")
	scheme, rest, ok := strings.Cut(url, ":")
	if !ok {
		return HTTPTransport{}, absolute
	}
	if lowered := asciiLower(scheme); lowered != "http" && lowered != "https" {
		return HTTPTransport{}, absolute
	}
	authority, ok := strings.CutPrefix(rest, "//")
	if !ok {
		return HTTPTransport{}, absolute
	}
	netloc := authority
	if index := strings.IndexAny(authority, "/?#"); index >= 0 {
		netloc = authority[:index]
	}
	if err := validateAuthority(netloc); err != nil {
		return HTTPTransport{}, err
	}
	if err := validateNetlocNormalization(netloc); err != nil {
		return HTTPTransport{}, err
	}
	hostinfo, credentials := netloc, false
	if index := strings.LastIndexByte(netloc, '@'); index >= 0 {
		hostinfo, credentials = netloc[index+1:], true
	}
	host, port, err := hostAndPort(hostinfo)
	if err != nil {
		return HTTPTransport{}, err
	}
	if host == "" {
		return HTTPTransport{}, absolute
	}
	if strings.HasPrefix(hostinfo, "[") {
		if err := validateBracketedHost(host); err != nil {
			return HTTPTransport{}, err
		}
	}
	_, fragment, hasFragment := strings.Cut(url, "#")
	if credentials || hasFragment && fragment != "" {
		return HTTPTransport{}, newValueError("MCP URL cannot embed credentials or a fragment")
	}
	if strings.ContainsFunc(url, func(r rune) bool { return unicodex.IsPythonSpace(r) || r < 32 }) {
		return HTTPTransport{}, newValueError("MCP URL cannot contain whitespace or control characters")
	}
	if port != "" {
		number, err := strconv.ParseUint(port, 10, 32)
		if strings.ContainsFunc(port, func(r rune) bool { return r < '0' || r > '9' }) || err != nil || number < 1 || number > 65_535 {
			return HTTPTransport{}, newValueError("MCP URL port must be 1-65535")
		}
	}
	if bearerToken == nil {
		return HTTPTransport{url: url}, nil
	}
	if _, err := NewEnvRef(bearerToken.name); err != nil {
		return HTTPTransport{}, err
	}
	token := *bearerToken
	return HTTPTransport{url: url, bearerToken: &token}, nil
}

func invalidIPv6() error {
	return newValueError("Invalid IPv6 URL")
}

func validateAuthority(netloc string) error {
	hostinfo := netloc
	if index := strings.LastIndexByte(netloc, '@'); index >= 0 {
		hostinfo = netloc[index+1:]
	}
	if !strings.ContainsAny(hostinfo, "[]") {
		return nil
	}
	bracketed, ok := strings.CutPrefix(hostinfo, "[")
	if !ok {
		return invalidIPv6()
	}
	_, suffix, ok := strings.Cut(bracketed, "]")
	if !ok || strings.ContainsAny(suffix, "[]") || suffix != "" && !strings.HasPrefix(suffix, ":") {
		return invalidIPv6()
	}
	return nil
}

func validateNetlocNormalization(netloc string) error {
	if !strings.ContainsFunc(netloc, func(r rune) bool { return r >= 0x80 }) {
		return nil
	}
	stripped := strings.Map(func(r rune) rune {
		if strings.ContainsRune("@:#?", r) {
			return -1
		}
		return r
	}, netloc)
	normalized := norm.NFKC.String(stripped)
	if stripped != normalized && strings.ContainsAny(normalized, "/?#@:") {
		return newValueError("netloc '" + netloc + "' contains invalid characters under NFKC normalization")
	}
	return nil
}

func hostAndPort(hostinfo string) (string, string, error) {
	if bracketed, ok := strings.CutPrefix(hostinfo, "["); ok {
		host, suffix, ok := strings.Cut(bracketed, "]")
		if !ok {
			return "", "", invalidIPv6()
		}
		port, ok := strings.CutPrefix(suffix, ":")
		if !ok {
			port = ""
		}
		return host, port, nil
	}
	host, port, _ := strings.Cut(hostinfo, ":")
	return host, port, nil
}

func isHexDigit(b byte) bool {
	return b >= '0' && b <= '9' || b >= 'a' && b <= 'f' || b >= 'A' && b <= 'F'
}

func allBytes(s string, accept func(byte) bool) bool {
	for index := range len(s) {
		if !accept(s[index]) {
			return false
		}
	}
	return true
}

func validateBracketedHost(host string) error {
	if host == "" || strings.ContainsAny(host, "[]") {
		return invalidIPv6()
	}
	if future, ok := strings.CutPrefix(host, "v"); ok {
		version, address, ok := strings.Cut(future, ".")
		if !ok || version == "" || !allBytes(version, isHexDigit) || address == "" {
			return newValueError("IPvFuture address is invalid")
		}
		return nil
	}
	return validateIPv6Address(host)
}

func validateIPv6Address(value string) error {
	invalid := newValueError("'" + value + "' does not appear to be an IPv4 or IPv6 address")
	address, zone, zoned := strings.Cut(value, "%")
	if !zoned {
		address = value
	}
	if address == "" || zoned && zone == "" {
		return invalid
	}
	compressed := strings.Count(address, "::")
	if compressed > 1 {
		return invalid
	}
	var groups int
	if left, right, ok := strings.Cut(address, "::"); ok {
		leftGroups, err := countIPv6Groups(left)
		if err != nil {
			return err
		}
		rightGroups, err := countIPv6Groups(right)
		if err != nil {
			return err
		}
		groups = leftGroups + rightGroups
		if groups >= 8 {
			return invalid
		}
	} else {
		count, err := countIPv6Groups(address)
		if err != nil {
			return err
		}
		if count != 8 {
			return invalid
		}
		groups = count
	}
	if compressed == 0 && groups == 0 {
		return invalid
	}
	return nil
}

func countIPv6Groups(value string) (int, error) {
	if value == "" {
		return 0, nil
	}
	parts := strings.Split(value, ":")
	count := 0
	for index, part := range parts {
		switch {
		case strings.Contains(part, "."):
			if index+1 != len(parts) || !validIPv4Tail(part) {
				return 0, newValueError("bracketed host is not a valid IPv6 address")
			}
			count += 2
		case len(part) >= 1 && len(part) <= 4 && allBytes(part, isHexDigit):
			count++
		default:
			return 0, newValueError("bracketed host is not a valid IPv6 address")
		}
	}
	return count, nil
}

func validIPv4Tail(value string) bool {
	parts := strings.Split(value, ".")
	return len(parts) == 4 && !slices.ContainsFunc(parts, func(part string) bool {
		number, err := strconv.ParseUint(part, 10, 16)
		return part == "" || len(part) > 1 && part[0] == '0' ||
			!allBytes(part, func(b byte) bool { return b >= '0' && b <= '9' }) || err != nil || number > 255
	})
}

// URL returns the server URL.
func (t HTTPTransport) URL() string { return t.url }

// BearerToken returns the environment reference holding the bearer token, if any.
func (t HTTPTransport) BearerToken() (EnvRef, bool) {
	if t.bearerToken == nil {
		return EnvRef{}, false
	}
	return *t.bearerToken, true
}

func (t HTTPTransport) modelValue() Value {
	token := Value(Null{})
	if t.bearerToken != nil {
		token = t.bearerToken.modelValue()
	}
	return typedRecord("HttpTransport", field("bearer_token", token), field("url", modelText(t.url)))
}

// MCPTransport is how an MCP server is reached: a local Command or an HTTPTransport.
type MCPTransport interface {
	isMCPTransport()
}

func (Command) isMCPTransport()       {}
func (HTTPTransport) isMCPTransport() {}

func checkTransport(transport MCPTransport) error {
	switch t := transport.(type) {
	case Command:
		if len(t.argv) == 0 {
			return newValueError("argv requires a command")
		}
		return nil
	case HTTPTransport:
		if t.url == "" {
			return newValueError("MCP URL must be an absolute HTTP or HTTPS URL")
		}
		return nil
	}
	return newValueError("MCP transports must be Command or HTTPTransport values")
}

func modelTransport(transport MCPTransport) Value {
	switch t := transport.(type) {
	case Command:
		return t.modelValue()
	case HTTPTransport:
		return t.modelValue()
	}
	return Null{}
}

// Artifact is one portable configuration unit of a bundle.
type Artifact interface {
	ID() string
	isArtifact()
}

func (SkillArtifact) isArtifact()       {}
func (InstructionArtifact) isArtifact() {}
func (MCPArtifact) isArtifact()         {}
func (HookArtifact) isArtifact()        {}
func (NativeArtifact) isArtifact()      {}

func checkArtifact(artifact Artifact) error {
	switch artifact.(type) {
	case SkillArtifact, InstructionArtifact, MCPArtifact, HookArtifact, NativeArtifact:
		return validateIdentifier(artifact.ID(), "artifact id")
	}
	return newValueError("artifacts must be SkillArtifact, InstructionArtifact, MCPArtifact, HookArtifact or NativeArtifact values")
}

func modelArtifact(artifact Artifact) Value {
	switch a := artifact.(type) {
	case SkillArtifact:
		return a.modelValue()
	case InstructionArtifact:
		return a.modelValue()
	case MCPArtifact:
		return a.modelValue()
	case HookArtifact:
		return a.modelValue()
	case NativeArtifact:
		return a.modelValue()
	}
	return Null{}
}

func artifactCommand(artifact Artifact) (Command, bool) {
	switch a := artifact.(type) {
	case HookArtifact:
		return a.command, true
	case MCPArtifact:
		command, ok := a.transport.(Command)
		return command, ok
	}
	return Command{}, false
}

// SkillArtifact is a skill directory installed under its name.
type SkillArtifact struct {
	id      string
	name    string
	content TreeContent
}

// NewSkillArtifact returns a skill artifact; content must contain a SKILL.md file.
func NewSkillArtifact(id, name string, content TreeContent) (SkillArtifact, error) {
	if err := validateIdentifier(id, "artifact id"); err != nil {
		return SkillArtifact{}, err
	}
	if err := validateIdentifier(name, "skill name"); err != nil {
		return SkillArtifact{}, err
	}
	if !slices.ContainsFunc(content.entries, func(entry BundleEntry) bool { return entry.Path == "SKILL.md" && !entry.Dir }) {
		return SkillArtifact{}, newValueError("skill must contain a SKILL.md file")
	}
	return SkillArtifact{id: id, name: name, content: content}, nil
}

// ID returns the artifact id.
func (a SkillArtifact) ID() string { return a.id }

// Name returns the skill name.
func (a SkillArtifact) Name() string { return a.name }

// Content returns the skill tree.
func (a SkillArtifact) Content() TreeContent { return a.content }

func (a SkillArtifact) modelValue() Value {
	return typedRecord("SkillArtifact",
		field("content", a.content.modelValue()),
		field("id", modelText(a.id)),
		field("name", modelText(a.name)),
	)
}

// InstructionArtifact is portable instruction text.
type InstructionArtifact struct {
	id   string
	text string
}

// NewInstructionArtifact returns an instruction artifact.
func NewInstructionArtifact(id, text string) (InstructionArtifact, error) {
	if err := validateIdentifier(id, "artifact id"); err != nil {
		return InstructionArtifact{}, err
	}
	if err := validText(text, "instruction text"); err != nil {
		return InstructionArtifact{}, err
	}
	if strings.Contains(text, "\x00") {
		return InstructionArtifact{}, newValueError("instruction text cannot contain NUL")
	}
	return InstructionArtifact{id: id, text: text}, nil
}

// ID returns the artifact id.
func (a InstructionArtifact) ID() string { return a.id }

// Text returns the instruction text.
func (a InstructionArtifact) Text() string { return a.text }

func (a InstructionArtifact) modelValue() Value {
	return typedRecord("InstructionArtifact", field("id", modelText(a.id)), field("text", modelText(a.text)))
}

// MCPArtifact is an MCP server registration.
type MCPArtifact struct {
	id        string
	name      string
	transport MCPTransport
}

// NewMCPArtifact returns an MCP server registration named name.
func NewMCPArtifact(id, name string, transport MCPTransport) (MCPArtifact, error) {
	if err := validateIdentifier(id, "artifact id"); err != nil {
		return MCPArtifact{}, err
	}
	if err := validateIdentifier(name, "MCP server name"); err != nil {
		return MCPArtifact{}, err
	}
	if err := checkTransport(transport); err != nil {
		return MCPArtifact{}, err
	}
	return MCPArtifact{id: id, name: name, transport: transport}, nil
}

// ID returns the artifact id.
func (a MCPArtifact) ID() string { return a.id }

// Name returns the server name.
func (a MCPArtifact) Name() string { return a.name }

// Transport returns the server transport.
func (a MCPArtifact) Transport() MCPTransport { return a.transport }

func (a MCPArtifact) modelValue() Value {
	return typedRecord("McpArtifact",
		field("id", modelText(a.id)),
		field("name", modelText(a.name)),
		field("transport", modelTransport(a.transport)),
	)
}

// HookEvent is the agent event a hook runs on.
type HookEvent string

// The hook events.
const (
	EventSessionStart HookEvent = "session-start"
	EventBeforeTool   HookEvent = "before-tool"
	EventAfterTool    HookEvent = "after-tool"
	EventPrompt       HookEvent = "prompt"
	EventStop         HookEvent = "stop"
)

var hookEvents = []HookEvent{EventSessionStart, EventBeforeTool, EventAfterTool, EventPrompt, EventStop}

// ParseHookEvent returns the hook event spelled s.
func ParseHookEvent(s string) (HookEvent, error) {
	return parseEnum(s, "HookEvent", hookEvents...)
}

// HookOutcome is a decision a hook may return.
type HookOutcome string

// The hook outcomes.
const (
	OutcomeContinue    HookOutcome = "continue"
	OutcomeBlock       HookOutcome = "block"
	OutcomeAsk         HookOutcome = "ask"
	OutcomeModifyInput HookOutcome = "modify-input"
	OutcomeContext     HookOutcome = "context"
)

var hookOutcomes = []HookOutcome{OutcomeContinue, OutcomeBlock, OutcomeAsk, OutcomeModifyInput, OutcomeContext}

// ParseHookOutcome returns the hook outcome spelled s.
func ParseHookOutcome(s string) (HookOutcome, error) {
	return parseEnum(s, "HookOutcome", hookOutcomes...)
}

// HookArtifactParams are the inputs of NewHookArtifact.
type HookArtifactParams struct {
	ID            string        // required
	Event         HookEvent     // required
	Command       Command       // required; the zero Command is rejected
	TimeoutMillis int           // required, 1-300000; zero is rejected
	Outcomes      []HookOutcome // required, nonempty, unique
	Tools         []string      // nil = every tool; only for tool events
}

// HookArtifact is a portable hook handler.
type HookArtifact struct {
	id              string
	event           HookEvent
	command         Command
	timeoutMillis   int
	outcomes        []HookOutcome
	tools           []string
	protocolVersion int
}

// NewHookArtifact returns a validated hook artifact with outcomes and tools in byte order.
func NewHookArtifact(p HookArtifactParams) (HookArtifact, error) {
	if err := validateIdentifier(p.ID, "artifact id"); err != nil {
		return HookArtifact{}, err
	}
	if err := checkEnum(p.Event, "HookEvent", hookEvents...); err != nil {
		return HookArtifact{}, err
	}
	if err := checkTransport(p.Command); err != nil {
		return HookArtifact{}, err
	}
	if p.TimeoutMillis < 1 || p.TimeoutMillis > 300_000 {
		return HookArtifact{}, newValueError("hook timeout must be 1-300000 integer milliseconds")
	}
	if len(p.Outcomes) == 0 {
		return HookArtifact{}, newValueError("hook requires declared HookOutcome values")
	}
	for index, outcome := range p.Outcomes {
		if err := checkEnum(outcome, "HookOutcome", hookOutcomes...); err != nil {
			return HookArtifact{}, err
		}
		if slices.Contains(p.Outcomes[:index], outcome) {
			return HookArtifact{}, newValueError("hook outcomes must be unique")
		}
	}
	for _, name := range p.Tools {
		if err := validText(name, "tool name"); err != nil {
			return HookArtifact{}, err
		}
		if name == "" || strings.Contains(name, "\x00") {
			return HookArtifact{}, newValueError("tool names must be nonempty and cannot contain NUL")
		}
	}
	for index, name := range p.Tools {
		if slices.Contains(p.Tools[:index], name) {
			return HookArtifact{}, newValueError("tool names must be unique")
		}
	}
	if len(p.Tools) > 0 && p.Event != EventBeforeTool && p.Event != EventAfterTool {
		return HookArtifact{}, newValueError("tool selection requires a tool event")
	}
	outcomes := slices.Clone(p.Outcomes)
	slices.Sort(outcomes)
	tools := slices.Clone(p.Tools)
	slices.Sort(tools)
	return HookArtifact{
		id:              p.ID,
		event:           p.Event,
		command:         p.Command,
		timeoutMillis:   p.TimeoutMillis,
		outcomes:        outcomes,
		tools:           tools,
		protocolVersion: 1,
	}, nil
}

// ID returns the artifact id.
func (a HookArtifact) ID() string { return a.id }

// Event returns the hook event.
func (a HookArtifact) Event() HookEvent { return a.event }

// Command returns the handler command.
func (a HookArtifact) Command() Command { return a.command }

// TimeoutMillis returns the handler timeout in milliseconds.
func (a HookArtifact) TimeoutMillis() int { return a.timeoutMillis }

// Outcomes returns the declared outcomes in byte order.
func (a HookArtifact) Outcomes() []HookOutcome { return slices.Clone(a.outcomes) }

// Tools returns the selected tool names in byte order; empty selects every tool.
func (a HookArtifact) Tools() []string { return slices.Clone(a.tools) }

// ProtocolVersion returns the hook protocol version, always 1.
func (a HookArtifact) ProtocolVersion() int { return a.protocolVersion }

func (a HookArtifact) modelValue() Value {
	return typedRecord("HookArtifact",
		field("command", a.command.modelValue()),
		field("event", modelText(string(a.event))),
		field("id", modelText(a.id)),
		field("outcomes", modelList(a.outcomes, func(outcome HookOutcome) Value { return modelText(string(outcome)) })),
		field("protocol_version", Int(a.protocolVersion)),
		field("timeout_ms", Int(a.timeoutMillis)),
		field("tools", modelList(a.tools, modelText)),
	)
}

// NativeArtifact is agent-specific content written at a destination relative to the target root.
type NativeArtifact struct {
	id          string
	family      Family
	audience    Audience
	destination string
	content     Content
}

// NewNativeArtifact returns a native artifact for audience at the relative destination.
func NewNativeArtifact(id string, family Family, audience Audience, destination string, content Content) (NativeArtifact, error) {
	if err := validateIdentifier(id, "artifact id"); err != nil {
		return NativeArtifact{}, err
	}
	if err := validateRelativePath(destination, "native destination"); err != nil {
		return NativeArtifact{}, err
	}
	if err := checkFamily(family); err != nil {
		return NativeArtifact{}, err
	}
	if err := checkEnum(audience.agent, "Agent", Agents()...); err != nil {
		return NativeArtifact{}, err
	}
	if err := checkContent(content); err != nil {
		return NativeArtifact{}, err
	}
	return NativeArtifact{id: id, family: family, audience: audience, destination: destination, content: content}, nil
}

// ID returns the artifact id.
func (a NativeArtifact) ID() string { return a.id }

// Family returns the artifact family.
func (a NativeArtifact) Family() Family { return a.family }

// Audience returns the audience.
func (a NativeArtifact) Audience() Audience { return a.audience }

// Destination returns the destination relative to the target root.
func (a NativeArtifact) Destination() string { return a.destination }

// Content returns the native content.
func (a NativeArtifact) Content() Content { return a.content }

func (a NativeArtifact) modelValue() Value {
	return typedRecord("NativeArtifact",
		field("audience", a.audience.modelValue()),
		field("content", modelContent(a.content)),
		field("destination", modelText(a.destination)),
		field("family", modelText(string(a.family))),
		field("id", modelText(a.id)),
	)
}

// SupportAsset is a tree of files that artifacts reference and that is installed beside them.
type SupportAsset struct {
	id      string
	family  Family
	content TreeContent
}

// NewSupportAsset returns a support asset.
func NewSupportAsset(id string, family Family, content TreeContent) (SupportAsset, error) {
	if err := validateIdentifier(id, "asset id"); err != nil {
		return SupportAsset{}, err
	}
	if err := checkFamily(family); err != nil {
		return SupportAsset{}, err
	}
	return SupportAsset{id: id, family: family, content: content}, nil
}

// ID returns the asset id.
func (a SupportAsset) ID() string { return a.id }

// Family returns the asset family.
func (a SupportAsset) Family() Family { return a.family }

// Content returns the asset tree.
func (a SupportAsset) Content() TreeContent { return a.content }

func (a SupportAsset) modelValue() Value {
	return typedRecord("SupportAsset",
		field("content", a.content.modelValue()),
		field("family", modelText(string(a.family))),
		field("id", modelText(a.id)),
	)
}
