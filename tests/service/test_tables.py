"""Objects' tables, matching two shape sets' modes, and a shape set's
values at a choice of DOFs."""

from __future__ import annotations

from pathlib import Path

import pytest

from hertzforge.service import projects
from hertzforge.service import views as v
from hertzforge.service.ops_project import OPERATIONS as PROJECT_OPERATIONS
from hertzforge.service.ops_tables import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in OPERATIONS + PROJECT_OPERATIONS}


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return op.run(project, values)


def test_a_mode_table_takes_descriptions(fitted, validate):
    """The Description column is typed into (Brandon, 2026-11-05): the
    edit names the mode by its row, the text is kept on the set and
    journaled, and a row the table does not have is refused."""
    project = fitted
    (table,) = run(project, 'project.table', object='Three')
    assert table['edit']['op'] == 'project.describe_mode'
    assert table['edit']['columns'] == [None, None, None, 'description']
    (after,) = run(project, 'project.describe_mode', shapes='Three', row=2, column='description',
                   value='  first bending ')
    assert after['rows'][1][3] == 'first bending'
    assert project['Three'].description[1] == 'first bending'
    assert any('set_description' in line for line in project.journal)
    wire_views([after], validate)
    with pytest.raises(ProtocolError, match='no mode 9'):
        run(project, 'project.describe_mode', shapes='Three', row=9, column='description',
            value='x')
    run(project, 'project.describe_mode', shapes='Three', row=2, column='description', value='')
    assert project['Three'].description[1] == ''


@pytest.fixture(scope='module')
def fitted():
    """The plate's modal run with two fits of its modes: three, then two
    in the same band, so the two sets share their first modes."""
    state, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    project = state.project
    run(project, 'project.compute_frfs')
    run(project, 'project.fit_modes', low_hz=300.0, high_hz=1300.0, count=3)
    project.rename('Time History Hv FRFs Modes', 'Three')
    run(project, 'project.fit_modes', low_hz=300.0, high_hz=1300.0, count=2)
    project.rename('Time History Hv FRFs Modes', 'Two')
    return project


def test_a_table_view_is_rows_of_text_under_its_columns(validate):
    view = v.table('Modes', ['Mode', 'Frequency [Hz]'], [[1, 439.2]])
    wire_views([view], validate)
    assert view['rows'] == [['1', '439.2']]
    with pytest.raises(ValueError, match='a row of 1 cells under 2 columns'):
        v.table('Modes', ['Mode', 'Frequency [Hz]'], [['1']])


def test_a_shape_sets_table_lists_its_modes(fitted, validate):
    (view,) = run(fitted, 'project.table', object='Three')
    wire_views([view], validate)
    assert view['columns'][:3] == ['Mode', 'Frequency [Hz]', 'Damping [%]']
    assert [row[0] for row in view['rows']] == ['1', '2', '3']
    assert float(view['rows'][0][1]) == pytest.approx(fitted['Three'].frequency[0], abs=1e-4)


def test_a_channel_tables_table_lists_its_channels(fitted):
    (view,) = run(fitted, 'project.table', object='Channel Table')
    assert len(view['rows']) == fitted['Channel Table'].num_channels


def test_matching_keeps_the_pairs_as_an_object(fitted, validate):
    (view,) = run(fitted, 'project.match_modes', first='Three', second='Two', name='Pairs')
    wire_views([view], validate)
    matched = fitted['Pairs']
    assert projects.kind_of(matched) == 'matched_modes' and matched.num_matches >= 2
    assert len(view['rows']) == matched.num_matches
    assert 'MAC' in view['columns']
    assert all(mac >= 0.7 for mac in matched.macs)


def test_matching_a_set_to_itself_or_finding_nothing_is_refused(fitted):
    with pytest.raises(ProtocolError, match='not itself'):
        run(fitted, 'project.match_modes', first='Three', second='Three')
    before = set(fitted)
    # past the param's own limit of 1, as a script could ask
    with pytest.raises(ProtocolError, match='no mode of Three has a MAC of 1.01'):
        OPS['project.match_modes'].run(fitted, {'first': 'Three', 'second': 'Two',
                                                'threshold': 1.01, 'name': 'Nothing'})
    assert set(fitted) == before


def test_deleting_matches_takes_out_the_rows_named(fitted):
    run(fitted, 'project.match_modes', first='Three', second='Two', name='To Trim')
    count = fitted['To Trim'].num_matches
    (view,) = run(fitted, 'project.delete_matches', matched='To Trim', rows='1')
    assert fitted['To Trim'].num_matches == len(view['rows']) == count - 1
    (said,) = run(fitted, 'project.delete_matches', matched='To Trim', rows=f'1-{count - 1}')
    assert 'To Trim' not in fitted, 'the last rows take the object with them'
    assert said['rows'][0]['value'] == 'To Trim, with its last row'


def test_shape_values_in_each_form(fitted, validate):
    (shapes,) = run(fitted, 'project.shape_values', shapes='Three')
    wire_views([shapes], validate)
    assert shapes['columns'] == ['DOF', 'M1', 'M2', 'M3']
    dofs = [row[0] for row in shapes['rows']]
    (response,) = run(fitted, 'project.shape_values', shapes='Three', form='response',
                      dofs=' '.join(dofs[:4]))
    assert response['columns'][0] == 'Mode' and len(response['columns']) == 5
    assert len(response['rows']) == 3
    (some,) = run(fitted, 'project.shape_values', shapes='Three', dofs=f'{dofs[0]}, 999X+')
    assert some['rows'][1][0] == '999X+ (not in the set)' and some['rows'][1][1] == ''
