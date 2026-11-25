"""Infer Geometry as the shell reaches it: the preview of the sensors placed
by their rigid motion, with the motions the band was read from, and Apply."""

import sys
from pathlib import Path

import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from test_motion_geometry import FOUR, KINDS, Layout

from hertzforge import Geometry, Project
from hertzforge.core.data import TimeHistory
from hertzforge.service import projects, verbs
from hertzforge.service.action_table import action_table
from hertzforge.service.ops_motion import OPERATIONS, SLIDE
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

(OP,) = OPERATIONS


@pytest.fixture(scope='module')
def layout():
    return Layout()


@pytest.fixture(scope='module')
def measured(layout):
    return layout.history()


@pytest.fixture
def project(layout, measured):
    out = Project()
    out.add('Run', TimeHistory(measured.abscissa, measured.ordinate,
                               response_dof=list(measured.response_dof),
                               ordinate_dim='acceleration', ordinate_unit='m/s**2'))
    out.add('Some Nodes', Geometry(node_id=list(FOUR),
                                   node_xyz=[layout.position[node] for node in FOUR],
                                   length_unit='m'))
    return out


def run(project, **given):
    values = OP.resolve({'source': 'Run', **given},
                        objects={name: projects.kind_of(obj) for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return OP.run(project, values)


def _rows(reading):
    return {row['label']: (row['value'], row['status']) for row in reading['rows']}


def test_the_preview_draws_the_sensors_and_changes_nothing(project, validate):
    journal = list(project.journal)
    scene, motions, table, reading = run(project)
    wire_views([scene, motions, table, reading], validate)
    assert list(project) == ['Run', 'Some Nodes'] and project.journal == journal
    assert [param.name for param in OP.params] == ['source', 'low', 'high', 'known', 'span',
                                                   'mirror', 'records', 'more', 'more_records',
                                                   'keep']
    assert OP.commits == 'keep' and OP.opens_previewed
    # a point a sensor, an arrow a channel, and a line through each one-axis sensor
    assert scene['unit'] == 'undefined' and scene['nodes'].shape == (11, 3)
    assert scene['node_ids'].tolist() == [10 * k for k in range(1, 12)]
    (points,) = scene['points']
    assert points['xyz'].shape == (11, 3) and points['colors'].shape == (11, 3)
    assert scene['arrows']['labels'] == list(project['Run'].response_dof)
    assert scene['arrows']['axes'].tolist() == ['XYZ'.index(axis) for kind in KINDS for axis in kind]
    (slides,) = scene['segments']
    assert slides['xyz'].shape == (6, 3) and slides['color'] == list(SLIDE)
    # the table: no unit with no size, and a one-axis sensor's place said
    assert table['columns'] == ['Node', 'Axes', 'X', 'Y', 'Z', 'Misfit [%]', 'Place']
    assert table['rows'][0][:2] == ['10', 'X Y Z'] and table['rows'][0][6] == ''
    assert table['rows'][8][1:2] == ['Z'] and table['rows'][8][6] == 'anywhere along its Z'
    said = _rows(reading)
    assert said['Sensors'] == ('11 placed: 5 of three axes, 3 of two axes, 3 of one axis', 'pass')
    assert said['Band'][0].endswith('the lowest in which six motions stand 5 times above the '
                                    'seventh')
    assert said['Size'][1] == 'warn' and 'nodes 20 and 40 are one apart' in said['Size'][0]
    assert said['Mirror image'][1] == 'warn' and said['Mirror image'][0].startswith('open')
    assert said['Motion not explained'][1] == 'pass'
    assert said['Not created'] == ('Apply adds the geometry', 'info')


def test_the_motions_are_stepped_across_their_bands_with_the_band_marked(project):
    _scene, motions, _table, _reading = run(project)
    assert motions['x']['scale'] == 'log' and motions['y']['unit'] is None
    assert [line['name'] for line in motions['series']] == [f'Motion {k}' for k in range(1, 9)]
    for line in motions['series']:
        assert line['shape'] == 'steps' and len(line['edges']) == len(line['y']) + 1
        assert (line['y'] > 0).all() and line['y'].max() <= 1.0
    assert max(line['y'].max() for line in motions['series']) == 1.0
    (band,) = motions['regions']
    assert band['kind'] == 'span' and band['label'] == 'Band, chosen' and 'drag' not in band
    # typed, the band is the one set, and is said as set
    _scene, again, _table, reading = run(project, low=20.0, high=60.0)
    (band,) = again['regions']
    assert (band['x0'], band['x1'], band['label']) == (20.0, 60.0, 'Band')
    assert _rows(reading)['Band'] == ('20 to 60 Hz', 'info')


def test_a_sensor_is_colored_by_what_the_fit_leaves_of_its_motion(project):
    from hertzforge.core.motion_geometry import POOR_MISFIT
    from hertzforge.service.ops_scan import AT_LIMIT, FAR, ON_SURFACE, _colors

    shades = _colors(np.array([0.0, POOR_MISFIT / 2, POOR_MISFIT, 4 * POOR_MISFIT]),
                     POOR_MISFIT / 2)
    assert shades.tolist() == [list(ON_SURFACE), list(AT_LIMIT), list(FAR), list(FAR)]
    scene, *_rest = run(project)
    # every sensor here is fitted to a fraction of a percent: all near green
    assert np.abs(scene['points'][0]['colors'].astype(int) - ON_SURFACE).max() < 25


def test_a_known_geometry_or_a_span_gives_the_size(project, layout):
    scene, _motions, table, reading = run(project, known='Some Nodes')
    said = _rows(reading)
    assert said['Size'] == ('from Some Nodes', 'pass')
    assert said['Mirror image'] == ('settled by Some Nodes', 'pass')
    assert scene['unit'] == 'm' and table['columns'][2:5] == ['X [m]', 'Y [m]', 'Z [m]']
    np.testing.assert_allclose([float(cell) for cell in table['rows'][4][2:5]],
                               layout.position[50], atol=2e-3)
    _scene, _motions, table, reading = run(project, span=0.5)
    assert _rows(reading)['Size'] == ('the span typed, between nodes 20 and 40', 'pass')
    at = {row[0]: np.array([float(cell) for cell in row[2:5]]) for row in table['rows']}
    assert np.linalg.norm(at['20'] - at['40']) == pytest.approx(0.5, rel=1e-5)


def test_mirror_draws_the_other_image(project):
    _scene, _motions, table, _reading = run(project)
    _scene, _motions, other, _reading = run(project, mirror=True)
    for row, was in zip(other['rows'], table['rows'], strict=True):
        if not row[6]:
            np.testing.assert_allclose([float(cell) for cell in row[2:5]],
                                       [-float(cell) for cell in was[2:5]], atol=1e-5)


def test_apply_adds_the_geometry(project):
    *_views, reading = run(project, known='Some Nodes', keep=True)
    assert list(project) == ['Run', 'Some Nodes', 'Run Sensors']
    assert _rows(reading)['Created'] == ('Run Sensors', 'pass')
    assert 'Not created' not in _rows(reading)
    assert project.journal[-1].startswith("project.geometry_from_motion('Run', ")
    assert project['Run Sensors'].num_nodes == 11 and project['Run Sensors'].length_unit == 'm'


def test_the_records_picked_and_more_histories_are_read(project, layout):
    _scene, _motions, table, _reading = run(project, records='1-21')
    assert [row[0] for row in table['rows']] == [str(10 * k) for k in range(1, 9)]
    again = layout.history(seed=5)
    project.add('Second', TimeHistory(again.abscissa, again.ordinate,
                                      response_dof=list(again.response_dof),
                                      ordinate_dim='acceleration', ordinate_unit='m/s**2'))
    _scene, _motions, table, reading = run(project, more='Second', records='1-21',
                                           more_records='1-21')
    assert _rows(reading)['Histories'] == ('Run, Second', 'info')
    assert len(table['rows']) == 8
    # the second history's own picks: its channels then differ from the first's
    with pytest.raises(ProtocolError, match='the histories hold different channels'):
        run(project, more='Second', records='1-21', more_records='1-18')


@pytest.mark.parametrize('given, said', [
    ({'low': 80.0, 'high': 20.0}, 'from a lower frequency to a higher one'),
    ({'low': 250.0, 'high': 350.0}, 'six rigid motions do not stand clear in 250 to 350 Hz'),
    ({'records': '1-6'}, '6 channels of translation; at least seven'),
    ({'span': 0.5, 'known': 'Some Nodes'}, 'not both'),
    ({'known': 'Some Nodes', 'mirror': True}, 'nothing for Mirror to choose'),
])
def test_what_cannot_be_read_is_refused_in_the_cores_words(project, given, said):
    journal = list(project.journal)
    for keep in (False, True):
        with pytest.raises(ProtocolError, match=said):
            run(project, keep=keep, **given)
    assert list(project) == ['Run', 'Some Nodes'] and project.journal == journal


def _offered(project, names, records=None):
    return {row['id']: row for column in action_table(project, names, records or {})['columns']
            for row in column['rows']}


def test_one_history_offers_it_and_a_geometry_beside_is_the_known_one(project):
    row = _offered(project, ['Run'], {'Run': [0, 1, 2]})[OP.id]
    assert row['ready'] and row['verb'] == 'Infer Geometry'
    assert row['fill'] == {'source': 'Run', 'records': '1-3', 'more': '', 'more_records': ''}
    assert {'records', 'more', 'more_records'} <= set(row['omit'])
    both = _offered(project, ['Run', 'Some Nodes'])[OP.id]
    assert both['ready'] and both['fill']['source'] == 'Run'
    assert both['fill']['known'] == 'Some Nodes'
    assert set(verbs.modules_of(OP)) == {'signals', 'sysid', 'random', 'modal'}


def test_several_histories_are_read_together_with_a_geometry_beside(project, measured):
    for name in ('Second', 'Third'):
        project.add(name, TimeHistory(measured.abscissa, measured.ordinate,
                                      response_dof=list(measured.response_dof),
                                      ordinate_dim='acceleration', ordinate_unit='m/s**2'))
    row = _offered(project, ['Run', 'Second', 'Third'], {'Second': [0, 1]})[OP.id]
    assert row['ready']
    assert row['fill'] == {'source': 'Run', 'more': 'Second\nThird', 'records': '',
                           'more_records': '1-2\n'}
    row = _offered(project, ['Run', 'Second', 'Some Nodes', 'Third'])[OP.id]
    assert row['ready'] and row['fill']['known'] == 'Some Nodes'
    assert (row['fill']['source'], row['fill']['more']) == ('Run', 'Second\nThird')
    # the operations that copy each history take no geometry with them
    assert 'project.rescale' not in _offered(project, ['Run', 'Second', 'Some Nodes'])
    assert _offered(project, ['Run', 'Second'])['project.rescale']['ready']


def test_a_history_with_too_few_translations_says_why_not(project, measured):
    project.add('Few', TimeHistory(measured.abscissa, measured.ordinate[:6],
                                   response_dof=list(measured.response_dof[:6]),
                                   ordinate_dim='acceleration', ordinate_unit='m/s**2'))
    row = _offered(project, ['Few'])[OP.id]
    assert not row['ready']
    assert row['reason'] == 'It needs seven channels of translation with their units declared'
