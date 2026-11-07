"""Readings on the project: an autospectrum's RMS against its time history,
and a channel's statistics in the time domain, each judged."""

from __future__ import annotations

from pathlib import Path

import pytest

from hertzforge.service import projects
from hertzforge.service.ops_readings import OPERATIONS
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


def test_a_computed_psd_reads_its_records_rms(project, validate):
    project.compute_psds('Time History')
    (reading,) = run(project, 'project.rms_check')
    wire_views([reading], validate)
    found = rows(reading)
    assert found['Gap']['status'] == 'pass' and abs(found['Gap']['value']) < 0.5
    assert found['Spectrum RMS']['unit'] == found['Record RMS']['unit'] == 'm/s²'


def test_a_psd_with_no_history_is_refused_in_the_cores_words(project):
    import numpy as np

    from hertzforge.core.data import Psd

    project.add('Imported', Psd(np.arange(1.0, 6.0), np.ones((1, 5)), response_dof='1Z+'))
    with pytest.raises(ProtocolError) as error:
        run(project, 'project.rms_check', spectrum='Imported')
    assert error.value.code == 'refused'


def test_a_channels_statistics_are_judged_against_their_bands(project, validate):
    (reading,) = run(project, 'project.time_statistics', record=1)
    wire_views([reading], validate)
    found = rows(reading)
    assert found['Kurtosis']['status'] == 'pass', 'a random record is Gaussian'
    assert found['Reading']['value'] == 'Every statistic is inside its band.'
    (force,) = run(project, 'project.time_statistics', record=9)
    assert rows(force)['Crest factor']['status'] == 'warn'


def test_a_record_past_the_objects_is_refused(project):
    with pytest.raises(ProtocolError, match='there is no record 99'):
        run(project, 'project.time_statistics', record=99)


G = 9.80665
RATE = 5120.0


@pytest.fixture
def ranged():
    """Three channels with a table linked giving each a 25 g range: the
    record at minus 6 dB, the record clipped through 25 g, and the record
    at minus 12 dB (the core's worked example, `tests/test_overload.py`)."""
    import numpy as np

    from hertzforge.core.channel_table import ChannelTable
    from hertzforge.core.data import TimeHistory
    from hertzforge.project import Project

    n = round(12.0 * RATE)
    spectrum = np.fft.rfft(np.random.default_rng(1403).standard_normal(n))
    f = np.fft.rfftfreq(n, 1.0 / RATE)
    clean = np.fft.irfft(spectrum * np.where((f >= 20) & (f <= 2000),
                                             np.sqrt(0.04 * RATE / 2), 0.0), n)
    rows = np.vstack([clean / 2, np.clip(clean, -25, 25), clean / 4]) * G
    project = Project()
    for name in ('Run', 'Unlinked'):
        project.add(name, TimeHistory(np.arange(n) / RATE, rows,
                                      response_dof=['1Z+', '2Z+', '3Z+'],
                                      ordinate_dim=['acceleration'] * 3,
                                      ordinate_unit=['g'] * 3))
    project.add('Channels', ChannelTable({
        'channel': [1, 2, 3], 'node': [1, 2, 3], 'direction': ['Z+'] * 3,
        'unit': ['g'] * 3, 'channel_type': ['acceleration'] * 3,
        'sensitivity': [400.0] * 3, 'daq_range': ['10'] * 3,
        'sensor_range': [''] * 3}))
    project.link('Run', 'Channels')
    return project


def overload(project, **given):
    op = OPS['project.overload']
    values = op.resolve({'history': 'Run', **given},
                        objects={name: projects.kind_of(obj) for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return op.run(project, values)


def test_overload_is_offered_only_where_a_linked_table_gives_ranges(ranged):
    assert 'overload_check' in projects.verbs_of(ranged, 'Run')
    assert 'overload_check' not in projects.verbs_of(ranged, 'Unlinked')
    assert 'overload_check' not in projects.verbs_of(ranged, 'Channels')


def test_overload_draws_the_clipped_channel_and_judges_each(ranged, validate):
    trace, headroom, numbers = overload(ranged)
    wire_views([trace, headroom, numbers], validate)
    assert trace['title'].startswith('Run, 2Z+: 287 samples on the rail')
    names = [s['name'] for s in trace['series']]
    assert names == ['2Z+', 'Range', 'Range, below zero']
    assert trace['series'][1]['y'][0] == pytest.approx(25 * G)
    found = rows(headroom)
    assert found['2Z+']['status'] == 'fail'
    assert found['1Z+']['status'] == 'warn'
    assert found['3Z+']['status'] == 'pass'
    assert found['Judgment']['status'] == 'fail'
    assert rows(numbers)['On the rail']['value'] == 287


def test_overload_draws_a_chosen_channel_with_its_next_level(ranged):
    trace, _headroom, numbers = overload(ranged, record=3, step_db=12.0)
    assert [s['role'] for s in trace['series']] == ['measured', 'abort', 'abort',
                                                    'tolerance', 'tolerance']
    assert trace['series'][3]['name'] == 'Largest peak at +12 dB'
    assert rows(numbers)['Channel']['value'] == '3Z+'


def test_overload_without_a_linked_table_is_refused(ranged):
    op = OPS['project.overload']
    with pytest.raises(ProtocolError, match='no linked channel table describes Unlinked'):
        op.run(ranged, {'history': 'Unlinked', 'step_db': 6.0, 'margin': 0.1, 'record': 0})


def test_a_table_with_no_ranges_says_why_once_and_judges_nothing():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'random.nc4')})
    op = OPS['project.overload']
    _trace, headroom, _numbers = op.run(state.project, {
        'history': 'Time History', 'step_db': 6.0, 'margin': 0.1, 'record': 0})
    found = rows(headroom)
    assert found['101Z+']['value'] == 'no range'
    assert 'neither its DAQ Range nor its Sensor Range' in found['No range']['value']
    assert [r['label'] for r in headroom['rows']].count('No range') == 1
    assert found['Judgment']['status'] == 'info'
