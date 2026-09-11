# Supported destinations and limits

Flyrail requires Python 3.11 or newer. Its platform adapters target macOS, Linux
and Windows. CI runs the shared gate for Python 3.11 and 3.14 on all three; native
runner results are required before claiming support for a release.

On Windows, mutations require Python **3.11.10+**, **3.12.4+**, or **3.13+**.
These releases apply a current-user/administrator access-control list when
creating a directory with mode `0700`; see the official
[3.11 `os.mkdir` documentation](https://docs.python.org/3.11/library/os.html#os.mkdir)
and [3.12 documentation](https://docs.python.org/3.12/library/os.html#os.mkdir).
Earlier Windows patches return `UNSUPPORTED` for each mutation target before
creating management state. Bundle loading and read-only inspection remain usable.

Use local filesystems that support the required advisory locks, exclusive
directory renames and same-volume staging/backup moves. Unsupported operations
report `UNSUPPORTED`; there is no network-filesystem, cloud-sync-folder or
power-loss guarantee. Symlinks, junctions, known reparse points, special files,
ambiguous portable names and overlapping managed roots are rejected. Ordinary
system symlinks above explicit roots can be canonicalized once.

## Agent scopes

User and project presets are provided for Claude Code, Codex, OpenCode, Pi,
Cursor, and Copilot CLI/VS Code. Every preset resolves a documented local skill
container; [destinations](inspection.md) lists exact paths, environment overrides
and source links. `Target.directory` supports explicit custom containers.
There is no automatic installed-agent selection or cloud-agent compatibility claim.

Presets do not imply exclusive discovery. Several agents read `.agents` or Claude
directories in addition to their own conventions. A host must choose the physical
installation it intends to manage and explain shared visibility to its user.
Flyrail deduplicates aliases within a request but does not track logical consumers
across applications or calls. Uninstalling a shared directory's bundle affects
every agent that reads it.

Bundles contain whole skill directories with exact `SKILL.md` files. Flyrail
preserves Markdown, YAML extensions, line endings and binary bytes; it does not
validate each agent's frontmatter semantics or rewrite content for an agent.
The author must supply a skill the intended consumers understand. POSIX executable
intent is applied and checked; Windows records the logical intent in receipts.

## Filesystem and recovery boundary

Each mutation is independently committed per target. Other targets continue when
one is conflicted, modified, busy or unreadable. Skills publish one directory at a
time, so readers can observe a partially published set before receipt commit.
Read-only inspection can encounter a live writer and reports pending work without
claiming that writer is abandoned.

New management directories use mode `0700`. On POSIX, each fresh transaction's
staging and backup roots exclude group/other access before receiving skill bytes,
including when existing management parents are permissive. Existing directory
permissions and the permanent lock inode are preserved. Installed file modes
remain `0755` for executable intent and `0644` otherwise.

Local edits in the requested bundle block the entire target operation unless
explicit replacement is authorized. Foreign ownership and untracked roots remain
conflicts. Malformed or contradictory management state fails closed. Ordinary edits
to a different, disjoint bundle do not block this bundle, while unsafe managed
paths and ownership contradictions still do.

Locks coordinate cooperating Flyrail writers. Inventory checks detect common
concurrent edits; they cannot defend against a hostile same-user process changing
paths between a check and a syscall. Receipts and hashes are integrity records,
not authenticated claims. Source and inspection reads are best effort against
external writers.

The [transaction protocol](../../spec/transaction-protocol.md) handles process interruption
where immutable intent and recorded inventories establish safe rollback/cleanup.
Unrecognized preparation data left before a complete intent was published stays
on disk with `INCOMPLETE` and recovery paths. There is no age-based deletion or
force-clean operation. Edited backups and ambiguous recovery layouts are retained
for examination. Source bytes are unnecessary for recognized recovery or removal.

An `APPLIED` result can still carry cleanup errors. Preserve its recovery paths;
the next mutation attempts safe cleanup before proceeding. Uninstall retains
unrelated data, the shared skill container, and small management/lock metadata.
There is no cross-target atomicity or power-loss durability promise: the protocol
does not implement an fsync ordering scheme.
