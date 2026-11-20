package flyrail

import (
	"crypto/sha256"
	"errors"
	"fmt"
	"maps"
	"path/filepath"
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

const loadoutInputLimit = 8 * 1024 * 1024

type loadoutTable map[string]Value

type loadoutInput struct {
	path, resolved string
	info           *sysx.Metadata
	digest         [sha256.Size]byte
}

func (expected loadoutInput) equal(current loadoutInput) bool {
	if current.resolved != expected.resolved || current.digest != expected.digest {
		return false
	}
	if current.info == nil || expected.info == nil {
		return current.info == nil && expected.info == nil
	}
	return current.info.Dev == expected.info.Dev && current.info.Ino == expected.info.Ino &&
		current.info.Mode == expected.info.Mode && current.info.UID == expected.info.UID && current.info.GID == expected.info.GID
}

func (expected loadoutInput) require() *TargetError {
	current, _, err := captureLoadoutInput(expected.path, expected.resolved)
	if err != nil || !expected.equal(current) {
		return failure(CodeConcurrentChange, "Loadout routing input changed", expected.path)
	}
	return nil
}

func (expected loadoutInput) requireBinding() *TargetError {
	if path, err := resolveLoadoutPath(expected.path); err != nil || path != expected.resolved {
		return failure(CodeConcurrentChange, "Loadout routing input changed", expected.path)
	}
	return nil
}

func resolveLoadoutPath(path string) (string, error) {
	current := path
	var missing []string
	for {
		info, failed := optionalLstat(current)
		if failed != nil {
			return "", failed
		}
		if info != nil {
			resolved, err := filepath.EvalSymlinks(current)
			if err != nil {
				return "", err
			}
			slices.Reverse(missing)
			return filepath.Join(append([]string{resolved}, missing...)...), nil
		}
		parent := filepath.Dir(current)
		if current == parent {
			return "", fmt.Errorf("cannot resolve Loadout path %q", path)
		}
		missing = append(missing, filepath.Base(current))
		current = parent
	}
}

func loadoutContains(root, candidate string) bool {
	child, err := filepath.Rel(root, candidate)
	return err == nil && child != ".." && !strings.HasPrefix(child, ".."+string(filepath.Separator))
}

func captureLoadoutInput(path, boundary string) (loadoutInput, []byte, error) {
	physical, err := resolveLoadoutPath(path)
	if err != nil {
		return loadoutInput{}, nil, err
	}
	if boundary != "" && !loadoutContains(boundary, physical) {
		return loadoutInput{}, nil, loadoutIssue("escaped-path", "Loadout input escapes its declared source", path)
	}
	stamp := loadoutInput{path: path, resolved: physical}
	info, failed := optionalLstat(physical)
	if failed != nil {
		return loadoutInput{}, nil, failed
	}
	if info == nil {
		return stamp, nil, nil
	}
	stamp.info = info
	if info.IsDir() {
		return stamp, nil, nil
	}
	if !info.IsRegular() || info.Size > loadoutInputLimit {
		return loadoutInput{}, nil, fmt.Errorf("expected an ordinary Loadout input within the size limit at %s", path)
	}
	parents, failed := captureAncestors(physical)
	if failed != nil {
		return loadoutInput{}, nil, failed
	}
	if failed := managedMetadata(*info, physical); failed != nil {
		return loadoutInput{}, nil, failed
	}
	data, failed := readFile(physical, *info, loadoutInputLimit, nil)
	if failed != nil {
		return loadoutInput{}, nil, failed
	}
	currentParents, failed := captureAncestors(physical)
	if failed != nil {
		return loadoutInput{}, nil, failed
	}
	after, failed := optionalLstat(physical)
	if failed != nil || !sameOptionalMetadata(info, after) || !ancestorsEqual(parents, currentParents) {
		return loadoutInput{}, nil, fmt.Errorf("loadout input changed while reading %s", path)
	}
	if current, err := resolveLoadoutPath(path); err != nil || physical != current {
		return loadoutInput{}, nil, fmt.Errorf("loadout input path changed while reading %s", path)
	}
	stamp.digest = sha256.Sum256(data)
	if data == nil {
		data = []byte{}
	}
	return stamp, data, nil
}

type loadoutInputs map[string]loadoutInput

func (inputs loadoutInputs) read(path, code, boundary string) (loadoutInput, []byte, error) {
	stamp, data, err := captureLoadoutInput(path, boundary)
	if err != nil {
		var issue *LoadoutDiagnostic
		if errors.As(err, &issue) {
			return loadoutInput{}, nil, issue
		}
		return loadoutInput{}, nil, loadoutIssue(code, err.Error(), path)
	}
	if previous, exists := inputs[path]; exists && !previous.equal(stamp) {
		return loadoutInput{}, nil, loadoutIssue("concurrent-change", "Loadout input changed during preview", path)
	}
	inputs[path] = stamp
	return stamp, data, nil
}

func (inputs loadoutInputs) conditions() []loadoutInput {
	paths := slices.Sorted(maps.Keys(inputs))
	out := make([]loadoutInput, len(paths))
	for i, path := range paths {
		out[i] = inputs[path]
	}
	return out
}

func (inputs loadoutInputs) file(path, code, boundary string) ([]byte, error) {
	stamp, data, err := inputs.read(path, code, boundary)
	if err == nil && stamp.info != nil && data == nil {
		err = loadoutIssue(code, "Expected a regular Loadout input file", path)
	}
	return data, err
}

func (inputs loadoutInputs) table(path, code, boundary string) (loadoutTable, error) {
	data, err := inputs.file(path, code, boundary)
	if err != nil || data == nil {
		return nil, err
	}
	doc, err := parseTOMLDocument(string(data))
	if err != nil {
		return nil, loadoutIssue(code, err.Error(), path)
	}
	native, err := doc.native(nil)
	if err != nil {
		return nil, loadoutIssue(code, err.Error(), path)
	}
	return toLoadoutTable(native, path, "TOML")
}

func (inputs loadoutInputs) directory(path string) (string, error) {
	stamp, _, err := inputs.read(path, "invalid-source", "")
	if err != nil {
		return "", err
	}
	if stamp.info == nil || !stamp.info.IsDir() {
		return "", loadoutIssue("invalid-source", "Expected an existing Loadout source directory", path)
	}
	return stamp.resolved, nil
}

func loadoutAbsolute(path string) bool {
	return filepath.IsAbs(path) && utf8.ValidString(path) && !strings.ContainsRune(path, '\x00')
}

func loadoutSourcePath(raw, home, relative string) (string, bool) {
	switch {
	case raw == "~":
		return home, true
	case strings.HasPrefix(raw, "~/"):
		return home + string(filepath.Separator) + raw[2:], true
	case raw == "" || strings.HasPrefix(raw, "~") || strings.ContainsRune(raw, '\x00'):
		return "", false
	case loadoutAbsolute(raw):
		return raw, true
	case relative != "":
		return relative + string(filepath.Separator) + raw, true
	default:
		return "", false
	}
}

func loadoutProfileFile(root, profile string) (string, error) {
	if profile == "" || profile == "." || profile == ".." || strings.ContainsAny(profile, "/\\\x00") {
		return "", loadoutIssue("invalid-profile", "Profile must name a sibling manifest", root)
	}
	if profile == "default" {
		return filepath.Join(root, "loadout.toml"), nil
	}
	return filepath.Join(root, profile+".toml"), nil
}

func loadoutText(value Value) (string, bool) {
	selected, ok := value.(String)
	return selected.text, ok
}

func loadoutStrings(value Value, path, label, code string) ([]string, error) {
	if array, ok := value.(Array); ok {
		items := make([]string, 0, array.Len())
		for _, entry := range array.items {
			selected, ok := loadoutText(entry)
			if !ok {
				break
			}
			items = append(items, selected)
		}
		if len(items) == array.Len() {
			return items, nil
		}
	}
	return nil, loadoutIssue(code, label+" must be an array of strings", path)
}

func toLoadoutTable(value Value, path, label string) (loadoutTable, error) {
	if object, ok := value.(Object); ok {
		return maps.Collect(object.All()), nil
	}
	if value == nil {
		return loadoutTable{}, nil
	}
	return nil, loadoutIssue("invalid-manifest", label+" must be a table", path)
}

func loadoutObject(table loadoutTable) Value {
	members := make([]ObjectMember, 0, len(table))
	for key, value := range table {
		members = append(members, field(key, value))
	}
	return modelObject(members...)
}

func loadoutKnownAgent(agent Agent) bool {
	return agent != AgentCopilot && slices.Contains(Agents(), agent)
}

type loadoutLayer struct {
	path string
	data loadoutTable
}

func loadoutChain(root, profile string, inputs loadoutInputs) ([]loadoutLayer, error) {
	marker := filepath.Join(root, "loadout.toml")
	base, err := inputs.table(marker, "invalid-manifest", root)
	if err != nil {
		return nil, err
	}
	if base == nil {
		return nil, loadoutIssue("invalid-manifest", "Loadout source has no loadout.toml", marker)
	}
	path, err := loadoutProfileFile(root, profile)
	if err != nil {
		return nil, err
	}
	data := base
	if path != marker {
		data, err = inputs.table(path, "invalid-profile", root)
		if err != nil {
			return nil, err
		}
		if data == nil {
			path, data = marker, base
		}
	}
	var chain []loadoutLayer
	known := map[string]bool{}
	for {
		physical := inputs[path].resolved
		if filepath.Dir(physical) != root {
			return nil, loadoutIssue("escaped-path", "Profile must resolve to a sibling manifest", path)
		}
		if known[physical] {
			return nil, loadoutIssue("profile-cycle", "Loadout profile extends cycle", path)
		}
		known[physical] = true
		chain = append(chain, loadoutLayer{physical, data})
		raw, present := data["extends"]
		if !present {
			return chain, nil
		}
		parent, ok := loadoutText(raw)
		if !ok {
			return nil, loadoutIssue("invalid-profile", "Profile extends must name a sibling manifest", path)
		}
		path, err = loadoutProfileFile(root, parent)
		if err != nil {
			return nil, err
		}
		data, err = inputs.table(path, "invalid-profile", root)
		if err != nil {
			return nil, err
		}
		if data == nil {
			return nil, loadoutIssue("invalid-profile", "Parent profile is missing", path)
		}
	}
}

type loadoutMerge struct {
	data    loadoutTable
	origins map[string]string
}

func (merged loadoutMerge) forget(key string) {
	for path := range merged.origins {
		if path == key || strings.HasPrefix(path, key+"\x00") {
			delete(merged.origins, path)
		}
	}
}

func (merged loadoutMerge) remove(paths []string, origin string) error {
	type parentTable struct {
		table loadoutTable
		key   string
	}
	known := map[string]bool{}
	for _, target := range paths {
		keys, err := headerPath(target)
		path := strings.Join(keys, "\x00")
		invalid := loadoutIssue("invalid-profile", "Invalid or overlapping profile remove path "+target, origin)
		if err != nil || len(keys) == 0 || known[path] {
			return invalid
		}
		for k := range known {
			if strings.HasPrefix(k, path+"\x00") || strings.HasPrefix(path, k+"\x00") {
				return invalid
			}
		}
		known[path] = true
		table := merged.data
		var parents []parentTable
		for _, key := range keys[:len(keys)-1] {
			object, ok := table[key].(Object)
			if !ok {
				return invalid
			}
			parents = append(parents, parentTable{table, key})
			table = maps.Collect(object.All())
		}
		leaf := keys[len(keys)-1]
		if _, ok := table[leaf]; !ok {
			return invalid
		}
		delete(table, leaf)
		for _, parent := range slices.Backward(parents) {
			parent.table[parent.key] = loadoutObject(table)
			table = parent.table
		}
		merged.forget(path)
	}
	return nil
}

func (merged loadoutMerge) merge(layer loadoutLayer) error {
	if raw, ok := layer.data["remove"]; ok {
		if layer.data["extends"] == nil {
			return loadoutIssue("invalid-profile", "Profile remove requires extends", layer.path)
		}
		paths, err := loadoutStrings(raw, layer.path, "remove", "invalid-profile")
		if err != nil {
			return err
		}
		if err = merged.remove(paths, layer.path); err != nil {
			return err
		}
	}

	type entry struct {
		path string
		name string
		v    Value
	}
	unfold := func(target loadoutTable, prefix string, field string) loadoutTable {
		if current, ok := target[field].(Object); ok {
			return maps.Collect(current.All())
		}
		delete(merged.origins, prefix)
		return loadoutTable{}
	}
	assign := func(target loadoutTable, item entry) {
		if _, ok := target[item.name].(Object); ok {
			merged.forget(item.path)
		}
		merged.origins[item.path] = layer.path
		target[item.name] = item.v
	}

	for key, value := range layer.data {
		if key == "extends" || key == "remove" {
			continue
		}
		named := slices.Contains([]string{"instructions", "permissions", "skills"}, key)
		fields, ok := value.(Object)
		if !ok {
			assign(merged.data, entry{key, key, value})
			continue
		}
		group := unfold(merged.data, key, key)
		for name, field := range fields.All() {
			prefix := key + "\x00" + name
			target, ok := field.(Object)
			if named && ok {
				fields := unfold(group, prefix, name)
				for child, value := range target.All() {
					assign(fields, entry{prefix + "\x00" + child, child, value})
				}
				group[name] = loadoutObject(fields)
			} else {
				assign(group, entry{prefix, name, field})
			}
		}
		merged.data[key] = loadoutObject(group)
	}
	return nil
}

func loadoutCategories() []string {
	return []string{"agents", "defaults", "hooks", "instructions", "kits", "mcp", "module-config", "permissions", "plugins", "settings", "skills"}
}

func loadoutAutomatic() []string {
	return []string{"agents", "mcp", "mcp-permissions", "module-config", "permissions", "skills"}
}

func loadoutSlices(agent Agent) []string {
	categories := slices.DeleteFunc(loadoutAutomatic(), func(category string) bool {
		return category == "mcp-permissions" && agent != AgentClaude || category == "module-config" && agent == AgentCodex
	})
	categories = append(categories, "instructions", "hooks")
	if slices.Contains([]Agent{AgentClaude, AgentCodex, AgentDroid, AgentPi}, agent) {
		categories = append(categories, "plugins")
	}
	if agent == AgentCodex {
		categories = append(categories, "defaults")
	}
	return categories
}

func loadoutSources(value Value, origin, root, home string, inputs loadoutInputs) ([]LoadoutSource, error) {
	array, ok := value.(Array)
	if !ok || array.Len() == 0 {
		return nil, loadoutIssue("invalid-manifest", "Declare at least one [[source]]", origin)
	}
	known := map[string]bool{}
	result := make([]LoadoutSource, 0, array.Len())
	for _, value := range array.items {
		source, err := toLoadoutTable(value, origin, "source")
		if err != nil {
			return nil, err
		}
		name, _ := loadoutText(source["name"])
		if name == "" || strings.Contains(name, "/") || validateRelativePath(name, "source name") != nil || known[portablePathKey(name)] {
			return nil, loadoutIssue("invalid-source", "Expected a unique single-component source name", origin)
		}
		known[portablePathKey(name)] = true
		for key := range source {
			if !slices.Contains([]string{"name", "path", "use", "overrides"}, key) {
				return nil, loadoutIssue("unsupported-manifest", "Unknown source field "+key, origin)
			}
		}
		raw, _ := loadoutText(source["path"])
		resolved, ok := loadoutSourcePath(raw, home, root)
		if !ok {
			return nil, loadoutIssue("invalid-source", "Expected a valid source path", origin)
		}
		path, err := inputs.directory(resolved)
		if err != nil {
			return nil, err
		}
		uses := loadoutCategories()
		if source["use"] != nil {
			uses, err = loadoutStrings(source["use"], origin, "source use", "invalid-source")
			if err != nil {
				return nil, err
			}
		}
		if len(uses) == 0 || slices.ContainsFunc(uses, func(use string) bool { return !slices.Contains(loadoutCategories(), use) }) {
			return nil, loadoutIssue("invalid-source", "Source use contains an unknown or empty category", origin)
		}
		for _, other := range result {
			if other.path == path && slices.ContainsFunc(uses, func(item string) bool { return slices.Contains(other.uses, item) }) {
				return nil, loadoutIssue("invalid-source", "Source aliases an earlier declared source", origin)
			}
		}
		if slices.Contains(uses, "kits") {
			templates, _, err := inputs.read(filepath.Join(path, "templates"), "invalid-source", path)
			if err != nil {
				return nil, err
			}
			if templates.info != nil {
				return nil, loadoutIssue("unsupported-manifest", "Rename templates/ to kits/ in this source", path)
			}
		}
		overrides, err := loadoutOverrides(source["overrides"], uses, origin)
		if err != nil {
			return nil, err
		}
		slices.Sort(uses)
		result = append(result, LoadoutSource{name, path, origin, slices.Compact(uses), overrides})
	}
	return result, nil
}

func loadoutOverrides(value Value, uses []string, path string) (map[string][]string, error) {
	raw, err := toLoadoutTable(value, path, "overrides")
	if err != nil {
		return nil, err
	}
	out := map[string][]string{}
	for category, value := range raw {
		if !slices.Contains(uses, category) || !slices.Contains([]string{"skills", "agents", "module-config"}, category) {
			return nil, loadoutIssue("invalid-source", "Unsupported source override category", path)
		}
		items, err := loadoutStrings(value, path, category, "invalid-source")
		if err != nil {
			return nil, err
		}
		known := map[string]bool{}
		for _, item := range items {
			head, _, scoped := strings.Cut(item, "/")
			if validateRelativePath(item, "source override") != nil || known[item] || slices.Contains(strings.Split(item, "/"), ".git") ||
				(category == "module-config" && (!scoped || !loadoutKnownAgent(Agent(head)))) || (category != "module-config" && scoped) {
				return nil, loadoutIssue("invalid-source", "Invalid source override path "+item, path)
			}
			known[item] = true
		}
		out[category] = items
	}
	return out, nil
}

type loadoutRoutes struct {
	harnesses []LoadoutHarness
	selectors []LoadoutSelector
	policies  map[string][]string
	custom    map[loadoutSlice]bool
}

type loadoutSlice struct {
	agent    Agent
	category string
}

func legacyLoadoutSlice(renderer string) (loadoutSlice, bool) {
	switch renderer {
	case "codex-config":
		return loadoutSlice{AgentCodex, "mcp"}, true
	case "codex-settings":
		return loadoutSlice{AgentCodex, "defaults"}, true
	case "claude-mcp-permissions":
		return loadoutSlice{AgentClaude, "mcp-permissions"}, true
	}
	for _, spec := range []struct{ suffix, category string }{
		{"-servers", "mcp"}, {"-hooks", "hooks"}, {"-plugins", "plugins"}, {"", "permissions"},
	} {
		if name, ok := strings.CutSuffix(renderer, spec.suffix); ok && loadoutKnownAgent(Agent(name)) {
			return loadoutSlice{Agent(name), spec.category}, true
		}
	}
	return loadoutSlice{}, false
}

func loadoutSubstitutions(value Value, file string) (map[string]string, error) {
	table, err := toLoadoutTable(value, file, "substitute")
	if err != nil {
		return nil, err
	}
	out := map[string]string{}
	for name, raw := range table {
		replacement, ok := loadoutText(raw)
		if !ok {
			return nil, loadoutIssue("invalid-manifest", "Substitute targets must be strings", file)
		}
		out[name] = replacement
	}
	return out, nil
}

func (routes *loadoutRoutes) add(agent Agent, categories ...string) {
	for i, current := range routes.harnesses {
		if current.agent == agent {
			values := append(slices.Clone(current.slices), categories...)
			slices.Sort(values)
			routes.harnesses[i].slices = slices.Compact(values)
			return
		}
	}
	slices.Sort(categories)
	routes.harnesses = append(routes.harnesses, LoadoutHarness{agent, categories})
}

func (routes *loadoutRoutes) legacy(merged loadoutMerge, profile, file string) (bool, error) {
	profileNamed := false
	outputs := map[string]bool{}
	for _, category := range []string{"instructions", "permissions"} {
		targets, err := toLoadoutTable(merged.data[category], file, category)
		if err != nil {
			return false, err
		}
		for _, name := range slices.Sorted(maps.Keys(targets)) {
			block, err := toLoadoutTable(targets[name], file, name)
			if err != nil {
				return false, err
			}
			count, err := legacyLoadoutPaths(block, file, outputs)
			if err != nil {
				return false, err
			}
			selected, hasProfile := loadoutText(block["profile"])
			if block["profile"] != nil && !hasProfile {
				return false, loadoutIssue("invalid-manifest", "Legacy profile must be a string", file)
			}
			profileNamed = profileNamed || selected == profile
			if category == "instructions" {
				agent := Agent(name)
				if !loadoutKnownAgent(agent) {
					return false, loadoutIssue("unsupported-manifest", "Unknown legacy instruction harness "+name, file)
				}
				order, err := loadoutStrings(block["order"], file, "order", "invalid-manifest")
				if err != nil {
					return false, err
				}
				swaps, err := loadoutSubstitutions(block["substitute"], file)
				if err != nil {
					return false, err
				}
				if !hasProfile || selected == profile {
					key := []string{category, name, "order"}
					origin := merged.origins[strings.Join(key, "\x00")]
					routes.selectors = append(routes.selectors, LoadoutSelector{agent, category, origin, key, order, swaps, true, count, !hasProfile})
					routes.add(agent, category)
				}
				continue
			}
			value, _ := loadoutText(block["render"])
			slice, supported := legacyLoadoutSlice(value)
			if !supported || block["rules"] != nil {
				return false, loadoutIssue("unsupported-manifest", "Unsupported legacy permission renderer "+value, file)
			}
			if !hasProfile || selected == profile {
				routes.add(slice.agent, slice.category)
				routes.custom[slice] = true
			}
		}
	}
	return profileNamed, nil
}

func legacyLoadoutPaths(block loadoutTable, file string, claimed map[string]bool) (int, error) {
	count := 0
	if block["destinations"] != nil {
		entries, err := loadoutStrings(block["destinations"], file, "destinations", "invalid-manifest")
		if err != nil {
			return 0, err
		}
		if slices.ContainsFunc(entries, func(entry string) bool { return entry == "" || strings.ContainsRune(entry, '\x00') }) {
			return 0, loadoutIssue("invalid-manifest", "Invalid legacy destination", file)
		}
		count = len(entries)
	}
	if block["output"] != nil {
		value, ok := loadoutText(block["output"])
		if !ok || validateRelativePath(value, "legacy output") != nil || claimed[value] {
			return 0, loadoutIssue("invalid-manifest", "Legacy output must be a unique relative path", file)
		}
		claimed[value] = true
		count++
	}
	if count == 0 {
		return 0, loadoutIssue("invalid-manifest", "Legacy target needs destinations or output", file)
	}
	return count, nil
}

func (routes *loadoutRoutes) modern(merged loadoutMerge, file string) error {
	common, err := toLoadoutTable(merged.data["all"], file, "all")
	if err != nil {
		return err
	}
	extras := []string{"preserve", "settings", "substitute"}
	for _, name := range slices.Sorted(maps.Keys(merged.data)) {
		agent := Agent(name)
		if !loadoutKnownAgent(agent) {
			if !slices.Contains([]string{"source", "all", "instructions", "permissions", "skills"}, name) {
				return loadoutIssue("unsupported-manifest", "Unknown top-level manifest key "+name, file)
			}
			continue
		}
		declared, err := toLoadoutTable(merged.data[name], file, name)
		if err != nil {
			return err
		}
		offered := loadoutSlices(agent)
		active, err := routes.harness(merged, agent, common, declared, offered, extras, file)
		if err != nil {
			return err
		}
		if len(active) > 0 {
			routes.add(agent, active...)
		}
	}
	return nil
}

func (routes *loadoutRoutes) harness(merged loadoutMerge, agent Agent, common, declared loadoutTable, offered, extras []string, file string) ([]string, error) {
	name := string(agent)
	for key := range declared {
		if (!slices.Contains(offered, key) && !slices.Contains(extras, key)) ||
			agent == AgentCodex && (key == "settings" || key == "preserve") {
			return nil, loadoutIssue("invalid-manifest", fmt.Sprintf("Unsupported %s field %s", agent, key), file)
		}
	}
	block := maps.Clone(common)
	maps.Copy(block, declared)
	var active []string
	for _, key := range slices.Sorted(maps.Keys(block)) {
		value := block[key]
		category := slices.Contains(offered, key)
		flag, boolean := value.(Bool)
		array, list := value.(Array)
		emptyRules := list && array.Len() == 0 && slices.Contains([]string{"mcp", "mcp-permissions", "permissions"}, key)
		if category && slices.Contains(loadoutAutomatic(), key) && !boolean && !emptyRules {
			return nil, loadoutIssue("invalid-manifest", fmt.Sprintf("%s.%s must be enabled or disabled", agent, key), file)
		}
		if key == "preserve" || slices.Contains([]string{"settings", "plugins", "defaults"}, key) &&
			(category || key == "settings") && value != Bool(false) {
			if _, text := loadoutText(value); key == "preserve" || !text {
				if _, err := loadoutStrings(value, file, key, "invalid-manifest"); err != nil {
					return nil, err
				}
			}
		}
		if !category || boolean && !bool(flag) {
			continue
		}
		active = append(active, key)
		if key != "hooks" && key != "instructions" {
			continue
		}
		stringValue, text := loadoutText(value)
		inline := key == "hooks" && text
		values := []string{stringValue}
		var err error
		if !inline {
			values, err = loadoutStrings(value, file, key, "invalid-manifest")
			if err != nil {
				return nil, err
			}
		}
		where := []string{"all", key}
		if declared[key] != nil {
			where[0] = name
		}
		var swaps map[string]string
		if key == "instructions" {
			swaps, err = loadoutSubstitutions(block["substitute"], file)
			if err != nil {
				return nil, err
			}
		}
		routes.selectors = append(routes.selectors, LoadoutSelector{agent, key, merged.origins[strings.Join(where, "\x00")], where, values, swaps, !inline, 1, false})
	}
	for _, key := range loadoutAutomatic() {
		if slices.Contains(offered, key) && block[key] == nil {
			active = append(active, key)
		}
	}
	if _, err := loadoutSubstitutions(block["substitute"], file); err != nil {
		return nil, err
	}
	if declared["settings"] != nil && !slices.ContainsFunc(loadoutSettingsReaders(agent), func(category string) bool {
		return slices.Contains(active, category)
	}) {
		return nil, loadoutIssue("invalid-manifest", name+".settings needs an enabled slice that reads it", file)
	}
	return active, nil
}

func loadoutSettingsReaders(agent Agent) []string {
	switch agent {
	case AgentClaude:
		return []string{"permissions", "hooks", "plugins"}
	case AgentCursor:
		return []string{"permissions"}
	case AgentDroid:
		return []string{"permissions", "plugins"}
	case AgentOpencode:
		return []string{"permissions", "mcp"}
	case AgentPi:
		return []string{"plugins"}
	default:
		return nil
	}
}

func loadoutSkillPolicies(routes loadoutRoutes, merged loadoutMerge, file string) (map[string][]string, error) {
	table, err := toLoadoutTable(merged.data["skills"], file, "skills")
	if err != nil {
		return nil, err
	}
	result := map[string][]string{}
	for name, value := range table {
		block, err := toLoadoutTable(value, file, "skills."+name)
		if err != nil {
			return nil, err
		}
		agents, err := loadoutStrings(block["agents"], file, "skill agents", "invalid-manifest")
		if err != nil {
			return nil, err
		}
		invalid := func() error { return loadoutIssue("invalid-manifest", "Invalid skill policy for "+name, file) }
		if name == "" || name == "." || name == ".." || strings.ContainsAny(name, "/\\\x00") {
			return nil, invalid()
		}
		seen := map[string]bool{}
		for _, name := range agents {
			if seen[name] || !slices.ContainsFunc(routes.harnesses, func(item LoadoutHarness) bool {
				return string(item.agent) == name && slices.Contains(item.slices, "skills")
			}) {
				return nil, invalid()
			}
			seen[name] = true
		}
		files := map[string]bool{}
		for key, value := range block {
			_, boolean := value.(Bool)
			if key != "agents" && (key != "render" || !boolean) {
				return nil, invalid()
			}
			files[merged.origins[strings.Join([]string{"skills", name, key}, "\x00")]] = true
		}
		result[name] = slices.Sorted(maps.Keys(files))
	}
	return result, nil
}

func loadoutManifest(root, profile, home string, inputs loadoutInputs) (loadoutRoutes, []LoadoutSource, []string, error) {
	chain, err := loadoutChain(root, profile, inputs)
	if err != nil {
		return loadoutRoutes{}, nil, nil, err
	}
	active := chain[0].path
	out := loadoutRoutes{custom: map[loadoutSlice]bool{}}
	merged := loadoutMerge{loadoutTable{}, map[string]string{}}
	for _, layer := range slices.Backward(chain) {
		if err := merged.merge(layer); err != nil {
			return loadoutRoutes{}, nil, nil, err
		}
	}
	named, err := out.legacy(merged, profile, active)
	if err != nil {
		return loadoutRoutes{}, nil, nil, err
	}
	requested, err := loadoutProfileFile(root, profile)
	if err != nil {
		return loadoutRoutes{}, nil, nil, err
	}
	if profile != "default" && (profile == "loadout" || inputs[requested].info == nil || chain[0].data["extends"] == nil) && !named {
		return loadoutRoutes{}, nil, nil, loadoutIssue("invalid-profile", "Unknown Loadout profile "+profile, active)
	}
	if err := out.modern(merged, active); err != nil {
		return loadoutRoutes{}, nil, nil, err
	}
	if len(out.harnesses) == 0 {
		return loadoutRoutes{}, nil, nil, loadoutIssue("invalid-manifest", "No active Loadout targets in this profile", active)
	}
	slices.SortFunc(out.harnesses, func(a, b LoadoutHarness) int { return strings.Compare(string(a.agent), string(b.agent)) })
	out.policies, err = loadoutSkillPolicies(out, merged, active)
	if err != nil {
		return loadoutRoutes{}, nil, nil, err
	}
	origin := merged.origins["source"]
	if origin == "" {
		origin = active
	}
	sources, err := loadoutSources(merged.data["source"], origin, root, home, inputs)
	if err != nil {
		return loadoutRoutes{}, nil, nil, err
	}
	paths := make([]string, len(chain))
	for i, layer := range chain {
		paths[i] = layer.path
	}
	return out, sources, paths, nil
}
