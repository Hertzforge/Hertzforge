"""The action table beside the project tree (Brandon, 2026-11-06).

The first column is the selection: what it draws by itself (its defaults),
then what can run now, then what cannot and why. Each other column is the
selection with one more object: its header says the object types, each row
names the project's objects that would complete it, and a row with none is
off. A column's rows come from the same operations `project.actions` always
read, from the joint readings `show` draws, and from the acts on the
selection itself (rename, remove, link) that are not operations.

The shell filters by module and sorts; every row says which modules offer it.
"""

from __future__ import annotations

from collections.abc import Callable, Iterable
from typing import Any

from . import projects, verbs
from .actions import _fill, _object_params, _placed, _runs
from .descriptors import Operation, Param

#: what two objects draw together, the table of `show._pair`'s branches
#: (tests/service/test_action_table.py holds the two to each other)
JOINT: dict[tuple[str, str], tuple[str, ...]] = {
    ('geometry', 'geometry'): ('Overlay',),
    ('geometry', 'shape_set'): ('Animate',),
    ('channel_table', 'geometry'): ('Arrows',),
    ('geometry', 'time_history'): ('Animate',),
    ('geometry', 'psd'): ('Animate',),
    ('geometry', 'specification'): ('Animate',),
    ('frf', 'geometry'): ('Animate',),
    ('shape_set', 'shape_set'): ('MAC',),
    ('frf', 'shape_set'): ('Resynthesis',),
    ('psd', 'specification'): ('Compare',),
    ('shock_specification', 'srs'): ('Compare',),
    ('time_history', 'transient_specification'): ('Compare',),
    ('sine_level_set', 'sine_sweep_specification'): ('Compare',),
    ('psd', 'psd'): ('Overlay',),
}

#: data objects of one type drawn on one stage (`show._stacked`)
STACKED = frozenset(('time_history', 'frf', 'coherence', 'multiple_coherence', 'spectrum', 'srs',
                     'specification', 'shock_specification', 'transient_specification'))

#: objects with nothing of their own to draw (`show._alone`'s last answers)
NO_VIEW = frozenset(('report', 'run_log_extract', 'predictions'))

_HISTORY = ('time_history',)
_RECORDS = ('time_history', 'spectrum', 'frf', 'psd', 'coherence', 'multiple_coherence', 'srs',
            'specification')

#: a core verb's object types, and why one of them may still not take it
#: (`Project.verbs`'s predicates answer only yes or no). A reason of '' means
#: the type is all it takes.
VERB_WHEN: dict[str, tuple[tuple[str, ...], str]] = {
    'integrate': (_HISTORY, 'It holds no quantity that integrates'),
    'differentiate': (_HISTORY, 'It holds no quantity that differentiates'),
    'filter_data': (_HISTORY, ''),
    'truncate_data': (_HISTORY, ''),
    'resample': (_HISTORY, ''),
    'detect_shocks': (_HISTORY, ''),
    'compute_spectra': (_HISTORY, ''),
    'compute_psds': (_HISTORY, ''),
    'compute_cpsds': (_HISTORY, ''),
    'compute_srs': (_HISTORY, ''),
    'compute_frfs': (_HISTORY, 'Its channels need both references and responses'),
    'compute_multiple_coherence': (_HISTORY, 'Its channels need both references and responses'),
    'extract_sine': (_HISTORY, 'The project has no sine sweep specification'),
    'compute_octave': (('psd', 'specification'), ''),
    'make_realizable': (('psd', 'specification'), 'It needs cross terms'),
    'convert_to_specification': (('psd',), ''),
    'convert_to_psd': (('specification',), ''),
    'fit_modes': (('frf',), ''),
    'transform': (_RECORDS, 'No shape set in the project reads it as physical'),
    'expand': (_RECORDS, 'No shape set in the project reads it as modal'),
    'generate_rigid_body_modes': (('geometry',), ''),
    'generate_signal': (('specification', 'transient_specification',
                         'sine_sweep_specification', 'psd', 'srs', 'time_history'),
                        'No signal can be made from it'),
    'solve_modes': (('geometry',), 'Its element groups have no properties yet'),
    'plan_instrumentation': (('shape_set',), 'It needs a mode above 0 Hz on a geometry with elements'),
    'project_onto_basis': (('shape_set',), 'It needs a second shape set'),
    'synthesize_modal_frfs': (('shape_set',), 'It needs real modes'),
    'overload_check': (_HISTORY, 'No linked channel table gives its ranges'),
}

#: the operations on the project row that run at once, asking nothing (Brandon, 2026-11-10)
INSTANT = frozenset({'project.new_geometry', 'project.new_channel_table'})

#: the rows that read a geometry's nodes, off until it has some
NEEDS_NODES = frozenset(('project.merge_coincident_nodes', 'project.mate_surfaces'))

#: an object param that takes this many types takes anything: it says nothing
#: about what a pair means, so it does not make a column of its own
_ANY = 8

#: takes objects of one type whatever its params list (`core.merge`), so its pair is a column
_ONE_TYPE = frozenset(('project.merge',))

Chosen = tuple[str, str, list[str]]


def _computed_from_history(project: Any, name: str, verb: str) -> bool:
    from hertzforge.core.data import TimeHistory

    provenance = project.provenance.get(name, {})
    return (provenance.get('verb') == verb
            and isinstance(project.get(provenance.get('source')), TimeHistory))


def _frf_with_its_history(project: Any, name: str) -> str:
    if not _computed_from_history(project, name, 'compute_frfs'):
        return 'It was not computed from a time history in this project'
    references = {str(dof) for dof in project[name].reference_dof}
    return '' if len(references) == 1 else 'It was computed against more than one reference'


def _coherence_with_its_history(project: Any, name: str) -> str:
    return ('' if _computed_from_history(project, name, 'compute_multiple_coherence')
            else 'It was not computed from a time history in this project')


def _cross_terms(project: Any, name: str) -> str:
    from hertzforge.core.data import Psd

    data = project[name]
    return 'It needs cross terms' if isinstance(data, Psd) and data.reference_dof is None else ''


def _solids(project: Any, name: str) -> str:
    return '' if project[name].solids else 'It has no solids yet'


def _made_of(project: Any, geometry: str, ids: list[Any]) -> bool:
    """Whether one of these solids is made of others (a boolean or a pattern), so
    taking it apart leaves something."""
    from hertzforge.core import solids

    held = project[geometry]
    return any(solids.solid(held, int(i)).operands for i in ids)


def _shocks_to_unmark(project: Any, name: str) -> str:
    history = project[name]
    if history.split_into_frames:
        return 'The file already cut it into captures'
    return '' if history.shocks else 'It has no shocks marked'


def _span(project: Any, name: str) -> str:
    return '' if project[name].truncation is not None else 'No span is set: Set Truncation sets one'


def _sensors_placed(project: Any, name: str) -> str:
    try:
        placed, _notes = project.sensors(name)
    except (TypeError, ValueError):
        placed = []
    return '' if placed else 'It names no sensor model at a node of its geometry'


#: what an operation needs of an object beyond its type, tested before the click so the row
#: can say so (Brandon, 2026-11-07): the param holding the object, and why it will not do, or ''
WHEN: dict[str, tuple[str, Callable[[Any, str], str]]] = {
    'project.transmissibility': ('frf', _frf_with_its_history),
    'project.partial_coherence': ('coherence', _coherence_with_its_history),
    'project.conditioning': ('data', _cross_terms),
    'project.realizability': ('data', _cross_terms),
    'project.mesh_solids': ('geometry', _solids),
    'project.remove_shock': ('history', _shocks_to_unmark),
    'project.truncate_data': ('source', _span),
    'project.sensors': ('table', _sensors_placed),
}

#: why the core (`Project.selection_verbs`) turns down a pair, where the verb's own reason
#: in `VERB_WHEN` does not cover it
PAIR_WHY = {'project.compute_modal_responses': 'It needs a force channel and real shapes',
            'project.export_state_space': 'It needs real shapes with a mass unit and a drive point'}


def _will_not_take(project: Any, op: Operation, fill: dict[str, str]) -> str:
    """Why `op` would refuse the objects in `fill`, as far as they say without typing; ''
    when nothing does. A pair goes by the core's rule, so the table and the core agree."""
    from hertzforge.core.merge import mergeable
    from hertzforge.project import PARTNER_VERBS

    when = WHEN.get(op.id)
    reason = when[1](project, fill[when[0]]) if when else ''
    verb = op.id.removeprefix('project.')
    names = fill.values()
    if op.id in _ONE_TYPE:
        said = mergeable([project[n] for n in names]) if len(fill) > 1 else None
        return said[0].upper() + said[1:] if said else ''
    if reason or verb not in PARTNER_VERBS or len(fill) < 2:
        return reason
    if verb in dict(project.selection_verbs(*names)):
        return ''
    return (PAIR_WHY.get(op.id)
            or _why_not(op, [projects.kind_of(project[n]) for n in names])
            or 'It cannot take this selection')


#: what a complex set refuses, since real data cannot carry a complex coordinate
REAL_SETS_ONLY = frozenset({'project.transform', 'project.expand',
                            'project.compute_modal_responses'})


def _rows_of(ops: Iterable[Operation]) -> list[Operation]:
    return [op for op in ops if op.id in verbs.VERBS]


def _kind_takes(param: Param, kind: str) -> bool:
    if param.verb:
        return kind in VERB_WHEN.get(param.verb, ((), ''))[0]
    return kind in param.kinds


def _kinds_fit(op: Operation, kinds: list[str]) -> bool:
    """Whether objects of `kinds` would fill `op`, whatever state they are in:
    one object per param, each used, every required param filled."""
    return (not _mixes_types(op, kinds)
            and _placed(_object_params(op), kinds, _kind_takes) is not None)


def _mixes_types(op: Operation, kinds: list[str]) -> bool:
    return op.id in _ONE_TYPE and len(set(kinds)) > 1


def _takes_anything(op: Operation) -> bool:
    return op.id not in _ONE_TYPE and any(len(p.kinds) >= _ANY for p in _object_params(op))


def defaults(project: Any, kinds: list[str], names: list[str]) -> list[str]:
    """What `show` draws by itself for objects of `kinds`. `names` are the
    objects, when they exist, for what depends on the object itself."""
    if len(kinds) == 1:
        kind = kinds[0]
        if kind == 'shape_set':
            return ['Animate' if names and _on_geometry(project, names[0]) else 'Table']
        if kind in ('channel_table', 'matched_modes'):
            return ['Table']
        return [] if kind in NO_VIEW else ['Plot']
    if len(kinds) == 2:
        joint = JOINT.get(tuple(sorted(kinds)))  # type: ignore[arg-type]
        if joint:
            return list(joint)
    if len(set(kinds)) == 1 and kinds[0] in STACKED:
        return ['Stack']
    if len(kinds) == 3 and sorted(kinds) == ['geometry', 'shape_set', 'shape_set']:
        return ['MAC']
    if len(kinds) == 3 and sorted(kinds) == ['geometry', 'shape_set', 'time_history']:
        history = next((n for n, k in zip(names, kinds, strict=False) if k == 'time_history'), None)
        return ['Animate'] if history is None or _holds_modal(project, history) else []
    return []


def _holds_modal(project: Any, name: str) -> bool:
    """Whether a time history holds modal coordinates, all `show._three_played` will play."""
    from hertzforge.core.validate import modal_coordinate

    return any(modal_coordinate(dof) is not None for dof in project[name].response_dof)


def _shape_views(project: Any, name: str) -> list[dict[str, Any]]:
    """A lone shape set's other views, each its own verb: the shell asks for one
    in `prefs.shape_view`, and clicking the default comes back."""
    from .show import SHAPE_VALUES_CELL_LIMIT

    shapes = project[name]
    count = int(shapes.num_shapes) * len(shapes.coordinate)
    first = (_row('show.table', 'Table', 'run') if _on_geometry(project, name)
             else _row('show.animate', 'Animate', 'run', 'It needs a geometry'))
    return [first, _row('show.mac', 'MAC', 'run'),
            _row('show.values', 'Values', 'run',
                 '' if count <= SHAPE_VALUES_CELL_LIMIT
                 else f'{count:,} shape values, too many for a table')]


def _on_geometry(project: Any, name: str) -> bool:
    from .show import _geometry_for

    return _geometry_for(project, name) is not None


def _row(row_id: str, verb: str, kind: str, reason: str = '', **more: Any) -> dict[str, Any]:
    row = {'id': row_id, 'verb': verb, 'kind': kind, 'ready': not reason, **more}
    if reason:
        row['reason'] = reason
    return row


def _op_row(op: Operation, reason: str = '', **more: Any) -> dict[str, Any]:
    return _row(op.id, verbs.VERBS[op.id], 'run', reason, modules=list(verbs.modules_of(op)),
                **more)


def _own_rows(project: Any, objects: list[str]) -> list[dict[str, Any]]:
    """The acts on the selection itself, which are the tree's and not operations."""
    one = objects[0] if len(objects) == 1 else None
    groups = project.object_groups
    grouped = next((g for g in groups if one in g['members']), None) if one else None
    stale = project.stale()
    rows = []
    if one and projects.kind_of(project[one]) == 'geometry':
        active = project.active_geometry == one
        rows.append(_row('self.activate', 'Make Active', 'self',
                         'It is the active geometry' if active else ''))
    units = [getattr(project[n], 'units_defined', None) for n in objects]
    if any(u is not None for u in units):
        rows.append(_row('self.units', 'Units', 'self', '' if one else 'Select one object'))
    rows.append(_row('self.recompute', 'Recompute', 'self',
                     '' if any(n in stale for n in objects) else 'Nothing here is out of date'))
    if len(objects) > 1:
        rows.append(_row('self.link', 'Link', 'self'))
    not_grouped = '' if grouped else 'It is in no group' if one else 'Select one object'
    rows.append(_row('self.unlink', 'Unlink', 'self', not_grouped))
    basis = grouped is not None and grouped.get('role') == 'Basis'
    rows.append(_row('self.basis', 'Basis', 'self',
                     'Its group is the Basis' if basis else not_grouped))
    rows.append(_row('self.name_group', 'Name Group', 'self', not_grouped))
    rows.append(_row('self.copy', 'Duplicate', 'self'))
    rows.append(_row('self.rename', 'Rename', 'self', '' if one else 'Select one object'))
    # no Remove: Delete in the tree removes the selection (Brandon, 2026-11-10)
    return rows


def _project_rows(project: Any) -> list[dict[str, Any]]:
    """The acts on the project as a whole, with nothing selected."""
    return [_row('self.open', 'Open', 'self'), _row('self.import', 'Import', 'self'),
            _row('self.save', 'Save', 'self'),
            _row('self.recompute_all', 'Recompute All', 'self',
                 '' if project.stale() else 'Nothing is out of date')]


def _fit_refusal(project: Any, names: list[str], records: dict[str, list[int]]) -> str:
    """Why this FRF, with this shape set when one is chosen, cannot start a fit,
    in the words `fit.start` would refuse in."""
    from hertzforge.core.modal_fit import fit_refusal
    from hertzforge.core.shapes import ShapeSet

    objects = [project[name] for name in names]
    frf = next(name for name, obj in zip(names, objects) if not isinstance(obj, ShapeSet))
    shapes = next((obj for obj in objects if isinstance(obj, ShapeSet)), None)
    # an Edit Fit takes its set's own records, as the shell starts it
    return fit_refusal(project[frf], None if shapes is not None else records.get(frf) or None, shapes)


def _fit_rows(kinds: list[str], reason: str = '') -> list[dict[str, Any]]:
    """The fitting screen's way in, which is not an operation: an FRF to fit by
    hand, or an FRF with the shape set to go on editing."""
    if kinds == ['frf']:
        return [_row('self.fit', 'Fit by Hand', 'self', reason, modules=['modal'])]
    if sorted(kinds) == ['frf', 'shape_set']:
        return [_row('self.edit_fit', 'Edit Fit', 'self', reason, modules=['modal'])]
    return []


def _current(project: Any, chosen: list[Chosen], records: dict[str, list[int]],
             part: dict[str, Any] | None) -> dict[str, Any]:
    from .ops_files import export_formats
    from .server import OPERATIONS

    names = [c[0] for c in chosen]
    kinds = [c[1] for c in chosen]
    rows = [_row(f'show.{verb.lower()}', verb, 'default') for verb in defaults(project, kinds, names)]
    if kinds == ['shape_set']:
        rows += _shape_views(project, names[0])
    if kinds == ['frf'] or sorted(kinds) == ['frf', 'shape_set']:
        rows += _fit_rows(kinds, _fit_refusal(project, names, records))
    if not chosen:
        rows += _project_rows(project)
    where = verbs.part_key(part)
    for op in _rows_of(OPERATIONS):
        homes = verbs.homes_of(op.id)
        if op.id in verbs.UNOFFERED or (where not in homes if homes else bool(where)):
            continue
        if not chosen:
            if not _object_params(op):
                # a new geometry or channel table asks for nothing: it is made at once and
                # renamed in the tree
                more = {'instant': True} if op.id in INSTANT else {}
                rows.append(_op_row(op, fill={}, **more))
            continue
        fill = _fill(op, chosen)
        if fill is None:
            if _kinds_fit(op, kinds):
                rows.append(_op_row(op, _why_not(op, kinds) or 'It cannot take this selection'))
            continue
        if _mixes_types(op, kinds):
            continue
        if op.id in REAL_SETS_ONLY and any(kind == 'shape_set' and project[name].is_complex
                                           for name, kind in zip(names, kinds)):
            rows.append(_op_row(op, 'A complex shape set cannot carry real data'))
            continue
        reason = _will_not_take(project, op, fill)
        if reason:
            rows.append(_op_row(op, reason))
            continue
        if op.id in NEEDS_NODES and not project[fill['geometry']].num_nodes:
            rows.append(_op_row(op, 'It has no nodes yet: mesh it first'))
            continue
        picks = [records.get(name, []) for name in fill.values()]
        if any(picks) and any(p.name == 'records' and p.type == 'string' for p in op.params):
            fill['records'] = _runs(next(r for r in picks if r))
        if where in ('mate', 'solid', 'group') and part:
            # what is picked in the tree is what the row acts on
            picked = verbs.picked_ids(part)
            if op.id in verbs.TOGETHER.get(where, ()) and len(picked) < 2:
                continue  # in the column of two picked together
            if any(p.name == where for p in op.params):
                if len(picked) > 1:
                    rows.append(_op_row(op, f'Pick one {where} in the tree', fill=fill, part=True))
                    continue
                fill[where] = str(part['id'])
            if any(p.name == f'{where}s' for p in op.params):
                fill[f'{where}s'] = ', '.join(str(i) for i in picked)
            if op.id == 'project.separate_solids' and not _made_of(project, fill['geometry'], picked):
                rows.append(_op_row(op, 'It is not a boolean or a pattern', fill=fill, part=True))
                continue
        if where == 'family' and any(p.name == 'family' for p in op.params):
            fill['family'] = str(part['id'])
        more: dict[str, Any] = {'part': True} if homes else {}
        if where and part:
            # what the pick fills is not asked again: the groups, solids or mates picked are
            # the ones it acts on, and more are picked in the tree (Brandon, 2026-11-10)
            picked_fields = [key for key in dict.fromkeys(
                (where, f'{where}s', 'family')) if key in fill]
            if picked_fields:
                more['omit'] = picked_fields
        if op.id == 'project.mesh_solids' and where == 'solid':
            # the solids picked, all at one size: the size they are likely to hold at,
            # as shells when every one is flat
            more['omit'] = list(dict.fromkeys([*more.get('omit', []), 'solids', 'sizes', 'detect']))
            fill.update(_mesh_fill(project, fill['geometry'], verbs.picked_ids(part)))
        if (op.id == 'project.group_properties' and where == 'group'
                and not _shells_among(project[fill['geometry']], verbs.picked_ids(part))):
            # a thickness is a shell's: a group of solid or beam elements is not asked for one
            more['omit'] = [*more.get('omit', []), 'thickness']
        if where == 'solids' and op.id in _adds():
            # a shape is made at once and then edited (Brandon, 2026-11-10)
            more['instant'] = True
        if op.id == 'project.save_as':
            more['formats'] = export_formats(project[fill['object']])
        if op.id == 'project.mate_surfaces':
            # one pane makes a mate from the groups picked and edits the mate picked
            # (Brandon, 2026-11-11)
            if where == 'mate':
                fill.update(_mate_fill(project[fill['geometry']], int(fill['mate'])))
                more['omit'] = [*more.get('omit', []), 'groups', 'tolerance', 'patch']
                rows.append({**_op_row(op, fill=fill, **more), 'verb': 'Edit'})
                continue
            more['omit'] = [*more.get('omit', []), 'mate']
        rows.append(_op_row(op, fill=fill, **more))
    if where == 'solid' and part and len(verbs.picked_ids(part)) == 1:
        edit = _solid_edit_row(project, names[0], part['id'], OPERATIONS)
        if edit is not None:
            rows = [edit, *(row for row in rows if row['id'] != 'project.edit_solid')]
    # a part picked is acted on by its own rows; the geometry's are its row's
    if chosen and not where:
        rows += _own_rows(project, names)
    column: dict[str, Any] = {'kinds': kinds, 'current': True, 'rows': rows}
    if where:
        column['parts'] = [where] * len(verbs.picked_ids(part))
    return column


def _adds() -> set[str]:
    from .ops_solids import ADD_OF_KIND

    return set(ADD_OF_KIND.values())


def _solid_edit_row(project: Any, geometry: str, solid_id: Any,
                    operations: Iterable[Operation]) -> dict[str, Any] | None:
    """A solid picked: its shape's Add, as Edit, filled with the solid's own values in the
    display units, each change applied as it is made (`live`). None for a boolean or a
    pattern, which Edit Solid changes."""
    from hertzforge.core import solids

    from . import display_units
    from .ops_solids import ADD_OF_KIND, solid_fill

    held = project[geometry]
    try:
        item = solids.solid(held, int(solid_id))
    except (KeyError, ValueError):
        return None
    op = next((o for o in operations if o.id == ADD_OF_KIND.get(item.kind)), None)
    if op is None:
        return None
    scale = display_units.words_in('length')[1] if held.units_defined else 1.0
    row = _op_row(op, fill={'geometry': geometry, **solid_fill(item, scale)}, part=True,
                  live=True)
    row['verb'] = 'Edit'
    return row


def _mate_fill(geometry: Any, mate_id: int) -> dict[str, str]:
    """Mate Surfaces opened on a kept mate: its name and how it holds, each stiffness in
    the display units."""
    from hertzforge.core import mates

    from . import display_units

    try:
        mated = mates.find(geometry, mate_id)
    except ValueError:
        return {}
    fill = {'name': mated.name, 'normal': mated.normal_mode, 'shear': mated.shear_mode}
    scale = display_units.words_in('force/length')[1]
    for side in ('normal', 'shear'):
        if getattr(mated, f'{side}_mode') == 'stiffness':
            fill[f'{side}_stiffness'] = f'{getattr(mated, f"{side}_stiffness") * scale:.6g}'
    return fill


def _mesh_fill(project: Any, geometry: str, ids: list[Any]) -> dict[str, str]:
    """Mesh's element type and size for the solids picked: shells for flat parts, else
    hexes, and the size they are likely to hold at, in the display units."""
    from hertzforge.core import solids

    from . import display_units
    from .ops_solids import likely_size

    held = project[geometry]
    picked = [int(i) for i in ids]
    elements = 'quads' if all(solids.is_flat(held, i) for i in picked) else 'hexes'
    fill = {'elements': elements}
    try:
        size = likely_size(held, picked, elements)
    except (ValueError, KeyError):
        return fill
    scale = display_units.words_in('length')[1] if held.units_defined else 1.0
    fill['size'] = f'{size * scale:.4g}'
    return fill


def _shells_among(geometry: Any, groups: list[Any]) -> bool:
    """Whether any of these element groups holds shells (face elements)."""
    import numpy as np

    from hertzforge.core.geometry import ELEMENT_TYPES

    held = np.isin(np.asarray(geometry.elem_group, dtype=int), [int(g) for g in groups])
    kinds = np.unique(np.asarray(geometry.elem_type, dtype=int)[held])
    return any(ELEMENT_TYPES.get(int(k), ('', 0, ''))[2] == 'face' for k in kinds)


def _why_not(op: Operation, kinds: list[str]) -> str:
    for param in _object_params(op):
        when = VERB_WHEN.get(param.verb)
        if when and any(k in when[0] for k in kinds) and when[1]:
            return when[1]
    return ''


def _partner_columns(project: Any, chosen: list[Chosen], taken: set[str],
                     by_kind: dict[str, list[Chosen]]) -> list[dict[str, Any]]:
    """One column per object type that could join the selection, whichever the
    project holds, with the objects that complete each row."""
    from .server import OPERATIONS

    names = [c[0] for c in chosen]
    kinds = [c[1] for c in chosen]
    partners = list(dict.fromkeys(kind for op in OPERATIONS for p in _object_params(op)
                                  for kind in (p.kinds or VERB_WHEN.get(p.verb, ((),))[0])))
    columns = []
    for kind in partners:
        joined = [*kinds, kind]
        present = [c for c in by_kind.get(kind, []) if c[0] not in names]
        rows = []
        for verb in [] if _shapes_on_their_geometry(joined) else defaults(project, joined, names):
            rows.append(_needs(_row(f'show.{verb.lower()}', verb, 'default', modules=None),
                               kind, [c[0] for c in present
                                      if verb in defaults(project, joined, [*names, c[0]])]))
        for row in _fit_rows(joined):
            rows.append(_needs(row, kind, [c[0] for c in present
                                           if not _fit_refusal(project, [*names, c[0]], {})]))
        for op in _rows_of(OPERATIONS):
            if op.id in taken or op.id in verbs.PARTS or not _kinds_fit(op, joined):
                continue
            if _takes_anything(op):
                continue
            fills = ((c, _fill(op, [*chosen, c])) for c in present)
            fits = [c[0] for c, fill in fills
                    if fill is not None and not _will_not_take(project, op, fill)
                    and not (op.id in REAL_SETS_ONLY and c[1] == 'shape_set'
                             and project[c[0]].is_complex)]
            rows.append(_needs(_op_row(op), kind, fits))
        if rows:
            columns.append({'kinds': joined, 'rows': rows})
    return columns


def _shapes_on_their_geometry(kinds: list[str]) -> bool:
    """Shape sets and one geometry: selecting the shape sets alone draws them on
    their geometry, so nothing is gained by offering to add it."""
    return sorted(set(kinds)) == ['geometry', 'shape_set'] and kinds.count('geometry') == 1


def _needs(row: dict[str, Any], kind: str, candidates: list[str]) -> dict[str, Any]:
    """A combination's row: the objects that complete it, or why none does."""
    row = {key: value for key, value in row.items() if value is not None}
    row['candidates'] = candidates
    row['ready'] = bool(candidates)
    if not candidates:
        row['reason'] = f'The project has no {kind.replace("_", " ")} for this'
    return row


def _part_ids(project: Any, geometry: str, where: str) -> list[int]:
    """The ids a part kind can be picked by in the tree: the solids that stand as
    parts, or the element groups."""
    from hertzforge.core import solids

    held = project[geometry]
    if where == 'solid':
        return [int(s.id) for s in solids.top_level(held)]
    return [int(g) for g in held.group_id]


def _together_column(project: Any, geometry: str, part: dict[str, Any], where: str,
                     taken: set[str]) -> dict[str, Any] | None:
    """A part with one more of its kind: what two or more picked together offer, each
    row naming the parts that would complete it."""
    from .server import OPERATIONS

    picked = {str(i) for i in verbs.picked_ids(part)}
    others = [str(i) for i in _part_ids(project, geometry, where) if str(i) not in picked]
    rows = []
    for op in _rows_of(OPERATIONS):
        if op.id not in verbs.TOGETHER[where] or op.id in taken or op.id in verbs.UNOFFERED:
            continue
        row = _op_row(op, part=True)
        row['candidates'] = others
        row['ready'] = bool(others)
        if not others:
            row['reason'] = f'The geometry has no other {where} for this'
        rows.append(row)
    if not rows:
        return None
    return {'kinds': [projects.kind_of(project[geometry])],
            'parts': [where] * (len(picked) + 1), 'rows': rows}


def action_table(project: Any, objects: list[str], records: dict[str, list[int]],
                 part: dict[str, Any] | None = None) -> dict[str, Any]:
    """`{'columns': [...]}`: the selection's column first, then one for each
    object that could be added, then one for any object at all."""
    verbs_of = {name: projects.verbs_of(project, name) for name, _obj in project.items()}
    every: list[Chosen] = [(name, projects.kind_of(obj), verbs_of[name]) for name, obj in project.items()]
    by_name = {c[0]: c for c in every}
    chosen = [by_name[name] for name in objects]
    by_kind: dict[str, list[Chosen]] = {}
    for item in every:
        by_kind.setdefault(item[1], []).append(item)
    current = _current(project, chosen, records, part)
    columns = [current]
    where = verbs.part_key(part)
    if chosen and where:
        # a part picked pairs with another of its kind, not with other objects
        taken = {row['id'] for row in current['rows'] if row.get('ready')}
        together = (_together_column(project, objects[0], part, where, taken)
                    if where in verbs.TOGETHER and len(objects) == 1 else None)
        return {'columns': [current, *([together] if together else [])]}
    if chosen:
        taken = {row['id'] for row in current['rows']}
        columns += _partner_columns(project, chosen, taken, by_kind)
        others = [name for name in by_name if name not in objects]
        if len(objects) == 1:
            columns.append({'kinds': [*current['kinds'], '*'],
                            'rows': [_needs(_row('self.link', 'Link', 'self'), 'other object', others)]})
    return {'columns': columns}
