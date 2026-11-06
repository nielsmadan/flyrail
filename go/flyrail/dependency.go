package flyrail

import "slices"

// DependencyMode is how a dependent artifact tracks what it requires.
type DependencyMode string

// The dependency modes.
const (
	DependencyRevision        DependencyMode = "revision"
	DependencyStableReference DependencyMode = "stable-reference"
)

// ParseDependencyMode returns the dependency mode spelled s.
func ParseDependencyMode(s string) (DependencyMode, error) {
	return parseEnum(s, "DependencyMode", DependencyRevision, DependencyStableReference)
}

// Dependency states that the dependent artifact requires another artifact or asset.
type Dependency struct {
	dependent string
	required  string
	mode      DependencyMode
}

// NewDependency returns a dependency edge; mode "" means DependencyRevision.
func NewDependency(dependent, required string, mode DependencyMode) (Dependency, error) {
	if mode == "" {
		mode = DependencyRevision
	}
	if err := checkEnum(mode, "DependencyMode", DependencyRevision, DependencyStableReference); err != nil {
		return Dependency{}, err
	}
	if err := validateIdentifier(dependent, "dependent artifact"); err != nil {
		return Dependency{}, err
	}
	if err := validateIdentifier(required, "required artifact"); err != nil {
		return Dependency{}, err
	}
	if dependent == required {
		return Dependency{}, newValueError("an artifact cannot depend on itself")
	}
	return Dependency{dependent: dependent, required: required, mode: mode}, nil
}

// Dependent returns the id of the dependent artifact.
func (d Dependency) Dependent() string { return d.dependent }

// Required returns the id of the required artifact or asset.
func (d Dependency) Required() string { return d.required }

// Mode returns the dependency mode.
func (d Dependency) Mode() DependencyMode { return d.mode }

func (d Dependency) check() error {
	return validateIdentifier(d.dependent, "dependent artifact")
}

func (d Dependency) modelValue() Value {
	return typedRecord("Dependency",
		field("dependent", modelText(d.dependent)),
		field("mode", modelText(string(d.mode))),
		field("required", modelText(d.required)),
	)
}

// DependencyOrder returns names ordered so every required name precedes its dependents, ties broken by byte order.
func DependencyOrder(names []string, deps []Dependency) ([]string, error) {
	pending := make(map[string]struct{}, len(names))
	for _, name := range names {
		pending[name] = struct{}{}
	}
	type pair struct{ dependent, required string }
	pairs := make(map[pair]struct{}, len(deps))
	for _, edge := range deps {
		if err := edge.check(); err != nil {
			return nil, err
		}
		key := pair{edge.dependent, edge.required}
		if _, duplicate := pairs[key]; duplicate {
			return nil, newValueError("duplicate dependency")
		}
		pairs[key] = struct{}{}
	}
	for _, edge := range deps {
		_, dependent := pending[edge.dependent]
		_, required := pending[edge.required]
		if !dependent || !required {
			return nil, newValueError("dependency references an unknown artifact or asset")
		}
	}
	result := make([]string, 0, len(pending))
	for len(pending) > 0 {
		var ready []string
		for name := range pending {
			blocked := slices.ContainsFunc(deps, func(edge Dependency) bool {
				_, waiting := pending[edge.required]
				return edge.dependent == name && waiting
			})
			if !blocked {
				ready = append(ready, name)
			}
		}
		if len(ready) == 0 {
			return nil, newValueError("cyclic artifact dependencies")
		}
		slices.Sort(ready)
		for _, name := range ready {
			delete(pending, name)
		}
		result = append(result, ready...)
	}
	return result, nil
}
