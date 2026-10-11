from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path

from flyrail._resource_models import (
    Ancestor,
    Generation,
    Index,
    Receipt,
    ResourceRef,
    Revision,
    absolute,
    digest,
    hexadecimal,
    sequence,
)
from flyrail._validation import validate_identifier, validate_version
from flyrail.bundle import Bundle
from flyrail.editors import Acquisition, EditStatus
from flyrail.models import OperationStatus
from flyrail.observations import TargetError
from flyrail.rendered import Notice, RenderedBundle
from flyrail.stores import StateStore


@dataclass(frozen=True, slots=True, init=False)
class InstallationTarget:
    store: StateStore
    context: tuple[tuple[str, str], ...]
    routing_context: tuple[tuple[str, str], ...]

    def __init__(
        self,
        store: StateStore,
        context: Iterable[tuple[str, str]] = (),
        *,
        routing_context: Iterable[tuple[str, str]] = (),
    ) -> None:
        if not isinstance(store, StateStore):
            raise TypeError("store must be a StateStore")
        object.__setattr__(self, "store", store)
        for name, values in (("context", context), ("routing_context", routing_context)):
            pairs = tuple((key, value) for key, value in values)
            if any(not isinstance(key, str) or not isinstance(value, str) for key, value in pairs):
                raise TypeError("target context must contain string pairs")
            if len({key for key, _ in pairs}) != len(pairs):
                raise ValueError("target context keys must be unique")
            object.__setattr__(self, name, tuple(sorted(pairs)))

    @property
    def context_digest(self) -> str:
        return digest(self.context)


@dataclass(frozen=True, slots=True)
class ClaimObservation:
    claim_id: str
    bundle_id: str
    artifact_id: str
    version: str
    bundle_digest: str
    render_digest: str
    status: EditStatus

    def __post_init__(self) -> None:
        hexadecimal(self.claim_id)
        validate_identifier(self.bundle_id, "bundle id")
        validate_identifier(self.artifact_id, "artifact id")
        validate_version(self.version)
        hexadecimal(self.bundle_digest)
        hexadecimal(self.render_digest)
        if not isinstance(self.status, EditStatus):
            raise TypeError("claim status must be an EditStatus")


@dataclass(frozen=True, slots=True)
class ResourceObservation:
    destination: Path
    claims: tuple[ClaimObservation, ...] = ()
    revision_digest: str | None = None
    error: TargetError | None = None
    recovery_paths: tuple[Path, ...] = ()

    def __post_init__(self) -> None:
        absolute(self.destination)
        if self.revision_digest is not None:
            hexadecimal(self.revision_digest)
        if self.error is not None and type(self.error) is not TargetError:
            raise TypeError("resource error must be a TargetError")
        object.__setattr__(self, "claims", sequence(self.claims, ClaimObservation))
        object.__setattr__(self, "recovery_paths", tuple(self.recovery_paths))
        for path in self.recovery_paths:
            absolute(path)


@dataclass(frozen=True, slots=True)
class InstallationObservation:
    bundle_id: str
    target: InstallationTarget
    resources: tuple[ResourceObservation, ...]
    is_current: bool
    pending: bool
    version: str | None = None
    bundle_digest: str | None = None
    render_digest: str | None = None
    error: TargetError | None = None

    def __post_init__(self) -> None:
        validate_identifier(self.bundle_id, "bundle id")
        if (
            type(self.target) is not InstallationTarget
            or type(self.is_current) is not bool
            or type(self.pending) is not bool
        ):
            raise TypeError("invalid installation observation")
        if self.version is not None:
            validate_version(self.version)
        for value in (self.bundle_digest, self.render_digest):
            if value is not None:
                hexadecimal(value)
        if self.error is not None and type(self.error) is not TargetError:
            raise TypeError("installation error must be a TargetError")
        object.__setattr__(self, "resources", sequence(self.resources, ResourceObservation))

    def matches(self, bundle: Bundle, rendered: RenderedBundle) -> bool:
        if type(bundle) is not Bundle or type(rendered) is not RenderedBundle:
            raise TypeError("comparison requires a Bundle and RenderedBundle")
        return (
            self.is_current
            and not self.pending
            and self.error is None
            and rendered.supported
            and self.bundle_id == bundle.id
            and self.version == bundle.version
            and self.bundle_digest == bundle.content_digest
            and self.render_digest == rendered.content_digest
        )


@dataclass(frozen=True, slots=True)
class ResourceResult:
    resource: ResourceRef
    status: OperationStatus
    error: TargetError | None = None
    recovery_paths: tuple[Path, ...] = ()

    def __post_init__(self) -> None:
        if type(self.resource) is not ResourceRef or not isinstance(self.status, OperationStatus):
            raise TypeError("invalid resource result")
        if self.error is not None and type(self.error) is not TargetError:
            raise TypeError("resource error must be a TargetError")
        object.__setattr__(self, "recovery_paths", tuple(self.recovery_paths))
        for path in self.recovery_paths:
            absolute(path)


@dataclass(frozen=True, slots=True)
class InstallationResult:
    status: OperationStatus
    observation: InstallationObservation
    resources: tuple[ResourceResult, ...] = ()
    error: TargetError | None = None
    notices: tuple[Notice, ...] = ()

    def __post_init__(self) -> None:
        if type(self.observation) is not InstallationObservation or not isinstance(
            self.status, OperationStatus
        ):
            raise TypeError("invalid installation result")
        if self.error is not None and type(self.error) is not TargetError:
            raise TypeError("installation error must be a TargetError")
        object.__setattr__(self, "resources", sequence(self.resources, ResourceResult))
        object.__setattr__(self, "notices", sequence(self.notices, Notice))


@dataclass(frozen=True, slots=True)
class ResourcePlan:
    resource: ResourceRef
    before: Revision
    after: Revision
    previous: Receipt | None
    receipt: Receipt
    ancestors: tuple[Ancestor, ...]
    recovery_paths: tuple[Path, ...]
    require_existing: bool = False
    error: TargetError | None = None

    def __post_init__(self) -> None:
        if type(self.resource) is not ResourceRef or type(self.receipt) is not Receipt:
            raise TypeError("resource plans require a resource and receipt")
        if self.previous is not None and type(self.previous) is not Receipt:
            raise TypeError("resource plans require a previous receipt")
        if self.receipt.resource != self.resource or (
            self.previous is not None and self.previous.resource != self.resource
        ):
            raise ValueError("planned receipts disagree with resource")
        sequence((self.before, self.after), Revision)
        if type(self.require_existing) is not bool or (
            self.error is not None and type(self.error) is not TargetError
        ):
            raise TypeError("invalid resource plan precondition")
        object.__setattr__(self, "ancestors", sequence(self.ancestors, Ancestor))
        object.__setattr__(self, "recovery_paths", tuple(self.recovery_paths))
        for path in self.recovery_paths:
            absolute(path)


@dataclass(frozen=True, slots=True)
class LifecyclePreview:
    bundle_id: str
    bundle: Bundle | None
    rendered: RenderedBundle
    target: InstallationTarget
    acquisition: Acquisition
    replace_modified: bool
    index: Index | None
    generation: Generation | None
    resources: tuple[ResourcePlan, ...]
    error: TargetError | None = None

    def __post_init__(self) -> None:
        if self.bundle is not None and not isinstance(self.bundle, Bundle):
            raise TypeError("preview requires an immutable bundle")
        if self.bundle is not None and self.bundle.id != self.bundle_id:
            raise ValueError("preview bundle identity disagrees")
        if type(self.rendered) is not RenderedBundle or type(self.target) is not InstallationTarget:
            raise TypeError("preview requires immutable rendering and target")
        if not isinstance(self.acquisition, Acquisition) or type(self.replace_modified) is not bool:
            raise TypeError("invalid preview policy")
        if self.index is not None and type(self.index) is not Index:
            raise TypeError("preview requires an immutable index")
        if self.generation is not None and type(self.generation) is not Generation:
            raise TypeError("preview requires an immutable generation")
        if self.error is not None and type(self.error) is not TargetError:
            raise TypeError("invalid preview metadata")
        validate_identifier(self.bundle_id, "bundle id")
        object.__setattr__(self, "resources", sequence(self.resources, ResourcePlan))

    @property
    def applicable(self) -> bool:
        return self.error is None and all(
            not item.error and not item.recovery_paths for item in self.resources
        )
