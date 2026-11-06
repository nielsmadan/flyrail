package flyrail

import (
	"slices"
	"strings"

	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/unicodex"
)

func pathsEqual(a, b string) bool {
	return slices.Equal(pathx.Components(a), pathx.Components(b))
}

func comparePaths(a, b string) int {
	return slices.Compare(pathx.Components(a), pathx.Components(b))
}

func hasParent(path string) bool {
	components := pathx.Components(path)
	return len(components) > 1 || len(components) == 1 && components[0] != "/" && components[0] != "."
}

func pathExtension(path string) (string, bool) {
	name, ok := pathx.FileName(path)
	if !ok {
		return "", false
	}
	dot := strings.LastIndexByte(name, '.')
	if dot <= 0 {
		return "", false
	}
	return name[dot+1:], true
}

// Mirrors Rust's codec::posix_path: lossy UTF-8, normalized components, a leading "//" kept.
func posixPath(path string) string {
	return unicodex.Lossy(normalizedPosixPath(path))
}

func normalizedPosixPath(text string) string {
	var out strings.Builder
	if strings.HasPrefix(text, "/") {
		if strings.HasPrefix(text, "//") && !strings.HasPrefix(text, "///") {
			out.WriteString("//")
		} else {
			out.WriteString("/")
		}
	}
	for part := range strings.SplitSeq(text, "/") {
		if part == "" || part == "." {
			continue
		}
		if out.Len() > 0 && !strings.HasSuffix(out.String(), "/") {
			out.WriteByte('/')
		}
		out.WriteString(part)
	}
	return out.String()
}
