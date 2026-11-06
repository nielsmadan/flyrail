package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/nielsmadan/flyrail/go/flyrail"
	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

const (
	program    = "flyrail-checksum"
	appVersion = "2.1.0"
	// Wait for a busy lock (another Flyrail run, or a child briefly inheriting it) instead of failing at once.
	lockTimeout = 5 * time.Second
)

const usage = `Usage:
  flyrail-checksum --version
  flyrail-checksum digest FILE
  flyrail-checksum ai status|install --target TARGET [--target TARGET ...] --state STATE
  flyrail-checksum ai update|uninstall --target TARGET [--target TARGET ...] --state STATE [--replace-modified]
  flyrail-checksum config status|update --project PROJECT --agent AGENT --mcp-command COMMAND --mcp-url URL [options]
  flyrail-checksum config uninstall --project PROJECT --agent AGENT [--replace-modified]

Config options: --node PATH --mcp-arg ARG (repeatable) --guidance TEXT --asset-root PATH
Agents: claude, codex, opencode, pi, cursor, copilot, droid`

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	context.AfterFunc(ctx, stop)
	code := run(ctx, os.Args[1:], os.Stdout, os.Stderr)
	stop()
	os.Exit(code)
}

func run(ctx context.Context, args []string, stdout, stderr io.Writer) int {
	if len(args) == 0 {
		return usageError(stderr, "missing command")
	}
	command, rest := args[0], args[1:]
	switch {
	case command == "--version" && len(rest) == 0:
		if _, err := fmt.Fprintf(stdout, "%s %s\n", program, appVersion); err != nil {
			return failure(stderr, err)
		}
		return 0
	case command == "digest" && len(rest) == 1:
		if err := digest(rest[0], stdout); err != nil {
			return failure(stderr, err)
		}
		return 0
	case command == "ai":
		request, err := parseAI(rest)
		if err != nil {
			return usageError(stderr, err.Error())
		}
		code, err := ai(ctx, request, stdout)
		if err != nil {
			return failure(stderr, err)
		}
		return code
	case command == "config":
		request, err := parseConfig(rest)
		if err != nil {
			return usageError(stderr, err.Error())
		}
		code, err := config(ctx, request, stdout)
		if err != nil {
			return failure(stderr, err)
		}
		return code
	default:
		return usageError(stderr, fmt.Sprintf("invalid command or arguments for '%s'", command))
	}
}

func usageError(stderr io.Writer, message string) int {
	_, _ = fmt.Fprintf(stderr, "%s: %s\n%s\n", program, message, usage)
	return 2
}

func failure(stderr io.Writer, err error) int {
	_, _ = fmt.Fprintf(stderr, "%s: %v\n", program, err)
	return 2
}

func digest(path string, stdout io.Writer) error {
	file, err := os.Open(path) //nolint:gosec // G703: reading the file the user names is this command's job.
	if err != nil {
		return err
	}
	sum, err := sha256Reader(file)
	if err = errors.Join(err, file.Close()); err != nil {
		return err
	}
	_, err = fmt.Fprintln(stdout, sum)
	return err
}

type aiRequest struct {
	action          string
	targets         []string
	state           string
	replaceModified bool
}

func parseAI(args []string) (aiRequest, error) {
	if len(args) == 0 {
		return aiRequest{}, errors.New("missing AI action")
	}
	request := aiRequest{action: args[0]}
	switch request.action {
	case "status", "install", "update", "uninstall":
	default:
		return aiRequest{}, fmt.Errorf("unknown AI action '%s'", request.action)
	}
	flags := flag.NewFlagSet("ai "+request.action, flag.ContinueOnError)
	flags.Var((*stringList)(&request.targets), "target", "skill directory (repeatable)")
	flags.StringVar(&request.state, "state", "", "Flyrail state directory")
	flags.BoolVar(&request.replaceModified, "replace-modified", false, "replace or remove a locally edited skill")
	if err := parseFlags(flags, args[1:]); err != nil {
		return aiRequest{}, err
	}
	switch {
	case len(request.targets) == 0:
		return aiRequest{}, errors.New("at least one --target is required")
	case request.state == "":
		return aiRequest{}, errors.New("--state is required")
	case request.replaceModified && request.action != "update" && request.action != "uninstall":
		return aiRequest{}, fmt.Errorf("%s does not accept --replace-modified", request.action)
	}
	return request, nil
}

func ai(ctx context.Context, request aiRequest, stdout io.Writer) (int, error) {
	targets, err := directoryTargets(request.targets)
	if err != nil {
		return 0, err
	}
	st, err := store.NewDirectoryStore(ctx, request.state)
	if err != nil {
		return 0, err
	}
	options := flyrail.SkillOptions{ReplaceModified: request.replaceModified, LockTimeout: lockTimeout}

	if request.action == "uninstall" {
		results, err := flyrail.Uninstall(ctx, bundleID, targets, st, options)
		if err != nil {
			return 0, err
		}
		return exitCode(allClean(results)), writeJSON(stdout, newReport(bundleID, "", resultReports(results)))
	}

	bundle, err := packagedBundle()
	if err != nil {
		return 0, err
	}
	if bundle.ID() != bundleID {
		return 0, errors.New("packaged bundle ID differs from the host's stable ID")
	}
	var results []flyrail.TargetResult
	switch request.action {
	case "status":
		inspections, err := flyrail.Inspect(ctx, bundle, targets, st)
		if err != nil {
			return 0, err
		}
		current := true
		for _, inspection := range inspections {
			current = current && inspection.Observation.IsCurrent()
		}
		return exitCode(current), writeJSON(stdout, newReport(bundleID, bundle.Version(), inspectionReports(inspections)))
	case "install":
		results, err = flyrail.Install(ctx, bundle, targets, st, flyrail.LockOptions{LockTimeout: lockTimeout})
	default:
		results, err = flyrail.Update(ctx, bundle, targets, st, options)
	}
	if err != nil {
		return 0, err
	}
	return exitCode(allClean(results)), writeJSON(stdout, newReport(bundleID, bundle.Version(), resultReports(results)))
}

type configRequest struct {
	action          string
	project         string
	agent           flyrail.Agent
	node            string
	mcpCommand      string
	mcpArgs         []string
	mcpURL          string
	guidance        string
	assetRoot       string
	replaceModified bool
}

func parseConfig(args []string) (configRequest, error) {
	if len(args) == 0 {
		return configRequest{}, errors.New("missing config action")
	}
	request := configRequest{action: args[0], guidance: guidance}
	switch request.action {
	case "status", "update", "uninstall":
	default:
		return configRequest{}, fmt.Errorf("unknown config action '%s'", request.action)
	}
	flags := flag.NewFlagSet("config "+request.action, flag.ContinueOnError)
	flags.StringVar(&request.project, "project", "", "project directory")
	flags.Func("agent", "agent whose project files to configure", func(value string) error {
		agent, err := flyrail.ParseAgent(value)
		request.agent = agent
		return err
	})
	flags.StringVar(&request.node, "node", "", "Node 24+ executable that runs hooks")
	flags.StringVar(&request.mcpCommand, "mcp-command", "", "stdio MCP server command")
	flags.Var((*stringList)(&request.mcpArgs), "mcp-arg", "stdio MCP server argument (repeatable)")
	flags.StringVar(&request.mcpURL, "mcp-url", "", "HTTP MCP server URL")
	flags.StringVar(&request.guidance, "guidance", guidance, "instruction text")
	flags.StringVar(&request.assetRoot, "asset-root", "", "directory for hook support files")
	flags.BoolVar(&request.replaceModified, "replace-modified", false, "replace or remove locally edited content")
	if err := parseFlags(flags, args[1:]); err != nil {
		return configRequest{}, err
	}
	installOnly := false
	flags.Visit(func(f *flag.Flag) {
		switch f.Name {
		case "node", "mcp-command", "mcp-arg", "mcp-url", "guidance", "asset-root":
			installOnly = true
		}
	})
	switch {
	case request.project == "":
		return configRequest{}, errors.New("--project is required")
	case request.agent == "":
		return configRequest{}, errors.New("--agent is required")
	case request.action == "uninstall" && installOnly:
		return configRequest{}, errors.New("uninstall accepts only project, agent, and replacement options")
	case request.action != "uninstall" && request.mcpCommand == "":
		return configRequest{}, errors.New("--mcp-command is required")
	case request.action != "uninstall" && request.mcpURL == "":
		return configRequest{}, errors.New("--mcp-url is required")
	case request.action == "status" && request.replaceModified:
		return configRequest{}, errors.New("status does not accept --replace-modified")
	}
	return request, nil
}

func config(ctx context.Context, request configRequest, stdout io.Writer) (int, error) {
	project, err := resolvePath(request.project)
	if err != nil {
		return 0, err
	}
	target, err := flyrail.ProjectTarget(request.agent, project)
	if err != nil {
		return 0, err
	}
	st, err := store.NewDirectoryStore(ctx, filepath.Join(project, ".cache", configBundleID))
	if err != nil {
		return 0, err
	}
	report := configReport{
		AppVersion:    appVersion,
		BundleID:      configBundleID,
		Agent:         string(request.agent),
		Project:       project,
		Activation:    "not-checked",
		Prerequisites: []string{},
		Notices:       []noticeJSON{},
		Resources:     []resourceResultJSON{},
	}

	if request.action == "uninstall" {
		rc, err := flyrail.NewRenderContext(target, platform(), flyrail.RenderOptions{})
		if err != nil {
			return 0, err
		}
		installation, err := rc.Installation(st, nil)
		if err != nil {
			return 0, err
		}
		result, err := flyrail.Remove(ctx, configBundleID, installation, flyrail.RemoveOptions{
			ReplaceModified: request.replaceModified,
			LockTimeout:     lockTimeout,
		})
		if err != nil {
			return 0, err
		}
		report.setResult(result)
		return exitCode(operationClean(result)), writeJSON(stdout, report)
	}

	bundle, err := configurationBundle(request.guidance, append([]string{request.mcpCommand}, request.mcpArgs...), request.mcpURL)
	if err != nil {
		return 0, err
	}
	options, err := renderOptions(request)
	if err != nil {
		return 0, err
	}
	rc, err := flyrail.NewRenderContext(target, platform(), options)
	if err != nil {
		return 0, err
	}
	installation, err := rc.Installation(st, nil)
	if err != nil {
		return 0, err
	}
	desired, err := flyrail.Render(bundle, rc)
	if err != nil {
		return 0, err
	}

	ok := true
	var observation flyrail.InstallationObservation
	if request.action == "update" {
		result, err := flyrail.Sync(ctx, bundle, desired, installation, flyrail.SyncOptions{
			Acquisition:     flyrail.AcquireConflict,
			ReplaceModified: request.replaceModified,
			LockTimeout:     lockTimeout,
		})
		if err != nil {
			return 0, err
		}
		report.setResult(result)
		ok = operationClean(result)
		observation = result.Observation
	} else {
		observation, err = flyrail.InspectInstallation(ctx, configBundleID, installation)
		if err != nil {
			return 0, err
		}
	}
	matches := observation.Matches(bundle, desired)
	version := bundle.Version()
	report.BundleVersion = &version
	report.ConfiguredCurrent = &matches
	report.RecordedCurrent = observation.Current
	report.Observation = newInstallationObservationJSON(observation)
	report.Notices = convert(desired.Notices(), newNoticeJSON)
	report.Prerequisites = prerequisites(desired.Notices())
	return exitCode(ok && matches), writeJSON(stdout, report)
}

func (r *configReport) setResult(result flyrail.InstallationResult) {
	status := string(result.Status)
	r.Status = &status
	r.Error = newErrorJSON(result.Err)
	r.Resources = convert(result.Resources, newResourceResultJSON)
	r.Observation = newInstallationObservationJSON(result.Observation)
	r.RecordedCurrent = result.Observation.Current
}

func renderOptions(request configRequest) (flyrail.RenderOptions, error) {
	node := ""
	if request.node != "" {
		resolved, err := resolvePath(request.node)
		if err != nil {
			return flyrail.RenderOptions{}, err
		}
		node = resolved
	}
	shell := flyrail.ShellPOSIX
	if runtime.GOOS == "windows" {
		shell = flyrail.ShellCmd
	}
	hookRuntime, err := flyrail.NewHookRuntime(node, shell)
	if err != nil {
		return flyrail.RenderOptions{}, err
	}
	options := flyrail.RenderOptions{HookRuntime: hookRuntime}
	if request.assetRoot != "" {
		if options.AssetRoot, err = resolvePath(request.assetRoot); err != nil {
			return flyrail.RenderOptions{}, err
		}
	}
	return options, nil
}

func prerequisites(notices []flyrail.Notice) []string {
	items := []string{
		"Provide the selected stdio MCP server and HTTP endpoint.",
		"Provide CHECKSUM_MCP_TOKEN and CHECKSUM_HTTP_TOKEN in the agent environment.",
		"Keep Python 3 available for the bundled hook.",
	}
	for _, notice := range notices {
		if notice.Kind == flyrail.NoticePrerequisite {
			items = append(items, notice.Message)
		}
	}
	return items
}

func platform() flyrail.Platform {
	switch runtime.GOOS {
	case "darwin":
		return flyrail.PlatformMacOS
	case "windows":
		return flyrail.PlatformWindows
	default:
		return flyrail.PlatformLinux
	}
}

func operationClean(result flyrail.InstallationResult) bool {
	if !result.Succeeded() || result.Err != nil || result.Observation.Err != nil {
		return false
	}
	for _, resource := range result.Resources {
		if resource.Err != nil || len(resource.RecoveryPaths) > 0 {
			return false
		}
	}
	return true
}

func directoryTargets(roots []string) ([]flyrail.Target, error) {
	targets := make([]flyrail.Target, 0, len(roots))
	for _, root := range roots {
		target, err := flyrail.DirectoryTarget(root)
		if err != nil {
			return nil, err
		}
		targets = append(targets, target)
	}
	return targets, nil
}

func allClean(results []flyrail.TargetResult) bool {
	for _, result := range results {
		if !result.Succeeded() || result.Err != nil || result.Observation.Err != nil || len(result.RecoveryPaths) > 0 {
			return false
		}
	}
	return true
}

func exitCode(ok bool) int {
	if ok {
		return 0
	}
	return 1
}

type stringList []string

func (l *stringList) String() string {
	return strings.Join(*l, ",")
}

func (l *stringList) Set(value string) error {
	*l = append(*l, value)
	return nil
}

func parseFlags(flags *flag.FlagSet, args []string) error {
	for len(args) > 0 {
		arg := args[0]
		args = args[1:]
		var option *flag.Flag
		if strings.HasPrefix(arg, "--") {
			option = flags.Lookup(arg[2:])
		}
		if option == nil {
			return fmt.Errorf("unknown option '%s'", arg)
		}
		value := "true"
		if !isBoolFlag(option) {
			if len(args) == 0 {
				return fmt.Errorf("%s requires a value", arg)
			}
			value = args[0]
			args = args[1:]
			if value == "--" && len(args) > 0 {
				value = args[0]
				args = args[1:]
			} else if strings.HasPrefix(value, "--") {
				return fmt.Errorf("%s requires a value", arg)
			}
		}
		if err := flags.Set(option.Name, value); err != nil {
			return err
		}
	}
	return nil
}

func isBoolFlag(f *flag.Flag) bool {
	b, ok := f.Value.(interface{ IsBoolFlag() bool })
	return ok && b.IsBoolFlag()
}
