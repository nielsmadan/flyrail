package native

import (
	"syscall"
	"time"

	"golang.org/x/sys/unix"

	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

// Security captures the native security metadata of path: nothing for a resource, which must carry no extended
// attribute (SELinux labels and ACLs included), and the schema-1 metadata record for an ancestor.
func Security(path string, ancestor bool) ([]byte, error) {
	if _, err := sysx.RawLstat(path); err != nil {
		return nil, ioFailure(err, path)
	}
	names, err := attributeNameList(path)
	if err != nil {
		return nil, err
	}
	if len(names) > 0 && !ancestor {
		return nil, refused(syscall.ENOTSUP, "extended attributes and ACLs are unsupported", path)
	}
	if !ancestor {
		return []byte{}, nil
	}
	values := make([]attribute, 0, len(names))
	for _, name := range names {
		payload, err := attributePayload(path, name)
		if err != nil {
			return nil, err
		}
		values = append(values, attribute{name: []byte(name), value: payload})
	}
	return ancestorMetadata(0, values, nil), nil
}

type fill uint8

const (
	fillComplete fill = iota
	fillRetry
	fillFailed
)

func fillOutcome(filled, capacity int, errno syscall.Errno) fill {
	switch {
	case filled >= 0 && filled <= capacity:
		return fillComplete
	case filled >= 0 || errno == syscall.ERANGE:
		return fillRetry
	}
	return fillFailed
}

func attributeNameList(path string) ([]string, error) {
	for {
		size, err := sysx.Llistxattr(path, nil)
		if err != nil {
			return nil, ioFailure(err, path)
		}
		if size == 0 {
			return nil, nil
		}
		buffer := make([]byte, size)
		filled, err := sysx.Llistxattr(path, buffer)
		if err != nil {
			filled = -1
		}
		switch fillOutcome(filled, len(buffer), sysx.Errno(err)) {
		case fillComplete:
			return parseNames(buffer[:filled]), nil
		case fillFailed:
			return nil, ioFailure(err, path)
		case fillRetry:
		}
	}
}

func parseNames(buffer []byte) []string {
	var names []string
	start := 0
	for offset, b := range buffer {
		if b == 0 {
			names = append(names, string(buffer[start:offset]))
			start = offset + 1
		}
	}
	if start < len(buffer) {
		names = append(names, string(buffer[start:]))
	}
	return names
}

func attributePayload(path, name string) ([]byte, error) {
	for {
		size, err := sysx.Lgetxattr(path, name, nil)
		if err != nil {
			return nil, ioFailure(err, path)
		}
		buffer := make([]byte, size)
		filled, err := sysx.Lgetxattr(path, name, buffer)
		if err != nil {
			filled = -1
		}
		switch fillOutcome(filled, len(buffer), sysx.Errno(err)) {
		case fillComplete:
			return buffer[:filled], nil
		case fillFailed:
			return nil, ioFailure(err, path)
		case fillRetry:
		}
	}
}

// PhysicalDirectory returns the canonical path of the directory path with every symbolic link resolved.
func PhysicalDirectory(path string) (string, error) {
	physical, err := sysx.Canonicalize(path)
	if err != nil {
		return "", ioFailure(err, path)
	}
	return physical, nil
}

func timesOf(st *unix.Stat_t) Times {
	return Times{Modified: time.Unix(st.Mtim.Unix())}
}

func mountTable() ([]byte, bool) {
	return sysx.MountInfo(), true
}
