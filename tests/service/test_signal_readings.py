"""What the signal readings say on the wire: values and their units moved
together into the display system, the record asked for and not its
neighbor, and refusals where a reading cannot be made."""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.averaging import Averaging
from hertzforge.core.data import Psd, Specification, TimeHistory
from hertzforge.service import display_units, projects
from hertzforge.service.protocol import ProtocolError
from hertzforge.service.server import OPERATIONS

OPS = {op.id: op for op in OPERATIONS}
G = 9.80665
LBF = 4.4482216152605
DENSITY = 'acceleration**2/frequency'


def _values(project, op_id, given):
    return OPS[op_id].resolve(given, objects={name: projects.kind_of(obj)
                                              for name, obj in project.items()},
                              verbs={name: projects.verbs_of(project, name) for name in project})


def run(project, op_id, **given):
    return OPS[op_id].run(project, _values(project, op_id, given))


def rows(views):
    return {row['label']: row for view in views if view.get('view') == 'reading'
            for row in view['rows']}


def _force_and_response():
    """A response of about 0.7 g per lbf under a measured force."""
    rng = np.random.default_rng(5)
    rate, n = 2048.0, 2 ** 15
    force = rng.standard_normal(n) * LBF
    response = (np.convolve(force, [0.5, 0.2], mode='same') * G / LBF
                + 0.1 * G * rng.standard_normal(n))
    record = TimeHistory(np.arange(n) / rate, np.vstack([force, response]),
                         response_dof=['1Z+', '2Z+'],
                         ordinate_dim=['force', 'acceleration'],
                         ordinate_unit=['lbf', 'g'])
    record.averaging = Averaging(frame_length=1024, overlap=0.5, window='hann').filled(n, rate)
    project = hertzforge.Project()
    project.add('Run', record)
    return project


def test_the_transmissibility_band_converts_with_its_unit():
    project = _force_and_response()
    frf = project.compute_frfs('Run', 'H1')
    views = run(project, 'project.transmissibility', frf=frf, frequency=200.0)
    si = {label: dict(row) for label, row in rows(views).items()}
    assert si['Band, low']['unit'] == si['H1']['unit'] == '(m/s²)/N'
    display_units.convert(views, display_units.START)
    found = rows(views)
    assert found['H1']['unit'] == found['Band, low']['unit'] == found['Band, high']['unit'] \
        == 'g/lbf'
    assert found['Band, low']['value'] == pytest.approx(si['Band, low']['value'] * LBF / G)
    assert found['Band, low']['value'] < found['H1']['value'] < found['Band, high']['value']


def test_words_with_a_unit_keep_the_unit_they_are_in():
    views = [{'view': 'reading', 'title': 'T', 'rows': [
        {'label': 'Said', 'value': 'not read', 'unit': 'm/s²', 'status': 'info'}]}]
    display_units.convert(views, display_units.START)
    assert views[0]['rows'][0]['unit'] == 'm/s²', \
        'a value left in SI words cannot be labeled in another system'


def _two_quantities_at_one_dof():
    rng = np.random.default_rng(4)
    rate, n = 1024.0, 2 ** 15
    force = rng.standard_normal(n)
    acceleration = 2.0 * force + 0.1 * rng.standard_normal(n)
    velocity = 0.01 * rng.standard_normal(n)
    other = 0.3 * force + rng.standard_normal(n)
    record = TimeHistory(np.arange(n) / rate, np.vstack([force, acceleration, velocity, other]),
                         response_dof=['1Z+', '3Z+', '3Z+', '4Z+'],
                         ordinate_dim=['force', 'acceleration', 'velocity', 'acceleration'])
    record.averaging = Averaging(frame_length=1024, overlap=0.5, window='hann').filled(n, rate)
    project = hertzforge.Project()
    project.add('Run', record)
    return project


@pytest.mark.parametrize(('number', 'label'), [(1, '3Z+ acceleration'), (2, '3Z+ velocity'),
                                               (3, '4Z+')])
def test_partial_coherence_reads_the_channel_asked_for(number, label):
    project = _two_quantities_at_one_dof()
    name = project.compute_multiple_coherence('Run')
    coherence = project[name]
    found = rows(run(project, 'project.partial_coherence', coherence=name, record=number))
    assert found['Response']['value'] == label
    assert found['Multiple, median']['value'] == pytest.approx(
        float(np.median(coherence.ordinate[number - 1])), abs=1e-9)


def test_partial_coherence_refuses_a_channel_no_longer_a_response():
    project = _two_quantities_at_one_dof()
    name = project.compute_multiple_coherence('Run')
    project['Run'].roles = {('3Z+', 'velocity'): 'monitor', ('1Z+', 'force'): 'reference'}
    with pytest.raises(ProtocolError, match=r'3Z\+'):
        run(project, 'project.partial_coherence', coherence=name, record=2)


def _flights():
    rng = np.random.default_rng(2)
    f = np.arange(1.0, 1001.0)
    count = 10
    shape = 1.0 + 20.0 * np.exp(-((f - 300.0) / 60.0) ** 2)
    levels = shape * 10.0 ** (rng.normal(0.0, 3.0, (count, 1)) / 10.0)
    project = hertzforge.Project()
    project.add('Set', Psd(f, levels, response_dof=['1Z+'] * count,
                           ordinate_dim=[DENSITY] * count,
                           block=[str(k) for k in range(count)]))
    return project


@pytest.mark.parametrize('settings', [
    {'statistic': 'mean_k', 'k': 1.5},
    {'statistic': 'mean_k', 'k': 2.0, 'sigma_db': 4.0},
    {'statistic': 'tolerance', 'sigma_db': 2.5},
    {'statistic': 'tolerance', 'max_overshoot_db': 1.0},
    {'statistic': 'tolerance', 'rms': 'conserve'},
    {'statistic': 'envelope', 'max_overshoot_db': 1.0},
], ids=str)
def test_derive_writes_the_envelope_field_limit_shows(settings):
    project = _flights()
    run(project, 'project.derive_specification', source='Set', **settings)
    written = project['Set Derived Specification']
    fit = project.field_limit('Set', **{'max_breakpoints': 12, **settings})
    assert written.abscissa == pytest.approx(fit.frequencies)
    assert np.real(written.ordinate[0]) == pytest.approx(fit.levels)
