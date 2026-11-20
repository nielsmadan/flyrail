from collections.abc import Iterable, Mapping
from dataclasses import dataclass, field, replace
from enum import StrEnum
from pathlib import Path

from flyrail._loadout_manifest import (
    HARNESS_NAMES,
    Inputs,
    LoadoutDiagnostic,
    LoadoutFailure,
    LoadoutHarness,
    LoadoutSelector,
    LoadoutSource,
    Manifest,
    absolute,
    load_manifest,
    profile_file,
    source_path,
    strings,
)
from flyrail._loadout_render import (
    LoadoutImpact,
    SourceRender,
    _id,
    hooks_supported,
    required_slices,
)
from flyrail._observation import ObservationFailure
from flyrail._read_preconditions import ReadPrecondition
from flyrail.bundle import Bundle
from flyrail.configuration import (
    InstallationObservation,
    InstallationResult,
    InstallationTarget,
    LifecyclePreview,
)
from flyrail.editors import Acquisition
from flyrail.hooks import HookRuntime
from flyrail.lifecycle import (
    apply_preview,
    inspect_installation,
    preview,
    preview_removal,
    recover_installation,
)
from flyrail.models import OperationStatus
from flyrail.observations import ErrorCode, TargetError
from flyrail.rendered import Notice, NoticeKind, RenderedBundle
from flyrail.stores import StateStore
from flyrail.targets import Agent, Platform

__all__ = [
    "LoadoutDestination",
    "LoadoutDiagnostic",
    "LoadoutDiscovery",
    "LoadoutHarness",
    "LoadoutImpact",
    "LoadoutLocation",
    "LoadoutPreview",
    "LoadoutResult",
    "LoadoutSelector",
    "LoadoutSetup",
    "LoadoutSource",
    "LoadoutStatus",
    "apply_loadout",
    "discover_loadout",
    "inspect_loadout",
    "preview_loadout",
    "preview_loadout_removal",
    "recover_loadout",
]


class LoadoutStatus(StrEnum):
    ABSENT = "absent"
    AVAILABLE = "available"
    INVALID = "invalid"


@dataclass(frozen=True, slots=True)
class LoadoutLocation:
    root: Path
    profile: str
    source_name: str
    source_path: Path

    def __post_init__(self) -> None:
        for path in (self.root, self.source_path):
            absolute(path, "recorded Loadout location")
        try:
            profile_file(self.root, self.profile)
        except LoadoutFailure as error:
            raise ValueError(str(error)) from error
        if not self.source_name or any(char in self.source_name for char in "/\\\0"):
            raise ValueError("source_name must be one Loadout source name")

    def installation(self, store: StateStore) -> InstallationTarget:
        return InstallationTarget(
            store,
            (
                ("destination", "loadout"),
                ("root", str(self.root)),
                ("profile", self.profile),
                ("source-name", self.source_name),
                ("source-path", str(self.source_path)),
            ),
        )


@dataclass(frozen=True, slots=True)
class LoadoutSetup:
    registry: Path
    home: Path
    _manifest: Manifest = field(repr=False)
    _preconditions: tuple[ReadPrecondition, ...] = field(repr=False)

    @property
    def root(self) -> Path:
        return self._manifest.root

    @property
    def profile(self) -> str:
        return self._manifest.profile

    @property
    def profile_chain(self) -> tuple[Path, ...]:
        return self._manifest.chain

    @property
    def sources(self) -> tuple[LoadoutSource, ...]:
        return self._manifest.sources

    @property
    def harnesses(self) -> tuple[LoadoutHarness, ...]:
        return self._manifest.harnesses

    @property
    def selectors(self) -> tuple[LoadoutSelector, ...]:
        return self._manifest.selectors

    def eligible_sources(
        self, bundle: Bundle, harnesses: Iterable[Agent]
    ) -> tuple[LoadoutSource, ...]:
        requested = _harnesses(harnesses)
        if not hooks_supported(bundle, requested):
            return ()
        required = {
            item for artifact in bundle.artifacts for item in required_slices(artifact, requested)
        }
        enabled = {item.agent: item.slices for item in self.harnesses}
        if any(
            agent not in requested
            or category not in enabled.get(agent, ())
            or (str(agent), category) in self._manifest.custom_routes
            for agent, category in required
        ):
            return ()
        return tuple(
            source
            for source in self.sources
            if all(category in source.use for _, category in required)
        )

    def select(
        self,
        source_name: str,
        harnesses: Iterable[Agent],
        *,
        accept_shared_scope: bool = False,
    ) -> "LoadoutDestination":
        source = next((item for item in self.sources if item.name == source_name), None)
        if source is None:
            raise ValueError(
                f"Select a declared Loadout source; there is no source named {source_name!r}"
            )
        return LoadoutDestination(self, source, _harnesses(harnesses), accept_shared_scope)


def _harnesses(values: Iterable[Agent]) -> tuple[Agent, ...]:
    selected = tuple(values)
    if (
        not selected
        or any(not isinstance(value, Agent) for value in selected)
        or len(set(selected)) != len(selected)
    ):
        raise ValueError("Select one or more distinct harnesses as Agent values")
    return tuple(sorted(selected))


@dataclass(frozen=True, slots=True)
class LoadoutDiscovery:
    status: LoadoutStatus
    registry: Path
    setup: LoadoutSetup | None = None
    diagnostic: LoadoutDiagnostic | None = None


@dataclass(frozen=True, slots=True)
class LoadoutDestination:
    setup: LoadoutSetup
    source: LoadoutSource
    harnesses: tuple[Agent, ...]
    accept_shared_scope: bool = False

    def __post_init__(self) -> None:
        if self.source not in self.setup.sources or type(self.accept_shared_scope) is not bool:
            raise ValueError(
                "A Loadout destination requires a declared source and explicit scope policy"
            )
        object.__setattr__(self, "harnesses", _harnesses(self.harnesses))

    @property
    def location(self) -> LoadoutLocation:
        return LoadoutLocation(
            self.setup.root, self.setup.profile, self.source.name, self.source.path
        )


@dataclass(frozen=True, slots=True)
class LoadoutPreview:
    location: LoadoutLocation
    lifecycle: LifecyclePreview
    impact: tuple[LoadoutImpact, ...] = ()
    diagnostic: LoadoutDiagnostic | None = None

    @property
    def applicable(self) -> bool:
        return self.lifecycle.applicable

    @property
    def scope_notice(self) -> str:
        return "Other profiles and projects using this source may also receive its content."


@dataclass(frozen=True, slots=True)
class LoadoutResult:
    location: LoadoutLocation
    lifecycle: InstallationResult

    @property
    def sync_required(self) -> bool:
        return self.lifecycle.status in {
            OperationStatus.APPLIED,
            OperationStatus.UNCHANGED,
            OperationStatus.PARTIAL,
            OperationStatus.INCOMPLETE,
        }


def discover_loadout(
    home: Path,
    *,
    config_home: Path | None = None,
    env: Mapping[str, str] | None = None,
) -> LoadoutDiscovery:
    home = absolute(home, "home")
    xdg = None if env is None else env.get("XDG_CONFIG_HOME")
    base = config_home if config_home is not None else Path(xdg) if xdg else home / ".config"
    registry = absolute(base, "Loadout configuration directory") / "loadout" / "config.toml"
    inputs = Inputs()
    try:
        table = inputs.table(registry, "invalid-registry", optional=True)
        if table is None:
            return LoadoutDiscovery(LoadoutStatus.ABSENT, registry)
        if set(table) - {"source", "profile", "harnesses"}:
            raise LoadoutFailure(
                "invalid-registry", "Loadout registration has unknown keys", registry
            )
        source, profile = table.get("source"), table.get("profile", "default")
        if not isinstance(source, str) or not source or not isinstance(profile, str) or not profile:
            raise LoadoutFailure(
                "invalid-registry",
                "Registration source and profile must be nonempty strings",
                registry,
            )
        if "harnesses" in table:
            names = strings(table["harnesses"], registry, "harnesses", "invalid-registry")
            if not names or len(names) != len(set(names)) or set(names) - HARNESS_NAMES:
                raise LoadoutFailure(
                    "invalid-registry",
                    "Registration harnesses must be unique, known harness names",
                    registry,
                )
        try:
            source_root = source_path(source, home)
        except ValueError as error:
            raise LoadoutFailure("invalid-registry", str(error), registry) from error
        root = inputs.directory(source_root)
        manifest = load_manifest(root, profile, home, inputs)
        for condition in inputs.preconditions:
            condition.require()
    except LoadoutFailure as error:
        return LoadoutDiscovery(LoadoutStatus.INVALID, registry, diagnostic=error.diagnostic)
    except ObservationFailure as error:
        diagnostic = LoadoutDiagnostic(
            "concurrent-change", str(error), error.error.path or registry
        )
        return LoadoutDiscovery(LoadoutStatus.INVALID, registry, diagnostic=diagnostic)
    setup = LoadoutSetup(registry, home, manifest, inputs.preconditions)
    return LoadoutDiscovery(LoadoutStatus.AVAILABLE, registry, setup)


def _sync_notice(bundle_id: str) -> Notice:
    return Notice(
        bundle_id,
        NoticeKind.ACTIVATION,
        "Run Loadout sync to reflect these source changes. An update or removal can retire "
        "assets still referenced by generated hooks and MCP servers.",
        "loadout-sync-required",
    )


def _scope_prefix(target: InstallationTarget) -> str:
    return target.context_digest[:32] + "-"


def _scope_ownership(plan: LifecyclePreview) -> tuple[LifecyclePreview, LoadoutDiagnostic | None]:
    prefix = _scope_prefix(plan.target)
    for resource in plan.resources:
        if resource.previous is None:
            continue
        if any(
            claim.bundle_id == plan.bundle_id and not claim.artifact_id.startswith(prefix)
            for claim in resource.previous.claims
        ):
            path = resource.resource.destination
            message = "This bundle already owns the resource through another installation scope"
            diagnostic = LoadoutDiagnostic("source-ownership", message, path)
            return replace(plan, error=TargetError(ErrorCode.CONFLICT, message, path)), diagnostic
    return plan, None


def preview_loadout(
    bundle: Bundle,
    destination: LoadoutDestination,
    store: StateStore,
    *,
    platform: Platform,
    hook_runtime: HookRuntime | None = None,
    execution_root: Path | None = None,
    replace_modified: bool = False,
) -> LoadoutPreview:
    target = destination.location.installation(store)
    inputs = Inputs()
    rendering = SourceRender(
        bundle,
        destination.setup._manifest,
        destination.source,
        destination.harnesses,
        destination.accept_shared_scope,
        destination.setup.home,
        platform,
        inputs,
        hook_runtime,
        execution_root,
    )
    diagnostic = None
    try:
        conditions = destination.setup._preconditions
        for condition in conditions:
            condition.require()
        source = rendering.render()
        prefix = _scope_prefix(target)
        ids = {item.id: prefix + _id("loadout", item.id)[:31] for item in source.artifacts}
        rendered = RenderedBundle(
            (replace(item, id=ids[item.id]) for item in source.artifacts),
            (
                replace(edge, dependent=ids[edge.dependent], required=ids[edge.required])
                for edge in source.dependencies
            ),
            (*source.notices, _sync_notice(bundle.id)),
        )
        lifecycle = preview(bundle, rendered, target, replace_modified=replace_modified)
        conditions += inputs.preconditions
        for condition in conditions:
            condition.require()
        lifecycle, diagnostic = _scope_ownership(replace(lifecycle, _read_preconditions=conditions))
    except (LoadoutFailure, ObservationFailure, OSError, ValueError, RuntimeError) as error:
        if isinstance(error, LoadoutFailure):
            diagnostic = error.diagnostic
            detail = TargetError(ErrorCode.UNSUPPORTED, diagnostic.message, diagnostic.path)
        elif isinstance(error, ObservationFailure):
            detail = error.error
            diagnostic = LoadoutDiagnostic(
                "concurrent-change", str(error), detail.path or destination.setup.registry
            )
        else:
            diagnostic = LoadoutDiagnostic("source-plan", str(error), destination.source.path)
            detail = TargetError(ErrorCode.INVALID_STATE, diagnostic.message, diagnostic.path)
        lifecycle = LifecyclePreview(
            bundle.id,
            bundle,
            RenderedBundle(()),
            target,
            Acquisition.CONFLICT,
            replace_modified,
            None,
            None,
            (),
            detail,
        )
    return LoadoutPreview(
        destination.location, lifecycle, tuple(dict.fromkeys(rendering.impacts)), diagnostic
    )


def preview_loadout_removal(
    bundle_id: str,
    location: LoadoutLocation,
    store: StateStore,
    *,
    replace_modified: bool = False,
) -> LoadoutPreview:
    existing = preview_removal(
        bundle_id, location.installation(store), replace_modified=replace_modified
    )
    lifecycle, diagnostic = _scope_ownership(
        replace(existing, rendered=RenderedBundle((), notices=(_sync_notice(bundle_id),)))
    )
    return LoadoutPreview(location, lifecycle, diagnostic=diagnostic)


def apply_loadout(plan: LoadoutPreview, *, lock_timeout: float = 0) -> LoadoutResult:
    lifecycle = plan.lifecycle
    if not lifecycle.applicable:
        error = lifecycle.error or next(
            (item.error for item in lifecycle.resources if item.error), None
        )
        error = error or TargetError(
            ErrorCode.RECOVERY_NEEDED, "Recover the Loadout source installation and preview again"
        )
        return LoadoutResult(
            plan.location,
            InstallationResult(
                OperationStatus.FAILED,
                inspect_installation(lifecycle.bundle_id, lifecycle.target),
                error=error,
            ),
        )
    return LoadoutResult(plan.location, apply_preview(lifecycle, lock_timeout=lock_timeout))


def inspect_loadout(
    bundle_id: str, location: LoadoutLocation, store: StateStore
) -> InstallationObservation:
    return inspect_installation(bundle_id, location.installation(store))


def recover_loadout(
    bundle_id: str,
    location: LoadoutLocation,
    store: StateStore,
    *,
    lock_timeout: float = 0,
) -> LoadoutResult:
    return LoadoutResult(
        location,
        recover_installation(bundle_id, location.installation(store), lock_timeout=lock_timeout),
    )
