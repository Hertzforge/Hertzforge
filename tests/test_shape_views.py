"""A lone shape set's three views: mode table, MAC and shape values.

Brandon, 2026-10-22: a shape set showed its mode table, which could be
toggled, and its MAC, which could not. Now each of the three views has
a toggle on the table bar: the mode table, the MAC, and the shape
values (a row per DOF, headed by it, and a column per mode, M1, M2,
...). One always stays up, and the values are offered only when there
are few enough of them for a table.
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.shapes import ShapeSet


def _shapes():
    return ShapeSet([10.0, 25.0, 40.0], [0.01, 0.02, 0.03],
                    ['101X+', '101Y+', '102Z+', '103Z+'],
                    [[1.0, 2.0, 3.0, 4.0],
                     [0.5, -0.5, 0.25, 0.0],
                     [1e-7, 2.0, 0.0, -3.5]])


def _rigid_project():
    """Four nodes of a square and their rigid shapes, with a channel
    table of twelve references (two of them written negative), four Z
    responses, and a monitor the set lacks; the responses and the
    monitor have Control checked."""
    from hertzforge.core.rigid import MassProperties, rigid_body_shapes

    geometry = hertzforge.Geometry(
        [1, 2, 3, 4], [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0]],
        length_unit='m')
    references = ['1X+', '1Y+', '1Z+', '2X-', '2Y+', '2Z+', '3X+', '3Y-',
                  '3Z+', '4X+', '4Y+', '4Z+']
    responses = ['1Z+', '2Z+', '3Z+', '4Z+']
    dofs = references + responses + ['9Z+']
    table = hertzforge.ChannelTable({
        'channel': list(range(1, len(dofs) + 1)),
        'node': [dof[:-2] for dof in dofs],
        'direction': [dof[-2:] for dof in dofs],
        'unit': ['lbf'] * 12 + ['g'] * 5,
        'role': ['reference'] * 12 + ['response'] * 4 + ['monitor'],
        # the monitor is a control channel too, so the control choice is
        # not the responses under another name
        'control': ['False'] * 12 + ['True'] * 5})
    project = hertzforge.Project('p')
    project.add('Square', geometry)
    project.add('Channels', table)
    project.add('Rigid', rigid_body_shapes(geometry, MassProperties((0.5, 0.5, 0))))
    project.link('Square', 'Channels')
    project.link('Square', 'Rigid')
    return project, references, responses


def test_the_shapes_at_a_choice_of_dofs_honor_the_sign():
    from hertzforge.core.transform import matrix_at

    project, _references, _responses = _rigid_project()
    shapes = project['Rigid']
    view = matrix_at(shapes, ['2X+', '2X-', '9Z+'])
    assert view.rows == ['2X+', '2X-', '9Z+'] and view.columns[0] == 'M1'
    assert np.allclose(view.values[1], -view.values[0]), '2X- is minus 2X+'
    assert np.isnan(view.values[2]).all() and view.missing == ['9Z+']
    whole = matrix_at(shapes)
    assert np.array_equal(whole.values, np.asarray(shapes.shape_matrix).T)


def test_the_transformations_are_what_the_transform_applies():
    """Φ⁺ times a motion is the modal response `to_modal` makes, and Φᵀ
    times a force is the modal force, at the same DOFs and signs."""
    from hertzforge.core.transform import matrix_at, to_modal

    project, references, _responses = _rigid_project()
    shapes = project['Rigid']
    rng = np.random.default_rng(4)
    t = np.arange(64) / 64.0
    values = rng.standard_normal((12, 64))
    for form, quantity in (('response', 'acceleration'), ('reference', 'force')):
        view = matrix_at(shapes, references, form)
        assert view.rows == [f'M{k}' for k in range(1, 7)]
        assert view.columns == references
        history = hertzforge.TimeHistory(
            t, values, response_dof=references, ordinate_dim=quantity)
        modal, _report = to_modal(history, shapes)
        assert np.allclose(view.values @ values, modal.ordinate), form
    response = matrix_at(shapes, references, 'response').values
    phi = matrix_at(shapes, references).values
    assert np.allclose(response @ phi, np.eye(6)), 'Φ⁺Φ = I'


def test_too_few_dofs_refuse_the_response_transformation():
    from hertzforge.core.transform import matrix_at

    project, _references, responses = _rigid_project()
    with pytest.raises(ValueError, match='resolve only 3 of 6 modes'):
        matrix_at(project['Rigid'], responses, 'response')
    assert matrix_at(project['Rigid'], responses, 'reference').values.shape == (6, 4)
    with pytest.raises(ValueError, match='not a form'):
        matrix_at(project['Rigid'], responses, 'inverse')
    # 'force' was the reference transformation's name until 2026-10-23
    with pytest.raises(ValueError, match='not a form'):
        matrix_at(project['Rigid'], responses, 'force')
    with pytest.raises(ValueError, match='none of these'):
        matrix_at(project['Rigid'], ['9Z+'], 'reference')


def test_a_tables_channels_are_chosen_by_one_rule():
    project, references, responses = _rigid_project()
    table = project['Channels']
    assert table.choices() == ['all', 'control', 'reference', 'response',
                               'monitor']
    assert [table.dof_strings()[i] for i in table.chosen('reference')] == references
    assert [table.dof_strings()[i] for i in table.chosen('control')] == \
        responses + ['9Z+']
    with pytest.raises(ValueError, match='not a choice'):
        table.chosen('drives')


def test_a_script_reads_a_choice_of_channels_in_a_form():
    project, references, _responses = _rigid_project()
    headers, rows = project.table('Rigid', values=True, dofs='reference',
                                  form='reference')
    assert headers == ['Mode'] + references
    assert rows[0][0] == 'M1' and rows[0][4] == '-1', "M1 at 2X- is -1"
    headers, rows = project.table('Rigid', values=True, dofs='monitor')
    assert rows == [['9Z+ (not in the set)', '', '', '', '', '', '']]
    project.unlink('Channels')
    with pytest.raises(ValueError, match='no channel table'):
        project.table('Rigid', values=True, dofs='reference')
