"""Which operations a selection fills.

`project.actions` answers with the action table (`action_table`), whose rows
come from here: of the service's operations on a project, the ones whose
object params the selected objects fill, every selected object used and every
required object param filled. Each comes with the params to fill from the
selection, so picking in the tree is picking the source, and the records
picked in a grid fill a `records` param as the shell numbers them.
"""

from __future__ import annotations

from collections.abc import Callable, Sequence
from itertools import permutations
from typing import Any

from .descriptors import Operation, Param


def _object_params(op: Operation) -> list[Param]:
    """The params an object fills: `objects:` kinds or `verb:` verbs."""
    return [p for p in op.params if p.kinds or p.verb]


def _placed(params: list[Param], items: Sequence[Any],
            takes: Callable[[Param, Any], bool]) -> tuple[Param, ...] | None:
    """The params to put `items` on, in their order: one item per param, each used, every
    required param filled. None when they cannot go."""
    required = [p for p in params if p.default != '']
    if len(items) > len(params):
        return None
    for order in permutations(params, len(items)):
        if (all(takes(p, item) for p, item in zip(order, items, strict=True))
                and all(p in order for p in required)):
            return order
    return None


def _takes(param: Param, kind: str, verbs: list[str]) -> bool:
    return param.verb in verbs if param.verb else kind in param.kinds


def _fill(op: Operation, chosen: list[tuple[str, str, list[str]]]) -> dict[str, str] | None:
    """The object params `chosen` [(name, kind, verbs)] fill; None when they cannot. A
    geometry selected beside data is the stage the data is shown on, so one no param takes
    may stand by. That is the one way this differs from `action_table._kinds_fit`, which
    asks of types alone, with no objects yet to stand by."""
    order = _placed(_object_params(op), chosen, lambda p, c: _takes(p, c[1], c[2]))
    if order is not None:
        return {p.name: c[0] for p, c in zip(order, chosen, strict=True)}
    stage = [c for c in chosen if c[1] == 'geometry']
    if len(stage) == 1 and len(chosen) > 1:
        return _fill(op, [c for c in chosen if c is not stage[0]])
    return None


def _runs(indices: list[int]) -> str:
    from .show import _runs

    return _runs(indices)



def actions(project: Any, objects: list[str], records: dict[str, list[int]],
            part: dict[str, Any] | None = None) -> dict[str, Any]:
    """What `objects` can take, as the action table beside the tree
    (`action_table`): `{'table': {'columns': [...]}}`."""
    from .action_table import action_table

    return {'table': action_table(project, objects, records, part)}
