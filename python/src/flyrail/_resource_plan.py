import os
import sys
import uuid
from dataclasses import replace
from pathlib import Path

from flyrail._resource_io import ancestors, failure, observe, read, validate_state
from flyrail._resource_models import (
    Node,
    OwnedClaim,
    Receipt,
    ResourceKind,
    ResourceRef,
    Revision,
)
from flyrail._schema import edit_schema_document, matches
from flyrail.authority import Claim, Ownership
from flyrail.bundle import Bundle
from flyrail.configuration import ClaimObservation, ResourceObservation, ResourcePlan
from flyrail.content import FileContent, FileMode, SectionContent, StructuredContent, TreeContent
from flyrail.editors import (
    Acquisition,
    Disposition,
    DocumentRequest,
    EditStatus,
    OwnedSelection,
    _observe_selections,
    content_claim,
)
from flyrail.observations import ErrorCode, TargetError
from flyrail.rendered import RenderedArtifact, RenderedBundle


def artifact_claim(artifact: RenderedArtifact) -> Claim:
    content = artifact.content
    if artifact.subtree is not None:
        return Claim(Ownership.SUBTREE, artifact.subtree)
    if isinstance(content, FileContent):
        return Claim(Ownership.FILE)
    if isinstance(content, TreeContent):
        return Claim(Ownership.TREE)
    return content_claim(content)


def resource(artifact: RenderedArtifact) -> ResourceRef:
    from flyrail.authority import ResourceAuthority

    authority = ResourceAuthority(artifact.destination)
    kind = (
        ResourceKind.SKILL_CONTAINER
        if artifact.subtree is not None
        else (
            ResourceKind.TREE
            if isinstance(artifact.content, TreeContent)
            else ResourceKind.DOCUMENT
        )
    )
    return ResourceRef(authority.destination, kind, authority.state_root)


def selected(revision: Revision, claim: Claim) -> Revision:
    if claim.kind is not Ownership.SUBTREE:
        return revision
    if not isinstance(claim.selector, str):
        raise ValueError("subtree selector must be a path")
    prefix = claim.selector + "/"
    return Revision(
        tuple(
            replace(node, path=node.path[len(prefix) :] if node.path != claim.selector else "")
            for node in revision.nodes
            if node.path == claim.selector or node.path.startswith(prefix)
        )
    )


def replace_selection(revision: Revision, claim: Claim, content: Revision) -> Revision:
    if claim.kind is not Ownership.SUBTREE:
        return content
    if not isinstance(claim.selector, str):
        raise ValueError("subtree selector must be a path")
    prefix = claim.selector + "/"
    nodes = [
        node
        for node in revision.nodes
        if node.path != claim.selector and not node.path.startswith(prefix)
    ]
    if content.nodes and not nodes:
        nodes.append(new_node("", None, 0o755))
    nodes.extend(
        replace(node, path=prefix + node.path if node.path else claim.selector)
        for node in content.nodes
    )
    if content.nodes:
        existing = {node.path for node in nodes}
        parts = claim.selector.split("/")
        for index in range(1, len(parts)):
            parent = "/".join(parts[:index])
            if parent not in existing:
                nodes.append(new_node(parent, None, 0o755))
    return Revision(tuple(sorted(nodes, key=lambda node: node.path)))


def new_node(path: str, data: bytes | None, mode: int) -> Node:
    uid = gid = 0
    if sys.platform != "win32":
        uid, gid = os.getuid(), os.getgid()
    return Node(
        path,
        data,
        mode,
        uid,
        gid,
        0,
        0,
    )


def desired_revision(content: FileContent | TreeContent) -> Revision:
    if isinstance(content, FileContent):
        mode = {FileMode.PRIVATE: 0o600, FileMode.READABLE: 0o644, FileMode.EXECUTABLE: 0o755}[
            content.mode
        ]
        return Revision((new_node("", content.data, mode),))
    return Revision(
        (
            new_node("", None, 0o755),
            *(
                new_node(
                    entry.path,
                    entry.data,
                    0o755 if entry.data is None or entry.executable else 0o644,
                )
                for entry in content.entries
            ),
        )
    )


def intact(actual: Revision, expected: Revision) -> bool:
    def content(revision: Revision) -> tuple[tuple[str, bytes | None, int], ...]:
        return tuple(
            (node.path, node.data, node.mode if os.name != "nt" else bool(node.mode & 0o222))
            for node in revision.nodes
        )

    return content(actual) == content(expected)


def claim_observations(receipt: Receipt, revision: Revision) -> tuple[ClaimObservation, ...]:
    selections = tuple(owned.selection for owned in receipt.claims if owned.selection is not None)
    statuses: dict[Claim, EditStatus] = {}
    if selections:
        statuses.update(
            (item.claim, item.status) for item in _observe_selections(revision.data, selections)
        )
    for owned in receipt.claims:
        if owned.installed is not None:
            statuses[owned.claim] = (
                EditStatus.CURRENT
                if intact(selected(revision, owned.claim), owned.installed)
                else EditStatus.MODIFIED
            )
    for owned in receipt.claims:
        if any(not matches(revision.data, schema) for schema in owned.schema_requirements):
            statuses[owned.claim] = EditStatus.MODIFIED
    return tuple(
        ClaimObservation(
            owned.claim_id,
            owned.bundle_id,
            owned.artifact_id,
            owned.version,
            owned.bundle_digest,
            owned.render_digest,
            statuses[owned.claim],
        )
        for owned in receipt.claims
    )


def summary(ref: ResourceRef) -> ResourceObservation:
    recovery = validate_state(ref)
    receipt = read(ref.state_root / "receipt.json", Receipt)
    if receipt is not None and receipt.resource != ref:
        raise failure(ErrorCode.INVALID_STATE, "receipt resource disagrees", ref.state_root)
    current = observe(ref.destination)
    observations = () if receipt is None else claim_observations(receipt, current)
    return ResourceObservation(
        ref.destination,
        ref.state_root,
        observations,
        current.content_digest,
        recovery_paths=recovery,
    )


def _owned(
    artifact: RenderedArtifact,
    bundle: Bundle,
    rendered: RenderedBundle,
    ref: ResourceRef,
    requirements: tuple[tuple[Path, str], ...],
    stable_requirements: tuple[tuple[Path, str], ...],
    *,
    selection: OwnedSelection | None = None,
    installed: Revision | None = None,
    disposition: Disposition = Disposition.CREATED,
    baseline: Revision | None = None,
) -> OwnedClaim:
    claim = artifact_claim(artifact)
    return OwnedClaim(
        claim,
        claim.identity(ref.authority),
        bundle.id,
        artifact.id,
        bundle.version,
        bundle.content_digest,
        rendered.content_digest,
        selection,
        installed,
        selection.disposition if selection is not None else disposition,
        baseline,
        requirements,
        stable_requirements,
        artifact.schema_requirements,
    )


def container_provenance(
    before: Revision, after: Revision, previous: Receipt | None, claims: list[OwnedClaim]
) -> tuple[Revision, tuple[Node, ...]]:
    existing = {node.path: node for node in before.nodes}
    created = {
        node.path: node
        for node in (() if previous is None else previous.created_directories)
        if existing.get(node.path) == node
    }
    selectors = [str(item.claim.selector) for item in claims]
    nodes = {node.path: node for node in after.nodes}
    for path, node in nodes.items():
        if (
            node.data is None
            and path not in existing
            and any(not path or selector.startswith(path + "/") for selector in selectors)
        ):
            created[path] = node
    retained = []
    for path, marker in sorted(created.items(), reverse=True):
        if any(not path or selector.startswith(path + "/") for selector in selectors):
            retained.append(marker)
        elif not any(
            other != path and (not path or other.startswith(path + "/")) for other in nodes
        ):
            nodes.pop(path, None)
    return Revision(tuple(nodes[path] for path in sorted(nodes))), tuple(
        sorted(retained, key=lambda node: node.path)
    )


def plan_resource(
    ref: ResourceRef,
    identifier: str,
    bundle: Bundle | None,
    rendered: RenderedBundle,
    artifacts: tuple[RenderedArtifact, ...],
    acquisition: Acquisition,
    replace_modified: bool,
    requirements: dict[str, tuple[tuple[Path, str], ...]],
    stable_requirements: dict[str, tuple[tuple[Path, str], ...]],
) -> ResourcePlan:
    recovery = validate_state(ref)
    previous = read(ref.state_root / "receipt.json", Receipt)
    if previous is not None and previous.resource != ref:
        raise failure(ErrorCode.INVALID_STATE, "receipt resource disagrees", ref.state_root)
    before = observe(ref.destination)
    state_revision = observe(ref.state_root)
    claims = () if previous is None else previous.claims
    provenance = previous.provenance if previous else Receipt(ref, "0" * 32).provenance
    after = before
    error: TargetError | None = None
    own = {owned.claim: owned for owned in claims if owned.bundle_id == identifier}
    desired = {artifact_claim(artifact): artifact for artifact in artifacts}
    if len(desired) != len(artifacts):
        raise ValueError("rendered artifacts duplicate a physical claim")
    for claim in desired:
        for existing in claims:
            if claim.overlaps(existing.claim) and (
                existing.bundle_id != identifier or claim != existing.claim
            ):
                error = TargetError(
                    ErrorCode.CONFLICT, "claim overlaps recorded ownership", ref.destination
                )
    required = any(artifact.require_existing for artifact in artifacts)
    if required and not before.nodes:
        error = TargetError(ErrorCode.CONFLICT, "resource must already exist", ref.destination)
    if before.nodes and (before.data is not None) != (ref.kind is ResourceKind.DOCUMENT):
        error = TargetError(
            ErrorCode.UNSAFE_PATH,
            "resource kind disagrees with existing destination",
            ref.destination,
        )
    result = [owned for owned in claims if owned.bundle_id != identifier]
    active_schemas = tuple(
        schema for owned in result for schema in owned.schema_requirements
    ) + tuple(schema for artifact in artifacts for schema in artifact.schema_requirements)
    if ref.kind is ResourceKind.DOCUMENT and any(
        claim.kind in {Ownership.SECTION, Ownership.STRUCTURED} for claim in (*own, *desired)
    ):
        if before.nodes and before.nodes[0].data is None:
            error = TargetError(
                ErrorCode.UNSAFE_PATH, "shared document is not a file", ref.destination
            )
        selections = tuple(owned.selection for owned in claims if owned.selection is not None)
        requests = [DocumentRequest.retire(claim) for claim in own if claim not in desired]
        for artifact in artifacts:
            if not isinstance(artifact.content, SectionContent | StructuredContent):
                error = TargetError(
                    ErrorCode.CONFLICT,
                    "whole and partial claims cannot share a document",
                    ref.destination,
                )
                continue
            requests.append(DocumentRequest.set(artifact.content, acquisition=acquisition))
        if error is None:
            edit = edit_schema_document(
                before.data,
                requests,
                ownership=selections,
                provenance=provenance,
                active_schemas=active_schemas,
                validate=bool(artifacts),
            )
            if replace_modified and not edit.applicable:
                current = {
                    item.claim: item.current
                    for item in edit.observations
                    if item.status is EditStatus.MODIFIED and item.current is not None
                }
                selections = tuple(
                    replace(item, installed=current[item.claim]) if item.claim in current else item
                    for item in selections
                )
                edit = edit_schema_document(
                    before.data,
                    requests,
                    ownership=selections,
                    provenance=provenance,
                    active_schemas=active_schemas,
                    validate=bool(artifacts),
                )
            if not edit.applicable:
                issue = next(
                    item
                    for item in edit.observations
                    if item.status in {EditStatus.MODIFIED, EditStatus.CONFLICT}
                )
                error = TargetError(
                    ErrorCode.MODIFIED
                    if issue.status is EditStatus.MODIFIED
                    else ErrorCode.CONFLICT,
                    issue.message or "owned selection cannot be changed",
                    ref.destination,
                )
            else:
                after = (
                    Revision()
                    if edit.after is None
                    else Revision(
                        (
                            replace(before.nodes[0], data=edit.after)
                            if before.nodes
                            else new_node("", edit.after, 0o600),
                        )
                    )
                )
                provenance = edit.provenance
                by_claim = {item.claim: item for item in edit.ownership}
                for claim, artifact in desired.items():
                    if bundle is None:
                        raise ValueError("desired content requires a bundle")
                    result.append(
                        _owned(
                            artifact,
                            bundle,
                            rendered,
                            ref,
                            requirements[artifact.id],
                            stable_requirements[artifact.id],
                            selection=by_claim[claim],
                        )
                    )
    elif error is None:
        for claim in sorted(
            own.keys() | desired.keys(), key=lambda item: item.identity(ref.authority)
        ):
            old = own.get(claim)
            selected_artifact = desired.get(claim)
            actual = selected(before, claim)
            if (
                old is not None
                and old.installed is not None
                and not intact(actual, old.installed)
                and not replace_modified
            ):
                error = TargetError(
                    ErrorCode.MODIFIED, "owned resource has local modifications", ref.destination
                )
                break
            if selected_artifact is None:
                if old is None:
                    raise ValueError("retirement requires ownership")
                replacement = old.baseline or Revision()
            else:
                if bundle is None or not isinstance(
                    selected_artifact.content, FileContent | TreeContent
                ):
                    raise ValueError("whole resource requires file or tree content")
                replacement = desired_revision(selected_artifact.content)
                disposition = old.disposition if old else Disposition.CREATED
                baseline = old.baseline if old else None
                if old is None and actual.nodes:
                    if acquisition is Acquisition.CONFLICT:
                        error = TargetError(
                            ErrorCode.CONFLICT, "unowned content already exists", ref.destination
                        )
                        break
                    if acquisition is Acquisition.ADOPT:
                        if not intact(actual, replacement):
                            error = TargetError(
                                ErrorCode.CONFLICT,
                                "adoption requires matching content",
                                ref.destination,
                            )
                            break
                        disposition = Disposition.ADOPTED
                    else:
                        disposition, baseline = Disposition.TAKEN_OVER, actual
                result.append(
                    _owned(
                        selected_artifact,
                        bundle,
                        rendered,
                        ref,
                        requirements[selected_artifact.id],
                        stable_requirements[selected_artifact.id],
                        installed=replacement,
                        disposition=disposition,
                        baseline=baseline,
                    )
                )
            if not intact(actual, replacement):
                after = replace_selection(after, claim, replacement)
    created_directories = previous.created_directories if previous else ()
    if ref.kind is ResourceKind.SKILL_CONTAINER and error is None:
        after, created_directories = container_provenance(before, after, previous, result)
    receipt = (
        Receipt(
            ref,
            uuid.uuid4().hex,
            tuple(sorted(result, key=lambda item: item.claim_id)),
            provenance,
            created_directories,
        )
        if error is None
        else (previous or Receipt(ref, uuid.uuid4().hex))
    )
    if (
        previous is not None
        and replace(receipt, transaction_id=previous.transaction_id) == previous
    ):
        receipt = previous
    return ResourcePlan(
        ref,
        before,
        after if error is None else before,
        previous,
        receipt,
        ancestors(ref.state_root / "transaction.json"),
        state_revision,
        recovery,
        required,
        error,
    )
