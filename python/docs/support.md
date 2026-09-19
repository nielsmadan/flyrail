# Supported destinations and limits

Flyrail requires Python 3.11 or newer. Its supported native adapters target macOS
and Linux. Native runner results are required before claiming support for a release;
a simulated native call is not a substitute.

A Windows adapter exists in the source but is **unsupported**. It has never passed a
native Windows runner, so no release claims Windows behavior and CI does not gate on
it. Treat the Windows rows and paragraphs below as a description of the unverified
adapter, not a support commitment.

## Content and destinations

Bundle inputs describe skills, instructions, MCP registrations, hooks and their
supporting assets. The general lifecycle accepts explicit rendered files, trees,
sections and structured selections. Skill convenience presets select documented
containers for six agents; see [destinations](inspection.md).

Flyrail does not select installed agents or manage cloud configuration. Configuration
current does not prove host discovery, activation, trust or runtime prerequisites.
The pure `InstallationObservation.matches(bundle, rendered)` check compares the
app's bundled generation with recorded integrity. It does not perform remote
update discovery or determine version ordering.
Shared agent discovery does not create logical reference counts: removing a bundle
from a shared physical destination affects every consumer of that destination.

Skill Markdown/frontmatter, binary bytes and explicit executable intent remain
authored content. The library does not infer executable intent from source modes.

## Security metadata

Partial section/key updates preserve current file mode and supported security
metadata, including a later restrictive chmod. They restore selected content from
the first takeover baseline without rolling back unrelated file-wide permissions.
Whole file/tree claims use their explicit mode contract.

| Platform | Supported metadata and refusal boundary |
| --- | --- |
| POSIX | Mode and owner/group are bound and preserved. Special mode bits, unsupported flags, extended ACLs or xattrs on managed resources refuse mutation. |
| macOS | Opaque `com.apple.provenance` bytes are captured and must match staged/publication evidence. Other resource xattrs and extended ACLs refuse. |
| Windows (unsupported) | Owner/group/DACL snapshots and supported native document replacement; read-only, encrypted, compressed, sparse or alternate-stream resources refuse. Tree/container metadata must satisfy the supported private-descriptor contract. |

Ancestor snapshots include identity, mode/ownership and security evidence.
Existing state, staging and backup directories must pass private-access checks;
Flyrail refuses unsafe directories without changing their permissions. New
payload files are private before their first write. On Windows, state files and
backup payloads also require private access themselves; placing a broadly readable
file under a private parent is insufficient.
See Microsoft's [traverse-checking behavior](https://learn.microsoft.com/en-us/windows/security/threat-protection/security-policy-settings/bypass-traverse-checking).

Existing Windows documents use
[ReplaceFileW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew)
without ignore-ACL/merge flags. The journal binds a temporary private-DACL transition
so its backup is private from creation. A crash can leave the public destination
restricted until recovery restores its original descriptor. Unsupported access or
metadata has no weaker in-place fallback.

## Filesystem and recovery

Use local filesystems with the required advisory locks, exclusive moves and
same-volume state. Symlinks, junctions/reparse points, hard links, special files,
ambiguous names and incompatible ancestor/descendant authorities are rejected.
Permanent authority fences remain after failed admission and uninstall.

Whole-target known-conflict preflight precedes publication. Resources commit
independently; readers can observe intermediate resource states. Read-only
inspection can encounter a live transaction.

Full revisions detect ordinary concurrent content/metadata changes. They do not
authenticate receipts or defend against hostile same-user writes between checks
and syscalls. Unknown preparation data, edited backups and unrecognized revisions
are retained with diagnostics. There is no age-based cleanup or force-recovery API.

The [transaction protocol](../../spec/transaction-protocol.md) handles process
interruption. It provides no cross-resource atomicity, simultaneous multi-file
visibility, network-filesystem/cloud-sync guarantee or power-loss durability.
