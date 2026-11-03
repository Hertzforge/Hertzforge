"""Tools that generate, convert and predict, on the project: Generate
Signal, SRS Synthesis, Compute Modal Responses, Convert to PSD at a
spacing, Sweep Rate, Fatigue, Drive Prediction, Notch Prediction,
Conditioning and Realizability."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

import hertzforge
from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.data import Frf, ShockSpecification, Specification, TimeHistory
from hertzforge.core.shapes import ShapeSet
from hertzforge.core.sine import LOG, SineSweepSpecification, SineTone
from hertzforge.service import projects
from hertzforge.service.ops_generate import OPERATIONS
from hertzforge.service.ops_project import OPERATIONS as PROJECT_OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .test_ops import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
OPS = {op.id: op for op in (*OPERATIONS, *PROJECT_OPERATIONS)}
G = 9.80665


def opened(name):
    state, _ = projects.import_file(None, {'path': str(PLATE / name)})
    return state.project


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()},
                        verbs={name: projects.verbs_of(project, name) for name in project})
    return op.run(project, values)


def rows(reading):
    return {row['label']: row for row in reading['rows']}


def test_the_descriptors_validate(validate):
    for op in OPERATIONS:
        validate(op.to_json(), 'operation')
    validate(OPS['project.convert_to_psd'].to_json(), 'operation')


# ---- Generate Signal --------------------------------------------------------------

def test_random_from_a_specification_is_added_at_its_level(validate):
    project = opened('random.nc4')
    spec = project['Specification']
    views = run(project, 'project.generate_signal', source='Specification', duration=4.0,
                seed=7)
    wire_views(views, validate)
    plot, reading = views
    found = rows(reading)
    added = project[found['Added']['value']]
    assert isinstance(added, TimeHistory) and added.num_records == spec.num_records
    from hertzforge.core.signals import suggested

    assert added.sample_rate == suggested(spec).sample_rate
    f = np.asarray(spec.abscissa, dtype=float)
    asked = np.sqrt(np.sum(np.real(spec.ordinate[0])) * (f[1] - f[0]))
    assert found['RMS, 101Z+']['value'] == pytest.approx(asked, rel=0.05)
    assert found['Seed']['value'] == 7.0
    assert {'reference', 'measured'} == {line['role'] for line in plot['series']}


def test_an_impulse_from_a_channel_table_and_a_kind_it_does_not_make():
    project = hertzforge.Project()
    project.add('Table', ChannelTable({
        'channel': [1, 2], 'node': ['1', '101'], 'direction': ['X+', 'Z+'],
        'unit': ['N', 'g'], 'role': ['reference', 'response'],
        'channel_type': ['force', 'acceleration']}))
    trace, spectrum, reading = run(project, 'project.generate_signal', source='Table',
                                   kind='impulse', sample_rate=8192.0, duration=0.5,
                                   peak=2.0, width=0.002)
    assert max(np.max(line['y']) for line in trace['series']) == pytest.approx(2.0, rel=1e-3)
    assert rows(reading)['−3 dB at']['value'] == pytest.approx(0.59 / 0.002, rel=0.01)
    assert spectrum['series'][0]['name'] == 'Impulse spectrum'
    with pytest.raises(ProtocolError, match='Sine is not made from this object'):
        run(project, 'project.generate_signal', source='Table', kind='sine')


# ---- SRS Synthesis ----------------------------------------------------------------

def _shock_project():
    project = hertzforge.Project()
    project.add('Requirement', ShockSpecification.from_breakpoints(
        [50.0, 400.0, 2000.0], [20.0, 100.0, 100.0], '1Z+', ordinate_unit='g',
        per_octave=6, warning=(-1.5, 3.0), abort=(-3.0, 6.0)))
    return project


def test_a_synthesis_meets_its_requirement_and_is_added(validate):
    from hertzforge.core.shock_synthesis import synthesize

    project = _shock_project()
    views = run(project, 'project.srs_synthesis', requirement='Requirement', stroke=0.0508,
                velocity=1.78)
    wire_views(views, validate)
    found = rows(views[-1])
    added = project[found['Added']['value']]
    made = synthesize(project['Requirement'], sample_rate=found['Sample rate']['value'])
    np.testing.assert_allclose(added.ordinate, made.history.ordinate)
    assert found['Inside the band']['status'] == 'pass'
    assert found['Half cycles kept']['value'] == 5.0
    assert found['Verdict']['value'] == 'fits'


def test_a_synthesis_too_slow_for_its_points_is_refused():
    project = _shock_project()
    with pytest.raises(ProtocolError, match='sample rate'):
        run(project, 'project.srs_synthesis', requirement='Requirement', sample_rate=4096.0)
    assert list(project) == ['Requirement']


# ---- Compute Modal Responses ------------------------------------------------------

def _modal_project():
    from scipy.linalg import eigh

    lam, phi = eigh(np.array([[3e5, -1e5], [-1e5, 1e5]]), np.diag([2.0, 1.0]))
    shapes = ShapeSet(np.sqrt(lam) / (2 * np.pi), np.array([0.02, 0.03]), ['1X+', '2X+'],
                      phi.T, mass_unit='kg')
    t = np.arange(8192) / 4096.0
    force = np.random.default_rng(1).standard_normal(t.size) * 10.0
    record = TimeHistory(t, np.vstack([force, np.zeros_like(t)]), response_dof=['1X+', '2X+'],
                         ordinate_dim=['force', 'acceleration'])
    project = hertzforge.Project()
    project.add('Forces', record)
    project.add('Modes', shapes)
    return project


def test_modal_responses_are_added_as_the_core_computes_them(validate):
    from hertzforge.core.modal_response import modal_responses

    project = _modal_project()
    views = run(project, 'project.compute_modal_responses', source='Forces', shapes='Modes',
                response='velocity', modes='2')
    wire_views(views, validate)
    found = rows(views[-1])
    added = project[found['Added']['value']]
    expected = modal_responses(project['Forces'], project['Modes'], quantity='velocity',
                               modes=[1])
    np.testing.assert_allclose(added.ordinate, expected.ordinate)
    assert found['Modes']['value'] == '1 of 2'
    assert found['Force channels']['value'] == 1.0
    with pytest.raises(ProtocolError, match='modes run from 1 to 2'):
        run(project, 'project.compute_modal_responses', source='Forces', shapes='Modes',
            modes='3')


# ---- Convert to PSD ---------------------------------------------------------------

def _breakpoints():
    a = np.array([1e-3, 4e-3, 4e-3])
    return Specification(np.array([20.0, 100.0, 1000.0]), np.array([a]),
                         response_dof=['1X+'], ordinate_dim=['force**2/frequency'])


def test_a_breakpoint_specification_converts_at_the_spacing_given(validate):
    project = hertzforge.Project()
    project.add('Spec', _breakpoints())
    views = run(project, 'project.convert_to_psd', source='Spec', spacing=5.0)
    wire_views(views, validate)
    found = rows(views[-1])
    lines = np.asarray(project['Spec'].abscissa, dtype=float)
    assert found['Breakpoints']['value'] == 3.0
    assert found['Lines']['value'] == float(lines.size)
    np.testing.assert_allclose(lines % 5.0, 0.0)
    assert lines[0] == 20.0 and lines[-1] == 1000.0


def test_no_spacing_is_guessed_and_one_on_lines_takes_none():
    project = hertzforge.Project()
    project.add('Spec', _breakpoints())
    with pytest.raises(ProtocolError, match='give the frequency spacing'):
        run(project, 'project.convert_to_psd', source='Spec')
    on_lines = opened('random.nc4')
    with pytest.raises(ProtocolError, match='already on lines'):
        run(on_lines, 'project.convert_to_psd', source='Specification', spacing=2.0)


# ---- Sweep Rate -------------------------------------------------------------------

def _sweep_project():
    tone = SineTone('Up', 0.0, [20.0, 200.0], [[G], [2 * G]], [LOG], [8.0])
    project = hertzforge.Project()
    project.add('Sweep', SineSweepSpecification([tone], ['1X+'], ordinate_unit='m/s**2'))
    return project


def test_a_sweep_is_checked_against_the_modes_typed(validate):
    from hertzforge.core.sweep_rate import check_sweep_rate

    project = _sweep_project()
    views = run(project, 'project.sweep_rate', specification='Sweep', modes='50 40, 120 5')
    wire_views(views, validate)
    plot, table, reading = views
    checks = check_sweep_rate(project['Sweep'].tones[0], [(50.0, 40.0), (120.0, 5.0)])
    found = rows(reading)
    assert found['Passes']['value'] == float(len(checks)) == 2.0
    assert found['Too fast']['value'] == float(sum(c.too_fast for c in checks))
    assert [row[6] for row in table['rows']] == [f'{c.ratio:.2f}' for c in checks]
    assert sum(line['role'] == 'abort' and 'rule' in line['name']
               for line in plot['series']) == found['Too fast']['value']
    with pytest.raises(ProtocolError, match='is not a frequency and a Q'):
        run(project, 'project.sweep_rate', specification='Sweep', modes='50')


# ---- Fatigue ----------------------------------------------------------------------

def test_the_ers_and_fds_are_the_cores_and_a_test_is_set_against_it(validate):
    from hertzforge.core.fatigue import fatigue_spectra

    project = opened('random_spectra.nc4')
    ers, fds, rule, reading = run(project, 'project.fatigue', source='Specification',
                                  duration=60.0)
    wire_views([ers, fds, rule, reading], validate)
    field = fatigue_spectra(project['Specification'], 0, b=7.5, q=10.0, duration=60.0)
    np.testing.assert_allclose(ers['series'][0]['y'], field.ers[field.ers > 0])
    views = run(project, 'project.fatigue', source='Specification', duration=60.0,
                test='PSD', test_record=1, test_duration=30.0)
    found = rows(views[-1])
    assert 'Least damage margin' in found and 'Factor that covers' in found
    assert len(views[0]['series']) == 2
    with pytest.raises(ProtocolError, match='density of force'):
        run(project, 'project.fatigue', source='PSD (2)', duration=60.0)


# ---- Drive Prediction -------------------------------------------------------------

def test_drives_are_predicted_through_the_frf(validate):
    from hertzforge.core.drive_prediction import predict_drives

    project = opened('random_spectra.nc4')
    project.convert_to_specification('PSD')
    views = run(project, 'project.drive_prediction', frf='FRF', specification='PSD',
                drive_limit=5.0)
    wire_views(views, validate)
    drives, middle, reading = views
    prediction = predict_drives(project['FRF'], project['PSD'], drive_limit=5.0)
    assert len(drives['series']) == len(prediction.drives) == 4
    f = np.asarray(prediction.frequency)
    asd = prediction.drive_asd(0)
    keep = np.isfinite(asd) & (asd > 0) & (f > 0)
    np.testing.assert_allclose(drives['series'][0]['y'], asd[keep])
    assert [line['name'] for line in middle['series']] == ['101Z+ predicted', 'target']
    assert rows(reading)['Verdict']['status'] == ('pass' if prediction.fits else 'fail')
    with pytest.raises(ProtocolError, match='no cross term'):
        run(project, 'project.drive_prediction', frf='FRF', specification='Specification')


# ---- Notch Prediction -------------------------------------------------------------

F = np.arange(0.0, 2001.0, 1.0)


def _notch_project(profile):
    gain = np.ones(F.size, dtype=complex)
    gain[(F >= 240.0) & (F <= 260.0)] = 3.0
    project = hertzforge.Project()
    project.add('T', Frf(F, gain[None, :], response_dof=['2Z+'], reference_dof=['1Z+'],
                         ordinate_dim='acceleration/acceleration'))
    level = 0.04 * G ** 2
    values = np.array([[level, level]] + ([[profile * G ** 2] * 2] if profile else []))
    nan = np.full(2, np.nan)
    project.add('Spec', Specification(
        np.array([20.0, 2000.0]), values, response_dof=['1Z+', '2Z+'][:len(values)],
        ordinate_dim='acceleration**2/frequency',
        abort_lower=np.array([values[0] * 0.5] + [nan] * (len(values) - 1)),
        abort_upper=np.array([values[0] * 2.0] + [nan] * (len(values) - 1))))
    return project


def test_a_channel_twice_over_its_profile_cuts_a_notch_three_db_deep(validate):
    project = _notch_project(0.18)
    views = run(project, 'project.notch_prediction', frf='T', specification='Spec',
                profiles='Spec')
    wire_views(views, validate)
    control, channel, table, reading = views
    assert table['rows'][0][1].startswith('3.0 ')
    assert len(control['regions']) == 1
    assert [line['role'] for line in channel['series']] == ['measured', 'limit']
    assert rows(reading)['Verdict']['status'] == 'pass'
    with pytest.raises(ProtocolError, match='limit profile'):
        run(_notch_project(None), 'project.notch_prediction', frf='T', specification='Spec',
            profiles='Spec')


# ---- Conditioning and Realizability -----------------------------------------------

def test_conditioning_is_read_at_the_frequency_given(validate):
    project = opened('random_spectra.nc4')
    views = run(project, 'project.conditioning', data='FRF', frequency=100.0)
    wire_views(views, validate)
    point = project['FRF'].conditioning().at(100.0)
    found = rows(views[-1])
    assert found['At']['value'] == point.frequency == 100.0
    assert found['Condition number']['value'].startswith(f'{point.condition:.4g} (within')
    assert found['Verdict']['status'] == 'pass'
    with pytest.raises(ProtocolError, match='the matrix is incomplete'):
        run(project, 'project.conditioning', data='Specification')


def test_realizability_reads_a_cpsd(validate):
    project = opened('random_spectra.nc4')
    views = run(project, 'project.realizability', data='PSD')
    wire_views(views, validate)
    found = rows(views[-1])
    assert found['Verdict']['status'] == 'pass'
    assert found['Channels']['value'] == 8.0
    assert found['This line']['value'] == 'realizable'
    with pytest.raises(ProtocolError, match='the matrix is incomplete'):
        run(project, 'project.realizability', data='Specification')


def test_an_optional_object_param_takes_none_or_a_tracked_level(validate):
    """Sweep Rate's tracked level is optional: '' runs without one, and one
    named reads a Q off it at each mode on each of its channels."""
    from .test_sine_shock import _sweep

    op = OPS['project.sweep_rate']
    levels = next(p for p in op.params if p.name == 'levels')
    assert levels.optional and levels.to_json()['default'] == ''
    project = _sweep()
    (added,) = project.extract_sine('Sweep')
    kinds = {name: projects.kind_of(obj) for name, obj in project.items()}
    alone = op.resolve({'specification': 'Profile', 'modes': '300 20'}, objects=kinds)
    assert alone['levels'] == ''
    *_views, reading = op.run(project, alone)
    assert not any('Hz' in row['label'] and ',' in row['label'] and 'Z+' in row['label']
                   for row in reading['rows'])
    named = op.resolve({'specification': 'Profile', 'modes': '300 20', 'levels': added},
                       objects=kinds)
    *_views, reading = op.run(project, named)
    assert any(row['label'] == '101Z+, 300 Hz' for row in reading['rows'])
    with pytest.raises(ProtocolError, match='not one of the choices'):
        op.resolve({'specification': 'Profile', 'levels': 'Nothing'}, objects=kinds)
