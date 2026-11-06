package flyrail

// ChangeAction is what applying a preview would do to one resource.
type ChangeAction string

// The change actions, spelled as on the wire.
const (
	ActionCreate    ChangeAction = "create"
	ActionUpdate    ChangeAction = "update"
	ActionDelete    ChangeAction = "delete"
	ActionUnchanged ChangeAction = "unchanged"
	ActionConflict  ChangeAction = "conflict"
)

// ParseChangeAction returns the ChangeAction spelled s.
func ParseChangeAction(s string) (ChangeAction, error) {
	return parseEnum(s, "ChangeAction", ActionCreate, ActionUpdate, ActionDelete, ActionUnchanged, ActionConflict)
}

// PlannedChange is the planned change to one resource and the artifacts routed to it.
type PlannedChange struct {
	ArtifactIDs   []string
	Families      []Family
	Kind          ResourceKind
	Destination   string
	Action        ChangeAction
	Err           *TargetError
	RecoveryPaths []string
}

// Failure returns Err as an error, or a nil error when there is no failure.
func (c PlannedChange) Failure() error {
	return failureOf(c.Err)
}

// PlanSummary describes a preview for display; Agent, Scope and Surface are "" when the routing context lacks them.
type PlanSummary struct {
	Agent      Agent
	Scope      TargetScope
	Surface    Surface
	Applicable bool
	Changes    []PlannedChange
	Notices    []Notice
	Err        *TargetError
}

// Failure returns Err as an error, or a nil error when there is no failure.
func (s PlanSummary) Failure() error {
	return failureOf(s.Err)
}

// Summarize describes what applying p would change, resource by resource.
func Summarize(p *LifecyclePreview) (PlanSummary, error) {
	if p == nil || p.rendered == nil || !p.target.valid() {
		return PlanSummary{}, newTypeError("summarize requires a LifecyclePreview")
	}
	routing := p.target.RoutingContext()
	agent, err := parseRouting(routing["agent"], ParseAgent)
	if err != nil {
		return PlanSummary{}, err
	}
	scope, err := parseRouting(routing["scope"], ParseTargetScope)
	if err != nil {
		return PlanSummary{}, err
	}
	surface, err := parseRouting(routing["surface"], ParseSurface)
	if err != nil {
		return PlanSummary{}, err
	}
	byDestination := map[string][]RenderedArtifact{}
	for _, artifact := range p.rendered.artifacts {
		key := pathKey(artifact.destination)
		byDestination[key] = append(byDestination[key], artifact)
	}
	changes := make([]PlannedChange, 0, len(p.resources))
	for _, plan := range p.resources {
		matched := byDestination[pathKey(plan.resource.Destination)]
		change := PlannedChange{
			ArtifactIDs:   make([]string, 0, len(matched)),
			Families:      make([]Family, 0, len(matched)),
			Kind:          plan.resource.Kind,
			Destination:   plan.resource.Destination,
			Action:        changeAction(plan),
			Err:           plan.Err(),
			RecoveryPaths: plan.RecoveryPaths(),
		}
		for _, artifact := range matched {
			change.ArtifactIDs = append(change.ArtifactIDs, artifact.id)
			change.Families = append(change.Families, artifact.family)
		}
		changes = append(changes, change)
	}
	return PlanSummary{
		Agent:      agent,
		Scope:      scope,
		Surface:    surface,
		Applicable: p.Applicable(),
		Changes:    changes,
		Notices:    p.rendered.Notices(),
		Err:        p.Err(),
	}, nil
}

func changeAction(plan ResourcePlan) ChangeAction {
	switch {
	case plan.err != nil || len(plan.recoveryPaths) > 0:
		return ActionConflict
	case plan.previous != nil && sameRecord(*plan.previous, plan.receipt) && plan.before.equal(plan.after):
		return ActionUnchanged
	case len(plan.after.nodes) == 0:
		return ActionDelete
	case len(plan.before.nodes) == 0:
		return ActionCreate
	}
	return ActionUpdate
}

func parseRouting[T ~string](value string, parse func(string) (T, error)) (T, error) {
	if value == "" {
		return "", nil
	}
	return parse(value)
}
