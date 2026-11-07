"""The display unit system: views computed in SI put into the system the
connection has chosen."""

from __future__ import annotations

import asyncio
from pathlib import Path

import numpy as np
import pytest

from hertzforge.service import display_units
from hertzforge.service import views as v

from .test_projects import _exchange

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
US = 'in-slinch-lbf-s (g)'
G = 9.80665


def test_si_labels_are_found_again_as_their_dimensions():
    assert display_units.dimension_of('m/s²') == 'acceleration'
    assert display_units.dimension_of('(m/s²)²/Hz') == 'acceleration**2/frequency'
    assert display_units.dimension_of('(m/s²)/N') == 'acceleration/force'
    assert display_units.dimension_of('dB') is None
    assert display_units.dimension_of(None) is None


def test_a_psd_spectrum_is_put_into_g_squared_per_hertz():
    psd = np.array([G ** 2, 4 * G ** 2])
    view = v.spectrum('PSD', v.axis('Frequency', 'Hz', 'log'), v.axis('PSD', '(m/s²)²/Hz', 'log'),
                      [v.series('1X+', 'measured', [10.0, 20.0], psd)],
                      regions=[v.region(10.0, 15.0, 'cut')])
    (out,) = display_units.convert([view], US)
    assert out['y']['unit'] == 'g²/Hz' and out['x']['unit'] == 'Hz'
    assert out['series'][0]['y'] == pytest.approx([1.0, 4.0])
    assert out['series'][0]['x'] == pytest.approx([10.0, 20.0])
    assert out['regions'][0]['x0'] == 10.0


def test_a_reading_and_a_temperature_take_the_systems_scale_and_offset():
    reading = v.reading('R', [v.row('Peak', 2 * G, 'm/s²', 'info'), v.row('Gap', 1.5, 'dB', 'info'),
                              v.row('Hot', 300.0, 'K', 'info'), v.row('Note', 'words', None, 'info')])
    (out,) = display_units.convert([reading], US)
    rows = {row['label']: row for row in out['rows']}
    assert (rows['Peak']['value'], rows['Peak']['unit']) == (pytest.approx(2.0), 'g')
    assert rows['Gap']['value'] == 1.5 and rows['Gap']['unit'] == 'dB'
    assert rows['Hot']['unit'] == 'degF' and rows['Hot']['value'] == pytest.approx(80.33, abs=0.01)


def test_si_is_left_as_computed():
    reading = v.reading('R', [v.row('Peak', 2 * G, 'm/s²', 'info')])
    (out,) = display_units.convert([reading], display_units.DEFAULT)
    assert out['rows'][0]['value'] == pytest.approx(2 * G)


def test_a_connection_starts_in_vds_system_and_chooses_another(validate):
    responses = asyncio.run(_exchange([
        ('units.system', {}),
        ('project.import', {'path': str(PLATE / 'random.nc4')}),
        ('op.run', {'op': 'project.time_statistics', 'params': {}}),
        ('units.system', {'name': display_units.DEFAULT}),
        ('op.run', {'op': 'project.time_statistics', 'params': {}}),
        ('units.system', {'name': 'furlongs'}),
    ]))
    first, _imported, us, si_set, si, wrong = (got[-1] for got in responses)
    assert first['result']['name'] == display_units.START == US
    assert US in first['result']['systems']
    rms = lambda r: next(row for row in r['result']['views'][0]['rows'] if row['label'] == 'RMS')
    assert rms(us)['unit'] == 'g' and rms(si)['unit'] == 'm/s²'
    assert rms(si)['value'] == pytest.approx(rms(us)['value'] * G)
    assert si_set['result']['name'] == display_units.DEFAULT
    assert wrong['error']['code'] == 'bad_request'


def test_export_writes_in_the_connections_system(tmp_path):
    path = tmp_path / 'plate.unv'
    responses = asyncio.run(_exchange([
        ('units.system', {}),
        ('project.import', {'path': str(PLATE / 'geometry.unv'), 'units': {'length_unit': 'mm'}}),
        ('op.run', {'op': 'project.save_as', 'params': {
            'object': 'Geometry', 'format': 'unv', 'path': str(path)}}),
    ]))
    rows = {r['label']: r for r in responses[-1][-1]['result']['views'][0]['rows']}
    assert rows['Units']['value'] == 'in-slinch-lbf-s'


def test_a_tables_unit_columns_are_converted_but_not_the_ones_typed_into():
    view = v.table('Nodes', ['Node', 'X [m]', 'Y [m]', 'Note [m]'],
                   [['1', '0.0254', '0.0508', 'here']],
                   v.editing('project.set_entity_cell', {}, ['id', None, 'y', None]))
    (out,) = display_units.convert([view], US)
    assert out['columns'] == ['Node', 'X [in]', 'Y [m]', 'Note [in]']
    assert out['rows'][0] == ['1', '1', '0.0508', 'here']


def _plate(unit):
    nodes = np.array([[0.0, 0.0, 0.0], [0.0254, 0.0, 0.0], [0.0, 0.0508, 0.0]])
    shape = np.array([[0.0, 0.0, 1.0], [0.0, 0.0, 2.0], [0.0, 0.0, 3.0]])
    view = v.shape3d('Plate', unit, nodes, [v.mode(1, 10.0, None, shape)],
                     triangles=np.array([[0, 1, 2]]))
    view['pick'] = v.picking('project.add_node', {'geometry': 'Plate'}, 'point', 'place')
    view['marks'] = {'nodes': np.array([[0.0, 0.0, 0.0], [0.0, 0.0, 0.0254]], dtype=np.float32),
                     'lines': np.array([[0, 1]], dtype=np.uint32)}
    return view


def test_a_geometry_is_drawn_in_the_systems_length_and_a_click_goes_back_to_si():
    (out,) = display_units.convert([_plate('m')], US)
    assert out['unit'] == 'in'
    assert np.asarray(out['nodes'])[1:, :2].max() == pytest.approx(2.0)
    assert np.asarray(out['nodes'])[1, 0] == pytest.approx(1.0)
    assert np.asarray(out['modes'][0]['shape'])[2, 2] == 3.0, 'a shape is a picture'
    scale, offset = out['pick']['to_si']
    assert 1.0 * scale + offset == pytest.approx(0.0254), 'an inch clicked is 0.0254 m'
    marks = out['marks']
    assert marks['nodes'].dtype == np.float32 and marks['nodes'][1, 2] == pytest.approx(1.0), \
        'the arrows over the model move with it'
    assert marks['lines'].dtype == np.uint32
    (raw,) = display_units.convert([_plate('undefined')], US)
    assert raw['unit'] == 'undefined' and 'to_si' not in raw['pick']
    assert np.asarray(raw['nodes'])[1, 0] == pytest.approx(0.0254), 'a file\'s own numbers'


def test_measured_motion_plays_in_the_systems_units():
    view = _plate('m')
    view['motion'] = {'label': 'Time', 'unit': 's', 'start': 0.0, 'step': 0.001,
                      'quantity': 'Acceleration', 'value_unit': 'm/s²',
                      'node': np.array([1], np.uint32), 'direction': np.array([[0, 0, 1.0]]),
                      'values': np.array([[G, -2 * G]], np.float32), 'peak': 2 * G}
    (out,) = display_units.convert([view], US)
    motion = out['motion']
    assert motion['value_unit'] == 'g' and motion['peak'] == pytest.approx(2.0)
    assert np.asarray(motion['values']).ravel() == pytest.approx([1.0, -2.0], rel=1e-6)
    assert motion['unit'] == 's' and motion['step'] == 0.001
