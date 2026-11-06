package native

import (
	"strings"

	"github.com/nielsmadan/flyrail/go/internal/unicodex"
)

// ManagementName reports whether name is portably spelled as a Flyrail transaction directory.
func ManagementName(name string) bool {
	folded := unicodex.PortableKey(name)
	return strings.HasPrefix(folded, ".flyrail-") && strings.HasSuffix(folded, ".txn")
}
