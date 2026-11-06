package darwin

import (
	"bytes"
	"syscall"
	"unsafe"

	"golang.org/x/sys/unix"
)

// GetPath returns the path the kernel records for an open descriptor via fcntl(F_GETPATH).
func GetPath(fd int) (string, error) {
	buffer := make([]byte, unix.PathMax)
	for {
		_, _, errno := syscall_syscall(libc_fcntl_trampoline_addr, uintptr(fd), uintptr(unix.F_GETPATH), uintptr(unsafe.Pointer(&buffer[0])))
		if errno == syscall.EINTR {
			continue
		}
		if errno != 0 {
			return "", errno
		}
		end := bytes.IndexByte(buffer, 0)
		if end < 0 {
			end = len(buffer)
		}
		return string(buffer[:end]), nil
	}
}
