package sysx

const (
	modeTypeMask  = 0o170000
	modeRegular   = 0o100000
	modeDirectory = 0o040000
)

// Signature identifies one observed state of a filesystem object: device, inode, mode, size and change times.
type Signature struct {
	Dev   uint64
	Ino   uint64
	Mode  uint32
	Size  int64
	Mtime int64
	Ctime int64
}

// IsRegular reports whether the object is a regular file.
func (s Signature) IsRegular() bool {
	return s.Mode&modeTypeMask == modeRegular
}

// IsDir reports whether the object is a directory.
func (s Signature) IsDir() bool {
	return s.Mode&modeTypeMask == modeDirectory
}
