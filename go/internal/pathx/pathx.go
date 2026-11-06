package pathx

import (
	"slices"
	"strings"
)

// IsAbsolute reports whether path starts with a separator.
func IsAbsolute(path string) bool {
	return strings.HasPrefix(path, "/")
}

// Components splits path like Rust's Path::components on Unix: repeated separators and interior "." vanish.
func Components(path string) []string {
	var components []string
	switch {
	case strings.HasPrefix(path, "/"):
		components = append(components, "/")
	case path == "." || strings.HasPrefix(path, "./"):
		components = append(components, ".")
	}
	for part := range strings.SplitSeq(path, "/") {
		if part != "" && part != "." {
			components = append(components, part)
		}
	}
	return components
}

// Join appends parts to base as Rust's PathBuf::push does: an absolute part replaces base.
func Join(base string, parts ...string) string {
	for _, part := range parts {
		switch {
		case strings.HasPrefix(part, "/") || base == "":
			base = part
		case strings.HasSuffix(base, "/"):
			base += part
		default:
			base += "/" + part
		}
	}
	return base
}

// FileName returns the final normal component of path, as Rust's Path::file_name does.
func FileName(path string) (string, bool) {
	components := Components(path)
	if len(components) == 0 {
		return "", false
	}
	last := components[len(components)-1]
	if last == "/" || last == "." || last == ".." {
		return "", false
	}
	return last, true
}

// Parent returns path without its last component, keeping the remaining text, as Rust's Path::parent does.
func Parent(path string) (string, bool) {
	trimmed := trimEnd(path)
	if trimmed == "" || trimmed == "/" || trimmed == "." {
		return "", trimmed == "."
	}
	cut := strings.LastIndexByte(trimmed, '/')
	if cut < 0 {
		return "", true
	}
	parent := trimEnd(trimmed[:cut+1])
	if parent == "" {
		return "/", true
	}
	return parent, true
}

func trimEnd(path string) string {
	for {
		switch {
		case len(path) > 1 && strings.HasSuffix(path, "/"):
			path = path[:len(path)-1]
		case strings.HasSuffix(path, "/."):
			path = path[:len(path)-1]
		default:
			return path
		}
	}
}

// NormalizedParent is Parent rebuilt from Components, so repeated separators and interior "." do not survive.
func NormalizedParent(path string) (string, bool) {
	components := Components(path)
	if len(components) == 0 || len(components) == 1 && components[0] == "/" {
		return "", false
	}
	return fromComponents(components[:len(components)-1]), true
}

func fromComponents(components []string) string {
	path := ""
	for _, component := range components {
		if component == "/" {
			path = "/"
			continue
		}
		path = Join(path, component)
	}
	return path
}

// Directory returns the directory holding path: trailing separators are ignored, a bare name gives "." and a path of only separators is returned unchanged.
func Directory(path string) string {
	trimmed := strings.TrimRight(path, "/")
	if trimmed == "" {
		return path
	}
	index := strings.LastIndexByte(trimmed, '/')
	if index < 0 {
		return "."
	}
	parent := strings.TrimRight(trimmed[:index], "/")
	if parent == "" {
		return "/"
	}
	return parent
}

// Absolute makes path absolute against the working directory current as Rust's std::path::absolute does: "." and repeated separators vanish, ".." and a leading "//" stay.
func Absolute(path, current string) string {
	base := "/"
	switch {
	case strings.HasPrefix(path, "//") && !strings.HasPrefix(path, "///"):
		base = "//"
	case !IsAbsolute(path):
		base = current
	}
	for part := range strings.SplitSeq(path, "/") {
		if part != "" && part != "." {
			base = Join(base, part)
		}
	}
	if strings.HasSuffix(path, "/") {
		base = Join(base, "")
	}
	return base
}

// HasPrefix reports whether the components of path start with those of prefix, as Rust's Path::starts_with does.
func HasPrefix(path, prefix string) bool {
	whole, head := Components(path), Components(prefix)
	return len(head) <= len(whole) && slices.Equal(whole[:len(head)], head)
}
