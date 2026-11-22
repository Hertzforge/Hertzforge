"""An action on shape sets reads the modes picked under each set in the tree, every mode of a
set picked whole: the MAC, Match Modes, Add Matches, Transform, Reduce and the rest
(`action_table.PICKED_MODES`). An action that takes a set whole says why
(`action_table.WHOLE_SETS`)."""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
import pytest

import hertzforge
from hertzforge.core.data import TimeHistory
from hertzforge.core.shapes import ShapeSet, cross_mac
from hertzforge.service import action_table, projects
from hertzforge.service.protocol import ProtocolError
from hertzforge.service.server import OPERATIONS
from hertzforge.service.show import _run

from .conftest import wire_views

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

CORNERS = [(x, y, z) for x in (0, 1) for y in (0, 1) for z in (0, 1)]
DOFS = [f'{n}{d}+' for n in range(1, 9) for d in 'XYZ']


def _sets():
    """A on the cube's corners with three modes, and B holding A's modes in order plus a
    fourth: each A mode matches the B mode of the same index and no other."""
    rng = np.random.default_rng(7)
    phi = rng.standard_normal((4, len(DOFS)))
    a = ShapeSet([10.0, 20.0, 30.0], [0.01] * 3, DOFS, phi[:3])
    b = ShapeSet([11.0, 21.0, 31.0, 41.0], [0.01] * 4, DOFS, phi)
    return a, b


@pytest.fixture
def state():
    held = projects.new({'title': 'Picked modes'})
    project = held.project
    a, b = _sets()
    project.add('Cube', hertzforge.Geometry(node_id=list(range(1, 9)), node_xyz=CORNERS))
    project.add('A', a)
    project.add('B', b)
    project.link('Cube', 'A', 'B')
    return held


def shown(state, *objects, records=None, validate=None):
    out = projects.show(state, {'objects': list(objects), 'records': records or {},
                                'part': None})
    if validate:
        wire_views(out['views'], validate)
        validate({**out, 'views': []}, 'shown')
    return out


def rows(state, *objects, records=None):
    out = projects.actions(state, {'objects': list(objects), 'records': records or {},
                                   'part': None})
    return [row for column in out['table']['columns'] for row in column['rows']]


def row(state, op, *objects, records=None):
    return next(r for r in rows(state, *objects, records=records) if r['id'] == op)


def test_the_mac_of_two_sets_is_of_the_modes_picked_under_each(state, validate):
    project = state.project
    whole = cross_mac(project['A'], project['B'])
    out = shown(state, 'A', 'B', records={'A': [0, 2], 'B': [1, 3]}, validate=validate)
    matrix, stage = out['views'][:2]
    assert matrix['rows']['ticks'] == ['10.0 Hz', '30.0 Hz']
    assert matrix['columns']['ticks'] == ['21.0 Hz', '41.0 Hz']
    assert matrix['rows']['indices'] == [0, 2] and matrix['columns']['indices'] == [1, 3]
    assert np.allclose(matrix['values'], whole[np.ix_([0, 2], [1, 3])], atol=1e-6)
    # the stage a square selects steps through the same modes the grid shows
    assert len(stage['modes']) == 2 and len(stage['overlays']) == 2
    assert '2 of the 3 modes of A' in out['note'] and '2 of the 4 modes of B' in out['note']


def test_a_set_picked_whole_beside_picks_in_the_other_gives_all_its_modes(state):
    out = shown(state, 'A', 'B', records={'B': [3]})
    matrix = out['views'][0]
    assert matrix['rows']['indices'] == [0, 1, 2] and matrix['columns']['indices'] == [3]
    assert np.asarray(matrix['values']).shape == (3, 1)
    assert 'modes of A' not in out['note']


def test_matched_pairs_are_marked_where_they_sit_on_the_grid(state):
    state.project.match_modes('A', 'B', pairs=[(0, 0), (2, 2)])
    assert shown(state, 'A', 'B')['views'][0]['matched'] == [[0, 0], [2, 2]]
    # a pair whose modes are both on the grid is marked at its cell; one with a mode off it
    # is not drawn
    picked = shown(state, 'A', 'B', records={'A': [1, 2], 'B': [2, 3]})['views'][0]
    assert picked['matched'] == [[1, 0]]


def test_match_modes_pairs_only_the_modes_picked(state):
    project = state.project
    filled = row(state, 'project.match_modes', 'A', 'B', records={'A': [1, 2], 'B': [0, 2]})
    assert (filled['fill']['first_modes'], filled['fill']['second_modes']) == ('2-3', '1, 3')
    assert {'first_modes', 'second_modes'} <= set(filled['omit'])
    _run(project, 'project.match_modes', first='A', second='B', threshold=0.7,
         first_modes=filled['fill']['first_modes'], second_modes=filled['fill']['second_modes'])
    picked = next(obj for name, obj in project.items() if name.startswith('Matched'))
    # A's mode 2 has its match (B's mode 2) off the picks: nothing above the threshold is left
    assert [tuple(pair) for pair in picked.pairs] == [(2, 2)]
    # with nothing picked every mode is paired
    whole = row(state, 'project.match_modes', 'A', 'B')
    assert (whole['fill']['first_modes'], whole['fill']['second_modes']) == ('', '')


def test_the_mac_operation_reads_the_modes_it_is_given(state):
    matrix = _run(state.project, 'project.mac', first='A', second='B', first_modes='3',
                  second_modes='')[0]
    assert matrix['rows']['ticks'] == ['30.0 Hz'] and matrix['columns']['indices'] == [0, 1, 2, 3]


def test_a_mode_the_set_does_not_have_is_refused_by_number(state):
    with pytest.raises(ProtocolError, match='modes run from 1 to 3; 4 is outside them'):
        _run(state.project, 'project.mac', first='A', second='B', first_modes='4')


def _history(project, dim='length'):
    times = np.arange(64) / 64.0
    values = np.vstack([np.sin(2 * np.pi * (k % 5 + 1) * times) for k in range(len(DOFS))])
    project.add('Run', TimeHistory(times, values, DOFS, ordinate_dim=dim))
    project.link('Cube', 'Run')


def test_transform_reads_the_modes_picked_and_not_the_records_from_them(state):
    project = state.project
    _history(project)
    filled = row(state, 'project.transform', 'Run', 'A', records={'A': [0, 2]})
    # the modes picked under the set are its modes; the history, picked whole, gives every
    # record, not the set's mode numbers
    assert filled['fill']['modes'] == '1, 3' and filled['fill']['records'] == ''
    _run(project, 'project.transform', source='Run', shapes='A', records='', modes='1, 3')
    name = next(name for name in project if name.endswith('Modal Coordinates'))
    assert project[name].num_records == 2
    # a refresh transforms through the same modes
    assert project.provenance[name]['params']['modes'] == [0, 2]


def test_compute_modal_responses_reads_the_modes_picked(state):
    project = state.project
    _history(project, 'force')
    filled = row(state, 'project.compute_modal_responses', 'Run', 'A', records={'A': [1]})
    assert filled['fill']['modes'] == '2' and filled['fill']['records'] == ''


@pytest.fixture
def apart():
    from test_correlate import _fem, _test_model

    fem_geometry, fem_shapes = _fem()
    test_geometry, test_shapes = _test_model()
    held = projects.new({'title': 'Two'})
    project = held.project
    project.add('FEM Geometry', fem_geometry)
    project.add('FEM Shapes', fem_shapes)
    project.add('Test Geometry', test_geometry)
    project.add('Test Shapes', test_shapes)
    project.link('FEM Geometry', 'FEM Shapes')
    project.link('Test Geometry', 'Test Shapes')
    return held


def test_reduce_projects_the_modes_picked(apart):
    project = apart.project
    filled = row(apart, 'project.project_onto_basis', 'FEM Shapes', 'Test Shapes',
                 records={'FEM Shapes': [1]})
    assert filled['fill']['source'] == 'FEM Shapes' and filled['fill']['modes'] == '2'
    _run(project, 'project.project_onto_basis', source='FEM Shapes', onto='Test Shapes',
         modes='2')
    added = project[next(name for name in project if '@' in name)]
    assert list(added.frequency) == [20.0]
    assert added.projection_report is not None


def test_the_projection_overlay_is_of_the_modes_picked(apart):
    views = _run(apart.project, 'project.projection_overlay', first='FEM Shapes',
                 second='Test Shapes', first_modes='2', second_modes='')
    assert views[0]['rows']['indices'] == [1] and np.asarray(views[0]['values']).shape == (1, 2)
    shown_out = projects.show(apart, {'objects': ['FEM Shapes', 'Test Shapes'],
                                      'records': {'Test Shapes': [0]}, 'part': None,
                                      'prefs': {'projection': True}})
    assert shown_out['views'][0]['columns']['indices'] == [0]


def test_the_kept_set_is_a_copy_of_the_modes_in_order():
    a, _b = _sets()
    kept = a.kept([2, 0, 2])
    assert list(kept.frequency) == [10.0, 30.0] and a.num_shapes == 3
    assert np.array_equal(kept.shape_matrix, a.shape_matrix[[0, 2]])
    assert a.kept(None) is a
    with pytest.raises(ValueError, match='no modes are picked'):
        a.kept([])


def test_every_action_on_a_shape_set_reads_its_picks_or_says_why_not(state):
    """The rule, enforced: an action that takes a shape set fills a param from the modes
    picked under it, or is listed with the reason it takes the set whole."""
    verbs = projects.verbs_of(state.project, 'A')
    takes = [op.id for op in action_table._rows_of(OPERATIONS)
             if any('shape_set' in p.kinds or (p.verb and p.verb in verbs)
                    for p in op.params if p.kinds or p.verb)]
    assert 'project.match_modes' in takes and 'project.transform' in takes
    neither = [op for op in takes
               if op not in action_table.PICKED_MODES and op not in action_table.WHOLE_SETS]
    assert neither == []
    both = set(action_table.PICKED_MODES) & set(action_table.WHOLE_SETS)
    assert both == set()
    # each param a set's picks fill is one the action has, and names a param holding a set
    by_id = {op.id: op for op in OPERATIONS}
    for op_id, fills in action_table.PICKED_MODES.items():
        names = {p.name for p in by_id[op_id].params}
        assert set(fills) <= names and set(fills.values()) <= names, op_id
