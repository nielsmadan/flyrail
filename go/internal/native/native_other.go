//go:build !darwin && !linux

package native

func unsupported(path string) *Error {
	return &Error{Kind: KindUnsupported, Message: "native filesystem operations are unsupported on this platform", Path: path}
}

// Security reports that native security capture is unsupported on this platform.
func Security(path string, _ bool) ([]byte, error) {
	return nil, unsupported(path)
}

// EnsurePrivate reports that private management storage is unsupported on this platform.
func EnsurePrivate(path string) error {
	return unsupported(path)
}

// FileTimes reports that native file times are unsupported on this platform.
func FileTimes(path string) (Times, error) {
	return Times{}, unsupported(path)
}

// CanonicalDestination reports that canonicalization is unsupported on this platform.
func CanonicalDestination(selected string) (string, error) {
	return "", unsupported(selected)
}

// CanonicalChild reports that canonicalization is unsupported on this platform.
func CanonicalChild(parent, _ string) (string, error) {
	return "", unsupported(parent)
}

// PhysicalDirectory reports that canonicalization is unsupported on this platform.
func PhysicalDirectory(path string) (string, error) {
	return "", unsupported(path)
}

// PhysicalLocation reports that canonicalization is unsupported on this platform.
func PhysicalLocation(path string) (string, error) {
	return "", unsupported(path)
}

// TransactionEntries reports that tree traversal is unsupported on this platform.
func TransactionEntries(destination string) ([]string, error) {
	return nil, unsupported(destination)
}

// ManagedSpelling reports that management storage checks are unsupported on this platform.
func ManagedSpelling(stateRoot string) error {
	return unsupported(stateRoot)
}

// SameMount reports that mount detection is unsupported on this platform.
func SameMount(first, _ string) (bool, error) {
	return false, unsupported(first)
}
