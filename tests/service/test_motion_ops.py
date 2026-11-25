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
    assert table['columns'] == ['Node', 'Axes', 'X', 'Y', 'Z', 'Misfit [%]', 'Note']
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


def _labeled(project, measured, change):
    project.add('Labeled', TimeHistory(
        measured.abscissa, measured.ordinate,
        response_dof=[change.get(dof, dof) for dof in measured.response_dof],
        ordinate_dim='acceleration', ordinate_unit='m/s**2'))
    return run(project, source='Labeled')


def test_a_left_handed_sensor_is_said_in_its_row_and_drawn_as_it_reads(project, measured):
    scene, _motions, table, reading = _labeled(project, measured,
                                               {'20X+': '20Y+', '20Y+': '20X+'})
    notes = {row[0]: row[6] for row in table['rows']}
    assert notes['20'] == 'left-handed as labeled' and notes['10'] == ''
    assert notes['90'] == 'anywhere along its Z'
    warned = [row['value'] for row in reading['rows']
              if row['label'] == 'Note' and row['status'] == 'warn']
    assert len(warned) == 1 and warned[0].startswith('Sensor 20: its X, Y and Z are a left-')
    # placed, and not passed: its DOFs are not as labeled
    assert _rows(reading)['Sensors'][1] == 'warn'
    # the three arrows of sensor 20 as the data has them: a left-handed set
    labels = scene['arrows']['labels']
    rows = [labels.index(dof) for dof in ('20X+', '20Y+', '20Z+')]
    assert np.linalg.det(scene['arrows']['directions'][rows].astype(float)) == pytest.approx(
        -1.0, abs=1e-3)
    others = [labels.index(dof) for dof in ('10X+', '10Y+', '10Z+')]
    assert np.linalg.det(scene['arrows']['directions'][others].astype(float)) == pytest.approx(
        1.0, abs=1e-3)


def test_a_channel_left_out_of_its_node_has_no_arrow_and_is_said(project, measured):
    scene, _motions, table, reading = _labeled(project, measured,
                                               {'20Z+': '40Z+', '40Z+': '20Z+'})
    labels = scene['arrows']['labels']
    assert '20Z+' not in labels and '40Z+' not in labels and len(labels) == 22
    notes = {row[0]: (row[1], row[6]) for row in table['rows']}
    assert notes['20'] == ('X Y', '20Z+ left out') and notes['40'] == ('X Y', '40Z+ left out')
    said = [row['value'] for row in reading['rows'] if row['label'] == 'Note']
    assert any('20Z+ does not measure at node 20 with 20X+ and 20Y+: its line passes through '
               'sensor 40' in note for note in said)


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


# ---- tests on a table: the sensors are turned and not placed ----------------

SOME = (10, 20, 30, 60, 90)
TABLE = {'source': 'Table X', 'more': 'Table Y\nTable Z'}


@pytest.fixture(scope='module')
def on_table(layout):
    return layout.table_tests()


@pytest.fixture
def table_project(layout, on_table):
    out = Project()
    for axis, one in zip('XYZ', on_table, strict=True):
        out.add(f'Table {axis}', TimeHistory(one.abscissa, one.ordinate,
                                             response_dof=list(one.response_dof),
                                             ordinate_dim='acceleration', ordinate_unit='m/s**2'))
    out.add('Layout', layout.geometry(SOME))
    return out


def test_table_tests_preview_the_sensors_turned_on_a_grid(table_project, validate):
    journal = list(table_project.journal)
    scene, motions, table, reading = run(table_project, **TABLE)
    wire_views([scene, motions, table, reading], validate)
    assert table_project.journal == journal and len(table_project) == 4
    # no positions: the sensors are set out one apart, four to a row, in the order of the data
    assert scene['title'] == ('Sensors of Table X, Table Y, Table Z as they are turned, set out '
                              'on a grid')
    assert scene['unit'] == 'undefined' and 'segments' not in scene
    assert scene['nodes'][:5].tolist() == [[0, 0, 0], [1, 0, 0], [2, 0, 0], [3, 0, 0], [0, -1, 0]]
    assert scene['arrows']['labels'] == list(table_project['Table X'].response_dof)
    # three motions are the measured ones, not six
    assert [line['role'] for line in motions['series']] == ['measured'] * 3 + ['other'] * 5
    assert table['columns'] == ['Node', 'Axes', 'X reads along', 'Y reads along',
                                'Z reads along', 'Misfit [%]', 'Note']
    # the frame is the first three-axis sensor's, and a one-axis sensor has one direction
    assert table['rows'][0][:5] == ['10', 'X Y Z', '(1.000, 0.000, 0.000)',
                                    '(0.000, 1.000, 0.000)', '(0.000, 0.000, 1.000)']
    assert table['rows'][8][1:4] == ['Z', '', ''] and table['rows'][8][4].startswith('(')
    said = _rows(reading)
    assert said['Sensors'] == (('11 turned, not placed: 5 of three axes, 3 of two axes, 3 of '
                                'one axis'), 'pass')
    assert said['Band'][0].endswith('the lowest in which three motions stand 5 times above the '
                                    'fourth and stay the same three')
    assert said['Positions'] == (('not in the data, which holds three translations and no '
                                  'rotation: a geometry that holds the nodes gives them'), 'warn')
    assert said['Frame'] == (('not known: the axes of sensor 10 are taken as the global ones. '
                              'A geometry that holds the nodes gives it'), 'warn')
    assert said['Mirror image'] == ('settled', 'pass')
    assert said['Motion not explained'][1] == 'pass'
    assert said['Motion not explained'][0].endswith(' %: most directions are within about 0.0 '
                                                    'degrees, and the worst about twice that')
    assert said['Not created'] == (('Apply needs a geometry that holds the nodes: the data '
                                    'does not place the sensors'), 'info')
    assert 'Size' not in said and 'One-axis sensors' not in said


def test_a_geometry_that_holds_the_nodes_places_them_and_shows_what_is_off(table_project, layout,
                                                                           on_table):
    scene, _motions, table, reading = run(table_project, known='Layout', **TABLE)
    assert scene['title'].endswith('as they are turned, at their nodes in Layout')
    assert scene['unit'] == 'm' and scene['node_ids'].tolist() == list(SOME)
    np.testing.assert_allclose(scene['nodes'], [layout.position[node] for node in SOME], atol=1e-6)
    assert scene['arrows']['labels'] == [dof for dof in layout.dofs if int(dof[:-2]) in SOME]
    assert table['columns'][5:] == ['Misfit [%]', 'Off [deg]', 'Note']
    off = {row[0]: row[6] for row in table['rows']}
    assert all(float(off[str(node)]) < 0.25 for node in SOME) and off['40'] == ''
    said = _rows(reading)
    assert said['Positions'] == (('not in the data, which holds three translations and no '
                                  'rotation: 5 of 11 sensors are at their nodes in Layout'), 'info')
    assert said['Frame'] == (('fitted to Layout: 12 of 12 channels read within 15 degrees of '
                              'their directions there'), 'pass')
    assert said['Not created'] == (('Apply adds the geometry, of the sensors the known '
                                    'geometry holds'), 'info')
    # a one-axis sensor wired backwards fits as well as before, and the geometry shows it
    for axis, one in zip('XYZ', on_table, strict=True):
        dofs = ['90Z-' if dof == '90Z+' else dof for dof in one.response_dof]
        table_project.add(f'Wired {axis}', TimeHistory(
            one.abscissa, one.ordinate, response_dof=dofs, ordinate_dim='acceleration',
            ordinate_unit='m/s**2'))
    _scene, _motions, table, reading = run(table_project, source='Wired X',
                                           more='Wired Y\nWired Z', known='Layout')
    row = next(row for row in table['rows'] if row[0] == '90')
    assert float(row[6]) > 179.0 and row[7] == 'Z reversed'
    said = _rows(reading)
    assert said['Sensors'][1] == 'warn' and said['Frame'][1] == 'warn'
    assert said['Frame'][0].startswith('fitted to Layout: 11 of 12 channels')
    assert ('90Z- reads opposite to its direction in the geometry: its polarity is reversed',
            'warn') in [(row['value'], row['status']) for row in reading['rows']
                        if row['label'] == 'Note']


def test_a_channel_left_out_on_the_table_has_no_arrow_and_is_in_its_row(table_project, on_table):
    for axis, one in zip('XYZ', on_table, strict=True):
        swap = {'40Z+': '90Z+', '90Z+': '40Z+'}
        table_project.add(f'Crossed {axis}', TimeHistory(
            one.abscissa, one.ordinate, response_dof=[swap.get(dof, dof) for dof in one.response_dof],
            ordinate_dim='acceleration', ordinate_unit='m/s**2'))
    scene, _motions, table, reading = run(table_project, source='Crossed X',
                                          more='Crossed Y\nCrossed Z')
    assert '40Z+' not in scene['arrows']['labels'] and len(scene['arrows']['labels']) == 23
    row = next(row for row in table['rows'] if row[0] == '40')
    assert row[1] == 'X Y' and row[4] == '' and row[-1] == '40Z+ left out'
    assert _rows(reading)['Sensors'][1] == 'warn'


def test_rocking_with_the_table_is_said_as_degrees_and_the_axes_are_off_by_about_that(layout):
    rocked = Project()
    names = [rocked.add(f'Table {axis}', one)
             for axis, one in zip('XYZ', layout.table_tests(rocking=0.03), strict=True)]
    rocked.add('Layout', layout.geometry())
    *_views, reading = run(rocked, known='Layout', **TABLE)
    value, status = _rows(reading)['Motion not explained']
    said = float(value.split('about ')[1].split(' degrees')[0])
    off = sorted(rocked.motion_sensors(*names, known='Layout').off.values())
    assert status == 'pass' and 0.8 < said < 1.5
    # most within it, and the worst about twice it
    assert off[len(off) // 2] < said < off[-1] < 3 * said


def test_apply_on_table_tests_needs_the_geometry_and_makes_its_nodes(table_project, layout):
    journal = list(table_project.journal)
    with pytest.raises(ProtocolError, match='pick a geometry that holds the nodes'):
        run(table_project, keep=True, **TABLE)
    for given, why in (({'span': 0.5}, 'no layout for a span to size'),
                       ({'mirror': True, 'known': 'Layout'}, 'no layout for Mirror to reflect')):
        for keep in (False, True):
            with pytest.raises(ProtocolError, match=why):
                run(table_project, keep=keep, **given, **TABLE)
    assert len(table_project) == 4 and table_project.journal == journal
    *_views, reading = run(table_project, keep=True, known='Layout', **TABLE)
    assert _rows(reading)['Created'] == ('Table X Sensors', 'pass')
    made = table_project['Table X Sensors']
    assert list(made.node_id) == list(SOME) and made.length_unit == 'm'
    assert np.degrees(np.arccos(made.dof_direction('60X+') @ layout.frame[60][0])) < 0.25
