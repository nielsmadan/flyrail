package flyrail

import (
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
)

func parseManifest(data []byte) (jsonx.Value, error) {
	if !utf8.Valid(data) {
		return jsonx.Value{}, newValueError("manifest is not valid UTF-8")
	}
	raw, err := jsonx.Parse(data)
	if err != nil {
		return jsonx.Value{}, valueErrorFromJSON(err)
	}
	return raw, nil
}

func readManifest(data []byte) (BundleIdentity, []SkillSpec, error) {
	raw, err := parseManifest(data)
	if err != nil {
		return BundleIdentity{}, nil, err
	}
	manifest, err := manifestObject(raw, []string{"schema_version", "id", "version", "skills"}, nil)
	if err != nil {
		return BundleIdentity{}, nil, err
	}
	if version, ok := manifest.get("schema_version").AsInt(); !ok || version != 1 {
		return BundleIdentity{}, nil, newValueError("schema_version must be the integer 1")
	}
	id, err := manifestString(manifest.get("id"), "bundle id")
	if err != nil {
		return BundleIdentity{}, nil, err
	}
	version, err := manifestString(manifest.get("version"), "version")
	if err != nil {
		return BundleIdentity{}, nil, err
	}
	identity, err := NewBundleIdentity(id, version)
	if err != nil {
		return BundleIdentity{}, nil, err
	}
	records, ok := manifest.get("skills").Items()
	if !ok || len(records) == 0 {
		return BundleIdentity{}, nil, newValueError("skills must be a nonempty JSON array")
	}
	specs := make([]SkillSpec, 0, len(records))
	names := make(map[string]struct{}, len(records))
	var paths []string
	for _, record := range records {
		spec, err := readSkillRecord(record)
		if err != nil {
			return BundleIdentity{}, nil, err
		}
		key := portablePathKey(spec.Path())
		if _, duplicate := names[spec.Name()]; duplicate {
			return BundleIdentity{}, nil, newValueError("duplicate skill name: " + spec.Name())
		}
		if slices.ContainsFunc(paths, func(other string) bool { return overlapping(key, other) }) {
			return BundleIdentity{}, nil, newValueError("skill source paths must not overlap")
		}
		names[spec.Name()] = struct{}{}
		paths = append(paths, key)
		specs = append(specs, spec)
	}
	return identity, specs, nil
}

func readSkillRecord(record jsonx.Value) (SkillSpec, error) {
	fields, err := manifestObject(record, []string{"name", "path"}, []string{"executables"})
	if err != nil {
		return SkillSpec{}, err
	}
	name, err := manifestString(fields.get("name"), "skill name")
	if err != nil {
		return SkillSpec{}, err
	}
	path, err := manifestString(fields.get("path"), "skill path")
	if err != nil {
		return SkillSpec{}, err
	}
	executables, err := manifestExecutables(fields)
	if err != nil {
		return SkillSpec{}, err
	}
	return NewSkillSpec(name, path, executables)
}

func manifestExecutables(fields manifestFields) ([]string, error) {
	value, ok := fields.lookup("executables")
	if !ok {
		return nil, nil
	}
	items, ok := value.Items()
	if !ok {
		return nil, newValueError("executables must be a JSON array")
	}
	executables := make([]string, 0, len(items))
	for _, item := range items {
		executable, err := manifestString(item, "executable path")
		if err != nil {
			return nil, err
		}
		executables = append(executables, executable)
	}
	return executables, nil
}

func overlapping(key, other string) bool {
	return key == other || strings.HasPrefix(key, other+"/") || strings.HasPrefix(other, key+"/")
}

type manifestFields struct {
	members []jsonx.Member
}

func manifestObject(value jsonx.Value, required, optional []string) (manifestFields, error) {
	members, ok := value.Members()
	if !ok {
		return manifestFields{}, newValueError("manifest and skill records must be JSON objects")
	}
	present := make(map[string]struct{}, len(members))
	unknown := false
	for _, member := range members {
		present[member.Key] = struct{}{}
		unknown = unknown || !slices.Contains(required, member.Key) && !slices.Contains(optional, member.Key)
	}
	missing := slices.ContainsFunc(required, func(field string) bool {
		_, ok := present[field]
		return !ok
	})
	if unknown || missing {
		return manifestFields{}, newValueError("expected fields " + fieldList(required) + "; optional fields " + fieldList(optional))
	}
	return manifestFields{members: members}, nil
}

func fieldList(fields []string) string {
	sorted := slices.Sorted(slices.Values(fields))
	quoted := make([]string, len(sorted))
	for index, field := range sorted {
		quoted[index] = "'" + field + "'"
	}
	return "[" + strings.Join(quoted, ", ") + "]"
}

func (f manifestFields) lookup(key string) (jsonx.Value, bool) {
	for _, member := range f.members {
		if member.Key == key {
			return member.Value, true
		}
	}
	return jsonx.Value{}, false
}

func (f manifestFields) get(key string) jsonx.Value {
	value, _ := f.lookup(key)
	return value
}

func manifestString(value jsonx.Value, label string) (string, error) {
	if text, ok := value.AsString(); ok {
		return text, nil
	}
	return "", newValueError(label + " must be a string")
}
