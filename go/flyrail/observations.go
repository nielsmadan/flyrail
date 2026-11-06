package flyrail

import "slices"

// ObservationState is the installation state an inspection found.
type ObservationState string

// The observation states, spelled as on the wire.
const (
	StateAbsent         ObservationState = "absent"
	StateInstalled      ObservationState = "installed"
	StateRecoveryNeeded ObservationState = "recovery_needed"
	StateUnknown        ObservationState = "unknown"
)

// ParseObservationState returns the ObservationState spelled s.
func ParseObservationState(s string) (ObservationState, error) {
	switch state := ObservationState(s); state {
	case StateAbsent, StateInstalled, StateRecoveryNeeded, StateUnknown:
		return state, nil
	}
	return "", newValueError("'" + s + "' is not a valid ObservationState")
}

// ModificationKind classifies how an installed path differs from its recorded inventory.
type ModificationKind string

// The modification kinds, spelled as on the wire.
const (
	ModificationMissing           ModificationKind = "missing"
	ModificationAdded             ModificationKind = "added"
	ModificationTypeChanged       ModificationKind = "type_changed"
	ModificationContentChanged    ModificationKind = "content_changed"
	ModificationExecutableChanged ModificationKind = "executable_changed"
)

// ParseModificationKind returns the ModificationKind spelled s.
func ParseModificationKind(s string) (ModificationKind, error) {
	switch kind := ModificationKind(s); kind {
	case ModificationMissing, ModificationAdded, ModificationTypeChanged, ModificationContentChanged, ModificationExecutableChanged:
		return kind, nil
	}
	return "", newValueError("'" + s + "' is not a valid ModificationKind")
}

// OperationStatus is the outcome of a lifecycle operation.
type OperationStatus string

// The operation outcomes, spelled as on the wire.
const (
	StatusApplied    OperationStatus = "applied"
	StatusUnchanged  OperationStatus = "unchanged"
	StatusFailed     OperationStatus = "failed"
	StatusPartial    OperationStatus = "partial"
	StatusIncomplete OperationStatus = "incomplete"
)

// InventoryEntry is one recorded path of an installed tree; a directory has Dir set, Size 0 and SHA256 "".
type InventoryEntry struct {
	Path       string
	Dir        bool
	Size       int64
	SHA256     string
	Executable bool
}

// Installation is a recorded installation of one bundle version.
type Installation struct {
	BundleID      string
	Version       string
	ContentDigest string
	TransactionID string
	Entries       []InventoryEntry
}

// Modification is a path whose current state differs from what a bundle installed.
type Modification struct {
	BundleID string
	Path     string
	Kind     ModificationKind
}

// Conflict is a path the operation cannot take; Owner is "" when no bundle owns it.
type Conflict struct {
	Path  string
	Owner string
}

// Observation is what an inspection found at one target; nil pointers mean not determined.
type Observation struct {
	State                  ObservationState
	Installed              *Installation
	Installations          []Installation
	VersionMatches         *bool
	RecordedContentMatches *bool
	ContentMatches         *bool
	Modifications          []Modification
	Conflicts              []Conflict
	Err                    *TargetError
	RecoveryPaths          []string
}

// IsCurrent reports whether the observed installation is complete, matches the bundle and has no conflicts or own modifications.
func (o Observation) IsCurrent() bool {
	if o.State != StateInstalled || o.Err != nil || o.Installed == nil || len(o.Conflicts) > 0 ||
		!isTrue(o.VersionMatches) || !isTrue(o.RecordedContentMatches) || !isTrue(o.ContentMatches) {
		return false
	}
	return !slices.ContainsFunc(o.Modifications, func(change Modification) bool {
		return change.BundleID == o.Installed.BundleID
	})
}

// Failure returns Err as an error, or a nil error when there is no failure.
func (o Observation) Failure() error {
	return failureOf(o.Err)
}

func isTrue(value *bool) bool {
	return value != nil && *value
}

// TargetInspection is the observation at one requested target; AliasOf is meaningful only when IsAlias is set.
type TargetInspection struct {
	Target      Target
	Root        string
	IsAlias     bool
	AliasOf     int
	Observation Observation
}

// TargetResult is the outcome at one requested target; AliasOf is meaningful only when IsAlias is set.
type TargetResult struct {
	Target        Target
	Root          string
	IsAlias       bool
	AliasOf       int
	Status        OperationStatus
	Observation   Observation
	Err           *TargetError
	RecoveryPaths []string
}

// Succeeded reports whether the target's status is applied or unchanged.
func (r TargetResult) Succeeded() bool {
	return succeeded(r.Status)
}

// Failure returns Err as an error, or a nil error when there is no failure.
func (r TargetResult) Failure() error {
	return failureOf(r.Err)
}

func succeeded(status OperationStatus) bool {
	return status == StatusApplied || status == StatusUnchanged
}

func failureOf(failed *TargetError) error {
	if failed == nil {
		return nil
	}
	return failed
}
