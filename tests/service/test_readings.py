"""Readings on the project: an autospectrum's RMS against its time history,
and a channel's statistics in the time domain, each judged."""

from __future__ import annotations

from pathlib import Path

import pytest

from hertzforge.service import projects
from hertzforge.service.ops_readings import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .test_ops import wire_views

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
