package store

// WithRecordLimit returns a copy of s that refuses records larger than limit bytes.
func (s *DirectoryStore) WithRecordLimit(limit int64) *DirectoryStore {
	limited := *s
	limited.maxRecord = limit
	return &limited
}

// NewMemoryStoreWithRecordLimit returns an empty memory store that refuses records larger than limit bytes.
func NewMemoryStoreWithRecordLimit(limit int64) *MemoryStore {
	return &MemoryStore{maxRecord: limit}
}

// MaxRecordBytes is the default record limit of both stores.
const MaxRecordBytes = maxRecordBytes
