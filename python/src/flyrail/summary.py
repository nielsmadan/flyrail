from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path

from flyrail._resource_models import ResourceKind
from flyrail.artifacts import Family
from flyrail.configuration import LifecyclePreview, ResourcePlan
from flyrail.observations import TargetError
from flyrail.rendered import Notice
from flyrail.targets import Agent, Surface, TargetScope


class ChangeAction(StrEnum):
    CREATE = "create"
    UPDATE = "update"
    DELETE = "delete"
    UNCHANGED = "unchanged"
    CONFLICT = "conflict"


@dataclass(frozen=True, slots=True)
class PlannedChange:
    artifact_id: str | None
    family: Family | None
    kind: ResourceKind
    destination: Path
    action: ChangeAction

    def __post_init__(self) -> None:
        if not isinstance(self.kind, ResourceKind) or not isinstance(self.action, ChangeAction):
            raise TypeError("planned changes require a ResourceKind and ChangeAction")
        if self.family is not None and not isinstance(self.family, Family):
            raise TypeError("planned change family must be a Family")
        if not isinstance(self.destination, Path) or not self.destination.is_absolute():
            raise ValueError("planned change destination must be an absolute Path")


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
        if type(self.applicable) is not bool:
            raise TypeError("plan summary applicability must be a bool")
        if any(type(change) is not PlannedChange for change in self.changes):
            raise TypeError("plan summary changes must be PlannedChange values")


def _action(plan: ResourcePlan) -> ChangeAction:
    if plan.error is not None:
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
    artifacts = {artifact.destination: artifact for artifact in preview.rendered.artifacts}
    changes = []
    for plan in preview.resources:
        artifact = artifacts.get(plan.resource.destination)
        changes.append(
            PlannedChange(
                None if artifact is None else artifact.id,
                None if artifact is None else artifact.family,
                plan.resource.kind,
                plan.resource.destination,
                _action(plan),
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
