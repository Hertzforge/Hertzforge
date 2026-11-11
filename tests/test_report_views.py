"""A report's blocks as the protocol's views (`hertzforge.report.views`).

What is pinned: every figure a starter template draws on the plate's data
comes out as views the schema accepts, and none is lost; a log axis's
values are the values, not the page's decades; a comparison's control
channels are a spectrum's groups, each with its target, response and limit
lines and its readouts; a synthesized curve follows the curve it models;
a bar chart is judged as the page judged it; and a scene, a map, a MAC, a
verdict and a photo each become their view.
"""

from __future__ import annotations

import base64
import json
from pathlib import Path

import jsonschema
import numpy as np
import pytest
from conftest import fixture_path

import hertzforge
from hertzforge.report.views import axis_of, block_views, page_blocks
from hertzforge.service import frames

SCHEMA = json.loads((Path(__file__).resolve().parents[1] / 'schema' /
                     'protocol.schema.json').read_text(encoding='utf-8'))
VIEWS = jsonschema.Draft202012Validator(
    {'$schema': SCHEMA['$schema'], '$defs': SCHEMA['$defs'], '$ref': '#/$defs/views'})


def _valid(views):
    wire, _arrays = frames.to_wire({'views': views})
    VIEWS.validate(wire['views'])


@pytest.mark.parametrize('files, template', [
    (['geometry.unv', 'modal.nc4'], 'modal'),
    (['geometry.unv', 'random.nc4'], 'random'),
    (['geometry.unv', 'shock.nc4'], 'shock'),
    (['frfs.unv'], 'modal')])
def test_every_figure_of_a_template_is_views(files, template):
    project = hertzforge.Project(template)
    for name in files:
        project.import_file(fixture_path('plate', name))
    report = project[project.generate_report(template)]
    blocks = page_blocks(report, dict(project.items()), object_groups=project.object_groups)
    figures = [b for b in blocks if b['kind'] == 'figure']
    assert figures, 'the plate draws figures'
    for block in figures:
        assert block['views'], block['label']
        _valid(block['views'])
    assert all(isinstance(b['block'], int) for b in blocks)
    assert all(b['kind'] in ('text', 'figure', 'unbound') for b in blocks)


def test_a_random_run_draws_its_grids_and_bars_as_views():
    project = hertzforge.random_vibration_run(
        fixture_path('plate', 'random.nc4'),
        geometry=fixture_path('plate', 'geometry.npz'), length_unit='m')
    name = project.generate_report('random')
    blocks = page_blocks(project[name], dict(project.items()),
                         object_groups=project.object_groups)
    views = [view for b in blocks for view in b.get('views', [])]
    _valid(views)
    grouped = [view for view in views if view['view'] == 'spectrum' and view.get('groups')]
    assert grouped, 'the control grid is a spectrum shown a channel at a time'
    first = grouped[0]
    roles = {first['series'][i]['role'] for i in first['groups'][0]['series']}
    assert {'reference', 'measured'} <= roles
    assert any(view['view'] == 'bars' for view in views)


def test_a_log_axis_carries_values_not_decades():
    built = {'kind': 'plot', 'label': 'Figure 1', 'logx': True, 'logy': True,
             'x': [1.0, 2.0, 3.0], 'xlabel': 'frequency [Hz]', 'ylabel': 'PSD [g²/Hz]',
             'curves': [{'label': '101Z+', 'x': None, 'y': [-3.0, -2.0, None], 'color': 0}]}
    [view] = block_views(built)
    assert view['x'] == {'label': 'Frequency', 'unit': 'Hz', 'scale': 'log'}
    assert view['y']['unit'] == 'g²/Hz'
    assert np.allclose(view['series'][0]['x'], [10.0, 100.0, 1000.0])
    assert np.allclose(view['series'][0]['y'][:2], [1e-3, 1e-2])
    assert np.isnan(view['series'][0]['y'][2])
    assert axis_of('time [s]', False) == {'label': 'Time', 'unit': 's', 'scale': 'linear'}
    assert axis_of('lbf', False)['unit'] is None


def test_control_channels_are_groups_with_their_limits_and_readouts():
    zones = [{'lower': [0.0, 0.0], 'upper': [1.0, 1.0], 'severity': 'abort'},
             {'lower': None, 'upper': [0.5, 0.5], 'severity': 'warning'}]
    built = {'kind': 'plot', 'label': 'Figure 2', 'logy': False, 'x': [10.0, 20.0],
             'xlabel': 'frequency [Hz]', 'ylabel': 'level',
             'curves': [{'label': 'A', 'x': None, 'y': [0.2, 0.2], 'gray': True},
                        {'label': 'measured', 'x': None, 'y': [0.3, 0.3], 'color': 0}],
             'channels': [
                 {'label': 'A', 'y': [0.2, 0.2], 'response': [0.3, 0.3], 'zones': zones,
                  'readouts': [{'label': 'RMS error', 'text': '1.2 dB', 'state': 'in'},
                               {'label': 'Lines outside abort', 'text': '12 %',
                                'state': 'over'}]},
                 {'label': 'B', 'y': [0.1, 0.1], 'response': [0.4, 0.4], 'zones': []}]}
    [view] = block_views(built)
    _valid([view])
    a, b = view['groups']
    assert a['label'] == 'A' and b['label'] == 'B'
    assert [view['series'][i]['role'] for i in a['series']] == [
        'reference', 'measured', 'abort', 'abort', 'tolerance']
    assert [r['status'] for r in a['readouts']] == ['pass', 'fail']
    assert [view['series'][i]['role'] for i in b['series']] == ['reference', 'measured']


def test_a_synthesized_curve_follows_the_curve_it_models():
    built = {'kind': 'plot', 'label': 'Figure 3', 'logy': True, 'x': [1.0, 2.0],
             'xlabel': 'frequency [Hz]', 'ylabel': 'CMIF',
             'curves': [{'label': 'CMIF 1', 'y': [0.0, 1.0], 'color': 0},
                        {'label': 'CMIF 2', 'y': [0.0, 0.5], 'color': 1},
                        {'label': 'Synthesized CMIF 1', 'y': [0.0, 1.0], 'color': 0,
                         'dash': True},
                        {'label': 'Synthesized CMIF 2', 'y': [0.0, 0.5], 'color': 1,
                         'dash': True}]}
    [view] = block_views(built)
    assert [s.get('follows') for s in view['series']] == [None, None, 0, 1]


def test_a_stepped_density_keeps_its_bins():
    built = {'kind': 'plot', 'label': 'Figure 4', 'logy': False, 'x': [1.0, 2.0, 3.0],
             'steps': True, 'edges': [0.5, 1.5, 2.5, 3.5], 'xlabel': 'frequency [Hz]',
             'ylabel': 'PSD', 'curves': [{'label': 'a', 'y': [1.0, 2.0, 3.0], 'color': 0}],
             'averaging': {'frames': [[0.0, 1.0], [0.5, 1.5]], 'label': 'frames'}}
    [view] = block_views(built)
    assert view['series'][0]['shape'] == 'steps'
    assert np.allclose(view['series'][0]['edges'], [0.5, 1.5, 2.5, 3.5])
    assert [(r['x0'], r['x1'], r['kind']) for r in view['regions']] == [
        (0.0, 1.0, 'frame'), (0.5, 1.5, 'frame')]


@pytest.mark.parametrize('built, states', [
    # a signal to noise: a floor, a missing number the worst
    ({'low': 6.0, 'high': None, 'side': 'under', 'values': [12.0, None, 2.0]},
     [None, 'under', 'under']),
    # a share of a band at its ceiling is in
    ({'low': 5.0, 'high': None, 'inside_at_ceiling': True, 'floor': 0.0,
      'values': [5.0, 6.0, 1.0]}, [None, 'over', None]),
    # a waveform error at its ceiling is out
    ({'low': 20.0, 'high': None, 'floor': 0.0, 'values': [20.0, 3.0, 25.0]},
     ['over', None, 'over']),
    # an RMS error, both ways
    ({'low': -3.0, 'high': 3.0, 'values': [-4.0, 0.0, 3.5]}, ['under', None, 'over'])])
def test_a_bar_chart_is_judged_as_the_page_judged_it(built, states):
    built = {'kind': 'bars', 'label': 'Figure 5', 'labels': ['a', 'b', 'c'],
             'ylabel': 'RMS error [dB]', 'units': ' dB', **built}
    [view] = block_views(built)
    _valid([view])
    assert view['states'] == states
    assert view['value'] == {'label': 'RMS error', 'unit': 'dB'}


def test_a_scene_is_a_mesh_with_its_modes():
    built = {'kind': 'scene', 'label': 'Figure 6',
             'points': [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0]],
             'faces': [{'nodes': [0, 1, 2, 3], 'color': '#000000'}], 'lines': [[0, 2, 'x']],
             'modes': [{'label': 'Mode 1', 'info': {'mode': 1, 'frequency': 12.5,
                                                     'damping': 2.0},
                        'real': [[0, 0, 1]] * 4, 'imag': [[0, 0, 0.5]] * 4}]}
    [view] = block_views(built, length_unit='in')
    _valid([view])
    assert view['unit'] == 'in'
    assert view['triangles'].tolist() == [[0, 1, 2], [0, 2, 3]]
    assert view['lines'].tolist() == [[0, 2]]
    [mode] = view['modes']
    assert mode['frequency_hz'] == 12.5 and mode['damping'] == pytest.approx(0.02)
    assert mode['shape_imag'].shape == (4, 3)


def test_a_map_a_mac_a_verdict_and_a_photo_are_their_views():
    coherence = {'kind': 'map', 'label': 'Figure 7', 'x': [10.0, 20.0],
                 'xlabel': 'frequency [Hz]', 'zlabel': 'coherence', 'labels': ['a', 'b'],
                 'rows': [[1.0, 0.5], [0.2, None]]}
    mac = {'kind': 'mac', 'label': 'Figure 8', 'rows': ['1', '2'], 'columns': ['1', '2'],
           'pairs': [], 'matrix': [[1.0, 0.1], [0.1, 1.0]]}
    verdict = {'kind': 'verdict', 'passed': False, 'test_level_db': 0.0,
               'level_label': 'Test level', 'channels': 8, 'lines_percent': 12.0,
               'rms_percent': 0.0, 'lines_limit': 10.0, 'rms_limit': 0.0}
    png = base64.b64encode(b'\x89PNG\r\n\x1a\n').decode('ascii')
    photo = {'kind': 'image', 'caption': 'Setup', 'src': f'data:image/png;base64,{png}'}
    views = [view for built in (coherence, mac, verdict, photo) for view in block_views(built)]
    _valid(views)
    kinds = [view['view'] for view in views]
    assert kinds == ['matrix', 'matrix', 'reading', 'image']
    assert views[0]['value_range'] == [0.0, 1.0] and np.isnan(views[0]['values'][1, 1])
    assert views[2]['rows'][0] == {'label': 'Result', 'value': 'Fail', 'unit': None,
                                   'status': 'fail'}
    assert bytes(views[3]['bytes']).startswith(b'\x89PNG')


def test_text_and_slots_draw_no_views():
    assert block_views({'kind': 'text', 'html': '<p>x</p>'}) == []
    assert block_views({'kind': 'unbound', 'was': 'plot'}) == []
