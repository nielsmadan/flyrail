package native

import "time"

// Times holds a file's modification time and, where the platform compares it, its birth time.
type Times struct {
	Modified time.Time
	Born     time.Time
	HasBorn  bool
}

// ChangedAfter reports whether the file was modified, or born where birth times are compared, after boundary.
func (t Times) ChangedAfter(boundary time.Time) bool {
	return t.Modified.After(boundary) || t.HasBorn && t.Born.After(boundary)
}
