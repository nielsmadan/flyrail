package sysx

import (
	"io"
	"os"
)

const maxMountInfo = 16 << 20

// MountInfo returns /proc/self/mountinfo, or nothing when it is unreadable or larger than any real mount table.
func MountInfo() []byte {
	return readMountInfo("/proc/self/mountinfo", maxMountInfo)
}

func readMountInfo(path string, limit int64) []byte {
	file, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer func() { _ = file.Close() }()
	data, err := io.ReadAll(io.LimitReader(file, limit+1))
	if err != nil || int64(len(data)) > limit {
		return nil
	}
	return data
}
