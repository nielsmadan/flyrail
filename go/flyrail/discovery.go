package flyrail

import (
	"context"
	"os"
	"runtime"
	"slices"
	"strings"

	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

// DiscoveryOptions are the explicit search inputs of DetectAgents; the library never reads PATH or the home directory itself.
type DiscoveryOptions struct {
	SearchPath     []string // nil = search nothing; "" entries mean "."; pass filepath.SplitList(os.Getenv("PATH")) for the ambient PATH
	Applications   []string // application directories; nil or empty = /Applications and <home>/Applications (the latter only when home != "") unless NoApplications
	NoApplications bool     // true = skip application discovery
}

// AgentPresence is the evidence that an agent is installed; an empty path means no such evidence.
type AgentPresence struct {
	Agent       Agent
	Executable  string
	Application string
}

// DetectAgents reports, in Agents() order, each agent with an executable on opts.SearchPath or, on macOS, an application bundle.
func DetectAgents(ctx context.Context, home string, opts DiscoveryOptions) ([]AgentPresence, error) {
	if err := requireContext(ctx); err != nil {
		return nil, err
	}
	directories := make([]string, 0, len(opts.SearchPath))
	for _, entry := range opts.SearchPath {
		if entry == "" {
			entry = "."
		}
		directories = append(directories, entry)
	}
	var applications []string
	switch {
	case opts.NoApplications:
	case len(opts.Applications) > 0:
		applications = opts.Applications
	default:
		applications = []string{"/Applications"}
		if home != "" {
			applications = append(applications, pathx.Join(home, "Applications"))
		}
	}
	if slices.ContainsFunc(applications, func(directory string) bool { return !pathx.IsAbsolute(directory) }) {
		return nil, newValueError("agent presence evidence must be an absolute Path")
	}
	var found []AgentPresence
	for _, agent := range Agents() {
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		presence := AgentPresence{
			Agent:       agent,
			Executable:  findExecutable(string(agent), directories),
			Application: applicationEvidence(agent, applications),
		}
		if presence.Executable == "" && presence.Application == "" {
			continue
		}
		if presence.Executable != "" && !pathx.IsAbsolute(presence.Executable) ||
			presence.Application != "" && !pathx.IsAbsolute(presence.Application) {
			return nil, newValueError("agent presence evidence must be an absolute Path")
		}
		found = append(found, presence)
	}
	return found, nil
}

func findExecutable(name string, directories []string) string {
	for _, directory := range directories {
		candidate := pathx.Join(directory, name)
		if sysx.IsExecutable(candidate) {
			return lexicalAbsolute(candidate)
		}
	}
	return ""
}

// Folds "." and ".." lexically, as Rust's discovery does.
func lexicalAbsolute(path string) string {
	if !pathx.IsAbsolute(path) {
		if current, err := sysx.Getwd(); err == nil {
			path = pathx.Join(current, path)
		}
	}
	var parts []string
	for _, component := range pathx.Components(path) {
		switch component {
		case "/", ".":
		case "..":
			if len(parts) > 0 {
				parts = parts[:len(parts)-1]
			}
		default:
			parts = append(parts, component)
		}
	}
	if !pathx.IsAbsolute(path) {
		return strings.Join(parts, "/")
	}
	return "/" + strings.Join(parts, "/")
}

func applicationEvidence(agent Agent, directories []string) string {
	if runtime.GOOS != "darwin" {
		return ""
	}
	var name string
	switch agent {
	case AgentCursor:
		name = "Cursor.app"
	case AgentCopilot:
		name = "Visual Studio Code.app"
	default:
		return ""
	}
	for _, directory := range directories {
		candidate := pathx.Join(directory, name)
		if info, err := os.Stat(candidate); err == nil && info.IsDir() {
			return candidate
		}
	}
	return ""
}
