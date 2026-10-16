# Supported platforms and limits

The Rust crate supports macOS and Linux with a stable Rust toolchain. Windows is
unsupported and is not an accepted CI platform.

Bundle inputs can describe skills, instructions, MCP registrations, hooks, native
payloads, and support assets. The lifecycle accepts explicit files, trees,
sections, and JSON/JSONC/TOML selections. Agent presets cover Claude, Codex,
OpenCode, Pi, Cursor, Copilot, and Droid, subject to the exact
[translation matrix](../../spec/translation.md).

`detect_agents` reports executable and supported macOS application evidence in
declaration order. Detection does not select an agent. Droid is detectable but
has no supported translation or lifecycle destination.

Flyrail does not manage cloud configuration, execute bundled commands during
installation, or contact MCP servers. A current installation does not prove host
discovery, trust, activation, credentials, or runtime readiness.

## Security metadata

Partial section and structured updates preserve current file mode and supported
security metadata, including later restrictive mode changes. Whole-file and tree
claims use their explicit content mode contract.

On supported POSIX systems, mode and owner/group evidence is bound and preserved.
Special mode bits, unsupported flags, ACLs, or extended attributes on managed
resources refuse mutation. On macOS, opaque `com.apple.provenance` bytes are
captured and checked; other resource xattrs and extended ACLs refuse.

Ancestor identity, mode, ownership, and security evidence are also bound.
Transaction, staging, and backup directories must pass private-access
checks; flyrail refuses unsafe directories without changing their
permissions. An existing `DirectoryStore` root, `records`, `locks`, or
`staging` directory must pass the same check: every store operation reports
`ErrorCode::IoError` naming the path otherwise. A custom `StateStore` is
responsible for keeping its own storage private.

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
