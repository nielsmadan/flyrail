# Physical resource authority

A resource is one complete physical document, an owned tree, or a skill container.
Skills use the container as the authority and named subtrees as claims, keeping
state outside agent skill discovery. Bundle identity, agent name and caller root
do not create separate authorities for the same physical resource.

## Canonical paths and authority records

Relative inputs resolve at construction. Existing ancestors are checked by
metadata; symlinks, known reparse points, special files and hard-linked documents
are rejected. The nearest existing parent is normalized to its physical path.
macOS uses `F_GETPATH`; Windows path resolution uses its final-path API; Linux
resolves the existing absolute path. Only the resource's parent, or the first
existing parent of an absent suffix, needs sibling enumeration for portable-name
collision checks. There is no recursive project, home or volume scan.

An existing case alias must refer to the same filesystem identity and is replaced
with actual spelling. Missing suffix components retain requested spelling.

Authority, receipt, preparation and journal records for a resource live in
the host's [state store](state-store.md) under `resource/<destination-key>/*`,
not beside the resource; `destination-key` is `sha256(portable_path_key(destination))`,
rendered as lowercase hexadecimal, of the canonical destination in slash form.
Admission writes the `resource/<destination-key>/authority` record with
compare-and-set under the resource's store lock, validating or initializing its
exact canonical destination and kind. Ancestor and descendant conflicts are
checked against authority records **in the same store**, plus `.flyrail-*.txn`
transaction directories found inside an existing tree. A transaction stages in
the store's staging directory when that directory is on the destination's
mount (same device id, and on Linux the same mount ID) and otherwise in a
fallback `.flyrail-*.txn`
directory beside the destination, whose name is derived from the basename and a
transaction ID; the choice is made per transaction and recorded in its
preparation. See [receipt format](receipt-format.md) for both layouts.
Management names are reserved under the same portable folding: `.flyrail-*.txn`
cannot appear anywhere in requested or resulting physical destinations. Tree hierarchy probes recognize these case
aliases as fences too.

Fences apply within one store only. Content recorded in a different store is
foreign to flyrail, which does not recognize its authority records as fences.
Applications sharing one store share its receipts and fences.

An existing resource must have the requested physical kind. A document claim
cannot acquire a directory, including through explicit takeover. Kind validation
precedes acquisition so a file declaration cannot bypass descendant fences.

## Admission and fences

Existence of the authority record reserves the boundary. Empty receipts,
interrupted initialization and removed-installation state remain fences. Mutation
performs these steps in this order:

1. Write the selected resource's own authority record under its store lock,
   compare-and-set if absent or validated if already present. The record is
   retained permanently: admission never deletes it.
2. Probe each explicit ancestor's authority record. A different ancestor's
   record conflicts. For a tree or container, scan the store for every
   descendant's authority record and every `.flyrail-*.txn` transaction
   directory inside that explicit tree; any such record or directory conflicts.
3. Reobserve the resource and all resource metadata, then plan/publish under the
   resource lock. An earlier read-only preview cannot authorize skipping admission.

The ordering is essential. Let tree attempt T and child attempt C each write
their own record before checking for a conflict. If T checks before C writes,
T's record already exists when C later checks, so C fails. The reverse order
makes T fail. If both write before either checks, both can fail. They cannot
both pass. Retaining records permanently prevents a losing contender from
deleting the winner's future exclusion evidence. Process barrier tests must
cover these orderings when implementing admission.

Tree/child conflicts remain unsupported after removal; empty receipts never silently
change ownership boundaries. This deliberately trades automatic boundary reuse
for simple, stable cooperation between independent processes sharing a store. An
application can choose disjoint routes before first installation. A project
inside the user home works when its actual resources are disjoint from user
resources. Distinct sections or structured claims in the same document share
one authority and can coexist.

A failed child admission keeps excluding the enclosing tree, and a failed tree
admission keeps excluding every descendant it already reserved, because nothing
ever deletes an authority record. Clearing one means editing the store directly;
there is no visible directory to remove.

Read-only canonicalization and hierarchy inspection create no record and take no
lock. A missing authority is absence, not permission to acquire without admission.
Parent creation during mutation needs provenance and revalidation under the
resource protocol. A resource copied or moved by some other means to a different
canonical destination does not automatically transfer its authority there: stale
index/header/claim identities must be reported, rather than interpreted as a new
acquisition.

## Claim identity and overlap

Claim kinds are whole `file`, whole `tree`, container-relative `subtree`, `section`
(actual boundary pair), and `structured` (typed key/member selector). Whole claims
overlap every claim in the resource. Subtrees use portable path-prefix overlap.
Sections sharing either boundary conflict; the editor also checks actual spans.
Structured ancestor/descendant selections overlap. Different object keys and
different identities using the same member key are disjoint; selectors that could
match the same member are conservatively conflicting. Cross-kind partial claims
within one resource conflict.

Claim identity is SHA-256 of `flyrail-claim-v1` plus NUL followed by semantic bytes
of `[canonical_destination, ownership_kind, selector]`. Selector is null for a
whole resource, a relative string for a subtree, `[start,end]` for a section, or an
array of `["key",name]` / `["member",key_path,semantic_identity_record]` components.
Artifact ID is attribution. A rename on the same claim retains its original
baseline. A new destination or selector retires the previous claim and acquires a
new one. Physical-parent case aliases must produce the same canonical destination
and identity, including when the final document is absent.

`fixtures/configurations.json` pins claim identities for every ownership kind,
typed array-member identities, overlap results, and destination keys and
transaction paths for a fixed transaction ID, including case-folded Unicode
names and relocation. Its
`/fixture/...` destinations are synthetic canonical POSIX paths, independent
of a host filesystem. A consumer can stub canonicalization to those paths
while exercising its real identity and transaction path functions; platform tests
separately exercise physical canonicalization.
