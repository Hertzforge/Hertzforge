"""Which operations a selection fills.

`project.actions` answers with the action table (`action_table`), whose rows
come from here: of the service's operations on a project, the ones whose
object params the selected objects fill, every selected object used and every
required object param filled. Each comes with the params to fill from the
selection, so picking in the tree is picking the source, and the records
picked in a grid fill a `records` param as the shell numbers them.

Readings change how the selection is read (a plot, a comparison, a
check); acts change the project (a verb that adds an object, an edit).
The split follows the core: an operation named for one of the project's
journaled verbs, or one marked as an edit, is an act.
"""

from __future__ import annotations

from itertools import permutations
from typing import Any

from .descriptors import Operation, Param

#: operations that add to or change the project without being named for a
#: journaled verb
ADDS = frozenset((
    'project.add_matches', 'project.compute_psds', 'project.compute_frfs', 'project.fit_modes',
    'project.create_specification', 'project.edit_specification', 'project.add_breakpoint',
    'project.remove_breakpoint', 'project.scale_specification', 'project.set_band_edge',
    'project.set_breakpoint',
    'project.new_channel_table', 'project.new_geometry', 'project.add_channels',
    'project.add_node', 'project.add_coordinate_system', 'project.add_element',
    'project.add_element_group', 'project.delete_entities', 'project.set_entity',
    'project.set_entity_cell', 'project.set_channel_cell', 'project.set_sensor_model',
    'project.delete_matches', 'project.group_properties', 'project.merge_element_groups',
    'project.delete_element_groups', 'project.rotate_coordinate_system',
    'project.slide_coordinate_system', 'project.reset_rotation', 'project.merge_coincident_nodes',
    'project.mate_surfaces', 'project.edit_mated_surfaces', 'project.edit_mate_sides', 'project.delete_mated_surfaces',
    'project.set_shock', 'project.remove_shock', 'project.plan_instrumentation',
    'project.generate_signal', 'project.shock_synthesis', 'project.srs_synthesis',
    'project.derive_specification', 'project.predict', 'project.work_up_sysid', 'project.work_up_random',
    'project.work_up_shock', 'project.work_up_transient', 'project.work_up_sine',
    'project.work_up_modal',
))


def _object_params(op: Operation) -> list[Param]:
    """The params an object fills: `objects:` kinds or `verb:` verbs."""
    return [p for p in op.params if (p.options_from or '').startswith(('objects:', 'verb:'))]


def _takes(param: Param, kind: str, verbs: list[str]) -> bool:
    source = param.options_from or ''
    if source.startswith('objects:'):
        return kind in source.removeprefix('objects:').split('|')
    return source.removeprefix('verb:') in verbs


def _fill(op: Operation, chosen: list[tuple[str, str, list[str]]]) -> dict[str, str] | None:
    """The object params `chosen` [(name, kind, verbs)] fill, one object per
    param, every object placed and every required param filled; None when
    they cannot. A geometry selected beside data is the stage the data is
    shown on, as in Visual Dynamics, so one no param takes may stand by."""
    params = _object_params(op)
    required = [p for p in params if p.default != '']
    if len(chosen) <= len(params):
        for order in permutations(params, len(chosen)):
            if (all(_takes(p, kind, verbs) for p, (_n, kind, verbs) in zip(order, chosen, strict=True))
                    and all(p in order for p in required)):
                return {p.name: name for p, (name, _k, _v) in zip(order, chosen, strict=True)}
    stage = [c for c in chosen if c[1] == 'geometry']
    if len(stage) == 1 and len(chosen) > 1:
        return _fill(op, [c for c in chosen if c is not stage[0]])
    return None


def _is_act(op: Operation) -> bool:
    from hertzforge.project import _JOURNALED_VERBS

    verb = op.id.partition('.')[2]
    return bool(op.edits) or verb in _JOURNALED_VERBS or op.id in ADDS


def _runs(indices: list[int]) -> str:
    from .show import _runs

    return _runs(indices)



def actions(project: Any, objects: list[str], records: dict[str, list[int]],
            part: dict[str, Any] | None = None) -> dict[str, Any]:
    """What `objects` can take, as the action table beside the tree
    (`action_table`): `{'table': {'columns': [...]}}`."""
    from .action_table import action_table

    return {'table': action_table(project, objects, records, part)}
