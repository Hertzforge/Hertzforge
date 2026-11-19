"""Synthesize Modal FRFs and Expand to Physical Responses at a channel
table, as the shell reaches them: the forms, what they draw and say, and
the action table's rows (Brandon, 2026-11-11, from the same workflow in
the older application)."""

from __future__ import annotations

import numpy as np
import pytest
from scipy.linalg import eigh

import hertzforge
from hertzforge.core.shapes import ShapeSet
from hertzforge.service import projects
from hertzforge.service.action_table import action_table
from hertzforge.service.ops_generate import OPERATIONS as GENERATE
from hertzforge.service.ops_project import OPERATIONS as PROJECT
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

OPS = {op.id: op for op in (*GENERATE, *PROJECT)}


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return op.run(project, values)


def _shapes():
    mass = np.diag([2.0, 1.0])
    stiffness = np.array([[3e5, -1e5], [-1e5, 1e5]])
    lam, phi = eigh(stiffness, mass)
    return ShapeSet(np.sqrt(lam) / (2 * np.pi), [0.02, 0.03], ['1X+', '2X+'], phi.T,
                    mass_unit='kg')


def _table(**changes):
    columns = {'channel': [1, 2, 3, 4], 'node': ['1', '2', '1', '2'],
               'direction': ['X+', 'X-', 'X+', 'X+'],
               'role': ['reference', 'response', 'monitor', 'monitor'],
               'unit': ['N', 'g', 'in/s', 'degC'],
               'channel_type': ['force', 'acceleration', 'velocity', 'temperature']}
    columns.update(changes)
    return hertzforge.ChannelTable(columns)


@pytest.fixture
def project():
    made = hertzforge.Project()
    made.add('Modes', _shapes())
    made.add('Channels', _table())
    return made


def test_the_form_opens_at_a_test_frame_and_reads_what_the_pane_reads(project, validate):
    op = OPS['project.synthesize_modal_frfs']
    defaults = {p.name: p.default for p in op.params}
    assert defaults['sample_rate'] == 8192.0 and defaults['frame_length'] == 8192
    assert defaults['response'] == 'acceleration'
    views = run(project, 'project.synthesize_modal_frfs', source='Modes', sample_rate=64.0,
                frame_length=64, response='velocity', modes='')
    wire_views(views, validate)
    assert [view['view'] for view in views] == ['waterfall', 'reading']
    made = project['Modes Modal FRFs']
    assert made.response_dof == ['M1', 'M2'] and made.abscissa.size == 33
    rows = {row['label']: row for row in views[1]['rows']}
    assert rows['Line spacing']['value'] == 1.0 and rows['Lines']['value'] == 33.0
    assert rows['Nyquist']['value'] == 32.0
    assert 'above the Nyquist' in rows['Note']['value'], 'both modes are above 32 Hz, and kept'
    assert "project.synthesize_modal_frfs('Modes'" in project.journal[-1]


def test_modes_typed_are_the_modes_synthesized(project):
    run(project, 'project.synthesize_modal_frfs', source='Modes', sample_rate=1024.0,
        frame_length=1024, response='acceleration', modes='2')
    assert project['Modes Modal FRFs'].response_dof == ['M2']


def test_a_complex_set_is_not_offered_and_a_real_one_is():
    project = hertzforge.Project()
    project.add('Real', _shapes())
    project.add('Complex', ShapeSet([10.0], [0.02], ['1X+'], [[1.0 + 1j]], modal_mass=[1j]))
    assert 'synthesize_modal_frfs' in projects.verbs_of(project, 'Real')
    assert 'synthesize_modal_frfs' not in projects.verbs_of(project, 'Complex')
    rows = {row['id']: row for row in action_table(project, ['Complex'], {})['columns'][0]['rows']}
    assert not rows['project.synthesize_modal_frfs']['ready']
    rows = {row['id']: row for row in action_table(project, ['Real'], {})['columns'][0]['rows']}
    assert rows['project.synthesize_modal_frfs']['ready']


def test_expand_at_a_table_draws_the_frfs_and_says_what_it_used(project, validate):
    modal = project.synthesize_modal_frfs('Modes', sample_rate=1024.0, frame_length=512)
    views = run(project, 'project.expand', source=modal, shapes='Channels', tolerance=2.0)
    wire_views(views, validate)
    made = project[f'{modal} @ Channels']
    assert made.response_dof == ['2X-', '1X+'] and made.reference_dof == ['1X+', '1X+']
    rows = {row['label']: row['value'] for row in views[1]['rows']}
    assert rows['Modes'] == 2.0 and rows['References'] == 1.0 and rows['Responses'] == 2.0
    assert rows['FRFs'] == 2.0 and rows['Through'] == 'Modes'
    assert rows['Shapes'] == "read at the set's own DOFs"
    assert rows['Skipped'] == '1 temperature'
    expected = _shapes().synthesize_frf(made.abscissa[1:], ['1X+'], ['1X+'], power=1)[0]
    assert np.allclose(made.ordinate[1, 1:], expected, rtol=1e-12, atol=0)


def test_expand_through_a_set_is_as_before(project):
    modal = project.synthesize_modal_frfs('Modes', sample_rate=256.0, frame_length=256)
    views = run(project, 'project.expand', source=modal, shapes='Modes', tolerance=2.0)
    assert [view['view'] for view in views] == ['waterfall']


def test_a_table_with_no_force_reference_is_refused_in_the_cores_words(project):
    modal = project.synthesize_modal_frfs('Modes', sample_rate=256.0, frame_length=256)
    project['Channels'] = _table(channel_type=['acceleration'] * 4)
    with pytest.raises(ProtocolError, match='no reference channel measuring force'):
        run(project, 'project.expand', source=modal, shapes='Channels', tolerance=2.0)


def test_the_action_table_offers_expand_for_modal_frfs_with_a_table(project):
    modal = project.synthesize_modal_frfs('Modes', sample_rate=256.0, frame_length=256)
    rows = {row['id']: row for row in
            action_table(project, [modal, 'Channels'], {})['columns'][0]['rows']}
    assert rows['project.expand']['ready']
