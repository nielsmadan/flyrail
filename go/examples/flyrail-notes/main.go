package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/flyrail"
	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

const (
	program    = "flyrail-notes"
	appVersion = "0.4.0"
	// Wait for a busy lock (another Flyrail run, or a child briefly inheriting it) instead of failing at once.
	lockTimeout = 5 * time.Second
)

const usage = `Usage:
  flyrail-notes --version
  flyrail-notes words FILE
  flyrail-notes ai status --source SOURCE --target TARGET [--target TARGET ...] --state STATE
  flyrail-notes ai install --source SOURCE --target TARGET [--target TARGET ...] --state STATE
  flyrail-notes ai update --source SOURCE --target TARGET [--target TARGET ...] --state STATE [--replace-modified]
  flyrail-notes ai uninstall --bundle-id ID --target TARGET [--target TARGET ...] --state STATE [--replace-modified]`

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
	case command == "words" && len(rest) == 1:
		if err := words(rest[0], stdout); err != nil {
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
	default:
		return usageError(stderr, fmt.Sprintf("invalid command or arguments for '%s'", command))
	}
}

func failure(stderr io.Writer, err error) int {
	_, _ = fmt.Fprintf(stderr, "%s: %v\n", program, err)
	return 2
}

func usageError(stderr io.Writer, message string) int {
	_, _ = fmt.Fprintf(stderr, "%s: %s\n%s\n", program, message, usage)
	return 2
}

func words(path string, stdout io.Writer) error {
	data, err := os.ReadFile(path) //nolint:gosec // G703: reading the file the user names is this command's job.
	if err != nil {
		return err
	}
	if !utf8.Valid(data) {
		return fmt.Errorf("%s is not valid UTF-8", path)
	}
	_, err = fmt.Fprintln(stdout, len(strings.Fields(string(data))))
	return err
}

type aiRequest struct {
	action          string
	source          string
	bundleID        string
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
	flags.StringVar(&request.source, "source", "", "bundle directory")
	flags.StringVar(&request.bundleID, "bundle-id", "", "installed bundle ID (uninstall)")
	flags.Var((*stringList)(&request.targets), "target", "skill directory (repeatable)")
	flags.StringVar(&request.state, "state", "", "Flyrail state directory")
	flags.BoolVar(&request.replaceModified, "replace-modified", false, "replace or remove locally edited skills")
	if err := parseFlags(flags, args[1:]); err != nil {
		return aiRequest{}, err
	}
	switch {
	case len(request.targets) == 0:
		return aiRequest{}, errors.New("at least one --target is required")
	case request.state == "":
		return aiRequest{}, errors.New("--state is required")
	case request.action == "uninstall" && request.source != "":
		return aiRequest{}, errors.New("uninstall does not accept --source")
	case request.action == "uninstall" && request.bundleID == "":
		return aiRequest{}, errors.New("uninstall requires --bundle-id")
	case request.action != "uninstall" && request.bundleID != "":
		return aiRequest{}, errors.New("only uninstall accepts --bundle-id")
	case request.action != "uninstall" && request.source == "":
		return aiRequest{}, fmt.Errorf("%s requires --source", request.action)
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
		results, err := flyrail.Uninstall(ctx, request.bundleID, targets, st, options)
		if err != nil {
			return 0, err
		}
		return exitCode(allClean(results)), writeJSON(stdout, newReport(request.bundleID, "", resultReports(results)))
	}

	bundle, err := flyrail.OpenBundle(ctx, request.source)
	if err != nil {
		return 0, err
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
		return exitCode(current), writeJSON(stdout, newReport(bundle.ID(), bundle.Version(), inspectionReports(inspections)))
	case "install":
		results, err = flyrail.Install(ctx, bundle, targets, st, flyrail.LockOptions{LockTimeout: lockTimeout})
	default:
		results, err = flyrail.Update(ctx, bundle, targets, st, options)
	}
	if err != nil {
		return 0, err
	}
	return exitCode(allClean(results)), writeJSON(stdout, newReport(bundle.ID(), bundle.Version(), resultReports(results)))
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
