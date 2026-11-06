package main

import (
	"encoding/json"
	"io"

	"github.com/nielsmadan/flyrail/go/flyrail"
)

type report struct {
	AppVersion    string  `json:"app_version"`
	BundleID      string  `json:"bundle_id"`
	BundleVersion *string `json:"bundle_version"`
	Results       any     `json:"results"`
}

type targetJSON struct {
	Root  string  `json:"root"`
	Agent *string `json:"agent"`
	Scope string  `json:"scope"`
}

type inspectionJSON struct {
	Target      targetJSON      `json:"target"`
	Root        string          `json:"root"`
	AliasOf     *int            `json:"alias_of"`
	Observation observationJSON `json:"observation"`
	IsCurrent   bool            `json:"is_current"`
}

type resultJSON struct {
	Target        targetJSON      `json:"target"`
	Root          string          `json:"root"`
	AliasOf       *int            `json:"alias_of"`
	Status        string          `json:"status"`
	Observation   observationJSON `json:"observation"`
	Error         *errorJSON      `json:"error"`
	RecoveryPaths []string        `json:"recovery_paths"`
	IsCurrent     bool            `json:"is_current"`
}

type observationJSON struct {
	State                  string             `json:"state"`
	Installed              *installationJSON  `json:"installed"`
	Installations          []installationJSON `json:"installations"`
	VersionMatches         *bool              `json:"version_matches"`
	RecordedContentMatches *bool              `json:"recorded_content_matches"`
	ContentMatches         *bool              `json:"content_matches"`
	Modifications          []modificationJSON `json:"modifications"`
	Conflicts              []conflictJSON     `json:"conflicts"`
	Error                  *errorJSON         `json:"error"`
	RecoveryPaths          []string           `json:"recovery_paths"`
}

type installationJSON struct {
	BundleID      string      `json:"bundle_id"`
	Version       string      `json:"version"`
	ContentDigest string      `json:"content_digest"`
	TransactionID string      `json:"transaction_id"`
	Entries       []entryJSON `json:"entries"`
}

type entryJSON struct {
	Path       string  `json:"path"`
	Size       *int64  `json:"size"`
	SHA256     *string `json:"sha256"`
	Executable bool    `json:"executable"`
}

type modificationJSON struct {
	BundleID string `json:"bundle_id"`
	Path     string `json:"path"`
	Kind     string `json:"kind"`
}

type conflictJSON struct {
	Path  string  `json:"path"`
	Owner *string `json:"owner"`
}

type errorJSON struct {
	Code    string  `json:"code"`
	Message string  `json:"message"`
	Path    *string `json:"path"`
	Errno   *int    `json:"errno"`
}

func newReport(bundleID, bundleVersion string, results any) report {
	return report{AppVersion: appVersion, BundleID: bundleID, BundleVersion: optional(bundleVersion), Results: results}
}

func writeJSON(w io.Writer, value any) error {
	encoder := json.NewEncoder(w)
	encoder.SetEscapeHTML(false)
	return encoder.Encode(value)
}

func inspectionReports(inspections []flyrail.TargetInspection) []inspectionJSON {
	return convert(inspections, func(item flyrail.TargetInspection) inspectionJSON {
		return inspectionJSON{
			Target:      newTargetJSON(item.Target),
			Root:        item.Root,
			AliasOf:     aliasOf(item.IsAlias, item.AliasOf),
			Observation: newObservationJSON(item.Observation),
			IsCurrent:   item.Observation.IsCurrent(),
		}
	})
}

func resultReports(results []flyrail.TargetResult) []resultJSON {
	return convert(results, func(item flyrail.TargetResult) resultJSON {
		return resultJSON{
			Target:        newTargetJSON(item.Target),
			Root:          item.Root,
			AliasOf:       aliasOf(item.IsAlias, item.AliasOf),
			Status:        string(item.Status),
			Observation:   newObservationJSON(item.Observation),
			Error:         newErrorJSON(item.Err),
			RecoveryPaths: list(item.RecoveryPaths),
			IsCurrent:     item.Observation.IsCurrent(),
		}
	})
}

func newTargetJSON(target flyrail.Target) targetJSON {
	return targetJSON{Root: target.Root(), Agent: optional(string(target.Agent())), Scope: string(target.Scope())}
}

func newObservationJSON(observation flyrail.Observation) observationJSON {
	var installed *installationJSON
	if observation.Installed != nil {
		value := newInstallationJSON(*observation.Installed)
		installed = &value
	}
	return observationJSON{
		State:                  string(observation.State),
		Installed:              installed,
		Installations:          convert(observation.Installations, newInstallationJSON),
		VersionMatches:         observation.VersionMatches,
		RecordedContentMatches: observation.RecordedContentMatches,
		ContentMatches:         observation.ContentMatches,
		Modifications: convert(observation.Modifications, func(item flyrail.Modification) modificationJSON {
			return modificationJSON{BundleID: item.BundleID, Path: item.Path, Kind: string(item.Kind)}
		}),
		Conflicts: convert(observation.Conflicts, func(item flyrail.Conflict) conflictJSON {
			return conflictJSON{Path: item.Path, Owner: optional(item.Owner)}
		}),
		Error:         newErrorJSON(observation.Err),
		RecoveryPaths: list(observation.RecoveryPaths),
	}
}

func newInstallationJSON(installation flyrail.Installation) installationJSON {
	return installationJSON{
		BundleID:      installation.BundleID,
		Version:       installation.Version,
		ContentDigest: installation.ContentDigest,
		TransactionID: installation.TransactionID,
		Entries: convert(installation.Entries, func(entry flyrail.InventoryEntry) entryJSON {
			if entry.Dir {
				return entryJSON{Path: entry.Path, Executable: entry.Executable}
			}
			return entryJSON{Path: entry.Path, Size: &entry.Size, SHA256: &entry.SHA256, Executable: entry.Executable}
		}),
	}
}

func newErrorJSON(err *flyrail.TargetError) *errorJSON {
	if err == nil {
		return nil
	}
	out := &errorJSON{Code: string(err.Code), Message: err.Message, Path: optional(err.Path)}
	if err.Errno != 0 {
		errno := int(err.Errno)
		out.Errno = &errno
	}
	return out
}

func aliasOf(isAlias bool, index int) *int {
	if !isAlias {
		return nil
	}
	return &index
}

func optional(value string) *string {
	if value == "" {
		return nil
	}
	return &value
}

// Keeps empty lists as [] rather than null.
func list(values []string) []string {
	if values == nil {
		return []string{}
	}
	return values
}

func convert[T, U any](items []T, fn func(T) U) []U {
	converted := make([]U, 0, len(items))
	for _, item := range items {
		converted = append(converted, fn(item))
	}
	return converted
}
