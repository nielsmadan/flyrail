package flyrail

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"maps"
	"path/filepath"
	"slices"
	"strings"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

func loadoutID(parts ...string) string {
	digest := sha256.Sum256([]byte(strings.Join(parts, "\x00")))
	return hex.EncodeToString(digest[:])
}

func loadoutModule(agent Agent) (directory, prefix string) {
	switch agent {
	case AgentOpencode:
		return "plugins", ".config/opencode/plugins/"
	case AgentPi:
		return "extensions", ".pi/agent/extensions/"
	default:
		return "", ""
	}
}

func loadoutRequired(bundle *Bundle, agents []Agent) []loadoutSlice {
	var result []loadoutSlice
	for _, artifact := range bundle.artifacts {
		if native, ok := artifact.(NativeArtifact); ok {
			result = append(result, loadoutSlice{native.audience.agent, "module-config"})
			continue
		}
		for _, agent := range agents {
			category := ""
			switch artifact.(type) {
			case InstructionArtifact:
				category = "instructions"
			case SkillArtifact:
				category = "skills"
			case MCPArtifact:
				category = "mcp"
			case HookArtifact:
				category = "hooks"
				if directory, _ := loadoutModule(agent); directory != "" {
					category = "module-config"
				}
			}
			item := loadoutSlice{agent, category}
			if !slices.Contains(result, item) {
				result = append(result, item)
			}
		}
	}
	return result
}

func loadoutHooksSupported(bundle *Bundle, agents []Agent) bool {
	return !slices.Contains(agents, AgentDroid) || !slices.ContainsFunc(bundle.artifacts, func(a Artifact) bool {
		_, hook := a.(HookArtifact)
		return hook
	})
}

func loadoutFiltered(path string) bool {
	for _, part := range strings.Split(path, "/") {
		if slices.Contains([]string{".git", ".hg", "node_modules", ".venv", "__pycache__", ".mypy_cache", ".ruff_cache", ".pytest_cache"}, part) {
			return true
		}
	}
	return slices.Contains([]string{".DS_Store", ".loadout-bundle.toml"}, filepath.Base(path)) ||
		slices.Contains([]string{".pyc", ".pyo", ".swp"}, filepath.Ext(path))
}

func loadoutStructured(keys []string, value Value, member bool) (StructuredContent, error) {
	steps := make([]SelectorPart, 0, len(keys)+1)
	for _, key := range keys {
		selected, err := NewKey(key)
		if err != nil {
			return StructuredContent{}, err
		}
		steps = append(steps, selected)
	}
	if member {
		selected, err := NewMember(value, nil)
		if err != nil {
			return StructuredContent{}, err
		}
		steps = append(steps, selected)
	}
	selector, err := NewSelector(steps...)
	if err != nil {
		return StructuredContent{}, err
	}
	return NewStructuredContent(FormatTOML, selector, value, nil)
}

type loadoutRender struct {
	bundle      *Bundle
	destination *LoadoutDestination
	opts        LoadoutOptions
	inputs      loadoutInputs
	artifacts   []RenderedArtifact
	edges       map[Dependency]bool
	notices     []Notice
	impacts     []LoadoutImpact
	assets      map[string]string
}

func (r *loadoutRender) source() LoadoutSource { return r.destination.source }

func (r *loadoutRender) fail(code, message string) *LoadoutDiagnostic {
	return loadoutIssue(code, message, r.destination.setup.chain[0])
}

func (r *loadoutRender) gate() error {
	setup := r.destination.setup
	for _, agent := range r.destination.agents {
		if !loadoutKnownAgent(agent) {
			return r.fail("unsupported-harness", fmt.Sprintf("%s is outside the Loadout harness set", agent))
		}
		if !slices.ContainsFunc(setup.routes.harnesses, func(h LoadoutHarness) bool { return h.agent == agent }) {
			return r.fail("disabled-harness", fmt.Sprintf("%s is absent from this profile", agent))
		}
	}
	if !loadoutHooksSupported(r.bundle, r.destination.agents) {
		return r.fail("unsupported-hook", "Droid hooks have no verified Loadout mapping")
	}
	for _, item := range loadoutRequired(r.bundle, r.destination.agents) {
		if !slices.Contains(r.source().uses, item.category) {
			return loadoutIssue("source-use", fmt.Sprintf("Source %s does not offer %s", r.source().name, item.category), r.source().declaredIn)
		}
		if !slices.Contains(r.destination.agents, item.agent) {
			return r.fail("native-audience", fmt.Sprintf("Native artifact targets %s outside this selection", item.agent))
		}
		if !setup.hasSlice(item) {
			return r.fail("disabled-slice", fmt.Sprintf("%s.%s is disabled in this profile", item.agent, item.category))
		}
		if setup.routes.custom[item] {
			return r.fail("unsupported-slice", fmt.Sprintf("%s.%s uses a custom legacy renderer", item.agent, item.category))
		}
	}
	return nil
}

func (r *loadoutRender) context(agent Agent) (RenderContext, error) {
	target, err := UserTarget(agent, r.destination.setup.home, nil)
	if err != nil {
		return RenderContext{}, err
	}
	return NewRenderContext(target, r.opts.Platform, RenderOptions{
		ExecutionRoot: r.opts.ExecutionRoot, HookRuntime: r.opts.HookRuntime,
		AssetRoot: filepath.Join(r.source().path, ".flyrail-assets"),
	})
}

func (r *loadoutRender) safe(path string) (string, error) {
	resolved, err := resolveLoadoutPath(path)
	if err != nil || resolved != path || !loadoutContains(r.source().path, path) {
		return "", loadoutIssue("escaped-path", "Source destination must stay within its declared root without aliases", path)
	}
	return path, nil
}

func (r *loadoutRender) impact(category, path string, agents []Agent, inherited bool) {
	shared := inherited || slices.ContainsFunc(agents, func(a Agent) bool { return !slices.Contains(r.destination.agents, a) })
	agents = slices.Clone(agents)
	slices.Sort(agents)
	agents = slices.Compact(agents)
	if !slices.ContainsFunc(r.impacts, func(item LoadoutImpact) bool {
		return item.category == category && item.path == path && item.shared == shared && slices.Equal(item.agents, agents)
	}) {
		r.impacts = append(r.impacts, LoadoutImpact{category, path, agents, shared})
	}

}

func (r *loadoutRender) fileSeen(source LoadoutSource, relative string) (bool, error) {
	data, err := r.inputs.file(filepath.Join(source.path, relative), "invalid-source", source.path)
	return data != nil, err
}

func (r *loadoutRender) write(id string, family Family, path string, payload Content, subtree string) error {
	item, err := NewRenderedArtifact(RenderedArtifactParams{ID: id, Family: family, Destination: path, Content: payload, Subtree: subtree})
	if err != nil {
		return err
	}
	r.artifacts = append(r.artifacts, item)
	return nil
}

func (r *loadoutRender) edge(dependent, required string, mode DependencyMode) error {
	edge, err := NewDependency(dependent, required, mode)
	if err != nil {
		return err
	}
	r.edges[edge] = true
	return nil
}

func (r *loadoutRender) skill(item SkillArtifact) error {
	name := item.name
	root, err := r.safe(filepath.Join(r.source().path, "skills"))
	if err != nil {
		return err
	}
	if loadoutFiltered(name) || slices.ContainsFunc(item.content.entries, func(e BundleEntry) bool { return loadoutFiltered(e.Path) }) {
		return r.fail("skill-filter", "Loadout excludes part of skill "+name)
	}
	for _, source := range r.destination.setup.sources {
		if !slices.Contains(source.uses, "skills") {
			continue
		}
		paths := []string{filepath.Join("skills", name+".local", "SKILL.md")}
		if source.name != r.source().name {
			paths = append(paths, filepath.Join("skills", name, "SKILL.md"))
		}
		if slices.Contains(source.overrides["skills"], name) {
			return loadoutIssue("source-collision", "An override controls skill "+name, source.declaredIn)
		}
		for _, path := range paths {
			exists, err := r.fileSeen(source, path)
			if err != nil {
				return err
			}
			if exists {
				return loadoutIssue("source-collision", "Skill "+name+" also resolves from "+source.name, filepath.Join(source.path, path))
			}
		}
	}
	active := r.destination.setup.chain[0]
	manifest := active
	if files, found := r.destination.setup.routes.policies[name]; found {
		if len(files) != 1 {
			return r.fail("skill-policy", "Split declarations control skill "+name)
		}
		manifest = files[0]
	}
	r.impact("skills", manifest, r.destination.agents, manifest != active)
	agents := modelList(r.destination.agents, func(agent Agent) Value { return modelText(string(agent)) })
	policy, err := loadoutStructured([]string{"skills", name}, modelObject(field("agents", agents), field("render", Bool(false))), false)
	if err != nil {
		return err
	}
	policyID := loadoutID("skill-policy", item.id)
	if err := r.write(policyID, FamilySkills, manifest, policy, ""); err != nil {
		return err
	}
	if err := r.write(item.id, FamilySkills, root, item.content, name); err != nil {
		return err
	}
	return r.edge(item.id, policyID, DependencyRevision)
}

func (r *loadoutRender) routes(category string) [][]LoadoutSelector {
	grouped := map[string][]LoadoutSelector{}
	for _, item := range r.destination.setup.routes.selectors {
		if item.category == category {
			key := strings.Join(append([]string{item.manifest}, item.key...), "\x00")
			grouped[key] = append(grouped[key], item)
		}
	}
	var result [][]LoadoutSelector
	for _, key := range slices.Sorted(maps.Keys(grouped)) {
		list := grouped[key]
		if slices.ContainsFunc(list, func(item LoadoutSelector) bool { return slices.Contains(r.destination.agents, item.agent) }) {
			result = append(result, list)
		}
	}
	return result
}

func (r *loadoutRender) entry(routes []LoadoutSelector, name string, family Family, requires string) error {
	route := routes[0]
	agents := make([]Agent, 0, len(routes))
	shared := route.manifest != r.destination.setup.chain[0]
	qualified := r.source().name + "/" + name
	for _, item := range routes {
		agents = append(agents, item.agent)
		shared = shared || item.destinations > 1 || item.allProfiles
		if _, swapped := item.swaps[qualified]; swapped || loadoutRefersTo(item, name, qualified, true) {
			return loadoutIssue("selector-shadowed", "An existing selector or substitution shadows "+qualified, item.manifest)
		}
	}
	r.impact(route.category, route.manifest, agents, shared)
	if !route.array {
		return loadoutIssue("selector-not-array", "Write the fragment selector as an array before adding a member", route.manifest)
	}
	content, err := loadoutStructured(route.key, modelText(qualified), true)
	if err != nil {
		return err
	}
	parts := append([]string{"selector", route.manifest}, route.key...)
	id := loadoutID(append(parts, qualified)...)
	if err := r.write(id, family, route.manifest, content, ""); err != nil {
		return err
	}
	return r.edge(id, requires, DependencyStableReference)
}

func (r *loadoutRender) fragmentPath(category, relative string) (string, error) {
	path, err := r.safe(filepath.Join(r.source().path, relative))
	if err != nil {
		return "", err
	}
	for _, source := range r.destination.setup.sources {
		if source.name == r.source().name || !slices.Contains(source.uses, category) {
			continue
		}
		found, err := r.fileSeen(source, relative)
		if err != nil {
			return "", err
		}
		if found {
			return "", loadoutIssue("source-collision", "Fragment already exists in "+source.name, filepath.Join(source.path, relative))
		}
	}
	return path, nil
}

func loadoutRefersTo(route LoadoutSelector, name, qualified string, precedesAdd bool) bool {
	return slices.ContainsFunc(route.entries, func(entry string) bool {
		if precedesAdd && entry == qualified {
			return false
		}
		effective, found := route.swaps[entry]
		if !found {
			effective = entry
		}
		return effective == name || effective == qualified
	})
}

func (r *loadoutRender) checkUnselected(category, name string, groups [][]LoadoutSelector) error {
	qualified := r.source().name + "/" + name
	for _, other := range r.destination.setup.routes.selectors {
		if other.category != category || slices.ContainsFunc(groups, func(group []LoadoutSelector) bool {
			return slices.ContainsFunc(group, func(route LoadoutSelector) bool {
				return route.manifest == other.manifest && slices.Equal(route.key, other.key)
			})
		}) {
			continue
		}
		if loadoutRefersTo(other, name, qualified, false) {
			return loadoutIssue("selector-shadowed", "An unselected "+category+" target refers to "+qualified, other.manifest)
		}
	}
	return nil
}

func (r *loadoutRender) instruction(item InstructionArtifact) error {
	name := "flyrail-" + loadoutID("instructions", r.bundle.ID(), item.id)
	path, err := r.fragmentPath("instructions", filepath.Join("instructions", name+".md"))
	if err != nil {
		return err
	}
	routes := r.routes("instructions")
	if err := r.checkUnselected("instructions", name, routes); err != nil {
		return err
	}
	payload, err := NewFileContent([]byte(item.text), ModePrivate)
	if err != nil {
		return err
	}
	if err := r.write(item.id, FamilyInstructions, path, payload, ""); err != nil {
		return err
	}
	for _, group := range routes {
		if err := r.entry(group, name, FamilyInstructions, item.id); err != nil {
			return err
		}
	}
	return nil
}

func (r *loadoutRender) mcpTables() (map[string][]string, error) {
	servers := map[string][]string{}
	for _, source := range r.destination.setup.sources {
		if !slices.Contains(source.uses, "mcp") {
			continue
		}
		path := filepath.Join(source.path, "mcp.toml")
		table, err := r.inputs.table(path, "invalid-mcp", source.path)
		if err != nil {
			return nil, err
		}
		names := slices.Sorted(maps.Keys(table))
		for _, name := range names {
			block, err := toLoadoutTable(table[name], path, name)
			if err != nil {
				return nil, loadoutIssue("invalid-mcp", err.Error(), path)
			}
			transport, _ := loadoutText(block["transport"])
			command, _ := loadoutText(block["command"])
			url, _ := loadoutText(block["url"])
			if (transport != "http" || url == "") && (transport != "stdio" || command == "") {
				return nil, loadoutIssue("invalid-mcp", "Invalid Loadout MCP definition "+name, path)
			}
			for key := range block {
				if !slices.Contains([]string{"transport", "command", "args", "env", "url", "auth_env_var", "headers"}, key) {
					return nil, loadoutIssue("invalid-mcp", "Unknown field in Loadout MCP definition "+name, path)
				}
			}
			if !loadoutMCPFields(block, transport, path) {
				return nil, loadoutIssue("invalid-mcp", "Invalid headers or environment for Loadout MCP definition "+name, path)
			}
		}
		servers[source.name] = names
	}
	return servers, nil
}

func loadoutMCPFields(block loadoutTable, transport, path string) bool {
	headers, headerErr := toLoadoutTable(block["headers"], path, "headers")
	environment, envErr := toLoadoutTable(block["env"], path, "env")
	auth, validAuth := loadoutText(block["auth_env_var"])
	if headerErr != nil || envErr != nil || (block["auth_env_var"] != nil && !validAuth) || (len(headers) > 0 && transport != "http") {
		return false
	}
	for key, value := range headers {
		text, ok := loadoutText(value)
		if !ok || strings.ContainsAny(text, "${") || strings.HasPrefix(text, "!") || (auth != "" && strings.EqualFold(key, "Authorization")) {
			return false
		}
	}
	for _, value := range environment {
		if _, ok := loadoutText(value); !ok {
			return false
		}
	}
	if raw := block["args"]; raw != nil {
		_, err := loadoutStrings(raw, path, "args", "invalid-mcp")
		return err == nil
	}
	return true
}

func loadoutNamespace(text string, pi bool) string {
	return strings.Map(func(c rune) rune {
		if c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' ||
			c == '_' || c == '*' || c == '?' || (!pi && c == '-') {
			return c
		}
		return '_'
	}, text)
}

func loadoutPermissionOverlap(other, tool, server string, pi bool) bool {
	separator := "_"
	if pi {
		separator = "__"
	}
	prefix := loadoutNamespace(server, pi) + separator
	namespace := loadoutNamespace(other, pi) + separator
	if strings.ContainsAny(other, "*?") {
		return false
	}
	if namespace == prefix {
		return other != server
	}
	pattern := namespace + loadoutNamespace(tool, pi)
	for i := range len(prefix) {
		if i == len(pattern) {
			return false
		}
		if pattern[i] == '*' {
			return true
		}
		if pattern[i] != '?' && pattern[i] != prefix[i] {
			return false
		}
	}
	return len(pattern) > len(prefix)
}

func (r *loadoutRender) checkMCPPermissions(servers []string) error {
	setup := r.destination.setup
	opencode := setup.hasSlice(loadoutSlice{AgentOpencode, "permissions"})
	pi := setup.hasSlice(loadoutSlice{AgentPi, "permissions"})
	if !opencode && !pi {
		return nil
	}
	for _, source := range setup.sources {
		if !slices.Contains(source.uses, "permissions") {
			continue
		}
		path := filepath.Join(source.path, "permissions.toml")
		table, err := r.inputs.table(path, "invalid-permissions", source.path)
		if err != nil {
			return err
		}
		rules, err := toLoadoutTable(table["mcp"], path, "mcp")
		if err != nil {
			return loadoutIssue("invalid-permissions", err.Error(), path)
		}
		for _, decision := range slices.Sorted(maps.Keys(rules)) {
			if !slices.Contains([]string{"allow", "deny", "ask"}, decision) {
				return loadoutIssue("invalid-permissions", "Expected MCP permission decision lists", path)
			}
			entries, err := loadoutStrings(rules[decision], path, "mcp."+decision, "invalid-permissions")
			if err != nil {
				return err
			}
			for _, rule := range entries {
				other, tool, _ := strings.Cut(rule, "/")
				if other == "" || tool == "" {
					return loadoutIssue("invalid-permissions", "MCP permissions must use server/tool entries", path)
				}
				for _, server := range servers {
					if (opencode && loadoutPermissionOverlap(other, tool, server, false)) || (pi && loadoutPermissionOverlap(other, tool, server, true)) {
						return loadoutIssue("mcp-permission-overlap", fmt.Sprintf("MCP permission %q overlaps server %q", rule, server), path)
					}
				}
			}
		}
	}
	return nil
}

func (r *loadoutRender) mcp(item MCPArtifact, tables map[string][]string) error {
	path, err := r.safe(filepath.Join(r.source().path, "mcp.toml"))
	if err != nil {
		return err
	}
	var agents []Agent
	for _, h := range r.destination.setup.routes.harnesses {
		if slices.Contains(h.slices, "mcp") {
			agents = append(agents, h.agent)
		}
	}
	r.impact("mcp", path, agents, false)
	var contexts []RenderContext
	for _, agent := range agents {
		if r.destination.setup.routes.custom[loadoutSlice{agent, "mcp"}] {
			return r.fail("unsupported-slice", "MCP also reaches a custom legacy renderer")
		}
		context, err := r.context(agent)
		if err != nil {
			return err
		}
		contexts = append(contexts, context)
	}
	for _, source := range r.destination.setup.sources {
		for _, other := range tables[source.name] {
			same := source.name == r.source().name && other == item.name
			piAlias := slices.Contains(agents, AgentPi) && strings.ReplaceAll(item.name, "-", "_") == strings.ReplaceAll(other, "-", "_")
			if !same && (portablePathKey(other) == portablePathKey(item.name) || piAlias) {
				return loadoutIssue("source-collision", fmt.Sprintf("MCP name %s collides in %s", item.name, source.name), filepath.Join(source.path, "mcp.toml"))
			}
		}
	}
	value, err := r.server(item, contexts)
	if err != nil {
		return err
	}
	content, err := loadoutStructured([]string{item.name}, value, false)
	if err != nil {
		return err
	}
	return r.write(item.id, FamilyMCP, path, content, "")
}

func (r *loadoutRender) server(item MCPArtifact, contexts []RenderContext) (Value, error) {
	if http, ok := item.transport.(HTTPTransport); ok {
		for _, context := range contexts {
			if problem := literal(http.url, context, "url"); problem != nil {
				return nil, r.fail(problem.Code, problem.Message)
			}
		}
		values := []ObjectMember{field("transport", modelText("http")), field("url", modelText(http.url))}
		if http.bearerToken != nil {
			values = append(values, field("auth_env_var", modelText(http.bearerToken.name)))
		}
		return modelObject(values...), nil
	}
	command, ok := item.transport.(Command)
	if !ok || len(command.argv) == 0 {
		return nil, r.fail("mcp-command", "Expected an MCP command")
	}
	if command.cwd != nil {
		return nil, r.fail("mcp-cwd", "Loadout cannot preserve the cwd of MCP server "+item.name)
	}
	var argv []string
	for _, arg := range command.argv {
		value, err := assetArgument(arg, r.assets)
		if err != nil {
			return nil, err
		}
		argv = append(argv, value)
	}
	env := make([]ObjectMember, 0, len(command.env))
	for _, variable := range command.env {
		if _, reference := variable.value.(EnvRef); reference {
			return nil, r.fail("mcp-env-reference", "Loadout cannot preserve stdio environment references for "+item.name)
		}
		text, ok := variable.value.(EnvText)
		if !ok {
			return nil, r.fail("mcp-env-reference", "Expected literal stdio environment variables")
		}
		env = append(env, field(variable.name, modelText(string(text))))
	}
	for _, context := range contexts {
		for _, value := range argv {
			if err := r.mcpLiteral(value, context, "argv"); err != nil {
				return nil, err
			}
		}
		for _, value := range env {
			text, _ := loadoutText(value.Value)
			if err := r.mcpLiteral(text, context, "env"); err != nil {
				return nil, err
			}
		}
	}
	return modelObject(field("transport", modelText("stdio")), field("command", modelText(argv[0])),
		field("args", textArray(argv[1:])), field("env", modelObject(env...))), nil
}

func (r *loadoutRender) mcpLiteral(value string, context RenderContext, fieldName string) error {
	if problem := literal(value, context, fieldName); problem != nil {
		return r.fail(problem.Code, problem.Message)
	}
	if context.audience.agent == AgentPi && strings.HasPrefix(value, "!") {
		return r.fail("mcp-literal", "Pi could interpret a literal MCP value")
	}
	return nil
}

func (r *loadoutRender) modulePath(agent Agent, relative string) (string, error) {
	tail := string(agent) + "/" + relative
	for _, source := range r.destination.setup.sources {
		if slices.Contains(source.uses, "module-config") && slices.Contains(source.overrides["module-config"], tail) {
			return "", loadoutIssue("source-collision", "An override controls "+tail, source.declaredIn)
		}
	}
	return r.fragmentPath("module-config", filepath.Join("module-config", tail))
}

func (r *loadoutRender) native(item NativeArtifact) error {
	agent := item.audience.agent
	directory, prefix := loadoutModule(agent)
	relative, selected := strings.CutPrefix(item.destination, prefix)
	_, file := item.content.(FileContent)
	if prefix == "" || !selected || strings.Contains(relative, "/") ||
		!slices.Contains([]string{".js", ".mjs", ".ts"}, filepath.Ext(relative)) ||
		slices.Contains([]string{"loadout-hooks.js", "loadout-hooks.ts"}, relative) ||
		item.audience.scope != ScopeUser || item.audience.surface != SurfaceCLI ||
		item.audience.platform != "" && item.audience.platform != r.opts.Platform ||
		!file || loadoutFiltered(relative) {
		return r.fail("native-path", "No Loadout source mapping for native artifact "+item.id)
	}
	path, err := r.modulePath(agent, directory+"/"+relative)
	if err != nil {
		return err
	}
	r.impact("module-config", path, []Agent{agent}, false)
	return r.write(item.id, item.family, path, item.content, "")
}

func (r *loadoutRender) hookFragment(agent Agent, hooks map[string]HookArtifact, outputs []RenderedArtifact) (string, error) {
	name := "flyrail-" + loadoutID("hooks", r.bundle.ID(), string(agent))
	path, err := r.fragmentPath("hooks", filepath.Join("hooks", name+".json"))
	if err != nil {
		return "", err
	}
	values := loadoutTable{}
	for _, output := range outputs {
		structured, ok := output.content.(StructuredContent)
		if !ok {
			return "", r.fail("hook-source", "Expected structured native hook registration")
		}
		value := structured.value
		if agent == AgentCursor {
			record, err := toLoadoutTable(value, path, "Cursor hook")
			if err != nil {
				return "", err
			}
			record["type"] = modelText("command")
			value = modelObject(field("hooks", modelArray([]Value{loadoutObject(record)})))
		}
		event := hookEventName(AgentCodex, hooks[output.id].event)
		values[event] = modelArray([]Value{value})
	}
	encoded, err := valueToJSON(loadoutObject(values))
	if err != nil {
		return "", err
	}
	payload, err := NewFileContent(append(jsonx.Encode(encoded), '\n'), ModePrivate)
	if err != nil {
		return "", err
	}
	identifier := loadoutID("hook-fragment", r.bundle.ID(), string(agent))
	if err := r.write(identifier, FamilyHooks, path, payload, ""); err != nil {
		return "", err
	}
	selected := slices.DeleteFunc(r.routes("hooks"), func(group []LoadoutSelector) bool {
		return !slices.ContainsFunc(group, func(route LoadoutSelector) bool { return route.agent == agent })
	})
	if err := r.checkUnselected("hooks", name, selected); err != nil {
		return "", err
	}
	for _, routes := range selected {
		if slices.ContainsFunc(routes, func(route LoadoutSelector) bool { return route.agent != agent }) {
			return "", r.fail("hook-shared-selector", "A shared hook selector cannot preserve distinct harness ABIs. Declare separate arrays.")
		}
		if err := r.entry(routes, name, FamilyHooks, identifier); err != nil {
			return "", err
		}
	}
	return identifier, nil
}

func (r *loadoutRender) hookModule(agent Agent, directory string, outputs []RenderedArtifact) (string, error) {
	if len(outputs) == 0 {
		return "", r.fail("hook-source", "No generated bridge")
	}
	item := outputs[0]
	path, err := r.modulePath(agent, directory+"/"+filepath.Base(item.destination))
	if err != nil {
		return "", err
	}
	r.impact("module-config", path, []Agent{agent}, false)
	identifier := loadoutID("hook-module", r.bundle.ID(), string(agent))
	return identifier, r.write(identifier, FamilyHooks, path, item.content, "")
}

func (r *loadoutRender) hooks() (map[string][]string, error) {
	hooks := map[string]HookArtifact{}
	for _, artifact := range r.bundle.artifacts {
		if hook, ok := artifact.(HookArtifact); ok {
			hooks[hook.id] = hook
		}
	}
	aliases := map[string][]string{}
	if len(hooks) == 0 {
		return aliases, nil
	}
	for _, agent := range r.destination.agents {
		context, err := r.context(agent)
		if err != nil {
			return nil, err
		}
		output, issue := hookArtifactsWithLauncher(r.bundle, context, r.assets, true)
		if issue != nil {
			if issue.unsupported != nil {
				return nil, r.fail(issue.unsupported.Code, issue.unsupported.Message)
			}
			return nil, issue.invalid
		}
		var authored []RenderedArtifact
		for _, artifact := range output.artifacts {
			if _, ok := hooks[artifact.id]; ok {
				authored = append(authored, artifact)
			} else {
				if _, err := r.safe(artifact.destination); err != nil {
					return nil, err
				}
				r.artifacts = append(r.artifacts, artifact)
			}
		}
		var identifier string
		if folder, _ := loadoutModule(agent); folder != "" {
			identifier, err = r.hookModule(agent, folder, authored)
		} else {
			identifier, err = r.hookFragment(agent, hooks, authored)
		}
		if err != nil {
			return nil, err
		}
		for _, item := range authored {
			aliases[item.id] = append(aliases[item.id], identifier)
		}
		for _, edge := range output.dependencies {
			if _, authored := hooks[edge.dependent]; authored {
				if err := r.edge(identifier, edge.required, edge.mode); err != nil {
					return nil, err
				}
			} else if _, authored := hooks[edge.required]; !authored {
				r.edges[edge] = true
			}
		}
		r.notices = append(r.notices, output.notices...)
	}
	return aliases, nil
}

func (r *loadoutRender) artifactsForBundle() error {
	var names []string
	for _, artifact := range r.bundle.artifacts {
		if mcp, ok := artifact.(MCPArtifact); ok {
			if slices.Contains(names, mcp.name) {
				return r.fail("source-collision", "A bundle may declare each MCP server once")
			}
			names = append(names, mcp.name)
		}
	}
	var tables map[string][]string
	if len(names) > 0 {
		var err error
		tables, err = r.mcpTables()
		if err != nil {
			return err
		}
		if err := r.checkMCPPermissions(names); err != nil {
			return err
		}
	}
	for _, artifact := range r.bundle.artifacts {
		var err error
		switch item := artifact.(type) {
		case SkillArtifact:
			err = r.skill(item)
		case InstructionArtifact:
			err = r.instruction(item)
		case MCPArtifact:
			err = r.mcp(item, tables)
		case NativeArtifact:
			err = r.native(item)
		}
		if err != nil {
			return err
		}
	}
	return nil
}

func renderLoadout(bundle *Bundle, destination *LoadoutDestination, options LoadoutOptions, inputs loadoutInputs, prefix string) (*RenderedBundle, []LoadoutImpact, error) {
	r := loadoutRender{bundle: bundle, destination: destination, opts: options, inputs: inputs, edges: map[Dependency]bool{}}
	if err := r.gate(); err != nil {
		return nil, nil, err
	}
	context, err := r.context(destination.agents[0])
	if err != nil {
		return nil, nil, err
	}
	assets, paths, err := supportAssetArtifacts(bundle, context)
	if err != nil {
		return nil, nil, err
	}
	for _, item := range assets {
		if _, err := r.safe(item.destination); err != nil {
			return nil, nil, err
		}
	}
	r.assets = paths
	if err := r.artifactsForBundle(); err != nil {
		return nil, r.impacts, err
	}
	aliases, err := r.hooks()
	if err != nil {
		return nil, r.impacts, err
	}
	selected := func(name string) []string {
		if ids, ok := aliases[name]; ok {
			return ids
		}
		return []string{name}
	}
	for _, edge := range bundle.dependencies {
		for _, dependent := range selected(edge.dependent) {
			for _, required := range selected(edge.required) {
				if dependent != required {
					if err := r.edge(dependent, required, edge.mode); err != nil {
						return nil, r.impacts, err
					}
				}
			}
		}
	}
	if !destination.acceptSharedScope {
		for _, impact := range r.impacts {
			if impact.shared {
				return nil, r.impacts, loadoutIssue("shared-scope", "Approve the shared source and selector scope shown in the preview", impact.path)
			}
		}
	}
	r.artifacts = append(r.artifacts, assets...)
	qualify := func(id string) string { return prefix + loadoutID("loadout", id)[:31] }
	for i, artifact := range r.artifacts {
		r.artifacts[i].id = qualify(artifact.id)
	}
	edges := make([]Dependency, 0, len(r.edges))
	for edge := range r.edges {
		edge.dependent, edge.required = qualify(edge.dependent), qualify(edge.required)
		edges = append(edges, edge)
	}
	r.notices = append(r.notices, loadoutNotice(bundle.ID()))
	result, err := NewRenderedBundle(r.artifacts, edges, r.notices, nil)
	return result, r.impacts, err
}
