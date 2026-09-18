# Physical resource authority

A resource is one complete physical document, an owned tree, or a skill container.
Skills use the container as the authority and named subtrees as claims, keeping
state outside agent skill discovery. Bundle identity, agent name and caller root
do not create separate authorities for the same physical resource.

## Canonical paths and adjacent state

Relative inputs resolve at construction. Existing ancestors are checked by
metadata; symlinks, known reparse points, special files and hard-linked documents
are rejected. The nearest existing parent is normalized to its physical path.
macOS uses `F_GETPATH`; Windows path resolution uses its final-path API; Linux
resolves the existing absolute path. Only the resource's parent, or the first
existing parent of an absent suffix, needs sibling enumeration for portable-name
collision checks. There is no recursive project, home or volume scan.

An existing case alias must refer to the same filesystem identity and is replaced
with actual spelling. Missing suffix components retain requested spelling. The
state directory beside destination `P/N` is:

```text
P/.flyrail-<sha256(UTF-8(NFC(full-casefold(N)))))>.state/
    authority.json
    lock
    receipt.json
    transaction.json
    staging/
    backup/
    baselines/
```

The digest portion is 64 lowercase hexadecimal characters. The fixed size avoids
filesystem basename limits; folding makes absent case aliases locate one fence.
`authority.json` records exact canonical destination spelling and resource kind.
Different physical spellings on a case-sensitive filesystem cannot reuse a fence.
Management names are reserved under the same portable folding and cannot appear
anywhere in requested or resulting physical destinations. Tree hierarchy probes
recognize these case aliases as fences too.

`lock` is a permanent regular advisory-lock file. Receipts, transaction journal,
staged revisions, backups and first-acquisition baselines belong to this resource,
on its volume. A whole resource has one commit boundary across all bundles and
agents. A logical installation index can reference it but cannot grant ownership.

An existing resource must have the requested physical kind. A document claim
cannot acquire a directory, including through explicit takeover. Kind validation
precedes acquisition so a file declaration cannot bypass descendant fences.

## Permanent admission fences

Existence of the authority directory reserves the boundary. Empty directories,
interrupted initialization and removed-installation state remain fences. Mutation
performs these steps in this order:

1. Create the selected resource's own authority directory, retaining it permanently.
2. Acquire its permanent lock; validate or initialize its exact resource identity.
3. Probe each explicit ancestor's sibling authority path. A different ancestor
   fence conflicts. For a tree/container, inspect that explicit tree for descendant
   fences; any such fence conflicts.
4. Reobserve the resource and all resource metadata, then plan/publish under the
   resource lock. An earlier read-only preview cannot authorize skipping admission.

The ordering is essential. Let tree attempt T and child attempt C each reserve
before validating. If T validates before C reserves, T's fence already exists
when C later validates, so C fails. The reverse order makes T fail. If both reserve
before either validates, both can fail. They cannot both pass. Retaining fences
prevents a losing contender from deleting the winner's future exclusion evidence.
Process barrier tests must cover these orderings when implementing admission.

Tree/child conflicts remain unsupported after removal; empty receipts never silently
change ownership boundaries. This deliberately trades automatic boundary reuse
for simple, stable cooperation between independent processes. An application can
choose disjoint routes before first installation. A project inside the user home
works when its actual resources are disjoint from user resources. Distinct sections
or structured claims in the same document share one authority and can coexist.

Read-only canonicalization and hierarchy inspection create no path and take no
lock. A missing authority is absence, not permission to acquire without admission.
Parent creation during mutation needs provenance and revalidation under the
resource protocol. A fence copied or moved to a different canonical destination
does not automatically transfer authority: stale index/header/claim identities
must be reported, rather than interpreted as a new acquisition.

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
typed array-member identities, overlap results, and sibling state paths including
case-folded Unicode names and relocation. Its `/fixture/...` destinations are
synthetic canonical POSIX paths, independent of a host filesystem. A consumer can
stub canonicalization to those paths while exercising its real identity and state
path functions; platform tests separately exercise physical canonicalization.
