"""TRAC Overlay as the shell reaches it: two time histories picked, the
form previews the score table, the first station's two traces overlaid
with the scoring window marked and a summary; a row picked in the table
draws its station; Apply saves the comparison. A saved comparison picked
in the tree draws the same reading, the stations picked under it
overlaid, and Scoring Window keeps a new window on it."""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.data import TimeComparison, TimeHistory
from hertzforge.service import projects, show
from hertzforge.service.action_table import action_table
from hertzforge.service.ops_settings import settings_of
from hertzforge.service.ops_time_compare import NOTHING_FITTED, OPERATIONS
from hertzforge.service.protocol import ProtocolError
from hertzforge.service.show import PREFS

from .conftest import wire_views

OPS = {op.id: op for op in OPERATIONS}


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return op.run(project, values)


@pytest.fixture
def project():
    """A test at 1 kHz with two accelerations and a force, and a model at
    2 kHz with the two accelerations: -2 times the test at 101X+ and half
    of it at 102Z-."""
    t = np.arange(1000) / 1000.0
    fine = np.arange(2000) / 2000.0
    made = hertzforge.Project()
    made.add('Test', TimeHistory(
        t, [np.sin(2 * np.pi * 7 * t), np.cos(2 * np.pi * 11 * t), np.sin(2 * np.pi * 3 * t)],
        response_dof=['101X+', '102Z-', '101X+'],
        ordinate_dim=['acceleration', 'acceleration', 'force']))
    made.add('Model', TimeHistory(
        fine, [-2.0 * np.sin(2 * np.pi * 7 * fine), 0.5 * np.cos(2 * np.pi * 11 * fine)],
        response_dof=['101X+', '102Z-'], ordinate_dim=['acceleration', 'acceleration']))
    return made


def test_the_preview_scores_every_station_draws_the_first_and_adds_nothing(project, validate):
    views = run(project, 'project.compare_time_histories', first='Test', second='Model')
    wire_views(views, validate)
    assert [view['view'] for view in views] == ['spectrum', 'table', 'reading']
    plot, table, reading = views
    assert [s['name'] for s in plot['series']] == ['101X+ Test', '101X+ Model']
    assert plot['series'][1]['follows'] == 0
    assert plot['title'].startswith('101X+: TRAC 1.000, sTRAC -0.500')
    assert plot['y']['unit'] == 'm/s²'
    assert plot['regions'] == [{'x0': 0.0, 'x1': 0.999, 'kind': 'span',
                                'label': 'Scoring window'}]
    assert table['columns'] == ['Station', 'DOF', 'Quantity', 'TRAC', 'sTRAC', 'Correlation',
                                'RMS ratio', 'Note']
    assert table['rows'] == [['1', '101X+', 'acceleration', '1.000', '-0.500', '-1.000',
                              '2.000', ''],
                             ['2', '102Z-', 'acceleration', '1.000', '2.000', '1.000',
                              '0.500', '']]
    assert table['select']['op'] == 'project.compare_time_histories'
    assert (table['select']['param'], table['select']['column']) == ('station', 0)
    rows = {row['label']: row for row in reading['rows']}
    assert rows['Stations scored']['value'] == '2 of 2'
    assert rows['Mean TRAC']['value'] == pytest.approx(1.0)
    assert rows['Note']['value'] == NOTHING_FITTED
    assert list(project) == ['Test', 'Model']


def test_a_row_picked_draws_its_station(project):
    first = run(project, 'project.compare_time_histories', first='Test', second='Model')
    select = first[1]['select']
    plot, _table, _reading = run(project, select['op'], **select['params'], station=2)
    assert [s['name'] for s in plot['series']] == ['102Z- Test', '102Z- Model']
    assert plot['title'].startswith('102Z-: TRAC 1.000, sTRAC 2.000')


def test_records_picked_under_the_test_take_only_their_partners(project):
    _plot, table, _reading = run(project, 'project.compare_time_histories', first='Test',
                                 second='Model', first_records='2')
    assert [row[1] for row in table['rows']] == ['102Z-']


def test_a_window_inside_the_clock_is_scored_and_one_outside_is_refused(project):
    _plot, table, reading = run(project, 'project.compare_time_histories', first='Test',
                                second='Model', whole=False, start=0.2, stop=0.6)
    assert table['title'] == 'Preview: scores, 0.2 s to 0.6 s'
    assert {row['label']: row['value'] for row in reading['rows']}['Samples in the window'] == 401
    with pytest.raises(ProtocolError, match='must lie inside the shared clock'):
        run(project, 'project.compare_time_histories', first='Test', second='Model',
            whole=False, start=0.2, stop=3.0)


def test_one_history_twice_and_an_unpaired_station_are_refused_in_words(project):
    with pytest.raises(ProtocolError, match='pick two time histories'):
        run(project, 'project.compare_time_histories', first='Test', second='Test')
    t = np.arange(1000) / 1000.0
    project.add('Short Model', TimeHistory(t, [np.sin(t)], response_dof=['101X+'],
                                           ordinate_dim=['acceleration']))
    with pytest.raises(ProtocolError, match=r'102Z- acceleration: 1 record \(Test\)'):
        run(project, 'project.compare_time_histories', first='Test', second='Short Model')


def test_a_dead_station_is_said_not_scored_wherever_it_is_read(project):
    model = project['Model']
    model.ordinate[1] = 0.0
    plot, table, reading = run(project, 'project.compare_time_histories', first='Test',
                               second='Model', station=2)
    assert plot['title'].startswith('102Z-: not scored, a trace is zero in the window')
    assert table['rows'][1][3:] == ['', '', '', '', 'a trace is zero in the window']
    rows = {row['label']: row for row in reading['rows']}
    assert (rows['Stations scored']['value'], rows['Stations scored']['status']) == ('1 of 2',
                                                                                     'warn')
    assert rows['Lowest TRAC']['value'] == '1.000 at 101X+'
    assert rows['Mean TRAC']['value'] == pytest.approx(1.0)  # over the station scored alone


def test_nothing_scored_says_so_and_reports_no_mean(project):
    project['Model'].ordinate[:] = 0.0
    _plot, _table, reading = run(project, 'project.compare_time_histories', first='Test',
                                 second='Model')
    rows = {row['label']: row for row in reading['rows']}
    assert rows['Stations scored']['value'] == '0 of 2'
    assert rows['Mean TRAC']['value'] == 'not read: no station could be scored'


def test_apply_saves_the_comparison_and_draws_its_reading(project, validate):
    views = run(project, 'project.compare_time_histories', first='Test', second='Model',
                keep=True)
    wire_views(views, validate)
    assert list(project) == ['Test', 'Model', 'Time Response Comparison']
    assert isinstance(project['Time Response Comparison'], TimeComparison)
    table = views[1]
    assert table['select'] == {'op': 'project.time_comparison',
                               'params': {'comparison': 'Time Response Comparison',
                                          'flat': True},
                               'param': 'station', 'column': 0}


def test_a_saved_comparison_picked_in_the_tree_draws_the_stations_picked_under_it(project,
                                                                                  validate):
    name = project.compare_time_histories('Test', 'Model')
    views, _notes = show._alone(project, name, [2, 3], {**PREFS, 'flat': True})
    wire_views(views, validate)
    plot, table, _reading = views
    assert [s['name'] for s in plot['series']] == ['102Z- Test', '102Z- Model']
    assert len(table['rows']) == 2
    drag = plot['regions'][0]['drag']
    assert drag == {'op': 'project.set_time_comparison_window',
                    'params': {'source': name, 'station': 2}, 'low': 'start', 'high': 'stop'}
    both, _notes = show._alone(project, name, [0, 3], {**PREFS, 'flat': True})
    assert [s['name'] for s in both[0]['series']] == ['101X+ Test', '101X+ Model',
                                                      '102Z- Test', '102Z- Model']
    assert both[0]['title'].startswith('2 of 2 picked stations')


def test_scoring_window_is_kept_and_the_form_opens_on_it(project):
    name = project.compare_time_histories('Test', 'Model')
    _plot, table, _reading = run(project, 'project.set_time_comparison_window',
                                 source=name, start=0.25, stop=0.75)
    assert table['title'] == f'{name}: scores, 0.25 s to 0.75 s'
    assert (project[name].start, project[name].stop) == (0.25, 0.75)
    assert settings_of(project[name]) == {'time_comparison': {'start': 0.25, 'stop': 0.75}}
    with pytest.raises(ProtocolError, match='ends after it starts'):
        run(project, 'project.set_time_comparison_window', source=name, start=0.5, stop=0.4)
    assert (project[name].start, project[name].stop) == (0.25, 0.75)


def _rows(table, column=0):
    return {row['verb']: row for row in table['columns'][column]['rows']}


def test_the_action_table_offers_trac_beside_two_histories_and_scores_for_a_comparison(
        project):
    rows = _rows(action_table(project, ['Test', 'Model'], {}))
    assert rows['TRAC Overlay']['ready']
    assert rows['TRAC Overlay']['fill']['first'] == 'Test'
    name = project.compare_time_histories('Test', 'Model')
    rows = _rows(action_table(project, [name], {}))
    assert 'Scores' in rows and 'Scoring Window' in rows and rows['Edit Pairs']['ready']
    assert 'Units' not in rows and 'Plot' not in rows


def test_the_bar_offers_the_stage_and_the_overlay_and_no_component_for_a_comparison(project):
    from hertzforge.service.show import PREFS, _offers

    name = project.compare_time_histories('Test', 'Model')
    offers = _offers(project, [name], PREFS)
    assert offers['flat'] is True
    assert offers['limits'] is False


def test_the_stage_draws_every_station_with_its_trac_and_the_overlay_is_2d(project, validate):
    from hertzforge.service.show import PREFS

    name = project.compare_time_histories('Test', 'Model')
    views, _notes = show._alone(project, name, [], {**PREFS, 'flat': False})
    wire_views(views, validate)
    stage, table, _reading = views
    assert stage['view'] == 'waterfall'
    assert stage['stations'] == ['101X+  TRAC 1.000', '102Z-  TRAC 1.000']
    assert stage['depths'] == [0, 0, 1, 1]
    assert stage['roles'] == ['reference', 'measured', 'reference', 'measured']
    assert stage['records'] == ['Model: 101X+', 'Test: 101X+', 'Model: 102Z-', 'Test: 102Z-']
    assert table['select']['params'] == {'comparison': name, 'flat': False}
    flat, _notes = show._alone(project, name, [], {**PREFS, 'flat': True})
    assert flat[0]['view'] == 'spectrum'


def test_a_station_not_scored_says_so_on_the_stage(project):
    from hertzforge.service.show import PREFS

    project['Model'].ordinate[1] = 0.0
    name = project.compare_time_histories('Test', 'Model')
    views, _notes = show._alone(project, name, [], {**PREFS, 'flat': False})
    assert views[0]['stations'][1] == '102Z-  not scored'


def test_the_stage_leaves_a_station_of_another_quantity_out_and_says_so(project):
    from hertzforge.service.show import PREFS

    t = np.arange(1000) / 1000.0
    project.add('Test V', TimeHistory(t, [np.sin(2 * np.pi * 7 * t), np.cos(t)],
                                      response_dof=['101X+', '5Y+'],
                                      ordinate_dim=['acceleration', 'velocity']))
    project.add('Model V', TimeHistory(t, [np.sin(2 * np.pi * 7 * t), np.cos(t)],
                                       response_dof=['101X+', '5Y+'],
                                       ordinate_dim=['acceleration', 'velocity']))
    name = project.compare_time_histories('Test V', 'Model V')
    views, _notes = show._alone(project, name, [], {**PREFS, 'flat': False})
    assert views[0]['stations'] == ['101X+  TRAC 1.000']
    assert views[0]['note'] == '1 station of another quantity left out; pick them to draw them.'


def _files():
    t = np.arange(500) / 500.0
    made = hertzforge.Project()
    names = []
    for side, gain in (('Test', 1.0), ('Model', -2.0)):
        for k, dof in enumerate(['1X+', '2X+', '3X+']):
            name = f'{side} {dof}'
            made.add(name, TimeHistory(t, [gain * np.sin(2 * np.pi * (5 + k) * t)],
                                       response_dof=[dof], ordinate_dim=['acceleration']))
            names.append(name)
    made.object_groups.append({'members': names[:3], 'role': None, 'name': 'test.unv'})
    made.object_groups.append({'members': names[3:], 'role': None, 'name': 'model.unv'})
    return made, names


def _trac_row(project, objects, records=None):
    rows = action_table(project, objects, records or {})['columns'][0]['rows']
    return next(row for row in rows if row['id'] == 'project.compare_time_histories')


def test_every_history_of_two_groups_picked_offers_trac_with_each_group_a_set():
    project, names = _files()
    row = _trac_row(project, names)
    assert row['ready']
    assert row['fill'] == {'first': 'Test 1X+', 'second': 'Model 1X+',
                           'first_more': 'Test 2X+\nTest 3X+',
                           'second_more': 'Model 2X+\nModel 3X+',
                           'first_records': '', 'second_records': ''}
    assert set(row['omit']) == set(row['fill'])
    plot, table, reading = run(project, 'project.compare_time_histories', **row['fill'])
    assert plot['title'].endswith('test.unv solid and model.unv dashed')
    assert [cells[1] for cells in table['rows']] == ['1X+', '2X+', '3X+']
    assert reading['title'] == 'Preview: test.unv against model.unv'


def test_swap_makes_the_second_group_the_test():
    project, names = _files()
    fill = _trac_row(project, names)['fill']
    _plot, before, _reading = run(project, 'project.compare_time_histories', **fill)
    plot, table, _reading = run(project, 'project.compare_time_histories', **fill, swap=True)
    assert plot['title'].endswith('model.unv solid and test.unv dashed')
    # TRAC, the correlation's square, is the same either way; sTRAC and the RMS ratio are not
    assert [row[3] for row in table['rows']] == [row[3] for row in before['rows']]
    assert (before['rows'][0][4], table['rows'][0][4]) == ('-0.500', '-2.000')
    assert (before['rows'][0][6], table['rows'][0][6]) == ('2.000', '0.500')
    run(project, 'project.compare_time_histories', **fill, swap=True, keep=True)
    saved = project['Time Response Comparison']
    assert saved.pairs[0] == ['Model 1X+', 0, 'Test 1X+', 0]
    views, _notes = show._alone(project, 'Time Response Comparison', [])
    assert views[2]['title'] == 'Time Response Comparison: model.unv against test.unv'


def test_part_of_a_group_or_records_picked_under_one_turn_the_row_off_with_the_reason():
    project, names = _files()
    assert _trac_row(project, names[:4])['reason'] == ('Model 2X+ of model.unv is not picked: '
                                                      'pick every history of the two groups')
    assert _trac_row(project, names, {'Test 1X+': [0]})['reason'] == ('Pick whole histories to '
                                                                     'compare two groups')


def test_two_histories_picked_leave_the_more_fields_off_the_form():
    project, _names = _files()
    row = _trac_row(project, ['Test 1X+', 'Model 1X+'])
    assert {'first_more', 'second_more'} <= set(row['omit'])
    assert (row['fill']['first_more'], row['fill']['second_more']) == ('', '')


@pytest.fixture
def renumbered():
    t = np.arange(500) / 500.0
    made = hertzforge.Project()
    made.add('Test', TimeHistory(t, [np.sin(2 * np.pi * 5 * t), np.sin(2 * np.pi * 9 * t),
                                     np.cos(t)], response_dof=['101X+', '102X+', '1Z+'],
                                 ordinate_dim=['acceleration', 'acceleration', 'force']))
    made.add('Model', TimeHistory(t, [2.0 * np.sin(2 * np.pi * 5 * t),
                                      -np.sin(2 * np.pi * 9 * t)],
                                  response_dof=['7X+', '8X+'],
                                  ordinate_dim=['acceleration', 'acceleration']))
    return made


def test_sets_numbered_differently_pair_in_record_order(renumbered):
    with pytest.raises(ProtocolError, match='pair the records in record order and edit'):
        run(renumbered, 'project.compare_time_histories', first='Test', second='Model')
    _plot, table, _reading = run(renumbered, 'project.compare_time_histories', first='Test',
                                 second='Model', pairing='order')
    assert [row[1] for row in table['rows']] == ['101X+', '102X+']
    assert [row[4] for row in table['rows']] == ['0.500', '-1.000']


def test_edit_pairs_shows_each_sides_records_and_a_choice_swaps_with_the_pair_holding_it(
        renumbered, validate):
    run(renumbered, 'project.compare_time_histories', first='Test', second='Model',
        pairing='order', keep=True)
    name = 'Time Response Comparison'
    table, = run(renumbered, 'project.edit_time_comparison_pairs', source=name)
    wire_views([table], validate)
    assert table['columns'] == ['Station', 'First (Test)', 'Second (Model)']
    assert table['rows'] == [['1', '101X+ · Test', '7X+ · Model'],
                             ['2', '102X+ · Test', '8X+ · Model']]
    assert table['edit']['columns'] == [None, 'first', 'second']
    assert table['edit']['choices']['second'] == ['7X+ · Model', '8X+ · Model']
    assert table['edit']['remove']['op'] == 'project.remove_time_comparison_pairs'
    table, = run(renumbered, 'project.set_time_comparison_pair', source=name, row=1,
                 column='second', value='8X+ · Model')
    assert [row[2] for row in table['rows']] == ['8X+ · Model', '7X+ · Model']
    assert renumbered[name].pairs == [['Test', 0, 'Model', 1], ['Test', 1, 'Model', 0]]


def test_a_pair_cell_refuses_what_is_not_that_sides_record_and_a_row_that_is_not_there(
        renumbered):
    run(renumbered, 'project.compare_time_histories', first='Test', second='Model',
        pairing='order', keep=True)
    name = 'Time Response Comparison'
    with pytest.raises(ProtocolError, match="'1Z\\+ · Test' is not a response record"):
        run(renumbered, 'project.set_time_comparison_pair', source=name, row=1,
            column='first', value='1Z+ · Test')
    with pytest.raises(ProtocolError, match='has no pair 5'):
        run(renumbered, 'project.set_time_comparison_pair', source=name, row=5,
            column='first', value='101X+ · Test')


def test_pairs_are_removed_by_station_and_the_last_one_stays(renumbered):
    run(renumbered, 'project.compare_time_histories', first='Test', second='Model',
        pairing='order', keep=True)
    name = 'Time Response Comparison'
    table, = run(renumbered, 'project.remove_time_comparison_pairs', source=name, ids='2')
    assert table['rows'] == [['1', '101X+ · Test', '7X+ · Model']]
    with pytest.raises(ProtocolError, match='keeps one pair at least'):
        run(renumbered, 'project.remove_time_comparison_pairs', source=name, ids='1')
    assert len(renumbered[name].pairs) == 1


@pytest.fixture
def mapped():
    """A comparison of two stations, 1Z+ scoring 1 and 2Z+ less, a rig of
    the two nodes joined by a beam, and a plate under them."""
    from hertzforge.core.geometry import Geometry

    t = np.arange(500) / 500.0
    made = hertzforge.Project()
    made.add('Test', TimeHistory(t, [np.sin(2 * np.pi * 5 * t), np.sin(2 * np.pi * 9 * t)],
                                 response_dof=['1Z+', '2Z+'], ordinate_dim=['acceleration'] * 2))
    made.add('Model', TimeHistory(t, [np.sin(2 * np.pi * 5 * t),
                                      np.sin(2 * np.pi * 9 * t) + np.sin(2 * np.pi * 20 * t)],
                                  response_dof=['1Z+', '2Z+'], ordinate_dim=['acceleration'] * 2))
    rig = Geometry(node_id=[1, 2], node_xyz=[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]])
    rig.add_element([1, 2], 21)
    made.add('Rig', rig)
    plate = Geometry(node_id=[10, 11, 12, 13],
                     node_xyz=[[0, -0.5, 0], [1, -0.5, 0], [1, 0.5, 0], [0, 0.5, 0]])
    plate.add_element([10, 11, 12, 13], 44)
    made.add('Plate', plate)
    made.compare_time_histories('Test', 'Model')
    return made


def test_the_map_draws_each_station_colored_by_its_score_and_says_the_mean(mapped, validate):
    from hertzforge.core.time_compare import scores

    name = 'Time Response Comparison'
    view, = run(mapped, 'project.time_comparison_map', comparison=name, geometry='Rig')
    wire_views([view], validate)
    layer = view['points'][-1]
    np.testing.assert_array_equal(layer['xyz'], [[0, 0, 0], [1, 0, 0]])
    assert layer['colors'][0].tolist() == [253, 231, 37]  # TRAC 1, the top of the scale
    traces = [row['trac'] for row in scores(mapped[name])]
    assert view['note'] == (f'Mean TRAC {np.mean(traces):.3f} over 2 stations; colored from 0 '
                            f'(purple) to 1 (yellow), unscored gray.')
    assert 'element_colors' not in view


def test_strac_reads_on_zero_to_two_with_one_in_the_middle(mapped):
    view, = run(mapped, 'project.time_comparison_map', comparison='Time Response Comparison',
                geometry='Rig', score='strac')
    assert view['note'].startswith('Mean sTRAC')
    assert 'colored from 0 (purple) to 2 (yellow)' in view['note']
    # sTRAC 1 at the first station: the middle stop of the scale
    assert view['points'][-1]['colors'][0].tolist() == [33, 144, 141]


def test_a_contour_colors_the_fe_model_see_through_under_the_stations(mapped, validate):
    view, = run(mapped, 'project.time_comparison_map', comparison='Time Response Comparison',
                geometry='Rig', fem='Plate', draw='nearest')
    wire_views([view], validate)
    colors = view['element_colors']
    assert len(colors['triangles']) == len(colors['triangle_colors']) == 2
    assert view['opacity'] == 0.6
    assert len(view['segments'][0]['xyz']) == 2  # the rig's beam over the plate
    assert view['title'].startswith('TRAC of Time Response Comparison on Plate')


def test_the_map_refuses_what_does_nothing_and_what_cannot_be_drawn(mapped):
    from hertzforge.core.geometry import Geometry

    name = 'Time Response Comparison'
    with pytest.raises(ProtocolError, match='leave FE model blank'):
        run(mapped, 'project.time_comparison_map', comparison=name, geometry='Rig',
            fem='Plate')
    with pytest.raises(ProtocolError, match='pick the FE model'):
        run(mapped, 'project.time_comparison_map', comparison=name, geometry='Rig',
            draw='smooth')
    mapped.add('Elsewhere', Geometry(node_id=[90], node_xyz=[[0.0, 0.0, 0.0]]))
    with pytest.raises(ProtocolError, match="none of the stations' nodes is in Elsewhere"):
        run(mapped, 'project.time_comparison_map', comparison=name, geometry='Elsewhere')
    with pytest.raises(ProtocolError, match='Rig has no faces'):
        run(mapped, 'project.time_comparison_map', comparison=name, geometry='Rig',
            fem='Rig', draw='nearest')


def test_a_station_off_the_geometry_is_counted_and_an_unscored_one_drawn_gray(mapped):
    from hertzforge.core.geometry import Geometry

    mapped['Model'].ordinate[1] = 0.0
    mapped.refresh('Time Response Comparison')
    mapped.add('Half', Geometry(node_id=[2], node_xyz=[[1.0, 0.0, 0.0]]))
    view, = run(mapped, 'project.time_comparison_map', comparison='Time Response Comparison',
                geometry='Half')
    assert view['points'][-1]['colors'].tolist() == [[140, 140, 140]]
    assert view['note'] == ('No station has a TRAC; colored from 0 (purple) to 1 (yellow), '
                            'unscored gray; 1 station not on Half.')


def test_a_comparison_picked_with_its_geometry_draws_the_map(mapped):
    views, _notes = show._pair(mapped, 'Rig', 'Time Response Comparison', {})
    assert views[0]['view'] == 'shape3d'
    assert views[0]['note'].startswith('Mean TRAC')
