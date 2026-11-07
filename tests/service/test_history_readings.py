"""Readings of a time history: the kurtosis of each channel, the amplitude
spectrum, and what resampling one channel does."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

from hertzforge.service import projects
from hertzforge.service.ops_history import OPERATIONS
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
    values = op.resolve({'history': 'Time History', **given},
                        objects={name: projects.kind_of(obj) for name, obj in project.items()})
    return op.run(project, values)


def rows(reading):
    return {row['label']: row for row in reading['rows']}


def test_the_descriptors_validate(validate):
    for op in OPERATIONS:
        validate(op.to_json(), 'operation')


def test_random_channels_read_a_kurtosis_near_3(project, validate):
    (reading,) = run(project, 'project.kurtosis')
    wire_views([reading], validate)
    found = rows(reading)
    assert found['101Z+']['value'] == pytest.approx(3.0, abs=0.3)
    assert found['Outside the band']['status'] == 'pass'


def test_a_narrow_band_flags_the_channels_outside_it(project):
    (reading,) = run(project, 'project.kurtosis', low=2.99, high=3.01)
    assert rows(reading)['Outside the band']['status'] == 'warn'
    with pytest.raises(ProtocolError, match='runs from its low edge up'):
        run(project, 'project.kurtosis', low=4.0, high=2.0)


def test_a_sine_reads_its_amplitude(validate):
    from hertzforge.core.data import TimeHistory
    from hertzforge.project import Project

    rate, n = 1024.0, 8192
    t = np.arange(n) / rate
    project = Project()
    project.add('Time History', TimeHistory(t, [3.0 * np.sin(2 * np.pi * 64.0 * t)],
                                            response_dof=['1Z+'],
                                            ordinate_dim=['acceleration'],
                                            ordinate_unit=['m/s^2']))
    view, reading = run(project, 'project.amplitude_spectrum', window='flattop')
    wire_views([view, reading], validate)
    found = rows(reading)
    assert found['Tallest line']['value'] == pytest.approx(3.0, rel=1e-3)
    assert found['At']['value'] == pytest.approx(64.0)
    assert found['Tallest line']['unit'] == 'm/s²'


def test_an_amplitude_spectrum_draws_every_channel(project):
    view, reading = run(project, 'project.amplitude_spectrum', record=2)
    assert len(view['series']) == project['Time History'].num_records
    assert rows(reading)['Channel']['value'] == view['series'][1]['name']
    with pytest.raises(ProtocolError, match='there is no channel 99'):
        run(project, 'project.amplitude_spectrum', record=99)


def test_resampling_draws_four_spectra_and_grays_the_dropped_lines(project, validate):
    view, reading = run(project, 'project.resample_reading', factor=4)
    wire_views([view, reading], validate)
    assert [s['name'] for s in view['series']] == ['As recorded', 'Decimated bare',
                                                   'Low-passed after', 'Filtered first']
    (cut,) = view['regions']
    rate = project['Time History'].sample_rate / 4
    assert cut['kind'] == 'cut' and cut['x0'] == pytest.approx(rate / 2.56)
    assert rows(reading)['Nyquist frequency']['value'] == pytest.approx(rate / 2)


def test_a_frequency_typed_is_folded_and_one_below_nyquist_is_said(project):
    _view, reading = run(project, 'project.resample_reading', factor=4,
                         frequencies='700, 100')
    labels = [row['label'] for row in reading['rows']]
    assert labels.count('Fold') == 1
    assert any('does not fold' in row['value'] for row in reading['rows']
               if row['label'] == 'Note')
    with pytest.raises(ProtocolError, match='not a list of frequencies'):
        run(project, 'project.resample_reading', frequencies='lots')
