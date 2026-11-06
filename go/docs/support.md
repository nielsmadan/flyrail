# Supported platforms and limits

The module supports macOS and Linux on amd64 and arm64 with Go 1.27 or later
(CI pins 1.27.3). It builds without cgo. Windows is unsupported and is not an
accepted CI platform.

Bundle inputs can describe skills, instructions, MCP registrations, hooks, native
payloads, and support assets. The lifecycle accepts explicit files, trees,
sections, and JSON/JSONC/TOML selections. Agent presets cover Claude, Codex,
OpenCode, Pi, Cursor, Copilot, and Droid, subject to the exact
[translation matrix](../../spec/translation.md).

`DetectAgents` reports executable and supported macOS application evidence in
`Agents()` order. Detection does not select an agent. Droid is detectable but
has no supported translation or lifecycle destination.

Flyrail does not manage cloud configuration, execute bundled commands during
installation, or contact MCP servers. A current installation does not prove host
discovery, trust, activation, credentials, or runtime readiness.

## Other operating systems

On any other `GOOS` (the checks cover `windows` and `freebsd`) the module
compiles and `go vet` passes, but nothing touches the filesystem. Pure functions
such as `Render`, `EditDocument`, `Summarize` and the value, bundle-from-memory
and hook-protocol constructors work, and so does `store.MemoryStore`.
`store.NewDirectoryStore` fails with a `*store.IOError` whose `Errno` is
`ENOTSUP` and whose message is `directory state stores are unsupported on this
platform`. Native operations (canonicalization, security capture, private
storage checks) return an unsupported error, which lifecycle calls report as
`CodeUnsupported`. A CLI should check `runtime.GOOS` and say so instead of
installing.

## Security metadata

Partial section and structured updates preserve current file mode and supported
security metadata, including later restrictive mode changes. Whole-file and tree
claims use their explicit content mode contract.

On supported systems, mode and owner/group evidence is bound and preserved.
Special mode bits, unsupported flags, ACLs, or extended attributes on managed
resources refuse mutation with `CodeUnsupported` and the message
`extended attributes and ACLs are unsupported`.

- **Linux:** any extended attribute on a managed resource is refused. That
  includes SELinux labels (`security.selinux`) and POSIX ACLs
  (`system.posix_acl_*`), which Linux stores as extended attributes, so a
  resource on a filesystem that labels new files, such as an SELinux-enforcing
  host, is refused until the attribute is absent. Ancestor directories may carry
  attributes; their values are captured and compared as evidence.
- **macOS:** the opaque `com.apple.provenance` bytes are captured and checked.
  Other resource extended attributes and extended ACLs refuse. ACLs are read
  with `acl_get_file` through the libSystem trampolines described in
  [architecture](architecture.md#no-cgo).

Ancestor identity, mode, ownership, and security evidence are also bound.
Transaction, staging, and backup directories must pass private-access
checks; Flyrail refuses unsafe directories without changing their
permissions. An existing `DirectoryStore` root, `records`, `locks`, or
`staging` directory must pass the same check: every store operation reports
`CodeIOError` naming the path otherwise. A custom `store.Store` is responsible
for keeping its own storage private.

## Linux without procfs

Canonical destinations on Linux are resolved by opening the path with `O_PATH`
and reading `/proc/self/fd/<n>`. When `/proc` is not mounted, as in some
minimal containers, Flyrail instead resolves the path component by component,
which gives the same answer. Mount information for the staging decision comes
from `/proc/self/mountinfo`; when it is unreadable no mount information is used,
and staging happens beside the destination.

## Resource and input limits

Untrusted sizes are capped before anything is allocated. A managed resource
over a cap is refused as unsupported with `resource exceeds the node/byte limit`.

| Input | Limit |
| --- | --- |
| One managed file | 64 MiB |
| One managed resource, total | 256 MiB |
| Nodes in one managed tree | 10,000 |
| Directory depth in one managed tree | 128 |
| ZIP archive | 256 MiB, 100,000 members, 64 MiB per member, 512 MiB uncompressed in total |
| Value written to or read from a state store | 4 GiB (`[Errno 27] File too large`), twice what the journal of a resource at the limits above needs: its four revisions, hex-encoded |
| Strict JSON nesting | 512 levels |
| Value nesting | 10,000 levels (`value nesting is too deep`) |
| Hook protocol input and output | `HookInputLimit`, `HookOutputLimit`, `HookErrorLimit`, `HookTextLimit`, `HookJSONDepthLimit`, `HookHandlerLimit` |

The caps follow Swift's values and are enforced before allocation; there is no
expansion-ratio cap. A state-store record cap sits above the resource caps.
Agent configuration does not approach these values. Library code does not panic
on any public path, including nil and zero-value arguments, which return
`*ValueError`.

## Filesystem and recovery

Use local filesystems that provide advisory locks, exclusive moves, and same-volume
private state. Managed symlinks, hard links, special files, ambiguous portable
names, and incompatible ancestor/descendant authorities are rejected. Permanent
authority fences can remain after failed admission or uninstall.

Full revisions detect ordinary concurrent content and metadata changes. They do
not authenticate receipts or defend against hostile same-user writes between
checks and syscalls. Unknown preparation data, edited backups, and unrecognized
revisions are retained for diagnosis. There is no age-based cleanup or
force-recovery API. Recovery also retains a destination that matches a
recorded revision but changed after the transaction was prepared. It compares
modification time on every platform and birth time only on macOS; a
replacement that keeps an older modification time and reuses the original's
inode is not detected.

The [transaction protocol](../../spec/transaction-protocol.md) provides
interruption recovery, not cross-resource atomicity, simultaneous visibility,
network-filesystem or cloud-sync safety, or power-loss durability.

## Known limitations

- Taking over an existing tree that contains a read-only directory publishes
  the new tree but leaves the operation `incomplete`, because the old tree
  cannot be removed from staging. Python and Rust share this behavior; Swift
  refuses such trees up front.
