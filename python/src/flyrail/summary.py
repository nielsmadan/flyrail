from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path

from flyrail._resource_models import ResourceKind, absolute, sequence
from flyrail.artifacts import Family
from flyrail.configuration import LifecyclePreview, ResourcePlan
from flyrail.observations import TargetError
from flyrail.rendered import Notice, RenderedArtifact
from flyrail.targets import Agent, Surface, TargetScope


class ChangeAction(StrEnum):
    CREATE = "create"
    UPDATE = "update"
    DELETE = "delete"
    UNCHANGED = "unchanged"
    CONFLICT = "conflict"


@dataclass(frozen=True, slots=True)
class PlannedChange:
    artifact_ids: tuple[str, ...]
    families: tuple[Family, ...]
    kind: ResourceKind
    destination: Path
    action: ChangeAction
    error: TargetError | None = None
    recovery_paths: tuple[Path, ...] = ()

    def __post_init__(self) -> None:
        if not isinstance(self.kind, ResourceKind) or not isinstance(self.action, ChangeAction):
            raise TypeError("planned changes require a ResourceKind and ChangeAction")
        object.__setattr__(self, "artifact_ids", sequence(self.artifact_ids, str))
        object.__setattr__(self, "families", tuple(self.families))
        if any(not isinstance(family, Family) for family in self.families):
            raise TypeError("planned change families must be Family values")
        if len(self.artifact_ids) != len(self.families):
            raise ValueError("planned change artifact ids and families must agree")
        if self.error is not None and type(self.error) is not TargetError:
            raise TypeError("planned change error must be a TargetError")
        if not isinstance(self.destination, Path) or not self.destination.is_absolute():
            raise ValueError("planned change destination must be an absolute Path")
        object.__setattr__(self, "recovery_paths", tuple(self.recovery_paths))
        for path in self.recovery_paths:
            absolute(path)


@dataclass(frozen=True, slots=True)
class PlanSummary:
    agent: Agent | None
    scope: TargetScope | None
    surface: Surface | None
    applicable: bool
    changes: tuple[PlannedChange, ...]
    notices: tuple[Notice, ...]
    error: TargetError | None

    def __post_init__(self) -> None:
        if (
            (self.agent is not None and not isinstance(self.agent, Agent))
            or (self.scope is not None and not isinstance(self.scope, TargetScope))
            or (self.surface is not None and not isinstance(self.surface, Surface))
        ):
            raise TypeError("plan summary routing must use Agent, TargetScope and Surface values")
        if type(self.applicable) is not bool:
            raise TypeError("plan summary applicability must be a bool")
        if self.error is not None and type(self.error) is not TargetError:
            raise TypeError("plan summary error must be a TargetError")
        object.__setattr__(self, "changes", tuple(self.changes))
        if any(type(change) is not PlannedChange for change in self.changes):
            raise TypeError("plan summary changes must be PlannedChange values")
        object.__setattr__(self, "notices", sequence(self.notices, Notice))


def _action(plan: ResourcePlan) -> ChangeAction:
    if plan.error is not None:
        return ChangeAction.CONFLICT
    if plan.recovery_paths:
        return ChangeAction.CONFLICT
    if plan.previous == plan.receipt and plan.before == plan.after:
        return ChangeAction.UNCHANGED
    if not plan.after.nodes:
        return ChangeAction.DELETE
    if not plan.before.nodes:
        return ChangeAction.CREATE
    return ChangeAction.UPDATE


def summarize(preview: LifecyclePreview) -> PlanSummary:
    if type(preview) is not LifecyclePreview:
        raise TypeError("summarize requires a LifecyclePreview")
    routing = dict(preview.target.routing_context)
    artifacts: dict[Path, list[RenderedArtifact]] = {}
    for artifact in preview.rendered.artifacts:
        artifacts.setdefault(artifact.destination, []).append(artifact)
    changes = []
    for plan in preview.resources:
        matched = sorted(artifacts.get(plan.resource.destination, ()), key=lambda item: item.id)
        changes.append(
            PlannedChange(
                tuple(artifact.id for artifact in matched),
                tuple(artifact.family for artifact in matched),
                plan.resource.kind,
                plan.resource.destination,
                _action(plan),
                plan.error,
                plan.recovery_paths,
            )
        )
    agent = routing.get("agent")
    scope = routing.get("scope")
    surface = routing.get("surface")
    return PlanSummary(
        Agent(agent) if agent else None,
        TargetScope(scope) if scope else None,
        Surface(surface) if surface else None,
        preview.applicable,
        tuple(changes),
        preview.rendered.notices,
        preview.error,
    )
