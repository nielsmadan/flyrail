package darwin

import (
	"syscall"
	_ "unsafe" // required by go:linkname
)

//go:linkname syscall_syscall syscall.syscall
func syscall_syscall(fn, a1, a2, a3 uintptr) (r1, r2 uintptr, err syscall.Errno)

//go:linkname syscall_syscallPtr syscall.syscallPtr
func syscall_syscallPtr(fn, a1, a2, a3 uintptr) (r1, r2 uintptr, err syscall.Errno)

var (
	libc_acl_get_file_trampoline_addr  uintptr
	libc_acl_get_entry_trampoline_addr uintptr
	libc_acl_to_text_trampoline_addr   uintptr
	libc_acl_free_trampoline_addr      uintptr
	libc_fcntl_trampoline_addr         uintptr
)

//go:cgo_import_dynamic libc_acl_get_file acl_get_file "/usr/lib/libSystem.B.dylib"
//go:cgo_import_dynamic libc_acl_get_entry acl_get_entry "/usr/lib/libSystem.B.dylib"
//go:cgo_import_dynamic libc_acl_to_text acl_to_text "/usr/lib/libSystem.B.dylib"
//go:cgo_import_dynamic libc_acl_free acl_free "/usr/lib/libSystem.B.dylib"
//go:cgo_import_dynamic libc_fcntl fcntl "/usr/lib/libSystem.B.dylib"
