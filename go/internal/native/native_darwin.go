package native

import (
	"bytes"
	"slices"
	"strings"
	"syscall"
	"time"

	"golang.org/x/sys/unix"

	"github.com/nielsmadan/flyrail/go/internal/darwin"
	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

const provenance = "com.apple.provenance"

// Security captures the native security metadata of path: for a resource, the framed com.apple.provenance value
// (refusing flags, any other extended attribute and extended ACLs); for an ancestor, the schema-1 metadata record.
func Security(path string, ancestor bool) ([]byte, error) {
	if strings.IndexByte(path, 0) >= 0 {
		return nil, invalid("embedded null byte")
	}
	st, err := sysx.RawLstat(path)
	if err != nil {
		return nil, ioFailure(err, path)
	}
	if st.Flags != 0 && !ancestor {
		return nil, refused(syscall.ENOTSUP, "file flags cannot be preserved safely", path)
	}
	return capture(path, ancestor, st.Flags)
}

func capture(path string, ancestor bool, flags uint32) ([]byte, error) {
	count, err := sysx.Llistxattr(path, nil)
	if err != nil {
		return nil, refused(sysx.Errno(err), "cannot inspect extended attributes", path)
	}
	var values []attribute
	framed := []byte{}
	if count > 0 {
		buffer := make([]byte, count)
		filled, err := sysx.Llistxattr(path, buffer)
		if err != nil || filled != count {
			return nil, refused(syscall.EAGAIN, "extended attributes changed", path)
		}
		names := attributeNames(buffer)
		if !ancestor && slices.ContainsFunc(names, func(name string) bool { return name != provenance }) {
			return nil, refused(syscall.ENOTSUP, "extended attributes are unsupported", path)
		}
		for _, name := range names {
			payload, err := attributeValue(path, name)
			if err != nil {
				return nil, err
			}
			values = append(values, attribute{name: []byte(name), value: payload})
		}
		if !ancestor && len(values) > 0 {
			framed = append(append(append(framed, provenance...), 0), values[0].value...)
		}
	}
	acl, err := aclText(path, ancestor)
	if err != nil {
		return nil, err
	}
	if ancestor {
		return ancestorMetadata(flags, values, acl), nil
	}
	return framed, nil
}

// A buffer of only NULs yields one empty name.
func attributeNames(buffer []byte) []string {
	stripped := bytes.TrimRight(buffer, "\x00")
	names := []string{""}
	if len(stripped) > 0 {
		names = strings.Split(string(stripped), "\x00")
	}
	slices.Sort(names)
	return slices.Compact(names)
}

func attributeValue(path, name string) ([]byte, error) {
	size, err := sysx.Lgetxattr(path, name, nil)
	if err != nil {
		return nil, refused(sysx.Errno(err), "cannot read extended metadata", path)
	}
	payload := make([]byte, size)
	filled, err := sysx.Lgetxattr(path, name, payload)
	if err != nil || filled != size {
		return nil, refused(syscall.EAGAIN, "extended metadata changed", path)
	}
	return payload, nil
}

func aclText(path string, ancestor bool) ([]byte, error) {
	acl, err := darwin.ACLGetFile(path, darwin.ACLTypeExtended)
	if err != nil {
		errno := sysx.Errno(err)
		if errno != syscall.ENOENT {
			return nil, refused(errno, "cannot inspect file ACL", path)
		}
		if _, err := sysx.RawLstat(path); err != nil {
			return nil, ioFailure(err, path)
		}
		return []byte{}, nil
	}
	text, failure := extendedText(acl, path, ancestor)
	_ = acl.Free()
	return text, failure
}

func extendedText(acl *darwin.ACL, path string, ancestor bool) ([]byte, error) {
	if !acl.HasEntry(darwin.ACLFirstEntry) {
		return []byte{}, nil
	}
	if !ancestor {
		return nil, refused(syscall.ENOTSUP, "extended ACLs are unsupported", path)
	}
	text, err := acl.Text()
	if text == nil {
		return nil, refused(sysx.Errno(err), "cannot capture ancestor ACL", path)
	}
	return text, nil
}

// PhysicalDirectory returns the kernel's spelling of the directory path, refusing a final link and a directory replaced while it was resolved.
func PhysicalDirectory(path string) (string, error) {
	fd, err := sysx.OpenDirectoryNoFollow(path)
	if err != nil {
		return "", ioFailure(err, path)
	}
	defer func() { _ = sysx.Close(fd) }()
	opened, err := sysx.Fstat(fd, path)
	if err != nil {
		return "", ioFailureWithoutPath(err)
	}
	physical, err := darwin.GetPath(fd)
	if err != nil {
		return "", ioFailureWithoutPath(err)
	}
	current, err := sysx.Lstat(physical)
	if err != nil {
		return "", ioFailure(err, physical)
	}
	if !sameObject(opened, current) {
		return "", unsafePath("resource parent changed during canonicalization", path)
	}
	return physical, nil
}

func timesOf(st *unix.Stat_t) Times {
	return Times{
		Modified: time.Unix(st.Mtim.Unix()),
		Born:     time.Unix(st.Btim.Unix()),
		HasBorn:  true,
	}
}

func mountTable() ([]byte, bool) {
	return nil, false
}

func sameObject(first, second sysx.Signature) bool {
	return first.Dev == second.Dev && first.Ino == second.Ino
}
