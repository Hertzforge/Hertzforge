"""Sheets filled in by hand: a channel table's cells, the calibration dates,
a specification sheet and a prediction."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

from hertzforge.service import projects
from hertzforge.service.ops_sheets import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in OPERATIONS}


@pytest.fixture
def project():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'random.nc4')})
    return state.project


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()})
    return op.run(project, values)


def rows(reading):
    return {row['label']: row for row in reading['rows']}


def check(views, validate):
    wire_views(views, validate)
    for view in views:
        if view['view'] == 'table':
            validate(view, 'tableView')


@pytest.fixture
def blank(project):
    """A new table with two channels."""
    (table, _reading) = run(project, 'project.new_channel_table', name='Hand Table')
    assert table['rows'] == []
    run(project, 'project.add_channels', table='Hand Table', count=2)
    return 'Hand Table'


def test_the_descriptors_validate(validate):
    for op in OPERATIONS:
        validate(op.to_json(), 'operation')


def test_add_channel_numbers_on_and_the_table_edits_by_schema_key(project, blank, validate):
    views = run(project, 'project.add_channels', table=blank)
    check(views, validate)
    table, reading = views
    assert [row[0] for row in table['rows']] == ['1', '2', '3']
    assert table['edit']['op'] == 'project.set_channel_cell'
    assert table['edit']['params'] == {'table': blank}
    assert table['edit']['columns'] == list(project[blank].column_names)
    assert rows(reading)['Added']['value'] == 'channel 3'
    with pytest.raises(ProtocolError, match='not one of the choices'):
        run(project, 'project.add_channels', table='Specification')


def test_a_cell_is_written_and_a_wrong_value_refused_unchanged(project, blank, validate):
    views = run(project, 'project.set_channel_cell', table=blank, row=1,
                column='channel_type', value='acceleration')
    check(views, validate)
    run(project, 'project.set_channel_cell', table=blank, row=1, column='unit', value='g')
    with pytest.raises(ProtocolError, match='change the type first'):
        run(project, 'project.set_channel_cell', table=blank, row=1, column='unit',
            value='N')
    assert project[blank].frame.loc[0, 'unit'] == 'g'
    with pytest.raises(ProtocolError, match='already exists'):
        run(project, 'project.set_channel_cell', table=blank, row=2, column='channel',
            value='1')
    with pytest.raises(ProtocolError, match='no row 9'):
        run(project, 'project.set_channel_cell', table=blank, row=9, column='node',
            value='1')
    (_table, reading) = run(project, 'project.set_channel_cell', table=blank, row=1,
                            column='expiration', value='10/24/2026')
    assert '2026-10-24' in rows(reading)['Read as']['value']


def test_a_catalog_model_fills_its_make_and_a_triax_its_other_leg(project, blank, validate):
    for row in (1, 2):
        run(project, 'project.set_channel_cell', table=blank, row=row, column='node',
            value='101')
        run(project, 'project.set_channel_cell', table=blank, row=row,
            column='channel_type', value='acceleration')
    views = run(project, 'project.set_sensor_model', table=blank, row=1, model='356A02')
    check(views, validate)
    _table, reading = views
    frame = project[blank].frame
    assert frame.loc[0, 'make'] == 'PCB' and frame.loc[1, 'model'] == '356A02'
    found = rows(reading)
    assert 'triaxial' in found['Sensor catalog']['value']
    assert found['Channel 2, Model']['value'].startswith('356A02')


def test_the_role_of_a_linked_table_is_set_on_its_history(project, validate):
    views = run(project, 'project.set_channel_cell', table='Channel Table', row=1,
                column='role', value='monitor')
    check(views, validate)
    history = project['Time History']
    assert history.channel_roles()[('101Z+', 'acceleration')] == 'monitor'
    assert views[0]['edit']['columns'][4] is None    # control is per environment


def test_calibration_dates_are_judged(project, blank, validate):
    run(project, 'project.set_channel_cell', table=blank, row=1, column='expiration',
        value='2026-10-22')
    views = run(project, 'project.calibration_check', table=blank, start='2026-10-20',
                end='2026-10-26')
    check(views, validate)
    found = rows(views[1])
    assert found['Channel 1']['status'] == 'warn'
    assert found['Channel 1']['value'].startswith('lapses during the test')
    assert found['Channel 2']['status'] == 'info'
    assert found['Calibration']['value'].startswith('Calibration on the test dates')
    with pytest.raises(ProtocolError, match='YYYY-MM-DD'):
        run(project, 'project.calibration_check', table=blank, start='10/20/2026')


def test_create_specification_writes_the_sheet_at_picked_rows(project, validate):
    views = run(project, 'project.create_specification', table='Channel Table', rows='1-2',
                breakpoints='20, 0.01\n80, 0.04\n2000, 0.04', all_pairs='0.5, 30',
                warning='-2, 2', name='Sheet')
    check(views, validate)
    plot, table, reading = views
    spec = project['Sheet']
    assert list(spec.abscissa) == [20.0, 80.0, 2000.0]
    assert table['columns'][0] == 'Hz' and table['columns'][1].startswith('101Z+ (')
    assert table['edit']['columns'] == ['frequency', '101Z+', '104Z+']
    assert table['rows'][1][:2] == ['80', '0.04']
    # 20 to 80 Hz at +6 dB/octave from 0.01, then flat: the closed-form area
    n = np.log(4.0) / np.log(4.0)
    area = 0.01 * 20 * (4.0 ** (n + 1) - 1) / (n + 1) + 0.04 * 1920
    assert rows(reading)['101Z+ rms']['value'] == pytest.approx(np.sqrt(area), rel=1e-3)
    assert rows(reading)['Cross terms']['status'] == 'pass'
    cross = next(k for k in range(spec.num_records)
             if spec.response_dof[k] == '101Z+' and spec.reference_dof[k] == '104Z+')
    assert np.angle(spec.ordinate[cross][0], deg=True) == pytest.approx(30.0)
    lower = spec.limits['warning_lower'][0][0]
    assert 10 * np.log10(lower / 0.01) == pytest.approx(-2.0)
    roles = {line['role'] for line in plot['series']}
    assert {'reference', 'tolerance', 'abort'} <= roles
    with pytest.raises(ProtocolError, match='coherence'):
        run(project, 'project.create_specification', table='Channel Table', rows='1-2',
            all_pairs='1.5, 0')


def test_breakpoints_by_slope_reach_their_levels(project):
    views = run(project, 'project.create_specification', table='Channel Table', rows='1',
                entry='slopes', breakpoints='20, 0.01\n80, 3\n2000, 0', name='Sloped')
    table = views[1]
    assert table['edit']['columns'] == ['frequency', '101Z+', '101Z+ dB/oct']
    assert table['edit']['params']['entry'] == 'slopes'
    level = float(table['rows'][1][1])
    assert level == pytest.approx(0.01 * 4.0 ** (3 / 3.0103), rel=1e-4)


def test_a_band_edge_drags_through_its_handle(project, validate):
    run(project, 'project.create_specification', table='Channel Table', rows='1-2',
        name='Sheet', breakpoints='20, 0.01\n200, 0.01\n2000, 0.01', abort='-6, 6')
    views = run(project, 'project.edit_specification', specification='Sheet')
    wire_views(views, validate)
    plot = next(view for view in views if view['view'] == 'spectrum')
    handles = [h for h in plot['handles'] if h['params']['kind'] == 'abort']
    upper = next(h for h in handles if h['params']['side'] == 'upper')
    assert upper['decibels'] == 6.0
    assert 'bort' in plot['series'][upper['series']]['name']
    views = run(project, upper['op'], **upper['params'], decibels=9.1)
    plot = next(view for view in views if view['view'] == 'spectrum')
    moved = next(h for h in plot['handles'] if h['params']['kind'] == 'abort'
                 and h['params']['side'] == 'upper')
    assert moved['decibels'] == 9.0, 'it lands on the quarter-decibel grid'


def test_set_breakpoint_moves_a_level_and_a_frequency(project, validate):
    run(project, 'project.create_specification', table='Channel Table', rows='1-2',
        name='Sheet', breakpoints='20, 0.01\n2000, 0.01')
    views = run(project, 'project.set_breakpoint', specification='Sheet', row=2,
                column='104Z+', value='0.02')
    check(views, validate)
    spec = project['Sheet']
    auto = next(k for k in range(spec.num_records)
            if spec.response_dof[k] == spec.reference_dof[k] == '104Z+')
    assert np.real(spec.ordinate[auto][-1]) == pytest.approx(0.02)
    views = run(project, 'project.set_breakpoint', specification='Sheet', row=1,
                column='frequency', value='3000')
    assert [row[0] for row in views[1]['rows']] == ['2000', '3000']
    with pytest.raises(ProtocolError, match='needs a positive level'):
        run(project, 'project.set_breakpoint', specification='Sheet', row=1,
            column='101Z+', value='-1')
    with pytest.raises(ProtocolError, match='already a breakpoint'):
        run(project, 'project.set_breakpoint', specification='Sheet', row=1,
            column='frequency', value='3000')


def test_edit_specification_writes_cross_terms_and_bands_in_place(project, validate):
    run(project, 'project.create_specification', table='Channel Table', rows='1-3',
        name='Sheet')
    views = run(project, 'project.edit_specification', specification='Sheet',
                pairs='1, 2, 0.9, 0\n104Z+ 110Z+ 0.2 -45', abort='-5, 5')
    check(views, validate)
    found = rows(views[2])
    assert found['Cross terms']['value'].startswith('2 of 3 stated')
    assert found['Cross terms']['status'] == 'warn'
    spec = project['Sheet']
    assert 10 * np.log10(spec.limits['abort_upper'][0][0] / np.real(spec.ordinate[0][0])) \
        == pytest.approx(5.0)
    with pytest.raises(ProtocolError, match='symmetric'):
        run(project, 'project.edit_specification', specification='Sheet', warning='-2, 4')
    before = project['Sheet']
    views = run(project, 'project.edit_specification', specification='Sheet')
    assert project['Sheet'] is before and 'opened' in rows(views[2])['Specification']['value']
    views = run(project, 'project.edit_specification', specification='Sheet',
                symmetric='off', warning='-2, 4')
    assert 'free' not in rows(views[2])['Bands']['value']


def test_a_prediction_is_entered_and_replaced(project, validate):
    views = run(project, 'project.predict', dof='101Z+', unit='g', peaks='385, 0.5, 12',
                frequency_uncertainty=10.0, source='stress analysis')
    check(views, validate)
    table, reading = views
    held = project['Predictions']
    assert len(held) == 1 and held.predictions[0].bands[0] == pytest.approx((346.5, 423.5))
    assert table['rows'][0][:2] == ['101Z+', 'asd']
    assert rows(reading)['Entered']['value'].startswith('101Z+, ASD in g²/Hz at +0 dB: 385 Hz')
    run(project, 'project.predict', dof='101Z+', unit='g', peaks='400, 0.4, 10, 350, 450')
    assert len(project['Predictions']) == 1
    assert project['Predictions'].predictions[0].peaks[0].frequency == 400.0
    with pytest.raises(ProtocolError, match='no band'):
        run(project, 'project.predict', dof='104Z+', unit='g', peaks='385, 0.5')
