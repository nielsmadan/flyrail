package sysx

import "golang.org/x/sys/unix"

// DeviceNumber returns the device number of st widened as Rust widens it.
func DeviceNumber(st *unix.Stat_t) uint64 {
	return uint64(st.Dev)
}
