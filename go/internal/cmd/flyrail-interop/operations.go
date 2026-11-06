package main

import (
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"io"
	"os"
	"slices"
	"strconv"
	"sync"
	"time"

	"github.com/nielsmadan/flyrail/go/flyrail"
	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/interopbridge"
	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/seams"
)

const (
	maxPayloadSize   = 1024 * 1024
	maxStateInput    = 16 * 1024 * 1024
	maxObserveRepeat = 1000
	payloadPattern   = "flyrail-interop-payload\n"
)

var sourceIncompatible = []string{"--mode", "--large", "--kind", "--section", "--payload", "--version"}

var barrierBoundaries = map[string]string{"prepare": seams.Prepare, "receipt-committed": seams.ReceiptCommitted}

type operation func(ctx context.Context, o options, stdin io.Reader, stdout io.Writer) error

var operations = map[string]operation{
	"encode":  encodeCommand,
	"write":   writeCommand,
	"install": writeCommand,
	"update":  writeCommand,
	"inspect": inspectCommand,
	"remove":  removeCommand,
	"recover": recoverCommand,
	"records": recordsCommand,
	"hold":    holdCommand,
	"observe": observeCommand,
}

func emit(stdout io.Writer, value jsonx.Value) error {
	_, err := stdout.Write(append(jsonx.Encode(value), '\n'))
	return err
}

func failureMessage(err error) string {
	if failed, ok := errors.AsType[*flyrail.TargetError](err); ok {
		return failed.Message
	}
	return err.Error()
}

func encodeCommand(_ context.Context, o options, stdin io.Reader, stdout io.Writer) error {
	input, err := io.ReadAll(io.LimitReader(stdin, maxStateInput+1))
	if err != nil {
		return fmt.Errorf("failed to read stdin: %w", err)
	}
	if len(input) > maxStateInput {
		return errors.New("JSON input exceeds the byte limit")
	}
	canonical, _, err := interopbridge.TranscodeStateRecord(input, o.value("--type", ""))
	if err != nil {
		return err
	}
	_, err = stdout.Write(canonical)
	return err
}

func installationTarget(ctx context.Context, o options) (*flyrail.InstallationTarget, error) {
	state, err := o.required("--state")
	if err != nil {
		return nil, err
	}
	st, err := store.NewDirectoryStore(ctx, state)
	if err != nil {
		return nil, err
	}
	return flyrail.NewInstallationTarget(st, nil, nil)
}

func payload(o options) ([]byte, error) {
	text, large := o.values["--large"]
	if !large {
		data := []byte(o.value("--payload", "generated\n"))
		if len(data) > maxPayloadSize {
			return nil, fmt.Errorf("payload must not exceed %d bytes", maxPayloadSize)
		}
		return data, nil
	}
	size, err := strconv.ParseInt(text, 10, 64)
	switch {
	case err != nil:
		return nil, errors.New("invalid --large")
	case size < 0:
		return nil, errors.New("large payload size must be nonnegative")
	case size > maxPayloadSize:
		return nil, fmt.Errorf("large payload size must not exceed %d", maxPayloadSize)
	}
	data := make([]byte, size)
	for index := range data {
		data[index] = payloadPattern[index%len(payloadPattern)]
	}
	return data, nil
}

func generatedContent(o options, kind string) (flyrail.Content, flyrail.Family, error) {
	mode, err := flyrail.ParseFileMode(o.value("--mode", "readable"))
	if err != nil {
		return nil, "", fmt.Errorf("invalid mode: %s", o.value("--mode", "readable"))
	}
	if kind != "document" && kind != "tree" {
		return nil, "", errors.New("invalid resource kind")
	}
	marker, section := o.values["--section"]
	if section && kind == "tree" {
		return nil, "", errors.New("tree resources cannot have a section")
	}
	data, err := payload(o)
	if err != nil {
		return nil, "", err
	}
	switch {
	case section:
		content, err := flyrail.NewSectionContent(marker, string(data), nil)
		return content, flyrail.FamilyInstructions, err
	case kind == "tree":
		content, err := flyrail.NewTreeContent([]flyrail.BundleEntry{
			{Path: "empty", Dir: true},
			{Path: "nested/file", Data: data, Executable: mode == flyrail.ModeExecutable},
		})
		return content, flyrail.FamilySkills, err
	}
	content, err := flyrail.NewFileContent(data, mode)
	return content, flyrail.FamilyInstructions, err
}

func generatedInstallation(o options, bundleID string) (*flyrail.Bundle, *flyrail.RenderedBundle, error) {
	if o.has("--agent") || o.has("--node") {
		return nil, nil, errors.New("--agent and --node require --source")
	}
	identity, err := flyrail.NewBundleIdentity(bundleID, o.value("--version", "1"))
	if err != nil {
		return nil, nil, err
	}
	instruction, err := flyrail.NewInstructionArtifact("guide", "text")
	if err != nil {
		return nil, nil, err
	}
	bundle, err := flyrail.NewBundle(identity, []flyrail.Artifact{instruction}, nil, nil)
	if err != nil {
		return nil, nil, err
	}
	destination, err := o.absolute("--destination")
	if err != nil {
		return nil, nil, err
	}
	content, family, err := generatedContent(o, o.value("--kind", "document"))
	if err != nil {
		return nil, nil, err
	}
	artifact, err := flyrail.NewRenderedArtifact(flyrail.RenderedArtifactParams{
		ID: "guide", Family: family, Destination: destination, Content: content,
	})
	if err != nil {
		return nil, nil, err
	}
	rendered, err := flyrail.NewRenderedBundle([]flyrail.RenderedArtifact{artifact}, nil, nil, nil)
	return bundle, rendered, err
}

func sourceInstallation(ctx context.Context, o options, bundleID string) (*flyrail.Bundle, *flyrail.RenderedBundle, error) {
	source, err := o.absolute("--source")
	if err != nil {
		return nil, nil, err
	}
	bundle, err := flyrail.OpenBundle(ctx, source)
	if err != nil {
		return nil, nil, err
	}
	if bundle.ID() != bundleID {
		return nil, nil, errors.New("source bundle id must match --bundle-id")
	}
	agent, err := flyrail.ParseAgent(o.value("--agent", "codex"))
	if err != nil {
		return nil, nil, err
	}
	destination, err := o.absolute("--destination")
	if err != nil {
		return nil, nil, err
	}
	target, err := flyrail.ProjectTarget(agent, destination)
	if err != nil {
		return nil, nil, err
	}
	runtime, err := flyrail.NewHookRuntime(o.value("--node", ""), "")
	if err != nil {
		return nil, nil, err
	}
	renderContext, err := flyrail.NewRenderContext(target, flyrail.PlatformMacOS, flyrail.RenderOptions{HookRuntime: runtime})
	if err != nil {
		return nil, nil, err
	}
	rendered, err := flyrail.Render(bundle, renderContext)
	return bundle, rendered, err
}

func barrier(ready, release string, wait time.Duration) error {
	if err := os.WriteFile(ready, []byte("ready\n"), 0o600); err != nil {
		return err
	}
	deadline := time.Now().Add(wait)
	for {
		if _, err := os.Stat(release); err == nil {
			return nil
		}
		if !time.Now().Before(deadline) {
			return seams.BarrierTimedOut{Path: release}
		}
		time.Sleep(5 * time.Millisecond)
	}
}

func transactionSeam(o options) (*seams.Hooks, error) {
	phase, hasPhase := o.values["--barrier-phase"]
	ready, hasReady := o.values["--barrier-ready"]
	release, hasRelease := o.values["--barrier-release"]
	hasTimeout := o.has("--barrier-timeout")
	if o.flags["--fail-index-completion"] {
		if hasPhase || hasReady || hasRelease || hasTimeout {
			return nil, errors.New("fault and barrier options cannot be combined")
		}
		return &seams.Hooks{Fault: func(boundary string) bool { return boundary == seams.IndexCompletion }}, nil
	}
	if !hasPhase {
		if hasReady || hasRelease {
			return nil, errors.New("--barrier-ready and --barrier-release require --barrier-phase")
		}
		if hasTimeout {
			return nil, errors.New("--barrier-timeout requires --barrier-phase")
		}
		return nil, nil
	}
	boundary, known := barrierBoundaries[phase]
	if !known {
		return nil, fmt.Errorf("invalid --barrier-phase: %s", phase)
	}
	if !hasReady || !hasRelease {
		return nil, errors.New("--barrier-phase requires --barrier-ready and --barrier-release")
	}
	wait, err := o.duration("--barrier-timeout", "30")
	if err != nil {
		return nil, err
	}
	readyPath, err := absolute(ready)
	if err != nil {
		return nil, err
	}
	releasePath, err := absolute(release)
	if err != nil {
		return nil, err
	}
	var once sync.Once
	var failure error
	return &seams.Hooks{Fail: func(selected string) error {
		if selected != boundary {
			return nil
		}
		once.Do(func() { failure = barrier(readyPath, releasePath, wait) })
		return failure
	}}, nil
}

func writeCommand(ctx context.Context, o options, _ io.Reader, stdout io.Writer) error {
	_, fromSource := o.values["--source"]
	if fromSource {
		if index := slices.IndexFunc(sourceIncompatible, o.has); index >= 0 {
			return fmt.Errorf("%s cannot be combined with --source", sourceIncompatible[index])
		}
	}
	target, err := installationTarget(ctx, o)
	if err != nil {
		return err
	}
	acquisition, err := flyrail.ParseAcquisition(o.value("--acquisition", "conflict"))
	if err != nil {
		return err
	}
	bundleID := o.value("--bundle-id", "team")
	var bundle *flyrail.Bundle
	var rendered *flyrail.RenderedBundle
	if fromSource {
		bundle, rendered, err = sourceInstallation(ctx, o, bundleID)
	} else {
		bundle, rendered, err = generatedInstallation(o, bundleID)
	}
	if err != nil {
		return err
	}
	timeout, err := o.duration("--lock-timeout", "0")
	if err != nil {
		return err
	}
	hooks, err := transactionSeam(o)
	if err != nil {
		return err
	}
	if hooks != nil {
		ctx = seams.With(ctx, hooks)
	}
	result, err := flyrail.Sync(ctx, bundle, rendered, target, flyrail.SyncOptions{
		Acquisition: acquisition, ReplaceModified: o.flags["--replace-modified"], LockTimeout: timeout,
	})
	if err != nil {
		return err
	}
	return emit(stdout, installationResultValue(result))
}

func inspectCommand(ctx context.Context, o options, _ io.Reader, stdout io.Writer) error {
	target, err := installationTarget(ctx, o)
	if err != nil {
		return err
	}
	if _, err := o.duration("--lock-timeout", "0"); err != nil {
		return err
	}
	observation, err := flyrail.InspectInstallation(ctx, o.value("--bundle-id", "team"), target)
	if err != nil {
		return err
	}
	return emit(stdout, observationValue(observation))
}

func removeCommand(ctx context.Context, o options, _ io.Reader, stdout io.Writer) error {
	target, err := installationTarget(ctx, o)
	if err != nil {
		return err
	}
	timeout, err := o.duration("--lock-timeout", "0")
	if err != nil {
		return err
	}
	result, err := flyrail.Remove(ctx, o.value("--bundle-id", "team"), target, flyrail.RemoveOptions{
		ReplaceModified: o.flags["--replace-modified"], LockTimeout: timeout,
	})
	if err != nil {
		return err
	}
	return emit(stdout, installationResultValue(result))
}

func recoverCommand(ctx context.Context, o options, _ io.Reader, stdout io.Writer) error {
	target, err := installationTarget(ctx, o)
	if err != nil {
		return err
	}
	timeout, err := o.duration("--lock-timeout", "0")
	if err != nil {
		return err
	}
	result, err := flyrail.RecoverInstallation(ctx, o.value("--bundle-id", "team"), target, flyrail.LockOptions{LockTimeout: timeout})
	if err != nil {
		return err
	}
	return emit(stdout, installationResultValue(result))
}

func recordsCommand(ctx context.Context, o options, _ io.Reader, stdout io.Writer) error {
	state, err := o.required("--state")
	if err != nil {
		return err
	}
	st, err := store.NewDirectoryStore(ctx, state)
	if err != nil {
		return err
	}
	keys, err := st.Keys(ctx, "")
	if err != nil {
		return err
	}
	records := make([]jsonx.Value, 0, len(keys))
	for _, key := range keys {
		raw, ok, err := st.Read(ctx, key)
		if err != nil {
			return err
		}
		if !ok {
			continue
		}
		canonical, tag, err := interopbridge.TranscodeStateRecord(raw, "")
		if err != nil {
			return fmt.Errorf("%s: %w", key, err)
		}
		records = append(records, jsonx.Object(
			member("key", jsonx.String(key)),
			member("type", jsonx.String(tag)),
			member("bytes", jsonx.String(base64.StdEncoding.EncodeToString(raw))),
			member("canonical", jsonx.String(base64.StdEncoding.EncodeToString(canonical))),
		))
	}
	return emit(stdout, jsonx.Object(member("records", jsonx.Array(records...))))
}

func resourceKind(o options) (string, error) {
	kind := o.value("--kind", "document")
	if _, err := flyrail.ParseResourceKind(kind); err != nil {
		return "", err
	}
	return kind, nil
}

func holdCommand(ctx context.Context, o options, _ io.Reader, stdout io.Writer) error {
	state, err := o.required("--state")
	if err != nil {
		return err
	}
	st, err := store.NewDirectoryStore(ctx, state)
	if err != nil {
		return err
	}
	destination, err := o.absolute("--destination")
	if err != nil {
		return err
	}
	barrierPath, err := o.absolute("--barrier")
	if err != nil {
		return err
	}
	kind, err := resourceKind(o)
	if err != nil {
		return err
	}
	wait, err := o.duration("--wait-timeout", "30")
	if err != nil {
		return err
	}
	timeout, err := o.duration("--lock-timeout", "0")
	if err != nil {
		return err
	}
	authority, err := flyrail.NewResourceAuthority(ctx, destination)
	if err != nil {
		return err
	}
	unlock, err := interopbridge.ResourceLock(ctx, st, authority.Destination(), kind, timeout)
	if err != nil {
		return err
	}
	defer func() { _ = unlock() }()
	if err := emit(stdout, jsonx.Object(member("status", jsonx.String("reserved")))); err != nil {
		return err
	}
	deadline := time.Now().Add(wait)
	for {
		if _, err := os.Stat(barrierPath); err == nil {
			break
		}
		if !time.Now().Before(deadline) {
			return errors.New("barrier timed out")
		}
		time.Sleep(5 * time.Millisecond)
	}
	status := "admitted"
	if err := interopbridge.Admit(ctx, st, authority.Destination(), kind); err != nil {
		failed, ok := errors.AsType[*flyrail.TargetError](err)
		if !ok {
			return err
		}
		status = string(failed.Code)
	}
	return emit(stdout, jsonx.Object(member("status", jsonx.String(status))))
}

// Opens a pipe so the runtime's lazily created poller descriptors exist before the baseline listing.
func startPoller() error {
	reader, writer, err := os.Pipe()
	if err != nil {
		return err
	}
	return errors.Join(reader.Close(), writer.Close())
}

func openDescriptors() ([]string, error) {
	entries, err := os.ReadDir("/dev/fd")
	if err != nil {
		return nil, err
	}
	names := make([]string, 0, len(entries))
	for _, entry := range entries {
		names = append(names, entry.Name())
	}
	return names, nil
}

func observation(ctx context.Context, destination, kind string) jsonx.Value {
	resource, revision, ancestors, err := interopbridge.ObserveResource(ctx, destination, kind)
	if err == nil {
		return jsonx.Object(
			member("resource", resource),
			member("revision", revision),
			member("ancestors", jsonx.Array(ancestors...)),
			member("error", jsonx.Null()),
		)
	}
	detail := jsonx.Object(
		member("code", jsonx.Null()),
		member("message", jsonx.String(err.Error())),
		member("path", jsonx.Null()),
		member("errno", jsonx.Null()),
	)
	if failed, ok := errors.AsType[*flyrail.TargetError](err); ok {
		errno := jsonx.Null()
		if failed.Errno != 0 {
			errno = jsonx.BigInt(strconv.FormatUint(uint64(failed.Errno), 10))
		}
		detail = jsonx.Object(
			member("code", jsonx.String(string(failed.Code))),
			member("message", jsonx.String(failed.Message)),
			member("path", optionalText(failed.Path)),
			member("errno", errno),
		)
	}
	return jsonx.Object(member("error", detail))
}

func observeCommand(ctx context.Context, o options, _ io.Reader, stdout io.Writer) error {
	destination, err := o.absolute("--destination")
	if err != nil {
		return err
	}
	kind, err := resourceKind(o)
	if err != nil {
		return err
	}
	repeat, err := strconv.Atoi(o.value("--repeat", "1"))
	if err != nil || repeat < 1 || repeat > maxObserveRepeat {
		return fmt.Errorf("repeat must be between 1 and %d", maxObserveRepeat)
	}
	var before []string
	if repeat > 1 {
		if err := startPoller(); err != nil {
			return err
		}
		if before, err = openDescriptors(); err != nil {
			return err
		}
	}
	for range repeat {
		if err := emit(stdout, observation(ctx, destination, kind)); err != nil {
			return err
		}
		if repeat == 1 {
			continue
		}
		after, err := openDescriptors()
		if err != nil {
			return err
		}
		if !slices.Equal(before, after) {
			return errors.New("observation retained filesystem descriptors")
		}
	}
	return nil
}

func targetErrorValue(failed *flyrail.TargetError) jsonx.Value {
	if failed == nil {
		return jsonx.Null()
	}
	return jsonx.Object(
		member("code", jsonx.String(string(failed.Code))),
		member("message", jsonx.String(failed.Message)),
		member("path", optionalText(failed.Path)),
	)
}

func pathsValue(paths []string) jsonx.Value {
	values := make([]jsonx.Value, 0, len(paths))
	for _, path := range paths {
		values = append(values, jsonx.String(path))
	}
	return jsonx.Array(values...)
}

func observationValue(observation flyrail.InstallationObservation) jsonx.Value {
	resources := make([]jsonx.Value, 0, len(observation.Resources))
	for _, resource := range observation.Resources {
		claims := make([]jsonx.Value, 0, len(resource.Claims))
		for _, claim := range resource.Claims {
			claims = append(claims, jsonx.Object(
				member("artifact_id", jsonx.String(claim.ArtifactID)),
				member("disposition", jsonx.String(string(claim.Status))),
			))
		}
		resources = append(resources, jsonx.Object(
			member("destination", jsonx.String(resource.Destination)),
			member("claims", jsonx.Array(claims...)),
			member("error", targetErrorValue(resource.Err)),
			member("recovery_paths", pathsValue(resource.RecoveryPaths)),
		))
	}
	return jsonx.Object(
		member("bundle_id", jsonx.String(observation.BundleID)),
		member("is_current", jsonx.Bool(observation.Current)),
		member("pending", jsonx.Bool(observation.Pending)),
		member("version", optionalText(observation.Version)),
		member("error", targetErrorValue(observation.Err)),
		member("resources", jsonx.Array(resources...)),
	)
}

func installationResultValue(result flyrail.InstallationResult) jsonx.Value {
	resources := make([]jsonx.Value, 0, len(result.Resources))
	for _, resource := range result.Resources {
		resources = append(resources, jsonx.Object(
			member("destination", jsonx.String(resource.Resource.Destination)),
			member("status", jsonx.String(string(resource.Status))),
			member("error", targetErrorValue(resource.Err)),
			member("recovery_paths", pathsValue(resource.RecoveryPaths)),
		))
	}
	return jsonx.Object(
		member("status", jsonx.String(string(result.Status))),
		member("error", targetErrorValue(result.Err)),
		member("observation", observationValue(result.Observation)),
		member("resources", jsonx.Array(resources...)),
	)
}
