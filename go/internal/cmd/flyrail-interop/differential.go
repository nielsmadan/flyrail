package main

import (
	"context"
	"encoding/hex"
	"errors"
	"io"
	"maps"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/flyrail"
	"github.com/nielsmadan/flyrail/go/internal/interopbridge"
)

const maxCorpusBytes = 16 * 1024 * 1024

var (
	errUsage         = errors.New("usage: differential --corpus FILE --root DIRECTORY")
	errInvalidCorpus = errors.New("invalid differential corpus")
)

var knownCategories = map[string]bool{
	"value": true, "bundle": true, "render": true, "edit": true, "lifecycle": true,
}

type corpus struct {
	values  []valueRow
	bundles []bundleRow
	renders []renderRow
	edits   []editRow
	cycles  []lifecycleRow
}

type renderRow struct {
	name        string
	destination string
	payload     []byte
	seed        string
	fileFirst   bool
}

type bundleRow struct {
	name     string
	manifest []byte
	entries  []flyrail.BundleEntry
}

type valueRow struct {
	name  string
	bytes []byte
}

func differential(args []string) ([]byte, error) {
	corpusPath, rootPath, err := differentialOptions(args)
	if err != nil {
		return nil, err
	}
	text, err := readCorpus(corpusPath)
	if err != nil {
		return nil, err
	}
	rows, err := parseCorpus(text)
	if err != nil {
		return nil, err
	}
	root, err := createRoot(rootPath)
	if err != nil {
		return nil, err
	}
	output, err := valueRecords(nil, rows.values)
	if err != nil {
		return nil, err
	}
	if output, err = bundleRecords(output, rows.bundles); err != nil {
		return nil, err
	}
	if output, err = renderRecords(output, root, rows.renders); err != nil {
		return nil, err
	}
	if output, err = editRecords(output, rows.edits); err != nil {
		return nil, err
	}
	return lifecycleRecords(output, root, rows.cycles)
}

func differentialOptions(args []string) (corpusPath, rootPath string, err error) {
	if len(args)%2 != 0 {
		return "", "", errUsage
	}
	values := map[string]string{}
	for index := 0; index < len(args); index += 2 {
		name := args[index]
		if _, seen := values[name]; seen || (name != "--corpus" && name != "--root") {
			return "", "", errUsage
		}
		values[name] = args[index+1]
	}
	corpusPath, hasCorpus := values["--corpus"]
	rootPath, hasRoot := values["--root"]
	if !hasCorpus || !hasRoot {
		return "", "", errUsage
	}
	return corpusPath, rootPath, nil
}

func readCorpus(path string) (string, error) {
	file, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer func() { _ = file.Close() }()
	data, err := io.ReadAll(io.LimitReader(file, maxCorpusBytes+1))
	if err != nil {
		return "", err
	}
	if len(data) > maxCorpusBytes || !utf8.Valid(data) {
		return "", errInvalidCorpus
	}
	return string(data), nil
}

func parseCorpus(text string) (corpus, error) {
	var parsed corpus
	for line := range strings.SplitSeq(text, "\n") {
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		row := strings.Split(line, "\t")
		if !knownCategories[row[0]] {
			return corpus{}, errors.New("unknown differential category")
		}
		if len(row) < 2 {
			return corpus{}, errors.New("invalid differential row")
		}
		switch row[0] {
		case "value":
			value, err := parseValueRow(row)
			if err != nil {
				return corpus{}, err
			}
			parsed.values = append(parsed.values, value)
		case "bundle":
			bundle, err := parseBundleRow(row)
			if err != nil {
				return corpus{}, err
			}
			parsed.bundles = append(parsed.bundles, bundle)
		case "render":
			render, err := parseRenderRow(row)
			if err != nil {
				return corpus{}, err
			}
			parsed.renders = append(parsed.renders, render)
		case "edit":
			edit, err := parseEditRow(row)
			if err != nil {
				return corpus{}, err
			}
			parsed.edits = append(parsed.edits, edit)
		case "lifecycle":
			cycle, err := parseLifecycleRow(row)
			if err != nil {
				return corpus{}, err
			}
			parsed.cycles = append(parsed.cycles, cycle)
		}
	}
	return parsed, nil
}

func parseBundleRow(row []string) (bundleRow, error) {
	if len(row) < 5 {
		return bundleRow{}, errors.New("invalid differential bundle row")
	}
	count, err := strconv.Atoi(row[4])
	if err != nil || count < 0 || len(row)-5 != count*3 {
		return bundleRow{}, errors.New(row[1] + ": invalid bundle entry count")
	}
	manifest, err := hex.DecodeString(row[3])
	if err != nil {
		return bundleRow{}, errors.New("invalid hexadecimal data")
	}
	entries := make([]flyrail.BundleEntry, 0, count)
	for index := 5; index < len(row); index += 3 {
		entry := flyrail.BundleEntry{Path: row[index], Dir: row[index+1] == "-", Executable: row[index+2] == "1"}
		if !entry.Dir {
			if entry.Data, err = hex.DecodeString(row[index+1]); err != nil {
				return bundleRow{}, errors.New("invalid hexadecimal data")
			}
		}
		entries = append(entries, entry)
	}
	return bundleRow{name: row[1], manifest: manifest, entries: entries}, nil
}

func parseRenderRow(row []string) (renderRow, error) {
	if len(row) != 7 {
		return renderRow{}, errors.New("invalid differential render row")
	}
	payload, err := hex.DecodeString(row[3])
	if err != nil {
		return renderRow{}, errors.New("invalid hexadecimal data")
	}
	return renderRow{
		name:        row[1],
		destination: row[2],
		payload:     payload,
		seed:        row[4],
		fileFirst:   row[5] == "file-first",
	}, nil
}

func parseValueRow(row []string) (valueRow, error) {
	if len(row) != 3 {
		return valueRow{}, errors.New("invalid differential value row")
	}
	data, err := hex.DecodeString(row[2])
	if err != nil {
		return valueRow{}, errors.New("invalid hexadecimal data")
	}
	return valueRow{name: row[1], bytes: data}, nil
}

func createRoot(path string) (string, error) {
	absolute, err := filepath.Abs(path)
	if err != nil {
		return "", err
	}
	parent, err := filepath.EvalSymlinks(filepath.Dir(absolute))
	if err != nil {
		return "", err
	}
	root := filepath.Join(parent, filepath.Base(absolute))
	if err := os.Mkdir(root, 0o777); err != nil { //nolint:gosec // same umask-filtered mode as the Python and Rust drivers
		return "", err
	}
	return root, nil
}

func valueRecords(dst []byte, rows []valueRow) ([]byte, error) {
	for _, row := range rows {
		value, err := flyrail.DecodeValue(row.bytes)
		if err != nil {
			return nil, err
		}
		semantic, err := flyrail.EncodeValue(value)
		if err != nil {
			return nil, err
		}
		dst = appendRecord(dst, "value", row.name, "semantic", semantic)
	}
	return dst, nil
}

func bundleRecords(dst []byte, rows []bundleRow) ([]byte, error) {
	for _, row := range rows {
		bundle, err := flyrail.NewBundleFromEntries(row.manifest, row.entries)
		if err != nil {
			return nil, err
		}
		canonical, err := interopbridge.BundleModel(bundle)
		if err != nil {
			return nil, err
		}
		dst = appendRecord(dst, "bundle", row.name, "digest", []byte(bundle.ContentDigest()))
		dst = appendRecord(dst, "bundle", row.name, "canonical", canonical)
	}
	return dst, nil
}

func renderRecords(dst []byte, root string, rows []renderRow) ([]byte, error) {
	for _, row := range rows {
		caseRoot := filepath.Join(root, row.name)
		if err := os.Mkdir(caseRoot, 0o777); err != nil { //nolint:gosec // same umask-filtered mode as the Python and Rust drivers
			return nil, err
		}
		rendered, err := renderedCase(caseRoot, row)
		if err != nil {
			return nil, err
		}
		canonical, err := interopbridge.RenderedModel(rendered)
		if err != nil {
			return nil, err
		}
		routing, err := routingModel(rendered.RoutingContext())
		if err != nil {
			return nil, err
		}
		dst = appendRecord(dst, "render", row.name, "digest", []byte(rendered.ContentDigest()))
		dst = appendRecord(dst, "render", row.name, "canonical", canonical)
		dst = appendRecord(dst, "render", row.name, "context", routing)
		if dst, err = claimRecords(dst, row.name, rendered); err != nil {
			return nil, err
		}
	}
	return dst, nil
}

func claimRecords(dst []byte, name string, rendered *flyrail.RenderedBundle) ([]byte, error) {
	for index, artifact := range rendered.Artifacts() {
		claim, err := renderedClaim(artifact)
		if err != nil {
			return nil, err
		}
		authority, err := flyrail.NewResourceAuthority(context.Background(), artifact.Destination())
		if err != nil {
			return nil, err
		}
		identity, err := claim.Identity(authority)
		if err != nil {
			return nil, err
		}
		dst = appendRecord(dst, "render", name, "claim-"+strconv.Itoa(index), []byte(identity))
	}
	return dst, nil
}

func renderedClaim(artifact flyrail.RenderedArtifact) (flyrail.Claim, error) {
	if section, ok := artifact.Content().(flyrail.SectionContent); ok {
		return flyrail.NewClaim(flyrail.OwnershipSection, section.Selector())
	}
	return flyrail.NewClaim(flyrail.OwnershipFile, flyrail.WholeSelector{})
}

func renderedCase(root string, row renderRow) (*flyrail.RenderedBundle, error) {
	suffix := row.name[strings.LastIndexByte(row.name, '-')+1:]
	fileContent, err := flyrail.NewFileContent(row.payload, flyrail.ModeReadable)
	if err != nil {
		return nil, err
	}
	first, err := flyrail.NewRenderedArtifact(flyrail.RenderedArtifactParams{
		ID:          "base-" + suffix,
		Family:      flyrail.FamilyInstructions,
		Destination: filepath.Join(root, row.destination),
		Content:     fileContent,
	})
	if err != nil {
		return nil, err
	}
	section, err := flyrail.NewSectionContent("section-"+suffix, "value-"+row.seed+"\n", nil)
	if err != nil {
		return nil, err
	}
	second, err := flyrail.NewRenderedArtifact(flyrail.RenderedArtifactParams{
		ID:          "section-" + suffix,
		Family:      flyrail.FamilyInstructions,
		Destination: filepath.Join(root, "AGENTS.md"),
		Content:     section,
	})
	if err != nil {
		return nil, err
	}
	dependency, err := flyrail.NewDependency(second.ID(), first.ID(), "")
	if err != nil {
		return nil, err
	}
	artifacts := []flyrail.RenderedArtifact{second, first}
	if row.fileFirst {
		artifacts = []flyrail.RenderedArtifact{first, second}
	}
	routing := map[string]string{"alpha": "Straße", "zeta": row.seed}
	return flyrail.NewRenderedBundle(artifacts, []flyrail.Dependency{dependency}, nil, routing)
}

func routingModel(routing map[string]string) ([]byte, error) {
	pairs := make([]flyrail.Value, 0, len(routing))
	for _, key := range slices.Sorted(maps.Keys(routing)) {
		name, err := flyrail.NewString(key)
		if err != nil {
			return nil, err
		}
		value, err := flyrail.NewString(routing[key])
		if err != nil {
			return nil, err
		}
		pair, err := flyrail.NewArray(name, value)
		if err != nil {
			return nil, err
		}
		pairs = append(pairs, pair)
	}
	model, err := flyrail.NewArray(pairs...)
	if err != nil {
		return nil, err
	}
	return flyrail.EncodeValue(model)
}

func appendRecord(dst []byte, category, name, field string, value []byte) []byte {
	dst = append(dst, category...)
	dst = append(dst, '\t')
	dst = append(dst, name...)
	dst = append(dst, '\t')
	dst = append(dst, field...)
	dst = append(dst, '\t')
	dst = hex.AppendEncode(dst, value)
	return append(dst, '\n')
}
