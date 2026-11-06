package darwin

import (
	"syscall"
	"unsafe"

	"golang.org/x/sys/unix"
)

// ACLTypeExtended selects the extended ACL of a file in ACLGetFile.
const ACLTypeExtended = 0x00000100

// ACLFirstEntry selects the first entry in ACLGetEntry.
const ACLFirstEntry = 0

// ACL is an acl_t owned by the caller, who must release it with Free; Free is idempotent and later calls on a freed ACL report no entry or EINVAL.
type ACL struct {
	handle uintptr
}

// ACLGetFile reads the ACL of the given type at path, following a final symbolic link as acl_get_file does.
func ACLGetFile(path string, aclType int) (*ACL, error) {
	name, err := unix.BytePtrFromString(path)
	if err != nil {
		return nil, err
	}
	for {
		handle, _, errno := syscall_syscallPtr(libc_acl_get_file_trampoline_addr, uintptr(unsafe.Pointer(name)), uintptr(aclType), 0)
		if errno == syscall.EINTR {
			continue
		}
		if errno != 0 {
			return nil, errno
		}
		if handle == 0 {
			return nil, syscall.EINVAL
		}
		return &ACL{handle: handle}, nil
	}
}

// HasEntry reports whether acl_get_entry finds an entry with the given id.
func (a *ACL) HasEntry(entryID int) bool {
	if a == nil || a.handle == 0 {
		return false
	}
	var entry uintptr
	code, _, _ := syscall_syscall(libc_acl_get_entry_trampoline_addr, a.handle, uintptr(entryID), uintptr(unsafe.Pointer(&entry)))
	return code&0xffffffff != 0xffffffff
}

// Text returns the acl_to_text form of the ACL.
func (a *ACL) Text() ([]byte, error) {
	if a == nil || a.handle == 0 {
		return nil, syscall.EINVAL
	}
	var length int64
	text, _, errno := syscall_syscallPtr(libc_acl_to_text_trampoline_addr, a.handle, uintptr(unsafe.Pointer(&length)), 0)
	if errno != 0 {
		return nil, errno
	}
	if text == 0 {
		return nil, syscall.EINVAL
	}
	payload := copyCBytes(text, length)
	return payload, aclFree(text)
}

// Free releases the ACL; freeing a nil, zero or already freed ACL does nothing.
func (a *ACL) Free() error {
	if a == nil || a.handle == 0 {
		return nil
	}
	handle := a.handle
	a.handle = 0
	return aclFree(handle)
}

func aclFree(object uintptr) error {
	_, _, errno := syscall_syscall(libc_acl_free_trampoline_addr, object, 0, 0)
	if errno != 0 {
		return errno
	}
	return nil
}

func copyCBytes(pointer uintptr, length int64) []byte {
	if length <= 0 {
		return []byte{}
	}
	// The pointer refers to libSystem's heap, which the Go collector never moves or frees.
	source := unsafe.Slice((*byte)(*(*unsafe.Pointer)(unsafe.Pointer(&pointer))), length)
	return append([]byte(nil), source...)
}
