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

from collections.abc import Iterable
from itertools import permutations
from typing import Any

from . import projects, verbs
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
    'generate_signal': (('specification', 'shock_specification', 'transient_specification',
                         'sine_sweep_specification', 'psd', 'srs', 'time_history'),
                        'No signal can be made from it'),
    'solve_modes': (('geometry',), 'Its element groups have no properties yet'),
    'plan_instrumentation': (('shape_set',), 'It needs a mode above 0 Hz on a geometry with elements'),
    'project_onto_basis': (('shape_set',), 'It needs a second shape set'),
    'overload_check': (_HISTORY, 'No linked channel table gives its ranges'),
}

#: an object param that takes this many types takes anything: it says nothing
#: about what a pair means, so it does not make a column of its own
_ANY = 8

Chosen = tuple[str, str, list[str]]


def _rows_of(ops: Iterable[Operation]) -> list[Operation]:
    return [op for op in ops if op.id in verbs.VERBS]


def _object_params(op: Operation) -> list[Param]:
    return [p for p in op.params if p.kinds or p.verb]


def _kind_takes(param: Param, kind: str) -> bool:
    if param.verb:
        return kind in VERB_WHEN.get(param.verb, ((), ''))[0]
    return kind in param.kinds


def _kinds_fit(op: Operation, kinds: list[str]) -> bool:
    """Whether objects of `kinds` would fill `op`, whatever state they are in:
    one object per param, each used, every required param filled. Merge asks
    for one type (`core.merge`), whatever its params take."""
    params = _object_params(op)
    if op.id == 'project.merge' and len(set(kinds)) > 1:
        return False
    if len(kinds) > len(params):
        return False
    required = [p for p in params if p.default != '']
    return any(all(_kind_takes(p, k) for p, k in zip(order, kinds, strict=True))
               and all(p in order for p in required)
               for order in permutations(params, len(kinds)))


def _takes_anything(op: Operation) -> bool:
    return any(len(p.kinds) >= _ANY for p in _object_params(op))


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
        return ['Animate']
    return []


def _shape_views(project: Any, name: str) -> list[dict[str, Any]]:
    """A lone shape set's other views, each its own verb: the shell asks for one
    in `prefs.shape_view`, and clicking the default comes back."""
    from .show import SHAPE_VALUES_CELL_LIMIT

    shapes = project[name]
    count = int(shapes.num_shapes) * len(shapes.coordinate)
    first = (_row('show.table', 'Table', 'reading') if _on_geometry(project, name)
             else _row('show.animate', 'Animate', 'reading', 'It needs a geometry'))
    return [first, _row('show.mac', 'MAC', 'reading'),
            _row('show.values', 'Values', 'reading',
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
    from .actions import _is_act

    return _row(op.id, verbs.VERBS[op.id], 'act' if _is_act(op) else 'reading', reason,
                modules=list(verbs.modules_of(op)), **more)


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
    rows.append(_row('self.copy', 'Copy', 'self'))
    rows.append(_row('self.rename', 'Rename', 'self', '' if one else 'Select one object'))
    rows.append(_row('self.remove', 'Remove', 'self'))
    return rows


def _project_rows(project: Any) -> list[dict[str, Any]]:
    """The acts on the project as a whole, with nothing selected."""
    return [_row('self.open', 'Open', 'self'), _row('self.import', 'Import', 'self'),
            _row('self.save', 'Save', 'self'),
            _row('self.recompute_all', 'Recompute All', 'self',
                 '' if project.stale() else 'Nothing is out of date')]


def _fit_rows(kinds: list[str]) -> list[dict[str, Any]]:
    """The fitting screen's way in, which is not an operation: an FRF to fit by
    hand, or an FRF with the shape set to go on editing."""
    if kinds == ['frf']:
        return [_row('self.fit', 'Fit by Hand', 'self', modules=['modal'])]
    if sorted(kinds) == ['frf', 'shape_set']:
        return [_row('self.edit_fit', 'Edit Fit', 'self', modules=['modal'])]
    return []


def _current(project: Any, chosen: list[Chosen], records: dict[str, list[int]],
             part: dict[str, Any] | None) -> dict[str, Any]:
    from .actions import _fill, _runs
    from .ops_files import export_formats
    from .server import OPERATIONS

    names = [c[0] for c in chosen]
    kinds = [c[1] for c in chosen]
    rows = [_row(f'show.{verb.lower()}', verb, 'default') for verb in defaults(project, kinds, names)]
    if kinds == ['shape_set']:
        rows += _shape_views(project, names[0])
    rows += _fit_rows(kinds)
    if not chosen:
        rows += _project_rows(project)
    where = verbs.part_key(part)
    for op in _rows_of(OPERATIONS):
        home = verbs.PARTS.get(op.id)
        if home is not None and home != where:
            continue
        if not chosen:
            if not _object_params(op):
                rows.append(_op_row(op, fill={}))
            continue
        fill = _fill(op, chosen)
        if fill is None:
            if _kinds_fit(op, kinds):
                rows.append(_op_row(op, _why_not(op, kinds, chosen) or 'It cannot take this selection'))
            continue
        if op.id == 'project.merge':
            if len(set(kinds)) > 1:
                continue
            from hertzforge.core.merge import mergeable

            refused = mergeable([project[n] for n in names])
            if refused:
                rows.append(_op_row(op, refused[0].upper() + refused[1:]))
                continue
        picks = [records.get(name, []) for name in fill.values()]
        if any(picks) and any(p.name == 'records' and p.type == 'string' for p in op.params):
            fill['records'] = _runs(next(r for r in picks if r))
        if where == 'mate' and part:
            # the set picked in the tree is the one the row acts on
            for held in ('mate', 'mates'):
                if any(p.name == held for p in op.params):
                    fill[held] = str(part['id'])
        more: dict[str, Any] = {'part': True} if home else {}
        if op.id == 'project.save_as':
            more['formats'] = export_formats(project[fill['object']])
        rows.append(_op_row(op, fill=fill, **more))
    if chosen:
        rows += _own_rows(project, names)
    return {'kinds': kinds, 'current': True, 'rows': rows}


def _why_not(op: Operation, kinds: list[str], chosen: list[Chosen]) -> str:
    for param in _object_params(op):
        when = VERB_WHEN.get(param.verb)
        if when and any(k in when[0] for k in kinds) and when[1]:
            return when[1]
    return ''


def _partner_columns(project: Any, chosen: list[Chosen], taken: set[str],
                     by_kind: dict[str, list[Chosen]]) -> list[dict[str, Any]]:
    """One column per object type that could join the selection, whichever the
    project holds, with the objects that complete each row."""
    from .actions import _fill
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
        for verb in [] if _shapes_on_their_geometry(joined) else defaults(project, joined, []):
            rows.append(_needs(_row(f'show.{verb.lower()}', verb, 'default', modules=None),
                               kind, [c[0] for c in present]))
        for row in _fit_rows(joined):
            rows.append(_needs(row, kind, [c[0] for c in present]))
        for op in _rows_of(OPERATIONS):
            if op.id in taken or op.id in verbs.PARTS or not _kinds_fit(op, joined):
                continue
            if _takes_anything(op) and op.id != 'project.merge':
                continue
            fits = [c[0] for c in present if _fill(op, [*chosen, c]) is not None]
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
    if chosen:
        taken = {row['id'] for row in current['rows']}
        columns += _partner_columns(project, chosen, taken, by_kind)
        others = [name for name in by_name if name not in objects]
        if len(objects) == 1:
            columns.append({'kinds': [*current['kinds'], '*'],
                            'rows': [_needs(_row('self.link', 'Link', 'self'), 'other object', others)]})
    return {'columns': columns}
