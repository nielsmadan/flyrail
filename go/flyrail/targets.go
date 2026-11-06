package flyrail

import (
	"slices"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

// Agent names a supported coding agent.
type Agent string

// The supported agents.
const (
	AgentClaude   Agent = "claude"
	AgentCodex    Agent = "codex"
	AgentOpencode Agent = "opencode"
	AgentPi       Agent = "pi"
	AgentCursor   Agent = "cursor"
	AgentCopilot  Agent = "copilot"
	AgentDroid    Agent = "droid"
)

// Agents returns every supported agent in Rust's declaration order.
func Agents() []Agent {
	return []Agent{AgentClaude, AgentCodex, AgentOpencode, AgentPi, AgentCursor, AgentCopilot, AgentDroid}
}

// ParseAgent returns the agent spelled s.
func ParseAgent(s string) (Agent, error) {
	return parseEnum(s, "Agent", Agents()...)
}

// TargetScope is the kind of destination a target installs into.
type TargetScope string

// The target scopes.
const (
	ScopeDirectory TargetScope = "directory"
	ScopeUser      TargetScope = "user"
	ScopeProject   TargetScope = "project"
)

// ParseTargetScope returns the target scope spelled s.
func ParseTargetScope(s string) (TargetScope, error) {
	return parseEnum(s, "TargetScope", ScopeDirectory, ScopeUser, ScopeProject)
}

// Surface is the agent front end a configuration is rendered for.
type Surface string

// The surfaces.
const (
	SurfaceCLI    Surface = "cli"
	SurfaceIDE    Surface = "ide"
	SurfaceVSCode Surface = "vscode"
)

// ParseSurface returns the surface spelled s.
func ParseSurface(s string) (Surface, error) {
	return parseEnum(s, "Surface", SurfaceCLI, SurfaceIDE, SurfaceVSCode)
}

// Platform is an operating system a configuration is rendered for.
type Platform string

// The platforms.
const (
	PlatformMacOS   Platform = "macos"
	PlatformLinux   Platform = "linux"
	PlatformWindows Platform = "windows"
)

// ParsePlatform returns the platform spelled s.
func ParsePlatform(s string) (Platform, error) {
	return parseEnum(s, "Platform", PlatformMacOS, PlatformLinux, PlatformWindows)
}

func parseEnum[T ~string](s, typeName string, values ...T) (T, error) {
	if slices.Contains(values, T(s)) {
		return T(s), nil
	}
	return "", newValueError("'" + s + "' is not a valid " + typeName)
}

func checkEnum[T ~string](value T, typeName string, values ...T) error {
	_, err := parseEnum(string(value), typeName, values...)
	return err
}

func validateRoute(path string) (string, error) {
	if !strings.HasPrefix(path, "/") {
		return "", newValueError("render routes must be absolute Paths")
	}
	if !utf8.ValidString(path) || strings.Contains(path, "\x00") || slices.Contains(strings.Split(path, "/"), "..") {
		return "", newValueError("render routes must be unambiguous paths without NUL")
	}
	return path, nil
}

var environmentKeys = []string{
	"HOME",
	"USERPROFILE",
	"CLAUDE_CONFIG_DIR",
	"CODEX_HOME",
	"XDG_CONFIG_HOME",
	"OPENCODE_CONFIG",
	"OPENCODE_CONFIG_DIR",
	"PI_CODING_AGENT_DIR",
	"COPILOT_HOME",
}

type contextEntry struct {
	key   string
	value string
}

func contextEntries(context map[string]string) []contextEntry {
	entries := make([]contextEntry, 0, len(context))
	for key, value := range context {
		entries = append(entries, contextEntry{key: key, value: value})
	}
	slices.SortFunc(entries, func(a, b contextEntry) int { return strings.Compare(a.key, b.key) })
	return entries
}

func contextMap(entries []contextEntry) map[string]string {
	context := make(map[string]string, len(entries))
	for _, entry := range entries {
		context[entry.key] = entry.value
	}
	return context
}

func validContextEntries(entries []contextEntry) bool {
	return !slices.ContainsFunc(entries, func(entry contextEntry) bool {
		return !utf8.ValidString(entry.key) || !utf8.ValidString(entry.value)
	})
}

// Target is a skill container destination: a plain directory, or an agent's project or user skills directory.
type Target struct {
	root        string
	agent       Agent
	scope       TargetScope
	anchor      string
	suffix      []string
	home        string
	environment []contextEntry
}

// DirectoryTarget returns a target for the skill container directory root, made absolute against the working directory.
func DirectoryTarget(root string) (Target, error) {
	absolute, err := absoluteTargetPath(root)
	if err != nil {
		return Target{}, err
	}
	return newTarget(absolute, nil, "", ScopeDirectory, "", nil)
}

// ProjectTarget returns the project skills directory of agent below the project root.
func ProjectTarget(agent Agent, root string) (Target, error) {
	if err := checkEnum(agent, "Agent", Agents()...); err != nil {
		return Target{}, err
	}
	absolute, err := absoluteTargetPath(root)
	if err != nil {
		return Target{}, err
	}
	suffix, err := projectSuffix(agent)
	if err != nil {
		return Target{}, err
	}
	return newTarget(absolute, suffix, agent, ScopeProject, "", nil)
}

// UserTarget returns the user skills directory of agent below home, applying the agent's relocation variable from lookupEnv.
// An empty home is read from lookupEnv's HOME, then USERPROFILE, and must be absolute; a nil lookupEnv means no variables.
func UserTarget(agent Agent, home string, lookupEnv func(string) (string, bool)) (Target, error) {
	if err := checkEnum(agent, "Agent", Agents()...); err != nil {
		return Target{}, err
	}
	environment, err := lookupEnvironment(lookupEnv)
	if err != nil {
		return Target{}, err
	}
	if home != "" {
		absolute, err := absoluteTargetPath(home)
		if err != nil {
			return Target{}, err
		}
		return userTarget(agent, absolute, environment)
	}
	value, found := environmentValueOf(environment, "HOME")
	if !found {
		value, found = environmentValueOf(environment, "USERPROFILE")
	}
	if !found {
		return Target{}, newValueError("an explicit env requires home, HOME, or USERPROFILE")
	}
	if !pathx.IsAbsolute(value) {
		return Target{}, newValueError("environment home must be absolute")
	}
	absolute, err := absoluteTargetPath(value)
	if err != nil {
		return Target{}, err
	}
	return userTarget(agent, absolute, environment)
}

func lookupEnvironment(lookupEnv func(string) (string, bool)) ([]contextEntry, error) {
	if lookupEnv == nil {
		return nil, nil
	}
	var environment []contextEntry
	for _, key := range environmentKeys {
		value, ok := lookupEnv(key)
		if !ok {
			continue
		}
		if !utf8.ValidString(value) {
			return nil, newValueError("env keys and values must be strings")
		}
		environment = append(environment, contextEntry{key: key, value: value})
	}
	slices.SortFunc(environment, func(a, b contextEntry) int { return strings.Compare(a.key, b.key) })
	return environment, nil
}

func environmentValueOf(environment []contextEntry, key string) (string, bool) {
	index := slices.IndexFunc(environment, func(entry contextEntry) bool { return entry.key == key })
	if index < 0 {
		return "", false
	}
	return environment[index].value, true
}

func userTarget(agent Agent, home string, environment []contextEntry) (Target, error) {
	suffix, err := userSuffix(agent)
	if err != nil {
		return Target{}, err
	}
	variable, relocatedSuffix, err := relocation(agent)
	if err != nil {
		return Target{}, err
	}
	anchor := home
	if variable != "" {
		if value, found := environmentValueOf(environment, variable); found && value != "" {
			relocatedAnchor, err := relocated(value, home)
			if err != nil {
				return Target{}, err
			}
			anchor, suffix = relocatedAnchor, relocatedSuffix
		}
	}
	return newTarget(anchor, suffix, agent, ScopeUser, home, environment)
}

func newTarget(anchor string, suffix []string, agent Agent, scope TargetScope, home string, environment []contextEntry) (Target, error) {
	root := pathx.Join(anchor, suffix...)
	if !hasParent(root) {
		return Target{}, newValueError("a filesystem root cannot be a skill container")
	}
	return Target{
		root:        root,
		agent:       agent,
		scope:       scope,
		anchor:      anchor,
		suffix:      slices.Clone(suffix),
		home:        home,
		environment: slices.Clone(environment),
	}, nil
}

// Root returns the absolute skill container directory.
func (t Target) Root() string { return t.root }

// Agent returns the agent, or "" for a directory target.
func (t Target) Agent() Agent { return t.agent }

// Scope returns the target scope.
func (t Target) Scope() TargetScope { return t.scope }

// Home returns the home directory of a user target, or "" for none.
func (t Target) Home() string { return t.home }

// Environment returns a copy of the retained relocation variables.
func (t Target) Environment() map[string]string { return contextMap(t.environment) }

// Equal reports whether t and o name the same container the same way; home and environment are not compared.
func (t Target) Equal(o Target) bool {
	return pathsEqual(t.root, o.root) && t.agent == o.agent && t.scope == o.scope &&
		pathsEqual(t.anchor, o.anchor) && slices.Equal(t.suffix, o.suffix)
}

func (t Target) environmentValue(key string) (string, bool) {
	return environmentValueOf(t.environment, key)
}

func absoluteTargetPath(path string) (string, error) {
	if !utf8.ValidString(path) {
		return "", newValueError("target roots must be string paths")
	}
	if path == "" || strings.Contains(path, "\x00") {
		return "", newValueError("target roots must be nonempty paths without NUL")
	}
	if pathx.IsAbsolute(path) {
		return path, nil
	}
	current, err := sysx.Getwd()
	if err != nil {
		return "", newValueError(sysx.RustOSErrorText(err))
	}
	return pathx.Join(current, path), nil
}

func relocated(value, home string) (string, error) {
	if value == "~" {
		return home, nil
	}
	if suffix, ok := strings.CutPrefix(value, "~/"); ok {
		return absoluteTargetPath(pathx.Join(home, suffix))
	}
	if !pathx.IsAbsolute(value) {
		return "", newValueError("environment paths must be absolute or start with ~/")
	}
	return absoluteTargetPath(value)
}

func unhandledAgent(agent Agent) error {
	return newValueError("unhandled Agent: " + strconv.Quote(string(agent)))
}

func projectSuffix(agent Agent) ([]string, error) {
	switch agent {
	case AgentClaude:
		return []string{".claude", "skills"}, nil
	case AgentCodex:
		return []string{".agents", "skills"}, nil
	case AgentOpencode:
		return []string{".opencode", "skills"}, nil
	case AgentPi:
		return []string{".pi", "skills"}, nil
	case AgentCursor:
		return []string{".cursor", "skills"}, nil
	case AgentCopilot:
		return []string{".github", "skills"}, nil
	case AgentDroid:
		return []string{".factory", "skills"}, nil
	default:
		return nil, unhandledAgent(agent)
	}
}

func userSuffix(agent Agent) ([]string, error) {
	switch agent {
	case AgentClaude:
		return []string{".claude", "skills"}, nil
	case AgentCodex:
		return []string{".agents", "skills"}, nil
	case AgentOpencode:
		return []string{".config", "opencode", "skills"}, nil
	case AgentPi:
		return []string{".pi", "agent", "skills"}, nil
	case AgentCursor:
		return []string{".cursor", "skills"}, nil
	case AgentCopilot:
		return []string{".copilot", "skills"}, nil
	case AgentDroid:
		return []string{".factory", "skills"}, nil
	default:
		return nil, unhandledAgent(agent)
	}
}

func relocation(agent Agent) (variable string, suffix []string, err error) {
	switch agent {
	case AgentClaude:
		return "CLAUDE_CONFIG_DIR", []string{"skills"}, nil
	case AgentOpencode:
		return "XDG_CONFIG_HOME", []string{"opencode", "skills"}, nil
	case AgentPi:
		return "PI_CODING_AGENT_DIR", []string{"skills"}, nil
	case AgentCopilot:
		return "COPILOT_HOME", []string{"skills"}, nil
	case AgentCodex, AgentCursor, AgentDroid:
		return "", nil, nil
	default:
		return "", nil, unhandledAgent(agent)
	}
}
