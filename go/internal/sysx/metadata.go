package sysx

// Metadata is the complete lstat(2) identity of one filesystem object, comparable with ==.
type Metadata struct {
	Dev       uint64
	Ino       uint64
	Mode      uint32
	Nlink     uint64
	UID       uint32
	GID       uint32
	Size      int64
	MtimeSec  int64
	MtimeNsec int64
	CtimeSec  int64
	CtimeNsec int64
}

// IsRegular reports whether the object is a regular file.
func (m Metadata) IsRegular() bool {
	return m.Mode&modeTypeMask == modeRegular
}

// IsDir reports whether the object is a directory.
func (m Metadata) IsDir() bool {
	return m.Mode&modeTypeMask == modeDirectory
}
