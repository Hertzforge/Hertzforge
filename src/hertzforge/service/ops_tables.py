"""Tables of a project's objects, and matching two shape sets' modes.

Visual Dynamics shows a shape set, a channel table and a set of matched
modes as a table when it is expanded (`core.tables.table_of`), a shape
set's values at a choice of DOFs (`core.tables.shape_values`), and keeps
the pairs picked on a MAC between two shape sets as a Matched Modes
object (`Project.match_modes`). Each is an operation here, its table a
`table` view.
"""

from __future__ import annotations

from typing import Any

from . import views as v
from .descriptors import Operation, Option, Param
from .ops_project import chosen_records
from .protocol import refused

TABLED = Param('object', 'Object', 'enum',
               options_from='objects:shape_set|channel_table|matched_modes')
SHAPES = Param('shapes', 'Shape set', 'enum', options_from='objects:shape_set')
FIRST = Param('first', 'Basis', 'enum', options_from='objects:shape_set')
SECOND = Param('second', 'Compared', 'enum', options_from='objects:shape_set')
THRESHOLD = Param('threshold', 'Lowest MAC', 'number', default=0.7, min=0.0, max=1.0,
                  step=0.05)
NAME = Param('name', 'Name', 'string', default='Matched Modes')
#: the MAC squares to keep, row and column numbered from 1 as the grid
#: shows them: '1,1; 2,3'
PAIRS = Param('pairs', 'Pairs', 'string', default='')
MATCHED = Param('matched', 'Matched modes', 'enum', options_from='objects:matched_modes')
ROWS = Param('rows', 'Rows', 'string', default='')
FORM = Param('form', 'Form', 'enum', default='shapes', options=(
    Option('shapes', 'Shapes, Φ'), Option('response', 'Response transformation, Φ⁺'),
    Option('reference', 'Reference transformation, Φᵀ')))
DOFS = Param('dofs', 'DOFs', 'string', default='')


def _table(project: Any, name: str) -> dict:
    from hertzforge.core.tables import table_of

    obj = project[name]
    geometry = project.geometry_for(name) if hasattr(project, 'geometry_for') else None
    found = table_of(obj, geometry=None if geometry is None else geometry[1],
                     objects=dict(project.items()))
    if found is None:
        raise refused(f'{name} has no table')
    headers, rows = found
    return v.table(name, headers, rows)


def view_table(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.table`: an object's table, as Visual Dynamics shows it
    expanded: a shape set's modes, a channel table's columns (with each
    channel's direction on the geometry linked with it), or matched
    modes beside both sets' parameters."""
    return [_table(project, params['object'])]


def match_modes(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.match_modes`: each of the basis set's modes paired with
    the compared set's mode of highest MAC, when that MAC is at least the
    lowest given (`Project.match_modes`, across geometries after
    projection), kept as a Matched Modes object, and its table."""
    first, second = params['first'], params['second']
    if first == second:
        raise refused('a set is matched against another set, not itself')
    try:
        name = project.match_modes(first, second, threshold=params['threshold'],
                                   name=params['name'] or 'Matched Modes')
    except ValueError as error:
        raise refused(str(error)) from None
    if not project[name].num_matches:
        project.remove(name)
        raise refused(f'no mode of {first} has a MAC of {params["threshold"]:g} or more '
                      f'with a mode of {second}')
    return [_table(project, name)]


def add_matches(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.add_matches`: the picked MAC squares kept as matched
    modes of this pair of sets, with the MAC as the comparison shows it
    (`Project.comparison_mac`, after projection across geometries): the
    Matched Modes object is created on first use and added to after
    (`MatchedModes.add`, journaled), as VD's Add Matches does."""
    from .projects import kind_of as projects_kind

    first, second = params['first'], params['second']
    if first == second:
        raise refused('a set is matched against another set, not itself')
    pairs = []
    for piece in params['pairs'].replace(';', ' ').split():
        row, _comma, column = piece.partition(',')
        if not (row.strip().isdigit() and column.strip().isdigit()):
            raise refused(f'"{piece}" is not a pair: row,column numbered from 1, such as 2,3')
        pairs.append((int(row) - 1, int(column) - 1))
    if not pairs:
        raise refused('pick the MAC squares to keep: row,column pairs such as 1,1; 2,3')
    try:
        values = project.comparison_mac(first, second)
    except ValueError as error:
        raise refused(str(error)) from None
    for row, column in pairs:
        if not (0 <= row < values.shape[0] and 0 <= column < values.shape[1]):
            raise refused(f'the MAC is {values.shape[0]} by {values.shape[1]}: no square '
                          f'{row + 1},{column + 1}')
    macs = [float(values[row, column]) for row, column in pairs]
    found = next(((name, obj) for name, obj in project.items()
                  if projects_kind(obj) == 'matched_modes' and obj.first == first
                  and obj.second == second), None)
    if found is None:
        name = project.match_modes(first, second, pairs=pairs, macs=macs)
    else:
        name, matched = found
        matched.add([list(pair) for pair in pairs], macs)
        project.record_call(matched, 'add', [[int(a), int(b)] for a, b in pairs], macs)
    return [_table(project, name)]


def delete_matches(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.delete_matches`: take rows out of a Matched Modes object
    (`MatchedModes.delete_matches`), numbered from 1 as its table shows
    them, '2, 4-5'."""
    name = params['matched']
    matched = project[name]
    if not params['rows'].strip():
        raise refused('name the rows to delete, such as 2, 4-5')
    rows = chosen_records(params['rows'], matched.num_matches)
    if len(rows) == matched.num_matches:
        raise refused('that is every row: delete the object instead')
    matched.delete_matches(rows)
    return [_table(project, name)]


def shape_values(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.shape_values`: a shape set's values at a choice of DOFs
    (`core.tables.shape_values`): the shapes Φ, a row per DOF and a
    column per mode, or the response (Φ⁺) or reference (Φᵀ)
    transformation the modal transform applies, a row per mode. A DOF
    the set does not have is labeled so; its values are blank."""
    from hertzforge.core.tables import shape_values as values_of

    name = params['shapes']
    dofs = params['dofs'].replace(',', ' ').split() or None
    try:
        headers, rows = values_of(project[name], dofs, params['form'])
    except ValueError as error:
        raise refused(str(error)) from None
    title = {'shapes': f'{name}, shapes',
             'response': f'{name}, response transformation',
             'reference': f'{name}, reference transformation'}[params['form']]
    return [v.table(title, headers, rows)]


OPERATIONS = (
    Operation(
        id='project.table', title='View Table', workspace='signals', everywhere=True, accepts='project',
        summary='An object\'s table: a shape set\'s modes, a channel table\'s columns, or '
                'matched modes beside both sets\' parameters.',
        params=(TABLED,), outputs=('table',), run=view_table),
    Operation(
        id='project.match_modes', title='Match Modes', workspace='modal', accepts='project',
        summary='Pair each mode of the basis set with the compared set\'s mode of highest '
                'MAC, at or above the lowest MAC given, and keep the pairs as an object.',
        params=(FIRST, SECOND, THRESHOLD, NAME), outputs=('table',), run=match_modes),
    Operation(
        id='project.add_matches', title='Add Matches', workspace='modal', accepts='project',
        summary='Keep the picked MAC squares as matched modes of this pair of sets, with the '
                'MAC as the comparison shows it; the Matched Modes object is created on first '
                'use and added to after.',
        params=(FIRST, SECOND, PAIRS), outputs=('table',), run=add_matches),
    Operation(
        id='project.delete_matches', title='Delete Matches', workspace='modal',
        accepts='project',
        summary='Take rows out of a set of matched modes, numbered as its table shows them.',
        params=(MATCHED, ROWS), outputs=('table',), run=delete_matches, edits=True),
    Operation(
        id='project.shape_values', title='Shape Values', workspace='modal', accepts='project',
        summary='A shape set\'s values at a choice of DOFs: the shapes, or the response or '
                'reference transformation the modal transform applies.',
        params=(SHAPES, FORM, DOFS), outputs=('table',), run=shape_values),
)
