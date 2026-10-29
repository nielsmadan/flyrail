# Native authority and observation

Flyrail's Swift filesystem layer supports macOS 13 and later. Construct
``ResourceAuthority`` with an explicit ``ResourcePath`` to discover its physical
destination and adjacent state path. Discovery is read-only: it creates no
directory, opens no lock and does not read agent configuration.

Paths retain native bytes. Managed destinations must use portable NFC UTF-8
names and cannot enter `.flyrail-*.state` storage. Existing case aliases must
identify the same node; absent components retain their requested spelling.
Only the selected resource's parent, or nearest existing parent, is enumerated
for canonicalization. Ancestor traversal uses search-only descriptors.

All managed-resource I/O is descriptor-relative and no-follow. Snapshots bind
every byte, ordered tree node, mode, owner/group, device and inode. Native
observation also retains link counts and modification/creation evidence to
detect changes during inspection and later replacement. Ancestor evidence binds
identity and security without treating unrelated sibling changes as replacement.

Ancestor ACL text, flags and raw extended attributes use the shared compact JSON
security framing. Resource flags, extended ACLs, foreign extended attributes,
special modes, symlinks, hard-linked documents and special files refuse
management. The opaque `com.apple.provenance` attribute is captured as evidence.

Package-internal admission reserves a permanent fence, holds its `flock` lock,
validates the exact authority header, probes hierarchy conflicts and reobserves
the resource. State directories and payload/metadata files must be owner-private;
existing unsafe permissions are never repaired. New directories use mode 0700
and files use 0600. For Python interoperability an empty owner-controlled permanent
lock may have read-only group/other bits, but never foreign write access. Its
containing directory must still be private. No payload is accepted in that lock.

``InstallationTarget`` canonicalizes its explicit index location. Internal index
observation validates closed records and installation context without creating
or promoting an index. Admission does not publish resources, write transaction
journals, recover interrupted transactions or select lifecycle policy.

`cd swift && just check-integration` exercises Swift/Python/Rust fence orderings,
alias lock contention, native revision fixtures and exact record/security parity.
The reference `hold` drivers receive canonical final spelling for replay; Swift's
driver additionally accepts existing physical case aliases.
