#include "textflag.h"

TEXT libc_acl_get_file_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_acl_get_file(SB)
GLOBL	·libc_acl_get_file_trampoline_addr(SB), RODATA, $8
DATA	·libc_acl_get_file_trampoline_addr(SB)/8, $libc_acl_get_file_trampoline<>(SB)

TEXT libc_acl_get_entry_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_acl_get_entry(SB)
GLOBL	·libc_acl_get_entry_trampoline_addr(SB), RODATA, $8
DATA	·libc_acl_get_entry_trampoline_addr(SB)/8, $libc_acl_get_entry_trampoline<>(SB)

TEXT libc_acl_to_text_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_acl_to_text(SB)
GLOBL	·libc_acl_to_text_trampoline_addr(SB), RODATA, $8
DATA	·libc_acl_to_text_trampoline_addr(SB)/8, $libc_acl_to_text_trampoline<>(SB)

TEXT libc_acl_free_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_acl_free(SB)
GLOBL	·libc_acl_free_trampoline_addr(SB), RODATA, $8
DATA	·libc_acl_free_trampoline_addr(SB)/8, $libc_acl_free_trampoline<>(SB)

TEXT libc_fcntl_trampoline<>(SB),NOSPLIT,$0-0
	JMP	libc_fcntl(SB)
GLOBL	·libc_fcntl_trampoline_addr(SB), RODATA, $8
DATA	·libc_fcntl_trampoline_addr(SB)/8, $libc_fcntl_trampoline<>(SB)
