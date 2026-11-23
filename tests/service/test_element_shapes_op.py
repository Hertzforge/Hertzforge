"""Strain and Stress in the window: the action on a solved shape set reads it
element by element, its form previews the model colored by one component, and the
set it adds opens on its model with each element its own color, its modes picked
in the tree and drawn by another component through Color By."""

from __future__ import annotations

import asyncio
import sys
from pathlib import Path

import numpy as np
import pytest

from hertzforge.service import display_units, projects
from hertzforge.service.server import OPERATIONS

from .test_projects import _exchange

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from test_element_shapes import _cells

OP = 'project.element_shapes'


def _state():
    state = projects.new({'title': 'p'})
    state.project.add('Cells', _cells(along=4))
    state.project.solve_modes('Cells', num_modes=9)
    return state


def _row(state, objects, op, records=None):
    columns = projects.actions(state, {'objects': objects, 'records': records or {},
                                       'part': None})['table']['columns']
    return next((row for column in columns for row in column['rows'] if row['id'] == op),
                None)


def _run(state, op_id, fill):
    op = next(o for o in OPERATIONS if o.id == op_id)
    kinds = {name: projects.kind_of(obj) for name, obj in state.project.items()}
    verbs = {name: projects.verbs_of(state.project, name) for name in state.project}
    return op.run(state.project, op.resolve(fill, objects=kinds, verbs=verbs))


def test_the_action_reads_the_modes_picked_and_draws_each_element_its_own_color():
    state = _state()
    row = _row(state, ['Cells Modes'], OP, {'Cells Modes': [6, 8]})
    assert row is not None and row['fill']['modes'] == '7, 9'
    views = _run(state, OP, {**row['fill'], 'component': 'xx'})
    view = next(v for v in views if v['view'] == 'shape3d')
    added = next(r['value'] for v in views if v['view'] == 'reading'
                 for r in v['rows'] if r['label'] == 'Added')
    read = state.project[added]
    assert read.num_shapes == 2
    held = view['element_values']
    assert held['label'] == 'Stress xx' and held['unit'] == 'Pa'
    # one value per element and mode, the elements' centroid xx stress in order
    assert held['values'].shape == (2, len(read.elements))
    np.testing.assert_allclose(held['values'][1], read.values('stress', 'xx', 1), rtol=1e-6)
    # the outside of 3 bricks and 2 wedges in a row: each triangle names its element
    assert held['owner'].max() == len(read.elements) - 1
    assert held['triangles'].shape[1] == 3 and len(held['triangles']) == len(held['owner'])
    assert held['triangles'].max() < len(view['nodes'])
    # each mode drawn moving, as the shape set it was read from moves
    assert len(view['modes']) == 2 and np.abs(view['modes'][0]['shape']).max() > 0


def test_a_shell_face_shared_by_two_elements_read_is_not_drawn():
    """The skin of the elements read: a face two of them share is inside them."""
    from hertzforge.service.ops_element_shapes import element_skin

    geometry = _cells(along=4)
    bricks = [int(e) for e, t in zip(geometry.elem_id, geometry.elem_type) if t == 115]
    skin, owner = element_skin(geometry, bricks)
    # three bricks in a row: 3 x 6 faces less the 2 shared, each twice over (both sides),
    # each face two triangles
    assert len(skin) == 2 * (3 * 6 - 2 * 2) and len(owner) == len(skin)
    assert sorted(set(owner.tolist())) == [0, 1, 2]


def test_the_set_opens_on_its_model_and_color_by_picks_another_component():
    state = _state()
    added = state.project.element_shapes('Cells Modes', modes=[6, 7, 8])
    shown = projects.show(state, {'objects': [added], 'records': {added: [1]}, 'part': None,
                                  'prefs': {}})
    view = next(v for v in shown['views'] if v['view'] == 'shape3d')
    assert view['element_values']['label'].startswith('Stress ')
    assert [m['index'] for m in view['modes']] == [2]
    row = _row(state, [added], 'project.color_element_shapes', {added: [0]})
    assert row is not None and row['fill']['modes'] == '1'
    (strain,) = _run(state, 'project.color_element_shapes',
                     {**row['fill'], 'quantity': 'strain', 'component': 'yz'})
    assert strain['element_values']['label'] == 'Strain yz'
    assert strain['element_values']['unit'] == ''
    np.testing.assert_allclose(strain['element_values']['values'][0],
                               state.project[added].values('strain', 'yz', 0), rtol=1e-6)


def test_the_largest_component_is_the_one_that_reaches_furthest():
    from hertzforge.core.element_shapes import COMPONENTS
    from hertzforge.service.ops_element_shapes import largest_component

    state = _state()
    added = state.project.element_shapes('Cells Modes', modes=[6])
    read = state.project[added]
    peaks = [np.abs(read.values('stress', c, 0)).max() for c in COMPONENTS]
    assert largest_component(read, 'stress') == COMPONENTS[int(np.argmax(peaks))]


def test_a_stress_is_drawn_in_the_display_systems_pressure():
    state = _state()
    added = state.project.element_shapes('Cells Modes', modes=[6])
    (view,) = _run(state, 'project.color_element_shapes',
                   {'object': added, 'quantity': 'stress', 'component': 'xx'})
    pascals = np.array(view['element_values']['values'])
    display_units.convert([view], 'in-slinch-lbf-s')
    assert view['element_values']['unit'] == 'psi'
    np.testing.assert_allclose(view['element_values']['values'], pascals / 6894.757293168,
                               rtol=1e-5)


def test_the_action_is_offered_only_on_modes_that_reach_a_model_of_solids():
    """A set at a test's DOFs, drawn on the active model, does not reach its
    solids' nodes: the row says why rather than offering a read that fails."""
    from hertzforge.core.shapes import ShapeSet

    state = _state()
    assert _row(state, ['Cells Modes'], OP)['ready']
    state.project.add('Test Modes', ShapeSet([5.0], [0.0], ['1X+'], [[1.0]]))
    row = _row(state, ['Test Modes'], OP)
    assert row is not None and not row['ready']
    assert row['reason'] == 'It needs real modes on a model with a group of solids'


def test_the_form_previews_and_adds_nothing(tmp_path):
    state = _state()
    path = tmp_path / 'cells.hforge'
    state.project.save(str(path))
    params = {'source': 'Cells Modes', 'groups': '', 'modes': '7', 'quantity': 'stress',
              'component': 'largest'}
    responses = asyncio.run(_exchange([
        ('project.open', {'path': str(path)}),
        ('op.run', {'op': OP, 'params': params, 'preview': True}),
        ('op.run', {'op': OP, 'params': params}),
    ]))
    preview, ran = (got[-1]['result'] for got in responses[1:])
    assert 'project' not in preview
    rows = {row['label']: row for view in preview['views'] if view['view'] == 'reading'
            for row in view['rows']}
    assert 'Would add' in rows and 'Added' not in rows
    assert any(view['view'] == 'shape3d' and 'element_values' in view
               for view in preview['views'])
    names = [o['name'] for o in ran['project']['objects']]
    assert names.count('Cells Modes Strain and Stress') == 1, 'the preview added nothing'


def test_a_group_the_model_does_not_have_is_refused_by_number():
    from hertzforge.service.protocol import ProtocolError

    state = _state()
    with pytest.raises(ProtocolError, match=r'Cells has element groups 1, 2; not 99'):
        _run(state, OP, {'source': 'Cells Modes', 'groups': '99', 'modes': '',
                         'quantity': 'stress', 'component': 'xx'})


def test_a_rigid_body_mode_is_drawn_at_zero_with_a_note():
    """Its stress is round-off, which its own peak would spread over the whole
    colormap as if it were a mode's stress; the stored set keeps what was read."""
    state = _state()
    added = state.project.element_shapes('Cells Modes', modes=[0, 6])
    (view,) = _run(state, 'project.color_element_shapes',
                   {'object': added, 'quantity': 'stress', 'component': 'xx'})
    values = view['element_values']['values']
    assert np.all(values[0] == 0.0) and np.abs(values[1]).max() > 0
    assert 'rigid-body mode at 0 Hz strains nothing' in view['note']
    assert np.abs(state.project[added].values('stress', 'xx', 0)).max() > 0, \
        'the round-off is kept in the set itself'
