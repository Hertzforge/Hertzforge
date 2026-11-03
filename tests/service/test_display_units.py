"""The display unit system: views computed in SI put into the system the
connection has chosen, as Visual Dynamics' unit selector puts every plot
and reading into its system."""

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
