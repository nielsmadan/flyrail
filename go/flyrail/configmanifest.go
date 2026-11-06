package flyrail

import (
	"math"
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

type configManifest struct {
	identity     BundleIdentity
	artifacts    []Artifact
	assets       []SupportAsset
	dependencies []Dependency
}

func configList(value jsonx.Value, present bool) ([]jsonx.Value, error) {
	if !present {
		return nil, nil
	}
	items, ok := value.Items()
	if !ok {
		return nil, newValueError("expected a JSON array")
	}
	return items, nil
}

func listField(fields manifestFields, key string) ([]jsonx.Value, error) {
	value, present := fields.lookup(key)
	return configList(value, present)
}

func sourcePath(value jsonx.Value) (string, error) {
	path, err := manifestString(value, "source path")
	if err != nil {
		return "", err
	}
	if err := validateRelativePath(path, "source path"); err != nil {
		return "", err
	}
	return path, nil
}

func kindOf(value jsonx.Value) string {
	kind, _ := value.Get("kind")
	text, _ := kind.AsString()
	return text
}

func configText(fields manifestFields, source bundleSource) (string, error) {
	text, hasText := fields.lookup("text")
	path, hasPath := fields.lookup("path")
	if hasText == hasPath {
		return "", invalidBundle("provide exactly one of text or path")
	}
	if hasText {
		return manifestString(text, "text")
	}
	relative, err := sourcePath(path)
	if err != nil {
		return "", err
	}
	data, err := source.readFile(relative)
	if err != nil {
		return "", err
	}
	if !utf8.Valid(data) {
		return "", invalidBundle("text is not valid UTF-8")
	}
	return string(data), nil
}

func configTree(fields manifestFields, source bundleSource) (TreeContent, error) {
	items, err := listField(fields, "executables")
	if err != nil {
		return TreeContent{}, err
	}
	executables := make([]string, 0, len(items))
	for _, item := range items {
		executable, err := sourcePath(item)
		if err != nil {
			return TreeContent{}, err
		}
		executables = append(executables, executable)
	}
	for index, executable := range executables {
		if slices.Contains(executables[:index], executable) {
			return TreeContent{}, invalidBundle("duplicate executable path")
		}
	}
	path, err := sourcePath(fields.get("path"))
	if err != nil {
		return TreeContent{}, err
	}
	entries, err := source.treeEntries(path, "content", executables)
	if err != nil {
		return TreeContent{}, err
	}
	rebased := make([]BundleEntry, 0, len(entries))
	for _, entry := range entries {
		if entry.Path == "content" {
			continue
		}
		relative, ok := strings.CutPrefix(entry.Path, "content/")
		if !ok {
			relative = entry.Path
		}
		rebasedEntry, err := newEntry(relative, entry.Data, entry.Dir, entry.Executable)
		if err != nil {
			return TreeContent{}, err
		}
		rebased = append(rebased, rebasedEntry)
	}
	content, err := NewTreeContent(rebased)
	if err != nil {
		return TreeContent{}, err
	}
	for _, executable := range executables {
		if !slices.ContainsFunc(content.entries, func(entry BundleEntry) bool { return !entry.Dir && entry.Path == executable }) {
			return TreeContent{}, invalidBundle("every executable must name an existing file")
		}
	}
	return content, nil
}

func configArgument(value jsonx.Value) (CommandArgument, error) {
	if _, ok := value.Members(); !ok {
		text, err := manifestString(value, "command argument")
		return ArgText(text), err
	}
	fields, err := manifestObject(value, []string{"asset"}, []string{"path"})
	if err != nil {
		return nil, err
	}
	assetID, err := manifestString(fields.get("asset"), "referenced asset id")
	if err != nil {
		return nil, err
	}
	raw, hasPath := fields.lookup("path")
	if !hasPath {
		return NewAssetRef(assetID, "")
	}
	path, err := sourcePath(raw)
	if err != nil {
		return nil, err
	}
	return NewAssetRef(assetID, path)
}

func configCommand(value jsonx.Value) (Command, error) {
	fields, err := manifestObject(value, []string{"argv"}, []string{"env", "cwd"})
	if err != nil {
		return Command{}, err
	}
	var env map[string]EnvironmentValue
	var names []string
	if raw, ok := fields.lookup("env"); ok {
		members, ok := raw.Members()
		if !ok {
			return Command{}, invalidBundle("command env must be a JSON object")
		}
		env = make(map[string]EnvironmentValue, len(members))
		for _, member := range members {
			names = append(names, member.Key)
			if _, isObject := member.Value.Members(); isObject {
				reference, err := manifestObject(member.Value, []string{"env"}, nil)
				if err != nil {
					return Command{}, err
				}
				name, err := manifestString(reference.get("env"), "environment reference")
				if err != nil {
					return Command{}, err
				}
				ref, err := NewEnvRef(name)
				if err != nil {
					return Command{}, err
				}
				env[member.Key] = ref
				continue
			}
			text, err := manifestString(member.Value, "environment value")
			if err != nil {
				return Command{}, err
			}
			env[member.Key] = EnvText(text)
		}
	}
	items, err := listField(fields, "argv")
	if err != nil {
		return Command{}, err
	}
	argv := make([]CommandArgument, 0, len(items))
	for _, item := range items {
		argument, err := configArgument(item)
		if err != nil {
			return Command{}, err
		}
		argv = append(argv, argument)
	}
	var cwd CommandArgument
	if raw, ok := fields.lookup("cwd"); ok {
		if cwd, err = configArgument(raw); err != nil {
			return Command{}, err
		}
	}
	return newCommand(argv, names, env, cwd)
}

func configMember(value jsonx.Value) (Member, error) {
	fields, err := manifestObject(value, nil, []string{"identity", "semantic_identity", "key"})
	if err != nil {
		return Member{}, err
	}
	identity, err := configValue(fields, "identity", "semantic_identity")
	if err != nil {
		return Member{}, err
	}
	items, err := listField(fields, "key")
	if err != nil {
		return Member{}, err
	}
	key := make([]string, 0, len(items))
	for _, item := range items {
		name, err := manifestString(item, "member key")
		if err != nil {
			return Member{}, err
		}
		key = append(key, name)
	}
	return NewMember(identity, key)
}

func configValue(fields manifestFields, plain, typed string) (Value, error) {
	plainValue, hasPlain := fields.lookup(plain)
	typedValue, hasTyped := fields.lookup(typed)
	if hasPlain == hasTyped {
		return nil, newValueError("provide exactly one of " + plain + " or " + typed)
	}
	if hasPlain {
		return valueFromJSON(plainValue)
	}
	return valueFromRecord(typedValue)
}

func configSelector(value jsonx.Value) (Selector, error) {
	items, err := configList(value, true)
	if err != nil {
		return Selector{}, err
	}
	parts := make([]SelectorPart, 0, len(items))
	for _, item := range items {
		if name, ok := item.AsString(); ok {
			key, err := NewKey(name)
			if err != nil {
				return Selector{}, err
			}
			parts = append(parts, key)
			continue
		}
		member, err := configMember(item)
		if err != nil {
			return Selector{}, err
		}
		parts = append(parts, member)
	}
	return NewSelector(parts...)
}

var contentFields = []string{
	"path", "text", "mode", "executables", "marker", "format", "selector", "value", "semantic_value", "placement", "boundaries",
}

func configContent(value jsonx.Value, source bundleSource) (Content, error) {
	if _, err := manifestObject(value, []string{"kind"}, contentFields); err != nil {
		return nil, err
	}
	switch kindOf(value) {
	case "file":
		return configFileContent(value, source)
	case "tree":
		fields, err := manifestObject(value, []string{"kind", "path"}, []string{"executables"})
		if err != nil {
			return nil, err
		}
		return configTree(fields, source)
	case "section":
		return configSectionContent(value, source)
	case "structured":
		return configStructuredContent(value)
	}
	return nil, invalidBundle("unknown native content kind")
}

func configFileContent(value jsonx.Value, source bundleSource) (Content, error) {
	fields, err := manifestObject(value, []string{"kind", "path"}, []string{"mode"})
	if err != nil {
		return nil, err
	}
	path, err := sourcePath(fields.get("path"))
	if err != nil {
		return nil, err
	}
	data, err := source.readFile(path)
	if err != nil {
		return nil, err
	}
	mode := ModePrivate
	if raw, ok := fields.lookup("mode"); ok {
		text, err := manifestString(raw, "file mode")
		if err != nil {
			return nil, err
		}
		if mode, err = ParseFileMode(text); err != nil {
			return nil, err
		}
	}
	return NewFileContent(data, mode)
}

func configSectionContent(value jsonx.Value, source bundleSource) (Content, error) {
	fields, err := manifestObject(value, []string{"kind", "marker"}, []string{"text", "path", "boundaries"})
	if err != nil {
		return nil, err
	}
	var boundaries *SectionBoundaries
	if raw, ok := fields.lookup("boundaries"); ok {
		lines, err := manifestObject(raw, []string{"start", "end"}, nil)
		if err != nil {
			return nil, err
		}
		start, err := manifestString(lines.get("start"), "start boundary")
		if err != nil {
			return nil, err
		}
		end, err := manifestString(lines.get("end"), "end boundary")
		if err != nil {
			return nil, err
		}
		parsed, err := NewSectionBoundaries(start, end)
		if err != nil {
			return nil, err
		}
		boundaries = &parsed
	}
	marker, err := manifestString(fields.get("marker"), "section marker")
	if err != nil {
		return nil, err
	}
	text, err := configText(fields, source)
	if err != nil {
		return nil, err
	}
	return NewSectionContent(marker, text, boundaries)
}

func configPlacement(raw jsonx.Value) (*Placement, error) {
	fields, err := manifestObject(raw, []string{"position"}, []string{"anchor"})
	if err != nil {
		return nil, err
	}
	text, err := manifestString(fields.get("position"), "placement position")
	if err != nil {
		return nil, err
	}
	position, err := ParsePosition(text)
	if err != nil {
		return nil, err
	}
	var anchor *Member
	if rawAnchor, ok := fields.lookup("anchor"); ok {
		member, err := configMember(rawAnchor)
		if err != nil {
			return nil, err
		}
		anchor = &member
	}
	placement, err := NewPlacement(position, anchor)
	if err != nil {
		return nil, err
	}
	return &placement, nil
}

func configStructuredContent(value jsonx.Value) (Content, error) {
	fields, err := manifestObject(value, []string{"kind", "format", "selector"}, []string{"value", "semantic_value", "placement"})
	if err != nil {
		return nil, err
	}
	var placement *Placement
	if raw, ok := fields.lookup("placement"); ok {
		if placement, err = configPlacement(raw); err != nil {
			return nil, err
		}
	}
	text, err := manifestString(fields.get("format"), "document format")
	if err != nil {
		return nil, err
	}
	format, err := ParseDocumentFormat(text)
	if err != nil {
		return nil, err
	}
	selector, err := configSelector(fields.get("selector"))
	if err != nil {
		return nil, err
	}
	semantic, err := configValue(fields, "value", "semantic_value")
	if err != nil {
		return nil, err
	}
	return NewStructuredContent(format, selector, semantic, placement)
}

var artifactFields = []string{
	"name", "path", "executables", "text", "transport", "event", "command", "timeout_ms", "outcomes", "tools",
	"protocol_version", "family", "audience", "destination", "content",
}

func configArtifact(value jsonx.Value, source bundleSource) (Artifact, error) {
	header, err := manifestObject(value, []string{"kind", "id"}, artifactFields)
	if err != nil {
		return nil, err
	}
	id, err := manifestString(header.get("id"), "artifact id")
	if err != nil {
		return nil, err
	}
	switch kindOf(value) {
	case "skill":
		fields, err := manifestObject(value, []string{"kind", "id", "name", "path"}, []string{"executables"})
		if err != nil {
			return nil, err
		}
		name, err := manifestString(fields.get("name"), "skill name")
		if err != nil {
			return nil, err
		}
		content, err := configTree(fields, source)
		if err != nil {
			return nil, err
		}
		return NewSkillArtifact(id, name, content)
	case "instruction":
		fields, err := manifestObject(value, []string{"kind", "id"}, []string{"text", "path"})
		if err != nil {
			return nil, err
		}
		text, err := configText(fields, source)
		if err != nil {
			return nil, err
		}
		return NewInstructionArtifact(id, text)
	case "mcp":
		return configMCPArtifact(value, id)
	case "hook":
		return configHookArtifact(value, id)
	case "native":
		return configNativeArtifact(value, id, source)
	}
	return nil, invalidBundle("unknown artifact kind")
}

func configMCPArtifact(value jsonx.Value, id string) (Artifact, error) {
	fields, err := manifestObject(value, []string{"kind", "id", "name", "transport"}, nil)
	if err != nil {
		return nil, err
	}
	raw := fields.get("transport")
	if _, err := manifestObject(raw, []string{"kind"}, []string{"command", "url", "bearer_token_env"}); err != nil {
		return nil, err
	}
	var transport MCPTransport
	switch kindOf(raw) {
	case "stdio":
		stdio, err := manifestObject(raw, []string{"kind", "command"}, nil)
		if err != nil {
			return nil, err
		}
		if transport, err = configCommand(stdio.get("command")); err != nil {
			return nil, err
		}
	case "http":
		http, err := manifestObject(raw, []string{"kind", "url"}, []string{"bearer_token_env"})
		if err != nil {
			return nil, err
		}
		url, err := manifestString(http.get("url"), "MCP URL")
		if err != nil {
			return nil, err
		}
		var token *EnvRef
		if rawToken, ok := http.lookup("bearer_token_env"); ok {
			name, err := manifestString(rawToken, "bearer environment reference")
			if err != nil {
				return nil, err
			}
			reference, err := NewEnvRef(name)
			if err != nil {
				return nil, err
			}
			token = &reference
		}
		if transport, err = NewHTTPTransport(url, token); err != nil {
			return nil, err
		}
	default:
		return nil, invalidBundle("unknown MCP transport")
	}
	name, err := manifestString(fields.get("name"), "MCP server name")
	if err != nil {
		return nil, err
	}
	return NewMCPArtifact(id, name, transport)
}

func configHookArtifact(value jsonx.Value, id string) (Artifact, error) {
	fields, err := manifestObject(value, []string{"kind", "id", "event", "command", "protocol_version"}, []string{"timeout_ms", "outcomes", "tools"})
	if err != nil {
		return nil, err
	}
	if version, ok := fields.get("protocol_version").AsInt(); !ok || version != 1 {
		return nil, invalidBundle("hook protocol_version must be integer 1")
	}
	timeout := 10_000
	if raw, ok := fields.lookup("timeout_ms"); ok {
		number, ok := raw.AsInt()
		if !ok {
			return nil, invalidBundle("hook timeout must be an integer")
		}
		if number < math.MinInt32 || number > math.MaxInt32 {
			return nil, invalidBundle("hook timeout must be 1-300000 integer milliseconds")
		}
		timeout = int(number)
	}
	text, err := manifestString(fields.get("event"), "hook event")
	if err != nil {
		return nil, err
	}
	event, err := ParseHookEvent(text)
	if err != nil {
		return nil, err
	}
	command, err := configCommand(fields.get("command"))
	if err != nil {
		return nil, err
	}
	outcomes := []HookOutcome{OutcomeContinue}
	if raw, ok := fields.lookup("outcomes"); ok {
		items, err := configList(raw, true)
		if err != nil {
			return nil, err
		}
		outcomes = make([]HookOutcome, 0, len(items))
		for _, item := range items {
			text, err := manifestString(item, "hook outcome")
			if err != nil {
				return nil, err
			}
			outcome, err := ParseHookOutcome(text)
			if err != nil {
				return nil, err
			}
			outcomes = append(outcomes, outcome)
		}
	}
	items, err := listField(fields, "tools")
	if err != nil {
		return nil, err
	}
	tools := make([]string, 0, len(items))
	for _, item := range items {
		tool, err := manifestString(item, "hook tool")
		if err != nil {
			return nil, err
		}
		tools = append(tools, tool)
	}
	return NewHookArtifact(HookArtifactParams{
		ID:            id,
		Event:         event,
		Command:       command,
		TimeoutMillis: timeout,
		Outcomes:      outcomes,
		Tools:         tools,
	})
}

func configAudience(value jsonx.Value) (Audience, error) {
	fields, err := manifestObject(value, []string{"agent", "scope"}, []string{"surface", "platform"})
	if err != nil {
		return Audience{}, err
	}
	agentText, err := manifestString(fields.get("agent"), "agent")
	if err != nil {
		return Audience{}, err
	}
	agent, err := ParseAgent(agentText)
	if err != nil {
		return Audience{}, err
	}
	scopeText, err := manifestString(fields.get("scope"), "scope")
	if err != nil {
		return Audience{}, err
	}
	scope, err := ParseTargetScope(scopeText)
	if err != nil {
		return Audience{}, err
	}
	surface := SurfaceCLI
	if raw, ok := fields.lookup("surface"); ok {
		text, err := manifestString(raw, "surface")
		if err != nil {
			return Audience{}, err
		}
		if surface, err = ParseSurface(text); err != nil {
			return Audience{}, err
		}
	}
	var platform Platform
	if raw, ok := fields.lookup("platform"); ok {
		text, err := manifestString(raw, "platform")
		if err != nil {
			return Audience{}, err
		}
		if platform, err = ParsePlatform(text); err != nil {
			return Audience{}, err
		}
	}
	return NewAudience(agent, scope, surface, platform)
}

func configNativeArtifact(value jsonx.Value, id string, source bundleSource) (Artifact, error) {
	fields, err := manifestObject(value, []string{"kind", "id", "family", "audience", "destination", "content"}, nil)
	if err != nil {
		return nil, err
	}
	if _, err := manifestObject(fields.get("audience"), []string{"agent", "scope"}, []string{"surface", "platform"}); err != nil {
		return nil, err
	}
	familyText, err := manifestString(fields.get("family"), "native family")
	if err != nil {
		return nil, err
	}
	family, err := ParseFamily(familyText)
	if err != nil {
		return nil, err
	}
	audience, err := configAudience(fields.get("audience"))
	if err != nil {
		return nil, err
	}
	destination, err := sourcePath(fields.get("destination"))
	if err != nil {
		return nil, err
	}
	content, err := configContent(fields.get("content"), source)
	if err != nil {
		return nil, err
	}
	return NewNativeArtifact(id, family, audience, destination, content)
}

func readConfigManifest(raw jsonx.Value, source bundleSource) (configManifest, error) {
	fields, err := manifestObject(raw, []string{"schema_version", "id", "version", "artifacts"}, []string{"assets", "dependencies"})
	if err != nil {
		return configManifest{}, err
	}
	if version, ok := fields.get("schema_version").AsInt(); !ok || version != 2 {
		return configManifest{}, invalidBundle("configuration schema_version must be integer 2")
	}
	items, err := listField(fields, "artifacts")
	if err != nil {
		return configManifest{}, err
	}
	artifacts := make([]Artifact, 0, len(items))
	for _, item := range items {
		artifact, err := configArtifact(item, source)
		if err != nil {
			return configManifest{}, err
		}
		artifacts = append(artifacts, artifact)
	}
	if items, err = listField(fields, "assets"); err != nil {
		return configManifest{}, err
	}
	assets := make([]SupportAsset, 0, len(items))
	for _, item := range items {
		asset, err := configSupportAsset(item, source)
		if err != nil {
			return configManifest{}, err
		}
		assets = append(assets, asset)
	}
	if items, err = listField(fields, "dependencies"); err != nil {
		return configManifest{}, err
	}
	dependencies := make([]Dependency, 0, len(items))
	for _, item := range items {
		dependency, err := configDependency(item)
		if err != nil {
			return configManifest{}, err
		}
		dependencies = append(dependencies, dependency)
	}
	id, err := manifestString(fields.get("id"), "bundle id")
	if err != nil {
		return configManifest{}, err
	}
	version, err := manifestString(fields.get("version"), "version")
	if err != nil {
		return configManifest{}, err
	}
	identity, err := NewBundleIdentity(id, version)
	if err != nil {
		return configManifest{}, err
	}
	return configManifest{identity: identity, artifacts: artifacts, assets: assets, dependencies: dependencies}, nil
}

func configSupportAsset(value jsonx.Value, source bundleSource) (SupportAsset, error) {
	fields, err := manifestObject(value, []string{"id", "family", "path"}, []string{"executables"})
	if err != nil {
		return SupportAsset{}, err
	}
	id, err := manifestString(fields.get("id"), "asset id")
	if err != nil {
		return SupportAsset{}, err
	}
	familyText, err := manifestString(fields.get("family"), "asset family")
	if err != nil {
		return SupportAsset{}, err
	}
	family, err := ParseFamily(familyText)
	if err != nil {
		return SupportAsset{}, err
	}
	content, err := configTree(fields, source)
	if err != nil {
		return SupportAsset{}, err
	}
	return NewSupportAsset(id, family, content)
}

func configDependency(value jsonx.Value) (Dependency, error) {
	fields, err := manifestObject(value, []string{"dependent", "required"}, []string{"mode"})
	if err != nil {
		return Dependency{}, err
	}
	dependent, err := manifestString(fields.get("dependent"), "dependent")
	if err != nil {
		return Dependency{}, err
	}
	required, err := manifestString(fields.get("required"), "required")
	if err != nil {
		return Dependency{}, err
	}
	mode := DependencyRevision
	if raw, ok := fields.lookup("mode"); ok {
		text, err := manifestString(raw, "dependency mode")
		if err != nil {
			return Dependency{}, err
		}
		if mode, err = ParseDependencyMode(text); err != nil {
			return Dependency{}, err
		}
	}
	return NewDependency(dependent, required, mode)
}
