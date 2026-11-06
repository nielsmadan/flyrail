package flyrail

import (
	"crypto/sha256"
	"encoding/hex"
	"slices"
	"strings"
)

func inventory(entries []BundleEntry) ([]InventoryEntry, error) {
	sorted := slices.Clone(entries)
	slices.SortStableFunc(sorted, func(a, b BundleEntry) int { return strings.Compare(a.Path, b.Path) })
	result := make([]InventoryEntry, 0, len(sorted))
	for _, entry := range sorted {
		if err := validateRelativePath(entry.Path, "inventory path"); err != nil {
			return nil, err
		}
		item := InventoryEntry{Path: entry.Path, Dir: entry.Dir, Executable: entry.Executable}
		if !entry.Dir {
			sum := sha256.Sum256(entry.Data)
			item.Size = int64(len(entry.Data))
			item.SHA256 = hex.EncodeToString(sum[:])
		}
		result = append(result, item)
	}
	return result, nil
}
