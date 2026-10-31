# Native authority and observation

Flyrail's Swift filesystem layer supports macOS 13 and later. Each managed
resource is identified by its canonical destination, discovered from an explicit
``ResourcePath``. Discovery is read-only: it creates no directory, opens no lock
and does not read agent configuration. No state lives beside a destination: authority
records, receipts and locks belong to the host's ``StateStore``.

Paths retain native bytes. Managed destinations must use portable NFC UTF-8
names and cannot enter reserved `.flyrail-*.txn` transaction directories.
Existing case aliases must identify the same node; absent components retain
their requested spelling. Only the selected resource's parent, or nearest
existing parent, is enumerated for canonicalization. Ancestor traversal uses
search-only descriptors.

All managed-resource I/O is descriptor-relative and no-follow. Snapshots bind
every byte, ordered tree node, mode, owner/group, device and inode. Native
observation also retains link counts and modification/creation evidence to
detect changes during inspection and later replacement. Ancestor evidence binds
identity and security without treating unrelated sibling changes as replacement.

Ancestor ACL text, flags and raw extended attributes use the shared compact JSON
security framing. Resource flags, extended ACLs, foreign extended attributes,
special modes, symlinks, hard-linked documents and special files refuse
management. The opaque `com.apple.provenance` attribute is captured as evidence.

Package-internal admission takes the resource's store lock, validates its
authority record, probes hierarchy conflicts and reobserves the resource. Each
resource's permanent fence is its authority record, and fences are checked only
within one store; a destination managed through another store is foreign content.
Filesystem stores keep their directories and files owner-private (new directories
0700, files 0600) and refuse an existing unsafe location rather than repairing it;
see <doc:Stores>.

``InstallationTarget`` names an installation by its ``StateStore`` and context.
Internal index observation validates closed records and installation context
without creating or promoting an index. Admission does not publish resources,
write transaction journals, recover interrupted transactions or select lifecycle
policy.

`cd swift && just check-integration` exercises Swift/Python/Rust authority
orderings, alias lock contention, native revision fixtures and exact record/security
parity.
The reference `hold` drivers receive canonical final spelling for replay; Swift's
driver additionally accepts existing physical case aliases.
