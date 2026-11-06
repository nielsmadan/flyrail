package main

import (
	"context"
	"encoding/hex"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"

	"github.com/nielsmadan/flyrail/go/flyrail"
	"github.com/nielsmadan/flyrail/go/flyrail/store"
	"github.com/nielsmadan/flyrail/go/internal/interopbridge"
	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

type lifecycleRow struct {
	name        string
	kind        string
	initial     string
	first       string
	second      string
	acquisition flyrail.Acquisition
}

func parseLifecycleRow(row []string) (lifecycleRow, error) {
	if len(row) != 7 {
		return lifecycleRow{}, errors.New("invalid differential lifecycle row")
	}
	acquisition, err := flyrail.ParseAcquisition(row[6])
	if err != nil {
		return lifecycleRow{}, err
	}
	if row[3] != "-" {
		if _, err := hex.DecodeString(row[3]); err != nil {
			return lifecycleRow{}, errors.New("invalid hexadecimal data")
		}
	}
	return lifecycleRow{name: row[1], kind: row[2], initial: row[3], first: row[4], second: row[5], acquisition: acquisition}, nil
}

func lifecycleBundle(version string, ids []string) (*flyrail.Bundle, error) {
	identity, err := flyrail.NewBundleIdentity("team", version)
	if err != nil {
		return nil, err
	}
	artifacts := make([]flyrail.Artifact, 0, len(ids))
	for _, identifier := range ids {
		artifact, err := flyrail.NewInstructionArtifact(identifier, identifier)
		if err != nil {
			return nil, err
		}
		artifacts = append(artifacts, artifact)
	}
	return flyrail.NewBundle(identity, artifacts, nil, nil)
}

func lifecycleDestination(root, kind string) string {
	switch kind {
	case "file", "alias":
		return filepath.Join(root, "owned")
	case "casefold":
		return filepath.Join(root, "Straße")
	case "toml":
		return filepath.Join(root, "config.toml")
	case "json":
		return filepath.Join(root, "config.json")
	}
	return filepath.Join(root, "AGENTS.md")
}

func instructionArtifact(identifier, destination string, content flyrail.Content) (flyrail.RenderedArtifact, error) {
	return flyrail.NewRenderedArtifact(flyrail.RenderedArtifactParams{
		ID:          identifier,
		Family:      flyrail.FamilyInstructions,
		Destination: destination,
		Content:     content,
	})
}

func pairedArtifacts(destination, kind, text string) ([]flyrail.RenderedArtifact, error) {
	artifacts := make([]flyrail.RenderedArtifact, 0, 2)
	for _, identifier := range []string{"alpha", "zeta"} {
		var content flyrail.Content
		var err error
		if kind == "shared" {
			content, err = flyrail.NewSectionContent(identifier, identifier+"-"+text+"\n", nil)
		} else {
			content, err = flyrail.NewFileContent([]byte(text), flyrail.ModeReadable)
		}
		if err != nil {
			return nil, err
		}
		artifact, err := instructionArtifact(identifier, destination, content)
		if err != nil {
			return nil, err
		}
		artifacts = append(artifacts, artifact)
	}
	if kind == "shared" {
		// Python passes shared sections in reverse id order to exercise the library's ordering.
		slices.Reverse(artifacts)
	}
	return artifacts, nil
}

func lifecycleRendered(root, kind, text string) (*flyrail.RenderedBundle, []string, error) {
	destination := lifecycleDestination(root, kind)
	if kind == "shared" || kind == "alias" {
		artifacts, err := pairedArtifacts(destination, kind, text)
		if err != nil {
			return nil, nil, err
		}
		rendered, err := flyrail.NewRenderedBundle(artifacts, nil, nil, nil)
		return rendered, []string{"alpha", "zeta"}, err
	}
	var content flyrail.Content
	var err error
	if kind == "file" || kind == "casefold" {
		content, err = flyrail.NewFileContent([]byte(text), flyrail.ModeReadable)
	} else {
		content, err = editContent(kind, text)
	}
	if err != nil {
		return nil, nil, err
	}
	artifact, err := instructionArtifact("managed", destination, content)
	if err != nil {
		return nil, nil, err
	}
	routing := map[string]string{"seed": "1592639710", "surface": "differential"}
	rendered, err := flyrail.NewRenderedBundle([]flyrail.RenderedArtifact{artifact}, nil, nil, routing)
	return rendered, []string{"managed"}, err
}

func lowerHex(text string) bool {
	return !strings.ContainsFunc(text, func(r rune) bool { return (r < '0' || r > '9') && (r < 'a' || r > 'f') })
}

// Mirrors Python's TRANSACTION_DIRECTORY regex, whose `$` also matches before a final newline.
func zeroedTransactionID(component string, last bool) string {
	prefix := len(component) - len(strings.TrimPrefix(component, ".flyrail-"))
	start := prefix + 65
	end := start + 32
	if len(component) < end || !lowerHex(component[prefix:start-1]) || component[start-1] != '-' || !lowerHex(component[start:end]) {
		return component
	}
	if tail := component[end:]; tail != "" && !strings.HasPrefix(tail, ".txn") && (!last || tail != "\n") {
		return component
	}
	return component[:start] + strings.Repeat("0", 32) + component[end:]
}

func relativePath(path, root string) string {
	text := path
	if relative, ok := strings.CutPrefix(path, root+"/"); ok {
		text = "$ROOT/" + relative
	}
	parts := strings.Split(text, "/")
	for index, part := range parts {
		parts[index] = zeroedTransactionID(part, index == len(parts)-1)
	}
	return strings.Join(parts, "/")
}

func optionalText(value string) jsonx.Value {
	if value == "" {
		return jsonx.Null()
	}
	return jsonx.String(value)
}

func errorJSON(failed *flyrail.TargetError, root string) jsonx.Value {
	if failed == nil {
		return jsonx.Null()
	}
	path := jsonx.Null()
	if failed.Path != "" {
		path = jsonx.String(relativePath(failed.Path, root))
	}
	return jsonx.Object(
		member("code", jsonx.String(string(failed.Code))),
		member("message", jsonx.String(failed.Message)),
		member("path", path),
	)
}

func claimJSON(claim flyrail.ClaimObservation) jsonx.Value {
	return jsonx.Object(
		member("artifact", jsonx.String(claim.ArtifactID)),
		member("bundle", jsonx.String(claim.BundleID)),
		member("bundle_digest", jsonx.String(claim.BundleDigest)),
		member("claim", jsonx.String(claim.ClaimID)),
		member("render_digest", jsonx.String(claim.RenderDigest)),
		member("status", jsonx.String(string(claim.Status))),
		member("version", jsonx.String(claim.Version)),
	)
}

func resultRecord(result flyrail.InstallationResult, root string) []byte {
	observation := result.Observation
	observed := make([]jsonx.Value, 0, len(observation.Resources))
	for _, resource := range observation.Resources {
		claims := make([]jsonx.Value, 0, len(resource.Claims))
		for _, claim := range resource.Claims {
			claims = append(claims, claimJSON(claim))
		}
		observed = append(observed, jsonx.Object(
			member("claims", jsonx.Array(claims...)),
			member("destination", jsonx.String(relativePath(resource.Destination, root))),
			member("error", errorJSON(resource.Err, root)),
			member("revision", optionalText(resource.RevisionDigest)),
		))
	}
	resources := make([]jsonx.Value, 0, len(result.Resources))
	for _, resource := range result.Resources {
		resources = append(resources, jsonx.Object(
			member("destination", jsonx.String(relativePath(resource.Resource.Destination, root))),
			member("error", errorJSON(resource.Err, root)),
			member("status", jsonx.String(string(resource.Status))),
		))
	}
	return jsonx.Encode(jsonx.Object(
		member("error", errorJSON(result.Err, root)),
		member("observation", jsonx.Object(
			member("bundle_digest", optionalText(observation.BundleDigest)),
			member("current", jsonx.Bool(observation.Current)),
			member("error", errorJSON(observation.Err, root)),
			member("pending", jsonx.Bool(observation.Pending)),
			member("render_digest", optionalText(observation.RenderDigest)),
			member("resources", jsonx.Array(observed...)),
			member("version", optionalText(observation.Version)),
		)),
		member("resources", jsonx.Array(resources...)),
		member("status", jsonx.String(string(result.Status))),
	))
}

func normalizeState(value jsonx.Value, root string) jsonx.Value {
	if items, ok := value.Items(); ok {
		normalized := make([]jsonx.Value, 0, len(items))
		for _, item := range items {
			normalized = append(normalized, normalizeState(item, root))
		}
		return jsonx.Array(normalized...)
	}
	members, ok := value.Members()
	if !ok {
		return value
	}
	normalized := make([]jsonx.Member, 0, len(members))
	for _, item := range members {
		normalized = append(normalized, member(item.Key, normalizeState(item.Value, root)))
	}
	if len(normalized) == 1 && normalized[0].Key == "path" {
		if path, ok := normalized[0].Value.AsString(); ok {
			normalized[0].Value = jsonx.String(relativePath(path, root))
		}
	}
	recordType, _ := value.Get("type")
	typeName, _ := recordType.AsString()
	for index, item := range normalized {
		fields, ok := item.Value.Members()
		if item.Key != "fields" || !ok {
			continue
		}
		switch typeName {
		case "Node", "Ancestor":
			for _, key := range []string{"device", "inode", "uid", "gid"} {
				fields = setMember(fields, key, jsonx.Int(0))
			}
			fields = setMember(fields, "security", jsonx.Object(member("bytes", jsonx.String(""))))
		case "Receipt", "Preparation":
			fields = setMember(fields, "transaction_id", jsonx.String(strings.Repeat("0", 32)))
		}
		normalized[index].Value = jsonx.Object(fields...)
	}
	return jsonx.Object(normalized...)
}

func setMember(members []jsonx.Member, key string, value jsonx.Value) []jsonx.Member {
	updated := slices.Clone(members)
	if index := slices.IndexFunc(updated, func(item jsonx.Member) bool { return item.Key == key }); index >= 0 {
		updated[index].Value = value
		return updated
	}
	return append(updated, member(key, value))
}

func stateKey(key string, record jsonx.Value, root string) (string, error) {
	if !strings.HasPrefix(key, "resource/") {
		return key, nil
	}
	fields, _ := record.Get("fields")
	resource := fields
	if recordType, _ := record.Get("type"); !isText(recordType, "ResourceRef") {
		nested, _ := fields.Get("resource")
		resource, _ = nested.Get("fields")
	}
	destination, _ := resource.Get("destination")
	path, _ := destination.Get("path")
	text, ok := path.AsString()
	if !ok {
		return "", errors.New(key + ": record names no destination")
	}
	return "resource/" + relativePath(text, root) + "/" + key[strings.LastIndexByte(key, '/')+1:], nil
}

func isText(value jsonx.Value, want string) bool {
	text, ok := value.AsString()
	return ok && text == want
}

func stateRecords(dst []byte, name, step, root string, st store.Store) ([]byte, error) {
	ctx := context.Background()
	keys, err := st.Keys(ctx, "")
	if err != nil {
		return nil, err
	}
	for _, key := range keys {
		raw, ok, err := st.Read(ctx, key)
		if err != nil {
			return nil, err
		}
		if !ok {
			continue
		}
		canonical, err := interopbridge.CanonicalStateRecord(raw)
		if err != nil {
			return nil, err
		}
		if string(canonical) != string(raw) {
			return nil, errors.New("state record is not canonical: " + key)
		}
		record, err := jsonx.Parse(raw)
		if err != nil {
			return nil, err
		}
		field, err := stateKey(key, record, root)
		if err != nil {
			return nil, err
		}
		normalized := append(jsonx.Encode(sortedJSON(normalizeState(record, root))), '\n')
		dst = appendRecord(dst, "lifecycle", name, step+"-state-"+field, normalized)
	}
	return dst, nil
}

// Matches Python's rglob plus is_file: directory links are not descended, file links count.
func collectPayloads(directory string, paths []string) ([]string, error) {
	entries, err := os.ReadDir(directory)
	if err != nil {
		return nil, err
	}
	for _, entry := range entries {
		path := filepath.Join(directory, entry.Name())
		if entry.IsDir() {
			if paths, err = collectPayloads(path, paths); err != nil {
				return nil, err
			}
			continue
		}
		if info, err := os.Stat(path); err == nil && info.Mode().IsRegular() {
			paths = append(paths, path)
		}
	}
	return paths, nil
}

func payloadRecord(root string) ([]byte, error) {
	paths, err := collectPayloads(root, nil)
	if err != nil {
		return nil, err
	}
	slices.SortFunc(paths, func(a, b string) int { return slices.Compare(strings.Split(a, "/"), strings.Split(b, "/")) })
	values := make([]jsonx.Value, 0, len(paths))
	for _, path := range paths {
		if strings.Contains(path, ".flyrail") || filepath.Base(path) == "lock" {
			continue
		}
		data, err := os.ReadFile(path)
		if err != nil {
			return nil, err
		}
		values = append(values, jsonx.Array(jsonx.String(relativePath(path, root)), jsonx.String(hex.EncodeToString(data))))
	}
	return jsonx.Encode(jsonx.Array(values...)), nil
}

func lifecycleStep(dst []byte, name, step string, result flyrail.InstallationResult, root string, target *flyrail.InstallationTarget) ([]byte, error) {
	dst = appendRecord(dst, "lifecycle", name, step+"-result", resultRecord(result, root))
	dst, err := stateRecords(dst, name, step, root, target.Store())
	if err != nil {
		return nil, err
	}
	payloads, err := payloadRecord(root)
	if err != nil {
		return nil, err
	}
	return appendRecord(dst, "lifecycle", name, step+"-payloads", payloads), nil
}

func syncStep(ctx context.Context, version string, ids []string, rendered *flyrail.RenderedBundle, target *flyrail.InstallationTarget, acquisition flyrail.Acquisition) (flyrail.InstallationResult, error) {
	bundle, err := lifecycleBundle(version, ids)
	if err != nil {
		return flyrail.InstallationResult{}, err
	}
	return flyrail.Sync(ctx, bundle, rendered, target, flyrail.SyncOptions{Acquisition: acquisition})
}

func lifecycleCase(dst []byte, root string, row lifecycleRow) ([]byte, error) {
	ctx := context.Background()
	caseRoot := filepath.Join(root, row.name)
	if err := os.Mkdir(caseRoot, 0o777); err != nil { //nolint:gosec // same umask-filtered mode as the Python and Rust drivers
		return nil, err
	}
	st, err := store.NewDirectoryStore(ctx, filepath.Join(root, ".state", row.name))
	if err != nil {
		return nil, err
	}
	target, err := flyrail.NewInstallationTarget(st, nil, nil)
	if err != nil {
		return nil, err
	}
	first, ids, err := lifecycleRendered(caseRoot, row.kind, row.first)
	if err != nil {
		return nil, err
	}
	if row.initial != "-" {
		data, _ := hex.DecodeString(row.initial)
		if row.acquisition == flyrail.AcquireAdopt {
			data = []byte(row.first)
		}
		if err := os.WriteFile(first.Artifacts()[0].Destination(), data, 0o666); err != nil { //nolint:gosec // same umask-filtered mode as the Python and Rust drivers
			return nil, err
		}
	}
	steps := []struct {
		name        string
		version     string
		text        string
		acquisition flyrail.Acquisition
	}{
		{"1-create", "1", row.first, row.acquisition},
		{"2-unchanged", "1", row.first, flyrail.AcquireConflict},
		{"3-update", "2", row.second, flyrail.AcquireConflict},
	}
	for _, step := range steps {
		rendered, _, err := lifecycleRendered(caseRoot, row.kind, step.text)
		if err != nil {
			return nil, err
		}
		result, err := syncStep(ctx, step.version, ids, rendered, target, step.acquisition)
		if err != nil {
			return nil, err
		}
		if dst, err = lifecycleStep(dst, row.name, step.name, result, caseRoot, target); err != nil {
			return nil, err
		}
	}
	recovered, err := flyrail.RecoverInstallation(ctx, "team", target, flyrail.LockOptions{})
	if err != nil {
		return nil, err
	}
	if dst, err = lifecycleStep(dst, row.name, "4-recover", recovered, caseRoot, target); err != nil {
		return nil, err
	}
	observed, err := flyrail.InspectInstallation(ctx, "team", target)
	if err != nil {
		return nil, err
	}
	revisions := make([]jsonx.Value, 0, len(observed.Resources))
	for _, resource := range observed.Resources {
		revisions = append(revisions, optionalText(resource.RevisionDigest))
	}
	dst = appendRecord(dst, "lifecycle", row.name, "5-inspect", jsonx.Encode(jsonx.Object(
		member("current", jsonx.Bool(observed.Current)),
		member("pending", jsonx.Bool(observed.Pending)),
		member("revision", jsonx.Array(revisions...)),
	)))
	removed, err := flyrail.Remove(ctx, "team", target, flyrail.RemoveOptions{})
	if err != nil {
		return nil, err
	}
	return lifecycleStep(dst, row.name, "6-remove", removed, caseRoot, target)
}

func lifecycleRecords(dst []byte, root string, rows []lifecycleRow) ([]byte, error) {
	for _, row := range rows {
		var err error
		if dst, err = lifecycleCase(dst, root, row); err != nil {
			return nil, err
		}
	}
	return dst, nil
}
