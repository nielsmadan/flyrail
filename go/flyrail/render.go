package flyrail

import (
	"maps"
	"slices"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
)

func contentEqual(a, b Content) bool {
	left, leftErr := EncodeValue(modelContent(a))
	right, rightErr := EncodeValue(modelContent(b))
	return leftErr == nil && rightErr == nil && string(left) == string(right)
}

func schemasEqual(a, b []DocumentSchema) bool {
	return slices.EqualFunc(a, b, func(x, y DocumentSchema) bool {
		return x.format == y.format && x.key == y.key && ValuesEqual(x.value, y.value)
	})
}

func collisionNotices(artifacts []RenderedArtifact) []Notice {
	var notices []Notice
	for index, artifact := range artifacts {
		for _, previous := range artifacts[:index] {
			if !pathsEqual(artifact.destination, previous.destination) {
				continue
			}
			left, leftStructured := artifact.content.(StructuredContent)
			right, rightStructured := previous.content.(StructuredContent)
			if leftStructured && rightStructured && left.format != right.format {
				notices = append(notices, Notice{
					ArtifactID: artifact.id,
					Kind:       NoticeUnsupported,
					Message:    "Incompatible document formats from " + previous.id + " and " + artifact.id + " at " + displayPath(artifact.destination) + ".",
					Code:       "document-format-collision",
				})
				continue
			}
			claim, previousClaim := artifactClaim(artifact), artifactClaim(previous)
			if claim.Overlaps(previousClaim) && (!claim.Equal(previousClaim) ||
				!contentEqual(artifact.content, previous.content) ||
				!schemasEqual(artifact.schemaRequirements, previous.schemaRequirements) ||
				artifact.family != previous.family) {
				notices = append(notices, Notice{
					ArtifactID: artifact.id,
					Kind:       NoticeUnsupported,
					Message:    "Incompatible claims from " + previous.id + " and " + artifact.id + " at " + displayPath(artifact.destination) + ".",
					Code:       "destination-collision",
				})
			}
		}
	}
	return notices
}

func keyPart(part SelectorPart) (Key, bool) {
	key, ok := part.(Key)
	return key, ok
}

func mcpServersSelector(content StructuredContent) bool {
	if len(content.selector.parts) != 2 {
		return false
	}
	key, ok := keyPart(content.selector.parts[0])
	return ok && key.name == "mcpServers"
}

func copilotPrecedenceNotices(artifacts []RenderedArtifact, contexts []RenderContext) []Notice {
	var roots []string
	for _, rc := range contexts {
		if rc.audience.agent != AgentCopilot || rc.surface != SurfaceCLI || rc.target.scope != ScopeProject {
			continue
		}
		if !slices.ContainsFunc(roots, func(root string) bool { return pathsEqual(root, rc.Root()) }) {
			roots = append(roots, rc.Root())
		}
	}
	slices.SortFunc(roots, comparePaths)
	var notices []Notice
	for _, root := range roots {
		higher := pathx.Join(root, ".mcp.json")
		lower := pathx.Join(root, ".github/mcp.json")
		for _, artifact := range artifacts {
			if !pathsEqual(artifact.destination, lower) {
				continue
			}
			for _, previous := range artifacts {
				if !pathsEqual(previous.destination, higher) {
					continue
				}
				if notice, ok := copilotPrecedenceNotice(artifact, previous); ok {
					notices = append(notices, notice)
				}
			}
		}
	}
	return notices
}

func copilotPrecedenceNotice(artifact, previous RenderedArtifact) (Notice, bool) {
	content, structured := artifact.content.(StructuredContent)
	previousContent, previousStructured := previous.content.(StructuredContent)
	if !structured || !previousStructured || !mcpServersSelector(content) || !mcpServersSelector(previousContent) {
		return precedenceUnverified(artifact, previous), true
	}
	server, ok := keyPart(content.selector.parts[1])
	if !ok {
		return precedenceUnverified(artifact, previous), true
	}
	if _, ok := keyPart(previousContent.selector.parts[1]); !ok {
		return precedenceUnverified(artifact, previous), true
	}
	if selectorsEqual(content.selector, previousContent.selector) && !ValuesEqual(content.value, previousContent.value) {
		return Notice{
			ArtifactID: artifact.id,
			Kind:       NoticeUnsupported,
			Message: "Copilot CLI selects " + displayPath(previous.destination) + " over " + displayPath(artifact.destination) +
				" for server " + server.name + "; the selected records disagree.",
			Code: "copilot-mcp-precedence-collision",
		}, true
	}
	return Notice{}, false
}

func precedenceUnverified(artifact, previous RenderedArtifact) Notice {
	return Notice{
		ArtifactID: artifact.id,
		Kind:       NoticeActivation,
		Message: "Copilot CLI gives " + displayPath(previous.destination) + " precedence over " + displayPath(artifact.destination) +
			"; native content requires caller review of the effective server definitions.",
		Code: "copilot-mcp-precedence-unverified",
	}
}

// AssembleTranslations combines translations into a rendered bundle, blocking every output when any notice is unsupported.
func AssembleTranslations(translations []ArtifactTranslation, deps []Dependency, contexts []RenderContext, routing map[string]string) (*RenderedBundle, error) {
	for _, rc := range contexts {
		if err := rc.check(); err != nil {
			return nil, err
		}
	}
	var artifacts []RenderedArtifact
	var notices []Notice
	for _, translation := range translations {
		if translation.hasArtifact {
			artifacts = append(artifacts, translation.artifact)
		}
		notices = append(notices, translation.notices...)
	}
	notices = append(notices, collisionNotices(artifacts)...)
	notices = append(notices, copilotPrecedenceNotices(artifacts, contexts)...)
	if blocking(notices) {
		return newRenderedBundle(nil, nil, notices, contextEntries(routing))
	}
	return newRenderedBundle(artifacts, deps, notices, contextEntries(routing))
}

// Render translates b for rc without touching the filesystem.
func Render(b *Bundle, rc RenderContext) (*RenderedBundle, error) {
	if b == nil {
		return nil, newTypeError("bundle must be a Bundle")
	}
	if err := rc.check(); err != nil {
		return nil, err
	}
	return render(b, rc)
}

func render(b *Bundle, rc RenderContext) (*RenderedBundle, error) {
	agent := rc.audience.agent
	assetArtifacts, assets, err := supportAssetArtifacts(b, rc)
	if err != nil {
		return nil, err
	}
	var artifacts []RenderedArtifact
	var notices []Notice
	var hookDependencies []Dependency
	output, failure := hookArtifacts(b, rc, assets)
	switch {
	case failure == nil:
		artifacts = append(artifacts, output.artifacts...)
		hookDependencies = append(hookDependencies, output.dependencies...)
		notices = append(notices, output.notices...)
	case failure.unsupported != nil:
		for _, artifact := range b.artifacts {
			if hook, ok := artifact.(HookArtifact); ok {
				notices = append(notices, Notice{ArtifactID: hook.id, Kind: NoticeUnsupported, Message: failure.unsupported.Message, Code: failure.unsupported.Code})
			}
		}
	default:
		return nil, failure.invalid
	}

	selected := map[string]struct{}{}
	for _, artifact := range b.artifacts {
		var native RenderedArtifact
		var hasNative bool
		if source, ok := artifact.(NativeArtifact); ok {
			native, hasNative, err = translateNativeArtifact(source, rc)
			if err != nil {
				return nil, err
			}
			if !hasNative {
				continue
			}
		}
		selected[artifact.ID()] = struct{}{}
		if _, hook := artifact.(HookArtifact); !hook && !IsTranslatable(agent) {
			notices = append(notices, Notice{ArtifactID: artifact.ID(), Kind: NoticeUnsupported, Message: UntranslatableMessage(agent), Code: "agent-unsupported"})
			continue
		}
		var translation ArtifactTranslation
		switch source := artifact.(type) {
		case SkillArtifact:
			rendered, err := NewRenderedArtifact(RenderedArtifactParams{
				ID: source.id, Family: FamilySkills, Destination: rc.Skills(), Content: source.content, Subtree: source.name,
			})
			if err != nil {
				return nil, err
			}
			translation = ArtifactTranslation{artifact: rendered, hasArtifact: true}
		case InstructionArtifact:
			if translation, err = translateInstruction(b.ID(), source, rc); err != nil {
				return nil, err
			}
		case MCPArtifact:
			if translation, err = translateMCP(source, rc, assets); err != nil {
				return nil, err
			}
		case HookArtifact:
			continue
		case NativeArtifact:
			translation = ArtifactTranslation{artifact: native, hasArtifact: hasNative}
		}
		if translation.hasArtifact {
			artifacts = append(artifacts, translation.artifact)
		}
		notices = append(notices, translation.notices...)
	}

	pending := maps.Clone(selected)
	for len(pending) > 0 {
		required := map[string]struct{}{}
		for _, edge := range b.dependencies {
			if _, ok := pending[edge.dependent]; ok {
				required[edge.required] = struct{}{}
			}
		}
		pending = map[string]struct{}{}
		for name := range required {
			if _, ok := selected[name]; !ok {
				pending[name] = struct{}{}
			}
			selected[name] = struct{}{}
		}
	}
	for _, asset := range assetArtifacts {
		if _, ok := selected[asset.id]; ok {
			artifacts = append(artifacts, asset)
		}
	}
	emitted := make(map[string]struct{}, len(artifacts))
	for _, artifact := range artifacts {
		emitted[artifact.id] = struct{}{}
	}
	var unavailable []string
	for name := range selected {
		if _, ok := emitted[name]; !ok {
			unavailable = append(unavailable, name)
		}
	}
	slices.Sort(unavailable)
	for _, name := range unavailable {
		if !slices.ContainsFunc(notices, func(notice Notice) bool { return notice.ArtifactID == name && notice.Kind == NoticeUnsupported }) {
			notices = append(notices, Notice{ArtifactID: name, Kind: NoticeUnsupported, Message: "A selected dependency is unavailable for this audience.", Code: "dependency-audience"})
		}
	}
	notices = append(notices, collisionNotices(artifacts)...)
	notices = append(notices, copilotPrecedenceNotices(artifacts, []RenderContext{rc})...)
	routing := rc.routingEntries()
	if blocking(notices) {
		return newRenderedBundle(nil, nil, notices, routing)
	}
	var dependencies []Dependency
	for _, edge := range b.dependencies {
		if _, ok := emitted[edge.dependent]; ok {
			dependencies = append(dependencies, edge)
		}
	}
	dependencies = append(dependencies, hookDependencies...)
	return newRenderedBundle(artifacts, dependencies, notices, routing)
}

func routeJSON(entries []contextEntry) string {
	pairs := make([]jsonx.Value, len(entries))
	for index, entry := range entries {
		pairs[index] = jsonx.Array(jsonx.String(entry.key), jsonx.String(entry.value))
	}
	return string(jsonx.EncodePython(jsonx.Array(pairs...)))
}

// RenderMany renders b for every context and merges the results under route-qualified identifiers.
func RenderMany(b *Bundle, contexts ...RenderContext) (*RenderedBundle, error) {
	if len(contexts) == 0 {
		return nil, newValueError("render_many requires at least one explicit context")
	}
	if b == nil {
		return nil, newTypeError("bundle must be a Bundle")
	}
	unique := map[string]*RenderedBundle{}
	for _, rc := range contexts {
		if err := rc.check(); err != nil {
			return nil, err
		}
		rendered, err := render(b, rc)
		if err != nil {
			return nil, err
		}
		unique[routeJSON(rendered.routing)] = rendered
	}
	var artifacts []RenderedArtifact
	var dependencies []Dependency
	var notices []Notice
	var routes []contextEntry
	for _, route := range slices.Sorted(maps.Keys(unique)) {
		rendered := unique[route]
		routes = append(routes, contextEntry{key: identifier(route), value: route})
		names := make(map[string]string, len(rendered.artifacts))
		for _, artifact := range rendered.artifacts {
			names[artifact.id] = identifier(route, artifact.id)
		}
		for _, artifact := range rendered.artifacts {
			params := artifact.params()
			params.ID = names[artifact.id]
			renamed, err := NewRenderedArtifact(params)
			if err != nil {
				return nil, err
			}
			artifacts = append(artifacts, renamed)
		}
		for _, edge := range rendered.dependencies {
			dependent, okDependent := names[edge.dependent]
			required, okRequired := names[edge.required]
			if !okDependent || !okRequired {
				return nil, newValueError("missing route dependency identifier")
			}
			renamed, err := NewDependency(dependent, required, edge.mode)
			if err != nil {
				return nil, err
			}
			dependencies = append(dependencies, renamed)
		}
		for _, notice := range rendered.notices {
			notice.ArtifactID = identifier(route, notice.ArtifactID)
			notices = append(notices, notice)
		}
	}
	notices = append(notices, collisionNotices(artifacts)...)
	notices = append(notices, copilotPrecedenceNotices(artifacts, contexts)...)
	if blocking(notices) {
		return newRenderedBundle(nil, nil, notices, routes)
	}
	return newRenderedBundle(artifacts, dependencies, notices, routes)
}

func selectorPartsEqual(a, b SelectorPart) bool {
	switch left := a.(type) {
	case Key:
		right, ok := b.(Key)
		return ok && left.name == right.name
	case Member:
		right, ok := b.(Member)
		return ok && left.equal(right)
	}
	return false
}

func selectorsEqual(a, b Selector) bool {
	return slices.EqualFunc(a.parts, b.parts, selectorPartsEqual)
}

func artifactClaim(artifact RenderedArtifact) Claim {
	if artifact.subtree != "" {
		return Claim{kind: OwnershipSubtree, selector: SubtreeSelector(artifact.subtree)}
	}
	switch content := artifact.content.(type) {
	case FileContent:
		return Claim{kind: OwnershipFile, selector: WholeSelector{}}
	case SectionContent:
		return Claim{kind: OwnershipSection, selector: content.Selector()}
	case StructuredContent:
		return Claim{kind: OwnershipStructured, selector: content.selector}
	}
	return Claim{kind: OwnershipTree, selector: WholeSelector{}}
}
