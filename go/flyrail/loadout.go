package flyrail

import (
	"context"
	"errors"
	"fmt"
	"maps"
	"path/filepath"
	"slices"
	"strings"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

// LoadoutStatus describes the result of looking for a global registration.
type LoadoutStatus string

// The global Loadout discovery outcomes.
const (
	LoadoutAbsent    LoadoutStatus = "absent"
	LoadoutAvailable LoadoutStatus = "available"
	LoadoutInvalid   LoadoutStatus = "invalid"
)

// LoadoutDiagnostic explains why a setup or requested source plan cannot be used.
type LoadoutDiagnostic struct {
	Code    string
	Message string
	Path    string
}

// Error returns the diagnostic message.
func (d *LoadoutDiagnostic) Error() string {
	if d == nil {
		return ""
	}
	return d.Message
}

func loadoutIssue(code, message, path string) *LoadoutDiagnostic {
	return &LoadoutDiagnostic{code, message, path}
}

// LoadoutDiscovery is the observation of the caller's global registration.
type LoadoutDiscovery struct {
	Status     LoadoutStatus
	Registry   string
	Setup      *LoadoutSetup
	Diagnostic *LoadoutDiagnostic
}

// LoadoutSource describes one declared source and the slices it offers.
type LoadoutSource struct {
	name, path, declaredIn string
	uses                   []string
	overrides              map[string][]string
}

// Name returns the source qualifier used in fragment selectors.
func (s LoadoutSource) Name() string { return s.name }

// Path returns the resolved source directory.
func (s LoadoutSource) Path() string { return s.path }

// DeclaredIn returns the physical manifest declaring the source.
func (s LoadoutSource) DeclaredIn() string { return s.declaredIn }

// Uses returns the content categories the source offers.
func (s LoadoutSource) Uses() []string { return slices.Clone(s.uses) }

// Overrides returns the source item names that replace earlier offerings.
func (s LoadoutSource) Overrides() map[string][]string {
	result := maps.Clone(s.overrides)
	for key, values := range result {
		result[key] = slices.Clone(values)
	}
	return result
}

// LoadoutHarness lists the slices an effective profile enables for a harness.
type LoadoutHarness struct {
	agent  Agent
	slices []string
}

// Agent returns the harness identified by the profile.
func (h LoadoutHarness) Agent() Agent { return h.agent }

// Slices returns a copy of the enabled slice names.
func (h LoadoutHarness) Slices() []string { return slices.Clone(h.slices) }

// LoadoutSelector identifies the declaration behind an effective fragment list.
type LoadoutSelector struct {
	agent              Agent
	category, manifest string
	key, entries       []string
	swaps              map[string]string
	array              bool
	destinations       int
	allProfiles        bool
}

// Agent returns the harness using this selector.
func (s LoadoutSelector) Agent() Agent { return s.agent }

// Category returns the fragment category.
func (s LoadoutSelector) Category() string { return s.category }

// Manifest returns the manifest declaring the list.
func (s LoadoutSelector) Manifest() string { return s.manifest }

// Key returns its TOML key path.
func (s LoadoutSelector) Key() []string { return slices.Clone(s.key) }

// Entries returns the fragments in declaration order.
func (s LoadoutSelector) Entries() []string { return slices.Clone(s.entries) }

// Substitutions returns the overrides used when resolving instruction fragments.
func (s LoadoutSelector) Substitutions() map[string]string { return maps.Clone(s.swaps) }

// LoadoutSetup is an immutable snapshot of the global registration and effective profile.
type LoadoutSetup struct {
	registry, home, root, profile string
	chain                         []string
	sources                       []LoadoutSource
	routes                        loadoutRoutes
	conditions                    []loadoutInput
}

// Registry returns the path where the registration was read.
func (s *LoadoutSetup) Registry() string {
	if s == nil {
		return ""
	}
	return s.registry
}

// Root returns the registered Loadout root.
func (s *LoadoutSetup) Root() string {
	if s == nil {
		return ""
	}
	return s.root
}

// Profile returns the active profile name.
func (s *LoadoutSetup) Profile() string {
	if s == nil {
		return ""
	}
	return s.profile
}

// ProfileChain returns the physical manifests from the active profile to the oldest ancestor.
func (s *LoadoutSetup) ProfileChain() []string {
	if s == nil {
		return nil
	}
	return slices.Clone(s.chain)
}

// Sources returns the declared sources in Loadout resolution order.
func (s *LoadoutSetup) Sources() []LoadoutSource {
	if s == nil {
		return nil
	}
	return slices.Clone(s.sources)
}

// Harnesses returns the harnesses declared by the effective profile.
func (s *LoadoutSetup) Harnesses() []LoadoutHarness {
	if s == nil {
		return nil
	}
	return slices.Clone(s.routes.harnesses)
}

// Selectors returns the effective instruction and hook lists with their provenance.
func (s *LoadoutSetup) Selectors() []LoadoutSelector {
	if s == nil {
		return nil
	}
	return slices.Clone(s.routes.selectors)
}

func (s *LoadoutSetup) hasSlice(item loadoutSlice) bool {
	return slices.ContainsFunc(s.routes.harnesses, func(h LoadoutHarness) bool {
		return h.agent == item.agent && slices.Contains(h.slices, item.category)
	})
}

// EligibleSources returns sources that offer the bundle's required active slices.
func (s *LoadoutSetup) EligibleSources(bundle *Bundle, harnesses []Agent) []LoadoutSource {
	if s == nil || bundle == nil || len(harnesses) == 0 {
		return nil
	}
	if !loadoutHooksSupported(bundle, harnesses) {
		return nil
	}
	required := loadoutRequired(bundle, harnesses)
	for _, item := range required {
		if !slices.Contains(harnesses, item.agent) || !s.hasSlice(item) || s.routes.custom[item] {
			return nil
		}
	}
	return slices.DeleteFunc(slices.Clone(s.sources), func(source LoadoutSource) bool {
		return slices.ContainsFunc(required, func(item loadoutSlice) bool {
			return !slices.Contains(source.uses, item.category)
		})
	})
}

// Select binds a declared source and the caller's requested harness coverage.
func (s *LoadoutSetup) Select(name string, harnesses []Agent, acceptSharedScope bool) (*LoadoutDestination, error) {
	if s == nil {
		return nil, newValueError("select requires a discovered Loadout setup")
	}
	agents := slices.Clone(harnesses)
	slices.Sort(agents)
	agents = slices.Compact(agents)
	if len(agents) == 0 || len(agents) != len(harnesses) {
		return nil, newValueError("select one or more distinct Loadout harnesses")
	}
	for _, source := range s.sources {
		if source.name == name {
			setup := *s
			return &LoadoutDestination{&setup, source, agents, acceptSharedScope}, nil
		}
	}
	return nil, newValueError(fmt.Sprintf("select a declared Loadout source, found no source %q", name))
}

// LoadoutDestination describes an explicit choice from a discovered setup.
type LoadoutDestination struct {
	setup             *LoadoutSetup
	source            LoadoutSource
	agents            []Agent
	acceptSharedScope bool
}

// Location returns the locator the host can retain for source-free inspection and removal.
func (d *LoadoutDestination) Location() LoadoutLocation {
	if d == nil || d.setup == nil {
		return LoadoutLocation{}
	}
	return LoadoutLocation{d.setup.root, d.setup.profile, d.source.name, d.source.path}
}

// Source returns the selected declaration.
func (d *LoadoutDestination) Source() LoadoutSource {
	if d == nil {
		return LoadoutSource{}
	}
	return d.source
}

// Harnesses returns the requested coverage.
func (d *LoadoutDestination) Harnesses() []Agent {
	if d == nil {
		return nil
	}
	return slices.Clone(d.agents)
}

// LoadoutLocation identifies the state index for one source in a registered profile.
type LoadoutLocation struct {
	root, profile, sourceName, sourcePath string
}

// NewLoadoutLocation reconstructs an earlier selection using its recorded location.
func NewLoadoutLocation(root, profile, sourceName, sourcePath string) (LoadoutLocation, error) {
	if !loadoutAbsolute(root) || !loadoutAbsolute(sourcePath) ||
		sourceName == "" || strings.ContainsAny(sourceName, "/\\\x00") {
		return LoadoutLocation{}, newValueError("Loadout location requires absolute paths and a source name")
	}
	if _, err := loadoutProfileFile(root, profile); err != nil {
		return LoadoutLocation{}, err
	}
	return LoadoutLocation{root, profile, sourceName, sourcePath}, nil
}

// Root returns the registered source root recorded at installation.
func (l LoadoutLocation) Root() string { return l.root }

// Profile returns the profile recorded at installation.
func (l LoadoutLocation) Profile() string { return l.profile }

// SourceName returns the declared source name recorded at installation.
func (l LoadoutLocation) SourceName() string { return l.sourceName }

// SourcePath returns the resolved source directory recorded at installation.
func (l LoadoutLocation) SourcePath() string { return l.sourcePath }

// Installation returns the lifecycle target without re-reading global registration.
func (l LoadoutLocation) Installation(st store.Store) (*InstallationTarget, error) {
	if _, err := NewLoadoutLocation(l.root, l.profile, l.sourceName, l.sourcePath); err != nil {
		return nil, err
	}
	return NewInstallationTarget(st, map[string]string{
		"destination": "loadout", "root": l.root, "profile": l.profile,
		"source-name": l.sourceName, "source-path": l.sourcePath,
	}, nil)
}

// LoadoutImpact describes an affected source file or selector and its harness reach.
type LoadoutImpact struct {
	category, path string
	agents         []Agent
	shared         bool
}

// Category returns the affected Loadout slice.
func (i LoadoutImpact) Category() string { return i.category }

// Path returns the affected source or declaring manifest.
func (i LoadoutImpact) Path() string { return i.path }

// Harnesses returns the affected harnesses in the known profile.
func (i LoadoutImpact) Harnesses() []Agent { return slices.Clone(i.agents) }

// Shared reports whether accepting wider scope is required.
func (i LoadoutImpact) Shared() bool { return i.shared }

// LoadoutOptions provides the explicit platform, runtime and replacement settings for a preview.
type LoadoutOptions struct {
	Platform        Platform
	HookRuntime     *HookRuntime
	ExecutionRoot   string
	ReplaceModified bool
}

// LoadoutPreview reports the chosen source, lifecycle plan and scope of each edit.
type LoadoutPreview struct {
	location   LoadoutLocation
	lifecycle  *LifecyclePreview
	impact     []LoadoutImpact
	diagnostic *LoadoutDiagnostic
}

// Location returns the recorded installation locator.
func (p *LoadoutPreview) Location() LoadoutLocation {
	if p == nil {
		return LoadoutLocation{}
	}
	return p.location
}

// Lifecycle returns the immutable plan for the source edits.
func (p *LoadoutPreview) Lifecycle() *LifecyclePreview {
	if p == nil || p.lifecycle == nil {
		return nil
	}
	snapshot := *p.lifecycle
	if snapshot.bundle != nil {
		bundle := *snapshot.bundle
		snapshot.bundle = &bundle
	}
	if snapshot.rendered != nil {
		rendered := *snapshot.rendered
		snapshot.rendered = &rendered
	}
	if snapshot.target != nil {
		target := *snapshot.target
		snapshot.target = &target
	}
	return &snapshot
}

// Impact returns the proposed edits' harness reach.
func (p *LoadoutPreview) Impact() []LoadoutImpact {
	if p == nil {
		return nil
	}
	return slices.Clone(p.impact)
}

// Diagnostic returns a copy of the capability failure, if any.
func (p *LoadoutPreview) Diagnostic() *LoadoutDiagnostic {
	if p == nil || p.diagnostic == nil {
		return nil
	}
	snapshot := *p.diagnostic
	return &snapshot
}

// Applicable reports whether the selected source edits can be applied.
func (p *LoadoutPreview) Applicable() bool { return p != nil && p.lifecycle.Applicable() }

// ScopeNotice reports the possible impact beyond the current profile.
func (*LoadoutPreview) ScopeNotice() string {
	return "Other profiles and projects using this source may also receive its content."
}

// LoadoutResult describes source changes, with the resulting installation locator.
type LoadoutResult struct {
	Location  LoadoutLocation
	Lifecycle InstallationResult
}

// SyncRequired reports that Loadout sync is needed to reflect source changes.
func (r LoadoutResult) SyncRequired() bool {
	return r.Lifecycle.Status != "" && r.Lifecycle.Status != StatusFailed
}

// DiscoverLoadout reads one explicit global Loadout registration and its profile chain.
func DiscoverLoadout(home, configHome string, env map[string]string) (LoadoutDiscovery, error) {
	if !loadoutAbsolute(home) {
		return LoadoutDiscovery{}, newValueError("home must be an absolute path")
	}
	if configHome == "" {
		configHome = env["XDG_CONFIG_HOME"]
	}
	if configHome == "" {
		configHome = filepath.Join(home, ".config")
	}
	if !loadoutAbsolute(configHome) {
		return LoadoutDiscovery{}, newValueError("Loadout configuration directory must be absolute")
	}
	registry := filepath.Join(configHome, "loadout", "config.toml")
	setup, err := readLoadoutSetup(home, registry)
	if err != nil {
		var issue *LoadoutDiagnostic
		if !errors.As(err, &issue) {
			issue = loadoutIssue("invalid-registry", err.Error(), registry)
		}
		return LoadoutDiscovery{LoadoutInvalid, registry, nil, issue}, nil
	}
	if setup == nil {
		return LoadoutDiscovery{Status: LoadoutAbsent, Registry: registry}, nil
	}
	return LoadoutDiscovery{Status: LoadoutAvailable, Registry: registry, Setup: setup}, nil
}

func readLoadoutSetup(home, registry string) (*LoadoutSetup, error) {
	inputs := loadoutInputs{}
	value, err := inputs.table(registry, "invalid-registry", "")
	if err != nil || value == nil {
		return nil, err
	}
	for key := range value {
		if !slices.Contains([]string{"source", "profile", "harnesses"}, key) {
			return nil, loadoutIssue("invalid-registry", "Unknown registration key "+key, registry)
		}
	}
	raw, _ := loadoutText(value["source"])
	logical, ok := loadoutSourcePath(raw, home, "")
	if !ok {
		return nil, loadoutIssue("invalid-registry", "Registration source must be absolute or begin with ~/", registry)
	}
	profile := "default"
	if value["profile"] != nil {
		profile, ok = loadoutText(value["profile"])
		if !ok || profile == "" {
			return nil, loadoutIssue("invalid-registry", "Registration profile must be a nonempty string", registry)
		}
	}
	if value["harnesses"] != nil {
		harnesses, err := loadoutStrings(value["harnesses"], registry, "harnesses", "invalid-registry")
		if err != nil {
			return nil, err
		}
		if len(harnesses) == 0 || slices.ContainsFunc(harnesses, func(name string) bool {
			return !loadoutKnownAgent(Agent(name))
		}) {
			return nil, loadoutIssue("invalid-registry", "Registration harnesses must be known names", registry)
		}
		slices.Sort(harnesses)
		if len(slices.Compact(harnesses)) != len(harnesses) {
			return nil, loadoutIssue("invalid-registry", "Registration harnesses contain duplicates", registry)
		}
	}
	root, err := inputs.directory(logical)
	if err != nil {
		return nil, err
	}
	routes, sources, chain, err := loadoutManifest(root, profile, home, inputs)
	if err != nil {
		return nil, err
	}
	conditions := inputs.conditions()
	if err := requireLoadoutInputs(conditions); err != nil {
		return nil, err
	}
	return &LoadoutSetup{registry, home, root, profile, chain, sources, routes, conditions}, nil
}

func requireLoadoutInputs(conditions []loadoutInput) *LoadoutDiagnostic {
	for _, seen := range conditions {
		if failure := seen.require(); failure != nil {
			return loadoutIssue("concurrent-change", "Loadout routing input changed", seen.path)
		}
	}
	return nil
}

func loadoutNotice(bundleID string) Notice {
	return Notice{ArtifactID: bundleID, Kind: NoticeActivation, Code: "loadout-sync-required",
		Message: "Run Loadout sync to reflect these source changes. An update or removal can retire assets still referenced by generated hooks and MCP servers."}
}

// PreviewLoadout plans installing a bundle into the selected authoritative source.
func PreviewLoadout(ctx context.Context, bundle *Bundle, destination *LoadoutDestination, st store.Store, opts LoadoutOptions) (*LoadoutPreview, error) {
	if ctx == nil || bundle == nil || destination == nil || destination.setup == nil {
		return nil, newValueError("Loadout preview requires a context, bundle and selected destination")
	}
	snapshot := *bundle
	bundle = &snapshot
	location := destination.Location()
	target, err := location.Installation(st)
	if err != nil {
		return nil, err
	}
	plan := &LoadoutPreview{location: location}
	inputs := loadoutInputs{}
	conditions := slices.Clone(destination.setup.conditions)
	issue := requireLoadoutInputs(conditions)
	var rendered *RenderedBundle
	if issue == nil {
		rendered, plan.impact, err = renderLoadout(bundle, destination, opts, inputs, loadoutScopePrefix(target))
		if err != nil && !errors.As(err, &issue) {
			issue = loadoutIssue("source-plan", err.Error(), destination.source.path)
		}
	}
	if issue == nil {
		conditions = append(conditions, inputs.conditions()...)
		issue = requireLoadoutInputs(conditions)
	}
	if issue != nil {
		code := CodeUnsupported
		if issue.Code == "concurrent-change" {
			code = CodeConcurrentChange
		}
		plan.diagnostic = issue
		plan.lifecycle = &LifecyclePreview{bundleID: bundle.ID(), bundle: bundle, rendered: EmptyRenderedBundle(),
			target: target, acquisition: AcquireConflict, replaceModified: opts.ReplaceModified,
			err: failure(code, issue.Message, issue.Path)}
		return plan, nil
	}
	plan.lifecycle, err = Preview(ctx, bundle, rendered, target, PreviewOptions{ReplaceModified: opts.ReplaceModified})
	if err != nil {
		return nil, err
	}
	plan.lifecycle.readPreconditions = conditions
	plan.guardOwnership()
	return plan, nil
}

func loadoutScopePrefix(target *InstallationTarget) string {
	return target.ContextDigest()[:32] + "-"
}

func (p *LoadoutPreview) guardOwnership() {
	prefix := loadoutScopePrefix(p.lifecycle.target)
	for _, resource := range p.lifecycle.resources {
		if resource.previous == nil {
			continue
		}
		if slices.ContainsFunc(resource.previous.claims, func(claim ownedClaim) bool {
			return claim.bundleID == p.lifecycle.bundleID && !strings.HasPrefix(claim.artifactID, prefix)
		}) {
			path := resource.resource.Destination
			message := "This bundle already owns the resource through another installation scope"
			p.diagnostic = loadoutIssue("source-ownership", message, path)
			p.lifecycle.err = failure(CodeConflict, message, path)
			return
		}
	}
}

// PreviewLoadoutRemoval plans removal from the previously recorded source.
func PreviewLoadoutRemoval(ctx context.Context, bundleID string, location LoadoutLocation, st store.Store, replaceModified bool) (*LoadoutPreview, error) {
	target, err := location.Installation(st)
	if err != nil {
		return nil, err
	}
	p, err := PreviewRemoval(ctx, bundleID, target, replaceModified)
	if err != nil {
		return nil, err
	}
	p.rendered, err = NewRenderedBundle(nil, nil, []Notice{loadoutNotice(bundleID)}, nil)
	if err != nil {
		return nil, err
	}
	plan := &LoadoutPreview{location: location, lifecycle: p}
	plan.guardOwnership()
	return plan, nil
}

// ApplyLoadout applies a source preview while enforcing its routing and ownership preconditions.
func ApplyLoadout(ctx context.Context, p *LoadoutPreview, options LockOptions) LoadoutResult {
	if ctx == nil || p == nil || p.lifecycle == nil {
		return LoadoutResult{Lifecycle: ApplyPreview(ctx, nil, options)}
	}
	if !p.Applicable() {
		issue := p.lifecycle.err
		for _, resource := range p.lifecycle.resources {
			if issue == nil {
				issue = resource.err
			}
		}
		if issue == nil {
			issue = failure(CodeRecoveryNeeded, "Recover the Loadout source installation and preview again", "")
		}
		return LoadoutResult{p.location, failedResult(ctx, p.lifecycle.bundleID, p.lifecycle.target, issue, nil)}
	}
	return LoadoutResult{p.location, ApplyPreview(ctx, p.lifecycle, options)}
}

// InspectLoadout inspects recorded source ownership independently of the active profile.
func InspectLoadout(ctx context.Context, bundleID string, location LoadoutLocation, st store.Store) (InstallationObservation, error) {
	target, err := location.Installation(st)
	if err != nil {
		return InstallationObservation{}, err
	}
	return InspectInstallation(ctx, bundleID, target)
}

// RecoverLoadout recovers the recorded source installation and its unfinished resources.
func RecoverLoadout(ctx context.Context, bundleID string, location LoadoutLocation, st store.Store, options LockOptions) (LoadoutResult, error) {
	target, err := location.Installation(st)
	if err != nil {
		return LoadoutResult{}, err
	}
	result, err := RecoverInstallation(ctx, bundleID, target, options)
	return LoadoutResult{location, result}, err
}
