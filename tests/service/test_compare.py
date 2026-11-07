"""Comparisons of a pair or group of project objects: a PSD against its
specification, a specification's levels, two densities, two signatures, a
cross-spectral matrix, shock spectra and a transient run."""

from __future__ import annotations

import warnings
from pathlib import Path

import numpy as np
import pytest

from hertzforge.service import projects
from hertzforge.service.ops_compare import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in OPERATIONS}


def imported(name):
    state, _ = projects.import_file(None, {'path': str(PLATE / name)})
    return state.project


@pytest.fixture
def project():
    found = imported('random.nc4')
    found.compute_psds('Time History')
    return found


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()})
    return op.run(project, values)


def rows(reading):
    return {row['label']: row for row in reading['rows']}


def test_the_descriptors_validate(validate):
    for op in OPERATIONS:
        validate(op.to_json(), 'operation')
        assert op.id.startswith('project.')


# ---- a PSD against its specification -------------------------------------------

def test_spectra_draw_the_channel_over_its_specification_and_limits(project, validate):
    plot, reading = run(project, 'project.spec_spectra')
    wire_views([plot, reading], validate)
    assert [s['role'] for s in plot['series']] == ['measured', 'reference', 'tolerance',
                                                   'tolerance', 'abort', 'abort']
    found = rows(reading)
    assert found['Measured RMS']['value'] == pytest.approx(2.385)
    assert found['Specification RMS']['value'] == pytest.approx(1.422)
    error = found['RMS error (±3 dB)']
    assert error['value'] == pytest.approx(4.49) and error['status'] == 'fail'
    lines = found['Lines outside abort (at most 10 %)']
    assert lines['value'] == pytest.approx(18.73) and lines['status'] == 'fail'
    assert found['Run (8 channels)']['value'] == 'fails'
    with pytest.raises(ProtocolError, match='there is no record 99'):
        run(project, 'project.spec_spectra', record=99)


def test_rms_error_and_lines_out_judge_every_channel(project, validate):
    (error,) = run(project, 'project.spec_rms_error')
    (lines,) = run(project, 'project.spec_lines_out')
    wire_views([error, lines], validate)
    found = rows(error)
    assert found['101Z+']['value'] == pytest.approx(4.49) and found['101Z+']['status'] == 'fail'
    assert found['104Z+']['value'] == pytest.approx(-2.59) and found['104Z+']['status'] == 'pass'
    assert found['Channels out']['value'].startswith('1 of 8 channels outside -3 to 3 dB')
    assert rows(lines)['Channels out']['value'].startswith('2 of 8 channels over 10%')
    (narrow,) = run(project, 'project.spec_rms_error', low=-2.0, high=2.0)
    assert rows(narrow)['Channels out']['value'].startswith('2 of 8')
    with pytest.raises(ProtocolError, match='runs from its low edge up'):
        run(project, 'project.spec_rms_error', low=0.0, high=0.0)


def test_chance_counts_the_comparisons_own_lines(project, validate):
    from hertzforge.core.compliance import compare

    plot, reading = run(project, 'project.spec_chance')
    wire_views([plot, reading], validate)
    abort = compare(project['Specification'], project['Time History PSDs'], 0, 0,
                    scale_db=0)['abort_lines']
    assert rows(reading)['Abort band']['value'].startswith(f'{abort} of 726 lines outside')
    (combined, _r) = run(project, 'project.spec_chance', combine=4)
    assert combined['series'][1]['name'] == 'Each line'
    psd = project['Time History PSDs']
    project.add('Imported', type(psd)(psd.abscissa, psd.ordinate, response_dof=psd.response_dof,
                                      ordinate_dim=psd.ordinate_dim))
    with pytest.raises(ProtocolError) as error:
        run(project, 'project.spec_chance', spectrum='Imported')
    assert error.value.code == 'refused'
    assert len(rows(run(project, 'project.spec_chance', spectrum='Imported', dof=120.0)[1]))


def test_the_composite_is_the_mean_of_its_channels(project, validate):
    plot, reading = run(project, 'project.spec_composite')
    wire_views([plot, reading], validate)
    psd = project['Time History PSDs']
    channels = [s for s in plot['series'] if s['role'] == 'measured']
    composite = channels[-1]
    assert composite['name'].startswith('Composite, average of')
    count = int(composite['name'].rsplit(' ', 1)[1])
    lines = np.asarray(psd.abscissa) > 0
    mean = np.mean(np.real(psd.ordinate[:count]), axis=0)[lines]
    assert np.allclose(composite['y'], mean, rtol=1e-9)
    assert rows(reading)['Composite']['value'].startswith(f'average of {count}:')
    with pytest.raises(ProtocolError, match='weights'):
        run(project, 'project.spec_composite', strategy='weighted', weights='a, b')


def test_scaling_is_held_on_the_measurement_and_read_by_every_comparison(project, validate):
    (reading,) = run(project, 'project.comparison_scale', scale='+6 dB')
    wire_views([reading], validate)
    assert rows(reading)['Comparison scale']['value'] == 6.0
    assert project['Time History PSDs'].scale_db == 6
    _plot, spectra = run(project, 'project.spec_spectra')
    found = rows(spectra)
    assert found['Measured RMS']['value'] == pytest.approx(2.385 * 10 ** (6 / 20), rel=1e-3)
    assert found['Comparison scale (held on the measurement)']['value'] == 6.0
    (back,) = run(project, 'project.comparison_scale')
    assert project['Time History PSDs'].scale_db is None
    assert rows(back)['Scaled']['value'] == 'detected from the data'
    with pytest.raises(ProtocolError, match='whole decibels'):
        run(project, 'project.comparison_scale', scale='lots')


def test_a_specification_alone_reads_its_levels(project, validate):
    from hertzforge.core.compliance import specification_rms

    (reading,) = run(project, 'project.spec_rms')
    wire_views([reading], validate)
    level = specification_rms(project['Specification'], 0)
    assert rows(reading)['101Z+']['value'] == pytest.approx(level, rel=1e-3)
    assert rows(reading)['101Z+']['unit'] == 'm/s²'
    assert rows(reading)['Channels']['value'].startswith('8 channels')
    project.add('Empty', project['Time History PSDs'])
    with pytest.raises(ProtocolError):
        run(project, 'project.spec_rms', specification='Empty')


# ---- two densities ---------------------------------------------------------------

@pytest.fixture
def pair(project):
    psd = project['Time History PSDs']
    noise = type(psd)(psd.abscissa, psd.ordinate * 0.01, response_dof=psd.response_dof,
                      ordinate_dim=psd.ordinate_dim)
    project.add('Noise', noise)
    return project


def test_signal_to_noise_is_the_louder_less_the_quieter_over_the_quieter(pair, validate):
    plot, reading = run(pair, 'project.signal_to_noise', first='Noise',
                        second='Time History PSDs')
    wire_views([plot, reading], validate)
    expected = 10 * np.log10(99.0)
    assert rows(reading)['Median']['value'] == pytest.approx(expected, abs=0.01)
    assert rows(reading)['Signal']['value'] == 'Time History PSDs'
    assert np.allclose(plot['series'][0]['y'], expected)
    (bars,) = run(pair, 'project.rms_signal_to_noise', first='Noise',
                  second='Time History PSDs')
    wire_views([bars], validate)
    assert rows(bars)['104Z+']['value'] == pytest.approx(expected, abs=0.01)
    assert rows(bars)['Channels under the floor']['status'] == 'pass'
    (high,) = run(pair, 'project.rms_signal_to_noise', first='Noise',
                  second='Time History PSDs', floor=30.0)
    assert rows(high)['104Z+']['status'] == 'fail'
    with pytest.raises(ProtocolError, match='against itself'):
        run(pair, 'project.signal_to_noise', first='Noise', second='Noise')


def test_an_overlay_draws_both_on_every_shared_channel(pair, validate):
    (plot,) = run(pair, 'project.psd_overlay', first='Noise', second='Time History PSDs')
    wire_views([plot], validate)
    roles = [s['role'] for s in plot['series']]
    assert roles.count('measured') == roles.count('reference') == 12
    assert plot['series'][0]['name'].startswith('Time History PSDs')


# ---- two signatures --------------------------------------------------------------

def _sdof(frequency, height=1.0):
    from hertzforge.core.data import Frf

    f = np.linspace(1.0, 400.0, 4000)
    zeta = 0.02
    h = height / (frequency ** 2 - f ** 2 + 2j * zeta * frequency * f)
    return Frf(f, [h * frequency ** 2], response_dof='1Z+', reference_dof='1Z+',
               ordinate_dim='acceleration/force')


def test_a_signature_shift_is_read_mode_by_mode(validate):
    from hertzforge.project import Project

    project = Project()
    project.add('Before', _sdof(100.0))
    project.add('After', _sdof(97.0))
    plot, reading = run(project, 'project.signature', before='Before', after='After')
    wire_views([plot, reading], validate)
    found = rows(reading)
    shift = next(row for label, row in found.items() if label.endswith('frequency shift'))
    assert shift['value'] == pytest.approx(-3.0, abs=0.1)
    assert found['Verdict']['status'] == 'pass'
    assert sum(s['role'] == 'tolerance' for s in plot['series']) == 2
    (_plot, tight) = run(project, 'project.signature', before='Before', after='After',
                         frequency_limit=2.0)
    assert rows(tight)['Verdict']['status'] == 'fail'
    named = rows(run(project, 'project.signature', before='Before', after='After',
                     modes='100, (300)')[1])
    assert named['Modes']['value'] == 'named'
    with pytest.raises(ProtocolError, match='is not a frequency'):
        run(project, 'project.signature', before='Before', after='After',
            modes='first bending')
    with pytest.raises(ProtocolError, match='against itself'):
        run(project, 'project.signature')


# ---- a cross-spectral matrix -----------------------------------------------------

def test_a_matrix_is_judged_level_by_level_on_its_diagonal(project, validate):
    from hertzforge.core.compliance import compare_all

    project.compute_cpsds('Time History')
    cpsd = next(name for name, obj in project.items()
                if projects.kind_of(obj) == 'psd' and obj.reference_dof is not None
                and not obj.is_autospectrum())
    grid, reading = run(project, 'project.matrix_comparison', cpsd=cpsd)
    wire_views([grid, reading], validate)
    levels = dict(compare_all(project['Specification'], project[cpsd], scale_db=0))
    assert grid['rows']['ticks'][0] == '101Z+'
    assert grid['values'][0, 0] == pytest.approx(levels['101Z+']['abort_percent'], rel=1e-5)
    assert np.isnan(grid['values'][0, 1]), 'a cross term not specified is not judged'
    assert rows(reading)['Verdict']['status'] == 'fail'
    with pytest.raises(ProtocolError, match='cross-spectral matrix'):
        run(project, 'project.matrix_comparison', cpsd='Time History PSDs')


# ---- shock spectra ---------------------------------------------------------------

@pytest.fixture
def shock():
    from hertzforge.core.data import ShockSpecification

    found = imported('shock.nc4')
    with warnings.catch_warnings():
        warnings.simplefilter('ignore')
        name = found.compute_srs('Time History')
    srs = found[name]
    first = np.real(srs.ordinate[0])
    found.add('Requirement', ShockSpecification(
        srs.abscissa, [first], response_dof=srs.response_dof[0],
        ordinate_dim=srs.ordinate_dim[0], q=srs.q,
        warning_lower=[first * 10 ** (-1.5 / 20)], warning_upper=[first * 10 ** (3 / 20)],
        abort_lower=[first * 10 ** (-3 / 20)], abort_upper=[first * 10 ** (6 / 20)]))
    return found


def test_shock_spectra_draw_every_event_against_the_requirement(shock, validate):
    plot, reading = run(shock, 'project.srs_spectra')
    wire_views([plot, reading], validate)
    srs = next(obj for obj in shock.values() if projects.kind_of(obj) == 'srs')
    dof = srs.response_dof[0]
    events = sum(str(d) == dof for d in srs.response_dof)
    assert sum(s['role'] == 'measured' for s in plot['series']) == events
    first = reading['rows'][0]
    assert first['value'] == pytest.approx(0.0, abs=1e-9), 'the requirement is this event'
    with pytest.raises(ProtocolError, match='no record 9'):
        run(shock, 'project.srs_spectra', record=9)


def test_srs_error_scores_each_channel_and_event(shock, validate):
    (reading,) = run(shock, 'project.srs_error')
    wire_views([reading], validate)
    first = reading['rows'][0]
    assert first['value'] == pytest.approx(0.0, abs=1e-9) and first['status'] == 'pass'
    assert reading['rows'][-1]['label'] == 'Channels out'
    with pytest.raises(ProtocolError, match='low edge up'):
        run(shock, 'project.srs_error', low=0.0, high=0.0)


def test_the_points_rule_reads_the_requirements_limits(shock, validate):
    plot, reading = run(shock, 'project.srs_points')
    wire_views([plot, reading], validate)
    found = rows(reading)
    assert found['Verdict']['value'] == 'passes'
    assert found['Share inside']['value'] == 100.0
    assert found['Band']['value'].endswith("(the requirement's limits)")
    assert [s['role'] for s in plot['series']].count('abort') == 2
    (_p, typed) = run(shock, 'project.srs_points', limits=False, inner_low=-0.5,
                      inner_high=0.5)
    assert rows(typed)['Verdict']['value'] == 'passes'
    with pytest.raises(ProtocolError, match='no record 999'):
        run(shock, 'project.srs_points', record=999)


# ---- a transient run against its reference --------------------------------------

@pytest.fixture
def transient():
    from hertzforge.core.data import TimeHistory, TransientSpecification
    from hertzforge.project import Project

    rate, n = 1024.0, 1024
    t = np.arange(n) / rate
    burst = np.sin(2 * np.pi * 40.0 * t) * np.exp(-((t - 0.5) / 0.15) ** 2)
    project = Project()
    project.add('Reference', TransientSpecification(
        t, [burst], response_dof=['1Z+'], ordinate_dim=['acceleration']))
    played = np.tile(1.1 * burst, 3)
    project.add('Run', TimeHistory(np.arange(3 * n) / rate, [played], response_dof=['1Z+'],
                                   ordinate_dim=['acceleration']))
    return project


def test_waveforms_overlay_one_playing_with_its_error(transient, validate):
    plot, reading = run(transient, 'project.replication_waveforms', playing=2)
    wire_views([plot, reading], validate)
    found = rows(reading)
    assert found['Waveform error']['value'] == pytest.approx(10.0, abs=0.01)
    assert found['Playing']['value'] == '2 of 3'
    assert [s['role'] for s in plot['series']] == ['measured', 'reference']
    with pytest.raises(ProtocolError, match='no whole playing 4'):
        run(transient, 'project.replication_waveforms', playing=4)


def test_waveform_error_scores_every_playing(transient, validate):
    (reading,) = run(transient, 'project.waveform_error')
    wire_views([reading], validate)
    found = rows(reading)
    assert [found[f'1Z+ e{k}']['value'] for k in (1, 2, 3)] == pytest.approx([10.0] * 3,
                                                                             abs=0.01)
    assert found['Channels out']['status'] == 'pass'
    (tight,) = run(transient, 'project.waveform_error', ceiling=5.0)
    assert rows(tight)['Channels out']['value'].startswith('3 of 3')


def test_segments_are_judged_each_by_its_kind(transient, validate):
    plot, reading = run(transient, 'project.replication_segments',
                        segments='0.2 0.8 transient burst')
    wire_views([plot, reading], validate)
    found = rows(reading)
    assert found['Whole-record RMS error']['value'] == pytest.approx(10.0, abs=0.01)
    assert found['burst']['status'] == 'pass'
    (_p, tight) = run(transient, 'project.replication_segments',
                      segments='0.2 0.8 transient burst', envelope_db=0.5)
    assert rows(tight)['burst']['status'] == 'fail'
    with pytest.raises(ProtocolError, match='no segment is marked'):
        run(transient, 'project.replication_segments')
    with pytest.raises(ProtocolError, match='not a kind of segment'):
        run(transient, 'project.replication_segments', segments='0.2 0.8 wobble')
