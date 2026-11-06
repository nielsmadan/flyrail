package flyrail

import (
	"context"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"io/fs"
	"math"
	"slices"
	"strings"
	"syscall"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/sysx"
	"github.com/nielsmadan/flyrail/go/internal/zipx"
)

// Bundle is an immutable snapshot of a bundle's identity, skills and content.
type Bundle struct {
	identity      BundleIdentity
	skills        []SkillSpec
	entries       []BundleEntry
	contentDigest string
	sourceRoots   []string
	artifacts     []Artifact
	assets        []SupportAsset
	dependencies  []Dependency
	schemaVersion int
}

func invalidBundle(message string) error {
	return &BundleError{Message: message, Err: newValueError(message)}
}

func contextBundleError(err error) error {
	return &BundleError{Message: err.Error(), Err: err}
}

func ioBundleError(err error) error {
	if failure, ok := errors.AsType[*fs.PathError](err); ok {
		if errno, ok := errors.AsType[syscall.Errno](failure.Err); ok {
			return &BundleError{Message: sysx.PathErrorMessage(errno, failure.Path), Err: err}
		}
	}
	return &BundleError{Message: err.Error(), Err: err}
}

func archiveBundleError(path string, err error) error {
	if failure, ok := errors.AsType[*zipx.Error](err); ok {
		return &BundleError{Message: failure.Reason, Err: &fs.PathError{Op: "unzip", Path: path, Err: failure}}
	}
	return ioBundleError(err)
}

func asBundleError(err error) error {
	if _, ok := errors.AsType[*BundleError](err); ok {
		return err
	}
	if failure, ok := errors.AsType[*ValueError](err); ok {
		return &BundleError{Message: failure.Message, Err: failure}
	}
	return ioBundleError(err)
}

func checkContext(ctx context.Context) error {
	if err := requireContext(ctx); err != nil {
		return err
	}
	if err := ctx.Err(); err != nil {
		return contextBundleError(err)
	}
	return nil
}

// OpenBundle snapshots the bundle in directory dir, refusing links, special files and sources that change while loading.
func OpenBundle(ctx context.Context, dir string) (*Bundle, error) {
	if err := checkContext(ctx); err != nil {
		return nil, err
	}
	source, err := newDirectorySource(ctx, dir)
	if err != nil {
		return nil, err
	}
	return loadBundle(source, []string{source.root})
}

// OpenBundleZip snapshots the bundle stored under member directory root of the ZIP archive at path.
func OpenBundleZip(ctx context.Context, path, root string) (*Bundle, error) {
	if err := checkContext(ctx); err != nil {
		return nil, err
	}
	if err := validateRelativePath(root, "archive resource"); err != nil {
		return nil, asBundleError(err)
	}
	parent, err := newDirectorySource(ctx, pathx.Directory(path))
	if err != nil {
		return nil, err
	}
	signature, err := parent.observe(path)
	if err != nil {
		return nil, err
	}
	limits := zipx.DefaultLimits()
	data, err := parent.snapshotFile(path, signature, int64(min(limits.ArchiveBytes, math.MaxInt64)))
	if err != nil {
		return nil, err
	}
	archive, err := zipx.Open(data, limits)
	if err != nil {
		return nil, archiveBundleError(path, err)
	}
	source, err := newZipSource(archive, root, path)
	if err != nil {
		return nil, err
	}
	canonical, err := sysx.Canonicalize(path)
	if err != nil {
		return nil, ioBundleError(err)
	}
	bundle, err := loadBundle(source, []string{canonical})
	if err != nil {
		return nil, err
	}
	if err := parent.finish(); err != nil {
		return nil, err
	}
	return bundle, nil
}

// LoadBundleFS snapshots the bundle at the root of fsys, such as an embed.FS; file modes are ignored and it has no source roots.
func LoadBundleFS(fsys fs.FS) (*Bundle, error) {
	if fsys == nil {
		return nil, invalidBundle("bundle file system must not be nil")
	}
	return loadBundle(&fsSource{fsys: fsys}, nil)
}

// NewBundleFromEntries loads a bundle from manifest bytes and in-memory entries; missing parent directories are implied.
func NewBundleFromEntries(manifest []byte, entries []BundleEntry) (*Bundle, error) {
	source, err := newMemorySource(manifest, entries)
	if err != nil {
		return nil, err
	}
	return loadBundle(source, nil)
}

// NewBundleFromValue loads a bundle from a manifest given as a semantic value and in-memory entries.
func NewBundleFromValue(manifest Value, entries []BundleEntry) (*Bundle, error) {
	if _, err := checkValue(manifest); err != nil {
		return nil, asBundleError(err)
	}
	raw, err := valueToJSON(manifest)
	if err != nil {
		return nil, asBundleError(err)
	}
	return NewBundleFromEntries(jsonx.EncodePython(raw), entries)
}

func valueToJSON(value Value) (jsonx.Value, error) {
	switch v := value.(type) {
	case Null:
		return jsonx.Null(), nil
	case Bool:
		return jsonx.Bool(bool(v)), nil
	case Int:
		return jsonx.Int(int64(v)), nil
	case Float:
		return jsonx.Float(v.Float64()), nil
	case String:
		return jsonx.String(v.String()), nil
	case Array:
		items := make([]jsonx.Value, 0, v.Len())
		for _, item := range v.items {
			converted, err := valueToJSON(item)
			if err != nil {
				return jsonx.Value{}, err
			}
			items = append(items, converted)
		}
		return jsonx.Array(items...), nil
	case Object:
		members := make([]jsonx.Member, 0, v.Len())
		for key, item := range v.All() {
			converted, err := valueToJSON(item)
			if err != nil {
				return jsonx.Value{}, err
			}
			members = append(members, jsonx.Member{Key: key, Value: converted})
		}
		return jsonx.Object(members...), nil
	}
	return jsonx.Value{}, newValueError("manifest values must be JSON objects, arrays or scalars")
}

func isSchemaTwo(raw jsonx.Value) bool {
	version, ok := raw.Get("schema_version")
	if !ok {
		return false
	}
	if integer, ok := version.AsInt(); ok {
		return integer == 2
	}
	float, ok := version.AsFloat()
	return ok && math.Float64bits(float) == math.Float64bits(2)
}

func loadBundle(source bundleSource, roots []string) (*Bundle, error) {
	data, err := source.readManifest()
	if err != nil {
		return nil, err
	}
	raw, err := parseManifest(data)
	if err != nil {
		return nil, asBundleError(err)
	}
	if isSchemaTwo(raw) {
		return loadConfigBundle(raw, source, roots)
	}
	identity, specs, err := readManifest(data)
	if err != nil {
		return nil, asBundleError(err)
	}
	var entries []BundleEntry
	for _, spec := range specs {
		tree, err := source.treeEntries(spec.Path(), spec.Name(), spec.executables)
		if err != nil {
			return nil, err
		}
		entries = append(entries, tree...)
	}
	sortEntries(entries)
	files := make(map[string]struct{}, len(entries))
	for _, entry := range entries {
		if !entry.Dir {
			files[entry.Path] = struct{}{}
		}
	}
	for _, spec := range specs {
		if _, ok := files[spec.Name()+"/SKILL.md"]; !ok {
			return nil, invalidBundle("skill must contain a SKILL.md file: " + spec.Name())
		}
		for _, executable := range spec.executables {
			if _, ok := files[spec.Name()+"/"+executable]; !ok {
				return nil, invalidBundle("every executable must name an existing file: " + spec.Name())
			}
		}
	}
	if err := source.finishLoading(); err != nil {
		return nil, err
	}
	artifacts := make([]Artifact, 0, len(specs))
	for _, spec := range specs {
		artifact, err := skillArtifactFromEntries(spec.Name(), entries)
		if err != nil {
			return nil, asBundleError(err)
		}
		artifacts = append(artifacts, artifact)
	}
	return &Bundle{
		identity:      identity,
		skills:        specs,
		entries:       entries,
		contentDigest: contentDigest(entries),
		sourceRoots:   roots,
		artifacts:     artifacts,
		schemaVersion: 1,
	}, nil
}

func skillArtifactFromEntries(name string, entries []BundleEntry) (SkillArtifact, error) {
	var tree []BundleEntry
	for _, entry := range entries {
		if relative, ok := strings.CutPrefix(entry.Path, name+"/"); ok {
			tree = append(tree, BundleEntry{Path: relative, Data: entry.Data, Dir: entry.Dir, Executable: entry.Executable})
		}
	}
	content, err := NewTreeContent(tree)
	if err != nil {
		return SkillArtifact{}, err
	}
	return NewSkillArtifact(name, name, content)
}

func loadConfigBundle(raw jsonx.Value, source bundleSource, roots []string) (*Bundle, error) {
	parsed, err := readConfigManifest(raw, source)
	if err != nil {
		return nil, asBundleError(err)
	}
	bundle, err := newConfigBundle(parsed.identity, parsed.artifacts, parsed.assets, parsed.dependencies)
	if err != nil {
		return nil, asBundleError(err)
	}
	if err := source.finishLoading(); err != nil {
		return nil, err
	}
	bundle.sourceRoots = roots
	return bundle, nil
}

// NewBundle returns a schema-2 bundle of artifacts, support assets and dependencies, ordered by id.
func NewBundle(id BundleIdentity, artifacts []Artifact, assets []SupportAsset, deps []Dependency) (*Bundle, error) {
	if _, err := NewBundleIdentity(id.id, id.version); err != nil {
		return nil, asBundleError(err)
	}
	for _, artifact := range artifacts {
		if err := checkArtifact(artifact); err != nil {
			return nil, asBundleError(err)
		}
	}
	for _, asset := range assets {
		if err := validateIdentifier(asset.id, "asset id"); err != nil {
			return nil, asBundleError(err)
		}
	}
	for _, dependency := range deps {
		if err := dependency.check(); err != nil {
			return nil, asBundleError(err)
		}
	}
	bundle, err := newConfigBundle(id, artifacts, assets, deps)
	if err != nil {
		return nil, asBundleError(err)
	}
	return bundle, nil
}

func newConfigBundle(identity BundleIdentity, artifacts []Artifact, assets []SupportAsset, deps []Dependency) (*Bundle, error) {
	if len(artifacts) == 0 {
		return nil, newValueError("a configuration bundle requires at least one artifact")
	}
	ids := make(map[string]struct{}, len(artifacts)+len(assets))
	names := make([]string, 0, len(artifacts)+len(assets))
	for _, id := range append(artifactIDs(artifacts), assetIDs(assets)...) {
		if _, duplicate := ids[id]; duplicate {
			return nil, newValueError("artifact and asset ids must be unique")
		}
		ids[id] = struct{}{}
		names = append(names, id)
	}
	skillNames := make(map[string]struct{})
	for _, artifact := range artifacts {
		skill, ok := artifact.(SkillArtifact)
		if !ok {
			continue
		}
		if _, duplicate := skillNames[skill.name]; duplicate {
			return nil, newValueError("duplicate skill name")
		}
		skillNames[skill.name] = struct{}{}
	}
	if _, err := DependencyOrder(names, deps); err != nil {
		return nil, err
	}
	if err := assetReferences(artifacts, assets, deps); err != nil {
		return nil, err
	}
	for _, asset := range assets {
		if !slices.ContainsFunc(deps, func(edge Dependency) bool { return edge.required == asset.id }) {
			return nil, newValueError("support assets must have an explicit dependent")
		}
	}
	ordered := slices.Clone(artifacts)
	slices.SortStableFunc(ordered, func(a, b Artifact) int { return strings.Compare(a.ID(), b.ID()) })
	orderedAssets := slices.Clone(assets)
	slices.SortStableFunc(orderedAssets, func(a, b SupportAsset) int { return strings.Compare(a.id, b.id) })
	orderedDeps := slices.Clone(deps)
	slices.SortStableFunc(orderedDeps, func(a, b Dependency) int {
		if order := strings.Compare(a.dependent, b.dependent); order != 0 {
			return order
		}
		return strings.Compare(a.required, b.required)
	})
	var skills []SkillSpec
	var entries []BundleEntry
	for _, artifact := range ordered {
		skill, ok := artifact.(SkillArtifact)
		if !ok {
			continue
		}
		var executables []string
		for _, entry := range skill.content.entries {
			if entry.Executable {
				executables = append(executables, entry.Path)
			}
		}
		spec, err := NewSkillSpec(skill.name, skill.name, executables)
		if err != nil {
			return nil, err
		}
		skills = append(skills, spec)
		top, err := directoryEntry(skill.name)
		if err != nil {
			return nil, err
		}
		entries = append(entries, top)
		for _, entry := range skill.content.entries {
			nested, err := newEntry(skill.name+"/"+entry.Path, entry.Data, entry.Dir, entry.Executable)
			if err != nil {
				return nil, err
			}
			entries = append(entries, nested)
		}
	}
	sortEntries(entries)
	return &Bundle{
		identity:      identity,
		skills:        skills,
		entries:       entries,
		contentDigest: configDigest(ordered, orderedAssets, orderedDeps),
		artifacts:     ordered,
		assets:        orderedAssets,
		dependencies:  orderedDeps,
		schemaVersion: 2,
	}, nil
}

func artifactIDs(artifacts []Artifact) []string {
	ids := make([]string, len(artifacts))
	for index, artifact := range artifacts {
		ids[index] = artifact.ID()
	}
	return ids
}

func assetIDs(assets []SupportAsset) []string {
	ids := make([]string, len(assets))
	for index, asset := range assets {
		ids[index] = asset.id
	}
	return ids
}

func assetReferences(artifacts []Artifact, assets []SupportAsset, deps []Dependency) error {
	for _, artifact := range artifacts {
		command, ok := artifactCommand(artifact)
		if !ok {
			continue
		}
		for index, argument := range command.argv {
			reference, ok := argument.(AssetRef)
			if !ok {
				continue
			}
			if err := validateAssetReference(artifact.ID(), reference, assets, deps); err != nil {
				return err
			}
			if entry, found := referencedEntry(reference, assets); index == 0 && (!found || entry.Dir) {
				return newValueError("asset command must reference a file")
			}
		}
		if reference, ok := command.cwd.(AssetRef); ok {
			if err := validateAssetReference(artifact.ID(), reference, assets, deps); err != nil {
				return err
			}
			if entry, found := referencedEntry(reference, assets); found && !entry.Dir {
				return newValueError("asset cwd must reference a directory")
			}
		}
	}
	return nil
}

func validateAssetReference(artifactID string, reference AssetRef, assets []SupportAsset, deps []Dependency) error {
	if !slices.ContainsFunc(assets, func(asset SupportAsset) bool { return asset.id == reference.assetID }) {
		return newValueError("asset reference names an unknown support asset")
	}
	if !slices.ContainsFunc(deps, func(edge Dependency) bool {
		return edge.dependent == artifactID && edge.required == reference.assetID && edge.mode == DependencyRevision
	}) {
		return newValueError("asset reference requires a direct dependency")
	}
	if _, found := referencedEntry(reference, assets); reference.path != "" && !found {
		return newValueError("asset reference path does not exist in snapshot")
	}
	return nil
}

func referencedEntry(reference AssetRef, assets []SupportAsset) (BundleEntry, bool) {
	if reference.path == "" {
		return BundleEntry{}, false
	}
	index := slices.IndexFunc(assets, func(asset SupportAsset) bool { return asset.id == reference.assetID })
	if index < 0 {
		return BundleEntry{}, false
	}
	entries := assets[index].content.entries
	position := slices.IndexFunc(entries, func(entry BundleEntry) bool { return entry.Path == reference.path })
	if position < 0 {
		return BundleEntry{}, false
	}
	return entries[position], true
}

func bundleModel(artifacts []Artifact, assets []SupportAsset, deps []Dependency) Value {
	return modelArray([]Value{
		modelList(artifacts, modelArtifact),
		modelList(assets, SupportAsset.modelValue),
		modelList(deps, Dependency.modelValue),
	})
}

func configDigest(artifacts []Artifact, assets []SupportAsset, deps []Dependency) string {
	semantic, _ := EncodeValue(bundleModel(artifacts, assets, deps))
	digest := sha256.New()
	digest.Write([]byte("flyrail-config-v2\x00"))
	digest.Write(semantic)
	return hex.EncodeToString(digest.Sum(nil))
}

func contentDigest(entries []BundleEntry) string {
	ordered := slices.Clone(entries)
	sortEntries(ordered)
	digest := sha256.New()
	digest.Write([]byte("flyrail-content-v1\x00"))
	digest.Write(binary.BigEndian.AppendUint64(nil, uint64(len(ordered))))
	for _, entry := range ordered {
		if entry.Dir {
			digest.Write([]byte("D"))
		} else {
			digest.Write([]byte("F"))
		}
		digest.Write(binary.BigEndian.AppendUint64(nil, uint64(len(entry.Path))))
		digest.Write([]byte(entry.Path))
		if !entry.Dir {
			executable := byte(0)
			if entry.Executable {
				executable = 1
			}
			digest.Write([]byte{executable})
			digest.Write(binary.BigEndian.AppendUint64(nil, uint64(len(entry.Data))))
			digest.Write(entry.Data)
		}
	}
	return hex.EncodeToString(digest.Sum(nil))
}

// Identity returns the bundle identity.
func (b *Bundle) Identity() BundleIdentity {
	if b == nil {
		return BundleIdentity{}
	}
	return b.identity
}

// ID returns the bundle id.
func (b *Bundle) ID() string {
	return b.Identity().ID()
}

// Version returns the bundle version label.
func (b *Bundle) Version() string {
	return b.Identity().Version()
}

// Skills returns the skill specifications in manifest order.
func (b *Bundle) Skills() []SkillSpec {
	if b == nil {
		return nil
	}
	return slices.Clone(b.skills)
}

// Entries returns copies of the snapshot entries in path byte order.
func (b *Bundle) Entries() []BundleEntry {
	if b == nil {
		return nil
	}
	return copyEntries(b.entries)
}

// ContentDigest returns the lowercase hexadecimal content digest.
func (b *Bundle) ContentDigest() string {
	if b == nil {
		return ""
	}
	return b.contentDigest
}

// SourceRoots returns the canonical directories or archives the bundle was loaded from.
func (b *Bundle) SourceRoots() []string {
	if b == nil {
		return nil
	}
	return slices.Clone(b.sourceRoots)
}

// Artifacts returns the artifacts; a schema-1 bundle has one skill artifact per skill, in manifest order.
func (b *Bundle) Artifacts() []Artifact {
	if b == nil {
		return nil
	}
	return slices.Clone(b.artifacts)
}

// Assets returns the support assets in id order.
func (b *Bundle) Assets() []SupportAsset {
	if b == nil {
		return nil
	}
	return slices.Clone(b.assets)
}

// Dependencies returns the dependencies ordered by dependent then required id.
func (b *Bundle) Dependencies() []Dependency {
	if b == nil {
		return nil
	}
	return slices.Clone(b.dependencies)
}

// SchemaVersion returns the manifest schema version.
func (b *Bundle) SchemaVersion() int {
	if b == nil {
		return 0
	}
	return b.schemaVersion
}
